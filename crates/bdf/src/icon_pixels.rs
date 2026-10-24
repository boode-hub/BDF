// BDF's logo: the Mangekyou Sharingan of Kakashi (default) or Itachi, drawn
// from the traced "Classic" outlines in BOoDe007/Sharingan-Monitor
// (Sharingan.ps1, $global:ClassicShapes; MIT licence), so the eyes match that
// app exactly. Shared by the window and in-app images (theme.rs) and the
// Windows .exe icon (build.rs).
//
// Format (same as Sharingan-Monitor's Get-ClassicArt): base64 of a list of
// outlines, each a big-endian u16 point count followed by one byte for x and
// one for y, mapped to 10 + b * 280 / 255 in a 300-unit square. Outlines are
// filled black with the even-odd rule, so holes stay red. The iris is a solid
// red disc with a black rim, as in the Classic set.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Eye {
    Kakashi,
    Itachi,
}

const KAKASHI: &str = "AEGAAIUAhwCQBZ0LqRSyG7skwSzIN89C1E3ZWtxq3XTegd2Q253nq/C28rztxerG4szX0s3Yxtu73rLhpOOW5IfjdeBl21vXU9NLzkTKQMY5wCPEFMUSxQ28DroNrAyfDZUPixCBFHYZax9fJlQuSzdCPzpIM1UrXyZrI3EQdAp4AHoAABRoKlsuUzNMOEk6QUM7SzhRNFgxYTBmLm8ueC6AL4gxjkZ7W2tgTGM9ABiaNZ5FoFKka7B0vX/HidaW15DYidh+13fWctNp0WLNWsdSwky8RrZCrzyoOKA2mjQAIH5meGdzam9ra29qc2d4Zn5mgWeIaoxskXCUc5Z4mH2ZgpmHmIyWkJOTkJaMmIeZgpl9mHeVc5NukGuMaodngWYAFn+qcbBct0G+RMJNyFTMW9Bi0mrUcNV31YDVh9OP0ZjPoMuoxq/As7yXs4Cq";
const ITACHI: &str = "AE6YAZkBkguNE4cghCeCMII4g0CFSIhOi1SPWJRem2KmabRywn3Hgc+J1ZDbmd6e46jmsOa55sHmx+PR4NbbytbB0rvLssetvae2o6ygqKCgoJuhlKOMpneyZbpWv0jDOMQswyXBH78ZvBS5DLIFqhqrJ6oxqDqmQqFJnE6VUZJUjFeAWXZaXVxKYDhjLmYnaiBtGnITeQ2ACIcFkAIAHn5tem52cHVxcXVwdW56bX5tgW6FcIlzjHWOepF+koGShZGJj4yNjoqRhZKBkn6Re492jHOKcYlvhW6BbQ==";

/// Classic iris colour, rim and radius (300-unit space), from Get-SharinganIcon:
/// a 116 px disc with a 4.5 px border on a 128 px canvas scaled by 0.401.
const IRIS: [u8; 3] = [195, 0, 0];
const IRIS_R: f32 = 58.0 / 0.401;
const RIM: f32 = 4.5 / 0.401;

fn base64(s: &str) -> Vec<u8> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes().filter_map(val) {
        acc = (acc << 6) | c;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    out
}

/// The traced outlines of an eye, in the 300-unit space.
pub fn outlines(eye: Eye) -> Vec<Vec<(f32, f32)>> {
    let b = base64(match eye {
        Eye::Kakashi => KAKASHI,
        Eye::Itachi => ITACHI,
    });
    let mut out = Vec::new();
    let mut i = 0;
    while i + 2 <= b.len() {
        let n = (b[i] as usize) << 8 | b[i + 1] as usize;
        i += 2;
        if i + 2 * n > b.len() {
            break;
        }
        let map = |v: u8| 10.0 + v as f32 * 280.0 / 255.0;
        out.push(
            (0..n)
                .map(|j| (map(b[i + 2 * j]), map(b[i + 2 * j + 1])))
                .collect(),
        );
        i += 2 * n;
    }
    out
}

