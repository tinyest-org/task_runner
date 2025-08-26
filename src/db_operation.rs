use crate::{
    Conn,
    action::ActionExecutor,
    dtos::{self, ActionDto, TaskDto},
    models::{self, Action, NewAction, Task},
    rule::{Matcher, Rules},
    workers::end_task,
};
use diesel::prelude::*;
use diesel::sql_types;
use diesel_async::RunQueryDsl;
use serde_json::{Value, json};
use uuid::Uuid;
pub type DbError = Box<dyn std::error::Error + Send + Sync>;

/// TaskDto is a data transfer object that represents a task with its actions.
impl TaskDto {
    pub fn new(base_task: Task, actions: Vec<Action>) -> Self {
        Self {
            id: base_task.id,
            name: base_task.name.unwrap_or("".to_string()),
            kind: base_task.kind,
            status: base_task.status,
            timeout: base_task.timeout,
            rules: base_task.start_condition,
            metadata: base_task.metadata,
            created_at: base_task.created_at,
            success: base_task.success,
            failures: base_task.failures,
            ended_at: base_task.ended_at,
            last_updated: base_task.last_updated,
            started_at: base_task.started_at,
            failure_reason: base_task.failure_reason,
            actions: actions
                .iter()
                .map(|a| dtos::ActionDto {
                    kind: a.kind.clone(),
                    params: a.params.clone(),
                    trigger: a.trigger.clone(),
                })
                .collect(),
        }
    }
}

/// Update all tasks with status running and last_updated older than timeout to failed and update
/// the ended_at field to the current time.
pub async fn ensure_pending_tasks_timeout<'a>(conn: &mut Conn<'a>) -> Result<Vec<Task>, DbError> {
    use {
        crate::schema::task::dsl::*,
        diesel::{dsl::now, pg::data_types::PgInterval},
    };
    let updated = diesel::update(
        task.filter(
            status
                .eq(models::StatusKind::Running)
                .and(started_at.is_not_null())
                .and(last_updated.lt(now.into_sql::<sql_types::Timestamptz>()
                    - (PgInterval::from_microseconds(1_000_000).into_sql::<sql_types::Interval>()
                        * timeout))),
        ),
    )
    .set((
        status.eq(models::StatusKind::Failure),
        ended_at.eq(now),
        failure_reason.eq("Timeout"),
    ))
    .returning(Task::as_returning())
    .get_results::<Task>(conn)
    .await?;
    Ok(updated)
}

pub async fn list_all_pending<'a>(conn: &mut Conn<'a>) -> Result<Vec<Task>, DbError> {
    use crate::schema::task::dsl::*;
    let tasks = task
        .filter(status.eq(models::StatusKind::Pending))
        .order(created_at.asc())
        .get_results(conn)
        .await?;
    Ok(tasks)
}
pub async fn update_task<'a>(
    evaluator: &ActionExecutor,
    conn: &mut Conn<'a>,
    task_id: Uuid,
    dto: dtos::UpdateTaskDto,
) -> Result<usize, DbError> {
    use crate::schema::task::dsl::*;
    log::debug!("Update task: {:?}", &dto);
    // TODO: make cleaner, ensure we only have failure or success
    // can't change to the other kind -> reserved for internal use
    if let Some(s) = &dto.status {
        if s == &models::StatusKind::Failure || s == &models::StatusKind::Success {
        } else {
            return Ok(2);
        }
    }
    let is_end = dto.status.is_some();
    let s = dto.status.clone();
    let is_failed = dto
        .status
        .as_ref()
        .map(|s| s == &models::StatusKind::Failure)
        .unwrap_or(false);
    if dto.failure_reason.is_some() && !is_failed {
        return Ok(2);
    }
    let res = diesel::update(
        task.filter(
            id.eq(task_id)
                // lock failed tasks for update
                .and(status.ne(models::StatusKind::Failure)),
        ),
    )
    .set((
        last_updated.eq(diesel::dsl::now),
        dto.new_success.map(|e| success.eq(success + e)),
        dto.new_failures.map(|e| failures.eq(failures + e)),
        // if success or failure then update ended_at
        s.filter(|e| e == &models::StatusKind::Success || e == &models::StatusKind::Failure)
            .map(|_| ended_at.eq(diesel::dsl::now)),
        dto.metadata.map(|m| metadata.eq(m)),
        dto.status.map(|m| status.eq(m)),
        dto.failure_reason.map(|m| failure_reason.eq(m)),
    ))
    .execute(conn)
    .await?;

    if is_end {
        // TODO: execute on end action triggers
        match end_task(evaluator, &task_id, conn).await {
            Ok(_) => log::debug!("task {} end actions are successfull", &task_id),
            Err(_) => log::error!("task {} end actions failed", &task_id),
        }
    }

    Ok(res)
}

