//! Deterministic mock backend used by unit tests and UI development.
//!
//! It records every executed action so tests can assert exactly what a
//! transaction would have done, without touching the host system.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::backend::{Backend, BackendId, Capabilities};
use crate::error::Result;
use crate::package::{Package, PackageId, UpdateEntry};
use crate::transaction::{Action, PreviewItem, Reason, TransactionPreview, TxEvent, TxEventSender};
use crate::util::cancel::CancelToken;

#[derive(Default)]
struct Inner {
    executed: Vec<Action>,
}

/// A configurable in-memory backend.
pub struct MockBackend {
    id: BackendId,
    label: &'static str,
    caps: Capabilities,
    packages: Vec<Package>,
    updates: Vec<UpdateEntry>,
    available: bool,
    /// Names for which execute() should emit a failure.
    fail_on: Vec<String>,
    delay: std::time::Duration,
    inner: Arc<Mutex<Inner>>,
}

impl MockBackend {
    pub fn new(id: BackendId) -> Self {
        Self {
            id,
            label: "Mock",
            caps: Capabilities {
                install: true,
                remove: true,
                upgrade: true,
                reinstall: true,
            },
            packages: Vec::new(),
            updates: Vec::new(),
            available: true,
            fail_on: Vec::new(),
            delay: std::time::Duration::from_millis(10),
            inner: Arc::default(),
        }
    }

    #[must_use]
    pub fn with_packages(mut self, packages: Vec<Package>) -> Self {
        self.packages = packages;
        self
    }

    #[must_use]
    pub fn with_updates(mut self, updates: Vec<UpdateEntry>) -> Self {
        self.updates = updates;
        self
    }

    #[must_use]
    pub fn unavailable(mut self) -> Self {
        self.available = false;
        self
    }

    #[must_use]
    pub fn fail_on(mut self, names: &[&str]) -> Self {
        self.fail_on = names.iter().map(|s| s.to_string()).collect();
        self
    }

    #[must_use]
    pub fn with_delay(mut self, d: std::time::Duration) -> Self {
        self.delay = d;
        self
    }

    pub fn executed(&self) -> Vec<Action> {
        self.inner.lock().expect("mock lock").executed.clone()
    }
}

fn pkg(name: &str, version: &str, installed: bool) -> Package {
    let mut p = Package {
        name: name.to_string(),
        version: version.to_string(),
        ..Default::default()
    };
    p.id = name.to_string();
    p.status = if installed {
        crate::package::InstallStatus::Installed {
            version: version.to_string(),
            reason: None,
        }
    } else {
        crate::package::InstallStatus::NotInstalled
    };
    p.source = Some(match name {
        "flatpak-app" => crate::package::Source::Flatpak {
            remote: "flathub".into(),
            scope: Some(crate::package::InstallScope::User),
        },
        _ => crate::package::Source::Repo {
            repo: "extra".into(),
        },
    });
    p.size.download = Some(1_000_000);
    p.size.installed = Some(4_000_000);
    p
}

impl MockBackend {
    /// Standard fixture set used across tests.
    pub fn standard() -> (Self, Vec<Package>) {
        let packages = vec![
            pkg("firefox", "133.0-2", false),
            pkg("gtk4", "4.16.0-1", false),
            pkg("installed-pkg", "1.0-1", true),
            pkg("outdated-pkg", "2.0-1", true),
            pkg("flatpak-app", "1.2.3", false),
        ];
        let updates = vec![UpdateEntry {
            package: pkg("outdated-pkg", "2.1-1", true),
            current_version: "2.0-1".into(),
            new_version: "2.1-1".into(),
        }];
        let backend = MockBackend::new(BackendId::Pacman)
            .with_packages(packages.clone())
            .with_updates(updates);
        (backend, packages)
    }
}

fn find<'a>(packages: &'a [Package], query: &str) -> Vec<Package> {
    let q = query.to_lowercase();
    packages
        .iter()
        .filter(|p| p.name.to_lowercase().contains(&q) || p.description.to_lowercase().contains(&q))
        .cloned()
        .collect()
}

#[async_trait]
impl Backend for MockBackend {
    fn id(&self) -> BackendId {
        self.id
    }

