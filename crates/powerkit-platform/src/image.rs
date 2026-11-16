//! Resizing pictures with the Windows Imaging Component, which reads every format
//! Windows has a codec for and writes BMP, GIF, JPEG, PNG and TIFF.

use crate::{Context, Error, Result};
use std::path::{Path, PathBuf};
use windows::core::{GUID, HSTRING, PWSTR};
use windows::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::StructuredStorage::{IPropertyBag2, PROPBAG2, PROPVARIANT};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::System::Variant::VARIANT;

/// The formats pictures are written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Bmp,
    Gif,
    Jpeg,
    Png,
    Tiff,
}

const FORMATS: [(Format, GUID, &str); 5] = [
    (Format::Bmp, GUID_ContainerFormatBmp, "bmp"),
    (Format::Gif, GUID_ContainerFormatGif, "gif"),
    (Format::Jpeg, GUID_ContainerFormatJpeg, "jpg"),
    (Format::Png, GUID_ContainerFormatPng, "png"),
    (Format::Tiff, GUID_ContainerFormatTiff, "tif"),
];

impl Format {
    fn container(self) -> GUID {
        FORMATS.iter().find(|f| f.0 == self).map(|f| f.1).unwrap_or_default()
    }

    /// File extension for a picture saved in this format, without the dot.
    pub fn extension(self) -> &'static str {
        FORMATS.iter().find(|f| f.0 == self).map_or("", |f| f.2)
    }

    /// Whether the format holds more than one frame (animation or pages).
    fn frames(self) -> bool {
        matches!(self, Format::Gif | Format::Tiff)
    }
}

/// How a picture is saved.
#[derive(Debug, Clone, Copy)]
pub struct SaveOptions {
    pub format: Format,
    /// Scale the picture to this size...
    pub size: (u32, u32),
    /// ...then keep this rectangle (x, y, width, height) of it.
    pub crop: Option<(u32, u32, u32, u32)>,
    /// JPEG quality, 1 to 100.
    pub quality: u32,
    pub keep_metadata: bool,
}

/// A picture opened for resizing.
pub struct Picture {
    factory: IWICImagingFactory,
    decoder: IWICBitmapDecoder,
    pub width: u32,
    pub height: u32,
    /// Horizontal and vertical resolution in dots per inch, as the file says.
    pub dpi: (f64, f64),
    /// The picture's own format, when it is one PowerKit writes.
    pub format: Option<Format>,
}

const INDEXED: [GUID; 4] = [
    GUID_WICPixelFormat1bppIndexed,
    GUID_WICPixelFormat2bppIndexed,
    GUID_WICPixelFormat4bppIndexed,
    GUID_WICPixelFormat8bppIndexed,
];

/// Open `path` and read its size.
pub fn open(path: &Path) -> Result<Picture> {
    // SAFETY: each call is on an interface created just before; the path outlives the
    // call that reads it.
    unsafe {
        // Already initialised in a compatible way is fine; the calls below report any
        // real failure.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let factory: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
            .context("starting Windows Imaging")?;
        let decoder = factory
            .CreateDecoderFromFilename(
                &HSTRING::from(path.as_os_str()),
                None,
                GENERIC_READ,
                WICDecodeMetadataCacheOnDemand,
            )
            .context("Windows cannot read this picture")?;
        let container = decoder.GetContainerFormat().context("reading the picture format")?;
        let frame = decoder.GetFrame(0).context("reading the picture")?;
        let (mut width, mut height, mut dx, mut dy) = (0, 0, 0.0, 0.0);
        frame
            .GetSize(&mut width, &mut height)
            .context("reading the picture size")?;
        frame
            .GetResolution(&mut dx, &mut dy)
            .context("reading the picture resolution")?;
        Ok(Picture {
            factory,
            decoder,
            width,
            height,
            dpi: (dx, dy),
            format: FORMATS.iter().find(|f| f.1 == container).map(|f| f.0),
        })
    }
}

