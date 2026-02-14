//! HTTP handlers for task runner endpoints.
//!
//! This module contains all HTTP handler functions that can be used by both
//! the main application and integration tests.

use std::sync::Arc;

use actix_web::http::header::{CacheControl, CacheDirective, ContentType};
use actix_web::{HttpResponse, error, web};
use tokio::sync::mpsc::Sender;
use uuid::Uuid;

use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::{
    DbPool,
    action::ActionExecutor,
    circuit_breaker::CircuitBreaker,
    config::Config,
    db_operation, dtos, metrics, validation,
    workers::{self, UpdateEvent},
};

/// Shared application state
#[derive(Clone)]
pub struct AppState {
    pub action_executor: ActionExecutor,
    pub pool: DbPool,
    pub sender: Sender<UpdateEvent>,
    pub config: Arc<Config>,
    pub circuit_breaker: Arc<CircuitBreaker>,
}

/// Health check response showing service and database status.
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct HealthResponse {
    /// Overall service status: "ok" or "degraded"
    pub status: String,
    /// Database connectivity status: "healthy" or error details
    pub database: String,
    /// Total number of connections in the pool
    pub pool_size: u32,
    /// Number of idle (available) connections in the pool
    pub pool_idle: u32,
}

/// Helper to get a connection from pool with retry logic and circuit breaker protection
pub async fn get_conn_with_retry<'a>(
    pool: &'a DbPool,
    circuit_breaker: &CircuitBreaker,
    max_retries: u32,
    retry_delay_ms: u64,
) -> Result<crate::Conn<'a>, actix_web::Error> {
    // Check circuit breaker first
    if let Err(state) = circuit_breaker.check().await {
        log::warn!("Circuit breaker is {:?}, rejecting request", state);
        metrics::record_circuit_breaker_rejection();
        return Err(error::ErrorServiceUnavailable(
            "Service temporarily unavailable (circuit breaker open)",
        ));
    }

    let mut last_error = None;

    let effective_retries = max_retries.max(1);

    for attempt in 0..effective_retries {
        match pool.get().await {
            Ok(conn) => {
                // Record success for circuit breaker
                circuit_breaker.record_success().await;
                return Ok(conn);
            }
            Err(e) => {
                last_error = Some(e);
                if attempt + 1 < effective_retries {
                    log::warn!(
                        "Failed to acquire connection (attempt {}/{}), retrying in {}ms",
                        attempt + 1,
                        effective_retries,
                        retry_delay_ms
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(retry_delay_ms)).await;
                }
            }
        }
    }

    // All retries failed - record failure for circuit breaker
    circuit_breaker.record_failure().await;

    let err = last_error.unwrap();
    log::error!(
        "Failed to acquire connection after {} attempts: {}",
        effective_retries,
        err
    );
    metrics::TASKS_BY_STATUS
        .with_label_values(&["pool_exhausted"])
        .inc();
    Err(error::ErrorServiceUnavailable(
        "Database connection unavailable",
    ))
}

// =============================================================================
// Health Check Handlers
// =============================================================================

#[utoipa::path(
    get,
    path = "/health",
    summary = "Health check",
    description = "Verifies database connectivity and returns pool statistics. Returns 200 if the database is reachable, 503 otherwise. Use this for liveness probes.",
    responses(
        (status = 200, description = "Service is healthy", body = HealthResponse),
        (status = 503, description = "Service is degraded — database unreachable", body = HealthResponse),
    ),
    tag = "health"
)]
/// Health check endpoint - verifies database connectivity
pub async fn health_check(state: web::Data<AppState>) -> HttpResponse {
    let pool_state = state.pool.state();

    // Try to get a connection to verify DB is accessible
    let db_status = match state.pool.get().await {
        Ok(_conn) => "healthy".to_string(),
        Err(e) => {
            log::warn!("Health check: database connection failed: {}", e);
            "unhealthy".to_string()
        }
    };

    let is_healthy = db_status == "healthy";

    let response = HealthResponse {
        status: if is_healthy {
            "ok".to_string()
        } else {
            "degraded".to_string()
        },
        database: db_status,
        pool_size: pool_state.connections,
        pool_idle: pool_state.idle_connections,
    };

    if is_healthy {
        HttpResponse::Ok().json(response)
    } else {
        HttpResponse::ServiceUnavailable().json(response)
    }
}

