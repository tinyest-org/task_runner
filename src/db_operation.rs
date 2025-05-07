use diesel::prelude::*;

use uuid::Uuid;

use crate::{
    dtos::{self, TaskDto},
    models::{self, Action, NewAction, Task},
};

type DbError = Box<dyn std::error::Error + Send + Sync>;

/// TaskDto is a data transfer object that represents a task with its actions.
impl TaskDto {
    pub fn new(base_task: Task, actions: Vec<Action>) -> Self {
        Self {
            id: base_task.id,
            name: base_task.name.unwrap_or("".to_string()),
            kind: base_task.kind,
            status: base_task.status,
            timeout: base_task.timeout,
            actions: actions
                .iter()
                .map(|a| dtos::ActionDto {
                    kind: a.kind.clone(),
                    params: a.params.clone(),
                })
                .collect(),
        }
    }
}

/// Run query using Diesel to find user by uid and return it.
pub fn find_detailed_task_by_id(
    conn: &mut PgConnection,
    task_id: Uuid,
) -> Result<Option<dtos::TaskDto>, DbError> {
    use crate::schema::task::dsl::*;

    let t = task
        .filter(id.eq(task_id))
        .first::<models::Task>(conn)
        .optional()?;
    match t {
        None => Ok(None),
        Some(base_task) => {
            let actions = Action::belonging_to(&base_task).load::<Action>(conn)?;
            Ok(Some(TaskDto::new(base_task, actions)))
        }
    }
}

pub fn list_task_filtered_paged(
    conn: &mut PgConnection,
    pagination: dtos::PaginationDto,
    filter: dtos::FilterDto,
) -> Result<Vec<dtos::BasicTaskDto>, DbError> {
    use crate::schema::task::dsl::*;
    // use diesel to find the required data
    let page_size = 50;
    let offset = pagination.page.unwrap_or(0) * page_size;
    let result = if let Some(dto_status) = filter.status {
        task.offset(offset)
            .filter(
                name.like(format!("%{}%", filter.name.unwrap_or("".to_string())))
                    .and(kind.like(format!("%{}%", filter.kind.unwrap_or("".to_string()))))
                    .and(status.eq(dto_status)),
            )
            .limit(page_size)
            .order(created_at.desc())
            .load::<models::Task>(conn)?
    } else {
        task.offset(offset)
            .filter(
                name.like(format!("%{}%", filter.name.unwrap_or("".to_string())))
                    .and(kind.like(format!("%{}%", filter.kind.unwrap_or("".to_string())))),
            )
            .limit(page_size)
            .order(created_at.desc())
            .load::<models::Task>(conn)?
    };

    let tasks: Vec<dtos::BasicTaskDto> = result
        .into_iter()
        .map(|base_task| dtos::BasicTaskDto {
            name: base_task.name.unwrap_or_default(),
            kind: base_task.kind,
            status: base_task.status,
            created_at: base_task.created_at,
            ended_at: base_task.ended_at,
        })
        .collect();

    Ok(tasks)
}

/// Insert a new task into the database.
pub fn insert_new_task(
    conn: &mut PgConnection,
    dto: dtos::NewTaskDto, // prevent collision with `name` column imported inside the function
) -> Result<TaskDto, DbError> {
    // It is common when using Diesel with Actix Web to import schema-related
    // modules inside a function's scope (rather than the normal module's scope)
    // to prevent import collisions and namespace pollution.
    use crate::schema::action::dsl::action;
    use crate::schema::task::dsl::task;
    let new_task = models::NewTask {
        name: dto.name,
        kind: dto.kind,
        // TODO: could be an enum
        status: models::StatusKind::Pending,
        timeout: dto.timeout.unwrap_or(60),
    };

    let new_task = diesel::insert_into(task)
        .values(new_task)
        .returning(Task::as_returning())
        .get_result(conn)?;

    let actions = if let Some(actions) = dto.actions {
        let items = actions
            .iter()
            .map(|a| NewAction {
                task_id: new_task.id,
                kind: a.kind.clone(),
                params: a.params.clone(),
                trigger: models::TriggerKind::End,
            })
            .collect::<Vec<_>>();
        
        diesel::insert_into(action)
            .values(items)
            .returning(Action::as_returning())
            .get_results(conn)?
    } else {
        vec![]
    };
    Ok(TaskDto::new(new_task, actions)) // Changed from Ok(r) to Ok(dto)
}
