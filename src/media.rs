//! Decoding: pictures through Windows Imaging Component, videos through the
//! Media Foundation source reader (only the picture: the sound stream is
//! never selected). Windows' own codecs, so no library ships with the exe:
//! JPEG, PNG, GIF, BMP, TIFF and JPEG XR always; HEIC, AVIF and WebP with
//! their extensions from the Microsoft Store; H.264, HEVC (with its
//! extension), MPEG-4, WMV and VP9 videos in MP4, MOV, MKV, AVI, WMV and
//! WebM.
//!
//! Both run on threads of their own, never on the UI thread: a picture is
//! decoded and scaled down to the size of the screen before the slide
//! before it ends (`loader.rs`), a video's frames come through a bounded
//! channel ([`Video`]), its thread waiting while the channel is full.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

use egui::{Color32, ColorImage};
use windows::Win32::Foundation::GENERIC_READ;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppRGBA, IWICBitmapSource, IWICImagingFactory,
    WICBitmapDitherTypeNone, WICBitmapInterpolationModeFant, WICBitmapPaletteTypeCustom,
    WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::Media::MediaFoundation::{
    IMF2DBuffer, IMFAttributes, IMFSample, IMFSourceReader, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_SIZE,
    MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING,
    MF_SOURCE_READERF_ENDOFSTREAM, MF_SOURCE_READERF_STREAMTICK, MF_VERSION, MFCreateAttributes,
    MFCreateMediaType, MFCreateSourceReaderFromURL, MFMediaType_Video, MFSTARTUP_FULL, MFShutdown,
    MFStartup, MFVideoFormat_RGB32,
};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::core::{GUID, PCWSTR, w};

/// COM for the current thread (multithreaded), released when dropped.
pub struct Com {
    initialised: bool,
}

impl Com {
    pub fn init() -> Com {
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        Com {
            initialised: hr.is_ok(),
        }
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.initialised {
            unsafe { CoUninitialize() };
        }
    }
}

fn err(what: &str, e: windows::core::Error) -> String {
    format!("{what}: {e}")
}

/// Decode the picture in `path`, turned upright by its EXIF orientation and
/// scaled down (never up) so that neither side exceeds `max_side` pixels.
/// On a thread with COM ([`Com::init`]).
pub fn decode_image(path: &Path, max_side: u32) -> Result<ColorImage, String> {
    let factory: IWICImagingFactory =
        unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }
            .map_err(|e| err("WIC", e))?;
    let name = crate::win::wide(path);
    let decoder = unsafe {
        factory.CreateDecoderFromFilename(
            PCWSTR(name.as_ptr()),
            None,
            GENERIC_READ,
            WICDecodeMetadataCacheOnDemand,
        )
    }
    .map_err(|e| err("no decoder", e))?;
    let frame = unsafe { decoder.GetFrame(0) }.map_err(|e| err("GetFrame", e))?;
    let orientation = orientation(&frame);
    let (mut w, mut h) = (0u32, 0u32);
    unsafe { frame.GetSize(&mut w, &mut h) }.map_err(|e| err("GetSize", e))?;
    if w == 0 || h == 0 {
        return Err(format!("picture of {w}x{h}"));
    }
    let (sw, sh) = fit_within(w, h, max_side);
    let mut source: IWICBitmapSource = frame.into();
    if (sw, sh) != (w, h) {
        let scaler = unsafe { factory.CreateBitmapScaler() }.map_err(|e| err("scaler", e))?;
        unsafe { scaler.Initialize(&source, sw, sh, WICBitmapInterpolationModeFant) }
            .map_err(|e| err("scaling", e))?;
        source = scaler.into();
    }
    let converter = unsafe { factory.CreateFormatConverter() }.map_err(|e| err("converter", e))?;
    unsafe {
        converter.Initialize(
            &source,
            &GUID_WICPixelFormat32bppRGBA,
            WICBitmapDitherTypeNone,
            None,
            0.0,
            WICBitmapPaletteTypeCustom,
        )
    }
    .map_err(|e| err("conversion", e))?;
    let mut rgba = vec![0u8; sw as usize * sh as usize * 4];
    unsafe { converter.CopyPixels(std::ptr::null(), sw * 4, &mut rgba) }
        .map_err(|e| err("CopyPixels", e))?;
    let (rgba, ow, oh) = orient(rgba, sw as usize, sh as usize, orientation);
    Ok(ColorImage::from_rgba_unmultiplied([ow, oh], &rgba))
}

