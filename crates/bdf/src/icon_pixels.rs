// BDF's logo: the Mangekyou Sharingan of Kakashi (default) or Itachi, drawn
// from the vector artwork in BOoDe007/Sharingan-Monitor (eyes/Kakashi-Obito.svg
// and eyes/Itachi.svg; MIT licence), so the eyes match that app exactly. Shared
// by the window and in-app images (theme.rs) and the Windows .exe icon
// (build.rs).
//
// The SVGs are reproduced as-is in their 300-unit space: an iris with a radial
// gradient and a black rim, the black pattern (cubic Béziers, nonzero fill),
// then the red centre. Each image is rendered for its exact pixel size with
// 64 to 256 samples per pixel, so edges stay smooth at every size.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Eye {
    Kakashi,
    Itachi,
}

type Pt = (f32, f32);
/// A closed path: a start point and cubic segments (control, control, end).
type Path = (Pt, &'static [[Pt; 3]]);

// eyes/Kakashi-Obito.svg: "tomoe 1" (two paths), repeated at +120 and -120 degrees.
const KAKASHI_TOMOE: [Path; 2] = [
    (
        (107.5, 150.0),
        &[
            [(110.0, 100.0), (130.0, 20.0), (150.0, 0.0)],
            [(170.0, 20.0), (190.0, 100.0), (192.5, 150.0)],
            [(168.0, 150.0), (132.0, 150.0), (107.5, 150.0)],
        ],
    ),
    (
        (250.2, 181.2),
        &[
            [(261.8, 142.0), (250.4, 46.8), (157.4, 10.0)],
            [(154.9, 31.7), (161.1, 51.3), (178.8, 67.3)],
            [(216.6, 76.0), (258.3, 118.6), (244.6, 175.2)],
            [(224.9, 193.2), (242.3, 203.3), (250.2, 181.2)],
        ],
    ),
];

// eyes/Itachi.svg: one three-bladed path.
const ITACHI_TOMOE: [Path; 1] = [(
    (177.6, 10.7),
    &[
        [(135.0, 68.4), (155.4, 100.7), (179.8, 118.5)],
        [(260.9, 160.6), (274.8, 214.5), (255.9, 244.9)],
        [(237.3, 191.9), (198.0, 172.4), (158.5, 194.9)],
        [(86.9, 238.6), (40.7, 231.2), (15.7, 196.6)],
        [(58.2, 203.1), (109.1, 193.5), (107.9, 128.3)],
        [(109.5, 97.6), (111.5, 16.6), (177.6, 10.7)],
    ],
)];

/// Iris: r = 145 filled with the radial gradient, stroked 10 wide in black.
const IRIS_R: f32 = 145.0;
const RIM_IN: f32 = 140.0;
const RIM_OUT: f32 = 150.0;
/// Gradient stops (#660000, #c30000, #a00000) at 0, 0.5 and 1 of the radius.
const STOPS: [(f32, [f32; 3]); 3] = [
    (0.0, [102.0, 0.0, 0.0]),
    (0.5, [195.0, 0.0, 0.0]),
    (1.0, [160.0, 0.0, 0.0]),
];
const CENTRE: [f32; 3] = [160.0, 0.0, 0.0];

fn centre_radius(eye: Eye) -> f32 {
    match eye {
        Eye::Kakashi => 28.0,
        Eye::Itachi => 20.0,
    }
}

/// The eye's black pattern as polygons (one per path), in the 300-unit space.
pub fn outlines(eye: Eye) -> Vec<Vec<Pt>> {
    let (paths, turns): (&[Path], &[f32]) = match eye {
        Eye::Kakashi => (&KAKASHI_TOMOE, &[0.0, 120.0, -120.0]),
        Eye::Itachi => (&ITACHI_TOMOE, &[0.0]),
    };
    // Fine enough that no segment is visible even at 1024 px.
    const STEPS: usize = 64;
    let mut out = Vec::new();
    for &deg in turns {
        let (s, c) = deg.to_radians().sin_cos();
        // SVG rotate(deg 150 150), y pointing down.
        let rot = |(x, y): Pt| {
            let (dx, dy) = (x - 150.0, y - 150.0);
            (150.0 + dx * c - dy * s, 150.0 + dx * s + dy * c)
        };
        for &(start, segs) in paths {
            let mut poly = vec![rot(start)];
            let mut p0 = start;
            for &[p1, p2, p3] in segs {
                for i in 1..=STEPS {
                    let t = i as f32 / STEPS as f32;
                    let u = 1.0 - t;
                    let (a, b, cc, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                    poly.push(rot((
                        a * p0.0 + b * p1.0 + cc * p2.0 + d * p3.0,
                        a * p0.1 + b * p1.1 + cc * p2.1 + d * p3.1,
                    )));
                }
                p0 = p3;
            }
            out.push(poly);
        }
    }
    out
}

/// Nonzero fill of `polys` into a `side` x `side` mask over the 300-unit square.
fn fill_mask(polys: &[Vec<Pt>], side: usize) -> Vec<bool> {
    let scale = side as f32 / 300.0;
    let mut mask = vec![false; side * side];
    let mut xs: Vec<(f32, i32)> = Vec::new();
    for row in 0..side {
        let y = (row as f32 + 0.5) / scale;
        for poly in polys {
            xs.clear();
            for k in 0..poly.len() {
                let (a, b) = (poly[k], poly[(k + 1) % poly.len()]);
                if (a.1 <= y) != (b.1 <= y) {
                    let x = a.0 + (y - a.1) / (b.1 - a.1) * (b.0 - a.0);
                    xs.push((x, if b.1 > a.1 { 1 } else { -1 }));
                }
            }
            xs.sort_by(|p, q| p.0.total_cmp(&q.0));
            let mut winding = 0;
            for w in xs.windows(2) {
                winding += w[0].1;
                if winding != 0 {
                    let x0 = (w[0].0 * scale - 0.5).ceil().max(0.0) as usize;
                    let x1 = (w[1].0 * scale - 0.5).floor().min(side as f32 - 1.0);
                    if x1 >= 0.0 {
                        for x in x0..=x1 as usize {
                            mask[row * side + x] = true;
                        }
                    }
                }
            }
        }
    }
    mask
}

fn gradient(t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    let (lo, hi) = if t <= STOPS[1].0 {
        (STOPS[0], STOPS[1])
    } else {
        (STOPS[1], STOPS[2])
    };
    let k = (t - lo.0) / (hi.0 - lo.0);
    [0, 1, 2].map(|i| lo.1[i] + (hi.1[i] - lo.1[i]) * k)
}

/// An eye as `n` x `n` RGBA (straight alpha), transparent outside the iris.
pub fn eye_rgba(eye: Eye, n: usize) -> Vec<u8> {
    let n = n.max(1);
    // Samples per pixel side: 16 (256 per pixel) for icons, at least 4 for big images.
    let ss = (2048 / n).clamp(4, 16);
    let side = n * ss;
    let scale = side as f32 / 300.0;
    let mask = fill_mask(&outlines(eye), side);
    let centre_r = centre_radius(eye);
    let mut out = vec![0u8; n * n * 4];
    for py in 0..n {
        for px in 0..n {
            let (mut sum, mut cover) = ([0.0f32; 3], 0.0f32);
            for sy in 0..ss {
                for sx in 0..ss {
                    let (mx, my) = (px * ss + sx, py * ss + sy);
                    let (x, y) = ((mx as f32 + 0.5) / scale, (my as f32 + 0.5) / scale);
                    let r = ((x - 150.0).powi(2) + (y - 150.0).powi(2)).sqrt();
                    // Painted in SVG order: iris, rim, pattern, centre.
                    let colour = if r < centre_r {
                        CENTRE
                    } else if mask[my * side + mx] || (RIM_IN..RIM_OUT).contains(&r) {
                        [0.0; 3]
                    } else if r < IRIS_R {
                        gradient(r / IRIS_R)
                    } else {
                        continue;
                    };
                    cover += 1.0;
                    for i in 0..3 {
                        sum[i] += colour[i];
                    }
                }
            }
            if cover > 0.0 {
                let i = (py * n + px) * 4;
                for c in 0..3 {
                    out[i + c] = (sum[c] / cover).round() as u8;
                }
                out[i + 3] = (cover / (ss * ss) as f32 * 255.0).round() as u8;
            }
        }
    }
    out
}

/// The application icon: Kakashi's Mangekyou Sharingan (used by build.rs).
#[allow(dead_code)]
pub fn icon_rgba(n: usize) -> Vec<u8> {
    eye_rgba(Eye::Kakashi, n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_closed_and_on_the_canvas() {
        for eye in [Eye::Kakashi, Eye::Itachi] {
            let polys = outlines(eye);
            assert_eq!(polys.len(), if eye == Eye::Kakashi { 6 } else { 1 });
            for poly in &polys {
                let (a, b) = (poly[0], poly[poly.len() - 1]);
                assert!((a.0 - b.0).abs() < 0.01 && (a.1 - b.1).abs() < 0.01);
                for p in poly {
                    assert!((-0.5..=300.5).contains(&p.0) && (-0.5..=300.5).contains(&p.1));
                }
            }
        }
    }

    #[test]
    fn eyes_render() {
        let n = 300;
        for eye in [Eye::Kakashi, Eye::Itachi] {
            let img = eye_rgba(eye, n);
            if let Some(dir) = std::env::var_os("BDF_ICON_PREVIEW") {
                let dir = std::path::PathBuf::from(dir);
                for size in [300, 96, 28, 16] {
                    let name = format!("{eye:?}-{size}.rgba");
                    std::fs::write(dir.join(name), eye_rgba(eye, size)).unwrap();
                }
            }
            let px = |x: usize, y: usize| {
                let i = (y * n + x) * 4;
                [img[i], img[i + 1], img[i + 2], img[i + 3]]
            };
            assert_eq!(px(2, 2)[3], 0, "outside is transparent");
            assert_eq!(px(150, 3)[..3], [0, 0, 0], "black rim");
            assert_eq!(px(150, 150), [160, 0, 0, 255], "red centre");
            // Some iris shows between the blades, and the blades are black.
            let reds = img
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|p| p[0] > 120 && p[3] == 255)
                .count();
            let blacks = img
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|p| p[0] == 0 && p[3] == 255)
                .count();
            assert!(
                reds > 10_000 && blacks > 10_000,
                "{eye:?}: {reds} red, {blacks} black"
            );
            // Anti-aliased: the rim's outer edge has partly covered pixels.
            assert!(
                img.as_chunks::<4>()
                    .0
                    .iter()
                    .any(|p| p[3] > 20 && p[3] < 235)
            );
        }
    }
}
