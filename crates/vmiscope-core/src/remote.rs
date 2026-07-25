//! Remote WMI with **alternate credentials** via raw DCOM.
//!
//! The `wmi` crate can connect to a remote host as the current user (SSO), but
//! its credentialed path resets the proxy identity to the local user, so
//! queries silently run as the wrong principal. Doing it correctly requires
//! calling `CoSetProxyBlanket` with a `COAUTHIDENTITY` — hence this raw layer.
//!
//! ⚠ **Unverified against a live remote host.** WMI forbids credentialed
//! *local* connections, so this path cannot be runtime-tested on a single
//! machine; it is compiled and its credential plumbing is exercised, but the
//! remote query path should be validated against a real remote target.

use std::collections::HashMap;
use std::ffi::c_void;

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::System::Com::{
    CoCreateInstance, CoSetProxyBlanket, CLSCTX_INPROC_SERVER, COAUTHIDENTITY, EOAC_NONE,
    RPC_C_AUTHN_LEVEL_PKT_PRIVACY, RPC_C_IMP_LEVEL_IMPERSONATE,
};
use windows::Win32::System::Rpc::{
    RPC_C_AUTHN_WINNT, RPC_C_AUTHZ_NONE, SEC_WINNT_AUTH_IDENTITY_UNICODE,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::System::Wmi::{
    IEnumWbemClassObject, IWbemClassObject, IWbemContext, IWbemLocator, IWbemServices, WbemLocator,
    WBEM_FLAG_CONNECT_USE_MAX_WAIT, WBEM_FLAG_FORWARD_ONLY, WBEM_FLAG_NONSYSTEM_ONLY,
    WBEM_FLAG_RETURN_IMMEDIATELY, WBEM_INFINITE,
};
use wmi::{Variant, WMIConnection};

use crate::value::variant_to_string;

/// Alternate credentials for a remote WMI connection. Password is redacted in
/// its `Debug` output so it never lands in a log.
#[derive(Clone, PartialEq, Eq)]
pub struct Credential {
    pub user: String,
    pub password: String,
    pub domain: Option<String>,
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credential")
            .field("user", &self.user)
            .field("password", &"<redacted>")
            .field("domain", &self.domain)
            .finish()
    }
}

/// A `COAUTHIDENTITY` plus the UTF-16 buffers it points at. DCOM dereferences
/// the identity on every call, so the buffers must outlive the connection and
/// the struct's address must be stable — hence it lives behind a `Box`.
struct PinnedAuth {
    ident: COAUTHIDENTITY,
    _user: Vec<u16>,
    _domain: Vec<u16>,
    _password: Vec<u16>,
}

impl PinnedAuth {
    fn new(cred: &Credential) -> Box<PinnedAuth> {
        let mut user: Vec<u16> = cred.user.encode_utf16().collect();
        let mut domain: Vec<u16> = cred
            .domain
            .clone()
            .unwrap_or_default()
            .encode_utf16()
            .collect();
        let mut password: Vec<u16> = cred.password.encode_utf16().collect();
        let ident = COAUTHIDENTITY {
            User: user.as_mut_ptr(),
            UserLength: user.len() as u32,
            Domain: domain.as_mut_ptr(),
            DomainLength: domain.len() as u32,
            Password: password.as_mut_ptr(),
            PasswordLength: password.len() as u32,
            Flags: SEC_WINNT_AUTH_IDENTITY_UNICODE.0,
        };
        // Moving the Vecs into the box keeps their heap buffers (and thus the
        // pointers stored in `ident`) valid.
        Box::new(PinnedAuth {
            ident,
            _user: user,
            _domain: domain,
            _password: password,
        })
    }
}

/// A raw DCOM connection to a remote host authenticated with alternate creds.
pub struct RemoteConn {
    svc: IWbemServices,
    auth: Box<PinnedAuth>,
}

unsafe fn set_blanket(proxy: &windows::core::IUnknown, ident: *const c_void) -> anyhow::Result<()> {
    CoSetProxyBlanket(
        proxy,
        RPC_C_AUTHN_WINNT,
        RPC_C_AUTHZ_NONE,
        PCWSTR::null(),
        RPC_C_AUTHN_LEVEL_PKT_PRIVACY,
        RPC_C_IMP_LEVEL_IMPERSONATE,
        Some(ident),
        EOAC_NONE,
    )?;
    Ok(())
}

