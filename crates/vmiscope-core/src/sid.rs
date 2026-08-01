//! Owner SID → account name, cached.
//!
//! `Win32_ProcessStartTrace` carries the owner's SID **on the event** as a raw
//! byte array. That is strictly better than the usual approach of calling
//! `Win32_Process.GetOwner` afterwards, which races the process exiting — but a
//! byte array is not something to put in a table cell, so it has to be resolved.
//!
//! Two properties drive the design:
//!
//! * **The same handful of SIDs repeat forever.** A busy machine is
//!   `SYSTEM`, `LOCAL SERVICE`, `NETWORK SERVICE` and one or two interactive
//!   users, over and over. So the resolver is a cache first and a lookup
//!   second, keyed on the raw bytes (the SID *is* its own identity).
//! * **`LookupAccountSidW` can block.** On a domain-joined machine an unknown
//!   SID may cost a round trip to a domain controller. Resolution therefore
//!   belongs on a background thread, never on a UI or event-pump thread — see
//!   [`crate::procmon`], which does it on its details thread.
//!
//! A SID that cannot be resolved to a name renders in SDDL form
//! (`S-1-5-21-…`), never blank: "we do not know who" and "nobody" must not look
//! the same in a security tool.

use std::collections::HashMap;

use windows::core::PWSTR;
use windows::Win32::Foundation::{LocalFree, HLOCAL};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{IsValidSid, LookupAccountSidW, PSID, SID_NAME_USE};

/// A memoizing SID resolver.
///
/// Not `Sync`; give each thread its own, or keep the one that lives on the
/// details thread. Sharing it behind a mutex would serialize exactly the calls
/// that can block.
#[derive(Debug, Default)]
pub struct SidResolver {
    cache: HashMap<Vec<u8>, String>,
}

impl SidResolver {
    /// An empty resolver.
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolve `sid` to `DOMAIN\user`, falling back to SDDL.
    ///
    /// An **empty** slice returns an empty string: the event carried no SID at
    /// all, which is a different fact from "this SID does not resolve" and
    /// should not be dressed up as an account.
    pub fn resolve(&mut self, sid: &[u8]) -> String {
        if sid.is_empty() {
            return String::new();
        }
        if let Some(hit) = self.cache.get(sid) {
            return hit.clone();
        }
        let name = resolve_sid(sid);
        self.cache.insert(sid.to_vec(), name.clone());
        name
    }

    /// How many distinct SIDs have been resolved. Exists so a caller can prove
    /// the cache is doing its job rather than assuming it.
    pub fn cached(&self) -> usize {
        self.cache.len()
    }
}

/// Resolve one SID with no caching. Prefer [`SidResolver::resolve`].
///
/// Never returns an empty string for a non-empty input: name, else SDDL, else a
/// last-resort marker naming the byte length.
pub fn resolve_sid(sid: &[u8]) -> String {
    if sid.is_empty() {
        return String::new();
    }
    // Structural check *before* any pointer crosses the FFI boundary. These
    // bytes came off a WMI property, so their `SubAuthorityCount` is untrusted
    // input that claims how far past the buffer the API may read — and
    // `IsValidSid` cannot catch that, because it is handed a bare pointer and
    // has no idea how long the allocation is. Checking the declared length
    // against the actual one is the only place that can.
    if !is_structurally_sound(sid) {
        return format!("<malformed SID, {} bytes>", sid.len());
    }
    // A `SID` contains `DWORD` sub-authorities, so the API may read it as
    // 4-byte-aligned. A `&[u8]` carries no such guarantee — it came out of a
    // COM `SAFEARRAY` of `UI1` copied into a `Vec<u8>` — so it is re-homed into
    // a `u32`-aligned buffer before any pointer is handed across.
    let aligned: Vec<u32> = {
        let words = sid.len().div_ceil(4);
        let mut buf = vec![0u32; words];
        // SAFETY: the destination is `words * 4 >= sid.len()` bytes of owned,
        // initialized memory, and `u32` has no invalid bit patterns.
        unsafe {
            std::ptr::copy_nonoverlapping(sid.as_ptr(), buf.as_mut_ptr().cast::<u8>(), sid.len());
        }
        buf
    };
    let psid = PSID(aligned.as_ptr() as *mut core::ffi::c_void);

    // SAFETY: `psid` points at `aligned`, which outlives every call below.
    // `IsValidSid` is checked first precisely because the bytes are untrusted
    // input from a WMI property — passing a malformed SID to `LookupAccountSidW`
    // is not defined.
    unsafe {
        // Belt and braces: the structural check above proves the buffer is long
        // enough, `IsValidSid` proves the revision and sub-authority count are
        // ones the OS recognizes.
        if !IsValidSid(psid).as_bool() {
            return format!("<malformed SID, {} bytes>", sid.len());
        }
        if let Some(name) = lookup_account(psid) {
            return name;
        }
        if let Some(sddl) = sid_to_sddl(psid) {
            return sddl;
        }
    }
    format!("<unresolvable SID, {} bytes>", sid.len())
}

