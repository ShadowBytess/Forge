//! Glue between the async backend world (tokio) and the GTK main loop.
//!
//! Pattern: heavy work runs on the global tokio runtime; completion handlers
//! run on the GTK main thread via [`gtk::glib::spawn_future_local`], which
//! awaits the tokio `JoinHandle`. Streaming events (transactions) use a
//! plain `tokio::sync::mpsc` consumed by a local future.

use gtk4 as gtk;

use gtk::glib;

/// Spawns a future on the global tokio runtime (fire-and-forget).
pub fn spawn<F>(future: F)
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    crate::util::runtime::global().spawn(future);
}

/// Runs `future` on the tokio runtime; when it completes, `then` runs on the
/// GTK main thread with the result.
///
/// Panics inside the future surface as an empty default via `unwrap_or_else`.
pub fn backend_task<T, Fut, Then>(future: Fut, then: Then)
where
    T: Send + 'static,
    Fut: std::future::Future<Output = T> + Send + 'static,
    Then: FnOnce(T) + 'static,
{
    let handle = spawn_handle(future);
    glib::spawn_future_local(async move {
        let result = handle
            .await
            .unwrap_or_else(|join_err| panic!("backend task failed: {join_err}"));
        then(result);
    });
}

fn spawn_handle<F>(future: F) -> tokio::task::JoinHandle<F::Output>
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    crate::util::runtime::global().spawn(future)
}

/// Runs `future` on the runtime and calls `then` on the main thread,
/// tolerating a cancelled/panicked task by passing `None`.
pub fn try_backend_task<T, Fut, Then>(future: Fut, then: Then)
where
    T: Send + 'static,
    Fut: std::future::Future<Output = T> + Send + 'static,
    Then: FnOnce(Option<T>) + 'static,
{
    let handle = spawn_handle(future);
    glib::spawn_future_local(async move {
        then(handle.await.ok());
    });
}
