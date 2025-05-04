use crate::schema::task;
use serde::{Deserialize, Serialize};

#[derive(Queryable, Selectable, Serialize)]
#[diesel(table_name = crate::schema::task)]
// #[diesel(check_for_backend(diesel::pg::Pg))]
pub struct Task {
    pub id: uuid::Uuid,
    pub name: String,
    pub kind: String,
    pub status: String,
    pub timeout: i32,
}

/// New user details.
#[derive(Debug, Clone, Serialize, Deserialize, Insertable)]
#[diesel(table_name = crate::schema::task)]
pub struct NewTask {
    pub name: String,
    pub kind: String,
    pub status: String,
    pub timeout: i32,
}

impl NewTask {
    /// Constructs new user details from name.
    #[cfg(test)] // only needed in tests
    pub fn new(name: impl Into<String>, kind: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kind: kind.into(),
            timeout: 60,
            status: "waiting".to_owned(),
        }
    }
}
