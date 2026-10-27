//! The application: tabs, panes, documents, and the glue between the UI and
//! the engine thread. Drawing of the chrome lives in `chrome.rs`, the
//! sidebar in `sidebar.rs` and dialogs in `dialogs.rs`.

use crate::bench::Bench;
use crate::doc::{AddedAnnot, DocState, SearchState};
use crate::theme;
use crate::tiles::TileCache;
use crate::view::{self, Jump, Tool, ViewAction, ViewState, ZoomMode};
use bdf_core::engine::{AnnotInfo, DocId, NewAnnot, OpenError, RegionReq};
use bdf_core::settings::{DataDir, Settings, ThemePref, UserBookmark, ZoomPref};
use bdf_core::text::SearchOptions;
use bdf_core::worker::{Request, Response, WorkerHandle};
use eframe::egui::{
    self, Color32, ColorImage, Context, CornerRadius, Id, Key, KeyboardShortcut, Modifiers, Rect,
    TextureHandle, TextureOptions, ViewportCommand, pos2, vec2,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub struct Tab {
    pub doc: DocId,
    pub view: ViewState,
}

#[derive(Default)]
pub struct Pane {
    pub tabs: Vec<Tab>,
    pub active: usize,
}

impl Pane {
    pub fn current(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    pub fn current_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SidebarTab {
    Thumbs,
    Outline,
    Bookmarks,
    Annots,
    Search,
}

pub struct Toast {
    pub text: String,
    pub error: bool,
    pub until: f64,
}

#[derive(Clone)]
pub enum NoteTarget {
    New((f32, f32)),
    Edit(AnnotInfo),
}

pub enum Dialog {
    Password {
        path: PathBuf,
        pane: usize,
        input: String,
        error: Option<String>,
    },
    Note {
        doc: DocId,
        page: u32,
        target: NoteTarget,
        text: String,
    },
    OpenUri(String),
    ConfirmClose {
        doc: DocId,
    },
    ConfirmQuit,
    ConfirmDelete {
        doc: DocId,
        page: u32,
        info: AnnotInfo,
    },
    Properties(DocId),
    Settings,
    Shortcuts,
    About,
    Error {
        title: String,
        text: String,
    },
    /// macOS and Linux only; Windows uses the system print dialog.
    #[cfg_attr(windows, allow(dead_code))]
    Print {
        doc: DocId,
        ranges: String,
        copies: u32,
        error: Option<String>,
    },
}

#[derive(Default)]
pub struct FindBar {
    pub open: bool,
    pub query: String,
    pub opts: SearchOptions,
    pub focus: bool,
    pub edited: Option<Instant>,
}

struct PendingOpen {
    path: PathBuf,
    pane: usize,
}

enum Edit {
    Add {
        doc: DocId,
        page: u32,
        annot: NewAnnot,
        redo: bool,
    },
    Remove {
        doc: DocId,
        page: u32,
        undo: Option<AddedAnnot>,
    },
    Text {
        doc: DocId,
        page: u32,
    },
}

/// Results of jobs run on the engine thread (printing).
#[cfg_attr(not(windows), allow(dead_code))]
pub enum JobResult {
    Printed(Result<u32, String>),
}

#[derive(Default)]
pub struct Perf {
    pub show: bool,
    pub frame_ms: VecDeque<f32>,
    pub last_sample: Option<(f64, crate::sysinfo::ProcStats)>,
}

pub struct BdfApp {
    pub data: DataDir,
    pub settings: Settings,
    pub settings_changed: bool,
    last_settings_save: Instant,
    /// The settings write in progress, off the UI thread (it waits for the disk).
    settings_saver: Option<std::thread::JoinHandle<()>>,
    pub worker: Option<WorkerHandle>,
    pub engine_error: Option<String>,
    pub docs: HashMap<DocId, DocState>,
    pub panes: Vec<Pane>,
    pub active_pane: usize,
    pub split_vertical: bool,
    pub split_ratio: f32,
    pub sync_scroll: bool,
    pub tiles: TileCache,
    pub tool: Tool,
    pending_opens: HashMap<DocId, PendingOpen>,
    pub dialog: Option<Dialog>,
    dialog_queue: VecDeque<Dialog>,
    pub toasts: Vec<Toast>,
    pub find: FindBar,
    pub sidebar_tab: SidebarTab,
    pub fullscreen: bool,
    magnifier: Option<(RegionReq, TextureHandle)>,
    next_ticket: u64,
    edits: HashMap<u64, Edit>,
    saves: HashMap<u64, DocId>,
    /// Documents whose tabs close once their save succeeds ("Save" when closing).
    close_after_save: HashSet<DocId>,
    /// A tab was closed or moved out of a pane, which may have emptied it.
    pane_emptied: bool,
    quit_after_save: bool,
    allow_close: bool,
    pub perf: Perf,
    pub bench: Option<Bench>,
    pub page_input: String,
    pub page_input_focus: bool,
    pub hwnd: Option<isize>,
    pub renderer: String,
    jobs_tx: mpsc::Sender<JobResult>,
    jobs_rx: mpsc::Receiver<JobResult>,
    pub thumbs_scrolled_to: Option<(DocId, usize)>,
    frame_start: Instant,
    pub first_frame_at: Option<Instant>,
    /// Page requested on the command line for the first document.
    start_page: Option<usize>,
    /// A web link clicked in the outline, confirmed through a dialog.
    pub outline_uri: Option<String>,
    last_title: String,
}

pub struct StartOptions {
    pub files: Vec<PathBuf>,
    pub page: Option<usize>,
    pub bench: Option<Bench>,
    pub renderer: String,
}

impl BdfApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        data: DataDir,
        settings: Settings,
        opts: StartOptions,
    ) -> Self {
        let ctx = cc.egui_ctx.clone();
        theme::install_fonts(&ctx);
        theme::apply_both(&ctx);
        theme::install_art(&ctx, settings.eye);
        ctx.set_theme(match settings.theme {
            ThemePref::System => egui::ThemePreference::System,
            ThemePref::Light => egui::ThemePreference::Light,
            ThemePref::Dark => egui::ThemePreference::Dark,
        });
        let repaint = ctx.clone();
        let (worker, engine_error) = match WorkerHandle::spawn(
            || {
                bdf_core::pdfium::PdfiumEngine::load().map(|e| {
                    log::info!("PDFium loaded from {}", e.library_path);
                    Box::new(e) as Box<dyn bdf_core::engine::Engine>
                })
            },
            Box::new(move || repaint.request_repaint()),
        ) {
            Ok(w) => (Some(w), None),
            Err(e) => {
                log::error!("engine failed to start: {e}");
                (None, Some(e))
            }
        };
        let (jobs_tx, jobs_rx) = mpsc::channel();
        let budget = settings.cache_mib as usize * 1024 * 1024;
        let mut app = Self {
            data,
            settings,
            settings_changed: false,
            last_settings_save: Instant::now(),
            settings_saver: None,
            worker,
            engine_error,
            docs: HashMap::new(),
            panes: vec![Pane::default()],
            active_pane: 0,
            split_vertical: false,
            split_ratio: 0.5,
            sync_scroll: false,
            tiles: TileCache::new(budget),
            tool: Tool::Select,
            pending_opens: HashMap::new(),
            dialog: None,
            dialog_queue: VecDeque::new(),
            toasts: Vec::new(),
            find: FindBar::default(),
            sidebar_tab: SidebarTab::Thumbs,
            fullscreen: false,
            magnifier: None,
            next_ticket: 1,
            edits: HashMap::new(),
            saves: HashMap::new(),
            close_after_save: HashSet::new(),
            pane_emptied: false,
            quit_after_save: false,
            allow_close: false,
            perf: Perf::default(),
            bench: opts.bench,
            page_input: String::new(),
            page_input_focus: false,
            hwnd: None,
            renderer: opts.renderer,
            jobs_tx,
            jobs_rx,
            thumbs_scrolled_to: None,
            frame_start: Instant::now(),
            first_frame_at: None,
            start_page: opts.page,
            outline_uri: None,
            last_title: String::new(),
        };
        let mut files = opts.files;
        if files.is_empty() && app.settings.restore_tabs && app.bench.is_none() {
            files = app.settings.open_tabs.clone();
        }
        for f in files {
            app.open_path(&f, 0);
        }
        app
    }

    pub fn ticket(&mut self) -> u64 {
        self.next_ticket += 1;
        self.next_ticket
    }

    pub fn toast(&mut self, ctx: &Context, text: impl Into<String>, error: bool) {
        let now = ctx.input(|i| i.time);
        let text = text.into();
        if error {
            log::warn!("{text}");
        }
        self.toasts.push(Toast {
            text,
            error,
            until: now + if error { 8.0 } else { 3.0 },
        });
        if self.toasts.len() > 5 {
            self.toasts.remove(0);
        }
    }

    pub fn show_dialog(&mut self, d: Dialog) {
        if self.dialog.is_none() {
            self.dialog = Some(d);
        } else {
            self.dialog_queue.push_back(d);
        }
    }

    pub fn close_dialog(&mut self) {
        self.dialog = self.dialog_queue.pop_front();
    }

    // ---------------------------------------------------------------------
    // Documents and tabs.

    pub fn active_tab(&self) -> Option<&Tab> {
        self.panes.get(self.active_pane).and_then(|p| p.current())
    }

    pub fn active_doc_id(&self) -> Option<DocId> {
        self.active_tab().map(|t| t.doc)
    }

    /// The active document and its view, mutably.
    pub fn active_mut(&mut self) -> Option<(&mut DocState, &mut ViewState)> {
        let pane = self.panes.get_mut(self.active_pane)?;
        let tab = pane.tabs.get_mut(pane.active)?;
        let doc = self.docs.get_mut(&tab.doc)?;
        Some((doc, &mut tab.view))
    }

    pub fn active_view_mut(&mut self) -> Option<&mut ViewState> {
        self.panes
            .get_mut(self.active_pane)?
            .current_mut()
            .map(|t| &mut t.view)
    }

    fn new_view(&self) -> ViewState {
        let zoom = match self.settings.default_zoom {
            ZoomPref::FitWidth => ZoomMode::FitWidth,
            ZoomPref::FitPage => ZoomMode::FitPage,
            ZoomPref::Actual => ZoomMode::Custom,
        };
        ViewState::new(self.settings.page_mode, self.settings.cover_page, zoom)
    }

    pub fn open_path(&mut self, path: &Path, pane: usize) {
        let pane = pane.min(self.panes.len() - 1);
        let canonical = crate::platform::clean_path(
            &path.canonicalize().unwrap_or_else(|_| path.to_path_buf()),
        );
        // Already open: show it.
        if let Some((&id, _)) = self
            .docs
            .iter()
            .find(|(_, d)| d.path == canonical || d.path == path)
        {
            if let Some(i) = self.panes[pane].tabs.iter().position(|t| t.doc == id) {
                self.panes[pane].active = i;
            } else {
                let mut view = self.new_view();
                if let Some(t) = self
                    .panes
                    .iter()
                    .flat_map(|p| p.tabs.iter())
                    .find(|t| t.doc == id)
                {
                    view.current_page = t.view.current_page;
                    view.pending = Some(Jump {
                        page: t.view.current_page,
                        y_user: None,
                        rect_user: None,
                    });
                }
                self.panes[pane].tabs.push(Tab { doc: id, view });
                self.panes[pane].active = self.panes[pane].tabs.len() - 1;
            }
            self.active_pane = pane;
            return;
        }
        if self.pending_opens.values().any(|p| p.path == canonical) {
            return;
        }
        self.request_open(canonical, None, pane);
    }

    fn request_open(&mut self, path: PathBuf, password: Option<String>, pane: usize) {
        let Some(worker) = &self.worker else { return };
        let doc = worker.alloc_doc_id();
        worker.send(Request::Open {
            doc,
            path: path.clone(),
            password,
        });
        self.pending_opens.insert(doc, PendingOpen { path, pane });
    }

    pub fn open_with_password(&mut self, path: PathBuf, password: String, pane: usize) {
        self.request_open(path, Some(password), pane);
    }

    pub fn quit_after_save_all(&mut self) {
        self.quit_after_save = true;
    }

    /// Closes without prompting (benchmark, after "Discard").
    pub fn force_close(&mut self, _ctx: &Context) {
        self.allow_close = true;
    }

    pub fn opening(&self) -> usize {
        self.pending_opens.len()
    }

    pub fn open_dialog(&mut self, pane: usize) {
        let mut d = rfd::FileDialog::new()
            .add_filter("PDF documents", &["pdf", "PDF"])
            .add_filter("All files", &["*"]);
        if let Some(dir) = self.settings.recent.first().and_then(|r| r.path.parent()) {
            d = d.set_directory(dir);
        }
        if let Some(files) = d.pick_files() {
            for f in files {
                self.open_path(&f, pane);
            }
        }
    }

    fn remember_position(&mut self, doc: DocId, page: usize) {
        if let Some(d) = self.docs.get(&doc) {
            let fp = d.info.fingerprint.clone();
            self.settings.doc_mut(&fp).last_page = page as u32;
            self.settings_changed = true;
        }
    }

    /// Closes a tab. Unsaved changes prompt unless `force`.
    pub fn close_tab(&mut self, pane: usize, index: usize, force: bool) {
        let Some(tab) = self.panes.get(pane).and_then(|p| p.tabs.get(index)) else {
            return;
        };
        let doc = tab.doc;
        let others = self
            .panes
            .iter()
            .flat_map(|p| p.tabs.iter())
            .filter(|t| t.doc == doc)
            .count()
            > 1;
        if !force && !others && self.docs.get(&doc).is_some_and(|d| d.dirty) {
            self.panes[pane].active = index;
            self.active_pane = pane;
            self.show_dialog(Dialog::ConfirmClose { doc });
            return;
        }
        let page = self.panes[pane].tabs[index].view.current_page;
        self.remember_position(doc, page);
        self.panes[pane].tabs.remove(index);
        let p = &mut self.panes[pane];
        if p.active >= p.tabs.len() {
            p.active = p.tabs.len().saturating_sub(1);
        }
        if !others {
            self.release_doc(doc);
        }
        // An emptied pane is removed by `collapse_empty_pane`, not here: this
        // runs while the panes are being drawn.
        self.pane_emptied = true;
    }

    /// Closing the last tab of a pane closes the split view. Runs between
    /// frames only, never while the panes are drawn, since removing a pane
    /// shifts their indices.
    fn collapse_empty_pane(&mut self) -> bool {
        if !std::mem::take(&mut self.pane_emptied) || self.panes.len() < 2 {
            return false;
        }
        let Some(empty) = self.panes.iter().position(|p| p.tabs.is_empty()) else {
            return false;
        };
        self.panes.remove(empty);
        self.active_pane = 0;
        true
    }

    /// "Save" when closing: close the document's tabs once the save succeeds.
    pub fn save_and_close(&mut self, ctx: &Context, doc: DocId) {
        self.save(ctx, doc, false);
        if self.docs.get(&doc).is_some_and(|d| d.saving) {
            self.close_after_save.insert(doc);
        }
    }

    /// Closes every tab of a document (used after "Don't save").
    pub fn close_doc_everywhere(&mut self, doc: DocId) {
        loop {
            let found =
                self.panes.iter().enumerate().find_map(|(pi, p)| {
                    p.tabs.iter().position(|t| t.doc == doc).map(|ti| (pi, ti))
                });
            match found {
                Some((pi, ti)) => self.close_tab(pi, ti, true),
                None => break,
            }
        }
    }

    fn release_doc(&mut self, doc: DocId) {
        if let Some(w) = &self.worker {
            w.send(Request::Close { doc });
        }
        self.tiles.forget_doc(doc);
        self.docs.remove(&doc);
        if self.magnifier.as_ref().is_some_and(|(r, _)| r.doc == doc) {
            self.magnifier = None;
        }
    }

    pub fn toggle_split(&mut self, vertical: bool) {
        if self.panes.len() == 2 && self.split_vertical == vertical {
            // Merge the second pane into the first.
            let second = self.panes.remove(1);
            for t in second.tabs {
                if !self.panes[0].tabs.iter().any(|x| x.doc == t.doc) {
                    self.panes[0].tabs.push(t);
                } else {
                    // Drop the duplicate view; the document stays open in pane 0.
                }
            }
            self.active_pane = 0;
            return;
        }
        self.split_vertical = vertical;
        if self.panes.len() == 1 {
            let mut pane = Pane::default();
            if let Some(t) = self.panes[0].current() {
                let mut view = self.new_view();
                view.rotation = t.view.rotation;
                view.mode = t.view.mode;
                view.current_page = t.view.current_page;
                view.pending = Some(Jump {
                    page: t.view.current_page,
                    y_user: None,
                    rect_user: None,
                });
                pane.tabs.push(Tab { doc: t.doc, view });
            }
            self.panes.push(pane);
            self.active_pane = 1;
        }
    }

    pub fn move_tab_to_other_pane(&mut self, pane: usize, index: usize) {
        if self.panes.len() == 1 {
            self.panes.push(Pane::default());
        }
        let target = 1 - pane.min(1);
        if index >= self.panes[pane].tabs.len() {
            return;
        }
        let tab = self.panes[pane].tabs.remove(index);
        let p = &mut self.panes[pane];
        if p.active >= p.tabs.len() {
            p.active = p.tabs.len().saturating_sub(1);
        }
        self.panes[target].tabs.push(tab);
        self.panes[target].active = self.panes[target].tabs.len() - 1;
        self.active_pane = target;
        // A now-empty source pane is removed by `collapse_empty_pane`.
        self.pane_emptied = true;
    }

    // ---------------------------------------------------------------------
    // Engine responses.

    fn pump(&mut self, ctx: &Context) {
        let Some(worker) = &self.worker else { return };
        if !worker.is_alive() && self.engine_error.is_none() {
            self.engine_error =
                Some("The PDF engine stopped unexpectedly. Save your work and restart BDF.".into());
        }
        let deadline = Instant::now() + Duration::from_millis(10);
        let mut batch = Vec::new();
        let mut tiles = 0;
        while let Some(r) = worker.try_recv() {
            if matches!(r, Response::Tile { .. }) {
                tiles += 1;
            }
            batch.push(r);
            if tiles >= 24 || Instant::now() > deadline {
                ctx.request_repaint();
                break;
            }
        }
        for r in batch {
            self.handle(ctx, r);
        }
        while let Ok(j) = self.jobs_rx.try_recv() {
            match j {
                JobResult::Printed(Ok(n)) => {
                    self.toast(ctx, format!("Sent {n} page(s) to the printer"), false)
                }
                JobResult::Printed(Err(e)) => {
                    self.toast(ctx, format!("Printing failed: {e}"), true)
                }
            }
        }
    }

    fn handle(&mut self, ctx: &Context, r: Response) {
        match r {
            Response::Tile { key, result } => match result {
                Ok(img) => {
                    let current = self
                        .docs
                        .get(&key.doc)
                        .is_some_and(|d| d.generation(key.page) == key.generation);
                    if current {
                        self.tiles.insert(ctx, key, img);
                    }
                }
                Err(e) => {
                    log::warn!("render of page {} failed: {e}", key.page + 1);
                    self.tiles.failed(&key);
                }
            },
            Response::Opened { doc, result } => {
                let Some(p) = self.pending_opens.remove(&doc) else {
                    return;
                };
                match result {
                    Ok(info) => {
                        let ds = DocState::new(doc, *info);
                        let fp = ds.info.fingerprint.clone();
                        let pages = ds.page_count();
                        self.settings.touch_recent(&p.path);
                        let mut page = 0;
                        if self.settings.remember_page {
                            page = self
                                .settings
                                .docs
                                .get(&fp)
                                .map(|m| m.last_page as usize)
                                .unwrap_or(0);
                        }
                        if let Some(sp) = self.start_page.take() {
                            page = sp.saturating_sub(1);
                        }
                        self.settings.doc_mut(&fp);
                        self.settings_changed = true;
                        let mut view = self.new_view();
                        view.current_page = page.min(pages.saturating_sub(1));
                        view.pending = Some(Jump {
                            page: view.current_page,
                            y_user: None,
                            rect_user: None,
                        });
                        self.docs.insert(doc, ds);
                        let pane = p.pane.min(self.panes.len() - 1);
                        self.panes[pane].tabs.push(Tab { doc, view });
                        self.panes[pane].active = self.panes[pane].tabs.len() - 1;
                        self.active_pane = pane;
                        log::info!("opened {} ({pages} pages)", p.path.display());
                    }
                    Err(OpenError::PasswordRequired) => self.show_dialog(Dialog::Password {
                        path: p.path,
                        pane: p.pane,
                        input: String::new(),
                        error: None,
                    }),
                    Err(OpenError::WrongPassword) => self.show_dialog(Dialog::Password {
                        path: p.path,
                        pane: p.pane,
                        input: String::new(),
                        error: Some("That password is not correct.".into()),
                    }),
                    Err(e) => {
                        let name = p
                            .path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        self.show_dialog(Dialog::Error {
                            title: format!("Could not open {name}"),
                            text: e.to_string(),
                        });
                    }
                }
            }
            Response::PageData { doc, page, data } => {
                if let Some(d) = self.docs.get_mut(&doc) {
                    match data {
                        Ok(pd) => d.store_page_data(page, *pd),
                        Err(e) => {
                            log::warn!("page data for page {} failed: {e}", page + 1);
                            d.page_data_failed(page);
                        }
                    }
                }
            }
            Response::Region { req, image } => {
                if let Ok(img) = image {
                    let color = ColorImage::from_rgba_unmultiplied(
                        [img.width as usize, img.height as usize],
                        &img.rgba,
                    );
                    match &mut self.magnifier {
                        Some((r, tex)) => {
                            tex.set(color, TextureOptions::LINEAR);
                            *r = req;
                        }
                        None => {
                            let tex = ctx.load_texture("magnifier", color, TextureOptions::LINEAR);
                            self.magnifier = Some((req, tex));
                        }
                    }
                }
            }
            Response::SearchHits { doc, ticket, hits } => {
                let mut jump = None;
                if let Some(d) = self.docs.get_mut(&doc)
                    && let Some(s) = d.search.as_mut().filter(|s| s.ticket == ticket)
                {
                    let first = s.hits.is_empty() && s.current.is_none();
                    s.add_hits(hits);
                    if first {
                        s.current = Some(0);
                        jump = s.hits.first().cloned();
                    }
                }
                if let Some(h) = jump {
                    self.jump_to_hit(doc, h.page, h.rects.first().copied(), true);
                }
            }
            Response::SearchProgress {
                doc,
                ticket,
                searched,
                total,
                done,
            } => {
                if let Some(s) = self
                    .docs
                    .get_mut(&doc)
                    .and_then(|d| d.search.as_mut())
                    .filter(|s| s.ticket == ticket)
                {
                    s.searched = searched;
                    s.total = total;
                    s.done = done;
                }
            }
            Response::Annots {
                doc,
                ticket,
                page,
                annots,
                done,
            } => {
                if let Some(d) = self.docs.get_mut(&doc)
                    && d.annots.ticket == ticket
                {
                    d.annots.items.extend(annots.into_iter().map(|a| (page, a)));
                    d.annots.pages_done = page + 1;
                    d.annots.complete = done;
                }
            }
            Response::AnnotEdited {
                doc: _,
                page: _,
                ticket,
                result,
            } => {
                let Some(edit) = self.edits.remove(&ticket) else {
                    return;
                };
                match (edit, result) {
                    (
                        Edit::Add {
                            doc,
                            page,
                            annot,
                            redo,
                        },
                        Ok(name),
                    ) => {
                        if let Some(d) = self.docs.get_mut(&doc) {
                            d.bump(page);
                            d.dirty = true;
                            d.undo.push(AddedAnnot { page, name, annot });
                            if !redo {
                                d.redo.clear();
                            }
                        }
                    }
                    (Edit::Remove { doc, page, undo }, Ok(_)) => {
                        if let Some(d) = self.docs.get_mut(&doc) {
                            d.bump(page);
                            d.dirty = true;
                            if let Some(a) = undo {
                                d.redo.push(a);
                            }
                        }
                        for p in &mut self.panes {
                            for t in &mut p.tabs {
                                if t.doc == doc {
                                    t.view.selected_annot = None;
                                }
                            }
                        }
                    }
                    (Edit::Text { doc, page }, Ok(_)) => {
                        if let Some(d) = self.docs.get_mut(&doc) {
                            d.bump(page);
                            d.dirty = true;
                        }
                    }
                    (_, Err(e)) => {
                        self.toast(ctx, format!("Could not change the annotation: {e}"), true)
                    }
                }
            }
            Response::Saved {
                doc,
                ticket,
                target,
                result,
            } => {
                self.saves.remove(&ticket);
                let name = target
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                match result {
                    Ok(()) => {
                        if let Some(d) = self.docs.get_mut(&doc) {
                            d.saving = false;
                            d.dirty = false;
                            d.path = target.clone();
                        }
                        self.settings.touch_recent(&target);
                        self.toast(ctx, format!("Saved {name}"), false);
                        if self.close_after_save.remove(&doc) {
                            self.close_doc_everywhere(doc);
                        }
                        if self.quit_after_save
                            && self.saves.is_empty()
                            && !self.docs.values().any(|d| d.dirty)
                        {
                            self.allow_close = true;
                            ctx.send_viewport_cmd(ViewportCommand::Close);
                        }
                    }
                    Err(e) => {
                        if let Some(d) = self.docs.get_mut(&doc) {
                            d.saving = false;
                        }
                        self.quit_after_save = false;
                        self.close_after_save.remove(&doc);
                        self.show_dialog(Dialog::Error {
                            title: format!("Could not save {name}"),
                            text: e,
                        });
                    }
                }
            }
            Response::FormDone {
                doc,
                page,
                ticket: _,
                result,
            } => match result {
                Ok(out) => {
                    if let Some(d) = self.docs.get_mut(&doc) {
                        if out.handled || out.changed {
                            d.bump(page);
                        }
                        if out.changed {
                            d.dirty = true;
                        }
                    }
                    for p in &mut self.panes {
                        for t in p.tabs.iter_mut().filter(|t| t.doc == doc) {
                            t.view.form_focus = out.focused.then_some(page as usize);
                        }
                    }
                }
                Err(e) => self.toast(
                    ctx,
                    format!("The form field could not be updated: {e}"),
                    true,
                ),
            },
            Response::Fault(msg) => {
                self.toast(ctx, format!("Internal error (recovered): {msg}"), true)
            }
        }
    }

    // ---------------------------------------------------------------------
    // Commands.

    pub fn jump_to_hit(&mut self, doc: DocId, page: u32, rect: Option<[f32; 4]>, remember: bool) {
        let pane = &mut self.panes[self.active_pane];
        if let Some(t) = pane.current_mut().filter(|t| t.doc == doc) {
            t.view.jump(
                Jump {
                    page: page as usize,
                    y_user: None,
                    rect_user: rect,
                },
                remember,
            );
        }
        // Make sure the geometry for precise scrolling arrives.
        if let (Some(d), Some(w)) = (self.docs.get_mut(&doc), &self.worker) {
            d.page_data(page, w);
        }
    }

    pub fn run_search(&mut self) {
        let query = self.find.query.trim().to_string();
        let opts = self.find.opts;
        let ticket = self.ticket();
        let Some(worker) = &self.worker else { return };
        let pane = &self.panes[self.active_pane];
        let Some(tab) = pane.current() else { return };
        let start = tab.view.current_page as u32;
        let Some(d) = self.docs.get_mut(&tab.doc) else {
            return;
        };
        worker.send(Request::CancelSearch { doc: d.id });
        if query.is_empty() {
            d.search = None;
            return;
        }
        if d.search
            .as_ref()
            .is_some_and(|s| s.query == query && s.opts == opts)
        {
            return;
        }
        d.search = Some(SearchState {
            query: query.clone(),
            opts,
            ticket,
            hits: Vec::new(),
            current: None,
            searched: 0,
            total: d.page_count() as u32,
            done: false,
        });
        worker.send(Request::Search {
            doc: d.id,
            ticket,
            query,
            opts,
            start_page: start,
        });
    }

    pub fn find_step(&mut self, forward: bool) {
        if self.find.edited.is_some() {
            self.find.edited = None;
            self.run_search();
        }
        let Some(tab) = self.active_tab() else { return };
        let (doc, from) = (tab.doc, tab.view.current_page as u32);
        let Some(d) = self.docs.get_mut(&doc) else {
            return;
        };
        let Some(s) = d.search.as_mut() else { return };
        if let Some(i) = s.step(forward, from) {
            let h = s.hits[i].clone();
            self.jump_to_hit(doc, h.page, h.rects.first().copied(), false);
        }
    }

    pub fn close_find(&mut self) {
        self.find.open = false;
        if let (Some(id), Some(w)) = (self.active_doc_id(), &self.worker) {
            w.send(Request::CancelSearch { doc: id });
            if let Some(d) = self.docs.get_mut(&id) {
                d.search = None;
            }
        }
    }

    pub fn add_annot(&mut self, doc: DocId, page: u32, annot: NewAnnot) {
        let ticket = self.ticket();
        if let Some(w) = &self.worker {
            w.send(Request::AddAnnot {
                doc,
                page,
                annot: annot.clone(),
                ticket,
            });
            self.edits.insert(
                ticket,
                Edit::Add {
                    doc,
                    page,
                    annot,
                    redo: false,
                },
            );
        }
    }

    pub fn delete_annot(&mut self, doc: DocId, page: u32, info: AnnotInfo, confirmed: bool) {
        // Annotations added in this session can be undone, so no prompt.
        let own = self.docs.get(&doc).and_then(|d| {
            info.name
                .as_ref()
                .and_then(|n| d.undo.iter().position(|a| &a.name == n))
        });
        if own.is_none() && !confirmed {
            self.show_dialog(Dialog::ConfirmDelete { doc, page, info });
            return;
        }
        let undo = own.and_then(|i| self.docs.get_mut(&doc).map(|d| d.undo.remove(i)));
        let ticket = self.ticket();
        if let Some(w) = &self.worker {
            w.send(Request::RemoveAnnot {
                doc,
                page,
                index: info.index,
                name: info.name.clone(),
                ticket,
            });
            self.edits.insert(ticket, Edit::Remove { doc, page, undo });
        }
        if let Some(d) = self.docs.get_mut(&doc) {
            d.annots
                .items
                .retain(|(p, a)| !(*p == page && a.index == info.index && a.name == info.name));
        }
    }

    pub fn set_annot_text(&mut self, doc: DocId, page: u32, info: &AnnotInfo, text: String) {
        let ticket = self.ticket();
        if let Some(w) = &self.worker {
            w.send(Request::SetAnnotText {
                doc,
                page,
                index: info.index,
                name: info.name.clone(),
                text,
                ticket,
            });
            self.edits.insert(ticket, Edit::Text { doc, page });
        }
    }

    pub fn undo(&mut self, ctx: &Context) {
        let Some(id) = self.active_doc_id() else {
            return;
        };
        let Some(a) = self.docs.get_mut(&id).and_then(|d| d.undo.pop()) else {
            self.toast(ctx, "Nothing to undo", false);
            return;
        };
        let ticket = self.ticket();
        if let Some(w) = &self.worker {
            w.send(Request::RemoveAnnot {
                doc: id,
                page: a.page,
                index: usize::MAX,
                name: Some(a.name.clone()),
                ticket,
            });
            self.edits.insert(
                ticket,
                Edit::Remove {
                    doc: id,
                    page: a.page,
                    undo: Some(a),
                },
            );
        }
    }

    pub fn redo(&mut self, ctx: &Context) {
        let Some(id) = self.active_doc_id() else {
            return;
        };
        let Some(a) = self.docs.get_mut(&id).and_then(|d| d.redo.pop()) else {
            self.toast(ctx, "Nothing to redo", false);
            return;
        };
        let ticket = self.ticket();
        if let Some(w) = &self.worker {
            w.send(Request::AddAnnot {
                doc: id,
                page: a.page,
                annot: a.annot.clone(),
                ticket,
            });
            self.edits.insert(
                ticket,
                Edit::Add {
                    doc: id,
                    page: a.page,
                    annot: a.annot,
                    redo: true,
                },
            );
        }
    }

    pub fn save(&mut self, ctx: &Context, doc: DocId, save_as: bool) {
        let Some(d) = self.docs.get(&doc) else { return };
        if d.saving {
            return;
        }
        let target = if save_as {
            let mut dlg = rfd::FileDialog::new()
                .add_filter("PDF documents", &["pdf"])
                .set_file_name(d.title());
            if let Some(dir) = d.path.parent() {
                dlg = dlg.set_directory(dir);
            }
            match dlg.save_file() {
                Some(mut p) => {
                    if p.extension().is_none() {
                        p.set_extension("pdf");
                    }
                    p
                }
                None => return,
            }
        } else {
            if !d.dirty {
                self.toast(ctx, "No changes to save", false);
                return;
            }
            d.path.clone()
        };
        let ticket = self.ticket();
        if let Some(w) = &self.worker {
            w.send(Request::Save {
                doc,
                target,
                ticket,
            });
            self.saves.insert(ticket, doc);
            if let Some(d) = self.docs.get_mut(&doc) {
                d.saving = true;
            }
        }
    }

    pub fn print(&mut self, ctx: &Context) {
        let Some(id) = self.active_doc_id() else {
            return;
        };
        let Some(d) = self.docs.get(&id) else { return };
        #[cfg(windows)]
        {
            let pages = d.page_count() as u32;
            let title = d.title();
            if let Some(job) = crate::print::windows::dialog(self.hwnd, pages) {
                let tx = self.jobs_tx.clone();
                let repaint = ctx.clone();
                if let Some(w) = &self.worker {
                    w.send(Request::Job {
                        job: Box::new(move |engine| {
                            let r = crate::print::windows::run(job, engine, id, &title);
                            let _ = tx.send(JobResult::Printed(r));
                            repaint.request_repaint();
                        }),
                    });
                    self.toast(ctx, "Printing…", false);
                }
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (ctx, &self.jobs_tx);
            let doc = d.id;
            self.show_dialog(Dialog::Print {
                doc,
                ranges: String::new(),
                copies: 1,
                error: None,
            });
        }
    }

    pub fn add_bookmark(&mut self, ctx: &Context, doc: DocId, page: u32) {
        let Some(d) = self.docs.get(&doc) else { return };
        let title = outline_title_for(&d.info.outline, page)
            .unwrap_or_else(|| format!("Page {}", d.label(page as usize)));
        let fp = d.info.fingerprint.clone();
        let m = self.settings.doc_mut(&fp);
        if m.bookmarks.iter().any(|b| b.page == page) {
            self.toast(ctx, "That page is already bookmarked", false);
            return;
        }
        m.bookmarks.push(UserBookmark { title, page });
        m.bookmarks.sort_by_key(|b| b.page);
        self.settings_changed = true;
        self.toast(ctx, format!("Bookmarked page {}", page + 1), false);
    }

    pub fn scan_annots(&mut self, doc: DocId) {
        let ticket = self.ticket();
        if let (Some(d), Some(w)) = (self.docs.get_mut(&doc), &self.worker) {
            d.annots = crate::doc::AnnotIndex {
                ticket,
                ..Default::default()
            };
            w.send(Request::ScanAnnots { doc, ticket });
        }
    }

    fn handle_view_actions(&mut self, ctx: &Context, doc: DocId, actions: Vec<ViewAction>) {
        for a in actions {
            match a {
                ViewAction::AddAnnot { page, annot } => self.add_annot(doc, page, annot),
                ViewAction::RemoveAnnot { page, info } => self.delete_annot(doc, page, info, false),
                ViewAction::EditAnnot { page, info } => {
                    let text = info.contents.clone();
                    self.show_dialog(Dialog::Note {
                        doc,
                        page,
                        target: NoteTarget::Edit(info),
                        text,
                    });
                }
                ViewAction::NewNote { page, at } => self.show_dialog(Dialog::Note {
                    doc,
                    page,
                    target: NoteTarget::New(at),
                    text: String::new(),
                }),
                ViewAction::OpenUri(u) => self.show_dialog(Dialog::OpenUri(u)),
                ViewAction::Copy(t) => {
                    ctx.copy_text(t);
                    self.toast(ctx, "Copied", false);
                }
                ViewAction::AddBookmark { page } => self.add_bookmark(ctx, doc, page),
                ViewAction::Form { page, input } => self.form_input(doc, page, input),
            }
        }
    }

    pub fn form_input(&mut self, doc: DocId, page: u32, input: bdf_core::engine::FormInput) {
        let ticket = self.ticket();
        if let Some(w) = &self.worker {
            w.send(Request::FormInput {
                doc,
                page,
                input,
                ticket,
            });
        }
    }

    /// While a form field has focus, typing goes to the field instead of
    /// moving around the document. Returns true if the field took the input.
    fn route_keys_to_form(&mut self, ctx: &Context) -> bool {
        use bdf_core::engine::{FormInput, vkey};
        let Some((doc, page)) = self
            .active_tab()
            .and_then(|t| t.view.form_focus.map(|p| (t.doc, p as u32)))
        else {
            return false;
        };
        if self.dialog.is_some() || ctx.egui_wants_keyboard_input() {
            return false;
        }
        let mut inputs = Vec::new();
        ctx.input_mut(|i| {
            i.events.retain(|e| match e {
                egui::Event::Text(t) | egui::Event::Paste(t) => {
                    inputs.extend(t.encode_utf16().map(FormInput::Char));
                    false
                }
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    let (shift, ctrl) = (modifiers.shift, modifiers.command);
                    let k = |code| FormInput::Key { code, shift, ctrl };
                    match key {
                        // Windows sends Backspace and Enter as characters too.
                        Key::Backspace => inputs.extend([k(vkey::BACK), FormInput::Char(0x08)]),
                        Key::Enter => inputs.extend([k(vkey::RETURN), FormInput::Char(0x0D)]),
                        Key::Tab => inputs.push(k(vkey::TAB)),
                        Key::Delete => inputs.push(k(vkey::DELETE)),
                        Key::ArrowLeft => inputs.push(k(vkey::LEFT)),
                        Key::ArrowRight => inputs.push(k(vkey::RIGHT)),
                        Key::ArrowUp => inputs.push(k(vkey::UP)),
                        Key::ArrowDown => inputs.push(k(vkey::DOWN)),
                        Key::Home => inputs.push(k(vkey::HOME)),
                        Key::End => inputs.push(k(vkey::END)),
                        Key::A if ctrl => inputs.push(k(vkey::A)),
                        Key::Escape => inputs.push(FormInput::KillFocus),
                        // Other shortcuts (Ctrl+S, ...) keep working.
                        _ => return true,
                    }
                    false
                }
                _ => true,
            })
        });
        let had = !inputs.is_empty();
        for input in inputs {
            self.form_input(doc, page, input);
        }
        had
    }

    pub fn copy_selection(&mut self, ctx: &Context) {
        let text = self
            .active_mut()
            .and_then(|(d, v)| view::selection_text(d, v));
        if let Some(t) = text {
            ctx.copy_text(t);
            self.toast(ctx, "Copied", false);
        }
    }

    /// Applies a markup tool to the current selection (toolbar shortcut).
    pub fn markup_selection(&mut self, kind: bdf_core::engine::AnnotKind) -> bool {
        let color = view::markup_color(kind, self.settings.highlight_color);
        let Some((d, v)) = self.active_mut() else {
            return false;
        };
        let Some(sel) = v.selection.take() else {
            return false;
        };
        let id = d.id;
        let parts = view::selection_rects(d, &sel);
        for (page, rects) in parts {
            self.add_annot(
                id,
                page,
                NewAnnot::Markup {
                    kind,
                    rects,
                    color,
                    text: String::new(),
                },
            );
        }
        true
    }

    pub fn set_tool(&mut self, tool: Tool) {
        if let Some(kind) = tool.markup_kind()
            && self.markup_selection(kind)
        {
            return;
        }
        self.tool = tool;
    }

    pub fn select_all_on_page(&mut self) {
        if let Some((d, v)) = self.active_mut() {
            let p = v.current_page;
            if let Some(pd) = d.peek_page_data(p as u32) {
                let chars = pd.text.select_all_visible();
                v.selection = Some(view::TextSel::Chars { page: p, chars });
            }
        }
    }

    pub fn set_fullscreen(&mut self, ctx: &Context, on: bool) {
        self.fullscreen = on;
        ctx.send_viewport_cmd(ViewportCommand::Fullscreen(on));
    }

    pub fn apply_theme(&mut self, ctx: &Context) {
        ctx.set_theme(match self.settings.theme {
            ThemePref::System => egui::ThemePreference::System,
            ThemePref::Light => egui::ThemePreference::Light,
            ThemePref::Dark => egui::ThemePreference::Dark,
        });
        self.settings_changed = true;
    }

    pub fn is_dark(ctx: &Context) -> bool {
        ctx.global_style().visuals.dark_mode
    }

    fn wait_for_settings_save(&mut self) {
        if let Some(h) = self.settings_saver.take() {
            let _ = h.join();
        }
    }

    fn save_settings_now(&mut self) {
        // Remember open tabs and positions.
        let mut tabs = Vec::new();
        let mut positions = Vec::new();
        for p in &self.panes {
            for t in &p.tabs {
                if let Some(d) = self.docs.get(&t.doc) {
                    if !tabs.contains(&d.path) {
                        tabs.push(d.path.clone());
                    }
                    positions.push((d.info.fingerprint.clone(), t.view.current_page));
                }
            }
        }
        for (fp, page) in positions {
            self.settings.doc_mut(&fp).last_page = page as u32;
        }
        self.settings.open_tabs = tabs;
        // One write at a time; the previous one finished long ago (saves are 10 s apart).
        self.wait_for_settings_save();
        let (data, settings) = (self.data.clone(), self.settings.clone());
        self.settings_saver = Some(std::thread::spawn(move || {
            if let Err(e) = data.save(&settings) {
                log::warn!("could not save settings: {e}");
            }
        }));
        self.settings_changed = false;
        self.last_settings_save = Instant::now();
    }

    // ---------------------------------------------------------------------
    // Keyboard.

    fn shortcuts(&mut self, ctx: &Context) {
        let cmd = Modifiers::COMMAND;
        let cs = Modifiers::COMMAND | Modifiers::SHIFT;
        let sc = |m: Modifiers, k: Key| KeyboardShortcut::new(m, k);
        let take = |s: KeyboardShortcut| ctx.input_mut(|i| i.consume_shortcut(&s));
        let modal = self.dialog.is_some();
        if modal {
            return;
        }
        let typing_in_form = self.route_keys_to_form(ctx);
        let pane = self.active_pane;
        if take(sc(cmd, Key::O)) {
            self.open_dialog(pane);
        }
        if take(sc(cs, Key::S))
            && let Some(id) = self.active_doc_id()
        {
            self.save(ctx, id, true);
        }
        if take(sc(cmd, Key::S))
            && let Some(id) = self.active_doc_id()
        {
            self.save(ctx, id, false);
        }
        if take(sc(cmd, Key::P)) {
            self.print(ctx);
        }
        // Only a fresh press closes a tab: holding Ctrl+W must not close one
        // tab after another as the key repeats.
        let close_pressed = ctx.input_mut(|i| {
            let mut fresh = false;
            i.events.retain(|e| match e {
                egui::Event::Key {
                    key: Key::W | Key::F4,
                    pressed: true,
                    repeat,
                    modifiers,
                    ..
                } if modifiers.matches_logically(cmd) => {
                    fresh |= !repeat;
                    false
                }
                _ => true,
            });
            fresh
        });
        if close_pressed {
            let p = &self.panes[pane];
            if !p.tabs.is_empty() {
                self.close_tab(pane, p.active, false);
            }
        }
        if take(sc(cmd, Key::F)) {
            self.find.open = true;
            self.find.focus = true;
        }
        if take(sc(Modifiers::SHIFT, Key::F3)) {
            self.find_step(false);
        }
        if take(sc(Modifiers::NONE, Key::F3)) {
            self.find_step(true);
        }
        if take(sc(cs, Key::Z)) || take(sc(cmd, Key::Y)) {
            self.redo(ctx);
        }
        if take(sc(cmd, Key::Z)) {
            self.undo(ctx);
        }
        if take(sc(cmd, Key::B))
            && let Some(t) = self.active_tab()
        {
            let (doc, page) = (t.doc, t.view.current_page as u32);
            self.add_bookmark(ctx, doc, page);
        }
        if take(sc(cmd, Key::D))
            && let Some(id) = self.active_doc_id()
        {
            self.show_dialog(Dialog::Properties(id));
        }
        if take(sc(Modifiers::NONE, Key::F4)) {
            self.settings.sidebar = !self.settings.sidebar;
            self.settings_changed = true;
        }
        if take(sc(Modifiers::NONE, Key::F11)) || take(sc(cmd, Key::L)) {
            let fs = !self.fullscreen;
            self.set_fullscreen(ctx, fs);
        }
        if take(sc(Modifiers::NONE, Key::F12)) {
            self.perf.show = !self.perf.show;
        }
        if take(sc(cmd, Key::Backslash)) {
            let v = self.split_vertical;
            self.toggle_split(v);
        }
        if take(sc(Modifiers::CTRL, Key::Tab)) {
            let p = &mut self.panes[pane];
            if !p.tabs.is_empty() {
                p.active = (p.active + 1) % p.tabs.len();
            }
        }
        if take(sc(Modifiers::CTRL | Modifiers::SHIFT, Key::Tab)) {
            let p = &mut self.panes[pane];
            if !p.tabs.is_empty() {
                p.active = (p.active + p.tabs.len() - 1) % p.tabs.len();
            }
        }
        if take(sc(cmd, Key::G)) {
            self.page_input_focus = true;
        }
        // Zoom.
        if (take(sc(cmd, Key::Plus)) || take(sc(cmd, Key::Equals)) || take(sc(cs, Key::Equals)))
            && let Some(v) = self.active_view_mut()
        {
            v.zoom_step(1, None);
        }
        if take(sc(cmd, Key::Minus))
            && let Some(v) = self.active_view_mut()
        {
            v.zoom_step(-1, None);
        }
        if take(sc(cmd, Key::Num0))
            && let Some(v) = self.active_view_mut()
        {
            v.fit(ZoomMode::FitPage);
        }
        if take(sc(cmd, Key::Num1))
            && let Some(v) = self.active_view_mut()
        {
            v.set_zoom(1.0, None, ZoomMode::Custom);
        }
        if take(sc(cmd, Key::Num2))
            && let Some(v) = self.active_view_mut()
        {
            v.fit(ZoomMode::FitWidth);
        }
        if take(sc(cs, Key::R))
            && let Some(v) = self.active_view_mut()
        {
            v.rotate(false);
        }
        if take(sc(cmd, Key::R))
            && let Some(v) = self.active_view_mut()
        {
            v.rotate(true);
        }
        if take(sc(Modifiers::ALT, Key::ArrowLeft))
            && let Some(v) = self.active_view_mut()
        {
            v.go_back();
        }
        if take(sc(Modifiers::ALT, Key::ArrowRight))
            && let Some(v) = self.active_view_mut()
        {
            v.go_forward();
        }
        if (take(sc(cmd, Key::C))
            || ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy))))
            && !ctx.egui_wants_keyboard_input()
        {
            self.copy_selection(ctx);
        }
        if take(sc(cmd, Key::A)) && !ctx.egui_wants_keyboard_input() {
            self.select_all_on_page();
        }

        // Plain keys only when no text field (ours or the document's) has focus.
        let form_focused = self
            .active_tab()
            .is_some_and(|t| t.view.form_focus.is_some());
        if ctx.egui_wants_keyboard_input() || typing_in_form || form_focused {
            return;
        }
        let pressed = |k: Key| ctx.input(|i| i.key_pressed(k) && i.modifiers.is_none());
        let shift_pressed = |k: Key| ctx.input(|i| i.key_pressed(k) && i.modifiers.shift_only());
        if pressed(Key::Escape) {
            if self.fullscreen {
                self.set_fullscreen(ctx, false);
            } else if self.find.open {
                self.close_find();
            } else if let Some(v) = self.active_view_mut() {
                v.selection = None;
                v.selected_annot = None;
            }
            self.tool = if matches!(self.tool, Tool::Hand) {
                Tool::Hand
            } else {
                Tool::Select
            };
        }
        if pressed(Key::Delete) || pressed(Key::Backspace) {
            let sel = self
                .active_tab()
                .and_then(|t| t.view.selected_annot.clone().map(|a| (t.doc, a)));
            if let Some((doc, (page, info))) = sel {
                self.delete_annot(doc, page, info, false);
            }
        }
        if pressed(Key::H) {
            self.tool = Tool::Hand;
        }
        if pressed(Key::V) {
            self.tool = Tool::Select;
        }
        if pressed(Key::Z) {
            self.tool = Tool::Magnifier;
        }
        let n = self
            .active_tab()
            .and_then(|t| self.docs.get(&t.doc))
            .map(|d| d.page_count())
            .unwrap_or(0);
        let Some(v) = self.active_view_mut() else {
            return;
        };
        let continuous = v.mode.is_continuous();
        if pressed(Key::PageDown) || pressed(Key::Space) {
            v.page_down(1);
        }
        if pressed(Key::PageUp) || shift_pressed(Key::Space) {
            v.page_down(-1);
        }
        if pressed(Key::ArrowDown) {
            v.scroll_by(vec2(0.0, 60.0));
        }
        if pressed(Key::ArrowUp) {
            v.scroll_by(vec2(0.0, -60.0));
        }
        if pressed(Key::ArrowRight) {
            if continuous && v.layout().width * v.zoom * view::PT_TO_PX > v.viewport.x {
                v.scroll_by(vec2(60.0, 0.0));
            } else {
                v.turn(1);
            }
        }
        if pressed(Key::ArrowLeft) {
            if continuous && v.layout().width * v.zoom * view::PT_TO_PX > v.viewport.x {
                v.scroll_by(vec2(-60.0, 0.0));
            } else {
                v.turn(-1);
            }
        }
        if pressed(Key::Home) {
            v.first_page();
        }
        if pressed(Key::End) {
            v.last_page(n);
        }
    }

    // ---------------------------------------------------------------------
    // Frame.

    fn draw_panes(&mut self, ui: &mut egui::Ui, ctx: &Context) {
        let full = ui.max_rect();
        let n = self.panes.len();
        let rects: Vec<Rect> = if n == 1 {
            vec![full]
        } else {
            let divider = 6.0;
            let (a, b) = if self.split_vertical {
                let h = (full.height() - divider) * self.split_ratio;
                (
                    Rect::from_min_size(full.min, vec2(full.width(), h)),
                    Rect::from_min_max(pos2(full.min.x, full.min.y + h + divider), full.max),
                )
            } else {
                let w = (full.width() - divider) * self.split_ratio;
                (
                    Rect::from_min_size(full.min, vec2(w, full.height())),
                    Rect::from_min_max(pos2(full.min.x + w + divider, full.min.y), full.max),
                )
            };
            // Draggable divider.
            let div = if self.split_vertical {
                Rect::from_min_max(pos2(full.min.x, a.max.y), pos2(full.max.x, b.min.y))
            } else {
                Rect::from_min_max(pos2(a.max.x, full.min.y), pos2(b.min.x, full.max.y))
            };
            let r = ui.interact(div, Id::unique("split-divider"), egui::Sense::drag());
            let dark = Self::is_dark(ctx);
            let pal = theme::palette(dark);
            ui.painter().rect_filled(
                div,
                CornerRadius::ZERO,
                if r.hovered() || r.dragged() {
                    pal.accent
                } else {
                    ui.visuals().widgets.noninteractive.bg_stroke.color
                },
            );
            if r.hovered() || r.dragged() {
                ctx.set_cursor_icon(if self.split_vertical {
                    egui::CursorIcon::ResizeVertical
                } else {
                    egui::CursorIcon::ResizeHorizontal
                });
            }
            if r.dragged()
                && let Some(p) = r.interact_pointer_pos()
            {
                self.split_ratio = if self.split_vertical {
                    (p.y - full.min.y) / full.height()
                } else {
                    (p.x - full.min.x) / full.width()
                }
                .clamp(0.15, 0.85);
            }
            vec![a, b]
        };
        let mut scrolled_pane = None;
        for (i, rect) in rects.into_iter().enumerate() {
            if self.draw_pane(ui, ctx, i, rect) {
                scrolled_pane = Some(i);
            }
        }
        // Synchronised scrolling between the two panes.
        if self.sync_scroll
            && self.panes.len() == 2
            && let Some(src) = scrolled_pane
        {
            let anchor = self.panes[src].current().and_then(|t| t.view.anchor());
            if let (Some(a), Some(t)) = (anchor, self.panes[1 - src].current_mut()) {
                t.view.sync_to = Some(a);
            }
        }
    }

    /// Draws one pane; returns true if its view scrolled.
    fn draw_pane(&mut self, ui: &mut egui::Ui, ctx: &Context, index: usize, rect: Rect) -> bool {
        let dark = Self::is_dark(ctx);
        let pal = theme::palette(dark);
        let tab_h = if self.fullscreen { 0.0 } else { 34.0 };
        let strip = Rect::from_min_size(rect.min, vec2(rect.width(), tab_h));
        let body = Rect::from_min_max(pos2(rect.min.x, rect.min.y + tab_h), rect.max);
        if index >= self.panes.len() {
            return false;
        }
        if !self.fullscreen {
            self.tab_strip(ui, ctx, index, strip);
        }
        if self.panes.get(index).is_none_or(|p| p.tabs.is_empty()) {
            self.welcome(ui, ctx, index, body);
            return false;
        }
        let active = index == self.active_pane;
        let Some(worker) = self.worker.as_ref() else {
            return false;
        };
        let pane = &mut self.panes[index];
        let Some(tab) = pane.tabs.get_mut(pane.active) else {
            return false;
        };
        let Some(doc) = self.docs.get_mut(&tab.doc) else {
            return false;
        };
        let doc_id = doc.id;
        let before = (tab.view.scroll, tab.view.zoom, tab.view.current_page);
        let mut actions = Vec::new();
        let out = view::show(
            ui,
            body,
            view::ViewCtx {
                doc,
                view: &mut tab.view,
                tiles: &mut self.tiles,
                worker,
                tool: self.tool,
                pal,
                dark_pages: self.settings.dark_pages,
                highlight_color: self.settings.highlight_color,
                ink_color: self.settings.ink_color,
                ink_width: self.settings.ink_width,
                magnifier_zoom: self.settings.magnifier_zoom,
                magnifier: self.magnifier.as_ref(),
                active,
                id: Id::unique(("doc-view", index)),
                actions: &mut actions,
            },
        );
        let after = (tab.view.scroll, tab.view.zoom, tab.view.current_page);
        if out.clicked {
            self.active_pane = index;
        }
        if self.panes.len() > 1 && active {
            ui.painter().rect_stroke(
                body.shrink(1.0),
                CornerRadius::ZERO,
                egui::Stroke::new(1.5, pal.accent.gamma_multiply(0.6)),
                egui::StrokeKind::Inside,
            );
        }
        if active && self.find.open {
            self.find_bar(ui, ctx, body);
        }
        self.handle_view_actions(ctx, doc_id, actions);
        before != after && active
    }

    fn toasts_ui(&mut self, ctx: &Context) {
        let now = ctx.input(|i| i.time);
        self.toasts.retain(|t| t.until > now);
        if self.toasts.is_empty() {
            return;
        }
        let dark = Self::is_dark(ctx);
        let pal = theme::palette(dark);
        let screen = ctx.content_rect();
        let mut y = screen.max.y - 40.0;
        let mut dismiss = None;
        for (i, t) in self.toasts.iter().enumerate().rev() {
            let area = egui::Area::new(Id::unique(("toast", i)))
                .order(egui::Order::Foreground)
                .fixed_pos(pos2(screen.max.x - 16.0, y))
                .pivot(egui::Align2::RIGHT_BOTTOM);
            let r = area.show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.set_max_width(380.0);
                        ui.horizontal(|ui| {
                            let (icon, color) = if t.error {
                                (theme::icon::WARNING, pal.danger)
                            } else {
                                (theme::icon::CHECK, pal.success)
                            };
                            ui.label(egui::RichText::new(icon).color(color).size(16.0));
                            ui.label(&t.text);
                        });
                    })
            });
            if r.response.clicked() {
                dismiss = Some(i);
            }
            y -= r.response.rect.height() + 8.0;
        }
        if let Some(i) = dismiss {
            self.toasts.remove(i);
        }
        ctx.request_repaint_after(Duration::from_millis(250));
    }

    fn drop_files(&mut self, ctx: &Context) {
        let (hovering, dropped) =
            ctx.input(|i| (!i.raw.hovered_files.is_empty(), i.raw.dropped_files.clone()));
        if hovering {
            let screen = ctx.content_rect();
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                Id::unique("drop"),
            ));
            let pal = theme::palette(Self::is_dark(ctx));
            painter.rect_filled(screen, CornerRadius::ZERO, Color32::from_black_alpha(120));
            painter.rect_stroke(
                screen.shrink(16.0),
                CornerRadius::same(12),
                egui::Stroke::new(3.0, pal.accent),
                egui::StrokeKind::Inside,
            );
            painter.text(
                screen.center(),
                egui::Align2::CENTER_CENTER,
                format!("{}  Drop to open", theme::icon::PDF),
                egui::FontId::proportional(28.0),
                Color32::WHITE,
            );
        }
        if !dropped.is_empty() {
            // Open into the pane under the pointer.
            let pane = self.active_pane;
            for f in dropped {
                {
                    let p = f.path().to_path_buf();
                    self.open_path(&p, pane);
                }
            }
        }
    }

    fn perf_overlay(&mut self, ctx: &Context) {
        if !self.perf.show {
            return;
        }
        let now = ctx.input(|i| i.time);
        let sample = match self.perf.last_sample {
            Some((t, s)) if now - t < 1.0 => s,
            _ => {
                let s = crate::sysinfo::sample();
                self.perf.last_sample = Some((now, s));
                s
            }
        };
        let mut ft: Vec<f32> = self.perf.frame_ms.iter().copied().collect();
        ft.sort_by(f32::total_cmp);
        let p = |q: f32| {
            ft.get(((ft.len() as f32 - 1.0) * q) as usize)
                .copied()
                .unwrap_or(0.0)
        };
        egui::Area::new(Id::unique("perf"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::RIGHT_TOP, vec2(-12.0, 90.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.monospace(format!(
                        "frame p50 {:5.1} ms  p95 {:5.1} ms",
                        p(0.5),
                        p(0.95)
                    ));
                    ui.monospace(format!(
                        "memory {}  peak {}",
                        crate::sysinfo::mib(sample.rss_bytes),
                        crate::sysinfo::mib(sample.peak_rss_bytes)
                    ));
                    ui.monospace(format!(
                        "textures {} ({:.0} MB)  pending {}",
                        self.tiles.len(),
                        self.tiles.used_bytes() as f64 / 1_048_576.0,
                        self.tiles.pending()
                    ));
                    if let Some(w) = &self.worker {
                        let s = &w.stats;
                        let n = s.tiles_rendered.load(std::sync::atomic::Ordering::Relaxed);
                        let us = s.render_micros.load(std::sync::atomic::Ordering::Relaxed);
                        ui.monospace(format!(
                            "rendered {n}  avg {:.1} ms  dropped {}",
                            us as f64 / 1000.0 / n.max(1) as f64,
                            s.tiles_dropped.load(std::sync::atomic::Ordering::Relaxed)
                        ));
                    }
                    ui.monospace(format!("renderer {}", self.renderer));
                });
            });
        ctx.request_repaint_after(Duration::from_millis(500));
    }
}

