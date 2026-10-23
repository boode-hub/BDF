//! BDF core: everything about documents that is not drawing widgets.
//!
//! * [`pdfium`]: the PDFium engine (the only unsafe code).
//! * [`worker`]: the engine thread and its scheduling.
//! * [`layout`], [`geom`]: where pages go and coordinate conversions.
//! * [`text`]: search and selection over extracted page text.
//! * [`cache`]: the byte-bounded LRU used for page images.
//! * [`settings`]: portable settings and per-document memory.
//! * [`testpdf`]: a small PDF writer for tests and benchmarks.

pub mod cache;
pub mod engine;
pub mod geom;
pub mod layout;
pub mod pdfium;
pub mod settings;
pub mod testpdf;
pub mod text;
pub mod worker;
