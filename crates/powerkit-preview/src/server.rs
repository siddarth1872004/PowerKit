//! The COM server: preview handlers, which prevhost.exe runs for the preview pane, and
//! thumbnail providers, which Explorer or a dllhost.exe runs. Nothing here may unwind
//! into the host: drawing a file runs under `guarded`, which turns a panic into an
//! error, and every failure becomes an HRESULT or a message in the pane.

use crate::image::Image;
use crate::rtf::Theme;
use crate::{code, decode_text, gcode, markdown, qoi, render, stl, Format, Handler, Kind, HANDLERS};
use std::cell::RefCell;
use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::Mutex;
use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, COLORREF, ERROR_CLASS_ALREADY_EXISTS, E_FAIL, E_NOTIMPL,
    E_POINTER, E_UNEXPECTED, HINSTANCE, HWND, LPARAM, LRESULT, RECT, S_FALSE, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateDIBSection, CreateSolidBrush, DeleteDC,
    DeleteObject, DrawTextW, EndPaint, FillRect, GetStockObject, InvalidateRect, SelectObject, SetBkMode,
    SetBrushOrgEx, SetStretchBltMode, SetTextColor, StretchDIBits, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DEFAULT_GUI_FONT, DIB_RGB_COLORS, DT_CENTER, DT_WORDBREAK, HALFTONE, HBITMAP, HDC, PAINTSTRUCT, SRCCOPY,
    TRANSPARENT,
};
use windows::Win32::System::Com::{
    CoTaskMemFree, IClassFactory, IClassFactory_Impl, IStream, STATFLAG_DEFAULT, STATSTG,
};
use windows::Win32::System::LibraryLoader::LoadLibraryW;
use windows::Win32::System::Ole::{IObjectWithSite, IObjectWithSite_Impl, IOleWindow, IOleWindow_Impl};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::System::SystemServices::DLL_PROCESS_ATTACH;
use windows::Win32::UI::Controls::RichEdit::{
    AURL_ENABLEURL, CHARRANGE, EM_AUTOURLDETECT, EM_GETTEXTRANGE, EM_SETBKGNDCOLOR, EM_SETEVENTMASK, EM_SETTEXTEX,
    ENLINK, ENM_LINK, EN_LINK, MSFTEDIT_CLASS, SETTEXTEX, ST_DEFAULT, TEXTRANGEW,
};
use windows::Win32::UI::Controls::{SetScrollInfo, EM_SETMARGINS, NMHDR};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, SetFocus, VK_DOWN, VK_END, VK_HOME, VK_NEXT, VK_PRIOR, VK_UP,
};
use windows::Win32::UI::Shell::PropertiesSystem::{IInitializeWithStream, IInitializeWithStream_Impl};
use windows::Win32::UI::Shell::{
    IPreviewHandler, IPreviewHandlerFrame, IPreviewHandlerVisuals, IPreviewHandlerVisuals_Impl, IPreviewHandler_Impl,
    IThumbnailProvider, IThumbnailProvider_Impl, ShellExecuteW, WTSAT_ARGB, WTS_ALPHATYPE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetScrollInfo, GetWindowLongPtrW, IsWindow,
    LoadCursorW, MoveWindow, PostMessageW, RegisterClassW, SendMessageW, SetParent, SetWindowLongPtrW, EC_LEFTMARGIN,
    EC_RIGHTMARGIN, ES_AUTOVSCROLL, ES_MULTILINE, ES_READONLY, GWLP_USERDATA, IDC_ARROW, MSG, SB_BOTTOM, SB_LINEDOWN,
    SB_LINEUP, SB_PAGEDOWN, SB_PAGEUP, SB_THUMBPOSITION, SB_THUMBTRACK, SB_TOP, SB_VERT, SCROLLINFO, SIF_PAGE, SIF_POS,
    SIF_RANGE, SIF_TRACKPOS, SW_SHOWNORMAL, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_ERASEBKGND, WM_KEYDOWN,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEWHEEL, WM_NCDESTROY, WM_NOTIFY, WM_PAINT, WM_SIZE, WM_VSCROLL, WNDCLASSW,
    WS_CHILD, WS_CLIPCHILDREN, WS_VISIBLE, WS_VSCROLL,
};
use windows_core::{
    implement, w, Error, IUnknown, Interface, Ref, Result, BOOL, GUID, HRESULT, HSTRING, PCWSTR, PWSTR,
};

static MODULE: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

fn module() -> HINSTANCE {
    HINSTANCE(MODULE.load(Ordering::Relaxed))
}

