//! `bdf --bench`: drives the real app through a scripted session and writes
//! a JSON report (start-up, first render, frame times, memory, CPU at idle,
//! search). CI runs it on every platform and fails on regressions.

use crate::app::BdfApp;
use crate::view::ZoomMode;
use eframe::egui::{Context, ViewportCommand, vec2};
use std::path::PathBuf;
use std::time::{Duration, Instant};

enum Phase {
    WaitOpen,
    WaitFirstRender,
    Scroll(u32),
    Zoom(u32),
    Split(u32),
    Search(Instant),
    Settle(u32),
    Idle(Instant, f64),
    Done,
}

pub struct Bench {
    pub file: PathBuf,
    report: PathBuf,
    launched: Instant,
    phase: Phase,
    phase_frames: Vec<f32>,
    scroll_frames: Vec<f32>,
    open_ms: Option<f64>,
    first_render_ms: Option<f64>,
    search_ms: Option<f64>,
    search_hits: usize,
    idle_cpu_percent: Option<f64>,
    pages: usize,
    idle_frames: u32,
    deadline: Instant,
    failures: Vec<String>,
}

/// With `BDF_BENCH_SHOTS=<dir>` on X11, saves a screenshot at each stage
/// (uses ImageMagick's `import`); used to review the UI in CI artifacts.
fn shot(name: &str) {
    if let Some(dir) = std::env::var_os("BDF_BENCH_SHOTS") {
        let path = std::path::Path::new(&dir).join(format!("{name}.png"));
        let _ = std::process::Command::new("import").args(["-window", "root"]).arg(path).status();
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
            first_render_ms: None,
            search_ms: None,
            search_hits: 0,
            idle_cpu_percent: None,
            pages: 0,
            idle_frames: 0,
            deadline: Instant::now() + Duration::from_secs(180),
            failures: Vec::new(),
        }
    }

    /// Advances the script; returns false once finished.
    pub fn tick(&mut self, app: &mut BdfApp, ctx: &Context) -> bool {
        if Instant::now() > self.deadline && !matches!(self.phase, Phase::Done) {
            self.failures.push("timed out".into());
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
                self.phase = if n >= 300 { Phase::Zoom(0) } else { Phase::Scroll(n + 1) };
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
                    if app.tiles.pending() > 0 {
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
                if app.tiles.pending() == 0 && n > 30 {
                    let cpu = crate::sysinfo::sample().cpu_secs.unwrap_or(0.0);
                    self.phase = Phase::Idle(Instant::now(), cpu);
                    // Come back after the idle window without forcing frames in between.
                    ctx.request_repaint_after(Duration::from_secs(3));
                    return true;
                }
                self.phase = Phase::Settle(n + 1);
            }
            Phase::Idle(t0, cpu0) => {
                self.idle_frames += 1;
                if self.idle_frames % 50 == 0 {
                    log::warn!("still repainting while idle: {:?}", ctx.repaint_causes());
                }
                if t0.elapsed() < Duration::from_secs(3) {
                    ctx.request_repaint_after(Duration::from_secs(3) - t0.elapsed());
                    return true;
                }
                let cpu = crate::sysinfo::sample().cpu_secs.unwrap_or(cpu0);
                self.idle_cpu_percent = Some((cpu - cpu0) / t0.elapsed().as_secs_f64() * 100.0);
                self.phase = Phase::Done;
            }
            Phase::Done => {}
        }
        if matches!(self.phase, Phase::Done) {
            self.write(app);
            app.force_close(ctx);
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return false;
        }
        if !matches!(self.phase, Phase::Idle(..)) {
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
                (w.stats.tiles_rendered.load(Relaxed), w.stats.render_micros.load(Relaxed), w.stats.tiles_dropped.load(Relaxed))
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
            "failures": self.failures,
        });
        let text = serde_json::to_string_pretty(&report).unwrap_or_default();
        println!("{text}");
        if let Err(e) = std::fs::write(&self.report, &text) {
            eprintln!("could not write {}: {e}", self.report.display());
        }
    }
}
