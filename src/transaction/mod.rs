//! Transaction abstraction: install / remove / upgrade / reinstall,
//! expressed uniformly across backends.
//!
//! A [`TransactionPreview`] is what the user confirms; an
//! [`Action`] list is what gets executed. The executor never invents
//! actions on its own — everything it runs was shown to the user first.

pub mod events;

use serde::{Deserialize, Serialize};

use crate::backend::BackendId;
use crate::package::PackageId;
use crate::util::cancel::CancelToken;

pub use events::{TxEvent, TxEventSender};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    Install,
    Remove,
    Upgrade,
    Reinstall,
}

impl ActionKind {
    pub fn label(self) -> &'static str {
        match self {
            ActionKind::Install => "Install",
            ActionKind::Remove => "Remove",
            ActionKind::Upgrade => "Upgrade",
            ActionKind::Reinstall => "Reinstall",
        }
    }

    pub fn gerund(self) -> &'static str {
        match self {
            ActionKind::Install => "Installing",
            ActionKind::Remove => "Removing",
            ActionKind::Upgrade => "Upgrading",
            ActionKind::Reinstall => "Reinstalling",
        }
    }
}

/// One concrete unit of work, tagged with the backend that must perform it.
/// `id` is the tool-facing identifier (pacman name, flatpak app id/ref).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Action {
    pub kind: ActionKind,
    pub backend: BackendId,
    pub id: PackageId,
    /// Display name (usually same as id).
    pub name: String,
    /// Target version where meaningful (upgrade/install).
    pub version: Option<String>,
}

impl Action {
    fn new(kind: ActionKind, pkg: &crate::package::Package) -> Self {
        Self {
            kind,
            backend: pkg
                .source
                .as_ref()
                .map(|s| s.backend())
                .unwrap_or(BackendId::Pacman),
            id: pkg.id.clone(),
            name: pkg.name.clone(),
            version: Some(pkg.version.clone()),
        }
    }

    pub fn install(pkg: &crate::package::Package) -> Self {
        Self::new(ActionKind::Install, pkg)
    }
    pub fn remove(pkg: &crate::package::Package) -> Self {
        Self::new(ActionKind::Remove, pkg)
    }
    pub fn upgrade(pkg: &crate::package::Package) -> Self {
        Self::new(ActionKind::Upgrade, pkg)
    }
    pub fn reinstall(pkg: &crate::package::Package) -> Self {
        Self::new(ActionKind::Reinstall, pkg)
    }

    pub fn summary(&self) -> String {
        match (&self.kind, &self.version) {
            (_, Some(v)) => format!("{} {} ({})", self.kind.label(), self.name, v),
            (_, None) => format!("{} {}", self.kind.label(), self.name),
        }
    }
}

/// Why an item appears in a preview.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// The user explicitly asked for this.
    Target,
    /// Pulled in automatically by the native resolver (pacman deps).
    Dependency,
    /// Removed because it is no longer needed after the change.
    NoLongerNeeded,
    /// Replaces a conflicting package.
    ConflictReplacement,
}

/// One row of a transaction preview.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PreviewItem {
    pub action: Action,
    pub reason: Reason,
    pub current_version: Option<String>,
    pub new_version: Option<String>,
    pub download_size: Option<u64>,
    pub installed_size: Option<u64>,
}

impl PreviewItem {
    pub fn title(&self) -> String {
        match (&self.action.version, self.reason) {
            (Some(v), _) => format!("{}-{}", self.action.name, v),
            (None, _) => self.action.name.clone(),
        }
    }
}

/// Everything the UI shows before the user confirms.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TransactionPreview {
    pub items: Vec<PreviewItem>,
    /// Human-readable notes, e.g. conflict warnings surfaced by pacman.
    pub warnings: Vec<String>,
    /// True when executing requires privilege escalation (polkit).
    pub needs_privileges: bool,
}

impl TransactionPreview {
    pub fn extend(&mut self, other: TransactionPreview) {
        self.items.extend(other.items);
        self.warnings.extend(other.warnings);
        self.needs_privileges |= other.needs_privileges;
    }

    pub fn total_download_size(&self) -> u64 {
        self.items.iter().filter_map(|i| i.download_size).sum()
    }

    pub fn total_installed_size(&self) -> u64 {
        self.items.iter().filter_map(|i| i.installed_size).sum()
    }