#[no_mangle]
extern "system" fn DllMain(module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> BOOL {
    if reason == DLL_PROCESS_ATTACH {
        MODULE.store(module.0, Ordering::Relaxed);
    }
    true.into()
}

/// # Safety
/// Called by COM with valid pointers, as for any in-process server.
#[no_mangle]
unsafe extern "system" fn DllGetClassObject(clsid: *const GUID, iid: *const GUID, out: *mut *mut c_void) -> HRESULT {
    if clsid.is_null() || iid.is_null() || out.is_null() {
        return E_POINTER;
    }
    // SAFETY: checked non-null above; COM passes valid pointers.
    unsafe { *out = std::ptr::null_mut() };
    let wanted = unsafe { (*clsid).to_u128() };
    let Some(handler) = HANDLERS.iter().find(|h| h.clsid == wanted) else {
        return CLASS_E_CLASSNOTAVAILABLE;
    };
    let factory: IClassFactory = Factory { handler }.into();
    // SAFETY: iid and out are valid, as checked above.
    unsafe { factory.query(iid, out) }
}

/// The DLL stays loaded for the life of the process: unloading while a window of ours
/// still exists would crash the host.
#[no_mangle]
extern "system" fn DllCanUnloadNow() -> HRESULT {
    S_FALSE
}

#[implement(IClassFactory)]
struct Factory {
    handler: &'static Handler,
}

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(&self, outer: Ref<IUnknown>, iid: *const GUID, out: *mut *mut c_void) -> Result<()> {
        if out.is_null() || iid.is_null() {
            return Err(E_POINTER.into());
        }
        // SAFETY: checked non-null above.
        unsafe { *out = std::ptr::null_mut() };
        if outer.is_some() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        let object: IUnknown = match self.handler.kind {
            Kind::Preview => Preview {
                format: self.handler.format,
                state: Mutex::new(State::default()),
            }
            .into(),
            Kind::Thumbnail => Thumbnail {
                format: self.handler.format,
                bytes: Mutex::new(None),
            }
            .into(),
        };
        // SAFETY: iid and out are valid, as checked above.
        unsafe { object.query(iid, out) }.ok()
    }

    fn LockServer(&self, _lock: BOOL) -> Result<()> {
        Ok(())
    }
}

fn failed(message: &str) -> Error {
    Error::new(E_FAIL, message)
}

/// Runs the drawing of a file, turning a panic in it into an error.
fn guarded<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| Err(failed("PowerKit could not read this file.")))
}

/// The file's bytes, at most `limit` of them, and whether there was more.
fn read_stream(stream: &IStream, limit: usize) -> Result<(Vec<u8>, bool)> {
    let mut bytes = Vec::new();
    let mut chunk = vec![0u8; 1 << 16];
    loop {
        let mut got = 0u32;
        // SAFETY: the buffer is as long as the length given.
        unsafe { stream.Read(chunk.as_mut_ptr().cast(), chunk.len() as u32, Some(&mut got)) }.ok()?;
        if got == 0 {
            return Ok((bytes, false));
        }
        bytes.extend_from_slice(&chunk[..(got as usize).min(chunk.len())]);
        if bytes.len() > limit {
            bytes.truncate(limit);
            return Ok((bytes, true));
        }
    }
}

/// The file's extension, lowercase and without the dot, which picks the language the
/// code preview colors; empty when the stream does not say its name.
fn stream_extension(stream: &IStream) -> String {
    let mut stat = STATSTG::default();
    // SAFETY: Stat fills `stat`; the name it allocates is freed below.
    if unsafe { stream.Stat(&mut stat, STATFLAG_DEFAULT) }.is_err() || stat.pwcsName.is_null() {
        return String::new();
    }
    // SAFETY: a non-null name from Stat is a nul-terminated string.
    let name = unsafe { stat.pwcsName.to_string() }.unwrap_or_default();
    // SAFETY: Stat allocated the name with the COM allocator.
    unsafe { CoTaskMemFree(Some(stat.pwcsName.0 as *const c_void)) };
    match name.rsplit_once('.') {
        Some((_, ext)) => ext.to_ascii_lowercase(),
        None => String::new(),
    }
}

fn gcode_picture(bytes: &[u8], max: u32) -> Result<Image> {
    let picture = gcode::largest(bytes).ok_or_else(|| failed("This G-code file has no picture of the print."))?;
    match picture.encoding {
        gcode::Encoding::Qoi => {
            qoi::decode(&picture.data).ok_or_else(|| failed("The picture in this file is damaged."))
        }
        gcode::Encoding::Png | gcode::Encoding::Jpeg => render::decode(&picture.data, max),
    }
}

#[implement(IThumbnailProvider, IInitializeWithStream)]
struct Thumbnail {
    format: Format,
    bytes: Mutex<Option<Vec<u8>>>,
}

