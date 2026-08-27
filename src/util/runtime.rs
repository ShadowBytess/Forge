//! Process-wide tokio runtime shared by the CLI and the GTK main loop.

use std::sync::OnceLock;

use tokio::runtime::Runtime;

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

/// Returns (initialising on first use) the global multi-threaded runtime.
pub fn global() -> &'static Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .thread_name("forge-async")
            .build()
            .expect("failed to build tokio runtime")
    })
}
