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

/// Fire webhooks for a specific trigger kind on a task.
/// Handles idempotency (claim/complete), action loading, and execution.
/// Used after a transaction commits — errors are intentionally non-fatal.
#[tracing::instrument(name = "fire_webhooks", skip(evaluator, conn), fields(task_id = %task_id, trigger_kind = ?trigger_kind))]
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

/// Propagates task completion to dependent children using batched queries.
///
/// When a parent task completes:
/// 1. If parent failed/canceled: batch-mark all requires_success children as Failure
/// 2. Batch-decrement wait_finished for all remaining children in Waiting status
/// 3. Batch-decrement wait_success for children where parent succeeded AND requires_success
/// 4. Batch-transition children to Pending where both counters reach 0
///
/// Returns the list of all task IDs that were cascade-failed (recursively),
/// so callers can fire on_failure webhooks for them after the transaction commits.
///
/// This uses O(1) queries per propagation level instead of O(N) per child.
#[tracing::instrument(name = "propagate_to_children", skip(conn), fields(parent_id = %parent_id, status = ?result_status))]
pub(crate) async fn propagate_to_children<'a>(
    parent_id: &uuid::Uuid,
    result_status: &StatusKind,
    conn: &mut Conn<'a>,
) -> Result<Vec<uuid::Uuid>, DbError> {
    use crate::schema::link::dsl as link_dsl;
    use crate::schema::task::dsl as task_dsl;

    let parent_succeeded = result_status == &StatusKind::Success;
    let parent_failed =
        result_status == &StatusKind::Failure || result_status == &StatusKind::Canceled;

    // Record dependency propagation metric
    let outcome = if parent_succeeded {
        "success"
    } else {
        "failure"
    };
    metrics::record_dependency_propagation(outcome);

    // Get all children of this parent task
    let children_links: Vec<(uuid::Uuid, bool)> = link_dsl::link
        .filter(link_dsl::parent_id.eq(parent_id))
        .select((link_dsl::child_id, link_dsl::requires_success))
        .load::<(uuid::Uuid, bool)>(conn)
        .await?;

    if children_links.is_empty() {
        return Ok(vec![]);
    }

    // Split children into groups for batched operations
    let mut fail_child_ids: Vec<uuid::Uuid> = Vec::new();
    let mut decrement_child_ids: Vec<uuid::Uuid> = Vec::new();
    let mut decrement_success_child_ids: Vec<uuid::Uuid> = Vec::new();

    for (child_id, requires_success) in &children_links {
        if parent_failed && *requires_success {
            fail_child_ids.push(*child_id);
        } else {
            decrement_child_ids.push(*child_id);
            if parent_succeeded && *requires_success {
                decrement_success_child_ids.push(*child_id);
            }
        }
    }

    // Collect all cascade-failed task IDs (direct + recursive)
    let mut all_cascade_failed: Vec<uuid::Uuid> = Vec::new();

    // 1. Batch-mark failed children (parent failed + requires_success)
    if !fail_child_ids.is_empty() {
        let failure_reason = format!("Required parent task {} failed", parent_id);
        let failed_ids: Vec<uuid::Uuid> = diesel::update(
            task_dsl::task.filter(
                task_dsl::id
                    .eq_any(&fail_child_ids)
                    .and(task_dsl::status.eq(StatusKind::Waiting)),
            ),
        )
        .set((
            task_dsl::status.eq(StatusKind::Failure),
            task_dsl::failure_reason.eq(&failure_reason),
            task_dsl::ended_at.eq(diesel::dsl::now),
        ))
        .returning(task_dsl::id)
        .get_results::<uuid::Uuid>(conn)
        .await?;

        for fid in &failed_ids {
            metrics::record_task_failed_by_dependency();
            log::info!(
                "Child task {} marked as failed due to required parent {} failure",
                fid,
                parent_id
            );
        }

        // Track direct failures
        all_cascade_failed.extend_from_slice(&failed_ids);

        // Recursively propagate failure to each actually-failed child's dependents
        for fid in &failed_ids {
            let recursive_failures =
                Box::pin(propagate_to_children(fid, &StatusKind::Failure, conn)).await?;
            all_cascade_failed.extend(recursive_failures);
        }
    }

    // 2. Batch-decrement counters for remaining children
    if !decrement_child_ids.is_empty() {
        // Decrement wait_finished for all remaining children
        diesel::update(
            task_dsl::task.filter(
                task_dsl::id
                    .eq_any(&decrement_child_ids)
                    .and(task_dsl::status.eq(StatusKind::Waiting)),
            ),
        )
        .set(task_dsl::wait_finished.eq(task_dsl::wait_finished - 1))
        .execute(conn)
        .await?;

        // Decrement wait_success only for children that require it
        if !decrement_success_child_ids.is_empty() {
            diesel::update(
                task_dsl::task.filter(
                    task_dsl::id
                        .eq_any(&decrement_success_child_ids)
                        .and(task_dsl::status.eq(StatusKind::Waiting)),
                ),
            )
            .set(task_dsl::wait_success.eq(task_dsl::wait_success - 1))
            .execute(conn)
            .await?;
        }

        // 3. Batch-transition to Pending where both counters reach 0
        let unblocked_ids: Vec<uuid::Uuid> = diesel::update(
            task_dsl::task.filter(
                task_dsl::id
                    .eq_any(&decrement_child_ids)
                    .and(task_dsl::status.eq(StatusKind::Waiting))
                    .and(task_dsl::wait_finished.eq(0))
                    .and(task_dsl::wait_success.eq(0)),
            ),
        )
        .set(task_dsl::status.eq(StatusKind::Pending))
        .returning(task_dsl::id)
        .get_results::<uuid::Uuid>(conn)
        .await?;

        for uid in &unblocked_ids {
            metrics::record_task_unblocked();
            metrics::record_status_transition("Waiting", "Pending");
            log::info!("Child task {} transitioned from Waiting to Pending", uid);
        }
    }

    Ok(all_cascade_failed)
}

