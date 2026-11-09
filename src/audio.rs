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
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, OnceLock};

use windows::Win32::Media::MediaFoundation::{
    CLSID_MFMediaEngineClassFactory, IMFActivate, IMFAttributes, IMFMediaEngine,
    IMFMediaEngineClassFactory, IMFMediaEngineNotify, IMFMediaEngineNotify_Impl,
    MF_E_INVALIDSTREAMNUMBER, MF_MEDIA_ENGINE_AUDIOONLY, MF_MEDIA_ENGINE_CALLBACK,
    MF_MEDIA_ENGINE_EVENT_ERROR, MF_MEDIA_ENGINE_EVENT_LOADEDMETADATA,
    MF_MEDIA_ENGINE_EVENT_PLAYING, MF_MEDIA_ENGINE_PRELOAD_AUTOMATIC, MF_MT_SUBTYPE,
    MF_SOURCE_READER_FIRST_AUDIO_STREAM, MF_VERSION, MFAudioFormat_AAC, MFAudioFormat_ALAC,
    MFAudioFormat_AMR_NB, MFAudioFormat_AMR_WB, MFAudioFormat_DTS, MFAudioFormat_Dolby_AC3,
    MFAudioFormat_Dolby_AC3_SPDIF, MFAudioFormat_Dolby_DDPlus, MFAudioFormat_FLAC,
    MFAudioFormat_Float, MFAudioFormat_MP3, MFAudioFormat_MPEG, MFAudioFormat_Opus,
    MFAudioFormat_PCM, MFAudioFormat_Vorbis, MFCreateAttributes, MFCreateSourceReaderFromURL,
    MFMediaType_Audio, MFSTARTUP_FULL, MFShutdown, MFStartup, MFT_CATEGORY_AUDIO_DECODER,
    MFT_ENUM_FLAG_ASYNCMFT, MFT_ENUM_FLAG_LOCALMFT, MFT_ENUM_FLAG_SORTANDFILTER,
    MFT_ENUM_FLAG_SYNCMFT, MFT_ENUM_FLAG_UNTRUSTED_STOREMFT, MFT_REGISTER_TYPE_INFO, MFTEnumEx,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
use windows::core::{BSTR, GUID, PCWSTR, implement};

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
    /// The video has sound that Windows has no decoder for: its format.
    undecodable: Arc<OnceLock<String>>,
}

impl Drop for Audio {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Stop);
    }
}

impl Audio {
    /// Open the sound of `path`, paused at its start until [`Audio::play`];
    /// `looped`: played round again at its end. A video without sound, or
    /// one the engine cannot open, stays silent; one whose sound Windows
    /// cannot decode says so ([`Audio::undecodable`]).
    pub fn start(path: &Path, looped: bool, volume: f64) -> Audio {
        let (tx, rx) = channel();
        let playing = Arc::new(AtomicBool::new(false));
        let undecodable = Arc::new(OnceLock::new());
        let path = path.to_path_buf();
        let (notify, started, format) = (tx.clone(), playing.clone(), undecodable.clone());
        let spawned = std::thread::Builder::new()
            .name("audio".into())
            .spawn(move || {
                let _com = Com::init();
                if let Err(e) = unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) } {
                    log::warn!("no sound: MFStartup: {e}");
                    return;
                }
                match stream(&path) {
                    Stream::None => log::debug!("{}: no sound stream", path.display()),
                    Stream::Undecodable(name) => {
                        log::info!("{}: no decoder for its sound, {name}", path.display());
                        let _ = format.set(name);
                    }
                    Stream::Decodable => match open(&path, looped, volume, notify) {
                        Ok(engine) => {
                            log::debug!("{}: sound loading", path.display());
                            run(&engine, &rx, looped, &started, &path);
                            unsafe {
                                let _ = engine.Shutdown();
                            }
                        }
                        Err(e) => log::warn!("{}: no sound: {e}", path.display()),
                    },
                }
                unsafe {
                    let _ = MFShutdown();
                }
            });
        if let Err(e) = spawned {
            log::warn!("cannot start a sound thread: {e}");
        }
        Audio {
            tx,
            playing,
            undecodable,
        }
    }

    /// The format of the video's sound when Windows has no decoder for it
    /// (known shortly after [`Audio::start`]).
    pub fn undecodable(&self) -> Option<&str> {
        self.undecodable.get().map(String::as_str)
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

/// What a video's sound is to Windows.
#[derive(Debug, PartialEq)]
enum Stream {
    /// The video has no sound.
    None,
    /// Windows has no decoder for its format (named): Windows 11 has none
    /// for Dolby Digital (AC-3), which Windows 10 had.
    Undecodable(String),
    /// It can be played, or the file could not be looked into here (the
    /// engine is left to try).
    Decodable,
}

/// The first sound stream of `path`, looked at with a source reader (the
/// engine only fails, without saying why): its format, and a decoder for it.
fn stream(path: &Path) -> Stream {
    let url = crate::win::wide(path);
    let Ok(reader) = (unsafe { MFCreateSourceReaderFromURL(PCWSTR(url.as_ptr()), None) }) else {
        return Stream::Decodable;
    };
    let first = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
    let native = match unsafe { reader.GetNativeMediaType(first, 0) } {
        Ok(native) => native,
        Err(e) if e.code() == MF_E_INVALIDSTREAMNUMBER => return Stream::None,
        Err(_) => return Stream::Decodable,
    };
    let Ok(subtype) = (unsafe { native.GetGUID(&MF_MT_SUBTYPE) }) else {
        return Stream::Decodable;
    };
    if subtype == MFAudioFormat_PCM || subtype == MFAudioFormat_Float || has_decoder(subtype) {
        Stream::Decodable
    } else {
        Stream::Undecodable(format_name(subtype))
    }
}

/// Whether Windows has a decoder for the sound format `subtype` (one that
/// cannot be asked is taken for there: no false alarm).
fn has_decoder(subtype: GUID) -> bool {
    let input = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Audio,
        guidSubtype: subtype,
    };
    let flags = MFT_ENUM_FLAG_SYNCMFT
        | MFT_ENUM_FLAG_ASYNCMFT
        | MFT_ENUM_FLAG_LOCALMFT
        | MFT_ENUM_FLAG_UNTRUSTED_STOREMFT
        | MFT_ENUM_FLAG_SORTANDFILTER;
    let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    let asked = unsafe {
        MFTEnumEx(
            MFT_CATEGORY_AUDIO_DECODER,
            flags,
            Some(&input),
            None,
            &mut list,
            &mut count,
        )
    };
    if !list.is_null() {
        unsafe {
            for i in 0..count as usize {
                std::ptr::drop_in_place(list.add(i));
            }
            CoTaskMemFree(Some(list as *const _));
        }
    }
    asked.is_err() || count > 0
}

