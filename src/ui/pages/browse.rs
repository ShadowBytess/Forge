//! Search / catalogue browsing. One implementation, three presets:
//! repository search, AUR search and Flatpak search.

use gtk4 as gtk;
use std::cell::RefCell;
use std::rc::Rc;

use gtk::{Align, Orientation};
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::package::Package;
use crate::transaction::ActionKind;
use crate::ui::state::App;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BrowsePreset {
    Repositories,
    Aur,
    Flatpak,
}

pub struct BrowsePage {
    root: adw::Clamp,
    search_entry: gtk::SearchEntry,
}

impl BrowsePage {
    pub fn new_repos(app: &App, nav: &adw::NavigationView, toast: &adw::ToastOverlay) -> Self {
        Self::build(app, nav, toast, BrowsePreset::Repositories)
    }

    pub fn new_aur(app: &App, nav: &adw::NavigationView, toast: &adw::ToastOverlay) -> Self {
        Self::build(app, nav, toast, BrowsePreset::Aur)
    }

    pub fn new_flatpak(app: &App, nav: &adw::NavigationView, toast: &adw::ToastOverlay) -> Self {
        Self::build(app, nav, toast, BrowsePreset::Flatpak)
    }

    fn build(
        app: &App,
        nav: &adw::NavigationView,
        toast: &adw::ToastOverlay,
        preset: BrowsePreset,
    ) -> Self {
        let title = match preset {
            BrowsePreset::Repositories => "Browse repositories",
            BrowsePreset::Aur => "Search the AUR",
            BrowsePreset::Flatpak => "Search Flatpak",
        };
        let root = adw::Clamp::builder().maximum_size(1000).build();
        let content = gtk::Box::new(Orientation::Vertical, 12);
        content.set_margin_top(16);
        content.set_margin_bottom(24);
        content.set_margin_start(16);
        content.set_margin_end(16);
        root.set_child(Some(&content));

        let heading = gtk::Label::new(Some(title));
        heading.add_css_class("title-2");
        heading.set_halign(Align::Start);
        content.append(&heading);

        let search_entry = gtk::SearchEntry::new();
        search_entry.set_placeholder_text(Some("Type to search…"));
        content.append(&search_entry);

        let status = Rc::new(gtk::Label::new(Some("Type at least two characters.")));
        status.add_css_class("dim-label");
        status.set_halign(Align::Start);
        content.append(&*status);

        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_vexpand(true);
        scrolled.set_min_content_height(420);
        scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);

        let list_box = gtk::ListBox::new();
        list_box.add_css_class("boxed-list");
        list_box.set_selection_mode(gtk::SelectionMode::None);
        scrolled.set_child(Some(&list_box));
        content.append(&scrolled);

        // Latest-query tracking so stale responses are dropped.
        let latest_query: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));

        {
            let app = app.clone();
            let nav = nav.clone();
            let toast = toast.clone();
            let status = status.clone();
            let list = list_box.clone();
            let latest = latest_query.clone();
            let render_results = move |query: String, packages: Vec<Package>| {
                if *latest.borrow() != query {
                    return;
                }
                clear_list(&list);
                if packages.is_empty() {
                    status.set_label(if query.is_empty() { "" } else { "No results." });
                } else {
                    status.set_label(&format!("{} result(s)", packages.len()));
                }
                for pkg in &packages {
                    let row = result_row(&app, pkg, preset, &nav, &toast);
                    list.append(&row);
                }
            };
            RENDER_RESULTS.with(|slot| *slot.borrow_mut() = Some(Box::new(render_results)));
        }

        {
            let latest = latest_query.clone();
            let cfg_snapshot = app.config();
            let backend_filter: Vec<crate::backend::BackendId> = match preset {
                BrowsePreset::Repositories => vec![crate::backend::BackendId::Pacman],
                BrowsePreset::Aur => vec![crate::backend::BackendId::Aur],
                BrowsePreset::Flatpak => vec![crate::backend::BackendId::Flatpak],
            };
            search_entry.connect_search_changed(move |entry| {
                let q = entry.text().trim().to_string();
                *latest.borrow_mut() = q.clone();
                if q.chars().count() < 2 {
                    return;
                }
                let backend_filter = backend_filter.clone();
                let cfg = cfg_snapshot.clone();
                crate::ui::bridge::backend_task(
                    async move {
                        let reg = crate::backend::Registry::with_config(&cfg);
                        let filter = crate::package::SearchFilter {
                            backends: backend_filter,
                            ..Default::default()
                        };
                        let mut merged: Vec<Package> = Vec::new();
                        for (_, res) in reg.search(&q, &filter).await {
                            if let Ok(pkgs) = res {
                                merged.extend(pkgs.into_iter().take(50));
                            }
                        }
                        (q, merged)
                    },
                    |(q, merged): (String, Vec<Package>)| {
                        RENDER_RESULTS.with(|slot| {
                            if let Some(render) = slot.borrow_mut().as_mut() {
                                render(q, merged);
                            }
                        });
                    },
                );
            });
        }

        Self { root, search_entry }
    }

    pub fn root(&self) -> &adw::Clamp {
        &self.root
    }

    pub fn focus_search(&self) {
        self.search_entry.grab_focus();
    }

    /// Simulates pressing search (used by Home's quick search).
    pub fn search_for(&self, query: &str) {
        self.search_entry.set_text(query);
    }
}

