// use crate::actions::ActionKindEnum;
use diesel::{prelude::*, sql_types::Text};
use serde::{Deserialize, Serialize};


#[derive(Identifiable, Queryable, Selectable, Serialize, Debug)]
#[diesel(table_name = crate::schema::task)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct Task {
    pub id: uuid::Uuid,
    pub name: String,
    pub kind: String,
    pub status: String,
    pub timeout: i32,
    // failures -> int as counter
    // succeses -> int as counter
    // metadata as JSONB
}

#[derive(Serialize, Debug)]
pub struct FullTask {
    pub task: Task,
    pub actions: Vec<Action>,
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
    // pub params: serde_json::Value,
    // TODO: add params
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

#[derive(Queryable, Associations, Selectable, PartialEq, Debug, Serialize, Insertable)]
#[diesel(
    table_name = crate::schema::action, 
    belongs_to(Task), 
    check_for_backend(diesel::pg::Pg))
]
pub struct NewAction {
    pub task_id: uuid::Uuid,
    pub kind: ActionKindEnum,
    // pub params: serde_json::Value,
    // TODO: add params
}

#[derive(Debug,PartialEq, Serialize, diesel_derive_enum::DbEnum, Deserialize, Clone)]
#[db_enum(existing_type_path = "crate::schema::sql_types::ActionKind")]
pub enum ActionKindEnum {
    Webhook
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