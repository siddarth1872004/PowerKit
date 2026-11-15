//! Drawing for the settings window with GDI+: rounded shapes, text in the Fluent
//! type ramp and Segoe Fluent Icons glyphs.

use crate::util::{pcwstr, wide};
use windows::Win32::Graphics::Gdi::{IntersectClipRect, RestoreDC, SaveDC, HDC};
use windows::Win32::Graphics::GdiPlus::*;
use windows::Win32::UI::WindowsAndMessaging::{DrawIconEx, DI_NORMAL, HICON};

/// A rectangle in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }
    pub fn inset(&self, dx: f32, dy: f32) -> Rect {
        Rect::new(
            self.x + dx,
            self.y + dy,
            (self.w - 2.0 * dx).max(0.0),
            (self.h - 2.0 * dy).max(0.0),
        )
    }
    pub fn offset(&self, dx: f32, dy: f32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.w, self.h)
    }
}

/// Faces of the type ramp. Each falls back to an older Windows font.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Face {
    Text,
    Strong,
    Title,
    Icons,
}

const FACES: [(Face, &[&str]); 4] = [
    (Face::Text, &["Segoe UI Variable Text", "Segoe UI"]),
    (
        Face::Strong,
        &["Segoe UI Variable Text Semibold", "Segoe UI Semibold", "Segoe UI"],
    ),
    (
        Face::Title,
        &["Segoe UI Variable Display Semibold", "Segoe UI Semibold", "Segoe UI"],
    ),
    (Face::Icons, &["Segoe Fluent Icons", "Segoe MDL2 Assets"]),
];

/// GDI+ font objects, made once per size and kept for the window's life.
pub(crate) struct Fonts {
    families: Vec<(Face, *mut GpFontFamily)>,
    made: Vec<((Face, u32), *mut GpFont)>,
}

impl Fonts {
    pub fn new() -> Fonts {
        let mut families = Vec::new();
        for (face, names) in FACES {
            for name in names {
                let w = wide(name);
                let mut fam = std::ptr::null_mut();
                // SAFETY: `w` outlives the call; `fam` is an out pointer.
                if unsafe { GdipCreateFontFamilyFromName(pcwstr(&w), std::ptr::null_mut(), &mut fam) } == Ok
                    && !fam.is_null()
                {
                    families.push((face, fam));
                    break;
                }
            }
        }
        Fonts {
            families,
            made: Vec::new(),
        }
    }

