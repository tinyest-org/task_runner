use crate::{
    Conn, DbPool,
    action::ActionExecutor,
    db_operation::{self, DbError},
    models::{Action, Task, TriggerKind},
    rule::Strategy,
};
use actix_web::rt;
use diesel::BelongingToDsl;
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use serde_json::json;
use std::collections::HashSet;

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

pub async fn start_loop(evaluator: &ActionExecutor, pool: DbPool) {
    'outer: loop {
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
                        if evaluate_rules(&t, &mut conn, &mut ctx).await {
                            match start_task(evaluator, &t, &mut conn).await {
                                Ok(_) => {
                                    // use logger instead of println
                                    log::debug!("Start worker: task {} started", t.id);
                                    // update the task status to running
                                    let mut i = 0;
                                    while db_operation::set_started_task(&mut conn, &t)
                                        .await
                                        .is_err()
                                    {
                                        log::warn!("failed to update task in database");
                                        actix_web::rt::time::sleep(std::time::Duration::from_secs(
                                            1,
                                        ))
                                        .await;
                                        i += 1;
                                        if i == 10 {
                                            // we want to stop starting tasks as we are in a inconsistant state
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
                    // use logger instead of println
                    log::error!("Start worker: error updating tasks: {:?}", e);
                }
            }
        }

        // thread::sleep(std::time::Duration::from_secs(1));
        actix_web::rt::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

pub async fn evaluate_rules<'a>(
    _task: &Task, // in order to avoid issue when importing the schema
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

                let is_same = &concurency_rule.matcher.kind == &_task.kind;

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
) -> Result<(), diesel::result::Error> {
    let actions = Action::belonging_to(&task).load::<Action>(conn).await?;
    // TODO: should be in the query directly
    for action in actions.iter().filter(|e| e.trigger == TriggerKind::Start) {
        let res = evaluator.execute(&action, task).await;
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

pub async fn end_task<'a>(
    evaluator: &ActionExecutor,
    task_id: &uuid::Uuid,
    conn: &mut Conn<'a>,
) -> Result<(), DbError> {
    use crate::schema::task::dsl::*;
    let t = task.filter(id.eq(task_id)).first::<Task>(conn).await?;
    let actions = Action::belonging_to(&t).load::<Action>(conn).await?;
    for action in actions.iter().filter(|e| e.trigger == TriggerKind::End) {
        let res = evaluator.execute(&action, &t).await;
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
