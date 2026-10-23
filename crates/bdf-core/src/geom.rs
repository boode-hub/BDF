//! Geometry shared by the engine and the UI.
//!
//! Two coordinate spaces matter:
//!
//! * **User space**: PDF points, origin at the bottom-left of the page box,
//!   y grows upwards. This is what PDFium reports for text, links and
//!   annotations, and what it expects when we create annotations.
//! * **Display space**: PDF points, origin at the top-left of the page *as it
//!   is shown* (after the page's own `/Rotate` and the view rotation),
//!   y grows downwards. Layout, hit-testing and drawing work in this space.
//!
//! [`PageSpace`] converts between the two.

use serde::{Deserialize, Serialize};

/// Axis-aligned rectangle. In display space `y0 < y1` means top-to-bottom.
#[derive(Clone, Copy, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Rect {
    pub const fn new(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Self { x0, y0, x1, y1 }
    }

    pub fn from_points(a: (f32, f32), b: (f32, f32)) -> Self {
        Self::new(a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1))
    }

    pub fn width(&self) -> f32 {
        self.x1 - self.x0
    }

    pub fn height(&self) -> f32 {
        self.y1 - self.y0
    }

    pub fn is_empty(&self) -> bool {
        !(self.x1 > self.x0 && self.y1 > self.y0)
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x0 && x <= self.x1 && y >= self.y0 && y <= self.y1
    }

    pub fn intersects(&self, o: &Rect) -> bool {
        self.x0 < o.x1 && o.x0 < self.x1 && self.y0 < o.y1 && o.y0 < self.y1
    }

    pub fn union(&self, o: &Rect) -> Rect {
        Rect::new(
            self.x0.min(o.x0),
            self.y0.min(o.y0),
            self.x1.max(o.x1),
            self.y1.max(o.y1),
        )
    }

    pub fn expand(&self, by: f32) -> Rect {
        Rect::new(self.x0 - by, self.y0 - by, self.x1 + by, self.y1 + by)
    }

    pub fn center(&self) -> (f32, f32) {
        ((self.x0 + self.x1) * 0.5, (self.y0 + self.y1) * 0.5)
    }

    /// Squared distance from a point to the rectangle (0 inside).
    pub fn dist2(&self, x: f32, y: f32) -> f32 {
        let dx = (self.x0 - x).max(0.0).max(x - self.x1);
        let dy = (self.y0 - y).max(0.0).max(y - self.y1);
        dx * dx + dy * dy
    }

    pub fn scale(&self, s: f32) -> Rect {
        Rect::new(self.x0 * s, self.y0 * s, self.x1 * s, self.y1 * s)
    }

    pub fn translate(&self, dx: f32, dy: f32) -> Rect {
        Rect::new(self.x0 + dx, self.y0 + dy, self.x1 + dx, self.y1 + dy)
    }
}

/// A clockwise rotation in quarter turns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Rotation {
    #[default]
    R0,
    R90,
    R180,
    R270,
}

impl Rotation {
    pub fn from_quarters(q: i32) -> Self {
        match q.rem_euclid(4) {
            0 => Rotation::R0,
            1 => Rotation::R90,
            2 => Rotation::R180,
            _ => Rotation::R270,
        }
    }

    /// Accepts any multiple of 90 degrees; anything else snaps to the nearest quarter.
    pub fn from_degrees(d: i32) -> Self {
        Self::from_quarters(((d as f32) / 90.0).round() as i32)
    }

    pub fn quarters(self) -> i32 {
        match self {
            Rotation::R0 => 0,
            Rotation::R90 => 1,
            Rotation::R180 => 2,
            Rotation::R270 => 3,
        }
    }

    pub fn degrees(self) -> i32 {
        self.quarters() * 90
    }

    pub fn then(self, other: Rotation) -> Rotation {
        Rotation::from_quarters(self.quarters() + other.quarters())
    }

    pub fn cw(self) -> Rotation {
        self.then(Rotation::R90)
    }

    pub fn ccw(self) -> Rotation {
        self.then(Rotation::R270)
    }

    pub fn swaps_axes(self) -> bool {
        matches!(self, Rotation::R90 | Rotation::R270)
    }

    /// Size after rotating a `(w, h)` box.
    pub fn apply_size(self, (w, h): (f32, f32)) -> (f32, f32) {
        if self.swaps_axes() { (h, w) } else { (w, h) }
    }
}

/// Maps a page between user space and display space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageSpace {
    /// Page box in user space: left, bottom, right, top.
    pub bbox: [f32; 4],
    /// Total clockwise rotation: the page's `/Rotate` plus the view rotation.
    pub rotation: Rotation,
}

impl PageSpace {
    pub fn new(bbox: [f32; 4], rotation: Rotation) -> Self {
        let [l, b, r, t] = bbox;
        Self {
            bbox: [l.min(r), b.min(t), l.max(r), b.max(t)],
            rotation,
        }
    }

    fn unrotated_size(&self) -> (f32, f32) {
        (self.bbox[2] - self.bbox[0], self.bbox[3] - self.bbox[1])
    }

    /// Display width and height in points.
    pub fn size(&self) -> (f32, f32) {
        self.rotation.apply_size(self.unrotated_size())
    }

    pub fn to_display(&self, x: f32, y: f32) -> (f32, f32) {
        let (w0, h0) = self.unrotated_size();
        let u = x - self.bbox[0];
        let v = self.bbox[3] - y;
        match self.rotation {
            Rotation::R0 => (u, v),
            Rotation::R90 => (h0 - v, u),
            Rotation::R180 => (w0 - u, h0 - v),
            Rotation::R270 => (v, w0 - u),
        }
    }

