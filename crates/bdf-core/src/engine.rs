//! The document engine interface.
//!
//! [`Engine`] is what the render worker drives. The real implementation is
//! [`crate::pdfium::PdfiumEngine`]; tests drive the worker with a fake.

use crate::geom::{Rect, Rotation, page_px, scale_of};
use crate::text::PageText;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DocId(pub u64);

/// Side of a square render tile, in device pixels.
pub const TILE: u32 = 1024;
/// Longest side of a thumbnail, in device pixels.
pub const THUMB: u32 = 240;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TileKind {
    /// One tile of a page rendered at `scale` (see [`crate::geom::quantize_scale`]).
    Tile { scale: u32, tx: u16, ty: u16 },
    /// The whole page with its longest side [`THUMB`] pixels.
    Thumb,
}

/// Identifies one rendered image. `gen` changes whenever a page's content
/// changes (annotation edits), so stale images are never reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TileKey {
    pub doc: DocId,
    pub page: u32,
    pub rotation: Rotation,
    pub invert: bool,
    pub generation: u32,
    pub kind: TileKind,
}

impl TileKey {
    /// Pixel rectangle `(x, y, w, h)` this key covers within the whole page
    /// rendered at its scale, plus the whole page's pixel size.
    /// `size_pt` is the page's display size including the view rotation.
    pub fn pixel_rect(&self, size_pt: (f32, f32)) -> ((u32, u32, u32, u32), (u32, u32)) {
        match self.kind {
            TileKind::Tile { scale, tx, ty } => {
                let (pw, ph) = page_px(size_pt, scale);
                let x = (tx as u32 * TILE).min(pw);
                let y = (ty as u32 * TILE).min(ph);
                let w = TILE.min(pw - x);
                let h = TILE.min(ph - y);
                ((x, y, w, h), (pw, ph))
            }
            TileKind::Thumb => {
                let (pw, ph) = thumb_px(size_pt);
                ((0, 0, pw, ph), (pw, ph))
            }
        }
    }
}

/// Thumbnail pixel size for a page.
pub fn thumb_px(size_pt: (f32, f32)) -> (u32, u32) {
    let (w, h) = (size_pt.0.max(1.0), size_pt.1.max(1.0));
    let s = THUMB as f32 / w.max(h);
    (
        ((w * s).round() as u32).max(1),
        ((h * s).round() as u32).max(1),
    )
}

/// Number of tiles across and down for a page at a scale.
pub fn tile_grid(size_pt: (f32, f32), scale: u32) -> (u16, u16) {
    let (pw, ph) = page_px(size_pt, scale);
    (
        pw.div_ceil(TILE).min(u16::MAX as u32) as u16,
        ph.div_ceil(TILE).min(u16::MAX as u32) as u16,
    )
}

/// The scale (device px per point) that the tile grid at `scale` represents.
pub fn tile_scale(kind: &TileKind) -> Option<f32> {
    match kind {
        TileKind::Tile { scale, .. } => Some(scale_of(*scale)),
        TileKind::Thumb => None,
    }
}

/// An RGBA8 image, rows packed.
#[derive(Clone)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl fmt::Debug for Image {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Image({}x{})", self.width, self.height)
    }
}

impl Image {
    pub fn invert(&mut self) {
        for px in self.rgba.chunks_exact_mut(4) {
            px[0] = 255 - px[0];
            px[1] = 255 - px[1];
            px[2] = 255 - px[2];
        }
    }
}