/// A name for a sound format, as the user may know it.
fn format_name(subtype: GUID) -> String {
    let known = [
        (MFAudioFormat_Dolby_AC3, "Dolby Digital (AC-3)"),
        (MFAudioFormat_Dolby_AC3_SPDIF, "Dolby Digital (AC-3)"),
        (MFAudioFormat_Dolby_DDPlus, "Dolby Digital Plus (E-AC-3)"),
        (MFAudioFormat_DTS, "DTS"),
        (MFAudioFormat_AAC, "AAC"),
        (MFAudioFormat_MP3, "MP3"),
        (MFAudioFormat_MPEG, "MPEG audio"),
        (MFAudioFormat_Opus, "Opus"),
        (MFAudioFormat_Vorbis, "Vorbis"),
        (MFAudioFormat_FLAC, "FLAC"),
        (MFAudioFormat_ALAC, "ALAC"),
        (MFAudioFormat_AMR_NB, "AMR"),
        (MFAudioFormat_AMR_WB, "AMR-WB"),
    ];
    if let Some((_, name)) = known.iter().find(|(g, _)| *g == subtype) {
        return (*name).to_string();
    }
    // The other formats are a WAVE format tag or a FourCC in the GUID's
    // first part.
    let code = subtype.data1;
    let chars = code.to_le_bytes();
    if code > 0xffff && chars.iter().all(|c| c.is_ascii_graphic() || *c == b' ') {
        String::from_utf8_lossy(&chars).trim().to_string()
    } else {
        format!("0x{code:04x}")
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
    path: &Path,
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
            Msg::Event(e, ..) if e == MF_MEDIA_ENGINE_EVENT_LOADEDMETADATA.0 as u32 => {
                log::debug!("{}: sound ready, {:.1} s", path.display(), unsafe {
                    engine.GetDuration()
                });
                ready = true;
            }
            Msg::Event(e, ..) if e == MF_MEDIA_ENGINE_EVENT_PLAYING.0 as u32 => {
                log::debug!(
                    "{}: sound playing, volume {:.2}, muted {}",
                    path.display(),
                    unsafe { engine.GetVolume() },
                    unsafe { engine.GetMuted() }.as_bool()
                );
                started.store(true, Relaxed);
            }
            Msg::Event(e, code, hr) if e == MF_MEDIA_ENGINE_EVENT_ERROR.0 as u32 => {
                // A video without sound: nothing to play.
                log::info!("{}: no sound: error {code}, {:#010x}", path.display(), hr);
                return;
            }
            Msg::Event(e, a, b) => {
                log::trace!("{}: sound event {e} ({a}, {b})", path.display());
                continue;
            }
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
            let done = unsafe {
                if wanted {
                    engine.Play()
                } else {
                    engine.Pause()
                }
            };
            if let Err(e) = done {
                log::warn!("{}: sound play/pause: {e}", path.display());
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

    #[test]
    fn decoders_and_format_names() {
        let _com = Com::init();
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) }.unwrap();
        assert!(has_decoder(MFAudioFormat_AAC));
        assert!(has_decoder(MFAudioFormat_MP3));
        let made_up = GUID::from_u128(0x5a5a5a5a_0000_0010_8000_00aa00389b71);
        assert!(!has_decoder(made_up));
        assert_eq!(format_name(made_up), "ZZZZ");
        assert_eq!(format_name(MFAudioFormat_Dolby_AC3), "Dolby Digital (AC-3)");
        let tag = GUID::from_u128(0x00002000_0000_0010_8000_00aa00389b71);
        assert_eq!(format_name(tag), "0x2000");
        unsafe {
            let _ = MFShutdown();
        }
    }

    /// What the sound of `QSLIDES_VIDEO` is to Windows.
    #[test]
    #[ignore]
    fn sound_stream() {
        let path = std::env::var("QSLIDES_VIDEO").expect("QSLIDES_VIDEO");
        let _com = Com::init();
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) }.unwrap();
        println!("{:?}", stream(Path::new(&path)));
        unsafe {
            let _ = MFShutdown();
        }
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
