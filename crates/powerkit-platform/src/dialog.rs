//! The standard Open and Save As dialogs.

use crate::util::{pcwstr, wide};
use crate::{Context, Result};
use std::path::{Path, PathBuf};
use windows::core::Interface;
use windows::Win32::Foundation::{ERROR_CANCELLED, HWND};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FileOpenDialog, FileSaveDialog, IFileDialog, IFileOpenDialog, IFileSaveDialog, SIGDN_FILESYSPATH,
};

/// What the dialog offers: `(description, pattern)` pairs such as
/// `("Registry files", "*.reg")`, and the extension added to a typed name.
pub struct FileKinds<'a> {
    pub filters: &'a [(&'a str, &'a str)],
    pub extension: &'a str,
}

/// Ask for a file to open (`save == false`) or a place to save (`save == true`,
/// suggesting `name`). `None` when the user cancelled. Runs a modal loop on the
/// calling thread, which must be the one that owns `owner`.
pub(crate) fn pick(owner: HWND, save: bool, kinds: &FileKinds, name: Option<&Path>) -> Result<Option<PathBuf>> {
    // The dialogs are COM objects; an apartment already set up on this thread is fine.
    // SAFETY: plain initialisation call.
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let texts: Vec<(Vec<u16>, Vec<u16>)> = kinds.filters.iter().map(|(n, p)| (wide(n), wide(p))).collect();
    let specs: Vec<COMDLG_FILTERSPEC> = texts
        .iter()
        .map(|(n, p)| COMDLG_FILTERSPEC {
            pszName: pcwstr(n),
            pszSpec: pcwstr(p),
        })
        .collect();
    let extension = wide(kinds.extension);
    // SAFETY: standard COM activation and calls; every string outlives the calls that
    // use it, and the returned path is freed with CoTaskMemFree once copied.
    unsafe {
        let dialog: IFileDialog = if save {
            CoCreateInstance::<_, IFileSaveDialog>(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)
                .context("creating the Save As dialog")?
                .cast()
                .context("IFileDialog")?
        } else {
            CoCreateInstance::<_, IFileOpenDialog>(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)
                .context("creating the Open dialog")?
                .cast()
                .context("IFileDialog")?
        };
        dialog.SetFileTypes(&specs).context("SetFileTypes")?;
        dialog
            .SetDefaultExtension(pcwstr(&extension))
            .context("SetDefaultExtension")?;
        if let Some(name) = name {
            if let Some(file) = name.file_name() {
                let file = wide(&file.to_string_lossy());
                dialog.SetFileName(pcwstr(&file)).context("SetFileName")?;
            }
        }
        match dialog.Show(Some(owner)) {
            Ok(()) => {}
            Err(e) if e.code() == ERROR_CANCELLED.to_hresult() => return Ok(None),
            Err(e) => return Err(e).context("showing the file dialog"),
        }
        let item = dialog.GetResult().context("reading the chosen file")?;
        let path = item
            .GetDisplayName(SIGDN_FILESYSPATH)
            .context("reading the chosen file")?;
        let text = path.to_string();
        CoTaskMemFree(Some(path.0 as *const _));
        Ok(Some(PathBuf::from(text.map_err(|e| {
            crate::Error::Other(format!("the chosen path is not valid text: {e}"))
        })?)))
    }
}

/// Ask a yes-or-no question in a message box of its own; true for yes.
pub fn ask(title: &str, text: &str) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, IDYES, MB_DEFBUTTON2, MB_ICONQUESTION, MB_YESNO};
    let (title, text) = (wide(title), wide(text));
    // SAFETY: the strings outlive the modal call.
    unsafe {
        MessageBoxW(
            None,
            pcwstr(&text),
            pcwstr(&title),
            MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2,
        ) == IDYES
    }
}

/// Show `text` in a message box of its own, as a problem when `error`.
pub fn tell(title: &str, text: &str, error: bool) {
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_ICONINFORMATION, MB_OK};
    let (title, text) = (wide(title), wide(text));
    let icon = if error { MB_ICONERROR } else { MB_ICONINFORMATION };
    // SAFETY: the strings outlive the modal call.
    let _ = unsafe { MessageBoxW(None, pcwstr(&text), pcwstr(&title), MB_OK | icon) };
}
