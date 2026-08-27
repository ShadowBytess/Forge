//! Structured progress events streamed from backends to UI/CLI.

use tokio::sync::mpsc::UnboundedSender;

use crate::backend::BackendId;
use crate::transaction::Action;

/// A single progress event during transaction execution.
#[derive(Clone, Debug)]
pub enum TxEvent {
    /// A backend batch is about to run.
    BatchStarted { backend: BackendId, steps: usize },
    /// The native tool started working on one package.
    StepStarted { action: Action },
    /// Byte-level download progress where the tool exposes it.
    DownloadProgress {
        name: String,
        received: u64,
        total: Option<u64>,
    },
    /// A raw output line from the underlying tool (for the log view).
    OutputLine { line: String, is_stderr: bool },
    /// Human-readable status message.
    Message(String),
    /// One package finished successfully.
    StepFinished { name: String },
    /// Something failed but the transaction may continue.
    Warning { message: String },
    /// Fatal error; execution stops.
    Error { message: String },
    /// User cancelled.
    Cancelled,
    /// Terminal event.
    Completed { succeeded: bool, message: String },
}

pub type TxEventSender = UnboundedSender<TxEvent>;

/// Collects a compact textual log of events (used by the CLI and tests).
#[derive(Default)]
pub struct EventLog {
    pub lines: Vec<String>,
}

impl TxEvent {
    pub fn to_log_line(&self) -> Option<String> {
        match self {
            TxEvent::BatchStarted { backend, steps } => {
                Some(format!("== {backend}: {} step(s) ==", steps))
            }
            TxEvent::StepStarted { action } => Some(action.kind.gerund().to_string()),
            TxEvent::DownloadProgress {
                name,
                received,
                total,
            } => match total {
                Some(t) => Some(format!(
                    "downloading {name} ({}/{})",
                    crate::util::size::format_size(*received),
                    crate::util::size::format_size(*t)
                )),
                None => Some(format!("downloading {name}")),
            },
            TxEvent::OutputLine { line, .. } => Some(line.clone()),
            TxEvent::Message(m) => Some(m.clone()),
            TxEvent::StepFinished { name } => Some(format!("done: {name}")),
            TxEvent::Warning { message } => Some(format!("warning: {message}")),
            TxEvent::Error { message } => Some(format!("error: {message}")),
            TxEvent::Cancelled => Some("cancelled".to_string()),
            TxEvent::Completed { succeeded, message } => Some(if *succeeded {
                format!("completed: {message}")
            } else {
                format!("failed: {message}")
            }),
        }
    }
}
