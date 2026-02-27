use actix_web::{HttpResponse, web};

use super::{AppState, HealthResponse};

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
