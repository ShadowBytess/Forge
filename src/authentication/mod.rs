//! Privilege escalation via polkit (`pkexec`).
//!
//! Forge **never** invokes `sudo`, never embeds passwords anywhere, and never
//! runs package tools through a shell. Privileged operations spawn
//!
//! ```text
//! pkexec <program> <arg> <arg> ...
//! ```
//!
//! which triggers the desktop's standard polkit authentication dialog. If the
//! user refuses, pkexec exits 126 and Forge surfaces that as
//! [`ForgeError::AuthDenied`]. Operations that do not require root (AUR
//! builds, user-scope Flatpak, all read-only queries) bypass this module
//! entirely.

use std::time::Duration;

use crate::error::{ForgeError, Result};
use crate::transaction::{TxEvent, TxEventSender};
use crate::util::cancel::CancelToken;
use crate::util::process::{RunSpec, Stream, resolve_program, run_streamed};

/// Exit status pkexec uses when the agent dialog is dismissed / denied.
const PKEXEC_DENIED_EXIT: i32 = 126;

/// True when `pkexec` is available on this system.
pub fn pkexec_available() -> bool {
    resolve_program("pkexec").is_ok()
}

/// Builds the argv for a privilege-escalated invocation.
/// Returned as `(program, args)` — still a plain argv vector, no shell.
pub fn privileged_argv(program: &str, args: &[String]) -> (String, Vec<String>) {
    let mut full = Vec::with_capacity(args.len() + 1);
    full.push(program.to_string());
    full.extend_from_slice(args);
    ("pkexec".to_string(), full)
}

fn classify_pkexec_failure(program: &str, exit: Option<i32>, message: String) -> ForgeError {
    if exit == Some(PKEXEC_DENIED_EXIT) || message.contains("Not authorized") {
        ForgeError::AuthDenied
    } else {
        ForgeError::Tool {
            tool: program.to_string(),
            exit,
            message,
        }
    }
}

/// Runs `program args...` under polkit, streaming output into `on_line`.
///
/// The closure receives both pkexec/pacman stdout and stderr lines; errors
/// carry the tail of stderr for display.
pub async fn run_privileged(
    program: &str,
    args: &[String],
    events: &TxEventSender,
    cancel: &CancelToken,
    timeout: Duration,
    cwd: Option<&std::path::Path>,
    mut on_line: impl FnMut(Stream, String) + Send,
) -> Result<()> {
    debug_assert!(!program.contains('/'), "pkexec requires bare program names");
    let (pkexec, full_args) = privileged_argv(program, args);
    if !pkexec_available() {
        return Err(ForgeError::ToolMissing(pkexec));
    }

    let spec = RunSpec {
        program: pkexec,
        args: full_args,
        cwd: cwd.map(std::borrow::ToOwned::to_owned),
    };
    let mut stderr_tail: Vec<String> = Vec::new();
    let result = run_streamed(&spec, cancel, timeout, |stream, line| {
        if stream == Stream::Stderr {
            stderr_tail.push(line.clone());
            if stderr_tail.len() > 30 {
                stderr_tail.remove(0);
            }
        }
        let is_stderr = stream == Stream::Stderr;
        let _ = events.send(TxEvent::OutputLine {
            line: line.clone(),
            is_stderr,
        });
        on_line(stream, line);
    })
    .await;

    match result {
        Ok(()) => Ok(()),
        Err(ForgeError::Tool {
            tool,
            exit,
            message,
        }) => Err(classify_pkexec_failure(&tool, exit, message)),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_plain_argv_without_shell() {
        let (prog, args) = privileged_argv(
            "pacman",
            &[
                "--sync".into(),
                "--noconfirm".into(),
                "--".into(),
                "firefox".into(),
            ],
        );
        assert_eq!(prog, "pkexec");
        assert_eq!(
            args,
            vec!["pacman", "--sync", "--noconfirm", "--", "firefox"]
        );
        assert!(!args.join(" ").contains(';'));
    }

    #[test]
    fn maps_denied_exit_to_auth_denied() {
        let err = classify_pkexec_failure("pacman", Some(PKEXEC_DENIED_EXIT), "dismissed".into());
        assert!(matches!(err, ForgeError::AuthDenied));
        let err = classify_pkexec_failure("pacman", Some(1), "target not found".into());
        assert!(matches!(err, ForgeError::Tool { .. }));
    }
}
