//! A tiny PDF writer for tests and benchmarks.
//!
//! Produces valid, uncompressed PDF 1.7 with Helvetica text, filled
//! rectangles, internal and web links, page rotation, and an outline.
//! Nothing here is used to edit user documents.

use std::fmt::Write as _;

#[derive(Clone, Debug, Default)]
pub struct PageSpec {
    /// Media box width and height in points.
    pub size: (f32, f32),
    /// `/Rotate`, a multiple of 90.
    pub rotate: i32,
    /// `(x, y, font size, text)`; y is the baseline in user space.
    pub text: Vec<(f32, f32, f32, String)>,
    /// `([l, b, r, t], [r, g, b])` filled rectangles.
    pub rects: Vec<([f32; 4], [f32; 3])>,
    pub links: Vec<([f32; 4], LinkSpec)>,
}

#[derive(Clone, Debug)]
pub enum LinkSpec {
    Page(usize),
    Uri(String),
}

#[derive(Clone, Debug)]
pub struct OutlineSpec {
    pub title: String,
    pub page: usize,
    pub children: Vec<OutlineSpec>,
}

#[derive(Clone, Debug, Default)]
pub struct PdfSpec {
    pub pages: Vec<PageSpec>,
    pub outline: Vec<OutlineSpec>,
    pub title: Option<String>,
    pub author: Option<String>,
}

impl PageSpec {
    pub fn letter() -> Self {
        Self {
            size: (612.0, 792.0),
            ..Default::default()
        }
    }

    pub fn with_text(mut self, x: f32, y: f32, size: f32, s: &str) -> Self {
        self.text.push((x, y, size, s.to_string()));
        self
    }
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '(' | ')' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x80 => out.push(c),
            // WinAnsi covers Latin-1; anything else becomes '?'.
            c if (c as u32) < 0x100 => {
                let _ = write!(out, "\\{:03o}", c as u32);
            }
            _ => out.push('?'),
        }
    }
    out
}

/// UTF-16BE text string with BOM, for titles.
fn text_string(s: &str) -> String {
    let mut out = String::from("<FEFF");
    for u in s.encode_utf16() {
        let _ = write!(out, "{u:04X}");
    }
    out.push('>');
    out
}

