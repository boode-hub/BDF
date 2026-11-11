//! BDF: a fast, portable PDF reader.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod bench;
mod chrome;
mod dialogs;
mod doc;
mod icon_pixels;
mod logger;
mod platform;
mod print;
mod sidebar;
mod sysinfo;
mod theme;
mod tiles;
mod updater;
mod view;

use bdf_core::settings::{DataDir, RendererPref};
use eframe::egui;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

const USAGE: &str = "\
BDF — a fast, portable PDF reader

Usage: bdf [options] [file.pdf ...]

Options:
  -p, --page N           Open the first file at page N
      --renderer gl|wgpu Choose the graphics backend
      --bench [file]     Run the built-in benchmark (generates a test file if none given)
      --report path      Where --bench writes its JSON report (default bdf-bench.json)
      --generate N path  Write an N-page test PDF and exit
      --generate-form path
                         Write a one-page test form and exit
      --list-annotations file
                         Print the annotations PDFium sees in a file and exit
      --check-update     Ask GitHub for the latest version (installs nothing)
      --install-update   Download, verify and install the latest version
      --self-test        Load the PDF engine, open and draw a test page, and exit
  -h, --help             Show this help
  -V, --version          Show the version
";

#[derive(Default)]
struct Args {
    files: Vec<PathBuf>,
    page: Option<usize>,
    renderer: Option<RendererPref>,
    bench: Option<Option<PathBuf>>,
    report: Option<PathBuf>,
    generate: Option<(usize, PathBuf)>,
    list_annotations: Option<PathBuf>,
    generate_form: Option<PathBuf>,
    check_update: bool,
    install_update: bool,
    self_test: bool,
    help: bool,
    version: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = std::env::args_os().skip(1).peekable();
    while let Some(arg) = it.next() {
        let s = arg.to_string_lossy().into_owned();
        match s.as_str() {
            "-h" | "--help" => a.help = true,
            "-V" | "--version" => a.version = true,
            "--check-update" => a.check_update = true,
            "--install-update" => a.install_update = true,
            "--self-test" => a.self_test = true,
            "-p" | "--page" => {
                let n = it.next().ok_or("--page needs a number")?;
                a.page = Some(
                    n.to_string_lossy()
                        .parse()
                        .map_err(|_| "--page needs a number")?,
                );
            }
            "--renderer" => {
                let r = it.next().ok_or("--renderer needs gl or wgpu")?;
                a.renderer = Some(match r.to_string_lossy().as_ref() {
                    "gl" | "opengl" | "glow" => RendererPref::OpenGl,
                    "wgpu" | "dx" | "vulkan" | "metal" => RendererPref::Wgpu,
                    other => return Err(format!("unknown renderer '{other}'")),
                });
            }
            "--bench" => {
                let file = match it.peek() {
                    Some(n) if !n.to_string_lossy().starts_with('-') => {
                        it.next().map(PathBuf::from)
                    }
                    _ => None,
                };
                a.bench = Some(file);
            }
            "--list-annotations" => {
                a.list_annotations =
                    Some(it.next().ok_or("--list-annotations needs a file")?.into())
            }
            "--generate-form" => {
                a.generate_form = Some(it.next().ok_or("--generate-form needs a path")?.into())
            }
            "--report" => a.report = Some(it.next().ok_or("--report needs a path")?.into()),
            "--generate" => {
                let n = it.next().ok_or("--generate needs a page count")?;
                let n: usize = n
                    .to_string_lossy()
                    .parse()
                    .map_err(|_| "--generate needs a page count")?;
                let p = it.next().ok_or("--generate needs an output path")?;
                a.generate = Some((n, p.into()));
            }
            _ if s.starts_with('-') && s.len() > 1 => return Err(format!("unknown option '{s}'")),
            _ => a.files.push(PathBuf::from(arg)),
        }
    }
    Ok(a)
}

