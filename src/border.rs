//! The picture frame painted round the window's edges, a style and a width
//! of the user's choice. The slide is placed in the opening inside it.
//!
//! A frame is a moulding seen from the front, lit from the top left: four
//! rails mitred at the corners, each a strip of bands across its width
//! whose colours (`stops`) give its profile (a light edge, a lip, the
//! shadow of the rebate), the top rail lightest and the bottom darkest
//! (`SIDE_SHADE`). The colours are blended between bands by the mesh, so a
//! frame costs a few dozen triangles and no texture. The mat (`Mat`) is a
//! thin moulding with a wide flat board inside, which also fills the bars
//! round a picture fitted whole, and a white bevel round the picture.

use egui::{Color32, Mesh, Painter, Pos2, Rect, Shape, pos2};

/// The style of the frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Border {
    /// No frame: the slide over the whole window.
    #[default]
    None,
    Black,
    White,
    /// Dark grey metal, as the frame of the program's icon.
    Graphite,
    /// Walnut, its grain along the rails.
    Wood,
    /// A thin dark moulding round a wide white mat.
    Mat,
}

/// The widths the slider sets, from 1 to this.
pub const MAX_WIDTH: u32 = 10;
/// The width of each step of the slider, a share of the window's shorter
/// side.
const WIDTH_STEP: f32 = 0.015;
/// The colour of the mat.
const MAT: Color32 = Color32::from_rgb(240, 238, 232);
/// The share of a mat's width its outer moulding takes.
const MAT_MOULDING: f32 = 0.22;
/// How much light each rail gets: top, right, bottom, left.
const SIDE_SHADE: [f32; 4] = [1.12, 0.86, 0.72, 1.0];

impl Border {
    pub const ALL: [Border; 6] = [
        Border::None,
        Border::Black,
        Border::White,
        Border::Graphite,
        Border::Wood,
        Border::Mat,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Border::None => tr!("None", "Нет"),
            Border::Black => tr!("Black", "Чёрная"),
            Border::White => tr!("White", "Белая"),
            Border::Graphite => tr!("Graphite", "Графит"),
            Border::Wood => tr!("Wood", "Дерево"),
            Border::Mat => tr!("Mat", "Паспарту"),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Border::None => "none",
            Border::Black => "black",
            Border::White => "white",
            Border::Graphite => "graphite",
            Border::Wood => "wood",
            Border::Mat => "mat",
        }
    }

    pub fn from_name(name: &str) -> Option<Border> {
        Border::ALL.into_iter().find(|b| b.name() == name)
    }

    /// The frame's width in points in a window of `area`, at the slider's
    /// `step` (1 to `MAX_WIDTH`); 0 without a frame.
    pub fn width(self, area: Rect, step: u32) -> f32 {
        if self == Border::None {
            return 0.0;
        }
        let short = area.width().min(area.height());
        (short * WIDTH_STEP * step.clamp(1, MAX_WIDTH) as f32)
            .max(3.0)
            .round()
    }

    /// The opening inside the frame, where the slide goes.
    pub fn opening(self, area: Rect, step: u32) -> Rect {
        let opening = area.shrink(self.width(area, step));
        if opening.is_positive() { opening } else { area }
    }

    /// The colour round a picture fitted whole in the opening: the mat's,
    /// or the window's black.
    pub fn ground(self) -> Color32 {
        match self {
            Border::Mat => MAT,
            _ => Color32::BLACK,
        }
    }

    /// The frame round `area` at the slider's `step`, over the slide;
    /// `picture` is where the slide's picture is, for the mat's bevel.
    pub fn paint(self, painter: &Painter, area: Rect, step: u32, picture: Option<Rect>) {
        let width = self.width(area, step);
        let opening = self.opening(area, step);
        if width <= 0.0 || opening == area {
            return;
        }
        let mut mesh = Mesh::default();
        let stops = stops(self);
        ring(
            &mut mesh,
            area,
            stops.iter().map(|&(t, color, shaded)| {
                (
                    t * width,
                    sides(|i| {
                        if shaded {
                            shade(color, SIDE_SHADE[i])
                        } else {
                            color
                        }
                    }),
                )
            }),
        );
        // The frame's shadow on what it holds, deeper under the top and left
        // rails, in from the edge a quarter of the frame's width.
        let (inner, depth) = match self {
            Border::Mat => (
                area.shrink(width * MAT_MOULDING),
                width * (1.0 - MAT_MOULDING) * 0.2,
            ),
            _ => (opening, width * 0.25),
        };
        let strength = [110.0, 60.0, 50.0, 100.0];
        ring(
            &mut mesh,
            inner,
            [
                (0.0, sides(|i| Color32::from_black_alpha(strength[i] as u8))),
                (depth.max(1.0), sides(|_| Color32::TRANSPARENT)),
            ],
        );
        if self == Border::Mat
            && let Some(picture) = picture.map(|p| p.intersect(opening))
            && picture.is_positive()
        {
            // The mat's bevel: a white edge cut round the picture.
            let bevel = (width * 0.04).clamp(1.5, 4.0);
            let white = Color32::from_rgb(252, 251, 248);
            let edge = Color32::from_rgb(222, 219, 212);
            ring(
                &mut mesh,
                picture.expand(bevel),
                [
                    (0.0, sides(|i| shade(edge, SIDE_SHADE[i]))),
                    (bevel * 0.4, sides(|i| shade(white, SIDE_SHADE[i]))),
                    (bevel, sides(|i| shade(white, SIDE_SHADE[i] * 0.97))),
                ],
            );
        }
        painter.add(Shape::mesh(mesh));
    }
}

