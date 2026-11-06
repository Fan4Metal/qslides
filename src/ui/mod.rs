//! User interface pieces: the app icon as a texture, a window of the
//! program's own shown without a white flash (`OwnWindow`, About), the
//! slideshow's buttons painted over the picture; the About window
//! (`about`), the screen shown while there is nothing to show (`start`)
//! and the painted icons shared by the family (`icons`).

pub mod about;
pub mod icons;
pub mod start;

use egui::{Color32, Pos2, Response, Sense, TextureHandle, TextureOptions, Ui};
use icons::Icon;

/// The app icon as a texture for `points` wide, from the smallest embedded
/// size (64, 128, 256 px) that covers it at the display's pixel density;
/// kept in `cache` and replaced when that density changes.
pub fn app_icon(
    ctx: &egui::Context,
    points: f32,
    cache: &mut Option<TextureHandle>,
) -> TextureHandle {
    let px = (points * ctx.pixels_per_point()).round() as u32;
    let size = [64u32, 128, 256]
        .into_iter()
        .find(|&s| s >= px)
        .unwrap_or(256);
    let n = size as usize;
    if cache.as_ref().is_none_or(|t| t.size() != [n, n]) {
        let image = egui::ColorImage::from_rgba_unmultiplied([n, n], crate::embedded_icon(size));
        let options = TextureOptions::LINEAR.with_mipmap_mode(Some(egui::TextureFilter::Linear));
        *cache = Some(ctx.load_texture(format!("app_icon_{size}"), image, options));
    }
    cache.clone().expect("set above")
}

/// A round button of the slideshow over the picture, `radius` points,
/// centred at `center`, at `opacity` (0 to 1: the buttons fade in and out
/// with the pointer).
pub fn round_button(
    ui: &mut Ui,
    icon: Icon,
    center: Pos2,
    radius: f32,
    opacity: f32,
    tip: &str,
) -> Response {
    let rect = egui::Rect::from_center_size(center, egui::vec2(radius * 2.0, radius * 2.0));
    let id = ui.id().with(("round", icon));
    let response = ui.interact(rect, id, Sense::click());
    let ground = if response.hovered() { 200 } else { 140 };
    let fill = Color32::from_black_alpha((ground as f32 * opacity) as u8);
    ui.painter().circle_filled(center, radius, fill);
    let ink = Color32::from_white_alpha((235.0 * opacity) as u8);
    icons::paint(ui.painter(), icon, center, radius * 0.95, ink, fill);
    if opacity > 0.5 {
        response.on_hover_text(tip)
    } else {
        response
    }
}

/// A window of the program's own (an immediate viewport: About) shown
/// without a white flash: Windows shows a new window white until its first
/// frame is presented. It is made hidden (`with_visible(false)`), found by
/// its title, cloaked, owned by the program's window (`win::adopt`), shown,
/// and uncloaked once it has painted a few frames (metalamp's).
#[derive(Default)]
pub struct OwnWindow {
    /// Found (none: not yet; `Some(None)`: given up looking, shown as it
    /// is), and the frames painted since it was shown.
    hwnd: Option<Option<isize>>,
    painted: u32,
    looked: u32,
}

impl OwnWindow {
    /// Frames painted before the window is uncloaked.
    const PAINTED: u32 = 3;

    /// Opened anew: found and shown again.
    pub fn reset(&mut self) {
        *self = OwnWindow::default();
    }

    /// The builder of a window made hidden.
    pub fn builder(builder: egui::ViewportBuilder) -> egui::ViewportBuilder {
        builder.with_visible(false)
    }

    /// Each frame inside the viewport's closure (`ctx` is the viewport's):
    /// finds, adopts and shows the window, then uncloaks it.
    pub fn follow(&mut self, ctx: &egui::Context, title: &str, owner: Option<isize>) {
        let more = || {
            ctx.request_repaint();
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        };
        if let Some(Some(hwnd)) = self.hwnd
            && !crate::win::is_window(hwnd)
        {
            self.reset();
        }
        match self.hwnd {
            None => {
                self.looked += 1;
                let found = owner.and_then(|main| Some((main, crate::win::thread_window(title)?)));
                if let Some((main, hwnd)) = found {
                    crate::caption::cloak(hwnd, true);
                    crate::win::adopt(hwnd, main);
                    crate::caption::set_dark(hwnd, true);
                    self.hwnd = Some(Some(hwnd));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                } else if self.looked > 10 {
                    // Not found (no owner): shown as it is.
                    self.hwnd = Some(None);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                }
                more();
            }
            Some(hwnd) if self.painted < Self::PAINTED => {
                self.painted += 1;
                if self.painted == Self::PAINTED
                    && let Some(hwnd) = hwnd
                {
                    crate::caption::cloak(hwnd, false);
                    crate::win::bring_forward(hwnd);
                }
                more();
            }
            Some(_) => {}
        }
    }
}
