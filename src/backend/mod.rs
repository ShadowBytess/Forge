//! Backend abstraction: the seam between Forge and package systems.
//!
//! A [`Backend`] wraps one packaging ecosystem (pacman, AUR, Flatpak, ...).
//! The UI and CLI only ever talk to this trait (through the [`Registry`]),
//! so adding a new package format later requires no UI changes.

pub mod aur;
pub mod flatpak;
pub mod mock;
pub mod pacman;

use std::sync::Arc;

use async_trait::async_trait;

use crate::error::Result;
use crate::package::{Package, PackageId, SearchFilter, StatusFilter, UpdateEntry};
use crate::transaction::{Action, TransactionPreview};
use crate::util::cancel::CancelToken;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendId {
    Pacman,
    Aur,
    Flatpak,
}

impl BackendId {
    pub fn label(self) -> &'static str {
        match self {
            BackendId::Pacman => "Repositories",
            BackendId::Aur => "AUR",
            BackendId::Flatpak => "Flatpak",
        }
    }
}

impl std::fmt::Display for BackendId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            BackendId::Pacman => "pacman",
            BackendId::Aur => "aur",
            BackendId::Flatpak => "flatpak",
        })
    }
}

/// What a backend can do. The UI adapts to this instead of hard-coding.
#[derive(Clone, Copy, Debug, Default)]
pub struct Capabilities {
    pub install: bool,
    pub remove: bool,
    pub upgrade: bool,
    pub reinstall: bool,
}

impl Capabilities {
    pub fn supports(&self, kind: crate::transaction::ActionKind) -> bool {
        use crate::transaction::ActionKind;
        match kind {
            ActionKind::Install => self.install,
            ActionKind::Remove => self.remove,
            ActionKind::Upgrade => self.upgrade,
            ActionKind::Reinstall => self.reinstall,
        }
    }
}

/// One packaging ecosystem.
#[async_trait]
pub trait Backend: Send + Sync {
    fn id(&self) -> BackendId;

    /// Human-readable name shown in the UI.
    fn name(&self) -> &'static str;

    fn capabilities(&self) -> Capabilities;

    /// Whether the underlying tool is present on this system.
    /// Backends that are not available are skipped by the registry.
    async fn available(&self) -> bool;

    /// Free-text search across the backend's catalogue.
    async fn search(&self, query: &str) -> Result<Vec<Package>>;

    /// Full metadata for a single package.
    async fn info(&self, id: &PackageId) -> Result<Option<Package>>;

    /// Everything currently installed through this backend.
    async fn installed(&self) -> Result<Vec<Package>>;

    /// Available updates for installed packages.
    async fn updates(&self) -> Result<Vec<UpdateEntry>>;

    /// Ask the *native tool* what it would do — pacman resolves dependencies
    /// for us (`--print-format`), flatpak reports its plan, etc.
    async fn preview(&self, actions: &[Action]) -> Result<TransactionPreview>;

    /// Execute previously previewed actions, emitting structured progress.
    async fn execute(
        &self,
        actions: Vec<Action>,
        events: crate::transaction::TxEventSender,
        cancel: CancelToken,
    ) -> Result<()>;

    /// Downcast support (e.g. reaching `AurBackend::mark_reviewed` from CLI).
    fn as_any(&self) -> &dyn std::any::Any;
}

/// Holds every compiled-in backend and fans queries out to them.
pub struct Registry {
    backends: Vec<Arc<dyn Backend>>,
    enabled: Vec<BackendId>,
}

/// The registry owns shared handles only, so cloning it hands out another
/// reference to the same backends (needed to move it into async tasks).
impl Clone for Registry {
    fn clone(&self) -> Self {
        Self {
            backends: self.backends.clone(),
            enabled: self.enabled.clone(),
        }
    }
}

impl Registry {
    /// Builds the default registry honouring `cfg` visibility settings.
    pub fn with_config(cfg: &crate::config::Config) -> Self {
        let mut backends: Vec<Arc<dyn Backend>> = Vec::new();
        if cfg.sources.enable_pacman {
            backends.push(Arc::new(pacman::PacmanBackend::new(cfg.clone())));
        }
        if cfg.sources.enable_aur {
            backends.push(Arc::new(aur::AurBackend::new(cfg.clone())));
        }
        if cfg.sources.enable_flatpak {
            backends.push(Arc::new(flatpak::FlatpakBackend::new(cfg.clone())));
        }
        Self {
            backends,
            enabled: vec![BackendId::Pacman, BackendId::Aur, BackendId::Flatpak],
        }
    }

    /// Test/CLI-friendly registry from explicit backends.
    pub fn from_backends(backends: Vec<Arc<dyn Backend>>) -> Self {
        let enabled = backends.iter().map(|b| b.id()).collect();
        Self { backends, enabled }
    }

    pub fn set_enabled(&mut self, enabled: Vec<BackendId>) {
        self.enabled = enabled;
    }

    pub fn get(&self, id: BackendId) -> Option<Arc<dyn Backend>> {
        self.backends.iter().find(|b| b.id() == id).cloned()
    }

    pub fn all(&self) -> &[Arc<dyn Backend>] {
        &self.backends
    }

    /// Backends that are both registered, enabled and whose tool exists.
    pub async fn active(&self) -> Vec<Arc<dyn Backend>> {
        let mut out = Vec::new();
        for b in &self.backends {
            if self.enabled.contains(&b.id()) && b.available().await {
                out.push(b.clone());
            }
        }
        out
    }

    /// Concurrent search across active backends; results keep their source
    /// tags so the UI can always show where a package came from.
    pub async fn search(
        &self,
        query: &str,
        filter: &SearchFilter,
    ) -> Vec<(BackendId, Result<Vec<Package>>)> {
        let backends: Vec<_> = self
            .active()
            .await
            .into_iter()
            .filter(|b| filter.backends.is_empty() || filter.backends.contains(&b.id()))
            .collect();
        let futures = backends.into_iter().map(|b| async move {
            let res = b.search(query).await;
            (b.id(), res)
        });
        futures::future::join_all(futures).await
    }

    /// Merged installed list across active backends.
    pub async fn installed_all(&self, _filter: StatusFilter) -> Vec<Package> {
        let results =
            futures::future::join_all(self.active().await.iter().map(|b| b.installed())).await;
        let mut out = Vec::new();
        for res in results {
            match res {
                Ok(mut pkgs) => out.append(&mut pkgs),
                Err(e) => tracing::warn!("installed listing failed: {e}"),
            }
        }
        out
    }

    /// Merged update list across active backends.
    pub async fn updates_all(&self) -> Vec<UpdateEntry> {
        let results =
            futures::future::join_all(self.active().await.iter().map(|b| b.updates())).await;
        let mut out = Vec::new();
        for res in results {
            match res {
                Ok(mut ups) => out.append(&mut ups),
                Err(e) => tracing::warn!("update check failed: {e}"),
            }
        }
        out
    }

    /// Combined preview across all backends referenced by `actions`.
    pub async fn preview(&self, actions: &[Action]) -> Result<TransactionPreview> {
        let mut merged = TransactionPreview::default();
        for backend in self.active().await {
            let subset: Vec<Action> = actions
                .iter()
                .filter(|a| a.backend == backend.id())
                .cloned()
                .collect();
            if subset.is_empty() {
                continue;
            }
            let caps = backend.capabilities();
            if !subset.iter().all(|a| caps.supports(a.kind)) {
                return Err(crate::error::ForgeError::Unsupported(
                    "one of the requested actions",
                ));
            }
            let part = backend.preview(&subset).await?;
            merged.extend(part);
        }
        Ok(merged)
    }
}
