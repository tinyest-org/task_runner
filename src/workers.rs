use crate::{
    Conn, DbPool,
    action::ActionExecutor,
    db_operation::{self, DbError},
    dtos::ActionDto,
    models::{self, Action, StatusKind, Task, TriggerKind},
    rule::Strategy,
};
use actix_web::rt;
use diesel::BelongingToDsl;
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use serde_json::json;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, atomic::AtomicI32},
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
                        log::warn!(
                            "Timeout worker: {} tasks failed, {:?}",
                            &failed.len(),
                            &failed.iter().map(|e| e.id).collect::<Vec<_>>()
                        );
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
    'outer: loop {
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
                                            // we want to stop starting tasks as we are in a inconsistant state
                                            log::error!(
                                                "Start worker: error saving task {}: after {} retries",
                                                t.id,
                                                max_retries,
                                            );
                                            break 'outer;
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
                            log::warn!("Start worker: task {} not started", t.id);
                        }
                    }
                }
                Err(e) => {
                    log::error!("Start worker: error updating tasks: {:?}", e);
                }
            }
        }
        sleep_secs(1).await;
    }
}

pub async fn evaluate_rules<'a>(
    _task: &Task,
    conn: &mut Conn<'a>,
    ctx: &mut EvaluationContext,
) -> bool {
    let conditions = &_task.start_condition.conditions;
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
        .set(status.eq(StatusKind::Canceled))
        .execute(conn)
        .await?;
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

/// receives success / failures updat events
pub async fn batch_updater(pool: DbPool, receiver: mpsc::Receiver<UpdateEvent>) {
    let data: Arc<Mutex<HashMap<uuid::Uuid, Entry>>> = Arc::new(Mutex::new(HashMap::new()));
    let mut receiver = receiver;
    tokio::spawn({
        let data = Arc::clone(&data); // Clone the Arc before moving into the closure
        async move {
            while let Some(evt) = receiver.recv().await {
                let mut items_guard = data.lock().await;
                let e = items_guard.entry(evt.task_id).or_default();
                e.success
                    .fetch_add(evt.success, std::sync::atomic::Ordering::Relaxed);
                e.failures
                    .fetch_add(evt.failures, std::sync::atomic::Ordering::Relaxed);
            }
        }
    });
    loop {
        // push updates to db
        if let Ok(mut conn) = pool.get().await {
            let mut items_guard = data.lock().await;
            for (task_id, entry) in items_guard.drain() {
                println!("{} {:?}", &task_id, &entry);
                match handle_one(task_id, &mut conn, entry).await {
                    Ok(_) => {}
                    Err(_) => {
                        log::error!("failed to apply batch update");
                    }
                }
            }
        }
        sleep_ms(100).await;
    }
}

async fn handle_one<'a>(
    task_id: uuid::Uuid,
    conn: &mut Conn<'a>,
    entry: Entry,
) -> Result<(), DbError> {
    use crate::schema::task::dsl::*;
    let res = diesel::update(
        task.filter(
            id.eq(task_id)
                // lock failed tasks for update
                .and(status.ne(models::StatusKind::Failure)),
        ),
    )
    .set((
        last_updated.eq(diesel::dsl::now),
        success.eq(success + entry.success.into_inner()),
        failures.eq(failures + entry.failures.into_inner()),
    ))
    .execute(conn)
    .await?;

    Ok(())
}
