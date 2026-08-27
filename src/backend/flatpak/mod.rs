//! Flatpak backend (CLI-driven, structured `--columns` output).
//!
//! * User-scope operations run unprivileged — never any root, ever.
//! * System-scope operations go through flatpak itself, which talks to its
//!   own polkit integration; Forge does not wrap them in pkexec.
//! * All output parsing uses the documented tab-separated `--columns`
//!   interface, never prose.

pub mod parser;

use async_trait::async_trait;

use crate::backend::{Backend, BackendId, Capabilities};
use crate::config::Config;
use crate::error::{ForgeError, Result};
use crate::package::{
    InstallScopeOption, InstallStatus, Package, PackageId, SizeInfo, Source, UpdateEntry,
};
use crate::transaction::{
    Action, ActionKind, PreviewItem, Reason, TransactionPreview, TxEvent, TxEventSender,
};
use crate::util::cancel::CancelToken;
use crate::util::process::{RunSpec, Stream, resolve_program, run_capture};
use crate::util::{QUERY_TIMEOUT, TRANSACTION_TIMEOUT};

/// Pseudo-target meaning "update every user/system app".
pub const UPGRADE_ALL_TARGET: &str = "__forge_flatpak_upgrade_all__";

const SCOPE_SUFFIX_USER: &str = "@user";
const SCOPE_SUFFIX_SYSTEM: &str = "@system";

pub struct FlatpakBackend {
    cfg: Config,
}

impl Default for FlatpakBackend {
    fn default() -> Self {
        Self::new(Config::default())
    }
}

impl FlatpakBackend {
    pub fn new(cfg: Config) -> Self {
        Self { cfg }
    }

    /// Scopes the configuration allows us to show/operate on.
    fn scopes(&self) -> Vec<crate::package::InstallScope> {
        let mut out = Vec::new();
        if self.cfg.flatpak.show_user {
            out.push(crate::package::InstallScope::User);
        }
        if self.cfg.flatpak.show_system {
            out.push(crate::package::InstallScope::System);
        }
        out
    }

    fn installed_id(app_id: &str, scope: crate::package::InstallScope) -> String {
        match scope {
            crate::package::InstallScope::User => format!("{app_id}{SCOPE_SUFFIX_USER}"),
            crate::package::InstallScope::System => format!("{app_id}{SCOPE_SUFFIX_SYSTEM}"),
        }
    }

    /// Splits a scoped id back into `(bare_app_id, scope)`.
    fn split_scoped_id(id: &str) -> (&str, Option<crate::package::InstallScope>) {
        use crate::package::InstallScope;
        if let Some(bare) = id.strip_suffix(SCOPE_SUFFIX_USER) {
            (bare, Some(InstallScope::User))
        } else if let Some(bare) = id.strip_suffix(SCOPE_SUFFIX_SYSTEM) {
            (bare, Some(InstallScope::System))
        } else {
            (id, None)
        }
    }

    async fn list_scope(
        &self,
        scope: crate::package::InstallScope,
    ) -> Result<Vec<parser::ListRow>> {
        let args: Vec<String> = vec![
            "list".into(),
            scope.flag().to_string(),
            "--app".into(),
            "--columns=application,name,version,branch,origin,installed-size,download-size".into(),
        ];
        let text = run_capture(&RunSpec::with_args("flatpak", args), QUERY_TIMEOUT).await?;
        Ok(parser::parse_list_output(&text))
    }

    fn row_to_package(row: &parser::ListRow, scope: crate::package::InstallScope) -> Package {
        Package {
            id: Self::installed_id(&row.app_id, scope),
            name: if row.name.is_empty() {
                row.app_id.clone()
            } else {
                row.name.clone()
            },
            version: row.version.clone(),
            source: Some(Source::Flatpak {
                remote: row.origin.clone(),
                scope: Some(scope),
            }),
            description: String::new(),
            status: InstallStatus::Installed {
                version: row.version.clone(),
                reason: None,
            },
            arch: None,
            size: SizeInfo {
                download: row.download_size,
                installed: row.installed_size,
            },
            ..Default::default()
        }
    }
}

#[async_trait]
impl Backend for FlatpakBackend {
    fn id(&self) -> BackendId {
        BackendId::Flatpak
    }

