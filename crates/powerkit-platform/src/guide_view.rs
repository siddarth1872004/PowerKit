//! A child window that plays a walkthrough: it redraws about 30 times a second
//! while it is visible, scaling the scene to fit and writing the caption under it.

use crate::util::{pcwstr, wide};
use crate::{Context, Result};
use powerkit_guide::{Align, Frame, Shape, HEIGHT, WIDTH};
use std::sync::OnceLock;
use std::time::Instant;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Graphics::GdiPlus::*;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::*;

const CLASS_NAME: &str = "PowerKit.Guide";
const TIMER: usize = 1;
const TICK_MS: u32 = 33;
/// Height of the caption under the scene, in 96-DPI pixels.
const CAPTION_H: i32 = 34;

/// What the view plays: the frame at a time, and how long before it starts over.
pub struct Scene {
    pub duration: f32,
    pub frame: Box<dyn Fn(f32) -> Frame>,
}

struct State {
    scene: Option<Scene>,
    start: Instant,
    colors: Colors,
}

/// Colors around the scene, as 0xAARRGGBB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Colors {
    pub back: u32,
    pub ink: u32,
    pub accent: u32,
}

/// The height of the view for a given width, so that the scene keeps its shape.
pub(crate) fn height_for(width: i32, dpi: u32) -> i32 {
    (width as f32 * HEIGHT / WIDTH) as i32 + (CAPTION_H * dpi as i32 + 48) / 96
}

pub(crate) fn gdiplus_ready() -> bool {
    static TOKEN: OnceLock<bool> = OnceLock::new();
    *TOKEN.get_or_init(|| {
        let mut token = 0usize;
        let input = GdiplusStartupInput {
            GdiplusVersion: 1,
            ..Default::default()
        };
        // SAFETY: valid in and out pointers. GDI+ stays up until the process ends.
        let status = unsafe { GdiplusStartup(&mut token, &input, std::ptr::null_mut()) };
        if status != Ok {
            tracing::warn!("GDI+ did not start ({}); walkthroughs will not be drawn", status.0);
        }
        status == Ok
    })
}

/// # Safety
/// `hwnd` must be a guide view that has not been destroyed.
unsafe fn state<'a>(hwnd: HWND) -> Option<&'a mut State> {
    // SAFETY: set in `create`, freed and cleared in WM_NCDESTROY.
    unsafe { (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State).as_mut() }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            // SAFETY: own window; the state outlives this call.
            unsafe { paint(hwnd, state(hwnd).as_deref()) };
            LRESULT(0)
        }
        WM_TIMER => {
            // SAFETY: own window. Hidden views, including those in a hidden settings
            // window, are not redrawn.
            unsafe {
                if IsWindowVisible(hwnd).as_bool() && state(hwnd).is_some_and(|s| s.scene.is_some()) {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
            }
            LRESULT(0)
        }
        WM_NCDESTROY => {
            // SAFETY: own window, being destroyed; nothing reads the state afterwards.
            unsafe {
                let _ = KillTimer(Some(hwnd), TIMER);
                let p = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) as *mut State;
                if !p.is_null() {
                    drop(Box::from_raw(p));
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
        }
        // SAFETY: forwarding the unmodified arguments of this callback.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Create a hidden view inside `parent`.
pub(crate) fn create(parent: HWND, instance: HINSTANCE, id: i32) -> Result<HWND> {
    let class = wide(CLASS_NAME);
    // SAFETY: strings outlive the calls; the state box is owned by the window from
    // here on and freed in WM_NCDESTROY.
    unsafe {
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            lpszClassName: pcwstr(&class),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassW(&wc); // "already registered" is expected after the first one
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            pcwstr(&class),
            None,
            WS_CHILD,
            0,
            0,
            0,
            0,
            Some(parent),
            Some(HMENU(id as isize as *mut _)),
            Some(instance),
            None,
        )
        .context("CreateWindowEx(guide)")?;
        let ink = GetSysColor(COLOR_BTNTEXT);
        let back = GetSysColor(COLOR_BTNFACE);
        let argb = |c: u32| 0xFF00_0000 | (c & 0xFF) << 16 | (c & 0xFF00) | (c >> 16) & 0xFF;
        let st = Box::new(State {
            scene: None,
            start: Instant::now(),
            colors: Colors {
                back: argb(back),
                ink: argb(ink),
                accent: 0xFF2E_74EC,
            },
        });
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(st) as isize);
        if SetTimer(Some(hwnd), TIMER, TICK_MS, None) == 0 {
            return Err(crate::Error::Other("SetTimer(guide) failed".into()));
        }
        std::result::Result::Ok(hwnd)
    }
}

