//! The engine thread.
//!
//! PDFium is single-threaded, so one thread owns it and every document. The
//! UI talks to it through [`WorkerHandle`] and never blocks on it.
//!
//! Scheduling, in order:
//! 1. Control requests (open, close, save, annotation edits) in arrival order.
//! 2. The latest magnifier region.
//! 3. Renders of what is on screen (priority below [`ON_SCREEN`]), so pages
//!    never sit blank while their text is being read.
//! 4. Page data (text, links, annotations, the hidden-text check) for visible
//!    pages.
//! 5. Other renders (the next screen, sidebar thumbnails) by priority. The UI
//!    sends [`Request::Retain`] whenever the set of tiles it wants changes;
//!    queued tiles outside that set are dropped, so scrolling fast never
//!    leaves a backlog behind.
//! 6. Search, one page per step, so it never holds up rendering.
//!
//! A panic inside a job is caught and reported; the thread keeps serving.

use crate::engine::{
    AnnotInfo, DocId, DocInfo, Engine, FormInput, FormOutcome, Image, NewAnnot, OpenError,
    PageData, RegionReq, TileKey,
};
use crate::text::{Match, SearchOptions};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Render priorities below this are for what is on screen now and run before
/// page data; higher ones (prefetch, sidebar thumbnails) run after it.
pub const ON_SCREEN: u32 = 1000;

pub enum Request {
    /// `doc` comes from [`WorkerHandle::alloc_doc_id`].
    Open {
        doc: DocId,
        path: PathBuf,
        password: Option<String>,
    },
    Close {
        doc: DocId,
    },
    Render {
        key: TileKey,
        priority: u32,
    },
    /// Keep only these queued renders, with updated priorities.
    Retain {
        keys: Vec<(TileKey, u32)>,
    },
    Region {
        req: RegionReq,
    },
    PageData {
        doc: DocId,
        page: u32,
    },
    Search {
        doc: DocId,
        ticket: u64,
        query: String,
        opts: SearchOptions,
        start_page: u32,
    },
    CancelSearch {
        doc: DocId,
    },
    /// List every annotation in the document, page by page.
    ScanAnnots {
        doc: DocId,
        ticket: u64,
    },
    AddAnnot {
        doc: DocId,
        page: u32,
        annot: NewAnnot,
        ticket: u64,
    },
    RemoveAnnot {
        doc: DocId,
        page: u32,
        index: usize,
        name: Option<String>,
        ticket: u64,
    },
    SetAnnotText {
        doc: DocId,
        page: u32,
        index: usize,
        name: Option<String>,
        text: String,
        ticket: u64,
    },
    Save {
        doc: DocId,
        target: PathBuf,
        ticket: u64,
    },
    FormInput {
        doc: DocId,
        page: u32,
        input: FormInput,
        ticket: u64,
    },
    /// Run a closure on the engine thread (printing, export).
    Job {
        job: EngineJob,
    },
    Shutdown,
}

pub type EngineJob = Box<dyn FnOnce(&mut dyn Engine) + Send>;

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub page: u32,
    pub rects: Vec<[f32; 4]>,
    pub snippet: String,
    /// The match inside `snippet` (byte range).
    pub snippet_match: std::ops::Range<usize>,
}

#[derive(Debug)]
pub enum Response {
    Opened {
        doc: DocId,
        result: Result<Box<DocInfo>, OpenError>,
    },
    Tile {
        key: TileKey,
        result: Result<Image, String>,
    },
    Region {
        req: RegionReq,
        image: Result<Image, String>,
    },
    PageData {
        doc: DocId,
        page: u32,
        data: Result<Box<PageData>, String>,
    },
    SearchHits {
        doc: DocId,
        ticket: u64,
        hits: Vec<SearchHit>,
    },
    SearchProgress {
        doc: DocId,
        ticket: u64,
        searched: u32,
        total: u32,
        done: bool,
    },
    Annots {
        doc: DocId,
        ticket: u64,
        page: u32,
        annots: Vec<AnnotInfo>,
        done: bool,
    },
    /// An annotation edit finished; `page` must be re-rendered.
    AnnotEdited {
        doc: DocId,
        page: u32,
        ticket: u64,
        result: Result<String, String>,
    },
    Saved {
        doc: DocId,
        ticket: u64,
        target: PathBuf,
        result: Result<(), String>,
    },
    FormDone {
        doc: DocId,
        page: u32,
        ticket: u64,
        result: Result<FormOutcome, String>,
    },
    /// The engine thread hit an unexpected panic in a job and recovered.
    Fault(String),
}

