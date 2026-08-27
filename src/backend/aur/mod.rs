//! AUR backend: search, details, update detection and the guarded
//! build/install workflow.
//!
//! Trust model — enforced in code, not just documented:
//!
//! * AUR packages are clearly labelled as community-produced everywhere
//!   they appear (see [`Package::trust_label`]).
//! * [`AurBackend::preview`] lists build dependencies and points at the
//!   PKGBUILD; installation is refused unless the user reviewed it
//!   (`config.aur.require_pkgbuild_review` + [`AurBackend::mark_reviewed`]).
//! * Updates are detected with pacman-compatible version comparison
//!   ([`vercmp`] — a pure function, not a resolver).

pub mod build;
pub mod rpc;
pub mod srcinfo;
pub mod vercmp;

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::backend::pacman::PacmanBackend;
use crate::backend::{Backend, BackendId, Capabilities};
use crate::config::Config;
use crate::error::{ForgeError, Result};
use crate::package::{
    InstallStatus, OptionalDep, Package, PackageId, SizeInfo, Source, UpdateEntry,
};
use crate::transaction::{
    Action, ActionKind, PreviewItem, Reason, TransactionPreview, TxEvent, TxEventSender,
};
use crate::util::cancel::CancelToken;
use crate::util::process::Stream;

/// Characters permitted in a built package archive filename.
fn artifact_name_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 200
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+' | '@'))
}

#[derive(Default)]
struct ReviewState {
    reviewed: HashSet<String>,
}

pub struct AurBackend {
    cfg: Config,
    client: rpc::AurClient,
    pacman: Arc<PacmanBackend>,
    review: Arc<Mutex<ReviewState>>,
}

impl AurBackend {
    pub fn new(cfg: Config) -> Self {
        Self {
            pacman: Arc::new(PacmanBackend::new(cfg.clone())),
            cfg,
            client: rpc::AurClient::new(),
            review: Arc::default(),
        }
    }

    fn aur_to_package(&self, info: &rpc::AurInfo) -> Package {
        Package {
            id: info.name.clone(),
            name: info.name.clone(),
            version: info.version.clone(),
            source: Some(Source::Aur),
            description: info.description.clone().unwrap_or_default(),
            status: InstallStatus::NotInstalled,
            arch: Some("any".into()),
            size: SizeInfo::default(),
            depends: info
                .depends
                .iter()
                .map(|d| crate::backend::pacman::parser::strip_version_spec(d).to_string())
                .collect(),
            opt_depends: info
                .opt_depends
                .iter()
                .map(|v| {
                    let (name, description) = crate::backend::pacman::parser::split_opt_dep(v);
                    OptionalDep { name, description }
                })
                .collect(),
            conflicts: info.conflicts.clone(),
            provides: info.provides.clone(),
            replaces: info.replaces.clone(),
            homepage: info.url.clone().filter(|u| u.starts_with("http")),
            licenses: info.license.clone(),
            maintainer: info.maintainer.clone(),
            num_votes: Some(info.num_votes),
            popularity: Some(info.popularity),
            out_of_date: info.out_of_date.is_some(),
        }
    }

    /// Installed-version map for AUR packages (foreign local packages).
    async fn installed_map(&self) -> Result<std::collections::HashMap<String, String>> {
        let foreign = self.pacman.foreign_names().await?;
        let locals = self.pacman.local_packages().await?;
        let mut map = std::collections::HashMap::new();
        for pkg in locals {
            if foreign.contains(&pkg.id) {
                if let Some(v) = pkg.status.installed_version() {
                    map.insert(pkg.id.clone(), v.to_string());
                }
            }
        }
        Ok(map)
    }

    /// Records that the user has reviewed this package's PKGBUILD, unlocking
    /// installation for it.
    pub fn mark_reviewed(&self, id: &str) {
        self.review
            .lock()
            .expect("review lock")
            .reviewed
            .insert(id.to_string());
    }

    pub fn has_been_reviewed(&self, id: &str) -> bool {
        self.review
            .lock()
            .expect("review lock")
            .reviewed
            .contains(id)
    }

    /// Directory containing the checked-out source for `pkgbase`.
    pub fn source_dir(&self, pkgbase: &str) -> std::path::PathBuf {
        build::build_root(&self.cfg).join(pkgbase)
    }

