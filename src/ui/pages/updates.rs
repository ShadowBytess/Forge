//! Updates page: available updates across backends, selective or full apply.

use gtk4 as gtk;
use std::cell::RefCell;
use std::rc::Rc;

use gtk::{Align, Orientation};
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::backend::BackendId;
use crate::package::UpdateEntry;
use crate::transaction::ActionKind;
use crate::ui::state::App;

pub struct UpdatesPage {
    root: adw::Clamp,
    refresh_fn: Rc<RefCell<Box<dyn Fn()>>>,
}

impl UpdatesPage {
    pub fn new(app: &App, nav: &adw::NavigationView, toast: &adw::ToastOverlay) -> Self {
        let root = adw::Clamp::builder().maximum_size(1000).build();
        let content = gtk::Box::new(Orientation::Vertical, 12);
        content.set_margin_top(16);
        content.set_margin_bottom(24);
        content.set_margin_start(16);
        content.set_margin_end(16);
        root.set_child(Some(&content));

        let heading_box = gtk::Box::new(Orientation::Horizontal, 8);
        let heading = gtk::Label::new(Some("Updates"));
        heading.add_css_class("title-2");
        heading.set_halign(Align::Start);
        heading.set_hexpand(true);
        heading_box.append(&heading);

        let refresh_btn = gtk::Button::from_icon_name("view-refresh-symbolic");
        refresh_btn.set_tooltip_text(Some("Check for updates"));
        heading_box.append(&refresh_btn);
        content.append(&heading_box);

        let status_label = Rc::new(gtk::Label::new(Some("Checking…")));
        status_label.add_css_class("dim-label");
        status_label.set_halign(Align::Start);
        content.append(&*status_label);

        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_vexpand(true);
        scrolled.set_min_content_height(380);
        scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        let list_box = gtk::ListBox::new();
        list_box.add_css_class("boxed-list");
        list_box.set_selection_mode(gtk::SelectionMode::None);
        scrolled.set_child(Some(&list_box));
        content.append(&scrolled);

        let buttons = gtk::Box::new(Orientation::Horizontal, 8);
        buttons.set_halign(Align::End);
        let update_selected = gtk::Button::with_label("Update selected");
        update_selected.add_css_class("suggested-action");
        let select_all = gtk::CheckButton::with_label("Select all");
        select_all.set_active(false);
        buttons.append(&select_all);
        buttons.append(&update_selected);
        content.append(&buttons);

        // state: checkbox per row
        type Checks = Vec<(crate::transaction::Action, gtk::CheckButton)>;
        let checks: Rc<RefCell<Checks>> = Rc::new(RefCell::new(Vec::new()));

        let refresh_fn: Rc<RefCell<Box<dyn Fn()>>> = Rc::new(RefCell::new(Box::new(|| {})));

        {
            let app = app.clone();
            let list = list_box.clone();
            let status = status_label.clone();
            let checks = checks.clone();
            let nav0 = nav.clone();
            let toast0 = toast.clone();
            let refresh_fn = refresh_fn.clone();
            let app0 = app.clone();
            let cfg_snapshot = app.config();
            let load = move || {
                let list = list.clone();
                let checks = checks.clone();
                let status = status.clone();
                clear_list(&list);
                checks.borrow_mut().clear();
                status.set_label("Checking…");
                let cfg = cfg_snapshot.clone();
                // Fresh handles per invocation keep `load` an `Fn`.
                let h_list = list.clone();
                let h_checks = checks.clone();
                let h_status = status.clone();
                let h_app = app0.clone();
                let h_nav = nav0.clone();
                let h_toast = toast0.clone();
                crate::ui::bridge::backend_task(
                    async move {
                        let reg = crate::backend::Registry::with_config(&cfg);
                        let mut ups = reg.updates_all().await;
                        ups.sort_by(|a, b| a.package.name.cmp(&b.package.name));
                        ups
                    },
                    move |entries: Vec<UpdateEntry>| {
                        clear_list(&h_list);
                        h_checks.borrow_mut().clear();
                        if entries.is_empty() {
                            h_status.set_label("System is up to date.");
                        } else {
                            let total: Option<u64> =
                                entries.iter().map(|e| e.download_size()).sum();
                            let size_note = total
                                .map(|t| {
                                    format!(" — {} to download", crate::util::size::format_size(t))
                                })
                                .unwrap_or_default();
                            h_status.set_label(&format!(
                                "{} update(s){}",
                                entries.len(),
                                size_note
                            ));
                        }
                        for entry in &entries {
                            let (row, check) =
                                update_row(&h_app, entry, &h_checks, &h_list, &h_nav, &h_toast);
                            h_list.append(&row);
                            h_checks.borrow_mut().push((action_for(entry), check));
                        }
                    },
                );
                checks.borrow_mut().clear();
            };
            *refresh_fn.borrow_mut() = Box::new(load);
            refresh_btn.connect_clicked({
                let f = refresh_fn.clone();
                move |_| (f.borrow())()
            });
        }

        // Select-all toggle.
        {
            let checks = checks.clone();
            select_all.connect_toggled(move |btn| {
                for (_, check) in checks.borrow().iter() {
                    check.set_active(btn.is_active());
                }
            });
        }

        // Apply selected / all.
        {
            let app = app.clone();
            let checks = checks.clone();
            let select_all = select_all.clone();
            update_selected.connect_clicked(move |_| {
                let any_selected = checks.borrow().iter().any(|(_, c)| c.is_active());
                let actions: Vec<_> = checks
                    .borrow()
                    .iter()
                    .filter(|(_, c)| if any_selected { c.is_active() } else { true })
                    .map(|(a, _)| a.clone())
                    .collect();
                let _ = &select_all;
                super::start_transaction(&app, actions);
            });
        }

        // Refresh after transactions complete.
        {
            let f = refresh_fn.clone();
            app.on_transaction_done(Box::new(move || (f.borrow())()));
        }

        Self { root, refresh_fn }
    }

