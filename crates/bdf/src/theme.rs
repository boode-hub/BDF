//! Look and feel: colours, spacing, fonts and icons.

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
    pub const SPLIT_V: &str = "\u{E874}";
    pub const BOOKMARK: &str = "\u{E0EA}";
    pub const BOOKMARKS: &str = "\u{E5F0}";
    pub const OUTLINE: &str = "\u{E2F2}";
    pub const THUMBS: &str = "\u{E836}";
    pub const CLOSE: &str = "\u{E4F6}";
    pub const MOON: &str = "\u{E330}";
    pub const SUN: &str = "\u{E472}";
    pub const PDF: &str = "\u{E702}";
    pub const RECENT: &str = "\u{E1A0}";
    pub const FULLSCREEN: &str = "\u{E0A2}";
    pub const EXIT_FULLSCREEN: &str = "\u{E09A}";
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
    pub const MORE: &str = "\u{E208}";
    pub const CHECK: &str = "\u{E182}";
    pub const WARNING: &str = "\u{E4E0}";
    pub const FIT_WIDTH: &str = "\u{E0A0}";
    pub const FIT_PAGE: &str = "\u{E1D0}";
    pub const LOCK: &str = "\u{E2FA}";
    pub const PLUS: &str = "\u{E3D4}";
    pub const MINUS: &str = "\u{E32A}";
    pub const SCROLL: &str = "\u{EB7A}";
    pub const EYE: &str = "\u{E220}";
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

pub fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            accent: Color32::from_rgb(96, 140, 255),
            accent_soft: Color32::from_rgba_unmultiplied(96, 140, 255, 40),
            canvas: Color32::from_rgb(30, 31, 34),
            page_shadow: Color32::from_black_alpha(140),
            selection: Color32::from_rgba_unmultiplied(70, 130, 255, 90),
            search_hit: Color32::from_rgba_unmultiplied(255, 200, 0, 90),
            search_current: Color32::from_rgba_unmultiplied(255, 120, 0, 140),
            muted: Color32::from_gray(150),
            danger: Color32::from_rgb(248, 113, 113),
            success: Color32::from_rgb(74, 222, 128),
        }
    } else {
        Palette {
            accent: Color32::from_rgb(37, 99, 235),
            accent_soft: Color32::from_rgba_unmultiplied(37, 99, 235, 28),
            canvas: Color32::from_rgb(229, 231, 235),
            page_shadow: Color32::from_black_alpha(60),
            selection: Color32::from_rgba_unmultiplied(37, 99, 235, 70),
            search_hit: Color32::from_rgba_unmultiplied(255, 200, 0, 110),
            search_current: Color32::from_rgba_unmultiplied(255, 120, 0, 150),
            muted: Color32::from_gray(110),
            danger: Color32::from_rgb(220, 38, 38),
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
        s.text_styles.insert(TextStyle::Body, FontId::proportional(14.0));
        s.text_styles.insert(TextStyle::Button, FontId::proportional(14.0));
        s.text_styles.insert(TextStyle::Small, FontId::proportional(11.5));
        s.text_styles.insert(TextStyle::Heading, FontId::proportional(20.0));
    });
}

fn visuals(dark: bool) -> Visuals {
    let p = palette(dark);
    let mut v = if dark { Visuals::dark() } else { Visuals::light() };
    let r6 = CornerRadius::same(6);
    if dark {
        v.panel_fill = Color32::from_rgb(40, 42, 46);
        v.window_fill = Color32::from_rgb(44, 46, 51);
        v.extreme_bg_color = Color32::from_rgb(28, 29, 32);
        v.faint_bg_color = Color32::from_rgb(48, 50, 55);
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_gray(60));
    } else {
        v.panel_fill = Color32::from_rgb(250, 250, 251);
        v.window_fill = Color32::WHITE;
        v.extreme_bg_color = Color32::WHITE;
        v.faint_bg_color = Color32::from_rgb(243, 244, 246);
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_gray(222));
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
    v.widgets.hovered.weak_bg_fill = if dark { Color32::from_gray(62) } else { Color32::from_gray(228) };
    v.widgets.hovered.bg_stroke = Stroke::NONE;
    v.widgets.active.weak_bg_fill = if dark { Color32::from_gray(72) } else { Color32::from_gray(214) };
    v.widgets.open.weak_bg_fill = v.widgets.hovered.weak_bg_fill;
    v
}

/// A 64x64 window icon: a page with a folded corner on the accent colour.
pub fn window_icon() -> egui::IconData {
    let n = 64usize;
    let mut rgba = vec![0u8; n * n * 4];
    let accent = [37u8, 99, 235];
    for y in 0..n {
        for x in 0..n {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            // Rounded square background.
            let r = 14.0;
            let cx = fx.clamp(r, n as f32 - r);
            let cy = fy.clamp(r, n as f32 - r);
            let d = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
            let alpha = (r + 0.5 - d).clamp(0.0, 1.0);
            let mut c = accent;
            // White page with a folded top-right corner.
            let in_page = (18.0..46.0).contains(&fx) && (12.0..52.0).contains(&fy);
            let fold = fx - 36.0 > fy - 12.0 && fx > 36.0 && fy < 22.0;
            if in_page && !fold {
                c = [255, 255, 255];
                // Three text lines.
                for ly in [28.0, 35.0, 42.0] {
                    if (ly..ly + 2.5).contains(&fy) && (23.0..41.0).contains(&fx) {
                        c = [148, 170, 220];
                    }
                }
            }
            let i = (y * n + x) * 4;
            rgba[i..i + 3].copy_from_slice(&c);
            rgba[i + 3] = (alpha * 255.0) as u8;
        }
    }
    egui::IconData { rgba, width: n as u32, height: n as u32 }
}