thread_local! {
    static RENDER_RESULTS:
        std::cell::RefCell<Option<Box<dyn Fn(String, Vec<Package>)>>> =
        const { std::cell::RefCell::new(None) };
}

fn clear_list(list: &gtk::ListBox) {
    let mut first = list.first_child();
    while let Some(child) = first {
        first = child.next_sibling();
        list.remove(&child);
    }
}

fn result_row(
    app: &App,
    pkg: &Package,
    preset: BrowsePreset,
    nav: &adw::NavigationView,
    toast: &adw::ToastOverlay,
) -> gtk::ListBoxRow {
    let box_ = gtk::Box::new(Orientation::Horizontal, 12);
    box_.set_margin_top(8);
    box_.set_margin_bottom(8);
    box_.set_margin_start(10);
    box_.set_margin_end(10);

    // Clickable text column opens details.
    let text_col = gtk::Button::builder().has_frame(true).build();
    let inner = gtk::Box::new(Orientation::Vertical, 2);
    inner.set_halign(Align::Start);
    let name = gtk::Label::new(Some(&pkg.name));
    name.add_css_class("heading");
    name.set_halign(Align::Start);
    inner.append(&name);
    if !pkg.description.is_empty() {
        let desc = gtk::Label::new(Some(&pkg.description));
        desc.add_css_class("dim-label");
        desc.set_halign(Align::Start);
        desc.set_ellipsize(gtk::pango::EllipsizeMode::End);
        desc.set_max_width_chars(70);
        inner.append(&desc);
    }
    text_col.set_child(Some(&inner));
    text_col.set_hexpand(true);
    {
        let app = app.clone();
        let nav = nav.clone();
        let toast = toast.clone();
        let pkg = pkg.clone();
        text_col.connect_clicked(move |_| {
            super::open_details(&app, &nav, &toast, &pkg);
        });
    }
    box_.append(&text_col);

    box_.append(&crate::ui::widgets::source_badge(pkg));

    let version = gtk::Label::new(Some(&pkg.version));
    version.add_css_class("dim-label");
    version.set_valign(Align::Center);
    box_.append(&version);

    if pkg.is_installed() {
        let remove = gtk::Button::with_label("Remove");
        remove.add_css_class("destructive-action");
        remove.set_valign(Align::Center);
        let app2 = app.clone();
        let action = vec![mk_action(pkg, ActionKind::Remove)];
        remove.connect_clicked(move |_| {
            super::start_transaction(&app2, action.clone());
        });
        box_.append(&remove);
    } else {
        let label = match preset {
            // AUR installs always route through PKGBUILD review first.
            BrowsePreset::Aur => "Install…",
            _ => "Install",
        };
        let install = gtk::Button::with_label(label);
        install.add_css_class("suggested-action");
        install.set_valign(Align::Center);
        let app2 = app.clone();
        let action = vec![mk_action(pkg, ActionKind::Install)];
        install.connect_clicked(move |_| {
            super::start_transaction(&app2, action.clone());
        });
        box_.append(&install);
    }

    let row = gtk::ListBoxRow::new();
    row.set_child(Some(&box_));
    row
}

fn mk_action(pkg: &Package, kind: ActionKind) -> crate::transaction::Action {
    match kind {
        ActionKind::Install => crate::transaction::Action::install(pkg),
        ActionKind::Remove => crate::transaction::Action::remove(pkg),
        ActionKind::Upgrade => crate::transaction::Action::upgrade(pkg),
        ActionKind::Reinstall => crate::transaction::Action::reinstall(pkg),
    }
}