impl IInitializeWithStream_Impl for Thumbnail_Impl {
    fn Initialize(&self, stream: Ref<IStream>, _mode: u32) -> Result<()> {
        let (bytes, _) = read_stream(stream.ok()?, self.format.read_limit())?;
        *self.bytes.lock().map_err(|_| E_FAIL)? = Some(bytes);
        Ok(())
    }
}

fn thumbnail(format: Format, bytes: &[u8], size: u32) -> Result<Image> {
    let image = match format {
        Format::Svg => render::svg(&decode_text(bytes), size)?,
        Format::Pdf => {
            let pdf = render::Pdf::open(bytes)?;
            let &(w, h) = pdf.pages.first().ok_or_else(|| failed("This PDF has no pages."))?;
            let width = if w >= h {
                size
            } else {
                (size as f32 * w / h.max(1.0)).round() as u32
            };
            pdf.page(0, width)?
        }
        Format::Qoi => qoi::decode(bytes).ok_or_else(|| failed("This is not a QOI picture."))?,
        Format::Gcode => gcode_picture(bytes, size)?,
        Format::Stl => stl::render(&stl::parse(bytes), size).ok_or_else(|| failed("This STL file has no shape."))?,
        Format::Markdown | Format::Code => return Err(E_NOTIMPL.into()),
    };
    Ok(image.fit(size))
}

/// A 32-bit top-down DIB section holding the image.
fn dib(image: &Image) -> Result<HBITMAP> {
    let info = bitmap_info(image);
    let mut bits = std::ptr::null_mut();
    // SAFETY: `info` describes a bitmap whose bits are exactly `image.pixels` long.
    unsafe {
        let bitmap = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
        if bits.is_null() {
            let _ = DeleteObject(bitmap.into());
            return Err(failed("Windows gave no memory for the thumbnail."));
        }
        std::ptr::copy_nonoverlapping(image.pixels.as_ptr(), bits.cast::<u8>(), image.pixels.len());
        Ok(bitmap)
    }
}

fn bitmap_info(image: &Image) -> BITMAPINFO {
    BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: image.width as i32,
            biHeight: -(image.height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    }
}

impl IThumbnailProvider_Impl for Thumbnail_Impl {
    fn GetThumbnail(&self, cx: u32, bitmap: *mut HBITMAP, alpha: *mut WTS_ALPHATYPE) -> Result<()> {
        if bitmap.is_null() || alpha.is_null() {
            return Err(E_POINTER.into());
        }
        let guard = self.bytes.lock().map_err(|_| E_FAIL)?;
        let bytes = guard.as_deref().ok_or(E_UNEXPECTED)?;
        let image = guarded(|| thumbnail(self.format, bytes, cx.clamp(1, 2560)))?;
        let out = dib(&image)?;
        // SAFETY: checked non-null above.
        unsafe {
            *bitmap = out;
            *alpha = WTSAT_ARGB;
        }
        Ok(())
    }
}

fn hwnd(raw: isize) -> HWND {
    HWND(raw as *mut c_void)
}

fn colorref([r, g, b]: [u8; 3]) -> COLORREF {
    COLORREF(u32::from(r) | u32::from(g) << 8 | u32::from(b) << 16)
}

fn rgb(color: COLORREF) -> [u8; 3] {
    [color.0 as u8, (color.0 >> 8) as u8, (color.0 >> 16) as u8]
}

fn send(window: HWND, message: u32, wparam: usize, lparam: isize) -> isize {
    // SAFETY: messages to our own windows, with arguments the message defines.
    unsafe { SendMessageW(window, message, Some(WPARAM(wparam)), Some(LPARAM(lparam))) }.0
}

/// Windows' app theme, for when Explorer does not say the pane's colors.
fn apps_use_light_theme() -> bool {
    let mut value = 1u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: the value is a DWORD written into `value`, whose size is given.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut value as *mut u32).cast()),
            Some(&mut size),
        )
    };
    // Without the value Windows itself uses the light theme.
    status.is_err() || value != 0
}

#[derive(Default)]
struct State {
    bytes: Option<(Vec<u8>, bool)>,
    extension: String,
    parent: isize,
    rect: RECT,
    window: isize,
    focus: isize,
    background: Option<[u8; 3]>,
    text: Option<[u8; 3]>,
    site: Option<IUnknown>,
}

impl State {
    fn theme(&self) -> Theme {
        let base = match self.background {
            Some(b) if Theme { background: b, text: b }.dark() => Theme::DARK,
            Some(_) => Theme::LIGHT,
            None if apps_use_light_theme() => Theme::LIGHT,
            None => Theme::DARK,
        };
        Theme {
            background: self.background.unwrap_or(base.background),
            text: self.text.unwrap_or(base.text),
        }
    }

