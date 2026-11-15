//! Where the primary taskbar's app buttons are, in the order Win+1..Win+0 opens them.

use crate::util::{pcwstr, wide};
use crate::{Context, Result};
use powerkit_core::Rect;
use windows::core::PCWSTR;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, TreeScope, TreeScope_Children, TreeScope_Descendants,
    UIA_ButtonControlTypeId,
};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, FindWindowW};

/// Windows 11 taskbar buttons, which live in a XAML island.
const WIN11_BUTTON: &str = "Taskbar.TaskListButtonAutomationPeer";

fn find(parent: Option<windows::Win32::Foundation::HWND>, class: &str) -> Option<windows::Win32::Foundation::HWND> {
    let c = wide(class);
    // SAFETY: `c` outlives the call.
    unsafe { FindWindowExW(parent, None, pcwstr(&c), PCWSTR::null()) }.ok()
}

fn collect(
    uia: &IUIAutomation,
    root: &IUIAutomationElement,
    scope: TreeScope,
    keep: impl Fn(&IUIAutomationElement) -> bool,
) -> Result<Vec<Rect>> {
    // SAFETY: COM calls on live interface pointers owned by this function.
    unsafe {
        let all = root
            .FindAll(scope, &uia.CreateTrueCondition().context("CreateTrueCondition")?)
            .context("FindAll")?;
        let mut out = Vec::new();
        for i in 0..all.Length().context("Length")? {
            let e = all.GetElement(i).context("GetElement")?;
            if !keep(&e) || e.CurrentIsOffscreen().is_ok_and(|o| o.as_bool()) {
                continue;
            }
            if let Ok(r) = e.CurrentBoundingRectangle() {
                if r.right > r.left && r.bottom > r.top {
                    out.push(Rect::new(r.left, r.top, r.right, r.bottom));
                }
            }
        }
        Ok(out)
    }
}

/// Screen rectangles of the app buttons on the primary taskbar, first to last.
pub fn buttons() -> Result<Vec<Rect>> {
    // UI Automation needs COM on this thread. The apartment is kept for the life of the
    // thread; "already initialised" results are fine.
    // SAFETY: plain initialisation call.
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    // SAFETY: standard COM activation and calls on the returned interfaces.
    unsafe {
        let uia: IUIAutomation =
            CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).context("CoCreateInstance(UIAutomation)")?;
        let tray_class = wide("Shell_TrayWnd");
        let tray = FindWindowW(pcwstr(&tray_class), PCWSTR::null()).context("taskbar window")?;
        let root = uia.ElementFromHandle(tray).context("ElementFromHandle(taskbar)")?;
        let win11 = collect(&uia, &root, TreeScope_Descendants, |e| {
            e.CurrentClassName().is_ok_and(|c| c == WIN11_BUTTON)
        })?;
        if !win11.is_empty() {
            return Ok(win11);
        }
        // Windows 10: a classic task list window whose children are the buttons.
        let list = find(Some(tray), "ReBarWindow32")
            .and_then(|w| find(Some(w), "MSTaskSwWClass"))
            .and_then(|w| find(Some(w), "MSTaskListWClass"));
        let Some(list) = list else { return Ok(Vec::new()) };
        let root = uia.ElementFromHandle(list).context("ElementFromHandle(task list)")?;
        collect(&uia, &root, TreeScope_Children, |e| {
            e.CurrentControlType().is_ok_and(|t| t == UIA_ButtonControlTypeId)
        })
    }
}