impl PdfSpec {
    pub fn build(&self) -> Vec<u8> {
        // Object numbers: 1 catalog, 2 pages, 3 font, 4 info, then per page
        // (page, content), then outline items.
        let n = self.pages.len();
        let page_obj = |i: usize| 5 + 2 * i;
        let content_obj = |i: usize| 6 + 2 * i;
        let mut objects: Vec<(usize, String)> = Vec::new();

        let mut flat: Vec<(usize, &OutlineSpec, Option<usize>)> = Vec::new();
        fn walk<'a>(
            items: &'a [OutlineSpec],
            parent: Option<usize>,
            out: &mut Vec<(usize, &'a OutlineSpec, Option<usize>)>,
        ) {
            for it in items {
                let idx = out.len();
                out.push((idx, it, parent));
                walk(&it.children, Some(idx), out);
            }
        }
        walk(&self.outline, None, &mut flat);
        let outlines_root = 5 + 2 * n;
        let item_obj = |i: usize| outlines_root + 1 + i;

        let mut catalog = String::from("<< /Type /Catalog /Pages 2 0 R");
        if !flat.is_empty() {
            let _ = write!(
                catalog,
                " /Outlines {outlines_root} 0 R /PageMode /UseOutlines"
            );
        }
        catalog.push_str(" >>");
        objects.push((1, catalog));

        let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", page_obj(i))).collect();
        objects.push((
            2,
            format!("<< /Type /Pages /Kids [{}] /Count {n} >>", kids.join(" ")),
        ));
        objects.push((
            3,
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
                .into(),
        ));
        let mut info = String::from("<< /Producer (BDF test writer)");
        if let Some(t) = &self.title {
            let _ = write!(info, " /Title {}", text_string(t));
        }
        if let Some(a) = &self.author {
            let _ = write!(info, " /Author {}", text_string(a));
        }
        info.push_str(" /CreationDate (D:20240102030405Z) >>");
        objects.push((4, info));

        for (i, p) in self.pages.iter().enumerate() {
            let mut content = String::new();
            for (r, c) in &p.rects {
                let _ = writeln!(
                    content,
                    "{} {} {} rg {} {} {} {} re f",
                    c[0],
                    c[1],
                    c[2],
                    r[0],
                    r[1],
                    r[2] - r[0],
                    r[3] - r[1]
                );
            }
            if !p.text.is_empty() {
                content.push_str("0 0 0 rg\n");
            }
            for (x, y, size, s) in &p.text {
                let _ = writeln!(content, "BT /F1 {size} Tf {x} {y} Td ({}) Tj ET", escape(s));
            }
            let mut annots = Vec::new();
            for (r, l) in &p.links {
                let target = match l {
                    LinkSpec::Page(t) => format!(
                        "/Dest [{} 0 R /XYZ 0 {} 0]",
                        page_obj(*t),
                        self.pages.get(*t).map(|p| p.size.1).unwrap_or(792.0)
                    ),
                    LinkSpec::Uri(u) => format!("/A << /S /URI /URI ({}) >>", escape(u)),
                };
                annots.push(format!(
                    "<< /Type /Annot /Subtype /Link /Rect [{} {} {} {}] /Border [0 0 0] {target} >>",
                    r[0], r[1], r[2], r[3]
                ));
            }
            let mut page = format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R",
                p.size.0,
                p.size.1,
                content_obj(i)
            );
            if p.rotate != 0 {
                let _ = write!(page, " /Rotate {}", p.rotate);
            }
            if !annots.is_empty() {
                let _ = write!(page, " /Annots [{}]", annots.join(" "));
            }
            page.push_str(" >>");
            objects.push((page_obj(i), page));
            objects.push((
                content_obj(i),
                format!(
                    "<< /Length {} >>\nstream\n{content}endstream",
                    content.len()
                ),
            ));
        }

        if !flat.is_empty() {
            let top: Vec<usize> = flat.iter().filter(|f| f.2.is_none()).map(|f| f.0).collect();
            objects.push((
                outlines_root,
                format!(
                    "<< /Type /Outlines /First {} 0 R /Last {} 0 R /Count {} >>",
                    item_obj(top[0]),
                    item_obj(*top.last().expect("non-empty")),
                    flat.len()
                ),
            ));
            for &(idx, item, parent) in &flat {
                let siblings: Vec<usize> =
                    flat.iter().filter(|f| f.2 == parent).map(|f| f.0).collect();
                let pos = siblings.iter().position(|&s| s == idx).expect("self");
                let children: Vec<usize> = flat
                    .iter()
                    .filter(|f| f.2 == Some(idx))
                    .map(|f| f.0)
                    .collect();
                let mut o = format!(
                    "<< /Title {} /Parent {} 0 R /Dest [{} 0 R /Fit]",
                    text_string(&item.title),
                    parent.map(item_obj).unwrap_or(outlines_root),
                    page_obj(item.page.min(n.saturating_sub(1)))
                );
                if pos > 0 {
                    let _ = write!(o, " /Prev {} 0 R", item_obj(siblings[pos - 1]));
                }
                if pos + 1 < siblings.len() {
                    let _ = write!(o, " /Next {} 0 R", item_obj(siblings[pos + 1]));
                }
                if !children.is_empty() {
                    let _ = write!(
                        o,
                        " /First {} 0 R /Last {} 0 R /Count {}",
                        item_obj(children[0]),
                        item_obj(*children.last().expect("non-empty")),
                        children.len()
                    );
                }
                o.push_str(" >>");
                objects.push((item_obj(idx), o));
            }
        }

