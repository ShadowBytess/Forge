//! Package details page, including the AUR PKGBUILD review flow.

use gtk::{Align, Orientation};
use gtk4 as gtk;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::backend::BackendId;
use crate::package::{InstallStatus, Package, Source};
use crate::ui::state::App;

/// Opens (and pushes) a details page for `pkg`.
pub fn open(app: &App, nav: &adw::NavigationView, toast: &adw::ToastOverlay, pkg: &Package) {
    let page = build(app, nav, toast, pkg);
    nav.push(&page);
}

fn build(
    app: &App,
    nav: &adw::NavigationView,
    toast: &adw::ToastOverlay,
    pkg: &Package,
) -> adw::NavigationPage {
    let scrolled = gtk::ScrolledWindow::new();
    scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);

    let content = gtk::Box::new(Orientation::Vertical, 16);
    content.set_margin_top(16);
    content.set_margin_bottom(24);
    content.set_margin_start(20);
    content.set_margin_end(20);
    content.set_valign(Align::Start);
    scrolled.set_child(Some(&content));

    // Header row.
    let header = gtk::Box::new(Orientation::Horizontal, 12);
    let name = gtk::Label::new(Some(&pkg.name));
    name.add_css_class("title-2");
    name.set_halign(Align::Start);
    name.set_hexpand(true);
    header.append(&name);
    header.append(&crate::ui::widgets::source_badge(pkg));
    if pkg.out_of_date {
        let ood = gtk::Label::new(Some("out-of-date"));
        ood.add_css_class("error");
        header.append(&ood);
    }
    content.append(&header);

    // Source / trust banner for non-official packages.
    match pkg.source.as_ref() {
        Some(Source::Aur) => {
            let banner = adw::Banner::builder()
                .title("AUR package — community-produced, not vetted by Arch Linux. Review the PKGBUILD before installing.")
                .button_label("View PKGBUILD")
                .revealed(true)
                .build();
            {
                let app = app.clone();
                let nav = nav.clone();
                let toast = toast.clone();
                let pkg = pkg.clone();
                banner.connect_button_clicked(move |_| {
                    open_pkgbuild_review(&app, &nav, &toast, &pkg);
                });
            }
            content.append(&banner);
        }
        Some(Source::Flatpak { scope, remote }) => {
            let note = match scope {
                Some(s) => format!("Flatpak from remote “{remote}” ({})", s.label()),
                None => format!("Flatpak from remote “{remote}”"),
            };
            let lbl = gtk::Label::new(Some(&note));
            lbl.add_css_class("dim-label");
            lbl.set_halign(Align::Start);
            content.append(&lbl);
        }
        _ => {}
    }

    let desc = gtk::Label::new(Some(&pkg.description));
    desc.set_wrap(true);
    desc.set_xalign(0.0);
    content.append(&desc);

    // Metadata grid.
    let mut rows: Vec<(&str, String)> = vec![
        ("Version", pkg.version.clone()),
        (
            "Source",
            format!("{} ({})", pkg.source_label(), pkg.trust_label()),
        ),
    ];
    if let Some(arch) = &pkg.arch {
        rows.push(("Architecture", arch.clone()));
    }
    if !pkg.depends.is_empty() {
        rows.push(("Depends on", pkg.depends.join(", ")));
    }
    if !pkg.provides.is_empty() {
        rows.push(("Provides", pkg.provides.join(", ")));
    }
    if !pkg.conflicts.is_empty() {
        rows.push(("Conflicts with", pkg.conflicts.join(", ")));
    }
    if let Some(url) = &pkg.homepage {
        rows.push(("Homepage", url.clone()));
    }
    if !pkg.licenses.is_empty() {
        rows.push(("Licenses", pkg.licenses.join(", ")));
    }
    if let Some(m) = &pkg.maintainer {
        rows.push(("Maintainer", m.clone()));
    }
    if pkg.size.download.is_some() {
        rows.push((
            "Download size",
            pkg.size
                .download
                .map(crate::util::size::format_size)
                .unwrap_or_default(),
        ));
    }
    if pkg.size.installed.is_some() {
        rows.push((
            "Installed size",
            pkg.size
                .installed
                .map(crate::util::size::format_size)
                .unwrap_or_default(),
        ));
    }
    match &pkg.status {
        InstallStatus::Installed { version, reason } => {
            rows.push((
                "Status",
                format!("installed {version}{}", reason_suffix(*reason)),
            ));
        }
        InstallStatus::NotInstalled => rows.push(("Status", "not installed".into())),
    }
    if let Some(votes) = pkg.num_votes {
        rows.push(("AUR votes", votes.to_string()));
    }

    let grid = crate::ui::widgets::detail_grid(&rows);
    grid.add_css_class("card");
    grid.set_margin_top(12);
    grid.set_margin_bottom(12);
    grid.set_margin_start(12);
    grid.set_margin_end(12);
    content.append(&grid);

    // Optional dependencies list.
    if !pkg.opt_depends.is_empty() {
        let head = gtk::Label::new(Some("Optional dependencies"));
        head.add_css_class("heading");
        head.set_halign(Align::Start);
        content.append(&head);
        for opt in &pkg.opt_depends {
            let text = match &opt.description {
                Some(d) => format!("{}: {d}", opt.name),
                None => opt.name.clone(),
            };
            let l = gtk::Label::new(Some(&text));
            l.add_css_class("dim-label");
            l.set_halign(Align::Start);
            content.append(&l);
        }
    }

    // Files listing for installed pacman packages.
    let is_pacman_installed = pkg.is_installed()
        && matches!(
            pkg.source.as_ref().map(|s| s.backend()),
            Some(BackendId::Pacman)
        );
    if is_pacman_installed {
        let files_head = gtk::Label::new(Some("Files"));
        files_head.add_css_class("heading");
        files_head.set_halign(Align::Start);
        content.append(&files_head);

        let files_view = gtk::TextView::builder()
            .editable(false)
            .cursor_visible(false)
            .monospace(true)
            .wrap_mode(gtk::WrapMode::None)
            .build();
        let files_scroll = gtk::ScrolledWindow::new();
        files_scroll.set_min_content_height(220);
        files_scroll.set_child(Some(&files_view));
        files_scroll.add_css_class("card");
        content.append(&files_scroll);

        // Fetch the file list asynchronously and fill the view.
        {
            let view = files_view.downgrade();
            let id = pkg.id.clone();
            crate::ui::bridge::backend_task(
                async move {
                    let reg = crate::backend::Registry::with_config(&crate::config::Config::load());
                    match reg.get(BackendId::Pacman) {
                        Some(pacman) => match pacman
                            .as_any()
                            .downcast_ref::<crate::backend::pacman::PacmanBackend>()
                        {
                            Some(b) => b.package_files(&id).await.unwrap_or_default(),
                            None => Vec::new(),
                        },
                        None => Vec::new(),
                    }
                },
                move |files: Vec<String>| {
                    if let Some(view) = view.upgrade() {
                        let text = files.join("\n");
                        view.buffer().set_text(if text.is_empty() {
                            "(file list unavailable)"
                        } else {
                            &text
                        });
                    }
                },
            );
        }
    }

    // ----- action bar ------------------------------------------------------
    let actions = gtk::Box::new(Orientation::Horizontal, 8);
    actions.set_halign(Align::End);

    let backend_id = pkg.source.as_ref().map(|s| s.backend());

    if pkg.is_installed() {
        if matches!(backend_id, Some(BackendId::Pacman)) {
            let reinstall = gtk::Button::with_label("Reinstall");
            reinstall.set_tooltip_text(Some("Reinstall from repository"));
            {
                let app2 = app.clone();
                let action = vec![crate::transaction::Action::reinstall(pkg)];
                reinstall.connect_clicked(move |_| super::start_transaction(&app2, action.clone()));
            }
            actions.append(&reinstall);
        }
        let remove = gtk::Button::with_label("Remove");
        remove.add_css_class("destructive-action");
        {
            let app2 = app.clone();
            let action = vec![crate::transaction::Action::remove(pkg)];
            remove.connect_clicked(move |_| super::start_transaction(&app2, action.clone()));
        }
        actions.append(&remove);
    } else {
        let install = gtk::Button::with_label(match backend_id {
            Some(BackendId::Aur) => "Install… (review PKGBUILD)",
            _ => "Install",
        });
        install.add_css_class("suggested-action");
        {
            let app2 = app.clone();
            let nav2 = nav.clone();
            let toast2 = toast.clone();
            let pkg2 = pkg.clone();
            install.connect_clicked(move |_| {
                if matches!(
                    pkg2.source.as_ref().map(|s| s.backend()),
                    Some(BackendId::Aur)
                ) {
                    open_pkgbuild_review(&app2, &nav2, &toast2, &pkg2);
                } else {
                    let action = vec![mk_action_for(&pkg2)];
                    super::start_transaction(&app2, action);
                }
            });
        }
        actions.append(&install);
    }
    content.append(&actions);

    adw::NavigationPage::builder()
        .child(&scrolled)
        .title(&pkg.name)
        .build()
}

