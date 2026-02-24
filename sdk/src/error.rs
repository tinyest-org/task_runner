use serde::Deserialize;

/// Errors returned by the Task Runner SDK.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// HTTP transport or connection error.
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// The server returned an error status code.
    #[error("API error (HTTP {status}): {message}")]
    Api {
        status: u16,
        message: String,
        details: Option<Vec<String>>,
    },

    /// Failed to deserialize the response body.
    #[error("Deserialization error: {0}")]
    Deserialize(String),
}

/// Structured error body returned by the Task Runner API.
#[derive(Debug, Deserialize)]
pub(crate) struct ApiErrorBody {
    pub error: Option<String>,
    pub details: Option<Vec<String>>,
}

pub type Result<T> = std::result::Result<T, Error>;
