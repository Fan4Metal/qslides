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

/// The frame's outer edge.
const FRAME_MIN: (f32, f32) = (0.06, 0.12);
const FRAME_MAX: (f32, f32) = (0.94, 0.88);
const FRAME_R: f32 = 0.07;
/// The width of the slim profile.
const PROFILE: f32 = 0.055;
/// The picture inside the mat.
const PIC_MIN: (f32, f32) = (0.205, 0.265);
const PIC_MAX: (f32, f32) = (0.795, 0.735);

/// The colour of the icon at `(x, y)`, both in 0..1 of the icon's side;
/// `px` is the size of a pixel of the layer drawn, in the same units, the
/// width the edges are smoothed over. A modern photo frame: a slim graphite
/// profile with rounded corners, lit along its top edge and darker at its
/// foot, a wide white mat with a fine bevel, and a bright flat landscape
/// (a sky from blue to warm, a sun, layered hills, a lake's band) under a
/// soft glint of glass. Below 40 px the mat narrows and the details go;
/// the profile keeps at least a pixel and a half, so the frame still reads
/// on a dark taskbar.
fn sample(x: f32, y: f32, px: f32) -> Rgba {
    let edge = |d: f32| smoothstep(px * 0.5, -px * 0.5, d);
    let small = px > 1.0 / 40.0;
    let mut c = [0.0; 4];

    // A soft shadow under the frame.
    let shadow = rounded_box(x, y - 0.03, FRAME_MIN, FRAME_MAX, FRAME_R);
    c = over(
        c,
        with_alpha(hex(0x000000), 0.30 * smoothstep(0.06, -0.03, shadow)),
    );

    // The profile: graphite, lighter at the top, a bright line along its
    // outer top edge and a dark one at its inner edge.
    let d = rounded_box(x, y, FRAME_MIN, FRAME_MAX, FRAME_R);
    let width = PROFILE.max(px * 1.5);
    let inner = d + width;
    let t = ((y - FRAME_MIN.1) / (FRAME_MAX.1 - FRAME_MIN.1)).clamp(0.0, 1.0);
    let graphite = mix(hex(0x4a515c), hex(0x1f2329), t);
    c = over(c, with_alpha(graphite, edge(d)));
    let line = px.max(0.008);
    let top_light = smoothstep(0.55, 0.0, t) * (1.0 - smoothstep(0.0, line * 1.6, -d));
    c = over(c, with_alpha(hex(0xc9d1db), 0.75 * top_light * edge(d)));
    // The face of the profile, a satin band in its middle.
    let across = (-d / width).clamp(0.0, 1.0);
    let satin = (1.0 - ((across - 0.45) / 0.3).powi(2)).max(0.0) * 0.12 * (1.0 - t);
    c = over(
        c,
        with_alpha(hex(0xffffff), satin * edge(d) * (1.0 - edge(inner))),
    );
    c = over(
        c,
        with_alpha(
            hex(0x0d0f12),
            0.8 * edge(inner) * (1.0 - edge(inner + line)),
        ),
    );

    // The mat: white, shaded just under the profile at the top.
    let (pic_min, pic_max) = if small {
        // Narrower: a white line of a pixel or so.
        let m = px * 1.2;
        let w = width + m;
        (
            (FRAME_MIN.0 + w, FRAME_MIN.1 + w),
            (FRAME_MAX.0 - w, FRAME_MAX.1 - w),
        )
    } else {
        (PIC_MIN, PIC_MAX)
    };
    let in_mat = edge(inner + line);
    if in_mat > 0.0 {
        let depth = -(inner + line);
        let shade = 1.0 - 0.10 * smoothstep(0.025, 0.0, depth) * (1.0 - t);
        let white = hex(0xf7f7f5);
        c = over(
            c,
            with_alpha(
                [white[0] * shade, white[1] * shade, white[2] * shade, 1.0],
                in_mat,
            ),
        );
    }

    // The picture, with a fine grey bevel line round it.
    let picture = rounded_box(x, y, pic_min, pic_max, 0.008);
    if !small {
        let bevel = edge(picture - 0.006) * (1.0 - edge(picture));
        c = over(c, with_alpha(hex(0xd9d6cf), bevel));
    }
    let in_picture = edge(picture);
    if in_picture > 0.0 {
        let v = ((y - pic_min.1) / (pic_max.1 - pic_min.1)).clamp(0.0, 1.0);
        let u = ((x - pic_min.0) / (pic_max.0 - pic_min.0)).clamp(0.0, 1.0);
        // Sky: deep blue at the top to a warm haze at the horizon.
        let mut p = if v < 0.62 {
            mix(hex(0x2b6fe0), hex(0x8fc8ff), v / 0.62)
        } else {
            mix(hex(0x8fc8ff), hex(0xffd9a8), (v - 0.62) / 0.12)
        };
        // The sun, low, with a glow.
        let (sx, sy) = (
            pic_min.0 + 0.68 * (pic_max.0 - pic_min.0),
            pic_min.1 + 0.42 * (pic_max.1 - pic_min.1),
        );
        let sr = 0.11 * (pic_max.0 - pic_min.0);
        let sun = circle(x, y, sx, sy, sr);
        p = over(
            p,
            with_alpha(hex(0xfff1c4), 0.45 * smoothstep(sr * 1.4, 0.0, sun)),
        );
        p = over(p, with_alpha(hex(0xffc93c), edge(sun)));
        // Hills: far ones blue-green, near ones bright green.
        let h = pic_max.1 - pic_min.1;
        let far = pic_min.1 + h * (0.62 - 0.08 * (u * 5.0).sin() * (1.0 - u * 0.4));
        p = over(p, with_alpha(hex(0x3f9a8c), edge(far - y)));
        let near = pic_min.1 + h * (0.74 - 0.10 * ((u - 0.15) * 3.2).cos());
        p = over(p, with_alpha(hex(0x4cbf5a), edge(near - y)));
        let front = pic_min.1 + h * (0.86 + 0.06 * ((u - 0.8) * 3.0).cos());
        p = over(p, with_alpha(hex(0x2f9446), edge(front - y)));
        if !small {
            // A glint of the glass across the top left.
            let s = u + v;
            let glint = smoothstep(0.30, 0.36, s) * (1.0 - smoothstep(0.46, 0.56, s));
            p = over(p, with_alpha(hex(0xffffff), 0.14 * glint));
        }
        c = over(c, with_alpha(p, in_picture));
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
