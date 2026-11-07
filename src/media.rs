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
use windows::Win32::Foundation::{E_FAIL, GENERIC_READ, HMODULE};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_SDK_VERSION,
    D3D11CreateDevice, ID3D11Device, ID3D11Multithread,
};
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
use windows::Win32::Media::MediaFoundation::{
    IMFDXGIDeviceManager, MF_PD_DURATION, MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS,
    MF_SOURCE_READER_D3D_MANAGER, MFCreateDXGIDeviceManager,
};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::core::Interface;
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
    /// The next frame begins another piece of the video, further on
    /// (shown with a cross-fade).
    Cut,
    /// The last frame was sent.
    End,
    Failed(String),
}

/// How a video is shown: the time it has, and in how many pieces when it
/// is longer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    /// Played again from the start while its time lasts (videos as long
    /// as pictures).
    pub looped: bool,
    /// The time it has (100 ns units); `None`: to its end.
    pub time: Option<i64>,
    /// The pieces a longer video is shown in, spread from its start to its
    /// end; 1: its start only.
    pub pieces: u32,
}

/// The shortest piece a video is cut into: shorter, the fades would be
/// most of it.
const SHORTEST_PIECE: i64 = 2 * SECOND;

/// The starts of the pieces a video of `duration` is shown in to fill
/// `time` with `wanted` of them, and their length (100 ns units): equal
/// pieces, the first at the start, the last ending at the end. `None` when
/// it is shown as it is: it fits (a second more is not worth a cut), or
/// its duration or its time is not known.
pub fn pieces(duration: Option<i64>, time: Option<i64>, wanted: u32) -> Option<(Vec<i64>, i64)> {
    let (d, t) = (duration?, time?);
    if t <= 0 || d <= t + SECOND {
        return None;
    }
    let most = (t / SHORTEST_PIECE).max(1);
    let k = i64::from(wanted.max(1)).min(most);
    let len = t / k;
    let starts = if k == 1 {
        vec![0]
    } else {
        (0..k).map(|i| i * (d - len) / (k - 1)).collect()
    };
    Some((starts, len))
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
    /// Start decoding `path`, its frames scaled down to fit `max_side`, as
    /// `plan` says. `ctx` is asked to repaint when a frame comes.
    pub fn start(path: &Path, max_side: u32, plan: Plan, ctx: egui::Context) -> Video {
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
                let result = decode_video(&path, max_side, plan, &tx, &stopped, &ctx);
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

/// A video open for decoding: its reader, the stream read, the size of the
/// frames it delivers, and the graphics card's device it decodes on, if it
/// does (kept alive as long as the reader, which is dropped first).
struct Opened {
    reader: IMFSourceReader,
    stream: u32,
    width: u32,
    height: u32,
    gpu: Option<(IMFDXGIDeviceManager, ID3D11Device)>,
}

/// A Direct3D 11 device for decoding video, in a device manager Media
/// Foundation takes (qrec's way to give its encoder the device).
fn gpu_device() -> windows::core::Result<(IMFDXGIDeviceManager, ID3D11Device)> {
    let mut device = None;
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_VIDEO_SUPPORT | D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            None,
        )?;
    }
    let device: ID3D11Device = device.ok_or(windows::core::Error::from(E_FAIL))?;
    // The decoder and the video processor use it from their own threads.
    unsafe {
        let _ = device
            .cast::<ID3D11Multithread>()?
            .SetMultithreadProtected(true);
    }
    let (mut token, mut manager) = (0u32, None);
    unsafe { MFCreateDXGIDeviceManager(&mut token, &mut manager)? };
    let manager = manager.ok_or(windows::core::Error::from(E_FAIL))?;
    unsafe { manager.ResetDevice(&device, token)? };
    Ok((manager, device))
}

/// The source reader of `path` with its first video stream selected and
/// decoded to 32-bit BGRX, scaled by the video processor to fit
/// `max_side` (as stored when the processor refuses). With `gpu`, the
/// decoder and the video processor work on the graphics card (DXVA):
/// without it, a 4K HEVC video of a phone at 60 frames a second was
/// decoded and scaled on the processor and played far too slowly.
fn open_video(path: &Path, max_side: u32, gpu: bool) -> Result<Opened, String> {
    let gpu = if gpu {
        match gpu_device() {
            Ok(d) => Some(d),
            Err(e) => {
                log::info!("no graphics card for video: {e}");
                None
            }
        }
    } else {
        None
    };
    let mut attributes: Option<IMFAttributes> = None;
    unsafe { MFCreateAttributes(&mut attributes, 3) }.map_err(|e| err("attributes", e))?;
    let attributes = attributes.ok_or("no attributes")?;
    let set = || -> windows::core::Result<()> {
        unsafe {
            attributes.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)?;
            if let Some((manager, _)) = &gpu {
                attributes.SetUnknown(&MF_SOURCE_READER_D3D_MANAGER, manager)?;
                attributes.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;
            }
        }
        Ok(())
    };
    set().map_err(|e| err("attributes", e))?;
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
    Ok(Opened {
        reader,
        stream,
        width: fw,
        height: fh,
        gpu,
    })
}

