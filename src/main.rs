//! Forge entry point.
//!
//! * `forge` (no arguments) launches the GUI on the main thread
//!   (GTK requires it).
//! * Every subcommand runs through the same backend architecture as the GUI.

use clap::Parser;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("forge=info")),
        )
        .init();

    let cli = forge::cli::Cli::parse();

    // No subcommand means "launch the UI", which must own the main thread.
    if cli.command.is_none() {
        std::process::exit(forge::cli::gui_entry());
    }

    let code = forge::util::runtime::global().block_on(async move {
        let cli = forge::cli::Cli {
            command: cli.command,
        };
        forge::cli::run(cli).await
    });
    std::process::exit(code);
}