#[utoipa::path(
    get,
    path = "/ready",
    summary = "Readiness check",
    description = "Stricter than /health — also verifies the connection pool is not exhausted. Returns 503 if all pool connections are in use. Use this for readiness probes.",
    responses(
        (status = 200, description = "Service is ready to accept traffic"),
        (status = 503, description = "Service is not ready — pool exhausted or DB unreachable"),
    ),
    tag = "health"
)]
/// Readiness check - more strict than health check
pub async fn readiness_check(state: web::Data<AppState>) -> HttpResponse {
    // Check if we have at least one idle connection
    let pool_state = state.pool.state();

    if pool_state.idle_connections == 0 && pool_state.connections >= state.config.pool.max_size {
        return HttpResponse::ServiceUnavailable().json(serde_json::json!({
            "status": "not_ready",
            "reason": "connection pool exhausted"
        }));
    }

    // Verify we can actually get a connection
    match state.pool.get().await {
        Ok(_) => HttpResponse::Ok().json(serde_json::json!({"status": "ready"})),
        Err(_) => HttpResponse::ServiceUnavailable().json(serde_json::json!({
            "status": "not_ready",
            "reason": "cannot acquire database connection"
        })),
    }
}

// =============================================================================
// Task Handlers
// =============================================================================

/// Enforce pagination limits from config
pub fn enforce_pagination_limits(
    mut pagination: dtos::PaginationDto,
    config: &Config,
) -> dtos::PaginationDto {
    // Apply default if not specified
    if pagination.page_size.is_none() {
        pagination.page_size = Some(config.pagination.default_per_page);
    }

    // Enforce maximum
    if let Some(page_size) = pagination.page_size {
        if page_size > config.pagination.max_per_page {
            log::debug!(
                "Capping page_size from {} to max {}",
                page_size,
                config.pagination.max_per_page
            );
            pagination.page_size = Some(config.pagination.max_per_page);
        }
        if page_size <= 0 {
            pagination.page_size = Some(config.pagination.default_per_page);
        }
    }

    // Ensure page is non-negative
    if let Some(page) = pagination.page
        && page < 0
    {
        pagination.page = Some(0);
    }

    // Prevent overflow when computing offset = page * page_size
    if let (Some(page), Some(page_size)) = (pagination.page, pagination.page_size) {
        if page_size > 0 && page > 0 {
            let max_page = i64::MAX / page_size;
            if page > max_page {
                log::debug!("Capping page from {} to max {}", page, max_page);
                pagination.page = Some(max_page);
            }
        }
    }

    pagination
}

#[utoipa::path(
    get,
    path = "/task",
    summary = "List tasks",
    description = "Returns a paginated list of tasks. Supports filtering by name, kind, status, batch_id, and metadata substring. Default page_size is 50, maximum is 100. Results are returned as lightweight BasicTaskDto (no actions/rules).",
    params(dtos::PaginationDto, dtos::FilterDto),
    responses(
        (status = 200, description = "Paginated array of tasks matching the filters", body = Vec<dtos::BasicTaskDto>),
    ),
    tag = "tasks"
)]
/// List tasks with filtering and pagination
pub async fn list_task(
    state: web::Data<AppState>,
    pagination: web::Query<dtos::PaginationDto>,
    filter: web::Query<dtos::FilterDto>,
) -> actix_web::Result<HttpResponse> {
    let mut conn = get_conn_with_retry(
        &state.pool,
        &state.circuit_breaker,
        state.config.pool.acquire_retries,
        state.config.pool.retry_delay.as_millis() as u64,
    )
    .await?;

    // Enforce pagination limits
    let pagination = enforce_pagination_limits(pagination.0, &state.config);

    let tasks = db_operation::list_task_filtered_paged(&mut conn, pagination, filter.0)
        .await
        .map_err(error::ErrorInternalServerError)?;
    Ok(HttpResponse::Ok().json(tasks))
}

#[utoipa::path(
    patch,
    path = "/task/{task_id}",
    summary = "Update task status (synchronous)",
    description = "Update a running task's status to `Success` or `Failure`. This is the primary way external systems report task completion after receiving the `on_start` webhook.

**Only `Success` and `Failure` are valid target statuses.** Setting `Failure` requires a `failure_reason`.

This endpoint is synchronous: it immediately triggers `on_success`/`on_failure` webhooks and propagates status to dependent children. For high-throughput counter updates, use `PUT /task/{task_id}` instead.

