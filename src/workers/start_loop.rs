use crate::{
    Conn, DbPool,
    action::{ActionExecutor, idempotency_key},
    db_operation,
    dtos::NewActionDto,
    metrics,
    models::{Action, Task, TriggerCondition, TriggerKind},
    rule::Strategy,
};
use actix_web::rt;
use diesel::BelongingToDsl;
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::watch;
use tokio::task::JoinSet;

/// In order to cache results and avoid too many db calls.
/// Uses lock keys (i64) instead of Strategy to ensure metadata-sensitive caching:
/// two tasks with the same rule but different metadata values get different lock keys.
struct EvaluationContext {
    ko: HashSet<i64>,
}

struct StartTaskResult {
    cancel_tasks: Vec<NewActionDto>,
    idempotency_key: String,
    claimed: bool,
}

pub async fn start_loop(
    evaluator: &ActionExecutor,
    pool: DbPool,
    interval: std::time::Duration,
    dead_end_enabled: bool,
    start_batch_size: i64,
    webhook_concurrency: usize,
    mut shutdown: watch::Receiver<bool>,
) {
    let semaphore = Arc::new(tokio::sync::Semaphore::new(webhook_concurrency));

    loop {
        let loop_start = std::time::Instant::now();

        // Phase 1: Claim (sequential, single connection)
        let claimed_tasks = claim_phase(&pool, start_batch_size).await;

        // Phase 2: Webhooks (parallel, JoinSet + Semaphore)
        let tasks_processed = webhook_phase(
            claimed_tasks,
            evaluator,
            &pool,
            &semaphore,
            dead_end_enabled,
        )
        .await;

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

/// Phase 1: Fetch pending tasks (with LIMIT) and claim them sequentially.
/// Returns the list of successfully claimed tasks.
async fn claim_phase(pool: &DbPool, _batch_size: i64) -> Vec<Task> {
    let conn = pool.get();
    let Ok(mut conn) = conn.await else {
        log::error!("Start worker: failed to acquire DB connection for claim phase");
        return vec![];
    };

    let res = db_operation::list_all_pending(&mut conn).await;
    let tasks = match res {
        Ok(tasks) => tasks,
        Err(e) => {
            log::error!("Start worker: error fetching pending tasks: {:?}", e);
            return vec![];
        }
    };

    let mut ctx = EvaluationContext { ko: HashSet::new() };
    let mut claimed = Vec::new();

    log::debug!("Start worker: found {} pending tasks", tasks.len());

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
                claimed.push(t);
            }
            Ok(db_operation::ClaimResult::RuleBlocked) => {
                // Cache the blocked lock keys for this iteration so subsequent
                // tasks with the same rule+metadata combo are skipped without a DB call.
                // Only cache Concurency keys; Capacity sums change with task progress
                // and cannot be reliably cached within a loop iteration.
                for strategy in &t.start_condition.0 {
                    match strategy {
                        Strategy::Concurency(rule) => {
                            let key = db_operation::concurrency_lock_key(rule, &t.metadata);
                            ctx.ko.insert(key);
                        }
                        Strategy::Capacity(_) => {
                            // Skip: capacity sum depends on live progress, can't cache
                        }
                    }
                }
                metrics::record_task_blocked_by_concurrency();
                log::warn!("Start worker: task {} blocked by concurrency rule", t.id);
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

    // Connection is dropped here (returned to pool)
    claimed
}

/// Phase 2: Execute on_start webhooks for all claimed tasks in parallel,
/// bounded by the semaphore.
async fn webhook_phase(
    claimed_tasks: Vec<Task>,
    evaluator: &ActionExecutor,
    pool: &DbPool,
    semaphore: &Arc<tokio::sync::Semaphore>,
    dead_end_enabled: bool,
) -> usize {
    if claimed_tasks.is_empty() {
        return 0;
    }

    let mut join_set = JoinSet::new();

    for t in claimed_tasks {
        // Acquire permit BEFORE spawning: bounds live spawned tasks to
        // semaphore capacity, preventing unbounded memory growth from
        // eagerly-spawned futures sitting in the JoinSet.
        let permit = Arc::clone(semaphore)
            .acquire_owned()
            .await
            .expect("semaphore should not be closed");

        let pool = pool.clone();
        let evaluator = evaluator.clone();

        join_set.spawn(async move {
            let _permit = permit; // released on drop
            let _guard = metrics::WebhooksInFlightGuard::new("start");
            execute_webhook_for_task(&evaluator, t, &pool, dead_end_enabled).await
        });
    }

    let mut tasks_processed = 0usize;
    while let Some(result) = join_set.join_next().await {
        match result {
            Ok(true) => tasks_processed += 1,
            Ok(false) => {}
            Err(e) => {
                log::error!("Start worker: webhook task panicked: {:?}", e);
            }
        }
    }

    tasks_processed
}

/// Execute the full webhook lifecycle for a single claimed task:
/// 1. start_task (on_start webhooks)
/// 2. mark_task_running
/// 3. save_cancel_actions
/// On failure: fail_task_and_propagate
///
/// Returns true if the task was successfully started.
async fn execute_webhook_for_task(
    evaluator: &ActionExecutor,
    task: Task,
    pool: &DbPool,
    dead_end_enabled: bool,
) -> bool {
    let Ok(mut conn) = pool.get().await else {
        log::error!(
            "Start worker: failed to acquire DB connection for task {}",
            task.id
        );
        return false;
    };

    match start_task(evaluator, &task, &mut conn).await {
        Ok(start_result) => {
            let StartTaskResult {
                cancel_tasks,
                idempotency_key,
                claimed,
            } = start_result;

            match db_operation::mark_task_running(&mut conn, &task.id).await {
                Ok(true) => {
                    metrics::record_status_transition("Claimed", "Running");
                    log::debug!("Start worker: task {} started", task.id);

                    // Save cancel actions returned by the webhook
                    if let Err(e) =
                        db_operation::save_cancel_actions(&mut conn, &task, &cancel_tasks).await
                    {
                        log::error!(
                            "Start worker: failed to save cancel actions for task {}: {:?}",
                            task.id,
                            e
                        );
                    }
                }
                Ok(false) => {
                    log::warn!(
                        "Start worker: task {} no longer claimed; skipping running transition",
                        task.id
                    );
                }
                Err(e) => {
                    log::error!(
                        "Start worker: failed to mark task {} as running: {:?}",
                        task.id,
                        e
                    );
                }
            }

            if claimed {
                if let Err(e) =
                    db_operation::complete_webhook_execution(&mut conn, &idempotency_key, true)
                        .await
                {
                    log::error!(
                        "Failed to complete webhook execution record for key {}: {}",
                        idempotency_key,
                        e
                    );
                }
            }

            true
        }
        Err(e) => {
            // Webhook failed after claim -> mark task as failed,
            // propagate to children, and fire on_failure webhooks
            log::error!(
                "Start worker: on_start webhook failed for task {}: {:?}",
                task.id,
                e
            );
            if let Err(e2) = db_operation::fail_task_and_propagate(
                evaluator,
                &mut conn,
                &task.id,
                "on_start webhook failed",
                dead_end_enabled,
            )
            .await
            {
                log::error!(
                    "Start worker: failed to mark task {} as failed and propagate: {:?}",
                    task.id,
                    e2
                );
            }
            false
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
        Strategy::Capacity(_) => false, // Can't prefilter: sum depends on live progress
    })
}

async fn start_task<'a>(
    evaluator: &ActionExecutor,
    task: &Task,
    conn: &mut Conn<'a>,
) -> Result<StartTaskResult, String> {
    use crate::schema::action::dsl::*;

    // Idempotency guard: claim the start trigger slot
    let key = idempotency_key(task.id, &TriggerKind::Start, &TriggerCondition::Success);
    let claimed = db_operation::try_claim_webhook_execution(
        conn,
        task.id,
        TriggerKind::Start,
        TriggerCondition::Success,
        &key,
        Some(evaluator.ctx.webhook_idempotency_timeout),
    )
    .await
    .map_err(|e| format!("Failed to claim webhook execution: {}", e))?;

    if !claimed {
        log::info!(
            "Start worker: skipping on_start webhooks for task {} — already executed (key={})",
            task.id,
            key
        );
        metrics::record_webhook_idempotent_skip("start");
        metrics::record_webhook_idempotent_conflict();
        return Ok(StartTaskResult {
            cancel_tasks: vec![],
            idempotency_key: key,
            claimed: false,
        });
    }

    let actions = Action::belonging_to(&task)
        .filter(trigger.eq(TriggerKind::Start))
        .load::<Action>(conn)
        .await
        .map_err(|e| e.to_string())?;
    let mut tasks = vec![];
    let mut errors: Vec<String> = Vec::new();
    for act in actions.iter() {
        let res = evaluator.execute(act, task, Some(&key)).await;
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

    let succeeded = errors.is_empty();
    if !succeeded {
        if let Err(e) = db_operation::complete_webhook_execution(conn, &key, false).await {
            log::error!(
                "Failed to complete webhook execution record for key {}: {}",
                key,
                e
            );
        }
        return Err(format!(
            "one or more on_start actions failed for task {}: {}",
            task.id,
            errors.join("; ")
        ));
    }
    Ok(StartTaskResult {
        cancel_tasks: tasks,
        idempotency_key: key,
        claimed: true,
    })
}
