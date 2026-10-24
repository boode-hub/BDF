//! Menus, toolbar, tab strips, status bar, find bar and the welcome screen.

use crate::app::{BdfApp, Dialog, SidebarTab};
use crate::theme::{self, icon};
use crate::view::{Tool, ZOOM_STEPS, ZoomMode};
use bdf_core::layout::PageMode;
use bdf_core::settings::ThemePref;
use eframe::egui::{
    self, Align, Button, Color32, Context, CornerRadius, Id, Layout, Rect, RichText, Sense, Stroke,
    StrokeKind, TextEdit, Ui, pos2, vec2,
};

/// Icon button with a tooltip; `selected` draws it as toggled on.
fn tool_button(ui: &mut Ui, glyph: &str, tip: &str, selected: bool, enabled: bool) -> bool {
    let pal = theme::palette(ui.visuals().dark_mode);
    let mut b = Button::new(RichText::new(glyph).size(17.0)).min_size(vec2(28.0, 28.0));
    if selected {
        b = b
            .fill(pal.accent_soft)
            .stroke(Stroke::new(1.0, pal.accent.gamma_multiply(0.6)));
    }
    ui.add_enabled(enabled, b).on_hover_text(tip).clicked()
}

/// Menu text with its icon in a fixed-width column. Labels written as
/// "<icon>  text" keep their icon; labels padded with spaces get an invisible
/// placeholder of the same width, so every item lines up.
fn menu_text(ui: &Ui, label: &str) -> egui::WidgetText {
    use egui::text::{LayoutJob, TextFormat};
    let font = egui::TextStyle::Button.resolve(ui.style());
    // PLACEHOLDER lets the button apply its own colour (normal, hovered, disabled).
    let color = Color32::PLACEHOLDER;
    let first = label.chars().next().unwrap_or(' ');
    let is_icon = ('\u{E000}'..='\u{F8FF}').contains(&first);
    let (glyph, glyph_color, rest) = if is_icon {
        (
            first.to_string(),
            color,
            label[first.len_utf8()..].trim_start(),
        )
    } else {
        (
            icon::CHECK.to_string(),
            Color32::TRANSPARENT,
            label.trim_start(),
        )
    };
    let mut job = LayoutJob::default();
    job.append(
        &glyph,
        0.0,
        TextFormat {
            font_id: font.clone(),
            color: glyph_color,
            ..Default::default()
        },
    );
    job.append(
        rest,
        10.0,
        TextFormat {
            font_id: font,
            color,
            ..Default::default()
        },
    );
    job.into()
}

fn menu_item(ui: &mut Ui, label: &str, shortcut: &str, enabled: bool) -> bool {
    let b = Button::new(menu_text(ui, label)).shortcut_text(shortcut);
    let r = ui.add_enabled(enabled, b);
    if r.clicked() {
        ui.close();
        true
    } else {
        false
    }
}

fn check_item(ui: &mut Ui, label: &str, shortcut: &str, on: bool) -> bool {
    let text = if on {
        format!("{}  {label}", icon::CHECK)
    } else {
        format!("      {label}")
    };
    menu_item(ui, &text, shortcut, true)
}

