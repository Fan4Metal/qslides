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
use crate::show::{Caption, CaptionPart, Fit, Layer, Playlist, VideoLength};
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
/// The longer sides videos are decoded at (their processor scales them),
/// the least that covers the window: each frame is copied off the card,
/// converted and uploaded, 30 a second, and a 1920 frame in a widget of
/// 480 cost about ten times the work it showed. Steps, so that a window
/// sized a little does not make each video another size; never larger
/// than the last.
const VIDEO_SIDES: [u32; 5] = [640, 960, 1280, 1600, 1920];
/// How often a tray icon the shell refused is asked for again.
const TRAY_RETRY: Duration = Duration::from_secs(3);
/// A library folder that cannot be read is looked for again after this,
/// then after twice as long each time, `SCAN_RETRIES` times (30 s to 8 min,
/// a quarter of an hour in all); then only F5 or a change of the library.
const SCAN_RETRY: Duration = Duration::from_secs(30);
const SCAN_RETRIES: u32 = 5;

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
    /// A frame received but not yet due, and whether it begins a piece.
    pending: Option<(i64, ColorImage, bool)>,
    /// The decoder sent its last frame.
    ended: bool,
    /// The time it has by the clock: as long as a picture, its limit when
    /// shown to its end; `None`: to its end, however long.
    limit: Option<Duration>,
    /// A cut came: the next frame begins another piece of the video.
    cut: bool,
    /// It is shown in pieces (a cut has come): the decoder ends it when its
    /// time is up, the clock only if it falls behind.
    in_pieces: bool,
    /// The texture of the piece before, to fade out under the new one.
    faded: Option<TextureHandle>,
}

/// How much longer than its time the clock lets a video shown in pieces
/// run: the decoder ends it, after seeks that took some time.
const PIECES_GRACE: Duration = Duration::from_secs(5);

impl Playing {
    /// The time after which the clock moves the show on, if any.
    fn clock_limit(&self) -> Option<Duration> {
        let grace = if self.in_pieces {
            PIECES_GRACE
        } else {
            Duration::ZERO
        };
        self.limit.map(|l| l + grace)
    }

