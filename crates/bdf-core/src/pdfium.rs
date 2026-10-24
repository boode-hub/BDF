//! The PDFium engine.
//!
//! Talks to PDFium through `pdfium-render`'s raw FFI bindings rather than its
//! high-level wrapper, because we need things the wrapper does not combine:
//! tiled rendering *with* form fields, incremental saves, ink strokes, and
//! documents read through our own file callback (Unicode paths on Windows,
//! no whole-file load, other programs can still open the file).
//!
//! All unsafe code in BDF lives in this file. Every handle is closed on every
//! path; documents own their pages, and pages are closed before documents.

use crate::engine::{
    AnnotInfo, AnnotKind, DocId, DocInfo, EResult, Engine, FieldKind, FormField, FormInput,
    FormOutcome, Image, Link, LinkTarget, NewAnnot, OpenError, OutlineItem, PageData, RegionReq,
    TileKey, TileKind, thumb_px,
};
use crate::geom::{Rotation, page_px};
use crate::text::{PageText, TextChar};
use pdfium_render::prelude::{
    FPDF_ACTION, FPDF_ANNOTATION, FPDF_BOOKMARK, FPDF_DEST, FPDF_DOCUMENT, FPDF_FILEACCESS,
    FPDF_FILEWRITE, FPDF_FORMFILLINFO, FPDF_FORMHANDLE, FPDF_LINK, FPDF_PAGE, FPDF_TEXTPAGE,
    FPDF_WCHAR, FS_POINTF, FS_QUADPOINTSF, FS_RECTF, FS_SIZEF, Pdfium, PdfiumLibraryBindings,
};
use std::collections::{HashMap, HashSet};
use std::ffi::{c_int, c_uchar, c_ulong, c_void};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicU64, Ordering};

// PDFium constants (public headers; stable across versions).
const FPDF_ANNOT: c_int = 0x01;
const FPDF_REVERSE_BYTE_ORDER: c_int = 0x10;
const FPDF_PRINTING: c_int = 0x800;
const FPDF_GRAYSCALE: c_int = 0x08;
const FPDF_ERR_FILE: c_ulong = 2;
const FPDF_ERR_FORMAT: c_ulong = 3;
const FPDF_ERR_PASSWORD: c_ulong = 4;
const FPDF_ERR_SECURITY: c_ulong = 5;
const FPDF_ERR_PAGE: c_ulong = 6;
const FPDF_INCREMENTAL: u32 = 1;
const FPDF_NO_INCREMENTAL: u32 = 2;
const PDFACTION_GOTO: c_ulong = 1;
const PDFACTION_URI: c_ulong = 3;
const ANNOT_TEXT: c_int = 1;
const ANNOT_LINK: c_int = 2;
const ANNOT_FREETEXT: c_int = 3;
const ANNOT_LINE: c_int = 4;
const ANNOT_SQUARE: c_int = 5;
const ANNOT_CIRCLE: c_int = 6;
const ANNOT_HIGHLIGHT: c_int = 9;
const ANNOT_UNDERLINE: c_int = 10;
const ANNOT_SQUIGGLY: c_int = 11;
const ANNOT_STRIKEOUT: c_int = 12;
const ANNOT_STAMP: c_int = 13;
const ANNOT_INK: c_int = 15;
const ANNOT_POPUP: c_int = 16;
const ANNOT_WIDGET: c_int = 20;
const ANNOT_XFAWIDGET: c_int = 24;
const ANNOT_FLAG_PRINT: c_int = 4;
const COLORTYPE_COLOR: u32 = 0;
const FORMTYPE_NONE: c_int = 0;

/// Engines alive in this process (see [`PdfiumEngine::with_bindings`]).
static LIB_USERS: std::sync::Mutex<usize> = std::sync::Mutex::new(0);

/// Pages kept loaded per document. Loading a page parses its content, so
/// keeping a few avoids re-parsing while tiles of the same page render.
const PAGE_CACHE: usize = 8;