/// The EXIF orientation of a decoded frame (1 to 8; 1 without one),
/// through the photo metadata policy, whatever the format keeps it in.
fn orientation(frame: &windows::Win32::Graphics::Imaging::IWICBitmapFrameDecode) -> u16 {
    let Ok(reader) = (unsafe { frame.GetMetadataQueryReader() }) else {
        return 1;
    };
    let mut value = PROPVARIANT::default();
    if unsafe { reader.GetMetadataByName(w!("System.Photo.Orientation"), &mut value) }.is_err() {
        return 1;
    }
    let found = u16::try_from(&value)
        .ok()
        .or_else(|| u32::try_from(&value).ok().map(|v| v as u16));
    found.filter(|o| (1..=8).contains(o)).unwrap_or(1)
}

/// `(w, h)` scaled down to fit `max_side` on both sides, the proportions
/// kept; as it is when it fits.
pub fn fit_within(w: u32, h: u32, max_side: u32) -> (u32, u32) {
    let max_side = max_side.max(1);
    if w <= max_side && h <= max_side {
        return (w, h);
    }
    let scale = max_side as f64 / w.max(h) as f64;
    (
        ((w as f64 * scale).round() as u32).max(1),
        ((h as f64 * scale).round() as u32).max(1),
    )
}

/// RGBA pixels of `w` x `h` turned as EXIF `orientation` says: 2 mirrored,
/// 3 turned half round, 4 upside down, 5 mirrored along the diagonal, 6 a
/// quarter clockwise, 7 along the other diagonal, 8 a quarter
/// anticlockwise. The new pixels and size.
pub fn orient(rgba: Vec<u8>, w: usize, h: usize, orientation: u16) -> (Vec<u8>, usize, usize) {
    if orientation <= 1 || orientation > 8 {
        return (rgba, w, h);
    }
    let swap = orientation >= 5;
    let (ow, oh) = if swap { (h, w) } else { (w, h) };
    let mut out = vec![0u8; rgba.len()];
    for y in 0..h {
        for x in 0..w {
            // Where the stored pixel (x, y) goes in the upright picture.
            let (nx, ny) = match orientation {
                2 => (w - 1 - x, y),
                3 => (w - 1 - x, h - 1 - y),
                4 => (x, h - 1 - y),
                5 => (y, x),
                6 => (h - 1 - y, x),
                7 => (h - 1 - y, w - 1 - x),
                _ => (y, w - 1 - x),
            };
            let from = (y * w + x) * 4;
            let to = (ny * ow + nx) * 4;
            out[to..to + 4].copy_from_slice(&rgba[from..from + 4]);
        }
    }
    (out, ow, oh)
}

/// One second in the 100 ns units of Media Foundation.
pub const SECOND: i64 = 10_000_000;

/// What a video's thread sends.
pub enum VideoMsg {
    /// A frame, and when it is shown counted from the start of the video
    /// (100 ns units; a looped video's times go on growing).
    Frame(i64, ColorImage),
    /// The last frame was sent.
    End,
    Failed(String),
}

/// A video being decoded on its own thread. Dropped, its thread stops.
pub struct Video {
    pub rx: Receiver<VideoMsg>,
    stop: Arc<AtomicBool>,
}

impl Drop for Video {
    fn drop(&mut self) {
        self.stop.store(true, Relaxed);
    }
}

/// Frames decoded ahead of the one shown.
const AHEAD: usize = 3;

