//! The widget: the eframe `App` showing the slideshow in a frameless
//! window. The picture fills the window (black around it); moving the
//! pointer over it brings up the Previous, Pause and Next buttons; the
//! rest is in the menu of the right button (`menu.rs`).
//!
//! The show keeps one slide on the screen (`current`) and gets the next
//! one ready while it is up (`upcoming`): a picture decoded by the loader
//! thread, a video started on its own thread with its first frame decoded,
//! so that the change is immediate. `step` counts the moves through the
//! playlist; a slide and the upcoming one carry the step they are for,
//! which tells whether the upcoming one is the next one or the one the
//! show is waiting for.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use egui::{
    Color32, ColorImage, Key, Modifiers, Pos2, Rect, Sense, TextureHandle, TextureOptions, pos2,
    vec2,
};

use crate::library::{Item, Kind, Scanned};
use crate::loader::{Done, Loader};
use crate::media::{Video, VideoMsg};
use crate::menu::Cmd;
use crate::settings::Settings;
use crate::show::{Fit, Playlist, VideoLength};
use crate::ui::about::AboutWindow;
use crate::ui::icons::Icon;
use crate::update::Updates;

/// Frames the window paints, cloaked, before it is shown, and the longest
/// it stays cloaked.
const PAINTED_TO_SHOW: u64 = 3;
const UNCLOAK_TIMEOUT: Duration = Duration::from_secs(1);
/// How long one slide fades into the next.
const FADE: Duration = Duration::from_millis(700);
/// The buttons go when the pointer has not moved for this long.
const CONTROLS_FOR: Duration = Duration::from_millis(2500);
/// Videos are decoded no larger than this (their processor scales them):
/// a larger frame costs more to convert and upload than it shows.
const VIDEO_SIDE: u32 = 1920;
/// How often a tray icon the shell refused is asked for again.
const TRAY_RETRY: Duration = Duration::from_secs(3);

/// The time a slide has been up, the pauses left out.
struct Clock {
    start: Instant,
    paused_at: Option<Instant>,
    paused_for: Duration,
}

impl Clock {
    fn new(paused: bool) -> Clock {
        let now = Instant::now();
        Clock {
            start: now,
            paused_at: paused.then_some(now),
            paused_for: Duration::ZERO,
        }
    }

    fn elapsed(&self) -> Duration {
        let end = self.paused_at.unwrap_or_else(Instant::now);
        end.saturating_duration_since(self.start)
            .saturating_sub(self.paused_for)
    }

    fn pause(&mut self, on: bool) {
        match (on, self.paused_at) {
            (true, None) => self.paused_at = Some(Instant::now()),
            (false, Some(at)) => {
                self.paused_for += at.elapsed();
                self.paused_at = None;
            }
            _ => {}
        }
    }
}

/// A video on the screen: its decoder, the texture its frames go into.
struct Playing {
    video: Video,
    texture: TextureHandle,
    /// The time of its first frame (100 ns units): the clock counts from it.
    first: i64,
    /// A frame received but not yet due.
    pending: Option<(i64, ColorImage)>,
    /// The decoder sent its last frame.
    ended: bool,
    /// Played again from the start after its end (videos as long as
    /// pictures).
    looped: bool,
}

impl Playing {
    /// Show the frames due at `elapsed`; the time until the next one, if
    /// one is waiting.
    fn pump(&mut self, elapsed: Duration) -> Option<Duration> {
        let now = self.first + (elapsed.as_nanos() / 100) as i64;
        loop {
            if self.pending.is_none() {
                match self.video.rx.try_recv() {
                    Ok(VideoMsg::Frame(t, image)) => self.pending = Some((t, image)),
                    Ok(VideoMsg::End) | Err(TryRecvError::Disconnected) => {
                        self.ended = true;
                        return None;
                    }
                    Ok(VideoMsg::Failed(e)) => {
                        log::info!("video stopped: {e}");
                        self.ended = true;
                        return None;
                    }
                    Err(TryRecvError::Empty) => return None,
                }
            }
            let due = self.pending.as_ref().map_or(i64::MAX, |(t, _)| *t);
            if due > now {
                return Some(Duration::from_nanos(((due - now) * 100) as u64));
            }
            if let Some((_, image)) = self.pending.take() {
                self.texture.set(image, TextureOptions::LINEAR);
            }
        }
    }
}

enum Content {
    Image(TextureHandle),
    Video(Playing),
}

impl Content {
    fn texture(&self) -> &TextureHandle {
        match self {
            Content::Image(t) => t,
            Content::Video(p) => &p.texture,
        }
    }
}

/// The slide on the screen.
struct Slide {
    item: Item,
    content: Content,
    clock: Clock,
    step: u64,
}

