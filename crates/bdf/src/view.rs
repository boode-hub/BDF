//! One document view: scrolling, zooming, drawing pages and overlays, and
//! the pointer tools. A view never talks to dialogs directly; it reports
//! what the user asked for through [`ViewAction`]s.

use crate::doc::DocState;
use crate::theme::Palette;
use crate::tiles::TileCache;
use bdf_core::engine::{
    AnnotInfo, AnnotKind, LinkTarget, NewAnnot, PageData, RegionReq, TILE, TileKey, TileKind, tile_grid,
};
use bdf_core::geom::{PageSpace, Rotation, page_px, quantize_scale};
use bdf_core::layout::{Layout, LayoutParams, PageMode};
use bdf_core::worker::{Request, WorkerHandle};
use eframe::egui::{
    self, Color32, CornerRadius, CursorIcon, Id, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, Vec2, pos2, vec2,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 100% zoom shows one PDF point as 96/72 logical pixels, like other readers.
pub const PT_TO_PX: f32 = 96.0 / 72.0;
pub const MIN_ZOOM: f32 = 0.05;
pub const MAX_ZOOM: f32 = 64.0;
pub const ZOOM_STEPS: [f32; 17] = [
    0.1, 0.25, 0.33, 0.5, 0.67, 0.75, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 3.0, 4.0, 6.0, 8.0,
];
/// Re-render at a new zoom only once zooming pauses for this long.
const ZOOM_SETTLE: Duration = Duration::from_millis(140);
const SCROLLBAR: f32 = 10.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZoomMode {
    FitWidth,
    FitPage,
    Custom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tool {
    Select,
    Hand,
    Highlight,
    Underline,
    StrikeOut,
    Note,
    Ink,
    Rect,
    Ellipse,
    Magnifier,
}

impl Tool {
    pub fn markup_kind(self) -> Option<AnnotKind> {
        match self {
            Tool::Highlight => Some(AnnotKind::Highlight),
            Tool::Underline => Some(AnnotKind::Underline),
            Tool::StrikeOut => Some(AnnotKind::StrikeOut),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Tool::Select => "Select text",
            Tool::Hand => "Hand",
            Tool::Highlight => "Highlight",
            Tool::Underline => "Underline",
            Tool::StrikeOut => "Strikeout",
            Tool::Note => "Note",
            Tool::Ink => "Pen",
            Tool::Rect => "Rectangle",
            Tool::Ellipse => "Ellipse",
            Tool::Magnifier => "Magnifier",
        }
    }
}

/// A text selection from `(page, char)` to `(page, char)`, end exclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextSel {
    pub start: (usize, usize),
    pub end: (usize, usize),
}

#[derive(Clone, Copy, Debug)]
pub struct Jump {
    pub page: usize,
    /// Target in user space; the view scrolls it near the top.
    pub y_user: Option<f32>,
    pub rect_user: Option<[f32; 4]>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Drag {
    None,
    Pan,
    Select { page: usize, at: (f32, f32) },
    Shape { page: usize, a: Pos2 },
    Ink { page: usize },
    VScroll { grab: f32 },
    HScroll { grab: f32 },
}

pub enum ViewAction {
    AddAnnot { page: u32, annot: NewAnnot },
    RemoveAnnot { page: u32, info: AnnotInfo },
    EditAnnot { page: u32, info: AnnotInfo },
    NewNote { page: u32, at: (f32, f32) },
    OpenUri(String),
    Copy(String),
    AddBookmark { page: u32 },
    Toast(String),
}

pub struct ViewState {
    pub mode: PageMode,
    pub cover: bool,
    pub rotation: Rotation,
    pub zoom: f32,
    pub zoom_mode: ZoomMode,
    render_zoom: f32,
    zoom_changed: Option<Instant>,
    pub scroll: Vec2,
    layout: Arc<Layout>,
    layout_key: Option<(PageMode, bool, Rotation, usize)>,
    pub current_page: usize,
    pub pending: Option<Jump>,
    back: Vec<usize>,
    forward: Vec<usize>,
    pub selection: Option<TextSel>,
    drag: Drag,
    pub selected_annot: Option<(u32, AnnotInfo)>,
    pub viewport: Vec2,
    initialized: bool,
    ink: Vec<(f32, f32)>,
    last_good_scale: HashMap<usize, u32>,
    overscroll: f32,
    indicator_until: f64,
    context_at: Option<(usize, (f32, f32))>,
    last_region: Option<RegionReq>,
    /// Set by the app for synchronised scrolling: (page, fraction down the page).
    pub sync_to: Option<(usize, f32)>,
}

impl ViewState {
    pub fn new(mode: PageMode, cover: bool, zoom_mode: ZoomMode) -> Self {
        Self {
            mode,
            cover,
            rotation: Rotation::R0,
            zoom: 1.0,
            zoom_mode,
            render_zoom: 1.0,
            zoom_changed: None,
            scroll: Vec2::ZERO,
            layout: Arc::new(Layout::default()),
            layout_key: None,
            current_page: 0,
            pending: None,
            back: Vec::new(),
            forward: Vec::new(),
            selection: None,
            drag: Drag::None,
            selected_annot: None,
            viewport: vec2(800.0, 600.0),
            initialized: false,
            ink: Vec::new(),
            last_good_scale: HashMap::new(),
            overscroll: 0.0,
            indicator_until: 0.0,
            context_at: None,
            last_region: None,
            sync_to: None,
        }
    }

    fn s(&self) -> f32 {
        self.zoom * PT_TO_PX
    }

    fn ensure_layout(&mut self, doc: &DocState) {
        let key = (self.mode, self.cover, self.rotation, doc.page_count());
        if self.layout_key != Some(key) {
            let params = LayoutParams { mode: self.mode, cover: self.cover, rotation: self.rotation, gap: 12.0 };
            self.layout = Arc::new(Layout::new(&doc.info.page_sizes, &params));
            self.layout_key = Some(key);
            self.last_good_scale.clear();
        }
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// Vertical scroll range in px (lo, hi) for the current mode.
    fn y_range(&self) -> (f32, f32) {
        let s = self.s();
        if self.mode.is_continuous() {
            (0.0, self.layout.height * s)
        } else {
            let (a, b) = self.layout.row_band(self.current_page);
            (a * s, b * s)
        }
    }

    fn clamp_scroll(&mut self) {
        let s = self.s();
        let (vw, vh) = (self.viewport.x, self.viewport.y);
        let cw = self.layout.width * s;
        self.scroll.x = if cw <= vw { (cw - vw) * 0.5 } else { self.scroll.x.clamp(0.0, cw - vw) };
        let (lo, hi) = self.y_range();
        let span = hi - lo;
        self.scroll.y = if span <= vh { lo - (vh - span) * 0.5 } else { self.scroll.y.clamp(lo, hi - vh) };
        if !self.scroll.x.is_finite() {
            self.scroll.x = 0.0;
        }
        if !self.scroll.y.is_finite() {
            self.scroll.y = 0.0;
        }
    }

    fn fit_zoom(&self, mode: ZoomMode) -> f32 {
        let (vw, vh) = (self.viewport.x / PT_TO_PX, self.viewport.y / PT_TO_PX);
        match mode {
            ZoomMode::FitWidth => self.layout.fit_row_width_zoom(self.current_page, vw - SCROLLBAR / PT_TO_PX),
            ZoomMode::FitPage => self.layout.fit_page_zoom(self.current_page, vw, vh),
            ZoomMode::Custom => self.zoom,
        }
        .clamp(MIN_ZOOM, MAX_ZOOM)
    }

    /// Changes zoom keeping the layout point under `anchor` (view-relative px) fixed.
    pub fn set_zoom(&mut self, z: f32, anchor: Option<Vec2>, mode: ZoomMode) {
        let z = z.clamp(MIN_ZOOM, MAX_ZOOM);
        self.zoom_mode = mode;
        if (z - self.zoom).abs() < 1e-6 {
            return;
        }
        let anchor = anchor.unwrap_or(vec2(self.viewport.x * 0.5, 0.0));
        let old = self.s();
        let pt = (self.scroll + anchor) / old;
        self.zoom = z;
        self.scroll = pt * self.s() - anchor;
        self.zoom_changed = Some(Instant::now());
        self.clamp_scroll();
    }

    pub fn zoom_step(&mut self, dir: i32, anchor: Option<Vec2>) {
        let z = self.zoom;
        let next = if dir > 0 {
            ZOOM_STEPS.iter().copied().find(|&s| s > z * 1.001).unwrap_or((z * 1.25).min(MAX_ZOOM))
        } else {
            ZOOM_STEPS.iter().rev().copied().find(|&s| s < z * 0.999).unwrap_or((z / 1.25).max(MIN_ZOOM))
        };
        self.set_zoom(next, anchor, ZoomMode::Custom);
    }

    pub fn fit(&mut self, mode: ZoomMode) {
        let page = self.current_page;
        let z = self.fit_zoom(mode);
        self.set_zoom(z, None, mode);
        self.zoom_mode = mode;
        self.goto(page, false);
    }

    pub fn set_mode(&mut self, mode: PageMode) {
        let page = self.current_page;
        self.mode = mode;
        self.layout_key = None;
        self.pending = Some(Jump { page, y_user: None, rect_user: None });
    }

    pub fn set_cover(&mut self, cover: bool) {
        let page = self.current_page;
        self.cover = cover;
        self.layout_key = None;
        self.pending = Some(Jump { page, y_user: None, rect_user: None });
    }

    pub fn rotate(&mut self, cw: bool) {
        let page = self.current_page;
        self.rotation = if cw { self.rotation.cw() } else { self.rotation.ccw() };
        self.layout_key = None;
        self.pending = Some(Jump { page, y_user: None, rect_user: None });
    }

    /// Goes to a page; `remember` records it for Back.
    pub fn goto(&mut self, page: usize, remember: bool) {
        self.jump(Jump { page, y_user: None, rect_user: None }, remember);
    }

    pub fn jump(&mut self, j: Jump, remember: bool) {
        if remember && j.page != self.current_page {
            self.back.push(self.current_page);
            if self.back.len() > 100 {
                self.back.remove(0);
            }
            self.forward.clear();
        }
        self.pending = Some(j);
    }

    pub fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    pub fn go_back(&mut self) {
        if let Some(p) = self.back.pop() {
            self.forward.push(self.current_page);
            self.goto(p, false);
        }
    }

    pub fn go_forward(&mut self) {
        if let Some(p) = self.forward.pop() {
            self.back.push(self.current_page);
            self.goto(p, false);
        }
    }

    pub fn scroll_by(&mut self, d: Vec2) {
        let (lo, hi) = self.y_range();
        let max_y = (hi - self.viewport.y).max(lo);
        if !self.mode.is_continuous() && self.layout.page_count() > 0 {
            // Turn the page when pushing past the ends of a spread.
            let at_end = self.scroll.y >= max_y - 0.5;
            let at_start = self.scroll.y <= lo + 0.5;
            if (d.y > 0.0 && at_end) || (d.y < 0.0 && at_start) {
                self.overscroll += d.y;
                if self.overscroll.abs() > 120.0 {
                    let dir = self.overscroll.signum() as i32;
                    self.overscroll = 0.0;
                    self.turn(dir);
                }
                self.scroll.x += d.x;
                self.clamp_scroll();
                return;
            }
            self.overscroll = 0.0;
        }
        self.scroll += d;
        self.clamp_scroll();
        self.indicator_until = 0.0;
    }

    /// Next/previous page or spread.
    pub fn turn(&mut self, dir: i32) {
        let n = self.layout.page_count();
        if n == 0 {
            return;
        }
        let target = self.layout.step_row(self.current_page.min(n - 1), dir);
        if self.mode.is_continuous() {
            self.goto(target, false);
        } else {
            self.current_page = target;
            let (lo, hi) = self.y_range();
            self.scroll.y = if dir < 0 { hi } else { lo };
            self.clamp_scroll();
        }
    }

    pub fn page_down(&mut self, dir: i32) {
        let step = (self.viewport.y - 40.0).max(40.0) * dir as f32;
        if self.mode.is_continuous() {
            self.scroll_by(vec2(0.0, step));
        } else {
            let (lo, hi) = self.y_range();
            let at_edge = if dir > 0 { self.scroll.y >= hi - self.viewport.y - 0.5 } else { self.scroll.y <= lo + 0.5 };
            if at_edge {
                self.turn(dir);
            } else {
                self.scroll.y += step;
                self.clamp_scroll();
            }
        }
    }

    pub fn first_page(&mut self) {
        self.goto(0, true);
    }

    pub fn last_page(&mut self, n: usize) {
        if n > 0 {
            self.goto(n - 1, true);
        }
    }

    /// Position as (page, fraction down that page), for synchronised scrolling.
    pub fn anchor(&self) -> Option<(usize, f32)> {
        let s = self.s();
        let y = (self.scroll.y + self.viewport.y * 0.3) / s;
        let p = self.current_page.min(self.layout.page_count().checked_sub(1)?);
        let r = self.layout.pages.get(p)?;
        Some((p, ((y - r.y0) / r.height().max(1.0)).clamp(0.0, 1.0)))
    }

    fn apply_sync(&mut self) {
        if let Some((page, frac)) = self.sync_to.take() {
            let Some(r) = self.layout.pages.get(page.min(self.layout.page_count().saturating_sub(1))).copied() else { return };
            let s = self.s();
            if !self.mode.is_continuous() {
                self.current_page = page;
            }
            self.scroll.y = (r.y0 + frac * r.height()) * s - self.viewport.y * 0.3;
            self.clamp_scroll();
            self.current_page = if self.mode.is_continuous() { page } else { self.current_page };
        }
    }

    fn apply_pending(&mut self, doc: &DocState) {
        let Some(j) = self.pending.take() else { return };
        let n = self.layout.page_count();
        if n == 0 {
            return;
        }
        let page = j.page.min(n - 1);
        self.current_page = page;
        if self.zoom_mode != ZoomMode::Custom {
            self.zoom = self.fit_zoom(self.zoom_mode);
            self.render_zoom = self.zoom;
        }
        let s = self.s();
        let r = self.layout.pages[page];
        let mut y = r.y0 - self.layout.gap * 0.5;
        // Scroll a target point near the top, when we know the page geometry.
        let target = j.rect_user.map(|r| (r[0], r[3])).or(j.y_user.map(|y| (0.0, y)));
        if let (Some((ux, uy)), Some(pd)) = (target, doc.peek_page_data(page as u32)) {
            let space = PageSpace::new(pd.bbox, pd.rotation.then(self.rotation));
            let (_, dy) = space.to_display(ux, uy);
            let k = r.height() / space.size().1.max(1.0);
            y = r.y0 + dy * k - 40.0 / s;
            if let Some(ru) = j.rect_user {
                // Horizontally, bring the target into view if zoomed in.
                let d = space.rect_to_display(ru);
                let x = (r.x0 + d.x0 * k) * s;
                if x < self.scroll.x || x > self.scroll.x + self.viewport.x - 80.0 {
                    self.scroll.x = x - 80.0;
                }
            }
        }
        self.scroll.y = y * s;
        self.clamp_scroll();
    }
}

/// Screen geometry of one page.
struct PageGeom {
    screen: Rect,
    space: Option<PageSpace>,
    /// Screen px per display point.
    k: f32,
}

impl PageGeom {
    fn to_screen(&self, x: f32, y: f32) -> Option<Pos2> {
        let sp = self.space?;
        let (dx, dy) = sp.to_display(x, y);
        Some(self.screen.min + vec2(dx, dy) * self.k)
    }

    fn rect_to_screen(&self, r: [f32; 4]) -> Option<Rect> {
        let sp = self.space?;
        let d = sp.rect_to_display(r);
        Some(Rect::from_min_max(
            self.screen.min + vec2(d.x0, d.y0) * self.k,
            self.screen.min + vec2(d.x1, d.y1) * self.k,
        ))
    }

    fn to_user(&self, p: Pos2) -> Option<(f32, f32)> {
        let sp = self.space?;
        let d = (p - self.screen.min) / self.k;
        Some(sp.to_user(d.x, d.y))
    }
}

pub struct ViewCtx<'a> {
    pub doc: &'a mut DocState,
    pub view: &'a mut ViewState,
    pub tiles: &'a mut TileCache,
    pub worker: &'a WorkerHandle,
    pub tool: Tool,
    pub pal: Palette,
    pub dark_pages: bool,
    pub highlight_color: [u8; 3],
    pub ink_color: [u8; 3],
    pub ink_width: f32,
    pub magnifier_zoom: f32,
    pub magnifier: Option<&'a (RegionReq, egui::TextureHandle)>,
    pub active: bool,
    pub id: Id,
    pub actions: &'a mut Vec<ViewAction>,
}

pub struct ViewOutput {
    pub clicked: bool,
    pub hovered: bool,
    pub scrolled: bool,
}

pub fn show(ui: &mut Ui, rect: Rect, cx: ViewCtx<'_>) -> ViewOutput {
    let ViewCtx { doc, view, tiles, worker, tool, pal, dark_pages, highlight_color, ink_color, ink_width, magnifier_zoom, magnifier, active, id, actions } = cx;
    let ctx = ui.ctx().clone();
    let ppp = ctx.pixels_per_point();
    let now = ctx.input(|i| i.time);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::ZERO, pal.canvas);

    view.ensure_layout(doc);
    let resized = (view.viewport - rect.size()).length() > 0.5;
    view.viewport = rect.size();
    if !view.initialized {
        view.initialized = true;
        if view.pending.is_none() {
            view.pending = Some(Jump { page: view.current_page, y_user: None, rect_user: None });
        }
    }
    if resized && view.zoom_mode != ZoomMode::Custom && view.pending.is_none() {
        let page = view.current_page;
        let z = view.fit_zoom(view.zoom_mode);
        view.set_zoom(z, None, view.zoom_mode);
        if (view.render_zoom - view.zoom).abs() > 1e-4 && view.zoom_changed.is_none() {
            view.zoom_changed = Some(Instant::now());
        }
        if !view.mode.is_continuous() {
            view.current_page = page;
        }
    }
    if std::env::var_os("BDF_DEBUG_VIEW").is_some() {
        log::info!("view rect {:?} resized {resized} zoom {} mode {:?} pending {:?}", rect.size(), view.zoom, view.zoom_mode, view.pending.is_some());
    }
    view.apply_pending(doc);
    view.apply_sync();
    view.clamp_scroll();

    let response = ui.interact(rect, id, Sense::click_and_drag());
    let mut out = ViewOutput { clicked: response.clicked() || response.drag_started(), hovered: response.hovered(), scrolled: false };
    let pointer = response.hover_pos();

    // ---- wheel and pinch ----
    if response.hovered() {
        let (scroll, zoom_delta, mods) = ctx.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.modifiers));
        if (zoom_delta - 1.0).abs() > 1e-4 {
            let anchor = pointer.map(|p| p - rect.min);
            view.set_zoom(view.zoom * zoom_delta, anchor, ZoomMode::Custom);
            out.scrolled = true;
        } else if scroll != Vec2::ZERO {
            let d = if mods.shift && scroll.x == 0.0 { vec2(-scroll.y, 0.0) } else { -scroll };
            view.scroll_by(d);
            out.scrolled = true;
            view.indicator_until = now + 1.2;
        }
    }

    // ---- settle the render zoom: keep the old images while zooming ----
    match view.zoom_changed {
        Some(t) if t.elapsed() < ZOOM_SETTLE => ctx.request_repaint_after(ZOOM_SETTLE - t.elapsed()),
        _ => {
            view.render_zoom = view.zoom;
            view.zoom_changed = None;
        }
    }

    let s = view.s();
    let origin = rect.min - view.scroll;
    let layout = view.layout.clone();
    let n = layout.page_count();
    if n == 0 {
        return out;
    }

    // Current page.
    let (vy0, vy1) = (view.scroll.y / s, (view.scroll.y + rect.height()) / s);
    if view.mode.is_continuous() {
        view.current_page = layout.current_page(vy0, vy1).min(n - 1);
    }

    // ---- page geometry for visible pages ----
    let margin = rect.height() / s; // prefetch one screen above and below
    let visible: Vec<usize> = layout.pages_in(vy0, vy1).collect();
    let prefetch: Vec<usize> = layout.pages_in(vy0 - margin, vy1 + margin).filter(|p| !visible.contains(p)).collect();
    let mut geoms: HashMap<usize, PageGeom> = HashMap::new();
    let mut datas: HashMap<usize, Arc<PageData>> = HashMap::new();
    for &p in &visible {
        let r = layout.pages[p];
        let mut min = origin + vec2(r.x0, r.y0) * s;
        // Snap to device pixels so tiles meet without seams.
        min = pos2((min.x * ppp).round() / ppp, (min.y * ppp).round() / ppp);
        let size = vec2(r.width(), r.height()) * s;
        let screen = Rect::from_min_size(min, size);
        let data = doc.page_data(p as u32, worker);
        let space = data.as_ref().map(|d| PageSpace::new(d.bbox, d.rotation.then(view.rotation)));
        let disp_w = space.map(|sp| sp.size().0).unwrap_or(r.width());
        geoms.insert(p, PageGeom { screen, space, k: screen.width() / disp_w.max(1.0) });
        if let Some(d) = data {
            datas.insert(p, d);
        }
    }

    // ---- draw pages ----
    let render_s = view.render_zoom * PT_TO_PX;
    let q = quantize_scale(render_s * ppp);
    let page_bg = if dark_pages { Color32::from_gray(18) } else { Color32::WHITE };
    let center_y = rect.center().y;
    for &p in &visible {
        let g = &geoms[&p];
        let screen = g.screen;
        // Shadow and paper.
        painter.add(egui::epaint::Shadow { offset: [0, 2], blur: 10, spread: 0, color: pal.page_shadow }.as_shape(screen, CornerRadius::same(2)));
        painter.rect_filled(screen, CornerRadius::same(1), page_bg);
        let base = TileKey {
            doc: doc.id,
            page: p as u32,
            rotation: view.rotation,
            invert: dark_pages,
            generation: doc.generation(p as u32),
            kind: TileKind::Thumb,
        };
        // Priority: thumbnails first (instant preview), then tiles nearest the middle.
        let dist = ((screen.center().y - center_y).abs() / 100.0) as u32;
        let thumb = tiles.want(base, dist.min(9));
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        if let Some(t) = &thumb {
            painter.image(t.id(), screen, uv, Color32::WHITE);
        }
        let size_rot = view.rotation.apply_size(doc.info.page_sizes[p]);
        let (pw, ph) = page_px(size_rot, q);
        let (nx, ny) = tile_grid(size_rot, q);
        let vis = screen.intersect(rect);
        if vis.width() <= 0.0 || vis.height() <= 0.0 {
            continue;
        }
        let fx0 = ((vis.min.x - screen.min.x) / screen.width()).clamp(0.0, 1.0);
        let fx1 = ((vis.max.x - screen.min.x) / screen.width()).clamp(0.0, 1.0);
        let fy0 = ((vis.min.y - screen.min.y) / screen.height()).clamp(0.0, 1.0);
        let fy1 = ((vis.max.y - screen.min.y) / screen.height()).clamp(0.0, 1.0);
        let tx0 = ((fx0 * pw as f32) / TILE as f32).floor() as u16;
        let tx1 = (((fx1 * pw as f32) / TILE as f32).ceil() as u16).min(nx).max(tx0 + 1);
        let ty0 = ((fy0 * ph as f32) / TILE as f32).floor() as u16;
        let ty1 = (((fy1 * ph as f32) / TILE as f32).ceil() as u16).min(ny).max(ty0 + 1);
        // Thumbnails already cover small pages; skip tiles when the page is tiny.
        if pw <= 240 && ph <= 240 && thumb.is_some() {
            continue;
        }
        let tile_rect = |tx: u16, ty: u16, pw: u32, ph: u32| -> Rect {
            let x0 = (tx as u32 * TILE) as f32 / pw as f32;
            let y0 = (ty as u32 * TILE) as f32 / ph as f32;
            let x1 = (((tx as u32 + 1) * TILE).min(pw)) as f32 / pw as f32;
            let y1 = (((ty as u32 + 1) * TILE).min(ph)) as f32 / ph as f32;
            Rect::from_min_max(
                pos2(screen.min.x + x0 * screen.width(), screen.min.y + y0 * screen.height()),
                pos2(screen.min.x + x1 * screen.width(), screen.min.y + y1 * screen.height()),
            )
        };
        // Previous scale underneath while the new one renders.
        let mut all_ready = true;
        let mut fresh = Vec::new();
        for ty in ty0..ty1 {
            for tx in tx0..tx1 {
                let key = TileKey { kind: TileKind::Tile { scale: q, tx, ty }, ..base };
                let tr = tile_rect(tx, ty, pw, ph);
                let d = ((tr.center().y - center_y).abs() / 100.0) as u32;
                match tiles.want(key, 10 + d) {
                    Some(t) => fresh.push((t, tr)),
                    None => all_ready = false,
                }
            }
        }
        if !all_ready {
            if let Some(&old) = view.last_good_scale.get(&p).filter(|&&o| o != q) {
                let (opw, oph) = page_px(size_rot, old);
                let (onx, ony) = tile_grid(size_rot, old);
                let otx0 = ((fx0 * opw as f32) / TILE as f32).floor() as u16;
                let otx1 = (((fx1 * opw as f32) / TILE as f32).ceil() as u16).min(onx);
                let oty0 = ((fy0 * oph as f32) / TILE as f32).floor() as u16;
                let oty1 = (((fy1 * oph as f32) / TILE as f32).ceil() as u16).min(ony);
                for ty in oty0..oty1 {
                    for tx in otx0..otx1 {
                        let key = TileKey { kind: TileKind::Tile { scale: old, tx, ty }, ..base };
                        if let Some(t) = tiles.cached(&key) {
                            painter.image(t.id(), tile_rect(tx, ty, opw, oph), uv, Color32::WHITE);
                        }
                    }
                }
            }
        } else {
            view.last_good_scale.insert(p, q);
        }
        for (t, tr) in fresh {
            painter.image(t.id(), tr, uv, Color32::WHITE);
        }
    }
    // Prefetch neighbours at low priority.
    for &p in &prefetch {
        let base = TileKey {
            doc: doc.id,
            page: p as u32,
            rotation: view.rotation,
            invert: dark_pages,
            generation: doc.generation(p as u32),
            kind: TileKind::Thumb,
        };
        tiles.want(base, 1000);
        let size_rot = view.rotation.apply_size(doc.info.page_sizes[p]);
        let (nx, ny) = tile_grid(size_rot, q);
        if (nx as u32) * (ny as u32) <= 4 {
            for ty in 0..ny {
                for tx in 0..nx {
                    tiles.want(TileKey { kind: TileKind::Tile { scale: q, tx, ty }, ..base }, 1100);
                }
            }
        }
    }
    view.last_good_scale.retain(|p, _| visible.contains(p) || prefetch.contains(p));

    // ---- overlays: search hits ----
    if let Some(search) = &doc.search {
        for &p in &visible {
            let g = &geoms[&p];
            for i in search.on_page(p as u32) {
                let current = search.current == Some(i);
                for r in &search.hits[i].rects {
                    if let Some(sr) = g.rect_to_screen(*r) {
                        let sr = sr.expand(1.0);
                        painter.rect_filled(sr, CornerRadius::same(2), if current { pal.search_current } else { pal.search_hit });
                        if current {
                            painter.rect_stroke(sr, CornerRadius::same(2), Stroke::new(1.5, Color32::from_rgb(255, 120, 0)), StrokeKind::Outside);
                        }
                    }
                }
            }
        }
    }

    // ---- overlays: selection ----
    if let Some(sel) = view.selection {
        for &p in &visible {
            if p < sel.start.0 || p > sel.end.0 {
                continue;
            }
            let (Some(d), Some(g)) = (datas.get(&p), geoms.get(&p)) else { continue };
            let a = if p == sel.start.0 { sel.start.1 } else { 0 };
            let b = if p == sel.end.0 { sel.end.1 } else { d.text.len() };
            for r in d.text.rects(a, b) {
                if let Some(sr) = g.rect_to_screen(r) {
                    painter.rect_filled(sr, CornerRadius::same(1), pal.selection);
                }
            }
        }
    }

    // ---- overlays: selected annotation ----
    if let Some((page, info)) = &view.selected_annot {
        if let Some(g) = geoms.get(&(*page as usize)) {
            if let Some(sr) = g.rect_to_screen(info.rect) {
                painter.rect_stroke(sr.expand(3.0), CornerRadius::same(3), Stroke::new(2.0, pal.accent), StrokeKind::Outside);
            }
        }
    }

    // ---- hover: links, annotations, text ----
    let hover_page = pointer.and_then(|p| visible.iter().copied().find(|i| geoms[i].screen.contains(p)));
    let hover_user = hover_page.and_then(|pg| pointer.and_then(|p| geoms[&pg].to_user(p)));
    let mut hover_link: Option<LinkTarget> = None;
    let mut hover_annot: Option<AnnotInfo> = None;
    let mut over_text = false;
    if let (Some(pg), Some((ux, uy))) = (hover_page, hover_user) {
        if let Some(d) = datas.get(&pg) {
            hover_link = d
                .links
                .iter()
                .find(|l| ux >= l.rect[0] && ux <= l.rect[2] && uy >= l.rect[1] && uy <= l.rect[3])
                .map(|l| l.target.clone());
            hover_annot = d
                .annots
                .iter()
                .rev()
                .find(|a| ux >= a.rect[0] && ux <= a.rect[2] && uy >= a.rect[1] && uy <= a.rect[3])
                .cloned();
            over_text = d.text.chars.iter().any(|c| {
                c.rect.is_some_and(|r| ux >= r[0] - 1.0 && ux <= r[2] + 1.0 && uy >= r[1] - 1.0 && uy <= r[3] + 1.0)
            });
        }
    }

    // ---- scrollbars ----
    let s_content = vec2(layout.width * s, layout.height * s);
    let vbar = view.mode.is_continuous() && s_content.y > rect.height() + 1.0;
    let hbar = s_content.x > rect.width() + 1.0;
    let vtrack = Rect::from_min_max(pos2(rect.max.x - SCROLLBAR, rect.min.y), pos2(rect.max.x, rect.max.y - if hbar { SCROLLBAR } else { 0.0 }));
    let htrack = Rect::from_min_max(pos2(rect.min.x, rect.max.y - SCROLLBAR), pos2(rect.max.x - if vbar { SCROLLBAR } else { 0.0 }, rect.max.y));
    let vthumb = {
        let h = (rect.height() / s_content.y * vtrack.height()).clamp(28.0, vtrack.height());
        let t = (view.scroll.y / (s_content.y - rect.height()).max(1.0)).clamp(0.0, 1.0);
        Rect::from_min_size(pos2(vtrack.min.x, vtrack.min.y + t * (vtrack.height() - h)), vec2(SCROLLBAR, h))
    };
    let hthumb = {
        let w = (rect.width() / s_content.x * htrack.width()).clamp(28.0, htrack.width());
        let t = (view.scroll.x / (s_content.x - rect.width()).max(1.0)).clamp(0.0, 1.0);
        Rect::from_min_size(pos2(htrack.min.x + t * (htrack.width() - w), htrack.min.y), vec2(w, SCROLLBAR))
    };

    // ---- pointer interaction ----
    let press_pos = ctx.input(|i| i.pointer.press_origin()).or(pointer);
    if response.drag_started() {
        let pp = press_pos.unwrap_or_default();
        view.drag = if vbar && vtrack.contains(pp) {
            Drag::VScroll { grab: if vthumb.contains(pp) { pp.y - vthumb.min.y } else { vthumb.height() * 0.5 } }
        } else if hbar && htrack.contains(pp) {
            Drag::HScroll { grab: if hthumb.contains(pp) { pp.x - hthumb.min.x } else { hthumb.width() * 0.5 } }
        } else {
            let pg = visible.iter().copied().find(|i| geoms[i].screen.contains(pp));
            let user = pg.and_then(|p| geoms[&p].to_user(pp));
            let middle = ctx.input(|i| i.pointer.middle_down());
            match (tool, pg, user) {
                _ if middle => Drag::Pan,
                (Tool::Hand | Tool::Magnifier, _, _) => Drag::Pan,
                (Tool::Select, Some(p), Some(u)) if over_text_at(&datas, p, u) => Drag::Select { page: p, at: u },
                (Tool::Select, _, _) => Drag::Pan,
                (Tool::Highlight | Tool::Underline | Tool::StrikeOut, Some(p), Some(u)) => Drag::Select { page: p, at: u },
                (Tool::Rect | Tool::Ellipse, Some(p), Some(_)) => Drag::Shape { page: p, a: pp },
                (Tool::Ink, Some(p), Some(u)) => {
                    view.ink.clear();
                    view.ink.push(u);
                    Drag::Ink { page: p }
                }
                _ => Drag::None,
            }
        };
        if matches!(view.drag, Drag::Select { .. }) {
            view.selection = None;
        }
    }
    if response.dragged() {
        let delta = response.drag_delta();
        match view.drag {
            Drag::Pan => {
                view.scroll -= delta;
                view.clamp_scroll();
            }
            Drag::VScroll { grab } => {
                if let Some(p) = pointer.or(ctx.input(|i| i.pointer.latest_pos())) {
                    let t = ((p.y - grab - vtrack.min.y) / (vtrack.height() - vthumb.height()).max(1.0)).clamp(0.0, 1.0);
                    view.scroll.y = t * (s_content.y - rect.height());
                    view.clamp_scroll();
                    view.indicator_until = now + 1.0;
                }
            }
            Drag::HScroll { grab } => {
                if let Some(p) = pointer.or(ctx.input(|i| i.pointer.latest_pos())) {
                    let t = ((p.x - grab - htrack.min.x) / (htrack.width() - hthumb.width()).max(1.0)).clamp(0.0, 1.0);
                    view.scroll.x = t * (s_content.x - rect.width());
                    view.clamp_scroll();
                }
            }
            Drag::Select { page, at } => {
                if let Some(p) = pointer.or(ctx.input(|i| i.pointer.latest_pos())) {
                    let focus = visible
                        .iter()
                        .copied()
                        .min_by(|a, b| geoms[a].screen.distance_sq_to_pos(p).total_cmp(&geoms[b].screen.distance_sq_to_pos(p)));
                    if let Some(fp) = focus {
                        if let (Some(u), Some(da), Some(df)) = (geoms[&fp].to_user(p), doc.peek_page_data(page as u32), datas.get(&fp)) {
                            view.selection = select_between(page, &da, at, fp, df, u);
                        }
                    }
                    autoscroll(view, rect, p);
                }
            }
            Drag::Ink { page } => {
                if let (Some(p), Some(g)) = (pointer, geoms.get(&page)) {
                    let p = p.clamp(g.screen.min, g.screen.max);
                    if let Some(u) = g.to_user(p) {
                        let far = view.ink.last().is_none_or(|l| ((l.0 - u.0).powi(2) + (l.1 - u.1).powi(2)).sqrt() > 0.6 / view.zoom);
                        if far && view.ink.len() < 20_000 {
                            view.ink.push(u);
                        }
                    }
                }
            }
            Drag::Shape { .. } | Drag::None => {}
        }
    }
    if response.drag_stopped() {
        match view.drag {
            Drag::Select { .. } => {
                if let (Some(kind), Some(sel)) = (tool.markup_kind(), view.selection) {
                    for (page, rects) in selection_rects(doc, &sel) {
                        actions.push(ViewAction::AddAnnot { page, annot: NewAnnot::Markup { kind, rects, color: markup_color(kind, highlight_color), text: String::new() } });
                    }
                    view.selection = None;
                }
            }
            Drag::Shape { page, a } => {
                if let (Some(b), Some(g)) = (pointer.or(ctx.input(|i| i.pointer.latest_pos())), geoms.get(&page)) {
                    let b = b.clamp(g.screen.min, g.screen.max);
                    if let (Some(ua), Some(ub)) = (g.to_user(a), g.to_user(b)) {
                        let r = [ua.0.min(ub.0), ua.1.min(ub.1), ua.0.max(ub.0), ua.1.max(ub.1)];
                        if r[2] - r[0] > 3.0 && r[3] - r[1] > 3.0 {
                            let kind = if tool == Tool::Ellipse { AnnotKind::Circle } else { AnnotKind::Square };
                            actions.push(ViewAction::AddAnnot { page: page as u32, annot: NewAnnot::Shape { kind, rect: r, color: ink_color, width: ink_width } });
                        }
                    }
                }
            }
            Drag::Ink { page } => {
                if view.ink.len() >= 2 {
                    let stroke = simplify(&view.ink, 0.35 / view.zoom.max(0.1));
                    actions.push(ViewAction::AddAnnot { page: page as u32, annot: NewAnnot::Ink { strokes: vec![stroke], color: ink_color, width: ink_width } });
                }
                view.ink.clear();
            }
            _ => {}
        }
        view.drag = Drag::None;
    }

    // Clicks (no drag).
    if response.clicked() {
        let p = pointer.unwrap_or_default();
        let on_bar = (vbar && vtrack.contains(p)) || (hbar && htrack.contains(p));
        if on_bar {
            if vbar && vtrack.contains(p) && !vthumb.contains(p) {
                view.page_down(if p.y > vthumb.center().y { 1 } else { -1 });
            }
        } else {
            match tool {
                Tool::Note => {
                    if let (Some(pg), Some(u)) = (hover_page, hover_user) {
                        actions.push(ViewAction::NewNote { page: pg as u32, at: u });
                    }
                }
                Tool::Magnifier => {
                    let shift = ctx.input(|i| i.modifiers.shift);
                    view.zoom_step(if shift { -1 } else { 1 }, Some(p - rect.min));
                }
                _ => {
                    view.selection = None;
                    if let Some(a) = &hover_annot {
                        view.selected_annot = Some((hover_page.unwrap_or(0) as u32, a.clone()));
                    } else {
                        view.selected_annot = None;
                        if let Some(link) = &hover_link {
                            follow(view, link, actions);
                        }
                    }
                }
            }
        }
    }
    if response.double_clicked() {
        if let (Some(a), Some(pg)) = (&hover_annot, hover_page) {
            actions.push(ViewAction::EditAnnot { page: pg as u32, info: a.clone() });
        } else if let (Some(pg), Some(u), Some(d)) = (hover_page, hover_user, hover_page.and_then(|p| datas.get(&p))) {
            if matches!(tool, Tool::Select | Tool::Highlight | Tool::Underline | Tool::StrikeOut) {
                if let Some((a, b)) = d.text.char_near(u.0, u.1).and_then(|i| d.text.word_at(i)) {
                    view.selection = Some(TextSel { start: (pg, a), end: (pg, b) });
                }
            }
        }
    }
    if response.triple_clicked() {
        if let (Some(pg), Some(u), Some(d)) = (hover_page, hover_user, hover_page.and_then(|p| datas.get(&p))) {
            if let Some((a, b)) = d.text.char_near(u.0, u.1).and_then(|i| d.text.line_at(i)) {
                view.selection = Some(TextSel { start: (pg, a), end: (pg, b) });
            }
        }
    }
    if response.secondary_clicked() {
        view.context_at = hover_page.zip(hover_user);
        if let Some(a) = &hover_annot {
            view.selected_annot = Some((hover_page.unwrap_or(0) as u32, a.clone()));
        }
    }

    // ---- cursor and tooltips ----
    if response.hovered() {
        let on_bar = pointer.is_some_and(|p| (vbar && vtrack.contains(p)) || (hbar && htrack.contains(p)));
        let icon = match (tool, view.drag) {
            (_, Drag::Pan) => CursorIcon::Grabbing,
            _ if on_bar => CursorIcon::Default,
            (Tool::Select | Tool::Hand, _) if hover_link.is_some() || hover_annot.is_some() => CursorIcon::PointingHand,
            (Tool::Select, _) if over_text => CursorIcon::Text,
            (Tool::Select | Tool::Hand, _) => CursorIcon::Grab,
            (Tool::Highlight | Tool::Underline | Tool::StrikeOut, _) => CursorIcon::Text,
            (Tool::Note | Tool::Ink | Tool::Rect | Tool::Ellipse, _) => CursorIcon::Crosshair,
            (Tool::Magnifier, _) => CursorIcon::ZoomIn,
        };
        ctx.set_cursor_icon(icon);
        if matches!(view.drag, Drag::None) {
            if let Some(LinkTarget::Uri(u)) = &hover_link {
                let u = u.clone();
                response.clone().on_hover_ui_at_pointer(|ui| {
                    ui.label(u);
                });
            } else if let Some(a) = hover_annot.as_ref().filter(|a| !a.contents.is_empty() || !a.author.is_empty()) {
                let a = a.clone();
                response.clone().on_hover_ui_at_pointer(|ui| {
                    ui.set_max_width(320.0);
                    if !a.author.is_empty() {
                        ui.strong(&a.author);
                    }
                    if !a.contents.is_empty() {
                        ui.label(&a.contents);
                    }
                    ui.weak(a.kind.label());
                });
            }
        }
    }

    // ---- in-progress drawing ----
    if let Drag::Ink { page } = view.drag {
        if let Some(g) = geoms.get(&page) {
            let pts: Vec<Pos2> = view.ink.iter().filter_map(|&(x, y)| g.to_screen(x, y)).collect();
            if pts.len() >= 2 {
                let c = Color32::from_rgb(ink_color[0], ink_color[1], ink_color[2]);
                painter.add(Shape::line(pts, Stroke::new((ink_width * g.k).max(1.0), c)));
            }
        }
    }
    if let Drag::Shape { page, a } = view.drag {
        if let (Some(b), Some(g)) = (pointer, geoms.get(&page)) {
            let r = Rect::from_two_pos(a, b.clamp(g.screen.min, g.screen.max));
            let c = Color32::from_rgb(ink_color[0], ink_color[1], ink_color[2]);
            let stroke = Stroke::new((ink_width * g.k).max(1.0), c);
            if tool == Tool::Ellipse {
                painter.add(Shape::ellipse_stroke(r.center(), r.size() * 0.5, stroke));
            } else {
                painter.rect_stroke(r, CornerRadius::ZERO, stroke, StrokeKind::Middle);
            }
        }
    }

    // ---- scrollbars (drawn last, over the pages) ----
    let bar_hover = pointer.is_some_and(|p| vtrack.expand(4.0).contains(p) || htrack.expand(4.0).contains(p));
    let bar_color = if bar_hover || matches!(view.drag, Drag::VScroll { .. } | Drag::HScroll { .. }) {
        Color32::from_gray(140).gamma_multiply(0.9)
    } else {
        Color32::from_gray(140).gamma_multiply(0.55)
    };
    if vbar {
        painter.rect_filled(vthumb.shrink2(vec2(2.0, 2.0)), CornerRadius::same(3), bar_color);
    }
    if hbar {
        painter.rect_filled(hthumb.shrink2(vec2(2.0, 2.0)), CornerRadius::same(3), bar_color);
    }

    // ---- page indicator while scrolling ----
    if now < view.indicator_until || matches!(view.drag, Drag::VScroll { .. }) {
        let text = format!("Page {}", doc.label(view.current_page));
        let galley = painter.layout_no_wrap(text, egui::FontId::proportional(13.0), Color32::WHITE);
        let pos = if matches!(view.drag, Drag::VScroll { .. }) {
            pos2(vthumb.min.x - galley.size().x - 24.0, vthumb.center().y - galley.size().y * 0.5)
        } else {
            pos2(rect.center().x - galley.size().x * 0.5, rect.max.y - 44.0)
        };
        let bg = Rect::from_min_size(pos - vec2(10.0, 5.0), galley.size() + vec2(20.0, 10.0));
        painter.rect_filled(bg, CornerRadius::same(12), Color32::from_black_alpha(170));
        painter.galley(pos, galley, Color32::WHITE);
        if matches!(view.drag, Drag::None) {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
    }

    // ---- magnifier ----
    if tool == Tool::Magnifier && matches!(view.drag, Drag::None) {
        if let (Some(p), Some(pg)) = (pointer, hover_page) {
            let g = &geoms[&pg];
            let size = 240.0;
            let mq = quantize_scale(s * magnifier_zoom * ppp);
            let size_rot = view.rotation.apply_size(doc.info.page_sizes[pg]);
            let (mpw, mph) = page_px(size_rot, mq);
            let fx = (p.x - g.screen.min.x) / g.screen.width();
            let fy = (p.y - g.screen.min.y) / g.screen.height();
            let dev = (size * ppp) as u32;
            let req = RegionReq {
                doc: doc.id,
                page: pg as u32,
                rotation: view.rotation,
                invert: dark_pages,
                scale: mq,
                x: (fx * mpw as f32) as i32 - dev as i32 / 2,
                y: (fy * mph as f32) as i32 - dev as i32 / 2,
                w: dev,
                h: dev,
            };
            if view.last_region != Some(req) {
                view.last_region = Some(req);
                worker.send(Request::Region { req });
            }
            let lens = Rect::from_center_size(p, vec2(size, size));
            let fg = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, id.with("lens")));
            fg.add(egui::epaint::Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(90) }.as_shape(lens, CornerRadius::same(10)));
            fg.rect_filled(lens, CornerRadius::same(10), page_bg);
            if let Some((r, tex)) = magnifier.filter(|(r, _)| r.doc == doc.id && r.page == pg as u32) {
                // Place the last rendered region where it belongs relative to the cursor.
                let k = 1.0 / ppp;
                let off = vec2((r.x - req.x) as f32 * k, (r.y - req.y) as f32 * k) * (mq as f32 / r.scale.max(1) as f32);
                let img = Rect::from_min_size(lens.min + off, vec2(r.w as f32, r.h as f32) * k * (mq as f32 / r.scale.max(1) as f32));
                let clip = fg.with_clip_rect(lens.shrink(1.0));
                clip.image(tex.id(), img, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
            fg.rect_stroke(lens, CornerRadius::same(10), Stroke::new(2.0, pal.accent), StrokeKind::Inside);
        }
    }

    // ---- context menu ----
    let has_sel = view.selection.is_some();
    response.context_menu(|ui| {
        ui.set_min_width(190.0);
        if has_sel {
            if ui.button(format!("{}  Copy", crate::theme::icon::COPY)).clicked() {
                if let Some(t) = selection_text(doc, view) {
                    actions.push(ViewAction::Copy(t));
                }
                ui.close();
            }
            for (label, kind) in [("Highlight", AnnotKind::Highlight), ("Underline", AnnotKind::Underline), ("Strikeout", AnnotKind::StrikeOut)] {
                if ui.button(format!("{}  {label}", crate::theme::icon::HIGHLIGHT)).clicked() {
                    if let Some(sel) = view.selection {
                        for (page, rects) in selection_rects(doc, &sel) {
                            actions.push(ViewAction::AddAnnot { page, annot: NewAnnot::Markup { kind, rects, color: markup_color(kind, highlight_color), text: String::new() } });
                        }
                    }
                    view.selection = None;
                    ui.close();
                }
            }
            ui.separator();
        }
        if let Some((page, info)) = view.selected_annot.clone() {
            if ui.button(format!("{}  Edit comment…", crate::theme::icon::COMMENT)).clicked() {
                actions.push(ViewAction::EditAnnot { page, info: info.clone() });
                ui.close();
            }
            if ui.button(format!("{}  Delete annotation", crate::theme::icon::TRASH)).clicked() {
                actions.push(ViewAction::RemoveAnnot { page, info });
                view.selected_annot = None;
                ui.close();
            }
            ui.separator();
        }
        if let Some((page, at)) = view.context_at {
            if ui.button(format!("{}  Add note here", crate::theme::icon::NOTE)).clicked() {
                actions.push(ViewAction::NewNote { page: page as u32, at });
                ui.close();
            }
            if ui.button(format!("{}  Bookmark this page", crate::theme::icon::BOOKMARK)).clicked() {
                actions.push(ViewAction::AddBookmark { page: page as u32 });
                ui.close();
            }
        }
        if ui.button(format!("{}  Rotate clockwise", crate::theme::icon::ROTATE_CW)).clicked() {
            view.rotate(true);
            ui.close();
        }
        if ui.button(format!("{}  Fit width", crate::theme::icon::FIT_WIDTH)).clicked() {
            view.fit(ZoomMode::FitWidth);
            ui.close();
        }
        if ui.button(format!("{}  Fit page", crate::theme::icon::FIT_PAGE)).clicked() {
            view.fit(ZoomMode::FitPage);
            ui.close();
        }
    });

    let _ = active;
    out
}

