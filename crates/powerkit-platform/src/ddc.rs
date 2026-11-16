//! Monitor controls over DDC/CI (brightness, contrast, volume, input and the other
//! VESA MCCS codes) through the Monitor Configuration API.
//!
//! Every call talks to the monitor over its cable: reads and writes take tens of
//! milliseconds and the capabilities request can take seconds, so use these off the
//! event loop thread. Built-in laptop panels do not answer DDC/CI.

use crate::monitor;
use crate::util::from_wide;
use crate::{Context, Error, Result};
use std::collections::HashMap;
use std::marker::PhantomData;
use windows::Win32::Devices::Display::*;
use windows::Win32::Foundation::{ERROR_SUCCESS, HANDLE};

/// One physical monitor. Its handle is released when this is dropped.
pub struct Display {
    /// The monitor's own name ("DELL U2720Q"), or its driver's description.
    pub name: String,
    /// GDI device name of the output it shows (`\\.\DISPLAY1`).
    pub device: String,
    handle: HANDLE,
    _not_send: PhantomData<*const ()>,
}

/// Every physical monitor on every output, in [`monitor::enumerate`] order.
pub fn displays() -> Result<Vec<Display>> {
    let names = friendly_names().unwrap_or_else(|e| {
        tracing::debug!("monitor names unavailable: {e}");
        HashMap::new()
    });
    let mut out = Vec::new();
    for m in monitor::enumerate()? {
        let mut count = 0u32;
        // SAFETY: valid out pointer; the handle came from EnumDisplayMonitors.
        if let Err(e) = unsafe { GetNumberOfPhysicalMonitorsFromHMONITOR(m.raw(), &mut count) } {
            tracing::debug!(device = %m.id, "no physical monitors: {e}");
            continue;
        }
        let mut list = vec![PHYSICAL_MONITOR::default(); count as usize];
        if list.is_empty() {
            continue;
        }
        // SAFETY: `list` has room for `count` entries.
        unsafe { GetPhysicalMonitorsFromHMONITOR(m.raw(), &mut list) }.context("GetPhysicalMonitorsFromHMONITOR")?;
        let own = names.get(&m.id).filter(|n| n.len() == list.len());
        for (i, p) in list.iter().copied().enumerate() {
            // PHYSICAL_MONITOR is packed; copy the fields out before borrowing them.
            let (handle, description) = (p.hPhysicalMonitor, p.szPhysicalMonitorDescription);
            // Windows lists mirrored monitors in the same order as the display paths;
            // when the counts differ the driver description is the only safe name.
            let name = own
                .map(|n| n[i].clone())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| from_wide(&description));
            out.push(Display {
                name,
                device: m.id.clone(),
                handle,
                _not_send: PhantomData,
            });
        }
    }
    Ok(out)
}

impl Display {
    /// The monitor's MCCS capabilities string, which lists the codes it supports.
    pub fn capabilities(&self) -> Result<String> {
        let mut len = 0u32;
        // SAFETY: own handle; valid out pointer.
        if unsafe { GetCapabilitiesStringLength(self.handle, &mut len) } == 0 {
            return Err(last_error("GetCapabilitiesStringLength"));
        }
        let mut buf = vec![0u8; len.max(1) as usize];
        // SAFETY: own handle; the buffer is the length the monitor asked for.
        if unsafe { CapabilitiesRequestAndCapabilitiesReply(self.handle, &mut buf) } == 0 {
            return Err(last_error("CapabilitiesRequestAndCapabilitiesReply"));
        }
        let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
        Ok(String::from_utf8_lossy(&buf[..end]).into_owned())
    }

    /// The current and maximum value of VCP `code`.
    pub fn get(&self, code: u8) -> Result<(u32, u32)> {
        let (mut current, mut max) = (0u32, 0u32);
        // SAFETY: own handle; valid out pointers.
        let ok = unsafe { GetVCPFeatureAndVCPFeatureReply(self.handle, code, None, &mut current, Some(&mut max)) };
        if ok == 0 {
            return Err(last_error(format!("reading VCP code {code:#04X}")));
        }
        Ok((current, max))
    }

    pub fn set(&self, code: u8, value: u32) -> Result<()> {
        // SAFETY: own handle.
        if unsafe { SetVCPFeature(self.handle, code, value) } == 0 {
            return Err(last_error(format!("setting VCP code {code:#04X}")));
        }
        Ok(())
    }
}

impl Drop for Display {
    fn drop(&mut self) {
        let list = [PHYSICAL_MONITOR {
            hPhysicalMonitor: self.handle,
            ..Default::default()
        }];
        // SAFETY: the handle came from GetPhysicalMonitorsFromHMONITOR and is released once.
        if let Err(e) = unsafe { DestroyPhysicalMonitors(&list) } {
            tracing::debug!("DestroyPhysicalMonitors: {e}");
        }
    }
}

fn last_error(context: impl Into<String>) -> Error {
    Error::Os {
        context: context.into(),
        source: windows::core::Error::from_thread(),
    }
}

/// The monitors' own names on each GDI output, in display path order.
fn friendly_names() -> Result<HashMap<String, Vec<String>>> {
    let (mut paths_len, mut modes_len) = (0u32, 0u32);
    // SAFETY: valid out pointers.
    let r = unsafe { GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut paths_len, &mut modes_len) };
    if r != ERROR_SUCCESS {
        return Err(Error::Other(format!("GetDisplayConfigBufferSizes failed ({})", r.0)));
    }
    let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); paths_len as usize];
    let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); modes_len as usize];
    // SAFETY: the arrays have the lengths passed with them.
    let r = unsafe {
        QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut paths_len,
            paths.as_mut_ptr(),
            &mut modes_len,
            modes.as_mut_ptr(),
            None,
        )
    };
    if r != ERROR_SUCCESS {
        return Err(Error::Other(format!("QueryDisplayConfig failed ({})", r.0)));
    }
    paths.truncate(paths_len as usize);
    let mut names: HashMap<String, Vec<String>> = HashMap::new();
    for p in &paths {
        let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
            header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                size: std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                adapterId: p.sourceInfo.adapterId,
                id: p.sourceInfo.id,
            },
            ..Default::default()
        };
        let mut target = DISPLAYCONFIG_TARGET_DEVICE_NAME {
            header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
                size: std::mem::size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32,
                adapterId: p.targetInfo.adapterId,
                id: p.targetInfo.id,
            },
            ..Default::default()
        };
        // SAFETY: each packet starts with a header whose size matches its struct.
        let ok = unsafe {
            DisplayConfigGetDeviceInfo(&mut source.header) == 0 && DisplayConfigGetDeviceInfo(&mut target.header) == 0
        };
        if ok {
            names
                .entry(from_wide(&source.viewGdiDeviceName))
                .or_default()
                .push(from_wide(&target.monitorFriendlyDeviceName));
        }
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Talks to the real monitors; external monitors answer only when DDC/CI is on in
    /// their menus. Run with `--ignored`.
    #[test]
    #[ignore]
    fn reads_the_attached_monitors() {
        for d in displays().unwrap() {
            let caps = d.capabilities();
            let brightness = d.get(0x10);
            println!(
                "{} on {}: brightness {brightness:?}, capabilities {caps:?}",
                d.name, d.device
            );
        }
    }
}
