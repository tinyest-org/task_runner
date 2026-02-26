use crate::{
    Conn,
    action::ActionExecutor,
    dtos, metrics,
    models::{self, StatusKind, Task},
    workers,
};
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use uuid::Uuid;

use super::{DbError, run_in_transaction, task_crud::insert_actions};

/// Result of attempting to update a running task.
#[derive(Debug, PartialEq)]
pub enum UpdateTaskResult {
    /// Task was successfully updated (transitioned from Running).
    Updated,
    /// Task does not exist or was not in Running state.
    NotFound,
}

#[tracing::instrument(name = "update_running_task", level = "debug", skip(evaluator, conn, dto), fields(task_id = %task_id))]
pub async fn update_running_task<'a>(
    evaluator: &ActionExecutor,
    conn: &mut Conn<'a>,
    task_id: Uuid,
    dto: dtos::UpdateTaskDto,
    dead_end_enabled: bool,
) -> Result<UpdateTaskResult, DbError> {
    use crate::schema::task::dsl::*;
    use tracing::Instrument;
    let s = dto.status;
    let final_status_clone = dto.status;

    let has_status_change = dto.status.is_some();

    // Collect cascade-failed task IDs from propagation (inside transaction)
    // so we can fire their on_failure webhooks after commit.
    let mut cascade_failed_ids: Vec<uuid::Uuid> = Vec::new();
    let mut canceled_ancestors: Vec<workers::propagation::CanceledAncestor> = Vec::new();

    let res = if has_status_change {
        // Status change: transaction needed for atomic UPDATE + propagation
        let (rows, cascade_failed, ancestors) = run_in_transaction(conn, |conn| {
            Box::pin(async move {
                let res = diesel::update(
                    task.filter(id.eq(task_id).and(status.eq(models::StatusKind::Running))),
                )
                .set((
                    last_updated.eq(diesel::dsl::now),
                    dto.new_success.map(|e| success.eq(success + e)),
                    dto.new_failures.map(|e| failures.eq(failures + e)),
                    s.filter(|e| {
                        e == &models::StatusKind::Success || e == &models::StatusKind::Failure
                    })
                    .map(|_| ended_at.eq(diesel::dsl::now)),
                    dto.metadata.map(|m| metadata.eq(m)),
                    dto.status.as_ref().map(|m| status.eq(m)),
                    dto.failure_reason.map(|m| failure_reason.eq(m)),
                    dto.expected_count.map(|c| expected_count.eq(c)),
                ))
                .execute(conn)
                .await?;

                let mut cascade = Vec::new();
                let mut ancestors = Vec::new();
                if res == 1 {
                    if let Some(ref final_status) = dto.status {
                        cascade =
                            workers::propagate_to_children(&task_id, final_status, conn).await?;

                        // Dead-end ancestor cancellation
                        if dead_end_enabled {
                            let mut terminal_ids = vec![task_id];
                            terminal_ids.extend_from_slice(&cascade);
                            ancestors =
                                workers::cancel_dead_end_ancestors(&terminal_ids, conn).await?;
                        }
                    }
                }

                Ok((res, cascade, ancestors))
            })
        })
        .instrument(tracing::info_span!("tx_update_and_propagate"))
        .await?;
        cascade_failed_ids = cascade_failed;
        canceled_ancestors = ancestors;
        rows
    } else {
        // Counter-only update: no transaction needed, autocommit for minimal row lock
        diesel::update(task.filter(id.eq(task_id).and(status.eq(models::StatusKind::Running))))
            .set((
                last_updated.eq(diesel::dsl::now),
                dto.new_success.map(|e| success.eq(success + e)),
                dto.new_failures.map(|e| failures.eq(failures + e)),
                dto.metadata.map(|m| metadata.eq(m)),
                dto.expected_count.map(|c| expected_count.eq(c)),
            ))
            .execute(conn)
            .await?
    };

    // After commit: fire webhooks and record metrics (best-effort, outside transaction)
    if let Some(ref final_status) = final_status_clone {
        if res == 1 {
            let outcome = match final_status {
                models::StatusKind::Success => "success",
                models::StatusKind::Failure => "failure",
                _ => "other",
            };
            metrics::record_status_transition("Running", outcome);
            workers::fire_end_webhooks_with_cascade(
                evaluator,
                &task_id,
                *final_status,
                &cascade_failed_ids,
                conn,
            )
            .instrument(tracing::info_span!("fire_end_webhooks_with_cascade"))
            .await;

            workers::fire_webhooks_for_canceled_ancestors(evaluator, &canceled_ancestors, conn)
                .await;
        } else {
            log::warn!(
                "update_running_task: task {} was not in Running state, skipping webhooks/propagation",
                task_id
            );
        }
    }

    if res == 1 {
        Ok(UpdateTaskResult::Updated)
    } else {
        Ok(UpdateTaskResult::NotFound)
    }
}

