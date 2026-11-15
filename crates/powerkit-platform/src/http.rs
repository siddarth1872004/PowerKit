//! HTTPS GET through WinHTTP, which uses the system's proxy settings and certificate
//! store.

use crate::util::{pcwstr, wide};
use crate::{Context, Error, Result};
use std::ffi::c_void;
use windows::core::PCWSTR;
use windows::Win32::Networking::WinHttp::*;

/// Sent instead of anything that identifies the user or machine.
const USER_AGENT: &str = "PowerKit-updater";
const TIMEOUT_MS: i32 = 30_000;

struct Handle(*mut c_void);

impl Handle {
    fn new(h: *mut c_void, what: &str) -> Result<Handle> {
        if h.is_null() {
            return Err(Error::Os {
                context: what.into(),
                source: windows::core::Error::from_thread(),
            });
        }
        Ok(Handle(h))
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: a handle WinHTTP returned, closed once.
        let _ = unsafe { WinHttpCloseHandle(self.0) };
    }
}

/// Fetch an `https://` URL, following redirects. Fails on a status other than 200 or a
/// body larger than `max_bytes`.
pub fn get(url: &str, accept: &str, max_bytes: usize) -> Result<Vec<u8>> {
    let rest = url
        .strip_prefix("https://")
        .ok_or_else(|| Error::Other(format!("only https URLs are fetched: {url}")))?;
    let (host, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    let path = if path.is_empty() { "/" } else { path };
    let (agent, host, path, verb) = (wide(USER_AGENT), wide(host), wide(path), wide("GET"));
    let headers: Vec<u16> = format!("Accept: {accept}\r\n").encode_utf16().collect();
    // SAFETY: every string outlives the calls that use it; handles are closed by `Handle`
    // in reverse order of creation.
    unsafe {
        let session = Handle::new(
            WinHttpOpen(
                pcwstr(&agent),
                WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                PCWSTR::null(),
                PCWSTR::null(),
                0,
            ),
            "WinHttpOpen",
        )?;
        WinHttpSetTimeouts(session.0, TIMEOUT_MS, TIMEOUT_MS, TIMEOUT_MS, TIMEOUT_MS).context("WinHttpSetTimeouts")?;
        let connect = Handle::new(
            WinHttpConnect(session.0, pcwstr(&host), INTERNET_DEFAULT_HTTPS_PORT, 0),
            "WinHttpConnect",
        )?;
        let request = Handle::new(
            WinHttpOpenRequest(
                connect.0,
                pcwstr(&verb),
                pcwstr(&path),
                PCWSTR::null(),
                PCWSTR::null(),
                std::ptr::null(),
                WINHTTP_FLAG_SECURE,
            ),
            "WinHttpOpenRequest",
        )?;
        WinHttpSendRequest(request.0, Some(&headers), None, 0, 0, 0).context("WinHttpSendRequest")?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut()).context("WinHttpReceiveResponse")?;
        let mut status = 0u32;
        let mut len = size_of::<u32>() as u32;
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some((&raw mut status).cast()),
            &mut len,
            std::ptr::null_mut(),
        )
        .context("WinHttpQueryHeaders")?;
        if status != 200 {
            return Err(Error::Other(format!("{url} answered HTTP {status}")));
        }
        let mut body = Vec::new();
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            let mut read = 0u32;
            WinHttpReadData(request.0, chunk.as_mut_ptr().cast(), chunk.len() as u32, &mut read)
                .context("WinHttpReadData")?;
            if read == 0 {
                break;
            }
            if body.len() + read as usize > max_bytes {
                return Err(Error::Other(format!("{url} is larger than {max_bytes} bytes")));
            }
            body.extend_from_slice(&chunk[..read as usize]);
        }
        Ok(body)
    }
}