/// The video on the graphics card, else on the processor: a card or a
/// driver that cannot take it is no reason not to show it.
fn open_video_somehow(path: &Path, max_side: u32) -> Result<Opened, String> {
    match open_video(path, max_side, true) {
        Ok(opened) => Ok(opened),
        Err(e) => {
            log::info!(
                "{}: not on the graphics card ({e}), on the processor",
                path.display()
            );
            open_video(path, max_side, false)
        }
    }
}

/// The least time between frames sent to the window: 30 a second. A video
/// of 60 frames a second has every other frame decoded but not copied off
/// the graphics card and converted, which halves that work; a photo frame
/// shows no difference.
const FRAME_STEP: i64 = SECOND / 31;

fn decode_video(
    path: &Path,
    max_side: u32,
    plan: Plan,
    tx: &SyncSender<VideoMsg>,
    stop: &AtomicBool,
    ctx: &egui::Context,
) -> Result<(), String> {
    let opened = open_video_somehow(path, max_side)?;
    log::debug!(
        "{}: {}x{} frames, on the {}",
        path.display(),
        opened.width,
        opened.height,
        if opened.gpu.is_some() {
            "graphics card"
        } else {
            "processor"
        }
    );
    let Opened {
        reader,
        stream,
        width: w,
        height: h,
        gpu: _gpu,
    } = opened;
    let stride = unsafe { reader.GetCurrentMediaType(stream) }
        .and_then(|t| unsafe { t.GetUINT32(&MF_MT_DEFAULT_STRIDE) })
        .map_or(w as i32 * 4, |s| s as i32);
    let length = duration(&reader);
    // A video longer than its time: its pieces, and the one played.
    let cut_up = pieces(length, plan.time, plan.pieces);
    let mut piece = 0usize;
    if let Some((starts, len)) = &cut_up {
        log::debug!(
            "{}: {:.0} s shown in {} pieces of {:.1} s",
            path.display(),
            length.unwrap_or(0) as f64 / SECOND as f64,
            starts.len(),
            *len as f64 / SECOND as f64
        );
    }
    let seek = |to: i64| {
        let at = PROPVARIANT::from(to);
        unsafe { reader.SetCurrentPosition(&GUID::zeroed(), &at) }.map_err(|e| err("seek", e))
    };
    // The time of the first frame: the pieces' starts count from it.
    let mut base = None::<i64>;
    // The next frame sent begins a piece after the first.
    let mut cut = false;
    // Added to the times of a looped video's later rounds and of the
    // pieces after the first, so that they go on growing.
    let mut offset = 0i64;
    let mut last = 0i64;
    let mut frames = 0u64;
    // Played again from the start: the offset is set by the round's first
    // frame, whose time need not be 0 (MPEG-TS from cameras, an MP4 with
    // an edit list), else every round began with a stop that long.
    let mut rewound = false;
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
            // The duration said more than there was: the next piece.
            if let Some((starts, _)) = &cut_up
                && frames > 0
                && piece + 1 < starts.len()
            {
                piece += 1;
                seek(starts[piece])?;
                (rewound, cut) = (true, true);
                continue;
            }
            if plan.looped && cut_up.is_none() && frames > 0 {
                rewound = true;
                seek(0)?;
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
        let base = *base.get_or_insert(time);
        if let Some((starts, len)) = &cut_up {
            let at = time - base;
            // Decoded from the key frame before the piece's start.
            if at < starts[piece] {
                continue;
            }
            if at >= starts[piece] + len {
                piece += 1;
                if piece == starts.len() {
                    let _ = tx.send(VideoMsg::End);
                    ctx.request_repaint();
                    return Ok(());
                }
                seek(starts[piece])?;
                (rewound, cut) = (true, true);
                continue;
            }
        }
        if cut {
            log::debug!(
                "piece {} from {:.2} s",
                piece + 1,
                (time - base) as f64 / SECOND as f64
            );
        }
        if rewound {
            rewound = false;
            offset = last + SECOND / 30 - time;
        }
        if frames > 0 && offset + time < last + FRAME_STEP {
            continue;
        }
        let image = pixels(&sample, w as usize, h as usize, stride).map_err(|e| err("frame", e))?;
        last = offset + time;
        frames += 1;
        if cut {
            cut = false;
            if tx.send(VideoMsg::Cut).is_err() {
                return Ok(());
            }
        }
        if tx.send(VideoMsg::Frame(last, image)).is_err() {
            // The video is no longer shown.
            return Ok(());
        }
        ctx.request_repaint();
    }
}

