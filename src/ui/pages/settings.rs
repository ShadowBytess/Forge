//! Settings page: binds the config file to simple controls.

use gtk4 as gtk;

use gtk::{Align, Orientation};
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::ui::state::App;

pub struct SettingsPage {
    root: adw::Clamp,
}

impl SettingsPage {
    pub fn new(app: &App) -> Self {
        let root = adw::Clamp::builder().maximum_size(720).build();
        let content = gtk::Box::new(Orientation::Vertical, 16);
        content.set_margin_top(16);
        content.set_margin_bottom(24);
        content.set_margin_start(16);
        content.set_margin_end(16);
        root.set_child(Some(&content));

        let heading = gtk::Label::new(Some("Settings"));
        heading.add_css_class("title-2");
        heading.set_halign(Align::Start);
        content.append(&heading);

        // --- General ------------------------------------------------------
        let confirm_switch = adw::SwitchRow::builder()
            .title("Confirm before transactions")
            .subtitle("Always show a preview and ask for confirmation")
            .build();
        let aur_review_switch = adw::SwitchRow::builder()
            .title("Require PKGBUILD review")
            .subtitle("Refuse AUR installs until the build script was reviewed")
            .build();
        let pacman_switch = adw::SwitchRow::builder()
            .title("Enable repository packages")
            .subtitle("pacman-managed official repositories")
            .build();
        let aur_switch = adw::SwitchRow::builder()
            .title("Enable AUR")
            .subtitle("Community packages from aur.archlinux.org")
            .build();
        let flatpak_switch = adw::SwitchRow::builder()
            .title("Enable Flatpak")
            .subtitle("Applications from Flatpak remotes")
            .build();

        let general_group = adw::PreferencesGroup::builder()
            .title("General and sources")
            .description(&format!(
                "Stored in {}",
                crate::config::default_config_path().display()
            ))
            .build();
        general_group.add(&confirm_switch);
        general_group.add(&aur_review_switch);
        general_group.add(&pacman_switch);
        general_group.add(&aur_switch);
        general_group.add(&flatpak_switch);
        content.append(&general_group);

        // --- UI -----------------------------------------------------------
        let theme_dropdown = gtk::DropDown::from_strings(&["System", "Light", "Dark"]);
        let ui_group = adw::PreferencesGroup::builder().title("Appearance").build();
        let theme_row = adw::ActionRow::builder().title("Colour scheme").build();
        theme_row.add_suffix(&theme_dropdown);
        ui_group.add(&theme_row);
        content.append(&ui_group);

        // Bind current values.
        {
            let cfg = app.config();
            confirm_switch.set_active(cfg.general.confirm_before_transaction);
            aur_review_switch.set_active(cfg.aur.require_pkgbuild_review);
            pacman_switch.set_active(cfg.sources.enable_pacman);
            aur_switch.set_active(cfg.sources.enable_aur);
            flatpak_switch.set_active(cfg.sources.enable_flatpak);
            theme_dropdown.set_selected(match cfg.ui.color_scheme.as_str() {
                "light" => 1,
                "dark" => 2,
                _ => 0,
            });
        }

        // Persist on any change.
        let persist = {
            let app = app.clone();
            let confirm_switch = confirm_switch.clone();
            let aur_review_switch = aur_review_switch.clone();
            let pacman_switch = pacman_switch.clone();
            let aur_switch = aur_switch.clone();
            let flatpak_switch = flatpak_switch.clone();
            let theme_dropdown = theme_dropdown.clone();
            move || {
                let mut cfg = app.config();
                cfg.general.confirm_before_transaction = confirm_switch.is_active();
                cfg.aur.require_pkgbuild_review = aur_review_switch.is_active();
                cfg.sources.enable_pacman = pacman_switch.is_active();
                cfg.sources.enable_aur = aur_switch.is_active();
                cfg.sources.enable_flatpak = flatpak_switch.is_active();
                cfg.ui.color_scheme = match theme_dropdown.selected() {
                    1 => "light".into(),
                    2 => "dark".into(),
                    _ => "system".into(),
                };
                app.set_config(cfg);
                app.save_config();

                // Live-apply colour scheme.
                let sm = adw::StyleManager::default();
                sm.set_color_scheme(match app.config().ui.color_scheme.as_str() {
                    "light" => adw::ColorScheme::ForceLight,
                    "dark" => adw::ColorScheme::ForceDark,
                    _ => adw::ColorScheme::Default,
                });
            }
        };
        // Values were bound above, before these handlers are attached, so no
        // spurious persistence happens during construction.
        macro_rules! on_change {
            ($w:expr) => {{
                let persist = persist.clone();
                $w.connect_active_notify(move |_| persist());
            }};
        }
        on_change!(confirm_switch);
        on_change!(aur_review_switch);
        on_change!(pacman_switch);
        on_change!(aur_switch);
        on_change!(flatpak_switch);
        {
            let persist = persist.clone();
            theme_dropdown.connect_selected_notify(move |_| persist());
        }

        let note = gtk::Label::new(Some(
            "Changes take effect for newly opened pages; restart Forge to \
             apply source visibility everywhere.",
        ));
        note.add_css_class("dim-label");
        note.set_wrap(true);
        note.set_xalign(0.0);
        content.append(&note);

        Self { root }
    }

    pub fn root(&self) -> &adw::Clamp {
        &self.root
    }
}