/// What the next slide is getting ready as.
enum Prep {
    /// The loader's job.
    Image(u64),
    /// Started, waiting for its first frame.
    Video(Video),
    Ready(Content),
    Failed(String),
}

struct Upcoming {
    item: Item,
    step: u64,
    prep: Prep,
}

pub struct App {
    folders: Vec<PathBuf>,
    playlist: Playlist,
    /// The file to go on from once the first scan is done.
    resume: Option<PathBuf>,
    scanning: bool,
    generation: u64,
    scans: Receiver<Scanned>,
    scan_tx: Sender<Scanned>,
    scan_ctx: Arc<OnceLock<egui::Context>>,
    loader: Loader,
    current: Option<Slide>,
    upcoming: Option<Upcoming>,
    step: u64,
    /// The picture fading out and when the fade began.
    fade: Option<(TextureHandle, Instant)>,
    paused: bool,
    interval: u32,
    fit: Fit,
    video: VideoLength,
    aspect: Option<(u32, u32)>,
    on_top: bool,
    tray_only: bool,
    tray: Option<crate::tray::Tray>,
    /// When to ask again for a tray icon the shell refused.
    tray_retry: Option<Instant>,
    about: AboutWindow,
    updates: Updates,
    /// The window's handle; `None` off Windows.
    hwnd: Option<isize>,
    /// The window is cloaked until it has painted a few frames: shown
    /// before its first frame reached DWM, it flashed white.
    cloak: Option<(Instant, u64)>,
    start_icon: Option<TextureHandle>,
    /// When the pointer last moved over the window.
    pointer_moved: Option<Instant>,
    /// The wheel turned since the last slide it changed.
    wheel: f32,
    /// The right button went down over the window: the menu opens when it
    /// is let go.
    menu_press: bool,
    menu: crate::menu::MenuWindow,
    /// The widget is disabled for About (`win::enable`).
    modal: bool,
    /// The folder dialog, on its own thread.
    dialog: crate::dialog::FolderDialog,
    /// Windows starts this copy at sign-in (read when the menu opens).
    autostart: bool,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        folders: Vec<PathBuf>,
        scans: (Sender<Scanned>, Receiver<Scanned>),
        scan_ctx: Arc<OnceLock<egui::Context>>,
    ) -> Self {
        let settings = cc.storage.map(Settings::load).unwrap_or_default();
        // Before any text is made.
        crate::i18n::set_lang(settings.language.resolve());
        let ctx = &cc.egui_ctx;
        // A photo frame: always dark.
        ctx.set_theme(egui::ThemePreference::Dark);
        ctx.options_mut(|o| {
            o.zoom_with_keyboard = false;
            o.input_options.max_double_click_delay = crate::win::double_click_time();
            o.input_options.line_scroll_speed = 100.0;
        });
        let _ = scan_ctx.set(ctx.clone());
        let hwnd = crate::caption::hwnd(cc);
        let mut cloak = None;
        if let Some(hwnd) = hwnd {
            // The window is still hidden (eframe shows it after the first
            // frame): cloaked until it has painted.
            crate::caption::cloak(hwnd, true);
            cloak = Some((Instant::now() + UNCLOAK_TIMEOUT, ctx.cumulative_frame_nr()));
            crate::caption::set_dark(hwnd, true);
            crate::win::round_corners(hwnd);
            let wake = ctx.clone();
            crate::win::hook_window(hwnd, move || wake.request_repaint());
            crate::instance::listen(ctx, hwnd);
            if settings.aspect.is_some() {
                crate::win::set_kept_aspect(settings.aspect);
            }
            if settings.tray_only {
                crate::win::set_taskbar_button(hwnd, false);
            }
        }
        if settings.on_top {
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
                egui::WindowLevel::AlwaysOnTop,
            ));
        }
        let mut updates = Updates::new(settings.check_updates, settings.last_update_check);
        updates.start_if_due(ctx);
        let mut about = AboutWindow::default();
        about.lang = settings.language;
        let scanning = !folders.is_empty();
        let mut app = Self {
            folders,
            playlist: Playlist::new(settings.order),
            resume: settings.last_file.clone(),
            scanning,
            generation: 0,
            scans: scans.1,
            scan_tx: scans.0,
            scan_ctx,
            loader: Loader::new(ctx.clone()),
            current: None,
            upcoming: None,
            step: 0,
            fade: None,
            paused: false,
            interval: settings.interval,
            fit: settings.fit,
            video: settings.video,
            aspect: settings.aspect,
            on_top: settings.on_top,
            tray_only: settings.tray_only,
            tray: None,
            tray_retry: None,
            about,
            updates,
            hwnd,
            cloak,
            start_icon: None,
            pointer_moved: None,
            wheel: 0.0,
            menu_press: false,
            menu: crate::menu::MenuWindow::default(),
            dialog: Default::default(),
            modal: false,
            autostart: false,
        };
        app.make_tray();
        app
    }

    /// Show the window once it has painted `PAINTED_TO_SHOW` frames.
    fn uncloak(&mut self, ctx: &egui::Context) {
        let Some((until, frame)) = self.cloak else {
            return;
        };
        if ctx.cumulative_frame_nr() >= frame + PAINTED_TO_SHOW || Instant::now() >= until {
            if let Some(hwnd) = self.hwnd {
                crate::caption::cloak(hwnd, false);
                // eframe has shown the window by now, which gave it a
                // taskbar button again.
                if self.tray_only {
                    crate::win::set_taskbar_button(hwnd, false);
                }
            }
            self.cloak = None;
        } else {
            ctx.request_repaint();
        }
    }

    fn rescan(&mut self) {
        self.generation += 1;
        self.scanning = !self.folders.is_empty();
        if self.folders.is_empty() {
            self.playlist.set_items(Vec::new(), None);
            self.current = None;
            self.upcoming = None;
            return;
        }
        crate::library::scan(
            self.folders.clone(),
            self.generation,
            self.scan_tx.clone(),
            self.scan_ctx.clone(),
        );
    }

    fn add_folder(&mut self, folder: PathBuf) {
        if self
            .folders
            .iter()
            .any(|f| crate::library::same_path(f, &folder))
        {
            return;
        }
        self.folders.push(folder);
        crate::library::save(&self.folders);
        self.rescan();
    }

    /// The largest side pictures are decoded at: the screen's, within what
    /// the graphics card takes.
    fn max_side(&self, ctx: &egui::Context) -> u32 {
        let screen = self.hwnd.map_or(1920, crate::win::screen_side);
        let texture = ctx.input(|i| i.max_texture_side) as u32;
        screen.min(texture.max(256))
    }

    /// The file the slide on the screen shows, or the one to resume from.
    fn shown_path(&self) -> Option<PathBuf> {
        self.current
            .as_ref()
            .map(|s| s.item.path.clone())
            .or_else(|| self.resume.clone())
    }

    fn take_scans(&mut self) {
        let mut newest = None;
        while let Ok(scanned) = self.scans.try_recv() {
            if scanned.generation == self.generation {
                newest = Some(scanned);
            }
        }
        let Some(scanned) = newest else { return };
        self.scanning = false;
        let keep = self.shown_path();
        self.playlist.set_items(scanned.items, keep.as_deref());
        self.resume = None;
        // The upcoming slide may no longer be the next one.
        self.upcoming = None;
        if self.playlist.is_empty() {
            self.current = None;
        } else if self.current.is_some() {
            // The slide on the screen is where the playlist is now.
            if let Some(slide) = &mut self.current {
                slide.step = self.step;
            }
        }
    }

    /// What should be getting ready: the slide the show waits for, or the
    /// one after the slide on the screen.
    fn wanted(&self) -> Option<(Item, u64)> {
        let waiting = self.current.as_ref().is_none_or(|s| s.step != self.step);
        if waiting {
            return self.playlist.current().map(|i| (i.clone(), self.step));
        }
        let next = self.playlist.peek_next()?;
        // A single picture stays; it is not shown again.
        if self.playlist.len() == 1 && next.kind == Kind::Image {
            return None;
        }
        Some((next.clone(), self.step + 1))
    }

    fn prepare(&mut self, ctx: &egui::Context) {
        let Some((item, step)) = self.wanted() else {
            return;
        };
        let same = self
            .upcoming
            .as_ref()
            .is_some_and(|u| u.step == step && u.item.path == item.path);
        if same {
            return;
        }
        let prep = match item.kind {
            Kind::Image => Prep::Image(self.loader.request(item.path.clone(), self.max_side(ctx))),
            Kind::Video => {
                let side = self.max_side(ctx).min(VIDEO_SIDE);
                let looped = self.video == VideoLength::Interval;
                Prep::Video(Video::start(&item.path, side, looped, ctx.clone()))
            }
        };
        self.upcoming = Some(Upcoming { item, step, prep });
    }

    /// Take what the loader and a starting video have ready.
    fn poll_upcoming(&mut self, done: Vec<Done>, ctx: &egui::Context) {
        let Some(up) = &mut self.upcoming else { return };
        match &mut up.prep {
            Prep::Image(id) => {
                if let Some(d) = done.into_iter().find(|d| d.id == *id) {
                    up.prep = match d.result {
                        Ok(texture) => Prep::Ready(Content::Image(texture)),
                        Err(e) => Prep::Failed(e),
                    };
                }
            }
            Prep::Video(video) => match video.rx.try_recv() {
                Ok(VideoMsg::Frame(t, image)) => {
                    let texture = ctx.load_texture("video", image, TextureOptions::LINEAR);
                    let Prep::Video(video) =
                        std::mem::replace(&mut up.prep, Prep::Failed(String::new()))
                    else {
                        unreachable!()
                    };
                    let looped = self.video == VideoLength::Interval;
                    up.prep = Prep::Ready(Content::Video(Playing {
                        video,
                        texture,
                        first: t,
                        pending: None,
                        ended: false,
                        looped,
                    }));
                }
                Ok(VideoMsg::End) => up.prep = Prep::Failed("no frame".into()),
                Ok(VideoMsg::Failed(e)) => up.prep = Prep::Failed(e),
                Err(TryRecvError::Disconnected) => {
                    up.prep = Prep::Failed("the decoder stopped".into())
                }
                Err(TryRecvError::Empty) => {}
            },
            Prep::Ready(_) | Prep::Failed(_) => {}
        }
    }

    /// Put the upcoming slide on the screen once the show waits for it and
    /// it is ready; drop a file that cannot be shown.
    fn install(&mut self) {
        let waiting = self.current.as_ref().is_none_or(|s| s.step != self.step);
        if !waiting {
            return;
        }
        let Some(up) = self.upcoming.take_if(|u| u.step == self.step) else {
            return;
        };
        let still_there = self
            .playlist
            .current()
            .is_some_and(|i| i.path == up.item.path);
        match up.prep {
            _ if !still_there => {}
            Prep::Ready(content) => {
                if let Some(old) = self.current.take() {
                    self.fade = Some((old.content.texture().clone(), Instant::now()));
                }
                log::debug!("showing {}", up.item.path.display());
                self.current = Some(Slide {
                    item: up.item,
                    content,
                    clock: Clock::new(self.paused),
                    step: self.step,
                });
            }
            Prep::Failed(e) => {
                log::info!("cannot show {}: {e}", up.item.path.display());
                self.playlist.remove_current();
                self.step += 1;
                if self.playlist.is_empty() {
                    self.current = None;
                }
            }
            prep => self.upcoming = Some(Upcoming { prep, ..up }),
        }
    }

    /// Move on when the slide's time is up; the time until it is, if it
    /// runs on a clock.
    fn advance_if_due(&mut self) -> Option<Duration> {
        if self.paused {
            return None;
        }
        let slide = self.current.as_ref()?;
        if slide.step != self.step {
            return None;
        }
        let interval = Duration::from_secs(u64::from(self.interval));
        let elapsed = slide.clock.elapsed();
        let due = match &slide.content {
            Content::Image(_) => {
                if self.playlist.len() == 1 {
                    return None;
                }
                elapsed >= interval
            }
            Content::Video(p) => {
                let by_clock = self.video == VideoLength::Interval || p.looped;
                (p.ended && p.pending.is_none()) || (by_clock && elapsed >= interval)
            }
        };
        if due {
            self.next();
            return None;
        }
        match &slide.content {
            Content::Video(p) if !p.looped && self.video == VideoLength::Whole => None,
            _ => Some(interval.saturating_sub(elapsed)),
        }
    }

    /// The frames of the video on the screen that are due; the time until
    /// the next one is.
    fn pump(&mut self) -> Option<Duration> {
        let Some(Slide {
            content: Content::Video(playing),
            clock,
            ..
        }) = &mut self.current
        else {
            return None;
        };
        let was_ended = playing.ended;
        let wait = playing.pump(clock.elapsed());
        if self.paused {
            return None;
        }
        // Just ended: a frame to move on in, whoever calls.
        if playing.ended && !was_ended {
            Some(Duration::ZERO)
        } else {
            wait
        }
    }

    fn next(&mut self) {
        if self.playlist.advance().is_some() {
            self.step += 1;
        }
    }

    fn previous(&mut self) {
        if self.playlist.back().is_some() {
            self.step += 1;
        }
    }

    fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        if let Some(slide) = &mut self.current {
            slide.clock.pause(paused);
        }
    }

    fn menu_state(&self) -> crate::menu::State {
        crate::menu::State {
            has_file: self.current.is_some(),
            paused: self.paused,
            folders: self.folders.clone(),
            interval: self.interval,
            order: self.playlist.order(),
            fit: self.fit,
            video: self.video,
            aspect: self.aspect,
            on_top: self.on_top,
            autostart: self.autostart,
            tray_only: self.tray_only,
            update: self
                .updates
                .newer()
                .map(|t| crate::update::version_of(t).to_owned()),
            position: self.playlist.position(),
        }
    }

    /// Open the menu at `at` (screen pixels).
    fn open_menu(&mut self, at: (i32, i32)) {
        // Read once per opening: the Run key may have been changed
        // elsewhere (Task Manager's start-up list).
        self.autostart = crate::win::autostart();
        self.menu.open_at(at);
    }

    fn run(&mut self, cmd: Cmd, ctx: &egui::Context) {
        let file = self.current.as_ref().map(|s| s.item.path.clone());
        match cmd {
            Cmd::Previous => self.previous(),
            Cmd::Next => self.next(),
            Cmd::Pause => self.set_paused(!self.paused),
            Cmd::OpenFile => {
                if let Some(f) = file {
                    crate::win::shell_open(&f);
                }
            }
            Cmd::ShowInFolder => {
                if let Some(f) = file {
                    crate::win::show_in_folder(&f);
                }
            }
            Cmd::AddFolder => {
                // On a thread of its own: in a frame, it stopped the show
                // while it was open. The folder comes back in `ui`.
                let title = tr!("Add a folder to the library", "Добавить папку в библиотеку");
                self.dialog.start(ctx, self.hwnd, title.to_owned());
            }
            Cmd::OpenFolder(i) => {
                if let Some(f) = self.folders.get(i) {
                    crate::win::shell_open(f);
                }
            }
            Cmd::RemoveFolder(i) => {
                if i < self.folders.len() {
                    self.folders.remove(i);
                    crate::library::save(&self.folders);
                    self.rescan();
                }
            }
            Cmd::Rescan => self.rescan(),
            Cmd::Interval(n) => self.interval = n,
            Cmd::Order(o) => {
                self.playlist.set_order(o);
                self.upcoming = None;
                if let Some(slide) = &mut self.current {
                    slide.step = self.step;
                }
            }
            Cmd::Fit(f) => self.fit = f,
            Cmd::Video(v) => {
                self.video = v;
                // A video made ready for the other way is made again.
                if self
                    .upcoming
                    .as_ref()
                    .is_some_and(|u| u.item.kind == Kind::Video)
                {
                    self.upcoming = None;
                }
            }
            Cmd::Aspect(a) => {
                self.aspect = a;
                crate::win::set_kept_aspect(a);
                if let (Some(hwnd), Some(a)) = (self.hwnd, a) {
                    crate::win::apply_aspect(hwnd, a);
                }
            }
            Cmd::OnTop => {
                self.on_top = !self.on_top;
                let level = if self.on_top {
                    egui::WindowLevel::AlwaysOnTop
                } else {
                    egui::WindowLevel::Normal
                };
                ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(level));
            }
            Cmd::Autostart => {
                let on = !crate::win::autostart();
                if let Err(e) = crate::win::set_autostart(on) {
                    log::warn!("start with Windows: {e}");
                    crate::win::error_box(
                        crate::APP_NAME,
                        &tr!(
                            format!("Could not change starting with Windows: {e}"),
                            format!("Не удалось изменить запуск вместе с Windows: {e}")
                        ),
                    );
                }
                self.autostart = crate::win::autostart();
            }
            Cmd::TrayOnly => {
                self.tray_only = !self.tray_only;
                if let Some(hwnd) = self.hwnd {
                    crate::win::set_taskbar_button(hwnd, !self.tray_only);
                }
                if self.tray_only {
                    self.make_tray();
                } else {
                    self.tray = None;
                    self.tray_retry = None;
                }
            }
            Cmd::About => self.about.toggle(),
            Cmd::Update => {
                if let Some(tag) = self.updates.newer() {
                    crate::win::shell_open(crate::update::release_url(tag));
                }
            }
        }
    }

    /// The tray icon while "Only in the tray" is on and there is none yet.
    /// At sign-in the shell may refuse it: tried again every
    /// `TRAY_RETRY` (and when Explorer says it has started), since without
    /// it the widget has neither a taskbar button nor an icon.
    fn make_tray(&mut self) {
        let Some(hwnd) = self.hwnd.filter(|_| self.tray_only && self.tray.is_none()) else {
            return;
        };
        self.tray = crate::tray::Tray::new(hwnd, crate::APP_NAME);
        if self.tray.is_none() {
            log::info!("the tray icon was refused, trying again");
            self.tray_retry = Some(Instant::now() + TRAY_RETRY);
        } else {
            self.tray_retry = None;
        }
    }

    /// Try the refused tray icon again when it is time; the time until then.
    fn retry_tray(&mut self) -> Option<Duration> {
        let at = self.tray_retry?;
        if !self.tray_only || self.tray.is_some() {
            self.tray_retry = None;
            return None;
        }
        let now = Instant::now();
        if now < at {
            return Some(at - now);
        }
        self.make_tray();
        self.tray_retry.map(|at| at.saturating_duration_since(now))
    }

    /// What the window's subclass queued: the tray icon, Explorer.
    fn take_hooked(&mut self, ctx: &egui::Context) {
        for hooked in crate::win::take_hooked() {
            match hooked {
                crate::win::Hooked::Exit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                crate::win::Hooked::TrayCreated => match &mut self.tray {
                    Some(tray) => {
                        tray.add();
                    }
                    // Refused at start-up, Explorer not ready yet (a start
                    // with Windows): made now.
                    None => self.make_tray(),
                },
                crate::win::Hooked::TaskbarButton => {
                    if self.tray_only
                        && let Some(hwnd) = self.hwnd
                    {
                        crate::win::set_taskbar_button(hwnd, false);
                    }
                }
            }
        }
    }

    /// The keys of the widget (the About window takes its own).
    fn keys(&mut self, ctx: &egui::Context) {
        let pressed = |key| ctx.input_mut(|i| i.consume_key(Modifiers::NONE, key));
        if pressed(Key::ArrowRight) || pressed(Key::PageDown) {
            self.next();
        }
        if pressed(Key::ArrowLeft) || pressed(Key::PageUp) {
            self.previous();
        }
        if pressed(Key::Space) {
            self.set_paused(!self.paused);
        }
        if pressed(Key::F1) {
            self.about.toggle();
        }
        if pressed(Key::F5) {
            self.rescan();
        }
        if pressed(Key::Enter) {
            self.run(Cmd::OpenFile, ctx);
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::O)) {
            self.run(Cmd::AddFolder, ctx);
        }
    }

    /// Folders (or files: their folders) dropped on the window join the
    /// library.
    fn dropped(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        for path in dropped {
            let folder = if path.is_dir() {
                Some(path)
            } else {
                path.parent().map(PathBuf::from)
            };
            if let Some(folder) = folder {
                self.add_folder(folder);
            }
        }
    }

    /// The picture or the frame of `slide` in `area`, whole or filling it,
    /// at `opacity`.
    fn paint_texture(
        &self,
        painter: &egui::Painter,
        texture: &TextureHandle,
        area: Rect,
        opacity: f32,
    ) {
        let [w, h] = texture.size().map(|n| n.max(1) as f32);
        let (rect, uv) = placed(vec2(w, h), area, self.fit);
        let tint = Color32::from_white_alpha((opacity * 255.0).round() as u8);
        painter.image(texture.id(), rect, uv, tint);
    }

    /// The slide, the one fading out under it, and the buttons.
    fn paint(&mut self, ui: &mut egui::Ui) -> Option<Duration> {
        let ctx = ui.ctx().clone();
        let area = ui.max_rect();
        let painter = ui.painter().clone();
        painter.rect_filled(area, 0.0, Color32::BLACK);
        let mut wake: Option<Duration> = None;
        let mut soon = |d: Duration| wake = Some(wake.map_or(d, |w: Duration| w.min(d)));

        // The whole window moves it; the buttons, registered later, take
        // their clicks first.
        let drag = crate::frameless::drag_area(ui, area, ui.id().with("drag"), Sense::click());
        crate::frameless::resize_edges(ui, true);

        let fading = self.fade.as_ref().map(|(t, at)| (t.clone(), at.elapsed()));
        let t = match fading {
            Some((ref texture, since)) if since < FADE => {
                let t = since.as_secs_f32() / FADE.as_secs_f32();
                let t = t * t * (3.0 - 2.0 * t);
                self.paint_texture(&painter, texture, area, 1.0 - t);
                soon(Duration::from_millis(16));
                t
            }
            _ => {
                self.fade = None;
                1.0
            }
        };
        if let Some(slide) = &self.current {
            self.paint_texture(&painter, slide.content.texture(), area, t);
        } else if self.fade.is_none() {
            let hint = if self.folders.is_empty() {
                tr!(
                    "Right-click to add a folder with pictures, or drop a folder here",
                    "Щёлкните правой кнопкой, чтобы добавить папку с картинками, или перетащите папку сюда"
                )
            } else if self.scanning {
                tr!(
                    "Looking for pictures and videos…",
                    "Поиск картинок и видео…"
                )
            } else if self.playlist.all_failed() {
                tr!(
                    "None of the library's files could be opened",
                    "Не удалось открыть ни один файл библиотеки"
                )
            } else if self.playlist.is_empty() {
                tr!(
                    "No pictures or videos in the library's folders",
                    "В папках библиотеки нет картинок и видео"
                )
            } else {
                ""
            };
            if !hint.is_empty() {
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area));
                crate::ui::start::show(&mut child, &mut self.start_icon, hint);
            }
        }

        // The buttons, while the pointer moves over the window.
        let hovering = ctx
            .input(|i| i.pointer.hover_pos())
            .is_some_and(|p| area.contains(p));
        if ctx.input(|i| i.pointer.delta() != egui::Vec2::ZERO || i.pointer.any_down()) && hovering
        {
            self.pointer_moved = Some(Instant::now());
        }
        let recent = self
            .pointer_moved
            .is_some_and(|at| at.elapsed() < CONTROLS_FOR);
        if let Some(at) = self.pointer_moved.filter(|_| recent) {
            soon(CONTROLS_FOR.saturating_sub(at.elapsed()) + Duration::from_millis(20));
        }
        let shown = hovering && recent && self.current.is_some();
        let opacity = ctx.animate_bool_with_time(ui.id().with("controls"), shown, 0.2);
        if opacity > 0.0 && area.height() >= 70.0 && area.width() >= 120.0 {
            let radius = (area.height() * 0.09).clamp(13.0, 20.0);
            let y = area.bottom() - radius - (area.height() * 0.05).clamp(6.0, 16.0);
            let gap = radius * 2.6;
            let c = area.center().x;
            let back = crate::ui::round_button(
                ui,
                Icon::SkipBack,
                pos2(c - gap, y),
                radius,
                opacity,
                tr!("Previous (Left)", "Предыдущий (Влево)"),
            );
            let (icon, tip) = if self.paused {
                (Icon::Play, tr!("Go on (Space)", "Продолжить (Пробел)"))
            } else {
                (Icon::Pause, tr!("Pause (Space)", "Пауза (Пробел)"))
            };
            let pause = crate::ui::round_button(ui, icon, pos2(c, y), radius, opacity, tip);
            let forward = crate::ui::round_button(
                ui,
                Icon::SkipForward,
                pos2(c + gap, y),
                radius,
                opacity,
                tr!("Next (Right)", "Следующий (Вправо)"),
            );
            if back.clicked() {
                self.previous();
            }
            if pause.clicked() {
                self.set_paused(!self.paused);
            }
            if forward.clicked() {
                self.next();
            }
        } else if self.paused && self.current.is_some() {
            // Paused, the buttons away: a small mark in the corner says so.
            let r = 9.0;
            let center = Pos2::new(area.left() + r + 8.0, area.top() + r + 8.0);
            crate::ui::round_button(ui, Icon::Pause, center, r, 0.8, "");
        }

        // The cross that ends the program, in the top right corner, with
        // the pointer; also over the start screen, where there are no other
        // buttons. Just inside the corner's resize square, so that most of
        // the corner still sizes the window.
        let r = (area.height() * 0.06).clamp(10.0, 14.0);
        let center = pos2(area.right() - r - 10.0, area.top() + r + 10.0);
        // It stays while the pointer rests on it, moving or not.
        let on_cross = ctx
            .input(|i| i.pointer.hover_pos())
            .is_some_and(|p| p.distance(center) <= r + 4.0);
        let close_opacity = ctx.animate_bool_with_time(
            ui.id().with("close"),
            hovering && (recent || on_cross),
            0.2,
        );
        if close_opacity > 0.0 && area.height() >= 40.0 && area.width() >= 60.0 {
            let tip = tr!("Exit", "Выход");
            if crate::ui::close_button(ui, center, r, close_opacity, tip).clicked() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }

        if drag.double_clicked() {
            self.run(Cmd::OpenFile, &ctx);
        }
        // As Windows' menus do (metalmpv's): the right button opens the
        // menu when let go, however the pointer moves meanwhile (egui's
        // click allows 6 points: a click on the move was lost).
        let (right_pressed, right_down) = ctx.input(|i| {
            (
                i.pointer.button_pressed(egui::PointerButton::Secondary),
                i.pointer.button_down(egui::PointerButton::Secondary),
            )
        });
        if right_pressed {
            self.menu_press = drag.contains_pointer();
        }
        if self.menu_press && !right_down {
            self.menu_press = false;
            self.open_menu(crate::win::cursor_pos());
        }
        // The wheel goes through the files, a notch a file.
        if hovering {
            self.wheel += ctx.input(|i| {
                i.raw
                    .events
                    .iter()
                    .map(|e| match e {
                        egui::Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Point,
                            delta,
                            ..
                        } => delta.y / 50.0,
                        egui::Event::MouseWheel { delta, .. } => delta.y,
                        _ => 0.0,
                    })
                    .sum::<f32>()
            });
            if self.wheel.abs() >= 1.0 {
                if self.wheel > 0.0 {
                    self.previous();
                } else {
                    self.next();
                }
                self.wheel = 0.0;
            }
        }
        wake
    }

    fn tray_tip(&mut self) {
        let Some(tray) = &mut self.tray else { return };
        let name = self
            .current
            .as_ref()
            .and_then(|s| s.item.path.file_name())
            .map(|n| n.to_string_lossy());
        let tip = match name {
            Some(n) => format!("{} — {n}", crate::APP_NAME),
            None => crate::APP_NAME.to_owned(),
        };
        tray.set_tip(&tip);
    }
}