/// The same value for the four sides, or one each: top, right, bottom,
/// left.
fn sides(f: impl Fn(usize) -> Color32) -> [Color32; 4] {
    [f(0), f(1), f(2), f(3)]
}

/// `color` lighter or darker by `factor`, its alpha kept.
fn shade(color: Color32, factor: f32) -> Color32 {
    let c = |v: u8| (v as f32 * factor).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgba_premultiplied(c(color.r()), c(color.g()), c(color.b()), color.a())
}

fn grey(v: u8) -> Color32 {
    Color32::from_rgb(v, v, v)
}

/// The profile of a style's moulding: from its outer edge (0) to the
/// opening (1), the colours at those places, and whether the rail's side
/// shades it (a mat is flat and lit alike).
fn stops(style: Border) -> Vec<(f32, Color32, bool)> {
    let shaded = |list: &[(f32, Color32)]| list.iter().map(|&(t, c)| (t, c, true)).collect();
    match style {
        Border::None => Vec::new(),
        Border::Black => shaded(&[
            (0.0, grey(62)),
            (0.05, grey(38)),
            (0.15, grey(27)),
            (0.7, grey(22)),
            (0.84, grey(36)),
            (0.91, grey(58)),
            (0.96, grey(18)),
            (1.0, grey(8)),
        ]),
        Border::White => shaded(&[
            (0.0, grey(190)),
            (0.07, grey(246)),
            (0.4, grey(236)),
            (0.75, grey(224)),
            (0.88, grey(247)),
            (0.95, grey(200)),
            (1.0, grey(150)),
        ]),
        Border::Graphite => {
            let g = |v: u8| Color32::from_rgb(v, v.saturating_add(2), v.saturating_add(6));
            shaded(&[
                (0.0, g(36)),
                (0.05, g(118)),
                (0.14, g(88)),
                (0.5, g(70)),
                (0.8, g(60)),
                (0.9, g(96)),
                (0.96, g(42)),
                (1.0, g(26)),
            ])
        }
        Border::Wood => {
            // The grain: bands of lighter and darker wood along the rails,
            // the same each time (a fixed seed), under the profile.
            let base = [128.0, 80.0, 46.0];
            let mut seed = 0x2545_f491_u32;
            let n = 18;
            (0..=n)
                .map(|k| {
                    let t = k as f32 / n as f32;
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    let grain = 0.8 + 0.28 * (seed % 1000) as f32 / 1000.0;
                    let profile = match t {
                        t if t < 0.06 => 0.62,
                        t if t > 0.95 => 0.5,
                        t if t > 0.84 => 1.22,
                        _ => 1.0,
                    };
                    let [r, g, b] = base.map(|v| (v * grain * profile).clamp(0.0, 255.0) as u8);
                    (t, Color32::from_rgb(r, g, b), true)
                })
                .collect()
        }
        Border::Mat => {
            let m = MAT_MOULDING;
            let mut list: Vec<(f32, Color32, bool)> = shaded(&[
                (0.0, grey(30)),
                (m * 0.25, grey(84)),
                (m * 0.7, grey(50)),
                (m, grey(22)),
            ]);
            list.extend([(m, MAT, false), (1.0, MAT, false)]);
            list
        }
    }
}

