//! Recording part of the screen to an MP4 (Media Foundation's H.264 encoder) or an
//! animated GIF (Windows Imaging), on a thread of its own so the event loop and the
//! hooks it serves never wait for a frame.

use crate::magnifier::Dib;
use crate::{Context, Error, Result};
use powerkit_core::Rect;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows::core::{GUID, HSTRING};
use windows::Win32::Foundation::GENERIC_WRITE;
use windows::Win32::Graphics::Gdi::{BitBlt, DeleteObject, GetDC, ReleaseDC, CAPTUREBLT, SRCCOPY};
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::{IPropertyBag2, PROPVARIANT, PROPVARIANT_0_0_0};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemAlloc, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::System::Variant::{VARENUM, VT_UI1, VT_UI2, VT_VECTOR};
use windows::Win32::UI::WindowsAndMessaging::{
    DrawIconEx, GetCursorInfo, GetIconInfo, CURSORINFO, CURSOR_SHOWING, DI_NORMAL, HICON, ICONINFO,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Mp4,
    Gif,
}

/// The area actually recorded: H.264 needs an even width and height.
pub fn recorded_area(area: Rect, format: Format) -> Rect {
    match format {
        Format::Mp4 => Rect::new(
            area.left,
            area.top,
            area.left + (area.width() & !1),
            area.top + (area.height() & !1),
        ),
        Format::Gif => area,
    }
}

/// A recording under way; dropping it without [`Recording::stop`] still ends it, but
/// any error is then lost.
pub struct Recording {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<Result<()>>>,
}

impl Recording {
    /// Start recording `area` into `path`. Returns once the first frame is written,
    /// so a recording that cannot start says why here.
    pub fn start(area: Rect, fps: u32, cursor: bool, format: Format, path: &Path) -> Result<Recording> {
        let area = recorded_area(area, format);
        if area.width() < 2 || area.height() < 2 {
            return Err(Error::Other("the area is too small to record".into()));
        }
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let (flag, path) = (stop.clone(), path.to_path_buf());
        let thread = std::thread::Builder::new()
            .name("screen recorder".into())
            .spawn(move || {
                let result = record(area, fps.max(1), cursor, format, &path, &flag, &tx);
                if result.is_err() {
                    // Half a file that no player opens is no use to anyone.
                    let _ = std::fs::remove_file(&path);
                }
                result
            })
            .map_err(|e| Error::Other(format!("starting the recorder: {e}")))?;
        let mut rec = Recording {
            stop,
            thread: Some(thread),
        };
        if rx.recv().is_err() {
            // The thread ended before its first frame; its result says why.
            return Err(rec
                .finish()
                .err()
                .unwrap_or_else(|| Error::Other("the recorder stopped".into())));
        }
        Ok(rec)
    }

    /// Whether the recording ended by itself, which only an error does.
    pub fn failed(&self) -> bool {
        self.thread.as_ref().is_some_and(JoinHandle::is_finished)
    }

    /// End the recording and finish the file.
    pub fn stop(mut self) -> Result<()> {
        self.finish()
    }

    fn finish(&mut self) -> Result<()> {
        self.stop.store(true, Ordering::Relaxed);
        match self.thread.take().map(JoinHandle::join) {
            Some(Ok(result)) => result,
            Some(Err(_)) => Err(Error::Other("the recorder crashed".into())),
            None => Ok(()),
        }
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        if let Err(e) = self.finish() {
            tracing::warn!("recording ended with an error: {e}");
        }
    }
}

fn record(
    area: Rect,
    fps: u32,
    cursor: bool,
    format: Format,
    path: &Path,
    stop: &AtomicBool,
    started: &mpsc::Sender<()>,
) -> Result<()> {
    // SAFETY: initializing COM on this thread, which owns every object made below.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let (w, h) = (area.width() as u32, area.height() as u32);
    let mut sink = match format {
        Format::Mp4 => Sink::Mp4(Mp4::new(path, w, h, fps)?),
        Format::Gif => Sink::Gif(Gif::new(path, w, h)?),
    };
    let mut dib = Dib::new(area.width(), area.height())?;
    let interval = Duration::from_secs(1) / fps;
    let start = Instant::now();
    let mut next = start;
    let mut first = true;
    loop {
        let at = start.elapsed();
        grab(&mut dib, area, cursor)?;
        sink.write(dib.pixels(), at)?;
        if first {
            first = false;
            // The receiver is gone only if `start` already returned.
            let _ = started.send(());
        }
        if stop.load(Ordering::Relaxed) {
            break;
        }
        next += interval;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            // Falling behind: skip frames rather than rush to catch up.
            next = now;
        }
        if stop.load(Ordering::Relaxed) {
            break;
        }
    }
    sink.finish(start.elapsed(), interval)
}

