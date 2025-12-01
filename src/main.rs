//! Task Runner HTTP Server
//!
//! A service for orchestrating task execution with DAG dependencies,
//! concurrency control, and webhook-based actions.

use std::sync::Arc;

use actix_web::{
    App, HttpResponse, HttpServer, Responder, delete, error, get, middleware, patch, post, put, web,
};
use actix_web_prometheus::PrometheusMetricsBuilder;
use diesel::{Connection, PgConnection};
use task_runner::{
    DbPool,
    action::{ActionContext, ActionExecutor},
    config::Config,
    db_operation, dtos,
    helper::Requester,
    initialize_db_pool, metrics, validation,
    workers::{self, UpdateEvent},
};
use tokio::sync::mpsc::{self, Sender};
use uuid::Uuid;

use diesel_migrations::MigrationHarness;
use diesel_migrations::{EmbeddedMigrations, embed_migrations};
pub const MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");

/// Shared application state
#[derive(Clone)]
struct AppState {
    pub action_executor: ActionExecutor,
    pub pool: DbPool,
    pub sender: Sender<UpdateEvent>,
    pub config: Arc<Config>,
}

/// Helper to get a connection from pool with retry logic
async fn get_conn_with_retry(
    pool: &DbPool,
    max_retries: u32,
    retry_delay_ms: u64,
) -> Result<task_runner::Conn<'_>, actix_web::Error> {
    let mut last_error = None;

    for attempt in 0..max_retries {
        match pool.get().await {
            Ok(conn) => return Ok(conn),
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
// Health Check Endpoint
// =============================================================================

/// Health check response
#[derive(serde::Serialize)]
struct HealthResponse {
    status: String,
    database: String,
    pool_size: u32,
    pool_idle: u32,
}

/// Health check endpoint - verifies database connectivity
#[get("/health")]
async fn health_check(state: web::Data<AppState>) -> impl Responder {
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
#[get("/ready")]
async fn readiness_check(state: web::Data<AppState>) -> impl Responder {
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
// Task Endpoints
// =============================================================================

#[get("/task")]
async fn list_task(
    state: web::Data<AppState>,
    pagination: web::Query<dtos::PaginationDto>,
    filter: web::Query<dtos::FilterDto>,
) -> actix_web::Result<impl Responder> {
    let mut conn = get_conn_with_retry(
        &state.pool,
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

/// Enforce pagination limits from config
fn enforce_pagination_limits(
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

#[patch("/task/{task_id}")]
async fn update_task(
    state: web::Data<AppState>,
    task_id: web::Path<Uuid>,
    form: web::Json<dtos::UpdateTaskDto>,
) -> actix_web::Result<impl Responder> {
    let mut conn = get_conn_with_retry(
        &state.pool,
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

#[get("/task/{task_id}")]
async fn get_task(
    state: web::Data<AppState>,
    task_id: web::Path<Uuid>,
) -> actix_web::Result<impl Responder> {
    let mut conn = get_conn_with_retry(
        &state.pool,
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
#[put("/task")]
async fn batch_task_updater(
    state: web::Data<AppState>,
    task_id: web::Path<Uuid>,
    form: web::Json<dtos::UpdateTaskDto>,
) -> impl Responder {
    match state
        .sender
        .send(UpdateEvent {
            success: form.new_success.unwrap_or(0),
            failures: form.new_failures.unwrap_or(0),
            task_id: task_id.into_inner(),
        })
        .await
    {
        Ok(_) => HttpResponse::Ok().body("OK"),
        Err(_) => HttpResponse::InternalServerError().body("Failed to send"),
    }
}

#[post("/task")]
async fn add_task(
    state: web::Data<AppState>,
    form: web::Json<Vec<dtos::NewTaskDto>>,
    requester: web::Header<Requester>,
) -> actix_web::Result<impl Responder> {
    use diesel_async::RunQueryDsl;
    use std::collections::HashMap;

    // Generate batch_id for tracing this entire DAG
    let batch_id = Uuid::new_v4();

    log::info!(
        "[batch_id={}] Creating task batch from requester={}, task_count={}",
        batch_id, requester.0, form.len()
    );

    let mut conn = get_conn_with_retry(
        &state.pool,
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
            batch_id, error_messages
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

        let task = match db_operation::insert_new_task(&mut conn, i, &id_mapping, Some(batch_id)).await {
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
        log::error!("[batch_id={}] Failed to commit transaction: {}", batch_id, e);
        let _ = diesel::sql_query("ROLLBACK").execute(&mut conn).await;
        return Err(error::ErrorInternalServerError(e));
    }

    log::info!(
        "[batch_id={}] Task batch created successfully, tasks_created={}",
        batch_id, result.len()
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

#[delete("/task/{task_id}")]
async fn cancel_task(
    state: web::Data<AppState>,
    task_id: web::Path<Uuid>,
) -> actix_web::Result<impl Responder> {
    let mut conn = get_conn_with_retry(
        &state.pool,
        state.config.pool.acquire_retries,
        state.config.pool.retry_delay.as_millis() as u64,
    )
    .await?;

    match workers::cancel_task(&state.action_executor, &task_id, &mut conn).await {
        Ok(_) => Ok(HttpResponse::Ok().finish()),
        Err(_) => Ok(HttpResponse::BadRequest().finish()),
    }
}

#[patch("/task/pause/{task_id}")]
async fn pause_task(
    state: web::Data<AppState>,
    task_id: web::Path<Uuid>,
) -> actix_web::Result<impl Responder> {
    let mut conn = get_conn_with_retry(
        &state.pool,
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
// Main Entry Point
// =============================================================================

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    dotenvy::dotenv().ok();
    env_logger::init_from_env(env_logger::Env::new().default_filter_or("info"));

    // Load configuration
    let config = Config::from_env().unwrap_or_else(|e| {
        log::error!("Configuration error: {}", e);
        std::process::exit(1);
    });
    let config = Arc::new(config);

    log::info!("Configuration loaded successfully");
    log::info!("Starting HTTP server at http://0.0.0.0:{}", config.port);
    log::info!("Using public url {}", &config.host_url);

    // Setup crypto provider
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    // Initialize database pool
    let pool = initialize_db_pool().await;
    log::info!("Database pool initialized");

    // Run migrations
    let mut conn = PgConnection::establish(&config.database_url).unwrap_or_else(|e| {
        log::error!("Failed to connect to database for migrations: {}", e);
        std::process::exit(1);
    });
    conn.run_pending_migrations(MIGRATIONS).unwrap_or_else(|e| {
        log::error!("Failed to run migrations: {}", e);
        std::process::exit(1);
    });
    log::info!("Database migrations completed");

    // Create channels for batch updates
    let (sender, receiver) = mpsc::channel::<UpdateEvent>(config.worker.batch_channel_capacity);

    // Build application state
    let app_data = AppState {
        pool: pool.clone(),
        sender,
        action_executor: ActionExecutor {
            ctx: ActionContext {
                host_address: config.host_url.clone(),
            },
        },
        config: config.clone(),
    };

    let action_context = Arc::new(ActionExecutor {
        ctx: ActionContext {
            host_address: config.host_url.clone(),
        },
    });

    // Spawn worker tasks
    let p = pool.clone();
    let a = action_context.clone();
    actix_web::rt::spawn(async move {
        task_runner::workers::start_loop(a.as_ref(), p).await;
    });

    let p2 = pool.clone();
    actix_web::rt::spawn(async {
        task_runner::workers::timeout_loop(p2).await;
    });

    let p3 = pool.clone();
    actix_web::rt::spawn(async {
        task_runner::workers::batch_updater(p3, receiver).await;
    });

    // Initialize custom metrics
    metrics::init_metrics();

    let prometheus = PrometheusMetricsBuilder::new("api")
        .endpoint("/metrics")
        .registry(metrics::REGISTRY.clone())
        .build()
        .unwrap();

    let port = config.port;

    // Build and run HTTP server with graceful shutdown
    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(app_data.clone()))
            .wrap(prometheus.clone())
            .wrap(middleware::Logger::default())
            // Health endpoints
            .service(health_check)
            .service(readiness_check)
            // Task endpoints
            .service(get_task)
            .service(add_task)
            .service(list_task)
            .service(update_task)
            .service(batch_task_updater)
            .service(cancel_task)
            .service(pause_task)
    })
    .bind(("0.0.0.0", port))?
    .shutdown_timeout(30) // Wait up to 30 seconds for graceful shutdown
    .run()
    .await
}