/// `dst` with `.pk-resize` added: written first, so a failure never leaves half a
/// picture under the real name, and the original can be read until the end.
fn temp_path(dst: &Path) -> PathBuf {
    let mut name = dst.as_os_str().to_owned();
    name.push(".pk-resize");
    PathBuf::from(name)
}

impl Picture {
    /// The first frame as premultiplied BGRA pixels, row by row, at `size`.
    pub fn pixels(&self, size: (u32, u32)) -> Result<Vec<u32>> {
        let (w, h) = (size.0.max(1), size.1.max(1));
        // SAFETY: each call is on an interface created just before; the buffer's
        // length in bytes is passed with it.
        unsafe {
            let frame = self.decoder.GetFrame(0).context("reading the picture")?;
            let pbgra = self.factory.CreateFormatConverter().context("converting the picture")?;
            pbgra
                .Initialize(
                    &frame,
                    &GUID_WICPixelFormat32bppPBGRA,
                    WICBitmapDitherTypeNone,
                    None,
                    0.0,
                    WICBitmapPaletteTypeCustom,
                )
                .context("converting the picture")?;
            let scaler = self.factory.CreateBitmapScaler().context("scaling the picture")?;
            scaler
                .Initialize(&pbgra, w, h, WICBitmapInterpolationModeHighQualityCubic)
                .context("scaling the picture")?;
            let mut pixels = vec![0u32; (w * h) as usize];
            let bytes = std::slice::from_raw_parts_mut(pixels.as_mut_ptr().cast::<u8>(), pixels.len() * 4);
            scaler
                .CopyPixels(std::ptr::null(), w * 4, bytes)
                .context("reading the picture's pixels")?;
            Ok(pixels)
        }
    }

    /// Save the resized picture as `dst`, replacing any file there (the picture itself
    /// included).
    pub fn save(self, dst: &Path, options: &SaveOptions) -> Result<()> {
        let temp = temp_path(dst);
        let written = self.write(&temp, options);
        // The decoder keeps the original open; it must be closed before replacing it.
        drop(self);
        let result = written.and_then(|()| {
            std::fs::rename(&temp, dst).map_err(|e| Error::Other(format!("saving {}: {e}", dst.display())))
        });
        if result.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        result
    }

    fn write(&self, path: &Path, options: &SaveOptions) -> Result<()> {
        // SAFETY: each call is on an interface created just before; every pointer
        // passed points at a local that outlives the call.
        unsafe {
            let stream = self.factory.CreateStream().context("creating the file")?;
            stream
                .InitializeFromFilename(&HSTRING::from(path.as_os_str()), GENERIC_WRITE.0)
                .context(format!("creating {}", path.display()))?;
            let container = options.format.container();
            let encoder = self
                .factory
                .CreateEncoder(&container, std::ptr::null())
                .context("starting the picture writer")?;
            encoder
                .Initialize(&stream, WICBitmapEncoderNoCache)
                .context("starting the picture writer")?;
            let same = self.format == Some(options.format);
            let frames = if options.format.frames() {
                self.decoder.GetFrameCount().context("reading the picture")?
            } else {
                1
            };
            for i in 0..frames {
                let frame = self.decoder.GetFrame(i).context("reading the picture")?;
                self.write_frame(&encoder, &frame, options, same && options.keep_metadata)?;
            }
            encoder.Commit().context("writing the picture")?;
            stream.Commit(Default::default()).context("writing the picture")?;
        }
        Ok(())
    }

