//! Screen sampling and the Color Picker magnifier: a small topmost, click-through
//! window that shows the pixels around the cursor enlarged, with a label underneath.
//! The window always sits beside the sampled area, so it never captures itself.

use crate::util::{pcwstr, wide};
use crate::{Context, Error, Result};
use powerkit_core::Rect;
use std::marker::PhantomData;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

const CLASS_NAME: &str = "PowerKit.Magnifier";
/// Gap between the sampled area and the magnifier, in 96-DPI pixels.
const GAP: u32 = 16;
const LABEL_HEIGHT: u32 = 24;
const FONT_SIZE: u32 = 13;
const BORDER: u32 = 0x00_80_80_80;

/// Pixels as `0x00RRGGBB`, row by row.
pub type Pixels = Vec<u32>;

/// Copy a screen area. Parts outside every monitor come back black.
pub fn capture(area: Rect) -> Result<Pixels> {
    let (w, h) = (area.width(), area.height());
    if w <= 0 || h <= 0 {
        return Err(Error::Other("empty capture area".into()));
    }
    let mut dib = Dib::new(w, h)?;
    // SAFETY: the screen DC is released on every path; `dib` owns its own DC.
    unsafe {
        let screen = GetDC(None);
        if screen.is_invalid() {
            return Err(Error::Other("GetDC(screen) failed".into()));
        }
        let blt = BitBlt(
            dib.dc,
            0,
            0,
            w,
            h,
            Some(screen),
            area.left,
            area.top,
            SRCCOPY | CAPTUREBLT,
        );
        ReleaseDC(None, screen);
        blt.context("BitBlt(screen)")?;
    }
    Ok(dib.pixels().iter().map(|p| p & 0x00FF_FFFF).collect())
}

/// A top-down 32-bit DIB selected into its own memory DC.
pub(crate) struct Dib {
    pub(crate) dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u32,
    len: usize,
}

impl Dib {
    pub(crate) fn new(w: i32, h: i32) -> Result<Dib> {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        // SAFETY: `info` describes a valid 32-bit top-down bitmap; the DC and bitmap are
        // released by `Drop`, or here if creation fails half way.
        unsafe {
            let dc = CreateCompatibleDC(None);
            if dc.is_invalid() {
                return Err(Error::Other("CreateCompatibleDC failed".into()));
            }
            let mut bits = std::ptr::null_mut();
            let bitmap = match CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(b) if !bits.is_null() => b,
                Ok(b) => {
                    let _ = DeleteObject(b.into());
                    let _ = DeleteDC(dc);
                    return Err(Error::Other("CreateDIBSection returned no pixels".into()));
                }
                Err(e) => {
                    let _ = DeleteDC(dc);
                    return Err(e).context("CreateDIBSection");
                }
            };
            let old = SelectObject(dc, bitmap.into());
            Ok(Dib {
                dc,
                bitmap,
                old,
                bits: bits as *mut u32,
                len: (w * h) as usize,
            })
        }
    }

    fn pixels(&mut self) -> &mut [u32] {
        // GDI may still be drawing into the bitmap asynchronously.
        // SAFETY: GdiFlush has no preconditions.
        let _ = unsafe { GdiFlush() };
        // SAFETY: `bits` points to `len` pixels owned by `bitmap`, which lives as long as self.
        unsafe { std::slice::from_raw_parts_mut(self.bits, self.len) }
    }
}

