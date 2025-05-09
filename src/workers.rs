use std::{sync::Arc, thread};

use diesel::{BelongingToDsl, PgConnection, RunQueryDsl};

use crate::{db_operation, models::{self, Action, Task}, DbPool};

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
                    Ok(count) => {
                        // use logger instead of println
                        if count > 0 {  
                            log::warn!("Timeout worker: {} tasks failed", count);    
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
                        if evaluate_rule(&t) {
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
                                        .set((status.eq(models::StatusKind::Running), started_at.eq(now)))
                                        .execute(&mut conn)
                                        .expect("Failed to update task status");

                                }
                                Err(e) => {
                                    // use logger instead of println
                                    log::error!("Start worker: error starting task {}: {:?}", t.id, e);
                                }
                            }
                        } else {
                            // use logger instead of println
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

fn evaluate_rule(_task: &Task) -> bool {
    // evaluate the rule
    // if the rule is satisfied, return true
    // else return false
    true
}

fn start_task(task: &Task, conn: &mut PgConnection) -> Result<(), diesel::result::Error> {
    // execuute actions where trigger is start
    // update the task status to running
    // update the task started_at timestamp
    let actions = Action::belonging_to(&task).load::<Action>(conn)?;
    for action in actions {
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