/// Copy the screen area into `dib`, with the mouse pointer drawn on when `cursor`.
fn grab(dib: &mut Dib, area: Rect, cursor: bool) -> Result<()> {
    // SAFETY: the screen DC is released on every path; `dib` owns its own DC; the
    // icon bitmaps GetIconInfo hands over are deleted here.
    unsafe {
        let screen = GetDC(None);
        if screen.is_invalid() {
            return Err(Error::Other("GetDC(screen) failed".into()));
        }
        let blt = BitBlt(
            dib.dc,
            0,
            0,
            area.width(),
            area.height(),
            Some(screen),
            area.left,
            area.top,
            SRCCOPY | CAPTUREBLT,
        );
        ReleaseDC(None, screen);
        blt.context("BitBlt(screen)")?;
        if !cursor {
            return Ok(());
        }
        let mut ci = CURSORINFO {
            cbSize: std::mem::size_of::<CURSORINFO>() as u32,
            ..Default::default()
        };
        if GetCursorInfo(&mut ci).is_err() || ci.flags != CURSOR_SHOWING {
            return Ok(());
        }
        let icon = HICON(ci.hCursor.0);
        let mut ii = ICONINFO::default();
        if GetIconInfo(icon, &mut ii).is_err() {
            return Ok(());
        }
        // A pointer that cannot be drawn is left out rather than ending the recording.
        let _ = DrawIconEx(
            dib.dc,
            ci.ptScreenPos.x - ii.xHotspot as i32 - area.left,
            ci.ptScreenPos.y - ii.yHotspot as i32 - area.top,
            icon,
            0,
            0,
            0,
            None,
            DI_NORMAL,
        );
        if !ii.hbmMask.is_invalid() {
            let _ = DeleteObject(ii.hbmMask.into());
        }
        if !ii.hbmColor.is_invalid() {
            let _ = DeleteObject(ii.hbmColor.into());
        }
    }
    Ok(())
}

enum Sink {
    Mp4(Mp4),
    Gif(Gif),
}

impl Sink {
    fn write(&mut self, pixels: &[u32], at: Duration) -> Result<()> {
        match self {
            Sink::Mp4(m) => m.write(pixels, at),
            Sink::Gif(g) => g.write(pixels, at),
        }
    }

    fn finish(self, end: Duration, interval: Duration) -> Result<()> {
        match self {
            Sink::Mp4(m) => m.finish(),
            Sink::Gif(g) => {
                let end = end.max(g.held_at + interval);
                g.finish(end)
            }
        }
    }
}

/// 100-nanosecond units, Media Foundation's clock.
fn hns(d: Duration) -> i64 {
    (d.as_nanos() / 100) as i64
}

struct Mp4 {
    writer: IMFSinkWriter,
    stream: u32,
    width: u32,
    height: u32,
    frame: i64,
}

