//! The widget's menu, the one control it has besides its buttons. It is
//! drawn by egui, but in a window of its own (an immediate viewport without
//! a frame, owned by the widget, `ui::OwnWindow`): a popup of egui is a
//! layer inside its window and would be cut by the edges of a small widget,
//! a window of its own opens beyond them. It opens at the pointer, flipped
//! left or up near the edges of the screen (`win::place_popup`), on a right
//! click on the widget, and closes when it loses the
//! focus, on Esc and after a command that leads elsewhere.
//!
//! It is a panel rather than a list of submenus: the controls of the show
//! at the top with About and a cross that closes the menu at their right,
//! the settings with a slider for the time a slide stays, the library's
//! folders under a header that opens, the switches. Exit is not here: the
//! widget has a cross of its own, and the file's buttons (open it, show it
//! in its folder) are on the widget too.

use std::path::PathBuf;

use egui::{Key, RichText, Ui};

use crate::border::{self, Border};
use crate::show::{
    ASPECTS, Caption, CaptionPart, Fit, INTERVALS, Kinds, Layer, MAX_PIECES, Order, VIDEO_TIMES,
    VideoLength, aspect_label, interval_label, nearest_interval, video_time_label, video_time_step,
};
use crate::ui::OwnWindow;
use crate::ui::icons::Icon;

/// What the menu can ask for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cmd {
    Previous,
    Next,
    Pause,
    OpenFile,
    ShowInFolder,
    AddFolder,
    OpenFolder(usize),
    RemoveFolder(usize),
    Rescan,
    Interval(u32),
    Order(Order),
    /// The kinds of files shown, one switched on or off.
    Kinds(Kinds),
    Fit(Fit),
    /// The frame's style and its width.
    Border(Border),
    BorderWidth(u32),
    /// The time a video has: the way, and the limit in seconds when shown
    /// to its end (0: none).
    VideoTime(VideoLength, u32),
    VideoPieces(u32),
    /// The seek bar over a video shown whole, switched on or off.
    VideoBar,
    /// The caption with a part switched on or off.
    Caption(Caption),
    Aspect(Option<(u32, u32)>),
    /// The window above all others or on the desktop, switched on or off.
    Layer(Layer),
    /// Give way to full-screen programs while always on top, or not.
    YieldFullScreen,
    Autostart,
    TrayOnly,
    About,
    /// Open the page of the newer release.
    Update,
    /// Close the menu (its cross).
    Close,
}

impl Cmd {
    /// Whether the menu closes after it: a command that opens something
    /// else. The controls of the show and the settings
    /// leave it open, so that their effect is seen and more can be changed.
    pub fn closes_menu(&self) -> bool {
        matches!(
            self,
            Cmd::OpenFile
                | Cmd::ShowInFolder
                | Cmd::AddFolder
                | Cmd::OpenFolder(_)
                | Cmd::About
                | Cmd::Update
                | Cmd::Close
        )
    }
}

/// What the menu shows: the state of the show and of the settings.
pub struct State {
    pub has_file: bool,
    pub paused: bool,
    pub folders: Vec<PathBuf>,
    /// For each of `folders`: the last scan could not read it.
    pub unreachable: Vec<bool>,
    pub interval: u32,
    pub order: Order,
    pub kinds: Kinds,
    pub fit: Fit,
    pub border: Border,
    pub border_width: u32,
    pub video: VideoLength,
    pub video_limit: u32,
    pub video_pieces: u32,
    pub video_bar: bool,
    pub caption: Caption,
    pub aspect: Option<(u32, u32)>,
    pub layer: Layer,
    pub yield_full_screen: bool,
    pub autostart: bool,
    pub tray_only: bool,
    /// The version of a newer release, when one was found.
    pub update: Option<String>,
    /// The place of the file shown in the show, from 1, and the number of
    /// files in it.
    pub position: Option<(usize, usize)>,
}

/// The width of the menu's content, in points.
const WIDTH: f32 = 300.0;
/// The panel's margin and border round the content.
const MARGIN: f32 = 10.0;
/// The widest a tooltip is, its frame left out: inside the menu's window,
/// which a tooltip cannot leave (a layer of it), with room for the frame.
pub const TOOLTIP_WIDTH: f32 = WIDTH - 20.0;
/// The window's title: never shown (it has no caption), it finds the
/// window among the thread's (`win::thread_window`).
const TITLE: &str = "qslides menu";

