//! Transactions page: live log, progress, cancel.

use gtk4 as gtk;
use std::cell::RefCell;

use gtk::{Align, Orientation};
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::transaction::TxEvent;
use crate::ui::state::App;

pub struct TransactionsPage {
    root: adw::Clamp,
}

impl TransactionsPage {
    pub fn new(app: &App) -> Self {
        let root = adw::Clamp::builder().maximum_size(1000).build();
        let content = gtk::Box::new(Orientation::Vertical, 12);
        content.set_margin_top(16);
        content.set_margin_bottom(24);
        content.set_margin_start(16);
        content.set_margin_end(16);
        root.set_child(Some(&content));

        let heading = gtk::Label::new(Some("Transactions"));
        heading.add_css_class("title-2");
        heading.set_halign(Align::Start);
        content.append(&heading);

        let status = gtk::Label::new(Some("Idle"));
        status.add_css_class("dim-label");
        status.set_halign(Align::Start);
        status.set_hexpand(true);

        let cancel_btn = gtk::Button::with_label("Cancel");
        cancel_btn.add_css_class("destructive-action");
        cancel_btn.set_sensitive(false);
        {
            let app2 = app.clone();
            let status2 = status.clone();
            let cancel_btn = cancel_btn.clone();
            // Lightweight poll keeps the cancel button in sync.
            glib_timeout_repeat(500, move || {
                let running = app2.has_active_transaction();
                cancel_btn.set_sensitive(running);
                if !running && status2.label() == "Running…" {
                    status2.set_label("Finished");
                }
            });
        }
        {
            let app2 = app.clone();
            cancel_btn.connect_clicked(move |_| {
                if app2.cancel_active() {
                    app2.toast("Cancelling…");
                }
            });
        }

        let header = gtk::Box::new(Orientation::Horizontal, 8);
        header.append(&status);
        header.append(&cancel_btn);
        content.append(&header);

        let progress = gtk::ProgressBar::new();
        progress.set_show_text(true);
        progress.set_text(Some("waiting"));
        progress.set_visible(false);
        content.append(&progress);

        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_vexpand(true);
        scrolled.set_min_content_height(420);
        scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);

        let log_view = gtk::TextView::builder()
            .editable(false)
            .cursor_visible(false)
            .monospace(true)
            .left_margin(6)
            .right_margin(6)
            .top_margin(6)
            .bottom_margin(6)
            .build();
        scrolled.set_child(Some(&log_view));
        scrolled.add_css_class("card");
        content.append(&scrolled);

        let clear_btn = gtk::Button::with_label("Clear log");
        clear_btn.set_halign(Align::End);
        {
            let view = log_view.clone();
            let progress = progress.clone();
            clear_btn.connect_clicked(move |_| {
                view.buffer().set_text("");
                progress.set_visible(false);
                progress.set_fraction(0.0);
                progress.set_text(Some("waiting"));
            });
        }
        content.append(&clear_btn);

        // Register as the global event sink.
        let sink_state: RefCell<Option<(u32, u32)>> = RefCell::new(None); // done,total counters
        {
            let _buffer = log_view.buffer();
            let view = log_view.clone();
            let progress = progress.clone();
            let status = status.clone();
            let sink_state = std::rc::Rc::new(sink_state);
            app.set_txn_sink(Box::new(move |event| match event {
                TxEvent::BatchStarted { backend, steps } => {
                    *sink_state.borrow_mut() = Some((0u32, steps as u32));
                    status.set_label(&format!("Running on {backend} ({steps} step/s)…"));
                    append_line(&view, &format!("== {backend}: {steps} step(s) =="));
                }
                TxEvent::StepStarted { action } => {
                    progress.set_visible(true);
                    progress.set_text(Some(&action.kind.gerund().to_string()));
                    append_line(&view, &action.summary());
                }
                TxEvent::DownloadProgress {
                    name,
                    received,
                    total,
                } => {
                    let text = match total {
                        Some(t) if t > 0 => format!(
                            "downloading {} ({}/{})",
                            name,
                            crate::util::size::format_size(received),
                            crate::util::size::format_size(t)
                        ),
                        _ => format!("downloading {name}"),
                    };
                    progress.set_text(Some(&text));
                    if let Some(t) = total {
                        if t > 0 {
                            progress.set_fraction((received.min(t)) as f64 / t as f64);
                        }
                    }
                }
                TxEvent::OutputLine { line, is_stderr } => {
                    append_line(&view, &line);
                    let _ = is_stderr;
                }
                TxEvent::Message(msg) => append_line(&view, &msg),
                TxEvent::StepFinished { name } => {
                    if let Some((done, total)) = sink_state.borrow_mut().as_mut() {
                        *done += 1;
                        progress.set_fraction(*done as f64 / (*total as f64).max(1.0));
                    }
                    append_line(&view, &format!("done: {name}"));
                }
                TxEvent::Warning { message } => append_line(&view, &format!("warning: {message}")),
                TxEvent::Error { message } => append_line(&view, &format!("error: {message}")),
                TxEvent::Cancelled => {
                    status.set_label("Cancelled");
                    progress.set_text(Some("cancelled"));
                    append_line(&view, "== cancelled ==");
                }
                TxEvent::Completed { succeeded, message } => {
                    status.set_label(if succeeded { "Completed" } else { "Failed" });
                    progress.set_text(Some(&message));
                    progress.set_visible(true);
                    append_line(
                        &view,
                        &format!("== {} ==", if succeeded { "completed" } else { "failed" }),
                    );
                }
            }));
        }

        Self { root }
    }

    pub fn root(&self) -> &adw::Clamp {
        &self.root
    }

    pub fn refresh_summary(&self) {}
}

fn append_line(view: &gtk::TextView, line: &str) {
    let buffer = view.buffer();
    let mut end = buffer.end_iter();
    buffer.insert(&mut end, line);
    buffer.insert(&mut end, "\n");
    // Auto-scroll to bottom.
    view.scroll_to_iter(&mut buffer.end_iter(), 0.0, false, 0.0, 1.0);
}

/// Repeating timeout helper (main loop).
fn glib_timeout_repeat<F: FnMut() + 'static>(ms: u64, mut f: F) {
    gtk::glib::timeout_add_local(std::time::Duration::from_millis(ms), move || {
        f();
        gtk::glib::ControlFlow::Continue
    });
}
