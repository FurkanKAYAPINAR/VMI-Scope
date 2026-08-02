//! Remote WMI with **alternate credentials** via raw DCOM.
//!
//! The `wmi` crate can connect to a remote host as the current user (SSO), but
//! its credentialed path resets the proxy identity to the local user, so
//! queries silently run as the wrong principal. Doing it correctly requires
//! calling `CoSetProxyBlanket` with a `COAUTHIDENTITY` — hence this raw layer.
//!
//! ⚠ **Unverified against a live remote host.** WMI forbids credentialed
//! *local* connections — `ConnectServer` refuses with
//! `WBEM_E_LOCAL_CREDENTIALS (0x80041064)` before the proxy blanket is even
//! set — so no successful session on this transport can exist on one machine
//! and nothing here can be observed returning data.
//!
//! What *is* established locally, by `examples/altcred.rs`: the credentials
//! reach DCOM and are rejected by it rather than mishandled here, and a worker
//! in this mode with credentials that cannot connect refuses **all fifteen**
//! request shapes rather than quietly answering one of them from a current-user
//! connection. The second is the property that matters — see
//! [`crate::worker`]'s dispatcher — and it did not hold before Phase 5.

use std::collections::HashMap;
use std::ffi::c_void;
use std::time::Duration;

use windows::core::PCWSTR;
use windows::Win32::System::Com::{
    CoCreateInstance, CoSetProxyBlanket, CLSCTX_INPROC_SERVER, COAUTHIDENTITY, EOAC_NONE,
    RPC_C_AUTHN_LEVEL_PKT_PRIVACY,
};
use windows::Win32::System::Rpc::{
    RPC_C_AUTHN_WINNT, RPC_C_AUTHZ_NONE, SEC_WINNT_AUTH_IDENTITY_UNICODE,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::System::Wmi::{
    IEnumWbemClassObject, IWbemClassObject, IWbemContext, IWbemLocator, IWbemServices, WbemLocator,
    CIMTYPE_ENUMERATION, WBEM_FLAG_CONNECT_USE_MAX_WAIT, WBEM_FLAG_FORWARD_ONLY,
    WBEM_FLAG_NONSYSTEM_ONLY, WBEM_FLAG_RETURN_IMMEDIATELY, WBEM_GENERIC_FLAG_TYPE,
};
use wmi::{Variant, WMIConnection};

use crate::enumerate::{depth, CancelToken, Completion, ENUM_FLAGS};
use crate::host::Impersonation;

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

impl Credential {
    /// `DOMAIN\user`, or bare `user` when no domain was supplied.
    ///
    /// The display and identity form — never the authentication form. WMI wants
    /// the domain in the `NTLMDOMAIN:` authority argument and the user name on
    /// its own, which is why [`RemoteConn::connect`] does not use this.
    pub fn qualified_user(&self) -> String {
        match &self.domain {
            Some(d) if !d.is_empty() => format!(r"{d}\{}", self.user),
            _ => self.user.clone(),
        }
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
    imp: Impersonation,
}

unsafe fn set_blanket(
    proxy: &windows::core::IUnknown,
    ident: *const c_void,
    imp: Impersonation,
) -> anyhow::Result<()> {
    CoSetProxyBlanket(
        proxy,
        RPC_C_AUTHN_WINNT,
        RPC_C_AUTHZ_NONE,
        PCWSTR::null(),
        RPC_C_AUTHN_LEVEL_PKT_PRIVACY,
        imp.level(),
        Some(ident),
        EOAC_NONE,
    )?;
    Ok(())
}

/// The system properties a snapshot diff needs for a stable row identity.
///
/// A `WBEM_FLAG_NONSYSTEM_ONLY` enumeration never yields a `__`-prefixed
/// property, but the query object is marshalled into this process *by value* on
/// both transports, so each of these is a free `Get` by name and costs no round
/// trip. `__RELPATH` is the identity a diff keys on when a class carries no key
/// property of its own; without it a comparison is reduced to whole-row
/// equality, which is exactly what the compare feature exists to avoid.
const SYSTEM_IDENTITY_PROPS: [&str; 3] = ["__PATH", "__RELPATH", "__CLASS"];

/// `Get` one property off an in-process object by name.
///
/// This reaches `__`-prefixed system properties that enumeration hides, because
/// `Get` names the property directly rather than walking the non-system view.
/// Returns `None` only when the `Get` HRESULT is an error — a present-but-null
/// property comes back as `Some(Variant::Null)`, which renders empty but keeps
/// the column.
unsafe fn get_named(obj: &IWbemClassObject, name: &str) -> Option<Variant> {
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let mut val = VARIANT::default();
    obj.Get(PCWSTR(wide.as_ptr()), 0, &mut val, None, None)
        .ok()?;
    Variant::from_variant(&val).ok()
}

/// Read every non-system property of `obj` into a name -> value map.
///
/// Shared with the chunked local query path in [`crate::enumerate`] so both
/// transports flatten an object the same way.
///
/// When `include_system` is set, the [`SYSTEM_IDENTITY_PROPS`] are additionally
/// fetched by name and folded in — the row-identity columns a diff needs. This
/// is a `Get` per property on the object already in hand, not a re-enumeration,
/// because the enumeration flags cannot be relaxed to surface them without also
/// dragging in every other system property.
///
/// # Safety
///
/// `obj` must be a live `IWbemClassObject` with no enumeration already open on
/// it — `BeginEnumeration` fails on a second concurrent walk.
pub(crate) unsafe fn object_to_map(
    obj: &IWbemClassObject,
    include_system: bool,
) -> anyhow::Result<HashMap<String, Variant>> {
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
        let raw = Variant::from_variant(&val).unwrap_or(Variant::Null);
        // WMI hands several CIM types over in a different VARIANT type than
        // they are declared as -- a `uint64` arrives as `VT_BSTR`, a null array
        // as `VT_NULL`. The `wmi` crate normalizes with the CIM type it reads
        // alongside the value, and so must we, or the same property would
        // render differently depending on which path fetched it. The
        // conversion is fallible (a provider may report a type its value
        // cannot hold), and a raw value beats no value.
        let value = raw
            .clone()
            .convert_into_cim_type(CIMTYPE_ENUMERATION(ctype))
            .unwrap_or(raw);
        map.insert(name.to_string(), value);
    }
    obj.EndEnumeration()?;
    if include_system {
        for name in SYSTEM_IDENTITY_PROPS {
            if let Some(v) = get_named(obj, name) {
                map.insert(name.to_string(), v);
            }
        }
    }
    Ok(map)
}

impl RemoteConn {
    /// Connect to `\\host\namespace` with alternate credentials at `imp`.
    ///
    /// `imp` reaches WMI because this function sets the proxy blanket by hand.
    /// Note that every proxy *derived* from this one is re-blanketed too
    /// ([`RemoteConn::blanket`]), which the SSO transport does not do — there,
    /// the level applies to the service proxy and enumerators fall back to the
    /// process-wide default. That difference is invisible in practice, because
    /// `ExecQuery` itself is issued on the service proxy and is refused first;
    /// see [`crate::host::Impersonation`] for the measured behaviour.
    pub fn connect(
        host: &str,
        namespace: &str,
        cred: &Credential,
        imp: Impersonation,
    ) -> anyhow::Result<RemoteConn> {
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
            set_blanket((&svc).into(), ident_ptr, imp)?;
            Ok(RemoteConn { svc, auth, imp })
        }
    }

    /// The impersonation level this connection was opened at.
    pub fn impersonation(&self) -> Impersonation {
        self.imp
    }

    /// The raw service proxy, for the calls that are not enumerations
    /// (`ExecMethod`). Every proxy *derived* from it must be re-blanketed by
    /// [`RemoteConn::blanket`]; objects are marshalled by value and need not.
    pub(crate) fn services(&self) -> &IWbemServices {
        &self.svc
    }

    /// Push this connection's credentials onto a freshly returned proxy.
    ///
    /// Enumerators come back as *separate* proxies and do not inherit the
    /// service's blanket, so skipping this turns every read into Access Denied.
    fn blanket(&self, proxy: &windows::core::IUnknown) -> anyhow::Result<()> {
        let ident_ptr = &self.auth.ident as *const COAUTHIDENTITY as *const c_void;
        unsafe { set_blanket(proxy, ident_ptr, self.imp) }
    }

    /// Start a WQL enumeration and hand back the raw enumerator, so callers
    /// that need to pace themselves (see [`crate::enumerate::drain`]) can.
    pub(crate) fn exec_enum(&self, wql: &str) -> anyhow::Result<IEnumWbemClassObject> {
        unsafe {
            let en: IEnumWbemClassObject = self.svc.ExecQuery(
                &windows::core::BSTR::from("WQL"),
                &windows::core::BSTR::from(wql),
                WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
                None::<&IWbemContext>,
            )?;
            self.blanket((&en).into())?;
            Ok(en)
        }
    }

    /// Enumerate class definitions — the credentialed twin of
    /// [`crate::enumerate::DirectConn::class_enum`].
    pub(crate) fn class_enum(
        &self,
        superclass: Option<&str>,
        deep: bool,
    ) -> anyhow::Result<IEnumWbemClassObject> {
        let root = match superclass {
            Some(s) => windows::core::BSTR::from(s),
            None => windows::core::BSTR::new(),
        };
        unsafe {
            let en = self.svc.CreateClassEnum(
                &root,
                WBEM_GENERIC_FLAG_TYPE(ENUM_FLAGS | depth(deep)),
                None::<&IWbemContext>,
            )?;
            self.blanket((&en).into())?;
            Ok(en)
        }
    }

    /// Enumerate the instances of one class.
    pub(crate) fn instance_enum(
        &self,
        class: &str,
        deep: bool,
    ) -> anyhow::Result<IEnumWbemClassObject> {
        unsafe {
            let en = self.svc.CreateInstanceEnum(
                &windows::core::BSTR::from(class),
                WBEM_GENERIC_FLAG_TYPE(ENUM_FLAGS | depth(deep)),
                None::<&IWbemContext>,
            )?;
            self.blanket((&en).into())?;
            Ok(en)
        }
    }

    /// Fetch one class definition or instance by path.
    ///
    /// No blanket call: WMI objects are custom-marshalled *by value*, so what
    /// comes back is a local object rather than a proxy to a remote one.
    pub(crate) fn get_object(&self, path: &str) -> anyhow::Result<IWbemClassObject> {
        unsafe {
            let mut obj: Option<IWbemClassObject> = None;
            self.svc.GetObject(
                &windows::core::BSTR::from(path),
                WBEM_GENERIC_FLAG_TYPE(0),
                None::<&IWbemContext>,
                Some(&mut obj),
                None,
            )?;
            obj.ok_or_else(|| anyhow::anyhow!("WMI returned no object for {path}"))
        }
    }

    /// Run a query and hand back the **objects**, not flattened maps.
    ///
    /// [`exec_maps`](RemoteConn::exec_maps) reads each object with
    /// `WBEM_FLAG_NONSYSTEM_ONLY` and throws the object away, which discards
    /// every `__`-prefixed system property — `__CLASS` above all. A subscription
    /// scan needs exactly that: an `__EventConsumer` row is uninterpretable
    /// without the concrete consumer class it is an instance of. Keeping the
    /// object lets the caller read system properties by name afterwards.
    pub fn exec_objects(
        &self,
        wql: &str,
        max_rows: Option<usize>,
        deadline: Option<Duration>,
        cancel: &CancelToken,
    ) -> anyhow::Result<(Vec<IWbemClassObject>, Completion)> {
        let en = self.exec_enum(wql)?;
        // Cloning an `IWbemClassObject` is an AddRef on an object this process
        // already owns outright, not a round trip.
        crate::enumerate::drain(&en, max_rows, deadline, cancel, |o| Ok(o.clone()))
    }

    /// Run a query, returning each row as a property map.
    pub fn exec_maps(&self, wql: &str) -> anyhow::Result<Vec<HashMap<String, Variant>>> {
        let en = self.exec_enum(wql)?;
        // Through `drain`, not a hand-rolled `Next(WBEM_INFINITE, ..)` loop:
        // that loop was the one place left in this crate where a slow provider
        // could park the worker thread with nothing able to interrupt it.
        let (rows, _) =
            crate::enumerate::drain(&en, None, None, &CancelToken::never(), |o| unsafe {
                object_to_map(o, false)
            })?;
        Ok(rows)
    }
}
