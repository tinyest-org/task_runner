use crate::{
    Conn,
    action::ActionExecutor,
    dtos::{self, TaskDto},
    metrics,
    models::{self, Action, Link, NewAction, StatusKind, Task},
    rule::Matcher,
    workers::end_task,
};
use diesel::prelude::*;
use diesel::sql_types;
use diesel_async::RunQueryDsl;
use serde_json::{Value, json};
use std::collections::HashMap;
use uuid::Uuid;
pub type DbError = Box<dyn std::error::Error + Send + Sync>;

/// TaskDto is a data transfer object that represents a task with its actions.
impl TaskDto {
    pub fn new(base_task: Task, actions: Vec<Action>) -> Self {
        Self {
            id: base_task.id,
            name: base_task.name,
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
            batch_id: base_task.batch_id,
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
    const TIMEOUT_REASON: &str = "Timeout";
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
        failure_reason.eq(TIMEOUT_REASON),
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
pub async fn update_running_task<'a>(
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
    let s = dto.status.clone();
    let is_failed = dto
        .status
        .as_ref()
        .map(|s| s == &models::StatusKind::Failure)
        .unwrap_or(false);
    if dto.failure_reason.is_some() && !is_failed {
        return Ok(2);
    }

    // Fetch task kind for metrics before updating
    let task_kind = task
        .filter(id.eq(task_id))
        .select(kind)
        .first::<String>(conn)
        .await
        .ok();

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
        dto.status.as_ref().map(|m| status.eq(m)),
        dto.failure_reason.map(|m| failure_reason.eq(m)),
    ))
    .execute(conn)
    .await?;

    if let Some(ref final_status) = dto.status {
        // Record completion metrics with task kind
        let outcome = match final_status {
            models::StatusKind::Success => "success",
            models::StatusKind::Failure => "failure",
            _ => "other",
        };
        metrics::record_status_transition("Running", outcome);
        metrics::record_task_completed(outcome, task_kind.as_deref().unwrap_or("unknown"));

        // TODO: execute on end action triggers
        match end_task(evaluator, &task_id, dto.status.clone().unwrap(), conn).await {
            Ok(_) => log::debug!("task {} end actions are successfull", &task_id),
            Err(_) => log::error!("task {} end actions failed", &task_id),
        }
    }

    Ok(res)
}

/// Find a task by ID with all its actions using a single LEFT JOIN query.
/// Returns None if the task doesn't exist.
pub async fn find_detailed_task_by_id<'a>(
    conn: &mut Conn<'a>,
    task_id: Uuid,
) -> Result<Option<dtos::TaskDto>, DbError> {
    use crate::schema::action::dsl as action_dsl;
    use crate::schema::task::dsl::*;

    // Use LEFT JOIN to fetch task and actions in a single query
    let results: Vec<(models::Task, Option<Action>)> = task
        .left_join(action_dsl::action)
        .filter(id.eq(task_id))
        .load::<(models::Task, Option<Action>)>(conn)
        .await?;

    if results.is_empty() {
        return Ok(None);
    }

    // All rows have the same task, collect actions
    let base_task = results[0].0.clone();
    let actions: Vec<Action> = results.into_iter().filter_map(|(_, a)| a).collect();

    Ok(Some(TaskDto::new(base_task, actions)))
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
    let page_size = pagination.page_size.unwrap_or(50);
    let offset = pagination.page.unwrap_or(0) * page_size;

    // Build base filter
    let name_filter = name.like(format!("%{}%", filter.name.unwrap_or("".to_string())));
    let kind_filter = kind.like(format!("%{}%", filter.kind.unwrap_or("".to_string())));
    let metadata_filter = metadata.contains(m.unwrap_or(json!({})));

    let result = match (filter.status, filter.batch_id) {
        (Some(dto_status), Some(bid)) => {
            task.offset(offset)
                .filter(
                    name_filter
                        .and(kind_filter)
                        .and(status.eq(dto_status))
                        .and(metadata_filter)
                        .and(batch_id.eq(bid)),
                )
                .limit(page_size)
                .order(created_at.desc())
                .load::<models::Task>(conn)
                .await?
        }
        (Some(dto_status), None) => {
            task.offset(offset)
                .filter(
                    name_filter
                        .and(kind_filter)
                        .and(status.eq(dto_status))
                        .and(metadata_filter),
                )
                .limit(page_size)
                .order(created_at.desc())
                .load::<models::Task>(conn)
                .await?
        }
        (None, Some(bid)) => {
            task.offset(offset)
                .filter(
                    name_filter
                        .and(kind_filter)
                        .and(metadata_filter)
                        .and(batch_id.eq(bid)),
                )
                .limit(page_size)
                .order(created_at.desc())
                .load::<models::Task>(conn)
                .await?
        }
        (None, None) => {
            task.offset(offset)
                .filter(name_filter.and(kind_filter).and(metadata_filter))
                .limit(page_size)
                .order(created_at.desc())
                .load::<models::Task>(conn)
                .await?
        }
    };

    let tasks: Vec<dtos::BasicTaskDto> = result
        .into_iter()
        .map(|base_task| dtos::BasicTaskDto {
            id: base_task.id,
            name: base_task.name,
            kind: base_task.kind,
            status: base_task.status,
            created_at: base_task.created_at,
            ended_at: base_task.ended_at,
            started_at: base_task.started_at,
            success: base_task.success,
            failures: base_task.failures,
            batch_id: base_task.batch_id,
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
    actions: &[dtos::NewActionDto],
    trigger: &models::TriggerKind,
    condition: &models::TriggerCondition,
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
            trigger,
            condition,
        })
        .collect::<Vec<_>>();

    let r = diesel::insert_into(action)
        .values(items)
        .returning(Action::as_returning())
        .get_results(conn)
        .await?;
    Ok(r)
}