/// PKGBUILD review flow: fetch/clone source, show PKGBUILD + structured
/// dependencies; only after explicit confirmation is installation unlocked
/// and started.
pub fn open_pkgbuild_review(
    app: &App,
    nav: &adw::NavigationView,
    toast: &adw::ToastOverlay,
    pkg: &Package,
) {
    let window_box = gtk::Box::new(Orientation::Vertical, 10);
    window_box.set_margin_top(12);
    window_box.set_margin_bottom(12);
    window_box.set_margin_start(14);
    window_box.set_margin_end(14);

    let warning = gtk::Label::new(Some(
        "The following build script will be executed by makepkg. \
         Read it carefully: AUR packages are community-produced.",
    ));
    warning.add_css_class("error");
    warning.set_wrap(true);
    warning.set_xalign(0.0);
    window_box.append(&warning);

    let deps_label = gtk::Label::new(Some("Loading dependency information…"));
    deps_label.add_css_class("dim-label");
    deps_label.set_halign(Align::Start);
    deps_label.set_wrap(true);
    deps_label.set_xalign(0.0);
    window_box.append(&deps_label);

    let text_view = gtk::TextView::builder()
        .editable(false)
        .cursor_visible(false)
        .monospace(true)
        .left_margin(6)
        .right_margin(6)
        .top_margin(6)
        .bottom_margin(6)
        .build();
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);
    scroll.set_min_content_height(380);
    scroll.set_child(Some(&text_view));
    scroll.add_css_class("card");
    window_box.append(&scroll);

    let buttons = gtk::Box::new(Orientation::Horizontal, 8);
    buttons.set_halign(Align::End);
    let cancel_btn = gtk::Button::with_label("Cancel");
    let proceed_btn = gtk::Button::with_label("I have reviewed it — install");
    proceed_btn.add_css_class("suggested-action");
    proceed_btn.set_sensitive(false);
    proceed_btn.set_tooltip_text(Some("Enabled once the PKGBUILD has been fetched"));
    buttons.append(&cancel_btn);
    buttons.append(&proceed_btn);
    window_box.append(&buttons);

    let page = adw::NavigationPage::builder()
        .child(&window_box)
        .title(&format!("PKGBUILD — {}", pkg.name))
        .build();

    // Fetch PKGBUILD (+ structured dependency info) asynchronously.
    {
        let id = pkg.id.clone();
        let proceed_btn = proceed_btn.clone();
        crate::ui::bridge::backend_task(
            async move {
                let cfg = crate::config::Config::load();
                let reg = crate::backend::Registry::with_config(&cfg);
                let Some(aur_ref) = reg.get(BackendId::Aur) else {
                    return (
                        false,
                        "# AUR backend unavailable".to_string(),
                        String::new(),
                    );
                };
                if aur_ref
                    .as_any()
                    .downcast_ref::<crate::backend::aur::AurBackend>()
                    .is_none()
                {
                    return (
                        false,
                        "# AUR backend unavailable".to_string(),
                        String::new(),
                    );
                }
                let events =
                    tokio::sync::mpsc::unbounded_channel::<crate::transaction::TxEvent>().0;
                match crate::backend::aur::build::prepare_source(
                    &cfg,
                    &id,
                    &events,
                    &crate::util::cancel::CancelToken::new(),
                )
                .await
                {
                    Ok(dir) => {
                        let text = crate::backend::aur::build::read_pkgbuild(&dir)
                            .unwrap_or_else(|_| "# failed to read PKGBUILD".into());
                        let srcinfo = crate::backend::aur::build::generate_srcinfo(&dir)
                            .await
                            .ok();
                        let mut deps = srcinfo
                            .as_ref()
                            .map(|s| {
                                let mut v = s.depends();
                                v.extend(s.build_depends());
                                v.sort();
                                v.dedup();
                                v
                            })
                            .unwrap_or_default();
                        deps.retain(|d| !d.is_empty());
                        (
                            true,
                            text,
                            if deps.is_empty() {
                                String::new()
                            } else {
                                format!("Build dependencies: {}", deps.join(", "))
                            },
                        )
                    }
                    Err(e) => (false, format!("# fetch failed: {e}"), String::new()),
                }
            },
            move |(ok, text, deps): (bool, String, String)| {
                deps_label.set_label(if deps.is_empty() { "" } else { &deps });
                text_view.buffer().set_text(&text);
                proceed_btn.set_sensitive(ok);
            },
        );
    }

    // Buttons.
    {
        let nav2 = nav.clone();
        cancel_btn.connect_clicked(move |_| {
            nav2.pop();
        });
    }
    {
        let app2 = app.clone();
        let nav2 = nav.clone();
        let toast2 = toast.clone();
        let pkg2 = pkg.clone();
        proceed_btn.connect_clicked(move |_| {
            app2.mark_aur_reviewed(&pkg2.id);
            let action = mk_action_for(&pkg2);
            nav2.pop();
            let _ = &toast2;
            super::start_transaction(&app2, vec![action]);
        });
    }

    nav.push(&page);
}

fn reason_suffix(reason: Option<crate::package::InstallReason>) -> &'static str {
    match reason {
        Some(crate::package::InstallReason::Explicit) => ", explicit",
        Some(crate::package::InstallReason::Dependency) => ", as dependency",
        None => "",
    }
}

fn mk_action_for(pkg: &Package) -> crate::transaction::Action {
    // Reinstall when already installed, otherwise plain install.
    if pkg.is_installed() {
        crate::transaction::Action::reinstall(pkg)
    } else {
        crate::transaction::Action::install(pkg)
    }
}