    fn destroy_window(&mut self) {
        if self.window != 0 {
            // SAFETY: our own window; a window already gone makes this fail harmlessly.
            unsafe {
                if IsWindow(Some(hwnd(self.window))).as_bool() {
                    let _ = DestroyWindow(hwnd(self.window));
                }
            }
        }
        self.window = 0;
        self.focus = 0;
    }
}

#[implement(
    IPreviewHandler,
    IPreviewHandlerVisuals,
    IOleWindow,
    IObjectWithSite,
    IInitializeWithStream
)]
struct Preview {
    format: Format,
    state: Mutex<State>,
}

impl Drop for Preview {
    fn drop(&mut self) {
        if let Ok(state) = self.state.get_mut() {
            state.destroy_window();
        }
    }
}

impl Preview_Impl {
    fn state(&self) -> Result<std::sync::MutexGuard<'_, State>> {
        self.state.lock().map_err(|_| E_FAIL.into())
    }
}

fn move_to(window: isize, rect: &RECT) {
    // SAFETY: our own window.
    let _ = unsafe {
        MoveWindow(
            hwnd(window),
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top,
            true,
        )
    };
}

impl IInitializeWithStream_Impl for Preview_Impl {
    fn Initialize(&self, stream: Ref<IStream>, _mode: u32) -> Result<()> {
        let stream = stream.ok()?;
        let read = read_stream(stream, self.format.read_limit())?;
        let mut state = self.state()?;
        state.extension = stream_extension(stream);
        state.bytes = Some(read);
        Ok(())
    }
}

impl IPreviewHandler_Impl for Preview_Impl {
    fn SetWindow(&self, parent: HWND, rect: *const RECT) -> Result<()> {
        if rect.is_null() {
            return Err(E_POINTER.into());
        }
        let mut state = self.state()?;
        state.parent = parent.0 as isize;
        // SAFETY: checked non-null above.
        state.rect = unsafe { *rect };
        if state.window != 0 {
            // SAFETY: our own window moved under the host's new one.
            unsafe { SetParent(hwnd(state.window), Some(parent)) }?;
            move_to(state.window, &state.rect);
        }
        Ok(())
    }

    fn SetRect(&self, rect: *const RECT) -> Result<()> {
        if rect.is_null() {
            return Err(E_POINTER.into());
        }
        let mut state = self.state()?;
        // SAFETY: checked non-null above.
        state.rect = unsafe { *rect };
        if state.window != 0 {
            move_to(state.window, &state.rect);
        }
        Ok(())
    }

    fn DoPreview(&self) -> Result<()> {
        let mut state = self.state()?;
        state.destroy_window();
        let theme = state.theme();
        let (bytes, cut) = state.bytes.as_ref().ok_or(E_UNEXPECTED)?;
        let rect = state.rect;
        register_class()?;
        // SAFETY: a child of the host's window, of the class registered above; its
        // state is attached only once it is built.
        let window = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                CLASS,
                None,
                WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN,
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                Some(hwnd(state.parent)),
                None,
                Some(module()),
                None,
            )
        }?;
        let view = guarded(|| build(self.format, bytes, *cut, &state.extension, theme, window))
            .unwrap_or_else(|e| View::Message(format!("PowerKit could not show this file.\n\n{}", e.message())));
        let focus = match &view {
            View::Text { edit, .. } => *edit,
            _ => window.0 as isize,
        };
        let pane = Box::new(RefCell::new(Pane { theme, view }));
        // SAFETY: the box is taken back and dropped in WM_NCDESTROY.
        unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, Box::into_raw(pane) as isize) };
        state.window = window.0 as isize;
        state.focus = focus;
        layout(window);
        Ok(())
    }

    fn Unload(&self) -> Result<()> {
        let mut state = self.state()?;
        state.destroy_window();
        state.bytes = None;
        Ok(())
    }

    fn SetFocus(&self) -> Result<()> {
        let state = self.state()?;
        if state.focus != 0 {
            // SAFETY: our own window.
            unsafe { SetFocus(Some(hwnd(state.focus))) }?;
        }
        Ok(())
    }

    fn QueryFocus(&self) -> Result<HWND> {
        // SAFETY: no arguments.
        let focus = unsafe { GetFocus() };
        if focus.is_invalid() {
            return Err(Error::from_thread());
        }
        Ok(focus)
    }

    fn TranslateAccelerator(&self, message: *const MSG) -> Result<()> {
        let site = self.state()?.site.clone();
        let Some(frame) = site.and_then(|s| s.cast::<IPreviewHandlerFrame>().ok()) else {
            return Err(Error::from_hresult(S_FALSE));
        };
        // The frame's S_FALSE (not handled) has to reach the host as it is, which the
        // generated wrapper would turn into Ok.
        // SAFETY: a live interface and the host's message.
        let hr = unsafe { (Interface::vtable(&frame).TranslateAccelerator)(Interface::as_raw(&frame), message) };
        if hr.0 == 0 {
            Ok(())
        } else {
            Err(Error::from_hresult(hr))
        }
    }
}

