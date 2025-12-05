use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::{
    models::{ActionKindEnum, StatusKind, TriggerKind},
    rule::{Matcher, Rules},
};

// Note: TriggerKind is only used in ActionDto (output), not NewActionDto (input)

#[derive(Debug, Serialize, Deserialize)]
pub struct NewTaskDto {
    // Local id used in order to handle dependencies
    // set by the client, only used to be a local graph (local to the query)
    pub id: String,

    pub name: String,

    pub kind: String,
    pub timeout: Option<i32>,
    pub metadata: Option<serde_json::Value>,
    /// if a task matches one of the matcher then the task is not created
    pub dedupe_strategy: Option<Vec<Matcher>>,

    pub rules: Option<Rules>,

    pub on_start: NewActionDto,

    pub dependencies: Option<Vec<Dependency>>,

    pub on_failure: Option<Vec<NewActionDto>>,
    pub on_success: Option<Vec<NewActionDto>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Dependency {
    pub id: String,
    pub requires_success: bool,
}

// only one action per tas

#[derive(Debug, Serialize, Deserialize)]
pub struct BasicTaskDto {
    pub id: uuid::Uuid,
    pub name: String,
    pub kind: String,
    pub status: StatusKind,
    pub created_at: chrono::DateTime<Utc>,
    pub started_at: Option<chrono::DateTime<Utc>>,
    pub success: i32,
    pub failures: i32,
    pub ended_at: Option<chrono::DateTime<Utc>>,
    pub batch_id: Option<uuid::Uuid>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateTaskDto {
    pub metadata: Option<serde_json::Value>,
    pub status: Option<StatusKind>,
    pub new_success: Option<i32>,
    pub new_failures: Option<i32>,
    pub failure_reason: Option<String>,
}

/// Input DTO for creating actions - trigger is determined by context (on_start, on_success, etc.)
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct NewActionDto {
    pub kind: ActionKindEnum,
    pub params: serde_json::Value,
}

/// Output DTO for returning actions - includes trigger from database
#[derive(Debug, Serialize, Deserialize, Clone)]
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
    pub failure_reason: Option<String>,
    pub batch_id: Option<uuid::Uuid>,
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
    pub metadata: Option<String>,
    pub batch_id: Option<uuid::Uuid>,
}

// on_start -> one
// on_success -> many
// on_failure -> many

/// Link between tasks for DAG visualization
#[derive(Debug, Serialize, Deserialize)]
pub struct LinkDto {
    pub parent_id: uuid::Uuid,
    pub child_id: uuid::Uuid,
    pub requires_success: bool,
}

/// DAG response containing tasks and their links for visualization
#[derive(Debug, Serialize, Deserialize)]
pub struct DagDto {
    pub tasks: Vec<BasicTaskDto>,
    pub links: Vec<LinkDto>,
}
