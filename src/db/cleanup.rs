use crate::{Conn, models::StatusKind};
use chrono::Utc;
use diesel::prelude::*;
use diesel_async::RunQueryDsl;

use super::{DbError, run_in_transaction};

/// Delete terminal tasks (Success/Failure/Canceled) with `ended_at` older than the
/// retention period. Deletes in FK order (actions → links → tasks) within a transaction.
/// Returns the number of tasks deleted.
pub(crate) async fn cleanup_old_terminal_tasks<'a>(
    conn: &mut Conn<'a>,
    retention_days: u32,
    batch_size: i64,
) -> Result<usize, DbError> {
    use crate::schema::task::dsl as task_dsl;

    let cutoff = Utc::now() - chrono::Duration::days(retention_days as i64);

    // Find task IDs eligible for cleanup
    let task_ids: Vec<uuid::Uuid> = task_dsl::task
        .filter(
            task_dsl::status
                .eq(StatusKind::Success)
                .or(task_dsl::status.eq(StatusKind::Failure))
                .or(task_dsl::status.eq(StatusKind::Canceled)),
        )
        .filter(task_dsl::ended_at.le(cutoff))
        .select(task_dsl::id)
        .limit(batch_size)
        .load::<uuid::Uuid>(conn)
        .await?;

    if task_ids.is_empty() {
        return Ok(0);
    }

    let count = task_ids.len();

    run_in_transaction(conn, |conn| {
        Box::pin(async move {
            use crate::schema::action::dsl as action_dsl;
            use crate::schema::link::dsl as link_dsl;

            // 1. Delete actions belonging to these tasks
            diesel::delete(action_dsl::action.filter(action_dsl::task_id.eq_any(&task_ids)))
                .execute(&mut *conn)
                .await?;

            // 2. Delete links referencing these tasks (as parent or child)
            diesel::delete(
                link_dsl::link.filter(
                    link_dsl::parent_id
                        .eq_any(&task_ids)
                        .or(link_dsl::child_id.eq_any(&task_ids)),
                ),
            )
            .execute(&mut *conn)
            .await?;

            // 3. Delete the tasks themselves
            diesel::delete(task_dsl::task.filter(task_dsl::id.eq_any(&task_ids)))
                .execute(&mut *conn)
                .await?;

            Ok(count)
        })
    })
    .await
}