impl Video {
    /// Start decoding `path`, its frames scaled down to fit `max_side`;
    /// with `looped`, from the start again after the end. `ctx` is asked to
    /// repaint when a frame comes.
    pub fn start(path: &Path, max_side: u32, looped: bool, ctx: egui::Context) -> Video {
        let (tx, rx) = sync_channel(AHEAD);
        let stop = Arc::new(AtomicBool::new(false));
        let path = path.to_path_buf();
        let stopped = stop.clone();
        let spawned = std::thread::Builder::new()
            .name("video".into())
            .spawn(move || {
                let _com = Com::init();
                if let Err(e) = unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) } {
                    let _ = tx.send(VideoMsg::Failed(err("MFStartup", e)));
                    ctx.request_repaint();
                    return;
                }
                let result = decode_video(&path, max_side, looped, &tx, &stopped, &ctx);
                if let Err(e) = result {
                    let _ = tx.send(VideoMsg::Failed(e));
                    ctx.request_repaint();
                }
                unsafe {
                    let _ = MFShutdown();
                }
            });
        if let Err(e) = spawned {
            log::warn!("cannot start a video thread: {e}");
        }
        Video { rx, stop }
    }
}

/// The source reader of `path` with its first video stream selected and
/// decoded to 32-bit BGRX, scaled by the video processor to fit
/// `max_side` (as stored when the processor refuses); the frame size.
fn open_video(path: &Path, max_side: u32) -> Result<(IMFSourceReader, u32, u32, u32), String> {
    let mut attributes: Option<IMFAttributes> = None;
    unsafe { MFCreateAttributes(&mut attributes, 1) }.map_err(|e| err("attributes", e))?;
    let attributes = attributes.ok_or("no attributes")?;
    unsafe { attributes.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1) }
        .map_err(|e| err("attributes", e))?;
    let url = crate::win::wide(path);
    let reader = unsafe { MFCreateSourceReaderFromURL(PCWSTR(url.as_ptr()), &attributes) }
        .map_err(|e| err("cannot open", e))?;
    let mut video = None;
    let mut size = (0, 0);
    for stream in 0..16u32 {
        let Ok(native) = (unsafe { reader.GetNativeMediaType(stream, 0) }) else {
            break;
        };
        let major = unsafe { native.GetGUID(&MF_MT_MAJOR_TYPE) }.unwrap_or(GUID::zeroed());
        let wanted = major == MFMediaType_Video && video.is_none();
        let _ = unsafe { reader.SetStreamSelection(stream, wanted) };
        if wanted {
            video = Some(stream);
            if let Ok(packed) = unsafe { native.GetUINT64(&MF_MT_FRAME_SIZE) } {
                size = ((packed >> 32) as u32, packed as u32);
            }
        }
    }
    let stream = video.ok_or("no video stream")?;
    let output = |frame: Option<(u32, u32)>| -> windows::core::Result<()> {
        let rgb = unsafe { MFCreateMediaType()? };
        unsafe {
            rgb.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            rgb.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
            if let Some((w, h)) = frame {
                rgb.SetUINT64(&MF_MT_FRAME_SIZE, (u64::from(w) << 32) | u64::from(h))?;
            }
            reader.SetCurrentMediaType(stream, None, &rgb)
        }
    };
    let (w, h) = size;
    let scaled = fit_within(w.max(1), h.max(1), max_side);
    // Even sides: the video processor refuses odd ones for some formats.
    let scaled = ((scaled.0 & !1).max(2), (scaled.1 & !1).max(2));
    if w == 0 || h == 0 || scaled == (w, h) || output(Some(scaled)).is_err() {
        output(None).map_err(|e| err("no RGB output", e))?;
    }
    let current =
        unsafe { reader.GetCurrentMediaType(stream) }.map_err(|e| err("output type", e))?;
    let packed =
        unsafe { current.GetUINT64(&MF_MT_FRAME_SIZE) }.map_err(|e| err("frame size", e))?;
    let (fw, fh) = ((packed >> 32) as u32, packed as u32);
    if fw == 0 || fh == 0 {
        return Err(format!("frames of {fw}x{fh}"));
    }
    Ok((reader, stream, fw, fh))
}

