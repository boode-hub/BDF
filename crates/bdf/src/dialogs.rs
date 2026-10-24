//! Modal dialogs.

use crate::app::{BdfApp, Dialog, NoteTarget};
use crate::theme::{self, icon};
use bdf_core::engine::{AnnotKind, NewAnnot};
use bdf_core::layout::PageMode;
use bdf_core::settings::{EyeStyle, RendererPref, ThemePref, ZoomPref};
use eframe::egui::{
    self, Align, Button, Context, Id, Key, Layout, Modal, RichText, TextEdit, Ui, vec2,
};

enum Outcome {
    Keep,
    Close,
}

fn buttons(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    ui.add_space(8.0);
    ui.with_layout(Layout::right_to_left(Align::Center), add);
}

fn primary(ui: &mut Ui, label: &str) -> bool {
    let pal = theme::palette(ui.visuals().dark_mode);
    ui.add(
        Button::new(RichText::new(label).color(egui::Color32::WHITE))
            .fill(pal.accent)
            .min_size(vec2(84.0, 28.0)),
    )
    .clicked()
}

fn secondary(ui: &mut Ui, label: &str) -> bool {
    ui.add(
        Button::new(label)
            .min_size(vec2(84.0, 28.0))
            .stroke(ui.visuals().widgets.noninteractive.bg_stroke),
    )
    .clicked()
}

/// Only web and mail links open; everything else (file:, javascript:, …) is shown but not followed.
pub fn is_safe_uri(u: &str) -> bool {
    let l = u.trim().to_ascii_lowercase();
    l.starts_with("https://") || l.starts_with("http://") || l.starts_with("mailto:")
}

impl BdfApp {
    pub fn dialogs(&mut self, ctx: &Context) {
        let Some(mut dialog) = self.dialog.take() else {
            return;
        };
        let outcome = self.dialog_ui(ctx, &mut dialog);
        match outcome {
            Outcome::Keep => {
                // A new dialog may have been queued while handling this one.
                if self.dialog.is_none() {
                    self.dialog = Some(dialog);
                } else {
                    let next = self.dialog.take();
                    self.dialog = Some(dialog);
                    if let Some(n) = next {
                        self.show_dialog(n);
                    }
                }
            }
            Outcome::Close => {
                if self.dialog.is_none() {
                    self.close_dialog();
                }
            }
        }
    }