/// Run query using Diesel to find user by uid and return it.
pub async fn find_detailed_task_by_id<'a>(
    conn: &mut Conn<'a>,
    task_id: Uuid,
) -> Result<Option<dtos::TaskDto>, DbError> {
    use crate::schema::task::dsl::*;

    let t = task
        .filter(id.eq(task_id))
        .first::<models::Task>(conn)
        .await;
    match t {
        Err(_) => Ok(None),
        Ok(base_task) => {
            let actions = Action::belonging_to(&base_task)
                .load::<Action>(conn)
                .await?;
            Ok(Some(TaskDto::new(base_task, actions)))
        }
    }
}

pub async fn list_task_filtered_paged<'a>(
    conn: &mut Conn<'a>,
    pagination: dtos::PaginationDto,
    filter: dtos::FilterDto,
) -> Result<Vec<dtos::BasicTaskDto>, DbError> {
    use crate::schema::task::dsl::*;
    let m = filter
        .metadata
        .map(|f| serde_json::from_str::<Value>(&f).ok())
        .flatten();
    let page_size = 50;
    let offset = pagination.page.unwrap_or(0) * page_size;
    let result = if let Some(dto_status) = filter.status {
        task.offset(offset)
            .filter(
                name.like(format!("%{}%", filter.name.unwrap_or("".to_string())))
                    .and(kind.like(format!("%{}%", filter.kind.unwrap_or("".to_string()))))
                    .and(status.eq(dto_status))
                    .and(metadata.contains(m.unwrap_or(json!({})))),
            )
            .limit(page_size)
            .order(created_at.desc())
            .load::<models::Task>(conn)
            .await?
    } else {
        task.offset(offset)
            .filter(
                name.like(format!("%{}%", filter.name.unwrap_or("".to_string())))
                    .and(kind.like(format!("%{}%", filter.kind.unwrap_or("".to_string()))))
                    .and(metadata.contains(m.unwrap_or(json!({})))),
            )
            .limit(page_size)
            .order(created_at.desc())
            .load::<models::Task>(conn)
            .await?
    };

    let tasks: Vec<dtos::BasicTaskDto> = result
        .into_iter()
        .map(|base_task| dtos::BasicTaskDto {
            id: base_task.id,
            name: base_task.name.unwrap_or_default(),
            kind: base_task.kind,
            status: base_task.status,
            created_at: base_task.created_at,
            ended_at: base_task.ended_at,
            started_at: base_task.started_at,
            success: base_task.success,
            failures: base_task.failures,
        })
        .collect();

    Ok(tasks)
}

/// ensure we avoid creating duplicate tasks
async fn handle_dedupe<'a>(
    conn: &mut Conn<'a>,
    rules: Vec<Matcher>,
    _metadata: &Option<serde_json::Value>,
) -> bool {
    for matcher in rules.iter() {
        use crate::schema::task::dsl::*;
        use diesel::PgJsonbExpressionMethods;
        let mut m = json!({});
        if let Some(_m) = _metadata {
            matcher.fields.iter().for_each(|e| {
                let k = _m.get(e);
                match k {
                    Some(v) => {
                        m[e] = v.clone();
                    }
                    None => unreachable!("None should't be there"),
                }
            });
        }
        let count = task
            .filter(
                kind.eq(&matcher.kind)
                    .and(status.eq(&matcher.status))
                    .and(metadata.contains(m)),
            )
            .count()
            .get_result::<i64>(conn)
            .await
            .expect("failed to count for execution");
        if count > 0 {
            return false;
        }
    }
    true
}