The `task_id` is the UUID returned by `POST /task`, also available from the `?handle=` query parameter passed to your webhook.",
    params(("task_id" = Uuid, Path, description = "The UUID of the task to update (returned by POST /task or from the ?handle= webhook query param)")),
    request_body(content = dtos::UpdateTaskDto, description = "Fields to update. Set `status` to `Success` or `Failure`. When setting `Failure`, `failure_reason` is required."),
    responses(
        (status = 200, description = "Task updated successfully. Webhooks and propagation triggered."),
        (status = 400, description = "Validation failed (invalid status transition, missing failure_reason, etc.)"),
        (status = 404, description = "Task not found"),
        (status = 409, description = "Task exists but is not in Running state"),
    ),
    tag = "tasks"
)]
/// Update a running task's status
pub async fn update_task(
    state: web::Data<AppState>,
    task_id: web::Path<Uuid>,
    form: web::Json<dtos::UpdateTaskDto>,
) -> actix_web::Result<HttpResponse> {
    // Validate update DTO before processing
    if let Err(errors) = validation::validate_update_task(&form) {
        let error_messages: Vec<String> = errors.iter().map(|e| e.to_string()).collect();
        return Ok(HttpResponse::BadRequest().json(serde_json::json!({
            "error": "Validation failed",
            "details": error_messages
        })));
    }

    let mut conn = get_conn_with_retry(
        &state.pool,
        &state.circuit_breaker,
        state.config.pool.acquire_retries,
        state.config.pool.retry_delay.as_millis() as u64,
    )
    .await?;

    let result =
        db_operation::update_running_task(&state.action_executor, &mut conn, *task_id, form.0)
            .await
            .map_err(error::ErrorInternalServerError)?;
    Ok(match result {
        db_operation::UpdateTaskResult::Updated => {
            HttpResponse::Ok().body("Task updated successfully")
        }
        db_operation::UpdateTaskResult::NotRunning => {
            HttpResponse::Conflict().json(serde_json::json!({
                "error": "Task is not in Running state"
            }))
        }
        db_operation::UpdateTaskResult::NotFound => {
            HttpResponse::NotFound().json(serde_json::json!({
                "error": "Task not found"
            }))
        }
    })
}

#[utoipa::path(
    get,
    path = "/task/{task_id}",
    summary = "Get task details",
    description = "Returns full task details including actions, rules, metadata, counters, and timestamps. Use this to inspect a task's current state, its registered webhooks, and concurrency rules.",
    params(("task_id" = Uuid, Path, description = "The UUID of the task")),
    responses(
        (status = 200, description = "Full task details with actions", body = dtos::TaskDto),
        (status = 404, description = "No task found with this ID"),
    ),
    tag = "tasks"
)]
/// Get a task by ID
pub async fn get_task(
    state: web::Data<AppState>,
    task_id: web::Path<Uuid>,
) -> actix_web::Result<HttpResponse> {
    let mut conn = get_conn_with_retry(
        &state.pool,
        &state.circuit_breaker,
        state.config.pool.acquire_retries,
        state.config.pool.retry_delay.as_millis() as u64,
    )
    .await?;

    let task = db_operation::find_detailed_task_by_id(&mut conn, *task_id)
        .await
        .map_err(error::ErrorInternalServerError)?;

    Ok(match task {
        Some(t) => HttpResponse::Ok().json(t),
        None => HttpResponse::NotFound().body("No task found with UID"),
    })
}

#[utoipa::path(
    put,
    path = "/task/{task_id}",
    summary = "Batch counter update (async)",
    description = "Queue an incremental success/failure counter update for a task. Unlike `PATCH`, this is **asynchronous** — updates are batched and flushed to the database periodically for high throughput.

Use this when your task processes many items and you want to report progress incrementally (e.g., 'processed 10 more items successfully'). At least one of `new_success` or `new_failures` must be non-zero. The `status` field is ignored by this endpoint.

