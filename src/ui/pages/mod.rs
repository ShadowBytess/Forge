//! Window assembly and the shared transaction runner used by all pages.

pub mod browse;
pub mod details;
pub mod home;
pub mod installed;
pub mod settings;
pub mod transactions;
pub mod updates;

use gtk::{Orientation, Stack};
use gtk4 as gtk;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::transaction::{TxEvent, execute as run_transaction};
use crate::ui::state::{App, AppState};

/// Builds and runs the whole application. Must be called on the main thread.
pub fn run_gui(cfg: crate::config::Config) -> i32 {
    let app_state = AppState::new(cfg);
    let app = adw::Application::builder()
        .application_id("io.github.forge.Forge")
        .build();
    {
        let app_state = app_state.clone();
        app.connect_activate(move |application| build_ui(application, &app_state));
    }
    crate::ui::pages::register_current_app(&app_state);
    i32::from(app.run())
}

fn build_ui(application: &adw::Application, app_state: &App) {
    let window = adw::ApplicationWindow::builder()
        .application(application)
        .title("Forge")
        .icon_name("forge")
        .default_width(app_state.config().ui.window_width.max(700))
        .default_height(app_state.config().ui.window_height.max(500))
        .build();
    app_state.set_window(&window);

    match app_state.config().ui.color_scheme.as_str() {
        "light" => adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceLight),
        "dark" => adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark),
        _ => {}
    }

    let toast = adw::ToastOverlay::new();
    app_state.set_toast_overlay(&toast);
    let nav = adw::NavigationView::new();

    // ----- pages ---------------------------------------------------------
    let stack = Stack::new();
    stack.set_vexpand(true);
    stack.set_hhomogeneous(false);

    let (home_page, refresh_all) = home::HomePage::new(app_state);
    stack.add_titled(home_page.root(), Some("home"), "Home");

    let browse_page = browse::BrowsePage::new_repos(app_state, &nav, &toast);
    stack.add_titled(browse_page.root(), Some("browse"), "Browse");
    let aur_page = browse::BrowsePage::new_aur(app_state, &nav, &toast);
    stack.add_titled(aur_page.root(), Some("aur"), "AUR");
    let flatpak_page = browse::BrowsePage::new_flatpak(app_state, &nav, &toast);
    stack.add_titled(flatpak_page.root(), Some("flatpak"), "Flatpak");

    let installed_page = installed::InstalledPage::new(app_state, &nav, &toast);
    stack.add_titled(installed_page.root(), Some("installed"), "Installed");

    let updates_page = updates::UpdatesPage::new(app_state, &nav, &toast);
    stack.add_titled(updates_page.root(), Some("updates"), "Updates");

    let txn_page = transactions::TransactionsPage::new(app_state);
    stack.add_titled(txn_page.root(), Some("transactions"), "Transactions");

    let settings_page = settings::SettingsPage::new(app_state);
    stack.add_titled(settings_page.root(), Some("settings"), "Settings");

    // ----- shell ----------------------------------------------------------
    let sidebar = gtk::StackSidebar::new();
    sidebar.set_stack(&stack);

    let content_box = gtk::Box::new(Orientation::Horizontal, 0);
    content_box.append(&sidebar);
    content_box.append(&stack);

    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&adw::WindowTitle::new("Forge", "Package manager")));

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&content_box));

    let root_page = adw::NavigationPage::new(&toolbar, "Forge");
    nav.push(&root_page);

    toast.set_child(Some(&nav));
    window.set_content(Some(&toast));
    window.present();

    // Kick off initial loads once the loop is running.
    let refresh_updates = updates_page.refresh_closure();
    let refresh_installed = installed_page.refresh_closure();
    gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(60), move || {
        refresh_all();
        refresh_updates();
        refresh_installed();
    });
}

// ---------------------------------------------------------------------------
// Cross-page plumbing
// ---------------------------------------------------------------------------

thread_local! {
    static CURRENT_APP: std::cell::RefCell<Option<App>> = const { std::cell::RefCell::new(None) };
}

fn current_app() -> Option<App> {
    CURRENT_APP.with(|c| c.borrow().clone())
}

pub fn register_current_app(app: &App) {
    CURRENT_APP.with(|c| *c.borrow_mut() = Some(app.clone()));
}

/// Opens the details view for a package from any list row.
pub fn open_details(
    app: &App,
    nav: &adw::NavigationView,
    toast: &adw::ToastOverlay,
    pkg: &crate::package::Package,
) {
    details::open(app, nav, toast, pkg);
}

/// Full preview → confirm → execute flow shared by every page.
///
/// Preview and execution run on the tokio runtime; the confirmation dialog
/// and event rendering stay on the UI thread. Events stream into the
/// Transactions page through the sink registered in [`App`].
pub fn start_transaction(app: &App, actions: Vec<crate::transaction::Action>) {
    if actions.is_empty() {
        return;
    }
    if app.busy.get() {
        app.toast("A transaction is already running");
        return;
    }

    let registry = app.registry.clone();
    let app_for_task = app.clone();
    crate::ui::bridge::backend_task(
        async move {
            match registry.preview(&actions).await {
                Ok(plan) => Ok((plan, actions)),
                Err(e) => Err(e.to_string()),
            }
        },
        move |res| match res {
            Err(msg) => app_for_task.toast(&format!("Preview failed: {msg}")),
            Ok((plan, actions)) => confirm_then_execute(app_for_task, plan, actions),
        },
    );
}

fn confirm_then_execute(
    app: App,
    plan: crate::transaction::TransactionPreview,
    actions: Vec<crate::transaction::Action>,
) {
    let needs_confirmation = app.config().general.confirm_before_transaction;
    if !needs_confirmation {
        execute_confirmed(app, plan, actions);
        return;
    }
    let Some(window) = app.window() else { return };
    let plan_for_dialog = plan.clone();
    crate::ui::widgets::show_preview_dialog(&window, &plan_for_dialog, move || {
        execute_confirmed(app, plan, actions)
    });
}

/// Executes an already-confirmed plan, streaming structured events into the
/// Transactions page and refreshing data when done.
fn execute_confirmed(
    app: App,
    plan: crate::transaction::TransactionPreview,
    _actions: Vec<crate::transaction::Action>,
) {
    if app.busy.get() {
        app.toast("A transaction is already running");
        return;
    }
    app.busy.set(true);
    let cancel = crate::util::cancel::CancelToken::new();
    *app.active_cancel.borrow_mut() = Some(cancel.clone());

    let registry = app.registry.clone();

    // Structured events stream from the backend task into the UI via a
    // local future consuming the tokio receiver.
    let (ev_tok_tx, mut ev_tok_rx) = tokio::sync::mpsc::unbounded_channel::<TxEvent>();
    {
        gtk::glib::spawn_future_local(async move {
            while let Some(event) = ev_tok_rx.recv().await {
                current_app().as_ref().map(|app| app.emit_txn_event(event));
            }
        });
    }

    crate::ui::bridge::try_backend_task(
        async move { run_transaction(&registry, &plan, &ev_tok_tx, cancel).await },
        move |_| {
            app.busy.set(false);
            *app.active_cancel.borrow_mut() = None;
            app.notify_transaction_done();
        },
    );
}
