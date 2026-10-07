use std::time::Duration;

/// Errors returned by the Replane client.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ReplaneError {
    #[error("config not found: {name}")]
    NotFound { name: String },

    #[error("config {name:?} can't be deserialized into the requested type: {source}")]
    Deserialize {
        name: String,
        #[source]
        source: serde_json::Error,
    },

    #[error("Replane connection timed out after {timeout:?}{}", last_error.as_ref().map(|e| format!(" (last error: {e})")).unwrap_or_default())]
    Timeout {
        timeout: Duration,
        last_error: Option<Box<ReplaneError>>,
    },

    #[error("unauthorized: invalid or missing SDK key")]
    Auth,

    #[error("forbidden: {0}")]
    Forbidden(String),

    #[error("server error {status}: {body}")]
    Server { status: u16, body: String },

    #[error("client error {status}: {body}")]
    Client { status: u16, body: String },

    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("unexpected response from Replane: {0}")]
    Protocol(String),

    #[error("invalid configuration: {0}")]
    InvalidOptions(String),
}

impl ReplaneError {
    /// Stable error code shared with the other Replane SDKs.
    pub fn code(&self) -> &'static str {
        match self {
            ReplaneError::NotFound { .. } => "not_found",
            ReplaneError::Deserialize { .. } => "client_error",
            ReplaneError::Timeout { .. } => "timeout",
            ReplaneError::Auth => "auth_error",
            ReplaneError::Forbidden(_) => "forbidden",
            ReplaneError::Server { .. } => "server_error",
            ReplaneError::Client { .. } => "client_error",
            ReplaneError::Network(_) => "network_error",
            ReplaneError::Protocol(_) => "server_error",
            ReplaneError::InvalidOptions(_) => "client_error",
        }
    }
}

pub type Result<T, E = ReplaneError> = std::result::Result<T, E>;
