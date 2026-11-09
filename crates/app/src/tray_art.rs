// SPDX-License-Identifier: AGPL-3.0-only
//! The tray icon, drawn in code (no image files): a white rounded square
//! with black record grooves, matching the app icon.

/// Icon size in pixels.
pub const SIZE: u32 = 32;

/// RGBA pixels, row by row.
pub fn rgba() -> Vec<u8> {
    let n = SIZE;
    let mut px = Vec::with_capacity((n * n * 4) as usize);
    let c = (n as f32 - 1.0) / 2.0;
    for y in 0..n {
        for x in 0..n {
            let (dx, dy) = (x as f32 - c, y as f32 - c);
            let r = (dx * dx + dy * dy).sqrt();
            // Rounded square mask.
            let (ax, ay) = (dx.abs() - 10.0, dy.abs() - 10.0);
            let outside = (ax.max(0.0).powi(2) + ay.max(0.0).powi(2)).sqrt() - 5.5;
            let alpha = (0.5 - outside).clamp(0.0, 1.0);
            let groove = ((r - 9.5).abs() < 1.0) || ((r - 5.5).abs() < 1.0) || r < 1.8;
            let v = if groove { 0u8 } else { 250u8 };
            px.extend_from_slice(&[v, v, v, (alpha * 255.0) as u8]);
        }
    }
    px
}

/// Same pixels as ARGB32 in network byte order (StatusNotifierItem).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn argb() -> Vec<u8> {
    rgba()
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&[r, g, b, a]| [a, r, g, b])
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn sizes_and_order() {
        let n = (super::SIZE * super::SIZE * 4) as usize;
        let (rgba, argb) = (super::rgba(), super::argb());
        assert_eq!((rgba.len(), argb.len()), (n, n));
        // Centre pixel is a black groove dot, fully opaque.
        let i = ((16 * super::SIZE + 16) * 4) as usize;
        assert_eq!(&rgba[i..i + 4], &[0, 0, 0, 255]);
        assert_eq!(&argb[i..i + 4], &[255, 0, 0, 0]);
        // Corner is transparent.
        assert_eq!(rgba[3], 0);
        assert_eq!(argb[0], 0);
    }
}
