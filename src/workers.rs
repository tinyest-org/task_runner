use crate::{
    Conn, DbPool,
    action::ActionExecutor,
    db_operation::{self, DbError},
    dtos::ActionDto,
    metrics,
    models::{self, Action, StatusKind, Task, TriggerKind},
    rule::Strategy,
};
use actix_web::rt;
use dashmap::DashMap;
use diesel::BelongingToDsl;
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use serde_json::json;
use std::{
    collections::HashSet,
    sync::{Arc, atomic::AtomicI32, atomic::Ordering},
};
use tokio::sync::{Mutex, OnceCell, mpsc};

pub static GLOBAL_SENDER: OnceCell<mpsc::Sender<UpdateEvent>> = OnceCell::const_new();
pub static GLOBAL_RECEIVER: OnceCell<Mutex<mpsc::Receiver<UpdateEvent>>> = OnceCell::const_new();

/// This ensures the non responding tasks are set to fail
///
/// Add the date of failure
pub async fn timeout_loop(pool: DbPool) {
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
        rt::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

/// In order to cache results and avoid too many db calls
pub struct EvaluationContext {
    ko: HashSet<Strategy>,
    // ok: HashSet<Strategy>,
}

async fn sleep_secs(secs: u64) {
    actix_web::rt::time::sleep(std::time::Duration::from_secs(secs)).await;
}

async fn sleep_ms(ms: u64) {
    actix_web::rt::time::sleep(std::time::Duration::from_millis(ms)).await;
}

pub async fn start_loop(evaluator: &ActionExecutor, pool: DbPool) {
    loop {
        let loop_start = std::time::Instant::now();
        let mut tasks_processed = 0usize;

        // note that obtaining a connection from the pool is also potentially blocking
        let conn = pool.get();
        let max_retries = 10;
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
                        if evaluate_rules(&t, &mut conn, &mut ctx).await {
                            match start_task(evaluator, &t, &mut conn).await {
                                Ok(cancel_tasks) => {
                                    tasks_processed += 1;
                                    metrics::record_status_transition("Pending", "Running");
                                    log::debug!("Start worker: task {} started", t.id);
                                    // update the task status to running
                                    let mut i = 0;
                                    while db_operation::set_started_task(
                                        &mut conn,
                                        &t,
                                        &cancel_tasks,
                                    )
                                    .await
                                    .is_err()
                                    {
                                        log::warn!("failed to update task in database");
                                        sleep_secs(1).await;
                                        i += 1;
                                        if i == max_retries {
                                            // Skip this task and continue with others
                                            // Task will be retried on next loop iteration
                                            // (it's still in Pending state since DB update failed)
                                            log::error!(
                                                "Start worker: error saving task {}: after {} retries, skipping",
                                                t.id,
                                                max_retries,
                                            );
                                            metrics::record_task_db_save_failure();
                                            break;
                                        }
                                    }
                                }
                                Err(e) => {
                                    log::error!(
                                        "Start worker: error starting task {}: {:?}",
                                        t.id,
                                        e
                                    );
                                }
                            }
                        } else {
                            metrics::record_task_blocked_by_concurrency();
                            log::warn!("Start worker: task {} not started", t.id);
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

        sleep_secs(1).await;
    }
}

pub async fn evaluate_rules<'a>(
    _task: &Task,
    conn: &mut Conn<'a>,
    ctx: &mut EvaluationContext,
) -> bool {
    let conditions = &_task.start_condition.0;
    if conditions.is_empty() {
        return true;
    }
    // let ok = &mut ctx.ok;
    let ko = &mut ctx.ko;
    for cond in conditions {
        // for now the ok is disabled
        // as the conditions may go from ok to ko
        // after starting a previous task
        // if ok.contains(cond) {
        //     continue;
        // }
        if ko.contains(cond) {
            return false;
        }
        match cond {
            crate::rule::Strategy::Concurency(concurency_rule) => {
                // cache partial result
                use crate::schema::task::dsl::*;
                use diesel::PgJsonbExpressionMethods;
                let mut m = json!({});
                concurency_rule.matcher.fields.iter().for_each(|e| {
                    let k = _task.metadata.get(e);
                    match k {
                        Some(v) => {
                            m[e] = v.clone();
                        }
                        None => unreachable!("None should't be there"),
                    }
                });
                let count = task
                    .filter(
                        kind.eq(&concurency_rule.matcher.kind)
                            .and(status.eq(&concurency_rule.matcher.status))
                            .and(metadata.contains(m)),
                    )
                    .count()
                    .get_result::<i64>(conn)
                    .await
                    .expect("failed to count for execution");

                let is_same = concurency_rule.matcher.kind == _task.kind;

                let res = {
                    if is_same {
                        // we start the new task of the same kind, so we must ensure we don't get over capacity
                        count < concurency_rule.max_concurency.into()
                    } else {
                        count <= concurency_rule.max_concurency.into()
                    }
                };
                // should use an id instead
                if res {
                    // relica
                    // ok.insert(cond.clone());
                } else {
                    ko.insert(cond.clone());
                    return false;
                }
            }
        }
    }
    true
}

async fn start_task<'a>(
    evaluator: &ActionExecutor,
    task: &Task,
    conn: &mut Conn<'a>,
) -> Result<Vec<ActionDto>, diesel::result::Error> {
    use crate::schema::action::dsl::*;
    let actions = Action::belonging_to(&task)
        .filter(trigger.eq(TriggerKind::Start))
        .load::<Action>(conn)
        .await?;
    let mut tasks = vec![];
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
            }
        }
    }
    Ok(tasks)
}

