//! Drawing through Windows: PNG/JPEG and other WIC formats, SVG with Direct2D, PDF with
//! Windows.Data.Pdf. The calling thread must have COM initialized.

use crate::image::{fit_size, Image};
use windows::Data::Pdf::{PdfDocument, PdfPageRenderOptions};
use windows::Storage::Streams::{DataReader, DataWriter, InMemoryRandomAccessStream};
use windows::Win32::Graphics::Direct2D::Common::{D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT, D2D_SIZE_F};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1DeviceContext5, ID2D1Factory, D2D1_FACTORY_TYPE_SINGLE_THREADED,
    D2D1_RENDER_TARGET_PROPERTIES,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICBitmapSource, IWICImagingFactory,
    WICBitmapCacheOnDemand, WICBitmapInterpolationModeFant, WICConvertBitmapSource, WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Shell::SHCreateMemStream;
use windows_core::{Error, Interface, Result};

fn wic() -> Result<IWICImagingFactory> {
    // SAFETY: plain COM activation of a system class.
    unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }
}

fn too_large() -> Error {
    Error::new(windows::Win32::Foundation::E_OUTOFMEMORY, "the picture is too large")
}

/// Copy a WIC source into an image, converting it to premultiplied BGRA and shrinking
/// it to fit `max` by `max` pixels on the way.
fn pixels(factory: &IWICImagingFactory, source: &IWICBitmapSource, max: u32) -> Result<Image> {
    // SAFETY: COM calls on live interfaces; the buffer is the size CopyPixels is told.
    unsafe {
        let (mut w, mut h) = (0, 0);
        source.GetSize(&mut w, &mut h)?;
        let (fw, fh) = fit_size(w, h, max);
        let source: IWICBitmapSource = if (fw, fh) != (w, h) {
            let scaler = factory.CreateBitmapScaler()?;
            scaler.Initialize(source, fw, fh, WICBitmapInterpolationModeFant)?;
            scaler.cast()?
        } else {
            source.clone()
        };
        let converted = WICConvertBitmapSource(&GUID_WICPixelFormat32bppPBGRA, &source)?;
        let mut img = Image::new(fw, fh).ok_or_else(too_large)?;
        converted.CopyPixels(std::ptr::null(), fw * 4, &mut img.pixels)?;
        Ok(img)
    }
}

/// Decode a picture in any format Windows reads (PNG, JPEG, BMP, GIF, ...).
pub fn decode(bytes: &[u8], max: u32) -> Result<Image> {
    let factory = wic()?;
    // SAFETY: COM calls on live interfaces; `bytes` outlives the stream reading it.
    unsafe {
        let stream = factory.CreateStream()?;
        stream.InitializeFromMemory(bytes)?;
        let decoder = factory.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)?;
        let frame = decoder.GetFrame(0)?;
        pixels(&factory, &frame.cast()?, max)
    }
}

/// Draw an SVG so it fits `max` by `max` pixels, on transparency.
pub fn svg(source: &str, max: u32) -> Result<Image> {
    let (text, w, h) = crate::svg::normalize(source)
        .ok_or_else(|| Error::new(windows::Win32::Foundation::E_INVALIDARG, "this is not an SVG picture"))?;
    let scale = max as f32 / w.max(h);
    let (pw, ph) = (
        ((w * scale).round() as u32).clamp(1, max),
        ((h * scale).round() as u32).clamp(1, max),
    );
    let factory = wic()?;
    // SAFETY: COM calls on live interfaces; the stream copies `text`.
    unsafe {
        let bitmap = factory.CreateBitmap(pw, ph, &GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnDemand)?;
        let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            ..Default::default()
        };
        let target = d2d.CreateWicBitmapRenderTarget(&bitmap, &props)?;
        let context: ID2D1DeviceContext5 = target.cast()?;
        let stream = SHCreateMemStream(Some(text.as_bytes()))
            .ok_or_else(|| Error::new(windows::Win32::Foundation::E_OUTOFMEMORY, "no memory for the picture"))?;
        let document = context.CreateSvgDocument(
            &stream,
            D2D_SIZE_F {
                width: pw as f32,
                height: ph as f32,
            },
        )?;
        context.BeginDraw();
        context.Clear(None);
        context.DrawSvgDocument(&document);
        context.EndDraw(None, None)?;
        let mut img = Image::new(pw, ph).ok_or_else(too_large)?;
        bitmap.CopyPixels(std::ptr::null(), pw * 4, &mut img.pixels)?;
        Ok(img)
    }
}