/// Locates the PDFium shared library: `BDF_PDFIUM` (file or folder), next to
/// the executable, `lib/` next to the executable, then the system.
pub fn locate_library() -> Vec<PathBuf> {
    let name = Pdfium::pdfium_platform_library_name();
    let mut out = Vec::new();
    if let Some(p) = std::env::var_os("BDF_PDFIUM") {
        let p = PathBuf::from(p);
        if p.is_dir() {
            out.push(p.join(&name));
        } else {
            out.push(p);
        }
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
    {
        out.push(dir.join(&name));
        out.push(dir.join("lib").join(&name));
    }
    out
}

pub struct PdfiumEngine {
    lib: Box<dyn PdfiumLibraryBindings>,
    pub library_path: String,
    docs: HashMap<DocId, Doc>,
    author: String,
}

// A document handle plus everything that must outlive it.
struct Doc {
    handle: FPDF_DOCUMENT,
    form: FPDF_FORMHANDLE,
    // PDFium keeps pointers to these; they are boxed so they never move.
    form_info: Option<Box<FormInfo>>,
    access: Box<Access>,
    path: PathBuf,
    password: Option<String>,
    sizes: Vec<(f32, f32)>,
    /// Most recently used last.
    pages: Vec<(u32, FPDF_PAGE)>,
}

impl PdfiumEngine {
    /// Loads PDFium from the first location that works.
    pub fn load() -> Result<Self, String> {
        let mut errors = Vec::new();
        for path in locate_library() {
            if !path.exists() {
                continue;
            }
            match Pdfium::bind_to_library(&path) {
                Ok(lib) => return Ok(Self::with_bindings(lib, path.display().to_string())),
                Err(e) => errors.push(format!("{}: {e:?}", path.display())),
            }
        }
        match Pdfium::bind_to_system_library() {
            Ok(lib) => Ok(Self::with_bindings(lib, "system library".into())),
            Err(e) => {
                errors.push(format!("system: {e:?}"));
                Err(format!(
                    "PDFium ({}) was not found. Put it next to the BDF executable.\n{}",
                    Pdfium::pdfium_platform_library_name().to_string_lossy(),
                    errors.join("\n")
                ))
            }
        }
    }

    /// Wraps loaded bindings. PDFium has one global state per process, so
    /// initialisation is reference counted across engines; PDFium is not
    /// thread-safe, so engines must not be used from two threads at once.
    pub fn with_bindings(lib: Box<dyn PdfiumLibraryBindings>, library_path: String) -> Self {
        let mut users = LIB_USERS.lock().unwrap_or_else(|e| e.into_inner());
        if *users == 0 {
            // SAFETY: first user in this process; nothing else is using PDFium.
            unsafe { lib.FPDF_InitLibrary() };
        }
        *users += 1;
        drop(users);
        let author = std::env::var("USERNAME")
            .or_else(|_| std::env::var("USER"))
            .unwrap_or_default();
        Self {
            lib,
            library_path,
            docs: HashMap::new(),
            author,
        }
    }

    fn doc(&mut self, id: DocId) -> EResult<&mut Doc> {
        self.docs
            .get_mut(&id)
            .ok_or_else(|| "document is not open".to_string())
    }

    fn page(&mut self, id: DocId, index: u32) -> EResult<FPDF_PAGE> {
        let lib = &*self.lib;
        let doc = self.docs.get_mut(&id).ok_or("document is not open")?;
        if index as usize >= doc.sizes.len() {
            return Err(format!("page {} is out of range", index + 1));
        }
        if let Some(pos) = doc.pages.iter().position(|(i, _)| *i == index) {
            let entry = doc.pages.remove(pos);
            let h = entry.1;
            doc.pages.push(entry);
            return Ok(h);
        }
        // SAFETY: valid document handle; index checked above.
        let h = unsafe { lib.FPDF_LoadPage(doc.handle, index as c_int) };
        if h.is_null() {
            return Err(format!("page {} could not be loaded", index + 1));
        }
        if !doc.form.is_null() {
            // SAFETY: page and form belong to the same document.
            unsafe { lib.FORM_OnAfterLoadPage(h, doc.form) };
        }
        doc.pages.push((index, h));
        if doc.pages.len() > PAGE_CACHE {
            let (_, old) = doc.pages.remove(0);
            close_page(lib, doc.form, old);
        }
        Ok(h)
    }

    /// Drops a cached page so the next use reloads it (after edits).
    fn evict_page(&mut self, id: DocId, index: u32) {
        let lib = &*self.lib;
        if let Some(doc) = self.docs.get_mut(&id)
            && let Some(pos) = doc.pages.iter().position(|(i, _)| *i == index)
        {
            let (_, h) = doc.pages.remove(pos);
            close_page(lib, doc.form, h);
        }
    }

    fn open_handle(
        &self,
        path: &Path,
        password: Option<&str>,
    ) -> Result<(FPDF_DOCUMENT, Box<Access>), OpenError> {
        let file = File::open(path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => OpenError::NotFound,
            _ => OpenError::Unreadable(e.to_string()),
        })?;
        let len = file
            .metadata()
            .map_err(|e| OpenError::Unreadable(e.to_string()))?
            .len();
        if len == 0 {
            return Err(OpenError::NotPdf);
        }
        // c_ulong is 32-bit on Windows, so PDFium's file callback caps file size there.
        if c_ulong::try_from(len).is_err() {
            return Err(OpenError::Unsupported(
                "files larger than 4 GB are not supported on this platform".into(),
            ));
        }
        let mut access = Access::new(file, len);
        // SAFETY: `access` is boxed and outlives the document (stored in Doc).
        let handle = unsafe { self.lib.FPDF_LoadCustomDocument(&mut access.fa, password) };
        if handle.is_null() {
            // SAFETY: plain query.
            let err = unsafe { self.lib.FPDF_GetLastError() };
            return Err(match err {
                FPDF_ERR_PASSWORD if password.is_some() => OpenError::WrongPassword,
                FPDF_ERR_PASSWORD => OpenError::PasswordRequired,
                FPDF_ERR_FILE => OpenError::Unreadable("read error".into()),
                FPDF_ERR_FORMAT | FPDF_ERR_PAGE => OpenError::NotPdf,
                FPDF_ERR_SECURITY => OpenError::Unsupported("unsupported security handler".into()),
                e => OpenError::Engine(format!("error code {e}")),
            });
        }
        Ok((handle, access))
    }

    fn build_doc(
        &self,
        handle: FPDF_DOCUMENT,
        access: Box<Access>,
        path: &Path,
        password: Option<&str>,
    ) -> Result<Doc, OpenError> {
        let lib = &*self.lib;
        // SAFETY: valid handle.
        let count = unsafe { lib.FPDF_GetPageCount(handle) };
        if count <= 0 {
            // SAFETY: valid handle, nothing else references it.
            unsafe { lib.FPDF_CloseDocument(handle) };
            return Err(OpenError::NotPdf);
        }
        let mut sizes = Vec::with_capacity(count as usize);
        for i in 0..count {
            let mut s = FS_SIZEF {
                width: 0.0,
                height: 0.0,
            };
            // SAFETY: valid handle and index; `s` is a valid out pointer.
            let ok = unsafe { lib.FPDF_GetPageSizeByIndexF(handle, i, &mut s) };
            let good = ok != 0
                && s.width.is_finite()
                && s.height.is_finite()
                && s.width > 0.0
                && s.height > 0.0;
            sizes.push(if good {
                (s.width, s.height)
            } else {
                (612.0, 792.0)
            });
        }
        // SAFETY: valid handle.
        let has_form = unsafe { lib.FPDF_GetFormType(handle) } != FORMTYPE_NONE;
        let (form, form_info) = if has_form {
            // SAFETY: an all-zero FPDF_FORMFILLINFO (null callbacks) is valid
            // input; PDFium checks each callback before use.
            let mut info = Box::new(FormInfo {
                info: unsafe { std::mem::zeroed() },
                changed: false,
            });
            info.info.version = 1;
            info.info.FFI_OnChange = Some(on_form_change);
            // SAFETY: `info` is boxed and kept alive in Doc until the form exits.
            let form = unsafe { lib.FPDFDOC_InitFormFillEnvironment(handle, &mut info.info) };
            if !form.is_null() {
                // Tint fillable fields light blue, like other readers do.
                // SAFETY: valid form handle; 0 means every field type.
                unsafe {
                    // PDFium reads this as 0x00BBGGRR: a light blue.
                    lib.FPDF_SetFormFieldHighlightColor(form, 0, 0x00FF_DCD0);
                    lib.FPDF_SetFormFieldHighlightAlpha(form, 90);
                }
            }
            (form, Some(info))
        } else {
            (null_mut(), None)
        };
        Ok(Doc {
            handle,
            form,
            form_info,
            access,
            path: path.to_path_buf(),
            password: password.map(str::to_string),
            sizes,
            pages: Vec::new(),
        })
    }

    fn doc_info(&self, doc: &Doc, path: &Path) -> DocInfo {
        let lib = &*self.lib;
        let h = doc.handle;
        let n = doc.sizes.len();
        let mut labels = Vec::new();
        if n <= 20_000 {
            let mut any = false;
            for i in 0..n {
                // SAFETY: valid handle; buffer handled by helper.
                let l = utf16_buffer(|buf, len| unsafe {
                    lib.FPDF_GetPageLabel(h, i as c_int, buf, len)
                });
                let l = l.filter(|s| !s.is_empty() && *s != (i + 1).to_string());
                any |= l.is_some();
                labels.push(l);
            }
            if !any {
                labels.clear();
            }
        }
        let mut metadata = Vec::new();
        for tag in [
            "Title",
            "Author",
            "Subject",
            "Keywords",
            "Creator",
            "Producer",
            "CreationDate",
            "ModDate",
        ] {
            // SAFETY: valid handle; buffer handled by helper.
            if let Some(v) =
                utf16_buffer(|buf, len| unsafe { lib.FPDF_GetMetaText(h, tag, buf, len) })
            {
                let v = if tag.ends_with("Date") {
                    format_pdf_date(&v)
                } else {
                    v
                };
                if !v.trim().is_empty() {
                    metadata.push((tag.to_string(), v));
                }
            }
        }
        let mut version = 0;
        // SAFETY: valid handle and out pointer.
        let pdf_version = (unsafe { lib.FPDF_GetFileVersion(h, &mut version) } != 0)
            .then(|| format!("{}.{}", version / 10, version % 10));
        // SAFETY: valid handle.
        let encrypted = unsafe { lib.FPDF_GetSecurityHandlerRevision(h) } != -1;
        let fingerprint = {
            let mut buf = vec![0u8; 64];
            // SAFETY: buffer is 64 bytes, as declared.
            let n = unsafe {
                lib.FPDF_GetFileIdentifier(h, 0, buf.as_mut_ptr().cast(), buf.len() as c_ulong)
            } as usize;
            if n > 1 && n <= buf.len() {
                buf.truncate(n - 1); // drop the NUL
                format!("id:{}", hex(&buf))
            } else {
                format!("fp:{:016x}:{}", doc.access.head_hash(), doc.access.len)
            }
        };
        let outline = read_outline(lib, h);
        DocInfo {
            path: path.to_path_buf(),
            file_size: doc.access.len,
            page_sizes: doc.sizes.clone(),
            page_labels: labels,
            outline,
            metadata,
            pdf_version,
            encrypted,
            fingerprint,
            form_fields: !doc.form.is_null(),
        }
    }

    fn close_doc(&mut self, id: DocId) {
        if let Some(doc) = self.docs.remove(&id) {
            close_doc_handles(&*self.lib, doc);
        }
    }

    /// Renders `rect` (x, y, w, h in page pixels) of a page drawn at `full`
    /// pixel size, with `rot` view rotation.
    fn render_rect(
        &mut self,
        id: DocId,
        index: u32,
        rot: Rotation,
        full: (u32, u32),
        rect: (i32, i32, u32, u32),
        printing: bool,
    ) -> EResult<Image> {
        let (x, y, w, h) = rect;
        if w == 0 || h == 0 || w > 16384 || h > 16384 {
            return Err(format!("invalid render size {w}x{h}"));
        }
        let page = self.page(id, index)?;
        let lib = &*self.lib;
        let form = self.docs.get(&id).map(|d| d.form).unwrap_or(null_mut());
        // SAFETY: sizes validated; a NULL return (allocation failure) is handled.
        let bmp = unsafe { lib.FPDFBitmap_Create(w as c_int, h as c_int, 0) };
        if bmp.is_null() {
            return Err("out of memory for the page image".into());
        }
        let mut flags = FPDF_ANNOT | FPDF_REVERSE_BYTE_ORDER;
        if printing {
            flags |= FPDF_PRINTING;
        }
        let (fw, fh) = (
            full.0.min(i32::MAX as u32) as c_int,
            full.1.min(i32::MAX as u32) as c_int,
        );
        let mut rgba = vec![0u8; w as usize * h as usize * 4];
        // SAFETY: bitmap and page are valid; PDFium clips drawing to the bitmap.
        unsafe {
            lib.FPDFBitmap_FillRect(bmp, 0, 0, w as c_int, h as c_int, 0xFFFF_FFFF);
            lib.FPDF_RenderPageBitmap(bmp, page, -x, -y, fw, fh, rot.quarters(), flags);
            if !form.is_null() {
                lib.FPDF_FFLDraw(form, bmp, page, -x, -y, fw, fh, rot.quarters(), flags);
            }
            let buf = lib.FPDFBitmap_GetBuffer(bmp) as *const u8;
            let stride = lib.FPDFBitmap_GetStride(bmp) as usize;
            if buf.is_null() || stride < w as usize * 4 {
                lib.FPDFBitmap_Destroy(bmp);
                return Err("PDFium returned no image buffer".into());
            }
            for row in 0..h as usize {
                let src = std::slice::from_raw_parts(buf.add(row * stride), w as usize * 4);
                let dst = &mut rgba[row * w as usize * 4..(row + 1) * w as usize * 4];
                dst.copy_from_slice(src);
            }
            lib.FPDFBitmap_Destroy(bmp);
        }
        // The bitmap has no alpha channel; its fourth byte is undefined.
        for px in rgba.as_chunks_mut::<4>().0 {
            px[3] = 255;
        }
        Ok(Image {
            width: w,
            height: h,
            rgba,
        })
    }

    fn size_of(&self, id: DocId, page: u32) -> EResult<(f32, f32)> {
        let d = self.docs.get(&id).ok_or("document is not open")?;
        d.sizes
            .get(page as usize)
            .copied()
            .ok_or_else(|| format!("page {} is out of range", page + 1))
    }

    fn find_annot(&self, page: FPDF_PAGE, index: usize, name: Option<&str>) -> Option<c_int> {
        let lib = &*self.lib;
        // SAFETY: valid page.
        let count = unsafe { lib.FPDFPage_GetAnnotCount(page) };
        match name {
            Some(name) if !name.is_empty() => (0..count).find(|&i| {
                // SAFETY: index within count; annotation closed below.
                let a = unsafe { lib.FPDFPage_GetAnnot(page, i) };
                if a.is_null() {
                    return false;
                }
                let nm = annot_string(lib, a, "NM");
                // SAFETY: handle from GetAnnot.
                unsafe { lib.FPDFPage_CloseAnnot(a) };
                nm.as_deref() == Some(name)
            }),
            _ => (index < count as usize).then_some(index as c_int),
        }
    }

    /// Extracts a page's text. With `check_visibility`, also marks the
    /// characters that draw nothing a reader can see (see [`mark_hidden`]).
    fn page_text_of(&mut self, id: DocId, index: u32, check_visibility: bool) -> EResult<PageText> {
        let page = self.page(id, index)?;
        let lib = &*self.lib;
        // SAFETY: valid page; text page closed below.
        let tp = unsafe { lib.FPDFText_LoadPage(page) };
        if tp.is_null() {
            return Ok(PageText::default());
        }
        let mut text = read_text(lib, tp);
        // SAFETY: handle from LoadPage.
        unsafe { lib.FPDFText_ClosePage(tp) };
        if check_visibility && text.chars.iter().any(|c| c.rect.is_some()) {
            mark_hidden(lib, page, &mut text);
        }
        Ok(text)
    }
}