/// Save cancel actions for a task that has been claimed.
/// Validates webhook URLs before saving to prevent SSRF via cancel action responses.
pub(crate) async fn save_cancel_actions<'a>(
    conn: &mut Conn<'a>,
    t: &Task,
    cancel_tasks: &[dtos::NewActionDto],
) -> Result<(), DbError> {
    if !cancel_tasks.is_empty() {
        // Validate each cancel action's webhook URL before persisting
        for action_dto in cancel_tasks {
            if let Err(e) =
                crate::validation::validate_action_params(&action_dto.kind, &action_dto.params)
            {
                log::warn!(
                    "Rejecting cancel action for task {} due to validation failure: {}",
                    t.id,
                    e
                );
                return Err(crate::error::TaskRunnerError::Validation(format!(
                    "Cancel action validation failed: {}",
                    e
                )));
            }
        }
        insert_actions(
            t.id,
            cancel_tasks,
            &models::TriggerKind::Cancel,
            &models::TriggerCondition::Success, // Condition doesn't matter for cancel
            conn,
        )
        .await?;
    }
    Ok(())
}

/// Mark a task as failed with a reason. Returns true if the task was updated.
/// Used when on_start webhook fails after claim.
pub(crate) async fn mark_task_failed<'a>(
    conn: &mut Conn<'a>,
    task_id: &uuid::Uuid,
    reason: &str,
) -> Result<bool, DbError> {
    use crate::schema::task::dsl::*;
    use diesel::dsl::now;

    let updated = diesel::update(
        task.filter(
            id.eq(task_id).and(
                status
                    .eq(StatusKind::Running)
                    .or(status.eq(StatusKind::Claimed)),
            ),
        ),
    )
    .set((
        status.eq(StatusKind::Failure),
        failure_reason.eq(reason),
        ended_at.eq(now),
        last_updated.eq(now),
    ))
    .execute(conn)
    .await?;
    Ok(updated == 1)
}

/// Mark a task as failed, propagate failure to children (in a transaction),
/// then fire on_failure webhooks (best-effort, outside transaction).
/// Used when on_start webhook fails after claim.
pub(crate) async fn fail_task_and_propagate<'a>(
    evaluator: &ActionExecutor,
    conn: &mut Conn<'a>,
    task_id: &uuid::Uuid,
    reason: &str,
    dead_end_enabled: bool,
) -> Result<(), DbError> {
    // Wrap status update + propagation in a transaction
    let tid = *task_id;
    let reason_owned = reason.to_string();
    let (updated, cascade_failed, canceled_ancestors) = run_in_transaction(conn, |conn| {
        Box::pin(async move {
            let updated = mark_task_failed(conn, &tid, &reason_owned).await?;
            let mut cascade = Vec::new();
            let mut ancestors = Vec::new();
            if updated {
                cascade = workers::propagate_to_children(&tid, &StatusKind::Failure, conn).await?;

                if dead_end_enabled {
                    let mut terminal_ids = vec![tid];
                    terminal_ids.extend_from_slice(&cascade);
                    ancestors = workers::cancel_dead_end_ancestors(&terminal_ids, conn).await?;
                }
            }
            Ok((updated, cascade, ancestors))
        })
    })
    .await?;

    if !updated {
        log::warn!(
            "fail_task_and_propagate: task {} not in Running/Claimed state; skipping failure propagation",
            task_id
        );
        return Ok(());
    }

    // After commit: fire on_failure webhooks (best-effort)
    workers::fire_end_webhooks_with_cascade(
        evaluator,
        task_id,
        StatusKind::Failure,
        &cascade_failed,
        conn,
    )
    .await;

    workers::fire_webhooks_for_canceled_ancestors(evaluator, &canceled_ancestors, conn).await;

    Ok(())
}

/// Result of stopping a batch. Contains counts per status category
/// and the list of Running task IDs that need cancel webhooks fired.
pub struct StopBatchResult {
    pub canceled_waiting: i64,
    pub canceled_pending: i64,
    pub canceled_claimed: i64,
    /// IDs of formerly-Running tasks (need cancel webhooks fired).
    pub canceled_running_ids: Vec<uuid::Uuid>,
    pub canceled_paused: i64,
    pub already_terminal: i64,
}