impl Drop for Dib {
    fn drop(&mut self) {
        // SAFETY: objects created in `new`; the original bitmap is restored before deletion.
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

/// Enlarge a `side`x`side` sample by `zoom` into `dst` (row stride `side * zoom`),
/// optionally with grid lines, and frame the center pixel.
pub(crate) fn render_zoom(src: &[u32], side: usize, zoom: usize, grid: bool, dst: &mut [u32]) {
    let w = side * zoom;
    for y in 0..w {
        for x in 0..w {
            let p = src[(y / zoom) * side + x / zoom];
            let on_line = grid && zoom >= 4 && (x % zoom == 0 || y % zoom == 0);
            dst[y * w + x] = if on_line { (p >> 1) & 0x007F_7F7F } else { p };
        }
    }
    let center = src[(side / 2) * side + side / 2];
    let frame = contrast(center);
    let (lo, hi) = ((side / 2) * zoom, (side / 2 + 1) * zoom - 1);
    for i in lo..=hi {
        for (x, y) in [(i, lo), (i, hi), (lo, i), (hi, i)] {
            dst[y * w + x] = frame;
        }
    }
}

/// Black or white, whichever reads better on `rgb`.
pub(crate) fn contrast(rgb: u32) -> u32 {
    let (r, g, b) = ((rgb >> 16) & 0xFF, (rgb >> 8) & 0xFF, rgb & 0xFF);
    if r * 299 + g * 587 + b * 114 > 128_000 {
        0
    } else {
        0x00FF_FFFF
    }
}

/// Top-left corner for a `w`x`h` window beside the area within `reach` pixels of the
/// cursor: below-right by default, flipped to the other side near monitor edges.
pub(crate) fn place(cursor: (i32, i32), reach: i32, (w, h): (i32, i32), monitor: Rect) -> (i32, i32) {
    let (x, y) = cursor;
    let mut left = x + reach;
    if left + w > monitor.right {
        left = x - reach - w;
    }
    let mut top = y + reach;
    if top + h > monitor.bottom {
        top = y - reach - h;
    }
    (left.max(monitor.left), top.max(monitor.top))
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        // Content is redrawn on every update; nothing to erase.
        WM_ERASEBKGND => LRESULT(1),
        // SAFETY: forwarding the unmodified arguments of this callback.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

pub struct Magnifier {
    hwnd: HWND,
    /// Label font and the DPI it was created for.
    font: Option<(u32, HFONT)>,
    visible: bool,
    _not_send: PhantomData<*const ()>,
}

impl Magnifier {
    pub fn new() -> Result<Magnifier> {
        let class = wide(CLASS_NAME);
        // SAFETY: `class` outlives the calls; `wndproc` matches the WNDPROC contract.
        let hwnd = unsafe {
            let instance = GetModuleHandleW(None).context("GetModuleHandle")?;
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: instance.into(),
                lpszClassName: pcwstr(&class),
                ..Default::default()
            };
            RegisterClassW(&wc); // "already registered" is expected after the first one
            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT | WS_EX_LAYERED,
                pcwstr(&class),
                pcwstr(&class),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance.into()),
                None,
            )
            .context("CreateWindowEx(magnifier)")?;
            if let Err(e) = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA) {
                let _ = DestroyWindow(hwnd);
                return Err(e).context("SetLayeredWindowAttributes");
            }
            hwnd
        };
        Ok(Magnifier {
            hwnd,
            font: None,
            visible: false,
            _not_send: PhantomData,
        })
    }

    fn font(&mut self, dpi: u32, size: i32) -> HFONT {
        match self.font {
            Some((d, f)) if d == dpi => f,
            _ => {
                let face = wide("Segoe UI");
                // SAFETY: `face` outlives the call; the previous font is no longer
                // selected anywhere (each draw selects and restores it).
                let f = unsafe {
                    if let Some((_, old)) = self.font.take() {
                        let _ = DeleteObject(old.into());
                    }
                    CreateFontW(
                        -size,
                        0,
                        0,
                        0,
                        FW_SEMIBOLD.0 as i32,
                        0,
                        0,
                        0,
                        DEFAULT_CHARSET,
                        OUT_DEFAULT_PRECIS,
                        CLIP_DEFAULT_PRECIS,
                        CLEARTYPE_QUALITY,
                        0,
                        pcwstr(&face),
                    )
                };
                self.font = Some((dpi, f));
                f
            }
        }
    }

    /// Show `sample` (a square of `side` pixels centred on `cursor`) enlarged by `zoom`
    /// with `label` underneath on the center color.
    pub fn show(
        &mut self,
        cursor: (i32, i32),
        sample: &[u32],
        side: u32,
        zoom: u32,
        grid: bool,
        label: &str,
    ) -> Result<()> {
        if side == 0 || sample.len() != (side * side) as usize {
            return Err(Error::Other("sample does not match its size".into()));
        }
        let monitor = crate::monitor::from_point(cursor.0, cursor.1)
            .ok_or_else(|| Error::Other("the cursor is not on a monitor".into()))?;
        let zoomed = (side * zoom) as i32;
        let label_h = monitor.scale(LABEL_HEIGHT);
        let (w, h) = (zoomed, zoomed + label_h);
        let reach = side as i32 / 2 + monitor.scale(GAP);
        let (left, top) = place(cursor, reach, (w, h), monitor.rect);

        let center = sample[(side * side / 2) as usize];
        let mut dib = Dib::new(w, h)?;
        let px = dib.pixels();
        render_zoom(sample, side as usize, zoom as usize, grid, &mut px[..(w * w) as usize]);
        px[(w * w) as usize..].fill(center);
        for x in 0..w as usize {
            px[x] = BORDER;
            px[(h as usize - 1) * w as usize + x] = BORDER;
        }
        for y in 0..h as usize {
            px[y * w as usize] = BORDER;
            px[y * w as usize + w as usize - 1] = BORDER;
        }

        let font = self.font(monitor.dpi, monitor.scale(FONT_SIZE));
        let mut text: Vec<u16> = label.encode_utf16().collect();
        let fg = contrast(center);
        // SAFETY: drawing into our own DIB and window; every DC obtained is released and
        // the original font restored.
        unsafe {
            let old = SelectObject(dib.dc, font.into());
            SetBkMode(dib.dc, TRANSPARENT);
            SetTextColor(dib.dc, COLORREF(fg));
            let mut rc = RECT {
                left: 0,
                top: zoomed,
                right: w,
                bottom: h,
            };
            DrawTextW(
                dib.dc,
                &mut text,
                &mut rc,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
            SelectObject(dib.dc, old);

            SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                left,
                top,
                w,
                h,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            )
            .context("SetWindowPos(magnifier)")?;
            self.visible = true;
            let dc = GetDC(Some(self.hwnd));
            if dc.is_invalid() {
                return Err(Error::Other("GetDC(magnifier) failed".into()));
            }
            let blt = BitBlt(dc, 0, 0, w, h, Some(dib.dc), 0, 0, SRCCOPY);
            ReleaseDC(Some(self.hwnd), dc);
            blt.context("BitBlt(magnifier)")?;
        }
        Ok(())
    }

    pub fn hide(&mut self) {
        if std::mem::take(&mut self.visible) {
            // SAFETY: own window. The return value is the previous visibility.
            let _ = unsafe { ShowWindow(self.hwnd, SW_HIDE) };
        }
    }
}