pub async fn end_task<'a>(
    evaluator: &ActionExecutor,
    task_id: &uuid::Uuid,
    result_status: StatusKind,
    conn: &mut Conn<'a>,
) -> Result<(), DbError> {
    use crate::schema::action::dsl::trigger;
    use crate::schema::task::dsl::*;
    let t = task.filter(id.eq(task_id)).first::<Task>(conn).await?;
    let actions = Action::belonging_to(&t)
        .filter(trigger.eq(TriggerKind::End))
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

/// Propagates task completion to dependent children.
///
/// When a parent task completes:
/// 1. Decrement wait_finished for all children in Waiting status
/// 2. If parent succeeded, also decrement wait_success for children where requires_success = true
/// 3. If both counters reach 0, transition child from Waiting to Pending
/// 4. If a required parent fails, mark child as Failure
async fn propagate_to_children<'a>(
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

    for (child_id, requires_success) in children_links {
        // If parent failed and this child required success, mark child as failed
        if parent_failed && requires_success {
            let failure_reason = format!("Required parent task {} failed", parent_id);
            let updated = diesel::update(
                task_dsl::task.filter(
                    task_dsl::id
                        .eq(child_id)
                        .and(task_dsl::status.eq(StatusKind::Waiting)),
                ),
            )
            .set((
                task_dsl::status.eq(StatusKind::Failure),
                task_dsl::failure_reason.eq(failure_reason),
                task_dsl::ended_at.eq(diesel::dsl::now),
            ))
            .execute(conn)
            .await?;

            if updated > 0 {
                metrics::record_task_failed_by_dependency();
                log::info!(
                    "Child task {} marked as failed due to required parent {} failure",
                    child_id,
                    parent_id
                );

                // Recursively propagate failure to this child's dependents
                // Use Box::pin to handle async recursion
                Box::pin(propagate_to_children(&child_id, &StatusKind::Failure, conn)).await?;
            }
            continue;
        }

        // Decrement counters for children in Waiting status
        // wait_finished is always decremented
        // wait_success is only decremented if parent succeeded AND child requires success
        let decrement_wait_success: i32 = if parent_succeeded && requires_success {
            1
        } else {
            0
        };

        // Decrement counters and transition to Pending if both reach 0
        // Uses RETURNING to get new values, then conditionally updates status
        diesel::update(
            task_dsl::task.filter(
                task_dsl::id
                    .eq(child_id)
                    .and(task_dsl::status.eq(StatusKind::Waiting)),
            ),
        )
        .set((
            task_dsl::wait_finished.eq(task_dsl::wait_finished - 1),
            task_dsl::wait_success.eq(task_dsl::wait_success - decrement_wait_success),
        ))
        .execute(conn)
        .await?;

        // Atomically transition to Pending only if counters are both 0
        // The WHERE clause ensures this is safe even with concurrent updates
        let updated_count = diesel::update(
            task_dsl::task.filter(
                task_dsl::id
                    .eq(child_id)
                    .and(task_dsl::status.eq(StatusKind::Waiting))
                    .and(task_dsl::wait_finished.eq(0))
                    .and(task_dsl::wait_success.eq(0)),
            ),
        )
        .set(task_dsl::status.eq(StatusKind::Pending))
        .execute(conn)
        .await?;

        if updated_count > 0 {
            metrics::record_task_unblocked();
            metrics::record_status_transition("Waiting", "Pending");
            log::info!(
                "Child task {} transitioned from Waiting to Pending",
                child_id
            );
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
    let t = task.filter(id.eq(task_id)).first::<Task>(conn).await?;
    match t.status {
        StatusKind::Pending | StatusKind::Paused => {
            // we do nothing
        }
        StatusKind::Running => {
            // running so we try to stop using cancel actions
            let actions = Action::belonging_to(&t)
                .filter(trigger.eq(TriggerKind::Cancel))
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
        }
        _ => {
            // invalid -> return error
            return Err(Box::from(
                "Invalid operation: cannot cancel task in this state",
            ));
        }
    }
    // we update to the canceled state
    diesel::update(task.filter(id.eq(task_id)))
        .set((
            status.eq(StatusKind::Canceled),
            ended_at.eq(diesel::dsl::now),
        ))
        .execute(conn)
        .await?;

    // Propagate cancellation to dependent children
    // Canceled is treated like failure for children that require success
    propagate_to_children(task_id, &StatusKind::Canceled, conn).await?;

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
pub struct Entry {
    success: AtomicI32,
    failures: AtomicI32,
}

/// Receives success/failure update events and batches them to the database.
/// Uses DashMap for lock-free concurrent access between receiver and updater.
pub async fn batch_updater(pool: DbPool, receiver: mpsc::Receiver<UpdateEvent>) {
    let data: Arc<DashMap<uuid::Uuid, Entry>> = Arc::new(DashMap::new());
    let mut receiver = receiver;

    // Receiver task - continuously drains channel without blocking updater
    tokio::spawn({
        let data = Arc::clone(&data);
        async move {
            while let Some(evt) = receiver.recv().await {
                // DashMap allows concurrent insert/update - no global lock
                data.entry(evt.task_id)
                    .or_default()
                    .success
                    .fetch_add(evt.success, Ordering::Relaxed);
                data.entry(evt.task_id)
                    .or_default()
                    .failures
                    .fetch_add(evt.failures, Ordering::Relaxed);
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
                    // Re-add the counts back for retry - no lock needed with DashMap
                    data.entry(task_id)
                        .or_default()
                        .success
                        .fetch_add(success_count, Ordering::Relaxed);
                    data.entry(task_id)
                        .or_default()
                        .failures
                        .fetch_add(failure_count, Ordering::Relaxed);
                    metrics::record_batch_update_failure();
                }
            }

            // Cleanup zero entries periodically
            data.retain(|_, entry| {
                AtomicI32::load(&entry.success, Ordering::Relaxed) != 0
                    || AtomicI32::load(&entry.failures, Ordering::Relaxed) != 0
            });
        }
        sleep_ms(100).await;
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
                .and(status.ne(models::StatusKind::Failure)),
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
