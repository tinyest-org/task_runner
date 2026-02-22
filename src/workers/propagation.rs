use crate::{
    Conn,
    action::ActionExecutor,
    db_operation::{self, DbError},
    metrics,
    models::{Action, StatusKind, Task, TriggerCondition, TriggerKind},
};
use diesel::BelongingToDsl;
use diesel::prelude::*;
use diesel_async::RunQueryDsl;

/// Fire end webhooks (on_success or on_failure) for a task without propagation.
/// Used after a transaction commits to fire webhooks best-effort.
#[tracing::instrument(name = "fire_end_webhooks", skip(evaluator, conn), fields(task_id = %task_id, status = ?result_status))]
pub(crate) async fn fire_end_webhooks<'a>(
    evaluator: &ActionExecutor,
    task_id: &uuid::Uuid,
    result_status: StatusKind,
    conn: &mut Conn<'a>,
) -> Result<(), DbError> {
    use crate::schema::action::dsl::{condition, trigger};
    use crate::schema::task::dsl::*;
    let t = task.filter(id.eq(task_id)).first::<Task>(conn).await?;
    let expected_condition = match result_status {
        StatusKind::Success => TriggerCondition::Success,
        _ => TriggerCondition::Failure,
    };
    let actions = Action::belonging_to(&t)
        .filter(trigger.eq(TriggerKind::End))
        .filter(condition.eq(expected_condition))
        .load::<Action>(conn)
        .await?;
    for act in actions.iter() {
        let res = evaluator.execute(act, &t).await;
        match res {
            Ok(_) => {
                log::debug!("Action {} executed successfully", act.id);
            }
            Err(e) => {
                log::error!("Action {} failed: {}", act.id, e);
            }
        }
    }
    Ok(())
}

/// Propagates task completion to dependent children using batched queries.
///
/// When a parent task completes:
/// 1. If parent failed/canceled: batch-mark all requires_success children as Failure
/// 2. Batch-decrement wait_finished for all remaining children in Waiting status
/// 3. Batch-decrement wait_success for children where parent succeeded AND requires_success
/// 4. Batch-transition children to Pending where both counters reach 0
///
/// This uses O(1) queries per propagation level instead of O(N) per child.
#[tracing::instrument(name = "propagate_to_children", skip(conn), fields(parent_id = %parent_id, status = ?result_status))]
pub(crate) async fn propagate_to_children<'a>(
    parent_id: &uuid::Uuid,
    result_status: &StatusKind,
    conn: &mut Conn<'a>,
) -> Result<(), DbError> {
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
        return Ok(());
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

        // Recursively propagate failure to each actually-failed child's dependents
        for fid in &failed_ids {
            Box::pin(propagate_to_children(fid, &StatusKind::Failure, conn)).await?;
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

    Ok(())
}

pub async fn cancel_task<'a>(
    evaluator: &ActionExecutor,
    task_id: &uuid::Uuid,
    conn: &mut Conn<'a>,
) -> Result<(), DbError> {
    use crate::schema::action::dsl::trigger;
    use crate::schema::task::dsl::*;
    let task_id = *task_id;

    let prev_status = db_operation::run_in_transaction(conn, |conn| {
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

            Ok(t.status)
        })
    })
    .await?;

    if prev_status == StatusKind::Running {
        let t = task.filter(id.eq(task_id)).first::<Task>(conn).await?;
        let actions = Action::belonging_to(&t)
            .filter(trigger.eq(TriggerKind::Cancel))
            .load::<Action>(conn)
            .await?;
        for act in actions.iter() {
            let res = evaluator.execute(act, &t).await;
            match res {
                Ok(_) => {
                    log::debug!("Action {} executed successfully", act.id);
                }
                Err(e) => {
                    log::error!("Action {} failed: {}", act.id, e);
                }
            }
        }
    }

    // Propagate cancellation to dependent children
    // Canceled is treated like failure for children that require success
    propagate_to_children(&task_id, &StatusKind::Canceled, conn).await?;

    metrics::record_task_cancelled();
    Ok(())
}