async fn insert_actions<'a>(
    task_id: Uuid,
    actions: &[ActionDto],
    trigger: Option<&models::TriggerKind>,
    conn: &mut Conn<'a>,
) -> Result<Vec<Action>, DbError> {
    use crate::schema::action::dsl::action;
    if actions.is_empty() {
        return Ok(vec![]);
    }
    let items = actions
        .iter()
        .map(|a| NewAction {
            task_id,
            kind: &a.kind,
            params: a.params.clone(),
            trigger: trigger.unwrap_or(&a.trigger),
        })
        .collect::<Vec<_>>();

    let r = diesel::insert_into(action)
        .values(items)
        .returning(Action::as_returning())
        .get_results(conn)
        .await?;
    Ok(r)
}

/// Insert a new task into the database.
pub async fn insert_new_task<'a>(
    conn: &mut Conn<'a>,
    dto: dtos::NewTaskDto,
) -> Result<Option<TaskDto>, DbError> {
    use crate::schema::task::dsl::task;

    let should_write = if let Some(s) = dto.dedupe_strategy {
        handle_dedupe(conn, s, &dto.metadata).await
    } else {
        true
    };

    if !should_write {
        return Ok(None);
    }

    let new_task = models::NewTask {
        name: dto.name,
        kind: dto.kind,
        status: models::StatusKind::Pending,
        timeout: dto.timeout.unwrap_or(60),
        metadata: dto.metadata.unwrap_or(serde_json::Value::Null),
        start_condition: dto.rules.unwrap_or(Rules { conditions: vec![] }),
    };

    let new_task = diesel::insert_into(task)
        .values(new_task)
        .returning(Task::as_returning())
        .get_result(conn)
        .await?;

    let actions = if let Some(actions) = dto.actions {
        insert_actions(new_task.id, &actions, None, conn).await?
    } else {
        vec![]
    };
    Ok(Some(TaskDto::new(new_task, actions))) // Changed from Ok(r) to Ok(dto)
}

// pub async fn set_started_task<'a>(
//     conn: &mut Conn<'a>,
//     t: &Task,
//     cancel_tasks: &[ActionDto],
// ) -> Result<(), DbError> {
//     use crate::schema::task::dsl::*;
//     use diesel::dsl::{case_when, now, sql};
//     use crate::schema::task::status as task_status; // Alias the column for clarity in sql
//     // TODO: save cancel task and bind to task
//     diesel::update(task.filter(id.eq(t.id)))
//         .set((
//             // Use diesel::dsl::sql for the conditional logic
//             task_status.eq(sql("CASE WHEN task.status = 'Pending' THEN 'Running' ELSE task.status END")),
//             // These fields are always updated
//             started_at.eq(now),
//             last_updated.eq(now),
//         ))
//         .execute(conn)
//         .await?;

//     insert_actions(
//         t.id,
//         &cancel_tasks,
//         Some(&models::TriggerKind::Cancel),
//         conn,
//     )
//     .await?;
//     Ok(())
// }




pub async fn set_started_task<'a>(
    conn: &mut Conn<'a>,
    t: &Task,
    cancel_tasks: &[ActionDto],
) -> Result<(), DbError> {
    use diesel::{ExpressionMethods, QueryDsl};
    use diesel_async::RunQueryDsl;

    use crate::schema::task::dsl::*;
    // 1. We need `sql` for the binding trick
    use diesel::dsl::{now, case_when, sql};
    use crate::schema::task::status as task_status; // Alias the column for clarity in sql
    // 2. Import the auto-generated SQL type for your enum
    use crate::schema::sql_types::StatusKind as StatusKindSql;

    // TODO: save cancel task and bind to task
    diesel::update(task.filter(id.eq(t.id)))
        .set((
            task_status.eq(sql("CASE WHEN task.status = 'Pending' THEN 'Running' ELSE task.status END")),
            // These fields are always updated
            started_at.eq(now),
            last_updated.eq(now),
        ))
        .execute(conn)
        .await?;

    insert_actions(
        t.id,
        &cancel_tasks,
        Some(&models::TriggerKind::Cancel),
        conn,
    )
    .await?;
    Ok(())
}