/// Play `scene` from its start, or show nothing.
pub(crate) fn set_scene(hwnd: HWND, scene: Option<Scene>) {
    // SAFETY: `hwnd` is a guide view made by `create`, on this thread.
    unsafe {
        if let Some(st) = state(hwnd) {
            st.scene = scene;
            st.start = Instant::now();
        }
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

/// Draw with `colors` from now on.
pub(crate) fn set_colors(hwnd: HWND, colors: Colors) {
    // SAFETY: `hwnd` is a guide view made by `create`, on this thread.
    unsafe {
        if let Some(st) = state(hwnd) {
            if st.colors == colors {
                return;
            }
            st.colors = colors;
        }
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

/// # Safety
/// `hwnd` must be a guide view, called while handling its WM_PAINT.
unsafe fn paint(hwnd: HWND, st: Option<&State>) {
    let mut ps = PAINTSTRUCT::default();
    // SAFETY: standard paint sequence into an off-screen bitmap, so the animation
    // does not flicker; everything made here is released before returning.
    unsafe {
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        let (w, h) = (rc.right, rc.bottom);
        if w > 0 && h > 0 {
            let mem = CreateCompatibleDC(Some(hdc));
            let bmp = CreateCompatibleBitmap(hdc, w, h);
            let old = SelectObject(mem, bmp.into());
            let colors = st.map(|s| s.colors);
            let back = colors.map_or(GetSysColor(COLOR_BTNFACE), |c| {
                (c.back & 0xFF) << 16 | (c.back & 0xFF00) | (c.back >> 16) & 0xFF
            });
            let brush = CreateSolidBrush(COLORREF(back));
            FillRect(mem, &rc, brush);
            let _ = DeleteObject(brush.into());
            if let (Some(scene), Some(colors)) = (st.and_then(|s| s.scene.as_ref().map(|sc| (sc, s.start))), colors) {
                if gdiplus_ready() {
                    let (scene, start) = scene;
                    let t = start.elapsed().as_secs_f32() % scene.duration.max(0.1);
                    draw(
                        mem,
                        w as f32,
                        h as f32,
                        GetDpiForWindow(hwnd).max(96),
                        &(scene.frame)(t),
                        t / scene.duration,
                        colors,
                    );
                }
            }
            let _ = BitBlt(hdc, 0, 0, w, h, Some(mem), 0, 0, SRCCOPY);
            SelectObject(mem, old);
            let _ = DeleteObject(bmp.into());
            let _ = DeleteDC(mem);
        }
        let _ = EndPaint(hwnd, &ps);
    }
}

/// Fonts made while drawing one frame, by size and weight.
struct Fonts {
    family: *mut GpFontFamily,
    made: Vec<((u32, bool), *mut GpFont)>,
}

impl Fonts {
    fn get(&mut self, size: f32, bold: bool) -> *mut GpFont {
        let key = (size.to_bits(), bold);
        if let Some((_, f)) = self.made.iter().find(|(k, _)| *k == key) {
            return *f;
        }
        let mut font = std::ptr::null_mut();
        let style = if bold { FontStyleBold.0 } else { FontStyleRegular.0 };
        // SAFETY: `family` is a live font family; `font` is an out pointer.
        unsafe { GdipCreateFont(self.family, size, style, UnitPixel, &mut font) };
        self.made.push((key, font));
        font
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        // SAFETY: each object was made by GDI+ and is deleted once.
        unsafe {
            for (_, f) in &self.made {
                if !f.is_null() {
                    GdipDeleteFont(*f);
                }
            }
            if !self.family.is_null() {
                GdipDeleteFontFamily(self.family);
            }
        }
    }
}

/// Paint `frame` and its caption into `hdc`, `w` x `h` pixels; `progress` (0 to 1)
/// is how far through the loop it is.
///
/// # Safety
/// `hdc` must be a valid device context.
unsafe fn draw(hdc: HDC, w: f32, h: f32, dpi: u32, frame: &Frame, progress: f32, colors: Colors) {
    let px = |v: f32| v * dpi as f32 / 96.0;
    let caption_h = px(CAPTION_H as f32);
    let k = (w / WIDTH).min((h - caption_h) / HEIGHT).max(0.01);
    let (ox, oy) = ((w - WIDTH * k) / 2.0, 0.0);
    // SAFETY: GDI+ objects made here are deleted before returning; every pointer
    // passed is either one of them or a local that outlives the call.
    unsafe {
        let mut g = std::ptr::null_mut();
        if GdipCreateFromHDC(hdc, &mut g) != Ok {
            return;
        }
        GdipSetSmoothingMode(g, SmoothingModeAntiAlias);
        GdipSetPixelOffsetMode(g, PixelOffsetModeHalf);
        GdipSetTextRenderingHint(g, TextRenderingHintAntiAliasGridFit);
        let face = wide("Segoe UI");
        let mut fonts = Fonts {
            family: std::ptr::null_mut(),
            made: Vec::new(),
        };
        GdipCreateFontFamilyFromName(pcwstr(&face), std::ptr::null_mut(), &mut fonts.family);
        let mut near = std::ptr::null_mut();
        let mut center = std::ptr::null_mut();
        for (f, align) in [(&mut near, StringAlignmentNear), (&mut center, StringAlignmentCenter)] {
            GdipCreateStringFormat(0, 0, f);
            GdipSetStringFormatFlags(*f, StringFormatFlagsNoWrap.0 | StringFormatFlagsNoClip.0);
            GdipSetStringFormatAlign(*f, align);
            GdipSetStringFormatLineAlign(*f, StringAlignmentCenter);
        }

        // The caption and a thin bar showing how far the loop has got.
        let bar_y = HEIGHT * k + px(4.0);
        fill_rect(g, ox, bar_y, WIDTH * k, px(2.0), colors.ink & 0x30FF_FFFF);
        fill_rect(
            g,
            ox,
            bar_y,
            WIDTH * k * progress.clamp(0.0, 1.0),
            px(2.0),
            colors.accent,
        );
        let rect = RectF {
            X: 0.0,
            Y: bar_y + px(2.0),
            Width: w,
            Height: caption_h - px(6.0),
        };
        text(
            g,
            &mut fonts,
            center,
            &frame.caption,
            px(13.0),
            false,
            colors.ink,
            &rect,
        );

        GdipScaleWorldTransform(g, k, k, MatrixOrderAppend);
        GdipTranslateWorldTransform(g, ox, oy, MatrixOrderAppend);
        GdipSetClipRect(g, 0.0, 0.0, WIDTH, HEIGHT, CombineModeReplace);
        for shape in &frame.shapes {
            draw_shape(g, &mut fonts, near, center, shape);
        }

        GdipDeleteStringFormat(near);
        GdipDeleteStringFormat(center);
        drop(fonts);
        GdipDeleteGraphics(g);
    }
}

/// # Safety
/// `g` must be a live GDI+ graphics object.
unsafe fn fill_rect(g: *mut GpGraphics, x: f32, y: f32, w: f32, h: f32, argb: u32) {
    let mut brush = std::ptr::null_mut();
    // SAFETY: the brush is made and deleted here.
    unsafe {
        GdipCreateSolidFill(argb, &mut brush);
        GdipFillRectangle(g, brush.cast(), x, y, w, h);
        GdipDeleteBrush(brush.cast());
    }
}

/// # Safety
/// `g`, `format` and the fonts must be live GDI+ objects.
#[allow(clippy::too_many_arguments)]
unsafe fn text(
    g: *mut GpGraphics,
    fonts: &mut Fonts,
    format: *mut GpStringFormat,
    s: &str,
    size: f32,
    bold: bool,
    argb: u32,
    rect: &RectF,
) {
    let s = wide(s);
    let mut brush = std::ptr::null_mut();
    // SAFETY: `s` and `rect` outlive the call; the brush is made and deleted here.
    unsafe {
        GdipCreateSolidFill(argb, &mut brush);
        GdipDrawString(
            g,
            pcwstr(&s),
            (s.len() - 1) as i32,
            fonts.get(size, bold),
            rect,
            format,
            brush.cast(),
        );
        GdipDeleteBrush(brush.cast());
    }
}

/// # Safety
/// `g`, the formats and the fonts must be live GDI+ objects.
unsafe fn draw_shape(
    g: *mut GpGraphics,
    fonts: &mut Fonts,
    near: *mut GpStringFormat,
    center: *mut GpStringFormat,
    shape: &Shape,
) {
    // SAFETY: brushes, pens and paths are made and deleted within each arm.
    unsafe {
        let brush = |c: powerkit_guide::Color| {
            let mut b = std::ptr::null_mut();
            GdipCreateSolidFill(c.0, &mut b);
            b
        };
        let pen = |(c, width): (powerkit_guide::Color, f32)| {
            let mut p = std::ptr::null_mut();
            GdipCreatePen1(c.0, width, UnitWorld, &mut p);
            GdipSetPenLineCap197819(p, LineCapRound, LineCapRound, DashCapFlat);
            p
        };
        match shape {
            Shape::Rect {
                x,
                y,
                w,
                h,
                radius,
                fill,
                stroke,
            } => {
                let mut path = std::ptr::null_mut();
                GdipCreatePath(FillModeAlternate, &mut path);
                let r = radius.min(w / 2.0).min(h / 2.0);
                if r <= 0.0 {
                    GdipAddPathRectangle(path, *x, *y, *w, *h);
                } else {
                    let d = r * 2.0;
                    GdipAddPathArc(path, *x, *y, d, d, 180.0, 90.0);
                    GdipAddPathArc(path, x + w - d, *y, d, d, 270.0, 90.0);
                    GdipAddPathArc(path, x + w - d, y + h - d, d, d, 0.0, 90.0);
                    GdipAddPathArc(path, *x, y + h - d, d, d, 90.0, 90.0);
                    GdipClosePathFigure(path);
                }
                if let Some(c) = fill {
                    let b = brush(*c);
                    GdipFillPath(g, b.cast(), path);
                    GdipDeleteBrush(b.cast());
                }
                if let Some(s) = stroke {
                    let p = pen(*s);
                    GdipDrawPath(g, p, path);
                    GdipDeletePen(p);
                }
                GdipDeletePath(path);
            }
            Shape::Ellipse {
                x,
                y,
                w,
                h,
                fill,
                stroke,
            } => {
                if let Some(c) = fill {
                    let b = brush(*c);
                    GdipFillEllipse(g, b.cast(), *x, *y, *w, *h);
                    GdipDeleteBrush(b.cast());
                }
                if let Some(s) = stroke {
                    let p = pen(*s);
                    GdipDrawEllipse(g, p, *x, *y, *w, *h);
                    GdipDeletePen(p);
                }
            }
            Shape::Line { from, to, color, width } => {
                let p = pen((*color, *width));
                GdipDrawLine(g, p, from.0, from.1, to.0, to.1);
                GdipDeletePen(p);
            }
            Shape::Polygon { points, fill, stroke } => {
                let pts: Vec<PointF> = points.iter().map(|&(x, y)| PointF { X: x, Y: y }).collect();
                let b = brush(*fill);
                GdipFillPolygon(g, b.cast(), pts.as_ptr(), pts.len() as i32, FillModeAlternate);
                GdipDeleteBrush(b.cast());
                if let Some(s) = stroke {
                    let p = pen(*s);
                    GdipDrawPolygon(g, p, pts.as_ptr(), pts.len() as i32);
                    GdipDeletePen(p);
                }
            }
            Shape::Text {
                x,
                y,
                w,
                h,
                text: s,
                size,
                color,
                align,
                bold,
            } => {
                let format = match align {
                    Align::Left => near,
                    Align::Center => center,
                };
                let rect = RectF {
                    X: *x,
                    Y: *y,
                    Width: *w,
                    Height: *h,
                };
                text(g, fonts, format, s, *size, *bold, color.0, &rect);
            }
        }
    }
}