fn decode_video(
    path: &Path,
    max_side: u32,
    looped: bool,
    tx: &SyncSender<VideoMsg>,
    stop: &AtomicBool,
    ctx: &egui::Context,
) -> Result<(), String> {
    let (reader, stream, w, h) = open_video(path, max_side)?;
    let stride = unsafe { reader.GetCurrentMediaType(stream) }
        .and_then(|t| unsafe { t.GetUINT32(&MF_MT_DEFAULT_STRIDE) })
        .map_or(w as i32 * 4, |s| s as i32);
    // Added to the times of a looped video's later rounds.
    let mut offset = 0i64;
    let mut last = 0i64;
    let mut frames = 0u64;
    loop {
        if stop.load(Relaxed) {
            return Ok(());
        }
        let (mut flags, mut time, mut sample) = (0u32, 0i64, None::<IMFSample>);
        unsafe {
            reader.ReadSample(
                stream,
                0,
                None,
                Some(&mut flags),
                Some(&mut time),
                Some(&mut sample),
            )
        }
        .map_err(|e| err("ReadSample", e))?;
        if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
            if looped && frames > 0 {
                offset = last + SECOND / 30;
                let start = PROPVARIANT::from(0i64);
                unsafe { reader.SetCurrentPosition(&GUID::zeroed(), &start) }
                    .map_err(|e| err("seek", e))?;
                continue;
            }
            if frames == 0 {
                return Err("no frame decoded".into());
            }
            let _ = tx.send(VideoMsg::End);
            ctx.request_repaint();
            return Ok(());
        }
        if flags & MF_SOURCE_READERF_STREAMTICK.0 as u32 != 0 {
            continue;
        }
        let Some(sample) = sample else { continue };
        let image = pixels(&sample, w as usize, h as usize, stride).map_err(|e| err("frame", e))?;
        last = offset + time;
        frames += 1;
        if tx.send(VideoMsg::Frame(last, image)).is_err() {
            // The video is no longer shown.
            return Ok(());
        }
        ctx.request_repaint();
    }
}