impl Drop for PdfiumEngine {
    fn drop(&mut self) {
        let ids: Vec<DocId> = self.docs.keys().copied().collect();
        for id in ids {
            self.close_doc(id);
        }
        let mut users = LIB_USERS.lock().unwrap_or_else(|e| e.into_inner());
        *users -= 1;
        if *users == 0 {
            // SAFETY: last user; every document is closed.
            unsafe { self.lib.FPDF_DestroyLibrary() };
        }
    }
}

fn close_page(lib: &dyn PdfiumLibraryBindings, form: FPDF_FORMHANDLE, page: FPDF_PAGE) {
    // SAFETY: page is a live handle from FPDF_LoadPage, closed exactly once.
    unsafe {
        if !form.is_null() {
            lib.FORM_OnBeforeClosePage(page, form);
        }
        lib.FPDF_ClosePage(page);
    }
}

fn close_doc_handles(lib: &dyn PdfiumLibraryBindings, mut doc: Doc) {
    for (_, p) in doc.pages.drain(..) {
        close_page(lib, doc.form, p);
    }
    // SAFETY: pages are closed; form exits before its document closes.
    unsafe {
        if !doc.form.is_null() {
            lib.FPDFDOC_ExitFormFillEnvironment(doc.form);
        }
        lib.FPDF_CloseDocument(doc.handle);
    }
    // `doc.access` and the form info drop here, after PDFium let go of them.
}

impl Engine for PdfiumEngine {
    fn open(
        &mut self,
        id: DocId,
        path: &Path,
        password: Option<&str>,
    ) -> Result<DocInfo, OpenError> {
        let (handle, access) = self.open_handle(path, password)?;
        let doc = self.build_doc(handle, access, path, password)?;
        let info = self.doc_info(&doc, path);
        if let Some(old) = self.docs.insert(id, doc) {
            close_doc_handles(&*self.lib, old);
        }
        Ok(info)
    }

    fn close(&mut self, id: DocId) {
        self.close_doc(id);
    }

    fn render(&mut self, key: &TileKey) -> EResult<Image> {
        let size = key.rotation.apply_size(self.size_of(key.doc, key.page)?);
        let mut img = match key.kind {
            TileKind::Tile { .. } => {
                let ((x, y, w, h), full) = key.pixel_rect(size);
                self.render_rect(
                    key.doc,
                    key.page,
                    key.rotation,
                    full,
                    (x as i32, y as i32, w, h),
                    false,
                )?
            }
            TileKind::Thumb => {
                let (w, h) = thumb_px(size);
                self.render_rect(key.doc, key.page, key.rotation, (w, h), (0, 0, w, h), false)?
            }
        };
        if key.invert {
            img.invert();
        }
        Ok(img)
    }

    fn render_region(&mut self, r: &RegionReq) -> EResult<Image> {
        let size = r.rotation.apply_size(self.size_of(r.doc, r.page)?);
        let full = page_px(size, r.scale);
        let mut img =
            self.render_rect(r.doc, r.page, r.rotation, full, (r.x, r.y, r.w, r.h), false)?;
        if r.invert {
            img.invert();
        }
        Ok(img)
    }

    fn render_page(
        &mut self,
        id: DocId,
        page: u32,
        w: u32,
        h: u32,
        printing: bool,
    ) -> EResult<Image> {
        self.render_rect(id, page, Rotation::R0, (w, h), (0, 0, w, h), printing)
    }

    fn page_size(&self, id: DocId, page: u32) -> Option<(f32, f32)> {
        self.docs
            .get(&id)
            .and_then(|d| d.sizes.get(page as usize).copied())
    }

    fn page_data(&mut self, id: DocId, index: u32) -> EResult<PageData> {
        let text = self.page_text_of(id, index, true)?;
        let page = self.page(id, index)?;
        let lib = &*self.lib;
        let doc = self.docs.get(&id).ok_or("document is not open")?;
        let mut bbox = FS_RECTF {
            left: 0.0,
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
        };
        // SAFETY: valid page and out pointer.
        let bbox = if unsafe { lib.FPDF_GetPageBoundingBox(page, &mut bbox) } != 0 {
            [bbox.left, bbox.bottom, bbox.right, bbox.top]
        } else {
            let (w, h) = doc.sizes[index as usize];
            [0.0, 0.0, w, h]
        };
        // SAFETY: valid page.
        let rotation = Rotation::from_quarters(unsafe { lib.FPDFPage_GetRotation(page) });
        let links = read_links(lib, doc.handle, page);
        let annots = read_annots(lib, page);
        let fields = read_fields(lib, doc.form, page);
        Ok(PageData {
            bbox,
            rotation,
            text,
            links,
            annots,
            fields,
        })
    }

