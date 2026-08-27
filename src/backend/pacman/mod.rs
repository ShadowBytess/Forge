//! pacman backend.
//!
//! Design rules enforced here:
//!
//! * **pacman resolves everything.** Dependency closures, conflicts and
//!   removal cascades come from `pacman --print-format` / `--recursive`,
//!   never from Forge's own resolver.
//! * All queries run with `--color never`; only machine-stable output is
//!   parsed (see [`parser`]).
//! * Mutating operations go through polkit via
//!   [`crate::authentication::run_privileged`] — no `sudo`, no shell, and
//!   every target name is validated before it ever reaches argv.

pub mod parser;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;

use crate::backend::{Backend, BackendId, Capabilities};
use crate::config::Config;
use crate::error::{ForgeError, Result};
use crate::package::{
    InstallReason, InstallStatus, OptionalDep, Package, PackageId, SizeInfo, Source, UpdateEntry,
};
use crate::transaction::{
    Action, ActionKind, PreviewItem, Reason, TransactionPreview, TxEvent, TxEventSender,
};
use crate::util::cancel::CancelToken;
use crate::util::process::{RunSpec, Stream, resolve_program, run_capture};
use crate::util::{QUERY_TIMEOUT, TRANSACTION_TIMEOUT};

/// Pseudo-target meaning "upgrade everything" (pacman `-Su`).
pub const UPGRADE_ALL_TARGET: &str = "__forge_upgrade_all__";

const INSTALLED_CACHE_TTL: Duration = Duration::from_secs(30);

#[derive(Default)]
struct CacheState {
    installed: Option<(Instant, Vec<Package>)>,
}

pub struct PacmanBackend {
    cfg: Config,
    cache: Arc<Mutex<CacheState>>,
}

impl PacmanBackend {
    pub fn new(cfg: Config) -> Self {
        Self {
            cfg,
            cache: Arc::default(),
        }
    }

    fn base_args() -> Vec<String> {
        vec!["--color".into(), "never".into()]
    }

    /// Runs a read-only pacman query and returns stdout.
    async fn query(&self, args: &[&str]) -> Result<String> {
        let mut full = Self::base_args();
        full.extend(args.iter().map(|s| s.to_string()));
        run_capture(&RunSpec::with_args("pacman", full), QUERY_TIMEOUT).await
    }

    fn block_to_package(&self, block: &parser::InfoBlock, repo_hint: Option<&str>) -> Package {
        let name = block.get("Name").unwrap_or_default().to_string();
        let version = block.get("Version").unwrap_or_default().to_string();
        let opt_depends: Vec<OptionalDep> = block
            .list("Optional Deps")
            .iter()
            .map(|v| {
                let (name, description) = parser::split_opt_dep(v);
                OptionalDep { name, description }
            })
            .collect();

        Package {
            id: name.clone(),
            name,
            version,
            source: Some(Source::Repo {
                repo: block
                    .get("Repository")
                    .or(repo_hint)
                    .unwrap_or_default()
                    .to_string(),
            }),
            description: block.get("Description").unwrap_or_default().to_string(),
            status: InstallStatus::NotInstalled,
            arch: block.get("Architecture").map(String::from),
            size: SizeInfo {
                download: block
                    .get("Download Size")
                    .and_then(|s| crate::util::size::parse_size(s).ok()),
                installed: block
                    .get("Installed Size")
                    .and_then(|s| crate::util::size::parse_size(s).ok()),
            },
            depends: block
                .list("Depends On")
                .iter()
                .map(|d| parser::strip_version_spec(d).to_string())
                .collect(),
            opt_depends,
            conflicts: block
                .list("Conflicts With")
                .iter()
                .map(|c| parser::strip_version_spec(c).to_string())
                .collect(),
            provides: block
                .list("Provides")
                .iter()
                .map(|p| parser::strip_version_spec(p).to_string())
                .collect(),
            replaces: block
                .list("Replaces")
                .iter()
                .map(|r| parser::strip_version_spec(r).to_string())
                .collect(),
            homepage: block.get("URL").filter(|u| !u.is_empty()).map(String::from),
            licenses: block.list("Licenses"),
            maintainer: block.get("Packager").map(String::from),
            ..Default::default()
        }
    }