/// Insert a new task into the database with optional dependencies.
///
/// `id_mapping` maps local client IDs to database UUIDs for resolving dependencies
/// within the same batch of tasks.
/// `batch_id` groups all tasks created in the same request for tracing.
pub async fn insert_new_task<'a>(
    conn: &mut Conn<'a>,
    dto: dtos::NewTaskDto,
    id_mapping: &HashMap<String, Uuid>,
    batch_id: Option<Uuid>,
) -> Result<Option<TaskDto>, DbError> {
    use crate::schema::link::dsl::link;
    use crate::schema::task::dsl::task;

    let should_write = if let Some(s) = dto.dedupe_strategy {
        handle_dedupe(conn, s, &dto.metadata).await
    } else {
        true
    };

    if !should_write {
        return Ok(None);
    }

    // Compute wait counters from dependencies
    let (wait_success, wait_finished, links) = if let Some(ref deps) = dto.dependencies {
        let mut ws = 0i32;
        let mut wf = 0i32;
        let mut resolved_links = Vec::new();

        for dep in deps {
            if let Some(&parent_id) = id_mapping.get(&dep.id) {
                wf += 1;
                if dep.requires_success {
                    ws += 1;
                }
                resolved_links.push(Link {
                    parent_id,
                    child_id: Uuid::nil(), // Will be set after task creation
                    requires_success: dep.requires_success,
                });
            } else {
                log::warn!("Dependency with local id '{}' not found in mapping", dep.id);
            }
        }
        (ws, wf, resolved_links)
    } else {
        (0, 0, Vec::new())
    };

    // Set status based on whether there are dependencies
    let initial_status = if wait_finished > 0 {
        models::StatusKind::Waiting
    } else {
        models::StatusKind::Pending
    };

    let new_task = models::NewTask {
        name: dto.name,
        kind: dto.kind,
        status: initial_status,
        timeout: dto.timeout.unwrap_or(60),
        metadata: dto.metadata.unwrap_or(serde_json::Value::Null),
        start_condition: dto.rules.unwrap_or_default(),
        wait_success,
        wait_finished,
        batch_id,
    };

    let new_task = diesel::insert_into(task)
        .values(new_task)
        .returning(Task::as_returning())
        .get_result(conn)
        .await?;

    // Insert links with the actual child_id
    if !links.is_empty() {
        let links_to_insert: Vec<Link> = links
            .into_iter()
            .map(|mut l| {
                l.child_id = new_task.id;
                l
            })
            .collect();

        diesel::insert_into(link)
            .values(&links_to_insert)
            .execute(conn)
            .await?;
    }

    // Insert actions: on_start (single), on_failure (many), on_success (many)
    let mut all_actions = Vec::new();

    // Insert on_start action (condition doesn't matter for start, use Success as default)
    let start_actions = insert_actions(
        new_task.id,
        &[dto.on_start],
        &models::TriggerKind::Start,
        &models::TriggerCondition::Success,
        conn,
    )
    .await?;
    all_actions.extend(start_actions);

    // Insert on_failure actions
    if let Some(failure_actions) = dto.on_failure {
        let inserted = insert_actions(
            new_task.id,
            &failure_actions,
            &models::TriggerKind::End,
            &models::TriggerCondition::Failure,
            conn,
        )
        .await?;
        all_actions.extend(inserted);
    }

    // Insert on_success actions
    if let Some(success_actions) = dto.on_success {
        let inserted = insert_actions(
            new_task.id,
            &success_actions,
            &models::TriggerKind::End,
            &models::TriggerCondition::Success,
            conn,
        )
        .await?;
        all_actions.extend(inserted);
    }

    // Record metrics
    metrics::record_task_created();
    if wait_finished > 0 {
        metrics::record_task_with_dependencies();
    }

    Ok(Some(TaskDto::new(new_task, all_actions)))
}

