use diesel::prelude::*;

use uuid::Uuid;

use crate::{
    dtos::{self, TaskDto},
    models::{self, Action, NewAction, Task},
};

type DbError = Box<dyn std::error::Error + Send + Sync>;

impl TaskDto {
    pub fn new(base_task: Task, actions: Vec<Action>) -> Self {
        Self {
            id: base_task.id,
            name: base_task.name,
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
pub fn find_user_task_by_id(
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
    use crate::schema::action::dsl::*;
    // use diesel to find the required data
    let page_size = 50;
    let result = task.offset(pagination.page.unwrap_or(0) * page_size)
        .limit(page_size)
        .load::<models::Task>(conn)?;

    let tasks: Vec<dtos::BasicTaskDto> = result.into_iter()
        .map(|base_task| dtos::BasicTaskDto {
            name: base_task.name,
            kind: base_task.kind,
        })
        .collect();
    
    Ok(tasks)
}

/// Run query using Diesel to insert a new database row and return the result.
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
        status: "waiting".to_owned(),
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
        let actions = diesel::insert_into(action)
            .values(items)
            .returning(Action::as_returning())
            .get_results(conn)?;
        actions
    } else {
        vec![]
    };
    Ok(TaskDto::new(new_task, actions)) // Changed from Ok(r) to Ok(dto)
}