/// Statistics for tests and the built-in benchmark.
#[derive(Default, Debug)]
pub struct Stats {
    pub tiles_rendered: AtomicU64,
    pub tiles_dropped: AtomicU64,
    pub render_micros: AtomicU64,
    pub jobs_panicked: AtomicU64,
    /// The engine thread has work queued (renders, search, annotation scan).
    pub busy: AtomicBool,
}

pub struct WorkerHandle {
    tx: Sender<Request>,
    pub rx: Receiver<Response>,
    pub stats: Arc<Stats>,
    next_doc: Arc<AtomicU64>,
    thread: Option<JoinHandle<()>>,
}

impl WorkerHandle {
    /// Starts the engine thread. `make_engine` runs on that thread; if it
    /// fails, the error is returned here. `notify` is called after every
    /// batch of responses (the UI uses it to request a repaint).
    pub fn spawn<F>(make_engine: F, notify: Box<dyn Fn() + Send>) -> Result<Self, String>
    where
        F: FnOnce() -> Result<Box<dyn Engine>, String> + Send + 'static,
    {
        let (tx, req_rx) = mpsc::channel::<Request>();
        let (resp_tx, rx) = mpsc::channel::<Response>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        let stats = Arc::new(Stats::default());
        let stats2 = stats.clone();
        let thread = std::thread::Builder::new()
            .name("bdf-engine".into())
            .stack_size(8 * 1024 * 1024)
            .spawn(move || {
                let engine = match catch_unwind(AssertUnwindSafe(make_engine)) {
                    Ok(Ok(e)) => {
                        let _ = ready_tx.send(Ok(()));
                        e
                    }
                    Ok(Err(e)) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                    Err(p) => {
                        let _ = ready_tx.send(Err(panic_text(&p)));
                        return;
                    }
                };
                Worker::new(engine, req_rx, resp_tx, notify, stats2).run();
            })
            .map_err(|e| format!("could not start the engine thread: {e}"))?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                tx,
                rx,
                stats,
                next_doc: Arc::new(AtomicU64::new(1)),
                thread: Some(thread),
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => Err("the engine thread exited during start-up".into()),
        }
    }

    /// Sends a request. Returns false if the engine thread is gone.
    pub fn send(&self, r: Request) -> bool {
        self.tx.send(r).is_ok()
    }

    pub fn is_alive(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }

    /// Reserves a document id; pass it back through [`Response::Opened`].
    pub fn alloc_doc_id(&self) -> DocId {
        DocId(self.next_doc.fetch_add(1, Ordering::Relaxed))
    }

    pub fn try_recv(&self) -> Option<Response> {
        self.rx.try_recv().ok()
    }
}

impl Drop for WorkerHandle {
    fn drop(&mut self) {
        let _ = self.tx.send(Request::Shutdown);
        if let Some(t) = self.thread.take() {
            // Don't hang the exit on a pathological render; give it a moment.
            let deadline = Instant::now() + Duration::from_secs(3);
            while !t.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            if t.is_finished() {
                let _ = t.join();
            }
        }
    }
}

fn panic_text(p: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".into()
    }
}

struct SearchJob {
    ticket: u64,
    query: String,
    opts: SearchOptions,
    total: u32,
    /// Pages still to search, in order (wrapping from the start page).
    queue: VecDeque<u32>,
    searched: u32,
    /// Hits sent so far; the search stops at [`MAX_SEARCH_HITS`].
    found: usize,
    last_progress: Instant,
}

/// A search stops after this many hits, so searching for "e" in a huge
/// book cannot fill memory: plenty to navigate, about 2 MB at most.
pub const MAX_SEARCH_HITS: usize = 10_000;

enum Control {
    Open {
        doc: DocId,
        path: PathBuf,
        password: Option<String>,
    },
    Close {
        doc: DocId,
    },
    AddAnnot {
        doc: DocId,
        page: u32,
        annot: NewAnnot,
        ticket: u64,
    },
    RemoveAnnot {
        doc: DocId,
        page: u32,
        index: usize,
        name: Option<String>,
        ticket: u64,
    },
    SetAnnotText {
        doc: DocId,
        page: u32,
        index: usize,
        name: Option<String>,
        text: String,
        ticket: u64,
    },
    Save {
        doc: DocId,
        target: PathBuf,
        ticket: u64,
    },
    FormInput {
        doc: DocId,
        page: u32,
        input: FormInput,
        ticket: u64,
    },
    Job {
        job: EngineJob,
    },
}