    /// Show the frame due at `elapsed`; the time until the next one, if
    /// one is waiting. Behind (the window dragged, a long frame), only the
    /// last of the frames due goes into the texture: each one set was
    /// uploaded, though only the last was seen. The first frame of another
    /// piece goes into a texture of its own, the old one left in `faded`.
    fn pump(&mut self, elapsed: Duration, ctx: &egui::Context) -> Option<Duration> {
        let now = self.first + (elapsed.as_nanos() / 100) as i64;
        let mut shown = None;
        let wait = loop {
            if self.pending.is_none() {
                match self.video.rx.try_recv() {
                    Ok(VideoMsg::Frame(t, image)) => {
                        self.pending = Some((t, image, std::mem::take(&mut self.cut)));
                    }
                    Ok(VideoMsg::Cut) => {
                        (self.cut, self.in_pieces) = (true, true);
                        continue;
                    }
                    Ok(VideoMsg::End) | Err(TryRecvError::Disconnected) => {
                        self.ended = true;
                        break None;
                    }
                    Ok(VideoMsg::Failed(e)) => {
                        log::info!("video stopped: {e}");
                        self.ended = true;
                        break None;
                    }
                    Err(TryRecvError::Empty) => break None,
                }
            }
            let due = self.pending.as_ref().map_or(i64::MAX, |(t, ..)| *t);
            if due > now {
                break Some(Duration::from_nanos(((due - now) * 100) as u64));
            }
            let Some((_, image, cut)) = self.pending.take() else {
                break None;
            };
            if cut {
                // The piece before ends on its last frame due.
                if let Some(last) = shown.take() {
                    self.texture.set(last, TextureOptions::LINEAR);
                }
                let new = ctx.load_texture("video", image, TextureOptions::LINEAR);
                self.faded = Some(std::mem::replace(&mut self.texture, new));
            } else {
                shown = Some(image);
            }
        };
        if let Some(image) = shown {
            self.texture.set(image, TextureOptions::LINEAR);
        }
        wait
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
    /// The library's folders the last scan could not read.
    unreachable: Vec<PathBuf>,
    /// When to scan again for them, and how many times it has been done.
    scan_retry: Option<Instant>,
    scan_tries: u32,
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
    /// Paused by the user (the button, Space).
    paused: bool,
    /// The session is locked, the display is off: the show is held, as
    /// if paused, while nobody can see it (the user's pause apart).
    locked: bool,
    display_off: bool,
    interval: u32,
    fit: Fit,
    video: VideoLength,
    /// Seconds a video shown to its end may take; 0: no limit.
    video_limit: u32,
    /// The pieces a video longer than its time is shown in.
    video_pieces: u32,
    caption: Caption,
    /// The dates files were taken, for `CaptionPart::Date`.
    dates: crate::dates::Dates,
    aspect: Option<(u32, u32)>,
    layer: Layer,
    /// Always on top, but under a full-screen program in front.
    yield_full_screen: bool,
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
        // Tooltips wrap within the menu's window (one width for all
        // windows; egui's 500 points went past its edge and were cut).
        ctx.all_styles_mut(|s| s.spacing.tooltip_width = crate::menu::TOOLTIP_WIDTH);
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
        if settings.layer != Layer::Normal {
            crate::win::set_on_desktop(settings.layer == Layer::Desktop);
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(settings.layer.level()));
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
            unreachable: Vec::new(),
            scan_retry: None,
            scan_tries: 0,
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
            locked: false,
            display_off: false,
            interval: settings.interval,
            fit: settings.fit,
            video: settings.video,
            video_limit: settings.video_limit,
            video_pieces: settings.video_pieces,
            caption: settings.caption,
            dates: crate::dates::Dates::new(ctx.clone()),
            aspect: settings.aspect,
            layer: settings.layer,
            yield_full_screen: settings.yield_full_screen,
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
        app.watch_full_screen();
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

    /// Scan the library again, as the user asked or the library changed:
    /// folders that cannot be read are looked for again as many times.
    fn rescan(&mut self) {
        self.scan_tries = 0;
        self.start_scan();
    }

    fn start_scan(&mut self) {
        self.generation += 1;
        self.scanning = !self.folders.is_empty();
        self.scan_retry = None;
        if self.folders.is_empty() {
            self.playlist.set_items(Vec::new(), None);
            self.current = None;
            self.upcoming = None;
            self.unreachable.clear();
            return;
        }
        crate::library::scan(
            self.folders.clone(),
            self.generation,
            self.scan_tx.clone(),
            self.scan_ctx.clone(),
        );
    }

    /// Add `folders` (those not in the library yet) to the library: saved
    /// once and scanned once, however many were dropped together.
    fn add_folders(&mut self, folders: impl IntoIterator<Item = PathBuf>) {
        let mut added = false;
        for folder in folders {
            if !self
                .folders
                .iter()
                .any(|f| crate::library::same_path(f, &folder))
            {
                self.folders.push(folder);
                added = true;
            }
        }
        if added {
            crate::library::save(&self.folders);
            self.rescan();
        }
    }

    /// The largest side pictures are decoded at: the screen's, within what
    /// the graphics card takes.
    fn max_side(&self, ctx: &egui::Context) -> u32 {
        let screen = self.hwnd.map_or(1920, crate::win::screen_side);
        let texture = ctx.input(|i| i.max_texture_side) as u32;
        screen.min(texture.max(256))
    }

    /// The longer side a video is decoded at for the window as it is now
    /// (a window made larger has its next video larger).
    fn video_side(&self, ctx: &egui::Context) -> u32 {
        let rect = ctx.content_rect();
        let long = rect.width().max(rect.height()) * ctx.pixels_per_point();
        video_side(long, self.fit, self.max_side(ctx))
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
        // The scan of this generation was of the folders as they are.
        self.unreachable = (scanned.unreachable.iter())
            .filter_map(|&i| self.folders.get(i).cloned())
            .collect();
        if !self.unreachable.is_empty() && self.scan_tries < SCAN_RETRIES {
            // A network drive at sign-in, before the network: looked for
            // again, less and less often.
            let wait = SCAN_RETRY * 2u32.pow(self.scan_tries);
            self.scan_tries += 1;
            self.scan_retry = Some(Instant::now() + wait);
        }
        let keep = self.shown_path();
        let kept = self.playlist.set_items(scanned.items, keep.as_deref());
        self.resume = None;
        // The upcoming slide may no longer be the next one.
        self.upcoming = None;
        if self.playlist.is_empty() {
            self.current = None;
        } else if let Some(slide) = &mut self.current {
            if kept {
                // The slide on the screen is where the playlist is now.
                slide.step = self.step;
            } else {
                // Its file is no longer in the library: it stays until the
                // playlist's first is ready, which is not skipped.
                self.step += 1;
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
                let side = self.video_side(ctx);
                let plan = crate::media::Plan {
                    looped: self.video == VideoLength::Interval,
                    time: (self.video_time()).map(|t| (t.as_nanos() / 100) as i64),
                    pieces: self.video_pieces,
                };
                Prep::Video(Video::start(&item.path, side, plan, ctx.clone()))
            }
        };
        self.upcoming = Some(Upcoming { item, step, prep });
    }

    /// Take what the loader and a starting video have ready.
    fn poll_upcoming(&mut self, done: Vec<Done>, ctx: &egui::Context) {
        let limit = self.video_time();
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
                    up.prep = Prep::Ready(Content::Video(Playing {
                        video,
                        texture,
                        first: t,
                        pending: None,
                        ended: false,
                        limit,
                        cut: false,
                        in_pieces: false,
                        faded: None,
                    }));
                }
                // Not before a first frame.
                Ok(VideoMsg::Cut) => {}
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
                    clock: Clock::new(self.halted()),
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
        if self.halted() {
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
                (p.ended && p.pending.is_none()) || p.clock_limit().is_some_and(|l| elapsed >= l)
            }
        };
        if due {
            self.next();
            return None;
        }
        match &slide.content {
            Content::Video(p) => p.clock_limit().map(|l| l.saturating_sub(elapsed)),
            Content::Image(_) => Some(interval.saturating_sub(elapsed)),
        }
    }

    /// A video made ready for other settings is made again.
    fn drop_upcoming_video(&mut self) {
        if (self.upcoming.as_ref()).is_some_and(|u| u.item.kind == Kind::Video) {
            self.upcoming = None;
        }
    }

    /// The time a video starting now has: as long as a picture, or its
    /// limit when shown to its end; `None`: to its end.
    fn video_time(&self) -> Option<Duration> {
        let secs = match self.video {
            VideoLength::Interval => self.interval,
            VideoLength::Whole => self.video_limit,
        };
        (secs > 0).then(|| Duration::from_secs(u64::from(secs)))
    }

    /// The frames of the video on the screen that are due; the time until
    /// the next one is.
    fn pump(&mut self, ctx: &egui::Context) -> Option<Duration> {
        let Some(Slide {
            content: Content::Video(playing),
            clock,
            ..
        }) = &mut self.current
        else {
            return None;
        };
        let was_ended = playing.ended;
        let wait = playing.pump(clock.elapsed(), ctx);
        // Another piece: the one before fades out under it, as a slide does.
        if let Some(old) = playing.faded.take() {
            self.fade = Some((old, Instant::now()));
        }
        if self.paused || self.locked || self.display_off {
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
        self.hold_clock();
    }

    /// The show stands still: paused by the user, or held while nobody
    /// can see it.
    fn halted(&self) -> bool {
        self.paused || self.locked || self.display_off
    }

    /// The slide's clock stopped or going, as the show is.
    fn hold_clock(&mut self) {
        let halted = self.halted();
        if let Some(slide) = &mut self.current {
            slide.clock.pause(halted);
        }
    }

    fn menu_state(&self) -> crate::menu::State {
        crate::menu::State {
            has_file: self.current.is_some(),
            paused: self.paused,
            folders: self.folders.clone(),
            unreachable: (self.folders.iter())
                .map(|f| (self.unreachable.iter()).any(|u| crate::library::same_path(f, u)))
                .collect(),
            interval: self.interval,
            order: self.playlist.order(),
            fit: self.fit,
            video: self.video,
            video_limit: self.video_limit,
            video_pieces: self.video_pieces,
            caption: self.caption,
            aspect: self.aspect,
            layer: self.layer,
            yield_full_screen: self.yield_full_screen,
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
                // The playlist keeps its current file: the slide on the
                // screen, or the one the show waits for, which it still
                // does (a slide's step is left as it is).
                self.playlist.set_order(o);
                self.upcoming = None;
            }
            Cmd::Fit(f) => self.fit = f,
            Cmd::Caption(c) => self.caption = c,
            Cmd::Video(v) => {
                self.video = v;
                self.drop_upcoming_video();
            }
            Cmd::VideoLimit(secs) => {
                self.video_limit = secs;
                self.drop_upcoming_video();
            }
            Cmd::VideoPieces(n) => {
                self.video_pieces = n;
                self.drop_upcoming_video();
            }
            Cmd::Aspect(a) => {
                self.aspect = a;
                crate::win::set_kept_aspect(a);
                if let (Some(hwnd), Some(a)) = (self.hwnd, a) {
                    crate::win::apply_aspect(hwnd, a);
                }
            }
            Cmd::Layer(layer) => {
                // Above all windows and on the desktop exclude each other:
                // one switched off, the window is as any other.
                self.layer = if self.layer == layer {
                    Layer::Normal
                } else {
                    layer
                };
                crate::win::set_on_desktop(self.layer == Layer::Desktop);
                // Before winit sets the level, at the end of the frame.
                self.watch_full_screen();
                ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(self.layer.level()));
            }
            Cmd::YieldFullScreen => {
                self.yield_full_screen = !self.yield_full_screen;
                self.watch_full_screen();
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

    /// Give way to full-screen programs while always on top, if the user
    /// wants it (`win::set_yield`).
    fn watch_full_screen(&self) {
        if let Some(hwnd) = self.hwnd {
            let on = self.layer == Layer::Top && self.yield_full_screen;
            crate::win::set_yield(hwnd, on);
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

    /// Scan again for folders that could not be read, when it is time; the
    /// time until then.
    fn retry_scan(&mut self) -> Option<Duration> {
        let at = self.scan_retry?;
        let now = Instant::now();
        if now < at {
            return Some(at - now);
        }
        log::info!("looking again for the folders that could not be read");
        self.start_scan();
        None
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
                // Nobody sees the show: it stands still, decoding nothing
                // (a video's thread waits on its full channel), and goes on
                // where it was.
                crate::win::Hooked::Locked(on) => {
                    log::info!("session {}", if on { "locked" } else { "unlocked" });
                    self.locked = on;
                    self.hold_clock();
                }
                crate::win::Hooked::DisplayOff(off) => {
                    log::info!("display {}", if off { "off" } else { "on" });
                    self.display_off = off;
                    self.hold_clock();
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
        if dropped.is_empty() {
            return;
        }
        let folders = dropped.into_iter().filter_map(|path| {
            if path.is_dir() {
                Some(path)
            } else {
                path.parent().map(PathBuf::from)
            }
        });
        self.add_folders(folders.collect::<Vec<_>>());
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
        let shown = self.current.as_ref().map(|s| s.content.texture().clone());
        let t = match fading {
            Some((ref old, since)) if since < FADE => {
                let t = since.as_secs_f32() / FADE.as_secs_f32();
                let t = t * t * (3.0 - 2.0 * t);
                match &shown {
                    // The old picture whole under the new one coming in:
                    // t·new + (1−t)·old. Faded out as well (1−t) under
                    // t·new, they summed to less, the change dimmed by a
                    // quarter in its middle. Where the new one does not
                    // reach (the bars of a picture fitted whole), the old
                    // one darkens.
                    Some(new) => {
                        self.paint_texture(&painter, old, area, 1.0);
                        let [w, h] = new.size().map(|n| n.max(1) as f32);
                        let (rect, _) = placed(vec2(w, h), area, self.fit);
                        let dark = Color32::from_black_alpha((t * 255.0).round() as u8);
                        for bar in outside(area, rect) {
                            painter.rect_filled(bar, 0.0, dark);
                        }
                    }
                    None => self.paint_texture(&painter, old, area, 1.0 - t),
                }
                soon(Duration::from_millis(16));
                t
            }
            _ => {
                self.fade = None;
                1.0
            }
        };
        if let Some(texture) = &shown {
            self.paint_texture(&painter, texture, area, t);
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
            } else if self.playlist.is_empty() && !self.unreachable.is_empty() {
                tr!(
                    "The library's folders cannot be read now",
                    "Папки библиотеки сейчас недоступны"
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
        // The caption, in the bottom left corner, coming in with its slide;
        // it gives way to the buttons, which take the bottom.
        if let Some(text) = self.caption_text() {
            paint_caption(&painter, area, &text, t * (1.0 - opacity));
        }
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
            crate::ui::round_mark(ui, Icon::Pause, center, r, 0.8);
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

    /// The caption of the slide on the screen: the parts switched on that
    /// it has, joined by a dot (a date once it has been read); `None` when
    /// there are none.
    fn caption_text(&mut self) -> Option<String> {
        if self.caption.is_empty() {
            return None;
        }
        let path = self.current.as_ref()?.item.path.clone();
        let lossy = |s: &std::ffi::OsStr| s.to_string_lossy().into_owned();
        let parts: Vec<String> = (self.caption.parts())
            .filter_map(|part| match part {
                CaptionPart::Folder => path.parent().and_then(|p| p.file_name()).map(lossy),
                CaptionPart::Name => path.file_stem().map(lossy),
                CaptionPart::Date => self.dates.get(&path).map(crate::dates::label),
            })
            .collect();
        (!parts.is_empty()).then(|| parts.join(" · "))
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

/// The caption `text` in the bottom left corner of `area`, white on a dark
/// band, cut with an ellipsis where it is too long, at `opacity`; none in
/// a window too small for it.
fn paint_caption(painter: &egui::Painter, area: Rect, text: &str, opacity: f32) {
    if opacity <= 0.01 || text.is_empty() || area.height() < 60.0 || area.width() < 100.0 {
        return;
    }
    let size = (area.height() * 0.045).clamp(11.0, 16.0);
    let margin = (area.height() * 0.03).clamp(6.0, 12.0);
    let pad = vec2(size * 0.5, size * 0.2);
    let format = egui::TextFormat {
        font_id: egui::FontId::proportional(size),
        color: Color32::WHITE.gamma_multiply(opacity),
        ..Default::default()
    };
    let mut job = egui::text::LayoutJob::single_section(text.to_owned(), format);
    job.wrap = egui::text::TextWrapping {
        max_width: (area.width() - 2.0 * (margin + pad.x)).max(10.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let galley = painter.layout_job(job);
    let size_with_pad = galley.size() + 2.0 * pad;
    let rect = Rect::from_min_size(
        pos2(
            area.left() + margin,
            area.bottom() - margin - size_with_pad.y,
        ),
        size_with_pad,
    );
    let band = Color32::from_black_alpha((140.0 * opacity).round() as u8);
    painter.rect_filled(rect, size * 0.35, band);
    painter.galley(rect.min + pad, galley, Color32::WHITE);
}

/// The longer side to decode a video at for a window whose longer side is
/// `window` pixels, within `max`. Fitted whole, a video shows no longer
/// than the window's longer side; filling it, it may be cut, and twice
/// that covers videos up to 2:1 in any window.
fn video_side(window: f32, fit: Fit, max: u32) -> u32 {
    let need = match fit {
        Fit::Fit => window,
        Fit::Fill => window * 2.0,
    };
    let last = VIDEO_SIDES[VIDEO_SIDES.len() - 1];
    VIDEO_SIDES
        .into_iter()
        .find(|&s| s as f32 >= need)
        .unwrap_or(last)
        .min(max)
}

/// The parts of `area` outside `inner`: the bars above and below it across
/// the whole width, those left and right of it between them; empty ones
/// left out.
fn outside(area: Rect, inner: Rect) -> Vec<Rect> {
    let inner = inner.intersect(area);
    if !inner.is_positive() {
        return vec![area];
    }
    [
        Rect::from_min_max(area.min, pos2(area.max.x, inner.min.y)),
        Rect::from_min_max(pos2(area.min.x, inner.max.y), area.max),
        Rect::from_min_max(
            pos2(area.min.x, inner.min.y),
            pos2(inner.min.x, inner.max.y),
        ),
        Rect::from_min_max(
            pos2(inner.max.x, inner.min.y),
            pos2(area.max.x, inner.max.y),
        ),
    ]
    .into_iter()
    .filter(|r| r.is_positive())
    .collect()
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
            self.add_folders([folder]);
        }

        let done = self.loader.take();
        self.poll_upcoming(done, &ctx);
        self.install();
        self.prepare(&ctx);
        // The video's frames first: its end, seen now, moves the show on in
        // this frame.
        let mut wake = self.pump(&ctx);
        if let Some(d) = self.advance_if_due() {
            wake = Some(wake.map_or(d, |w| w.min(d)));
        }
        // A move asked for just now: the upcoming slide may be ready, and a
        // video put up shows its first frames (its thread, waiting with a
        // full channel, asks for no repaint).
        self.install();
        self.prepare(&ctx);
        if let Some(d) = self.pump(&ctx) {
            wake = Some(wake.map_or(d, |w| w.min(d)));
        }
        // The next slide's date read ahead, ready when it comes.
        if self.caption.has(CaptionPart::Date)
            && let Some(up) = &self.upcoming
        {
            let path = up.item.path.clone();
            self.dates.get(&path);
        }

        let painted = egui::CentralPanel::no_frame()
            .show(root_ui, |ui| self.paint(ui))
            .inner;
        if let Some(d) = painted {
            wake = Some(wake.map_or(d, |w| w.min(d)));
        }
        let owner = self.hwnd;
        // On the desktop, About and the menu are above the others all the
        // same: they would open under them.
        let above = self.layer != Layer::Normal;
        self.about.show(&ctx, owner, above, &mut self.updates);
        // About is modal: the widget takes no clicks while it is open, and
        // a click on it brings About forward (Windows does that for the
        // enabled window a disabled owner owns).
        if self.about.open != self.modal
            && let Some(hwnd) = self.hwnd
        {
            self.modal = self.about.open;
            crate::win::enable(hwnd, !self.modal);
        }
        // Gathered only while the menu is open (it copies the folders).
        let cmds = if self.menu.open {
            let state = self.menu_state();
            self.menu.show(&ctx, owner, &state)
        } else {
            Vec::new()
        };
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
        if let Some(d) = self.retry_scan() {
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
            video_limit: self.video_limit,
            video_pieces: self.video_pieces,
            caption: self.caption,
            aspect: self.aspect,
            layer: self.layer,
            yield_full_screen: self.yield_full_screen,
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
    fn videos_decoded_for_the_window() {
        // The widget's default size: a step above its 480.
        assert_eq!(video_side(480.0, Fit::Fit, 3840), 640);
        assert_eq!(video_side(480.0, Fit::Fill, 3840), 960);
        assert_eq!(video_side(1000.0, Fit::Fit, 3840), 1280);
        // Never above the last step, nor the screen's side.
        assert_eq!(video_side(3000.0, Fit::Fit, 3840), 1920);
        assert_eq!(video_side(1000.0, Fit::Fill, 1366), 1366);
    }

    #[test]
    fn bars_around_a_picture() {
        let area = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 200.0));
        // Fitted whole, bars left and right.
        let inner = Rect::from_min_size(pos2(100.0, 0.0), vec2(200.0, 200.0));
        assert_eq!(
            outside(area, inner),
            [
                Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 200.0)),
                Rect::from_min_max(pos2(300.0, 0.0), pos2(400.0, 200.0)),
            ]
        );
        // Filling the window: none.
        assert!(outside(area, area).is_empty());
        // Bars all round: their area and the picture's make the window's.
        let inner = Rect::from_min_size(pos2(50.0, 20.0), vec2(100.0, 60.0));
        let bars: f32 = outside(area, inner).iter().map(|r| r.area()).sum();
        assert_eq!(bars + inner.area(), area.area());
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
