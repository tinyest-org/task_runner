
use chrono::Utc;
// use crate::actions::ActionKindEnum;
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

use crate::rule::Rules;


#[derive(Identifiable, Queryable, Selectable, Serialize, Debug)]
#[diesel(table_name = crate::schema::task)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct Task {
    pub id: uuid::Uuid,
    pub name: Option<String>,
    pub kind: String,
    pub status: StatusKind,
    pub created_at: chrono::DateTime<Utc>,
    pub timeout: i32,
    pub started_at: Option<chrono::DateTime<Utc>>,
    pub last_updated: chrono::DateTime<Utc>,
    pub success: i32,
    pub failures: i32,
    pub metadata: serde_json::Value,
    pub ended_at: Option<chrono::DateTime<Utc>>,
    pub start_condition: Rules,
    pub failure_reason: Option<String>,
    /// We use this field to know, if the task shall be evaluated for startup
    /// 
    /// for example, if the task should run only after other tasks are done, 
    /// 
    /// then no need to evaluate all rules
    pub startable: bool,

    /// count parent_success to know if we can move to the startable state
    pub parent_success: i32,
    /// count parent_failures to know if we can move to the startable state
    pub parent_failures: i32,
}

#[derive(Identifiable, Queryable, Associations, Selectable, PartialEq, Debug, Serialize)]
#[diesel(
    table_name = crate::schema::action, 
    belongs_to(Task), 
    check_for_backend(diesel::pg::Pg))
]
pub struct Action {
    pub id: uuid::Uuid,
    pub task_id: uuid::Uuid,
    pub kind: ActionKindEnum,
    pub params: serde_json::Value,
    pub trigger: TriggerKind,
    pub success: Option<bool>,
}


/// New user details.
#[derive(Debug, Clone, Serialize, Deserialize, Insertable)]
#[diesel(table_name = crate::schema::task)]
pub struct NewTask {
    pub name: String,
    pub kind: String,
    pub status: StatusKind,
    pub timeout: i32,
    pub metadata: serde_json::Value,
    pub start_condition: Rules,
}

#[derive(Associations, PartialEq, Debug, Serialize, Insertable)]
#[diesel(
    table_name = crate::schema::action, 
    belongs_to(Task), 
    check_for_backend(diesel::pg::Pg))
]
pub struct NewAction<'a> {
    pub task_id: uuid::Uuid,
    pub kind: &'a ActionKindEnum,
    pub params: serde_json::Value,
    pub trigger: &'a TriggerKind,
}

// impl NewAction {
//     pub fn new(task_id: uuid::Uuid, kind: ActionKindEnum, params: serde_json::Value, trigger: TriggerKind) -> Self {
//         // should validate the params against the kind
//         Self {
//             task_id,
//             kind,
//             params,
//             trigger,
//         }
//     }
// }

#[derive(Debug,PartialEq, Serialize, diesel_derive_enum::DbEnum, Deserialize, Clone)]
#[db_enum(existing_type_path = "crate::schema::sql_types::ActionKind")]
pub enum ActionKindEnum {
    Webhook
}

#[derive(Debug,PartialEq, Serialize, diesel_derive_enum::DbEnum, Deserialize, Clone)]
#[db_enum(existing_type_path = "crate::schema::sql_types::TriggerKind")]
pub enum TriggerKind {
    Start, End, Cancel
}

#[derive(Debug,PartialEq, Serialize, diesel_derive_enum::DbEnum, Deserialize, Clone, Hash, Eq)]
#[db_enum(existing_type_path = "crate::schema::sql_types::StatusKind")]
pub enum StatusKind {
    Pending, Running, Failure, Success, Paused
}