pub struct MenuWindow {
    pub open: bool,
    /// Where it was opened, in screen pixels.
    at: (i32, i32),
    own: OwnWindow,
    /// The height of the content, measured in the frame before.
    height: f32,
    /// The window has had the focus: losing it now closes the menu.
    focused: bool,
}

impl Default for MenuWindow {
    fn default() -> Self {
        Self {
            open: false,
            at: (0, 0),
            own: OwnWindow::default(),
            height: 420.0,
            focused: false,
        }
    }
}

impl MenuWindow {
    /// Open the menu at `at` (screen pixels), or move it there.
    pub fn open_at(&mut self, at: (i32, i32)) {
        self.open = true;
        self.at = at;
        self.own.reset();
        self.focused = false;
    }

    /// Draw the menu while it is open; the commands chosen in this frame.
    /// `owner` is the widget's window.
    pub fn show(&mut self, ctx: &egui::Context, owner: Option<isize>, state: &State) -> Vec<Cmd> {
        if !self.open {
            return Vec::new();
        }
        let builder = OwnWindow::builder(
            egui::ViewportBuilder::default()
                .with_title(TITLE)
                .with_inner_size([WIDTH + 2.0 * MARGIN, self.height])
                .with_decorations(false)
                .with_resizable(false)
                .with_taskbar(false)
                .with_window_level(egui::WindowLevel::AlwaysOnTop),
        );
        let mut cmds = Vec::new();
        let id = egui::ViewportId::from_hash_of("menu");
        let closed = ctx.show_viewport_immediate(id, builder, |ui, _class| {
            let ctx = ui.ctx().clone();
            self.own.follow(&ctx, TITLE, owner, false);
            if let Some(hwnd) = self.own.hwnd() {
                crate::win::place_popup(hwnd, self.at);
            }
            let visuals = ctx.global_style().visuals.clone();
            let frame = egui::Frame::new()
                .fill(visuals.window_fill)
                .stroke(visuals.window_stroke)
                .inner_margin(MARGIN - visuals.window_stroke.width);
            let panel = egui::CentralPanel::default().frame(frame).show(ui, |ui| {
                ui.set_width(WIDTH);
                // The content's own height: the panel's is the window's.
                ui.vertical(|ui| content(ui, state, &mut cmds))
                    .response
                    .rect
                    .height()
            });
            let height = (panel.inner + 2.0 * MARGIN).round();
            if height > 50.0 && (height - self.height).abs() > 1.0 {
                self.height = height;
                ctx.request_repaint();
            }
            let focused = ctx.input(|i| i.viewport().focused);
            if focused == Some(true) {
                self.focused = true;
            }
            let lost = self.focused && focused == Some(false);
            lost || ctx.input(|i| i.key_pressed(Key::Escape) || i.viewport().close_requested())
        });
        if closed || cmds.iter().any(Cmd::closes_menu) {
            self.open = false;
        }
        cmds
    }
}

/// A row of buttons, one of which is chosen: the command of the one
/// clicked.
fn choice<T: Copy + PartialEq>(
    ui: &mut Ui,
    current: T,
    options: impl IntoIterator<Item = (T, String)>,
) -> Option<T> {
    let mut chosen = None;
    ui.horizontal_wrapped(|ui| {
        for (value, label) in options {
            if ui.selectable_label(value == current, label).clicked() && value != current {
                chosen = Some(value);
            }
        }
    });
    chosen
}

/// A heading of a group of settings.
fn heading(ui: &mut Ui, text: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(text).small().weak());
}

