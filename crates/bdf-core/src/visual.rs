//! Selection by what the reader sees, not by the order text sits in the file.
//!
//! Complex PDFs store text in an order that has little to do with the page:
//! hidden blocks (covered by images or white boxes, switched-off layers,
//! white-on-white, off the page) sit between visible paragraphs, columns
//! interleave, headers come last. Selecting "from character i to character j"
//! in that order highlights random places. Here instead:
//!
//! 1. Only visible characters take part (the engine marks hidden ones by
//!    checking each glyph's box for ink on a rendered copy of the page).
//! 2. Visible characters are grouped into lines, and lines are split where a
//!    wide gap separates columns or table cells.
//! 3. A drag selects the lines between its two ends that sit in the same
//!    column, from the start point on the first line to the end point on the
//!    last one.

use crate::text::PageText;

/// One visual line: character indices left to right, and its bounding box.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub chars: Vec<usize>,
    /// `[left, bottom, right, top]` in user space.
    pub rect: [f32; 4],
}

impl Line {
    fn height(&self) -> f32 {
        (self.rect[3] - self.rect[1]).max(0.1)
    }

    fn width(&self) -> f32 {
        (self.rect[2] - self.rect[0]).max(0.1)
    }
}

fn center(r: &[f32; 4]) -> (f32, f32) {
    ((r[0] + r[2]) * 0.5, (r[1] + r[3]) * 0.5)
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '\'' || c == '\u{2019}' || c == '-'
}

impl PageText {
    /// Characters that take part in selection: visible ones with a box. If
    /// the visibility check found nothing visible at all (it failed, or the
    /// page is unusual), every positioned character takes part instead.
    pub fn selectable(&self) -> Vec<usize> {
        let boxed = |c: &&crate::text::TextChar| {
            c.rect.is_some_and(|r| r[2] > r[0] && r[3] > r[1]) && !c.ch.is_whitespace()
        };
        let visible: Vec<usize> = self
            .chars
            .iter()
            .enumerate()
            .filter(|(_, c)| boxed(c) && !c.hidden)
            .map(|(i, _)| i)
            .collect();
        if !visible.is_empty() {
            return visible;
        }
        self.chars
            .iter()
            .enumerate()
            .filter(|(_, c)| boxed(c))
            .map(|(i, _)| i)
            .collect()
    }

    fn rect_of(&self, i: usize) -> [f32; 4] {
        self.chars[i].rect.unwrap_or_default()
    }

    /// Visual lines in reading order: top to bottom, then left to right.
    pub fn lines(&self) -> Vec<Line> {
        let mut idx = self.selectable();
        if idx.is_empty() {
            return Vec::new();
        }
        // Rows: characters whose vertical centres agree.
        idx.sort_by(|&a, &b| {
            center(&self.rect_of(b))
                .1
                .total_cmp(&center(&self.rect_of(a)).1)
        });
        let mut rows: Vec<(f32, f32, Vec<usize>)> = Vec::new(); // (centre y, height, chars)
        for i in idx {
            let r = self.rect_of(i);
            let (_, cy) = center(&r);
            let h = r[3] - r[1];
            match rows.last_mut() {
                Some((rcy, rh, chars)) if (cy - *rcy).abs() < 0.45 * h.min(*rh) => {
                    let n = chars.len() as f32;
                    *rcy = (*rcy * n + cy) / (n + 1.0);
                    *rh = rh.max(h);
                    chars.push(i);
                }
                _ => rows.push((cy, h, vec![i])),
            }
        }
        // Lines: split rows where a wide gap separates columns or cells.
        let mut lines = Vec::new();
        for (_, _, mut chars) in rows {
            chars.sort_by(|&a, &b| self.rect_of(a)[0].total_cmp(&self.rect_of(b)[0]));
            let mut cur: Vec<usize> = Vec::new();
            let mut rect = [0.0f32; 4];
            for i in chars {
                let r = self.rect_of(i);
                let h = (r[3] - r[1]).max(rect[3] - rect[1]);
                if !cur.is_empty() && r[0] - rect[2] > 1.6 * h {
                    lines.push(Line {
                        chars: std::mem::take(&mut cur),
                        rect,
                    });
                }
                if cur.is_empty() {
                    rect = r;
                } else {
                    rect = [
                        rect[0].min(r[0]),
                        rect[1].min(r[1]),
                        rect[2].max(r[2]),
                        rect[3].max(r[3]),
                    ];
                }
                cur.push(i);
            }
            if !cur.is_empty() {
                lines.push(Line { chars: cur, rect });
            }
        }
        lines
    }

