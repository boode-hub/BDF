//! Printing.
//!
//! * Windows: the system print dialog, then each page is rendered by PDFium
//!   at printer resolution (capped at 300 dpi) and sent through GDI. What is
//!   printed is what is on screen, including unsaved annotations.
//! * macOS and Linux: the saved file is handed to CUPS (`lp`).

#[derive(Clone, Debug, PartialEq)]
pub struct PageRanges(pub Vec<(u32, u32)>);

impl PageRanges {
    /// Parses "1-3, 5, 8-" (1-based, inclusive) against a page count.
    /// An empty string means all pages.
    pub fn parse(s: &str, pages: u32) -> Result<Self, String> {
        let s = s.trim();
        if s.is_empty() {
            return Ok(PageRanges(vec![(1, pages)]));
        }
        let mut out = Vec::new();
        for part in s.split([',', ';']) {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let num = |t: &str| -> Result<u32, String> {
                t.trim()
                    .parse::<u32>()
                    .map_err(|_| format!("'{}' is not a page number", t.trim()))
            };
            let (a, b) = match part.split_once('-') {
                Some((a, b)) => (
                    if a.trim().is_empty() { 1 } else { num(a)? },
                    if b.trim().is_empty() { pages } else { num(b)? },
                ),
                None => {
                    let n = num(part)?;
                    (n, n)
                }
            };
            if a == 0 || b == 0 || a > pages || b > pages {
                return Err(format!("pages must be between 1 and {pages}"));
            }
            out.push((a.min(b), a.max(b)));
        }
        if out.is_empty() {
            return Err("no pages selected".into());
        }
        Ok(PageRanges(out))
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn pages(&self) -> impl Iterator<Item = u32> + '_ {
        self.0.iter().flat_map(|&(a, b)| a..=b)
    }

    /// CUPS syntax, e.g. "1-3,5".
    #[cfg_attr(windows, allow(dead_code))]
    pub fn to_cups(&self) -> String {
        self.0
            .iter()
            .map(|&(a, b)| {
                if a == b {
                    a.to_string()
                } else {
                    format!("{a}-{b}")
                }
            })
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// Fits a page of `page_pt` points into a printable area of `area` device
/// pixels at `dpi`, never upscaling past the page's real size, centred.
/// Returns (x, y, w, h) in device pixels.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn fit_on_paper(
    page_pt: (f32, f32),
    area: (i32, i32),
    dpi: (i32, i32),
) -> (i32, i32, i32, i32) {
    let real_w = page_pt.0 / 72.0 * dpi.0 as f32;
    let real_h = page_pt.1 / 72.0 * dpi.1 as f32;
    let k = (area.0 as f32 / real_w)
        .min(area.1 as f32 / real_h)
        .min(1.0);
    let w = (real_w * k).round().max(1.0) as i32;
    let h = (real_h * k).round().max(1.0) as i32;
    ((area.0 - w) / 2, (area.1 - h) / 2, w, h)
}

#[cfg(windows)]
pub mod windows {
    use super::*;
    use bdf_core::engine::{DocId, Engine};
    use windows_sys::Win32::Foundation::{GlobalFree, HWND};
    use windows_sys::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteDC, GetDeviceCaps, HALFTONE,
        HDC, HORZRES, LOGPIXELSX, LOGPIXELSY, SRCCOPY, SetStretchBltMode, StretchDIBits, VERTRES,
    };
    use windows_sys::Win32::Storage::Xps::{
        AbortDoc, DOCINFOW, EndDoc, EndPage, StartDocW, StartPage,
    };
    use windows_sys::Win32::UI::Controls::Dialogs::{
        PD_ALLPAGES, PD_NOSELECTION, PD_PAGENUMS, PD_RETURNDC, PD_USEDEVMODECOPIESANDCOLLATE,
        PRINTDLGW, PrintDlgW,
    };

    /// A printer chosen in the system dialog.
    pub struct PrintJob {
        hdc: isize,
        pub ranges: PageRanges,
    }

    // SAFETY: the DC is used by one thread at a time (handed to the engine thread).
    unsafe impl Send for PrintJob {}

    /// Shows the print dialog. `None` if cancelled.
    pub fn dialog(owner: Option<isize>, pages: u32) -> Option<PrintJob> {
        let pages16 = pages.min(u16::MAX as u32) as u16;
        // SAFETY: PRINTDLGW is plain data; zero is valid for every field.
        let mut pd: PRINTDLGW = unsafe { std::mem::zeroed() };
        pd.lStructSize = std::mem::size_of::<PRINTDLGW>() as u32;
        pd.hwndOwner = owner.unwrap_or(0) as HWND;
        pd.Flags = PD_RETURNDC | PD_ALLPAGES | PD_NOSELECTION | PD_USEDEVMODECOPIESANDCOLLATE;
        pd.nFromPage = 1;
        pd.nToPage = pages16;
        pd.nMinPage = 1;
        pd.nMaxPage = pages16;
        pd.nCopies = 1;
        // SAFETY: valid, initialised struct.
        let ok = unsafe { PrintDlgW(&mut pd) } != 0;
        // SAFETY: the dialog allocated these; we own them now.
        unsafe {
            if !pd.hDevMode.is_null() {
                GlobalFree(pd.hDevMode);
            }
            if !pd.hDevNames.is_null() {
                GlobalFree(pd.hDevNames);
            }
        }
        if !ok || pd.hDC.is_null() {
            return None;
        }
        let ranges = if pd.Flags & PD_PAGENUMS != 0 {
            PageRanges(vec![(
                pd.nFromPage.max(1) as u32,
                (pd.nToPage as u32).clamp(1, pages),
            )])
        } else {
            PageRanges(vec![(1, pages)])
        };
        Some(PrintJob {
            hdc: pd.hDC as isize,
            ranges,
        })
    }

