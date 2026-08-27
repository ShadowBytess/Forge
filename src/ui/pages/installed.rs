//! Installed packages page: listing, filter, sort, remove/reinstall.

use gtk4 as gtk;
use std::cell::RefCell;
use std::rc::Rc;

use gtk::{Align, Orientation};
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::package::Package;
use crate::ui::state::App;

#[derive(Clone, Copy, PartialEq, Eq)]
enum SortKey {
    Name,
    Version,
    Size,
    Source,
}

type RenderFn = Rc<RefCell<Box<dyn Fn()>>>;
type PackagesRc = Rc<RefCell<Vec<Package>>>;

pub struct InstalledPage {
    root: adw::Clamp,
    refresh_fn: RenderFn,
}

impl InstalledPage {
    pub fn new(app: &App, nav: &adw::NavigationView, toast: &adw::ToastOverlay) -> Self {
        let root = adw::Clamp::builder().maximum_size(1000).build();
        let content = gtk::Box::new(Orientation::Vertical, 12);
        content.set_margin_top(16);
        content.set_margin_bottom(24);
        content.set_margin_start(16);
        content.set_margin_end(16);
        root.set_child(Some(&content));

        let heading = gtk::Label::new(Some("Installed packages"));
        heading.add_css_class("title-2");
        heading.set_halign(Align::Start);
        content.append(&heading);

        // ----- controls ---------------------------------------------------
        let controls = gtk::Box::new(Orientation::Horizontal, 8);
        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some("Filter installed packages…"));
        search.set_hexpand(true);
        controls.append(&search);

        let sort_dropdown =
            gtk::DropDown::from_strings(&["Name", "Version", "Installed size", "Source"]);
        sort_dropdown.set_selected(0);
        controls.append(&sort_dropdown);

        let refresh_btn = gtk::Button::from_icon_name("view-refresh-symbolic");
        refresh_btn.set_tooltip_text(Some("Refresh"));
        controls.append(&refresh_btn);
        content.append(&controls);

        let status_label = Rc::new(gtk::Label::new(Some("Loading…")));
        status_label.add_css_class("dim-label");
        status_label.set_halign(Align::Start);
        content.append(&*status_label);

        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_vexpand(true);
        scrolled.set_min_content_height(430);
        scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        let list_box = gtk::ListBox::new();
        list_box.add_css_class("boxed-list");
        list_box.set_selection_mode(gtk::SelectionMode::None);
        scrolled.set_child(Some(&list_box));
        content.append(&scrolled);

        // ----- state ------------------------------------------------------
        let packages: PackagesRc = Rc::new(RefCell::new(Vec::new()));
        let refresh_fn: RenderFn = Rc::new(RefCell::new(Box::new(|| {})));

        // ----- render -----------------------------------------------------
        let render: RenderFn = {
            let app = app.clone();
            let nav = nav.clone();
            let toast = toast.clone();
            let list = list_box.clone();
            let status = status_label.clone();
            let pkgs = packages.clone();
            let search_handle = search.clone();
            let sort_handle = sort_dropdown.clone();
            Rc::new(RefCell::new(Box::new(move || {
                let filter = search_handle.text().trim().to_lowercase();
                let sort_key: SortKey = match sort_handle.selected() {
                    1 => SortKey::Version,
                    2 => SortKey::Size,
                    3 => SortKey::Source,
                    _ => SortKey::Name,
                };
                clear_list(&list);
                let mut items: Vec<Package> = pkgs
                    .borrow()
                    .iter()
                    .filter(|p| {
                        filter.is_empty()
                            || p.name.to_lowercase().contains(&filter)
                            || p.description.to_lowercase().contains(&filter)
                    })
                    .cloned()
                    .collect();

                match sort_key {
                    SortKey::Name => items.sort_by(|a, b| a.name.cmp(&b.name)),
                    SortKey::Version => items.sort_by(|a, b| a.version.cmp(&b.version)),
                    SortKey::Size => {
                        items.sort_by_key(|p| std::cmp::Reverse(p.size.installed.unwrap_or(0)))
                    }
                    SortKey::Source => {
                        items.sort_by(|a, b| a.source_label().cmp(&b.source_label()))
                    }
                }

                status.set_label(&format!("{} installed package(s)", items.len()));
                for pkg in &items {
                    list.append(&installed_row(&app, pkg, &nav, &toast));
                }
            })))
        };

