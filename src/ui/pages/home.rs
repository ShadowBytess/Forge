//! Home page: summary cards and quick search entry.

use gtk::{Align, Orientation};
use gtk4 as gtk;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::backend::BackendId;
use crate::ui::state::App;

pub struct HomePage {
    root: adw::Clamp,
    search_entry: gtk::SearchEntry,
}

impl HomePage {
    /// Returns the page plus a closure that refreshes all summary cards.
    pub fn new(app: &App) -> (Self, impl Fn() + 'static) {
        let root = adw::Clamp::builder().maximum_size(860).build();
        let content = gtk::Box::new(Orientation::Vertical, 24);
        content.set_margin_top(24);
        content.set_margin_bottom(24);
        content.set_margin_start(16);
        content.set_margin_end(16);
        root.set_child(Some(&content));

        let title = gtk::Label::new(Some("Welcome to Forge"));
        title.add_css_class("title-1");
        title.set_halign(Align::Start);
        content.append(&title);

        let search_row = adw::Clamp::builder().maximum_size(560).build();
        let search_entry = gtk::SearchEntry::new();
        search_entry.set_placeholder_text(Some("Search packages…"));
        search_row.set_child(Some(&search_entry));
        content.append(&search_row);

        // Summary cards.
        let grid = gtk::Grid::new();
        grid.set_column_spacing(12);
        grid.set_row_spacing(12);

        let make_card = |heading: &str| -> (gtk::Box, gtk::Label) {
            let card = gtk::Box::new(Orientation::Vertical, 4);
            card.add_css_class("card");
            card.set_margin_top(14);
            card.set_margin_bottom(14);
            card.set_margin_start(10);
            card.set_margin_end(10);
            card.set_valign(Align::Center);
            let value = gtk::Label::new(Some("…"));
            value.add_css_class("title-2");
            let head = gtk::Label::new(Some(heading));
            head.add_css_class("dim-label");
            card.append(&value);
            card.append(&head);
            (card, value)
        };

        let (updates_bin, updates_label) = make_card("Available updates");
        let (installed_bin, installed_label) = make_card("Repository packages");
        let (aur_bin, aur_label) = make_card("AUR packages");
        let (flatpak_bin, flatpak_label) = make_card("Flatpak apps");
        grid.attach(&updates_bin, 0, 0, 1, 1);
        grid.attach(&installed_bin, 1, 0, 1, 1);
        grid.attach(&aur_bin, 0, 1, 1, 1);
        grid.attach(&flatpak_bin, 1, 1, 1, 1);
        content.append(&grid);

        let hint = gtk::Label::new(Some(
            "Forge orchestrates pacman, the AUR and Flatpak.\n\
             pacman remains authoritative for repository packages; AUR builds \
             always require PKGBUILD review.",
        ));
        hint.add_css_class("dim-label");
        hint.set_justify(gtk::Justification::Center);
        hint.set_wrap(true);
        content.append(&hint);

        let labels = (
            updates_label.clone(),
            installed_label.clone(),
            aur_label.clone(),
            flatpak_label.clone(),
        );

        let cfg_snapshot = app.config();
        let refresh = move || {
            let (ul, il, al, fl) = labels.clone();
            let cfg = cfg_snapshot.clone();
            crate::ui::bridge::backend_task(
                async move {
                    let reg = crate::backend::Registry::with_config(&cfg);
                    let updates = reg.updates_all().await.len() as u64;
                    let mut counts = [0u64; 3];
                    for b in reg.active().await {
                        let n = b.installed().await.map(|p| p.len()).unwrap_or(0) as u64;
                        match b.id() {
                            BackendId::Pacman => counts[0] = n,
                            BackendId::Aur => counts[1] = n,
                            BackendId::Flatpak => counts[2] = n,
                        }
                    }
                    vec![updates, counts[0], counts[1], counts[2]]
                },
                move |vals: Vec<u64>| {
                    if vals.len() == 4 {
                        ul.set_label(&vals[0].to_string());
                        il.set_label(&vals[1].to_string());
                        al.set_label(&vals[2].to_string());
                        fl.set_label(&vals[3].to_string());
                    }
                },
            );
        };

        (Self { root, search_entry }, refresh)
    }

    pub fn root(&self) -> &adw::Clamp {
        &self.root
    }

    pub fn search_entry(&self) -> &gtk::SearchEntry {
        &self.search_entry
    }
}