impl Mp4 {
    fn new(path: &Path, width: u32, height: u32, fps: u32) -> Result<Mp4> {
        // SAFETY: Media Foundation is started before any of it is used and shut down
        // in Drop; every call is on an object created just before.
        unsafe {
            MFStartup(MF_VERSION, MFSTARTUP_FULL).context("starting Media Foundation")?;
            let made = (|| -> Result<Mp4> {
                let mut attrs = None;
                MFCreateAttributes(&mut attrs, 1).context("MFCreateAttributes")?;
                let attrs = attrs.ok_or_else(|| Error::Other("no attributes".into()))?;
                attrs
                    .SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)
                    .context("allowing the graphics card's encoder")?;
                let writer = MFCreateSinkWriterFromURL(&HSTRING::from(path.as_os_str()), None, &attrs)
                    .context(format!("creating {}", path.display()))?;
                let video = |subtype: &GUID| -> Result<IMFMediaType> {
                    let t = MFCreateMediaType().context("MFCreateMediaType")?;
                    t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                        .context("describing the video")?;
                    t.SetGUID(&MF_MT_SUBTYPE, subtype).context("describing the video")?;
                    t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
                        .context("describing the video")?;
                    t.SetUINT64(&MF_MT_FRAME_SIZE, (u64::from(width) << 32) | u64::from(height))
                        .context("describing the video")?;
                    t.SetUINT64(&MF_MT_FRAME_RATE, (u64::from(fps) << 32) | 1)
                        .context("describing the video")?;
                    t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1 << 32) | 1)
                        .context("describing the video")?;
                    Ok(t)
                };
                let out = video(&MFVideoFormat_H264)?;
                // About a tenth of a bit per pixel shown: sharp text without huge files.
                let bitrate = (u64::from(width) * u64::from(height) * u64::from(fps) / 10).clamp(500_000, 40_000_000);
                out.SetUINT32(&MF_MT_AVG_BITRATE, bitrate as u32)
                    .context("describing the video")?;
                let stream = writer.AddStream(&out).context("choosing H.264")?;
                let input = video(&MFVideoFormat_RGB32)?;
                writer
                    .SetInputMediaType(stream, &input, None)
                    .context("this size cannot be recorded as MP4; try a smaller area or GIF")?;
                writer.BeginWriting().context("starting the video")?;
                Ok(Mp4 {
                    writer,
                    stream,
                    width,
                    height,
                    frame: hns(Duration::from_secs(1) / fps),
                })
            })();
            if made.is_err() {
                let _ = MFShutdown();
            }
            made
        }
    }

    fn write(&mut self, pixels: &[u32], at: Duration) -> Result<()> {
        let row = self.width as usize;
        let len = (row * 4 * self.height as usize) as u32;
        // SAFETY: the buffer is locked for `len` bytes while the rows are copied in,
        // and unlocked before it is handed to the writer.
        unsafe {
            let buffer = MFCreateMemoryBuffer(len).context("MFCreateMemoryBuffer")?;
            let mut data = std::ptr::null_mut();
            buffer.Lock(&mut data, None, None).context("locking the frame")?;
            let dst = std::slice::from_raw_parts_mut(data.cast::<u32>(), row * self.height as usize);
            // Uncompressed RGB frames are stored bottom row first.
            for (d, s) in dst.chunks_exact_mut(row).zip(pixels.chunks_exact(row).rev()) {
                d.copy_from_slice(s);
            }
            buffer.Unlock().context("unlocking the frame")?;
            buffer.SetCurrentLength(len).context("copying the frame")?;
            let sample = MFCreateSample().context("MFCreateSample")?;
            sample.AddBuffer(&buffer).context("copying the frame")?;
            sample.SetSampleTime(hns(at)).context("timing the frame")?;
            sample.SetSampleDuration(self.frame).context("timing the frame")?;
            self.writer
                .WriteSample(self.stream, &sample)
                .context("writing a frame")?;
        }
        Ok(())
    }

    fn finish(self) -> Result<()> {
        // SAFETY: the writer was started in `new`.
        unsafe { self.writer.Finalize() }.context("finishing the video")?;
        Ok(())
    }
}

impl Drop for Mp4 {
    fn drop(&mut self) {
        // SAFETY: matches the MFStartup in `new`.
        let _ = unsafe { MFShutdown() };
    }
}

/// A frame is written once the next arrives, so its delay is how long it was really
/// on screen.
struct Gif {
    factory: IWICImagingFactory,
    stream: IWICStream,
    encoder: IWICBitmapEncoder,
    width: u32,
    height: u32,
    held: Vec<u32>,
    held_at: Duration,
}

fn propvariant(vt: VARENUM, set: impl FnOnce(&mut PROPVARIANT_0_0_0)) -> PROPVARIANT {
    let mut v = PROPVARIANT::default();
    // SAFETY: PROPVARIANT is a plain C union; `vt` names the member `set` fills.
    unsafe {
        let inner = &mut v.Anonymous.Anonymous;
        inner.vt = vt;
        set(&mut inner.Anonymous);
    }
    v
}

/// A byte-vector PROPVARIANT. The GIF metadata writer frees the bytes it is given
/// (a Rust-owned buffer there corrupts the heap), so they are allocated the COM way
/// and handed over.
fn bytes(data: &[u8]) -> Result<PROPVARIANT> {
    // SAFETY: `p` is checked for null and has room for `data`.
    let p = unsafe { CoTaskMemAlloc(data.len()) }.cast::<u8>();
    if p.is_null() {
        return Err(Error::Other("out of memory".into()));
    }
    unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len()) };
    Ok(propvariant(VARENUM(VT_VECTOR.0 | VT_UI1.0), |u| {
        u.caub.cElems = data.len() as u32;
        u.caub.pElems = p;
    }))
}

impl Gif {
    fn new(path: &Path, width: u32, height: u32) -> Result<Gif> {
        // SAFETY: each call is on an interface created just before.
        unsafe {
            let factory: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                .context("starting Windows Imaging")?;
            let stream = factory.CreateStream().context("creating the file")?;
            stream
                .InitializeFromFilename(&HSTRING::from(path.as_os_str()), GENERIC_WRITE.0)
                .context(format!("creating {}", path.display()))?;
            let encoder = factory
                .CreateEncoder(&GUID_ContainerFormatGif, std::ptr::null())
                .context("starting the GIF writer")?;
            encoder
                .Initialize(&stream, WICBitmapEncoderNoCache)
                .context("starting the GIF writer")?;
            // Play over and over, as every browser and chat expects of a GIF.
            let meta = encoder.GetMetadataQueryWriter().context("GIF settings")?;
            let app = b"NETSCAPE2.0";
            meta.SetMetadataByName(&HSTRING::from("/appext/application"), &bytes(app)?)
                .context("making the GIF loop")?;
            let data = [3u8, 1, 0, 0, 0];
            meta.SetMetadataByName(&HSTRING::from("/appext/data"), &bytes(&data)?)
                .context("making the GIF loop")?;
            Ok(Gif {
                factory,
                stream,
                encoder,
                width,
                height,
                held: Vec::new(),
                held_at: Duration::ZERO,
            })
        }
    }