/// Where a picture of `size` goes in `area` and which part of it shows:
/// whole and centred (`Fit`), or over all of `area` with its edges cut
/// (`Fill`).
fn placed(size: egui::Vec2, area: Rect, fit: Fit) -> (Rect, Rect) {
    let full = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    let sx = area.width() / size.x;
    let sy = area.height() / size.y;
    match fit {
        Fit::Fit => {
            let s = sx.min(sy);
            (Rect::from_center_size(area.center(), size * s), full)
        }
        Fit::Fill => {
            let s = sx.max(sy);
            let shown = vec2(area.width() / (size.x * s), area.height() / (size.y * s));
            let uv = Rect::from_center_size(pos2(0.5, 0.5), shown);
            (area, uv)
        }
    }
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 1.0]
    }

    fn ui(&mut self, root_ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root_ui.ctx().clone();
        self.updates.poll();
        self.uncloak(&ctx);
        self.take_hooked(&ctx);
        self.take_scans();
        if !self.about.open {
            self.keys(&ctx);
        }
        self.dropped(&ctx);
        if let Some(folder) = self.dialog.take() {
            self.add_folder(folder);
        }

        let done = self.loader.take();
        self.poll_upcoming(done, &ctx);
        self.install();
        self.prepare(&ctx);
        // The video's frames first: its end, seen now, moves the show on in
        // this frame.
        let mut wake = self.pump();
        if let Some(d) = self.advance_if_due() {
            wake = Some(wake.map_or(d, |w| w.min(d)));
        }
        // A move asked for just now: the upcoming slide may be ready, and a
        // video put up shows its first frames (its thread, waiting with a
        // full channel, asks for no repaint).
        self.install();
        self.prepare(&ctx);
        if let Some(d) = self.pump() {
            wake = Some(wake.map_or(d, |w| w.min(d)));
        }

        let painted = egui::CentralPanel::no_frame()
            .show(root_ui, |ui| self.paint(ui))
            .inner;
        if let Some(d) = painted {
            wake = Some(wake.map_or(d, |w| w.min(d)));
        }
        let owner = self.hwnd;
        self.about.show(&ctx, owner, self.on_top, &mut self.updates);
        // About is modal: the widget takes no clicks while it is open, and
        // a click on it brings About forward (Windows does that for the
        // enabled window a disabled owner owns).
        if self.about.open != self.modal
            && let Some(hwnd) = self.hwnd
        {
            self.modal = self.about.open;
            crate::win::enable(hwnd, !self.modal);
        }
        let state = self.menu_state();
        let cmds = self.menu.show(&ctx, owner, &state);
        if !cmds.is_empty() {
            for cmd in cmds {
                self.run(cmd, &ctx);
            }
            // Taken after this frame's show: the next frame acts on them.
            ctx.request_repaint();
        }
        self.tray_tip();
        if let Some(d) = self.retry_tray() {
            wake = Some(wake.map_or(d, |w| w.min(d)));
        }
        if let Some(d) = wake {
            ctx.request_repaint_after(d);
        }
    }

    /// Called by eframe every `auto_save_interval` and on exit.
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        Settings {
            language: self.about.lang,
            check_updates: self.updates.enabled,
            last_update_check: self.updates.last_check,
            interval: self.interval,
            order: self.playlist.order(),
            fit: self.fit,
            video: self.video,
            aspect: self.aspect,
            on_top: self.on_top,
            tray_only: self.tray_only,
            last_file: self.shown_path(),
        }
        .save(storage);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_and_fill() {
        let area = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 200.0));
        // A square picture: whole, with bars left and right.
        let (rect, uv) = placed(vec2(100.0, 100.0), area, Fit::Fit);
        assert_eq!(
            rect,
            Rect::from_min_size(pos2(100.0, 0.0), vec2(200.0, 200.0))
        );
        assert_eq!(uv, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)));
        // Filling: the whole window, the top and the bottom quarter cut.
        let (rect, uv) = placed(vec2(100.0, 100.0), area, Fit::Fill);
        assert_eq!(rect, area);
        assert_eq!(uv, Rect::from_min_max(pos2(0.0, 0.25), pos2(1.0, 0.75)));
    }

    #[test]
    fn the_clock_stops_while_paused() {
        let mut clock = Clock::new(true);
        std::thread::sleep(Duration::from_millis(30));
        assert!(clock.elapsed() < Duration::from_millis(5));
        clock.pause(false);
        std::thread::sleep(Duration::from_millis(30));
        assert!(clock.elapsed() >= Duration::from_millis(25));
    }

    #[test]
    fn a_second_in_media_units() {
        assert_eq!(crate::media::SECOND, 10_000_000);
    }
}