    fn page_text(&mut self, id: DocId, page: u32) -> EResult<PageText> {
        // Search reads every page; skip the visibility pass to stay fast.
        self.page_text_of(id, page, false)
    }

    fn annotations(&mut self, id: DocId, index: u32) -> EResult<Vec<AnnotInfo>> {
        let page = self.page(id, index)?;
        Ok(read_annots(&*self.lib, page))
    }

    fn add_annot(&mut self, id: DocId, index: u32, annot: &NewAnnot) -> EResult<String> {
        let page = self.page(id, index)?;
        let lib = &*self.lib;
        let subtype = match annot {
            NewAnnot::Markup { kind, .. } => match kind {
                AnnotKind::Underline => ANNOT_UNDERLINE,
                AnnotKind::StrikeOut => ANNOT_STRIKEOUT,
                AnnotKind::Squiggly => ANNOT_SQUIGGLY,
                _ => ANNOT_HIGHLIGHT,
            },
            NewAnnot::Note { .. } => ANNOT_TEXT,
            NewAnnot::Ink { .. } => ANNOT_INK,
            NewAnnot::Shape {
                kind: AnnotKind::Circle,
                ..
            } => ANNOT_CIRCLE,
            NewAnnot::Shape { .. } => ANNOT_SQUARE,
        };
        // SAFETY: valid page; the annotation is closed at the end.
        let a = unsafe { lib.FPDFPage_CreateAnnot(page, subtype) };
        if a.is_null() {
            return Err("the annotation could not be created".into());
        }
        let name = unique_name();
        let now = pdf_now();
        let result = (|| -> EResult<()> {
            set_annot_string(lib, a, "NM", &name)?;
            set_annot_string(lib, a, "M", &now)?;
            set_annot_string(lib, a, "CreationDate", &now)?;
            if !self.author.is_empty() {
                set_annot_string(lib, a, "T", &self.author)?;
            }
            // SAFETY: valid annotation.
            unsafe { lib.FPDFAnnot_SetFlags(a, ANNOT_FLAG_PRINT) };
            let set_color = |c: &[u8; 3]| -> EResult<()> {
                // SAFETY: valid annotation.
                let ok = unsafe {
                    lib.FPDFAnnot_SetColor(
                        a,
                        COLORTYPE_COLOR as _,
                        c[0] as _,
                        c[1] as _,
                        c[2] as _,
                        255,
                    )
                };
                if ok == 0 {
                    Err("could not set the colour".into())
                } else {
                    Ok(())
                }
            };
            let set_rect = |r: [f32; 4]| -> EResult<()> {
                let fr = FS_RECTF {
                    left: r[0],
                    bottom: r[1],
                    right: r[2],
                    top: r[3],
                };
                // SAFETY: valid annotation and rect pointer.
                if unsafe { lib.FPDFAnnot_SetRect(a, &fr) } == 0 {
                    Err("could not set the rectangle".into())
                } else {
                    Ok(())
                }
            };
            match annot {
                NewAnnot::Markup {
                    rects, color, text, ..
                } => {
                    if rects.is_empty() {
                        return Err("nothing selected".into());
                    }
                    set_color(color)?;
                    let mut bounds = rects[0];
                    for r in rects {
                        let q = FS_QUADPOINTSF {
                            x1: r[0],
                            y1: r[3],
                            x2: r[2],
                            y2: r[3],
                            x3: r[0],
                            y3: r[1],
                            x4: r[2],
                            y4: r[1],
                        };
                        // SAFETY: valid annotation and quad pointer.
                        if unsafe { lib.FPDFAnnot_AppendAttachmentPoints(a, &q) } == 0 {
                            return Err("could not set the highlight area".into());
                        }
                        bounds = [
                            bounds[0].min(r[0]),
                            bounds[1].min(r[1]),
                            bounds[2].max(r[2]),
                            bounds[3].max(r[3]),
                        ];
                    }
                    set_rect(bounds)?;
                    if !text.is_empty() {
                        set_annot_string(lib, a, "Contents", text)?;
                    }
                }
                NewAnnot::Note { x, y, color, text } => {
                    set_color(color)?;
                    set_rect([*x, *y - 24.0, *x + 24.0, *y])?;
                    set_annot_string(lib, a, "Contents", text)?;
                }
                NewAnnot::Ink {
                    strokes,
                    color,
                    width,
                } => {
                    set_color(color)?;
                    let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
                    let mut any = false;
                    for s in strokes.iter().filter(|s| !s.is_empty()) {
                        let pts: Vec<FS_POINTF> =
                            s.iter().map(|&(x, y)| FS_POINTF { x, y }).collect();
                        for p in &pts {
                            b = [b[0].min(p.x), b[1].min(p.y), b[2].max(p.x), b[3].max(p.y)];
                        }
                        // SAFETY: valid annotation; pointer/len describe `pts`.
                        if unsafe { lib.FPDFAnnot_AddInkStroke(a, pts.as_ptr(), pts.len()) } < 0 {
                            return Err("could not add the ink stroke".into());
                        }
                        any = true;
                    }
                    if !any {
                        return Err("empty drawing".into());
                    }
                    // SAFETY: valid annotation.
                    unsafe { lib.FPDFAnnot_SetBorder(a, 0.0, 0.0, *width) };
                    let pad = width.max(1.0);
                    set_rect([b[0] - pad, b[1] - pad, b[2] + pad, b[3] + pad])?;
                }
                NewAnnot::Shape {
                    rect, color, width, ..
                } => {
                    set_color(color)?;
                    // SAFETY: valid annotation.
                    unsafe { lib.FPDFAnnot_SetBorder(a, 0.0, 0.0, *width) };
                    set_rect(*rect)?;
                }
            }
            Ok(())
        })();
        // SAFETY: handle from CreateAnnot.
        unsafe { lib.FPDFPage_CloseAnnot(a) };
        if let Err(e) = result {
            // Undo the half-built annotation.
            // SAFETY: valid page.
            let count = unsafe { lib.FPDFPage_GetAnnotCount(page) };
            if count > 0 {
                // SAFETY: the new annotation is the last one.
                unsafe { lib.FPDFPage_RemoveAnnot(page, count - 1) };
            }
            self.evict_page(id, index);
            return Err(e);
        }
        self.evict_page(id, index);
        Ok(name)
    }

    fn remove_annot(
        &mut self,
        id: DocId,
        index: u32,
        ai: usize,
        name: Option<&str>,
    ) -> EResult<()> {
        let page = self.page(id, index)?;
        let i = self
            .find_annot(page, ai, name)
            .ok_or("the annotation no longer exists")?;
        // SAFETY: valid page and index.
        let ok = unsafe { self.lib.FPDFPage_RemoveAnnot(page, i) };
        self.evict_page(id, index);
        if ok == 0 {
            Err("the annotation could not be removed".into())
        } else {
            Ok(())
        }
    }

    fn set_annot_contents(
        &mut self,
        id: DocId,
        index: u32,
        ai: usize,
        name: Option<&str>,
        text: &str,
    ) -> EResult<()> {
        let page = self.page(id, index)?;
        let i = self
            .find_annot(page, ai, name)
            .ok_or("the annotation no longer exists")?;
        let lib = &*self.lib;
        // SAFETY: valid page and index; closed below.
        let a = unsafe { lib.FPDFPage_GetAnnot(page, i) };
        if a.is_null() {
            return Err("the annotation could not be opened".into());
        }
        let r = set_annot_string(lib, a, "Contents", text)
            .and_then(|_| set_annot_string(lib, a, "M", &pdf_now()));
        // SAFETY: handle from GetAnnot.
        unsafe { lib.FPDFPage_CloseAnnot(a) };
        self.evict_page(id, index);
        r
    }