/// The length of the video the reader reads (100 ns units), if its source
/// says.
fn duration(reader: &IMFSourceReader) -> Option<i64> {
    /// `MF_SOURCE_READER_MEDIASOURCE`: the source itself, not a stream.
    const SOURCE: u32 = 0xFFFF_FFFF;
    let value = unsafe { reader.GetPresentationAttribute(SOURCE, &MF_PD_DURATION) }.ok()?;
    let d = u64::try_from(&value).ok()?;
    i64::try_from(d).ok().filter(|&d| d > 0)
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

    /// A video shown to its end, in one piece.
    const WHOLE: Plan = Plan {
        looped: false,
        time: None,
        pieces: 1,
    };

    #[test]
    fn pieces_of_a_long_video() {
        let s = SECOND;
        // A minute in 6 pieces of a 2-minute video: 10 s each, the first
        // at the start, the last ending at the end.
        let (starts, len) = pieces(Some(120 * s), Some(60 * s), 6).unwrap();
        assert_eq!(len, 10 * s);
        assert_eq!(starts, [0, 22, 44, 66, 88, 110].map(|n| n * s));
        // One piece: the start.
        assert_eq!(
            pieces(Some(120 * s), Some(30 * s), 1),
            Some((vec![0], 30 * s))
        );
        // No piece shorter than 2 s: 10 s give 5 of them, not 8.
        assert_eq!(pieces(Some(120 * s), Some(10 * s), 8).unwrap().0.len(), 5);
        // A video that fits, or of no known length, is shown as it is.
        assert_eq!(pieces(Some(60 * s), Some(60 * s), 6), None);
        assert_eq!(pieces(Some(60 * s), Some(59 * s + s / 2), 6), None);
        assert_eq!(pieces(None, Some(10 * s), 6), None);
        assert_eq!(pieces(Some(120 * s), None, 6), None);
    }

    /// The video named by `QSLIDES_VIDEO` (longer than 7 s) shown for 6 s in
    /// 3 pieces: two cuts, the frames' times going on growing, with
    /// `--nocapture`.
    #[test]
    #[ignore]
    fn video_in_pieces() {
        let path = std::path::PathBuf::from(std::env::var("QSLIDES_VIDEO").expect("QSLIDES_VIDEO"));
        let plan = Plan {
            looped: false,
            time: Some(6 * SECOND),
            pieces: 3,
        };
        let started = std::time::Instant::now();
        let video = Video::start(&path, 320, plan, egui::Context::default());
        let (mut times, mut cuts) = (Vec::new(), 0);
        loop {
            match video.rx.recv_timeout(std::time::Duration::from_secs(10)) {
                Ok(VideoMsg::Cut) => {
                    cuts += 1;
                    println!(
                        "cut after {:.3} s, at {:.2} s of decoding",
                        *times.last().unwrap() as f64 / SECOND as f64,
                        started.elapsed().as_secs_f64()
                    );
                }
                Ok(VideoMsg::Frame(t, _)) => times.push(t),
                Ok(VideoMsg::End) => break,
                Ok(VideoMsg::Failed(e)) => panic!("{e}"),
                Err(e) => panic!("{e}"),
            }
        }
        let shown = (*times.last().unwrap() - times[0]) as f64 / SECOND as f64;
        println!("{} frames, {cuts} cuts, {shown:.2} s shown", times.len());
        assert_eq!(cuts, 2);
        assert!(times.windows(2).all(|w| w[1] > w[0]));
        assert!((5.5..6.5).contains(&shown), "{shown}");
    }

    /// Decode the video named by `QSLIDES_VIDEO` and print its frames'
    /// size and times and the decoding speed, with `--nocapture`.
    #[test]
    #[ignore]
    fn video_file() {
        let path = std::path::PathBuf::from(std::env::var("QSLIDES_VIDEO").expect("QSLIDES_VIDEO"));
        let ctx = egui::Context::default();
        let started = std::time::Instant::now();
        let video = Video::start(&path, 1280, WHOLE, ctx);
        let mut frames = 0;
        loop {
            match video.rx.recv_timeout(std::time::Duration::from_secs(10)) {
                Ok(VideoMsg::Cut) => {}
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

    /// The picture named by `QSLIDES_IMAGE` decoded for screens of a few
    /// sizes: the time each takes, with `--release --nocapture`.
    #[test]
    #[ignore]
    fn image_file() {
        let path = std::path::PathBuf::from(std::env::var("QSLIDES_IMAGE").expect("QSLIDES_IMAGE"));
        let _com = Com::init();
        for side in [1920, 3840, 1920] {
            let started = std::time::Instant::now();
            let image = decode_image(&path, side).unwrap();
            println!(
                "{side}: {:?} in {:.0} ms",
                image.size,
                started.elapsed().as_secs_f64() * 1e3
            );
        }
    }

    /// The video named by `QSLIDES_VIDEO` looped: its frames' times go on
    /// over the seam without a gap (its first frame's time is not 0 in
    /// MPEG-TS from cameras), with `--nocapture`.
    #[test]
    #[ignore]
    fn video_loops() {
        let path = std::path::PathBuf::from(std::env::var("QSLIDES_VIDEO").expect("QSLIDES_VIDEO"));
        let times = |looped: bool, until: Option<i64>| {
            let plan = Plan { looped, ..WHOLE };
            let video = Video::start(&path, 320, plan, egui::Context::default());
            let mut times = Vec::new();
            loop {
                match video.rx.recv_timeout(std::time::Duration::from_secs(10)) {
                    Ok(VideoMsg::Cut) => {}
                    Ok(VideoMsg::Frame(t, _)) => {
                        times.push(t);
                        if until.is_some_and(|u| t > u) {
                            return times;
                        }
                    }
                    Ok(VideoMsg::End) => return times,
                    Ok(VideoMsg::Failed(e)) => panic!("{e}"),
                    Err(e) => panic!("{e}"),
                }
            }
        };
        let once = times(false, None);
        let (first, end) = (once[0], *once.last().unwrap());
        let looped = times(true, Some(end + SECOND));
        let gap = looped.windows(2).map(|w| w[1] - w[0]).max().unwrap();
        println!(
            "first frame at {:.3} s, last at {:.3} s, longest gap looped {:.3} s",
            first as f64 / SECOND as f64,
            end as f64 / SECOND as f64,
            gap as f64 / SECOND as f64
        );
        assert!(looped.windows(2).all(|w| w[1] > w[0]));
        assert!(gap < SECOND / 5, "a stop at the seam");
    }

    /// Every frame of the video named by `QSLIDES_VIDEO` decoded, scaled to
    /// 1920 and copied out, on the graphics card and on the processor:
    /// frames a second of each way, with `--nocapture`.
    #[test]
    #[ignore]
    fn video_gpu_and_cpu() {
        let path = std::path::PathBuf::from(std::env::var("QSLIDES_VIDEO").expect("QSLIDES_VIDEO"));
        let _com = Com::init();
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) }.unwrap();
        for gpu in [true, false] {
            let started = std::time::Instant::now();
            let opened = match open_video(&path, 1920, gpu) {
                Ok(o) => o,
                Err(e) => {
                    println!("gpu {gpu}: {e}");
                    continue;
                }
            };
            let on_gpu = opened.gpu.is_some();
            let (mut frames, mut copied) = (0u32, 0u32);
            loop {
                let (mut flags, mut sample) = (0u32, None::<IMFSample>);
                unsafe {
                    opened.reader.ReadSample(
                        opened.stream,
                        0,
                        None,
                        Some(&mut flags),
                        None,
                        Some(&mut sample),
                    )
                }
                .unwrap();
                if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 || frames >= 600 {
                    break;
                }
                let Some(sample) = sample else { continue };
                frames += 1;
                if frames % 2 == 0 {
                    let w = opened.width as usize;
                    pixels(&sample, w, opened.height as usize, w as i32 * 4).unwrap();
                    copied += 1;
                }
            }
            let secs = started.elapsed().as_secs_f64();
            println!(
                "on the {}: {}x{}, {frames} frames ({copied} copied) in {secs:.2} s, {:.0} a second",
                if on_gpu { "graphics card" } else { "processor" },
                opened.width,
                opened.height,
                frames as f64 / secs
            );
        }
        unsafe { MFShutdown() }.unwrap();
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
