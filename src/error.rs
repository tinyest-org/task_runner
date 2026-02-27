//! Error types for the ArcRun application.
//!
//! This module defines strongly-typed errors for better error handling
//! and more informative error messages.

use actix_web::{HttpResponse, ResponseError, http::StatusCode};
use thiserror::Error;

/// Main error type for ArcRun operations.
#[derive(Error, Debug)]
pub enum ArcRunError {
    /// Database-related errors
    #[error("Database error: {0}")]
    Database(#[from] diesel::result::Error),

    /// Connection pool errors
    #[error("Connection pool error: {0}")]
    Pool(String),

    /// Task not found
    #[error("Task not found: {0}")]
    TaskNotFound(uuid::Uuid),

    /// Generic not found
    #[error("Not found: {message}")]
    NotFound { message: String },

    /// Invalid task state for operation
    #[error("Invalid task state: {message}")]
    InvalidState { message: String },

    /// Validation errors
    #[error("Validation error: {0}")]
    Validation(String),

    /// Action execution error
    #[error("Action execution failed: {0}")]
    ActionExecution(String),

    /// Webhook error
    #[error("Webhook error: {0}")]
    Webhook(String),

    /// Dependency error
    #[error("Dependency error: {0}")]
    Dependency(String),

    /// Concurrency rule violation
    #[error("Concurrency limit reached for task kind: {kind}")]
    ConcurrencyLimit { kind: String },

    /// Timeout error
    #[error("Task timeout: {0}")]
    Timeout(uuid::Uuid),

    /// Internal error (unexpected)
    #[error("Internal error: {0}")]
    Internal(String),
}

/// Error type for API operations (converts to HTTP responses).
#[derive(Error, Debug)]
pub enum ApiError {
    #[error("Bad request: {0}")]
    BadRequest(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Conflict: {0}")]
    Conflict(String),

    #[error("Internal server error: {0}")]
    InternalServerError(String),
}

impl ResponseError for ApiError {
    fn status_code(&self) -> StatusCode {
        match self {
            ApiError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::NotFound(_) => StatusCode::NOT_FOUND,
            ApiError::Conflict(_) => StatusCode::CONFLICT,
            ApiError::InternalServerError(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn error_response(&self) -> HttpResponse {
        let status = self.status_code();
        HttpResponse::build(status).json(serde_json::json!({
            "error": self.to_string(),
            "status": status.as_u16()
        }))
    }
}

impl From<ArcRunError> for ApiError {
    fn from(err: ArcRunError) -> Self {
        match err {
            ArcRunError::TaskNotFound(id) => ApiError::NotFound(format!("Task {} not found", id)),
            ArcRunError::NotFound { message } => ApiError::NotFound(message),
            ArcRunError::InvalidState { message } => ApiError::BadRequest(message),
            ArcRunError::Validation(e) => ApiError::BadRequest(e),
            ArcRunError::ConcurrencyLimit { kind } => {
                ApiError::Conflict(format!("Concurrency limit reached for kind: {}", kind))
            }
            ArcRunError::Database(e) => {
                log::error!("Database error: {}", e);
                ApiError::InternalServerError("Database error".to_string())
            }
            ArcRunError::Pool(e) => {
                log::error!("Pool error: {}", e);
                ApiError::InternalServerError("Connection pool error".to_string())
            }
            ArcRunError::ActionExecution(e) => {
                log::error!("Action execution error: {}", e);
                ApiError::InternalServerError("Action execution failed".to_string())
            }
            ArcRunError::Webhook(e) => {
                log::error!("Webhook error: {}", e);
                ApiError::InternalServerError("Webhook error".to_string())
            }
            ArcRunError::Dependency(e) => {
                log::error!("Dependency error: {}", e);
                ApiError::InternalServerError("Dependency error".to_string())
            }
            ArcRunError::Timeout(id) => {
                log::error!("Task timeout: {}", id);
                ApiError::InternalServerError("Task timeout".to_string())
            }
            ArcRunError::Internal(e) => {
                log::error!("Internal error: {}", e);
                ApiError::InternalServerError("Internal server error".to_string())
            }
        }
    }
}

/// Result type alias for ArcRun operations.
pub type TaskResult<T> = Result<T, ArcRunError>;

/// Result type alias for API operations.
pub type ApiResult<T> = Result<T, ApiError>;