    fn form_input(&mut self, id: DocId, index: u32, input: &FormInput) -> EResult<FormOutcome> {
        let page = self.page(id, index)?;
        let lib = &*self.lib;
        let doc = self.docs.get_mut(&id).ok_or("document is not open")?;
        let form = doc.form;
        let Some(info) = doc.form_info.as_mut().filter(|_| !form.is_null()) else {
            return Ok(FormOutcome::default());
        };
        info.changed = false;
        let mods = |shift: bool, ctrl: bool| (shift as c_int) | ((ctrl as c_int) << 1);
        // SAFETY: page and form belong to the same open document.
        let handled = unsafe {
            match *input {
                FormInput::Down { x, y } => {
                    lib.FORM_OnMouseMove(form, page, 0, x as f64, y as f64);
                    lib.FORM_OnLButtonDown(form, page, 0, x as f64, y as f64)
                }
                FormInput::Up { x, y } => lib.FORM_OnLButtonUp(form, page, 0, x as f64, y as f64),
                FormInput::DoubleClick { x, y } => {
                    lib.FORM_OnLButtonDoubleClick(form, page, 0, x as f64, y as f64)
                }
                FormInput::Key { code, shift, ctrl } => {
                    lib.FORM_OnKeyDown(form, page, code, mods(shift, ctrl))
                }
                FormInput::Char(c) => lib.FORM_OnChar(form, page, c as c_int, 0),
                FormInput::KillFocus => lib.FORM_ForceToKillFocus(form),
            }
        } != 0;
        let mut page_index: c_int = -1;
        let mut annot: FPDF_ANNOTATION = null_mut();
        // SAFETY: out pointers are valid; a returned annotation is closed right away.
        let focused = unsafe { lib.FORM_GetFocusedAnnot(form, &mut page_index, &mut annot) } != 0
            && !annot.is_null();
        if !annot.is_null() {
            // SAFETY: handle returned by FORM_GetFocusedAnnot.
            unsafe { lib.FPDFPage_CloseAnnot(annot) };
        }
        Ok(FormOutcome {
            handled,
            focused,
            changed: info.changed,
        })
    }

    fn save(&mut self, id: DocId, target: &Path) -> EResult<()> {
        let lib = &*self.lib;
        let doc = self.docs.get(&id).ok_or("document is not open")?;
        if !doc.form.is_null() {
            // Commit a field that is still being edited.
            // SAFETY: valid form handle.
            unsafe { lib.FORM_ForceToKillFocus(doc.form) };
        }
        let dir = target
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let stem = target
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "document".into());
        let tmp = dir.join(format!("{stem}.bdf-saving.pdf"));
        // SAFETY: valid handle.
        let incremental = unsafe { lib.FPDF_DocumentHasValidCrossReferenceTable(doc.handle) } != 0;
        {
            let file =
                File::create(&tmp).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
            let mut w = Box::new(Writer::new(file));
            let flags = if incremental {
                FPDF_INCREMENTAL
            } else {
                FPDF_NO_INCREMENTAL
            };
            // SAFETY: `w` lives across the call; its first field is the FPDF_FILEWRITE.
            let ok = unsafe { lib.FPDF_SaveAsCopy(doc.handle, &mut w.fw, flags as _) };
            let finished = w.finish();
            if ok == 0 || finished.is_err() {
                let _ = std::fs::remove_file(&tmp);
                return Err(match finished {
                    Err(e) => format!("writing failed: {e}"),
                    Ok(()) => "PDFium could not save this document".into(),
                });
            }
        }
        // Prove the new file opens before touching the original.
        let check = self.open_handle(&tmp, doc.password.as_deref());
        match check {
            Ok((h, _access)) => {
                // SAFETY: valid handle; closed right away.
                let pages = unsafe { lib.FPDF_GetPageCount(h) };
                unsafe { lib.FPDF_CloseDocument(h) };
                if pages as usize != doc.sizes.len() {
                    let _ = std::fs::remove_file(&tmp);
                    return Err(
                        "the saved copy failed verification; the original is unchanged".into(),
                    );
                }
            }
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(format!(
                    "the saved copy failed verification ({e}); the original is unchanged"
                ));
            }
        }
        let password = doc.password.clone();
        let same_file = same_path(&doc.path, target);
        // Close our handle so the file can be replaced (Windows locks open files).
        let doc = self.docs.remove(&id).expect("checked above");
        let old_path = doc.path.clone();
        close_doc_handles(lib, doc);
        let replaced = replace_file(&tmp, target);
        let (reopen_path, err) = match replaced {
            Ok(()) => (target.to_path_buf(), None),
            Err(e) => {
                // Keep the user's changes: continue from the verified copy.
                let msg = if same_file {
                    format!(
                        "{} could not be replaced ({e}). Your changes are in {}.",
                        target.display(),
                        tmp.display()
                    )
                } else {
                    format!(
                        "{} could not be written ({e}). Your changes are in {}.",
                        target.display(),
                        tmp.display()
                    )
                };
                (tmp.clone(), Some(msg))
            }
        };
        let reopened = self
            .open_handle(&reopen_path, password.as_deref())
            .and_then(|(h, a)| self.build_doc(h, a, &reopen_path, password.as_deref()));
        match reopened {
            Ok(d) => {
                self.docs.insert(id, d);
            }
            Err(e) => {
                // Last resort: the original file.
                if let Ok(d) = self
                    .open_handle(&old_path, password.as_deref())
                    .and_then(|(h, a)| self.build_doc(h, a, &old_path, password.as_deref()))
                {
                    self.docs.insert(id, d);
                }
                return Err(format!(
                    "saved, but the document could not be reopened: {e}"
                ));
            }
        }
        match err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

impl PdfiumEngine {
    /// Where an open document currently lives (changes after Save As).
    pub fn doc_path(&self, id: DocId) -> Option<&Path> {
        self.docs.get(&id).map(|d| d.path.as_path())
    }

