//! Small OS-specific helpers.

use std::path::Path;

/// The native window handle on Windows (for the print dialog's owner).
#[cfg(windows)]
pub fn hwnd(frame: &eframe::Frame) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match frame.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
}

#[cfg(not(windows))]
pub fn hwnd(_frame: &eframe::Frame) -> Option<isize> {
    None
}

pub fn pdfium_name() -> &'static str {
    if cfg!(windows) {
        "pdfium.dll"
    } else if cfg!(target_os = "macos") {
        "libpdfium.dylib"
    } else {
        "libpdfium.so"
    }
}

/// Opens a folder in the system file manager.
pub fn reveal(path: &Path) {
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    if let Err(e) = std::process::Command::new(program).arg(path).spawn() {
        log::warn!("could not open {}: {e}", path.display());
    }
}

/// Lets a GUI-subsystem executable print to the console it was started from.
#[cfg(windows)]
pub fn attach_console() {
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    // SAFETY: no preconditions; failure (no parent console) is harmless.
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

#[cfg(not(windows))]
pub fn attach_console() {}

/// `canonicalize` on Windows returns verbatim paths (`\\?\C:\x`, `\\?\UNC\srv\x`);
/// turn them back into the ordinary form people recognise and other programs accept.
pub fn clean_path(p: &Path) -> std::path::PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return std::path::PathBuf::from(format!(r"\\{rest}"));
    }
    match s.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => std::path::PathBuf::from(rest),
        _ => p.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbatim_paths_are_cleaned() {
        assert_eq!(
            clean_path(Path::new(r"\\?\C:\docs\a.pdf")),
            Path::new(r"C:\docs\a.pdf")
        );
        assert_eq!(
            clean_path(Path::new(r"\\?\UNC\srv\share\a.pdf")),
            Path::new(r"\\srv\share\a.pdf")
        );
        assert_eq!(
            clean_path(Path::new("/home/a.pdf")),
            Path::new("/home/a.pdf")
        );
        assert_eq!(
            clean_path(Path::new(r"\\?\Volume{x}\a.pdf")),
            Path::new(r"\\?\Volume{x}\a.pdf")
        );
    }
}
