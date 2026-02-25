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
    db_operation, dtos,
    error::ApiError,
    metrics, validation,
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

impl AppState {
    /// Get a database connection with retry logic and circuit breaker protection.
    pub async fn conn(&self) -> Result<crate::Conn<'_>, actix_web::Error> {
        get_conn_with_retry(
            &self.pool,
            &self.circuit_breaker,
            self.config.pool.acquire_retries,
            self.config.pool.retry_delay.as_millis() as u64,
        )
        .await
    }
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
    if let Err(state) = circuit_breaker.check() {
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
                circuit_breaker.record_success();
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
    circuit_breaker.record_failure();

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
    let mut conn = state.conn().await?;
    let pagination = pagination.0.resolve(&state.config);
    let filter = filter.0.resolve();

    let tasks = db_operation::list_task_filtered_paged(&mut conn, pagination, filter)
        .await
        .map_err(ApiError::from)?;
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

    let mut conn = state.conn().await?;

    let result =
        db_operation::update_running_task(&state.action_executor, &mut conn, *task_id, form.0)
            .await
            .map_err(ApiError::from)?;
    Ok(match result {
        db_operation::UpdateTaskResult::Updated => {
            HttpResponse::Ok().body("Task updated successfully")
        }
        db_operation::UpdateTaskResult::NotFound => {
            HttpResponse::NotFound().json(serde_json::json!({
                "error": "Task not found or not in Running state"
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
    let mut conn = state.conn().await?;

    let task = db_operation::find_detailed_task_by_id(&mut conn, *task_id)
        .await
        .map_err(ApiError::from)?;

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

    let mut conn = state.conn().await?;

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
        ApiError::from(e)
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
    let mut conn = state.conn().await?;

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
    let mut conn = state.conn().await?;

    match db_operation::pause_task(&task_id, &mut conn).await {
        Ok(_) => Ok(HttpResponse::Ok().finish()),
        Err(_) => Ok(HttpResponse::BadRequest().finish()),
    }
}

// =============================================================================
// Batch Operations
// =============================================================================

#[utoipa::path(
    delete,
    path = "/batch/{batch_id}",
    summary = "Stop a batch",
    description = "Cancel all non-terminal tasks in a batch. Waiting, Pending, Paused, and Running tasks are set to `Canceled` with failure_reason `\"Batch stopped\"`. Running tasks with registered cancel webhooks will have those webhooks fired.

Tasks already in a terminal state (Success, Failure, Canceled) are not modified.

Returns a summary of how many tasks were affected per status category.",
    params(("batch_id" = Uuid, Path, description = "The batch UUID (from the X-Batch-ID response header of POST /task)")),
    responses(
        (status = 200, description = "Batch stopped. Returns counts per status category.", body = dtos::StopBatchResponseDto),
        (status = 404, description = "No tasks found for this batch_id"),
    ),
    tag = "batches"
)]
/// Stop all non-terminal tasks in a batch
pub async fn stop_batch(
    state: web::Data<AppState>,
    batch_id: web::Path<Uuid>,
) -> actix_web::Result<HttpResponse> {
    let mut conn = state.conn().await?;
    let batch_id = *batch_id;

    let result = db_operation::stop_batch(&mut conn, batch_id)
        .await
        .map_err(ApiError::from)?;

    // Fire cancel webhooks for formerly-Running tasks (best-effort)
    for task_id in &result.canceled_running_ids {
        if let Err(e) =
            fire_cancel_webhooks_for_task(&state.action_executor, task_id, &mut conn).await
        {
            log::error!(
                "stop_batch: failed to fire cancel webhooks for task {}: {:?}",
                task_id,
                e
            );
        }
    }

    let canceled_running = result.canceled_running_ids.len() as i64;

    // Record metrics for each canceled task
    let total_canceled = result.canceled_waiting
        + result.canceled_pending
        + result.canceled_claimed
        + canceled_running
        + result.canceled_paused;
    for _ in 0..total_canceled {
        metrics::record_task_cancelled();
    }

    log::info!(
        "Batch {} stopped: waiting={}, pending={}, claimed={}, running={}, paused={}, already_terminal={}",
        batch_id,
        result.canceled_waiting,
        result.canceled_pending,
        result.canceled_claimed,
        canceled_running,
        result.canceled_paused,
        result.already_terminal,
    );

    Ok(HttpResponse::Ok().json(dtos::StopBatchResponseDto {
        batch_id,
        canceled_waiting: result.canceled_waiting,
        canceled_pending: result.canceled_pending,
        canceled_claimed: result.canceled_claimed,
        canceled_running,
        canceled_paused: result.canceled_paused,
        already_terminal: result.already_terminal,
    }))
}

/// Fire cancel webhooks for a single task (extracted from workers::cancel_task pattern).
async fn fire_cancel_webhooks_for_task<'a>(
    evaluator: &ActionExecutor,
    task_id: &Uuid,
    conn: &mut crate::Conn<'a>,
) -> Result<(), crate::error::TaskRunnerError> {
    use crate::action::idempotency_key;
    use crate::models::{Action, Task, TriggerCondition, TriggerKind};
    use crate::schema::action::dsl::trigger;
    use crate::schema::task::dsl::*;
    use diesel::BelongingToDsl;
    use diesel::prelude::*;
    use diesel_async::RunQueryDsl;

    let key = idempotency_key(*task_id, &TriggerKind::Cancel, &TriggerCondition::Success);
    let claimed = db_operation::try_claim_webhook_execution(
        conn,
        *task_id,
        TriggerKind::Cancel,
        TriggerCondition::Success,
        &key,
        Some(evaluator.ctx.webhook_idempotency_timeout),
    )
    .await?;

    if !claimed {
        log::info!(
            "stop_batch: skipping cancel webhooks for task {} — already executed",
            task_id
        );
        return Ok(());
    }

    let t = task.filter(id.eq(task_id)).first::<Task>(conn).await?;
    let actions = Action::belonging_to(&t)
        .filter(trigger.eq(TriggerKind::Cancel))
        .load::<Action>(conn)
        .await?;

    let mut all_succeeded = true;
    for act in actions.iter() {
        match evaluator.execute(act, &t, Some(&key)).await {
            Ok(_) => log::debug!("Cancel action {} executed successfully", act.id),
            Err(e) => {
                log::error!("Cancel action {} failed: {}", act.id, e);
                all_succeeded = false;
            }
        }
    }

    if let Err(e) = db_operation::complete_webhook_execution(conn, &key, all_succeeded).await {
        log::error!(
            "Failed to complete webhook execution record for key {}: {}",
            key,
            e
        );
    }

    Ok(())
}

// =============================================================================
// Batch Listing Handlers
// =============================================================================

#[utoipa::path(
    get,
    path = "/batches",
    summary = "List batches with statistics",
    description = "Returns a paginated list of batches with aggregated task statistics per batch. Supports filtering by task name, kind, status, and creation time range. Each batch includes status counts, distinct kinds, and timestamps.",
    params(dtos::PaginationDto, dtos::BatchFilterDto),
    responses(
        (status = 200, description = "Paginated array of batch summaries", body = Vec<dtos::BatchSummaryDto>),
    ),
    tag = "batches"
)]
/// List batches with aggregated statistics
pub async fn list_batches(
    state: web::Data<AppState>,
    pagination: web::Query<dtos::PaginationDto>,
    filter: web::Query<dtos::BatchFilterDto>,
) -> actix_web::Result<HttpResponse> {
    let mut conn = state.conn().await?;
    let pagination = pagination.0.resolve(&state.config);

    let batches = db_operation::list_batches(&mut conn, pagination, filter.0)
        .await
        .map_err(ApiError::from)?;
    Ok(HttpResponse::Ok().json(batches))
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
    let mut conn = state.conn().await?;

    let dag = db_operation::get_dag_for_batch(&mut conn, *batch_id)
        .await
        .map_err(ApiError::from)?;

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
        stop_batch,
        list_batches,
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
        dtos::StopBatchResponseDto,
        dtos::BatchSummaryDto,
        dtos::BatchStatusCounts,
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
        (name = "batches", description = "Batch discovery. List all batches with aggregated task statistics, supporting filtering and pagination."),
        (name = "dag", description = "DAG visualization. Retrieve the full graph (tasks + dependency links) for a batch, or render the built-in HTML viewer."),
    ),
    info(
        title = "Task Runner API",
        version = "0.1.0",
        description = include_str!("../docs/openapi_description.md"),
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
        .route("/batch/{batch_id}", web::delete().to(stop_batch))
        .route("/batches", web::get().to(list_batches))
        .route("/dag/{batch_id}", web::get().to(get_dag))
        .route("/view", web::get().to(view_dag_page))
        .service(
            SwaggerUi::new("/swagger-ui/{_:.*}").url("/api-docs/openapi.json", ApiDoc::openapi()),
        );
}