    #[doc(hidden)]
    pub fn loaded_pages(&mut self, id: DocId) -> usize {
        self.doc(id).map(|d| d.pages.len()).unwrap_or(0)
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Moves `tmp` over `target`, retrying briefly (antivirus and indexers hold
/// files for a moment on Windows), then falls back to copying.
fn replace_file(tmp: &Path, target: &Path) -> std::io::Result<()> {
    let mut last = None;
    for attempt in 0..5 {
        match std::fs::rename(tmp, target) {
            Ok(()) => return Ok(()),
            Err(e) => last = Some(e),
        }
        std::thread::sleep(std::time::Duration::from_millis(50 * (attempt + 1)));
    }
    match std::fs::copy(tmp, target) {
        Ok(_) => {
            let _ = std::fs::remove_file(tmp);
            Ok(())
        }
        Err(_) => Err(last.unwrap_or_else(|| std::io::Error::other("rename failed"))),
    }
}

// ---------------------------------------------------------------------------
// File access: PDFium pulls bytes through this callback.

#[repr(C)]
struct Access {
    // Must stay the first field: PDFium gets a pointer to it.
    fa: FPDF_FILEACCESS,
    file: File,
    len: u64,
    /// Small read-through cache: PDFium issues many tiny reads.
    blocks: Vec<(u64, Box<[u8]>)>,
}

const BLOCK: u64 = 128 * 1024;
const MAX_BLOCKS: usize = 16;

impl Access {
    fn new(file: File, len: u64) -> Box<Self> {
        let mut a = Box::new(Access {
            fa: FPDF_FILEACCESS {
                m_FileLen: c_ulong::try_from(len).unwrap_or(c_ulong::MAX),
                m_GetBlock: Some(get_block),
                m_Param: null_mut(),
            },
            file,
            len,
            blocks: Vec::new(),
        });
        let p: *mut Access = &mut *a;
        a.fa.m_Param = p.cast();
        a
    }

    fn read_at(&mut self, pos: u64, out: &mut [u8]) -> std::io::Result<()> {
        let end = pos
            .checked_add(out.len() as u64)
            .ok_or_else(|| std::io::Error::other("overflow"))?;
        if end > self.len {
            return Err(std::io::Error::other("read past end"));
        }
        if out.len() as u64 >= BLOCK {
            return read_exact_at(&self.file, pos, out);
        }
        let mut done = 0usize;
        while done < out.len() {
            let p = pos + done as u64;
            let bi = p / BLOCK;
            let data = self.block(bi)?;
            let off = (p - bi * BLOCK) as usize;
            let n = (data.len() - off).min(out.len() - done);
            if n == 0 {
                return Err(std::io::Error::other("short block"));
            }
            out[done..done + n].copy_from_slice(&data[off..off + n]);
            done += n;
        }
        Ok(())
    }

    fn block(&mut self, bi: u64) -> std::io::Result<&[u8]> {
        if let Some(i) = self.blocks.iter().position(|(b, _)| *b == bi) {
            let e = self.blocks.remove(i);
            self.blocks.push(e);
        } else {
            let start = bi * BLOCK;
            let n = BLOCK.min(self.len - start) as usize;
            let mut buf = vec![0u8; n].into_boxed_slice();
            read_exact_at(&self.file, start, &mut buf)?;
            if self.blocks.len() >= MAX_BLOCKS {
                self.blocks.remove(0);
            }
            self.blocks.push((bi, buf));
        }
        Ok(&self.blocks.last().expect("just pushed").1)
    }

    /// Hash of the first 64 KiB, for documents without a file ID.
    fn head_hash(&self) -> u64 {
        let n = self.len.min(65536) as usize;
        let mut buf = vec![0u8; n];
        if read_exact_at(&self.file, 0, &mut buf).is_err() {
            return 0;
        }
        fnv1a(&buf)
    }
}

pub(crate) fn fnv1a(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in data {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

#[cfg(unix)]
fn read_exact_at(f: &File, pos: u64, out: &mut [u8]) -> std::io::Result<()> {
    use std::os::unix::fs::FileExt;
    f.read_exact_at(out, pos)
}

#[cfg(windows)]
fn read_exact_at(f: &File, mut pos: u64, mut out: &mut [u8]) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !out.is_empty() {
        match f.seek_read(out, pos) {
            Ok(0) => return Err(std::io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => {
                out = &mut out[n..];
                pos += n as u64;
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

unsafe extern "C" fn get_block(
    param: *mut c_void,
    pos: c_ulong,
    buf: *mut c_uchar,
    size: c_ulong,
) -> c_int {
    if param.is_null() || buf.is_null() {
        return 0;
    }
    // SAFETY: `param` is the boxed Access set up in Access::new, alive while the
    // document is open; PDFium guarantees `buf` holds `size` bytes.
    let (access, out) = unsafe {
        (
            &mut *(param as *mut Access),
            std::slice::from_raw_parts_mut(buf, size as usize),
        )
    };
    // `pos` is c_ulong: 32-bit on Windows, 64-bit elsewhere.
    #[allow(clippy::useless_conversion)]
    let pos = u64::from(pos);
    match access.read_at(pos, out) {
        Ok(()) => 1,
        Err(e) => {
            log::warn!("read of {size} bytes at {pos} failed: {e}");
            0
        }
    }
}

// ---------------------------------------------------------------------------
// Form filling.

#[repr(C)]
struct FormInfo {
    // Must stay the first field: PDFium gets a pointer to it.
    info: FPDF_FORMFILLINFO,
    /// Set by PDFium whenever a field value changes.
    changed: bool,
}

unsafe extern "C" fn on_form_change(this: *mut FPDF_FORMFILLINFO) {
    if !this.is_null() {
        // SAFETY: `this` is the first field of a live, boxed FormInfo.
        unsafe { (*(this as *mut FormInfo)).changed = true };
    }
}

// ---------------------------------------------------------------------------
// File writing for saves.

#[repr(C)]
struct Writer {
    // Must stay the first field: PDFium gets a pointer to it.
    fw: FPDF_FILEWRITE,
    out: Option<BufWriter<File>>,
    error: Option<std::io::Error>,
}

impl Writer {
    fn new(file: File) -> Self {
        Self {
            fw: FPDF_FILEWRITE {
                version: 1,
                WriteBlock: Some(write_block),
            },
            out: Some(BufWriter::with_capacity(1 << 20, file)),
            error: None,
        }
    }

    fn finish(&mut self) -> std::io::Result<()> {
        if let Some(e) = self.error.take() {
            return Err(e);
        }
        let w = self
            .out
            .take()
            .ok_or_else(|| std::io::Error::other("already finished"))?;
        let f = w.into_inner().map_err(|e| e.into_error())?;
        f.sync_all()
    }
}

unsafe extern "C" fn write_block(
    this: *mut FPDF_FILEWRITE,
    data: *const c_void,
    size: c_ulong,
) -> c_int {
    if this.is_null() || (data.is_null() && size > 0) {
        return 0;
    }
    // SAFETY: `this` points at the first field of a live, boxed Writer.
    let w = unsafe { &mut *(this as *mut Writer) };
    if w.error.is_some() {
        return 0;
    }
    let bytes = if size == 0 {
        &[][..]
    } else {
        // SAFETY: PDFium passes `size` readable bytes.
        unsafe { std::slice::from_raw_parts(data as *const u8, size as usize) }
    };
    match w.out.as_mut().map(|o| o.write_all(bytes)) {
        Some(Ok(())) => 1,
        Some(Err(e)) => {
            w.error = Some(e);
            0
        }
        None => 0,
    }
}

// ---------------------------------------------------------------------------
// Readers.

/// Calls a PDFium "fill this UTF-16 buffer" function twice (size, then data).
fn utf16_buffer(mut f: impl FnMut(*mut c_void, c_ulong) -> c_ulong) -> Option<String> {
    let n = f(null_mut(), 0) as usize;
    if !(2..=1 << 24).contains(&n) {
        return None;
    }
    let mut buf = vec![0u16; n.div_ceil(2)];
    let got = f(buf.as_mut_ptr().cast(), (buf.len() * 2) as c_ulong) as usize;
    if got < 2 {
        return None;
    }
    let units = (got / 2).min(buf.len());
    let s = String::from_utf16_lossy(&buf[..units]);
    Some(s.trim_end_matches('\0').to_string())
}

fn annot_string(lib: &dyn PdfiumLibraryBindings, a: FPDF_ANNOTATION, key: &str) -> Option<String> {
    let n =
        // SAFETY: NULL buffer asks for the size.
        unsafe { lib.FPDFAnnot_GetStringValue(a, key, null_mut(), 0) } as usize;
    if !(2..=1 << 24).contains(&n) {
        return None;
    }
    let mut buf: Vec<FPDF_WCHAR> = vec![0; n.div_ceil(2)];
    // SAFETY: the buffer has the size PDFium asked for.
    let got = unsafe {
        lib.FPDFAnnot_GetStringValue(a, key, buf.as_mut_ptr(), (buf.len() * 2) as c_ulong)
    } as usize;
    let units = (got / 2).min(buf.len());
    let s = String::from_utf16_lossy(&buf[..units]);
    let s = s.trim_end_matches('\0').to_string();
    (!s.is_empty()).then_some(s)
}

fn set_annot_string(
    lib: &dyn PdfiumLibraryBindings,
    a: FPDF_ANNOTATION,
    key: &str,
    value: &str,
) -> EResult<()> {
    let w: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: NUL-terminated UTF-16 buffer alive for the call.
    if unsafe { lib.FPDFAnnot_SetStringValue(a, key, w.as_ptr()) } == 0 {
        Err(format!("could not set {key}"))
    } else {
        Ok(())
    }
}

fn read_text(lib: &dyn PdfiumLibraryBindings, tp: FPDF_TEXTPAGE) -> PageText {
    // SAFETY: valid text page for all calls below; indices within count.
    let n = unsafe { lib.FPDFText_CountChars(tp) }.max(0);
    let mut chars = Vec::with_capacity(n as usize);
    let mut i = 0;
    while i < n {
        let u = unsafe { lib.FPDFText_GetUnicode(tp, i) };
        let mut ch = char::from_u32(u);
        let mut step = 1;
        // PDFium reports UTF-16 units; join surrogate pairs.
        if (0xD800..0xDC00).contains(&u) && i + 1 < n {
            let lo = unsafe { lib.FPDFText_GetUnicode(tp, i + 1) };
            if (0xDC00..0xE000).contains(&lo) {
                ch = char::from_u32(0x10000 + ((u - 0xD800) << 10) + (lo - 0xDC00));
                step = 2;
            }
        }
        let ch = ch.unwrap_or('\u{FFFD}');
        let generated = unsafe { lib.FPDFText_IsGenerated(tp, i) } == 1;
        let rect = if generated || ch == '\r' || ch == '\n' {
            None
        } else {
            let mut r = FS_RECTF {
                left: 0.0,
                top: 0.0,
                right: 0.0,
                bottom: 0.0,
            };
            let ok = unsafe { lib.FPDFText_GetLooseCharBox(tp, i, &mut r) } != 0;
            let (l, rr) = (r.left.min(r.right), r.left.max(r.right));
            let (b, t) = (r.bottom.min(r.top), r.bottom.max(r.top));
            (ok && rr > l && t > b && [l, b, rr, t].iter().all(|v| v.is_finite()))
                .then_some([l, b, rr, t])
        };
        chars.push(TextChar {
            ch,
            rect,
            hidden: false,
        });
        i += step;
    }
    PageText { chars }
}

/// Marks characters that put no ink on the page.
///
/// The page is rendered once in grey (without annotations or form fields)
/// and every glyph box is checked for contrast. A glyph is hidden when its
/// box is flat: invisible-mode text over a blank area, text covered by an
/// image or a white box, white on white, clipped away, on a layer that is
/// switched off, or off the page. OCR text over a scanned image stays
/// visible because the scan supplies the ink.
fn mark_hidden(lib: &dyn PdfiumLibraryBindings, page: FPDF_PAGE, text: &mut PageText) {
    // SAFETY: valid page.
    let (pw, ph) = unsafe { (lib.FPDF_GetPageWidthF(page), lib.FPDF_GetPageHeightF(page)) };
    if !(pw > 0.0 && ph > 0.0) {
        return;
    }
    // About 1.5 px per point, capped at ~3 megapixels for posters and maps.
    let k = (1.5f32).min((3_000_000.0 / (pw * ph)).sqrt()).max(0.25);
    let (w, h) = (
        ((pw * k).round() as c_int).max(1),
        ((ph * k).round() as c_int).max(1),
    );
    // SAFETY: sizes are positive and bounded; NULL is handled.
    let bmp = unsafe { lib.FPDFBitmap_Create(w, h, 0) };
    if bmp.is_null() {
        return;
    }
    // SAFETY: valid bitmap and page; the buffer is read within its stride.
    unsafe {
        lib.FPDFBitmap_FillRect(bmp, 0, 0, w, h, 0xFFFF_FFFF);
        lib.FPDF_RenderPageBitmap(bmp, page, 0, 0, w, h, 0, FPDF_GRAYSCALE);
        let buf = lib.FPDFBitmap_GetBuffer(bmp) as *const u8;
        let stride = lib.FPDFBitmap_GetStride(bmp) as usize;
        if !buf.is_null() && stride >= w as usize * 4 {
            let pixels = std::slice::from_raw_parts(buf, stride * h as usize);
            for c in text.chars.iter_mut() {
                let Some(r) = c.rect else { continue };
                let to_dev = |x: f32, y: f32| {
                    let (mut dx, mut dy) = (0, 0);
                    lib.FPDF_PageToDevice(
                        page, 0, 0, w, h, 0, x as f64, y as f64, &mut dx, &mut dy,
                    );
                    (dx, dy)
                };
                // Shrink the box a little vertically so the line above or
                // below does not lend its ink.
                let inset = (r[3] - r[1]) * 0.12;
                let (ax, ay) = to_dev(r[0], r[1] + inset);
                let (bx, by) = to_dev(r[2], r[3] - inset);
                let (x0, x1) = (ax.min(bx).clamp(0, w - 1), ax.max(bx).clamp(0, w - 1));
                let (y0, y1) = (ay.min(by).clamp(0, h - 1), ay.max(by).clamp(0, h - 1));
                let off_page =
                    ax.max(bx) < 0 || ax.min(bx) >= w || ay.max(by) < 0 || ay.min(by) >= h;
                let (mut lo, mut hi) = (255u8, 0u8);
                for y in y0..=y1 {
                    let row = &pixels[y as usize * stride..];
                    for x in x0..=x1 {
                        let v = row[x as usize * 4 + 1]; // grey: any channel
                        lo = lo.min(v);
                        hi = hi.max(v);
                    }
                }
                c.hidden = off_page || hi.saturating_sub(lo) < 40;
            }
        }
        lib.FPDFBitmap_Destroy(bmp);
    }
}

fn dest_target(
    lib: &dyn PdfiumLibraryBindings,
    doc: FPDF_DOCUMENT,
    dest: FPDF_DEST,
) -> Option<(u32, Option<f32>)> {
    if dest.is_null() {
        return None;
    }
    // SAFETY: valid document and destination handles.
    let page = unsafe { lib.FPDFDest_GetDestPageIndex(doc, dest) };
    if page < 0 {
        return None;
    }
    let (mut hx, mut hy, mut hz) = (0, 0, 0);
    let (mut x, mut y, mut z) = (0.0f32, 0.0f32, 0.0f32);
    // SAFETY: valid destination and out pointers.
    let ok = unsafe {
        lib.FPDFDest_GetLocationInPage(dest, &mut hx, &mut hy, &mut hz, &mut x, &mut y, &mut z)
    };
    let y = (ok != 0 && hy != 0 && y.is_finite()).then_some(y);
    Some((page as u32, y))
}

fn action_target(
    lib: &dyn PdfiumLibraryBindings,
    doc: FPDF_DOCUMENT,
    action: FPDF_ACTION,
) -> Option<LinkTarget> {
    if action.is_null() {
        return None;
    }
    // SAFETY: valid action handle.
    match unsafe { lib.FPDFAction_GetType(action) } {
        PDFACTION_GOTO => {
            // SAFETY: GOTO actions have a destination.
            let dest = unsafe { lib.FPDFAction_GetDest(doc, action) };
            dest_target(lib, doc, dest).map(|(page, y)| LinkTarget::Page { page, y })
        }
        PDFACTION_URI => {
            // SAFETY: size query, then a buffer of that size.
            let n = unsafe { lib.FPDFAction_GetURIPath(doc, action, null_mut(), 0) } as usize;
            if !(2..=1 << 16).contains(&n) {
                return None;
            }
            let mut buf = vec![0u8; n];
            unsafe {
                lib.FPDFAction_GetURIPath(doc, action, buf.as_mut_ptr().cast(), n as c_ulong)
            };
            let s = String::from_utf8_lossy(&buf)
                .trim_end_matches('\0')
                .trim()
                .to_string();
            (!s.is_empty()).then_some(LinkTarget::Uri(s))
        }
        _ => None,
    }
}

fn read_links(lib: &dyn PdfiumLibraryBindings, doc: FPDF_DOCUMENT, page: FPDF_PAGE) -> Vec<Link> {
    let mut out = Vec::new();
    let mut pos: c_int = 0;
    let mut link: FPDF_LINK = null_mut();
    // SAFETY: standard enumeration protocol; `pos` advances each call.
    while unsafe { lib.FPDFLink_Enumerate(page, &mut pos, &mut link) } != 0 {
        if link.is_null() || out.len() > 10_000 {
            break;
        }
        let mut r = FS_RECTF {
            left: 0.0,
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
        };
        // SAFETY: valid link handle.
        if unsafe { lib.FPDFLink_GetAnnotRect(link, &mut r) } == 0 {
            continue;
        }
        let rect = [
            r.left.min(r.right),
            r.bottom.min(r.top),
            r.left.max(r.right),
            r.bottom.max(r.top),
        ];
        // SAFETY: valid handles.
        let dest = unsafe { lib.FPDFLink_GetDest(doc, link) };
        let target = dest_target(lib, doc, dest)
            .map(|(page, y)| LinkTarget::Page { page, y })
            .or_else(|| {
                // SAFETY: valid link handle.
                let action = unsafe { lib.FPDFLink_GetAction(link) };
                action_target(lib, doc, action)
            });
        if let Some(target) = target {
            out.push(Link { rect, target });
        }
    }
    out
}

fn read_fields(
    lib: &dyn PdfiumLibraryBindings,
    form: FPDF_FORMHANDLE,
    page: FPDF_PAGE,
) -> Vec<FormField> {
    let mut out = Vec::new();
    if form.is_null() {
        return out;
    }
    // SAFETY: valid page and form; each annotation is closed after reading.
    let count = unsafe { lib.FPDFPage_GetAnnotCount(page) };
    for i in 0..count.min(10_000) {
        let a = unsafe { lib.FPDFPage_GetAnnot(page, i) };
        if a.is_null() {
            continue;
        }
        if unsafe { lib.FPDFAnnot_GetSubtype(a) } == ANNOT_WIDGET {
            let mut r = FS_RECTF {
                left: 0.0,
                top: 0.0,
                right: 0.0,
                bottom: 0.0,
            };
            if unsafe { lib.FPDFAnnot_GetRect(a, &mut r) } != 0 {
                let kind = match unsafe { lib.FPDFAnnot_GetFormFieldType(form, a) } {
                    1 => FieldKind::PushButton,
                    2 => FieldKind::CheckBox,
                    3 => FieldKind::RadioButton,
                    4 => FieldKind::ComboBox,
                    5 => FieldKind::ListBox,
                    6 => FieldKind::Text,
                    7 => FieldKind::Signature,
                    _ => FieldKind::Unknown,
                };
                let read_only = unsafe { lib.FPDFAnnot_GetFormFieldFlags(form, a) } & 1 != 0;
                out.push(FormField {
                    rect: [
                        r.left.min(r.right),
                        r.bottom.min(r.top),
                        r.left.max(r.right),
                        r.bottom.max(r.top),
                    ],
                    kind,
                    read_only,
                });
            }
        }
        unsafe { lib.FPDFPage_CloseAnnot(a) };
    }
    out
}

fn read_annots(lib: &dyn PdfiumLibraryBindings, page: FPDF_PAGE) -> Vec<AnnotInfo> {
    let mut out = Vec::new();
    // SAFETY: valid page; each annotation is closed after reading.
    let count = unsafe { lib.FPDFPage_GetAnnotCount(page) };
    for i in 0..count.min(10_000) {
        let a = unsafe { lib.FPDFPage_GetAnnot(page, i) };
        if a.is_null() {
            continue;
        }
        let subtype = unsafe { lib.FPDFAnnot_GetSubtype(a) };
        let kind = match subtype {
            ANNOT_LINK | ANNOT_POPUP | ANNOT_WIDGET | ANNOT_XFAWIDGET => None,
            ANNOT_TEXT => Some(AnnotKind::Note),
            ANNOT_FREETEXT => Some(AnnotKind::FreeText),
            ANNOT_LINE => Some(AnnotKind::Line),
            ANNOT_SQUARE => Some(AnnotKind::Square),
            ANNOT_CIRCLE => Some(AnnotKind::Circle),
            ANNOT_HIGHLIGHT => Some(AnnotKind::Highlight),
            ANNOT_UNDERLINE => Some(AnnotKind::Underline),
            ANNOT_SQUIGGLY => Some(AnnotKind::Squiggly),
            ANNOT_STRIKEOUT => Some(AnnotKind::StrikeOut),
            ANNOT_STAMP => Some(AnnotKind::Stamp),
            ANNOT_INK => Some(AnnotKind::Ink),
            _ => Some(AnnotKind::Other),
        };
        if let Some(kind) = kind {
            let mut r = FS_RECTF {
                left: 0.0,
                top: 0.0,
                right: 0.0,
                bottom: 0.0,
            };
            let has_rect = unsafe { lib.FPDFAnnot_GetRect(a, &mut r) } != 0;
            let (mut cr, mut cg, mut cb, mut ca) = (0u32, 0u32, 0u32, 0u32);
            let color = (unsafe {
                lib.FPDFAnnot_GetColor(a, COLORTYPE_COLOR as _, &mut cr, &mut cg, &mut cb, &mut ca)
            } != 0)
                .then_some([cr.min(255) as u8, cg.min(255) as u8, cb.min(255) as u8]);
            if has_rect {
                out.push(AnnotInfo {
                    index: i as usize,
                    name: annot_string(lib, a, "NM"),
                    kind,
                    rect: [
                        r.left.min(r.right),
                        r.bottom.min(r.top),
                        r.left.max(r.right),
                        r.bottom.max(r.top),
                    ],
                    contents: annot_string(lib, a, "Contents").unwrap_or_default(),
                    author: annot_string(lib, a, "T").unwrap_or_default(),
                    color,
                });
            }
        }
        unsafe { lib.FPDFPage_CloseAnnot(a) };
    }
    out
}

fn read_outline(lib: &dyn PdfiumLibraryBindings, doc: FPDF_DOCUMENT) -> Vec<OutlineItem> {
    let mut seen: HashSet<usize> = HashSet::new();
    let mut budget = 50_000usize;
    read_outline_level(lib, doc, null_mut(), 0, &mut seen, &mut budget)
}

fn read_outline_level(
    lib: &dyn PdfiumLibraryBindings,
    doc: FPDF_DOCUMENT,
    parent: FPDF_BOOKMARK,
    depth: usize,
    seen: &mut HashSet<usize>,
    budget: &mut usize,
) -> Vec<OutlineItem> {
    let mut out = Vec::new();
    if depth > 32 {
        return out;
    }
    // SAFETY: NULL parent means the root; handles come from PDFium.
    let mut bm = unsafe { lib.FPDFBookmark_GetFirstChild(doc, parent) };
    while !bm.is_null() && *budget > 0 {
        // Malformed outlines can loop; stop at the first repeat.
        if !seen.insert(bm as usize) {
            break;
        }
        *budget -= 1;
        let title = utf16_buffer(|buf, len| unsafe { lib.FPDFBookmark_GetTitle(bm, buf, len) })
            .unwrap_or_default();
        let dest = unsafe { lib.FPDFBookmark_GetDest(doc, bm) };
        let (mut page, mut y, mut uri) = (None, None, None);
        if let Some((p, yy)) = dest_target(lib, doc, dest) {
            page = Some(p);
            y = yy;
        } else {
            let action = unsafe { lib.FPDFBookmark_GetAction(bm) };
            match action_target(lib, doc, action) {
                Some(LinkTarget::Page { page: p, y: yy }) => {
                    page = Some(p);
                    y = yy;
                }
                Some(LinkTarget::Uri(u)) => uri = Some(u),
                None => {}
            }
        }
        let children = read_outline_level(lib, doc, bm, depth + 1, seen, budget);
        out.push(OutlineItem {
            title: title.replace(['\r', '\n'], " ").trim().to_string(),
            page,
            y,
            uri,
            children,
        });
        bm = unsafe { lib.FPDFBookmark_GetNextSibling(doc, bm) };
    }
    out
}

// ---------------------------------------------------------------------------
// Small helpers.

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unique_name() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!(
        "bdf-{nanos:x}-{:x}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

/// Current UTC time as a PDF date string.
pub fn pdf_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, mo, d, h, mi, s) = civil_from_unix(secs as i64);
    format!("D:{y:04}{mo:02}{d:02}{h:02}{mi:02}{s:02}Z")
}

/// UTC calendar fields from Unix seconds (Howard Hinnant's algorithm).
pub fn civil_from_unix(t: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = t.div_euclid(86_400);
    let rem = t.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (
        y,
        m,
        d,
        (rem / 3600) as u32,
        ((rem % 3600) / 60) as u32,
        (rem % 60) as u32,
    )
}

/// "D:20230102030405+01'00'" -> "2023-01-02 03:04:05 +01:00". Unparseable
/// input is returned unchanged.
pub fn format_pdf_date(s: &str) -> String {
    let t = s.trim().trim_start_matches("D:");
    let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.len() < 4 {
        return s.to_string();
    }
    let part = |a: usize, b: usize, def: &str| digits.get(a..b).unwrap_or(def).to_string();
    let mut out = format!(
        "{}-{}-{} {}:{}:{}",
        part(0, 4, "0000"),
        part(4, 6, "01"),
        part(6, 8, "01"),
        part(8, 10, "00"),
        part(10, 12, "00"),
        part(12, 14, "00")
    );
    let tz = &t[digits.len()..];
    if tz.starts_with('Z') {
        out.push_str(" UTC");
    } else if let Some(sign) = tz.chars().next().filter(|c| *c == '+' || *c == '-') {
        let tzd: String = tz[1..].chars().filter(|c| c.is_ascii_digit()).collect();
        if tzd.len() >= 2 {
            out.push_str(&format!(
                " {sign}{}:{}",
                &tzd[0..2],
                tzd.get(2..4).unwrap_or("00")
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(
            format_pdf_date("D:20230102030405+01'00'"),
            "2023-01-02 03:04:05 +01:00"
        );
        assert_eq!(format_pdf_date("D:2023"), "2023-01-01 00:00:00");
        assert_eq!(
            format_pdf_date("D:20230102030405Z"),
            "2023-01-02 03:04:05 UTC"
        );
        assert_eq!(format_pdf_date("garbage"), "garbage");
        assert_eq!(civil_from_unix(0), (1970, 1, 1, 0, 0, 0));
        assert_eq!(civil_from_unix(951_782_400), (2000, 2, 29, 0, 0, 0));
        assert_eq!(civil_from_unix(1_798_761_599), (2026, 12, 31, 23, 59, 59));
        assert!(pdf_now().starts_with("D:20"));
    }

    #[test]
    fn names_are_unique() {
        assert_ne!(unique_name(), unique_name());
    }
}
