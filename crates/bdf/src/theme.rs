//! Look and feel: colours, spacing, fonts and icons.

use bdf_core::settings::EyeStyle;
use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, Margin, Stroke, Visuals,
};

/// Phosphor icon codepoints used by the UI (MIT licensed font, see assets/).
pub mod icon {
    pub const OPEN: &str = "\u{E256}";
    pub const SAVE: &str = "\u{E248}";
    pub const PRINT: &str = "\u{E3DC}";
    pub const SIDEBAR: &str = "\u{EAB6}";
    pub const HAND: &str = "\u{E298}";
    pub const SELECT: &str = "\u{E7D8}";
    pub const HIGHLIGHT: &str = "\u{EC76}";
    pub const UNDERLINE: &str = "\u{E5C4}";
    pub const STRIKE: &str = "\u{E5C2}";
    pub const NOTE: &str = "\u{E348}";
    pub const PEN: &str = "\u{E3B4}";
    pub const SQUARE: &str = "\u{E45E}";
    pub const CIRCLE: &str = "\u{E18A}";
    pub const SEARCH: &str = "\u{E30C}";
    pub const ZOOM_IN: &str = "\u{E310}";
    pub const ZOOM_OUT: &str = "\u{E30E}";
    pub const PREV: &str = "\u{E138}";
    pub const NEXT: &str = "\u{E13A}";
    pub const UP: &str = "\u{E13C}";
    pub const DOWN: &str = "\u{E136}";
    pub const FIRST: &str = "\u{E128}";
    pub const LAST: &str = "\u{E12A}";
    pub const ROTATE_CW: &str = "\u{E036}";
    pub const ROTATE_CCW: &str = "\u{E038}";
    pub const SPLIT_H: &str = "\u{E870}";
    pub const BOOKMARK: &str = "\u{E0EA}";
    pub const BOOKMARKS: &str = "\u{E5F0}";
    pub const OUTLINE: &str = "\u{E2F2}";
    pub const THUMBS: &str = "\u{E836}";
    pub const CLOSE: &str = "\u{E4F6}";
    pub const MOON: &str = "\u{E330}";
    pub const SUN: &str = "\u{E472}";
    pub const PDF: &str = "\u{E702}";
    pub const RECENT: &str = "\u{E1A0}";
    pub const BOOK: &str = "\u{E0E6}";
    pub const COPY: &str = "\u{E1CA}";
    pub const TRASH: &str = "\u{E4A6}";
    pub const UNDO: &str = "\u{E08A}";
    pub const REDO: &str = "\u{E08C}";
    pub const INFO: &str = "\u{E2CE}";
    pub const KEYBOARD: &str = "\u{E2D8}";
    pub const GEAR: &str = "\u{E270}";
    pub const BACK: &str = "\u{E058}";
    pub const FORWARD: &str = "\u{E06C}";
    pub const COMMENT: &str = "\u{E17A}";
    pub const LINK: &str = "\u{E2E2}";
    pub const CHECK: &str = "\u{E182}";
    pub const WARNING: &str = "\u{E4E0}";
    pub const FIT_WIDTH: &str = "\u{E0A0}";
    pub const FIT_PAGE: &str = "\u{E1D0}";
    pub const LOCK: &str = "\u{E2FA}";
    pub const PLUS: &str = "\u{E3D4}";
    pub const SCROLL: &str = "\u{EB7A}";
    pub const COLUMNS: &str = "\u{E546}";
    pub const FILE: &str = "\u{E230}";
}

/// Colours that are not part of egui's `Visuals`.
#[derive(Clone, Copy)]
pub struct Palette {
    pub accent: Color32,
    pub accent_soft: Color32,
    pub canvas: Color32,
    pub page_shadow: Color32,
    pub selection: Color32,
    pub search_hit: Color32,
    pub search_current: Color32,
    pub muted: Color32,
    pub danger: Color32,
    pub success: Color32,
}

/// The Sharingan palette: crimson on near-black (dark), crimson on warm
/// paper (light).
pub fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            accent: Color32::from_rgb(227, 34, 43),
            accent_soft: Color32::from_rgba_unmultiplied(227, 34, 43, 42),
            canvas: Color32::from_rgb(12, 11, 12),
            // A faint red glow around pages instead of a grey shadow.
            page_shadow: Color32::from_rgba_unmultiplied(150, 0, 8, 70),
            selection: Color32::from_rgba_unmultiplied(227, 34, 43, 80),
            search_hit: Color32::from_rgba_unmultiplied(255, 80, 70, 85),
            search_current: Color32::from_rgba_unmultiplied(255, 35, 35, 150),
            muted: Color32::from_gray(140),
            danger: Color32::from_rgb(255, 77, 77),
            success: Color32::from_rgb(74, 222, 128),
        }
    } else {
        Palette {
            accent: Color32::from_rgb(181, 18, 27),
            accent_soft: Color32::from_rgba_unmultiplied(181, 18, 27, 30),
            canvas: Color32::from_rgb(232, 225, 225),
            page_shadow: Color32::from_rgba_unmultiplied(90, 0, 0, 55),
            selection: Color32::from_rgba_unmultiplied(200, 20, 30, 60),
            search_hit: Color32::from_rgba_unmultiplied(255, 80, 70, 90),
            search_current: Color32::from_rgba_unmultiplied(220, 20, 30, 140),
            muted: Color32::from_gray(110),
            danger: Color32::from_rgb(200, 20, 30),
            success: Color32::from_rgb(22, 163, 74),
        }
    }
}

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "phosphor".into(),
        FontData::from_static(include_bytes!("../assets/Phosphor.ttf")).into(),
    );
    // Icons resolve after the text font, so ordinary text is unaffected.
    if let Some(list) = fonts.families.get_mut(&FontFamily::Proportional) {
        let at = list.len().min(1);
        list.insert(at, "phosphor".into());
    }
    if let Some(list) = fonts.families.get_mut(&FontFamily::Monospace) {
        list.push("phosphor".into());
    }
    ctx.set_fonts(fonts);
}