/// The pixels of a BGRX sample as an egui image.
fn pixels(
    sample: &IMFSample,
    width: usize,
    height: usize,
    stride: i32,
) -> windows::core::Result<ColorImage> {
    use windows::core::Interface;
    let buffer = unsafe { sample.ConvertToContiguousBuffer()? };
    // A 2-D buffer knows its own pitch (negative for a bottom-up picture);
    // a plain one has the pitch of the media type.
    let (data, pitch, locked_2d) = match buffer.cast::<IMF2DBuffer>() {
        Ok(planar) => {
            let (mut scanline0, mut pitch) = (std::ptr::null_mut(), 0i32);
            unsafe { planar.Lock2D(&mut scanline0, &mut pitch)? };
            (scanline0.cast_const(), pitch as isize, Some(planar))
        }
        Err(_) => {
            let (mut data, mut length) = (std::ptr::null_mut(), 0u32);
            unsafe { buffer.Lock(&mut data, None, Some(&mut length))? };
            if (length as usize) < height * stride.unsigned_abs() as usize {
                unsafe { buffer.Unlock()? };
                return Err(windows::core::Error::from(
                    windows::Win32::Foundation::E_UNEXPECTED,
                ));
            }
            // A negative stride: the first row is the last in memory.
            let data = if stride < 0 {
                unsafe { data.add((height - 1) * (-stride) as usize) }
            } else {
                data
            };
            (data.cast_const(), stride as isize, None)
        }
    };
    let mut out = Vec::with_capacity(width * height);
    for y in 0..height {
        let row = unsafe { std::slice::from_raw_parts(data.offset(pitch * y as isize), width * 4) };
        out.extend(
            row.as_chunks::<4>()
                .0
                .iter()
                .map(|p| Color32::from_rgb(p[2], p[1], p[0])),
        );
    }
    unsafe {
        match locked_2d {
            Some(planar) => planar.Unlock2D()?,
            None => buffer.Unlock()?,
        }
    }
    Ok(ColorImage::new([width, height], out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_within_the_screen() {
        assert_eq!(fit_within(4000, 3000, 2000), (2000, 1500));
        assert_eq!(fit_within(3000, 4000, 2000), (1500, 2000));
        assert_eq!(fit_within(800, 600, 2000), (800, 600));
        assert_eq!(fit_within(10000, 1, 100), (100, 1));
    }

    /// A 3x2 picture with distinct pixels: its value is its stored index.
    fn grid() -> Vec<u8> {
        (0..6u8).flat_map(|i| [i, 0, 0, 255]).collect()
    }

    fn values(rgba: &[u8]) -> Vec<u8> {
        rgba.chunks(4).map(|p| p[0]).collect()
    }

    #[test]
    fn orientations() {
        // Stored:  0 1 2
        //          3 4 5
        let (p, w, h) = orient(grid(), 3, 2, 1);
        assert_eq!((values(&p), w, h), (vec![0, 1, 2, 3, 4, 5], 3, 2));
        let (p, w, h) = orient(grid(), 3, 2, 2);
        assert_eq!((values(&p), w, h), (vec![2, 1, 0, 5, 4, 3], 3, 2));
        let (p, _, _) = orient(grid(), 3, 2, 3);
        assert_eq!(values(&p), [5, 4, 3, 2, 1, 0]);
        let (p, _, _) = orient(grid(), 3, 2, 4);
        assert_eq!(values(&p), [3, 4, 5, 0, 1, 2]);
        // A quarter clockwise: the left column becomes the top row.
        let (p, w, h) = orient(grid(), 3, 2, 6);
        assert_eq!((values(&p), w, h), (vec![3, 0, 4, 1, 5, 2], 2, 3));
        let (p, _, _) = orient(grid(), 3, 2, 8);
        assert_eq!(values(&p), [2, 5, 1, 4, 0, 3]);
        let (p, _, _) = orient(grid(), 3, 2, 5);
        assert_eq!(values(&p), [0, 3, 1, 4, 2, 5]);
        let (p, _, _) = orient(grid(), 3, 2, 7);
        assert_eq!(values(&p), [5, 2, 4, 1, 3, 0]);
    }

    /// Windows' own PNG and BMP codecs, through a file written here.
    #[test]
    fn decodes_a_bmp() {
        let _com = Com::init();
        let dir = std::env::temp_dir().join(format!("qslides_media_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("red.bmp");
        std::fs::write(&file, bmp_4x2()).unwrap();
        let img = decode_image(&file, 4096).unwrap();
        assert_eq!(img.size, [4, 2]);
        assert_eq!(img.pixels[0], Color32::from_rgb(255, 0, 0));
        let small = decode_image(&file, 2).unwrap();
        assert_eq!(small.size, [2, 1]);
        assert!(decode_image(&dir.join("missing.jpg"), 100).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Decode the video named by `QSLIDES_VIDEO` and print its frames'
    /// size and times and the decoding speed, with `--nocapture`.
    #[test]
    #[ignore]
    fn video_file() {
        let path = std::path::PathBuf::from(std::env::var("QSLIDES_VIDEO").expect("QSLIDES_VIDEO"));
        let ctx = egui::Context::default();
        let started = std::time::Instant::now();
        let video = Video::start(&path, 1280, false, ctx);
        let mut frames = 0;
        loop {
            match video.rx.recv_timeout(std::time::Duration::from_secs(10)) {
                Ok(VideoMsg::Frame(t, image)) => {
                    if frames < 3 || frames % 100 == 0 {
                        println!(
                            "frame {frames} at {:.3} s: {:?}",
                            t as f64 / SECOND as f64,
                            image.size
                        );
                    }
                    frames += 1;
                }
                Ok(VideoMsg::End) => break,
                Ok(VideoMsg::Failed(e)) => panic!("{e}"),
                Err(e) => panic!("{e}"),
            }
        }
        let secs = started.elapsed().as_secs_f64();
        println!(
            "{frames} frames in {secs:.2} s ({:.0} a second)",
            frames as f64 / secs
        );
        assert!(frames > 0);
    }

    /// A 24-bit 4x2 BMP, all red.
    fn bmp_4x2() -> Vec<u8> {
        let row = [0u8, 0, 255].repeat(4); // 12 bytes, already a multiple of 4
        let pixels: Vec<u8> = row.repeat(2);
        let size = 14 + 40 + pixels.len() as u32;
        let mut out = Vec::new();
        out.extend(b"BM");
        out.extend(size.to_le_bytes());
        out.extend([0u8; 4]);
        out.extend(54u32.to_le_bytes());
        out.extend(40u32.to_le_bytes());
        out.extend(4i32.to_le_bytes());
        out.extend(2i32.to_le_bytes());
        out.extend(1u16.to_le_bytes());
        out.extend(24u16.to_le_bytes());
        out.extend([0u8; 24]);
        out.extend(pixels);
        out
    }
}
