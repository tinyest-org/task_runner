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
pub struct ActionDto {
    pub name: String,
    pub kind: ActionKindEnum,
    pub params: serde_json::Value,
}