/// An open PDF whose pages are drawn on demand.
pub struct Pdf {
    document: PdfDocument,
    /// Each page's size in device-independent pixels (1/96 inch).
    pub pages: Vec<(f32, f32)>,
}

impl Pdf {
    pub fn open(bytes: &[u8]) -> Result<Pdf> {
        let stream = InMemoryRandomAccessStream::new()?;
        let writer = DataWriter::CreateDataWriter(&stream)?;
        writer.WriteBytes(bytes)?;
        writer.StoreAsync()?.join()?;
        writer.DetachStream()?;
        stream.Seek(0)?;
        let document = PdfDocument::LoadFromStreamAsync(&stream)?.join()?;
        let pages = (0..document.PageCount()?)
            .map(|i| {
                let size = document.GetPage(i)?.Size()?;
                Ok((size.Width, size.Height))
            })
            .collect::<Result<_>>()?;
        Ok(Pdf { document, pages })
    }

    /// Page `index` drawn `width` pixels wide, on white as printed.
    pub fn page(&self, index: usize, width: u32) -> Result<Image> {
        let page = self.document.GetPage(index as u32)?;
        let options = PdfPageRenderOptions::new()?;
        options.SetDestinationWidth(width.max(1))?;
        let out = InMemoryRandomAccessStream::new()?;
        page.RenderWithOptionsToStreamAsync(&out, &options)?.join()?;
        let size = u32::try_from(out.Size()?).map_err(|_| too_large())?;
        let reader = DataReader::CreateDataReader(&out.GetInputStreamAt(0)?)?;
        reader.LoadAsync(size)?.join()?;
        let mut encoded = vec![0u8; size as usize];
        reader.ReadBytes(&mut encoded)?;
        decode(&encoded, u32::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};

    /// COM for one test thread, single-threaded as in the preview host, and closed again
    /// when the test ends: with multithreaded COM the PDF renderer's threads keep the
    /// test process from exiting.
    struct Com;

    impl Drop for Com {
        fn drop(&mut self) {
            // SAFETY: matches the CoInitializeEx in com(); every COM object of the test
            // was created after the guard and so is dropped before it.
            unsafe { CoUninitialize() };
        }
    }

    fn com() -> Com {
        // SAFETY: initializing COM for the test thread.
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
            .ok()
            .expect("COM starts");
        Com
    }

    #[test]
    fn draws_an_svg_to_the_size_asked() {
        let _com = com();
        let img = svg(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><rect x="10" width="10" height="10" fill="#ff0000"/></svg>"##,
            100,
        )
        .unwrap();
        assert_eq!((img.width, img.height), (100, 50));
        assert_eq!(&img.pixels[..4], &[0, 0, 0, 0], "the left half is empty");
        let right = ((25 * 100 + 75) * 4) as usize;
        assert_eq!(
            &img.pixels[right..right + 4],
            &[0, 0, 255, 255],
            "the right half is red"
        );
    }

    #[test]
    fn decodes_what_it_drew_as_png_through_a_pdf() {
        let _com = com();
        // A one-page PDF, 200 by 100 points, with a black rectangle on the left half.
        let content = "0 0 0 rg 0 0 100 100 re f";
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R >>".to_string(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        ];
        let mut pdf = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).bytes());
        }
        let xref = pdf.len();
        pdf.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
        for o in offsets {
            pdf.extend(format!("{o:010} 00000 n \n").bytes());
        }
        pdf.extend(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .bytes(),
        );
        let doc = Pdf::open(&pdf).unwrap();
        assert_eq!(doc.pages.len(), 1);
        assert!((doc.pages[0].0 - 200.0 * 96.0 / 72.0).abs() < 0.01);
        let page = doc.page(0, 400).unwrap();
        assert_eq!((page.width, page.height), (400, 200));
        let at = |x: usize, y: usize| page.pixels[(y * 400 + x) * 4..][..4].to_vec();
        assert_eq!(at(50, 100), vec![0, 0, 0, 255]);
        assert_eq!(at(350, 100), vec![255, 255, 255, 255]);
        assert!(Pdf::open(b"not a pdf").is_err());
    }
}
