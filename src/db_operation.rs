use diesel::prelude::*;

use uuid::Uuid;

use crate::{
    dtos,
    models::{self, Action, FullTask, NewAction, Task},
};

type DbError = Box<dyn std::error::Error + Send + Sync>;

/// Run query using Diesel to find user by uid and return it.
pub fn find_user_task_by_id(
    conn: &mut PgConnection,
    task_id: Uuid,
) -> Result<Option<models::FullTask>, DbError> {
    use crate::schema::task::dsl::*;

    let t = task
        .filter(id.eq(task_id))
        .first::<models::Task>(conn)
        .optional()?;
    match t {
        None => Ok(None),
        Some(base_task) => {
            let actions = Action::belonging_to(&base_task).load::<Action>(conn)?;
            let res = FullTask {
                task: base_task,
                actions,
            };
            Ok(Some(res))
        }
    }
}

/// Run query using Diesel to insert a new database row and return the result.
pub fn insert_new_task(
    conn: &mut PgConnection,
    dto: dtos::NewTaskDto, // prevent collision with `name` column imported inside the function
) -> Result<FullTask, DbError> {
    // It is common when using Diesel with Actix Web to import schema-related
    // modules inside a function's scope (rather than the normal module's scope)
    // to prevent import collisions and namespace pollution.
    use crate::schema::action::dsl::action;
    use crate::schema::task::dsl::task;
    let new_task = models::NewTask {
        name: dto.name,
        kind: dto.kind,
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
    let r = FullTask {
        task: new_task,
        actions,
    };
    Ok(r)
}