impl IPreviewHandlerVisuals_Impl for Preview_Impl {
    fn SetBackgroundColor(&self, color: COLORREF) -> Result<()> {
        self.state()?.background = Some(rgb(color));
        Ok(())
    }

    fn SetFont(&self, _font: *const windows::Win32::Graphics::Gdi::LOGFONTW) -> Result<()> {
        Ok(())
    }

    fn SetTextColor(&self, color: COLORREF) -> Result<()> {
        self.state()?.text = Some(rgb(color));
        Ok(())
    }
}

impl IOleWindow_Impl for Preview_Impl {
    fn GetWindow(&self) -> Result<HWND> {
        Ok(hwnd(self.state()?.parent))
    }

    fn ContextSensitiveHelp(&self, _enter: BOOL) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
}

impl IObjectWithSite_Impl for Preview_Impl {
    fn SetSite(&self, site: Ref<IUnknown>) -> Result<()> {
        self.state()?.site = site.cloned();
        Ok(())
    }

    fn GetSite(&self, iid: *const GUID, out: *mut *mut c_void) -> Result<()> {
        if out.is_null() || iid.is_null() {
            return Err(E_POINTER.into());
        }
        // SAFETY: checked non-null above.
        unsafe { *out = std::ptr::null_mut() };
        let site = self.state()?.site.clone().ok_or(E_FAIL)?;
        // SAFETY: iid and out are valid, as checked above.
        unsafe { site.query(iid, out) }.ok()
    }
}

const CLASS: PCWSTR = w!("PowerKitPreviewPane");
/// Posted when the window changed size while it was busy, to lay it out again after.
const WM_LAYOUT: u32 = WM_APP + 1;
/// Space around and between PDF pages.
const GAP: i32 = 8;
const LINE: i32 = 40;
const CACHED_PAGES: usize = 6;

fn register_class() -> Result<()> {
    // SAFETY: a window class of this module, whose procedure lives as long as it does.
    unsafe {
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: module(),
            lpszClassName: CLASS,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 {
            let error = Error::from_thread();
            if error.code() != ERROR_CLASS_ALREADY_EXISTS.to_hresult() {
                return Err(error);
            }
        }
    }
    Ok(())
}

struct Pane {
    theme: Theme,
    view: View,
}

enum View {
    Text {
        edit: isize,
        links: Vec<(String, String)>,
    },
    /// Drawn smaller to fit the pane, never larger.
    Picture(Image),
    Pdf(PdfView),
    Message(String),
}

struct PdfView {
    document: render::Pdf,
    top: i32,
    /// Pages drawn lately, by index and width; a page that failed keeps its error.
    drawn: Vec<(usize, u32, std::result::Result<Image, String>)>,
}

impl PdfView {
    fn page_width(client: &RECT) -> u32 {
        (client.right - 2 * GAP).max(1) as u32
    }

    fn heights(&self, width: u32) -> impl Iterator<Item = i32> + '_ {
        self.document
            .pages
            .iter()
            .map(move |&(w, h)| (width as f32 * h / w.max(1.0)).round() as i32)
    }

    fn total(&self, width: u32) -> i32 {
        self.heights(width).fold(GAP, |sum, h| sum.saturating_add(h + GAP))
    }

    fn page(&mut self, index: usize, width: u32) -> &std::result::Result<Image, String> {
        let at = match self.drawn.iter().position(|(i, w, _)| *i == index && *w == width) {
            Some(at) => at,
            None => {
                if self.drawn.len() >= CACHED_PAGES {
                    drop(self.drawn.remove(0));
                }
                let page = guarded(|| self.document.page(index, width)).map_err(|e| e.message());
                self.drawn.push((index, width, page));
                self.drawn.len() - 1
            }
        };
        &self.drawn[at].2
    }
}