pub fn markup_color(kind: AnnotKind, highlight: [u8; 3]) -> [u8; 3] {
    match kind {
        AnnotKind::Underline => [37, 99, 235],
        AnnotKind::StrikeOut => [220, 38, 38],
        AnnotKind::Squiggly => [22, 163, 74],
        _ => highlight,
    }
}

fn over_text_at(datas: &HashMap<usize, Arc<PageData>>, page: usize, u: (f32, f32)) -> bool {
    datas.get(&page).is_some_and(|d| {
        d.text.chars.iter().any(|c| c.rect.is_some_and(|r| u.0 >= r[0] - 2.0 && u.0 <= r[2] + 2.0 && u.1 >= r[1] - 2.0 && u.1 <= r[3] + 2.0))
    })
}

fn follow(view: &mut ViewState, link: &LinkTarget, actions: &mut Vec<ViewAction>) {
    match link {
        LinkTarget::Page { page, y } => view.jump(Jump { page: *page as usize, y_user: *y, rect_user: None }, true),
        LinkTarget::Uri(u) => actions.push(ViewAction::OpenUri(u.clone())),
    }
}

fn autoscroll(view: &mut ViewState, rect: Rect, p: Pos2) {
    let edge = 30.0;
    let dy = if p.y < rect.min.y + edge {
        -(rect.min.y + edge - p.y).min(40.0)
    } else if p.y > rect.max.y - edge {
        (p.y - (rect.max.y - edge)).min(40.0)
    } else {
        0.0
    };
    if dy != 0.0 {
        view.scroll.y += dy * 0.5;
        view.clamp_scroll();
    }
}