pub async fn set_started_task<'a>(
    conn: &mut Conn<'a>,
    t: &Task,
    cancel_tasks: &[dtos::NewActionDto],
) -> Result<(), DbError> {
    use diesel::{ExpressionMethods, QueryDsl};
    use diesel_async::RunQueryDsl;

    use crate::schema::task::dsl::{id, last_updated, started_at, task};
    // 1. We need `sql` for the binding trick
    use crate::schema::task::status as task_status;
    use diesel::dsl::{now, sql}; // Alias the column for clarity in sql

    // TODO: save cancel task and bind to task
    diesel::update(task.filter(id.eq(t.id)))
        .set((
            task_status.eq(sql(
                "CASE WHEN task.status = 'pending' THEN 'running' ELSE task.status END",
            )),
            // These fields are always updated
            started_at.eq(now),
            last_updated.eq(now),
        ))
        .execute(conn)
        .await?;

    insert_actions(
        t.id,
        cancel_tasks,
        &models::TriggerKind::Cancel,
        &models::TriggerCondition::Success, // Condition doesn't matter for cancel
        conn,
    )
    .await?;
    Ok(())
}

pub async fn pause_task<'a>(task_id: &uuid::Uuid, conn: &mut Conn<'a>) -> Result<(), DbError> {
    use crate::schema::task::dsl::{id, status, task};
    diesel::update(task.filter(id.eq(task_id)))
        .set(status.eq(StatusKind::Paused))
        .execute(conn)
        .await?;
    Ok(())
}

/// Get DAG data for a batch: all tasks and their links
pub async fn get_dag_for_batch<'a>(
    conn: &mut Conn<'a>,
    bid: Uuid,
) -> Result<dtos::DagDto, DbError> {
    use crate::schema::link::dsl::link;
    use crate::schema::task::dsl::*;

    // Get all tasks in the batch
    let tasks_result = task
        .filter(batch_id.eq(bid))
        .order(created_at.asc())
        .load::<models::Task>(conn)
        .await?;

    let task_ids: Vec<Uuid> = tasks_result.iter().map(|t| t.id).collect();

    // Get all links where both parent and child are in this batch
    let links_result = link
        .filter(
            crate::schema::link::dsl::parent_id
                .eq_any(&task_ids)
                .and(crate::schema::link::dsl::child_id.eq_any(&task_ids)),
        )
        .load::<Link>(conn)
        .await?;

    let tasks_dto: Vec<dtos::BasicTaskDto> = tasks_result
        .into_iter()
        .map(|t| dtos::BasicTaskDto {
            id: t.id,
            name: t.name,
            kind: t.kind,
            status: t.status,
            created_at: t.created_at,
            ended_at: t.ended_at,
            started_at: t.started_at,
            success: t.success,
            failures: t.failures,
            batch_id: t.batch_id,
        })
        .collect();

    let links_dto: Vec<dtos::LinkDto> = links_result
        .into_iter()
        .map(|l| dtos::LinkDto {
            parent_id: l.parent_id,
            child_id: l.child_id,
            requires_success: l.requires_success,
        })
        .collect();

    Ok(dtos::DagDto {
        tasks: tasks_dto,
        links: links_dto,
    })
}