fn build(format: Format, bytes: &[u8], cut: bool, extension: &str, theme: Theme, window: HWND) -> Result<View> {
    Ok(match format {
        Format::Markdown => {
            let mut doc = markdown::to_rtf(&decode_text(bytes), theme);
            if cut {
                doc.rtf.pop();
                doc.rtf.push_str(&format!(
                    "\\pard{{\\cf{} \\i Only the first {} MB are shown.}}\\par}}",
                    crate::rtf::Color::Muted as u8,
                    format.read_limit() >> 20
                ));
            }
            View::Text {
                edit: rich_edit(window, &doc.rtf, theme)?,
                links: doc.links,
            }
        }
        Format::Code => View::Text {
            edit: rich_edit(window, &code::to_rtf(&decode_text(bytes), extension, theme, cut), theme)?,
            links: Vec::new(),
        },
        // Drawings mostly assume a white page.
        Format::Svg => View::Picture(render::svg(&decode_text(bytes), 2048)?.over([255, 255, 255])),
        Format::Qoi => View::Picture(
            qoi::decode(bytes)
                .ok_or_else(|| failed("This is not a QOI picture."))?
                .fit(4096)
                .over(theme.background),
        ),
        Format::Gcode => View::Picture(gcode_picture(bytes, 4096)?.fit(4096).over(theme.background)),
        Format::Pdf => View::Pdf(PdfView {
            document: render::Pdf::open(bytes)?,
            top: 0,
            drawn: Vec::new(),
        }),
        Format::Stl => return Err(E_NOTIMPL.into()),
    })
}

fn rich_edit(parent: HWND, rtf: &str, theme: Theme) -> Result<isize> {
    // SAFETY: loading a system library, which stays loaded for the window class.
    unsafe { LoadLibraryW(w!("msftedit.dll")) }?;
    let style = WINDOW_STYLE((ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL) as u32);
    // SAFETY: a child of our own window.
    let edit = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            MSFTEDIT_CLASS,
            None,
            WS_CHILD | WS_VISIBLE | WS_VSCROLL | style,
            0,
            0,
            0,
            0,
            Some(parent),
            None,
            Some(module()),
            None,
        )
    }?;
    send(edit, EM_SETBKGNDCOLOR, 0, colorref(theme.background).0 as isize);
    send(
        edit,
        EM_SETMARGINS,
        (EC_LEFTMARGIN | EC_RIGHTMARGIN) as usize,
        12 | 12 << 16,
    );
    send(edit, EM_SETEVENTMASK, 0, ENM_LINK as isize);
    send(edit, EM_AUTOURLDETECT, AURL_ENABLEURL as usize, 0);
    let text = SETTEXTEX {
        flags: ST_DEFAULT,
        codepage: 0,
    };
    let mut rtf = rtf.as_bytes().to_vec();
    rtf.push(0);
    send(
        edit,
        EM_SETTEXTEX,
        &text as *const SETTEXTEX as usize,
        rtf.as_ptr() as isize,
    );
    Ok(edit.0 as isize)
}

/// Opens a clicked link, if it is a web or mail address.
fn open_link(edit: HWND, range: CHARRANGE, links: &[(String, String)]) {
    let len = range.cpMax.saturating_sub(range.cpMin);
    if !(1..=4096).contains(&len) {
        return;
    }
    let mut buffer = vec![0u16; len as usize + 1];
    let mut request = TEXTRANGEW {
        chrg: range,
        lpstrText: PWSTR(buffer.as_mut_ptr()),
    };
    let got = send(edit, EM_GETTEXTRANGE, 0, &mut request as *mut TEXTRANGEW as isize);
    let shown = String::from_utf16_lossy(&buffer[..(got.max(0) as usize).min(len as usize)]);
    let target = match shown.split_once("HYPERLINK \"") {
        Some((_, rest)) => rest.split('"').next().unwrap_or_default().to_string(),
        None => links
            .iter()
            .find(|(text, _)| *text == shown)
            .map_or(shown, |(_, url)| url.clone()),
    };
    let lower = target.to_ascii_lowercase();
    if ["http://", "https://", "mailto:"].iter().any(|p| lower.starts_with(p)) {
        // SAFETY: opening an address with its default program.
        unsafe {
            ShellExecuteW(
                None,
                w!("open"),
                &HSTRING::from(target.as_str()),
                None,
                None,
                SW_SHOWNORMAL,
            )
        };
    }
}

fn client(window: HWND) -> RECT {
    let mut rect = RECT::default();
    // SAFETY: our own window.
    let _ = unsafe { GetClientRect(window, &mut rect) };
    rect
}

