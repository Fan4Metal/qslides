//! Procedurally drawn application icon. Dependency-free so that `build.rs`
//! can include this file (`#[path = "src/icon.rs"]`) to produce the `.ico`
//! embedded in the executable and the raw RGBA the app shows (window icon,
//! start page, About), so that nothing is rasterised at start-up.
//!
//! Replace `sample` with the app's own drawing; keep `rgba`, `ico` and
//! `ICO_SIZES`, which build.rs, `--export-icon` and `ui::app_icon` rely on.

// build.rs and the app use different parts.
#![allow(dead_code)]

/// Sizes in the `.ico` of the executable.
pub const ICO_SIZES: [u32; 8] = [16, 20, 24, 32, 40, 48, 64, 256];

/// Straight RGBA, channels 0..1.
type Rgba = [f32; 4];

fn hex(c: u32) -> Rgba {
    let ch = |s: u32| ((c >> s) & 0xff) as f32 / 255.0;
    [ch(16), ch(8), ch(0), 1.0]
}

fn with_alpha(c: Rgba, a: f32) -> Rgba {
    [c[0], c[1], c[2], c[3] * a.clamp(0.0, 1.0)]
}

/// Paint `top` over `under`.
fn over(under: Rgba, top: Rgba) -> Rgba {
    let a = top[3] + under[3] * (1.0 - top[3]);
    if a <= 0.0 {
        return [0.0; 4];
    }
    let mut out = [0.0, 0.0, 0.0, a];
    for i in 0..3 {
        out[i] = (top[i] * top[3] + under[i] * under[3] * (1.0 - top[3])) / a;
    }
    out
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Signed distance from `(x, y)` to a box from `min` to `max` with corners
/// rounded by `r` (negative inside).
fn rounded_box(x: f32, y: f32, min: (f32, f32), max: (f32, f32), r: f32) -> f32 {
    let (cx, cy) = ((min.0 + max.0) / 2.0, (min.1 + max.1) / 2.0);
    let (hx, hy) = ((max.0 - min.0) / 2.0 - r, (max.1 - min.1) / 2.0 - r);
    let qx = (x - cx).abs() - hx;
    let qy = (y - cy).abs() - hy;
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - r
}

fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        a[3] + (b[3] - a[3]) * t,
    ]
}

fn circle(x: f32, y: f32, cx: f32, cy: f32, r: f32) -> f32 {
    ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - r
}

/// The colour of the icon at `(x, y)`, both in 0..1 of the icon's side;
/// `px` is the size of a pixel of the layer drawn, in the same units, the
/// width the edges are smoothed over. A photo frame on the desk: a wooden
/// frame, lit from above, round a landscape (sky, sun, two green hills),
/// with three dots of a slideshow on its lower edge (not in the small
/// layers, where they would be a smudge).
fn sample(x: f32, y: f32, px: f32) -> Rgba {
    let edge = |d: f32| smoothstep(px * 0.5, -px * 0.5, d);
    let mut c = [0.0; 4];
    let (outer_min, outer_max) = ((0.06, 0.13), (0.94, 0.87));
    // A soft shadow below.
    let shadow = rounded_box(x, y - 0.025, outer_min, outer_max, 0.09);
    c = over(
        c,
        with_alpha(hex(0x000000), 0.28 * smoothstep(0.05, -0.02, shadow)),
    );
    // The frame: warm wood, lighter at the top, a darker rim.
    let frame = rounded_box(x, y, outer_min, outer_max, 0.08);
    let wood = mix(
        hex(0xd9a066),
        hex(0x8f5628),
        (y - outer_min.1) / (outer_max.1 - outer_min.1),
    );
    c = over(c, with_alpha(mix(wood, hex(0x5c3215), 0.65), edge(frame)));
    c = over(c, with_alpha(wood, edge(frame + px.max(0.012))));
    // The picture.
    let (pic_min, pic_max) = ((0.17, 0.23), (0.83, 0.74));
    let picture = rounded_box(x, y, pic_min, pic_max, 0.02);
    let inside = edge(picture);
    if inside > 0.0 {
        let t = (y - pic_min.1) / (pic_max.1 - pic_min.1);
        let mut p = mix(hex(0x3d9be9), hex(0xbfe6ff), t);
        p = over(
            p,
            with_alpha(hex(0xffd23f), edge(circle(x, y, 0.64, 0.38, 0.075))),
        );
        p = over(
            p,
            with_alpha(hex(0x66bb4a), edge(circle(x, y, 0.32, 1.02, 0.40))),
        );
        p = over(
            p,
            with_alpha(hex(0x2f8a3a), edge(circle(x, y, 0.80, 1.10, 0.43))),
        );
        c = over(c, with_alpha(p, inside));
        // A thin dark line round the picture, where it sits in the frame.
        let rim = edge(picture.abs() - px.max(0.006) * 0.5) * 0.45;
        c = over(c, with_alpha(hex(0x3a2210), rim));
    }
    // The slideshow's dots, the first one lit.
    if px < 1.0 / 40.0 {
        for (i, dx) in [-0.06f32, 0.0, 0.06].into_iter().enumerate() {
            let dot = edge(circle(x, y, 0.5 + dx, 0.805, 0.019));
            let colour = if i == 0 { hex(0xfff4e0) } else { hex(0x6b3d1b) };
            c = over(c, with_alpha(colour, dot));
        }
    }
    c
}