    /// Path of the PKGBUILD if the package was cloned before.
    pub fn pkgbuild_path(&self, id: &str) -> Option<std::path::PathBuf> {
        let dir = self.source_dir(id);
        let path = dir.join("PKGBUILD");
        path.is_file().then_some(path)
    }

    async fn run_privileged_pacman(
        &self,
        args: &[String],
        events: TxEventSender,
        cancel: CancelToken,
    ) -> Result<()> {
        use crate::authentication::run_privileged;
        use crate::util::TRANSACTION_TIMEOUT;
        run_privileged(
            "pacman",
            args,
            &events,
            &cancel,
            TRANSACTION_TIMEOUT,
            None,
            |stream, line| {
                crate::backend::pacman::emit_step_events(&events, stream, &line);
                if stream == Stream::Stderr {
                    let _ = events.send(TxEvent::OutputLine {
                        line,
                        is_stderr: true,
                    });
                }
            },
        )
        .await
    }
}

#[async_trait]
impl Backend for AurBackend {
    fn id(&self) -> BackendId {
        BackendId::Aur
    }

    fn name(&self) -> &'static str {
        "Arch User Repository"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            install: true,
            remove: true,
            upgrade: true,
            reinstall: false,
        }
    }

    async fn available(&self) -> bool {
        self.cfg.sources.enable_aur
    }

    async fn search(&self, query: &str) -> Result<Vec<Package>> {
        let query = query.trim();
        if query.len() < 2 {
            return Ok(Vec::new());
        }
        let results = self.client.search(query).await?;
        let installed = self.installed_map().await.unwrap_or_default();
        let mut packages: Vec<Package> = results
            .iter()
            .map(|info| self.aur_to_package(info))
            .collect();
        for p in &mut packages {
            if let Some(version) = installed.get(&p.id) {
                p.status = InstallStatus::Installed {
                    version: version.clone(),
                    reason: None,
                };
            }
        }
        packages.sort_by(|a, b| {
            b.num_votes
                .unwrap_or(0)
                .cmp(&a.num_votes.unwrap_or(0))
                .then(a.name.cmp(&b.name))
        });
        Ok(packages)
    }

    async fn info(&self, id: &PackageId) -> Result<Option<Package>> {
        crate::util::validate::ensure_valid_identifier(id)?;
        let infos = self.client.info(std::slice::from_ref(id)).await?;
        let Some(info) = infos.into_iter().find(|i| i.name == *id) else {
            return Ok(None);
        };
        let mut package = self.aur_to_package(&info);
        if let Ok(installed) = self.installed_map().await {
            if let Some(version) = installed.get(id) {
                package.status = InstallStatus::Installed {
                    version: version.clone(),
                    reason: None,
                };
            }
        }
        Ok(Some(package))
    }

    async fn installed(&self) -> Result<Vec<Package>> {
        let foreign = self.pacman.foreign_names().await?;
        let locals = self.pacman.local_packages().await?;
        Ok(locals
            .into_iter()
            .filter(|p| foreign.contains(&p.id))
            .map(|mut p| {
                // Foreign local packages are AUR candidates by definition.
                p.source = Some(Source::Aur);
                p
            })
            .collect())
    }

    async fn updates(&self) -> Result<Vec<UpdateEntry>> {
        let installed = self.installed().await?;
        if installed.is_empty() {
            return Ok(Vec::new());
        }
        let names: Vec<String> = installed.iter().map(|p| p.id.clone()).collect();
        let infos = self.client.info(&names).await?;
        let mut entries = Vec::new();
        for pkg in installed {
            let current = match pkg.status.installed_version() {
                Some(v) => v.to_string(),
                None => continue,
            };
            let Some(info) = infos.iter().find(|i| i.name == pkg.id) else {
                continue; // vanished from the AUR
            };
            if vercmp::is_newer(&current, &info.version) {
                entries.push(UpdateEntry {
                    package: self.aur_to_package(info),
                    current_version: current,
                    new_version: info.version.clone(),
                });
            }
        }
        Ok(entries)
    }

    async fn preview(&self, actions: &[Action]) -> Result<TransactionPreview> {
        let mut plan = TransactionPreview::default();

        let installs: Vec<&Action> = actions
            .iter()
            .filter(|a| matches!(a.kind, ActionKind::Install | ActionKind::Upgrade))
            .collect();
        let removes: Vec<Action> = actions
            .iter()
            .filter(|a| a.kind == ActionKind::Remove)
            .cloned()
            .collect();

        if !installs.is_empty() {
            plan.warnings.push(
                "AUR packages are community-produced and NOT vetted by Arch Linux. \
                 Review the PKGBUILD before installing."
                    .to_string(),
            );
            plan.needs_privileges = true;

            let names: Vec<String> = installs.iter().map(|a| a.id.clone()).collect();
            let infos = self.client.info(&names).await?;

            for action in installs {
                let info = infos.iter().find(|i| i.name == action.id);
                let version = info
                    .map(|i| i.version.clone())
                    .or_else(|| action.version.clone());
                let base = info
                    .map(|i| i.package_base.as_str())
                    .unwrap_or(action.id.as_str());

                if self.cfg.aur.require_pkgbuild_review && !self.has_been_reviewed(&action.id) {
                    plan.warnings.push(format!(
                        "{}: PKGBUILD must be reviewed before install",
                        action.id
                    ));
                }
                plan.items.push(PreviewItem {
                    action: action.clone(),
                    reason: Reason::Target,
                    current_version: None,
                    new_version: version,
                    download_size: None,
                    installed_size: None,
                });

                // Build-time dependencies are informational preview rows;
                // they will be offered through pacman during execution.
                if let Some(info) = info {
                    let mut build_deps: Vec<String> = Vec::new();
                    for dep in info.depends.iter().chain(info.make_depends.iter()) {
                        let bare =
                            crate::backend::pacman::parser::strip_version_spec(dep).to_string();
                        if !build_deps.contains(&bare) {
                            build_deps.push(bare);
                        }
                    }
                    for dep in build_deps {
                        plan.items.push(PreviewItem {
                            action: Action {
                                kind: ActionKind::Install,
                                backend: BackendId::Pacman,
                                id: dep.clone(),
                                name: dep,
                                version: None,
                            },
                            reason: Reason::Dependency,
                            current_version: None,
                            new_version: None,
                            download_size: None,
                            installed_size: None,
                        });
                    }
                    plan.warnings.push(format!(
                        "{}: source will be fetched from {} into {}",
                        info.name,
                        rpc::AUR_BASE_URL,
                        self.source_dir(base).display()
                    ));
                }
            }
        }

        if !removes.is_empty() {
            // AUR packages live in pacman's local database; removal is a
            // plain pacman operation, so reuse that backend's preview so the
            // cascade/orphan analysis comes from pacman itself.
            let pacman_actions: Vec<Action> = removes
                .iter()
                .map(|a| Action {
                    backend: BackendId::Pacman,
                    ..a.clone()
                })
                .collect();
            let part = self.pacman.preview(&pacman_actions).await?;
            plan.extend(part);
        }

        Ok(plan)
    }

    async fn execute(
        &self,
        actions: Vec<Action>,
        events: TxEventSender,
        cancel: CancelToken,
    ) -> Result<()> {
        let removes: Vec<String> = actions
            .iter()
            .filter(|a| a.kind == ActionKind::Remove)
            .map(|a| a.id.clone())
            .collect();

        if !removes.is_empty() {
            crate::backend::pacman::PacmanBackend::validate_targets(&removes)?;
            let mut args: Vec<String> = ["--remove", "--recursive", "--noconfirm"]
                .iter()
                .map(|s| s.to_string())
                .collect();
            args.push("--".into());
            args.extend(removes);
            self.run_privileged_pacman(&args, events.clone(), cancel.clone())
                .await?;
            self.pacman.invalidate_installed_cache().await;
        }

        for action in actions
            .iter()
            .filter(|a| matches!(a.kind, ActionKind::Install | ActionKind::Upgrade))
        {
            if cancel.is_cancelled() {
                return Err(ForgeError::Cancelled);
            }
            self.install_one(action, &events, &cancel).await?;
        }

        self.pacman.invalidate_installed_cache().await;
        Ok(())
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl AurBackend {
    async fn install_one(
        &self,
        action: &Action,
        events: &TxEventSender,
        cancel: &CancelToken,
    ) -> Result<()> {
        crate::util::validate::ensure_valid_identifier(&action.id)?;

        // ---- Gate 1: explicit PKGBUILD review ----
        if self.cfg.aur.require_pkgbuild_review && !self.has_been_reviewed(&action.id) {
            let message = format!(
                "{}: refusing to install — the PKGBUILD has not been reviewed. \
                 Open it in Forge or pass --reviewed on the CLI.",
                action.id
            );
            let _ = events.send(TxEvent::Error {
                message: message.clone(),
            });
            return Err(ForgeError::InvalidInput(message));
        }

        let _ = events.send(TxEvent::StepStarted {
            action: action.clone(),
        });
        let result = self.build_and_install(action, events, cancel).await;

        match result {
            Ok(()) => {
                let _ = events.send(TxEvent::StepFinished {
                    name: action.name.clone(),
                });
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    async fn build_and_install(
        &self,
        action: &Action,
        events: &TxEventSender,
        cancel: &CancelToken,
    ) -> Result<()> {
        // ---- Source acquisition ----
        let dir = build::prepare_source(&self.cfg, &action.id, events, cancel).await?;
        let pkgbuild = build::read_pkgbuild(&dir)?;
        let _ = events.send(TxEvent::Message(format!(
            "PKGBUILD ready for inspection: {}",
            dir.join("PKGBUILD").display()
        )));
        let _ = events.send(TxEvent::OutputLine {
            line: format!("PKGBUILD ({} bytes):", pkgbuild.len()),
            is_stderr: false,
        });

        // ---- Structured dependency information from makepkg itself ----
        let srcinfo = build::generate_srcinfo(&dir).await?;
        let mut dep_names: Vec<String> = Vec::new();
        for dep in srcinfo.depends().into_iter().chain(srcinfo.build_depends()) {
            let bare = crate::backend::pacman::parser::strip_version_spec(&dep).to_string();
            crate::util::validate::ensure_valid_identifier(&bare)?;
            if !dep_names.contains(&bare) {
                dep_names.push(bare);
            }
        }

        // ---- Ensure build dependencies via polkit (never makepkg -s) ----
        let missing = build::missing_dependencies(&dep_names).await?;
        if !missing.is_empty() {
            let _ = events.send(TxEvent::Warning {
                message: format!("installing build dependencies: {}", missing.join(", ")),
            });
            let mut args: Vec<String> = vec![
                "--sync".into(),
                "--needed".into(),
                "--noconfirm".into(),
                "--asdeps".into(),
                "--".into(),
            ];
            args.extend(missing);
            self.run_privileged_pacman(&args, events.clone(), cancel.clone())
                .await?;
            self.pacman.invalidate_installed_cache().await;
        }

        // ---- Build as the unprivileged user ----
        let _ = events.send(TxEvent::Message(format!("building {}", action.id)));
        let artifact = build::build_package(&dir, events, cancel).await?;
        let _ = events.send(TxEvent::Message(format!("built {}", artifact.display())));

        // ---- Install the archive through polkit ----
        let mut args: Vec<String> = vec![
            "--upgrade".to_string(),
            "--noconfirm".to_string(),
            "--".to_string(),
        ];
        args.push(artifact.display().to_string());
        self.run_privileged_pacman(&args, events.clone(), cancel.clone())
            .await?;
        Ok(())
    }
}

/// CLI helper: mark several packages as reviewed at once.
impl AurBackend {
    pub fn mark_many_reviewed<I: IntoIterator<Item = S>, S: AsRef<str>>(&self, ids: I) {
        let mut guard = self.review.lock().expect("review lock");
        for id in ids {
            guard.reviewed.insert(id.as_ref().to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_names_are_screened() {
        assert!(artifact_name_ok("yay-12.3.5-1-x86_64.pkg.tar.zst"));
        // Epochs use '_' in file names; ':' never appears.
        assert!(!artifact_name_ok("foo-bar-1:2.0-1-any.pkg.tar.xz"));
        assert!(!artifact_name_ok("evil; rm -rf /"));
        assert!(!artifact_name_ok("$(id).pkg.tar.zst"));
        assert!(!artifact_name_ok(""));
    }

    #[test]
    fn review_gate_starts_closed() {
        let cfg = Config::default();
        let backend = AurBackend::new(cfg);
        assert!(!backend.has_been_reviewed("yay"));
        backend.mark_reviewed("yay");
        assert!(backend.has_been_reviewed("yay"));
        assert!(!backend.has_been_reviewed("paru"));
    }
}
