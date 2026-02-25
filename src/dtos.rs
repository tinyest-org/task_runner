use chrono::Utc;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::{
    config::Config,
    models::{Action, ActionKindEnum, StatusKind, Task, TriggerKind},
    rule::{Matcher, Rules},
};

/// Resolved pagination parameters ready for DB queries.
pub struct Pagination {
    pub offset: i64,
    pub limit: i64,
}

// =============================================================================
// Batch DTOs
// =============================================================================

/// Summary of a batch with aggregated task statistics.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct BatchSummaryDto {
    /// The batch UUID grouping tasks created in the same POST /task call.
    pub batch_id: uuid::Uuid,
    /// Total number of tasks in this batch.
    pub total_tasks: i64,
    /// When the first task in this batch was created.
    pub first_created_at: chrono::DateTime<Utc>,
    /// When the most recent task in this batch was last updated.
    pub latest_updated_at: chrono::DateTime<Utc>,
    /// Breakdown of task counts by status.
    pub status_counts: BatchStatusCounts,
    /// Distinct task kinds in this batch.
    pub kinds: Vec<String>,
}

/// Per-status task counts within a batch.
#[derive(Debug, Serialize, Deserialize, ToSchema, Default)]
pub struct BatchStatusCounts {
    pub waiting: i64,
    pub pending: i64,
    pub claimed: i64,
    pub running: i64,
    pub success: i64,
    pub failure: i64,
    pub paused: i64,
    pub canceled: i64,
}

/// Aggregated counters for a single batch.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct BatchStatsDto {
    /// The batch UUID.
    pub batch_id: uuid::Uuid,
    /// Total number of tasks in this batch.
    pub total_tasks: i64,
    /// Sum of all tasks' success counters.
    pub total_success: i64,
    /// Sum of all tasks' failure counters.
    pub total_failures: i64,
    /// Sum of all tasks' expected_count values. Null when at least one task has no expected_count set,
    /// meaning total progress cannot be meaningfully computed.
    pub total_expected: Option<i64>,
    /// Breakdown of task counts by status.
    pub status_counts: BatchStatusCounts,
}

/// Filter parameters for batch listing. All filters are optional and combined with AND logic.
/// Filters apply to tasks within each batch — a batch is included if it contains at least one
/// matching task.
#[derive(Debug, Serialize, Deserialize, Default, IntoParams)]
pub struct BatchFilterDto {
    /// Filter batches containing tasks with this kind (substring match).
    pub kind: Option<String>,
    /// Filter batches containing tasks with this status.
    pub status: Option<StatusKind>,
    /// Filter batches containing tasks with this name (substring match).
    pub name: Option<String>,
    /// Only include batches with tasks created on or after this timestamp.
    pub created_after: Option<chrono::DateTime<Utc>>,
    /// Only include batches with tasks created on or before this timestamp.
    pub created_before: Option<chrono::DateTime<Utc>>,
}