    fn dialog_ui(&mut self, ctx: &Context, dialog: &mut Dialog) -> Outcome {
        let esc = ctx.input(|i| i.key_pressed(Key::Escape));
        let enter = ctx.input(|i| i.key_pressed(Key::Enter) && !i.modifiers.shift);
        let mut outcome = Outcome::Keep;
        let modal = Modal::new(Id::unique("bdf-dialog"));
        match dialog {
            Dialog::Password {
                path,
                pane,
                input,
                error,
            } => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let mut submit = false;
                modal.show(ctx, |ui| {
                    ui.set_width(380.0);
                    ui.heading(format!("{}  Password required", icon::LOCK));
                    ui.label(format!(
                        "“{name}” is protected. Enter its password to open it."
                    ));
                    ui.add_space(6.0);
                    let r = ui.add(
                        TextEdit::singleline(input)
                            .password(true)
                            .hint_text("Password")
                            .desired_width(f32::INFINITY),
                    );
                    r.request_focus();
                    if let Some(e) = error {
                        ui.colored_label(theme::palette(ui.visuals().dark_mode).danger, e.as_str());
                    }
                    buttons(ui, |ui| {
                        if primary(ui, "Open") {
                            submit = true;
                        }
                        if secondary(ui, "Cancel") {
                            outcome = Outcome::Close;
                        }
                    });
                });
                if submit || (enter && !input.is_empty()) {
                    let (p, pw, pn) = (path.clone(), input.clone(), *pane);
                    self.open_with_password(p, pw, pn);
                    outcome = Outcome::Close;
                }
            }
            Dialog::Note {
                doc,
                page,
                target,
                text,
            } => {
                let mut save = false;
                modal.show(ctx, |ui| {
                    ui.set_width(420.0);
                    let title = match target {
                        NoteTarget::New(_) => "New note",
                        NoteTarget::Edit(a) if a.kind == AnnotKind::Note => "Edit note",
                        NoteTarget::Edit(_) => "Comment",
                    };
                    ui.heading(format!("{}  {title}", icon::NOTE));
                    let r = ui.add(
                        TextEdit::multiline(text)
                            .desired_rows(6)
                            .desired_width(f32::INFINITY)
                            .hint_text("Type your note…"),
                    );
                    // The note box owns the keyboard while the dialog is open.
                    r.request_focus();
                    ui.weak("Ctrl+Enter to save");
                    buttons(ui, |ui| {
                        if primary(ui, "Save") {
                            save = true;
                        }
                        if secondary(ui, "Cancel") {
                            outcome = Outcome::Close;
                        }
                    });
                });
                if ctx.input(|i| i.key_pressed(Key::Enter) && i.modifiers.command) {
                    save = true;
                }
                if save {
                    let (doc, page) = (*doc, *page);
                    match target.clone() {
                        NoteTarget::New((x, y)) => {
                            if !text.trim().is_empty() {
                                let color = [255, 213, 0];
                                self.add_annot(
                                    doc,
                                    page,
                                    NewAnnot::Note {
                                        x,
                                        y,
                                        color,
                                        text: text.clone(),
                                    },
                                );
                            }
                        }
                        NoteTarget::Edit(info) => {
                            self.set_annot_text(doc, page, &info, text.clone())
                        }
                    }
                    outcome = Outcome::Close;
                }
            }
            Dialog::OpenUri(u) => {
                let safe = is_safe_uri(u);
                modal.show(ctx, |ui| {
                    ui.set_width(440.0);
                    ui.heading(format!("{}  Open link?", icon::LINK));
                    ui.label("This document links to:");
                    ui.add(egui::Label::new(RichText::new(u.as_str()).monospace()).wrap());
                    if !safe {
                        ui.colored_label(
                            theme::palette(ui.visuals().dark_mode).danger,
                            "Only web and e-mail links can be opened from BDF.",
                        );
                    }
                    buttons(ui, |ui| {
                        if safe && primary(ui, "Open in browser") {
                            ctx.open_url(egui::OpenUrl::new_tab(u.clone()));
                            outcome = Outcome::Close;
                        }
                        if secondary(ui, "Copy link") {
                            ctx.copy_text(u.clone());
                            outcome = Outcome::Close;
                        }
                        if secondary(ui, "Cancel") {
                            outcome = Outcome::Close;
                        }
                    });
                });
            }
            Dialog::ConfirmClose { doc } => {
                let doc = *doc;
                let name = self.docs.get(&doc).map(|d| d.title()).unwrap_or_default();
                modal.show(ctx, |ui| {
                    ui.set_width(400.0);
                    ui.heading("Save changes?");
                    ui.label(format!("“{name}” has annotations that are not saved."));
                    buttons(ui, |ui| {
                        if primary(ui, "Save") {
                            self.save_and_close(ctx, doc);
                            outcome = Outcome::Close;
                        }
                        if secondary(ui, "Don't save") {
                            if let Some(d) = self.docs.get_mut(&doc) {
                                d.dirty = false;
                            }
                            self.close_doc_everywhere(doc);
                            outcome = Outcome::Close;
                        }
                        if secondary(ui, "Cancel") {
                            outcome = Outcome::Close;
                        }
                    });
                });
            }
            Dialog::ConfirmQuit => {
                let dirty: Vec<_> = self
                    .docs
                    .values()
                    .filter(|d| d.dirty)
                    .map(|d| (d.id, d.title()))
                    .collect();
                modal.show(ctx, |ui| {
                    ui.set_width(420.0);
                    ui.heading("Unsaved changes");
                    ui.label("These documents have changes that are not saved:");
                    for (_, t) in &dirty {
                        ui.label(format!("  •  {t}"));
                    }
                    buttons(ui, |ui| {
                        if primary(ui, "Save all") {
                            for (id, _) in &dirty {
                                self.save(ctx, *id, false);
                            }
                            self.quit_after_save_all();
                            outcome = Outcome::Close;
                        }
                        if secondary(ui, "Discard") {
                            for d in self.docs.values_mut() {
                                d.dirty = false;
                            }
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            outcome = Outcome::Close;
                        }
                        if secondary(ui, "Cancel") {
                            outcome = Outcome::Close;
                        }
                    });
                });
            }
            Dialog::ConfirmDelete { doc, page, info } => {
                let mut yes = false;
                modal.show(ctx, |ui| {
                    ui.set_width(380.0);
                    ui.heading(format!("{}  Delete annotation?", icon::TRASH));
                    ui.label(format!(
                        "This {} was not added in this session, so deleting it cannot be undone.",
                        info.kind.label().to_lowercase()
                    ));
                    buttons(ui, |ui| {
                        if primary(ui, "Delete") {
                            yes = true;
                        }
                        if secondary(ui, "Cancel") {
                            outcome = Outcome::Close;
                        }
                    });
                });
                if yes {
                    let (d, p, i) = (*doc, *page, info.clone());
                    self.delete_annot(d, p, i, true);
                    outcome = Outcome::Close;
                }
            }
            Dialog::Properties(doc) => {
                let Some(d) = self.docs.get(doc) else {
                    return Outcome::Close;
                };
                let info = d.info.clone();
                let path = d.path.clone();
                modal.show(ctx, |ui| {
                    ui.set_width(520.0);
                    ui.heading(format!("{}  Document properties", icon::INFO));
                    ui.add_space(4.0);
                    egui::Grid::new("props")
                        .num_columns(2)
                        .striped(true)
                        .spacing(vec2(14.0, 6.0))
                        .show(ui, |ui| {
                            let mut row = |k: &str, v: String| {
                                ui.label(RichText::new(k).strong());
                                ui.add(egui::Label::new(v).wrap());
                                ui.end_row();
                            };
                            row("File", path.display().to_string());
                            row(
                                "Size",
                                format!(
                                    "{:.2} MB ({} bytes)",
                                    info.file_size as f64 / 1_048_576.0,
                                    info.file_size
                                ),
                            );
                            row("Pages", info.page_sizes.len().to_string());
                            if let Some((w, h)) = info.page_sizes.first() {
                                row(
                                    "Page size",
                                    format!(
                                        "{:.0} × {:.0} pt ({:.1} × {:.1} mm)",
                                        w,
                                        h,
                                        w / 72.0 * 25.4,
                                        h / 72.0 * 25.4
                                    ),
                                );
                            }
                            row(
                                "PDF version",
                                info.pdf_version.clone().unwrap_or_else(|| "unknown".into()),
                            );
                            row(
                                "Encrypted",
                                if info.encrypted { "yes" } else { "no" }.into(),
                            );
                            row(
                                "Form fields",
                                if info.form_fields { "yes" } else { "no" }.into(),
                            );
                            for (k, v) in &info.metadata {
                                row(k, v.clone());
                            }
                        });
                    buttons(ui, |ui| {
                        if primary(ui, "Close") {
                            outcome = Outcome::Close;
                        }
                    });
                });
            }
            Dialog::Settings => {
                modal.show(ctx, |ui| {
                    ui.set_width(460.0);
                    ui.heading(format!("{}  Preferences", icon::GEAR));
                    ui.add_space(6.0);
                    let s = &mut self.settings;
                    let before = s.clone();
                    egui::Grid::new("prefs")
                        .num_columns(2)
                        .spacing(vec2(16.0, 10.0))
                        .show(ui, |ui| {
                            ui.label("Theme");
                            ui.horizontal(|ui| {
                                ui.radio_value(&mut s.theme, ThemePref::System, "System");
                                ui.radio_value(&mut s.theme, ThemePref::Light, "Light");
                                ui.radio_value(&mut s.theme, ThemePref::Dark, "Dark");
                            });
                            ui.end_row();
                            ui.label("Page layout");
                            egui::ComboBox::from_id_salt("pref-layout")
                                .selected_text(s.page_mode.label())
                                .show_ui(ui, |ui| {
                                    for m in PageMode::ALL {
                                        ui.selectable_value(&mut s.page_mode, m, m.label());
                                    }
                                });
                            ui.end_row();
                            ui.label("Opening zoom");
                            egui::ComboBox::from_id_salt("pref-zoom")
                                .selected_text(match s.default_zoom {
                                    ZoomPref::FitWidth => "Fit width",
                                    ZoomPref::FitPage => "Fit page",
                                    ZoomPref::Actual => "Actual size",
                                })
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut s.default_zoom,
                                        ZoomPref::FitWidth,
                                        "Fit width",
                                    );
                                    ui.selectable_value(
                                        &mut s.default_zoom,
                                        ZoomPref::FitPage,
                                        "Fit page",
                                    );
                                    ui.selectable_value(
                                        &mut s.default_zoom,
                                        ZoomPref::Actual,
                                        "Actual size",
                                    );
                                });
                            ui.end_row();
                            ui.label("Reading");
                            ui.vertical(|ui| {
                                ui.checkbox(
                                    &mut s.remember_page,
                                    "Reopen documents at the last page read",
                                );
                                ui.checkbox(&mut s.restore_tabs, "Reopen my tabs when BDF starts");
                                ui.checkbox(
                                    &mut s.cover_page,
                                    "Facing layouts: show the cover on its own",
                                );
                                ui.checkbox(&mut s.dark_pages, "Dark pages (night reading)");
                            });
                            ui.end_row();
                            ui.label("Highlight colour");
                            ui.color_edit_button_srgb(&mut s.highlight_color);
                            ui.end_row();
                            ui.label("Pen");
                            ui.horizontal(|ui| {
                                ui.color_edit_button_srgb(&mut s.ink_color);
                                ui.add(
                                    egui::Slider::new(&mut s.ink_width, 0.5..=12.0).suffix(" pt"),
                                );
                            });
                            ui.end_row();
                            ui.label("Sharingan");
                            ui.horizontal(|ui| {
                                ui.radio_value(&mut s.eye, EyeStyle::Kakashi, "Kakashi");
                                ui.radio_value(&mut s.eye, EyeStyle::Itachi, "Itachi");
                            });
                            ui.end_row();
                            ui.label("Magnifier");
                            ui.add(egui::Slider::new(&mut s.magnifier_zoom, 1.5..=8.0).suffix("×"));
                            ui.end_row();
                            ui.label("Image memory");
                            ui.add(
                                egui::Slider::new(&mut s.cache_mib, 64..=2048)
                                    .suffix(" MB")
                                    .logarithmic(true),
                            );
                            ui.end_row();
                            ui.label("Graphics");
                            egui::ComboBox::from_id_salt("pref-gfx")
                                .selected_text(match s.renderer {
                                    RendererPref::Auto => "Automatic",
                                    RendererPref::OpenGl => "OpenGL",
                                    RendererPref::Wgpu => "DirectX / Vulkan / Metal",
                                })
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut s.renderer,
                                        RendererPref::Auto,
                                        "Automatic",
                                    );
                                    ui.selectable_value(
                                        &mut s.renderer,
                                        RendererPref::OpenGl,
                                        "OpenGL",
                                    );
                                    ui.selectable_value(
                                        &mut s.renderer,
                                        RendererPref::Wgpu,
                                        "DirectX / Vulkan / Metal",
                                    );
                                });
                            ui.end_row();
                        });
                    ui.add_space(6.0);
                    ui.weak(format!(
                        "Settings are stored in {}{}",
                        self.data.path.display(),
                        if self.data.portable {
                            " (portable)"
                        } else {
                            ""
                        }
                    ));
                    if s.renderer != before.renderer {
                        ui.weak("The graphics change applies the next time BDF starts.");
                        // A new choice gets a fresh try ("Automatic" tries OpenGL again).
                        s.opengl_failed = false;
                    }
                    let changed = *s != before;
                    let theme_changed = s.theme != before.theme;
                    let eye = (s.eye != before.eye).then_some(s.eye);
                    let cache = s.cache_mib;
                    if changed {
                        self.settings_changed = true;
                        if theme_changed {
                            self.apply_theme(ctx);
                        }
                        if let Some(eye) = eye {
                            theme::install_art(ctx, eye);
                            ctx.send_viewport_cmd(egui::ViewportCommand::Icon(Some(
                                std::sync::Arc::new(theme::window_icon(eye)),
                            )));
                        }
                        self.tiles.set_budget(cache as usize * 1024 * 1024);
                    }
                    buttons(ui, |ui| {
                        if primary(ui, "Done") {
                            outcome = Outcome::Close;
                        }
                        if secondary(ui, "Open settings folder") {
                            crate::platform::reveal(&self.data.path);
                        }
                    });
                });
            }
            Dialog::Shortcuts => {
                modal.show(ctx, |ui| {
                    ui.set_width(520.0);
                    ui.heading(format!("{}  Keyboard shortcuts", icon::KEYBOARD));
                    egui::ScrollArea::vertical()
                        .max_height(420.0)
                        .show(ui, |ui| {
                            egui::Grid::new("keys")
                                .num_columns(2)
                                .striped(true)
                                .spacing(vec2(20.0, 4.0))
                                .show(ui, |ui| {
                                    for (k, v) in SHORTCUTS {
                                        ui.monospace(*k);
                                        ui.label(*v);
                                        ui.end_row();
                                    }
                                });
                        });
                    buttons(ui, |ui| {
                        if primary(ui, "Close") {
                            outcome = Outcome::Close;
                        }
                    });
                });
            }
            Dialog::About => {
                modal.show(ctx, |ui| {
                    ui.set_width(420.0);
                    ui.vertical_centered(|ui| {
                        theme::logo(ui, 72.0);
                        ui.heading(format!("BDF {}", env!("CARGO_PKG_VERSION")));
                        ui.label("A fast, portable PDF reader.");
                    });
                    ui.add_space(8.0);
                    egui::Grid::new("about").num_columns(2).spacing(vec2(12.0, 4.0)).show(ui, |ui| {
                        ui.label("Graphics");
                        ui.label(&self.renderer);
                        ui.end_row();
                        ui.label("Settings");
                        ui.label(format!("{}{}", self.data.path.display(), if self.data.portable { " (portable)" } else { "" }));
                        ui.end_row();
                    });
                    ui.add_space(6.0);
                    ui.weak("PDF rendering by PDFium (BSD licence). Icons by Phosphor (MIT licence). User interface built with egui (MIT/Apache-2.0).");
                    buttons(ui, |ui| {
                        if primary(ui, "Close") {
                            outcome = Outcome::Close;
                        }
                    });
                });
            }
            Dialog::Error { title, text } => {
                modal.show(ctx, |ui| {
                    ui.set_width(440.0);
                    ui.heading(format!("{}  {title}", icon::WARNING));
                    ui.add(egui::Label::new(text.as_str()).wrap());
                    buttons(ui, |ui| {
                        if primary(ui, "OK") {
                            outcome = Outcome::Close;
                        }
                    });
                });
                if enter {
                    outcome = Outcome::Close;
                }
            }
            Dialog::Print {
                doc,
                ranges,
                copies,
                error,
            } => {
                let Some(d) = self.docs.get(doc) else {
                    return Outcome::Close;
                };
                let (pages, dirty, path) = (d.page_count() as u32, d.dirty, d.path.clone());
                let mut go = false;
                modal.show(ctx, |ui| {
                    ui.set_width(400.0);
                    ui.heading(format!("{}  Print", icon::PRINT));
                    egui::Grid::new("print")
                        .num_columns(2)
                        .spacing(vec2(12.0, 8.0))
                        .show(ui, |ui| {
                            ui.label("Pages");
                            ui.add(
                                TextEdit::singleline(ranges)
                                    .hint_text(format!("all (1-{pages}), or e.g. 1-3, 5")),
                            );
                            ui.end_row();
                            ui.label("Copies");
                            ui.add(egui::DragValue::new(copies).range(1..=99));
                            ui.end_row();
                        });
                    if dirty {
                        ui.colored_label(
                            theme::palette(ui.visuals().dark_mode).danger,
                            "Unsaved annotations are not printed. Save first to include them.",
                        );
                    }
                    ui.weak("Prints on the default printer through CUPS.");
                    if let Some(e) = error {
                        ui.colored_label(theme::palette(ui.visuals().dark_mode).danger, e.as_str());
                    }
                    buttons(ui, |ui| {
                        if primary(ui, "Print") {
                            go = true;
                        }
                        if secondary(ui, "Cancel") {
                            outcome = Outcome::Close;
                        }
                    });
                });
                if go {
                    match crate::print::PageRanges::parse(ranges, pages) {
                        Err(e) => *error = Some(e),
                        Ok(r) => {
                            #[cfg(not(windows))]
                            match crate::print::lp_print(&path, &r, *copies) {
                                Ok(()) => {
                                    self.toast(ctx, "Sent to the printer", false);
                                    outcome = Outcome::Close;
                                }
                                Err(e) => *error = Some(e),
                            }
                            #[cfg(windows)]
                            {
                                let _ = (&r, &path);
                                outcome = Outcome::Close;
                            }
                        }
                    }
                }
            }
        }
        if esc && !matches!(dialog, Dialog::ConfirmQuit) {
            outcome = Outcome::Close;
        }
        outcome
    }
}