/// Stop all non-terminal tasks in a batch by setting them to Canceled.
/// Runs in a transaction for atomicity. Returns per-status counts and the
/// IDs of formerly-Running tasks (for cancel webhook firing).
#[tracing::instrument(name = "stop_batch", skip(conn), fields(batch_id = %batch_id))]
pub(crate) async fn stop_batch<'a>(
    conn: &mut Conn<'a>,
    batch_id: Uuid,
) -> Result<StopBatchResult, DbError> {
    use crate::schema::task::dsl;

    // Check that the batch exists before starting the transaction
    let total: i64 = dsl::task
        .filter(dsl::batch_id.eq(batch_id))
        .count()
        .get_result(conn)
        .await?;

    if total == 0 {
        return Err(crate::error::TaskRunnerError::NotFound {
            message: format!("No tasks found for batch {}", batch_id),
        });
    }

    // Macro to build the cancel changeset inline (Diesel types are not Copy)
    macro_rules! cancel_set {
        () => {
            (
                dsl::status.eq(StatusKind::Canceled),
                dsl::failure_reason.eq("Batch stopped"),
                dsl::ended_at.eq(diesel::dsl::now),
                dsl::last_updated.eq(diesel::dsl::now),
            )
        };
    }

    run_in_transaction(conn, |conn| {
        Box::pin(async move {
            // Count already-terminal tasks
            let already_terminal: i64 = dsl::task
                .filter(
                    dsl::batch_id.eq(batch_id).and(
                        dsl::status
                            .eq(StatusKind::Success)
                            .or(dsl::status.eq(StatusKind::Failure))
                            .or(dsl::status.eq(StatusKind::Canceled)),
                    ),
                )
                .count()
                .get_result(conn)
                .await?;

            // Cancel Waiting tasks
            let canceled_waiting = diesel::update(
                dsl::task.filter(
                    dsl::batch_id
                        .eq(batch_id)
                        .and(dsl::status.eq(StatusKind::Waiting)),
                ),
            )
            .set(cancel_set!())
            .execute(conn)
            .await? as i64;

            // Cancel Pending tasks
            let canceled_pending = diesel::update(
                dsl::task.filter(
                    dsl::batch_id
                        .eq(batch_id)
                        .and(dsl::status.eq(StatusKind::Pending)),
                ),
            )
            .set(cancel_set!())
            .execute(conn)
            .await? as i64;

            // Cancel Paused tasks
            let canceled_paused = diesel::update(
                dsl::task.filter(
                    dsl::batch_id
                        .eq(batch_id)
                        .and(dsl::status.eq(StatusKind::Paused)),
                ),
            )
            .set(cancel_set!())
            .execute(conn)
            .await? as i64;

            // Cancel Claimed tasks (no cancel webhooks needed — on_start not yet called)
            let canceled_claimed = diesel::update(
                dsl::task.filter(
                    dsl::batch_id
                        .eq(batch_id)
                        .and(dsl::status.eq(StatusKind::Claimed)),
                ),
            )
            .set(cancel_set!())
            .execute(conn)
            .await? as i64;

            // Cancel Running tasks (return their IDs for cancel webhook firing)
            let canceled_running_ids: Vec<uuid::Uuid> = diesel::update(
                dsl::task.filter(
                    dsl::batch_id
                        .eq(batch_id)
                        .and(dsl::status.eq(StatusKind::Running)),
                ),
            )
            .set(cancel_set!())
            .returning(dsl::id)
            .get_results(conn)
            .await?;

            Ok(StopBatchResult {
                canceled_waiting,
                canceled_pending,
                canceled_claimed,
                canceled_running_ids,
                canceled_paused,
                already_terminal,
            })
        })
    })
    .await
}

pub(crate) async fn pause_task<'a>(
    task_id: &uuid::Uuid,
    conn: &mut Conn<'a>,
) -> Result<(), DbError> {
    use crate::schema::task::dsl::{id, status, task};
    let updated = diesel::update(
        task.filter(
            id.eq(task_id).and(
                status
                    .eq(StatusKind::Pending)
                    .or(status.eq(StatusKind::Claimed))
                    .or(status.eq(StatusKind::Running))
                    .or(status.eq(StatusKind::Waiting)),
            ),
        ),
    )
    .set(status.eq(StatusKind::Paused))
    .execute(conn)
    .await?;
    if updated == 0 {
        return Err(crate::error::TaskRunnerError::InvalidState {
            message: "Invalid operation: cannot pause task in this state".into(),
        });
    }
    Ok(())
}