/// Input DTO for creating a new task. Tasks are always created in batches via `POST /task`.
///
/// ## Webhook callback mechanism
/// When the worker picks up a `Pending` task, it calls the `on_start` webhook with a
/// `?handle=<host>/task/<uuid>` query parameter. Your service uses this URL to report
/// completion via `PATCH` (status update) or `PUT` (counter update).
///
/// ## Example
/// ```json
/// {
///   "id": "build",
///   "name": "Build Application",
///   "kind": "ci",
///   "timeout": 300,
///   "metadata": {"project": "my-app", "branch": "main"},
///   "on_start": {
///     "kind": "Webhook",
///     "params": {"url": "https://ci.example.com/build", "verb": "Post", "body": {"ref": "main"}}
///   },
///   "on_success": [
///     {"kind": "Webhook", "params": {"url": "https://slack.example.com/notify", "verb": "Post"}}
///   ]
/// }
/// ```
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct NewTaskDto {
    /// Local identifier for this task within the batch. This is NOT a UUID — it's a string you
    /// choose (e.g. "build", "deploy", "step-1") used only to wire dependencies between tasks
    /// in the same request. The server assigns the real UUID. Other tasks reference this id in
    /// their `dependencies` array.
    pub id: String,

    /// Human-readable name for the task. Must be non-empty, max 255 characters.
    pub name: String,

    /// Task category/type. Used for concurrency rule matching and filtering. Must be non-empty,
    /// max 100 characters. Examples: "ci", "deploy", "clustering", "etl".
    pub kind: String,
    /// Maximum execution time in seconds. If a Running task exceeds this, it is marked as Failure.
    /// Must be positive, max 86400 (24 hours). Defaults to a server-side value if omitted.
    pub timeout: Option<i32>,
    /// Arbitrary JSON metadata attached to the task. Used for concurrency rule matching (via
    /// Matcher.fields) and deduplication. Max 64KB. Example: `{"projectId": "abc", "env": "prod"}`
    pub metadata: Option<serde_json::Value>,
    /// Deduplication strategy. If any matcher matches an existing task in the database (same status,
    /// kind, and metadata field values), this task is skipped (not created). Useful to avoid
    /// duplicate task creation in retry scenarios.
    pub dedupe_strategy: Option<Vec<Matcher>>,

    /// Concurrency rules for this task. Each rule is a Strategy that can limit how many tasks of a
    /// given kind can run concurrently. The worker checks these rules before starting a Pending task.
    pub rules: Option<Rules>,

    /// Webhook action called when the worker starts this task (transitions Pending -> Running).
    /// The webhook receives a `?handle=<callback_url>` query parameter for reporting completion.
    /// The webhook response body can optionally return a `NewActionDto` JSON to register a cancel action.
    pub on_start: NewActionDto,

    /// Dependencies on other tasks in the same batch. If specified, this task starts as `Waiting`
    /// and transitions to `Pending` only when all dependencies are met.
    pub dependencies: Option<Vec<Dependency>>,

    /// Expected total count for progress tracking. Purely informational — the task is NOT
    /// auto-completed when `success + failures` reaches this value. Use it to compute
    /// `progress = (success + failures) / expected_count`. Must be >= 0 if provided.
    pub expected_count: Option<i32>,

    /// Webhook actions called when this task ends with `Failure` status. Multiple actions supported.
    pub on_failure: Option<Vec<NewActionDto>>,
    /// Webhook actions called when this task ends with `Success` status. Multiple actions supported.
    pub on_success: Option<Vec<NewActionDto>>,

    /// If true, this task acts as a dead-end barrier: it can be canceled by dead-end detection,
    /// but the upward cascade stops here — its parents are NOT checked. Defaults to false.
    pub dead_end_barrier: Option<bool>,
}

/// A dependency on another task within the same batch.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct Dependency {
    /// The local `id` of the parent task (as defined in its `NewTaskDto.id` field).
    pub id: String,
    /// If true, this child task will be marked as `Failure` (and propagated to its own children)
    /// if the parent fails or is canceled. If false, the child only waits for the parent to
    /// finish (regardless of success or failure) before becoming Pending.
    pub requires_success: bool,
}

/// Lightweight task representation returned by list endpoints and batch creation.
/// Does not include actions, rules, or metadata for performance.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct BasicTaskDto {
    /// Server-assigned UUID. Use this for all subsequent API calls (GET, PATCH, PUT, DELETE).
    pub id: uuid::Uuid,
    /// Human-readable name.
    pub name: String,
    /// Task category used for filtering and concurrency rules.
    pub kind: String,
    /// Current status in the task lifecycle.
    pub status: StatusKind,
    /// When the task was created.
    pub created_at: chrono::DateTime<Utc>,
    /// When the worker started the task (called on_start webhook). Null if not yet started.
    pub started_at: Option<chrono::DateTime<Utc>>,
    /// Accumulated success counter (incremented via PUT endpoint).
    pub success: i32,
    /// Accumulated failure counter (incremented via PUT endpoint).
    pub failures: i32,
    /// Expected total count for progress tracking (`progress = (success + failures) / expected_count`).
    pub expected_count: Option<i32>,
    /// When the task reached a terminal state (Success, Failure, Canceled). Null if still active.
    pub ended_at: Option<chrono::DateTime<Utc>>,
    /// The batch this task belongs to. All tasks created in the same POST /task call share a batch_id.
    pub batch_id: Option<uuid::Uuid>,
    /// If true, dead-end upward cascade stops at this task.
    pub dead_end_barrier: bool,
}

/// Payload for updating a task. Used by both `PATCH /task/{id}` (status update) and `PUT /task/{id}` (counter update).
///
/// ## For PATCH (status update):
/// Set `status` to `Success` or `Failure`. When `Failure`, `failure_reason` is required.
/// Other fields are optional.
///
/// ## For PUT (counter update):
/// Only `new_success` and `new_failures` are used. At least one must be non-zero.
/// The `status` field is ignored.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UpdateTaskDto {
    /// Updated metadata (merged with existing). Only used by PATCH.
    pub metadata: Option<serde_json::Value>,
    /// New status. Only `Success` and `Failure` are accepted via the API. Only used by PATCH.
    pub status: Option<StatusKind>,
    /// Number of successful sub-items to add to the counter. Only used by PUT. Must be non-negative.
    pub new_success: Option<i32>,
    /// Number of failed sub-items to add to the counter. Only used by PUT. Must be non-negative.
    pub new_failures: Option<i32>,
    /// Reason for failure. Required when `status` is `Failure`, ignored otherwise.
    pub failure_reason: Option<String>,
    /// Updated expected count for progress tracking. Must be >= 0 if provided.
    pub expected_count: Option<i32>,
}

