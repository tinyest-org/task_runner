use crate::Conn;
use crate::models::{TriggerCondition, TriggerKind, WebhookExecutionStatus};
use diesel::ExpressionMethods;
use diesel::QueryDsl;
use diesel_async::RunQueryDsl;

use super::DbError;

/// Attempt to claim a webhook execution slot for idempotency.
///
/// Uses INSERT ... ON CONFLICT to atomically claim or skip:
/// - If no row exists: inserts a new `pending` row → returns `Ok(true)` (proceed)
/// - If a row exists with `status = 'success'`: no update → returns `Ok(false)` (skip)
/// - If a row exists with `status = 'failure'`: resets to pending → returns `Ok(true)` (retry)
/// - If a row exists with `status = 'pending'`: retries only when `stale_after` elapsed
/// Note: for Start/Cancel triggers, `condition` is stored as `Success` sentinel.
pub async fn try_claim_webhook_execution<'a>(
    conn: &mut Conn<'a>,
    task_id: uuid::Uuid,
    trigger_kind: TriggerKind,
    trigger_condition: TriggerCondition,
    key: &str,
    stale_after: Option<std::time::Duration>,
) -> Result<bool, DbError> {
    use crate::schema::sql_types as st;

    let stale_after_micros = stale_after.map(|d| i64::try_from(d.as_micros()).unwrap_or(i64::MAX));

    // INSERT ... ON CONFLICT with a WHERE filter on the DO UPDATE.
    // - If status = 'success' → no update (0 rows affected)
    // - If status = 'failure' → update, reset to pending (retry allowed)
    // - If status = 'pending' → update only if stale_after is provided and elapsed
    let result: usize = diesel::sql_query(
        "INSERT INTO webhook_execution (task_id, trigger, condition, idempotency_key, status, attempts)
         VALUES ($1, $2, $3, $4, 'pending', 1)
         ON CONFLICT (idempotency_key) DO UPDATE
         SET attempts = webhook_execution.attempts + 1,
             status = 'pending',
             updated_at = now()
         WHERE webhook_execution.status = 'failure'
            OR (
                webhook_execution.status = 'pending'
                AND $5 IS NOT NULL
                AND webhook_execution.updated_at < (now() - ($5::bigint * interval '1 microsecond'))
            )"
    )
    .bind::<diesel::sql_types::Uuid, _>(task_id)
    .bind::<st::TriggerKind, _>(trigger_kind)
    .bind::<st::TriggerCondition, _>(trigger_condition)
    .bind::<diesel::sql_types::Text, _>(key)
    .bind::<diesel::sql_types::Nullable<diesel::sql_types::BigInt>, _>(stale_after_micros)
    .execute(conn)
    .await?;

    Ok(result > 0)
}

/// Mark a webhook execution as success or failure after execution completes.
pub async fn complete_webhook_execution<'a>(
    conn: &mut Conn<'a>,
    key: &str,
    succeeded: bool,
) -> Result<(), DbError> {
    use crate::schema::webhook_execution::dsl;

    let new_status = if succeeded {
        WebhookExecutionStatus::Success
    } else {
        WebhookExecutionStatus::Failure
    };

    diesel::update(dsl::webhook_execution.filter(dsl::idempotency_key.eq(key)))
        .set((
            dsl::status.eq(new_status),
            dsl::updated_at.eq(diesel::dsl::now),
        ))
        .execute(conn)
        .await?;

    Ok(())
}