/// Bands round the inside of `outer`, mitred at the corners: each stop is
/// an inset from `outer`'s edge (points, growing) and the colours there on
/// the top, right, bottom and left sides, blended between stops. Insets
/// past the middle of `outer` are left out.
fn ring(mesh: &mut Mesh, outer: Rect, stops: impl IntoIterator<Item = (f32, [Color32; 4])>) {
    let half = outer.width().min(outer.height()) / 2.0;
    let corners = |d: f32| -> [Pos2; 4] {
        // Top left, top right, bottom right, bottom left, `d` inside.
        [
            pos2(outer.left() + d, outer.top() + d),
            pos2(outer.right() - d, outer.top() + d),
            pos2(outer.right() - d, outer.bottom() - d),
            pos2(outer.left() + d, outer.bottom() - d),
        ]
    };
    let mut before: Option<(f32, [Color32; 4])> = None;
    for (d, colors) in stops {
        let d = d.min(half);
        if let Some((d0, c0)) = before
            && d > d0
        {
            let (a, b) = (corners(d0), corners(d));
            for side in 0..4 {
                let next = (side + 1) % 4;
                let i = mesh.vertices.len() as u32;
                mesh.colored_vertex(a[side], c0[side]);
                mesh.colored_vertex(a[next], c0[side]);
                mesh.colored_vertex(b[next], colors[side]);
                mesh.colored_vertex(b[side], colors[side]);
                mesh.add_triangle(i, i + 1, i + 2);
                mesh.add_triangle(i, i + 2, i + 3);
            }
        }
        before = Some((d, colors));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::vec2;

    #[test]
    fn names() {
        for b in Border::ALL {
            assert_eq!(Border::from_name(b.name()), Some(b));
        }
        assert_eq!(Border::from_name("gold"), None);
    }

    #[test]
    fn the_opening_inside_the_frame() {
        let area = Rect::from_min_size(pos2(0.0, 0.0), vec2(480.0, 320.0));
        assert_eq!(Border::None.opening(area, 5), area);
        // 4 steps of 1.5 % of 320.
        assert_eq!(Border::Wood.width(area, 4), 19.0);
        assert_eq!(Border::Wood.opening(area, 4), area.shrink(19.0));
        // Never thinner than 3 points, nor wider than the window.
        let tiny = Rect::from_min_size(pos2(0.0, 0.0), vec2(120.0, 90.0));
        assert_eq!(Border::Black.width(tiny, 1), 3.0);
        assert!(Border::Black.opening(tiny, MAX_WIDTH).is_positive());
    }

    #[test]
    fn every_style_paints_a_closed_ring() {
        let area = Rect::from_min_size(pos2(0.0, 0.0), vec2(480.0, 320.0));
        for b in Border::ALL {
            let stops = stops(b);
            if b == Border::None {
                assert!(stops.is_empty());
                continue;
            }
            assert_eq!(stops.first().map(|s| s.0), Some(0.0), "{b:?}");
            assert_eq!(stops.last().map(|s| s.0), Some(1.0), "{b:?}");
            assert!(stops.windows(2).all(|w| w[0].0 <= w[1].0), "{b:?}");
            let mut mesh = Mesh::default();
            ring(
                &mut mesh,
                area,
                stops.iter().map(|&(t, c, _)| (t * 20.0, [c; 4])),
            );
            assert!(!mesh.indices.is_empty());
            assert!(mesh.is_valid());
        }
    }
}
