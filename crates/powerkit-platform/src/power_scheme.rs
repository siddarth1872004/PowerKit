//! Windows power plans and the power mode (the Windows 11 slider of the Balanced
//! plan). Plans and modes are named by their GUIDs, written in lowercase.

use crate::{Error, Result};
use windows::core::GUID;
use windows::Win32::Foundation::{LocalFree, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS, HLOCAL, WIN32_ERROR};
use windows::Win32::System::Power::{
    PowerEnumerate, PowerGetActiveScheme, PowerReadFriendlyName, PowerSetActiveScheme, ACCESS_SCHEME,
};

// Exported by powrprof.dll since Windows 10 1709 but not described in the
// metadata the `windows` crate is generated from.
#[link(name = "powrprof", kind = "raw-dylib")]
unsafe extern "system" {
    fn PowerGetEffectiveOverlayScheme(guid: *mut GUID) -> u32;
    fn PowerSetActiveOverlayScheme(guid: GUID) -> u32;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub guid: String,
    pub name: String,
}

fn check(code: WIN32_ERROR, what: &str) -> Result<()> {
    if code == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(Error::Other(format!(
            "{what}: {}",
            windows::core::Error::from(code.to_hresult()).message()
        )))
    }
}

fn text(guid: &GUID) -> String {
    format!("{guid:?}").to_lowercase()
}

fn parse(guid: &str) -> Result<GUID> {
    GUID::try_from(guid).map_err(|_| Error::Other(format!("{guid:?} is not a GUID")))
}

fn name_of(guid: &GUID) -> Result<String> {
    let mut size = 0u32;
    // SAFETY: the first call only reads the size; the buffer passed to the second is
    // `size` bytes long.
    unsafe {
        check(
            PowerReadFriendlyName(None, Some(guid), None, None, None, &mut size),
            "reading a power plan's name",
        )?;
        let mut buf = vec![0u16; (size as usize).div_ceil(2)];
        check(
            PowerReadFriendlyName(None, Some(guid), None, None, Some(buf.as_mut_ptr().cast()), &mut size),
            "reading a power plan's name",
        )?;
        Ok(crate::util::from_wide(&buf))
    }
}

/// The power plans on this computer, in the order Windows lists them.
pub fn plans() -> Result<Vec<Plan>> {
    let mut out = Vec::new();
    for index in 0.. {
        let mut guid = GUID::zeroed();
        let mut size = std::mem::size_of::<GUID>() as u32;
        // SAFETY: the buffer is a GUID and `size` is its length in bytes.
        let code = unsafe {
            PowerEnumerate(
                None,
                None,
                None,
                ACCESS_SCHEME,
                index,
                Some((&mut guid as *mut GUID).cast()),
                &mut size,
            )
        };
        if code == ERROR_NO_MORE_ITEMS {
            break;
        }
        check(code, "listing the power plans")?;
        out.push(Plan {
            name: name_of(&guid)?,
            guid: text(&guid),
        });
    }
    Ok(out)
}

pub fn active_plan() -> Result<String> {
    let mut ptr: *mut GUID = std::ptr::null_mut();
    // SAFETY: Windows allocates the GUID; it is read once and freed with LocalFree.
    unsafe {
        check(PowerGetActiveScheme(None, &mut ptr), "reading the power plan")?;
        let guid = *ptr;
        let _ = LocalFree(Some(HLOCAL(ptr.cast())));
        Ok(text(&guid))
    }
}

pub fn set_plan(guid: &str) -> Result<()> {
    let g = parse(guid)?;
    // SAFETY: `g` outlives the call.
    check(
        unsafe { PowerSetActiveScheme(None, Some(&g)) },
        "switching the power plan",
    )
}

/// The power mode in effect; the all-zero GUID is the plain Balanced mode.
pub fn active_mode() -> Result<String> {
    let mut guid = GUID::zeroed();
    // SAFETY: valid out pointer.
    let code = unsafe { PowerGetEffectiveOverlayScheme(&mut guid) };
    check(WIN32_ERROR(code), "reading the power mode")?;
    Ok(text(&guid))
}

pub fn set_mode(guid: &str) -> Result<()> {
    let g = parse(guid)?;
    // SAFETY: plain call taking the GUID by value.
    let code = unsafe { PowerSetActiveOverlayScheme(g) };
    check(WIN32_ERROR(code), "switching the power mode")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_active_plan_is_one_of_the_listed_plans() {
        let plans = plans().unwrap();
        assert!(plans.iter().all(|p| !p.name.is_empty()));
        let active = active_plan().unwrap();
        assert!(plans.iter().any(|p| p.guid == active), "{active} not in {plans:?}");
        assert_eq!(parse(&active).map(|g| text(&g)).unwrap(), active);
    }
}
