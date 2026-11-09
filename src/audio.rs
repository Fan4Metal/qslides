//! The sound of a video, when the user has turned it up (`volume` above
//! 0): the Media Foundation Media Engine in its audio-only mode, which
//! decodes the sound stream, converts it for the default output device and
//! plays it there. The picture is decoded apart (`media.rs`), so the two
//! are kept together by the widget: the sound starts when the video's slide
//! comes on the screen, pauses with the show, jumps with the video to each
//! of its pieces (`seek`) and plays round again with a looped video.
//!
//! Each sound has a thread of its own that owns the engine (made and
//! used in one place) and waits for commands and the engine's events in
//! one channel; dropping [`Audio`] shuts the engine down and ends it.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::mpsc::{Sender, channel};

use windows::Win32::Media::MediaFoundation::{
    CLSID_MFMediaEngineClassFactory, IMFAttributes, IMFMediaEngine, IMFMediaEngineClassFactory,
    IMFMediaEngineNotify, IMFMediaEngineNotify_Impl, MF_MEDIA_ENGINE_AUDIOONLY,
    MF_MEDIA_ENGINE_CALLBACK, MF_MEDIA_ENGINE_EVENT_ERROR, MF_MEDIA_ENGINE_EVENT_LOADEDMETADATA,
    MF_MEDIA_ENGINE_EVENT_PLAYING, MF_MEDIA_ENGINE_PRELOAD_AUTOMATIC, MF_VERSION,
    MFCreateAttributes, MFSTARTUP_FULL, MFShutdown, MFStartup,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::core::{BSTR, implement};

use crate::media::Com;

/// What the thread of a sound is told, by the widget or by its engine.
enum Msg {
    Play,
    Pause,
    /// Go to this many seconds from the start.
    Seek(f64),
    Volume(f64),
    /// An event of the engine (`MF_MEDIA_ENGINE_EVENT`) and its two values.
    Event(u32, usize, u32),
    Stop,
}

/// The engine's callback: its events go to the thread's channel.
#[implement(IMFMediaEngineNotify)]
struct Notify {
    tx: Sender<Msg>,
}

impl IMFMediaEngineNotify_Impl for Notify_Impl {
    fn EventNotify(&self, event: u32, param1: usize, param2: u32) -> windows::core::Result<()> {
        let _ = self.tx.send(Msg::Event(event, param1, param2));
        Ok(())
    }
}

/// The volume the engine is given for the menu's `percent` (0 to 100):
/// squared, as loudness is heard, so that the slider's middle is about
/// half as loud rather than a little quieter.
pub fn gain(percent: u32) -> f64 {
    let p = f64::from(percent.min(100)) / 100.0;
    p * p
}

/// The sound of a video playing, or ready to. Dropped, it stops.
pub struct Audio {
    tx: Sender<Msg>,
    /// The engine has begun to play (for the tests).
    #[cfg_attr(not(test), allow(dead_code))]
    playing: Arc<AtomicBool>,
}

impl Drop for Audio {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Stop);
    }
}

impl Audio {
    /// Open the sound of `path`, paused at its start until [`Audio::play`];
    /// `looped`: played round again at its end. A video without sound, or
    /// one the engine cannot open, stays silent.
    pub fn start(path: &Path, looped: bool, volume: f64) -> Audio {
        let (tx, rx) = channel();
        let playing = Arc::new(AtomicBool::new(false));
        let path = path.to_path_buf();
        let (notify, started) = (tx.clone(), playing.clone());
        let spawned = std::thread::Builder::new()
            .name("audio".into())
            .spawn(move || {
                let _com = Com::init();
                if let Err(e) = unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) } {
                    log::info!("no sound: MFStartup: {e}");
                    return;
                }
                match open(&path, looped, volume, notify) {
                    Ok(engine) => {
                        run(&engine, &rx, looped, &started);
                        unsafe {
                            let _ = engine.Shutdown();
                        }
                    }
                    Err(e) => log::info!("{}: no sound: {e}", path.display()),
                }
                unsafe {
                    let _ = MFShutdown();
                }
            });
        if let Err(e) = spawned {
            log::warn!("cannot start a sound thread: {e}");
        }
        Audio { tx, playing }
    }

    pub fn play(&self) {
        let _ = self.tx.send(Msg::Play);
    }

    pub fn pause(&self) {
        let _ = self.tx.send(Msg::Pause);
    }

    /// Go to `seconds` from the start of the video (round again within it
    /// when looped).
    pub fn seek(&self, seconds: f64) {
        let _ = self.tx.send(Msg::Seek(seconds));
    }

    pub fn set_volume(&self, volume: f64) {
        let _ = self.tx.send(Msg::Volume(volume));
    }

    /// Whether the engine has begun to play.
    #[cfg(test)]
    pub fn playing(&self) -> bool {
        self.playing.load(Relaxed)
    }
}

