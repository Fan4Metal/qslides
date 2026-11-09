//! The About window: icon, name, version, a short description, the author,
//! the homepage, the license, where the settings are kept, the interface
//! language and the update check, as in every app of the family. The
//! widget is often smaller than About, so About is a window of its own (an
//! immediate viewport owned by the widget, `ui::OwnWindow`), not a modal
//! inside it.

use egui::{Key, TextureHandle, Ui};

use crate::i18n::LangChoice;
use crate::update::{Status, Updates, release_url, version_of};

const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
const LICENSE: &str = env!("CARGO_PKG_LICENSE");
/// Cargo joins several authors with `:`.
const AUTHORS: &str = env!("CARGO_PKG_AUTHORS");
/// Displayed icon size in points.
const ICON: f32 = 64.0;
/// Width of the language list, enough for its longest entry.
const LANG_WIDTH: f32 = 220.0;
/// Width of the window's content.
const WIDTH: f32 = 340.0;

pub struct AboutWindow {
    pub open: bool,
    /// Interface language (kept in the settings).
    pub lang: LangChoice,
    icon: Option<TextureHandle>,
    own: super::OwnWindow,
    /// The height of the content, measured in the frame before: the window
    /// is made that high (while it is cloaked the change is not seen).
    height: f32,
}

impl Default for AboutWindow {
    fn default() -> Self {
        Self {
            open: false,
            lang: LangChoice::System,
            icon: None,
            own: Default::default(),
            height: 480.0,
        }
    }
}

impl AboutWindow {
    pub fn toggle(&mut self) {
        self.open = !self.open;
        if self.open {
            self.own.reset();
        }
    }

    /// Draws the window while it is open; closes it on its cross, Esc or
    /// Enter. A language picked in it is applied at once and left in
    /// `lang` for the settings. `owner` is the widget's window; `on_top`:
    /// the widget is always on top, so About is too (an owned window is
    /// kept above its owner, but a plain one was still covered by a
    /// topmost owner), or on the desktop, where About would open under
    /// the other windows.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        owner: Option<isize>,
        on_top: bool,
        updates: &mut Updates,
    ) {
        if !self.open {
            return;
        }
        let title = tr!(
            format!("About {}", crate::APP_NAME),
            format!("О программе {}", crate::APP_NAME)
        );
        let builder = super::OwnWindow::builder(
            egui::ViewportBuilder::default()
                .with_title(&title)
                .with_inner_size([WIDTH + 16.0, self.height])
                .with_resizable(false)
                .with_minimize_button(false)
                .with_maximize_button(false)
                .with_window_level(if on_top {
                    egui::WindowLevel::AlwaysOnTop
                } else {
                    egui::WindowLevel::Normal
                })
                .with_icon(crate::window_icon()),
        );
        let id = egui::ViewportId::from_hash_of("about");
        let closed = ctx.show_viewport_immediate(id, builder, |ui, _class| {
            let ctx = ui.ctx().clone();
            self.own.follow(&ctx, &title, owner, true);
            let icon = super::app_icon(&ctx, ICON, &mut self.icon);
            let panel = egui::CentralPanel::default().show(ui, |ui| {
                ui.set_width(WIDTH);
                ui.vertical_centered(|ui| self.content(ui, &icon, updates))
                    .response
                    .rect
                    .height()
            });
            // The content and the panel's margins; while the window is
            // still cloaked, its first frames find the height unseen.
            let height = (panel.inner + 16.0).round();
            if height > 100.0 && (height - self.height).abs() > 1.0 {
                self.height = height;
                ctx.request_repaint();
            }
            ctx.input(|i| {
                i.viewport().close_requested()
                    || i.key_pressed(Key::Escape)
                    || i.key_pressed(Key::Enter)
            })
        });
        if closed {
            self.open = false;
        }
    }

    fn content(&mut self, ui: &mut Ui, icon: &TextureHandle, updates: &mut Updates) {
        ui.add_space(8.0);
        ui.image((icon.id(), egui::vec2(ICON, ICON)));
        ui.add_space(6.0);
        ui.heading(crate::APP_NAME);
        ui.label(tr!(
            format!("Version {}", crate::VERSION),
            format!("Версия {}", crate::VERSION)
        ));
        ui.add_space(8.0);
        // One sentence, the same as Cargo.toml's description and the
        // README tagline; the Russian one is written by hand.
        ui.label(tr!(
            "A desktop photo frame that shows pictures and videos from your folders.",
            "Фоторамка на рабочем столе: слайд-шоу картинок и видео из ваших папок."
        ));
        ui.add_space(8.0);
        if !AUTHORS.is_empty() {
            let authors = AUTHORS.replace(':', ", ");
            ui.label(tr!(
                format!("Author: {authors}"),
                format!("Автор: {authors}")
            ));
        }
        // Who the program was thought of by.
        ui.label(tr!("Idea: helldragon", "Идея: helldragon"));
        if !REPOSITORY.is_empty()
            && ui
                .link(tr!("Homepage", "Сайт проекта"))
                .on_hover_text(REPOSITORY)
                .clicked()
            && !crate::win::shell_open(REPOSITORY)
        {
            log::warn!("could not open {REPOSITORY}");
        }
        if !LICENSE.is_empty() {
            ui.weak(tr!(
                format!("{LICENSE} License"),
                format!("Лицензия {LICENSE}")
            ));
        }
        let stored = crate::settings::location();
        let place = ui.weak(if stored.portable {
            tr!(
                "Settings: next to the program (portable)",
                "Настройки: рядом с программой (портативно)"
            )
        } else {
            tr!(
                "Settings: in the user profile",
                "Настройки: в профиле пользователя"
            )
        });
        match &stored.file {
            Some(path) => place.on_hover_text(path.display().to_string()),
            None => place.on_hover_text(tr!("Settings are not saved", "Настройки не сохраняются")),
        };
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);
        // The label over the list, both centred; a fixed width keeps the
        // list from jumping when the language changes.
        ui.weak(tr!("Interface language", "Язык интерфейса"));
        let mut choice = self.lang;
        // A combo box lays itself out left to right, ignoring the
        // centring: indented by hand (`width` is its outer width).
        ui.horizontal(|ui| {
            ui.add_space(((ui.available_width() - LANG_WIDTH) / 2.0).max(0.0));
            egui::ComboBox::from_id_salt("language")
                .selected_text(choice.label())
                .width(LANG_WIDTH)
                .show_ui(ui, |ui| {
                    for c in LangChoice::ALL {
                        ui.selectable_value(&mut choice, c, c.label());
                    }
                });
        });
        if choice != self.lang {
            self.lang = choice;
            crate::i18n::set_lang(choice.resolve());
        }
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);
        update_section(ui, updates);
        ui.add_space(8.0);
    }
}