    /// The line nearest to a point; vertical distance counts most.
    fn nearest_line(lines: &[Line], p: (f32, f32)) -> Option<usize> {
        lines
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let dx = (l.rect[0] - p.0).max(0.0).max(p.0 - l.rect[2]);
                let dy = (l.rect[1] - p.1).max(0.0).max(p.1 - l.rect[3]);
                (i, dx * dx + 9.0 * dy * dy)
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }

    /// Characters between two points, in reading order. `words` extends both
    /// ends to whole words (used by the highlight tools).
    pub fn select_visual(&self, a: (f32, f32), b: (f32, f32), words: bool) -> Vec<usize> {
        let lines = self.lines();
        let (Some(la), Some(lb)) = (Self::nearest_line(&lines, a), Self::nearest_line(&lines, b))
        else {
            return Vec::new();
        };
        let ((s, sp), (e, ep)) = if (la, a.0) <= (lb, b.0) {
            ((la, a), (lb, b))
        } else {
            ((lb, b), (la, a))
        };
        let mid = |i: usize| center(&self.rect_of(i)).0;
        let mut out: Vec<usize> = Vec::new();
        if s == e {
            let (x0, x1) = (sp.0.min(ep.0), sp.0.max(ep.0));
            out.extend(
                lines[s]
                    .chars
                    .iter()
                    .copied()
                    .filter(|&i| mid(i) > x0 && mid(i) < x1),
            );
        } else {
            // The column the drag runs in: the horizontal span of its end lines.
            let band = (
                lines[s].rect[0].min(lines[e].rect[0]),
                lines[s].rect[2].max(lines[e].rect[2]),
            );
            out.extend(lines[s].chars.iter().copied().filter(|&i| mid(i) > sp.0));
            for l in &lines[s + 1..e] {
                let overlap = l.rect[2].min(band.1) - l.rect[0].max(band.0);
                if overlap >= 0.5 * l.width() {
                    out.extend(l.chars.iter().copied());
                }
            }
            out.extend(lines[e].chars.iter().copied().filter(|&i| mid(i) < ep.0));
        }
        if words && !out.is_empty() {
            out = self.snap_to_words(&lines, out);
        }
        out
    }

    /// Everything from a point to the end of the page (a drag continuing onto
    /// the next page).
    pub fn select_after(&self, a: (f32, f32)) -> Vec<usize> {
        let lines = self.lines();
        let Some(s) = Self::nearest_line(&lines, a) else {
            return Vec::new();
        };
        let mut out: Vec<usize> = lines[s]
            .chars
            .iter()
            .copied()
            .filter(|&i| center(&self.rect_of(i)).0 > a.0)
            .collect();
        for l in &lines[s + 1..] {
            out.extend(l.chars.iter().copied());
        }
        out
    }

    /// Everything from the top of the page to a point.
    pub fn select_before(&self, b: (f32, f32)) -> Vec<usize> {
        let lines = self.lines();
        let Some(e) = Self::nearest_line(&lines, b) else {
            return Vec::new();
        };
        let mut out: Vec<usize> = Vec::new();
        for l in &lines[..e] {
            out.extend(l.chars.iter().copied());
        }
        out.extend(
            lines[e]
                .chars
                .iter()
                .copied()
                .filter(|&i| center(&self.rect_of(i)).0 < b.0),
        );
        out
    }

    /// All visible text on the page, in reading order.
    pub fn select_all_visible(&self) -> Vec<usize> {
        self.lines().into_iter().flat_map(|l| l.chars).collect()
    }