impl Drop for Magnifier {
    fn drop(&mut self) {
        // SAFETY: window and font were created by this object on this thread.
        unsafe {
            let _ = DestroyWindow(self.hwnd);
            if let Some((_, f)) = self.font.take() {
                let _ = DeleteObject(f.into());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_scales_pixels_and_frames_center() {
        let src = [
            0x00FF_0000,
            0x0000_FF00,
            0x0000_00FF,
            0x00FF_FFFF,
            0,
            0x0012_3456,
            1,
            2,
            3,
        ];
        let mut dst = vec![0; 36];
        render_zoom(&src, 3, 2, false, &mut dst);
        assert_eq!(dst[0], 0x00FF_0000);
        assert_eq!(dst[1], 0x00FF_0000);
        assert_eq!(dst[2], 0x0000_FF00);
        assert_eq!(dst[6], 0x00FF_0000, "second row repeats the first source row");
        // Center pixel (black) is framed in white.
        assert_eq!(dst[2 * 6 + 2], 0x00FF_FFFF);
        assert_eq!(dst[5 * 6 + 5], 3);
    }

    #[test]
    fn grid_darkens_cell_edges() {
        let src = vec![0x00FF_FFFF; 9];
        let mut dst = vec![0; 144];
        render_zoom(&src, 3, 4, true, &mut dst);
        assert_eq!(dst[0], 0x007F_7F7F);
        assert_eq!(dst[1], 0x007F_7F7F, "first row of every cell is a grid line");
        assert_eq!(dst[12 + 1], 0x00FF_FFFF);
    }

    #[test]
    fn contrast_picks_readable_text() {
        assert_eq!(contrast(0x00FF_FFFF), 0);
        assert_eq!(contrast(0x00FF_FF00), 0);
        assert_eq!(contrast(0x0000_0080), 0x00FF_FFFF);
    }

    #[test]
    fn placement_flips_at_monitor_edges() {
        let m = Rect::new(0, 0, 1920, 1080);
        assert_eq!(place((100, 100), 20, (200, 220), m), (120, 120));
        assert_eq!(place((1900, 1060), 20, (200, 220), m), (1680, 820));
        assert_eq!(
            place((10, 1060), 20, (200, 220), Rect::new(0, 900, 1920, 1080)),
            (30, 900)
        );
    }
}
