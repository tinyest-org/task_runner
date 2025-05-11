use std::{sync::Arc, thread};

use diesel::{BelongingToDsl, PgConnection, RunQueryDsl};
use serde_json::json;

use crate::{
    DbPool,
    db_operation::{self, DbError},
    models::{self, Action, Task, TriggerKind},
};

pub fn timeout_loop(pool: DbPool) {
    let p = Arc::from(pool);
    loop {
        // select all pending tasks
        // check if the timeout is reached
        // if so, update the task status to failed
        // and update the ended_at timestamp
        let pp = p.clone();

        // note that obtaining a connection from the pool is also potentially blocking
        let conn = pp.get();

        if let Ok(mut conn) = conn {
            let res = db_operation::ensure_pending_tasks_timeout(&mut conn);
            match res {
                Ok(failed) => {
                    // use logger instead of println
                    if failed.len() > 0 {
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

pub fn start_loop(pool: DbPool) {
    let p = Arc::from(pool);
    loop {
        // select all pending tasks
        // check if the timeout is reached
        // if so, update the task status to failed
        // and update the ended_at timestamp
        let pp = p.clone();

        // note that obtaining a connection from the pool is also potentially blocking
        let conn = pp.get();

        if let Ok(mut conn) = conn {
            let res = db_operation::list_all_pending(&mut conn);
            match res {
                Ok(tasks) => {
                    // use logger instead of println
                    log::debug!("Start worker: found {} tasks", tasks.len());
                    for t in tasks {
                        if evaluate_rules(&t, &mut conn) {
                            // start the task
                            let res = start_task(&t, &mut conn);
                            match res {
                                Ok(_) => {
                                    // use logger instead of println
                                    log::debug!("Start worker: task {} started", t.id);
                                    // update the task status to running
                                    use crate::schema::task::dsl::*;
                                    use diesel::dsl::now;
                                    use diesel::prelude::*;
                                    diesel::update(task.filter(id.eq(t.id)))
                                        .set((
                                            status.eq(models::StatusKind::Running),
                                            started_at.eq(now),
                                            last_updated.eq(now),
                                        ))
                                        .execute(&mut conn)
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

fn evaluate_rules(_task: &Task, conn: &mut PgConnection) -> bool {
    let conditions = &_task.start_condition.conditions;
    if conditions.len() == 0 {
        return true;
    }
    return conditions.iter().all(|cond| match cond {
        crate::rule::Rule::None => true,
        crate::rule::Rule::Concurency(concurency_rule) => {
            use crate::schema::task::dsl::*;
            use diesel::PgJsonbExpressionMethods;
            use diesel::prelude::*;
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
                .expect("failed to count for execution");
            log::info!("count: {}", &l.len());
            l.len() < concurency_rule.max_concurency.try_into().unwrap()
        }
    });
}

fn start_task(task: &Task, conn: &mut PgConnection) -> Result<(), diesel::result::Error> {
    let actions = Action::belonging_to(&task).load::<Action>(conn)?;
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

pub fn end_task(task_id: &uuid::Uuid, conn: &mut PgConnection) -> Result<(), DbError> {
    use {crate::schema::task::dsl::*, diesel::prelude::*};
    let t = task.filter(id.eq(task_id)).first::<Task>(conn)?;
    let actions = Action::belonging_to(&t).load::<Action>(conn)?;
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