    fn get(&mut self, face: Face, px: f32) -> *mut GpFont {
        let key = (face, px.to_bits());
        if let Some((_, f)) = self.made.iter().find(|(k, _)| *k == key) {
            return *f;
        }
        let Some(&(_, fam)) = self.families.iter().find(|(f, _)| *f == face) else {
            return std::ptr::null_mut();
        };
        let mut font = std::ptr::null_mut();
        // SAFETY: `fam` is a live family; `font` is an out pointer.
        unsafe { GdipCreateFont(fam, px, FontStyleRegular.0, UnitPixel, &mut font) };
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
            for (_, fam) in &self.families {
                GdipDeleteFontFamily(*fam);
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Align {
    Left,
    Center,
    Right,
}

/// How a piece of text is set.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Style {
    pub face: Face,
    pub px: f32,
    pub color: u32,
    pub align: Align,
    /// Wrap onto further lines instead of ending with an ellipsis.
    pub wrap: bool,
}

pub(crate) struct Painter<'a> {
    g: *mut GpGraphics,
    fonts: &'a mut Fonts,
    clip: Option<Rect>,
}

/// Something that can measure text, so layout can be tested without GDI+.
pub(crate) trait Measure {
    /// Width and height of `s` set in `style`, wrapped at `width` when it wraps.
    fn measure(&mut self, s: &str, style: &Style, width: f32) -> (f32, f32);
}

impl<'a> Painter<'a> {
    /// # Safety
    /// `hdc` must stay valid until the painter is dropped.
    pub unsafe fn new(hdc: HDC, fonts: &'a mut Fonts) -> Option<Painter<'a>> {
        let mut g = std::ptr::null_mut();
        // SAFETY: the caller guarantees `hdc`; `g` is deleted in Drop.
        unsafe {
            if GdipCreateFromHDC(hdc, &mut g) != Ok {
                return None;
            }
            GdipSetSmoothingMode(g, SmoothingModeAntiAlias);
            GdipSetPixelOffsetMode(g, PixelOffsetModeHalf);
            GdipSetTextRenderingHint(g, TextRenderingHintClearTypeGridFit);
        }
        Some(Painter { g, fonts, clip: None })
    }

    pub fn set_clip(&mut self, clip: Option<Rect>) {
        self.clip = clip;
        // SAFETY: live graphics object.
        unsafe {
            match clip {
                Some(r) => GdipSetClipRect(self.g, r.x, r.y, r.w, r.h, CombineModeReplace),
                None => GdipResetClip(self.g),
            };
        }
    }

    fn round_path(r: Rect, radius: f32) -> *mut GpPath {
        let mut path = std::ptr::null_mut();
        // SAFETY: the path is made here and deleted by the caller.
        unsafe {
            GdipCreatePath(FillModeAlternate, &mut path);
            let rad = radius.min(r.w / 2.0).min(r.h / 2.0);
            if rad <= 0.0 {
                GdipAddPathRectangle(path, r.x, r.y, r.w, r.h);
            } else {
                let d = rad * 2.0;
                GdipAddPathArc(path, r.x, r.y, d, d, 180.0, 90.0);
                GdipAddPathArc(path, r.right() - d, r.y, d, d, 270.0, 90.0);
                GdipAddPathArc(path, r.right() - d, r.bottom() - d, d, d, 0.0, 90.0);
                GdipAddPathArc(path, r.x, r.bottom() - d, d, d, 90.0, 90.0);
                GdipClosePathFigure(path);
            }
        }
        path
    }

    pub fn fill(&mut self, r: Rect, radius: f32, argb: u32) {
        if argb >> 24 == 0 || r.w <= 0.0 || r.h <= 0.0 {
            return;
        }
        // SAFETY: GDI+ objects made and deleted here.
        unsafe {
            let mut brush = std::ptr::null_mut();
            GdipCreateSolidFill(argb, &mut brush);
            if radius <= 0.0 {
                GdipFillRectangle(self.g, brush.cast(), r.x, r.y, r.w, r.h);
            } else {
                let path = Self::round_path(r, radius);
                GdipFillPath(self.g, brush.cast(), path);
                GdipDeletePath(path);
            }
            GdipDeleteBrush(brush.cast());
        }
    }

    /// A border drawn just inside `r`.
    pub fn stroke(&mut self, r: Rect, radius: f32, argb: u32, width: f32) {
        if argb >> 24 == 0 {
            return;
        }
        let half = width / 2.0;
        let r = r.inset(half, half);
        // SAFETY: GDI+ objects made and deleted here.
        unsafe {
            let mut pen = std::ptr::null_mut();
            GdipCreatePen1(argb, width, UnitPixel, &mut pen);
            let path = Self::round_path(r, (radius - half).max(0.0));
            GdipDrawPath(self.g, pen, path);
            GdipDeletePath(path);
            GdipDeletePen(pen);
        }
    }

    pub fn ellipse(&mut self, r: Rect, argb: u32) {
        // SAFETY: GDI+ objects made and deleted here.
        unsafe {
            let mut brush = std::ptr::null_mut();
            GdipCreateSolidFill(argb, &mut brush);
            GdipFillEllipse(self.g, brush.cast(), r.x, r.y, r.w, r.h);
            GdipDeleteBrush(brush.cast());
        }
    }

    /// A control's border: even all round, a little darker along the bottom.
    pub fn control_border(&mut self, r: Rect, radius: f32, stroke: u32, bottom: u32, px: f32) {
        self.stroke(r, radius, stroke, px);
        self.fill(
            Rect::new(r.x + radius, r.bottom() - px, r.w - 2.0 * radius, px),
            0.0,
            bottom,
        );
    }

    fn format(style: &Style) -> *mut GpStringFormat {
        let mut f = std::ptr::null_mut();
        // SAFETY: the format is made here and deleted by the caller.
        unsafe {
            GdipCreateStringFormat(0, 0, &mut f);
            let mut flags = StringFormatFlagsNoClip.0;
            if !style.wrap {
                flags |= StringFormatFlagsNoWrap.0;
                GdipSetStringFormatTrimming(f, StringTrimmingEllipsisCharacter);
            }
            GdipSetStringFormatFlags(f, flags);
            GdipSetStringFormatAlign(
                f,
                match style.align {
                    Align::Left => StringAlignmentNear,
                    Align::Center => StringAlignmentCenter,
                    Align::Right => StringAlignmentFar,
                },
            );
            GdipSetStringFormatLineAlign(f, StringAlignmentCenter);
        }
        f
    }

    /// `s` in `r`, centered vertically.
    pub fn text(&mut self, s: &str, r: Rect, style: &Style) {
        if s.is_empty() {
            return;
        }
        let w = wide(s);
        let font = self.fonts.get(style.face, style.px);
        let rect = RectF {
            X: r.x,
            Y: r.y,
            Width: r.w,
            Height: r.h,
        };
        // SAFETY: GDI+ objects made and deleted here; `w` and `rect` outlive the call.
        unsafe {
            let f = Self::format(style);
            let mut brush = std::ptr::null_mut();
            GdipCreateSolidFill(style.color, &mut brush);
            GdipDrawString(self.g, pcwstr(&w), (w.len() - 1) as i32, font, &rect, f, brush.cast());
            GdipDeleteBrush(brush.cast());
            GdipDeleteStringFormat(f);
        }
    }

    /// A Segoe Fluent Icons glyph centered on `r`.
    pub fn glyph(&mut self, ch: char, r: Rect, px: f32, argb: u32) {
        let style = Style {
            face: Face::Icons,
            px,
            color: argb,
            align: Align::Center,
            wrap: false,
        };
        self.text(&ch.to_string(), r, &style);
    }

    /// A Windows icon at its own size in `r`.
    pub fn icon(&mut self, icon: HICON, r: Rect) {
        // SAFETY: the GDI+ device context is released before GDI+ is used again;
        // `icon` is a live icon the caller keeps.
        unsafe {
            let mut hdc = HDC::default();
            if GdipGetDC(self.g, &mut hdc) != Ok {
                return;
            }
            let saved = SaveDC(hdc);
            if let Some(c) = self.clip {
                IntersectClipRect(hdc, c.x as i32, c.y as i32, c.right() as i32, c.bottom() as i32);
            }
            let _ = DrawIconEx(
                hdc,
                r.x.round() as i32,
                r.y.round() as i32,
                icon,
                r.w.round() as i32,
                r.h.round() as i32,
                0,
                None,
                DI_NORMAL,
            );
            let _ = RestoreDC(hdc, saved);
            GdipReleaseDC(self.g, hdc);
        }
    }
}

impl Measure for Painter<'_> {
    fn measure(&mut self, s: &str, style: &Style, width: f32) -> (f32, f32) {
        if s.is_empty() {
            return (0.0, 0.0);
        }
        let w = wide(s);
        let font = self.fonts.get(style.face, style.px);
        let layout = RectF {
            X: 0.0,
            Y: 0.0,
            Width: if style.wrap { width } else { 100_000.0 },
            Height: 100_000.0,
        };
        let mut out = RectF::default();
        // SAFETY: GDI+ objects made and deleted here; pointers outlive the call.
        unsafe {
            let f = Self::format(&Style {
                align: Align::Left,
                ..*style
            });
            GdipSetStringFormatLineAlign(f, StringAlignmentNear);
            GdipMeasureString(
                self.g,
                pcwstr(&w),
                (w.len() - 1) as i32,
                font,
                &layout,
                f,
                &mut out,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            GdipDeleteStringFormat(f);
        }
        (out.Width, out.Height)
    }
}

impl Drop for Painter<'_> {
    fn drop(&mut self) {
        // SAFETY: made in `new`, deleted once.
        unsafe { GdipDeleteGraphics(self.g) };
    }
}
