use diesel::prelude::*;
use uuid::Uuid;

use crate::models::{self, Action, FullTask, Task};

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
                actions: actions,
            };
            Ok(Some(res))
        }
    }
}

/// Run query using Diesel to insert a new database row and return the result.
pub fn insert_new_user(
    conn: &mut PgConnection,
    new_task: models::NewTask, // prevent collision with `name` column imported inside the function
) -> Result<models::Task, DbError> {
    // It is common when using Diesel with Actix Web to import schema-related
    // modules inside a function's scope (rather than the normal module's scope)
    // to prevent import collisions and namespace pollution.
    use crate::schema::task::dsl::*;

    let res = diesel::insert_into(task)
        .values(&new_task)
        .returning(Task::as_returning())
        .get_result(conn)?;

    Ok(res)
}
