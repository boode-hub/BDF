//! Page layout: where every page sits in a document view, in display points
//! at zoom 1.
//!
//! All modes share one model: pages are grouped into rows (one page per row,
//! or two for facing modes). Continuous modes scroll through every row;
//! non-continuous modes show one row at a time, which the view enforces by
//! clamping its scroll range to the current row.

use crate::geom::{Rect, Rotation};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum PageMode {
    /// One page at a time.
    Single,
    /// All pages in one vertical strip.
    #[default]
    Continuous,
    /// Two pages side by side, one spread at a time.
    Facing,
    /// Two pages side by side, scrolling.
    ContinuousFacing,
}

impl PageMode {
    pub fn is_continuous(self) -> bool {
        matches!(self, PageMode::Continuous | PageMode::ContinuousFacing)
    }

    pub fn is_facing(self) -> bool {
        matches!(self, PageMode::Facing | PageMode::ContinuousFacing)
    }

    pub const ALL: [PageMode; 4] = [
        PageMode::Single,
        PageMode::Continuous,
        PageMode::Facing,
        PageMode::ContinuousFacing,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PageMode::Single => "Single page",
            PageMode::Continuous => "Continuous",
            PageMode::Facing => "Facing",
            PageMode::ContinuousFacing => "Continuous facing",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutParams {
    pub mode: PageMode,
    /// In facing modes, show the first page on its own (book layout).
    pub cover: bool,
    pub rotation: Rotation,
    /// Gap between pages and around the edges, in points.
    pub gap: f32,
}

impl Default for LayoutParams {
    fn default() -> Self {
        Self {
            mode: PageMode::Continuous,
            cover: true,
            rotation: Rotation::R0,
            gap: 12.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub y0: f32,
    pub y1: f32,
    pub first: usize,
    pub count: usize,
}

#[derive(Clone, Debug, Default)]
pub struct Layout {
    /// Page rectangles, indexed by page number, in display points.
    pub pages: Vec<Rect>,
    /// Row index of each page.
    pub row_of: Vec<usize>,
    pub rows: Vec<Row>,
    pub width: f32,
    pub height: f32,
    pub gap: f32,
}

impl Layout {
    /// `sizes` are each page's display size before the view rotation.
    pub fn new(sizes: &[(f32, f32)], p: &LayoutParams) -> Self {
        let gap = p.gap.max(0.0);
        let n = sizes.len();
        let rotated: Vec<(f32, f32)> = sizes
            .iter()
            .map(|&(w, h)| {
                let w = if w.is_finite() && w > 0.0 { w } else { 612.0 };
                let h = if h.is_finite() && h > 0.0 { h } else { 792.0 };
                p.rotation.apply_size((w, h))
            })
            .collect();

        // Group pages into rows.
        let mut groups: Vec<(usize, usize)> = Vec::new();
        if p.mode.is_facing() {
            let mut i = 0;
            if p.cover && n > 0 {
                groups.push((0, 1));
                i = 1;
            }
            while i < n {
                let c = (n - i).min(2);
                groups.push((i, c));
                i += c;
            }
        } else {
            groups.extend((0..n).map(|i| (i, 1)));
        }

        let row_width = |&(first, count): &(usize, usize)| -> f32 {
            rotated[first..first + count]
                .iter()
                .map(|s| s.0)
                .sum::<f32>()
                + gap * (count.saturating_sub(1)) as f32
        };
        // In facing modes a lone cover page sits on the right, like a book.
        let content_w = groups
            .iter()
            .map(|g| {
                if p.mode.is_facing() && g.1 == 1 && n > 1 {
                    rotated[g.0].0 * 2.0 + gap
                } else {
                    row_width(g)
                }
            })
            .fold(0.0f32, f32::max);
        let width = content_w + 2.0 * gap;

        let mut pages = vec![Rect::default(); n];
        let mut row_of = vec![0; n];
        let mut rows = Vec::with_capacity(groups.len());
        let mut y = gap;
        for (ri, &(first, count)) in groups.iter().enumerate() {
            let rh = rotated[first..first + count]
                .iter()
                .map(|s| s.1)
                .fold(0.0f32, f32::max);
            let rw = row_width(&(first, count));
            let mut x = gap + (content_w - rw) * 0.5;
            if p.mode.is_facing() && count == 1 && n > 1 {
                let w = rotated[first].0;
                let is_cover = p.cover && first == 0;
                // Cover sits on the right half; a trailing odd page on the left half.
                x = if is_cover {
                    gap + content_w * 0.5 + gap * 0.5
                } else {
                    gap + content_w * 0.5 - gap * 0.5 - w
                };
            }
            for i in first..first + count {
                let (w, h) = rotated[i];
                let top = y + (rh - h) * 0.5;
                pages[i] = Rect::new(x, top, x + w, top + h);
                row_of[i] = ri;
                x += w + gap;
            }
            rows.push(Row {
                y0: y,
                y1: y + rh,
                first,
                count,
            });
            y += rh + gap;
        }
        let height = if rows.is_empty() { 2.0 * gap } else { y };
        Layout {
            pages,
            row_of,
            rows,
            width,
            height,
            gap,
        }
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// Rows overlapping the vertical band `[y0, y1)` (zoom-1 points).
    pub fn rows_in(&self, y0: f32, y1: f32) -> std::ops::Range<usize> {
        let start = self.rows.partition_point(|r| r.y1 + self.gap * 0.5 < y0);
        let end = self.rows.partition_point(|r| r.y0 - self.gap * 0.5 < y1);
        start..end.max(start)
    }

    /// Pages overlapping the band, in order.
    pub fn pages_in(&self, y0: f32, y1: f32) -> impl Iterator<Item = usize> + '_ {
        self.rows_in(y0, y1).flat_map(move |ri| {
            let r = &self.rows[ri];
            r.first..r.first + r.count
        })
    }

    /// Page under a point, if any.
    pub fn page_at(&self, x: f32, y: f32) -> Option<usize> {
        self.pages_in(y, y + f32::EPSILON)
            .find(|&i| self.pages[i].contains(x, y))
    }

    /// Page closest to a point (for clicks in the gutter).
    pub fn nearest_page(&self, x: f32, y: f32) -> Option<usize> {
        let rows = self.rows_in(y - 1000.0, y + 1000.0);
        let candidates: Vec<usize> = if rows.is_empty() {
            (0..self.pages.len()).collect()
        } else {
            rows.flat_map(|ri| {
                let r = &self.rows[ri];
                r.first..r.first + r.count
            })
            .collect()
        };
        candidates.into_iter().min_by(|&a, &b| {
            self.pages[a]
                .dist2(x, y)
                .total_cmp(&self.pages[b].dist2(x, y))
        })
    }

    /// The "current" page for a viewport: the page covering the most of it,
    /// preferring the earliest on ties.
    pub fn current_page(&self, view_y0: f32, view_y1: f32) -> usize {
        let mut best = (0usize, -1.0f32);
        for i in self.pages_in(view_y0, view_y1) {
            let r = &self.pages[i];
            let vis = (r.y1.min(view_y1) - r.y0.max(view_y0)).max(0.0);
            if vis > best.1 + 0.5 {
                best = (i, vis);
            }
        }
        best.0
    }

    /// Zoom that makes the widest row fill `viewport_w` points.
    pub fn fit_width_zoom(&self, viewport_w: f32) -> f32 {
        if self.width <= 0.0 {
            return 1.0;
        }
        (viewport_w / self.width).max(0.01)
    }

    /// Zoom that makes the row holding `page` exactly as wide as the
    /// viewport. Fitting the current row rather than the widest page keeps
    /// documents that mix portrait and landscape pages readable.
    pub fn fit_row_width_zoom(&self, page: usize, viewport_w: f32) -> f32 {
        let Some(&ri) = self.row_of.get(page) else {
            return self.fit_width_zoom(viewport_w);
        };
        let r = &self.rows[ri];
        let mut w: f32 = self.pages[r.first..r.first + r.count]
            .iter()
            .map(|p| p.width())
            .sum::<f32>()
            + self.gap * (r.count.saturating_sub(1)) as f32;
        // A lone cover page in a facing layout counts as a full spread.
        if r.count == 1 && self.rows.iter().any(|x| x.count == 2) {
            w = w * 2.0 + self.gap;
        }
        let w = w + 2.0 * self.gap;
        if w <= 0.0 {
            1.0
        } else {
            (viewport_w / w).max(0.01)
        }
    }

    /// Zoom that makes the row containing `page` fit the viewport entirely.
    pub fn fit_page_zoom(&self, page: usize, viewport_w: f32, viewport_h: f32) -> f32 {
        let Some(&ri) = self.row_of.get(page) else {
            return 1.0;
        };
        let r = &self.rows[ri];
        let h = (r.y1 - r.y0) + 2.0 * self.gap;
        let zw = self.fit_row_width_zoom(page, viewport_w);
        let zh = if h > 0.0 { viewport_h / h } else { zw };
        zw.min(zh).max(0.01)
    }

    /// Vertical extent of the row holding `page`, including half the gap
    /// on each side; used to confine scrolling in non-continuous modes.
    pub fn row_band(&self, page: usize) -> (f32, f32) {
        match self.row_of.get(page) {
            Some(&ri) => {
                let r = &self.rows[ri];
                (r.y0 - self.gap, r.y1 + self.gap)
            }
            None => (0.0, self.height),
        }
    }

    /// First page of the row after (`dir > 0`) or before the one holding `page`.
    pub fn step_row(&self, page: usize, dir: i32) -> usize {
        let Some(&ri) = self.row_of.get(page) else {
            return 0;
        };
        let target = (ri as i64 + dir as i64).clamp(0, self.rows.len() as i64 - 1) as usize;
        self.rows[target].first
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn letter(n: usize) -> Vec<(f32, f32)> {
        vec![(612.0, 792.0); n]
    }

    #[test]
    fn continuous_stacks_pages() {
        let l = Layout::new(&letter(3), &LayoutParams::default());
        assert_eq!(l.rows.len(), 3);
        assert_eq!(l.pages[0], Rect::new(12.0, 12.0, 624.0, 804.0));
        assert_eq!(l.pages[1].y0, 804.0 + 12.0);
        assert_eq!(l.width, 636.0);
        assert_eq!(l.height, 12.0 + 3.0 * (792.0 + 12.0));
    }

    #[test]
    fn mixed_sizes_are_centred() {
        let l = Layout::new(&[(612.0, 792.0), (300.0, 400.0)], &LayoutParams::default());
        let a = l.pages[0];
        let b = l.pages[1];
        assert!((a.center().0 - b.center().0).abs() < 1e-3);
    }

    #[test]
    fn facing_with_cover_puts_cover_on_the_right() {
        let p = LayoutParams {
            mode: PageMode::ContinuousFacing,
            ..Default::default()
        };
        let l = Layout::new(&letter(4), &p);
        assert_eq!(l.rows.len(), 3); // [0] [1 2] [3]
        assert_eq!(l.rows[1].first, 1);
        assert_eq!(l.rows[1].count, 2);
        let mid = l.width * 0.5;
        assert!(l.pages[0].x0 > mid - 1.0, "cover should be right of centre");
        assert!(
            l.pages[3].x1 < mid + 1.0,
            "trailing page should be left of centre"
        );
        assert!(l.pages[1].x1 < l.pages[2].x0);
    }

    #[test]
    fn facing_without_cover_pairs_from_page_one() {
        let p = LayoutParams {
            mode: PageMode::Facing,
            cover: false,
            ..Default::default()
        };
        let l = Layout::new(&letter(5), &p);
        assert_eq!(
            l.rows
                .iter()
                .map(|r| (r.first, r.count))
                .collect::<Vec<_>>(),
            vec![(0, 2), (2, 2), (4, 1)]
        );
    }

    #[test]
    fn rotation_swaps_sizes() {
        let p = LayoutParams {
            rotation: Rotation::R90,
            ..Default::default()
        };
        let l = Layout::new(&letter(1), &p);
        assert_eq!(l.pages[0].width(), 792.0);
        assert_eq!(l.pages[0].height(), 612.0);
    }

    #[test]
    fn visibility_queries() {
        let l = Layout::new(&letter(100), &LayoutParams::default());
        let row_h = 792.0 + 12.0;
        let v: Vec<usize> = l
            .pages_in(row_h * 10.0 + 100.0, row_h * 11.0 + 100.0)
            .collect();
        assert_eq!(v, vec![10, 11]);
        assert_eq!(
            l.current_page(row_h * 10.0 + 100.0, row_h * 11.0 + 100.0),
            10
        );
        assert_eq!(
            l.current_page(row_h * 10.0 + 500.0, row_h * 11.0 + 500.0),
            11
        );
        assert_eq!(l.page_at(300.0, row_h * 42.0 + 50.0), Some(42));
        assert_eq!(l.page_at(5.0, row_h * 42.0 + 50.0), None); // in the margin
        assert_eq!(l.nearest_page(5.0, row_h * 42.0 + 50.0), Some(42));
        assert_eq!(l.pages_in(-500.0, -1.0).count(), 0);
        assert_eq!(l.pages_in(l.height + 10.0, l.height + 20.0).count(), 0);
    }

    #[test]
    fn fit_zooms() {
        let l = Layout::new(&letter(2), &LayoutParams::default());
        assert!((l.fit_width_zoom(636.0) - 1.0).abs() < 1e-6);
        let z = l.fit_page_zoom(0, 2000.0, 816.0);
        assert!((z - 816.0 / (792.0 + 24.0)).abs() < 1e-6);
    }

    #[test]
    fn fit_width_uses_the_current_page() {
        let l = Layout::new(&[(612.0, 792.0), (792.0, 612.0)], &LayoutParams::default());
        assert!((l.fit_row_width_zoom(0, 636.0) - 1.0).abs() < 1e-6);
        assert!((l.fit_row_width_zoom(1, 816.0) - 1.0).abs() < 1e-6);
        let p = LayoutParams {
            mode: PageMode::ContinuousFacing,
            ..Default::default()
        };
        let l = Layout::new(&[(612.0, 792.0); 3], &p);
        // The cover fits as if it were a spread, like the pair after it.
        assert!((l.fit_row_width_zoom(0, 1260.0) - l.fit_row_width_zoom(1, 1260.0)).abs() < 1e-6);
    }

    #[test]
    fn row_stepping_and_bands() {
        let p = LayoutParams {
            mode: PageMode::Facing,
            ..Default::default()
        };
        let l = Layout::new(&letter(6), &p);
        assert_eq!(l.step_row(0, 1), 1);
        assert_eq!(l.step_row(2, 1), 3);
        assert_eq!(l.step_row(5, 1), 5);
        assert_eq!(l.step_row(3, -1), 1);
        assert_eq!(l.step_row(0, -1), 0);
        let (a, b) = l.row_band(2);
        assert!(a < l.pages[1].y0 && b > l.pages[2].y1);
    }

    #[test]
    fn degenerate_inputs_do_not_panic() {
        let l = Layout::new(&[], &LayoutParams::default());
        assert_eq!(l.current_page(0.0, 100.0), 0);
        assert_eq!(l.page_at(1.0, 1.0), None);
        assert_eq!(l.nearest_page(1.0, 1.0), None);
        assert_eq!(l.step_row(3, 1), 0);
        let l = Layout::new(&[(f32::NAN, -3.0), (0.0, 0.0)], &LayoutParams::default());
        assert!(l.pages.iter().all(|r| !r.is_empty()));
    }
}
