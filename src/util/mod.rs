//! Small shared utilities: cancellation, safe process spawning, input
//! validation, human-readable sizes and the global runtime.

pub mod cancel;
pub mod process;
pub mod runtime;
pub mod size;
pub mod validate;

use std::time::Duration;

/// Default timeout for short query commands.
pub const QUERY_TIMEOUT: Duration = Duration::from_secs(60);

/// Timeout for long-running package operations (downloads/builds).
pub const TRANSACTION_TIMEOUT: Duration = Duration::from_secs(60 * 45);