/// Input DTO for creating actions. The trigger (Start/End/Cancel) is determined by context:
/// `on_start` field -> Start trigger, `on_success` -> End+Success, `on_failure` -> End+Failure.
///
/// ## Example (Webhook action)
/// ```json
/// {
///   "kind": "Webhook",
///   "params": {
///     "url": "https://my-service.com/webhook",
///     "verb": "Post",
///     "body": {"key": "value"},
///     "headers": {"Authorization": "Bearer token123"}
///   }
/// }
/// ```
#[derive(Debug, Serialize, Deserialize, Clone, ToSchema)]
pub struct NewActionDto {
    /// Action type. Currently only `Webhook` is supported.
    pub kind: ActionKindEnum,
    /// Action parameters. For `Webhook` kind, this must be a `WebhookParams` JSON object
    /// with fields: `url` (required, string), `verb` (required, one of Get/Post/Put/Patch/Delete),
    /// `body` (optional, JSON), `headers` (optional, key-value map).
    pub params: serde_json::Value,
}

/// Output DTO for a registered action on a task. Includes the trigger type (Start/End/Cancel)
/// which is determined by context during creation and stored in the database.
#[derive(Debug, Serialize, Deserialize, Clone, ToSchema)]
pub struct ActionDto {
    /// Action type (currently only Webhook).
    pub kind: ActionKindEnum,
    /// When this action fires: `Start` (task begins), `End` (task completes), or `Cancel` (task canceled).
    pub trigger: TriggerKind,
    /// Action parameters (WebhookParams for Webhook kind).
    pub params: serde_json::Value,
}

/// Full task representation returned by `GET /task/{id}`. Includes all details: actions,
/// rules, metadata, counters, and timestamps.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TaskDto {
    /// Server-assigned UUID.
    pub id: uuid::Uuid,
    /// Human-readable name.
    pub name: String,
    /// Task category.
    pub kind: String,
    /// Current lifecycle status.
    pub status: StatusKind,
    /// Timeout in seconds. Running tasks exceeding this are marked as Failure.
    pub timeout: i32,
    /// Concurrency rules (empty array means no concurrency constraints).
    pub rules: Rules,
    /// Arbitrary JSON metadata.
    pub metadata: serde_json::Value,
    /// All registered actions (on_start, on_success, on_failure, cancel).
    pub actions: Vec<ActionDto>,
    /// When the task was created.
    pub created_at: chrono::DateTime<Utc>,
    /// When the worker started the task. Null if not yet started.
    pub started_at: Option<chrono::DateTime<Utc>>,
    /// When the task reached a terminal state. Null if still active.
    pub ended_at: Option<chrono::DateTime<Utc>>,
    /// Last status or counter change timestamp.
    pub last_updated: chrono::DateTime<Utc>,
    /// Accumulated success counter.
    pub success: i32,
    /// Accumulated failure counter.
    pub failures: i32,
    /// Expected total count for progress tracking (`progress = (success + failures) / expected_count`).
    pub expected_count: Option<i32>,
    /// Reason for failure (set via API or by the system for timeouts/dependency failures).
    pub failure_reason: Option<String>,
    /// Batch UUID grouping tasks created in the same POST /task call.
    pub batch_id: Option<uuid::Uuid>,
    /// If true, dead-end upward cascade stops at this task.
    pub dead_end_barrier: bool,
}

/// Pagination parameters for list endpoints.
#[derive(Debug, Serialize, Deserialize, Default, IntoParams)]
pub struct PaginationDto {
    /// Page number (0-indexed). Defaults to 0.
    pub page: Option<i64>,
    /// Number of items per page. Defaults to 50, maximum 100. Values above the max are clamped.
    pub page_size: Option<i64>,
}

impl PaginationDto {
    /// Resolve raw query params into validated offset + limit, enforcing config limits.
    pub fn resolve(self, config: &Config) -> Pagination {
        let mut page_size = self.page_size.unwrap_or(config.pagination.default_per_page);
        if page_size > config.pagination.max_per_page {
            page_size = config.pagination.max_per_page;
        }
        if page_size <= 0 {
            page_size = config.pagination.default_per_page;
        }

        let mut page = self.page.unwrap_or(0).max(0);

        // Prevent overflow when computing offset = page * page_size
        if page_size > 0 && page > 0 {
            let max_page = i64::MAX / page_size;
            if page > max_page {
                page = max_page;
            }
        }

        let offset = page.saturating_mul(page_size);

        Pagination {
            offset,
            limit: page_size,
        }
    }
}