/// A magnifier request: an arbitrary pixel rectangle of a page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RegionReq {
    pub doc: DocId,
    pub page: u32,
    pub rotation: Rotation,
    pub invert: bool,
    pub scale: u32,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutlineItem {
    pub title: String,
    pub page: Option<u32>,
    /// Target y in user space, when the destination specifies one.
    pub y: Option<f32>,
    pub uri: Option<String>,
    pub children: Vec<OutlineItem>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DocInfo {
    pub path: PathBuf,
    pub file_size: u64,
    /// Display size of each page in points (page `/Rotate` applied).
    pub page_sizes: Vec<(f32, f32)>,
    pub page_labels: Vec<Option<String>>,
    pub outline: Vec<OutlineItem>,
    /// Title, Author, ... in a fixed order; empty values omitted.
    pub metadata: Vec<(String, String)>,
    pub pdf_version: Option<String>,
    pub encrypted: bool,
    /// A stable id for remembering per-file state (bookmarks, last page).
    pub fingerprint: String,
    pub form_fields: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LinkTarget {
    Page { page: u32, y: Option<f32> },
    Uri(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    /// User space `[left, bottom, right, top]`.
    pub rect: [f32; 4],
    pub target: LinkTarget,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AnnotKind {
    Highlight,
    Underline,
    StrikeOut,
    Squiggly,
    Note,
    Ink,
    Square,
    Circle,
    FreeText,
    Stamp,
    Line,
    Other,
}

impl AnnotKind {
    pub fn label(self) -> &'static str {
        match self {
            AnnotKind::Highlight => "Highlight",
            AnnotKind::Underline => "Underline",
            AnnotKind::StrikeOut => "Strikeout",
            AnnotKind::Squiggly => "Squiggly",
            AnnotKind::Note => "Note",
            AnnotKind::Ink => "Ink",
            AnnotKind::Square => "Rectangle",
            AnnotKind::Circle => "Ellipse",
            AnnotKind::FreeText => "Text box",
            AnnotKind::Stamp => "Stamp",
            AnnotKind::Line => "Line",
            AnnotKind::Other => "Annotation",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AnnotInfo {
    /// Position in the page's annotation array at the time of listing.
    pub index: usize,
    /// The annotation's `/NM`, when it has one. Used to find it again.
    pub name: Option<String>,
    pub kind: AnnotKind,
    pub rect: [f32; 4],
    pub contents: String,
    pub author: String,
    /// `/C`, when PDFium reports it (it does not once an appearance exists).
    pub color: Option<[u8; 3]>,
}

/// Everything the UI needs about one page beyond pixels.
#[derive(Clone, Debug, Default)]
pub struct PageData {
    /// Page box in user space `[left, bottom, right, top]`.
    pub bbox: [f32; 4],
    /// The page's own `/Rotate`.
    pub rotation: Rotation,
    pub text: PageText,
    pub links: Vec<Link>,
    pub annots: Vec<AnnotInfo>,
}

/// An annotation to add. Geometry is in user space.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum NewAnnot {
    /// Highlight / underline / strikeout / squiggly over text line boxes.
    Markup {
        kind: AnnotKind,
        rects: Vec<[f32; 4]>,
        color: [u8; 3],
        text: String,
    },
    Note {
        x: f32,
        y: f32,
        color: [u8; 3],
        text: String,
    },
    Ink {
        strokes: Vec<Vec<(f32, f32)>>,
        color: [u8; 3],
        width: f32,
    },
    Shape {
        kind: AnnotKind,
        rect: [f32; 4],
        color: [u8; 3],
        width: f32,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenError {
    PasswordRequired,
    WrongPassword,
    NotFound,
    Unreadable(String),
    NotPdf,
    Unsupported(String),
    Engine(String),
}

impl fmt::Display for OpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpenError::PasswordRequired => write!(f, "This document is password protected."),
            OpenError::WrongPassword => write!(f, "The password is incorrect."),
            OpenError::NotFound => write!(f, "The file does not exist."),
            OpenError::Unreadable(e) => write!(f, "The file could not be read: {e}"),
            OpenError::NotPdf => write!(f, "The file is not a PDF or is badly damaged."),
            OpenError::Unsupported(e) => write!(f, "Unsupported document: {e}"),
            OpenError::Engine(e) => write!(f, "PDF engine error: {e}"),
        }
    }
}

pub type EResult<T> = Result<T, String>;

pub trait Engine {
    fn open(&mut self, id: DocId, path: &Path, password: Option<&str>) -> Result<DocInfo, OpenError>;
    fn close(&mut self, id: DocId);
    /// Renders a tile or thumbnail.
    fn render(&mut self, key: &TileKey) -> EResult<Image>;
    fn render_region(&mut self, req: &RegionReq) -> EResult<Image>;
    /// Renders a whole page (no view rotation) at an exact pixel size, for
    /// printing and export.
    fn render_page(&mut self, id: DocId, page: u32, w: u32, h: u32, printing: bool) -> EResult<Image>;
    /// Display size of a page in points, if the document is open.
    fn page_size(&self, id: DocId, page: u32) -> Option<(f32, f32)>;
    fn page_data(&mut self, id: DocId, page: u32) -> EResult<PageData>;
    fn page_text(&mut self, id: DocId, page: u32) -> EResult<PageText>;
    /// Adds an annotation and returns its `/NM`.
    fn add_annot(&mut self, id: DocId, page: u32, annot: &NewAnnot) -> EResult<String>;
    fn remove_annot(&mut self, id: DocId, page: u32, index: usize, name: Option<&str>) -> EResult<()>;
    fn set_annot_contents(&mut self, id: DocId, page: u32, index: usize, name: Option<&str>, text: &str) -> EResult<()>;
    /// Saves to `target` (which may be the open file) and keeps the document open.
    fn save(&mut self, id: DocId, target: &Path) -> EResult<()>;
}

/// Converts the display-space rect of a region request into a [`Rect`].
pub fn region_rect(r: &RegionReq) -> Rect {
    Rect::new(r.x as f32, r.y as f32, r.x as f32 + r.w as f32, r.y as f32 + r.h as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::quantize_scale;

    #[test]
    fn tile_rects_cover_the_page_exactly() {
        let size = (612.0, 792.0);
        let scale = quantize_scale(3.0); // 1836 x 2376 px
        let (nx, ny) = tile_grid(size, scale);
        assert_eq!((nx, ny), (2, 3));
        let mut area = 0u64;
        for tx in 0..nx {
            for ty in 0..ny {
                let k = TileKey {
                    doc: DocId(1),
                    page: 0,
                    rotation: Rotation::R0,
                    invert: false,
                    generation: 0,
                    kind: TileKind::Tile { scale, tx, ty },
                };
                let ((_, _, w, h), full) = k.pixel_rect(size);
                assert_eq!(full, (1836, 2376));
                assert!(w > 0 && h > 0 && w <= TILE && h <= TILE);
                area += w as u64 * h as u64;
            }
        }
        assert_eq!(area, 1836 * 2376);
    }

    #[test]
    fn thumbs_keep_aspect() {
        assert_eq!(thumb_px((612.0, 792.0)), (185, 240));
        assert_eq!(thumb_px((792.0, 612.0)), (240, 185));
        assert_eq!(thumb_px((0.0, 0.0)), (240, 240));
    }

    #[test]
    fn invert_keeps_alpha() {
        let mut i = Image { width: 1, height: 1, rgba: vec![10, 20, 30, 255] };
        i.invert();
        assert_eq!(i.rgba, vec![245, 235, 225, 255]);
    }
}
