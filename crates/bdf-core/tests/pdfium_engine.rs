//! Engine tests against the real PDFium library.
//!
//! PDFium is found through `BDF_PDFIUM` (file or folder) or next to the test
//! binary. These tests fail, rather than skip, when PDFium is missing, so CI
//! can never pass without exercising the engine; set `BDF_SKIP_PDFIUM=1` to
//! skip them deliberately.

use bdf_core::engine::{
    AnnotKind, DocId, Engine, LinkTarget, NewAnnot, OpenError, RegionReq, TileKey, TileKind,
    thumb_px,
};
use bdf_core::geom::{PageSpace, Rotation, quantize_scale};
use bdf_core::pdfium::PdfiumEngine;
use bdf_core::testpdf::{LinkSpec, OutlineSpec, PageSpec, PdfSpec, long_document};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::Instant;

/// PDFium is not thread-safe: tests take this lock for their whole body.
static SERIAL: Mutex<()> = Mutex::new(());

fn engine() -> Option<PdfiumEngine> {
    if std::env::var_os("BDF_SKIP_PDFIUM").is_some() {
        return None;
    }
    Some(PdfiumEngine::load().expect("PDFium must be available (set BDF_PDFIUM)"))
}

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("bdf-engine-tests-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

fn write(name: &str, bytes: &[u8]) -> PathBuf {
    let p = tmp(name);
    std::fs::write(&p, bytes).unwrap();
    p
}

fn sample() -> Vec<u8> {
    let mut p0 = PageSpec::letter()
        .with_text(72.0, 700.0, 24.0, "Hello BDF reader")
        .with_text(
            72.0,
            660.0,
            12.0,
            "The quick brown fox jumps over the lazy dog.",
        );
    p0.links
        .push(([72.0, 600.0, 200.0, 620.0], LinkSpec::Page(2)));
    p0.links.push((
        [72.0, 560.0, 200.0, 580.0],
        LinkSpec::Uri("https://example.com/x".into()),
    ));
    let mut p1 = PageSpec {
        size: (842.0, 595.0),
        ..Default::default()
    }
    .with_text(72.0, 500.0, 14.0, "Landscape page");
    p1.rects.push(([0.0, 0.0, 842.0, 100.0], [0.0, 0.0, 0.0]));
    let mut p2 = PageSpec::letter().with_text(72.0, 700.0, 14.0, "Rotated page text");
    p2.rotate = 90;
    PdfSpec {
        pages: vec![p0, p1, p2],
        outline: vec![
            OutlineSpec {
                title: "Intro".into(),
                page: 0,
                children: vec![OutlineSpec {
                    title: "Détails ✓".into(),
                    page: 1,
                    children: vec![],
                }],
            },
            OutlineSpec {
                title: "End".into(),
                page: 2,
                children: vec![],
            },
        ],
        title: Some("Sample title".into()),
        author: Some("Tester".into()),
    }
    .build()
}

fn thumb(doc: DocId, page: u32) -> TileKey {
    TileKey {
        doc,
        page,
        rotation: Rotation::R0,
        invert: false,
        generation: 0,
        kind: TileKind::Thumb,
    }
}

fn dark_fraction(rgba: &[u8]) -> f32 {
    let dark = rgba
        .chunks_exact(4)
        .filter(|p| (p[0] as u32 + p[1] as u32 + p[2] as u32) < 300)
        .count();
    dark as f32 / (rgba.len() / 4) as f32
}

#[test]
fn opens_and_describes_a_document() {
    let _serial = serial();
    let Some(mut e) = engine() else { return };
    let path = write("sample.pdf", &sample());
    let info = e.open(DocId(1), &path, None).expect("opens");
    assert_eq!(info.page_sizes.len(), 3);
    assert_eq!(info.page_sizes[0], (612.0, 792.0));
    assert_eq!(info.page_sizes[1], (842.0, 595.0));
    // /Rotate 90 is applied to the reported size.
    assert_eq!(info.page_sizes[2], (792.0, 612.0));
    assert_eq!(info.outline.len(), 2);
    assert_eq!(info.outline[0].title, "Intro");
    assert_eq!(info.outline[0].page, Some(0));
    assert_eq!(info.outline[0].children[0].title, "Détails ✓");
    assert_eq!(info.outline[0].children[0].page, Some(1));
    assert_eq!(info.outline[1].page, Some(2));
    assert!(
        info.metadata
            .contains(&("Title".into(), "Sample title".into()))
    );
    assert!(
        info.metadata
            .contains(&("CreationDate".into(), "2024-01-02 03:04:05 UTC".into()))
    );
    assert_eq!(info.pdf_version.as_deref(), Some("1.7"));
    assert!(
        info.fingerprint.starts_with("id:00112233"),
        "{}",
        info.fingerprint
    );
    assert!(!info.encrypted);
    assert!(info.page_labels.is_empty());
    e.close(DocId(1));
}

#[test]
fn renders_tiles_thumbnails_regions_and_dark_mode() {
    let _serial = serial();
    let Some(mut e) = engine() else { return };
    let path = write("render.pdf", &sample());
    let doc = DocId(1);
    e.open(doc, &path, None).unwrap();

    let t = e.render(&thumb(doc, 0)).unwrap();
    assert_eq!((t.width, t.height), thumb_px((612.0, 792.0)));
    assert!(
        dark_fraction(&t.rgba) > 0.0005,
        "thumbnail should show text"
    );
    assert!(t.rgba.chunks_exact(4).all(|p| p[3] == 255));

    // Page 1 has a black band along its bottom 100pt.
    let scale = quantize_scale(1.0);
    let key = TileKey {
        kind: TileKind::Tile {
            scale,
            tx: 0,
            ty: 0,
        },
        page: 1,
        ..thumb(doc, 0)
    };
    let img = e.render(&key).unwrap();
    assert_eq!((img.width, img.height), (842, 595));
    let px = |img: &bdf_core::engine::Image, x: u32, y: u32| {
        let i = ((y * img.width + x) * 4) as usize;
        [img.rgba[i], img.rgba[i + 1], img.rgba[i + 2]]
    };
    assert_eq!(px(&img, 400, 590), [0, 0, 0]);
    assert_eq!(px(&img, 400, 10), [255, 255, 255]);

    // Dark pages invert colours.
    let inv = e
        .render(&TileKey {
            invert: true,
            ..key
        })
        .unwrap();
    assert_eq!(px(&inv, 400, 590), [255, 255, 255]);
    assert_eq!(px(&inv, 400, 10), [0, 0, 0]);

    // A second tile row at 2x: tiles stitch into the full page.
    let scale2 = quantize_scale(2.0);
    let full = e
        .render(&TileKey {
            kind: TileKind::Tile {
                scale: scale2,
                tx: 1,
                ty: 1,
            },
            ..key
        })
        .unwrap();
    assert_eq!((full.width, full.height), (1684 - 1024, 1190 - 1024));
    // Bottom band is black at 2x too.
    assert_eq!(px(&full, 10, full.height - 5), [0, 0, 0]);

    // A region (magnifier) matches the same pixels as the tile path.
    let r = e
        .render_region(&RegionReq {
            doc,
            page: 1,
            rotation: Rotation::R0,
            invert: false,
            scale,
            x: 390,
            y: 580,
            w: 20,
            h: 15,
        })
        .unwrap();
    assert_eq!((r.width, r.height), (20, 15));
    assert_eq!(px(&r, 10, 12), [0, 0, 0]);

    // View rotation swaps the pixel size.
    let rot = e
        .render(&TileKey {
            rotation: Rotation::R90,
            ..key
        })
        .unwrap();
    assert_eq!((rot.width, rot.height), (595, 842));

    // Printing path: exact size.
    let p = e.render_page(doc, 0, 300, 400, true).unwrap();
    assert_eq!((p.width, p.height), (300, 400));

    // Bad requests are errors, not crashes.
    assert!(e.render(&thumb(doc, 99)).is_err());
    assert!(e.render(&thumb(DocId(42), 0)).is_err());
    assert!(
        e.render_region(&RegionReq {
            doc,
            page: 0,
            rotation: Rotation::R0,
            invert: false,
            scale,
            x: 0,
            y: 0,
            w: 0,
            h: 10
        })
        .is_err()
    );
}

#[test]
fn text_links_and_geometry() {
    let _serial = serial();
    let Some(mut e) = engine() else { return };
    let path = write("text.pdf", &sample());
    let doc = DocId(1);
    e.open(doc, &path, None).unwrap();
    let d = e.page_data(doc, 0).unwrap();
    let all = d.text.all();
    assert!(all.contains("Hello BDF reader"), "{all:?}");
    assert!(all.contains("quick brown fox"));
    let m = d.text.search("brown fox", Default::default());
    assert_eq!(m.len(), 1);
    let rects = d.text.rects(m[0].start, m[0].end);
    assert_eq!(rects.len(), 1);
    // Baseline at 660, 12pt: the box straddles it.
    assert!(rects[0][1] < 660.0 && rects[0][3] > 665.0, "{:?}", rects[0]);
    assert!(rects[0][0] > 72.0 && rects[0][2] < 400.0);

    assert_eq!(d.links.len(), 2);
    assert!(
        d.links
            .iter()
            .any(|l| matches!(l.target, LinkTarget::Page { page: 2, .. }))
    );
    assert!(
        d.links
            .iter()
            .any(|l| l.target == LinkTarget::Uri("https://example.com/x".into()))
    );

    // Rotated page: our PageSpace agrees with PDFium's reported size.
    let info_size = (792.0, 612.0);
    let d2 = e.page_data(doc, 2).unwrap();
    assert_eq!(d2.rotation, Rotation::R90);
    let space = PageSpace::new(d2.bbox, d2.rotation);
    assert_eq!(space.size(), info_size);
    // The text near the top-left of the unrotated page appears near the
    // top-right after the clockwise turn.
    let m = d2.text.search("Rotated", Default::default());
    let r = space.rect_to_display(d2.text.rects(m[0].start, m[0].end)[0]);
    assert!(r.x0 > 600.0 && r.y0 < 200.0, "{r:?}");
    assert!(r.height() > r.width(), "rotated text runs downwards: {r:?}");
}

#[test]
fn annotations_round_trip_through_an_in_place_save() {
    let _serial = serial();
    let Some(mut e) = engine() else { return };
    let path = write("annots.pdf", &sample());
    let original = std::fs::read(&path).unwrap();
    let doc = DocId(1);
    e.open(doc, &path, None).unwrap();

    let d = e.page_data(doc, 0).unwrap();
    let m = d.text.search("quick brown", Default::default())[0];
    let rects = d.text.rects(m.start, m.end);
    let before = e.render(&thumb(doc, 0)).unwrap();
    let nm = e
        .add_annot(
            doc,
            0,
            &NewAnnot::Markup {
                kind: AnnotKind::Highlight,
                rects: rects.clone(),
                color: [255, 0, 0],
                text: "important".into(),
            },
        )
        .unwrap();
    let note = e
        .add_annot(
            doc,
            0,
            &NewAnnot::Note {
                x: 400.0,
                y: 750.0,
                color: [255, 220, 0],
                text: "a note".into(),
            },
        )
        .unwrap();
    let ink = e
        .add_annot(
            doc,
            1,
            &NewAnnot::Ink {
                strokes: vec![vec![(100.0, 300.0), (200.0, 350.0), (300.0, 300.0)]],
                color: [0, 0, 255],
                width: 3.0,
            },
        )
        .unwrap();
    e.add_annot(
        doc,
        1,
        &NewAnnot::Shape {
            kind: AnnotKind::Square,
            rect: [400.0, 200.0, 500.0, 300.0],
            color: [0, 128, 0],
            width: 2.0,
        },
    )
    .unwrap();
    assert!(
        e.add_annot(
            doc,
            0,
            &NewAnnot::Ink {
                strokes: vec![],
                color: [0, 0, 0],
                width: 1.0
            }
        )
        .is_err()
    );

    // The highlight changes the rendered page.
    let after = e.render(&thumb(doc, 0)).unwrap();
    let reddish = |img: &bdf_core::engine::Image| {
        img.rgba
            .chunks_exact(4)
            .filter(|p| p[0] > 200 && p[1] < 120 && p[2] < 120)
            .count()
    };
    assert!(
        reddish(&after) > reddish(&before) + 20,
        "highlight should render"
    );

    let d = e.page_data(doc, 0).unwrap();
    let hl = d
        .annots
        .iter()
        .find(|a| a.name.as_deref() == Some(&nm))
        .expect("highlight listed");
    assert_eq!(hl.kind, AnnotKind::Highlight);
    assert_eq!(hl.contents, "important");
    // PDFium only reports /C while an annotation has no appearance stream.
    assert!(hl.color.is_none() || hl.color == Some([255, 0, 0]));
    assert!(
        d.annots
            .iter()
            .any(|a| a.name.as_deref() == Some(&note) && a.kind == AnnotKind::Note)
    );
    let d1 = e.page_data(doc, 1).unwrap();
    assert!(
        d1.annots
            .iter()
            .any(|a| a.name.as_deref() == Some(&ink) && a.kind == AnnotKind::Ink)
    );
    assert!(d1.annots.iter().any(|a| a.kind == AnnotKind::Square));

    e.set_annot_contents(doc, 0, 0, Some(&note), "edited note")
        .unwrap();

    // Save over the open file.
    e.save(doc, &path).unwrap();
    let saved = std::fs::read(&path).unwrap();
    assert!(saved.len() > original.len());
    assert_eq!(
        &saved[..original.len()],
        &original[..],
        "incremental save keeps the original bytes"
    );
    assert!(
        !tmp("annots.bdf-saving.pdf").exists(),
        "temp file cleaned up"
    );

    // Still usable after the save, and the data is there.
    let d = e.page_data(doc, 0).unwrap();
    assert!(d.annots.iter().any(|a| a.contents == "edited note"));

    // A fresh engine sees the annotations too.
    let mut e2 = engine().unwrap();
    e2.open(DocId(9), &path, None).unwrap();
    let d = e2.page_data(DocId(9), 0).unwrap();
    assert_eq!(
        d.annots
            .iter()
            .filter(|a| a.kind == AnnotKind::Highlight)
            .count(),
        1
    );
    drop(e2);

    // Remove by name, then save as a new file.
    e.remove_annot(doc, 0, 0, Some(&nm)).unwrap();
    assert!(e.remove_annot(doc, 0, 0, Some("no-such-name")).is_err());
    let copy = tmp("annots-copy.pdf");
    e.save(doc, &copy).unwrap();
    let mut e3 = engine().unwrap();
    e3.open(DocId(3), &copy, None).unwrap();
    let d = e3.page_data(DocId(3), 0).unwrap();
    assert!(d.annots.iter().all(|a| a.kind != AnnotKind::Highlight));
    assert!(d.annots.iter().any(|a| a.kind == AnnotKind::Note));
}

#[test]
fn passwords() {
    let _serial = serial();
    let Some(mut e) = engine() else { return };
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/encrypted-rc4.pdf");
    assert_eq!(
        e.open(DocId(1), &fixture, None).unwrap_err(),
        OpenError::PasswordRequired
    );
    assert_eq!(
        e.open(DocId(1), &fixture, Some("wrong")).unwrap_err(),
        OpenError::WrongPassword
    );
    let info = e.open(DocId(1), &fixture, Some("bdf-secret")).unwrap();
    assert!(info.encrypted);
    assert!(
        e.page_data(DocId(1), 0)
            .unwrap()
            .text
            .all()
            .contains("Secret PAGE-1")
    );
    // Saving an encrypted document keeps it encrypted and openable with the password.
    let copy = tmp("encrypted-copy.pdf");
    e.add_annot(
        DocId(1),
        0,
        &NewAnnot::Note {
            x: 10.0,
            y: 780.0,
            color: [255, 255, 0],
            text: "x".into(),
        },
    )
    .unwrap();
    e.save(DocId(1), &copy).unwrap();
    let mut e2 = engine().unwrap();
    assert_eq!(
        e2.open(DocId(2), &copy, None).unwrap_err(),
        OpenError::PasswordRequired
    );
    e2.open(DocId(2), &copy, Some("bdf-secret")).unwrap();
}

#[test]
fn bad_input_never_panics() {
    let _serial = serial();
    let Some(mut e) = engine() else { return };
    assert_eq!(
        e.open(DocId(1), &tmp("does-not-exist.pdf"), None)
            .unwrap_err(),
        OpenError::NotFound
    );
    assert_eq!(
        e.open(DocId(1), &write("empty.pdf", b""), None)
            .unwrap_err(),
        OpenError::NotPdf
    );
    assert_eq!(
        e.open(DocId(1), &write("text.pdf", b"hello, not a pdf"), None)
            .unwrap_err(),
        OpenError::NotPdf
    );
    let _ = e.open(DocId(1), tmp("").as_path(), None).unwrap_err();

    // Truncated and corrupted copies: either open (PDFium repairs) or fail
    // cleanly; when they open, every page renders or errors cleanly.
    let good = long_document(5);
    let mut seed = 0x9E3779B9u32;
    let mut rand = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    let mut opened = 0;
    for i in 0..150 {
        let mut bytes = good.clone();
        match i % 3 {
            0 => bytes.truncate((rand() as usize) % bytes.len()),
            1 => {
                for _ in 0..1 + rand() % 20 {
                    let at = rand() as usize % bytes.len();
                    bytes[at] = rand() as u8;
                }
            }
            _ => {
                let at = rand() as usize % bytes.len();
                let len = (rand() as usize % 4000).min(bytes.len() - at);
                bytes.drain(at..at + len);
            }
        }
        let p = write(&format!("fuzz-{i}.pdf"), &bytes);
        if let Ok(info) = e.open(DocId(100 + i), &p, None) {
            opened += 1;
            for page in 0..info.page_sizes.len() as u32 {
                let _ = e.render(&thumb(DocId(100 + i), page));
                let _ = e.page_data(DocId(100 + i), page);
            }
            e.close(DocId(100 + i));
        }
        let _ = std::fs::remove_file(&p);
    }
    assert!(
        opened > 10,
        "PDFium should repair many damaged files ({opened} opened)"
    );
}

#[test]
fn large_documents_open_fast_and_memory_stays_bounded() {
    let _serial = serial();
    let Some(mut e) = engine() else { return };
    let path = write("long.pdf", &long_document(2000));
    let t0 = Instant::now();
    let info = e.open(DocId(1), &path, None).unwrap();
    let open_ms = t0.elapsed().as_millis();
    assert_eq!(info.page_sizes.len(), 2000);
    assert_eq!(info.page_sizes[9], (792.0, 612.0));
    assert_eq!(info.outline.len(), 200);

    let t0 = Instant::now();
    let scale = quantize_scale(1.5);
    for page in [0u32, 999, 1999] {
        let k = TileKey {
            kind: TileKind::Tile {
                scale,
                tx: 0,
                ty: 0,
            },
            ..thumb(DocId(1), page)
        };
        let img = e.render(&k).unwrap();
        assert!(dark_fraction(&img.rgba) > 0.01);
    }
    let render_ms = t0.elapsed().as_millis() / 3;

    // Walk through every page: the loaded-page cache must stay small.
    for page in 0..2000 {
        e.render(&thumb(DocId(1), page)).unwrap();
    }
    assert!(e.loaded_pages(DocId(1)) <= 8);

    let d = e.page_data(DocId(1), 1234).unwrap();
    assert!(d.text.all().contains("PAGE-1235"));
    println!("open 2000 pages: {open_ms} ms; first tile: {render_ms} ms per page");
    // Generous limits for slow CI machines; typical numbers are far lower.
    assert!(open_ms < 3000, "open took {open_ms} ms");
    assert!(render_ms < 1000, "render took {render_ms} ms");
}