/// Sizes the text box or the PDF's scroll bar to the window and redraws it.
fn layout(window: HWND) {
    let Some(cell) = pane_of(window) else { return };
    let Ok(mut pane) = cell.try_borrow_mut() else {
        // SAFETY: our own window, which lays itself out once it is free.
        let _ = unsafe { PostMessageW(Some(window), WM_LAYOUT, WPARAM(0), LPARAM(0)) };
        return;
    };
    let rect = client(window);
    match &mut pane.view {
        View::Text { edit, .. } => {
            let edit = *edit;
            drop(pane);
            // SAFETY: our own child window.
            let _ = unsafe { MoveWindow(hwnd(edit), 0, 0, rect.right, rect.bottom, true) };
        }
        View::Pdf(pdf) => {
            let total = pdf.total(PdfView::page_width(&rect));
            pdf.top = pdf.top.min(total - rect.bottom).max(0);
            let info = SCROLLINFO {
                cbSize: size_of::<SCROLLINFO>() as u32,
                fMask: SIF_RANGE | SIF_PAGE | SIF_POS,
                nMin: 0,
                nMax: total - 1,
                nPage: rect.bottom.max(0) as u32,
                nPos: pdf.top,
                nTrackPos: 0,
            };
            drop(pane);
            // SAFETY: our own window; showing or hiding the bar resizes it, which
            // comes back here.
            unsafe { SetScrollInfo(window, SB_VERT, &info, true) };
        }
        View::Picture(_) | View::Message(_) => {}
    }
    // SAFETY: our own window.
    let _ = unsafe { InvalidateRect(Some(window), None, false) };
}

fn pane_of(window: HWND) -> Option<&'static RefCell<Pane>> {
    // SAFETY: the pointer is the box set in DoPreview, alive until WM_NCDESTROY.
    unsafe { (GetWindowLongPtrW(window, GWLP_USERDATA) as *const RefCell<Pane>).as_ref() }
}

fn scroll(window: HWND, to: impl FnOnce(i32, i32) -> i32) {
    let Some(cell) = pane_of(window) else { return };
    let Ok(mut pane) = cell.try_borrow_mut() else { return };
    if let View::Pdf(pdf) = &mut pane.view {
        let page = client(window).bottom;
        pdf.top = to(pdf.top, page);
        drop(pane);
        layout(window);
    }
}

