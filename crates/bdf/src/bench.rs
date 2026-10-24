//! `bdf --bench`: drives the real app through a scripted session and writes
//! a JSON report (start-up, first render, frame times, memory, CPU at idle,
//! search). CI runs it on every platform and fails on regressions.

use crate::app::BdfApp;
use crate::view::ZoomMode;
use eframe::egui::{Context, ViewportCommand, vec2};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long the idle CPU is measured.
const IDLE_WINDOW: Duration = Duration::from_secs(10);
/// Quiet time before the idle measurement starts, so the last frames of the
/// run (and their painting) are not counted.
const IDLE_LEAD_IN: Duration = Duration::from_secs(2);
/// How long to wait for outstanding images before reporting them and moving on.
const IMAGE_WAIT: Duration = Duration::from_secs(15);

#[derive(Debug)]
enum Phase {
    WaitOpen,
    WaitFirstRender,
    Scroll(u32),
    Zoom(u32),
    Split(u32),
    Search(Instant),
    Settle(u32),
    /// Idle CPU is measured on another thread, so the measurement itself
    /// causes no frames; it fills this in (`Some(None)` if unsupported).
    Idle(Arc<Mutex<Option<Option<f64>>>>),
    Done,
}

impl Phase {
    fn name(&self) -> &'static str {
        match self {
            Phase::WaitOpen => "open",
            Phase::WaitFirstRender => "first_render",
            Phase::Scroll(_) => "scroll",
            Phase::Zoom(_) => "zoom",
            Phase::Split(_) => "split",
            Phase::Search(_) => "search",
            Phase::Settle(_) => "settle",
            Phase::Idle(_) => "idle",
            Phase::Done => "done",
        }
    }
}

