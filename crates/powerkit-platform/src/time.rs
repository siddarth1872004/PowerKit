//! Local time conversion using the system time zone rules, including the daylight
//! saving rule that applied at the given instant (not the one in effect today).

use crate::{Context, Result};
use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToFileTime, SystemTimeToTzSpecificLocalTime};

/// Seconds between 1601-01-01 (FILETIME epoch) and 1970-01-01.
const EPOCH_DIFF: i64 = 11_644_473_600;
const TICKS: i64 = 10_000_000;

/// Unix seconds (UTC) to "local Unix seconds": the same calendar fields shifted so that
/// formatting them as UTC shows local wall-clock time.
pub fn to_local(utc_secs: i64) -> Result<i64> {
    let ticks = (utc_secs + EPOCH_DIFF).max(0) as u64 * TICKS as u64;
    let ft = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let mut utc = SYSTEMTIME::default();
    let mut local = SYSTEMTIME::default();
    let mut out = FILETIME::default();
    // SAFETY: every pointer refers to a live local of the right type.
    unsafe {
        FileTimeToSystemTime(&ft, &mut utc).context("FileTimeToSystemTime")?;
        SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).context("SystemTimeToTzSpecificLocalTime")?;
        SystemTimeToFileTime(&local, &mut out).context("SystemTimeToFileTime")?;
    }
    let ticks = (u64::from(out.dwHighDateTime) << 32) | u64::from(out.dwLowDateTime);
    Ok(ticks as i64 / TICKS - EPOCH_DIFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_is_plausible() {
        let now = 1_700_000_000;
        let offset = to_local(now).unwrap() - now;
        assert!(offset.abs() <= 14 * 3600, "offset {offset}");
        assert_eq!(offset % 900, 0, "time zone offsets are multiples of 15 minutes");
    }
}
