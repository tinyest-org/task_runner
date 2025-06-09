use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::{
    models::{ActionKindEnum, StatusKind, TriggerKind},
    rule::Rules,
};


#[derive(Debug, Serialize, Deserialize)]
pub struct NewTaskDto {
    pub name: String,
    pub kind: String,
    pub timeout: Option<i32>,
    pub actions: Option<Vec<ActionDto>>,
    pub metadata: Option<serde_json::Value>,
    pub rules: Option<Rules>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BasicTaskDto {
    pub name: String,
    pub kind: String,
    pub status: StatusKind,
    pub created_at: chrono::DateTime<Utc>,
    pub ended_at: Option<chrono::DateTime<Utc>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateTaskDto {
    pub metadata: Option<serde_json::Value>,
    pub status: Option<StatusKind>,
    pub new_success: Option<i32>,
    pub new_failures: Option<i32>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ActionDto {
    pub kind: ActionKindEnum,
    pub trigger: TriggerKind,
    pub params: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TaskDto {
    pub id: uuid::Uuid,
    pub name: String,
    pub kind: String,
    pub status: StatusKind,
    pub timeout: i32,
    pub rules: Rules,
    pub metadata: serde_json::Value,
    pub actions: Vec<ActionDto>,
    pub created_at: chrono::DateTime<Utc>,
    pub started_at: Option<chrono::DateTime<Utc>>,
    pub ended_at: Option<chrono::DateTime<Utc>>,
    pub last_updated: chrono::DateTime<Utc>,
    pub success: i32,
    pub failures: i32,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct PaginationDto {
    pub page: Option<i64>,
    pub page_size: Option<i64>,
}
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct FilterDto {
    pub name: Option<String>,
    pub kind: Option<String>,
    pub status: Option<StatusKind>,
    pub timeout: Option<i32>,
}