    fn write(&mut self, pixels: &[u32], at: Duration) -> Result<()> {
        if !self.held.is_empty() {
            self.put(at - self.held_at)?;
        }
        self.held.clear();
        self.held.extend_from_slice(pixels);
        self.held_at = at;
        Ok(())
    }

    fn put(&self, shown: Duration) -> Result<()> {
        let (w, h) = (self.width, self.height);
        // Hundredths of a second; browsers treat 0 and 1 as "as fast as possible".
        let delay = (shown.as_millis() / 10).clamp(2, u128::from(u16::MAX)) as u16;
        // SAFETY: each call is on an interface created just before; `bytes` covers the
        // held frame, which outlives the bitmap that copies it.
        unsafe {
            let bytes = std::slice::from_raw_parts(self.held.as_ptr().cast::<u8>(), self.held.len() * 4);
            let bitmap = self
                .factory
                .CreateBitmapFromMemory(w, h, &GUID_WICPixelFormat32bppBGR, w * 4, bytes)
                .context("copying the frame")?;
            let palette = self.factory.CreatePalette().context("CreatePalette")?;
            palette
                .InitializeFromBitmap(&bitmap, 256, false)
                .context("choosing the frame's colors")?;
            let indexed = self.factory.CreateFormatConverter().context("CreateFormatConverter")?;
            indexed
                .Initialize(
                    &bitmap,
                    &GUID_WICPixelFormat8bppIndexed,
                    WICBitmapDitherTypeNone,
                    &palette,
                    0.0,
                    WICBitmapPaletteTypeCustom,
                )
                .context("reducing the frame to 256 colors")?;
            let mut out: Option<IWICBitmapFrameEncode> = None;
            let mut props: Option<IPropertyBag2> = None;
            self.encoder
                .CreateNewFrame(&mut out, &mut props)
                .context("adding a frame")?;
            let out = out.ok_or_else(|| Error::Other("the GIF writer gave no frame".into()))?;
            out.Initialize(props.as_ref()).context("adding a frame")?;
            out.SetSize(w, h).context("setting the frame size")?;
            let mut format = GUID_WICPixelFormat8bppIndexed;
            out.SetPixelFormat(&mut format).context("choosing the pixel format")?;
            out.SetPalette(&palette).context("setting the frame's colors")?;
            let meta = out.GetMetadataQueryWriter().context("frame settings")?;
            let delay = propvariant(VT_UI2, |u| u.uiVal = delay);
            meta.SetMetadataByName(&HSTRING::from("/grctlext/Delay"), &delay)
                .context("setting the frame time")?;
            out.WriteSource(&indexed, std::ptr::null()).context("writing a frame")?;
            out.Commit().context("writing a frame")?;
        }
        Ok(())
    }

    fn finish(self, end: Duration) -> Result<()> {
        if !self.held.is_empty() {
            self.put(end - self.held_at)?;
        }
        // SAFETY: the encoder and stream were initialized in `new`.
        unsafe {
            self.encoder.Commit().context("finishing the GIF")?;
            self.stream.Commit(Default::default()).context("finishing the GIF")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mp4_areas_are_even() {
        let r = recorded_area(Rect::new(-5, 3, 100, 54), Format::Mp4);
        assert_eq!((r.width(), r.height()), (104, 50));
        let g = recorded_area(Rect::new(-5, 3, 100, 54), Format::Gif);
        assert_eq!((g.width(), g.height()), (105, 51));
    }

    /// Records the top-left of the real screen for a second in each format and reads
    /// the result back. Run with `--ignored`.
    #[test]
    #[ignore = "records the real screen"]
    fn records_mp4_and_gif() {
        let dir = std::env::temp_dir().join(format!("powerkit-rec-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (format, name) in [(Format::Mp4, "a.mp4"), (Format::Gif, "a.gif")] {
            let path = dir.join(name);
            let rec = Recording::start(Rect::new(0, 0, 321, 241), 15, true, format, &path).unwrap();
            std::thread::sleep(Duration::from_millis(1200));
            assert!(!rec.failed());
            rec.stop().unwrap();
            let len = std::fs::metadata(&path).unwrap().len();
            assert!(len > 1000, "{name} is only {len} bytes");
            eprintln!("{name}: {len} bytes at {}", path.display());
        }
        let gif = crate::image::open(&dir.join("a.gif")).unwrap();
        assert_eq!((gif.width, gif.height), (321, 241));
    }
}
