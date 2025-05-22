use crate::{
    Conn, DbPool,
    db_operation::{self, DbError},
    models::{self, Action, Task, TriggerKind},
    rule::Rule,
};
use diesel::prelude::*;
use diesel::{BelongingToDsl};
use diesel_async::RunQueryDsl;
use serde_json::json;
use std::{collections::HashSet, sync::Arc, thread};

/// This ensures the non responding tasks are set to fail
///
/// Add the date of failure
pub async fn timeout_loop(pool: DbPool) {
    let p = Arc::from(pool);
    loop {
        // select all pending tasks
        // check if the timeout is reached
        // if so, update the task status to failed
        // and update the ended_at timestamp
        let pp = p.clone();

        // note that obtaining a connection from the pool is also potentially blocking
        let conn = pp.get();

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

        thread::sleep(std::time::Duration::from_secs(1));
    }
}

/// In order to cache results and avoid too many db calls
pub struct EvaluationContext {
    ko: HashSet<Rule>,
    ok: HashSet<Rule>,
}

pub async fn start_loop(pool: DbPool) {
    let p = Arc::from(pool);
    loop {
        // select all pending tasks
        // check if the timeout is reached
        // if so, update the task status to failed
        // and update the ended_at timestamp
        let pp = p.clone();

        // note that obtaining a connection from the pool is also potentially blocking
        let conn = pp.get();

        if let Ok(mut conn) = conn.await {
            let res = db_operation::list_all_pending(&mut conn).await;
            match res {
                Ok(tasks) => {
                    let mut ctx = EvaluationContext {
                        ok: HashSet::new(),
                        ko: HashSet::new(),
                    };
                    // use logger instead of println
                    log::debug!("Start worker: found {} tasks", tasks.len());
                    for t in tasks {
                        if evaluate_rules(&t, &mut conn, &mut ctx).await {
                            match start_task(&t, &mut conn).await {
                                Ok(_) => {
                                    // use logger instead of println
                                    log::debug!("Start worker: task {} started", t.id);
                                    // update the task status to running
                                    use crate::schema::task::dsl::*;
                                    use diesel::dsl::now;
                                    diesel::update(task.filter(id.eq(t.id)))
                                        .set((
                                            status.eq(models::StatusKind::Running),
                                            started_at.eq(now),
                                            last_updated.eq(now),
                                        ))
                                        .execute(&mut conn)
                                        .await
                                        .expect("Failed to update task status");
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
                    // use logger instead of println
                    log::error!("Start worker: error updating tasks: {:?}", e);
                }
            }
        }

        thread::sleep(std::time::Duration::from_secs(1));
    }
}

async fn evaluate_rules(_task: &Task, conn: &mut Conn, ctx: &mut EvaluationContext) -> bool {
    let conditions = &_task.start_condition.conditions;
    if conditions.is_empty() {
        return true;
    }
    let ok = &mut ctx.ok;
    let ko = &mut ctx.ko;
    for cond in conditions {
        if ok.contains(cond) {
            continue;
        }
        if ko.contains(cond) {
            return false;
        }
        match cond {
            crate::rule::Rule::None => {}
            crate::rule::Rule::Concurency(concurency_rule) => {
                // cache partial result
                use crate::schema::task::dsl::*;
                use diesel::PgJsonbExpressionMethods;
                let m1 = concurency_rule.matcher.clone();
                let mut m = json!({});
                m1.fields.iter().for_each(|e| {
                    let k = _task.metadata.get(e);
                    match k {
                        Some(v) => {
                            m[e] = v.clone();
                        }
                        None => unreachable!("None should't be there"),
                    }
                });
                let l = task
                    .filter(
                        kind.eq(m1.kind)
                            .and(status.eq(m1.status))
                            .and(metadata.contains(m)),
                    )
                    .load::<Task>(conn)
                    .await
                    .expect("failed to count for execution");
                log::info!("count: {}", &l.len());
                let res = l.len() < concurency_rule.max_concurency.try_into().unwrap();
                // should use an id instead
                if res {
                    ok.insert(cond.clone());
                } else {
                    ko.insert(cond.clone());
                    return false;
                }
            }
        }
    }
    true
}

async fn start_task(task: &Task, conn: &mut Conn) -> Result<(), diesel::result::Error> {
    let actions = Action::belonging_to(&task).load::<Action>(conn).await?;
    // TODO: should be in the query directly
    for action in actions.iter().filter(|e| e.trigger == TriggerKind::Start) {
        let res = action.execute(task);
        match res {
            Ok(_) => {
                // update the action status to success
                // update the action ended_at timestamp
                log::debug!("Action {} executed successfully", action.id);
            }
            Err(e) => {
                // update the action status to failure
                // update the action ended_at timestamp
                log::error!("Action {} failed: {}", action.id, e);
            }
        }
    }
    Ok(())
}

pub async fn end_task(task_id: &uuid::Uuid, conn: &mut Conn) -> Result<(), DbError> {
    use crate::schema::task::dsl::*;
    let t = task.filter(id.eq(task_id)).first::<Task>(conn).await?;
    let actions = Action::belonging_to(&t).load::<Action>(conn).await?;
    for action in actions.iter().filter(|e| e.trigger == TriggerKind::End) {
        let res = action.execute(&t);
        match res {
            Ok(_) => {
                // update the action status to success
                // update the action ended_at timestamp
                log::debug!("Action {} executed successfully", action.id);
            }
            Err(e) => {
                // update the action status to failure
                // update the action ended_at timestamp
                log::error!("Action {} failed: {}", action.id, e);
            }
        }
    }
    Ok(())
}