    /// # Safety
    /// `encoder` must be initialised and `frame` come from this picture's decoder.
    unsafe fn write_frame(
        &self,
        encoder: &IWICBitmapEncoder,
        frame: &IWICBitmapFrameDecode,
        options: &SaveOptions,
        copy_metadata: bool,
    ) -> Result<()> {
        // SAFETY: as for `write`.
        unsafe {
            let (mut fw, mut fh, mut dx, mut dy) = (0, 0, 0.0, 0.0);
            frame.GetSize(&mut fw, &mut fh).context("reading the picture size")?;
            frame
                .GetResolution(&mut dx, &mut dy)
                .context("reading the picture resolution")?;
            // Later frames of an animation can be smaller pieces; they scale by the same
            // factor and are not cropped.
            let first = (fw, fh) == (self.width, self.height);
            let sx = options.size.0 as f64 / self.width.max(1) as f64;
            let sy = options.size.1 as f64 / self.height.max(1) as f64;
            let (w, h) = if first {
                options.size
            } else {
                (
                    ((fw as f64 * sx).round() as u32).max(1),
                    ((fh as f64 * sy).round() as u32).max(1),
                )
            };

            let pbgra = self.factory.CreateFormatConverter().context("converting the picture")?;
            pbgra
                .Initialize(
                    frame,
                    &GUID_WICPixelFormat32bppPBGRA,
                    WICBitmapDitherTypeNone,
                    None,
                    0.0,
                    WICBitmapPaletteTypeCustom,
                )
                .context("converting the picture")?;
            let scaler = self.factory.CreateBitmapScaler().context("scaling the picture")?;
            scaler
                .Initialize(&pbgra, w, h, WICBitmapInterpolationModeHighQualityCubic)
                .context("scaling the picture")?;
            let mut source: IWICBitmapSource = scaler.into();
            let (mut out_w, mut out_h) = (w, h);
            if let (true, Some((x, y, cw, ch))) = (first, options.crop) {
                let clipper = self.factory.CreateBitmapClipper().context("cropping the picture")?;
                let rect = WICRect {
                    X: x as i32,
                    Y: y as i32,
                    Width: cw as i32,
                    Height: ch as i32,
                };
                clipper.Initialize(&source, &rect).context("cropping the picture")?;
                source = clipper.into();
                (out_w, out_h) = (cw, ch);
            }

            let mut out: Option<IWICBitmapFrameEncode> = None;
            let mut props: Option<IPropertyBag2> = None;
            encoder.CreateNewFrame(&mut out, &mut props).context("adding a frame")?;
            let out = out.ok_or_else(|| Error::Other("the picture writer gave no frame".into()))?;
            if let (Format::Jpeg, Some(props)) = (options.format, &props) {
                let mut name: Vec<u16> = "ImageQuality".encode_utf16().chain([0]).collect();
                let bag = PROPBAG2 {
                    pstrName: PWSTR(name.as_mut_ptr()),
                    ..Default::default()
                };
                let value = VARIANT::from(options.quality.clamp(1, 100) as f32 / 100.0);
                props.Write(1, &bag, &value).context("setting the JPEG quality")?;
            }
            out.Initialize(props.as_ref()).context("adding a frame")?;
            out.SetSize(out_w, out_h).context("setting the picture size")?;
            out.SetResolution(dx, dy).context("setting the picture resolution")?;
            if copy_metadata {
                if let Err(e) = copy_metadata_blocks(frame, &out, options.format, (sx, sy)) {
                    tracing::warn!("picture metadata not copied: {e}");
                }
            }
            let mut format = GUID_WICPixelFormat32bppBGRA;
            out.SetPixelFormat(&mut format).context("choosing the pixel format")?;
            if format != GUID_WICPixelFormat32bppPBGRA {
                let palette = if INDEXED.contains(&format) {
                    let palette = self.factory.CreatePalette().context("making a palette")?;
                    // An indexed original keeps its own colours, so a transparent colour
                    // named in its metadata stays the same entry.
                    let own = frame
                        .GetPixelFormat()
                        .is_ok_and(|f| INDEXED.contains(&f))
                        .then(|| frame.CopyPalette(&palette))
                        .is_some_and(|r| r.is_ok());
                    if !own {
                        palette
                            .InitializeFromBitmap(&source, 256, true)
                            .context("making a palette")?;
                    }
                    out.SetPalette(&palette).context("setting the palette")?;
                    Some(palette)
                } else {
                    None
                };
                let converter = self.factory.CreateFormatConverter().context("converting the picture")?;
                converter
                    .Initialize(
                        &source,
                        &format,
                        if palette.is_some() {
                            WICBitmapDitherTypeErrorDiffusion
                        } else {
                            WICBitmapDitherTypeNone
                        },
                        palette.as_ref(),
                        50.0,
                        WICBitmapPaletteTypeCustom,
                    )
                    .context("converting the picture")?;
                source = converter.into();
            }
            out.WriteSource(&source, std::ptr::null())
                .context("writing the picture")?;
            out.Commit().context("writing the picture")?;
        }
        Ok(())
    }
}