        // ----- data loading -------------------------------------------------
        let cfg_snapshot = app.config();
        {
            let _app = app.clone();
            let pkgs = packages.clone();
            let render = render.clone();
            let refresh_fn = refresh_fn.clone();
            let load = move || {
                let cfg = cfg_snapshot.clone();
                let pkgs = pkgs.clone();
                let render = render.clone();
                crate::ui::bridge::backend_task(
                    async move {
                        let reg = crate::backend::Registry::with_config(&cfg);
                        let mut all = reg
                            .installed_all(crate::package::StatusFilter::InstalledOnly)
                            .await;
                        all.sort_by(|a, b| a.name.cmp(&b.name));
                        all
                    },
                    move |loaded| {
                        *pkgs.borrow_mut() = loaded;
                        (render.borrow())();
                    },
                );
            };
            *refresh_fn.borrow_mut() = Box::new(load);
            refresh_btn.connect_clicked({
                let refresh_fn = refresh_fn.clone();
                move |_| (refresh_fn.borrow())()
            });
        }

        search.connect_search_changed({
            let render = render.clone();
            move |_| (render.borrow())()
        });
        sort_dropdown.connect_selected_notify({
            let render = render.clone();
            move |_| (render.borrow())()
        });

        // Refresh automatically after any transaction.
        {
            let refresh_fn = refresh_fn.clone();
            app.on_transaction_done(Box::new(move || (refresh_fn.borrow())()));
        }

        Self { root, refresh_fn }
    }

    pub fn root(&self) -> &adw::Clamp {
        &self.root
    }

    /// Closure that reloads the list (used for the initial load).
    pub fn refresh_closure(&self) -> impl Fn() + 'static {
        let f = self.refresh_fn.clone();
        move || (f.borrow())()
    }
}

fn clear_list(list: &gtk::ListBox) {
    let mut first = list.first_child();
    while let Some(child) = first {
        first = child.next_sibling();
        list.remove(&child);
    }
}

fn installed_row(
    app: &App,
    pkg: &Package,
    nav: &adw::NavigationView,
    toast: &adw::ToastOverlay,
) -> gtk::ListBoxRow {
    let box_ = gtk::Box::new(Orientation::Horizontal, 12);
    box_.set_margin_top(8);
    box_.set_margin_bottom(8);
    box_.set_margin_start(10);
    box_.set_margin_end(10);

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

    if let Some(bytes) = pkg.size.installed {
        let size = gtk::Label::new(Some(&crate::util::size::format_size(bytes)));
        size.add_css_class("dim-label");
        size.set_valign(Align::Center);
        size.set_tooltip_text(Some("Installed size"));
        box_.append(&size);
    }

    let backend = pkg.source.as_ref().map(|s| s.backend());
    let reinstall_supported = matches!(
        backend,
        Some(crate::backend::BackendId::Pacman) | Some(crate::backend::BackendId::Flatpak)
    );
    let reinstall = gtk::Button::with_label("Reinstall");
    reinstall.set_valign(Align::Center);
    reinstall.set_sensitive(reinstall_supported);
    reinstall.set_tooltip_text(if reinstall_supported {
        None
    } else {
        Some("Rebuild the package from the AUR instead".into())
    });
    {
        let app2 = app.clone();
        let action = vec![crate::transaction::Action::reinstall(pkg)];
        reinstall.connect_clicked(move |_| super::start_transaction(&app2, action.clone()));
    }
    box_.append(&reinstall);

    let remove = gtk::Button::with_label("Remove");
    remove.add_css_class("destructive-action");
    remove.set_valign(Align::Center);
    {
        let app2 = app.clone();
        let action = vec![crate::transaction::Action::remove(pkg)];
        remove.connect_clicked(move |_| super::start_transaction(&app2, action.clone()));
    }
    box_.append(&remove);

    let row = gtk::ListBoxRow::new();
    row.set_child(Some(&box_));
    row
}
