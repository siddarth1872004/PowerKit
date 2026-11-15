//! Keeping the computer (and optionally the display) awake.
//!
//! The request belongs to the calling thread and lasts until it is changed or the
//! thread exits, so it must be made from the long-lived event-loop thread.

use crate::{Error, Result};
use windows::Win32::System::Power::{
    SetThreadExecutionState, ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, EXECUTION_STATE,
};

fn set(flags: EXECUTION_STATE) -> Result<()> {
    // SAFETY: plain flag argument, no pointers.
    if unsafe { SetThreadExecutionState(flags) }.0 == 0 {
        return Err(Error::Other("SetThreadExecutionState rejected the request".into()));
    }
    Ok(())
}

/// Prevent sleep, and also the display turning off when `display` is set.
pub fn keep_awake(display: bool) -> Result<()> {
    let mut flags = ES_CONTINUOUS | ES_SYSTEM_REQUIRED;
    if display {
        flags |= ES_DISPLAY_REQUIRED;
    }
    set(flags)
}

/// Hand power management back to the power plan.
pub fn release() -> Result<()> {
    set(ES_CONTINUOUS)
}