pub struct Bench {
    pub file: PathBuf,
    report: PathBuf,
    launched: Instant,
    phase: Phase,
    phase_frames: Vec<f32>,
    scroll_frames: Vec<f32>,
    open_ms: Option<f64>,
    /// Process start to the first page image on screen (the end of white pages).
    first_image_ms: Option<f64>,
    first_render_ms: Option<f64>,
    search_ms: Option<f64>,
    search_hits: usize,
    idle_cpu_percent: Option<f64>,
    pages: usize,
    idle_frames: u32,
    deadline: Instant,
    /// Since when the script has been waiting for outstanding images.
    waiting_since: Option<Instant>,
    /// Wall time spent in each phase, to see where a slow run went.
    phase_seconds: Vec<(&'static str, f64)>,
    phase_started: Instant,
    failures: Vec<String>,
}

/// With `BDF_BENCH_SHOTS=<dir>` on X11, saves a screenshot at each stage
/// (uses ImageMagick's `import`); used to review the UI in CI artifacts.
fn shot(name: &str) {
    if let Some(dir) = std::env::var_os("BDF_BENCH_SHOTS") {
        let path = std::path::Path::new(&dir).join(format!("{name}.png"));
        let _ = std::process::Command::new("import")
            .args(["-window", "root"])
            .arg(path)
            .status();
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

impl Bench {
    pub fn new(file: PathBuf, report: PathBuf, launched: Instant) -> Self {
        Self {
            file,
            report,
            launched,
            phase: Phase::WaitOpen,
            phase_frames: Vec::new(),
            scroll_frames: Vec::new(),
            open_ms: None,
            first_image_ms: None,
            first_render_ms: None,
            search_ms: None,
            search_hits: 0,
            idle_cpu_percent: None,
            pages: 0,
            idle_frames: 0,
            // Generous: runners without a GPU draw each frame in software
            // (Windows' Basic Render Driver takes ~0.4 s a frame).
            deadline: Instant::now() + Duration::from_secs(600),
            waiting_since: None,
            phase_seconds: Vec::new(),
            phase_started: Instant::now(),
            failures: Vec::new(),
        }
    }

    /// True once no images are outstanding. Images that never arrive are
    /// reported as a failure after [`IMAGE_WAIT`] instead of stalling the run.
    fn images_settled(&mut self, app: &BdfApp) -> bool {
        let pending = app.tiles.pending_keys();
        if pending.is_empty() {
            self.waiting_since = None;
            return true;
        }
        let since = *self.waiting_since.get_or_insert_with(Instant::now);
        if since.elapsed() < IMAGE_WAIT {
            return false;
        }
        self.failures.push(format!(
            "{} image(s) never arrived during {:?}, e.g. {:?}",
            pending.len(),
            self.phase,
            &pending[..pending.len().min(3)]
        ));
        self.waiting_since = None;
        true
    }

    /// Advances the script; returns false once finished.
    pub fn tick(&mut self, app: &mut BdfApp, ctx: &Context) -> bool {
        let before = self.phase.name();
        let keep = self.step(app, ctx);
        if self.phase.name() != before {
            let secs = self.phase_started.elapsed().as_secs_f64();
            self.phase_seconds.push((before, secs));
            self.phase_started = Instant::now();
        }
        if matches!(self.phase, Phase::Done) {
            self.write(app);
            app.force_close(ctx);
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return false;
        }
        keep
    }

    fn step(&mut self, app: &mut BdfApp, ctx: &Context) -> bool {
        if Instant::now() > self.deadline && !matches!(self.phase, Phase::Done) {
            let search = app
                .active_tab()
                .and_then(|t| app.docs.get(&t.doc))
                .and_then(|d| d.search.as_ref())
                .map(|s| (s.done, s.hits.len()));
            self.failures.push(format!(
                "timed out in {:?}: {} image(s) pending, search (done, hits) {:?}",
                self.phase,
                app.tiles.pending(),
                search
            ));
            self.phase = Phase::Done;
        }
        if let Some(e) = &app.engine_error {
            self.failures.push(format!("engine: {e}"));
            self.phase = Phase::Done;
        }
        let last_frame = app.perf.frame_ms.back().copied();
        match self.phase {
            Phase::WaitOpen => {
                if let Some(t) = app.active_tab() {
                    self.open_ms = Some(ms(self.launched.elapsed()));
                    self.pages = app.docs.get(&t.doc).map(|d| d.page_count()).unwrap_or(0);
                    self.phase = Phase::WaitFirstRender;
                } else if app.dialog.is_some() {
                    self.failures.push("the document did not open".into());
                    self.phase = Phase::Done;
                }
            }
            Phase::WaitFirstRender => {
                if self.first_image_ms.is_none() && app.tiles.len() > 0 {
                    self.first_image_ms = Some(ms(self.launched.elapsed()));
                }
                if app.tiles.len() > 0 && app.tiles.pending() == 0 {
                    self.first_render_ms = Some(ms(self.launched.elapsed()));
                    shot("1-first-page");
                    self.phase = Phase::Scroll(0);
                }
            }
            Phase::Scroll(n) => {
                if n > 0 {
                    self.scroll_frames.extend(last_frame);
                }
                if let Some(v) = app.active_view_mut() {
                    let step = v.viewport.y * 0.35;
                    v.scroll_by(vec2(0.0, step));
                }
                self.phase = if n >= 300 {
                    Phase::Zoom(0)
                } else {
                    Phase::Scroll(n + 1)
                };
            }
            Phase::Zoom(n) => {
                self.phase_frames.extend(last_frame);
                if let Some(v) = app.active_view_mut() {
                    let z = 0.6 + (n as f32 * 0.15).sin().abs() * 2.4;
                    v.set_zoom(z, None, ZoomMode::Custom);
                }
                if n == 59 {
                    shot("2-zoomed");
                }
                if n >= 60 {
                    if let Some(v) = app.active_view_mut() {
                        v.fit(ZoomMode::FitWidth);
                    }
                    app.toggle_split(false);
                    self.phase = Phase::Split(0);
                } else {
                    self.phase = Phase::Zoom(n + 1);
                }
            }
            Phase::Split(n) => {
                self.phase_frames.extend(last_frame);
                for p in &mut app.panes {
                    if let Some(t) = p.current_mut() {
                        t.view.scroll_by(vec2(0.0, 220.0));
                    }
                }
                if n == 79 {
                    shot("3-split");
                }
                if n >= 80 {
                    app.toggle_split(false);
                    let target = (self.pages / 2).max(1);
                    app.find.open = true;
                    app.find.query = format!("PAGE-{target}");
                    app.run_search();
                    self.phase = Phase::Search(Instant::now());
                } else {
                    self.phase = Phase::Split(n + 1);
                }
            }
            Phase::Search(t0) => {
                let done = app
                    .active_tab()
                    .and_then(|t| app.docs.get(&t.doc))
                    .and_then(|d| d.search.as_ref())
                    .map(|s| (s.done, s.hits.len()));
                if let Some((true, hits)) = done {
                    if !self.images_settled(app) {
                        ctx.request_repaint();
                        return true;
                    }
                    shot("4-search");
                    self.search_ms = Some(ms(t0.elapsed()));
                    self.search_hits = hits;
                    app.close_find();
                    if let Some(v) = app.active_view_mut() {
                        v.goto(0, false);
                    }
                    self.phase = Phase::Settle(0);
                }
            }
            Phase::Settle(n) => {
                // Let rendering finish before measuring idle CPU.
                if n > 30 && self.images_settled(app) {
                    let result = Arc::new(Mutex::new(None));
                    let (out, ctx) = (result.clone(), ctx.clone());
                    std::thread::spawn(move || {
                        std::thread::sleep(IDLE_LEAD_IN);
                        let t0 = Instant::now();
                        let cpu0 = crate::sysinfo::sample().cpu_secs;
                        std::thread::sleep(IDLE_WINDOW);
                        let cpu1 = crate::sysinfo::sample().cpu_secs;
                        let pct = cpu0
                            .zip(cpu1)
                            .map(|(a, b)| (b - a) / t0.elapsed().as_secs_f64() * 100.0);
                        *out.lock().unwrap_or_else(|e| e.into_inner()) = Some(pct);
                        ctx.request_repaint();
                    });
                    self.phase = Phase::Idle(result);
                    return true;
                }
                self.phase = Phase::Settle(n + 1);
            }
            Phase::Idle(ref result) => {
                // Any frame here is a repaint while nothing happens.
                self.idle_frames += 1;
                if self.idle_frames.is_multiple_of(50) {
                    log::warn!("still repainting while idle: {:?}", ctx.repaint_causes());
                }
                let done = *result.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(pct) = done {
                    self.idle_cpu_percent = pct;
                    self.phase = Phase::Done;
                }
                return true;
            }
            Phase::Done => {}
        }
        if !matches!(self.phase, Phase::Idle(..) | Phase::Done) {
            ctx.request_repaint();
        }
        true
    }

    fn write(&self, app: &BdfApp) {
        let stats = crate::sysinfo::sample();
        let pct = |v: &[f32], q: f64| -> f64 {
            if v.is_empty() {
                return 0.0;
            }
            let mut s = v.to_vec();
            s.sort_by(f32::total_cmp);
            s[((s.len() - 1) as f64 * q).round() as usize] as f64
        };
        let (rendered, render_us, dropped) = app
            .worker
            .as_ref()
            .map(|w| {
                use std::sync::atomic::Ordering::Relaxed;
                (
                    w.stats.tiles_rendered.load(Relaxed),
                    w.stats.render_micros.load(Relaxed),
                    w.stats.tiles_dropped.load(Relaxed),
                )
            })
            .unwrap_or_default();
        let mut all = self.scroll_frames.clone();
        all.extend(&self.phase_frames);
        let report = serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "os": std::env::consts::OS,
            "renderer": app.renderer,
            "file": self.file.display().to_string(),
            "pages": self.pages,
            "open_ms": self.open_ms,
            "first_image_ms": self.first_image_ms,
            "first_render_ms": self.first_render_ms,
            "scroll_frames": self.scroll_frames.len(),
            "frame_ms_p50": pct(&all, 0.5),
            "frame_ms_p95": pct(&all, 0.95),
            "frame_ms_max": pct(&all, 1.0),
            "scroll_frame_ms_p95": pct(&self.scroll_frames, 0.95),
            "tiles_rendered": rendered,
            "tiles_dropped": dropped,
            "avg_tile_ms": render_us as f64 / 1000.0 / rendered.max(1) as f64,
            "search_ms": self.search_ms,
            "search_hits": self.search_hits,
            "idle_cpu_percent": self.idle_cpu_percent,
            "idle_frames": self.idle_frames,
            "rss_mb": stats.rss_bytes.map(|b| b as f64 / 1_048_576.0),
            "peak_rss_mb": stats.peak_rss_bytes.map(|b| b as f64 / 1_048_576.0),
            "cpu_seconds": stats.cpu_secs,
            "texture_mb": app.tiles.used_bytes() as f64 / 1_048_576.0,
            "phase_seconds": self
                .phase_seconds
                .iter()
                .map(|(name, s)| ((*name).to_string(), serde_json::json!(s)))
                .collect::<serde_json::Map<_, _>>(),
            "failures": self.failures,
        });
        let text = serde_json::to_string_pretty(&report).unwrap_or_default();
        println!("{text}");
        if let Err(e) = std::fs::write(&self.report, &text) {
            eprintln!("could not write {}: {e}", self.report.display());
        }
    }
}
