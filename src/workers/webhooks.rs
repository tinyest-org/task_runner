use crate::{
    Conn,
    action::{ActionExecutor, idempotency_key},
    db_operation::{self, DbError},
    metrics,
    models::{Action, StatusKind, Task, TriggerCondition, TriggerKind},
};
use diesel::BelongingToDsl;
use diesel::prelude::*;
use diesel_async::RunQueryDsl;

use super::propagation::CanceledAncestor;

/// Fire webhooks for a specific trigger kind on a task.
/// Handles idempotency (claim/complete), action loading, and execution.
/// Used after a transaction commits — errors are intentionally non-fatal.
#[tracing::instrument(name = "fire_webhooks", level = "debug", skip(evaluator, conn), fields(task_id = %task_id, trigger_kind = ?trigger_kind))]
pub(crate) async fn fire_webhooks<'a>(
    evaluator: &ActionExecutor,
    task_id: &uuid::Uuid,
    trigger_kind: TriggerKind,
    trigger_condition: TriggerCondition,
    conn: &mut Conn<'a>,
) -> Result<(), DbError> {
    use crate::schema::action::dsl::{condition, trigger};
    use crate::schema::task::dsl::*;

    // Idempotency guard
    let key = idempotency_key(*task_id, &trigger_kind, &trigger_condition);
    let claimed = db_operation::try_claim_webhook_execution(
        conn,
        *task_id,
        trigger_kind,
        trigger_condition,
        &key,
        Some(evaluator.ctx.webhook_idempotency_timeout),
    )
    .await?;

    if !claimed {
        let trigger_label = match (&trigger_kind, &trigger_condition) {
            (TriggerKind::End, TriggerCondition::Success) => "end_success",
            (TriggerKind::End, TriggerCondition::Failure) => "end_failure",
            (TriggerKind::Cancel, _) => "cancel",
            (TriggerKind::Start, _) => "start",
        };
        log::info!(
            "Skipping {} webhooks for task {} — already executed (key={})",
            trigger_label,
            task_id,
            key
        );
        metrics::record_webhook_idempotent_skip(trigger_label);
        metrics::record_webhook_idempotent_conflict();
        return Ok(());
    }

    let t = task.filter(id.eq(task_id)).first::<Task>(conn).await?;
    let actions = Action::belonging_to(&t)
        .filter(trigger.eq(trigger_kind))
        .filter(condition.eq(trigger_condition))
        .load::<Action>(conn)
        .await?;

    let mut all_succeeded = true;
    for act in actions.iter() {
        match evaluator.execute(act, &t, Some(&key)).await {
            Ok(_) => log::debug!("Action {} executed successfully", act.id),
            Err(e) => {
                log::error!("Action {} failed: {}", act.id, e);
                all_succeeded = false;
            }
        }
    }

    if let Err(e) = db_operation::complete_webhook_execution(conn, &key, all_succeeded).await {
        log::error!(
            "Failed to complete webhook execution record for key {}: {}",
            key,
            e
        );
    }

    Ok(())
}

/// Convenience: fire end webhooks (on_success or on_failure) for a task.
#[inline]
pub(crate) async fn fire_end_webhooks<'a>(
    evaluator: &ActionExecutor,
    task_id: &uuid::Uuid,
    result_status: StatusKind,
    conn: &mut Conn<'a>,
) -> Result<(), DbError> {
    let cond = match result_status {
        StatusKind::Success => TriggerCondition::Success,
        _ => TriggerCondition::Failure,
    };
    fire_webhooks(evaluator, task_id, TriggerKind::End, cond, conn).await
}

/// Convenience: fire cancel webhooks for a task.
#[inline]
pub(crate) async fn fire_cancel_webhooks<'a>(
    evaluator: &ActionExecutor,
    task_id: &uuid::Uuid,
    conn: &mut Conn<'a>,
) -> Result<(), DbError> {
    fire_webhooks(
        evaluator,
        task_id,
        TriggerKind::Cancel,
        TriggerCondition::Success,
        conn,
    )
    .await
}

/// Fire end webhooks for a task, then on_failure webhooks for each cascade-failed child.
/// All errors are logged but not returned — this is best-effort post-commit work.
pub(crate) async fn fire_end_webhooks_with_cascade<'a>(
    evaluator: &ActionExecutor,
    task_id: &uuid::Uuid,
    result_status: StatusKind,
    cascade_failed_ids: &[uuid::Uuid],
    conn: &mut Conn<'a>,
) {
    if let Err(e) = fire_end_webhooks(evaluator, task_id, result_status, conn).await {
        log::error!("Failed to fire end webhooks for task {}: {:?}", task_id, e);
    }

    for child_id in cascade_failed_ids {
        if let Err(e) = fire_end_webhooks(evaluator, child_id, StatusKind::Failure, conn).await {
            log::error!(
                "Failed to fire on_failure webhooks for cascade-failed child {}: {:?}",
                child_id,
                e
            );
        }
    }
}

/// Fire webhooks for ancestors canceled by dead-end detection.
/// For previously-Running tasks: fire cancel webhooks.
/// For all: fire on_failure end webhooks.
/// Best-effort — errors are logged but not returned.
pub(crate) async fn fire_webhooks_for_canceled_ancestors<'a>(
    evaluator: &ActionExecutor,
    ancestors: &[CanceledAncestor],
    conn: &mut Conn<'a>,
) {
    for ancestor in ancestors {
        if ancestor.was_running
            && let Err(e) = fire_cancel_webhooks(evaluator, &ancestor.id, conn).await
        {
            log::error!(
                "Failed to fire cancel webhooks for dead-end ancestor {}: {:?}",
                ancestor.id,
                e
            );
        }
        if let Err(e) = fire_end_webhooks(evaluator, &ancestor.id, StatusKind::Failure, conn).await
        {
            log::error!(
                "Failed to fire on_failure webhooks for dead-end ancestor {}: {:?}",
                ancestor.id,
                e
            );
        }
    }
}
