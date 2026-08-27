//! GUI entry point. The full libadwaita interface lives in [`crate::ui`].
//!
//! This module exists so `forge` with no arguments has a stable home while
//! the UI module stays feature-gated.

#[cfg(feature = "gui")]
pub fn launch(cfg: crate::config::Config) -> i32 {
    crate::ui::launch(cfg)
}

#[cfg(not(feature = "gui"))]
pub fn launch(_cfg: crate::config::Config) -> i32 {
    eprintln!("forge: this build has no GUI (rebuild with --features gui)");
    1
}