Returns 202 Accepted immediately — the actual database update happens in the background.",
    params(("task_id" = Uuid, Path, description = "The UUID of the task to update counters for")),
    request_body(content = dtos::UpdateTaskDto, description = "Only `new_success` and `new_failures` are used. At least one must be non-zero."),
    responses(
        (status = 202, description = "Counter update queued for batch processing"),
        (status = 400, description = "Validation failed or both counters are zero"),
        (status = 500, description = "Internal error — failed to queue the update"),
    ),
    tag = "tasks"
)]
/// Push to update event queue for update batching
pub async fn batch_task_updater(
    state: web::Data<AppState>,
    task_id: web::Path<Uuid>,
    form: web::Json<dtos::UpdateTaskDto>,
) -> HttpResponse {
    // Validate only counter fields for PUT (batch counter) endpoint
    if let Err(errors) = validation::validate_update_task_counters(&form) {
        let error_messages: Vec<String> = errors.iter().map(|e| e.to_string()).collect();
        return HttpResponse::BadRequest().json(serde_json::json!({
            "error": "Validation failed",
            "details": error_messages
        }));
    }

    let task_id = task_id.into_inner();
    let success = form.new_success.unwrap_or(0);
    let failures = form.new_failures.unwrap_or(0);

    if success == 0 && failures == 0 {
        return HttpResponse::BadRequest()
            .body("At least one of new_success or new_failures must be non-zero");
    }

    match state
        .sender
        .send(UpdateEvent {
            success,
            failures,
            task_id,
        })
        .await
    {
        Ok(_) => HttpResponse::Accepted().body("Queued"),
        Err(_) => HttpResponse::InternalServerError().body("Failed to queue update"),
    }
}

#[utoipa::path(
    post,
    path = "/task",
    summary = "Create task batch (DAG)",
    description = "Create one or more tasks as a batch, optionally forming a DAG via dependencies. All tasks in a single request share the same `batch_id` (returned in the `X-Batch-ID` response header).

**How dependencies work:** Each task has a local `id` (a string you choose, e.g. `\"build\"`, `\"deploy\"`). Other tasks in the same batch can reference this `id` in their `dependencies` array. Tasks with no dependencies (or whose dependencies are all met) start as `Pending`; tasks with unmet dependencies start as `Waiting`.

**Deduplication:** If a task's `dedupe_strategy` matches an existing task in the database (by status, kind, and metadata fields), that task is skipped (not created). If all tasks are deduplicated, the response is 204 No Content.

**Validation:** The entire batch is validated before any inserts. Circular dependencies, invalid webhook URLs, empty names/kinds, and SSRF attempts are rejected with 400.

**Transaction:** All tasks are created in a single database transaction — either all succeed or none are created.",
    request_body(content = Vec<dtos::NewTaskDto>, description = "Array of tasks to create. Order matters: a task can only depend on tasks defined earlier in the array (or in the same position — the server resolves local IDs). See NewTaskDto schema for field details."),
    responses(
        (status = 201, description = "Tasks created successfully. Response body is the array of created tasks with their server-assigned UUIDs. The `X-Batch-ID` header contains the batch UUID.", body = Vec<dtos::BasicTaskDto>),
        (status = 204, description = "All tasks were deduplicated — nothing was created. The `X-Batch-ID` header is still returned."),
        (status = 400, description = "Validation failed. Response body contains `error`, `batch_id`, and `details` (array of error strings)."),
    ),
    tag = "tasks"
)]
/// Create new tasks (batch)
pub async fn add_task(
    state: web::Data<AppState>,
    form: web::Json<Vec<dtos::NewTaskDto>>,
    // requester: web::Header<Requester>,
) -> actix_web::Result<HttpResponse> {
    use std::collections::HashMap;

    // Generate batch_id for tracing this entire DAG
    let batch_id = Uuid::new_v4();

    log::info!(
        "[batch_id={}] Creating task batch from requester={}, task_count={}",
        batch_id,
        "",
        form.len()
    );

    // Validate the entire batch BEFORE acquiring a connection
    let f = form.0;
    if let Err(errors) = validation::validate_task_batch(&f) {
        let error_messages: Vec<String> = errors.iter().map(|e| e.to_string()).collect();
        log::warn!(
            "[batch_id={}] Task batch validation failed: {:?}",
            batch_id,
            error_messages
        );
        return Ok(HttpResponse::BadRequest().json(serde_json::json!({
            "error": "Validation failed",
            "batch_id": batch_id,
            "details": error_messages
        })));
    }

    let mut conn = get_conn_with_retry(
        &state.pool,
        &state.circuit_breaker,
        state.config.pool.acquire_retries,
        state.config.pool.retry_delay.as_millis() as u64,
    )
    .await?;

    let result = db_operation::run_in_transaction(&mut conn, |conn| {
        Box::pin(async move {
            let mut result = vec![];
            let mut id_mapping: HashMap<String, uuid::Uuid> = HashMap::new();

            for i in f.into_iter() {
                let local_id = i.id.clone();

                let task =
                    db_operation::insert_new_task(conn, i, &id_mapping, Some(batch_id)).await?;
                if let Some(t) = task {
                    id_mapping.insert(local_id, t.id);
                    result.push(t);
                }
            }

            Ok(result)
        })
    })
    .await
    .map_err(|e| {
        log::error!("[batch_id={}] Failed to create task batch: {}", batch_id, e);
        error::ErrorInternalServerError(e)
    })?;

    log::info!(
        "[batch_id={}] Task batch created successfully, tasks_created={}",
        batch_id,
        result.len()
    );

    if result.is_empty() {
        Ok(HttpResponse::NoContent()
            .insert_header(("X-Batch-ID", batch_id.to_string()))
            .finish())
    } else {
        Ok(HttpResponse::Created()
            .insert_header(("X-Batch-ID", batch_id.to_string()))
            .json(result))
    }
}