/// Filter parameters for task listing. All filters are optional and combined with AND logic.
#[derive(Debug, Serialize, Deserialize, Default, IntoParams)]
pub struct FilterDto {
    /// Filter by task name (exact match).
    pub name: Option<String>,
    /// Filter by task kind (exact match). Example: "ci", "deploy".
    pub kind: Option<String>,
    /// Filter by task status. Example: "Running", "Pending", "Success".
    pub status: Option<StatusKind>,
    /// Filter by timeout value (exact match).
    pub timeout: Option<i32>,
    /// Filter by metadata substring match (searches within the JSON metadata).
    pub metadata: Option<String>,
    /// Filter by batch UUID. Use this to get all tasks from a specific batch/DAG.
    pub batch_id: Option<uuid::Uuid>,
}

/// Resolved filter with escaped/parsed values ready for DB queries.
pub struct Filter {
    pub name: String,
    pub kind: String,
    pub metadata: Option<serde_json::Value>,
    pub status: Option<StatusKind>,
    pub timeout: Option<i32>,
    pub batch_id: Option<uuid::Uuid>,
}

/// Escape LIKE wildcards in user input to prevent pattern injection.
pub(crate) fn escape_like_pattern(input: &str) -> String {
    input
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

impl FilterDto {
    /// Resolve raw query params into escaped/parsed values ready for DB queries.
    pub fn resolve(self) -> Filter {
        Filter {
            name: escape_like_pattern(&self.name.unwrap_or_default()),
            kind: escape_like_pattern(&self.kind.unwrap_or_default()),
            metadata: self.metadata.and_then(|f| serde_json::from_str(&f).ok()),
            status: self.status,
            timeout: self.timeout,
            batch_id: self.batch_id,
        }
    }
}

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

// =============================================================================
// Batch Rules Update DTOs
// =============================================================================

/// Payload for updating concurrency/capacity rules on non-terminal tasks in a batch,
/// filtered by task kind.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UpdateBatchRulesDto {
    /// The task kind to target. Only non-terminal tasks with this kind will be updated.
    pub kind: String,
    /// The new concurrency/capacity rules to apply.
    /// Pass an empty array to remove all rules.
    pub rules: Rules,
}

/// Response from updating batch rules. Reports how many tasks were affected.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UpdateBatchRulesResponseDto {
    /// The batch UUID that was updated.
    pub batch_id: uuid::Uuid,
    /// The task kind that was targeted.
    pub kind: String,
    /// Number of tasks whose rules were updated.
    pub updated_count: i64,
}

// =============================================================================
// Batch Stop DTOs
// =============================================================================

/// Response from stopping an entire batch. Reports how many tasks were affected per status.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct StopBatchResponseDto {
    /// The batch UUID that was stopped.
    pub batch_id: uuid::Uuid,
    /// Number of Waiting tasks that were canceled.
    pub canceled_waiting: i64,
    /// Number of Pending tasks that were canceled.
    pub canceled_pending: i64,
    /// Number of Claimed tasks that were canceled (on_start not yet called).
    pub canceled_claimed: i64,
    /// Number of Running tasks that were canceled (cancel webhooks fired if registered).
    pub canceled_running: i64,
    /// Number of Paused tasks that were canceled.
    pub canceled_paused: i64,
    /// Number of tasks already in a terminal state (Success, Failure, Canceled) — not modified.
    pub already_terminal: i64,
}

// =============================================================================
// Conversions
// =============================================================================

impl From<Task> for BasicTaskDto {
    fn from(t: Task) -> Self {
        Self {
            id: t.id,
            name: t.name,
            kind: t.kind,
            status: t.status,
            created_at: t.created_at,
            started_at: t.started_at,
            success: t.success,
            failures: t.failures,
            expected_count: t.expected_count,
            ended_at: t.ended_at,
            batch_id: t.batch_id,
            dead_end_barrier: t.dead_end_barrier,
        }
    }
}

impl TaskDto {
    pub fn new(base_task: Task, actions: Vec<Action>) -> Self {
        Self {
            id: base_task.id,
            name: base_task.name,
            kind: base_task.kind,
            status: base_task.status,
            timeout: base_task.timeout,
            rules: base_task.start_condition,
            metadata: base_task.metadata,
            created_at: base_task.created_at,
            success: base_task.success,
            failures: base_task.failures,
            expected_count: base_task.expected_count,
            ended_at: base_task.ended_at,
            last_updated: base_task.last_updated,
            started_at: base_task.started_at,
            failure_reason: base_task.failure_reason,
            batch_id: base_task.batch_id,
            dead_end_barrier: base_task.dead_end_barrier,
            actions: actions
                .into_iter()
                .map(|a| ActionDto {
                    kind: a.kind,
                    params: a.params,
                    trigger: a.trigger,
                })
                .collect(),
        }
    }
}
