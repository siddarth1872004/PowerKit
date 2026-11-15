//! Icons from .ico files built into the program, for window title bars and lists.

use crate::{Context, Result};
use std::cell::RefCell;
use std::collections::HashMap;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconFromResourceEx, SendMessageW, HICON, ICON_BIG, ICON_SMALL, LR_DEFAULTCOLOR, SM_CXICON, SM_CXSMICON,
    WM_SETICON,
};

/// The image in `ico` best suited to `size` pixels: the smallest at least that big,
/// otherwise the biggest. `None` when `ico` is not an icon file.
pub(crate) fn pick(ico: &[u8], size: u32) -> Option<&[u8]> {
    let u16_at = |i: usize| Some(u16::from_le_bytes(ico.get(i..i + 2)?.try_into().ok()?));
    let u32_at = |i: usize| Some(u32::from_le_bytes(ico.get(i..i + 4)?.try_into().ok()?));
    if u16_at(0)? != 0 || u16_at(2)? != 1 {
        return None;
    }
    let mut best: Option<(u32, &[u8])> = None;
    for n in 0..u16_at(4)? as usize {
        let e = 6 + n * 16;
        let side = match *ico.get(e)? {
            0 => 256,
            w => u32::from(w),
        };
        let (len, at) = (u32_at(e + 8)? as usize, u32_at(e + 12)? as usize);
        let image = ico.get(at..at.checked_add(len)?)?;
        let better = match best {
            None => true,
            Some((b, _)) if b < size => side > b,
            Some((b, _)) => side >= size && side < b,
        };
        if better {
            best = Some((side, image));
        }
    }
    best.map(|(_, image)| image)
}

thread_local! {
    /// Icons made so far, by file and size. They are kept until the process ends:
    /// there are only a few, and windows and lists may still be showing them.
    static MADE: RefCell<HashMap<(usize, i32), isize>> = RefCell::new(HashMap::new());
}

/// An icon `size` pixels square from the .ico file `ico`.
pub fn load(ico: &'static [u8], size: i32) -> Result<HICON> {
    let key = (ico.as_ptr() as usize, size);
    if let Some(h) = MADE.with(|m| m.borrow().get(&key).copied()) {
        return Ok(HICON(h as *mut _));
    }
    let image = pick(ico, size.max(1) as u32).ok_or_else(|| crate::Error::Other("not an icon file".into()))?;
    // SAFETY: `image` is one complete image from the icon file; 0x30000 is the
    // format version Windows expects.
    let icon = unsafe { CreateIconFromResourceEx(image, true, 0x0003_0000, size, size, LR_DEFAULTCOLOR) }
        .context("CreateIconFromResourceEx")?;
    MADE.with(|m| m.borrow_mut().insert(key, icon.0 as isize));
    Ok(icon)
}

/// Give `hwnd` the icon `ico` in its title bar and in the taskbar.
pub(crate) fn set_window_icon(hwnd: HWND, ico: &'static [u8]) -> Result<()> {
    // SAFETY: plain queries and messages on a window the caller owns.
    unsafe {
        let dpi = GetDpiForWindow(hwnd).max(96);
        for (which, metric) in [(ICON_SMALL, SM_CXSMICON), (ICON_BIG, SM_CXICON)] {
            let icon = load(ico, GetSystemMetricsForDpi(metric, dpi))?;
            SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(which as usize)),
                Some(LPARAM(icon.0 as isize)),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::pick;

    fn ico(sides: &[u8]) -> Vec<u8> {
        let mut out = vec![0, 0, 1, 0, sides.len() as u8, 0];
        let first = 6 + 16 * sides.len() as u32;
        for (at, &s) in (first..).zip(sides) {
            out.extend([s, s, 0, 0, 1, 0, 32, 0]);
            out.extend(1u32.to_le_bytes());
            out.extend(at.to_le_bytes());
        }
        out.extend(sides);
        out
    }

    #[test]
    fn picks_the_smallest_image_that_is_big_enough() {
        let file = ico(&[16, 48, 32, 0]);
        assert_eq!(pick(&file, 20), Some(&[32u8][..]));
        assert_eq!(pick(&file, 32), Some(&[32u8][..]));
        assert_eq!(pick(&file, 10), Some(&[16u8][..]));
        assert_eq!(pick(&file, 100), Some(&[0u8][..]), "256 is stored as 0");
        let small = ico(&[16, 24]);
        assert_eq!(pick(&small, 64), Some(&[24u8][..]), "else the biggest");
    }

    #[test]
    fn rejects_what_is_not_an_icon() {
        assert_eq!(pick(b"", 16), None);
        assert_eq!(pick(b"\x89PNG\r\n\x1a\n", 16), None);
        let mut cut = ico(&[16]);
        cut.pop();
        assert_eq!(pick(&cut, 16), None);
    }
}
