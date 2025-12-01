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
    pub name: String,
    pub kind: String,
    pub status: StatusKind,
    pub timeout: i32,
    pub created_at: chrono::DateTime<Utc>,
    pub started_at: Option<chrono::DateTime<Utc>>,
    pub last_updated: chrono::DateTime<Utc>,
    pub metadata: serde_json::Value,
    pub ended_at: Option<chrono::DateTime<Utc>>,
    pub start_condition: Rules,
    pub wait_success: i32,
    pub wait_finished: i32,
    pub success: i32,
    pub failures: i32,
    pub failure_reason: Option<String>,
    pub batch_id: Option<uuid::Uuid>,
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
    pub condition: TriggerCondition,
    pub success: Option<bool>,
}

/// New task details.
#[derive(Debug, Clone, Serialize, Deserialize, Insertable)]
#[diesel(table_name = crate::schema::task)]
pub struct NewTask {
    pub name: String,
    pub kind: String,
    pub status: StatusKind,
    pub timeout: i32,
    pub metadata: serde_json::Value,
    pub start_condition: Rules,
    pub wait_success: i32,
    pub wait_finished: i32,
    pub batch_id: Option<uuid::Uuid>,
}

/// Link represents a dependency between two tasks (parent -> child).
/// Used primarily for visualization/debugging. Actual execution uses
/// wait_success and wait_finished counters on the task.
#[derive(Queryable, Selectable, Insertable, Debug, Clone)]
#[diesel(table_name = crate::schema::link)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct Link {
    pub parent_id: uuid::Uuid,
    pub child_id: uuid::Uuid,
    pub requires_success: bool,
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
    pub condition: &'a TriggerCondition,
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

#[derive(Debug, PartialEq, Serialize, diesel_derive_enum::DbEnum, Deserialize, Clone)]
#[db_enum(existing_type_path = "crate::schema::sql_types::ActionKind")]
pub enum ActionKindEnum {
    Webhook,
}

#[derive(Debug, PartialEq, Serialize, diesel_derive_enum::DbEnum, Deserialize, Clone)]
#[db_enum(existing_type_path = "crate::schema::sql_types::TriggerKind")]
pub enum TriggerKind {
    Start,
    End,
    Cancel,
}

#[derive(Debug, PartialEq, Serialize, diesel_derive_enum::DbEnum, Deserialize, Clone)]
#[db_enum(existing_type_path = "crate::schema::sql_types::TriggerCondition")]
pub enum TriggerCondition {
    Success,
    Failure,
}

#[derive(Debug, PartialEq, Serialize, diesel_derive_enum::DbEnum, Deserialize, Clone, Hash, Eq)]
#[db_enum(existing_type_path = "crate::schema::sql_types::StatusKind")]
pub enum StatusKind {
    Waiting,
    Pending,
    Running,
    Failure,
    Success,
    Paused,
    Canceled,
}
