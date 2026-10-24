// Times each engine step for the first page of a PDF: what stands between
// opening a file and seeing it. Usage: cargo run --release --example timing -- file.pdf
use bdf_core::engine::{DocId, Engine, TILE, TileKey, TileKind, tile_grid};
use bdf_core::geom::{Rotation, quantize_scale};
use bdf_core::pdfium::PdfiumEngine;
use std::time::Instant;

fn main() {
    let path = std::env::args().nth(1).expect("file.pdf");
    let mut e = PdfiumEngine::load().expect("pdfium");
    let id = DocId(1);
    let t = Instant::now();
    let info = e.open(id, path.as_ref(), None).expect("open");
    println!("open            {:7.1} ms", t.elapsed().as_secs_f64() * 1e3);
    let size = info.page_sizes[0];
    let key = |kind| TileKey {
        doc: id,
        page: 0,
        rotation: Rotation::R0,
        invert: false,
        generation: 0,
        kind,
    };
    let t = Instant::now();
    e.render(&key(TileKind::Thumb)).unwrap();
    println!(
        "thumbnail       {:7.1} ms  (first use of the page: includes parsing)",
        t.elapsed().as_secs_f64() * 1e3
    );
    let q = quantize_scale(1.24 * 96.0 / 72.0);
    let (nx, ny) = tile_grid(size, q);
    let t = Instant::now();
    for ty in 0..ny {
        for tx in 0..nx {
            e.render(&key(TileKind::Tile { scale: q, tx, ty })).unwrap();
        }
    }
    println!(
        "sharp tiles     {:7.1} ms  ({} tiles of {TILE}px)",
        t.elapsed().as_secs_f64() * 1e3,
        nx * ny
    );
    let t = Instant::now();
    e.render(&key(TileKind::Thumb)).unwrap();
    // A second thumbnail of another generation: raster cost alone, page already parsed.
    let mut k2 = key(TileKind::Thumb);
    k2.generation = 1;
    let t2 = Instant::now();
    e.render(&k2).unwrap();
    println!(
        "thumb, parsed   {:7.1} ms",
        t2.elapsed().as_secs_f64() * 1e3
    );
    let _ = t;
    let t = Instant::now();
    e.page_data(id, 0).unwrap();
    println!(
        "page data       {:7.1} ms  (text, links, hidden-text check)",
        t.elapsed().as_secs_f64() * 1e3
    );
    let t = Instant::now();
    e.page_text(id, 0).unwrap();
    println!("text only       {:7.1} ms", t.elapsed().as_secs_f64() * 1e3);
}