#[utoipa::path(
    delete,
    path = "/task/{task_id}",
    summary = "Cancel a task",
    description = "Cancel a task and propagate cancellation to its dependents. The task is set to `Canceled` status, which behaves like `Failure` for dependency propagation — children with `requires_success=true` will also be marked as `Failure` recursively.

If the task has a registered `Cancel` action (returned by the `on_start` webhook response), that webhook is called.

Only tasks in `Pending`, `Waiting`, or `Running` status can be canceled.",
    params(("task_id" = Uuid, Path, description = "The UUID of the task to cancel")),
    responses(
        (status = 200, description = "Task canceled and propagation completed"),
        (status = 400, description = "Task could not be canceled (already finished or invalid state)"),
    ),
    tag = "tasks"
)]
/// Cancel a task
pub async fn cancel_task(
    state: web::Data<AppState>,
    task_id: web::Path<Uuid>,
) -> actix_web::Result<HttpResponse> {
    let mut conn = get_conn_with_retry(
        &state.pool,
        &state.circuit_breaker,
        state.config.pool.acquire_retries,
        state.config.pool.retry_delay.as_millis() as u64,
    )
    .await?;

    match workers::cancel_task(&state.action_executor, &task_id, &mut conn).await {
        Ok(_) => Ok(HttpResponse::Ok().finish()),
        Err(_) => Ok(HttpResponse::BadRequest().finish()),
    }
}

#[utoipa::path(
    patch,
    path = "/task/pause/{task_id}",
    summary = "Pause a task",
    description = "Set a task's status to `Paused`. The worker loop will skip paused tasks. This does NOT propagate to children. To resume a paused task, update its status back to `Pending` via PATCH.",
    params(("task_id" = Uuid, Path, description = "The UUID of the task to pause")),
    responses(
        (status = 200, description = "Task paused"),
        (status = 400, description = "Task could not be paused (already finished or invalid state)"),
    ),
    tag = "tasks"
)]
/// Pause a task
pub async fn pause_task(
    state: web::Data<AppState>,
    task_id: web::Path<Uuid>,
) -> actix_web::Result<HttpResponse> {
    let mut conn = get_conn_with_retry(
        &state.pool,
        &state.circuit_breaker,
        state.config.pool.acquire_retries,
        state.config.pool.retry_delay.as_millis() as u64,
    )
    .await?;

    match db_operation::pause_task(&task_id, &mut conn).await {
        Ok(_) => Ok(HttpResponse::Ok().finish()),
        Err(_) => Ok(HttpResponse::BadRequest().finish()),
    }
}

// =============================================================================
// DAG Visualization Handlers
// =============================================================================

