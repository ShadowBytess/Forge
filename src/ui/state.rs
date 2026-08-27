//! Shared application state for the GUI.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use libadwaita as adw;

use crate::backend::{BackendId, Registry, aur::AurBackend};
use crate::config::Config;
use crate::transaction::TxEvent;
use crate::util::cancel::CancelToken;

/// Everything the pages need to talk to each other and to the backends.
pub struct AppState {
    pub registry: Registry,
    cfg: RefCell<Config>,
    window: RefCell<Option<adw::ApplicationWindow>>,
    toast: RefCell<Option<adw::ToastOverlay>>,
    pub(crate) active_cancel: RefCell<Option<CancelToken>>,
    txn_sink: RefCell<Option<Box<dyn FnMut(TxEvent)>>>,
    txn_done_hooks: RefCell<Vec<Box<dyn Fn()>>>,
    pub(crate) busy: Cell<bool>,
}

pub type App = Rc<AppState>;

impl AppState {
    pub fn new(cfg: Config) -> App {
        Rc::new(Self {
            registry: Registry::with_config(&cfg),
            cfg: RefCell::new(cfg),
            window: RefCell::new(None),
            toast: RefCell::new(None),
            active_cancel: RefCell::new(None),
            txn_sink: RefCell::new(None),
            txn_done_hooks: RefCell::new(Vec::new()),
            busy: Cell::new(false),
        })
    }

    pub fn config(&self) -> Config {
        self.cfg.borrow().clone()
    }

    pub fn set_config(&self, cfg: Config) {
        *self.cfg.borrow_mut() = cfg;
    }

    pub fn save_config(&self) {
        if let Err(e) = self.cfg.borrow().save() {
            tracing::warn!("failed to save config: {e}");
        }
    }

    pub fn set_window(&self, window: &adw::ApplicationWindow) {
        *self.window.borrow_mut() = Some(window.clone());
    }

    pub fn window(&self) -> Option<adw::ApplicationWindow> {
        self.window.borrow().clone()
    }

    pub fn set_toast_overlay(&self, toast: &adw::ToastOverlay) {
        *self.toast.borrow_mut() = Some(toast.clone());
    }

    /// Shows a transient toast notification.
    pub fn toast(&self, message: &str) {
        if let Some(toast) = self.toast.borrow().as_ref() {
            toast.add_toast(adw::Toast::new(message));
        } else {
            tracing::info!("toast: {message}");
        }
    }

    /// True while a transaction is executing.
    pub fn has_active_transaction(&self) -> bool {
        self.active_cancel.borrow().is_some()
    }

    pub fn cancel_active(&self) -> bool {
        if let Some(token) = self.active_cancel.borrow().as_ref() {
            token.cancel();
            return true;
        }
        false
    }

    pub fn is_busy(&self) -> bool {
        self.busy.get()
    }

    /// Registers the callback the Transactions page uses to render events.
    pub fn set_txn_sink(&self, sink: Box<dyn FnMut(TxEvent)>) {
        *self.txn_sink.borrow_mut() = Some(sink);
    }

    pub(crate) fn emit_txn_event(&self, event: TxEvent) {
        if let Some(sink) = self.txn_sink.borrow_mut().as_mut() {
            sink(event);
        }
    }

    /// Pages register refresh closures here; they run after a transaction.
    pub fn on_transaction_done(&self, hook: Box<dyn Fn()>) {
        self.txn_done_hooks.borrow_mut().push(hook);
    }

    pub(crate) fn notify_transaction_done(&self) {
        let hooks: Vec<_> = self.txn_done_hooks.borrow_mut().drain(..).collect();
        for hook in hooks {
            hook();
        }
    }

    /// Marks an AUR package's PKGBUILD as reviewed (unlocks installation).
    pub fn mark_aur_reviewed(&self, id: &str) {
        if let Some(aur) = self.registry.get(BackendId::Aur) {
            if let Some(b) = aur.as_any().downcast_ref::<AurBackend>() {
                b.mark_reviewed(id);
            }
        }
    }

    /// Runs `f` with a reference to the concrete AUR backend, when enabled.
    pub fn with_aur<R>(&self, f: impl FnOnce(&AurBackend) -> R) -> Option<R> {
        let aur = self.registry.get(BackendId::Aur)?;
        let b = aur.as_any().downcast_ref::<AurBackend>()?;
        Some(f(b))
    }
}