    pub fn root(&self) -> &adw::Clamp {
        &self.root
    }

    pub fn refresh(&self) {
        (self.refresh_fn.borrow())();
    }

    pub fn refresh_closure(&self) -> impl Fn() + 'static {
        let f = self.refresh_fn.clone();
        move || (f.borrow())()
    }
}

fn action_for(entry: &UpdateEntry) -> crate::transaction::Action {
    match entry.package.source.as_ref().map(|s| s.backend()) {
        Some(BackendId::Flatpak) => crate::transaction::Action {
            kind: ActionKind::Upgrade,
            backend: BackendId::Flatpak,
            id: entry.package.id.clone(),
            name: entry.package.name.clone(),
            version: Some(entry.new_version.clone()),
        },
        Some(BackendId::Aur) => crate::transaction::Action {
            kind: ActionKind::Upgrade,
            backend: BackendId::Aur,
            id: entry.package.id.clone(),
            name: entry.package.name.clone(),
            version: Some(entry.new_version.clone()),
        },
        _ => crate::transaction::Action {
            kind: ActionKind::Upgrade,
            backend: BackendId::Pacman,
            id: entry.package.id.clone(),
            name: entry.package.name.clone(),
            version: Some(entry.new_version.clone()),
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn update_row(
    app: &App,
    entry: &UpdateEntry,
    checks: &Rc<RefCell<Vec<(crate::transaction::Action, gtk::CheckButton)>>>,
    list: &gtk::ListBox,
    nav: &adw::NavigationView,
    toast: &adw::ToastOverlay,
) -> (gtk::ListBoxRow, gtk::CheckButton) {
    let box_ = gtk::Box::new(Orientation::Horizontal, 12);
    box_.set_margin_top(8);
    box_.set_margin_bottom(8);
    box_.set_margin_start(10);
    box_.set_margin_end(10);

    let check = gtk::CheckButton::builder().active(true).build();
    box_.append(&check);

    let text_col = gtk::Button::builder().has_frame(true).build();
    let inner = gtk::Box::new(Orientation::Vertical, 2);
    inner.set_halign(Align::Start);
    let name = gtk::Label::new(Some(&entry.package.name));
    name.add_css_class("heading");
    name.set_halign(Align::Start);
    inner.append(&name);
    let versions = gtk::Label::new(Some(&format!(
        "{} → {}",
        entry.current_version, entry.new_version
    )));
    versions.add_css_class("dim-label");
    versions.set_halign(Align::Start);
    inner.append(&versions);
    text_col.set_child(Some(&inner));
    text_col.set_hexpand(true);
    {
        let app = app.clone();
        let pkg = entry.package.clone();
        let nav = nav.clone();
        let toast = toast.clone();
        text_col.connect_clicked(move |_| {
            super::open_details(&app, &nav, &toast, &pkg);
        });
    }
    box_.append(&text_col);

    box_.append(&crate::ui::widgets::source_badge(&entry.package));

    if let Some(bytes) = entry.download_size() {
        let size = gtk::Label::new(Some(&crate::util::size::format_size(bytes)));
        size.add_css_class("dim-label");
        size.set_valign(Align::Center);
        size.set_tooltip_text(Some("Download size"));
        box_.append(&size);
    }

    let row = gtk::ListBoxRow::new();
    row.set_child(Some(&box_));
    let _ = checks;
    let _ = list;
    (row, check)
}

fn clear_list(list: &gtk::ListBox) {
    let mut first = list.first_child();
    while let Some(child) = first {
        first = child.next_sibling();
        list.remove(&child);
    }
}