fn content(ui: &mut Ui, s: &State, cmds: &mut Vec<Cmd>) {
    // The show: back, pause, on; the file shown, opened or found.
    ui.horizontal(|ui| {
        ui.add_enabled_ui(s.has_file, |ui| {
            if crate::ui::icon_button(
                ui,
                Icon::SkipBack,
                tr!("Previous (Left)", "Предыдущий (Влево)"),
            )
            .clicked()
            {
                cmds.push(Cmd::Previous);
            }
            let (icon, tip) = if s.paused {
                (Icon::Play, tr!("Go on (Space)", "Продолжить (Пробел)"))
            } else {
                (Icon::Pause, tr!("Pause (Space)", "Пауза (Пробел)"))
            };
            if crate::ui::icon_button(ui, icon, tip).clicked() {
                cmds.push(Cmd::Pause);
            }
            if crate::ui::icon_button(
                ui,
                Icon::SkipForward,
                tr!("Next (Right)", "Следующий (Вправо)"),
            )
            .clicked()
            {
                cmds.push(Cmd::Next);
            }
            if let Some((at, of)) = s.position {
                ui.add_space(6.0);
                ui.label(format!("{at} / {of}")).on_hover_text(tr!(
                    "The file shown and the number of files in the show",
                    "Показанный файл и число файлов в показе"
                ));
            }
        });
        // The file's own buttons are on the widget, top left; here, About
        // and a cross that closes the menu, as a window's title row has.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if crate::ui::icon_button(ui, Icon::Close, tr!("Close (Esc)", "Закрыть (Esc)"))
                .clicked()
            {
                cmds.push(Cmd::Close);
            }
            let about = tr!(
                format!("About {} (F1)", crate::APP_NAME),
                format!("О программе {} (F1)", crate::APP_NAME)
            );
            if crate::ui::icon_button(ui, Icon::Info, &about).clicked() {
                cmds.push(Cmd::About);
            }
        });
    });
    if let Some(v) = &s.update
        && ui
            .link(tr!(
                format!("Version {v} is available…"),
                format!("Доступна версия {v}…")
            ))
            .clicked()
    {
        cmds.push(Cmd::Update);
    }
    ui.separator();

    heading(ui, tr!("Show each for", "Время показа"));
    ui.horizontal(|ui| {
        let mut step = nearest_interval(s.interval);
        ui.spacing_mut().slider_width = WIDTH - 110.0;
        let slider = egui::Slider::new(&mut step, 0..=INTERVALS.len() - 1).show_value(false);
        let response = ui.add(slider);
        ui.label(interval_label(INTERVALS[step]));
        if response.changed() && INTERVALS[step] != s.interval {
            cmds.push(Cmd::Interval(INTERVALS[step]));
        }
    });
    heading(ui, tr!("Order", "Порядок"));
    if let Some(o) = choice(ui, s.order, Order::ALL.map(|o| (o, o.label().to_owned()))) {
        cmds.push(Cmd::Order(o));
    }
    // Switches that add up, as the caption's; the last one on stays.
    heading(ui, tr!("Show", "Показывать"));
    ui.horizontal_wrapped(|ui| {
        for kind in Kinds::EACH {
            let toggled = s.kinds.toggled(kind);
            if ui
                .selectable_label(s.kinds.has(kind), Kinds::label(kind))
                .clicked()
                && toggled != s.kinds
            {
                cmds.push(Cmd::Kinds(toggled));
            }
        }
    });
    heading(ui, tr!("Picture", "Картинка"));
    if let Some(f) = choice(ui, s.fit, Fit::ALL.map(|f| (f, f.label().to_owned()))) {
        cmds.push(Cmd::Fit(f));
    }
    heading(ui, tr!("Frame", "Рамка"));
    if let Some(b) = choice(ui, s.border, Border::ALL.map(|b| (b, b.label().to_owned()))) {
        cmds.push(Cmd::Border(b));
    }
    ui.horizontal(|ui| {
        let framed = s.border != Border::None;
        ui.add_enabled(framed, egui::Label::new(tr!("Width", "Ширина")));
        ui.spacing_mut().slider_width = WIDTH - 110.0;
        let mut width = s.border_width.clamp(1, border::MAX_WIDTH);
        let slider = egui::Slider::new(&mut width, 1..=border::MAX_WIDTH).show_value(false);
        let response = ui
            .add_enabled(framed, slider)
            .on_hover_text(tr!(
                "A share of the window's shorter side",
                "Доля короткой стороны окна"
            ))
            .on_disabled_hover_text(tr!("With a frame", "С рамкой"));
        ui.add_enabled(framed, egui::Label::new(width.to_string()));
        if response.changed() && width != s.border_width {
            cmds.push(Cmd::BorderWidth(width));
        }
    });
    heading(ui, tr!("Videos", "Видео"));
    // The time a video has, one slider from "as long as pictures" through
    // a limit to "whole", and the pieces of a video longer than that.
    egui::Grid::new("video_time")
        .num_columns(3)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            ui.spacing_mut().slider_width = 110.0;
            ui.label(tr!("Length", "Длина"));
            let mut step = video_time_step(s.video, s.video_limit);
            let slider = egui::Slider::new(&mut step, 0..=VIDEO_TIMES.len() - 1).show_value(false);
            let response = ui.add(slider).on_hover_text(tr!(
                "The longest a video is shown: as long as a picture (a shorter one played again), a time of its own, or whole however long it is",
                "Сколько показывается видео: столько же, сколько картинка (более короткое повторяется), своё время или целиком, какой бы длины оно ни было"
            ));
            ui.label(video_time_label(VIDEO_TIMES[step]));
            let (video, limit) = VIDEO_TIMES[step];
            if response.changed() && step != video_time_step(s.video, s.video_limit) {
                cmds.push(Cmd::VideoTime(video, limit));
            }
            ui.end_row();

            // Whole, a video is never longer than its time and is not cut:
            // a seek bar instead, under the slider.
            if s.video == VideoLength::Whole && s.video_limit == 0 {
                ui.label("");
                let mut on = s.video_bar;
                let response = ui.checkbox(&mut on, tr!("Seek bar", "Полоса прокрутки"));
                let response = response.on_hover_text(tr!(
                    "A bar along the bottom of a video, with the buttons: where it is, and a click or a drag goes elsewhere in it",
                    "Полоса внизу видео, вместе с кнопками: где оно сейчас, а щелчок или перетаскивание переходит в другое место"
                ));
                if response.changed() {
                    cmds.push(Cmd::VideoBar);
                }
            } else {
                ui.label(tr!("In pieces", "Кусками"));
                let mut pieces = s.video_pieces.clamp(1, MAX_PIECES);
                let slider = egui::Slider::new(&mut pieces, 1..=MAX_PIECES).show_value(false);
                let response = ui.add(slider).on_hover_text(tr!(
                    "A video longer than its time is shown in so many pieces spread from its start to its end, with a fade between them; 1: its start only",
                    "Видео длиннее своего времени показывается столькими кусками от начала до конца, с растворением между ними; 1 — только начало"
                ));
                ui.label(pieces.to_string());
                if response.changed() && pieces != s.video_pieces {
                    cmds.push(Cmd::VideoPieces(pieces));
                }
            }
            ui.end_row();
        });
    // Switches that add up, not a choice: each part on or off.
    heading(ui, tr!("Caption", "Подпись"));
    ui.horizontal_wrapped(|ui| {
        for part in CaptionPart::ALL {
            if ui
                .selectable_label(s.caption.has(part), part.label())
                .clicked()
            {
                cmds.push(Cmd::Caption(s.caption.toggled(part)));
            }
        }
    });
    heading(ui, tr!("Window proportions", "Пропорции окна"));
    if let Some(a) = choice(ui, s.aspect, ASPECTS.map(|a| (a, aspect_label(a)))) {
        cmds.push(Cmd::Aspect(a));
    }
    ui.add_space(4.0);
    ui.separator();

    let title = tr!(
        format!("Folders ({})", s.folders.len()),
        format!("Папки ({})", s.folders.len())
    );
    egui::CollapsingHeader::new(title)
        .id_salt("folders")
        .default_open(s.folders.is_empty())
        .show(ui, |ui| {
            for (i, folder) in s.folders.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let remove = tr!("Remove from the library", "Убрать из библиотеки");
                        if crate::ui::icon_button(ui, Icon::Delete, remove).clicked() {
                            cmds.push(Cmd::RemoveFolder(i));
                        }
                        let open = tr!("Open in Explorer", "Открыть в Проводнике");
                        if crate::ui::icon_button(ui, Icon::Folder, open).clicked() {
                            cmds.push(Cmd::OpenFolder(i));
                        }
                        let text = folder.display().to_string();
                        let unreachable = s.unreachable.get(i).copied().unwrap_or(false);
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            if unreachable {
                                let label =
                                    egui::RichText::new(&text).color(ui.visuals().warn_fg_color);
                                let tip = tr!(
                                    format!("{text}\nCannot be read now (a drive not connected?)"),
                                    format!("{text}\nСейчас недоступна (диск не подключён?)")
                                );
                                ui.add(egui::Label::new(label).truncate())
                                    .on_hover_text(tip);
                            } else {
                                ui.add(egui::Label::new(&text).truncate())
                                    .on_hover_text(&text);
                            }
                        });
                    });
                });
            }
            ui.horizontal(|ui| {
                if ui
                    .button(tr!("Add a folder…", "Добавить папку…"))
                    .on_hover_text("Ctrl+O")
                    .clicked()
                {
                    cmds.push(Cmd::AddFolder);
                }
                let rescan = egui::Button::new(tr!("Look for new files", "Найти новые файлы"));
                if ui
                    .add_enabled(!s.folders.is_empty(), rescan)
                    .on_hover_text("F5")
                    .clicked()
                {
                    cmds.push(Cmd::Rescan);
                }
            });
        });
    ui.separator();

    let mut switch = |ui: &mut Ui, on: bool, label: &str, cmd: Cmd| {
        let mut value = on;
        if ui.checkbox(&mut value, label).changed() {
            cmds.push(cmd);
        }
    };
    switch(
        ui,
        s.layer == Layer::Top,
        tr!("Always on top", "Поверх всех окон"),
        Cmd::Layer(Layer::Top),
    );
    // Under it, as it matters only above all windows.
    ui.indent("yield", |ui| {
        ui.add_enabled_ui(s.layer == Layer::Top, |ui| {
            switch(
                ui,
                s.yield_full_screen,
                tr!(
                    "Not over full-screen programs",
                    "Не поверх полноэкранных программ"
                ),
                Cmd::YieldFullScreen,
            );
        })
        .response
        .on_disabled_hover_text(tr!(
            "With \"Always on top\"",
            "С пунктом «Поверх всех окон»"
        ));
    });
    switch(
        ui,
        s.layer == Layer::Desktop,
        tr!(
            "On the desktop, under all windows",
            "На рабочем столе, под всеми окнами"
        ),
        Cmd::Layer(Layer::Desktop),
    );
    switch(
        ui,
        s.autostart,
        tr!("Start with Windows", "Запускать вместе с Windows"),
        Cmd::Autostart,
    );
    switch(
        ui,
        s.tray_only,
        tr!("Only in the tray", "Только в трее"),
        Cmd::TrayOnly,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> State {
        State {
            has_file: true,
            paused: false,
            folders: vec![PathBuf::from(r"C:\Photos & Co"), PathBuf::from(r"D:\Video")],
            unreachable: vec![false, true],
            interval: 10,
            order: Order::Random,
            kinds: Kinds::ALL,
            fit: Fit::Fill,
            border: Border::Mat,
            border_width: 4,
            video: VideoLength::Whole,
            video_limit: 120,
            video_pieces: 6,
            video_bar: false,
            caption: Caption::NONE.toggled(CaptionPart::Name),
            aspect: Some((16, 9)),
            layer: Layer::Top,
            yield_full_screen: true,
            autostart: false,
            tray_only: false,
            update: Some("0.2.0".into()),
            position: Some((12, 340)),
        }
    }

    /// The menu drawn headless; a frame with no input asks for nothing.
    #[test]
    fn the_menu_draws_and_asks_for_nothing_by_itself() {
        let ctx = egui::Context::default();
        // With a limit (the pieces), and whole (the seek bar).
        let whole = State {
            video_limit: 0,
            ..state()
        };
        for s in [state(), state(), whole] {
            let mut cmds = Vec::new();
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                egui::CentralPanel::default().show(ui, |ui| content(ui, &s, &mut cmds));
            });
            out.textures_delta.clear();
            assert!(cmds.is_empty(), "{cmds:?}");
        }
    }

    #[test]
    fn what_closes_the_menu() {
        assert!(Cmd::About.closes_menu());
        assert!(Cmd::OpenFolder(0).closes_menu());
        assert!(Cmd::Close.closes_menu());
        assert!(!Cmd::Interval(5).closes_menu());
        assert!(!Cmd::Next.closes_menu());
        assert!(!Cmd::RemoveFolder(0).closes_menu());
    }
}
