// The BDF icon, drawn procedurally at any size: a white page with a folded
// corner and three text lines on a rounded accent square. Shared by the
// window icon (theme.rs) and the Windows .exe icon (build.rs).

/// RGBA pixels of an `n` x `n` icon.
pub fn icon_rgba(n: usize) -> Vec<u8> {
    let k = n as f32 / 64.0; // designed on a 64 px grid
    let mut rgba = vec![0u8; n * n * 4];
    let accent = [37u8, 99, 235];
    for y in 0..n {
        for x in 0..n {
            let (fx, fy) = ((x as f32 + 0.5) / k, (y as f32 + 0.5) / k);
            let r = 14.0;
            let cx = fx.clamp(r, 64.0 - r);
            let cy = fy.clamp(r, 64.0 - r);
            let d = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
            let alpha = ((r - d) * k + 0.5).clamp(0.0, 1.0);
            let mut c = accent;
            let in_page = (18.0..46.0).contains(&fx) && (12.0..52.0).contains(&fy);
            let fold = fx - 36.0 > fy - 12.0 && fx > 36.0 && fy < 22.0;
            if in_page && !fold {
                c = [255, 255, 255];
                for ly in [28.0, 35.0, 42.0] {
                    if (ly..ly + 2.5).contains(&fy) && (23.0..41.0).contains(&fx) {
                        c = [148, 170, 220];
                    }
                }
            }
            let i = (y * n + x) * 4;
            rgba[i..i + 3].copy_from_slice(&c);
            rgba[i + 3] = (alpha * 255.0) as u8;
        }
    }
    rgba
}