/// Even-odd fill of `polys` into a `side` x `side` mask over the 300-unit square.
fn fill_mask(polys: &[Vec<(f32, f32)>], side: usize) -> Vec<bool> {
    let scale = side as f32 / 300.0;
    let mut mask = vec![false; side * side];
    let mut xs: Vec<f32> = Vec::new();
    for row in 0..side {
        let y = (row as f32 + 0.5) / scale;
        xs.clear();
        for poly in polys {
            for k in 0..poly.len() {
                let (a, b) = (poly[k], poly[(k + 1) % poly.len()]);
                if (a.1 <= y) != (b.1 <= y) {
                    xs.push(a.0 + (y - a.1) / (b.1 - a.1) * (b.0 - a.0));
                }
            }
        }
        xs.sort_by(f32::total_cmp);
        for pair in xs.chunks_exact(2) {
            let x0 = (pair[0] * scale - 0.5).ceil().max(0.0) as usize;
            let x1 = (pair[1] * scale - 0.5).floor().min(side as f32 - 1.0);
            if x1 >= 0.0 {
                for x in x0..=x1 as usize {
                    mask[row * side + x] = true;
                }
            }
        }
    }
    mask
}

/// An eye as `n` x `n` RGBA, transparent outside the iris.
pub fn eye_rgba(eye: Eye, n: usize) -> Vec<u8> {
    let ss = if n <= 48 { 6 } else { 3 };
    let side = n * ss;
    let scale = side as f32 / 300.0;
    let mask = fill_mask(&outlines(eye), side);
    let (outer, inner) = (IRIS_R + RIM / 2.0, IRIS_R - RIM / 2.0);
    let mut out = vec![0u8; n * n * 4];
    for py in 0..n {
        for px in 0..n {
            let (mut red, mut cover) = (0.0f32, 0.0f32);
            for sy in 0..ss {
                for sx in 0..ss {
                    let (mx, my) = (px * ss + sx, py * ss + sy);
                    let (x, y) = (
                        (mx as f32 + 0.5) / scale - 150.0,
                        (my as f32 + 0.5) / scale - 150.0,
                    );
                    let d = (x * x + y * y).sqrt();
                    if d > outer {
                        continue;
                    }
                    cover += 1.0;
                    if d < inner && !mask[my * side + mx] {
                        red += 1.0;
                    }
                }
            }
            if cover > 0.0 {
                let i = (py * n + px) * 4;
                let k = red / cover;
                out[i] = (IRIS[0] as f32 * k).round() as u8;
                out[i + 1] = (IRIS[1] as f32 * k).round() as u8;
                out[i + 2] = (IRIS[2] as f32 * k).round() as u8;
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
    fn traced_data_decodes() {
        // Outline point counts, as Sharingan-Monitor's data defines them.
        let k: Vec<usize> = outlines(Eye::Kakashi).iter().map(Vec::len).collect();
        assert_eq!(k, vec![65, 20, 24, 32, 22]);
        let i: Vec<usize> = outlines(Eye::Itachi).iter().map(Vec::len).collect();
        assert_eq!(i, vec![78, 30]);
        for p in outlines(Eye::Kakashi).iter().flatten() {
            assert!((10.0..=290.0).contains(&p.0) && (10.0..=290.0).contains(&p.1));
        }
    }

    #[test]
    fn eyes_render() {
        let n = 300;
        for eye in [Eye::Kakashi, Eye::Itachi] {
            let img = eye_rgba(eye, n);
            if let Some(dir) = std::env::var_os("BDF_ICON_PREVIEW") {
                let dir = std::path::PathBuf::from(dir);
                std::fs::write(dir.join(format!("{eye:?}-300.rgba")), &img).unwrap();
                std::fs::write(dir.join(format!("{eye:?}-32.rgba")), eye_rgba(eye, 32)).unwrap();
            }
            let px = |x: usize, y: usize| {
                let i = (y * n + x) * 4;
                [img[i], img[i + 1], img[i + 2], img[i + 3]]
            };
            assert_eq!(px(2, 2)[3], 0, "outside is transparent");
            assert_eq!(px(150, 3)[..3], [0, 0, 0], "black rim");
            let reds = img
                .chunks_exact(4)
                .filter(|p| p[0] == 195 && p[3] == 255)
                .count();
            let blacks = img
                .chunks_exact(4)
                .filter(|p| p[0] == 0 && p[3] == 255)
                .count();
            assert!(
                reds > 10_000 && blacks > 10_000,
                "{eye:?}: {reds} red, {blacks} black"
            );
        }
    }
}
