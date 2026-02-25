use crate::{
    Conn, dtos,
    models::{self, Link, Task},
};
use diesel::prelude::*;
use diesel::sql_types;
use diesel_async::RunQueryDsl;
use uuid::Uuid;

use super::DbError;

/// Find all Running tasks that have exceeded their timeout (based on `last_updated`).
/// Returns matching task IDs without modifying them — callers should use
/// `timeout_task_and_propagate` to atomically mark each as failed and propagate.
pub(crate) async fn find_timed_out_tasks<'a>(
    conn: &mut Conn<'a>,
) -> Result<Vec<uuid::Uuid>, DbError> {
    use {
        crate::schema::task::dsl::*,
        diesel::{dsl::now, pg::data_types::PgInterval},
    };
    let ids = task
        .filter(
            status
                .eq(models::StatusKind::Running)
                .and(started_at.is_not_null())
                .and(last_updated.lt(now.into_sql::<sql_types::Timestamptz>()
                    - (PgInterval::from_microseconds(1_000_000).into_sql::<sql_types::Interval>()
                        * timeout))),
        )
        .select(id)
        .get_results::<uuid::Uuid>(conn)
        .await?;
    Ok(ids)
}

/// Atomically mark a single task as timed-out (Failed) and propagate to children,
/// all within one transaction. Returns the failed task and cascade-failed child IDs.
/// Uses FOR UPDATE SKIP LOCKED to avoid conflicts with concurrent workers.
pub(crate) async fn timeout_task_and_propagate<'a>(
    conn: &mut Conn<'a>,
    task_id: uuid::Uuid,
) -> Result<Option<(Task, Vec<uuid::Uuid>)>, DbError> {
    use super::run_in_transaction;
    use {crate::schema::task::dsl::*, diesel::dsl::now};
    const TIMEOUT_REASON: &str = "Timeout";

    run_in_transaction(conn, |conn| {
        Box::pin(async move {
            // Lock the task; SKIP LOCKED so concurrent timeout workers don't block
            let t: Option<Task> = task
                .filter(id.eq(task_id).and(status.eq(models::StatusKind::Running)))
                .for_update()
                .skip_locked()
                .first::<Task>(conn)
                .await
                .optional()?;

            let Some(t) = t else {
                // Already transitioned (e.g. completed or cancelled concurrently)
                return Ok(None);
            };

            // Mark as failed (last_updated intentionally NOT updated —
            // it preserves when the task last showed activity, useful for diagnostics)
            diesel::update(task.filter(id.eq(task_id)))
                .set((
                    status.eq(models::StatusKind::Failure),
                    ended_at.eq(now),
                    failure_reason.eq(TIMEOUT_REASON),
                ))
                .execute(conn)
                .await?;

            // Propagate failure to children (inside tx)
            let cascade_failed =
                crate::workers::propagate_to_children(&task_id, &models::StatusKind::Failure, conn)
                    .await?;

            Ok(Some((t, cascade_failed)))
        })
    })
    .await
}

/// Requeue Claimed tasks that never started within the claim timeout.
/// Returns the tasks moved back to Pending.
pub(crate) async fn requeue_stale_claimed_tasks<'a>(
    conn: &mut Conn<'a>,
    claim_timeout: std::time::Duration,
) -> Result<Vec<Task>, DbError> {
    use {
        crate::schema::task::dsl::*,
        diesel::{dsl::now, pg::data_types::PgInterval},
    };

    let micros = i64::try_from(claim_timeout.as_micros()).unwrap_or(i64::MAX);
    let interval = PgInterval::from_microseconds(micros).into_sql::<sql_types::Interval>();

    let updated = diesel::update(
        task.filter(
            status
                .eq(models::StatusKind::Claimed)
                .and(last_updated.lt(now.into_sql::<sql_types::Timestamptz>() - interval)),
        ),
    )
    .set((status.eq(models::StatusKind::Pending), last_updated.eq(now)))
    .returning(Task::as_returning())
    .get_results::<Task>(conn)
    .await?;

    Ok(updated)
}

pub(crate) async fn list_all_pending<'a>(conn: &mut Conn<'a>) -> Result<Vec<Task>, DbError> {
    use crate::schema::task::dsl::*;
    let tasks = task
        .filter(status.eq(models::StatusKind::Pending))
        .order(created_at.asc())
        .get_results(conn)
        .await?;
    Ok(tasks)
}

pub(crate) async fn list_task_filtered_paged<'a>(
    conn: &mut Conn<'a>,
    pagination: dtos::Pagination,
    filter: dtos::Filter,
) -> Result<Vec<dtos::BasicTaskDto>, DbError> {
    use crate::schema::task::dsl::*;
    use diesel::PgJsonbExpressionMethods;

    let mut query = task
        .into_boxed()
        .offset(pagination.offset)
        .limit(pagination.limit)
        .order(created_at.desc())
        .filter(name.like(format!("%{}%", filter.name)))
        .filter(kind.like(format!("%{}%", filter.kind)));

    if let Some(val) = filter.metadata {
        query = query.filter(metadata.contains(val));
    }

    if let Some(s) = filter.status {
        query = query.filter(status.eq(s));
    }

    if let Some(bid) = filter.batch_id {
        query = query.filter(batch_id.eq(bid));
    }

    if let Some(t) = filter.timeout {
        query = query.filter(timeout.eq(t));
    }

    let result = query.load::<models::Task>(conn).await?;

    let tasks: Vec<dtos::BasicTaskDto> = result.into_iter().map(dtos::BasicTaskDto::from).collect();

    Ok(tasks)
}

/// Get DAG data for a batch: all tasks and their links
pub(crate) async fn get_dag_for_batch<'a>(
    conn: &mut Conn<'a>,
    bid: Uuid,
) -> Result<dtos::DagDto, DbError> {
    use crate::schema::link::dsl::link;
    use crate::schema::task::dsl::*;

    // Get all tasks in the batch
    let tasks_result = task
        .filter(batch_id.eq(bid))
        .order(created_at.asc())
        .load::<models::Task>(conn)
        .await?;

    let task_ids: Vec<Uuid> = tasks_result.iter().map(|t| t.id).collect();

    // Get all links where both parent and child are in this batch
    let links_result = link
        .filter(
            crate::schema::link::dsl::parent_id
                .eq_any(&task_ids)
                .and(crate::schema::link::dsl::child_id.eq_any(&task_ids)),
        )
        .load::<Link>(conn)
        .await?;

    let tasks_dto: Vec<dtos::BasicTaskDto> = tasks_result
        .into_iter()
        .map(dtos::BasicTaskDto::from)
        .collect();

    let links_dto: Vec<dtos::LinkDto> = links_result
        .into_iter()
        .map(|l| dtos::LinkDto {
            parent_id: l.parent_id,
            child_id: l.child_id,
            requires_success: l.requires_success,
        })
        .collect();

    Ok(dtos::DagDto {
        tasks: tasks_dto,
        links: links_dto,
    })
}