    fn name(&self) -> &'static str {
        self.label
    }

    fn capabilities(&self) -> Capabilities {
        self.caps
    }

    async fn available(&self) -> bool {
        self.available
    }

    async fn search(&self, query: &str) -> Result<Vec<Package>> {
        Ok(find(&self.packages, query))
    }

    async fn info(&self, id: &PackageId) -> Result<Option<Package>> {
        Ok(self.packages.iter().find(|p| &p.id == id).cloned())
    }

    async fn installed(&self) -> Result<Vec<Package>> {
        Ok(self
            .packages
            .iter()
            .filter(|p| p.is_installed())
            .cloned()
            .collect())
    }

    async fn updates(&self) -> Result<Vec<UpdateEntry>> {
        Ok(self.updates.clone())
    }

    async fn preview(&self, actions: &[Action]) -> Result<TransactionPreview> {
        let mut items = Vec::new();
        for action in actions {
            let known = self.packages.iter().find(|p| p.id == action.id);
            items.push(PreviewItem {
                action: action.clone(),
                reason: Reason::Target,
                current_version: known
                    .and_then(|p| p.status.installed_version())
                    .map(String::from),
                new_version: action.version.clone(),
                download_size: known.map(|p| p.size.download).unwrap_or(Some(500_000)),
                installed_size: known.map(|p| p.size.installed).unwrap_or(Some(2_000_000)),
            });
        }
        // A canned dependency row so previews have something to display.
        if actions
            .iter()
            .any(|a| a.kind == crate::transaction::ActionKind::Install)
        {
            let mut dep = pkg("mock-dep", "0.9-1", false);
            dep.name = "mock-dep".into();
            items.push(PreviewItem {
                action: Action {
                    kind: crate::transaction::ActionKind::Install,
                    backend: self.id,
                    id: dep.id.clone(),
                    name: dep.name.clone(),
                    version: Some(dep.version.clone()),
                },
                reason: Reason::Dependency,
                current_version: None,
                new_version: Some(dep.version.clone()),
                download_size: dep.size.download,
                installed_size: dep.size.installed,
            });
        }
        Ok(TransactionPreview {
            items,
            warnings: vec![],
            needs_privileges: self.id == BackendId::Pacman,
        })
    }

    async fn execute(
        &self,
        actions: Vec<Action>,
        events: TxEventSender,
        cancel: CancelToken,
    ) -> Result<()> {
        use crate::transaction::ActionKind;
        let _ = events.send(TxEvent::BatchStarted {
            backend: self.id,
            steps: actions.len(),
        });
        for action in actions {
            if cancel.is_cancelled() {
                return Err(crate::error::ForgeError::Cancelled);
            }
            tokio::time::sleep(self.delay).await;
            let _ = events.send(TxEvent::StepStarted {
                action: action.clone(),
            });

            if self.fail_on.contains(&action.name) || self.fail_on.iter().any(|n| n == "*") {
                let message = format!("mock failure for {}", action.name);
                let _ = events.send(TxEvent::Error {
                    message: message.clone(),
                });
                return Err(crate::error::ForgeError::Tool {
                    tool: format!("mock-{}", self.id),
                    exit: Some(1),
                    message,
                });
            }

            self.inner
                .lock()
                .expect("mock lock")
                .executed
                .push(action.clone());
            match action.kind {
                ActionKind::Remove => {
                    let _ = events.send(TxEvent::Message(format!("removing {}", action.name)));
                }
                _ => {
                    let _ = events.send(TxEvent::DownloadProgress {
                        name: action.name.clone(),
                        received: 500_000,
                        total: Some(1_000_000),
                    });
                }
            }
            let _ = events.send(TxEvent::StepFinished {
                name: action.name.clone(),
            });
        }
        Ok(())
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn search_and_info_work() {
        let (backend, _) = MockBackend::standard();
        let hits = backend.search("fire").await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "firefox");
        let info = backend.info(&"gtk4".to_string()).await.unwrap().unwrap();
        assert_eq!(info.version, "4.16.0-1");
        assert!(!info.is_installed());
    }

    #[tokio::test]
    async fn records_executed_actions() {
        use crate::transaction::TxEventSender;
        let (backend, packages) = MockBackend::standard();
        let firefox = packages.into_iter().find(|p| p.name == "firefox").unwrap();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let sender: TxEventSender = tx;
        backend
            .execute(vec![Action::install(&firefox)], sender, CancelToken::new())
            .await
            .unwrap();
        let done = backend.executed();
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].kind, crate::transaction::ActionKind::Install);
    }
}
