//! The shared package data model rendered by both the CLI and the GUI.

use serde::{Deserialize, Serialize};

use crate::backend::BackendId;

/// A unique identifier for a package within its backend
/// (pacman name, AUR pkgbase name, Flatpak app id or ref).
pub type PackageId = String;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    /// Official repository package (repo name carried along: `core`, `extra`, ...).
    Repo { repo: String },
    /// Arch User Repository package.
    Aur,
    /// Flatpak application or runtime; scope may be unknown for bare search hits.
    Flatpak {
        remote: String,
        scope: InstallScopeOption,
    },
}

impl Source {
    pub fn backend(&self) -> BackendId {
        match self {
            Source::Repo { .. } => BackendId::Pacman,
            Source::Aur => BackendId::Aur,
            Source::Flatpak { .. } => BackendId::Flatpak,
        }
    }

    /// Short human label, e.g. `extra`, `AUR` or `flathub (system)`.
    pub fn label(&self) -> String {
        match self {
            Source::Repo { repo } => repo.clone(),
            Source::Aur => "AUR".to_string(),
            Source::Flatpak { remote, scope } => match scope {
                Some(scope) => format!("{remote} ({})", scope.label()),
                None => remote.clone(),
            },
        }
    }
}

/// Whether a Flatpak install is machine-wide or per-user.
///
/// `None` means the scope is not yet known (e.g. a bare search hit before
/// an info lookup) and must not be displayed as authoritative.
pub type InstallScopeOption = Option<InstallScope>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InstallScope {
    System,
    User,
}

impl InstallScope {
    pub fn label(&self) -> &'static str {
        match self {
            InstallScope::System => "system-wide",
            InstallScope::User => "per-user",
        }
    }

    /// CLI flag for the flatpak tool; user operations need no privileges.
    pub fn flag(&self) -> &'static str {
        match self {
            InstallScope::System => "--system",
            InstallScope::User => "--user",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallReason {
    Explicit,
    Dependency,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallStatus {
    NotInstalled,
    Installed {
        version: String,
        reason: Option<InstallReason>,
    },
}

impl Default for InstallStatus {
    fn default() -> Self {
        InstallStatus::NotInstalled
    }
}

impl InstallStatus {
    pub fn installed_version(&self) -> Option<&str> {
        match self {
            InstallStatus::NotInstalled => None,
            InstallStatus::Installed { version, .. } => Some(version),
        }
    }

    pub fn is_installed(&self) -> bool {
        matches!(self, InstallStatus::Installed { .. })
    }
}

/// Download / installed sizes in bytes when a backend exposes them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SizeInfo {
    pub download: Option<u64>,
    pub installed: Option<u64>,
}

/// An optional dependency with its human description (`"gtk4: GUI support"`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OptionalDep {
    pub name: String,
    pub description: Option<String>,
}

/// The universal package record. Backends fill what they know; the UI renders
/// whatever is present.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Package {
    pub id: PackageId,
    pub name: String,
    pub version: String,
    pub source: Option<Source>,
    pub description: String,
    pub status: InstallStatus,
    pub arch: Option<String>,
    pub size: SizeInfo,
    pub depends: Vec<String>,
    pub opt_depends: Vec<OptionalDep>,
    pub conflicts: Vec<String>,
    pub provides: Vec<String>,
    pub replaces: Vec<String>,
    pub homepage: Option<String>,
    pub licenses: Vec<String>,
    pub maintainer: Option<String>,
    /// AUR-only metadata.
    pub num_votes: Option<u64>,
    pub popularity: Option<f64>,
    pub out_of_date: bool,
}

impl Package {
    pub fn source_label(&self) -> String {
        self.source
            .as_ref()
            .map(Source::label)
            .unwrap_or_else(|| "?".into())
    }

    pub fn is_installed(&self) -> bool {
        self.status.is_installed()
    }

    /// Trust level used for warnings in the UI.
    pub fn trust_label(&self) -> &'static str {
        match &self.source {
            Some(Source::Repo { .. }) => "Official repository",
            Some(Source::Aur) => "Community (AUR) — not vetted by Arch Linux",
            Some(Source::Flatpak { .. }) => "Flathub / Flatpak remote",
            None => "Unknown source",
        }
    }
}

/// An available update for one installed package.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UpdateEntry {
    pub package: Package,
    pub current_version: String,
    pub new_version: String,
}

impl UpdateEntry {
    pub fn download_size(&self) -> Option<u64> {
        self.package.size.download
    }
}

/// Which packages a search should cover.
#[derive(Clone, Debug)]
pub struct SearchFilter {
    /// Only these backends (empty = all enabled backends).
    pub backends: Vec<BackendId>,
    /// Installed-status restriction.
    pub status: StatusFilter,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusFilter {
    Any,
    InstalledOnly,
    NotInstalledOnly,
}

impl Default for SearchFilter {
    fn default() -> Self {
        Self {
            backends: Vec::new(),
            status: StatusFilter::Any,
        }
    }
}
