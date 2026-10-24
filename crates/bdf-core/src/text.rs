//! Page text: search, selection and the rectangles that cover a run of text.
//!
//! The engine extracts every character with its box (user space). Everything
//! here is plain Rust over that data, so it is fully testable without PDFium.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextChar {
    pub ch: char,
    /// Loose box in user space `[left, bottom, right, top]`; `None` for
    /// characters PDFium synthesises (inferred spaces and line breaks).
    pub rect: Option<[f32; 4]>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PageText {
    pub chars: Vec<TextChar>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SearchOptions {
    pub match_case: bool,
    pub whole_word: bool,
}

/// A match: half-open char range `[start, end)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Match {
    pub start: usize,
    pub end: usize,
}

fn fold(c: char, match_case: bool) -> char {
    if match_case {
        c
    } else {
        // Single-char lowercase covers every script we care about for search;
        // multi-char expansions (e.g. 'İ') keep their first char.
        c.to_lowercase().next().unwrap_or(c)
    }
}

fn normalize_char(c: char) -> char {
    match c {
        '\u{00A0}' | '\u{2007}' | '\u{202F}' | '\t' | '\r' | '\n' => ' ',
        '\u{2018}' | '\u{2019}' | '\u{201B}' | '\u{2032}' => '\'',
        '\u{201C}' | '\u{201D}' | '\u{201F}' | '\u{2033}' => '"',
        '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2212}' => '-',
        c => c,
    }
}

