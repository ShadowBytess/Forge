//! Forge: a package-management frontend for Arch Linux.
//!
//! pacman remains the authority for native packages — Forge orchestrates
//! existing tooling (pacman, makepkg, flatpak) through the [`backend`]
//! abstraction instead of reimplementing dependency resolution.

pub mod app;
pub mod authentication;
pub mod backend;
pub mod cache;
pub mod cli;
pub mod config;
pub mod error;
pub mod package;
pub mod transaction;
pub mod ui;
pub mod util;
