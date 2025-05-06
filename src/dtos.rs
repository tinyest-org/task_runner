use serde::{Deserialize, Serialize};

use crate::models::ActionKindEnum;

#[derive(Debug, Serialize, Deserialize)]
pub struct NewTaskDto {
    pub name: String,
    pub kind: String,
    pub timeout: Option<i32>,
    pub actions: Option<Vec<ActionDto>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BasicTaskDto {
    pub name: String,
    pub kind: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateTaskDto {
    pub metadata: Option<serde_json::Value>,
    pub status: Option<String>,
    pub new_success: Option<i32>,
    pub new_failures: Option<i32>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ActionDto {
    pub kind: ActionKindEnum,
    pub params: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TaskDto {
    pub id: uuid::Uuid,
    pub name: String,
    pub kind: String,
    pub status: String,
    pub timeout: i32,
    pub actions: Vec<ActionDto>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PaginationDto {
    pub page: Option<i64>,
    pub page_size: Option<i64>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct FilterDto {
    pub name: Option<String>,
    pub kind: Option<String>,
    pub status: Option<String>,
    pub timeout: Option<i32>,
}