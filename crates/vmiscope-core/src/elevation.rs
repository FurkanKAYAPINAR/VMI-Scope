//! Whether this process is running with an elevated token.
//!
//! The Process view needs this for one reason: the WMI Kernel Trace Event
//! Provider refuses a `Win32_ProcessStartTrace` subscription on this machine's
//! UAC-filtered admin token (`WBEM_E_ACCESS_DENIED`, measured — see
//! `docs/FINDINGS.md`). Elevation is the *suspected* lift for that denial, but
//! it has never been observed working, so nothing here decides anything: the
//! monitor always tries the trace subscription and falls back on the actual
//! error. [`is_elevated`] exists to label the situation for the operator, not
//! to gate a code path.
//!
//! "Elevated" here means exactly what `TokenElevation` means: the token is not
//! a UAC-filtered one. That is deliberately *not* the same question as "is this
//! account an administrator" — a filtered admin token still carries
//! `BUILTIN\Administrators` as a deny-only SID, so a membership check would say
//! yes while every privileged operation still failed.

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// Is this process running with a non-filtered (elevated) token?
///
/// Returns `false` on any failure. A token query that cannot even be opened is
/// not an elevated process by any useful definition, and this value only ever
/// drives a label, so failing closed is both safe and honest.
pub fn is_elevated() -> bool {
    // SAFETY: `GetCurrentProcess` returns a pseudo-handle that needs no close
    // and is valid for the life of the process. The token handle it yields is
    // real and is closed on every path below.
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut returned = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some((&mut elevation as *mut TOKEN_ELEVATION).cast()),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
        .is_ok();
        // The handle is ours; closing it cannot invalidate anything the caller
        // still holds, because nothing is handed out.
        let _ = CloseHandle(token);
        // `TokenIsElevated` is a `u32` used as a boolean, so any non-zero value
        // counts. It is only meaningful when the call succeeded *and* filled
        // the whole struct — a short write would leave stack garbage.
        ok && returned as usize == std::mem::size_of::<TOKEN_ELEVATION>()
            && elevation.TokenIsElevated != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// There is no way to assert the *value* portably — it depends on how the
    /// test runner was launched. What can be asserted is that the call is
    /// well-formed: it returns rather than trapping, and it is stable.
    #[test]
    fn elevation_query_is_stable() {
        let a = is_elevated();
        let b = is_elevated();
        assert_eq!(a, b);
    }
}