fn main() -> ExitCode {
    let launched = Instant::now();
    updater::remember_program();
    #[cfg(bdf_embedded_pdfium)]
    let _ = bdf_core::pdfium::EMBEDDED.set(include_bytes!(env!("BDF_EMBEDDED_PDFIUM")));
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            platform::attach_console();
            eprintln!("bdf: {e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    if args.help
        || args.version
        || args.generate.is_some()
        || args.bench.is_some()
        || args.list_annotations.is_some()
        || args.generate_form.is_some()
        || args.check_update
        || args.install_update
        || args.self_test
    {
        platform::attach_console();
    }
    if args.help {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    if args.version {
        println!("BDF {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    if let Some((n, path)) = &args.generate {
        return match std::fs::write(path, bdf_core::testpdf::long_document((*n).max(1))) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("bdf: {e}");
                ExitCode::FAILURE
            }
        };
    }

    if let Some(path) = &args.generate_form {
        return match std::fs::write(path, bdf_core::testpdf::form_document()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("bdf: {e}");
                ExitCode::FAILURE
            }
        };
    }
    if let Some(path) = &args.list_annotations {
        return list_annotations(path);
    }
    if args.self_test {
        return self_test();
    }
    if args.check_update {
        return check_update();
    }
    if args.install_update {
        return match updater::install_from_command_line() {
            Ok(done) => {
                println!("{done}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("bdf: {e}");
                ExitCode::FAILURE
            }
        };
    }

    let data = DataDir::locate();
    logger::init(&data.log_file(), cfg!(debug_assertions));
    log::info!(
        "BDF {} starting; data in {} (portable: {})",
        env!("CARGO_PKG_VERSION"),
        data.path.display(),
        data.portable
    );
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("panic: {info}");
        log::logger().flush();
        default_hook(info);
    }));
    let settings = data.load();

    let mut files = args.files.clone();
    let bench = match &args.bench {
        Some(file) => {
            let mut setup = Duration::ZERO;
            let file = file.clone().unwrap_or_else(|| {
                let p = std::env::temp_dir().join("bdf-bench-2000.pdf");
                if !p.exists() {
                    let t = Instant::now();
                    let _ = std::fs::write(&p, bdf_core::testpdf::long_document(2000));
                    setup = t.elapsed();
                }
                p
            });
            files = vec![file.clone()];
            let report = args
                .report
                .clone()
                .unwrap_or_else(|| PathBuf::from("bdf-bench.json"));
            // Writing the test file is not part of starting up.
            Some(bench::Bench::new(file, report, launched + setup))
        }
        None => None,
    };

    let pref = args.renderer.unwrap_or(settings.renderer);
    let renderer = choose_renderer(pref, settings.opengl_failed);
    let auto_tried_opengl = pref == RendererPref::Auto && renderer == eframe::Renderer::Glow;
    let data_dir = data.clone();
    let renderer_name = match renderer {
        eframe::Renderer::Wgpu => "wgpu",
        _ => "OpenGL",
    }
    .to_string();

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("BDF")
        .with_app_id("bdf")
        .with_icon(theme::window_icon(settings.eye))
        .with_min_inner_size([640.0, 420.0])
        .with_drag_and_drop(true);
    match settings.window {
        Some([x, y, w, h]) if args.bench.is_none() => {
            viewport = viewport.with_position([x, y]).with_inner_size([w, h]);
        }
        _ => viewport = viewport.with_inner_size([1280.0, 860.0]),
    }
    if settings.maximized && args.bench.is_none() {
        viewport = viewport.with_maximized(true);
    }
    let options = eframe::NativeOptions {
        viewport,
        renderer,
        ..Default::default()
    };

    let start = app::StartOptions {
        files,
        page: args.page,
        bench,
        renderer: renderer_name,
    };
    let result = eframe::run_native(
        "BDF",
        options,
        Box::new(move |cc| Ok(Box::new(app::BdfApp::new(cc, data, settings, start)))),
    );
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            log::error!("graphics start-up failed with {:?}: {e}", pref);
            // OpenGL is missing on some virtual machines and remote desktops:
            // start again with DirectX / Vulkan / Metal (a process can only
            // create one window event loop, so this needs a fresh process).
            if auto_tried_opengl
                && std::env::var_os("BDF_NO_FALLBACK").is_none()
                && let Ok(exe) = std::env::current_exe()
            {
                // Remember, so later starts skip the failed attempt.
                let mut s = data_dir.load();
                s.opengl_failed = true;
                if let Err(e) = data_dir.save(&s) {
                    log::warn!("could not remember the graphics fallback: {e}");
                }
                let mut cmd = std::process::Command::new(exe);
                cmd.args(std::env::args_os().skip(1))
                    .arg("--renderer")
                    .arg("wgpu")
                    .env("BDF_NO_FALLBACK", "1");
                if let Ok(status) = cmd.status() {
                    return if status.success() {
                        ExitCode::SUCCESS
                    } else {
                        ExitCode::FAILURE
                    };
                }
            }
            platform::attach_console();
            eprintln!("bdf: could not start the window: {e}");
            ExitCode::FAILURE
        }
    }
}

