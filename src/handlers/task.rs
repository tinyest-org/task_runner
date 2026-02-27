use actix_web::{HttpResponse, web};
use uuid::Uuid;

use crate::{db_operation, dtos, error::ApiError, validation, workers};

use super::AppState;
use super::response::validation_error_response;

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
        return Ok(validation_error_response(&errors));
    }

    let mut conn = state.conn().await?;

    let result = db_operation::update_running_task(
        &state.action_executor,
        &mut conn,
        *task_id,
        form.0,
        state.config.worker.dead_end_cancel_enabled,
    )
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
        return validation_error_response(&errors);
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
        .send(workers::UpdateEvent {
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

    match workers::cancel_task(
        &state.action_executor,
        &task_id,
        state.config.worker.dead_end_cancel_enabled,
        &mut conn,
    )
    .await
    {
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