pub async fn cancel_task<'a>(
    evaluator: &ActionExecutor,
    task_id: &uuid::Uuid,
    conn: &mut Conn<'a>,
) -> Result<(), DbError> {
    use crate::schema::task::dsl::*;
    let task_id = *task_id;

    // Phase 1: transaction — cancel + propagation are atomic
    let (prev_status, cascade_failed) = db_operation::run_in_transaction(conn, |conn| {
        Box::pin(async move {
            let t = task
                .filter(id.eq(task_id))
                .for_update()
                .first::<Task>(conn)
                .await?;

            match t.status {
                StatusKind::Pending
                | StatusKind::Paused
                | StatusKind::Claimed
                | StatusKind::Running => {}
                _ => {
                    return Err(crate::error::TaskRunnerError::InvalidState {
                        message: "Invalid operation: cannot cancel task in this state".into(),
                    });
                }
            }

            diesel::update(task.filter(id.eq(task_id)))
                .set((
                    status.eq(StatusKind::Canceled),
                    ended_at.eq(diesel::dsl::now),
                    last_updated.eq(diesel::dsl::now),
                ))
                .execute(conn)
                .await?;

            // Propagate cancellation to dependent children (inside tx)
            let cascade_failed =
                propagate_to_children(&task_id, &StatusKind::Canceled, conn).await?;

            Ok((t.status, cascade_failed))
        })
    })
    .await?;

    // Phase 2: best-effort webhooks (outside transaction, errors logged not returned)
    if prev_status == StatusKind::Running {
        if let Err(e) = fire_cancel_webhooks(evaluator, &task_id, conn).await {
            log::error!(
                "Failed to fire cancel webhooks for task {}: {:?}",
                task_id,
                e
            );
        }
    }

    for child_id in &cascade_failed {
        if let Err(e) = fire_end_webhooks(evaluator, child_id, StatusKind::Failure, conn).await {
            log::error!(
                "Failed to fire on_failure webhooks for cascade-failed child {}: {:?}",
                child_id,
                e
            );
        }
    }

    metrics::record_task_cancelled();
    Ok(())
}