        objects.sort_by_key(|o| o.0);
        let mut out: Vec<u8> = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec();
        let count = objects.last().map(|o| o.0).unwrap_or(0) + 1;
        let mut offsets = vec![0usize; count];
        for (num, body) in &objects {
            offsets[*num] = out.len();
            out.extend_from_slice(format!("{num} 0 obj\n{body}\nendobj\n").as_bytes());
        }
        let xref = out.len();
        let mut x = format!("xref\n0 {count}\n0000000000 65535 f \n");
        for off in offsets.iter().skip(1) {
            let _ = writeln!(x, "{off:010} 00000 n ");
        }
        let _ = write!(
            x,
            "trailer\n<< /Size {count} /Root 1 0 R /Info 4 0 R /ID [<00112233445566778899AABBCCDDEEFF> <00112233445566778899AABBCCDDEEFF>] >>\nstartxref\n{xref}\n%%EOF\n"
        );
        out.extend_from_slice(x.as_bytes());
        out
    }
}

const WORDS: &[&str] = &[
    "forensic",
    "evidence",
    "timeline",
    "artifact",
    "registry",
    "memory",
    "network",
    "packet",
    "volume",
    "partition",
    "signature",
    "hash",
    "record",
    "journal",
    "process",
    "thread",
    "kernel",
    "driver",
    "service",
    "account",
    "session",
    "browser",
    "cache",
    "history",
];

/// A long synthetic document: `pages` letter pages of text, every tenth page
/// rotated 90 degrees, an outline entry per ten pages, and a link to the
/// next page on each page. Page `i` contains the marker `PAGE-<i+1>`.
pub fn long_document(pages: usize) -> Vec<u8> {
    let mut spec = PdfSpec {
        title: Some("BDF benchmark document".into()),
        author: Some("BDF".into()),
        ..Default::default()
    };
    let mut seed: u32 = 12345;
    for i in 0..pages {
        let mut p = PageSpec::letter();
        if i % 10 == 9 {
            p.rotate = 90;
        }
        p.text.push((
            72.0,
            740.0,
            18.0,
            format!("Chapter {} PAGE-{}", i / 10 + 1, i + 1),
        ));
        for line in 0..40 {
            let mut s = String::new();
            for _ in 0..11 {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                s.push_str(WORDS[(seed >> 16) as usize % WORDS.len()]);
                s.push(' ');
            }
            p.text.push((72.0, 710.0 - line as f32 * 16.0, 10.0, s));
        }
        p.rects.push(([72.0, 40.0, 540.0, 46.0], [0.2, 0.4, 0.9]));
        if i + 1 < pages {
            p.links
                .push(([480.0, 20.0, 540.0, 36.0], LinkSpec::Page(i + 1)));
            p.text.push((482.0, 24.0, 10.0, "Next".into()));
        }
        spec.pages.push(p);
        if i % 10 == 0 {
            spec.outline.push(OutlineSpec {
                title: format!("Chapter {}", i / 10 + 1),
                page: i,
                children: vec![],
            });
        }
    }
    spec.build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xref_offsets_point_at_objects() {
        let pdf = PdfSpec {
            pages: vec![PageSpec::letter().with_text(72.0, 700.0, 12.0, "Hello (world)")],
            outline: vec![OutlineSpec {
                title: "One".into(),
                page: 0,
                children: vec![],
            }],
            ..Default::default()
        }
        .build();
        let tail = String::from_utf8_lossy(&pdf[pdf.len() - 40..]).into_owned();
        let xref_at: usize = tail
            .rsplit("startxref\n")
            .next()
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(pdf[xref_at..].starts_with(b"xref"));
        let table = String::from_utf8_lossy(&pdf[xref_at..]).into_owned();
        let mut checked = 0;
        for (i, line) in table
            .lines()
            .skip(3)
            .take_while(|l| l.ends_with(" n "))
            .enumerate()
        {
            let off: usize = line[..10].parse().unwrap();
            assert!(
                pdf[off..].starts_with(format!("{} 0 obj", i + 1).as_bytes()),
                "object {}",
                i + 1
            );
            checked += 1;
        }
        assert!(checked >= 7);
        let s = String::from_utf8_lossy(&pdf);
        assert!(s.contains("(Hello \\(world\\)) Tj"));
    }

    #[test]
    fn long_document_has_markers() {
        let pdf = long_document(3);
        let s = String::from_utf8_lossy(&pdf);
        assert!(s.contains("PAGE-3"));
        assert_eq!(s.matches("/Type /Page ").count(), 3);
    }
}
