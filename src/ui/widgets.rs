//! Shared widget builders: package rows, preview dialogs and small helpers.

use gtk::{Align, Label, Orientation};
use gtk4 as gtk;
use libadwaita::prelude::*;

use crate::package::{InstallStatus, Package};

/// Styled pill badge.
pub fn badge(text: &str, css: &str) -> gtk::Label {
    let l = Label::new(Some(text));
    l.add_css_class(css);
    l.set_valign(Align::Center);
    l
}

pub fn source_badge(pkg: &Package) -> gtk::Label {
    let (text, css) = match pkg.source.as_ref() {
        Some(crate::package::Source::Repo { repo }) => (repo.clone(), "accent"),
        Some(crate::package::Source::Aur) => ("AUR".to_string(), "warning"),
        Some(crate::package::Source::Flatpak { .. }) => ("Flatpak".to_string(), "success"),
        None => ("?".into(), "dim-label"),
    };
    badge(&text, css)
}

/// One row in a package list. Returns the row plus its "action button"
/// (install/remove/upgrade), which the page wires up separately.
pub fn package_row(pkg: &Package) -> (gtk::ListBoxRow, gtk::Button) {
    let row_box = gtk::Box::new(Orientation::Horizontal, 12);
    row_box.set_margin_top(6);
    row_box.set_margin_bottom(6);
    row_box.set_margin_start(8);
    row_box.set_margin_end(8);

    let text_col = gtk::Box::new(Orientation::Vertical, 2);
    text_col.set_hexpand(true);

    let title_row = gtk::Box::new(Orientation::Horizontal, 8);
    let name = Label::new(Some(&pkg.name));
    name.add_css_class("heading");
    name.set_halign(Align::Start);
    title_row.append(&name);
    title_row.append(&source_badge(pkg));
    if pkg.out_of_date {
        title_row.append(&badge("out-of-date", "error"));
    }
    if let InstallStatus::Installed { version, .. } = &pkg.status {
        title_row.append(&badge(&format!("installed {version}"), "dim-label"));
    }
    text_col.append(&title_row);

    if !pkg.description.is_empty() {
        let desc = Label::new(Some(&pkg.description));
        desc.add_css_class("dim-label");
        desc.set_halign(Align::Start);
        desc.set_ellipsize(gtk::pango::EllipsizeMode::End);
        desc.set_max_width_chars(60);
        text_col.append(&desc);
    }
    row_box.append(&text_col);

    let version = Label::new(Some(&pkg.version));
    version.add_css_class("dim-label");
    version.set_valign(Align::Center);
    row_box.append(&version);

    // Primary action for this row; pages override behaviour via connect.
    let action = gtk::Button::from_icon_name("system-run-symbolic");
    action.set_tooltip_text(Some("Open details"));
    action.set_valign(Align::Center);
    action.set_has_frame(false);
    row_box.append(&action);

    let row = gtk::ListBoxRow::new();
    row.set_child(Some(&row_box));
    (row, action)
}

/// Builds a details grid (`label : value`) used by the package page.
pub fn detail_grid(rows: &[(&str, String)]) -> gtk::Grid {
    let grid = gtk::Grid::new();
    grid.set_column_spacing(16);
    grid.set_row_spacing(6);
    for (i, (key, value)) in rows.iter().enumerate() {
        let k = Label::new(Some(*key));
        k.set_halign(Align::Start);
        k.add_css_class("dim-label");
        let v = Label::new(Some(value));
        v.set_halign(Align::Start);
        v.set_wrap(true);
        v.set_xalign(0.0);
        grid.attach(&k, 0, i as i32, 1, 1);
        grid.attach(&v, 1, i as i32, 1, 1);
    }
    grid
}

/// Shows the transaction preview inside an Adwaita dialog; `on_confirm`
/// runs when the user explicitly accepts.
pub fn show_preview_dialog(
    parent: &impl IsA<gtk::Window>,
    plan: &crate::transaction::TransactionPreview,
    on_confirm: impl FnOnce() + 'static,
) {
    use crate::transaction::Reason;

    let mut body = String::from("The following will be performed:\n\n");
    for item in &plan.items {
        let tag = match item.reason {
            Reason::Target => "",
            Reason::Dependency => "+ dependency",
            Reason::NoLongerNeeded => "- no longer needed",
            Reason::ConflictReplacement => "! conflict",
        };
        if tag.is_empty() {
            body.push_str(&format!("• {}\n", item.action.summary()));
        } else {
            body.push_str(&format!("• {} ({tag})\n", item.action.summary()));
        }
    }
    if plan.total_download_size() > 0 {
        body.push_str(&format!(
            "\nDownload size: {}\n",
            crate::util::size::format_size(plan.total_download_size())
        ));
    }
    if plan.total_installed_size() > 0 {
        body.push_str(&format!(
            "Installed size: {}\n",
            crate::util::size::format_size(plan.total_installed_size())
        ));
    }
    for w in &plan.warnings {
        body.push_str(&format!("\n⚠ {w}\n"));
    }
    if plan.needs_privileges {
        body.push_str("\nAuthentication will be requested via polkit.\n");
    }

    let dialog = libadwaita::AlertDialog::builder()
        .heading("Confirm transaction")
        .body(&body)
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("confirm", "Proceed");
    dialog.set_response_appearance("confirm", libadwaita::ResponseAppearance::Suggested);
    dialog.set_close_response("cancel");
    dialog.choose(
        Some(parent.upcast_ref()),
        None::<&gtk::gio::Cancellable>,
        move |res| {
            if res == "confirm" {
                on_confirm();
            }
        },
    );
}