    /// `-Si` blocks for one or more sync packages (single invocation).
    async fn sync_info_blocks(&self, names: &[String]) -> Result<Vec<parser::InfoBlock>> {
        if names.is_empty() {
            return Ok(Vec::new());
        }
        for n in names {
            crate::util::validate::ensure_valid_identifier(n)?;
        }
        let mut args = vec!["--sync", "--info"];
        args.extend(names.iter().map(|n| n.as_str()));
        let text = self.query(&args).await?;
        Ok(parser::parse_info_blocks(&text))
    }

    /// Full local database listing (cached briefly).
    pub async fn local_packages(&self) -> Result<Vec<Package>> {
        {
            let cache = self.cache.lock().expect("pacman cache");
            if let Some((at, pkgs)) = &cache.installed {
                if at.elapsed() < INSTALLED_CACHE_TTL {
                    return Ok(pkgs.clone());
                }
            }
        }
        let text = self.query(&["--query", "--info"]).await?;
        let mut out = Vec::new();
        for block in parser::parse_info_blocks(&text) {
            let mut pkg = self.block_to_package(&block, Some("local"));
            let version = pkg.version.clone();
            pkg.status = InstallStatus::Installed {
                version,
                reason: match block.get("Install Reason") {
                    Some(r) if r.contains("dependency") => Some(InstallReason::Dependency),
                    _ => Some(InstallReason::Explicit),
                },
            };
            // Local-only packages are foreign (AUR candidates).
            if let Some(Source::Repo { repo }) = &mut pkg.source {
                *repo = "local".to_string();
            }
            pkg.size.installed = block
                .get("Installed Size")
                .and_then(|s| crate::util::size::parse_size(s).ok());
            out.push(pkg);
        }
        let mut cache = self.cache.lock().expect("pacman cache");
        cache.installed = Some((Instant::now(), out.clone()));
        Ok(out)
    }

    /// File list of an installed package (`pacman -Qlq`).
    pub async fn package_files(&self, id: &str) -> Result<Vec<String>> {
        crate::util::validate::ensure_valid_identifier(id)?;
        let text = self
            .query(&["--query", "--list", "--quiet", "--", id])
            .await?;
        Ok(text
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect())
    }

    /// Names of foreign packages (`pacman -Qmq`) — AUR candidates.
    pub async fn foreign_names(&self) -> Result<Vec<String>> {
        let text = self.query(&["--query", "--foreign", "--quiet"]).await?;
        Ok(text
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect())
    }

    pub async fn invalidate_installed_cache(&self) {
        self.cache.lock().expect("pacman cache").installed = None;
    }

    /// Validates every name before it may reach pacman's argv.
    pub fn validate_targets(names: &[String]) -> Result<()> {
        names
            .iter()
            .try_for_each(|n| crate::util::validate::ensure_valid_identifier(n))
    }
}

fn install_reason_to_status(reason: Option<InstallReason>, version: String) -> InstallStatus {
    InstallStatus::Installed { version, reason }
}

#[async_trait]
impl Backend for PacmanBackend {
    fn id(&self) -> BackendId {
        BackendId::Pacman
    }

