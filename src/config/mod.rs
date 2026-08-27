//! User configuration stored at `~/.config/forge/config.toml`.
//!
//! Every field has a sensible default so a missing or partial file is never
//! an error; unknown keys are ignored to keep forward compatibility.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::{ForgeError, Result};

pub fn default_config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("forge")
        .join("config.toml")
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub general: General,
    pub sources: Sources,
    pub aur: AurSettings,
    pub flatpak: FlatpakSettings,
    pub ui: UiSettings,
    pub download: DownloadSettings,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            general: General::default(),
            sources: Sources::default(),
            aur: AurSettings::default(),
            flatpak: FlatpakSettings::default(),
            ui: UiSettings::default(),
            download: DownloadSettings::default(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct General {
    /// Always show a preview before executing a transaction.
    pub confirm_before_transaction: bool,
}

impl Default for General {
    fn default() -> Self {
        Self {
            confirm_before_transaction: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Sources {
    pub enable_pacman: bool,
    pub enable_aur: bool,
    pub enable_flatpak: bool,
    /// Repositories listed first (and preferred) in search results.
    pub preferred_repositories: Vec<String>,
}

impl Default for Sources {
    fn default() -> Self {
        Self {
            enable_pacman: true,
            enable_aur: true,
            enable_flatpak: true,
            preferred_repositories: vec!["core".into(), "extra".into()],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AurSettings {
    /// Require explicit PKGBUILD review before every AUR install.
    pub require_pkgbuild_review: bool,
    /// Where AUR repos are cloned and built.
    pub build_directory: String,
    /// Show AUR results mixed into global search.
    pub enabled_in_search: bool,
}

impl Default for AurSettings {
    fn default() -> Self {
        Self {
            require_pkgbuild_review: true,
            build_directory: "~/.cache/forge/aur".to_string(),
            enabled_in_search: true,
        }
    }
}

impl AurSettings {
    pub fn expanded_build_directory(&self) -> PathBuf {
        expand_tilde(&self.build_directory)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct FlatpakSettings {
    pub show_system: bool,
    pub show_user: bool,
    pub enabled_in_search: bool,
}

impl Default for FlatpakSettings {
    fn default() -> Self {
        Self {
            show_system: true,
            show_user: true,
            enabled_in_search: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct UiSettings {
    pub default_page: String,
    pub window_width: i32,
    pub window_height: i32,
    /// "system" | "light" | "dark"
    pub color_scheme: String,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            default_page: "home".to_string(),
            window_width: 1150,
            window_height: 760,
            color_scheme: "system".to_string(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DownloadSettings {
    /// Upper bound shown/passed through where supported.
    pub parallel_downloads: u32,
}

impl Default for DownloadSettings {
    fn default() -> Self {
        Self {
            parallel_downloads: 4,
        }
    }
}

/// Expands a leading `~` using the current user's home directory.
pub fn expand_tilde(path: &str) -> PathBuf {
    if path == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from(path));
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(path)
}

impl Config {
    /// Loads the config from `path`, falling back to defaults for any part
    /// that is missing or unreadable. Never fails hard: a broken file is
    /// reported via `tracing` and defaults are used.
    pub fn load_from(path: &std::path::Path) -> Config {
        match std::fs::read_to_string(path) {
            Ok(text) => match toml::from_str(&text) {
                Ok(cfg) => cfg,
                Err(err) => {
                    tracing::warn!("failed to parse {}: {err}; using defaults", path.display());
                    Config::default()
                }
            },
            Err(_) => Config::default(),
        }
    }

    pub fn load() -> Config {
        Self::load_from(&default_config_path())
    }

    /// Writes the config back to disk, creating parent directories.
    pub fn save_to(&self, path: &std::path::Path) -> Result<()> {
        let dir = path.parent().ok_or_else(|| {
            ForgeError::Config(format!("config path {} has no parent", path.display()))
        })?;
        std::fs::create_dir_all(dir)?;
        let body = toml::to_string_pretty(self)
            .map_err(|e| ForgeError::Config(format!("serialize: {e}")))?;
        // Write via temp file then rename for atomicity.
        let tmp = dir.join(".forge-config.tmp");
        std::fs::write(&tmp, body)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&default_config_path())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let cfg = Config::default();
        assert!(cfg.general.confirm_before_transaction);
        assert!(cfg.sources.enable_pacman && cfg.sources.enable_aur);
        assert_eq!(cfg.ui.default_page, "home");
        // Security-relevant defaults:
        assert!(cfg.aur.require_pkgbuild_review);
    }

    #[test]
    fn parses_partial_file_with_defaults() {
        let text = r#"
[ui]
window_width = 800
"#;
        let cfg: Config = toml::from_str(text).unwrap();
        assert_eq!(cfg.ui.window_width, 800);
        assert_eq!(cfg.ui.window_height, 760);
        assert!(cfg.sources.enable_flatpak);
    }

    #[test]
    fn parses_full_document() {
        let text = r#"
[general]
confirm_before_transaction = false

[sources]
enable_pacman = true
enable_aur = true
enable_flatpak = false
preferred_repositories = ["core", "extra", "multilib"]

[aur]
require_pkgbuild_review = false
build_directory = "/tmp/builds"
enabled_in_search = true

[flatpak]
show_system = true
show_user = false
enabled_in_search = true

[ui]
default_page = "updates"
window_width = 1000
window_height = 600
color_scheme = "dark"

[download]
parallel_downloads = 8
"#;
        let cfg: Config = toml::from_str(text).unwrap();
        assert!(!cfg.general.confirm_before_transaction);
        assert!(!cfg.flatpak.show_user);
        assert_eq!(cfg.download.parallel_downloads, 8);
        assert_eq!(cfg.ui.color_scheme, "dark");
        assert_eq!(
            cfg.aur.expanded_build_directory(),
            PathBuf::from("/tmp/builds")
        );
    }

    #[test]
    fn roundtrips_through_toml() {
        let cfg = Config::default();
        let text = toml::to_string_pretty(&cfg).unwrap();
        let parsed: Config = toml::from_str(&text).unwrap();
        assert_eq!(parsed, cfg);
    }

    #[test]
    fn expands_tilde() {
        assert_eq!(
            expand_tilde("~/.cache/x"),
            dirs::home_dir().unwrap().join(".cache/x")
        );
        assert_eq!(expand_tilde("/abs/path"), PathBuf::from("/abs/path"));
    }

    #[test]
    fn save_and_load_roundtrip_on_disk() {
        let dir = std::env::temp_dir().join("forge-test-config");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("nested/config.toml");
        let mut cfg = Config::default();
        cfg.ui.window_width = 640;
        cfg.save_to(&path).unwrap();
        let loaded = Config::load_from(&path);
        assert_eq!(loaded.ui.window_width, 640);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
