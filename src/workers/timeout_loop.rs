use crate::{DbPool, action::ActionExecutor, db_operation, metrics, models::StatusKind};
use actix_web::rt;
use std::sync::Arc;
use tokio::sync::watch;

use super::propagation::{fire_end_webhooks, propagate_to_children};

/// This ensures the non responding tasks are set to fail
///
/// Add the date of failure
pub async fn timeout_loop(
    evaluator: Arc<ActionExecutor>,
    pool: DbPool,
    interval: std::time::Duration,
    claim_timeout: std::time::Duration,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        // note that obtaining a connection from the pool is also potentially blocking
        let conn = pool.get();

        if let Ok(mut conn) = conn.await {
            let requeued =
                db_operation::requeue_stale_claimed_tasks(&mut conn, claim_timeout).await;
            match requeued {
                Ok(tasks) => {
                    if !tasks.is_empty() {
                        for _ in &tasks {
                            metrics::record_status_transition("Claimed", "Pending");
                        }
                        log::warn!(
                            "Timeout worker: requeued {} stale claimed tasks, {:?}",
                            tasks.len(),
                            tasks.iter().map(|t| t.id).collect::<Vec<_>>()
                        );
                    } else {
                        log::debug!("Timeout worker: no stale claimed tasks");
                    }
                }
                Err(e) => {
                    log::error!("Timeout worker: error requeuing claimed tasks: {:?}", e);
                }
            }

            let res = db_operation::ensure_pending_tasks_timeout(&mut conn).await;
            match res {
                Ok(failed) => {
                    if !failed.is_empty() {
                        // Record timeout metrics
                        for _ in &failed {
                            metrics::record_task_timeout();
                        }
                        log::warn!(
                            "Timeout worker: {} tasks failed, {:?}",
                            &failed.len(),
                            &failed.iter().map(|e| e.id).collect::<Vec<_>>()
                        );
                        // Propagate timeout failures to dependent children
                        for failed_task in &failed {
                            if let Err(e) = propagate_to_children(
                                &failed_task.id,
                                &StatusKind::Failure,
                                &mut conn,
                            )
                            .await
                            {
                                log::error!(
                                    "Timeout worker: failed to propagate failure for task {}: {:?}",
                                    failed_task.id,
                                    e
                                );
                            }

                            if let Err(e) = fire_end_webhooks(
                                evaluator.as_ref(),
                                &failed_task.id,
                                StatusKind::Failure,
                                &mut conn,
                            )
                            .await
                            {
                                log::error!(
                                    "Timeout worker: failed to fire on_failure webhooks for task {}: {:?}",
                                    failed_task.id,
                                    e
                                );
                            }
                        }
                    } else {
                        log::debug!("Timeout worker: no tasks failed");
                    }
                }
                Err(e) => {
                    log::error!("Timeout worker: error updating tasks: {:?}", e);
                }
            }
        }
        tokio::select! {
            _ = shutdown.changed() => {
                log::info!("Timeout worker: shutdown signal received, exiting");
                return;
            }
            _ = rt::time::sleep(interval) => {}
        }
    }
}
