//! Safe, shell-free execution of external package tools.
//!
//! Every backend goes through [`run_streamed`] / [`run_capture`] here. The
//! contract is:
//!
//! * Programs are resolved from `PATH` by hand and spawned directly with
//!   `tokio::process::Command`. **No shell is ever involved**, so user input
//!   can never be interpreted by one.
//! * Every argv element that originates from user input must pass
//!   [`crate::util::validate::ensure_valid_identifier`] first.
//! * Privileged commands are wrapped in `pkexec` (see the `authentication`
//!   module), never `sudo`.
//! * Cancellation kills the whole process group.

use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

use crate::error::{ForgeError, Result};
use crate::util::cancel::CancelToken;

/// Everything needed to spawn one external tool safely.
#[derive(Clone, Debug)]
pub struct RunSpec {
    pub program: String,
    pub args: Vec<String>,
    /// Working directory for the child (None = inherit).
    pub cwd: Option<std::path::PathBuf>,
}

impl RunSpec {
    pub fn new(program: impl Into<String>, args: &[&str]) -> Self {
        Self {
            program: program.into(),
            args: args.iter().map(|a| a.to_string()).collect(),
            cwd: None,
        }
    }

    pub fn with_args(program: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            program: program.into(),
            args,
            cwd: None,
        }
    }

    /// Builder-style working directory override (used by makepkg/git flows).
    pub fn cwd(mut self, cwd: Option<std::path::PathBuf>) -> Self {
        self.cwd = cwd;
        self
    }
}

/// Which stream a line came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

/// Locates `program` on `PATH` without spawning anything.
///
/// Returns an error if it cannot be found, so backends can degrade
/// gracefully when e.g. flatpak or makepkg is not installed.
pub fn resolve_program(program: &str) -> Result<std::path::PathBuf> {
    use std::path::PathBuf;
    let candidate = PathBuf::from(program);
    if candidate.is_absolute() {
        if candidate.is_file() {
            return Ok(candidate);
        }
        return Err(ForgeError::ToolMissing(program.to_string()));
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path) {
        let full = dir.join(program);
        // Reject anything surprising that happens to sit on PATH.
        if full.is_file() {
            return Ok(full);
        }
    }
    Err(ForgeError::ToolMissing(program.to_string()))
}

fn base_command(resolved: &std::path::Path, cwd: Option<&std::path::Path>) -> Command {
    let mut cmd = Command::new(resolved);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // Own process group so cancellation can kill tool + its children
    // (important for makepkg, which spawns helper processes).
    cmd.process_group(0);
    cmd
}

/// Signals a whole process group (`kill(2)` with a negative pid).
///
/// SAFETY: this is a plain syscall wrapper; no memory is touched. The pid
/// comes from tokio's spawned child and is guaranteed positive here.
#[allow(unsafe_code)]
fn kill_process_group(pid: u32, sig: i32) {
    // SAFETY: see fn docs.
    #[allow(unsafe_code)]
    unsafe {
        libc::kill(-(pid as i32), sig);
    }
}

async fn pump(
    stream: Stream,
    raw: impl tokio::io::AsyncRead + Unpin + Send + 'static,
    tx: mpsc::Sender<(Stream, String)>,
) {
    let mut lines = BufReader::new(raw).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if tx.send((stream, line)).await.is_err() {
            break;
        }
    }
}

/// Runs `program args...`, streaming every output line to `on_line`.
///
/// Returns the exit status; a non-zero status becomes
/// [`ForgeError::Tool`] with the last stderr lines attached. Honours
/// `cancel` and `timeout`.
pub async fn run_streamed(
    spec: &RunSpec,
    cancel: &CancelToken,
    timeout: Duration,
    mut on_line: impl FnMut(Stream, String) + Send,
) -> Result<()> {
    let resolved = resolve_program(&spec.program)?;
    let mut cmd = base_command(&resolved, spec.cwd.as_deref());
    cmd.args(&spec.args);

    let mut child = cmd.spawn()?;
    let pid = child.id().ok_or_else(|| ForgeError::Tool {
        tool: spec.program.clone(),
        exit: None,
        message: "failed to obtain child pid".into(),
    })?;

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    let (tx, mut rx) = mpsc::channel::<(Stream, String)>(512);
    tokio::spawn(pump(Stream::Stdout, stdout, tx.clone()));
    tokio::spawn(pump(Stream::Stderr, stderr, tx));

    let mut stderr_tail: std::collections::VecDeque<String> = Default::default();
    let result = loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                kill_process_group(pid, libc::SIGTERM);
                if tokio::time::timeout(Duration::from_secs(5), child.wait()).await.is_err() {
                    kill_process_group(pid, libc::SIGKILL);
                    let _ = child.wait().await;
                }
                break Err(ForgeError::Cancelled);
            }
            maybe_line = rx.recv() => match maybe_line {
                Some((stream, line)) => {
                    if stream == Stream::Stderr {
                        stderr_tail.push_back(line.clone());
                        if stderr_tail.len() > 30 {
                            stderr_tail.pop_front();
                        }
                    }
                    on_line(stream, line);
                }
                None => {
                    // Streams closed; collect final status.
                    match child.wait().await {
                        Ok(status) => {
                            if status.success() {
                                break Ok(());
                            }
                            break Err(tool_error(
                                &spec.program.clone(),
                                status.code(),
                                &stderr_tail,
                            ));
                        }
                        Err(e) => break Err(e.into()),
                    }
                }
            },
            _ = tokio::time::sleep(timeout) => {
                kill_process_group(pid, libc::SIGKILL);
                let _ = child.wait().await;
                break Err(ForgeError::Tool {
                    tool: spec.program.clone(),
                    exit: None,
                    message: format!("timed out after {}s", timeout.as_secs()),
                });
            }
        }
    };

    result
}