#[utoipa::path(
    get,
    path = "/dag/{batch_id}",
    summary = "Get DAG structure",
    description = "Returns all tasks and dependency links for a given batch. Use this to visualize or inspect the full dependency graph. The `batch_id` is returned in the `X-Batch-ID` header of `POST /task` responses.",
    params(("batch_id" = Uuid, Path, description = "The batch UUID (from the X-Batch-ID response header of POST /task)")),
    responses(
        (status = 200, description = "DAG structure with tasks and links", body = dtos::DagDto),
    ),
    tag = "dag"
)]
/// Get DAG data for a batch (tasks + links)
pub async fn get_dag(
    state: web::Data<AppState>,
    batch_id: web::Path<Uuid>,
) -> actix_web::Result<HttpResponse> {
    let mut conn = get_conn_with_retry(
        &state.pool,
        &state.circuit_breaker,
        state.config.pool.acquire_retries,
        state.config.pool.retry_delay.as_millis() as u64,
    )
    .await?;

    let dag = db_operation::get_dag_for_batch(&mut conn, *batch_id)
        .await
        .map_err(error::ErrorInternalServerError)?;

    Ok(HttpResponse::Ok().json(dag))
}

#[utoipa::path(
    get,
    path = "/view",
    summary = "DAG visualization UI",
    description = "Serves an HTML page with an interactive DAG visualizer. Open this in a browser and provide a `batch_id` to render the task graph. This is a human-facing UI, not an API endpoint.",
    responses(
        (status = 200, description = "HTML page for DAG visualization", content_type = "text/html"),
    ),
    tag = "dag"
)]
/// Serve the DAG visualization HTML page
pub async fn view_dag_page() -> HttpResponse {
    HttpResponse::Ok()
        .content_type(ContentType::html())
        .insert_header(CacheControl(vec![CacheDirective::NoCache]))
        .body(include_str!("../static/dag.html"))
}

// =============================================================================
// OpenAPI Documentation
// =============================================================================

