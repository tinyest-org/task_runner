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

#[tracing::instrument(name = "update_running_task", skip(evaluator, conn, dto), fields(task_id = %task_id))]
pub async fn update_running_task<'a>(
    evaluator: &ActionExecutor,
    conn: &mut Conn<'a>,
    task_id: Uuid,
    dto: dtos::UpdateTaskDto,
) -> Result<UpdateTaskResult, DbError> {
    use crate::schema::task::dsl::*;
    use tracing::Instrument;
    let s = dto.status;
    let final_status_clone = dto.status;

    let has_status_change = dto.status.is_some();

    let res = if has_status_change {
        // Status change: transaction needed for atomic UPDATE + propagation
        run_in_transaction(conn, |conn| {
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
                ))
                .execute(conn)
                .await?;

                if res == 1 {
                    if let Some(ref final_status) = dto.status {
                        workers::propagate_to_children(&task_id, final_status, conn).await?;
                    }
                }

                Ok(res)
            })
        })
        .instrument(tracing::info_span!("tx_update_and_propagate"))
        .await?
    } else {
        // Counter-only update: no transaction needed, autocommit for minimal row lock
        diesel::update(task.filter(id.eq(task_id).and(status.eq(models::StatusKind::Running))))
            .set((
                last_updated.eq(diesel::dsl::now),
                dto.new_success.map(|e| success.eq(success + e)),
                dto.new_failures.map(|e| failures.eq(failures + e)),
                dto.metadata.map(|m| metadata.eq(m)),
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
            match workers::fire_end_webhooks(evaluator, &task_id, *final_status, conn)
                .instrument(tracing::info_span!("fire_end_webhooks"))
                .await
            {
                Ok(_) => log::debug!("task {} end webhooks fired successfully", &task_id),
                Err(e) => log::error!("task {} end webhooks failed: {}", &task_id, e),
            }
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
) -> Result<(), DbError> {
    // Wrap status update + propagation in a transaction
    let tid = *task_id;
    let reason_owned = reason.to_string();
    let updated = run_in_transaction(conn, |conn| {
        Box::pin(async move {
            let updated = mark_task_failed(conn, &tid, &reason_owned).await?;
            if updated {
                workers::propagate_to_children(&tid, &StatusKind::Failure, conn).await?;
            }
            Ok(updated)
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
    match workers::fire_end_webhooks(evaluator, task_id, StatusKind::Failure, conn).await {
        Ok(_) => log::debug!(
            "task {} on_failure webhooks fired after on_start failure",
            task_id
        ),
        Err(e) => log::error!("task {} on_failure webhooks failed: {}", task_id, e),
    }

    Ok(())
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