/// Installs BDF's light and dark looks; egui switches between them with the theme.
pub fn apply_both(ctx: &egui::Context) {
    ctx.set_visuals_of(egui::Theme::Light, visuals(false));
    ctx.set_visuals_of(egui::Theme::Dark, visuals(true));
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = egui::vec2(6.0, 6.0);
        s.spacing.button_padding = egui::vec2(7.0, 4.0);
        s.spacing.menu_margin = Margin::same(6);
        s.spacing.interact_size.y = 26.0;
        s.interaction.tooltip_delay = 0.35;
        use egui::{FontId, TextStyle};
        s.text_styles
            .insert(TextStyle::Body, FontId::proportional(14.0));
        s.text_styles
            .insert(TextStyle::Button, FontId::proportional(14.0));
        s.text_styles
            .insert(TextStyle::Small, FontId::proportional(11.5));
        s.text_styles
            .insert(TextStyle::Heading, FontId::proportional(20.0));
    });
}

fn visuals(dark: bool) -> Visuals {
    let p = palette(dark);
    let mut v = if dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    let r6 = CornerRadius::same(6);
    if dark {
        v.panel_fill = Color32::from_rgb(21, 19, 20);
        v.window_fill = Color32::from_rgb(27, 24, 25);
        v.extreme_bg_color = Color32::from_rgb(10, 9, 10);
        v.faint_bg_color = Color32::from_rgb(31, 27, 28);
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(52, 36, 38));
        v.window_stroke = Stroke::new(1.0, Color32::from_rgb(90, 20, 26));
    } else {
        v.panel_fill = Color32::from_rgb(251, 248, 248);
        v.window_fill = Color32::from_rgb(255, 253, 253);
        v.extreme_bg_color = Color32::WHITE;
        v.faint_bg_color = Color32::from_rgb(245, 238, 238);
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(228, 214, 214));
    }
    v.selection.bg_fill = p.accent;
    v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    v.hyperlink_color = p.accent;
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = r6;
    }
    // Flat buttons: no fill until hovered.
    v.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
    v.widgets.inactive.bg_stroke = Stroke::NONE;
    // Hover and press tints lean red.
    v.widgets.hovered.weak_bg_fill = if dark {
        Color32::from_rgb(52, 26, 29)
    } else {
        Color32::from_rgb(243, 224, 225)
    };
    v.widgets.hovered.bg_stroke = Stroke::NONE;
    v.widgets.active.weak_bg_fill = if dark {
        Color32::from_rgb(76, 28, 33)
    } else {
        Color32::from_rgb(234, 205, 207)
    };
    v.widgets.hovered.bg_fill = v.widgets.hovered.weak_bg_fill;
    v.widgets.active.bg_fill = v.widgets.active.weak_bg_fill;
    // Sliders, checkboxes and text cursors in crimson.
    v.text_cursor.stroke = Stroke::new(2.0, p.accent);
    v.widgets.open.weak_bg_fill = v.widgets.hovered.weak_bg_fill;
    v
}

/// The window icon.
pub fn window_icon(eye: EyeStyle) -> egui::IconData {
    let n = 64;
    egui::IconData {
        rgba: crate::icon_pixels::eye_rgba(to_eye(eye), n),
        width: n as u32,
        height: n as u32,
    }
}

fn image(n: usize, rgba: Vec<u8>) -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied([n, n], &rgba)
}

fn art_id(name: &str) -> egui::Id {
    egui::Id::unique(("bdf-art", name))
}

fn to_eye(e: EyeStyle) -> crate::icon_pixels::Eye {
    match e {
        EyeStyle::Kakashi => crate::icon_pixels::Eye::Kakashi,
        EyeStyle::Itachi => crate::icon_pixels::Eye::Itachi,
    }
}

/// Uploads the chosen eye as the logo; call again when the choice changes.
pub fn install_art(ctx: &egui::Context, eye: EyeStyle) {
    let logo = ctx.load_texture(
        "bdf-logo",
        image(256, crate::icon_pixels::eye_rgba(to_eye(eye), 256)),
        egui::TextureOptions::LINEAR,
    );
    ctx.data_mut(|d| d.insert_temp(art_id("logo"), logo));
}

/// The Mangekyou logo at `size` points.
pub fn logo(ui: &mut egui::Ui, size: f32) -> egui::Response {
    match ui
        .ctx()
        .data(|d| d.get_temp::<egui::TextureHandle>(art_id("logo")))
    {
        Some(t) => ui.add(egui::Image::new(&t).fit_to_exact_size(egui::vec2(size, size))),
        None => ui.label(egui::RichText::new(icon::PDF).size(size)),
    }
}

/// The Mangekyou spinning, shown while work is in progress.
pub fn spinner(ui: &mut egui::Ui, size: f32) -> egui::Response {
    match ui
        .ctx()
        .data(|d| d.get_temp::<egui::TextureHandle>(art_id("logo")))
    {
        Some(t) => {
            let angle = -(ui.input(|i| i.time) as f32) * 4.0;
            ui.ctx().request_repaint();
            ui.add(
                egui::Image::new(&t)
                    .fit_to_exact_size(egui::vec2(size, size))
                    .rotate(angle, egui::vec2(0.5, 0.5)),
            )
        }
        None => ui.add(egui::Spinner::new().size(size)),
    }
}