unsafe fn object_to_map(obj: &IWbemClassObject) -> anyhow::Result<HashMap<String, Variant>> {
    let mut map = HashMap::new();
    obj.BeginEnumeration(WBEM_FLAG_NONSYSTEM_ONLY.0)?;
    loop {
        let mut name = windows::core::BSTR::new();
        let mut val = VARIANT::default();
        let mut ctype = 0i32;
        let mut flavor = 0i32;
        // WBEM_S_NO_MORE_DATA is a success HRESULT -> Ok(()) with an empty name.
        obj.Next(0, &mut name, &mut val, &mut ctype, &mut flavor)?;
        if name.is_empty() {
            break;
        }
        map.insert(
            name.to_string(),
            Variant::from_variant(&val).unwrap_or(Variant::Null),
        );
    }
    obj.EndEnumeration()?;
    Ok(map)
}

unsafe fn get_string_prop(obj: &IWbemClassObject, prop: &str) -> Option<String> {
    let h = HSTRING::from(prop);
    let mut val = VARIANT::default();
    obj.Get(PCWSTR(h.as_ptr()), 0, &mut val, None, None).ok()?;
    Some(variant_to_string(&Variant::from_variant(&val).ok()?))
}

impl RemoteConn {
    /// Connect to `\\host\namespace` with alternate credentials.
    pub fn connect(host: &str, namespace: &str, cred: &Credential) -> anyhow::Result<RemoteConn> {
        // Cold-start guard: ensure COM + CoInitializeSecurity ran on this thread
        // before the raw CoCreateInstance (the `wmi` crate does this lazily).
        let _com = WMIConnection::new();

        let full = format!(r"\\{host}\{namespace}");
        let authority = match &cred.domain {
            Some(d) if !d.is_empty() => format!("NTLMDOMAIN:{d}"),
            _ => String::new(),
        };
        let auth = PinnedAuth::new(cred);
        unsafe {
            let loc: IWbemLocator = CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER)?;
            let svc: IWbemServices = loc.ConnectServer(
                &windows::core::BSTR::from(full),
                &windows::core::BSTR::from(cred.user.as_str()),
                &windows::core::BSTR::from(cred.password.as_str()),
                &windows::core::BSTR::new(),
                WBEM_FLAG_CONNECT_USE_MAX_WAIT.0,
                &windows::core::BSTR::from(authority),
                None::<&IWbemContext>,
            )?;
            let ident_ptr = &auth.ident as *const COAUTHIDENTITY as *const c_void;
            set_blanket((&svc).into(), ident_ptr)?;
            Ok(RemoteConn { svc, auth })
        }
    }

    fn exec(&self, wql: &str) -> anyhow::Result<IEnumWbemClassObject> {
        unsafe {
            let en: IEnumWbemClassObject = self.svc.ExecQuery(
                &windows::core::BSTR::from("WQL"),
                &windows::core::BSTR::from(wql),
                WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
                None::<&IWbemContext>,
            )?;
            // The enumerator is a separate proxy and does NOT inherit the
            // service's blanket — re-apply the credentials or every read is
            // Access Denied.
            let ident_ptr = &self.auth.ident as *const COAUTHIDENTITY as *const c_void;
            set_blanket((&en).into(), ident_ptr)?;
            Ok(en)
        }
    }

    /// Run a query, returning each row as a property map.
    pub fn exec_maps(&self, wql: &str) -> anyhow::Result<Vec<HashMap<String, Variant>>> {
        let en = self.exec(wql)?;
        let mut out = Vec::new();
        unsafe {
            loop {
                let mut objs: [Option<IWbemClassObject>; 1] = [None];
                let mut returned = 0u32;
                if en.Next(WBEM_INFINITE, &mut objs, &mut returned).is_err() {
                    break;
                }
                if returned == 0 {
                    break;
                }
                if let Some(obj) = objs[0].take() {
                    out.push(object_to_map(&obj)?);
                }
            }
        }
        Ok(out)
    }

    /// Enumerate class names via `meta_class`.
    pub fn list_class_names(&self) -> anyhow::Result<Vec<String>> {
        let en = self.exec("SELECT * FROM meta_class")?;
        let mut names = Vec::new();
        unsafe {
            loop {
                let mut objs: [Option<IWbemClassObject>; 1] = [None];
                let mut returned = 0u32;
                if en.Next(WBEM_INFINITE, &mut objs, &mut returned).is_err() || returned == 0 {
                    break;
                }
                if let Some(obj) = objs[0].take() {
                    if let Some(name) = get_string_prop(&obj, "__CLASS") {
                        if !name.is_empty() {
                            names.push(name);
                        }
                    }
                }
            }
        }
        names.sort_unstable();
        names.dedup();
        Ok(names)
    }
}