/// Selection between an anchor and the pointer, possibly across pages.
fn select_between(pa: usize, da: &PageData, ua: (f32, f32), pf: usize, df: &PageData, uf: (f32, f32)) -> Option<TextSel> {
    if pa == pf {
        let (s, e) = da.text.select(ua, uf)?;
        return Some(TextSel { start: (pa, s), end: (pa, e) });
    }
    let ia = da.text.char_near(ua.0, ua.1)?;
    let jf = df.text.char_near(uf.0, uf.1);
    if pa < pf {
        let end = jf.map(|j| j + 1).unwrap_or(0);
        Some(TextSel { start: (pa, ia), end: (pf, end) })
    } else {
        let start = jf.unwrap_or(0);
        Some(TextSel { start: (pf, start), end: (pa, ia + 1) })
    }
}

/// Line rectangles of a selection, per page (user space).
pub fn selection_rects(doc: &DocState, sel: &TextSel) -> Vec<(u32, Vec<[f32; 4]>)> {
    let mut out = Vec::new();
    for p in sel.start.0..=sel.end.0 {
        let Some(d) = doc.peek_page_data(p as u32) else { continue };
        let a = if p == sel.start.0 { sel.start.1 } else { 0 };
        let b = if p == sel.end.0 { sel.end.1 } else { d.text.len() };
        let rects = d.text.rects(a, b);
        if !rects.is_empty() {
            out.push((p as u32, rects));
        }
    }
    out
}