    pub fn to_user(&self, dx: f32, dy: f32) -> (f32, f32) {
        let (w0, h0) = self.unrotated_size();
        let (u, v) = match self.rotation {
            Rotation::R0 => (dx, dy),
            Rotation::R90 => (dy, h0 - dx),
            Rotation::R180 => (w0 - dx, h0 - dy),
            Rotation::R270 => (w0 - dy, dx),
        };
        (u + self.bbox[0], self.bbox[3] - v)
    }

    /// Converts a user-space rectangle `[left, bottom, right, top]` to display space.
    pub fn rect_to_display(&self, r: [f32; 4]) -> Rect {
        Rect::from_points(self.to_display(r[0], r[1]), self.to_display(r[2], r[3]))
    }

    /// Converts a display rectangle to user space `[left, bottom, right, top]`.
    pub fn rect_to_user(&self, r: &Rect) -> [f32; 4] {
        let a = self.to_user(r.x0, r.y0);
        let b = self.to_user(r.x1, r.y1);
        [a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)]
    }
}

/// Quantisation of the render scale (device pixels per PDF point).
pub const SCALE_DENOM: f32 = 4096.0;

/// Rendering scale, stored as an integer so it can key caches.
pub fn quantize_scale(px_per_pt: f32) -> u32 {
    (px_per_pt.clamp(0.01, 64.0) * SCALE_DENOM).round() as u32
}

pub fn scale_of(q: u32) -> f32 {
    q as f32 / SCALE_DENOM
}

/// Pixel size of a whole page of `size_pt` (display space) at scale `q`.
/// Both the UI and the engine use this, so tile grids always agree.
pub fn page_px(size_pt: (f32, f32), q: u32) -> (u32, u32) {
    let s = scale_of(q);
    let w = (size_pt.0 * s).round().max(1.0);
    let h = (size_pt.1 * s).round().max(1.0);
    (w.min(1_000_000.0) as u32, h.min(1_000_000.0) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: (f32, f32), b: (f32, f32)) -> bool {
        (a.0 - b.0).abs() < 1e-3 && (a.1 - b.1).abs() < 1e-3
    }

    #[test]
    fn rotation_arithmetic() {
        assert_eq!(Rotation::R270.cw(), Rotation::R0);
        assert_eq!(Rotation::R0.ccw(), Rotation::R270);
        assert_eq!(Rotation::from_degrees(-90), Rotation::R270);
        assert_eq!(Rotation::from_degrees(450), Rotation::R90);
        assert_eq!(Rotation::R90.then(Rotation::R180), Rotation::R270);
    }

    #[test]
    fn corners_map_like_a_clockwise_turn() {
        // Letter page with an offset box, to catch origin mistakes.
        let bbox = [10.0, 20.0, 622.0, 812.0]; // 612 x 792
        let s = PageSpace::new(bbox, Rotation::R90);
        assert!(close(s.size(), (792.0, 612.0)));
        // Unrotated top-left goes to the top-right after a clockwise quarter turn.
        assert!(close(s.to_display(10.0, 812.0), (792.0, 0.0)));
        // Unrotated bottom-left goes to the top-left.
        assert!(close(s.to_display(10.0, 20.0), (0.0, 0.0)));
        let s = PageSpace::new(bbox, Rotation::R270);
        assert!(close(s.to_display(10.0, 812.0), (0.0, 612.0)));
        let s = PageSpace::new(bbox, Rotation::R180);
        assert!(close(s.to_display(10.0, 812.0), (612.0, 792.0)));
    }

    #[test]
    fn round_trip_every_rotation() {
        let bbox = [-5.0, 3.0, 400.0, 900.0];
        for q in 0..4 {
            let s = PageSpace::new(bbox, Rotation::from_quarters(q));
            for &(x, y) in &[(0.0, 3.0), (17.5, 400.25), (399.0, 899.0), (-5.0, 900.0)] {
                let d = s.to_display(x, y);
                assert!(close(s.to_user(d.0, d.1), (x, y)), "q={q} ({x},{y}) -> {d:?}");
                let (w, h) = s.size();
                assert!(d.0 >= -1e-3 && d.0 <= w + 1e-3 && d.1 >= -1e-3 && d.1 <= h + 1e-3);
            }
            let r = [10.0, 20.0, 30.0, 60.0];
            let back = s.rect_to_user(&s.rect_to_display(r));
            for i in 0..4 {
                assert!((back[i] - r[i]).abs() < 1e-3);
            }
        }
    }

    #[test]
    fn rect_helpers() {
        let r = Rect::new(0.0, 0.0, 10.0, 5.0);
        assert!(r.contains(10.0, 5.0));
        assert!(!r.contains(10.1, 5.0));
        assert_eq!(r.dist2(13.0, 9.0), 25.0);
        assert!(r.intersects(&Rect::new(9.0, 4.0, 20.0, 20.0)));
        assert!(!r.intersects(&Rect::new(10.0, 0.0, 20.0, 5.0)));
        assert!(Rect::new(1.0, 1.0, 1.0, 2.0).is_empty());
    }

    #[test]
    fn scale_quantisation_is_stable() {
        let q = quantize_scale(1.5);
        assert_eq!(scale_of(q), 1.5);
        assert_eq!(page_px((612.0, 792.0), q), (918, 1188));
        assert_eq!(page_px((0.0, 0.0), q), (1, 1));
        assert_eq!(quantize_scale(f32::NAN.max(0.0)), quantize_scale(0.01));
    }
}