struct Worker {
    engine: Box<dyn Engine>,
    rx: Receiver<Request>,
    tx: Sender<Response>,
    notify: Box<dyn Fn() + Send>,
    stats: Arc<Stats>,
    control: VecDeque<Control>,
    region: Option<RegionReq>,
    page_data: VecDeque<(DocId, u32)>,
    /// Render queue ordered by (priority, arrival).
    queue: BTreeMap<(u32, u64), TileKey>,
    queued: HashMap<TileKey, (u32, u64)>,
    seq: u64,
    searches: HashMap<DocId, SearchJob>,
    scans: HashMap<DocId, (u64, u32, u32)>,
    open_docs: HashMap<DocId, u32>,
    shutdown: bool,
    dirty: bool,
}

impl Worker {
    fn new(
        engine: Box<dyn Engine>,
        rx: Receiver<Request>,
        tx: Sender<Response>,
        notify: Box<dyn Fn() + Send>,
        stats: Arc<Stats>,
    ) -> Self {
        Self {
            engine,
            rx,
            tx,
            notify,
            stats,
            control: VecDeque::new(),
            region: None,
            page_data: VecDeque::new(),
            queue: BTreeMap::new(),
            queued: HashMap::new(),
            seq: 0,
            searches: HashMap::new(),
            scans: HashMap::new(),
            open_docs: HashMap::new(),
            shutdown: false,
            dirty: false,
        }
    }

    fn has_work(&self) -> bool {
        !self.control.is_empty()
            || self.region.is_some()
            || !self.page_data.is_empty()
            || !self.queue.is_empty()
            || !self.searches.is_empty()
            || !self.scans.is_empty()
    }