/// Expands common Latin ligatures so "office" matches "o\u{FB03}ce".
fn expand(c: char) -> &'static str {
    match c {
        '\u{FB00}' => "ff",
        '\u{FB01}' => "fi",
        '\u{FB02}' => "fl",
        '\u{FB03}' => "ffi",
        '\u{FB04}' => "ffl",
        '\u{FB06}' => "st",
        _ => "",
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl PageText {
    pub fn len(&self) -> usize {
        self.chars.len()
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    /// The text of `[start, end)`, with PDFium's line breaks preserved.
    pub fn slice(&self, start: usize, end: usize) -> String {
        let end = end.min(self.chars.len());
        let start = start.min(end);
        let mut s = String::with_capacity(end - start);
        let mut prev_cr = false;
        for c in &self.chars[start..end] {
            match c.ch {
                '\r' => {
                    s.push('\n');
                    prev_cr = true;
                    continue;
                }
                '\n' if prev_cr => {}
                '\u{FFFE}' | '\u{FFFF}' | '\0' => {}
                // Soft hyphen at a line end: PDFium reports it as U+0002.
                '\u{2}' => s.push('-'),
                ch => s.push(ch),
            }
            prev_cr = false;
        }
        s
    }

    pub fn all(&self) -> String {
        self.slice(0, self.chars.len())
    }

    /// Finds every occurrence of `query`. Runs of whitespace in the query
    /// match any run of whitespace (including line breaks) in the page.
    pub fn search(&self, query: &str, opts: SearchOptions) -> Vec<Match> {
        // Normalised query: folded chars, whitespace collapsed to one space.
        let mut q: Vec<char> = Vec::new();
        for c in query.trim().chars() {
            let c = normalize_char(c);
            let e = expand(c);
            let seq: Vec<char> = if e.is_empty() {
                vec![c]
            } else {
                e.chars().collect()
            };
            for c in seq {
                let c = fold(c, opts.match_case);
                if c.is_whitespace() {
                    if q.last() != Some(&' ') {
                        q.push(' ');
                    }
                } else {
                    q.push(c);
                }
            }
        }
        if q.is_empty() {
            return Vec::new();
        }

        // Normalised haystack with a map back to source char indices.
        let mut hay: Vec<char> = Vec::with_capacity(self.chars.len());
        let mut src: Vec<usize> = Vec::with_capacity(self.chars.len());
        let mut joining = false;
        for (i, tc) in self.chars.iter().enumerate() {
            let c = normalize_char(tc.ch);
            if c == '\u{2}' {
                // A hyphenation break: drop it and the line break after it,
                // so "hyphen-\nated" matches "hyphenated".
                joining = true;
                continue;
            }
            if joining && (c.is_whitespace() || c == '\0') {
                continue;
            }
            joining = false;
            let e = expand(c);
            if !e.is_empty() {
                for ec in e.chars() {
                    hay.push(fold(ec, opts.match_case));
                    src.push(i);
                }
                continue;
            }
            let c = fold(c, opts.match_case);
            if c.is_whitespace() || c == '\0' {
                if hay.last() == Some(&' ') {
                    continue;
                }
                hay.push(' ');
            } else {
                hay.push(c);
            }
            src.push(i);
        }

        let mut out = Vec::new();
        if q.len() > hay.len() {
            return out;
        }
        let mut i = 0;
        while i + q.len() <= hay.len() {
            if hay[i..i + q.len()] == q[..] {
                let ok = !opts.whole_word
                    || ((i == 0 || !is_word(hay[i - 1]) || !is_word(q[0]))
                        && (i + q.len() == hay.len()
                            || !is_word(hay[i + q.len()])
                            || !is_word(q[q.len() - 1])));
                if ok {
                    let start = src[i];
                    let end = src[i + q.len() - 1] + 1;
                    // Ligature expansion can make two hits share a source char.
                    if out.last().is_none_or(|m: &Match| start >= m.end) {
                        out.push(Match { start, end });
                    }
                    i += q.len();
                    continue;
                }
            }
            i += 1;
        }
        out
    }

    /// Line-merged rectangles (user space) covering `[start, end)`.
    pub fn rects(&self, start: usize, end: usize) -> Vec<[f32; 4]> {
        let end = end.min(self.chars.len());
        let mut out: Vec<[f32; 4]> = Vec::new();
        for c in &self.chars[start.min(end)..end] {
            let Some(r) = c.rect else { continue };
            if !(r[2] > r[0] || r[3] > r[1]) {
                continue;
            }
            if let Some(last) = out.last_mut()
                && same_line(last, &r)
            {
                last[0] = last[0].min(r[0]);
                last[1] = last[1].min(r[1]);
                last[2] = last[2].max(r[2]);
                last[3] = last[3].max(r[3]);
                continue;
            }
            out.push(r);
        }
        out
    }

    /// Index of the character nearest to a user-space point, if the page has
    /// any positioned text. A point inside a box wins outright; otherwise
    /// characters on the same line are preferred.
    pub fn char_near(&self, x: f32, y: f32) -> Option<usize> {
        let mut best: Option<(usize, f32)> = None;
        for (i, c) in self.chars.iter().enumerate() {
            let Some(r) = c.rect else { continue };
            if x >= r[0] && x <= r[2] && y >= r[1] && y <= r[3] {
                return Some(i);
            }
            let dx = (r[0] - x).max(0.0).max(x - r[2]);
            let dy = (r[1] - y).max(0.0).max(y - r[3]);
            // Vertical distance counts more: a click left of a line should
            // pick that line, not the line above that happens to reach further.
            let d = dx * dx + 9.0 * dy * dy;
            if best.is_none_or(|b| d < b.1) {
                best = Some((i, d));
            }
        }
        best.map(|b| b.0)
    }

    /// Selection range `[start, end)` between two user-space points. Like a
    /// text editor, a character is included once the pointer passes its
    /// middle. Returns `None` for an empty selection.
    pub fn select(&self, a: (f32, f32), b: (f32, f32)) -> Option<(usize, usize)> {
        let i = self.char_near(a.0, a.1)?;
        let j = self.char_near(b.0, b.1)?;
        let ((s, sp), (e, ep)) = if (i, a.0) <= (j, b.0) {
            ((i, a), (j, b))
        } else {
            ((j, b), (i, a))
        };
        let mid = |k: usize| self.chars[k].rect.map(|r| (r[0] + r[2]) * 0.5);
        let start = if mid(s).is_some_and(|m| sp.0 > m) {
            s + 1
        } else {
            s
        };
        let end = if mid(e).is_some_and(|m| ep.0 < m) {
            e
        } else {
            e + 1
        };
        (end > start).then_some((start, end))
    }

    /// Word around char `i` (for double-click).
    pub fn word_at(&self, i: usize) -> Option<(usize, usize)> {
        if i >= self.chars.len() || !is_word(self.chars[i].ch) {
            return None;
        }
        let mut s = i;
        while s > 0 && is_word(self.chars[s - 1].ch) {
            s -= 1;
        }
        let mut e = i + 1;
        while e < self.chars.len() && is_word(self.chars[e].ch) {
            e += 1;
        }
        Some((s, e))
    }

    /// Line around char `i` (for triple-click): bounded by line breaks.
    pub fn line_at(&self, i: usize) -> Option<(usize, usize)> {
        if i >= self.chars.len() {
            return None;
        }
        let brk = |c: char| c == '\r' || c == '\n';
        let mut s = i;
        while s > 0 && !brk(self.chars[s - 1].ch) {
            s -= 1;
        }
        let mut e = i;
        while e < self.chars.len() && !brk(self.chars[e].ch) {
            e += 1;
        }
        Some((s, e.max(i + 1)))
    }

    /// A short excerpt around a match for result lists.
    pub fn snippet(&self, m: Match, context: usize) -> String {
        let s = m.start.saturating_sub(context);
        let e = (m.end + context).min(self.chars.len());
        let mut t: String = self
            .slice(s, e)
            .chars()
            .map(|c| if c.is_whitespace() { ' ' } else { c })
            .collect();
        t = t.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut out = String::new();
        if s > 0 {
            out.push('…');
        }
        out.push_str(&t);
        if e < self.chars.len() {
            out.push('…');
        }
        out
    }
}

fn same_line(a: &[f32; 4], b: &[f32; 4]) -> bool {
    let (ab, at) = (a[1], a[3]);
    let (bb, bt) = (b[1], b[3]);
    let overlap = at.min(bt) - ab.max(bb);
    let h = (at - ab).min(bt - bb).max(0.1);
    // Same line if they overlap vertically by more than half the smaller
    // height, and the new box does not start far to the left (a new column).
    overlap > 0.5 * h && b[0] >= a[0] - h
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a page from lines; every glyph is 10 wide, lines are 20 apart.
    fn page(lines: &[&str]) -> PageText {
        let mut chars = Vec::new();
        for (li, line) in lines.iter().enumerate() {
            let top = 800.0 - li as f32 * 20.0;
            for (ci, ch) in line.chars().enumerate() {
                let x = 50.0 + ci as f32 * 10.0;
                let rect = if ch == ' ' && ci == 0 {
                    None
                } else {
                    Some([x, top - 12.0, x + 10.0, top])
                };
                chars.push(TextChar { ch, rect });
            }
            if li + 1 < lines.len() {
                chars.push(TextChar {
                    ch: '\r',
                    rect: None,
                });
                chars.push(TextChar {
                    ch: '\n',
                    rect: None,
                });
            }
        }
        PageText { chars }
    }

    fn found(p: &PageText, q: &str, o: SearchOptions) -> Vec<String> {
        p.search(q, o)
            .iter()
            .map(|m| p.slice(m.start, m.end))
            .collect()
    }

    #[test]
    fn case_insensitive_by_default() {
        let p = page(&["The cat and the CAT", "concatenate"]);
        assert_eq!(
            found(&p, "cat", Default::default()),
            vec!["cat", "CAT", "cat"]
        );
        let o = SearchOptions {
            match_case: true,
            ..Default::default()
        };
        assert_eq!(found(&p, "CAT", o), vec!["CAT"]);
    }

    #[test]
    fn whole_word() {
        let p = page(&["cat concatenate cat_s cat."]);
        let o = SearchOptions {
            whole_word: true,
            ..Default::default()
        };
        let m = p.search("cat", o);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].start, 0);
        assert_eq!(p.slice(m[1].start, m[1].end + 1), "cat.");
    }

    #[test]
    fn whitespace_and_line_breaks() {
        let p = page(&["the quick", "brown fox"]);
        assert_eq!(found(&p, "quick   brown", Default::default()).len(), 1);
        assert_eq!(found(&p, "quick\nbrown", Default::default()).len(), 1);
        assert!(found(&p, "   ", Default::default()).is_empty());
        assert!(found(&p, "", Default::default()).is_empty());
    }

    #[test]
    fn typographic_normalisation_and_ligatures() {
        let p = page(&["it\u{2019}s an o\u{FB03}ce \u{201C}x\u{201D} 1\u{2013}2"]);
        assert_eq!(found(&p, "it's", Default::default()).len(), 1);
        assert_eq!(found(&p, "office", Default::default()), vec!["o\u{FB03}ce"]);
        assert_eq!(found(&p, "\"x\"", Default::default()).len(), 1);
        assert_eq!(found(&p, "1-2", Default::default()).len(), 1);
    }

    #[test]
    fn hyphenation_break_is_ignored() {
        let mut p = page(&["hyphen"]);
        p.chars.push(TextChar {
            ch: '\u{2}',
            rect: Some([110.0, 788.0, 115.0, 800.0]),
        });
        p.chars.push(TextChar {
            ch: '\r',
            rect: None,
        });
        p.chars.push(TextChar {
            ch: '\n',
            rect: None,
        });
        for (i, ch) in "ated".chars().enumerate() {
            let x = 50.0 + i as f32 * 10.0;
            p.chars.push(TextChar {
                ch,
                rect: Some([x, 768.0, x + 10.0, 780.0]),
            });
        }
        assert_eq!(found(&p, "hyphenated", Default::default()).len(), 1);
        assert!(found(&p, "hyphen ated", Default::default()).is_empty());
        assert_eq!(p.all(), "hyphen-\nated");
    }

    #[test]
    fn non_overlapping_results() {
        let p = page(&["aaaa"]);
        assert_eq!(p.search("aa", Default::default()).len(), 2);
    }

    #[test]
    fn rects_merge_per_line() {
        let p = page(&["hello world", "second"]);
        let m = p.search("world second", Default::default());
        assert_eq!(m.len(), 1);
        let r = p.rects(m[0].start, m[0].end);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0], [110.0, 788.0, 160.0, 800.0]);
        assert_eq!(r[1], [50.0, 768.0, 110.0, 780.0]);
    }

    #[test]
    fn selection_and_words() {
        let p = page(&["hello world", "second line"]);
        // Drag from inside "world" on line 1 to "sec" on line 2.
        let (s, e) = p.select((125.0, 795.0), (75.0, 775.0)).unwrap();
        assert_eq!(p.slice(s, e), "orld\nsec");
        // Short of the middle of 'c': it is not included. Order does not matter.
        let (s, e) = p.select((72.0, 775.0), (125.0, 795.0)).unwrap();
        assert_eq!(p.slice(s, e), "orld\nse");
        // A click without a drag selects nothing.
        assert_eq!(p.select((122.0, 795.0), (122.0, 795.0)), None);
        // Clicking left of the second line picks its first char.
        assert_eq!(p.char_near(0.0, 770.0), Some(13));
        let (ws, we) = p.word_at(8).unwrap();
        assert_eq!(p.slice(ws, we), "world");
        assert_eq!(p.word_at(5), None); // the space
        let (ls, le) = p.line_at(14).unwrap();
        assert_eq!(p.slice(ls, le), "second line");
    }

    #[test]
    fn empty_page_is_safe() {
        let p = PageText::default();
        assert!(p.search("x", Default::default()).is_empty());
        assert_eq!(p.char_near(1.0, 1.0), None);
        assert_eq!(p.select((0.0, 0.0), (1.0, 1.0)), None);
        assert!(p.rects(5, 2).is_empty());
        assert_eq!(p.slice(10, 3), "");
        assert_eq!(p.word_at(0), None);
    }

    #[test]
    fn snippet_has_context_and_ellipses() {
        let p = page(&["one two three four five six seven"]);
        let m = p.search("four", Default::default())[0];
        assert_eq!(p.snippet(m, 6), "…three four five…");
    }
}