extern "system" fn window_proc(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match message {
        WM_SIZE | WM_LAYOUT => layout(window),
        WM_ERASEBKGND => return LRESULT(1),
        WM_PAINT => paint(window),
        WM_LBUTTONDOWN => {
            // SAFETY: our own window, taking the keys for scrolling.
            let _ = unsafe { SetFocus(Some(window)) };
        }
        WM_VSCROLL => {
            let code = (wparam.0 & 0xffff) as i32;
            let track = || {
                let mut info = SCROLLINFO {
                    cbSize: size_of::<SCROLLINFO>() as u32,
                    fMask: SIF_TRACKPOS,
                    ..Default::default()
                };
                // SAFETY: our own window's scroll bar.
                let _ = unsafe { GetScrollInfo(window, SB_VERT, &mut info) };
                info.nTrackPos
            };
            scroll(window, |top, page| match code {
                c if c == SB_LINEUP.0 => top - LINE,
                c if c == SB_LINEDOWN.0 => top + LINE,
                c if c == SB_PAGEUP.0 => top - page,
                c if c == SB_PAGEDOWN.0 => top + page,
                c if c == SB_THUMBTRACK.0 || c == SB_THUMBPOSITION.0 => track(),
                c if c == SB_TOP.0 => 0,
                c if c == SB_BOTTOM.0 => i32::MAX,
                _ => top,
            });
        }
        WM_MOUSEWHEEL => {
            let delta = (wparam.0 >> 16) as u16 as i16 as i32;
            scroll(window, |top, _| top - delta * 3 * LINE / 120);
        }
        WM_KEYDOWN => {
            let key = wparam.0 as u16;
            scroll(window, |top, page| match key {
                k if k == VK_UP.0 => top - LINE,
                k if k == VK_DOWN.0 => top + LINE,
                k if k == VK_PRIOR.0 => top - page,
                k if k == VK_NEXT.0 => top + page,
                k if k == VK_HOME.0 => 0,
                k if k == VK_END.0 => i32::MAX,
                _ => top,
            });
        }
        WM_NOTIFY => {
            // SAFETY: WM_NOTIFY carries an NMHDR, and EN_LINK an ENLINK, which is packed.
            let header = unsafe { std::ptr::read_unaligned(lparam.0 as *const NMHDR) };
            if header.code == EN_LINK {
                let link = unsafe { std::ptr::read_unaligned(lparam.0 as *const ENLINK) };
                if link.msg == WM_LBUTTONUP {
                    if let Some(Ok(pane)) = pane_of(window).map(RefCell::try_borrow) {
                        if let View::Text { edit, links } = &pane.view {
                            let _ = catch_unwind(AssertUnwindSafe(|| open_link(hwnd(*edit), link.chrg, links)));
                        }
                    }
                }
            }
        }
        WM_NCDESTROY => {
            // SAFETY: the box set in DoPreview, taken back once.
            unsafe {
                let raw = SetWindowLongPtrW(window, GWLP_USERDATA, 0) as *mut RefCell<Pane>;
                if !raw.is_null() {
                    drop(Box::from_raw(raw));
                }
            }
        }
        _ => {}
    }
    // SAFETY: default handling of our own window's message.
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

fn paint(window: HWND) {
    let mut ps = PAINTSTRUCT::default();
    // SAFETY: painting our own window into an off-screen bitmap, then onto it; every
    // GDI object made is deleted.
    unsafe {
        let dc = BeginPaint(window, &mut ps);
        let rect = client(window);
        let memory = CreateCompatibleDC(Some(dc));
        let bitmap = CreateCompatibleBitmap(dc, rect.right.max(1), rect.bottom.max(1));
        let old = SelectObject(memory, bitmap.into());
        if let Some(Ok(mut pane)) = pane_of(window).map(RefCell::try_borrow_mut) {
            let pane = &mut *pane;
            let _ = catch_unwind(AssertUnwindSafe(|| draw(memory, &rect, pane.theme, &mut pane.view)));
        }
        let _ = BitBlt(dc, 0, 0, rect.right, rect.bottom, Some(memory), 0, 0, SRCCOPY);
        SelectObject(memory, old);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(memory);
        let _ = EndPaint(window, &ps);
    }
}

fn draw(dc: HDC, rect: &RECT, theme: Theme, view: &mut View) {
    // SAFETY: drawing on the off-screen DC `paint` made.
    unsafe {
        let brush = CreateSolidBrush(colorref(theme.background));
        FillRect(dc, rect, brush);
        let _ = DeleteObject(brush.into());
        SetStretchBltMode(dc, HALFTONE);
        let _ = SetBrushOrgEx(dc, 0, 0, None);
    }
    match view {
        View::Text { .. } => {}
        View::Picture(image) => {
            let (w, h) = (image.width, image.height);
            let (fw, fh) = if w <= rect.right as u32 && h <= rect.bottom as u32 {
                (w, h)
            } else {
                let scale = (rect.right as f32 / w as f32).min(rect.bottom as f32 / h as f32);
                (
                    (w as f32 * scale).round().max(1.0) as u32,
                    (h as f32 * scale).round().max(1.0) as u32,
                )
            };
            let x = (rect.right - fw as i32) / 2;
            let y = (rect.bottom - fh as i32) / 2;
            blit(dc, image, x, y, fw, fh);
        }
        View::Pdf(pdf) => {
            let width = PdfView::page_width(rect);
            let heights: Vec<i32> = pdf.heights(width).collect();
            let mut y = GAP - pdf.top;
            for (index, height) in heights.into_iter().enumerate() {
                if y > rect.bottom {
                    break;
                }
                if y + height >= 0 {
                    match pdf.page(index, width) {
                        Ok(image) => blit(dc, image, GAP, y, image.width, image.height),
                        Err(e) => {
                            let mut area = RECT {
                                left: GAP,
                                top: y,
                                right: GAP + width as i32,
                                bottom: y + height,
                            };
                            text(
                                dc,
                                &mut area,
                                theme,
                                &format!("Page {} could not be shown.\n\n{e}", index + 1),
                            );
                        }
                    }
                }
                y += height + GAP;
            }
        }
        View::Message(message) => {
            let mut area = RECT {
                left: 16,
                top: 16,
                right: rect.right - 16,
                bottom: rect.bottom - 16,
            };
            text(dc, &mut area, theme, message);
        }
    }
}

fn blit(dc: HDC, image: &Image, x: i32, y: i32, width: u32, height: u32) {
    let info = bitmap_info(image);
    // SAFETY: `info` describes `image.pixels`.
    unsafe {
        StretchDIBits(
            dc,
            x,
            y,
            width as i32,
            height as i32,
            0,
            0,
            image.width as i32,
            image.height as i32,
            Some(image.pixels.as_ptr().cast()),
            &info,
            DIB_RGB_COLORS,
            SRCCOPY,
        );
    }
}

fn text(dc: HDC, area: &mut RECT, theme: Theme, message: &str) {
    let mut wide: Vec<u16> = message.encode_utf16().collect();
    // SAFETY: drawing on the off-screen DC with a stock font.
    unsafe {
        let old = SelectObject(dc, GetStockObject(DEFAULT_GUI_FONT));
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, colorref(theme.text));
        DrawTextW(dc, &mut wide, area, DT_CENTER | DT_WORDBREAK);
        SelectObject(dc, old);
    }
}