pub const SHORTCUTS: &[(&str, &str)] = &[
    ("Ctrl+O", "Open"),
    ("Ctrl+S / Ctrl+Shift+S", "Save / Save as"),
    ("Ctrl+P", "Print"),
    ("Ctrl+W", "Close tab"),
    ("Ctrl+Tab / Ctrl+Shift+Tab", "Next / previous tab"),
    ("Ctrl+\\", "Split view on or off"),
    ("Ctrl+F, F3, Shift+F3", "Find, next, previous"),
    ("Ctrl+G", "Go to page"),
    ("Home / End", "First / last page"),
    ("PgUp / PgDn, Space", "Previous / next screen"),
    ("Left / Right", "Previous / next page"),
    ("Alt+Left / Alt+Right", "Back / forward"),
    ("Ctrl + mouse wheel", "Zoom"),
    ("Ctrl++ / Ctrl+-", "Zoom in / out"),
    (
        "Ctrl+0 / Ctrl+1 / Ctrl+2",
        "Fit page / actual size / fit width",
    ),
    (
        "Ctrl+R / Ctrl+Shift+R",
        "Rotate clockwise / counter-clockwise",
    ),
    ("V / H / Z", "Select / hand / magnifier tool"),
    ("Middle mouse drag", "Pan with any tool"),
    ("Ctrl+C / Ctrl+A", "Copy / select all on page"),
    ("Double / triple click", "Select word / line"),
    ("Delete", "Delete selected annotation"),
    ("Ctrl+Z / Ctrl+Y", "Undo / redo annotation"),
    ("Ctrl+B", "Bookmark current page"),
    ("Ctrl+D", "Document properties"),
    ("F4", "Sidebar"),
    ("F11 / Esc", "Full screen / leave full screen"),
    ("F12", "Performance overlay"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_web_and_mail_links_open() {
        assert!(is_safe_uri("https://example.com"));
        assert!(is_safe_uri(" HTTP://x"));
        assert!(is_safe_uri("mailto:a@b.c"));
        assert!(!is_safe_uri("file:///etc/passwd"));
        assert!(!is_safe_uri("javascript:alert(1)"));
        assert!(!is_safe_uri("\\\\server\\share\\x.exe"));
        assert!(!is_safe_uri("cmd.exe"));
    }
}