/// Deepest outline entry pointing at `page`, for bookmark titles.
fn outline_title_for(items: &[bdf_core::engine::OutlineItem], page: u32) -> Option<String> {
    let mut best = None;
    for it in items {
        if it.page == Some(page) && !it.title.is_empty() {
            best = Some(it.title.clone());
        }
        if let Some(t) = outline_title_for(&it.children, page) {
            best = Some(t);
        }
    }
    best
}

impl eframe::App for BdfApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let t0 = Instant::now();
        if self.first_frame_at.is_none() {
            self.first_frame_at = Some(t0);
        }
        self.hwnd = crate::platform::hwnd(frame);
        if let Some(mut bench) = self.bench.take() {
            let keep = bench.tick(self, &ctx);
            if keep {
                self.bench = Some(bench);
            }
        }
        self.pump(&ctx);
        self.tiles.begin_frame();

        // Closing: ask about unsaved changes.
        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_close {
            if self.docs.values().any(|d| d.dirty) {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                if !matches!(self.dialog, Some(Dialog::ConfirmQuit)) {
                    self.show_dialog(Dialog::ConfirmQuit);
                }
            } else {
                self.allow_close = true;
            }
        }

        self.shortcuts(&ctx);
        self.collapse_empty_pane();
        if self
            .find
            .edited
            .is_some_and(|t| t.elapsed() > Duration::from_millis(300))
        {
            self.find.edited = None;
            self.run_search();
        }

        if !self.fullscreen {
            self.menu_and_toolbar(ui, &ctx);
            self.status_bar(ui, &ctx);
            if self.settings.sidebar && !self.panes.iter().all(|p| p.tabs.is_empty()) {
                self.sidebar(ui, &ctx);
            }
        }
        egui::CentralPanel::no_frame().show(ui, |ui| {
            if let Some(err) = self.engine_error.clone() {
                self.engine_missing(ui, &err);
            } else {
                self.draw_panes(ui, &ctx);
            }
        });
        if self.collapse_empty_pane() {
            ctx.request_repaint();
        }
        self.dialogs(&ctx);
        self.toasts_ui(&ctx);
        self.drop_files(&ctx);
        self.perf_overlay(&ctx);

        if let Some(w) = &self.worker {
            self.tiles.end_frame(w);
        }
        // Window title.
        let title = match self.active_tab().and_then(|t| self.docs.get(&t.doc)) {
            Some(d) => format!("{}{} — BDF", if d.dirty { "• " } else { "" }, d.title()),
            None => "BDF".to_string(),
        };
        // Viewport commands trigger a repaint, so only send real changes.
        if title != self.last_title {
            ctx.send_viewport_cmd(ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }
        self.track_window(&ctx);
        if self.settings_changed && self.last_settings_save.elapsed() > Duration::from_secs(10) {
            self.save_settings_now();
        }
        let ms = t0.elapsed().as_secs_f32() * 1000.0;
        self.perf.frame_ms.push_back(ms);
        if self.perf.frame_ms.len() > 240 {
            self.perf.frame_ms.pop_front();
        }
        self.frame_start = t0;
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_settings_now();
        self.wait_for_settings_save();
        log::info!("exiting");
    }
}

impl BdfApp {
    fn track_window(&mut self, ctx: &Context) {
        if self.fullscreen {
            return;
        }
        let (rect, max) = ctx.input(|i| {
            (
                i.viewport().outer_rect,
                i.viewport().maximized.unwrap_or(false),
            )
        });
        if max != self.settings.maximized {
            self.settings.maximized = max;
            self.settings_changed = true;
        }
        if let Some(r) = rect.filter(|_| !max) {
            let w = [r.min.x, r.min.y, r.width(), r.height()];
            if self
                .settings
                .window
                .is_none_or(|o| o.iter().zip(w).any(|(a, b)| (a - b).abs() > 1.0))
            {
                self.settings.window = Some(w);
                self.settings_changed = true;
            }
        }
    }

    fn engine_missing(&mut self, ui: &mut egui::Ui, err: &str) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.25);
            ui.label(egui::RichText::new(theme::icon::WARNING).size(48.0));
            ui.heading("The PDF engine could not be loaded");
            ui.add_space(8.0);
            ui.label("BDF needs PDFium next to its executable:");
            ui.monospace(crate::platform::pdfium_name());
            ui.add_space(8.0);
            ui.label(egui::RichText::new(err).weak().small());
        });
    }
}
