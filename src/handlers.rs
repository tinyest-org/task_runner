//! HTTP handlers for task runner endpoints.
//!
//! This module contains all HTTP handler functions that can be used by both
//! the main application and integration tests.

use std::sync::Arc;

use actix_web::http::header::{CacheControl, CacheDirective, ContentType};
use actix_web::{HttpResponse, error, web};
use tokio::sync::mpsc::Sender;
use uuid::Uuid;

use crate::{
    DbPool,
    action::ActionExecutor,
    circuit_breaker::CircuitBreaker,
    config::Config,
    db_operation, dtos,
    helper::Requester,
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

/// Health check response
#[derive(serde::Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub database: String,
    pub pool_size: u32,
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

    for attempt in 0..max_retries {
        match pool.get().await {
            Ok(conn) => {
                // Record success for circuit breaker
                circuit_breaker.record_success().await;
                return Ok(conn);
            }
            Err(e) => {
                last_error = Some(e);
                if attempt < max_retries - 1 {
                    log::warn!(
                        "Failed to acquire connection (attempt {}/{}), retrying in {}ms",
                        attempt + 1,
                        max_retries,
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
        max_retries,
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

/// Health check endpoint - verifies database connectivity
pub async fn health_check(state: web::Data<AppState>) -> HttpResponse {
    let pool_state = state.pool.state();

    // Try to get a connection to verify DB is accessible
    let db_status = match state.pool.get().await {
        Ok(_conn) => "healthy".to_string(),
        Err(e) => {
            log::warn!("Health check: database connection failed: {}", e);
            format!("unhealthy: {}", e)
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
    if let Some(page) = pagination.page {
        if page < 0 {
            pagination.page = Some(0);
        }
    }

    pagination
}

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

/// Update a running task's status
pub async fn update_task(
    state: web::Data<AppState>,
    task_id: web::Path<Uuid>,
    form: web::Json<dtos::UpdateTaskDto>,
) -> actix_web::Result<HttpResponse> {
    let mut conn = get_conn_with_retry(
        &state.pool,
        &state.circuit_breaker,
        state.config.pool.acquire_retries,
        state.config.pool.retry_delay.as_millis() as u64,
    )
    .await?;

    let count =
        db_operation::update_running_task(&state.action_executor, &mut conn, *task_id, form.0)
            .await
            .map_err(error::ErrorInternalServerError)?;
    Ok(match count {
        1 => HttpResponse::Ok().body("Task updated successfully".to_string()),
        _ => HttpResponse::NotFound().body("Task not found".to_string()),
    })
}

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

/// Push to update event queue for update batching
pub async fn batch_task_updater(
    state: web::Data<AppState>,
    task_id: web::Path<Uuid>,
    form: web::Json<dtos::UpdateTaskDto>,
) -> HttpResponse {
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

/// Create new tasks (batch)
pub async fn add_task(
    state: web::Data<AppState>,
    form: web::Json<Vec<dtos::NewTaskDto>>,
    requester: web::Header<Requester>,
) -> actix_web::Result<HttpResponse> {
    use diesel_async::RunQueryDsl;
    use std::collections::HashMap;

    // Generate batch_id for tracing this entire DAG
    let batch_id = Uuid::new_v4();

    log::info!(
        "[batch_id={}] Creating task batch from requester={}, task_count={}",
        batch_id,
        requester.0,
        form.len()
    );

    let mut conn = get_conn_with_retry(
        &state.pool,
        &state.circuit_breaker,
        state.config.pool.acquire_retries,
        state.config.pool.retry_delay.as_millis() as u64,
    )
    .await?;

    diesel::sql_query("BEGIN")
        .execute(&mut conn)
        .await
        .map_err(error::ErrorInternalServerError)?;

    let mut result = vec![];
    let mut id_mapping: HashMap<String, uuid::Uuid> = HashMap::new();

    // Validate the entire batch first
    let f = form.0;
    if let Err(errors) = validation::validate_task_batch(&f) {
        let error_messages: Vec<String> = errors.iter().map(|e| e.to_string()).collect();
        log::warn!(
            "[batch_id={}] Task batch validation failed: {:?}",
            batch_id,
            error_messages
        );
        // Rollback on validation failure
        let _ = diesel::sql_query("ROLLBACK").execute(&mut conn).await;
        return Ok(HttpResponse::BadRequest().json(serde_json::json!({
            "error": "Validation failed",
            "batch_id": batch_id,
            "details": error_messages
        })));
    }

    // Process tasks in order
    for i in f.into_iter() {
        let local_id = i.id.clone();

        let task =
            match db_operation::insert_new_task(&mut conn, i, &id_mapping, Some(batch_id)).await {
                Ok(t) => t,
                Err(e) => {
                    log::error!("[batch_id={}] Failed to insert task: {}", batch_id, e);
                    let _ = diesel::sql_query("ROLLBACK").execute(&mut conn).await;
                    return Err(error::ErrorInternalServerError(e));
                }
            };
        if let Some(t) = task {
            id_mapping.insert(local_id, t.id);
            result.push(t);
        }
    }

    if let Err(e) = diesel::sql_query("COMMIT").execute(&mut conn).await {
        log::error!(
            "[batch_id={}] Failed to commit transaction: {}",
            batch_id,
            e
        );
        let _ = diesel::sql_query("ROLLBACK").execute(&mut conn).await;
        return Err(error::ErrorInternalServerError(e));
    }

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

/// Serve the DAG visualization HTML page
pub async fn view_dag_page() -> HttpResponse {
    HttpResponse::Ok()
        .content_type(ContentType::html())
        .insert_header(CacheControl(vec![CacheDirective::NoCache]))
        .body(include_str!("../static/dag.html"))
}

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
        .route("/view", web::get().to(view_dag_page));
}
