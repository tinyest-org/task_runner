use crate::{
    Conn, DbPool,
    action::ActionExecutor,
    config::RetentionConfig,
    db_operation::{self, DbError},
    dtos::NewActionDto,
    metrics,
    models::{self, Action, StatusKind, Task, TriggerCondition, TriggerKind},
    rule::Strategy,
};
use actix_web::rt;
use dashmap::DashMap;
use diesel::BelongingToDsl;
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use std::{
    collections::HashSet,
    sync::{Arc, atomic::AtomicI32, atomic::Ordering},
};
use tokio::sync::{mpsc, watch};

/// This ensures the non responding tasks are set to fail
///
/// Add the date of failure
pub async fn timeout_loop(
    evaluator: Arc<ActionExecutor>,
    pool: DbPool,
    interval: std::time::Duration,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        // note that obtaining a connection from the pool is also potentially blocking
        let conn = pool.get();

        if let Ok(mut conn) = conn.await {
            let res = db_operation::ensure_pending_tasks_timeout(&mut conn).await;
            match res {
                Ok(failed) => {
                    // use logger instead of println
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
                    // use logger instead of println
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
                    let mut ctx = EvaluationContext {
                        // ok: HashSet::new(),
                        ko: HashSet::new(),
                    };
                    // use logger instead of println
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
                                // Task claimed, now execute the on_start webhook
                                match start_task(evaluator, &t, &mut conn).await {
                                    Ok(cancel_tasks) => {
                                        tasks_processed += 1;
                                        metrics::record_status_transition("Pending", "Running");
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
                // update the action status to success
                // update the action ended_at timestamp
                if let Some(t) = r {
                    tasks.push(t);
                };
                log::debug!("Action {} executed successfully", act.id);
            }
            Err(e) => {
                // update the action status to failure
                // update the action ended_at timestamp
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

pub async fn end_task<'a>(
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
                // update the action status to success
                // update the action ended_at timestamp
                log::debug!("Action {} executed successfully", act.id);
            }
            Err(e) => {
                // update the action status to failure
                // update the action ended_at timestamp
                log::error!("Action {} failed: {}", act.id, e);
            }
        }
    }

    // Propagate completion to dependent children
    propagate_to_children(task_id, &result_status, conn).await?;

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
                StatusKind::Pending | StatusKind::Paused | StatusKind::Running => {}
                _ => {
                    return Err(Box::from(
                        "Invalid operation: cannot cancel task in this state",
                    ));
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

#[derive(Debug)]
pub struct UpdateEvent {
    pub success: i32,
    pub failures: i32,
    pub task_id: uuid::Uuid,
}

#[derive(Debug, Default)]
struct Entry {
    success: AtomicI32,
    failures: AtomicI32,
}

/// Receives success/failure update events and batches them to the database.
/// Uses DashMap for lock-free concurrent access between receiver and updater.
pub async fn batch_updater(
    pool: DbPool,
    receiver: mpsc::Receiver<UpdateEvent>,
    flush_interval: std::time::Duration,
    mut shutdown: watch::Receiver<bool>,
) {
    let data: Arc<DashMap<uuid::Uuid, Entry>> = Arc::new(DashMap::new());
    let mut receiver = receiver;

    // Receiver task - continuously drains channel without blocking updater
    tokio::spawn({
        let data = Arc::clone(&data);
        async move {
            while let Some(evt) = receiver.recv().await {
                // Single entry() call holds the shard lock for both updates
                let entry = data.entry(evt.task_id).or_default();
                entry.success.fetch_add(evt.success, Ordering::Relaxed);
                entry.failures.fetch_add(evt.failures, Ordering::Relaxed);
            }
        }
    });

    // Updater loop - persists batched counts to database
    loop {
        if let Ok(mut conn) = pool.get().await {
            // Collect updates - DashMap iter() doesn't block other operations on different shards
            let updates: Vec<(uuid::Uuid, i32, i32)> = data
                .iter()
                .map(|entry| {
                    let task_id = *entry.key();
                    let s = entry.value().success.swap(0, Ordering::Relaxed);
                    let f = entry.value().failures.swap(0, Ordering::Relaxed);
                    (task_id, s, f)
                })
                .filter(|(_, s, f)| *s != 0 || *f != 0)
                .collect();

            // Process updates
            for (task_id, success_count, failure_count) in updates {
                log::debug!(
                    "Batch update for task {}: success={}, failures={}",
                    &task_id,
                    success_count,
                    failure_count
                );

                if let Err(e) =
                    handle_one_with_counts(task_id, &mut conn, success_count, failure_count).await
                {
                    log::error!(
                        "Failed to apply batch update for task {}: {:?}, re-queuing counts",
                        task_id,
                        e
                    );
                    // Re-add the counts back for retry - single entry() holds shard lock
                    let entry = data.entry(task_id).or_default();
                    entry.success.fetch_add(success_count, Ordering::Relaxed);
                    entry.failures.fetch_add(failure_count, Ordering::Relaxed);
                    metrics::record_batch_update_failure();
                }
            }

            // Cleanup zero entries periodically
            data.retain(|_, entry| {
                AtomicI32::load(&entry.success, Ordering::Relaxed) != 0
                    || AtomicI32::load(&entry.failures, Ordering::Relaxed) != 0
            });
        }
        tokio::select! {
            _ = shutdown.changed() => {
                log::info!("Batch updater: shutdown signal received, flushing remaining data");
                final_flush_batch_data(&data, &pool).await;
                log::info!("Batch updater: final flush complete, exiting");
                return;
            }
            _ = rt::time::sleep(flush_interval) => {}
        }
    }
}

/// Flush all remaining batch data to the database before shutdown.
async fn final_flush_batch_data(data: &DashMap<uuid::Uuid, Entry>, pool: &DbPool) {
    let updates: Vec<(uuid::Uuid, i32, i32)> = data
        .iter()
        .map(|e| {
            (
                *e.key(),
                e.value().success.swap(0, Ordering::Relaxed),
                e.value().failures.swap(0, Ordering::Relaxed),
            )
        })
        .filter(|(_, s, f)| *s != 0 || *f != 0)
        .collect();

    if updates.is_empty() {
        return;
    }

    log::info!(
        "Batch updater final flush: {} entries to persist",
        updates.len()
    );

    if let Ok(mut conn) = pool.get().await {
        for (task_id, s, f) in updates {
            if let Err(e) = handle_one_with_counts(task_id, &mut conn, s, f).await {
                log::error!("Final flush FAILED for task {}: {:?}", task_id, e);
            }
        }
    } else {
        log::error!("Final flush: could not acquire DB connection, data lost");
    }
}

/// Background loop that periodically deletes old terminal tasks based on retention config.
/// Returns immediately if retention is not enabled.
pub async fn retention_cleanup_loop(
    pool: DbPool,
    retention_config: RetentionConfig,
    mut shutdown: watch::Receiver<bool>,
) {
    if !retention_config.enabled {
        log::info!("Retention cleanup: disabled, exiting");
        return;
    }

    log::info!(
        "Retention cleanup: enabled, retention_days={}, interval={}s, batch_size={}",
        retention_config.retention_days,
        retention_config.cleanup_interval_secs,
        retention_config.batch_size
    );

    let interval = std::time::Duration::from_secs(retention_config.cleanup_interval_secs);

    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                log::info!("Retention cleanup: shutdown signal received, exiting");
                return;
            }
            _ = rt::time::sleep(interval) => {}
        }

        let start = std::time::Instant::now();
        match pool.get().await {
            Ok(mut conn) => {
                match db_operation::cleanup_old_terminal_tasks(
                    &mut conn,
                    retention_config.retention_days,
                    retention_config.batch_size,
                )
                .await
                {
                    Ok(count) => {
                        let duration = start.elapsed().as_secs_f64();
                        if count > 0 {
                            log::info!(
                                "Retention cleanup: deleted {} tasks in {:.2}s",
                                count,
                                duration
                            );
                        } else {
                            log::debug!("Retention cleanup: no tasks to clean up");
                        }
                        metrics::record_retention_cleanup("success", count, duration);
                    }
                    Err(e) => {
                        let duration = start.elapsed().as_secs_f64();
                        log::error!("Retention cleanup: error: {:?}", e);
                        metrics::record_retention_cleanup("error", 0, duration);
                    }
                }
            }
            Err(e) => {
                let duration = start.elapsed().as_secs_f64();
                log::error!(
                    "Retention cleanup: could not acquire DB connection: {:?}",
                    e
                );
                metrics::record_retention_cleanup("error", 0, duration);
            }
        }
    }
}

async fn handle_one_with_counts<'a>(
    task_id: uuid::Uuid,
    conn: &mut Conn<'a>,
    success_count: i32,
    failure_count: i32,
) -> Result<(), DbError> {
    use crate::schema::task::dsl::*;
    let _res = diesel::update(
        task.filter(
            id.eq(task_id)
                // Don't update failed tasks
                .and(status.ne(models::StatusKind::Failure))
                .and(status.ne(models::StatusKind::Success))
                .and(status.ne(models::StatusKind::Canceled)),
        ),
    )
    .set((
        last_updated.eq(diesel::dsl::now),
        success.eq(success + success_count),
        failures.eq(failures + failure_count),
    ))
    .execute(conn)
    .await?;

    Ok(())
}