    /// Prints on the engine thread. Returns pages printed.
    pub fn run(
        job: PrintJob,
        engine: &mut dyn Engine,
        doc: DocId,
        title: &str,
    ) -> Result<u32, String> {
        let hdc = job.hdc as HDC;
        let name: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
        let di = DOCINFOW {
            cbSize: std::mem::size_of::<DOCINFOW>() as i32,
            lpszDocName: name.as_ptr(),
            lpszOutput: std::ptr::null(),
            lpszDatatype: std::ptr::null(),
            fwType: 0,
        };
        // SAFETY: hdc is a printer DC from PrintDlgW, used only on this thread.
        unsafe {
            if StartDocW(hdc, &di) <= 0 {
                DeleteDC(hdc);
                return Err("the printer refused the job".into());
            }
            let area = (
                GetDeviceCaps(hdc, HORZRES as i32),
                GetDeviceCaps(hdc, VERTRES as i32),
            );
            let dpi = (
                GetDeviceCaps(hdc, LOGPIXELSX as i32).max(72),
                GetDeviceCaps(hdc, LOGPIXELSY as i32).max(72),
            );
            SetStretchBltMode(hdc, HALFTONE);
            let mut printed = 0;
            let mut err = None;
            for page in job.ranges.pages() {
                let index = page - 1;
                let Some(size) = engine.page_size(doc, index) else {
                    continue;
                };
                let (x, y, w, h) = fit_on_paper(size, area, dpi);
                // Render at no more than 300 dpi to bound memory.
                let cap = (300.0 / dpi.0 as f32).min(1.0);
                let (rw, rh) = (
                    ((w as f32) * cap).max(1.0) as u32,
                    ((h as f32) * cap).max(1.0) as u32,
                );
                let img = match engine.render_page(doc, index, rw, rh, true) {
                    Ok(i) => i,
                    Err(e) => {
                        err = Some(e);
                        break;
                    }
                };
                // GDI wants BGRA, top-down via a negative height.
                let mut bgra = img.rgba;
                for px in bgra.as_chunks_mut::<4>().0 {
                    px.swap(0, 2);
                }
                let mut bmi: BITMAPINFO = std::mem::zeroed();
                bmi.bmiHeader = BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: img.width as i32,
                    biHeight: -(img.height as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    biSizeImage: 0,
                    biXPelsPerMeter: 0,
                    biYPelsPerMeter: 0,
                    biClrUsed: 0,
                    biClrImportant: 0,
                };
                if StartPage(hdc) <= 0 {
                    err = Some("the printer refused a page".to_string());
                    break;
                }
                StretchDIBits(
                    hdc,
                    x,
                    y,
                    w,
                    h,
                    0,
                    0,
                    img.width as i32,
                    img.height as i32,
                    bgra.as_ptr().cast(),
                    &bmi,
                    DIB_RGB_COLORS,
                    SRCCOPY,
                );
                if EndPage(hdc) <= 0 {
                    err = Some("the printer failed while printing".to_string());
                    break;
                }
                printed += 1;
            }
            if err.is_some() {
                AbortDoc(hdc);
            } else {
                EndDoc(hdc);
            }
            DeleteDC(hdc);
            match err {
                Some(e) => Err(e),
                None => Ok(printed),
            }
        }
    }
}

#[cfg(not(windows))]
pub fn lp_print(path: &std::path::Path, ranges: &PageRanges, copies: u32) -> Result<(), String> {
    let out = std::process::Command::new("lp")
        .arg("-n")
        .arg(copies.max(1).to_string())
        .arg("-P")
        .arg(ranges.to_cups())
        .arg("--")
        .arg(path)
        .output()
        .map_err(|e| format!("printing needs CUPS (the `lp` command): {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        assert_eq!(PageRanges::parse("", 10).unwrap().0, vec![(1, 10)]);
        assert_eq!(
            PageRanges::parse("1-3, 5;8-", 10).unwrap().0,
            vec![(1, 3), (5, 5), (8, 10)]
        );
        assert_eq!(PageRanges::parse("-2", 10).unwrap().0, vec![(1, 2)]);
        assert_eq!(PageRanges::parse("4-2", 10).unwrap().0, vec![(2, 4)]);
        assert!(PageRanges::parse("0", 10).is_err());
        assert!(PageRanges::parse("11", 10).is_err());
        assert!(PageRanges::parse("a-b", 10).is_err());
        assert!(PageRanges::parse(",", 10).is_err());
        let r = PageRanges::parse("1-3,5", 10).unwrap();
        assert_eq!(r.to_cups(), "1-3,5");
        assert_eq!(r.pages().collect::<Vec<_>>(), vec![1, 2, 3, 5]);
    }

    #[test]
    fn fitting() {
        // Letter on letter at 600 dpi with a 0.25" margin each side.
        let (x, y, w, h) = fit_on_paper((612.0, 792.0), (4800, 6300), (600, 600));
        assert_eq!((w, h), (4800, 6212));
        assert_eq!((x, y), (0, 44));
        // A small page is not enlarged.
        let (_, _, w, h) = fit_on_paper((100.0, 100.0), (4800, 6300), (600, 600));
        assert_eq!((w, h), (833, 833));
    }
}