    fn run(mut self) {
        while !self.shutdown {
            // Block when idle; otherwise just drain what has arrived.
            let busy = self.has_work();
            self.stats.busy.store(busy, Ordering::Relaxed);
            if !busy {
                match self.rx.recv_timeout(Duration::from_secs(3600)) {
                    Ok(r) => self.accept(r),
                    Err(RecvTimeoutError::Timeout) => continue,
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
            loop {
                match self.rx.try_recv() {
                    Ok(r) => self.accept(r),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.shutdown = true;
                        break;
                    }
                }
            }
            if self.shutdown {
                break;
            }
            self.step();
            if self.dirty {
                self.dirty = false;
                (self.notify)();
            }
        }
        // Close everything so PDFium releases file handles.
        let docs: Vec<DocId> = self.open_docs.keys().copied().collect();
        for d in docs {
            self.engine.close(d);
        }
    }

    fn send(&mut self, r: Response) {
        let _ = self.tx.send(r);
        self.dirty = true;
    }

    fn accept(&mut self, r: Request) {
        match r {
            Request::Open {
                doc,
                path,
                password,
            } => {
                self.control.push_back(Control::Open {
                    doc,
                    path,
                    password,
                });
            }
            Request::Close { doc } => {
                self.drop_doc_work(doc);
                self.control.push_back(Control::Close { doc });
            }
            Request::Render { key, priority } => {
                if self.queued.contains_key(&key) {
                    self.reprioritise(key, priority);
                } else {
                    self.seq += 1;
                    self.queue.insert((priority, self.seq), key);
                    self.queued.insert(key, (priority, self.seq));
                }
            }
            Request::Retain { keys } => {
                let keep: HashMap<TileKey, u32> = keys.into_iter().collect();
                let drop: Vec<TileKey> = self
                    .queued
                    .keys()
                    .filter(|k| !keep.contains_key(k))
                    .copied()
                    .collect();
                self.stats
                    .tiles_dropped
                    .fetch_add(drop.len() as u64, Ordering::Relaxed);
                for k in drop {
                    if let Some(slot) = self.queued.remove(&k) {
                        self.queue.remove(&slot);
                    }
                }
                for (k, p) in keep {
                    if self.queued.contains_key(&k) {
                        self.reprioritise(k, p);
                    }
                }
            }
            Request::Region { req } => self.region = Some(req),
            Request::PageData { doc, page } => {
                if !self.page_data.contains(&(doc, page)) {
                    self.page_data.push_back((doc, page));
                }
            }
            Request::Search {
                doc,
                ticket,
                query,
                opts,
                start_page,
            } => {
                let total = self.open_docs.get(&doc).copied().unwrap_or(0);
                let start = if total == 0 {
                    0
                } else {
                    start_page.min(total - 1)
                };
                let queue: VecDeque<u32> = (start..total).chain(0..start).collect();
                self.searches.insert(
                    doc,
                    SearchJob {
                        ticket,
                        query,
                        opts,
                        total,
                        queue,
                        searched: 0,
                        found: 0,
                        last_progress: Instant::now(),
                    },
                );
            }
            Request::CancelSearch { doc } => {
                self.searches.remove(&doc);
            }
            Request::ScanAnnots { doc, ticket } => {
                let total = self.open_docs.get(&doc).copied().unwrap_or(0);
                self.scans.insert(doc, (ticket, 0, total));
            }
            Request::AddAnnot {
                doc,
                page,
                annot,
                ticket,
            } => self.control.push_back(Control::AddAnnot {
                doc,
                page,
                annot,
                ticket,
            }),
            Request::RemoveAnnot {
                doc,
                page,
                index,
                name,
                ticket,
            } => self.control.push_back(Control::RemoveAnnot {
                doc,
                page,
                index,
                name,
                ticket,
            }),
            Request::SetAnnotText {
                doc,
                page,
                index,
                name,
                text,
                ticket,
            } => self.control.push_back(Control::SetAnnotText {
                doc,
                page,
                index,
                name,
                text,
                ticket,
            }),
            Request::Save {
                doc,
                target,
                ticket,
            } => self.control.push_back(Control::Save {
                doc,
                target,
                ticket,
            }),
            Request::FormInput {
                doc,
                page,
                input,
                ticket,
            } => self.control.push_back(Control::FormInput {
                doc,
                page,
                input,
                ticket,
            }),
            Request::Job { job } => self.control.push_back(Control::Job { job }),
            Request::Shutdown => self.shutdown = true,
        }
    }

    fn reprioritise(&mut self, key: TileKey, priority: u32) {
        if let Some(&(p, s)) = self.queued.get(&key)
            && p != priority
        {
            self.queue.remove(&(p, s));
            self.queue.insert((priority, s), key);
            self.queued.insert(key, (priority, s));
        }
    }

    fn drop_doc_work(&mut self, doc: DocId) {
        self.searches.remove(&doc);
        self.scans.remove(&doc);
        self.page_data.retain(|(d, _)| *d != doc);
        if self.region.is_some_and(|r| r.doc == doc) {
            self.region = None;
        }
        let drop: Vec<TileKey> = self
            .queued
            .keys()
            .filter(|k| k.doc == doc)
            .copied()
            .collect();
        for k in drop {
            if let Some(slot) = self.queued.remove(&k) {
                self.queue.remove(&slot);
            }
        }
    }

    /// Runs `f` and turns a panic into an error string.
    fn guarded<T>(&mut self, f: impl FnOnce(&mut dyn Engine) -> T) -> Result<T, String> {
        let engine = &mut *self.engine;
        match catch_unwind(AssertUnwindSafe(|| f(engine))) {
            Ok(v) => Ok(v),
            Err(p) => {
                self.stats.jobs_panicked.fetch_add(1, Ordering::Relaxed);
                let msg = panic_text(&p);
                log::error!("engine job panicked: {msg}");
                let _ = self.tx.send(Response::Fault(msg.clone()));
                self.dirty = true;
                Err(format!("internal error: {msg}"))
            }
        }
    }

    fn step(&mut self) {
        if let Some(c) = self.control.pop_front() {
            self.run_control(c);
            return;
        }
        if let Some(req) = self.region.take() {
            if self.open_docs.contains_key(&req.doc) {
                let image = self.guarded(|e| e.render_region(&req)).and_then(|r| r);
                self.send(Response::Region { req, image });
            }
            return;
        }
        if self
            .queue
            .keys()
            .next()
            .is_some_and(|&(priority, _)| priority < ON_SCREEN)
        {
            self.render_next();
            return;
        }
        if let Some((doc, page)) = self.page_data.pop_front() {
            if self.open_docs.contains_key(&doc) {
                let data = self
                    .guarded(|e| e.page_data(doc, page))
                    .and_then(|r| r)
                    .map(Box::new);
                self.send(Response::PageData { doc, page, data });
            }
            return;
        }
        if !self.queue.is_empty() {
            self.render_next();
        } else if !self.searches.is_empty() {
            self.search_step();
        } else {
            self.scan_step();
        }
    }

    /// Renders the most urgent queued tile or thumbnail.
    fn render_next(&mut self) {
        let Some((&slot, &key)) = self.queue.iter().next() else {
            return;
        };
        self.queue.remove(&slot);
        self.queued.remove(&key);
        if self.open_docs.contains_key(&key.doc) {
            let t0 = Instant::now();
            let result = self.guarded(|e| e.render(&key)).and_then(|r| r);
            self.stats
                .render_micros
                .fetch_add(t0.elapsed().as_micros() as u64, Ordering::Relaxed);
            self.stats.tiles_rendered.fetch_add(1, Ordering::Relaxed);
            self.send(Response::Tile { key, result });
        }
    }

    fn scan_step(&mut self) {
        let Some((&doc, &(ticket, page, total))) = self.scans.iter().next() else {
            return;
        };
        if page >= total {
            self.scans.remove(&doc);
            self.send(Response::Annots {
                doc,
                ticket,
                page,
                annots: Vec::new(),
                done: true,
            });
            return;
        }
        let annots = self
            .guarded(|e| e.annotations(doc, page))
            .and_then(|r| r)
            .unwrap_or_default();
        let done = page + 1 >= total;
        if done {
            self.scans.remove(&doc);
        } else {
            self.scans.insert(doc, (ticket, page + 1, total));
        }
        if !annots.is_empty() || done {
            self.send(Response::Annots {
                doc,
                ticket,
                page,
                annots,
                done,
            });
        }
    }

    fn run_control(&mut self, c: Control) {
        match c {
            Control::Open {
                doc,
                path,
                password,
            } => {
                let result = self
                    .guarded(|e| e.open(doc, &path, password.as_deref()))
                    .unwrap_or_else(|e| Err(OpenError::Engine(e)));
                if let Ok(info) = &result {
                    self.open_docs.insert(doc, info.page_sizes.len() as u32);
                }
                self.send(Response::Opened {
                    doc,
                    result: result.map(Box::new),
                });
            }
            Control::Close { doc } => {
                if self.open_docs.remove(&doc).is_some() {
                    let _ = self.guarded(|e| e.close(doc));
                }
            }
            Control::AddAnnot {
                doc,
                page,
                annot,
                ticket,
            } => {
                let result = self
                    .guarded(|e| e.add_annot(doc, page, &annot))
                    .and_then(|r| r);
                self.send(Response::AnnotEdited {
                    doc,
                    page,
                    ticket,
                    result,
                });
            }
            Control::RemoveAnnot {
                doc,
                page,
                index,
                name,
                ticket,
            } => {
                let result = self
                    .guarded(|e| e.remove_annot(doc, page, index, name.as_deref()))
                    .and_then(|r| r)
                    .map(|_| name.unwrap_or_default());
                self.send(Response::AnnotEdited {
                    doc,
                    page,
                    ticket,
                    result,
                });
            }
            Control::SetAnnotText {
                doc,
                page,
                index,
                name,
                text,
                ticket,
            } => {
                let result = self
                    .guarded(|e| e.set_annot_contents(doc, page, index, name.as_deref(), &text))
                    .and_then(|r| r)
                    .map(|_| name.unwrap_or_default());
                self.send(Response::AnnotEdited {
                    doc,
                    page,
                    ticket,
                    result,
                });
            }
            Control::Save {
                doc,
                target,
                ticket,
            } => {
                let result = self.guarded(|e| e.save(doc, &target)).and_then(|r| r);
                self.send(Response::Saved {
                    doc,
                    ticket,
                    target,
                    result,
                });
            }
            Control::FormInput {
                doc,
                page,
                input,
                ticket,
            } => {
                let result = self
                    .guarded(|e| e.form_input(doc, page, &input))
                    .and_then(|r| r);
                self.send(Response::FormDone {
                    doc,
                    page,
                    ticket,
                    result,
                });
            }
            Control::Job { job } => {
                let _ = self.guarded(|e| job(e));
            }
        }
    }

    fn search_step(&mut self) {
        let Some(&doc) = self.searches.keys().next() else {
            return;
        };
        let mut job = self.searches.remove(&doc).expect("present");
        let Some(page) = job.queue.pop_front() else {
            self.send(Response::SearchProgress {
                doc,
                ticket: job.ticket,
                searched: job.searched,
                total: job.total,
                done: true,
            });
            return;
        };
        let text = self.guarded(|e| e.page_text(doc, page)).and_then(|r| r);
        job.searched += 1;
        if let Ok(text) = text {
            let matches: Vec<Match> = text.search(&job.query, job.opts);
            let room = MAX_SEARCH_HITS - job.found;
            if !matches.is_empty() {
                let hits: Vec<SearchHit> = matches
                    .iter()
                    .take(room)
                    .map(|m| {
                        let (snippet, snippet_match) = text.snippet(*m, 40);
                        SearchHit {
                            page,
                            rects: text.rects(m.start, m.end),
                            snippet,
                            snippet_match,
                        }
                    })
                    .collect();
                job.found += hits.len();
                self.send(Response::SearchHits {
                    doc,
                    ticket: job.ticket,
                    hits,
                });
            }
        }
        let done = job.queue.is_empty() || job.found >= MAX_SEARCH_HITS;
        if done || job.last_progress.elapsed() > Duration::from_millis(100) {
            job.last_progress = Instant::now();
            self.send(Response::SearchProgress {
                doc,
                ticket: job.ticket,
                searched: job.searched,
                total: job.total,
                done,
            });
        }
        if !done {
            self.searches.insert(doc, job);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{EResult, TileKind};
    use crate::geom::Rotation;
    use crate::text::{PageText, TextChar};
    use std::path::Path;
    use std::sync::Mutex;

    /// An engine that records what it was asked to do.
    struct Fake {
        log: Arc<Mutex<Vec<String>>>,
        pages: u32,
    }

    impl Engine for Fake {
        fn open(
            &mut self,
            _id: DocId,
            path: &Path,
            _pw: Option<&str>,
        ) -> Result<DocInfo, OpenError> {
            if path.to_string_lossy().contains("missing") {
                return Err(OpenError::NotFound);
            }
            Ok(DocInfo {
                path: path.into(),
                file_size: 1,
                page_sizes: vec![(612.0, 792.0); self.pages as usize],
                page_labels: vec![],
                outline: vec![],
                metadata: vec![],
                pdf_version: None,
                encrypted: false,
                fingerprint: "fake".into(),
                form_fields: false,
            })
        }
        fn close(&mut self, id: DocId) {
            self.log.lock().unwrap().push(format!("close {}", id.0));
        }
        fn render(&mut self, key: &TileKey) -> EResult<Image> {
            if key.page == 666 {
                panic!("boom");
            }
            self.log
                .lock()
                .unwrap()
                .push(format!("render {}", key.page));
            Ok(Image {
                width: 1,
                height: 1,
                rgba: vec![0; 4],
            })
        }
        fn render_region(&mut self, _r: &RegionReq) -> EResult<Image> {
            self.log.lock().unwrap().push("region".into());
            Ok(Image {
                width: 1,
                height: 1,
                rgba: vec![0; 4],
            })
        }
        fn render_page(&mut self, _: DocId, _: u32, w: u32, h: u32, _: bool) -> EResult<Image> {
            Ok(Image {
                width: w,
                height: h,
                rgba: vec![0; (w * h * 4) as usize],
            })
        }
        fn page_size(&self, _: DocId, _: u32) -> Option<(f32, f32)> {
            Some((612.0, 792.0))
        }
        fn page_data(&mut self, _: DocId, page: u32) -> EResult<PageData> {
            self.log.lock().unwrap().push(format!("data {page}"));
            Ok(PageData::default())
        }
        fn annotations(&mut self, _: DocId, page: u32) -> EResult<Vec<AnnotInfo>> {
            Ok(if page == 2 {
                vec![AnnotInfo {
                    index: 0,
                    name: None,
                    kind: crate::engine::AnnotKind::Note,
                    rect: [0.0; 4],
                    contents: "hi".into(),
                    author: String::new(),
                    color: None,
                }]
            } else {
                vec![]
            })
        }
        fn page_text(&mut self, _: DocId, page: u32) -> EResult<PageText> {
            let s = if page.is_multiple_of(3) {
                "a needle here"
            } else {
                "hay"
            };
            Ok(PageText {
                chars: s
                    .chars()
                    .enumerate()
                    .map(|(i, ch)| TextChar {
                        ch,
                        rect: Some([i as f32, 0.0, i as f32 + 1.0, 1.0]),
                        hidden: false,
                    })
                    .collect(),
            })
        }
        fn add_annot(&mut self, _: DocId, _: u32, _: &NewAnnot) -> EResult<String> {
            Ok("nm".into())
        }
        fn remove_annot(&mut self, _: DocId, _: u32, _: usize, _: Option<&str>) -> EResult<()> {
            Err("nope".into())
        }
        fn set_annot_contents(
            &mut self,
            _: DocId,
            _: u32,
            _: usize,
            _: Option<&str>,
            _: &str,
        ) -> EResult<()> {
            Ok(())
        }
        fn save(&mut self, _: DocId, _: &Path) -> EResult<()> {
            Ok(())
        }
        fn form_input(&mut self, _: DocId, _: u32, _: &FormInput) -> EResult<FormOutcome> {
            Ok(FormOutcome::default())
        }
    }

    fn start(pages: u32) -> (WorkerHandle, Arc<Mutex<Vec<String>>>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let l2 = log.clone();
        let w = WorkerHandle::spawn(
            move || Ok(Box::new(Fake { log: l2, pages }) as Box<dyn Engine>),
            Box::new(|| {}),
        )
        .unwrap();
        (w, log)
    }

    fn key(doc: DocId, page: u32) -> TileKey {
        TileKey {
            doc,
            page,
            rotation: Rotation::R0,
            invert: false,
            generation: 0,
            kind: TileKind::Thumb,
        }
    }

    fn recv(w: &WorkerHandle) -> Response {
        w.rx.recv_timeout(Duration::from_secs(5))
            .expect("worker response")
    }

    fn open(w: &WorkerHandle) -> DocId {
        let doc = w.alloc_doc_id();
        w.send(Request::Open {
            doc,
            path: "x.pdf".into(),
            password: None,
        });
        match recv(w) {
            Response::Opened {
                doc: d,
                result: Ok(_),
            } => assert_eq!(d, doc),
            other => panic!("unexpected {other:?}"),
        }
        doc
    }

    /// Holds the engine thread inside a job until the returned sender fires.
    fn pause(w: &WorkerHandle) -> mpsc::Sender<()> {
        let (tx, rx) = mpsc::channel::<()>();
        let (started_tx, started_rx) = mpsc::channel::<()>();
        w.send(Request::Job {
            job: Box::new(move |_| {
                let _ = started_tx.send(());
                let _ = rx.recv_timeout(Duration::from_secs(5));
            }),
        });
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        tx
    }

    #[test]
    fn renders_in_priority_order_and_retain_drops() {
        let (w, log) = start(100);
        let doc = open(&w);
        let gate = pause(&w);
        for (page, prio) in [(1, 50), (2, 10), (3, 30), (4, 20), (5, 40)] {
            w.send(Request::Render {
                key: key(doc, page),
                priority: prio,
            });
        }
        // Page 5 no longer wanted; page 1 becomes urgent.
        w.send(Request::Retain {
            keys: vec![
                (key(doc, 1), 0),
                (key(doc, 2), 10),
                (key(doc, 3), 30),
                (key(doc, 4), 20),
            ],
        });
        gate.send(()).unwrap();
        let mut order = Vec::new();
        for _ in 0..4 {
            if let Response::Tile { key, result } = recv(&w) {
                assert!(result.is_ok());
                order.push(key.page);
            }
        }
        assert_eq!(order, vec![1, 2, 4, 3]);
        assert!(
            w.rx.recv_timeout(Duration::from_millis(100)).is_err(),
            "page 5 must be dropped"
        );
        assert_eq!(w.stats.tiles_dropped.load(Ordering::Relaxed), 1);
        assert!(!log.lock().unwrap().contains(&"render 5".to_string()));
    }

    #[test]
    fn duplicate_requests_render_once() {
        let (w, _log) = start(10);
        let doc = open(&w);
        let gate = pause(&w);
        for _ in 0..5 {
            w.send(Request::Render {
                key: key(doc, 2),
                priority: 1,
            });
        }
        gate.send(()).unwrap();
        assert!(matches!(recv(&w), Response::Tile { .. }));
        assert!(w.rx.recv_timeout(Duration::from_millis(100)).is_err());
    }

    #[test]
    fn panics_are_contained() {
        let (w, _log) = start(1000);
        let doc = open(&w);
        w.send(Request::Render {
            key: key(doc, 666),
            priority: 0,
        });
        let mut saw_fault = false;
        let mut saw_err = false;
        for _ in 0..2 {
            match recv(&w) {
                Response::Fault(m) => saw_fault = m.contains("boom"),
                Response::Tile { result: Err(e), .. } => saw_err = e.contains("boom"),
                other => panic!("unexpected {other:?}"),
            }
        }
        assert!(saw_fault && saw_err);
        // Still alive and serving.
        w.send(Request::Render {
            key: key(doc, 1),
            priority: 0,
        });
        assert!(matches!(recv(&w), Response::Tile { result: Ok(_), .. }));
        assert_eq!(w.stats.jobs_panicked.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn search_wraps_from_the_start_page_and_finishes() {
        let (w, _log) = start(10);
        let doc = open(&w);
        w.send(Request::Search {
            doc,
            ticket: 7,
            query: "NEEDLE".into(),
            opts: SearchOptions::default(),
            start_page: 4,
        });
        let mut pages = Vec::new();
        loop {
            match recv(&w) {
                Response::SearchHits { ticket, hits, .. } => {
                    assert_eq!(ticket, 7);
                    pages.extend(hits.iter().map(|h| h.page));
                    assert!(hits[0].snippet.contains("needle"));
                    assert_eq!(hits[0].rects.len(), 1);
                }
                Response::SearchProgress {
                    done: true,
                    searched,
                    total,
                    ..
                } => {
                    assert_eq!((searched, total), (10, 10));
                    break;
                }
                Response::SearchProgress { .. } => {}
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(pages, vec![6, 9, 0, 3]);
    }

    #[test]
    fn search_stops_at_the_hit_limit() {
        // One hit on every third page: 3 * MAX + 30 pages hold MAX + 10 hits.
        let pages = 3 * MAX_SEARCH_HITS as u32 + 30;
        let (w, _log) = start(pages);
        let doc = open(&w);
        w.send(Request::Search {
            doc,
            ticket: 1,
            query: "needle".into(),
            opts: SearchOptions::default(),
            start_page: 0,
        });
        let mut found = 0;
        loop {
            match recv(&w) {
                Response::SearchHits { hits, .. } => found += hits.len(),
                Response::SearchProgress {
                    done: true,
                    searched,
                    ..
                } => {
                    assert!(searched < pages, "stopped early, not after every page");
                    break;
                }
                Response::SearchProgress { .. } => {}
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(found, MAX_SEARCH_HITS);
    }

    #[test]
    fn rendering_preempts_search() {
        let (w, log) = start(5000);
        let doc = open(&w);
        let gate = pause(&w);
        w.send(Request::Search {
            doc,
            ticket: 1,
            query: "zzz".into(),
            opts: SearchOptions::default(),
            start_page: 0,
        });
        // On-screen renders come first, then page data, then off-screen renders.
        w.send(Request::Render {
            key: key(doc, 4),
            priority: ON_SCREEN,
        });
        w.send(Request::PageData { doc, page: 3 });
        w.send(Request::Render {
            key: key(doc, 3),
            priority: 0,
        });
        gate.send(()).unwrap();
        loop {
            if let Response::Tile { key, .. } = recv(&w)
                && key.page == 4
            {
                break;
            }
        }
        let l = log.lock().unwrap();
        let first: Vec<&String> = l.iter().take(3).collect();
        assert_eq!(first, vec!["render 3", "data 3", "render 4"]);
        drop(l);
        w.send(Request::CancelSearch { doc });
    }

    #[test]
    fn close_drops_pending_work_and_errors_are_reported() {
        let (w, log) = start(10);
        let doc = open(&w);
        let gate = pause(&w);
        w.send(Request::Render {
            key: key(doc, 1),
            priority: 0,
        });
        w.send(Request::Close { doc });
        gate.send(()).unwrap();
        // A missing file reports NotFound.
        let d2 = w.alloc_doc_id();
        w.send(Request::Open {
            doc: d2,
            path: "missing.pdf".into(),
            password: None,
        });
        match recv(&w) {
            Response::Opened {
                result: Err(OpenError::NotFound),
                ..
            } => {}
            other => panic!("unexpected {other:?}"),
        }
        assert!(log.lock().unwrap().iter().all(|l| l != "render 1"));
        assert!(log.lock().unwrap().contains(&format!("close {}", doc.0)));
        // Requests for a closed document are ignored, not crashes.
        w.send(Request::Render {
            key: key(doc, 2),
            priority: 0,
        });
        w.send(Request::RemoveAnnot {
            doc: d2,
            page: 0,
            index: 0,
            name: None,
            ticket: 1,
        });
        match recv(&w) {
            Response::AnnotEdited { result: Err(e), .. } => assert_eq!(e, "nope"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn only_the_latest_region_is_rendered() {
        let (w, log) = start(10);
        let doc = open(&w);
        let gate = pause(&w);
        for x in 0..5 {
            w.send(Request::Region {
                req: RegionReq {
                    doc,
                    page: 0,
                    rotation: Rotation::R0,
                    invert: false,
                    scale: 4096,
                    x,
                    y: 0,
                    w: 10,
                    h: 10,
                },
            });
        }
        gate.send(()).unwrap();
        match recv(&w) {
            Response::Region { req, .. } => assert_eq!(req.x, 4),
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(
            log.lock()
                .unwrap()
                .iter()
                .filter(|l| *l == "region")
                .count(),
            1
        );
    }

    #[test]
    fn annotation_scan_streams_pages_and_finishes() {
        let (w, _log) = start(5);
        let doc = open(&w);
        w.send(Request::ScanAnnots { doc, ticket: 3 });
        let mut pages = Vec::new();
        loop {
            match recv(&w) {
                Response::Annots {
                    ticket,
                    page,
                    annots,
                    done,
                    ..
                } => {
                    assert_eq!(ticket, 3);
                    if !annots.is_empty() {
                        pages.push(page);
                    }
                    if done {
                        break;
                    }
                }
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(pages, vec![2]);
        // Nothing left to do: the benchmark's idle measurement waits for this.
        let t0 = Instant::now();
        while w.stats.busy.load(Ordering::Relaxed) {
            assert!(t0.elapsed() < Duration::from_secs(5), "worker stays busy");
            std::thread::yield_now();
        }
    }

    #[test]
    fn failing_engine_start_is_reported() {
        let r = WorkerHandle::spawn(|| Err("no pdfium".to_string()), Box::new(|| {}));
        assert_eq!(r.err().as_deref(), Some("no pdfium"));
    }
}