impl BdfApp {
    pub fn menu_and_toolbar(&mut self, ui: &mut Ui, ctx: &Context) {
        egui::Panel::top("menu")
            .frame(
                egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin::symmetric(8, 2)),
            )
            .show(ui, |ui| {
                egui::MenuBar::new().ui(ui, |ui| {
                    crate::theme::logo(ui, 18.0).on_hover_text("BDF");
                    self.menus(ui, ctx)
                });
            });
        egui::Panel::top("toolbar")
            .frame(
                egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin::symmetric(8, 5)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    self.toolbar(ui, ctx)
                });
            });
    }

    fn menus(&mut self, ui: &mut Ui, ctx: &Context) {
        let has_doc = self.active_tab().is_some();
        let doc = self.active_doc_id();
        let pane = self.active_pane;
        ui.menu_button("File", |ui| {
            ui.set_min_width(230.0);
            if menu_item(ui, &format!("{}  Open…", icon::OPEN), "Ctrl+O", true) {
                self.open_dialog(pane);
            }
            ui.menu_button(format!("{}  Open recent", icon::RECENT), |ui| {
                ui.set_min_width(320.0);
                let recent = self.settings.recent.clone();
                if recent.is_empty() {
                    ui.weak("Nothing yet");
                }
                for r in recent {
                    let name = r
                        .path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let exists = r.path.exists();
                    if ui
                        .add_enabled(exists, Button::new(name))
                        .on_hover_text(r.path.display().to_string())
                        .clicked()
                    {
                        self.open_path(&r.path, pane);
                        ui.close();
                    }
                }
                ui.separator();
                if ui.button("Clear list").clicked() {
                    self.settings.recent.clear();
                    self.settings_changed = true;
                    ui.close();
                }
            });
            ui.separator();
            let dirty = doc.and_then(|d| self.docs.get(&d)).is_some_and(|d| d.dirty);
            if menu_item(ui, &format!("{}  Save", icon::SAVE), "Ctrl+S", dirty)
                && let Some(d) = doc
            {
                self.save(ctx, d, false);
            }
            if menu_item(ui, "      Save as…", "Ctrl+Shift+S", has_doc)
                && let Some(d) = doc
            {
                self.save(ctx, d, true);
            }
            if menu_item(ui, &format!("{}  Print…", icon::PRINT), "Ctrl+P", has_doc) {
                self.print(ctx);
            }
            ui.separator();
            if menu_item(
                ui,
                &format!("{}  Properties", icon::INFO),
                "Ctrl+D",
                has_doc,
            ) && let Some(d) = doc
            {
                self.show_dialog(Dialog::Properties(d));
            }
            if menu_item(
                ui,
                &format!("{}  Close tab", icon::CLOSE),
                "Ctrl+W",
                has_doc,
            ) {
                let a = self.panes[pane].active;
                self.close_tab(pane, a, false);
            }
            if menu_item(ui, "      Exit", "Alt+F4", true) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
        ui.menu_button("Edit", |ui| {
            ui.set_min_width(230.0);
            let (can_undo, can_redo) = doc
                .and_then(|d| self.docs.get(&d))
                .map(|d| (!d.undo.is_empty(), !d.redo.is_empty()))
                .unwrap_or((false, false));
            if menu_item(ui, &format!("{}  Undo", icon::UNDO), "Ctrl+Z", can_undo) {
                self.undo(ctx);
            }
            if menu_item(ui, &format!("{}  Redo", icon::REDO), "Ctrl+Y", can_redo) {
                self.redo(ctx);
            }
            ui.separator();
            let has_sel = self
                .active_tab()
                .is_some_and(|t| t.view.selection.is_some());
            if menu_item(ui, &format!("{}  Copy", icon::COPY), "Ctrl+C", has_sel) {
                self.copy_selection(ctx);
            }
            if menu_item(ui, "      Select all on page", "Ctrl+A", has_doc) {
                self.select_all_on_page();
            }
            ui.separator();
            if menu_item(ui, &format!("{}  Find…", icon::SEARCH), "Ctrl+F", has_doc) {
                self.find.open = true;
                self.find.focus = true;
            }
            if menu_item(
                ui,
                &format!("{}  Add bookmark", icon::BOOKMARK),
                "Ctrl+B",
                has_doc,
            ) && let Some(t) = self.active_tab()
            {
                let (d, p) = (t.doc, t.view.current_page as u32);
                self.add_bookmark(ctx, d, p);
            }
            ui.separator();
            if menu_item(ui, &format!("{}  Preferences…", icon::GEAR), "", true) {
                self.show_dialog(Dialog::Settings);
            }
        });
        ui.menu_button("View", |ui| {
            ui.set_min_width(250.0);
            if check_item(ui, "Sidebar", "F4", self.settings.sidebar) {
                self.settings.sidebar = !self.settings.sidebar;
                self.settings_changed = true;
            }
            ui.menu_button("      Page layout", |ui| {
                ui.set_min_width(220.0);
                let mode = self
                    .active_tab()
                    .map(|t| t.view.mode)
                    .unwrap_or(self.settings.page_mode);
                for m in PageMode::ALL {
                    if check_item(ui, m.label(), "", mode == m) {
                        if let Some(v) = self.active_view_mut() {
                            v.set_mode(m);
                        }
                        self.settings.page_mode = m;
                        self.settings_changed = true;
                    }
                }
                ui.separator();
                let cover = self
                    .active_tab()
                    .map(|t| t.view.cover)
                    .unwrap_or(self.settings.cover_page);
                if check_item(ui, "Show cover page separately", "", cover) {
                    if let Some(v) = self.active_view_mut() {
                        v.set_cover(!cover);
                    }
                    self.settings.cover_page = !cover;
                    self.settings_changed = true;
                }
            });
            ui.menu_button("      Zoom", |ui| {
                ui.set_min_width(200.0);
                if menu_item(
                    ui,
                    &format!("{}  Zoom in", icon::ZOOM_IN),
                    "Ctrl++",
                    has_doc,
                ) && let Some(v) = self.active_view_mut()
                {
                    v.zoom_step(1, None);
                }
                if menu_item(
                    ui,
                    &format!("{}  Zoom out", icon::ZOOM_OUT),
                    "Ctrl+-",
                    has_doc,
                ) && let Some(v) = self.active_view_mut()
                {
                    v.zoom_step(-1, None);
                }
                if menu_item(ui, "      Actual size", "Ctrl+1", has_doc)
                    && let Some(v) = self.active_view_mut()
                {
                    v.set_zoom(1.0, None, ZoomMode::Custom);
                }
                if menu_item(
                    ui,
                    &format!("{}  Fit width", icon::FIT_WIDTH),
                    "Ctrl+2",
                    has_doc,
                ) && let Some(v) = self.active_view_mut()
                {
                    v.fit(ZoomMode::FitWidth);
                }
                if menu_item(
                    ui,
                    &format!("{}  Fit page", icon::FIT_PAGE),
                    "Ctrl+0",
                    has_doc,
                ) && let Some(v) = self.active_view_mut()
                {
                    v.fit(ZoomMode::FitPage);
                }
            });
            if menu_item(
                ui,
                &format!("{}  Rotate clockwise", icon::ROTATE_CW),
                "Ctrl+R",
                has_doc,
            ) && let Some(v) = self.active_view_mut()
            {
                v.rotate(true);
            }
            if menu_item(
                ui,
                &format!("{}  Rotate counter-clockwise", icon::ROTATE_CCW),
                "Ctrl+Shift+R",
                has_doc,
            ) && let Some(v) = self.active_view_mut()
            {
                v.rotate(false);
            }
            ui.separator();
            let split = self.panes.len() == 2;
            if check_item(
                ui,
                "Split side by side",
                "Ctrl+\\",
                split && !self.split_vertical,
            ) {
                self.toggle_split(false);
            }
            if check_item(ui, "Split top and bottom", "", split && self.split_vertical) {
                self.toggle_split(true);
            }
            if check_item(ui, "Synchronised scrolling", "", self.sync_scroll) {
                self.sync_scroll = !self.sync_scroll;
            }
            ui.separator();
            if check_item(
                ui,
                "Dark pages (night reading)",
                "",
                self.settings.dark_pages,
            ) {
                self.settings.dark_pages = !self.settings.dark_pages;
                self.settings_changed = true;
            }
            ui.menu_button("      Theme", |ui| {
                for (t, label) in [
                    (ThemePref::System, "Follow system"),
                    (ThemePref::Light, "Light"),
                    (ThemePref::Dark, "Dark"),
                ] {
                    if check_item(ui, label, "", self.settings.theme == t) {
                        self.settings.theme = t;
                        self.apply_theme(ctx);
                    }
                }
            });
            if check_item(ui, "Full screen", "F11", self.fullscreen) {
                let fs = !self.fullscreen;
                self.set_fullscreen(ctx, fs);
            }
            if check_item(ui, "Performance overlay", "F12", self.perf.show) {
                self.perf.show = !self.perf.show;
            }
        });
        ui.menu_button("Go", |ui| {
            ui.set_min_width(220.0);
            let (back, fwd) = self
                .active_tab()
                .map(|t| (t.view.can_go_back(), t.view.can_go_forward()))
                .unwrap_or((false, false));
            let n = doc
                .and_then(|d| self.docs.get(&d))
                .map(|d| d.page_count())
                .unwrap_or(0);
            if menu_item(ui, &format!("{}  First page", icon::FIRST), "Home", has_doc)
                && let Some(v) = self.active_view_mut()
            {
                v.first_page();
            }
            if menu_item(
                ui,
                &format!("{}  Previous page", icon::PREV),
                "Left",
                has_doc,
            ) && let Some(v) = self.active_view_mut()
            {
                v.turn(-1);
            }
            if menu_item(ui, &format!("{}  Next page", icon::NEXT), "Right", has_doc)
                && let Some(v) = self.active_view_mut()
            {
                v.turn(1);
            }
            if menu_item(ui, &format!("{}  Last page", icon::LAST), "End", has_doc)
                && let Some(v) = self.active_view_mut()
            {
                v.last_page(n);
            }
            if menu_item(ui, "      Go to page…", "Ctrl+G", has_doc) {
                self.page_input_focus = true;
            }
            ui.separator();
            if menu_item(ui, &format!("{}  Back", icon::BACK), "Alt+Left", back)
                && let Some(v) = self.active_view_mut()
            {
                v.go_back();
            }
            if menu_item(ui, &format!("{}  Forward", icon::FORWARD), "Alt+Right", fwd)
                && let Some(v) = self.active_view_mut()
            {
                v.go_forward();
            }
        });
        ui.menu_button("Tools", |ui| {
            ui.set_min_width(220.0);
            for (t, glyph, key) in [
                (Tool::Select, icon::SELECT, "V"),
                (Tool::Hand, icon::HAND, "H"),
                (Tool::Magnifier, icon::SEARCH, "Z"),
                (Tool::Highlight, icon::HIGHLIGHT, ""),
                (Tool::Underline, icon::UNDERLINE, ""),
                (Tool::StrikeOut, icon::STRIKE, ""),
                (Tool::Note, icon::NOTE, ""),
                (Tool::Ink, icon::PEN, ""),
                (Tool::Rect, icon::SQUARE, ""),
                (Tool::Ellipse, icon::CIRCLE, ""),
            ] {
                let label = format!("{glyph}  {}", t.label());
                let text = if self.tool == t {
                    RichText::new(label).strong()
                } else {
                    RichText::new(label)
                };
                if ui.add(Button::new(text).shortcut_text(key)).clicked() {
                    self.set_tool(t);
                    ui.close();
                }
            }
        });
        ui.menu_button("Help", |ui| {
            ui.set_min_width(220.0);
            if menu_item(
                ui,
                &format!("{}  Keyboard shortcuts", icon::KEYBOARD),
                "",
                true,
            ) {
                self.show_dialog(Dialog::Shortcuts);
            }
            if menu_item(ui, &format!("{}  About BDF", icon::INFO), "", true) {
                self.show_dialog(Dialog::About);
            }
        });
    }

    fn toolbar(&mut self, ui: &mut Ui, ctx: &Context) {
        let has_doc = self.active_tab().is_some();
        let pane = self.active_pane;
        let doc = self.active_doc_id();
        let dirty = doc.and_then(|d| self.docs.get(&d)).is_some_and(|d| d.dirty);
        if tool_button(ui, icon::OPEN, "Open (Ctrl+O)", false, true) {
            self.open_dialog(pane);
        }
        if tool_button(ui, icon::SAVE, "Save (Ctrl+S)", false, dirty)
            && let Some(d) = doc
        {
            self.save(ctx, d, false);
        }
        if tool_button(ui, icon::PRINT, "Print (Ctrl+P)", false, has_doc) {
            self.print(ctx);
        }
        ui.separator();
        if tool_button(
            ui,
            icon::SIDEBAR,
            "Sidebar (F4)",
            self.settings.sidebar,
            true,
        ) {
            self.settings.sidebar = !self.settings.sidebar;
            self.settings_changed = true;
        }
        ui.separator();
        for (t, glyph, tip) in [
            (Tool::Select, icon::SELECT, "Select text (V)"),
            (Tool::Hand, icon::HAND, "Hand (H)"),
            (
                Tool::Magnifier,
                icon::SEARCH,
                "Magnifier (Z) — click to zoom in, Shift+click to zoom out",
            ),
        ] {
            if tool_button(ui, glyph, tip, self.tool == t, true) {
                self.set_tool(t);
            }
        }
        ui.separator();
        for (t, glyph, tip) in [
            (Tool::Highlight, icon::HIGHLIGHT, "Highlight text"),
            (Tool::Underline, icon::UNDERLINE, "Underline text"),
            (Tool::StrikeOut, icon::STRIKE, "Strike out text"),
            (Tool::Note, icon::NOTE, "Add a note"),
            (Tool::Ink, icon::PEN, "Pen"),
            (Tool::Rect, icon::SQUARE, "Rectangle"),
            (Tool::Ellipse, icon::CIRCLE, "Ellipse"),
        ] {
            if tool_button(ui, glyph, tip, self.tool == t, has_doc) {
                self.set_tool(t);
            }
        }
        // Colour for the current tool.
        let c = match self.tool {
            Tool::Highlight => &mut self.settings.highlight_color,
            _ => &mut self.settings.ink_color,
        };
        let mut rgb = *c;
        let swatch = ui
            .scope(|ui| {
                ui.spacing_mut().interact_size = vec2(20.0, 20.0);
                ui.color_edit_button_srgb(&mut rgb)
            })
            .inner
            .on_hover_text("Colour for the current tool");
        if swatch.changed() {
            *c = rgb;
            self.settings_changed = true;
        }
        ui.separator();

        // Page navigation.
        let (page, n) = self
            .active_tab()
            .and_then(|t| {
                self.docs
                    .get(&t.doc)
                    .map(|d| (t.view.current_page, d.page_count()))
            })
            .unwrap_or_default();
        if tool_button(ui, icon::PREV, "Previous page", false, has_doc && page > 0)
            && let Some(v) = self.active_view_mut()
        {
            v.turn(-1);
        }
        let id = Id::unique("page-input");
        let focused = ctx.memory(|m| m.has_focus(id));
        if !focused {
            self.page_input = if has_doc {
                (page + 1).to_string()
            } else {
                String::new()
            };
        }
        let r = ui.add_enabled(
            has_doc,
            TextEdit::singleline(&mut self.page_input)
                .id(id)
                .desired_width(44.0)
                .align(egui::Align2::CENTER_CENTER),
        );
        if self.page_input_focus {
            r.request_focus();
            self.page_input_focus = false;
        }
        if r.lost_focus()
            && ctx.input(|i| i.key_pressed(egui::Key::Enter))
            && let Some(target) = self.resolve_page_input(&self.page_input.clone())
            && let Some(v) = self.active_view_mut()
        {
            v.goto(target, true);
        }
        ui.label(RichText::new(format!("/ {n}")).weak());
        if tool_button(ui, icon::NEXT, "Next page", false, has_doc && page + 1 < n)
            && let Some(v) = self.active_view_mut()
        {
            v.turn(1);
        }
        ui.separator();

        // Zoom.
        let zoom = self.active_tab().map(|t| (t.view.zoom, t.view.zoom_mode));
        if tool_button(ui, icon::ZOOM_OUT, "Zoom out (Ctrl+-)", false, has_doc)
            && let Some(v) = self.active_view_mut()
        {
            v.zoom_step(-1, None);
        }
        let label = match zoom {
            Some((_, ZoomMode::FitWidth)) => "Fit width".to_string(),
            Some((_, ZoomMode::FitPage)) => "Fit page".to_string(),
            Some((z, _)) => format!("{:.0}%", z * 100.0),
            None => "—".into(),
        };
        let label = match zoom {
            Some((z, m)) if m != ZoomMode::Custom => format!("{label} ({:.0}%)", z * 100.0),
            _ => label,
        };
        ui.add_enabled_ui(has_doc, |ui| {
            egui::ComboBox::from_id_salt("zoom")
                .selected_text(label)
                .width(130.0)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(false, format!("{}  Fit width", icon::FIT_WIDTH))
                        .clicked()
                        && let Some(v) = self.active_view_mut()
                    {
                        v.fit(ZoomMode::FitWidth);
                    }
                    if ui
                        .selectable_label(false, format!("{}  Fit page", icon::FIT_PAGE))
                        .clicked()
                        && let Some(v) = self.active_view_mut()
                    {
                        v.fit(ZoomMode::FitPage);
                    }
                    ui.separator();
                    for z in ZOOM_STEPS.iter().copied().chain([12.0, 16.0, 32.0, 64.0]) {
                        if ui
                            .selectable_label(false, format!("{:.0}%", z * 100.0))
                            .clicked()
                            && let Some(v) = self.active_view_mut()
                        {
                            v.set_zoom(z, None, ZoomMode::Custom);
                        }
                    }
                });
        });
        if tool_button(ui, icon::ZOOM_IN, "Zoom in (Ctrl++)", false, has_doc)
            && let Some(v) = self.active_view_mut()
        {
            v.zoom_step(1, None);
        }
        ui.separator();
        if tool_button(
            ui,
            icon::ROTATE_CW,
            "Rotate clockwise (Ctrl+R)",
            false,
            has_doc,
        ) && let Some(v) = self.active_view_mut()
        {
            v.rotate(true);
        }
        let pm = self
            .active_tab()
            .map(|t| t.view.mode)
            .unwrap_or(self.settings.page_mode);
        let (glyph, tip) = match pm {
            PageMode::Single => (icon::FILE, "Layout: single page"),
            PageMode::Continuous => (icon::SCROLL, "Layout: continuous"),
            PageMode::Facing => (icon::BOOK, "Layout: facing"),
            PageMode::ContinuousFacing => (icon::COLUMNS, "Layout: continuous facing"),
        };
        ui.add_enabled_ui(has_doc, |ui| {
            ui.menu_button(RichText::new(glyph).size(17.0), |ui| {
                for m in PageMode::ALL {
                    if ui.selectable_label(pm == m, m.label()).clicked() {
                        if let Some(v) = self.active_view_mut() {
                            v.set_mode(m);
                        }
                        self.settings.page_mode = m;
                        self.settings_changed = true;
                        ui.close();
                    }
                }
            })
            .response
            .on_hover_text(tip);
        });
        let split = self.panes.len() == 2;
        if tool_button(ui, icon::SPLIT_H, "Split view (Ctrl+\\)", split, true) {
            self.toggle_split(self.split_vertical);
        }
        if tool_button(
            ui,
            if self.settings.dark_pages {
                icon::SUN
            } else {
                icon::MOON
            },
            "Dark pages (night reading)",
            self.settings.dark_pages,
            true,
        ) {
            self.settings.dark_pages = !self.settings.dark_pages;
            self.settings_changed = true;
        }

        // Search on the right.
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if tool_button(ui, icon::SEARCH, "Find (Ctrl+F)", self.find.open, has_doc) {
                if self.find.open {
                    self.close_find();
                } else {
                    self.find.open = true;
                    self.find.focus = true;
                }
            }
        });
    }

    /// "12" or a page label like "iv".
    fn resolve_page_input(&self, s: &str) -> Option<usize> {
        let s = s.trim();
        let d = self.active_tab().and_then(|t| self.docs.get(&t.doc))?;
        if let Some(i) = d
            .info
            .page_labels
            .iter()
            .position(|l| l.as_deref() == Some(s))
        {
            return Some(i);
        }
        let n: usize = s.parse().ok()?;
        (n >= 1).then(|| (n - 1).min(d.page_count().saturating_sub(1)))
    }

    pub fn status_bar(&mut self, ui: &mut Ui, _ctx: &Context) {
        egui::Panel::bottom("status")
            .frame(
                egui::Frame::side_top_panel(ui.style())
                    .inner_margin(egui::Margin::symmetric(10, 3)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let small = |t: String| RichText::new(t).size(12.5);
                    if let Some(t) = self.active_tab() {
                        if let Some(d) = self.docs.get(&t.doc) {
                            ui.label(small(format!("Page {}", d.label(t.view.current_page))));
                            ui.separator();
                            ui.label(small(format!("{:.0}%", t.view.zoom * 100.0)));
                            ui.separator();
                            ui.label(small(self.tool.label().to_string()));
                            if d.dirty {
                                ui.separator();
                                ui.label(
                                    small("Unsaved changes".into())
                                        .color(theme::palette(ui.visuals().dark_mode).accent),
                                );
                            }
                            if d.saving {
                                ui.separator();
                                crate::theme::spinner(ui, 14.0);
                                ui.label(small("Saving…".into()));
                            }
                        }
                    } else {
                        ui.label(small("No document".into()).weak());
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if self.tiles.pending() > 0 || self.opening() > 0 {
                            crate::theme::spinner(ui, 14.0);
                        }
                        if let Some(t) = self.active_tab()
                            && let Some(d) = self.docs.get(&t.doc)
                        {
                            ui.label(small(d.path.display().to_string()).weak());
                        }
                    });
                });
            });
    }

    pub fn tab_strip(&mut self, ui: &mut Ui, ctx: &Context, pane: usize, rect: Rect) {
        let dark = ui.visuals().dark_mode;
        let pal = theme::palette(dark);
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, CornerRadius::ZERO, ui.visuals().panel_fill);
        painter.line_segment(
            [rect.left_bottom(), rect.right_bottom()],
            ui.visuals().widgets.noninteractive.bg_stroke,
        );
        let mut x = rect.min.x + 6.0;
        let mut close = None;
        let mut activate = None;
        let mut menu_action: Option<(usize, u8)> = None;
        let font = egui::FontId::proportional(13.0);
        let n_tabs = self.panes[pane].tabs.len();
        let max_w = ((rect.width() - 50.0) / n_tabs.max(1) as f32).clamp(90.0, 220.0);
        for (i, tab) in self.panes[pane].tabs.iter().enumerate() {
            let Some(d) = self.docs.get(&tab.doc) else {
                continue;
            };
            let title = d.title();
            let active = i == self.panes[pane].active;
            let galley =
                painter.layout_no_wrap(title.clone(), font.clone(), ui.visuals().text_color());
            let w = (galley.size().x + 56.0).min(max_w);
            let r = Rect::from_min_size(pos2(x, rect.min.y + 4.0), vec2(w, rect.height() - 4.0));
            if r.min.x > rect.max.x - 40.0 {
                break;
            }
            let resp = ui.interact(r, Id::unique(("tab", pane, i)), Sense::click());
            let fill = if active {
                ui.visuals().extreme_bg_color
            } else if resp.hovered() {
                ui.visuals().widgets.hovered.weak_bg_fill
            } else {
                Color32::TRANSPARENT
            };
            painter.rect_filled(
                r,
                CornerRadius {
                    nw: 7,
                    ne: 7,
                    sw: 0,
                    se: 0,
                },
                fill,
            );
            if active {
                let accent = if pane == self.active_pane || self.panes.len() == 1 {
                    pal.accent
                } else {
                    pal.muted
                };
                painter.line_segment(
                    [
                        r.left_top() + vec2(6.0, 0.5),
                        r.right_top() + vec2(-6.0, 0.5),
                    ],
                    Stroke::new(2.0, accent),
                );
            }
            painter.text(
                r.left_center() + vec2(10.0, 0.0),
                egui::Align2::LEFT_CENTER,
                icon::PDF,
                egui::FontId::proportional(14.0),
                pal.danger,
            );
            let text_clip = Rect::from_min_max(r.min + vec2(28.0, 0.0), r.max - vec2(22.0, 0.0));
            painter.with_clip_rect(text_clip).galley(
                pos2(text_clip.min.x, r.center().y - galley.size().y * 0.5),
                galley,
                ui.visuals().text_color(),
            );
            // Close button, or a dot for unsaved changes.
            let cr = Rect::from_center_size(pos2(r.max.x - 13.0, r.center().y), vec2(18.0, 18.0));
            let cresp = ui.interact(cr, Id::unique(("tab-close", pane, i)), Sense::click());
            if cresp.hovered() {
                painter.rect_filled(
                    cr,
                    CornerRadius::same(4),
                    ui.visuals().widgets.hovered.weak_bg_fill,
                );
            }
            if d.dirty && !cresp.hovered() {
                painter.circle_filled(cr.center(), 3.5, pal.accent);
            } else if active || resp.hovered() || cresp.hovered() {
                painter.text(
                    cr.center(),
                    egui::Align2::CENTER_CENTER,
                    icon::CLOSE,
                    egui::FontId::proportional(12.0),
                    ui.visuals().text_color(),
                );
            }
            if cresp.clicked() || resp.middle_clicked() {
                close = Some(i);
            } else if resp.clicked() {
                activate = Some(i);
            }
            let resp = resp.on_hover_text(d.path.display().to_string());
            resp.context_menu(|ui| {
                if ui.button("Close").clicked() {
                    menu_action = Some((i, 0));
                    ui.close();
                }
                if ui.button("Close other tabs").clicked() {
                    menu_action = Some((i, 1));
                    ui.close();
                }
                let label = if self.panes.len() == 2 {
                    "Move to other pane"
                } else {
                    "Move to split view"
                };
                if ui.button(label).clicked() {
                    menu_action = Some((i, 2));
                    ui.close();
                }
            });
            x += w + 2.0;
        }
        // "+" opens a file into this pane.
        let plus = Rect::from_min_size(
            pos2(x + 4.0, rect.min.y + 6.0),
            vec2(26.0, rect.height() - 10.0),
        );
        if plus.max.x <= rect.max.x {
            let r = ui.interact(plus, Id::unique(("tab-plus", pane)), Sense::click());
            if r.hovered() {
                painter.rect_filled(
                    plus,
                    CornerRadius::same(6),
                    ui.visuals().widgets.hovered.weak_bg_fill,
                );
            }
            painter.text(
                plus.center(),
                egui::Align2::CENTER_CENTER,
                icon::PLUS,
                egui::FontId::proportional(14.0),
                ui.visuals().text_color(),
            );
            if r.on_hover_text("Open a file here").clicked() {
                self.active_pane = pane;
                self.open_dialog(pane);
            }
        }
        if let Some(i) = activate {
            self.panes[pane].active = i;
            self.active_pane = pane;
        }
        if let Some(i) = close {
            self.close_tab(pane, i, false);
        }
        match menu_action {
            Some((i, 0)) => self.close_tab(pane, i, false),
            Some((i, 1)) => {
                let keep = self.panes[pane].tabs[i].doc;
                let mut j = 0;
                while j < self.panes[pane].tabs.len() {
                    if self.panes[pane].tabs[j].doc != keep
                        && !self
                            .docs
                            .get(&self.panes[pane].tabs[j].doc)
                            .is_some_and(|d| d.dirty)
                    {
                        self.close_tab(pane, j, false);
                    } else {
                        j += 1;
                    }
                }
            }
            Some((i, 2)) => self.move_tab_to_other_pane(pane, i),
            _ => {}
        }
        let _ = ctx;
    }

    pub fn find_bar(&mut self, ui: &mut Ui, ctx: &Context, body: Rect) {
        let doc = self.active_doc_id();
        let pal = theme::palette(ui.visuals().dark_mode);
        egui::Area::new(Id::unique("find-bar"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos2(body.max.x - 470.0, body.min.y + 10.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .corner_radius(CornerRadius::same(10))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(icon::SEARCH).size(16.0));
                            let id = Id::unique("find-input");
                            let r = ui.add(
                                TextEdit::singleline(&mut self.find.query)
                                    .id(id)
                                    .hint_text("Find in document")
                                    .desired_width(200.0),
                            );
                            if self.find.focus {
                                r.request_focus();
                                self.find.focus = false;
                            }
                            if r.changed() {
                                self.find.edited = Some(std::time::Instant::now());
                            }
                            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                let back = ui.input(|i| i.modifiers.shift);
                                self.find_step(!back);
                                r.request_focus();
                            }
                            let case = self.find.opts.match_case;
                            if ui
                                .selectable_label(case, "Aa")
                                .on_hover_text("Match case")
                                .clicked()
                            {
                                self.find.opts.match_case = !case;
                                self.find.edited = Some(std::time::Instant::now());
                            }
                            let word = self.find.opts.whole_word;
                            if ui
                                .selectable_label(word, RichText::new("ab").underline())
                                .on_hover_text("Whole words")
                                .clicked()
                            {
                                self.find.opts.whole_word = !word;
                                self.find.edited = Some(std::time::Instant::now());
                            }
                            if let Some(s) = doc
                                .and_then(|d| self.docs.get(&d))
                                .and_then(|d| d.search.as_ref())
                            {
                                let count = s.hits.len();
                                let pos = s.current.map(|c| c + 1).unwrap_or(0);
                                let t = if count == 0 && s.done {
                                    "No results".to_string()
                                } else {
                                    format!("{pos} / {count}")
                                };
                                ui.label(RichText::new(t).size(12.5).color(
                                    if count == 0 && s.done {
                                        pal.danger
                                    } else {
                                        ui.visuals().text_color()
                                    },
                                ));
                                if !s.done {
                                    crate::theme::spinner(ui, 14.0).on_hover_text(format!(
                                        "Searched {} of {} pages",
                                        s.searched, s.total
                                    ));
                                }
                            }
                            if ui
                                .button(icon::UP)
                                .on_hover_text("Previous (Shift+F3)")
                                .clicked()
                            {
                                self.find_step(false);
                            }
                            if ui.button(icon::DOWN).on_hover_text("Next (F3)").clicked() {
                                self.find_step(true);
                            }
                            if ui
                                .button(icon::CLOSE)
                                .on_hover_text("Close (Esc)")
                                .clicked()
                            {
                                self.close_find();
                            }
                        });
                    });
            });
    }

    pub fn welcome(&mut self, ui: &mut Ui, _ctx: &Context, pane: usize, rect: Rect) {
        let pal = theme::palette(ui.visuals().dark_mode);
        ui.painter()
            .rect_filled(rect, CornerRadius::ZERO, pal.canvas);
        let r = ui.interact(rect, Id::unique(("welcome", pane)), Sense::click());
        if r.clicked() {
            self.active_pane = pane;
        }
        let card_w = 460.0f32.min(rect.width() - 40.0);
        let card = Rect::from_center_size(
            rect.center(),
            vec2(card_w, (rect.height() - 40.0).min(520.0)),
        );
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(card)
                .layout(Layout::top_down(Align::Center)),
        );
        egui::Frame::new()
            .fill(ui.visuals().window_fill)
            .corner_radius(CornerRadius::same(14))
            .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
            .inner_margin(egui::Margin::same(24))
            .show(&mut child, |ui| {
                ui.set_width(card_w - 48.0);
                ui.vertical_centered(|ui| {
                    crate::theme::logo(ui, 96.0);
                    ui.label(RichText::new("BDF").size(26.0).strong());
                    ui.label(RichText::new("Fast, portable PDF reader").weak());
                    ui.add_space(14.0);
                    let b = Button::new(
                        RichText::new(format!("{}  Open a PDF", icon::OPEN))
                            .size(15.0)
                            .color(Color32::WHITE),
                    )
                    .fill(pal.accent)
                    .min_size(vec2(180.0, 36.0))
                    .corner_radius(CornerRadius::same(8));
                    if ui.add(b).clicked() {
                        self.active_pane = pane;
                        self.open_dialog(pane);
                    }
                    ui.label(RichText::new("or drop files anywhere").weak().size(12.5));
                    ui.add_space(16.0);
                });
                if !self.settings.recent.is_empty() {
                    ui.label(RichText::new("Recent").strong());
                    ui.add_space(4.0);
                    let recent = self.settings.recent.clone();
                    egui::ScrollArea::vertical()
                        .max_height(220.0)
                        .show(ui, |ui| {
                            for r in recent.iter().take(10) {
                                let name = r
                                    .path
                                    .file_name()
                                    .map(|n| n.to_string_lossy().into_owned())
                                    .unwrap_or_default();
                                let dir = r
                                    .path
                                    .parent()
                                    .map(|p| p.display().to_string())
                                    .unwrap_or_default();
                                let exists = r.path.exists();
                                let resp = ui
                                    .add_enabled(
                                        exists,
                                        Button::new(RichText::new(format!(
                                            "{}  {name}",
                                            icon::FILE
                                        )))
                                        .min_size(vec2(ui.available_width(), 26.0))
                                        .right_text(
                                            RichText::new(shorten(&dir, 34)).weak().size(11.5),
                                        ),
                                    )
                                    .on_hover_text(r.path.display().to_string());
                                if resp.clicked() {
                                    self.open_path(&r.path, pane);
                                }
                            }
                        });
                }
            });
        let _ = StrokeKind::Inside;
        let _ = SidebarTab::Thumbs;
    }
}

fn shorten(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    let tail: String = s.chars().skip(n - (max - 1)).collect();
    format!("…{tail}")
}
