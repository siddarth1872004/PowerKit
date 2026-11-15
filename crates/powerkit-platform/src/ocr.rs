//! Reading text in a picture with the OCR engine built into Windows.

use crate::{Context, Error, Result};
use windows::core::HSTRING;
use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Security::Cryptography::CryptographicBuffer;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

/// The largest width or height the engine reads; bigger pictures must be scaled down.
pub fn max_dimension() -> u32 {
    // 2600 is the documented limit, should the engine not say.
    OcrEngine::MaxImageDimension().unwrap_or(2600)
}

/// Tags and names of the languages that have OCR installed.
pub fn languages() -> Result<Vec<(String, String)>> {
    let list = OcrEngine::AvailableRecognizerLanguages().context("listing the OCR languages")?;
    let mut out = Vec::new();
    for i in 0..list.Size().context("listing the OCR languages")? {
        let l = list.GetAt(i).context("listing the OCR languages")?;
        out.push((
            l.LanguageTag().map(|t| t.to_string()).unwrap_or_default(),
            l.DisplayName().map(|t| t.to_string()).unwrap_or_default(),
        ));
    }
    Ok(out)
}

fn engine(language: &str) -> Result<OcrEngine> {
    if language.is_empty() {
        return OcrEngine::TryCreateFromUserProfileLanguages()
            .map_err(|_| Error::Other("Windows has no text recognition for its display languages".into()));
    }
    let lang = Language::CreateLanguage(&HSTRING::from(language)).context("reading the language")?;
    if !OcrEngine::IsLanguageSupported(&lang).unwrap_or(false) {
        let installed: Vec<String> = languages()?.into_iter().map(|(tag, _)| tag).collect();
        return Err(Error::Other(format!(
            "Text recognition for {language} is not installed (Settings > Time & language > Language & region; installed: {})",
            installed.join(", ")
        )));
    }
    OcrEngine::TryCreateFromLanguage(&lang).context("starting text recognition")
}

/// The lines of text in a `width` x `height` picture of `0x00RRGGBB` pixels.
pub fn recognize(pixels: &[u32], width: u32, height: u32, language: &str) -> Result<Vec<String>> {
    if pixels.len() != width as usize * height as usize || width == 0 || height == 0 {
        return Err(Error::Other("the picture is empty".into()));
    }
    // Already initialised in a compatible way is fine; the calls below report any
    // real failure.
    // SAFETY: no pointers are passed.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let engine = engine(language)?;
    // Little-endian 0x00RRGGBB is B, G, R, 0 in memory: BGRA once alpha is set.
    let bytes: Vec<u8> = pixels.iter().flat_map(|p| (p | 0xFF00_0000).to_le_bytes()).collect();
    let buffer = CryptographicBuffer::CreateFromByteArray(&bytes).context("copying the picture")?;
    let bitmap = SoftwareBitmap::CreateCopyFromBuffer(&buffer, BitmapPixelFormat::Bgra8, width as i32, height as i32)
        .context("copying the picture")?;
    let result = engine
        .RecognizeAsync(&bitmap)
        .and_then(|op| op.join())
        .context("reading the text")?;
    let lines = result.Lines().context("reading the text")?;
    let mut out = Vec::new();
    for i in 0..lines.Size().context("reading the text")? {
        out.push(
            lines
                .GetAt(i)
                .and_then(|l| l.Text())
                .context("reading the text")?
                .to_string(),
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::core::w;
    use windows::Win32::Graphics::Gdi::*;

    /// `text` drawn black on white by GDI, as 0x00RRGGBB pixels.
    fn draw(text: &str, w: i32, h: i32) -> Vec<u32> {
        let mut px = vec![0x00FF_FFFFu32; (w * h) as usize];
        // SAFETY: every handle made here is released here; the bitmap bits are
        // copied out before the bitmap is deleted.
        unsafe {
            let dc = CreateCompatibleDC(None);
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bmp = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0).unwrap();
            std::ptr::copy_nonoverlapping(px.as_ptr(), bits as *mut u32, px.len());
            let old = SelectObject(dc, bmp.into());
            let font = CreateFontW(
                48,
                0,
                0,
                0,
                400,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                ANTIALIASED_QUALITY,
                0,
                w!("Segoe UI"),
            );
            let old_font = SelectObject(dc, font.into());
            SetTextColor(dc, windows::Win32::Foundation::COLORREF(0));
            SetBkMode(dc, TRANSPARENT);
            let wide: Vec<u16> = text.encode_utf16().collect();
            let _ = TextOutW(dc, 20, 20, &wide);
            let _ = GdiFlush();
            std::ptr::copy_nonoverlapping(bits as *const u32, px.as_mut_ptr(), px.len());
            SelectObject(dc, old_font);
            SelectObject(dc, old);
            let _ = DeleteObject(font.into());
            let _ = DeleteObject(bmp.into());
            let _ = DeleteDC(dc);
        }
        px.iter().map(|p| p & 0x00FF_FFFF).collect()
    }

    // Needs the OCR feature of a Windows display language, which build servers may lack.
    #[test]
    #[ignore]
    fn reads_drawn_text() {
        let px = draw("Hello PowerKit 42", 600, 100);
        let lines = recognize(&px, 600, 100, "").unwrap();
        assert_eq!(lines, ["Hello PowerKit 42"]);
        assert!(recognize(&vec![0xFFFFFF; 100], 10, 10, "").unwrap().is_empty());
        assert!(recognize(&[], 0, 0, "").is_err());
    }
}