/// The engine, audio only, with `path` loading.
fn open(
    path: &Path,
    looped: bool,
    volume: f64,
    tx: Sender<Msg>,
) -> windows::core::Result<IMFMediaEngine> {
    let factory: IMFMediaEngineClassFactory =
        unsafe { CoCreateInstance(&CLSID_MFMediaEngineClassFactory, None, CLSCTX_INPROC_SERVER)? };
    let mut attributes: Option<IMFAttributes> = None;
    unsafe { MFCreateAttributes(&mut attributes, 1)? };
    let attributes = attributes.ok_or(windows::core::Error::from(
        windows::Win32::Foundation::E_FAIL,
    ))?;
    let notify: IMFMediaEngineNotify = Notify { tx }.into();
    unsafe {
        attributes.SetUnknown(&MF_MEDIA_ENGINE_CALLBACK, &notify)?;
        let engine = factory.CreateInstance(MF_MEDIA_ENGINE_AUDIOONLY.0 as u32, &attributes)?;
        engine.SetLoop(looped)?;
        engine.SetVolume(volume)?;
        // Loaded now, not when it is played: it is made while the slide
        // before is still up, so that it starts with its video.
        engine.SetPreload(MF_MEDIA_ENGINE_PRELOAD_AUTOMATIC)?;
        engine.SetSource(&BSTR::from(path.to_string_lossy().as_ref()))?;
        engine.Load()?;
        Ok(engine)
    }
}

/// The commands until [`Msg::Stop`]. Playing and seeking wait for the
/// engine to know the video's length (`LOADEDMETADATA`), so that a seek
/// is not lost and the sound does not begin before it.
fn run(
    engine: &IMFMediaEngine,
    rx: &std::sync::mpsc::Receiver<Msg>,
    looped: bool,
    started: &AtomicBool,
) {
    let mut ready = false;
    let mut wanted = false;
    let mut playing = false;
    let mut seek = None::<f64>;
    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::Stop => return,
            Msg::Play => wanted = true,
            Msg::Pause => wanted = false,
            Msg::Seek(at) => seek = Some(at),
            Msg::Volume(v) => unsafe {
                let _ = engine.SetVolume(v);
            },
            Msg::Event(e, ..) if e == MF_MEDIA_ENGINE_EVENT_LOADEDMETADATA.0 as u32 => ready = true,
            Msg::Event(e, ..) if e == MF_MEDIA_ENGINE_EVENT_PLAYING.0 as u32 => {
                started.store(true, Relaxed);
            }
            Msg::Event(e, code, hr) if e == MF_MEDIA_ENGINE_EVENT_ERROR.0 as u32 => {
                // A video without sound: nothing to play.
                log::info!("no sound: error {code}, {:#010x}", hr);
                return;
            }
            Msg::Event(..) => continue,
        }
        if !ready {
            continue;
        }
        if let Some(at) = seek.take() {
            let length = unsafe { engine.GetDuration() };
            let at = if looped && length.is_finite() && length > 0.0 {
                at % length
            } else {
                at
            };
            unsafe {
                let _ = engine.SetCurrentTime(at.max(0.0));
            }
        }
        if wanted != playing {
            playing = wanted;
            unsafe {
                let _ = if wanted {
                    engine.Play()
                } else {
                    engine.Pause()
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn loudness() {
        assert_eq!(gain(0), 0.0);
        assert_eq!(gain(100), 1.0);
        assert_eq!(gain(50), 0.25);
        assert_eq!(gain(500), 1.0);
    }

    /// The sound of the video named by `QSLIDES_VIDEO`, played for three
    /// seconds from its tenth (it is heard).
    #[test]
    #[ignore]
    fn video_sound() {
        let path = std::env::var("QSLIDES_VIDEO").expect("QSLIDES_VIDEO");
        let audio = Audio::start(Path::new(&path), false, gain(40));
        audio.seek(10.0);
        audio.play();
        let start = Instant::now();
        while !audio.playing() && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(audio.playing(), "the sound did not start");
        println!(
            "playing after {:.0} ms",
            start.elapsed().as_secs_f64() * 1000.0
        );
        std::thread::sleep(Duration::from_secs(3));
    }
}