    /// The visible word under a point.
    pub fn word_at_point(&self, p: (f32, f32)) -> Vec<usize> {
        let lines = self.lines();
        let Some(li) = Self::nearest_line(&lines, p) else {
            return Vec::new();
        };
        let l = &lines[li];
        let Some(pos) = l.chars.iter().position(|&i| {
            let r = self.rect_of(i);
            p.0 >= r[0] && p.0 <= r[2]
        }) else {
            return Vec::new();
        };
        if !is_word(self.chars[l.chars[pos]].ch) {
            return Vec::new();
        }
        self.snap_to_words(&lines, vec![l.chars[pos]])
    }

    /// The visible line under a point.
    pub fn line_at_point(&self, p: (f32, f32)) -> Vec<usize> {
        let lines = self.lines();
        Self::nearest_line(&lines, p)
            .map(|i| lines[i].chars.clone())
            .unwrap_or_default()
    }

    /// Extends the first and last selected characters to whole words.
    fn snap_to_words(&self, lines: &[Line], sel: Vec<usize>) -> Vec<usize> {
        let line_of = |c: usize| lines.iter().position(|l| l.chars.contains(&c));
        let first = sel[0];
        let last = *sel.last().expect("non-empty");
        let mut before = Vec::new();
        if let Some(li) = line_of(first) {
            let l = &lines[li].chars;
            let mut k = l.iter().position(|&c| c == first).expect("in line");
            while k > 0 && self.joined(l[k - 1], l[k]) {
                k -= 1;
                before.push(l[k]);
            }
            before.reverse();
        }
        let mut after = Vec::new();
        if let Some(li) = line_of(last) {
            let l = &lines[li].chars;
            let mut k = l.iter().position(|&c| c == last).expect("in line");
            while k + 1 < l.len() && self.joined(l[k], l[k + 1]) {
                k += 1;
                after.push(l[k]);
            }
        }
        before.into_iter().chain(sel).chain(after).collect()
    }

    /// Two neighbouring characters belong to the same word.
    fn joined(&self, a: usize, b: usize) -> bool {
        let (ra, rb) = (self.rect_of(a), self.rect_of(b));
        let h = (ra[3] - ra[1]).max(rb[3] - rb[1]);
        is_word(self.chars[a].ch) && is_word(self.chars[b].ch) && rb[0] - ra[2] < 0.15 * h
    }

    /// One rectangle per run of selected characters on each line.
    pub fn rects_for(&self, sel: &[usize]) -> Vec<[f32; 4]> {
        let set: std::collections::HashSet<usize> = sel.iter().copied().collect();
        let mut out = Vec::new();
        for l in self.lines() {
            let mut run: Option<[f32; 4]> = None;
            for &i in &l.chars {
                if set.contains(&i) {
                    let r = self.rect_of(i);
                    // Use the line's full height so highlights look even.
                    let r = [r[0], l.rect[1], r[2], l.rect[3]];
                    run = Some(match run {
                        Some(a) => [a[0].min(r[0]), a[1], a[2].max(r[2]), a[3]],
                        None => r,
                    });
                } else if let Some(r) = run.take() {
                    out.push(r);
                }
            }
            if let Some(r) = run {
                out.push(r);
            }
        }
        out
    }

