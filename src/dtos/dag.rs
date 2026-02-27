use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::task::BasicTaskDto;

/// A directed dependency link between two tasks in a DAG.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct LinkDto {
    /// UUID of the parent (upstream) task.
    pub parent_id: uuid::Uuid,
    /// UUID of the child (downstream) task that depends on the parent.
    pub child_id: uuid::Uuid,
    /// If true, parent must succeed for child to proceed. If parent fails, child is also marked as Failure.
    pub requires_success: bool,
}

/// Complete DAG structure for a batch — all tasks and their dependency links.
/// Returned by `GET /dag/{batch_id}`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct DagDto {
    /// All tasks in this batch.
    pub tasks: Vec<BasicTaskDto>,
    /// Dependency links (parent -> child edges in the DAG).
    pub links: Vec<LinkDto>,
}
