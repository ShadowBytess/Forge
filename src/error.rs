//! Central error type shared by all backends, the transaction engine, CLI and GUI.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ForgeError {
    /// An external tool (pacman, flatpak, makepkg, git, ...) failed.
    #[error("{tool} failed (exit {exit:?}): {message}")]
    Tool {
        tool: String,
        exit: Option<i32>,
        message: String,
    },

    /// A required tool is not installed on this system.
    #[error("required tool '{0}' was not found on PATH")]
    ToolMissing(String),

    /// Structured data could not be parsed.
    #[error("failed to parse {context}: {detail}")]
    Parse {
        context: &'static str,
        detail: String,
    },

    /// A network request failed.
    #[error("network error: {0}")]
    Network(String),

    /// Input failed validation before ever reaching a backend tool.
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// The operation was cancelled by the user or a timeout.
    #[error("operation cancelled")]
    Cancelled,

    /// The backend does not support the requested operation.
    #[error("backend does not support: {0}")]
    Unsupported(&'static str),

    /// The user refused the privilege escalation prompt.
    #[error("authentication was denied")]
    AuthDenied,

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("config error: {0}")]
    Config(String),
}

pub type Result<T, E = ForgeError> = std::result::Result<T, E>;

impl From<reqwest::Error> for ForgeError {
    fn from(err: reqwest::Error) -> Self {
        if err.is_timeout() {
            ForgeError::Network(format!("request timed out: {err}"))
        } else if err.is_connect() {
            ForgeError::Network(format!("connection failed: {err}"))
        } else {
            ForgeError::Network(err.to_string())
        }
    }
}