#[derive(OpenApi)]
#[openapi(
    paths(
        health_check,
        readiness_check,
        list_task,
        add_task,
        get_task,
        update_task,
        batch_task_updater,
        cancel_task,
        pause_task,
        get_dag,
        view_dag_page,
    ),
    components(schemas(
        HealthResponse,
        dtos::NewTaskDto,
        dtos::TaskDto,
        dtos::BasicTaskDto,
        dtos::UpdateTaskDto,
        dtos::NewActionDto,
        dtos::ActionDto,
        dtos::Dependency,
        dtos::LinkDto,
        dtos::DagDto,
        crate::models::StatusKind,
        crate::models::ActionKindEnum,
        crate::models::TriggerKind,
        crate::models::TriggerCondition,
        crate::rule::Strategy,
        crate::rule::Rules,
        crate::rule::ConcurencyRule,
        crate::rule::Matcher,
        crate::action::HttpVerb,
        crate::action::WebhookParams,
    )),
    tags(
        (name = "health", description = "Health and readiness probes. Use GET /health for liveness and GET /ready for readiness."),
        (name = "tasks", description = "Task CRUD and lifecycle management. Tasks are created in batches via POST /task, forming a DAG. The worker loop picks up Pending tasks and calls their on_start webhook. External systems report completion via PATCH or PUT on /task/{task_id}."),
        (name = "dag", description = "DAG visualization. Retrieve the full graph (tasks + dependency links) for a batch, or render the built-in HTML viewer."),
    ),
    info(
        title = "Task Runner API",
        version = "0.1.0",
        description = "# Task Runner - Task Orchestration Service

A webhook-based task scheduler that executes DAGs (Directed Acyclic Graphs) of tasks with concurrency control.

## Core Workflow

1. **Create a batch** of tasks via `POST /task` with an array of `NewTaskDto`. Each task has a local `id` (string, your choice) used to wire dependencies within the batch. The server assigns real UUIDs and returns them. The response includes an `X-Batch-ID` header grouping all tasks.

2. **The worker loop** (server-side, automatic) picks up `Pending` tasks, checks concurrency rules, and calls each task's `on_start` webhook. The webhook URL receives a `?handle=<host>/task/<uuid>` query parameter — this is the callback URL your service must use to report completion.

3. **Report completion** by calling `PATCH /task/{task_id}` with `{\"status\": \"Success\"}` or `{\"status\": \"Failure\", \"failure_reason\": \"...\"}`. Only `Success` and `Failure` are valid status transitions via the API.

4. **Dependency propagation** happens automatically: when a parent succeeds, its children's counters decrement; when all counters reach zero, the child becomes `Pending`. When a parent fails and `requires_success` was true on the link, the child (and its descendants) are marked as `Failure`.

## Task States

| State | Meaning |
|-------|---------|
| `Waiting` | Has unmet dependencies — will transition to `Pending` automatically when parents complete |
| `Pending` | Ready to run — the worker loop will pick it up and call its `on_start` webhook |
| `Running` | `on_start` webhook has been called — waiting for external completion report |
| `Success` | Completed successfully (set via API) |
| `Failure` | Failed — either reported via API, timed out, or failed due to parent failure |
| `Paused` | Manually paused via `PATCH /task/pause/{task_id}` — worker ignores it |
| `Canceled` | Manually canceled via `DELETE /task/{task_id}` — treated like Failure for propagation |

## PATCH vs PUT for Updates

- **`PATCH /task/{task_id}`**: Synchronous update. Use this to set final status (`Success`/`Failure`). Triggers `on_success`/`on_failure` webhooks and dependency propagation immediately.
- **`PUT /task/{task_id}`**: Batched counter update. Use this for high-throughput incremental `new_success`/`new_failures` counter bumps. Updates are accumulated and flushed periodically (not immediately). At least one of `new_success` or `new_failures` must be non-zero.

## Webhooks

When the worker starts a task, it calls the `on_start` webhook with a `?handle=<callback_url>` query parameter. Your webhook handler can optionally return a `NewActionDto` JSON body to register a cancel action.

Webhook params are stored as JSON in the `params` field of `NewActionDto`:
```json
{\"kind\": \"Webhook\", \"params\": {\"url\": \"https://my-service.com/run\", \"verb\": \"Post\", \"body\": {\"key\": \"value\"}, \"headers\": {\"Authorization\": \"Bearer xxx\"}}}
```

## Concurrency Rules

Tasks can have `rules` (array of `Strategy`) that limit concurrent execution. A `Concurency` rule specifies: match currently `Running` tasks by `kind` and metadata `fields`, and block if `max_concurency` is reached. Fields are keys in the task's `metadata` JSON — two tasks match if they share the same values for all listed fields.

## Deduplication

`dedupe_strategy` on `NewTaskDto` allows skipping task creation if a matching task already exists. Each `Matcher` specifies a `status`, `kind`, and `fields` (metadata keys) to match against.

## Minimal Example

Create two tasks where `deploy` depends on `build`:
```json
[
  {
    \"id\": \"build\",
    \"name\": \"Build App\",
    \"kind\": \"ci\",
    \"timeout\": 300,
    \"on_start\": {\"kind\": \"Webhook\", \"params\": {\"url\": \"https://ci.example.com/build\", \"verb\": \"Post\"}}
  },
  {
    \"id\": \"deploy\",
    \"name\": \"Deploy App\",
    \"kind\": \"cd\",
    \"timeout\": 600,
    \"on_start\": {\"kind\": \"Webhook\", \"params\": {\"url\": \"https://cd.example.com/deploy\", \"verb\": \"Post\"}},
    \"dependencies\": [{\"id\": \"build\", \"requires_success\": true}],
    \"on_success\": [{\"kind\": \"Webhook\", \"params\": {\"url\": \"https://slack.example.com/notify\", \"verb\": \"Post\", \"body\": {\"text\": \"Deploy succeeded\"}}}]
  }
]
```
The `build` task starts as `Pending` (no deps), `deploy` starts as `Waiting`. Once `build` succeeds, `deploy` transitions to `Pending` and the worker starts it.",
    )
)]
pub struct ApiDoc;

// =============================================================================
// Route Configuration
// =============================================================================

/// Configure all routes for the application.
/// This can be used by both the main application and integration tests.
pub fn configure_routes(cfg: &mut web::ServiceConfig) {
    cfg.route("/health", web::get().to(health_check))
        .route("/ready", web::get().to(readiness_check))
        .route("/task", web::get().to(list_task))
        .route("/task", web::post().to(add_task))
        .route("/task/{task_id}", web::get().to(get_task))
        .route("/task/{task_id}", web::patch().to(update_task))
        .route("/task/{task_id}", web::put().to(batch_task_updater))
        .route("/task/{task_id}", web::delete().to(cancel_task))
        .route("/task/pause/{task_id}", web::patch().to(pause_task))
        .route("/dag/{batch_id}", web::get().to(get_dag))
        .route("/view", web::get().to(view_dag_page))
        .service(
            SwaggerUi::new("/swagger-ui/{_:.*}").url("/api-docs/openapi.json", ApiDoc::openapi()),
        );
}
