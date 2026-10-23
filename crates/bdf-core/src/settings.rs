//! User settings and per-document memory (last page, bookmarks).
//!
//! BDF is portable: settings live in `BDF-data/` next to the executable when
//! that folder is writable, and otherwise in the per-user config folder.
//! Nothing is written to the registry. Files are replaced atomically, and a
//! corrupt settings file is set aside instead of crashing the app.

use crate::layout::PageMode;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ThemePref {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum RendererPref {
    #[default]
    Auto,
    OpenGl,
    Wgpu,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ZoomPref {
    #[default]
    FitWidth,
    FitPage,
    Actual,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UserBookmark {
    pub title: String,
    pub page: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DocMemory {
    pub last_page: u32,
    pub bookmarks: Vec<UserBookmark>,
    /// Last time this document was opened (Unix seconds), for pruning.
    pub seen: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecentFile {
    pub path: PathBuf,
    pub opened: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemePref,
    pub renderer: RendererPref,
    pub page_mode: PageMode,
    pub cover_page: bool,
    pub default_zoom: ZoomPref,
    pub dark_pages: bool,
    pub sidebar: bool,
    pub sidebar_width: f32,
    pub remember_page: bool,
    pub restore_tabs: bool,
    /// GPU memory for page images, in MiB.
    pub cache_mib: u32,
    pub highlight_color: [u8; 3],
    pub ink_color: [u8; 3],
    pub ink_width: f32,
    pub magnifier_zoom: f32,
    pub recent: Vec<RecentFile>,
    pub open_tabs: Vec<PathBuf>,
    pub docs: HashMap<String, DocMemory>,
    pub window: Option<[f32; 4]>,
    pub maximized: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemePref::System,
            renderer: RendererPref::Auto,
            page_mode: PageMode::Continuous,
            cover_page: false,
            default_zoom: ZoomPref::FitWidth,
            dark_pages: false,
            sidebar: true,
            sidebar_width: 220.0,
            remember_page: true,
            restore_tabs: false,
            cache_mib: 256,
            highlight_color: [255, 226, 64],
            ink_color: [220, 38, 38],
            ink_width: 2.0,
            magnifier_zoom: 2.5,
            recent: Vec::new(),
            open_tabs: Vec::new(),
            docs: HashMap::new(),
            window: None,
            maximized: false,
        }
    }
}

pub const MAX_RECENT: usize = 15;
const MAX_DOC_MEMORY: usize = 500;

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Settings {
    /// Moves `path` to the top of the recent list.
    pub fn touch_recent(&mut self, path: &Path) {
        self.recent.retain(|r| r.path != path);
        self.recent.insert(0, RecentFile { path: path.to_path_buf(), opened: now_secs() });
        self.recent.truncate(MAX_RECENT);
    }

    pub fn doc_mut(&mut self, fingerprint: &str) -> &mut DocMemory {
        if !self.docs.contains_key(fingerprint) && self.docs.len() >= MAX_DOC_MEMORY {
            // Forget the documents not seen for the longest time, but never
            // ones with bookmarks.
            let mut by_age: Vec<(u64, String)> = self
                .docs
                .iter()
                .filter(|(_, m)| m.bookmarks.is_empty())
                .map(|(k, m)| (m.seen, k.clone()))
                .collect();
            by_age.sort();
            for (_, k) in by_age.into_iter().take(self.docs.len() + 1 - MAX_DOC_MEMORY) {
                self.docs.remove(&k);
            }
        }
        let m = self.docs.entry(fingerprint.to_string()).or_default();
        m.seen = now_secs();
        m
    }

    /// Clamps values a hand-edited file might have broken.
    pub fn sanitize(&mut self) {
        self.cache_mib = self.cache_mib.clamp(32, 4096);
        self.sidebar_width = if self.sidebar_width.is_finite() { self.sidebar_width.clamp(140.0, 600.0) } else { 220.0 };
        self.ink_width = if self.ink_width.is_finite() { self.ink_width.clamp(0.5, 20.0) } else { 2.0 };
        self.magnifier_zoom = if self.magnifier_zoom.is_finite() { self.magnifier_zoom.clamp(1.5, 8.0) } else { 2.5 };
        self.recent.truncate(MAX_RECENT);
        if let Some(w) = self.window {
            if w.iter().any(|v| !v.is_finite()) || w[2] < 200.0 || w[3] < 150.0 {
                self.window = None;
            }
        }
    }
}

/// Where settings and logs go.
#[derive(Clone, Debug)]
pub struct DataDir {
    pub path: PathBuf,
    pub portable: bool,
}

impl DataDir {
    /// `BDF-data` next to the executable if writable, else the user config dir.
    pub fn locate() -> DataDir {
        if let Some(p) = std::env::var_os("BDF_DATA_DIR") {
            let path = PathBuf::from(p);
            let _ = std::fs::create_dir_all(&path);
            return DataDir { path, portable: true };
        }
        if let Some(exe_dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
            let path = exe_dir.join("BDF-data");
            if is_writable_dir(&path) {
                return DataDir { path, portable: true };
            }
        }
        let base = user_config_dir().unwrap_or_else(std::env::temp_dir);
        let path = base.join("BDF");
        let _ = std::fs::create_dir_all(&path);
        DataDir { path, portable: false }
    }

    pub fn settings_file(&self) -> PathBuf {
        self.path.join("settings.json")
    }

    pub fn log_file(&self) -> PathBuf {
        self.path.join("bdf.log")
    }

    pub fn load(&self) -> Settings {
        load_settings(&self.settings_file())
    }

    pub fn save(&self, s: &Settings) -> std::io::Result<()> {
        save_settings(&self.settings_file(), s)
    }
}

fn is_writable_dir(p: &Path) -> bool {
    if std::fs::create_dir_all(p).is_err() {
        return false;
    }
    let probe = p.join(".write-test");
    let ok = std::fs::write(&probe, b"ok").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

fn user_config_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    }
}

pub fn load_settings(file: &Path) -> Settings {
    let Ok(bytes) = std::fs::read(file) else {
        return Settings::default();
    };
    match serde_json::from_slice::<Settings>(&bytes) {
        Ok(mut s) => {
            s.sanitize();
            s
        }
        Err(e) => {
            log::warn!("settings file is damaged ({e}); starting with defaults");
            let _ = std::fs::rename(file, file.with_extension("json.broken"));
            Settings::default()
        }
    }
}

pub fn save_settings(file: &Path, s: &Settings) -> std::io::Result<()> {
    let json = serde_json::to_vec_pretty(s).map_err(std::io::Error::other)?;
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, file)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("bdf-settings-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn round_trip() {
        let d = tmpdir("rt");
        let f = d.join("settings.json");
        let mut s = Settings { dark_pages: true, ..Default::default() };
        s.doc_mut("id:ab").bookmarks.push(UserBookmark { title: "x".into(), page: 3 });
        save_settings(&f, &s).unwrap();
        let back = load_settings(&f);
        assert!(back.dark_pages);
        assert_eq!(back.docs["id:ab"].bookmarks[0].page, 3);
    }

    #[test]
    fn corrupt_file_falls_back_and_is_kept() {
        let d = tmpdir("bad");
        let f = d.join("settings.json");
        std::fs::write(&f, b"{ not json").unwrap();
        assert_eq!(load_settings(&f), Settings::default());
        assert!(d.join("settings.json.broken").exists());
    }

    #[test]
    fn missing_fields_take_defaults_and_values_are_clamped() {
        let d = tmpdir("partial");
        let f = d.join("settings.json");
        std::fs::write(&f, br#"{"cache_mib": 999999, "ink_width": -4}"#).unwrap();
        let s = load_settings(&f);
        assert_eq!(s.cache_mib, 4096);
        assert_eq!(s.ink_width, 0.5);
        assert!(s.sidebar);
    }

    #[test]
    fn recent_list_is_bounded_and_deduplicated() {
        let mut s = Settings::default();
        for i in 0..30 {
            s.touch_recent(Path::new(&format!("/f{i}.pdf")));
        }
        s.touch_recent(Path::new("/f20.pdf"));
        assert_eq!(s.recent.len(), MAX_RECENT);
        assert_eq!(s.recent[0].path, PathBuf::from("/f20.pdf"));
        assert_eq!(s.recent.iter().filter(|r| r.path == Path::new("/f20.pdf")).count(), 1);
    }

    #[test]
    fn doc_memory_is_pruned_but_bookmarks_survive() {
        let mut s = Settings::default();
        s.doc_mut("keep").bookmarks.push(UserBookmark { title: "b".into(), page: 1 });
        s.docs.get_mut("keep").unwrap().seen = 0;
        for i in 0..(MAX_DOC_MEMORY + 20) {
            s.doc_mut(&format!("d{i}"));
        }
        assert!(s.docs.len() <= MAX_DOC_MEMORY);
        assert!(s.docs.contains_key("keep"));
    }
}
