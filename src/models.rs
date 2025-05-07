// use crate::actions::ActionKindEnum;
use diesel::{prelude::*, sql_types::{Text, Timestamp}};
use serde::{Deserialize, Serialize};


#[derive(Identifiable, Queryable, Selectable, Serialize, Debug)]
#[diesel(table_name = crate::schema::task)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct Task {
    pub id: uuid::Uuid,
    pub name: Option<String>,
    pub kind: String,
    pub status: StatusKind,
    pub created_at: chrono::NaiveDateTime,
    pub timeout: i32,
    pub last_updated: chrono::NaiveDateTime,
    pub success: i32,
    pub failures: i32,
    pub metadata: serde_json::Value,
    pub ended_at: Option<chrono::NaiveDateTime>,
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
}

#[derive(Queryable, Associations, Selectable, PartialEq, Debug, Serialize, Insertable)]
#[diesel(
    table_name = crate::schema::action, 
    belongs_to(Task), 
    check_for_backend(diesel::pg::Pg))
]
pub struct NewAction {
    pub task_id: uuid::Uuid,
    pub kind: ActionKindEnum,
    pub params: serde_json::Value,
    pub trigger: TriggerKind,
    // TODO: add params
}

#[derive(Debug,PartialEq, Serialize, diesel_derive_enum::DbEnum, Deserialize, Clone)]
#[db_enum(existing_type_path = "crate::schema::sql_types::ActionKind")]
pub enum ActionKindEnum {
    Webhook
}

#[derive(Debug,PartialEq, Serialize, diesel_derive_enum::DbEnum, Deserialize, Clone)]
#[db_enum(existing_type_path = "crate::schema::sql_types::TriggerKind")]
pub enum TriggerKind {
    Start, End
}

#[derive(Debug,PartialEq, Serialize, diesel_derive_enum::DbEnum, Deserialize, Clone)]
#[db_enum(existing_type_path = "crate::schema::sql_types::StatusKind")]
pub enum StatusKind {
    Pending, Running, Failure, Success
}

#[derive(Debug, Deserialize, Serialize)]
pub enum HttpVerb {
    Get, Post, Delete, Put, Patch
}

#[derive(Debug, Deserialize, Serialize)]
pub struct WebhookParamas {
    url: String,
    verb: HttpVerb,
}