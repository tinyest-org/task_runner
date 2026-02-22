use crate::{
    Conn, DbPool,
    action::ActionExecutor,
    db_operation,
    dtos::NewActionDto,
    metrics,
    models::{Action, Task, TriggerKind},
    rule::Strategy,
};
use actix_web::rt;
use diesel::BelongingToDsl;
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use std::collections::HashSet;
use tokio::sync::watch;

/// In order to cache results and avoid too many db calls.
/// Uses lock keys (i64) instead of Strategy to ensure metadata-sensitive caching:
/// two tasks with the same rule but different metadata values get different lock keys.
struct EvaluationContext {
    ko: HashSet<i64>,
}

pub async fn start_loop(
    evaluator: &ActionExecutor,
    pool: DbPool,
    interval: std::time::Duration,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        let loop_start = std::time::Instant::now();
        let mut tasks_processed = 0usize;

        // note that obtaining a connection from the pool is also potentially blocking
        let conn = pool.get();
        if let Ok(mut conn) = conn.await {
            let res = db_operation::list_all_pending(&mut conn).await;
            match res {
                Ok(tasks) => {
                    let mut ctx = EvaluationContext { ko: HashSet::new() };
                    log::debug!("Start worker: found {} tasks", tasks.len());
                    for t in tasks {
                        // Pre-filter: if we already know a rule is blocked in this
                        // iteration, skip the DB call entirely.
                        if is_prefilter_blocked(&t, &ctx) {
                            metrics::record_task_blocked_by_concurrency();
                            log::warn!("Start worker: task {} blocked by cached rule", t.id);
                            continue;
                        }

                        // Atomically check concurrency rules + claim in a single transaction
                        match db_operation::claim_task_with_rules(&mut conn, &t).await {
                            Ok(db_operation::ClaimResult::Claimed) => {
                                metrics::record_status_transition("Pending", "Claimed");
                                // Task claimed, now execute the on_start webhook
                                match start_task(evaluator, &t, &mut conn).await {
                                    Ok(cancel_tasks) => {
                                        match db_operation::mark_task_running(&mut conn, &t.id)
                                            .await
                                        {
                                            Ok(true) => {
                                                tasks_processed += 1;
                                                metrics::record_status_transition(
                                                    "Claimed", "Running",
                                                );
                                                log::debug!("Start worker: task {} started", t.id);
                                                // Save cancel actions returned by the webhook
                                                if let Err(e) = db_operation::save_cancel_actions(
                                                    &mut conn,
                                                    &t,
                                                    &cancel_tasks,
                                                )
                                                .await
                                                {
                                                    log::error!(
                                                        "Start worker: failed to save cancel actions for task {}: {:?}",
                                                        t.id,
                                                        e
                                                    );
                                                }
                                            }
                                            Ok(false) => {
                                                log::warn!(
                                                    "Start worker: task {} no longer claimed; skipping running transition",
                                                    t.id
                                                );
                                            }
                                            Err(e) => {
                                                log::error!(
                                                    "Start worker: failed to mark task {} as running: {:?}",
                                                    t.id,
                                                    e
                                                );
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        // Webhook failed after claim -> mark task as failed,
                                        // propagate to children, and fire on_failure webhooks
                                        log::error!(
                                            "Start worker: on_start webhook failed for task {}: {:?}",
                                            t.id,
                                            e
                                        );
                                        if let Err(e2) = db_operation::fail_task_and_propagate(
                                            evaluator,
                                            &mut conn,
                                            &t.id,
                                            "on_start webhook failed",
                                        )
                                        .await
                                        {
                                            log::error!(
                                                "Start worker: failed to mark task {} as failed and propagate: {:?}",
                                                t.id,
                                                e2
                                            );
                                        }
                                    }
                                }
                            }
                            Ok(db_operation::ClaimResult::RuleBlocked) => {
                                // Cache the blocked lock keys for this iteration so subsequent
                                // tasks with the same rule+metadata combo are skipped without a DB call
                                for strategy in &t.start_condition.0 {
                                    match strategy {
                                        Strategy::Concurency(rule) => {
                                            let key = db_operation::concurrency_lock_key(
                                                rule,
                                                &t.metadata,
                                            );
                                            ctx.ko.insert(key);
                                        }
                                    }
                                }
                                metrics::record_task_blocked_by_concurrency();
                                log::warn!(
                                    "Start worker: task {} blocked by concurrency rule",
                                    t.id
                                );
                            }
                            Ok(db_operation::ClaimResult::AlreadyClaimed) => {
                                log::debug!(
                                    "Start worker: task {} already claimed by another worker",
                                    t.id
                                );
                            }
                            Err(e) => {
                                log::error!("Start worker: failed to claim task {}: {:?}", t.id, e);
                            }
                        }
                    }
                }
                Err(e) => {
                    log::error!("Start worker: error updating tasks: {:?}", e);
                }
            }
        }
        // Record worker loop metrics
        let loop_duration = loop_start.elapsed().as_secs_f64();
        metrics::record_worker_loop_iteration(loop_duration, tasks_processed);

        tokio::select! {
            _ = shutdown.changed() => {
                log::info!("Start worker: shutdown signal received, exiting");
                return;
            }
            _ = rt::time::sleep(interval) => {}
        }
    }
}

/// Pre-filter check: if any of the task's rule+metadata lock keys were already
/// blocked in this iteration, skip the expensive DB call. Within a single loop
/// iteration, counts can only increase (we only claim tasks), so a blocked key
/// stays blocked.
fn is_prefilter_blocked(task: &Task, ctx: &EvaluationContext) -> bool {
    let conditions = &task.start_condition.0;
    if conditions.is_empty() {
        return false;
    }
    conditions.iter().any(|cond| match cond {
        Strategy::Concurency(rule) => {
            let key = db_operation::concurrency_lock_key(rule, &task.metadata);
            ctx.ko.contains(&key)
        }
    })
}

async fn start_task<'a>(
    evaluator: &ActionExecutor,
    task: &Task,
    conn: &mut Conn<'a>,
) -> Result<Vec<NewActionDto>, String> {
    use crate::schema::action::dsl::*;
    let actions = Action::belonging_to(&task)
        .filter(trigger.eq(TriggerKind::Start))
        .load::<Action>(conn)
        .await
        .map_err(|e| e.to_string())?;
    let mut tasks = vec![];
    let mut errors: Vec<String> = Vec::new();
    for act in actions.iter() {
        let res = evaluator.execute(act, task).await;
        match res {
            Ok(r) => {
                if let Some(t) = r {
                    tasks.push(t);
                };
                log::debug!("Action {} executed successfully", act.id);
            }
            Err(e) => {
                log::error!("Action {} failed: {}", act.id, e);
                errors.push(e);
            }
        }
    }
    if !errors.is_empty() {
        return Err(format!(
            "one or more on_start actions failed for task {}: {}",
            task.id,
            errors.join("; ")
        ));
    }
    Ok(tasks)
}