fn tool_error(
    program: &str,
    code: Option<i32>,
    tail: &std::collections::VecDeque<String>,
) -> ForgeError {
    let mut message = tail.iter().cloned().collect::<Vec<_>>().join("\n");
    if message.trim().is_empty() {
        message = "(no error output)".to_string();
    }
    ForgeError::Tool {
        tool: program.into(),
        exit: code,
        message,
    }
}

/// Runs a command and captures stdout plus the raw exit status.
///
/// Several package tools use non-zero exits to encode "nothing found"
/// (e.g. `pacman -Qu` with no upgrades); callers decide what a status means
/// while still getting the output.
pub async fn run_capture_with_status(
    spec: &RunSpec,
    timeout: Duration,
) -> Result<(String, Option<i32>)> {
    use std::sync::{Arc, Mutex};
    let cancel = CancelToken::new();
    let out: Arc<Mutex<Vec<String>>> = Arc::default();
    let status_code;
    let program = spec.program.clone();
    // Reuse the streaming runner but keep going after non-zero exits.
    let resolved = resolve_program(&spec.program)?;
    let mut cmd = base_command(&resolved, spec.cwd.as_deref());
    cmd.args(&spec.args);
    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");
    let (tx, mut rx) = mpsc::channel::<(Stream, String)>(512);
    tokio::spawn(pump(Stream::Stdout, stdout, tx.clone()));
    tokio::spawn(pump(Stream::Stderr, stderr, tx));
    loop {
        tokio::select! {
            maybe_line = rx.recv() => match maybe_line {
                Some((Stream::Stdout, line)) => out.lock().expect("capture lock").push(line),
                Some(_) => {}
                None => {
                    let st = child.wait().await?;
                    status_code = st.code();
                    break;
                }
            },
            _ = cancel.cancelled() => return Err(ForgeError::Cancelled),
            _ = tokio::time::sleep(timeout) => {
                return Err(ForgeError::Tool {
                    tool: program,
                    exit: None,
                    message: "timed out".into(),
                });
            }
        }
    }
    let text = Arc::try_unwrap(out)
        .map(|buf| {
            buf.into_inner().expect("capture lock").join(
                "
",
            )
        })
        .unwrap_or_default();
    Ok((text, status_code))
}

/// Runs a short command and captures all of stdout (lossy utf-8).
/// Stderr is captured only to build the error message on failure.
pub async fn run_capture(spec: &RunSpec, timeout: Duration) -> Result<String> {
    use std::sync::{Arc, Mutex};
    let cancel = CancelToken::new();
    let out: Arc<Mutex<Vec<String>>> = Arc::default();
    run_streamed(spec, &cancel, timeout, {
        let out = out.clone();
        move |stream, line| {
            if stream == Stream::Stdout {
                out.lock().expect("run_capture lock").push(line);
            }
        }
    })
    .await?;
    Ok(Arc::try_unwrap(out)
        .map(|buf| buf.into_inner().expect("run_capture lock").join("\n"))
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ForgeError;
    use std::sync::Arc;

    #[tokio::test]
    async fn capture_runs_simple_tool() {
        let out = run_capture(&RunSpec::new("echo", &["hello"]), Duration::from_secs(10))
            .await
            .unwrap();
        assert_eq!(out, "hello");
    }

    #[tokio::test]
    async fn missing_program_is_reported() {
        let err = run_capture(
            &RunSpec::new("definitely-not-a-tool-xyz", &[]),
            Duration::from_secs(5),
        )
        .await;
        assert!(matches!(err, Err(ForgeError::ToolMissing(_))));
    }

    #[tokio::test]
    async fn nonzero_exit_becomes_tool_error() {
        // `cat` reads stdin which we set to null => exits non-zero? Actually
        // cat with stdin null prints nothing and exits 0. Use `false`.
        let err = run_capture(&RunSpec::new("false", &[]), Duration::from_secs(5))
            .await
            .unwrap_err();
        match err {
            ForgeError::Tool { tool, exit, .. } => {
                assert_eq!(tool, "false");
                assert_eq!(exit, Some(1));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn streaming_sees_lines() {
        use std::sync::{Arc, Mutex};
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        run_streamed(
            &RunSpec::with_args("printf", vec!["a\nb\nc\n".to_string()]),
            &CancelToken::new(),
            Duration::from_secs(10),
            move |_, line| sink.lock().unwrap().push(line),
        )
        .await
        .unwrap();
        assert_eq!(*seen.lock().unwrap(), vec!["a", "b", "c"]);
    }

    #[tokio::test]
    async fn cancellation_kills_process() {
        let token = CancelToken::new();
        let started = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let s2 = started.clone();
        let spec = RunSpec::with_args("sleep", vec!["60".to_string()]);
        let task_token = token.clone();
        let handle = tokio::spawn(async move {
            run_streamed(&spec, &task_token, Duration::from_secs(120), move |_, _| {
                s2.store(true, std::sync::atomic::Ordering::Relaxed);
            })
            .await
        });
        tokio::time::sleep(Duration::from_millis(300)).await;
        token.cancel();
        let res = tokio::time::timeout(Duration::from_secs(10), handle)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(res, Err(ForgeError::Cancelled)));
    }
}