    /// The selected text in reading order, with spaces where words are apart
    /// and a line break between lines.
    pub fn text_for(&self, sel: &[usize]) -> String {
        let set: std::collections::HashSet<usize> = sel.iter().copied().collect();
        let mut out = String::new();
        for l in self.lines() {
            let mut line = String::new();
            let mut prev: Option<usize> = None;
            for &i in l.chars.iter().filter(|i| set.contains(i)) {
                if let Some(p) = prev {
                    let gap = self.rect_of(i)[0] - self.rect_of(p)[2];
                    if gap > 0.2 * l.height() {
                        line.push(' ');
                    }
                }
                line.push(self.chars[i].ch);
                prev = Some(i);
            }
            if !line.is_empty() {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&line);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use crate::text::{PageText, TextChar};

    /// Adds `s` as glyphs 6 wide, 10 high, baseline at `y`, starting at `x`.
    fn put(p: &mut PageText, x: f32, y: f32, s: &str, hidden: bool) {
        for (k, ch) in s.chars().enumerate() {
            let x0 = x + k as f32 * 6.0;
            let rect = (ch != ' ').then_some([x0, y, x0 + 6.0, y + 10.0]);
            p.chars.push(TextChar { ch, rect, hidden });
        }
        p.chars.push(TextChar {
            ch: '\r',
            rect: None,
            hidden: false,
        });
        p.chars.push(TextChar {
            ch: '\n',
            rect: None,
            hidden: false,
        });
    }

    fn text(p: &PageText, sel: &[usize]) -> String {
        p.text_for(sel)
    }

    /// Two columns, stored in the file in a scrambled order with a hidden
    /// block in the middle — the case that used to highlight random places.
    fn messy_page() -> PageText {
        let mut p = PageText::default();
        put(&mut p, 50.0, 700.0, "left one", false);
        put(&mut p, 300.0, 700.0, "right one", false);
        put(&mut p, 50.0, 400.0, "HIDDEN BLOCK OF INVISIBLE WORDS", true);
        put(&mut p, 50.0, 685.0, "left two", false);
        put(&mut p, 300.0, 685.0, "right two", false);
        put(&mut p, 50.0, 670.0, "left three", false);
        put(&mut p, 300.0, 670.0, "right three", false);
        p
    }

    #[test]
    fn lines_follow_the_page_not_the_file() {
        let p = messy_page();
        let lines: Vec<String> = p.lines().iter().map(|l| text(&p, &l.chars)).collect();
        assert_eq!(
            lines,
            [
                "left one",
                "right one",
                "left two",
                "right two",
                "left three",
                "right three"
            ]
        );
    }

    #[test]
    fn drag_stays_in_its_column_and_skips_hidden_text() {
        let p = messy_page();
        // From "one" on the first left line to "two" on the second.
        let sel = p.select_visual((76.0, 705.0), (98.0, 690.0), false);
        assert_eq!(text(&p, &sel), "one\nleft two");
        // Down the whole left column.
        let sel = p.select_visual((49.0, 705.0), (120.0, 675.0), false);
        assert_eq!(text(&p, &sel), "left one\nleft two\nleft three");
        assert!(sel.iter().all(|&i| !p.chars[i].hidden));
    }

    #[test]
    fn words_snap_and_points_pick_words_and_lines() {
        let p = messy_page();
        // Starting mid-word and ending mid-word still covers whole words.
        let sel = p.select_visual((60.0, 705.0), (75.0, 690.0), true);
        assert_eq!(text(&p, &sel), "left one\nleft");
        assert_eq!(text(&p, &p.word_at_point((310.0, 703.0))), "right");
        assert_eq!(text(&p, &p.line_at_point((320.0, 672.0))), "right three");
        assert!(
            p.word_at_point((50.0, 403.0))
                .iter()
                .all(|&i| !p.chars[i].hidden)
        );
    }

    #[test]
    fn across_pages_and_whole_page() {
        let p = messy_page();
        assert_eq!(text(&p, &p.select_after((300.0, 672.0))), "right three");
        assert_eq!(text(&p, &p.select_before((80.0, 703.0))), "left");
        assert_eq!(text(&p, &p.select_all_visible()).lines().count(), 6);
    }

    #[test]
    fn rects_cover_each_run_once() {
        let p = messy_page();
        let sel = p.select_visual((49.0, 705.0), (120.0, 675.0), false);
        let r = p.rects_for(&sel);
        assert_eq!(r.len(), 3);
        assert_eq!(r[0], [50.0, 700.0, 98.0, 710.0]);
    }

    #[test]
    fn scanned_pages_with_only_invisible_text_still_select() {
        let mut p = PageText::default();
        put(&mut p, 50.0, 700.0, "ocr layer text", true);
        let sel = p.select_visual((49.0, 705.0), (200.0, 705.0), false);
        assert_eq!(text(&p, &sel), "ocr layer text");
    }

    #[test]
    fn empty_and_tiny_drags() {
        let p = PageText::default();
        assert!(p.select_visual((0.0, 0.0), (10.0, 10.0), true).is_empty());
        assert!(p.lines().is_empty());
        let p = messy_page();
        assert!(
            p.select_visual((52.0, 705.0), (53.0, 705.0), false)
                .is_empty()
        );
    }
}