    fn name(&self) -> &'static str {
        "Arch repositories (pacman)"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            install: true,
            remove: true,
            upgrade: true,
            reinstall: true,
        }
    }

    async fn available(&self) -> bool {
        resolve_program("pacman").is_ok()
    }

    async fn search(&self, query: &str) -> Result<Vec<Package>> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let text = self.query(&["--sync", "--search", "--", query]).await?;
        let rows = parser::parse_search_output(&text);

        // Prefer configured repositories first, then alphabetical.
        let pref: Vec<String> = self.cfg.sources.preferred_repositories.clone();
        let mut packages: Vec<Package> = rows
            .into_iter()
            .map(|row| Package {
                id: row.name.clone(),
                name: row.name.clone(),
                version: row.version.clone(),
                source: Some(Source::Repo { repo: row.repo }),
                description: row.description,
                status: match row.installed_version {
                    Some(v) => install_reason_to_status(None, v),
                    None => InstallStatus::NotInstalled,
                },
                ..Default::default()
            })
            .collect();
        packages.sort_by(|a, b| {
            let rank = |p: &Package| match &p.source {
                Some(Source::Repo { repo }) => pref.iter().position(|r| r == repo),
                _ => None,
            };
            let ra = rank(a).unwrap_or(usize::MAX);
            let rb = rank(b).unwrap_or(usize::MAX);
            (ra, &a.name).cmp(&(rb, &b.name))
        });
        Ok(packages)
    }

    async fn info(&self, id: &PackageId) -> Result<Option<Package>> {
        crate::util::validate::ensure_valid_identifier(id)?;
        let sync_blocks = self.sync_info_blocks(std::slice::from_ref(id)).await?;
        if let Some(block) = sync_blocks.into_iter().next() {
            let mut package = self.block_to_package(&block, None);
            // Merge install state from the local database.
            if let Ok(locals) = self.local_packages().await {
                if let Some(local) = locals.iter().find(|l| l.id == package.id) {
                    package.status = local.status.clone();
                }
            }
            return Ok(Some(package));
        }
        // Not in any sync repo; maybe it is a locally-installed package.
        let text = self.query(&["--query", "--info", "--", id]).await?;
        Ok(parser::parse_info_blocks(&text)
            .into_iter()
            .next()
            .map(|block| {
                let mut p = self.block_to_package(&block, Some("local"));
                let version = p.version.clone();
                p.status = install_reason_to_status(
                    match block.get("Install Reason") {
                        Some(r) if r.contains("dependency") => Some(InstallReason::Dependency),
                        _ => Some(InstallReason::Explicit),
                    },
                    version,
                );
                p
            }))
    }

    async fn installed(&self) -> Result<Vec<Package>> {
        self.local_packages().await
    }

    async fn updates(&self) -> Result<Vec<UpdateEntry>> {
        // pacman exits 1 when there are no upgrades; treat that as empty.
        let (text, _status) = crate::util::process::run_capture_with_status(
            &RunSpec::with_args("pacman", {
                let mut full = Self::base_args();
                full.extend(["--query".to_string(), "--upgrades".to_string()]);
                full
            }),
            QUERY_TIMEOUT,
        )
        .await?;
        let rows = parser::parse_qu_output(&text);
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        let names: Vec<String> = rows.iter().map(|r| r.name.clone()).collect();
        let blocks = self.sync_info_blocks(&names).await.unwrap_or_default();
        let mut entries = Vec::new();
        for row in rows {
            let mut package = blocks
                .iter()
                .find(|b| b.get("Name") == Some(row.name.as_str()))
                .map(|b| self.block_to_package(b, row.repo.as_deref()))
                .unwrap_or_else(|| Package {
                    id: row.name.clone(),
                    name: row.name.clone(),
                    version: row.new.clone(),
                    source: Some(Source::Repo {
                        repo: row.repo.clone().unwrap_or_default(),
                    }),
                    ..Default::default()
                });
            package.status = InstallStatus::Installed {
                version: row.current.clone(),
                reason: None,
            };
            entries.push(UpdateEntry {
                package,
                current_version: row.current,
                new_version: row.new,
            });
        }
        Ok(entries)
    }

    async fn preview(&self, actions: &[Action]) -> Result<TransactionPreview> {
        let mut plan = TransactionPreview {
            needs_privileges: true,
            ..Default::default()
        };

        let installs: Vec<Action> = actions
            .iter()
            .filter(|a| {
                matches!(
                    a.kind,
                    ActionKind::Install | ActionKind::Upgrade | ActionKind::Reinstall
                )
            })
            .cloned()
            .collect();
        let removes: Vec<Action> = actions
            .iter()
            .filter(|a| a.kind == ActionKind::Remove)
            .cloned()
            .collect();

        // ---------- install / upgrade side ----------
        if !installs.is_empty() {
            let upgrade_all = installs.iter().any(|a| a.id == UPGRADE_ALL_TARGET);
            let resolved: Vec<parser::PlanRow> = if upgrade_all {
                self.updates()
                    .await?
                    .into_iter()
                    .map(|u| parser::PlanRow {
                        repo: u.package.source_label(),
                        name: u.package.name.clone(),
                        version: u.new_version.clone(),
                    })
                    .collect()
            } else {
                let targets: Vec<String> = installs.iter().map(|a| a.id.clone()).collect();
                Self::validate_targets(&targets)?;
                let mut args = vec!["--sync", "--print", "--print-format", "%r\t%n\t%v", "--"];
                args.extend(targets.iter().map(|t| t.as_str()));
                parser::parse_plan_output(&self.query(&args).await?)
            };

            let requested: std::collections::HashSet<&str> =
                installs.iter().map(|a| a.id.as_str()).collect();
            let names: Vec<String> = resolved.iter().map(|r| r.name.clone()).collect();
            let blocks = self.sync_info_blocks(&names).await?;

            for row in resolved {
                let block = blocks
                    .iter()
                    .find(|b| b.get("Name") == Some(row.name.as_str()));
                let action_kind = if requested.contains(row.name.as_str()) {
                    installs
                        .iter()
                        .find(|a| a.id == row.name)
                        .map(|a| a.kind)
                        .unwrap_or(ActionKind::Install)
                } else {
                    ActionKind::Install
                };
                plan.items.push(PreviewItem {
                    action: Action {
                        kind: action_kind,
                        backend: self.id(),
                        id: row.name.clone(),
                        name: row.name.clone(),
                        version: Some(row.version.clone()),
                    },
                    reason: if requested.contains(row.name.as_str()) {
                        Reason::Target
                    } else {
                        Reason::Dependency
                    },
                    current_version: None,
                    new_version: Some(row.version.clone()),
                    download_size: block
                        .and_then(|b| b.get("Download Size"))
                        .and_then(|s| crate::util::size::parse_size(s).ok()),
                    installed_size: block
                        .and_then(|b| b.get("Installed Size"))
                        .and_then(|s| crate::util::size::parse_size(s).ok()),
                });
            }

            // Surface conflicts among the resolved set.
            for b in &blocks {
                let conflicts = b.list("Conflicts With");
                if conflicts.is_empty() {
                    continue;
                }
                let name = b.get("Name").unwrap_or_default().to_string();
                for c in conflicts {
                    let bare = parser::strip_version_spec(&c);
                    if names.iter().any(|n| n.as_str() == bare) || requested.contains(bare) {
                        plan.warnings.push(format!("{name} conflicts with {bare}"));
                    }
                }
            }
        }

        // ---------- removal side ----------
        if !removes.is_empty() {
            let targets: Vec<String> = removes.iter().map(|a| a.id.clone()).collect();
            Self::validate_targets(&targets)?;
            // Ask pacman what -Rs would take with it. The printed set IS the
            // removal plan; nothing is decided by Forge.
            // Ask pacman what -Rs would take with it. The printed set IS
            // the removal plan; nothing is decided by Forge.
            let mut args = vec![
                "--remove",
                "--recursive",
                "--print",
                "--print-format",
                "%n",
                "--",
            ];
            args.extend(targets.iter().map(|t| t.as_str()));
            let (recursive_ok, text) = match self.query(&args).await {
                Ok(text) => (true, text),
                Err(recursive_error) => {
                    // Recursive removal refused (e.g. another package needs
                    // a target). Fall back to the plain removal set so the
                    // user still gets a preview, and surface pacman's reason.
                    plan.warnings.push(format!(
                        "pacman rejected recursive removal: {recursive_error}"
                    ));
                    let mut plain = vec!["--remove", "--print", "--print-format", "%n", "--"];
                    plain.extend(targets.iter().map(|t| t.as_str()));
                    let text = self.query(&plain).await.map_err(|e| {
                        crate::error::ForgeError::InvalidInput(format!(
                            "pacman cannot remove the requested package(s): {e}"
                        ))
                    })?;
                    (false, text)
                }
            };
            let _ = recursive_ok;
            let removed_names: Vec<String> = text
                .lines()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect();

            let requested: std::collections::HashSet<&str> =
                targets.iter().map(|s| s.as_str()).collect();
            let cascade: Vec<&str> = removed_names
                .iter()
                .map(|s| s.as_str())
                .filter(|n| !requested.contains(n))
                .collect();

            for name in &removed_names {
                let action = removes
                    .iter()
                    .find(|a| a.id == *name)
                    .cloned()
                    .unwrap_or_else(|| Action {
                        kind: ActionKind::Remove,
                        backend: self.id(),
                        id: name.clone(),
                        name: name.clone(),
                        version: None,
                    });
                let is_target = requested.contains(name.as_str());
                plan.items.push(PreviewItem {
                    action,
                    reason: if is_target {
                        Reason::Target
                    } else {
                        Reason::NoLongerNeeded
                    },
                    current_version: None,
                    new_version: None,
                    download_size: None,
                    installed_size: None,
                });
            }
            if !cascade.is_empty() {
                plan.warnings.push(format!(
                    "{} unneeded dependency(ies) will also be removed: {}",
                    cascade.len(),
                    cascade.join(", ")
                ));
            }
        }

        Ok(plan)
    }

    async fn execute(
        &self,
        actions: Vec<Action>,
        events: TxEventSender,
        cancel: CancelToken,
    ) -> Result<()> {
        use crate::authentication::run_privileged;

        let has_reinstall = actions.iter().any(|a| a.kind == ActionKind::Reinstall);
        let upgrades_all = actions.iter().any(|a| a.id == UPGRADE_ALL_TARGET);
        let installs: Vec<&Action> = actions
            .iter()
            .filter(|a| {
                matches!(
                    a.kind,
                    ActionKind::Install | ActionKind::Upgrade | ActionKind::Reinstall
                )
            })
            .collect();
        let removes: Vec<&Action> = actions
            .iter()
            .filter(|a| a.kind == ActionKind::Remove)
            .collect();

        let mut result: Result<()> = Ok(());

        if upgrades_all || !installs.is_empty() {
            let mut args: Vec<String> = ["--sync", "--noconfirm"]
                .iter()
                .map(|s| s.to_string())
                .collect();
            if !has_reinstall {
                args.push("--needed".into());
            }
            if upgrades_all {
                args.push("--refresh".into());
                args.push("--sysupgrade".into());
            } else {
                let mut names: Vec<String> = installs.iter().map(|a| a.id.clone()).collect();
                names.sort();
                names.dedup();
                Self::validate_targets(&names)?;
                args.push("--".into());
                args.extend(names);
            }
            result = track_pacman_progress(
                run_privileged(
                    "pacman",
                    &args,
                    &events,
                    &cancel,
                    TRANSACTION_TIMEOUT,
                    None,
                    |stream, line| emit_step_events(&events, stream, &line),
                ),
                &events,
            )
            .await;
        }

        if result.is_ok() && !removes.is_empty() {
            let mut names: Vec<String> = removes.iter().map(|a| a.id.clone()).collect();
            names.sort();
            names.dedup();
            Self::validate_targets(&names)?;
            let mut args: Vec<String> = ["--remove", "--recursive", "--noconfirm"]
                .iter()
                .map(|s| s.to_string())
                .collect();
            args.push("--".into());
            args.extend(names);
            result = track_pacman_progress(
                run_privileged(
                    "pacman",
                    &args,
                    &events,
                    &cancel,
                    TRANSACTION_TIMEOUT,
                    None,
                    |stream, line| emit_step_events(&events, stream, &line),
                ),
                &events,
            )
            .await;
        }

        self.invalidate_installed_cache().await;
        result
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Maps pacman's progress lines to structured [`TxEvent`]s.
pub(crate) fn emit_step_events(events: &TxEventSender, stream: Stream, line: &str) {
    let trimmed = line.trim_start();
    if trimmed.starts_with('(') {
        // "(1/3) installing gtk4"
        if let Some(rest) = trimmed.strip_prefix('(') {
            if let Some((_counter, rest)) = rest.split_once(')') {
                let mut tokens = rest.trim().splitn(2, char::is_whitespace);
                if let (Some(verb), Some(pkg)) = (tokens.next(), tokens.next()) {
                    let kind = match verb {
                        "installing" => Some(ActionKind::Install),
                        "reinstalling" => Some(ActionKind::Reinstall),
                        "upgrading" | "downgrading" => Some(ActionKind::Upgrade),
                        "removing" => Some(ActionKind::Remove),
                        _ => None,
                    };
                    if let Some(kind) = kind {
                        let _ = events.send(TxEvent::StepStarted {
                            action: Action {
                                kind,
                                backend: BackendId::Pacman,
                                id: pkg.to_string(),
                                name: pkg.to_string(),
                                version: None,
                            },
                        });
                        let _ = events.send(TxEvent::StepFinished {
                            name: pkg.to_string(),
                        });
                        return;
                    }
                }
            }
        }
    }
    if let Some(rest) = trimmed.strip_prefix("downloading ") {
        let file = rest.split_whitespace().next().unwrap_or_default();
        let name = file
            .strip_suffix(".pkg.tar.zst")
            .or_else(|| file.strip_suffix(".pkg.tar.xz"))
            .unwrap_or(file);
        let _ = events.send(TxEvent::DownloadProgress {
            name: name.to_string(),
            received: 0,
            total: None,
        });
        return;
    }
    if trimmed.starts_with("warning:") {
        let _ = events.send(TxEvent::Warning {
            message: trimmed.to_string(),
        });
        return;
    }
    if trimmed.starts_with("::") || stream == Stream::Stdout {
        let _ = events.send(TxEvent::Message(trimmed.to_string()));
    }
}

async fn track_pacman_progress(
    fut: impl std::future::Future<Output = Result<()>>,
    events: &TxEventSender,
) -> Result<()> {
    match fut.await {
        Ok(()) => Ok(()),
        Err(e @ (ForgeError::AuthDenied | ForgeError::Cancelled)) => Err(e),
        Err(e) => {
            let _ = events.send(TxEvent::Error {
                message: e.to_string(),
            });
            Err(e)
        }
    }
}

/// Convenience used by the CLI/GUI to build an "upgrade everything" action.
pub fn upgrade_all_action() -> Action {
    Action {
        kind: ActionKind::Upgrade,
        backend: BackendId::Pacman,
        id: UPGRADE_ALL_TARGET.to_string(),
        name: "All repository updates".to_string(),
        version: None,
    }
}

/// Refreshes the sync databases through polkit (`pacman -Sy`).
///
/// Note: Forge always combines this with a subsequent full upgrade on the
/// Updates page; a bare refresh followed by partial installs is discouraged
/// (see README security notes).
pub async fn refresh_databases(events: TxEventSender, cancel: CancelToken) -> Result<()> {
    use crate::authentication::run_privileged;
    let args: Vec<String> = ["--sync", "--refresh", "--noconfirm"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    run_privileged(
        "pacman",
        &args,
        &events,
        &cancel,
        TRANSACTION_TIMEOUT,
        None,
        |_, _| {},
    )
    .await
}
