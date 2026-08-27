//! The GTK4 / libadwaita interface (feature-gated).

/// GUI entry point (must be called on the main thread).
#[cfg(feature = "gui")]
pub fn launch(cfg: crate::config::Config) -> i32 {
    pages::run_gui(cfg)
}

#[cfg(not(feature = "gui"))]
pub fn launch(_cfg: crate::config::Config) -> i32 {
    eprintln!("forge: this build has no GUI (rebuild with --features gui)");
    1
}

#[cfg(feature = "gui")]
pub mod bridge;
#[cfg(feature = "gui")]
pub mod pages;
#[cfg(feature = "gui")]
pub mod state;
#[cfg(feature = "gui")]
pub mod widgets;