    pub fn targets(&self) -> impl Iterator<Item = &PreviewItem> {
        self.items.iter().filter(|i| i.reason == Reason::Target)
    }

    /// Actions that should actually be executed (explicit targets only;
    /// dependencies are resolved again by the native tool at run time).
    pub fn target_actions(&self) -> Vec<Action> {
        self.targets().map(|i| i.action.clone()).collect()
    }

    pub fn count_by_kind(&self, kind: ActionKind) -> usize {
        self.items.iter().filter(|i| i.action.kind == kind).count()
    }
}

/// Outcome of a completed transaction.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ExecutionSummary {
    pub succeeded: bool,
    pub cancelled: bool,
    pub packages_touched: Vec<String>,
    pub message: String,
}

/// Runs a confirmed set of target actions through their backends, streaming
/// [`TxEvent`]s into `events`. This is the single entry point both CLI and
/// GUI use.
pub async fn execute(
    registry: &crate::backend::Registry,
    plan: &TransactionPreview,
    events: &TxEventSender,
    cancel: CancelToken,
) -> crate::error::Result<ExecutionSummary> {
    let actions = plan.target_actions();
    if actions.is_empty() {
        return Ok(ExecutionSummary {
            succeeded: true,
            cancelled: false,
            packages_touched: vec![],
            message: "Nothing to do".into(),
        });
    }

    let mut touched = Vec::new();
    let backends = registry.active().await;

    // Execute one backend batch at a time (pacman first so AUR builds see
    // fresh official packages, Flatpak last).
    for order in [BackendId::Pacman, BackendId::Aur, BackendId::Flatpak] {
        let batch: Vec<Action> = actions
            .iter()
            .filter(|a| a.backend == order)
            .cloned()
            .collect();
        if batch.is_empty() {
            continue;
        }
        let Some(backend) = backends.iter().find(|b| b.id() == order).cloned() else {
            let _ = events.send(TxEvent::Error {
                message: format!("backend {order} is not available"),
            });
            continue;
        };

        let _ = events.send(TxEvent::BatchStarted {
            backend: order,
            steps: batch.len(),
        });
        match backend
            .execute(batch.clone(), events.clone(), cancel.clone())
            .await
        {
            Ok(()) => touched.extend(batch.into_iter().map(|a| a.name)),
            Err(e) if matches!(e, crate::error::ForgeError::Cancelled) => {
                let _ = events.send(TxEvent::Cancelled);
                return Ok(ExecutionSummary {
                    succeeded: false,
                    cancelled: true,
                    packages_touched: touched,
                    message: "Cancelled".into(),
                });
            }
            Err(e) => {
                let _ = events.send(TxEvent::Error {
                    message: e.to_string(),
                });
                let _ = events.send(TxEvent::Completed {
                    succeeded: false,
                    message: e.to_string(),
                });
                return Ok(ExecutionSummary {
                    succeeded: false,
                    cancelled: false,
                    packages_touched: touched,
                    message: e.to_string(),
                });
            }
        }
    }

    let _ = events.send(TxEvent::Completed {
        succeeded: true,
        message: format!("{} package(s) processed", touched.len()),
    });
    Ok(ExecutionSummary {
        succeeded: true,
        cancelled: false,
        packages_touched: touched.clone(),
        message: format!("{} package(s) processed", touched.len()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::Registry;
    use crate::backend::mock::MockBackend;
    use std::sync::Arc;

    /// Builds the plan for `actions` against a mock registry.
    async fn preview_with(backend: Arc<MockBackend>, actions: &[Action]) -> TransactionPreview {
        let reg = Registry::from_backends(vec![backend]);
        reg.preview(actions).await.unwrap()
    }

    #[tokio::test]
    async fn executor_runs_targets_through_backend() {
        let (backend, packages) = MockBackend::standard();
        let backend = Arc::new(backend);
        let firefox = packages
            .iter()
            .find(|p| p.name == "firefox")
            .cloned()
            .unwrap();

        let actions = vec![Action::install(&firefox)];
        let plan = preview_with(backend.clone(), &actions).await;
        // Preview contains target + canned dependency row, but only targets execute.
        assert_eq!(plan.target_actions().len(), 1);
        assert!(plan.total_download_size() > 0);

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let summary = execute(
            &Registry::from_backends(vec![backend.clone()]),
            &plan,
            &tx,
            CancelToken::new(),
        )
        .await
        .unwrap();

        assert!(summary.succeeded);
        assert_eq!(backend.executed().len(), 1);

        // Event stream: batch → step(s) → completed.
        let mut kinds = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            kinds.push(match ev {
                TxEvent::BatchStarted { .. } => "batch",
                TxEvent::StepStarted { .. } => "step-start",
                TxEvent::StepFinished { .. } => "step-done",
                TxEvent::DownloadProgress { .. } => "progress",
                TxEvent::Completed { .. } => "completed",
                _ => "other",
            });
        }
        let _ = &mut kinds;
        assert!(kinds.contains(&"batch"));
        assert!(kinds.contains(&"step-start"));
        assert!(kinds.contains(&"completed"));
    }

    #[tokio::test]
    async fn empty_plan_is_a_noop_success() {
        let plan = TransactionPreview::default();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let reg = Registry::from_backends(vec![Arc::new(MockBackend::standard().0)]);
        let summary = execute(&reg, &plan, &tx, CancelToken::new()).await.unwrap();
        assert!(summary.succeeded);
        assert_eq!(summary.message, "Nothing to do");
    }

    #[tokio::test]
    async fn backend_failure_surfaces_in_summary() {
        let (b, _) = MockBackend::standard();
        let backend = Arc::new(b.fail_on(&["firefox"]));

        let mut p = crate::package::Package::default();
        p.id = "firefox".into();
        p.name = "firefox".into();
        p.version = "1.0".into();
        p.source = Some(crate::package::Source::Repo {
            repo: "extra".into(),
        });
        let actions = vec![Action::install(&p)];
        let plan = preview_with(backend.clone(), &actions).await;

        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let summary = execute(
            &Registry::from_backends(vec![backend]),
            &plan,
            &tx,
            CancelToken::new(),
        )
        .await
        .unwrap();
        assert!(!summary.succeeded);
        assert!(summary.message.contains("mock failure"));
    }

    #[tokio::test]
    async fn cancellation_stops_execution() {
        let (b, packages) = MockBackend::standard();
        let backend = Arc::new(b.with_delay(std::time::Duration::from_millis(100)));
        let targets: Vec<Action> = packages.iter().map(Action::install).collect();
        assert!(targets.len() >= 2);

        let cancel = CancelToken::new();
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            c2.cancel();
        });

        let reg = Registry::from_backends(vec![backend.clone()]);
        let plan = preview_with(backend.clone(), &targets).await;
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let summary = execute(&reg, &plan, &tx, cancel).await.unwrap();

        assert!(summary.cancelled, "expected cancellation, got {summary:?}");
        assert!(backend.executed().len() < targets.len());
    }

    #[test]
    fn preview_aggregates_sizes_and_targets() {
        use crate::package::{Package, Source};
        let mut p = Package::default();
        p.id = "firefox".into();
        p.name = "firefox".into();
        p.version = "133.0-2".into();
        p.source = Some(Source::Repo {
            repo: "extra".into(),
        });
        p.size.installed = Some(250_000_000);
        let item_target = PreviewItem {
            action: Action::install(&p),
            reason: Reason::Target,
            current_version: None,
            new_version: Some(p.version.clone()),
            download_size: Some(80_000_000),
            installed_size: Some(250_000_000),
        };
        let mut dep = Package::default();
        dep.id = "gtk4".into();
        dep.name = "gtk4".into();
        dep.source = Some(Source::Repo {
            repo: "extra".into(),
        });
        let item_dep = PreviewItem {
            action: Action::install(&dep),
            reason: Reason::Dependency,
            current_version: None,
            new_version: Some("4.16.0".into()),
            download_size: Some(6_000_000),
            installed_size: Some(40_000_000),
        };
        let plan = TransactionPreview {
            items: vec![item_target, item_dep],
            warnings: vec!["gtk4 conflicts with gtk3-demo".to_string()],
            needs_privileges: true,
        };
        assert_eq!(plan.total_download_size(), 86_000_000);
        assert_eq!(plan.total_installed_size(), 290_000_000);
        assert_eq!(plan.target_actions().len(), 1, "only targets are executed");
        assert_eq!(plan.count_by_kind(ActionKind::Install), 2);
        assert!(plan.needs_privileges);
    }
}