    fn name(&self) -> &'static str {
        "Flatpak"
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
        resolve_program("flatpak").is_ok()
    }

    async fn search(&self, query: &str) -> Result<Vec<Package>> {
        let query = query.trim();
        if query.len() < 2 {
            return Ok(Vec::new());
        }
        let mut args: Vec<String> = vec![
            "search".into(),
            "--columns=application,name,version,branch,remotes,description".into(),
        ];
        args.push(query.to_string());
        let text = run_capture(&RunSpec::with_args("flatpak", args), QUERY_TIMEOUT).await?;
        let installed_ids: std::collections::HashSet<String> = self
            .installed()
            .await
            .unwrap_or_default()
            .iter()
            .map(|p| p.id.clone())
            .collect();

        Ok(parser::parse_search_output(&text)
            .into_iter()
            .map(|row| {
                // A search hit has no installation scope yet; mark it
                // installed if either scope already has it.
                let is_installed = installed_ids.contains(&Self::installed_id(
                    &row.app_id,
                    crate::package::InstallScope::User,
                )) || installed_ids.contains(&Self::installed_id(
                    &row.app_id,
                    crate::package::InstallScope::System,
                ));
                Package {
                    id: row.app_id.clone(),
                    name: if row.name.is_empty() {
                        row.app_id.clone()
                    } else {
                        row.name
                    },
                    version: row.version.clone(),
                    source: Some(Source::Flatpak {
                        remote: row.origin.clone(),
                        scope: None,
                    }),
                    description: row.description,
                    status: if is_installed {
                        InstallStatus::Installed {
                            version: row.version.clone(),
                            reason: None,
                        }
                    } else {
                        InstallStatus::NotInstalled
                    },
                    ..Default::default()
                }
            })
            .collect())
    }

    async fn info(&self, id: &PackageId) -> Result<Option<Package>> {
        let (bare, scope) = Self::split_scoped_id(id);
        crate::util::validate::ensure_valid_identifier(bare)?;
        let scope: InstallScopeOption = scope;

        // Structured metadata from `flatpak info`.
        let mut info_args: Vec<String> = vec!["info".into()];
        if let Some(s) = scope {
            info_args.push(s.flag().into());
        }
        info_args.push(bare.into());
        let info_text = run_capture(&RunSpec::with_args("flatpak", info_args), QUERY_TIMEOUT)
            .await
            .ok();
        let block = info_text.as_deref().map(parser::parse_info_output);

        // Description/name via search (the CLI's appstream window).
        let description = {
            let s_args: Vec<String> = vec![
                "search".into(),
                "--columns=application,name,version,branch,remotes,description".into(),
                bare.into(),
            ];
            run_capture(&RunSpec::with_args("flatpak", s_args), QUERY_TIMEOUT)
                .await
                .ok()
                .and_then(|text| {
                    parser::parse_search_output(&text)
                        .into_iter()
                        .find(|r| r.app_id == bare)
                        .map(|r| r.description)
                })
        };

        let version = block
            .as_ref()
            .and_then(|b| b.get("Version"))
            .unwrap_or_default()
            .to_string();
        let origin = block
            .as_ref()
            .and_then(|b| b.get("Origin"))
            .unwrap_or_default()
            .to_string();
        let installation = block
            .as_ref()
            .and_then(|b| b.get("Installation"))
            .unwrap_or("");
        let resolved_scope = scope.or(match installation.to_ascii_lowercase().as_str() {
            "user" => Some(crate::package::InstallScope::User),
            "system" => Some(crate::package::InstallScope::System),
            _ => None,
        });

        if block.is_none() && origin.is_empty() {
            return Ok(None);
        }

        Ok(Some(Package {
            id: id.clone(),
            name: bare.to_string(),
            version: version.clone(),
            source: Some(Source::Flatpak {
                remote: origin,
                scope: resolved_scope,
            }),
            description: description.unwrap_or_default(),
            status: InstallStatus::Installed {
                version,
                reason: None,
            },
            size: SizeInfo {
                download: block
                    .as_ref()
                    .and_then(|b| b.get("Download size"))
                    .or_else(|| block.as_ref().and_then(|b| b.get("Installed")))
                    .and_then(|s| crate::util::size::parse_size(s).ok()),
                installed: block
                    .as_ref()
                    .and_then(|b| b.get("Installed"))
                    .and_then(|s| crate::util::size::parse_size(s).ok()),
            },
            homepage: None,
            licenses: block
                .as_ref()
                .and_then(|b| b.get("License"))
                .map(|l| l.split_whitespace().map(String::from).collect())
                .unwrap_or_default(),
            maintainer: None,
            ..Default::default()
        }))
    }

    async fn installed(&self) -> Result<Vec<Package>> {
        let results = futures::future::join_all(self.scopes().into_iter().map(async |scope| {
            self.list_scope(scope).await.map(|rows| {
                rows.iter()
                    .map(|r| Self::row_to_package(r, scope))
                    .collect::<Vec<_>>()
            })
        }))
        .await;
        let mut out = Vec::new();
        for res in results {
            match res {
                Ok(mut pkgs) => out.append(&mut pkgs),
                Err(e) => tracing::debug!("flatpak scope listing failed: {e}"),
            }
        }
        Ok(out)
    }

    async fn updates(&self) -> Result<Vec<UpdateEntry>> {
        let installed = self.installed().await?;
        let current: std::collections::HashMap<String, &Package> =
            installed.iter().map(|p| (p.id.clone(), p)).collect();

        let per_scope = futures::future::join_all(self.scopes().into_iter().map(async |scope| {
            let args: Vec<String> = vec![
                "remote-ls".into(),
                "--updates".into(),
                "--app".into(),
                scope.flag().to_string(),
                "--columns=application,version,branch,origin".into(),
            ];
            (
                scope,
                run_capture(&RunSpec::with_args("flatpak", args), QUERY_TIMEOUT).await,
            )
        }))
        .await;

        let mut entries = Vec::new();
        for (scope, text) in per_scope {
            let Ok(text) = text else { continue };
            for row in parser::parse_remote_ls_output(&text) {
                let scoped = Self::installed_id(&row.app_id, scope);
                let package = Package {
                    id: scoped.clone(),
                    name: row.app_id.clone(),
                    version: row.version.clone(),
                    source: Some(Source::Flatpak {
                        remote: row.origin.clone(),
                        scope: Some(scope),
                    }),
                    status: InstallStatus::Installed {
                        version: current
                            .get(&scoped)
                            .map(|p| p.version.clone())
                            .unwrap_or_default(),
                        reason: None,
                    },
                    ..Default::default()
                };
                let current_version = current
                    .get(&scoped)
                    .map(|p| p.version.clone())
                    .unwrap_or_default();
                entries.push(UpdateEntry {
                    package,
                    current_version,
                    new_version: row.version,
                });
            }
        }
        Ok(entries)
    }

    async fn preview(&self, actions: &[Action]) -> Result<TransactionPreview> {
        let mut plan = TransactionPreview::default();
        for action in actions {
            let (_, scope) = Self::split_scoped_id(&action.id);
            plan.needs_privileges |= !matches!(scope, Some(crate::package::InstallScope::User));
            plan.items.push(PreviewItem {
                action: action.clone(),
                reason: Reason::Target,
                current_version: None,
                new_version: action.version.clone(),
                download_size: None,
                installed_size: None,
            });
            match scope {
                Some(crate::package::InstallScope::User) => {
                    plan.warnings.push(format!(
                        "{}: per-user Flatpak operation (no root required)",
                        action.name
                    ));
                }
                _ => {
                    plan.warnings.push(format!(
                        "{}: system-wide Flatpak operation (polkit authentication may be requested)",
                        action.name
                    ));
                }
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
        use crate::package::InstallScope;

        let upgrade_all = actions.iter().any(|a| a.id.starts_with(UPGRADE_ALL_TARGET));
        let mut result: Result<()> = Ok(());

        if !actions.is_empty() {
            let _ = events.send(TxEvent::BatchStarted {
                backend: self.id(),
                steps: actions.len(),
            });
            let spec = |args: Vec<String>| RunSpec::with_args("flatpak", args);

            async fn run(spec: RunSpec, cancel: CancelToken, events: TxEventSender) -> Result<()> {
                crate::util::process::run_streamed(&spec, &cancel, TRANSACTION_TIMEOUT, {
                    move |stream, line| {
                        if stream == Stream::Stderr {
                            let _ = events.send(TxEvent::OutputLine {
                                line,
                                is_stderr: true,
                            });
                        } else {
                            let _ = events.send(TxEvent::Message(line));
                        }
                    }
                })
                .await
            }

            if upgrade_all {
                for scope in self.scopes() {
                    result = run(
                        spec(vec![
                            "update".into(),
                            "--noninteractive".into(),
                            scope.flag().into(),
                        ]),
                        cancel.clone(),
                        events.clone(),
                    )
                    .await;
                    if result.is_err() {
                        break;
                    }
                }
            } else {
                for action in &actions {
                    if cancel.is_cancelled() {
                        result = Err(ForgeError::Cancelled);
                        break;
                    }
                    let (bare, scope) = Self::split_scoped_id(&action.id);
                    crate::util::validate::ensure_valid_identifier(bare)?;
                    let flag = scope.unwrap_or(InstallScope::User).flag();
                    let _ = events.send(TxEvent::StepStarted {
                        action: action.clone(),
                    });
                    let args = match action.kind {
                        ActionKind::Install => vec![
                            "install".into(),
                            "--noninteractive".into(),
                            "--or-update".into(),
                            flag.into(),
                            bare.into(),
                        ],
                        ActionKind::Upgrade => vec![
                            "update".into(),
                            "--noninteractive".into(),
                            flag.into(),
                            bare.into(),
                        ],
                        ActionKind::Remove => vec![
                            "uninstall".into(),
                            "--noninteractive".into(),
                            flag.into(),
                            bare.into(),
                        ],
                        ActionKind::Reinstall => vec![
                            "install".into(),
                            "--noninteractive".into(),
                            "--force-update".into(),
                            flag.into(),
                            bare.into(),
                        ],
                    };
                    result = run(spec(args), cancel.clone(), events.clone()).await;
                    if result.is_ok() {
                        let _ = events.send(TxEvent::StepFinished {
                            name: action.name.clone(),
                        });
                    } else {
                        break;
                    }
                }
            }
        }

        result
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
