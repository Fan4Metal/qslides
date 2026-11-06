//! The screen shown while there is nothing to show (the library is empty,
//! is being scanned, has no pictures): the app icon, the name, the version
//! and one line on how to begin, as in qview and MetalMD. The widget is
//! often small, so the icon and the gaps shrink with the window, and the
//! name and the version are left out where there is no room for them.

use egui::{Align, Layout, RichText, TextureHandle, Ui, UiBuilder, vec2};

/// The icon's largest side in points: 112 in qview and MetalMD.
const ICON: f32 = 112.0;

/// Fill the rest of `ui` with the start screen. `hint` is one line, the one
/// or two ways to begin.
pub fn show(ui: &mut Ui, icon_cache: &mut Option<TextureHandle>, hint: &str) {
    let rect = ui.available_rect_before_wrap();
    let side = (rect.height() * 0.3).clamp(24.0, ICON);
    let icon = super::app_icon(ui.ctx(), side, icon_cache);
    let roomy = rect.height() >= 220.0;
    let layout = Layout::top_down(Align::Center);
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(rect.shrink2(vec2(12.0, 0.0)))
            .layout(layout),
        |ui| {
            // The icon just above the middle, the text just below it, so the
            // screen stays put whatever the hint's length.
            ui.add_space((rect.height() * 0.5 - side).max(4.0));
            ui.image((icon.id(), vec2(side, side)));
            ui.add_space(if roomy { 14.0 } else { 6.0 });
            if roomy {
                ui.label(RichText::new(crate::APP_NAME).size(30.0).strong());
                ui.weak(tr!(
                    format!("Version {}", crate::VERSION),
                    format!("Версия {}", crate::VERSION)
                ));
                ui.add_space(18.0);
            }
            ui.label(hint);
        },
    );
}
