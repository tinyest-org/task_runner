//! Error types for the task runner application.
//!
//! This module defines strongly-typed errors for better error handling
//! and more informative error messages.

use actix_web::{HttpResponse, ResponseError, http::StatusCode};
use thiserror::Error;

/// Main error type for task runner operations.
#[derive(Error, Debug)]
pub enum TaskRunnerError {
    /// Database-related errors
    #[error("Database error: {0}")]
    Database(#[from] diesel::result::Error),

    /// Connection pool errors
    #[error("Connection pool error: {0}")]
    Pool(String),

    /// Task not found
    #[error("Task not found: {0}")]
    TaskNotFound(uuid::Uuid),

    /// Invalid task state for operation
    #[error("Invalid task state: {message}")]
    InvalidState { message: String },

    /// Validation errors
    #[error("Validation error: {0}")]
    Validation(#[from] ValidationError),

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

/// Validation-specific errors with detailed field information.
#[derive(Error, Debug)]
pub enum ValidationError {
    #[error("Field '{field}' is required")]
    Required { field: String },

    #[error("Field '{field}' is invalid: {reason}")]
    Invalid { field: String, reason: String },

    #[error("Field '{field}' exceeds maximum length of {max_length}")]
    TooLong { field: String, max_length: usize },

    #[error("Field '{field}' must be positive")]
    MustBePositive { field: String },

    #[error("Field '{field}' is out of range: {reason}")]
    OutOfRange { field: String, reason: String },

    #[error("Circular dependency detected: {details}")]
    CircularDependency { details: String },

    #[error("Self-referencing dependency in task '{task_id}'")]
    SelfReference { task_id: String },

    #[error("Duplicate dependency: '{dep_id}' in task '{task_id}'")]
    DuplicateDependency { task_id: String, dep_id: String },

    #[error("Unknown dependency: '{dep_id}' not found")]
    UnknownDependency { dep_id: String },

    #[error("Invalid URL: {url}")]
    InvalidUrl { url: String },

    #[error("Invalid status transition from {from} to {to}")]
    InvalidStatusTransition { from: String, to: String },

    #[error("Multiple validation errors: {0:?}")]
    Multiple(Vec<String>),
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

impl From<TaskRunnerError> for ApiError {
    fn from(err: TaskRunnerError) -> Self {
        match err {
            TaskRunnerError::TaskNotFound(id) => {
                ApiError::NotFound(format!("Task {} not found", id))
            }
            TaskRunnerError::InvalidState { message } => ApiError::BadRequest(message),
            TaskRunnerError::Validation(e) => ApiError::BadRequest(e.to_string()),
            TaskRunnerError::ConcurrencyLimit { kind } => {
                ApiError::Conflict(format!("Concurrency limit reached for kind: {}", kind))
            }
            TaskRunnerError::Database(e) => {
                log::error!("Database error: {}", e);
                ApiError::InternalServerError("Database error".to_string())
            }
            TaskRunnerError::Pool(e) => {
                log::error!("Pool error: {}", e);
                ApiError::InternalServerError("Connection pool error".to_string())
            }
            _ => {
                log::error!("Internal error: {}", err);
                ApiError::InternalServerError("Internal server error".to_string())
            }
        }
    }
}

/// Result type alias for task runner operations.
pub type TaskResult<T> = Result<T, TaskRunnerError>;

/// Result type alias for API operations.
pub type ApiResult<T> = Result<T, ApiError>;
