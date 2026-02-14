//! Task Runner HTTP Server
//!
//! A service for orchestrating task execution with DAG dependencies,
//! concurrency control, and webhook-based actions.

use std::sync::Arc;

use actix_web::{App, HttpServer, middleware, web};
use actix_web_prometheus::PrometheusMetricsBuilder;
use diesel::{Connection, PgConnection};
use task_runner::{
    action::{ActionContext, ActionExecutor},
    circuit_breaker::{CircuitBreaker, CircuitBreakerConfig},
    config::Config,
    handlers::{self, AppState},
    initialize_db_pool, metrics,
    tracing::{TracingConfig, init_tracing, shutdown_tracing},
    validation,
    workers::UpdateEvent,
};
use tokio::sync::{mpsc, watch};

use diesel_migrations::MigrationHarness;
use diesel_migrations::{EmbeddedMigrations, embed_migrations};
pub const MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    dotenvy::dotenv().ok();

    // Initialize distributed tracing (before env_logger if enabled)
    let tracing_config = TracingConfig::from_env();
    let tracer_provider = if tracing_config.enabled {
        init_tracing(&tracing_config)
    } else {
        // Fall back to env_logger when tracing is disabled
        env_logger::init_from_env(env_logger::Env::new().default_filter_or("info"));
        None
    };

    // Load configuration
    let config = Config::from_env().unwrap_or_else(|e| {
        log::error!("Configuration error: {}", e);
        std::process::exit(1);
    });
    let config = Arc::new(config);

    log::info!("Configuration loaded successfully");
    log::info!("Starting HTTP server at http://0.0.0.0:{}", config.port);
    log::info!("Using public url {}", &config.host_url);

    // Initialize security config for validation
    validation::init_security_config(config.security.clone());
    if config.security.skip_ssrf_validation {
        log::warn!("SSRF validation is disabled - this should only be used in development!");
    }

    // Setup crypto provider
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    // Initialize database pool
    let pool = initialize_db_pool(&config.pool).await;
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

    // Create shutdown signal channel for graceful worker termination
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Create circuit breaker for connection pool resilience
    let circuit_breaker = if config.circuit_breaker.enabled {
        log::info!(
            "Circuit breaker enabled with failure_threshold={}, recovery_timeout={}s",
            config.circuit_breaker.failure_threshold,
            config.circuit_breaker.recovery_timeout_secs
        );
        Arc::new(CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: config.circuit_breaker.failure_threshold,
            failure_window: std::time::Duration::from_secs(
                config.circuit_breaker.failure_window_secs,
            ),
            recovery_timeout: std::time::Duration::from_secs(
                config.circuit_breaker.recovery_timeout_secs,
            ),
            success_threshold: config.circuit_breaker.success_threshold,
        }))
    } else {
        log::info!("Circuit breaker disabled");
        // Create a circuit breaker that never trips (very high threshold)
        Arc::new(CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: u32::MAX,
            failure_window: std::time::Duration::from_secs(1),
            recovery_timeout: std::time::Duration::from_secs(1),
            success_threshold: 1,
        }))
    };

    // Build application state
    let app_data = AppState {
        pool: pool.clone(),
        sender,
        action_executor: ActionExecutor::new(ActionContext {
            host_address: config.host_url.clone(),
        }),
        config: config.clone(),
        circuit_breaker,
    };

    let action_context = Arc::new(ActionExecutor::new(ActionContext {
        host_address: config.host_url.clone(),
    }));

    // Spawn worker tasks with shutdown signals and configured intervals
    let p = pool.clone();
    let a = action_context.clone();
    let shutdown_rx_start = shutdown_rx.clone();
    let start_interval = config.worker.loop_interval;
    let start_handle = actix_web::rt::spawn(async move {
        task_runner::workers::start_loop(a.as_ref(), p, start_interval, shutdown_rx_start).await;
    });

    let p2 = pool.clone();
    let a2 = action_context.clone();
    let shutdown_rx_timeout = shutdown_rx.clone();
    let timeout_interval = config.worker.timeout_check_interval;
    let timeout_handle = actix_web::rt::spawn(async move {
        task_runner::workers::timeout_loop(a2, p2, timeout_interval, shutdown_rx_timeout).await;
    });

    let p3 = pool.clone();
    let shutdown_rx_batch = shutdown_rx.clone();
    let batch_flush_interval = config.worker.batch_flush_interval;
    let batch_handle = actix_web::rt::spawn(async move {
        task_runner::workers::batch_updater(p3, receiver, batch_flush_interval, shutdown_rx_batch)
            .await;
    });

    let p4 = pool.clone();
    let shutdown_rx_retention = shutdown_rx.clone();
    let retention_config = config.retention.clone();
    let retention_handle = actix_web::rt::spawn(async move {
        task_runner::workers::retention_cleanup_loop(p4, retention_config, shutdown_rx_retention)
            .await;
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
    let server_result = HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(app_data.clone()))
            .wrap(prometheus.clone())
            .wrap(middleware::Logger::default())
            .configure(handlers::configure_routes)
    })
    .bind(("0.0.0.0", port))?
    .shutdown_timeout(30) // Wait up to 30 seconds for graceful shutdown
    .run()
    .await;

    // Signal workers to shut down and wait for them
    log::info!("HTTP server stopped, signaling workers to shut down");
    let _ = shutdown_tx.send(true);

    let shutdown_timeout = std::time::Duration::from_secs(10);
    let _ = tokio::time::timeout(shutdown_timeout, async {
        let _ = start_handle.await;
        let _ = timeout_handle.await;
        let _ = batch_handle.await;
        let _ = retention_handle.await;
    })
    .await;
    log::info!("All workers shut down");

    // Shutdown tracing provider gracefully
    shutdown_tracing(tracer_provider);

    server_result
}
