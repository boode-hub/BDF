// BDF's Sharingan artwork, drawn procedurally at any size so it stays crisp
// from a 16 px taskbar icon to the welcome screen. Shared by the window and
// in-app images (theme.rs) and the Windows .exe icon (build.rs).
//
// * `icon_rgba`: a Mangekyou-style eye: crimson iris, three curved blades
//   meeting at the pupil, a black rim and a small reflection.
// * `tomoe_rgba`: the classic three-tomoe Sharingan, used as the spinner.

use std::f32::consts::{PI, TAU};

type Rgba = [f32; 4];

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Smallest signed difference between two angles, in (-PI, PI].
fn angle_diff(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(TAU);
    if d > PI { d - TAU } else { d }
}

const BLACK: [f32; 3] = [0.03, 0.02, 0.02];
const RED_HOT: [f32; 3] = [1.0, 0.17, 0.15];
const RED_DEEP: [f32; 3] = [0.52, 0.0, 0.02];

/// Iris colour at radius `r` (0 centre, 1 rim): bright near the pupil, deep at the edge.
fn iris(r: f32) -> [f32; 3] {
    mix(RED_HOT, RED_DEEP, ((r - 0.2) / 0.75).powf(1.3))
}

/// Supersamples `shade` (coordinates in [-1, 1], y up) into an n x n RGBA image.
fn render(n: usize, shade: impl Fn(f32, f32) -> Rgba) -> Vec<u8> {
    let ss = if n <= 64 { 4 } else { 3 };
    let mut out = vec![0u8; n * n * 4];
    for py in 0..n {
        for px in 0..n {
            let mut acc = [0.0f32; 4];
            for sy in 0..ss {
                for sx in 0..ss {
                    let x = ((px as f32 + (sx as f32 + 0.5) / ss as f32) / n as f32) * 2.0 - 1.0;
                    let y = 1.0 - ((py as f32 + (sy as f32 + 0.5) / ss as f32) / n as f32) * 2.0;
                    let c = shade(x, y);
                    // Premultiply while averaging so edges blend correctly.
                    acc[0] += c[0] * c[3];
                    acc[1] += c[1] * c[3];
                    acc[2] += c[2] * c[3];
                    acc[3] += c[3];
                }
            }
            let k = (ss * ss) as f32;
            let a = acc[3] / k;
            let i = (py * n + px) * 4;
            if a > 0.0 {
                for c in 0..3 {
                    out[i + c] = ((acc[c] / acc[3]).clamp(0.0, 1.0) * 255.0).round() as u8;
                }
            }
            out[i + 3] = (a * 255.0).round() as u8;
        }
    }
    out
}

/// The application icon: a Mangekyou-style eye, `n` x `n` RGBA.
pub fn icon_rgba(n: usize) -> Vec<u8> {
    render(n, |x, y| {
        let r = (x * x + y * y).sqrt();
        if r > 1.0 {
            return [0.0; 4];
        }
        let theta = y.atan2(x);
        // Black rim with a thin crimson edge so it reads on dark taskbars.
        if r > 0.9 {
            let c = if r > 0.975 { [0.45, 0.0, 0.03] } else { BLACK };
            return [c[0], c[1], c[2], 1.0];
        }
        let mut c = iris(r);
        // Three blades curving clockwise out of the pupil, widest mid-way.
        let rot = 0.35;
        for k in 0..3 {
            let base = rot + k as f32 * TAU / 3.0;
            let centre = base - 2.1 * r.powf(1.15);
            let t = ((r - 0.1) / 0.8).clamp(0.0, 1.0);
            let half_width = 0.06 + 0.2 * (PI * t).sin().powf(0.75) * (1.0 - 0.35 * t);
            // Blades are sharper on their leading edge: offset the profile.
            let d = angle_diff(theta, centre) * r;
            let lean = if d > 0.0 { 1.0 } else { 0.62 };
            if d.abs() < half_width * lean && r > 0.08 {
                c = BLACK;
            }
        }
        // Pupil, with a faint red halo.
        if r < 0.17 {
            c = BLACK;
        } else if r < 0.2 {
            c = mix(BLACK, c, (r - 0.17) / 0.03);
        }
        // A small reflection, upper left.
        let (hx, hy) = (x + 0.36, y - 0.42);
        let h = (hx * hx + hy * hy).sqrt();
        if h < 0.09 {
            c = mix(c, [1.0, 0.95, 0.95], (1.0 - h / 0.09) * 0.85);
        }
        [c[0], c[1], c[2], 1.0]
    })
}

/// The classic three-tomoe Sharingan, `n` x `n` RGBA (used as the spinner).
pub fn tomoe_rgba(n: usize) -> Vec<u8> {
    render(n, |x, y| {
        let r = (x * x + y * y).sqrt();
        if r > 1.0 {
            return [0.0; 4];
        }
        if r > 0.9 {
            return [BLACK[0], BLACK[1], BLACK[2], 1.0];
        }
        let theta = y.atan2(x);
        let mut c = iris(r);
        let ring = 0.57;
        if (r - ring).abs() < 0.022 {
            c = mix(c, BLACK, 0.85);
        }
        for k in 0..3 {
            let a = k as f32 * TAU / 3.0 + PI / 2.0;
            // Head: a disc sitting on the ring.
            let (cx, cy) = (ring * a.cos(), ring * a.sin());
            let head = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
            if head < 0.13 {
                c = BLACK;
            }
            // Tail: a tapering comma trailing counter-clockwise, curling outward.
            let phi = angle_diff(a, theta);
            if phi > 0.0 && phi < 1.05 {
                let f = phi / 1.05;
                let rc = ring + 0.07 * f + 0.05 * (PI * f).sin();
                let half = 0.12 * (1.0 - f).powf(1.4);
                if (r - rc).abs() < half {
                    c = BLACK;
                }
            }
        }
        if r < 0.15 {
            c = BLACK;
        }
        [c[0], c[1], c[2], 1.0]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn images_are_round_and_opaque_inside() {
        for img in [icon_rgba(64), tomoe_rgba(64)] {
            assert_eq!(img.len(), 64 * 64 * 4);
            assert_eq!(img[3], 0, "corner is transparent");
            assert_eq!(img[(32 * 64 + 32) * 4 + 3], 255, "centre is opaque");
        }
        // Raw RGBA dumps for reviewing the artwork.
        if let Some(dir) = std::env::var_os("BDF_ICON_PREVIEW") {
            let dir = std::path::PathBuf::from(dir);
            std::fs::write(dir.join("icon-256.rgba"), icon_rgba(256)).unwrap();
            std::fs::write(dir.join("icon-32.rgba"), icon_rgba(32)).unwrap();
            std::fs::write(dir.join("tomoe-256.rgba"), tomoe_rgba(256)).unwrap();
        }
    }
}