/// Does `sid` actually contain the SID its own header describes?
///
/// Layout: `Revision` (1 byte, must be 1), `SubAuthorityCount` (1 byte, at most
/// `SID_MAX_SUB_AUTHORITIES` = 15), a 6-byte identifier authority, then that
/// many little-endian `u32` sub-authorities.
fn is_structurally_sound(sid: &[u8]) -> bool {
    if sid.len() < 8 || sid[0] != 1 {
        return false;
    }
    let subs = sid[1] as usize;
    subs <= 15 && sid.len() >= 8 + subs * 4
}

/// `LookupAccountSidW` in its two-call form: ask for the sizes, then the data.
///
/// # Safety
/// `psid` must point at a valid, correctly aligned SID.
unsafe fn lookup_account(psid: PSID) -> Option<String> {
    let mut name_len = 0u32;
    let mut domain_len = 0u32;
    let mut kind = SID_NAME_USE::default();

    // The sizing call is *expected* to fail (`ERROR_INSUFFICIENT_BUFFER`); the
    // lengths it writes are what we are after. A SID that is genuinely unknown
    // also fails here, but with zero lengths, which is what the check catches.
    let _ = unsafe {
        LookupAccountSidW(
            None,
            psid,
            None,
            &mut name_len,
            None,
            &mut domain_len,
            &mut kind,
        )
    };
    if name_len == 0 {
        return None;
    }

    let mut name = vec![0u16; name_len as usize];
    let mut domain = vec![0u16; domain_len.max(1) as usize];
    unsafe {
        LookupAccountSidW(
            None,
            psid,
            Some(PWSTR(name.as_mut_ptr())),
            &mut name_len,
            Some(PWSTR(domain.as_mut_ptr())),
            &mut domain_len,
            &mut kind,
        )
        .ok()?;
    }

    let name = String::from_utf16_lossy(&name[..name_len as usize]);
    let domain = String::from_utf16_lossy(&domain[..domain_len as usize]);
    if name.is_empty() {
        None
    } else if domain.is_empty() {
        Some(name)
    } else {
        Some(format!("{domain}\\{name}"))
    }
}

/// Render a SID in SDDL string form (`S-1-5-18`).
///
/// # Safety
/// `psid` must point at a valid, correctly aligned SID.
unsafe fn sid_to_sddl(psid: PSID) -> Option<String> {
    let mut out = PWSTR::null();
    unsafe { ConvertSidToStringSidW(psid, &mut out).ok()? };
    if out.is_null() {
        return None;
    }
    // SAFETY: on success the API allocated a NUL-terminated wide string with
    // `LocalAlloc`, so it is ours to read and ours to free — and it must be
    // freed with `LocalFree`, not dropped.
    let s = unsafe { out.to_string().ok() };
    unsafe { LocalFree(Some(HLOCAL(out.0.cast()))) };
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `S-1-5-18` in its binary form: revision 1, one sub-authority, identifier
    /// authority 5 (NT Authority), sub-authority 18 (LOCAL SYSTEM).
    const SYSTEM_SID: [u8; 12] = [1, 1, 0, 0, 0, 0, 0, 5, 18, 0, 0, 0];

    /// `S-1-5-21-1111111111-2222222222-3333333333-4444` — well-formed, and
    /// (essentially certainly) not an account on the machine running the test.
    fn unknown_domain_sid() -> Vec<u8> {
        let mut v = vec![1u8, 5, 0, 0, 0, 0, 0, 5];
        for sub in [21u32, 1_111_111_111, 2_222_222_222, 3_333_333_333, 4_444] {
            v.extend_from_slice(&sub.to_le_bytes());
        }
        v
    }

    #[test]
    fn well_known_sid_resolves_to_an_account() {
        let name = resolve_sid(&SYSTEM_SID);
        // The domain and account names are localized, so the assertion is on
        // shape rather than on the English text: a `DOMAIN\user` pair.
        assert!(name.contains('\\'), "expected DOMAIN\\user, got {name:?}");
        assert!(!name.starts_with("S-1-"), "should not have fallen back");
    }

    #[test]
    fn an_unresolvable_sid_renders_as_sddl_not_blank() {
        let name = resolve_sid(&unknown_domain_sid());
        assert!(
            name.starts_with("S-1-5-21-"),
            "expected SDDL form, got {name:?}"
        );
    }

    #[test]
    fn a_malformed_sid_is_reported_rather_than_passed_to_the_api() {
        // Revision 1 claiming 5 sub-authorities in a buffer that holds none.
        let bogus = vec![1u8, 5, 0, 0, 0, 0, 0, 5];
        let name = resolve_sid(&bogus);
        assert!(!name.is_empty());
        assert!(name.starts_with('<'), "got {name:?}");
    }

    #[test]
    fn an_absent_sid_is_blank_not_a_fake_account() {
        assert_eq!(resolve_sid(&[]), "");
        assert_eq!(SidResolver::new().resolve(&[]), "");
    }

    #[test]
    fn the_cache_answers_the_second_time() {
        let mut r = SidResolver::new();
        assert_eq!(r.cached(), 0);
        let first = r.resolve(&SYSTEM_SID);
        assert_eq!(r.cached(), 1);
        let second = r.resolve(&SYSTEM_SID);
        assert_eq!(first, second);
        assert_eq!(r.cached(), 1, "a repeat lookup must not add an entry");
    }
}
