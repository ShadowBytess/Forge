//! AUR build workflow: source acquisition, PKGBUILD inspection, dependency
//! preparation and package construction.
//!
//! Security model:
//!
//! * The AUR is **community-produced**. Forge never runs anything without
//!   the user having had the opportunity to read the `PKGBUILD` first
//!   (enforced by [`AurBackend::mark_reviewed`] / config).
//! * Source is fetched over HTTPS from aur.archlinux.org with `git`
//!   (argv only, no shell).
//! * Build dependencies are installed through polkit (`pacman -S --asdeps`)
//!   after being listed in the preview — makepkg itself never escalates.
//! * `makepkg` runs as the unprivileged user; the resulting archive is
//!   installed via polkit (`pacman -U`).

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::{ForgeError, Result};
use crate::transaction::TxEvent;
use crate::transaction::TxEventSender;
use crate::util::cancel::CancelToken;
use crate::util::process::{RunSpec, run_capture, run_streamed};
use crate::util::{QUERY_TIMEOUT, TRANSACTION_TIMEOUT};

/// Where cloned AUR repositories live (`config.aur.build_directory`).
pub fn build_root(cfg: &crate::config::Config) -> PathBuf {
    cfg.aur.expanded_build_directory()
}

fn valid_pkgbase(name: &str) -> bool {
    crate::util::validate::valid_identifier(name)
}

/// Clones (or fast-forwards) the AUR git repo for `pkgbase`, returning the
/// source directory.
pub async fn prepare_source(
    cfg: &crate::config::Config,
    pkgbase: &str,
    events: &TxEventSender,
    cancel: &CancelToken,
) -> Result<PathBuf> {
    if !valid_pkgbase(pkgbase) {
        return Err(ForgeError::InvalidInput(format!(
            "'{pkgbase}' is not a valid AUR package base"
        )));
    }
    let dir = build_root(cfg).join(pkgbase);
    std::fs::create_dir_all(&dir)?;

    let git_dir = dir.join(".git");
    let spec = if git_dir.exists() {
        let _ = events.send(TxEvent::Message(format!("updating {pkgbase} from AUR")));
        RunSpec::with_args(
            "git",
            vec![
                "-C".into(),
                dir.display().to_string(),
                "pull".into(),
                "--ff-only".into(),
            ],
        )
    } else {
        let url = format!("{}/{}.git", crate::backend::aur::rpc::AUR_BASE_URL, pkgbase);
        let _ = events.send(TxEvent::Message(format!("cloning {url}")));
        // Clone into a fresh directory; remove leftovers of a failed clone.
        if dir
            .read_dir()
            .map(|mut i| i.next().is_some())
            .unwrap_or(false)
        {
            std::fs::remove_dir_all(&dir)?;
            std::fs::create_dir_all(&dir)?;
        }
        RunSpec::with_args("git", vec!["clone".into(), url, dir.display().to_string()])
    };

    run_streamed(&spec, cancel, QUERY_TIMEOUT, |_, line| {
        let _ = events.send(TxEvent::OutputLine {
            line,
            is_stderr: false,
        });
    })
    .await?;
    Ok(dir)
}

/// Returns the full text of the checked-out PKGBUILD (for user inspection).
pub fn read_pkgbuild(dir: &Path) -> Result<String> {
    Ok(std::fs::read_to_string(dir.join("PKGBUILD"))?)
}

/// Runs `makepkg --printsrcinfo` and returns the parsed result.
pub async fn generate_srcinfo(dir: &Path) -> Result<crate::backend::aur::srcinfo::SrcInfo> {
    let spec =
        RunSpec::with_args("makepkg", vec!["--printsrcinfo".into()]).cwd(Some(dir.to_path_buf()));
    let text = run_capture(&spec, QUERY_TIMEOUT).await?;
    Ok(crate::backend::aur::srcinfo::parse(&text))
}

/// Names from `pacman -T` that are not satisfied on this system.
pub async fn missing_dependencies(names: &[String]) -> Result<Vec<String>> {
    if names.is_empty() {
        return Ok(Vec::new());
    }
    for n in names {
        crate::util::validate::ensure_valid_identifier(n)?;
    }
    let mut args = vec!["--check".to_string(), "--".to_string()];
    args.extend(names.iter().cloned());
    let spec = RunSpec::with_args("pacman", args);
    // pacman -T exits 1 when every target is already satisfied.
    let (text, _status) =
        crate::util::process::run_capture_with_status(&spec, QUERY_TIMEOUT).await?;
    Ok(text
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

/// Builds the package as the current user (`makepkg --force --noconfirm`).
pub async fn build_package(
    dir: &Path,
    events: &TxEventSender,
    cancel: &CancelToken,
) -> Result<PathBuf> {
    // Archives written up to 5s *before* the build started are ignored so a
    // stale artifact from an earlier session can never be installed.
    let before = std::time::SystemTime::now() - Duration::from_secs(5);
    let spec = RunSpec::with_args(
        "makepkg",
        vec![
            "--force".into(),
            "--noconfirm".into(),
            "--noprogressbar".into(),
        ],
    )
    .cwd(Some(dir.to_path_buf()));
    run_streamed(&spec, cancel, TRANSACTION_TIMEOUT, |_, line| {
        let _ = events.send(TxEvent::OutputLine {
            line,
            is_stderr: false,
        });
    })
    .await?;

    find_artifact(dir, before).ok_or_else(|| ForgeError::Tool {
        tool: "makepkg".into(),
        exit: None,
        message: "no package archive was produced".into(),
    })
}

/// Newest `*.pkg.tar.{zst,xz,gz}` written into `dir` no earlier than
/// `min_time`. Filenames are validated before ever reaching pacman's argv.
pub fn find_artifact(dir: &Path, min_time: std::time::SystemTime) -> Option<PathBuf> {
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        let name = path.file_name()?.to_string_lossy().into_owned();
        let is_archive = name.ends_with(".pkg.tar.zst")
            || name.ends_with(".pkg.tar.xz")
            || name.ends_with(".pkg.tar.gz");
        if !is_archive || name.contains(".sig") {
            continue;
        }
        if !crate::backend::aur::artifact_name_ok(&name) {
            continue;
        }
        let modified = entry.metadata().and_then(|m| m.modified()).ok()?;
        if modified < min_time {
            continue;
        }
        if best.as_ref().is_none_or(|(t, _)| modified > *t) {
            best = Some((modified, path));
        }
    }
    best.map(|(_, p)| p)
}