/// Copy the original frame's metadata (camera details, colour profile, animation
/// timing) to the new one. A GIF frame's position is scaled with the picture.
///
/// # Safety
/// `out` must be initialised and not yet written.
unsafe fn copy_metadata_blocks(
    frame: &IWICBitmapFrameDecode,
    out: &IWICBitmapFrameEncode,
    format: Format,
    scale: (f64, f64),
) -> windows::core::Result<()> {
    use windows::core::Interface;
    // SAFETY: both frames are live; the caller guarantees `out` accepts metadata.
    unsafe {
        let reader: IWICMetadataBlockReader = frame.cast()?;
        let writer: IWICMetadataBlockWriter = out.cast()?;
        writer.InitializeFromBlockReader(&reader)?;
        if format == Format::Gif {
            let query = out.GetMetadataQueryWriter()?;
            for (name, s) in [("/imgdesc/Left", scale.0), ("/imgdesc/Top", scale.1)] {
                let name = HSTRING::from(name);
                let mut v = PROPVARIANT::default();
                if query.GetMetadataByName(&name, &mut v).is_ok() {
                    if let Ok(at) = u16::try_from(&v) {
                        let scaled = PROPVARIANT::from((at as f64 * s).round().min(u16::MAX as f64) as u16);
                        query.SetMetadataByName(&name, &scaled)?;
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 4 x 2 PNG: left half red, right half blue.
    fn sample(path: &Path) {
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        // SAFETY: as in `Picture::write`.
        unsafe {
            let factory: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).unwrap();
            let pixels: Vec<u8> = (0..8)
                .flat_map(|i| if i % 4 < 2 { [0, 0, 255, 255] } else { [255, 0, 0, 255] })
                .collect();
            let bitmap = factory
                .CreateBitmapFromMemory(4, 2, &GUID_WICPixelFormat32bppBGRA, 16, &pixels)
                .unwrap();
            let stream = factory.CreateStream().unwrap();
            stream
                .InitializeFromFilename(&HSTRING::from(path.as_os_str()), GENERIC_WRITE.0)
                .unwrap();
            let encoder = factory
                .CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())
                .unwrap();
            encoder.Initialize(&stream, WICBitmapEncoderNoCache).unwrap();
            let mut frame = None;
            encoder.CreateNewFrame(&mut frame, std::ptr::null_mut()).unwrap();
            let frame = frame.unwrap();
            frame.Initialize(None).unwrap();
            frame.WriteSource(&bitmap, std::ptr::null()).unwrap();
            frame.Commit().unwrap();
            encoder.Commit().unwrap();
        }
    }

    #[test]
    fn pictures_are_scaled_cropped_and_converted() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.png");
        sample(&src);
        let p = open(&src).unwrap();
        assert_eq!((p.width, p.height, p.format), (4, 2, Some(Format::Png)));
        let dst = dir.path().join("b.jpg");
        let options = SaveOptions {
            format: Format::Jpeg,
            size: (8, 4),
            crop: Some((2, 0, 4, 4)),
            quality: 80,
            keep_metadata: true,
        };
        p.save(&dst, &options).unwrap();
        let q = open(&dst).unwrap();
        assert_eq!((q.width, q.height, q.format), (4, 4, Some(Format::Jpeg)));
        drop(q);

        // Replacing the picture itself, as a GIF.
        let options = SaveOptions {
            format: Format::Gif,
            size: (2, 1),
            crop: None,
            ..options
        };
        open(&dst).unwrap().save(&dst, &options).unwrap();
        let r = open(&dst).unwrap();
        assert_eq!((r.width, r.height, r.format), (2, 1, Some(Format::Gif)));
        assert!(!temp_path(&dst).exists());
        assert!(open(&dir.path().join("missing.png")).is_err());
    }
}