/// Straight (non-premultiplied) RGBA pixels, `size * size * 4` bytes, row
/// major from the top. 4x4 supersampling gives smooth edges.
pub fn rgba(size: u32) -> Vec<u8> {
    const SS: u32 = 4;
    let n = size as usize;
    let mut out = vec![0u8; n * n * 4];
    for py in 0..size {
        for px in 0..size {
            // Averaged premultiplied, so transparent samples add no colour.
            let mut acc = [0.0f32; 4];
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = (px as f32 + (sx as f32 + 0.5) / SS as f32) / size as f32;
                    let y = (py as f32 + (sy as f32 + 0.5) / SS as f32) / size as f32;
                    let c = sample(x, y, 1.0 / size as f32);
                    for i in 0..3 {
                        acc[i] += c[i] * c[3];
                    }
                    acc[3] += c[3];
                }
            }
            let count = (SS * SS) as f32;
            let alpha = acc[3] / count;
            let i = (py as usize * n + px as usize) * 4;
            if alpha > 0.0 {
                for ch in 0..3 {
                    out[i + ch] = (acc[ch] / acc[3] * 255.0).round().clamp(0.0, 255.0) as u8;
                }
                out[i + 3] = (alpha * 255.0).round() as u8;
            }
        }
    }
    out
}

/// Encode the icon as a multi-resolution `.ico` (32-bit BMP entries).
/// qview stores the 256 px layer as PNG through the `png` crate instead:
/// ~50 KB an icon instead of ~306 KB.
pub fn ico(sizes: &[u32]) -> Vec<u8> {
    let images: Vec<Vec<u8>> = sizes.iter().map(|&s| bmp_entry(s)).collect();
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]); // reserved, type = icon
    out.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (&s, img) in sizes.iter().zip(&images) {
        let dim = if s >= 256 { 0 } else { s as u8 };
        out.extend_from_slice(&[dim, dim, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bpp
        out.extend_from_slice(&(img.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += img.len() as u32;
    }
    for img in images {
        out.extend_from_slice(&img);
    }
    out
}

/// BITMAPINFOHEADER + bottom-up BGRA pixels + 1-bpp AND mask.
fn bmp_entry(size: u32) -> Vec<u8> {
    let px = rgba(size);
    let n = size as usize;
    let mask_stride = n.div_ceil(32) * 4;
    let mut out = Vec::with_capacity(40 + n * n * 4 + mask_stride * n);
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(size as i32).to_le_bytes());
    out.extend_from_slice(&(2 * size as i32).to_le_bytes()); // XOR + AND
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&[0u8; 24]); // compression .. colours important
    for y in (0..n).rev() {
        for x in 0..n {
            let i = (y * n + x) * 4;
            out.extend_from_slice(&[px[i + 2], px[i + 1], px[i], px[i + 3]]);
        }
    }
    // Alpha channel carries transparency; an all-zero AND mask is correct.
    out.resize(out.len() + mask_stride * n, 0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_shape() {
        let px = rgba(64);
        assert_eq!(px.len(), 64 * 64 * 4);
        // The corner is transparent, the centre opaque.
        assert_eq!(px[3], 0);
        let c = ((32 * 64) + 32) * 4;
        assert_eq!(px[c + 3], 255);
        let ico = ico(&[16, 32]);
        assert_eq!(&ico[..6], &[0, 0, 1, 0, 2, 0]);
    }
}
