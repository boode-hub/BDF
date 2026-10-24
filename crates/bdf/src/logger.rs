//! A minimal logger writing to `bdf.log` in the data folder.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;

struct FileLogger {
    file: Mutex<Option<File>>,
    start: Instant,
    echo: bool,
}

impl log::Log for FileLogger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::Level::Info
            || (m.level() <= log::Level::Debug && m.target().starts_with("bdf"))
    }

    fn log(&self, r: &log::Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        // Keep third-party chatter out unless it is a warning.
        if !r.target().starts_with("bdf") && r.level() > log::Level::Warn {
            return;
        }
        let line = format!(
            "{:>9.3} {:<5} {}: {}\n",
            self.start.elapsed().as_secs_f64(),
            r.level(),
            r.target(),
            r.args()
        );
        if self.echo {
            eprint!("{line}");
        }
        if let Ok(mut g) = self.file.lock()
            && let Some(f) = g.as_mut()
        {
            let _ = f.write_all(line.as_bytes());
        }
    }

    fn flush(&self) {
        if let Ok(mut g) = self.file.lock()
            && let Some(f) = g.as_mut()
        {
            let _ = f.flush();
        }
    }
}

pub fn init(path: &Path, echo: bool) {
    // Keep one previous log; start fresh when it grows past 1 MB.
    if std::fs::metadata(path)
        .map(|m| m.len() > 1 << 20)
        .unwrap_or(false)
    {
        let _ = std::fs::rename(path, path.with_extension("log.old"));
    }
    let file = OpenOptions::new().create(true).append(true).open(path).ok();
    let logger = Box::new(FileLogger {
        file: Mutex::new(file),
        start: Instant::now(),
        echo,
    });
    if log::set_boxed_logger(logger).is_ok() {
        log::set_max_level(log::LevelFilter::Debug);
    }
}