pub fn selection_text(doc: &DocState, view: &ViewState) -> Option<String> {
    let sel = view.selection?;
    let mut s = String::new();
    for p in sel.start.0..=sel.end.0 {
        let Some(d) = doc.peek_page_data(p as u32) else { continue };
        let a = if p == sel.start.0 { sel.start.1 } else { 0 };
        let b = if p == sel.end.0 { sel.end.1 } else { d.text.len() };
        if !s.is_empty() && !s.ends_with('\n') {
            s.push('\n');
        }
        s.push_str(&d.text.slice(a, b));
    }
    (!s.is_empty()).then_some(s)
}

/// Ramer-Douglas-Peucker simplification of an ink stroke.
pub fn simplify(pts: &[(f32, f32)], eps: f32) -> Vec<(f32, f32)> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let mut keep = vec![false; pts.len()];
    keep[0] = true;
    keep[pts.len() - 1] = true;
    let mut stack = vec![(0usize, pts.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        let (ax, ay) = pts[a];
        let (bx, by) = pts[b];
        let (dx, dy) = (bx - ax, by - ay);
        let len = (dx * dx + dy * dy).sqrt().max(1e-6);
        let mut best = (0.0f32, 0usize);
        for (i, &(px, py)) in pts.iter().enumerate().take(b).skip(a + 1) {
            let d = ((px - ax) * dy - (py - ay) * dx).abs() / len;
            if d > best.0 {
                best = (d, i);
            }
        }
        if best.0 > eps {
            keep[best.1] = true;
            stack.push((a, best.1));
            stack.push((best.1, b));
        }
    }
    pts.iter().zip(keep).filter(|(_, k)| *k).map(|(p, _)| *p).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simplify_keeps_shape_and_drops_noise() {
        let line: Vec<(f32, f32)> = (0..100).map(|i| (i as f32, (i as f32 * 0.001).sin() * 0.01)).collect();
        assert_eq!(simplify(&line, 0.1).len(), 2);
        let corner = vec![(0.0, 0.0), (5.0, 0.0), (10.0, 0.0), (10.0, 5.0), (10.0, 10.0)];
        assert_eq!(simplify(&corner, 0.1), vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)]);
    }

    #[test]
    fn zoom_steps_and_anchor() {
        let mut v = ViewState::new(PageMode::Continuous, false, ZoomMode::Custom);
        v.layout = Arc::new(Layout::new(&[(612.0, 792.0); 10], &LayoutParams::default()));
        v.viewport = vec2(800.0, 600.0);
        v.zoom_step(1, None);
        assert!((v.zoom - 1.1).abs() < 1e-6);
        v.zoom_step(-1, None);
        v.zoom_step(-1, None);
        assert!((v.zoom - 0.9).abs() < 1e-6);
        // Anchored zoom keeps the point under the cursor.
        v.set_zoom(1.0, None, ZoomMode::Custom);
        v.scroll = vec2(0.0, 2000.0);
        v.clamp_scroll();
        let anchor = vec2(400.0, 300.0);
        let before = (v.scroll + anchor) / v.s();
        v.set_zoom(2.0, Some(anchor), ZoomMode::Custom);
        let after = (v.scroll + anchor) / v.s();
        assert!((before - after).length() < 0.01, "{before:?} vs {after:?}");
        v.set_zoom(1000.0, None, ZoomMode::Custom);
        assert_eq!(v.zoom, MAX_ZOOM);
    }

    #[test]
    fn single_page_mode_turns_pages() {
        let mut v = ViewState::new(PageMode::Single, false, ZoomMode::Custom);
        v.layout = Arc::new(Layout::new(&[(612.0, 792.0); 5], &LayoutParams { mode: PageMode::Single, ..Default::default() }));
        v.layout_key = Some((PageMode::Single, false, Rotation::R0, 5));
        v.viewport = vec2(800.0, 2000.0); // page fits entirely
        v.clamp_scroll();
        v.page_down(1);
        assert_eq!(v.current_page, 1);
        v.page_down(-1);
        assert_eq!(v.current_page, 0);
        v.page_down(-1);
        assert_eq!(v.current_page, 0);
        v.turn(1);
        v.turn(1);
        assert_eq!(v.current_page, 2);
        // Wheel past the end flips after enough overscroll.
        for _ in 0..10 {
            v.scroll_by(vec2(0.0, 30.0));
        }
        assert_eq!(v.current_page, 3);
    }

    #[test]
    fn history() {
        let mut v = ViewState::new(PageMode::Continuous, false, ZoomMode::Custom);
        v.goto(5, true);
        v.current_page = 5;
        v.goto(9, true);
        v.current_page = 9;
        assert!(v.can_go_back());
        v.go_back();
        assert_eq!(v.pending.unwrap().page, 5);
        v.current_page = 5;
        v.go_forward();
        assert_eq!(v.pending.unwrap().page, 9);
    }
}