/// The update check: the start-up option, a button to check now and the
/// outcome.
fn update_section(ui: &mut Ui, updates: &mut Updates) {
    let before = updates.enabled;
    ui.checkbox(
        &mut updates.enabled,
        tr!("Check for updates at start-up (once a day)", "Проверять обновления при запуске (раз в сутки)"),
    )
    .on_hover_text(tr!(
        "Asks api.github.com for the latest release; nothing else is sent, and nothing is downloaded",
        "Запрашивает у api.github.com последний выпуск; больше ничего не отправляется и не скачивается"
    ));
    if updates.enabled && !before {
        updates.start_if_due(ui.ctx());
    }
    let checking = updates.status == Status::Checking;
    if ui
        .add_enabled(
            !checking,
            egui::Button::new(tr!("Check now", "Проверить сейчас")),
        )
        .clicked()
    {
        updates.start(ui.ctx());
    }
    match &updates.status {
        Status::Unknown => {}
        Status::Checking => {
            ui.horizontal(|ui| {
                // Centred by hand, as the language list above.
                ui.add_space(((ui.available_width() - 110.0) / 2.0).max(0.0));
                ui.add(egui::Spinner::new());
                ui.weak(tr!("Checking…", "Проверка…"));
            });
        }
        Status::UpToDate => {
            ui.weak(tr!(
                "This is the latest version",
                "Установлена последняя версия"
            ));
        }
        Status::Newer(tag) => {
            let version = version_of(tag);
            let url = release_url(tag);
            if ui
                .link(tr!(
                    format!("Version {version} is available"),
                    format!("Доступна версия {version}")
                ))
                .on_hover_text(url.as_str())
                .clicked()
                && !crate::win::shell_open(&url)
            {
                log::warn!("could not open {url}");
            }
        }
        Status::Failed(e) => {
            ui.weak(tr!(
                "Could not check for updates",
                "Не удалось проверить обновления"
            ))
            .on_hover_text(e);
        }
    }
}