/// OpenGL starts fastest and is the default; "Automatic" skips it once it has
/// failed on this computer.
fn choose_renderer(pref: RendererPref, opengl_failed: bool) -> eframe::Renderer {
    match pref {
        RendererPref::Wgpu => eframe::Renderer::Wgpu,
        RendererPref::Auto if opengl_failed => eframe::Renderer::Wgpu,
        RendererPref::OpenGl | RendererPref::Auto => eframe::Renderer::Glow,
    }
}

/// `--list-annotations`: one line per annotation, as PDFium sees the pages.
fn list_annotations(path: &std::path::Path) -> ExitCode {
    use bdf_core::engine::{DocId, Engine};
    let mut engine = match bdf_core::pdfium::PdfiumEngine::load() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("bdf: {e}");
            return ExitCode::FAILURE;
        }
    };
    let info = match engine.open(DocId(1), path, None) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("bdf: {e}");
            return ExitCode::FAILURE;
        }
    };
    for page in 0..info.page_sizes.len() as u32 {
        for a in engine.annotations(DocId(1), page).unwrap_or_default() {
            println!(
                "page {}\t{}\t{}",
                page + 1,
                a.kind.label(),
                a.contents.replace('\n', " ")
            );
        }
    }
    ExitCode::SUCCESS
}

/// `--self-test`: what a downloaded update must pass before it replaces
/// this program. Prints `BDF <version> self-test ok` on success.
fn self_test() -> ExitCode {
    use bdf_core::engine::{DocId, Engine, TileKey, TileKind};
    let run = || -> Result<(), String> {
        let mut engine = bdf_core::pdfium::PdfiumEngine::load()?;
        let path = std::env::temp_dir().join(format!("bdf-self-test-{}.pdf", std::process::id()));
        std::fs::write(&path, bdf_core::testpdf::long_document(2)).map_err(|e| e.to_string())?;
        let doc = DocId(1);
        let result = (|| {
            engine.open(doc, &path, None).map_err(|e| e.to_string())?;
            let thumb = TileKey {
                doc,
                page: 0,
                rotation: Default::default(),
                invert: false,
                generation: 0,
                kind: TileKind::Thumb,
            };
            let img = engine.render(&thumb)?;
            if img.width == 0 || img.rgba.iter().all(|&b| b == 255) {
                return Err("the test page rendered blank".to_string());
            }
            let text = engine.page_data(doc, 0)?.text.all();
            if !text.contains("PAGE-1") {
                return Err("the test page's text was not found".into());
            }
            engine.close(doc);
            Ok(())
        })();
        let _ = std::fs::remove_file(&path);
        result
    };
    match run() {
        Ok(()) => {
            println!("BDF {} self-test ok", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("bdf: self-test failed: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `--check-update`: the version on GitHub, compared with this one.
fn check_update() -> ExitCode {
    let current = bdf_core::update::Version::current();
    match updater::latest() {
        Ok(r) if r.version > current => {
            println!(
                "BDF {} is available (this is {current}): {}",
                r.version, r.page
            );
            ExitCode::SUCCESS
        }
        Ok(r) => {
            println!(
                "BDF {current} is up to date (latest release: {})",
                r.version
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("bdf: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_skips_opengl_once_it_failed() {
        use eframe::Renderer::{Glow, Wgpu};
        assert_eq!(choose_renderer(RendererPref::Auto, false), Glow);
        assert_eq!(choose_renderer(RendererPref::Auto, true), Wgpu);
        // An explicit choice is always honoured.
        assert_eq!(choose_renderer(RendererPref::OpenGl, true), Glow);
        assert_eq!(choose_renderer(RendererPref::Wgpu, false), Wgpu);
    }
}
