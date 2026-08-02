//! Host identity, per-host facts, and the impersonation level a connection asks
//! for.
//!
//! A [`HostRef`] is the *identity* of a WMI target — which machine, and as
//! whom — and deliberately not a connection. It is the key of
//! [`crate::registry::WorkerRegistry`]'s map, so it carries no password: a
//! `Hash`/`Eq` over a secret puts that secret in a hash table, in a `Debug`
//! line, and eventually in a log.

use crate::remote::Credential;

use windows::Win32::System::Com::{
    RPC_C_IMP_LEVEL, RPC_C_IMP_LEVEL_DELEGATE, RPC_C_IMP_LEVEL_IDENTIFY,
    RPC_C_IMP_LEVEL_IMPERSONATE,
};

/// Which machine a request is aimed at, and under which principal.
///
/// `Sso` and `Alt` are separate identities for the same host on purpose. They
/// see different things — a connection as `DOMAIN\svc_backup` reads what that
/// account can read — so folding them onto one key would let a request answer
/// with another principal's view of the machine, which is the same class of bug
/// as the one task 5.6 fixes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum HostRef {
    /// This machine, as the current user. No `\\host` prefix anywhere.
    Local,
    /// A remote host as the current user (Kerberos/NTLM single sign-on).
    Sso { host: String },
    /// A remote host as `user`, authenticated with alternate credentials.
    ///
    /// `user` is domain-qualified where a domain was supplied, so
    /// `CORP\admin` and `admin` are distinct targets — they usually are.
    Alt { host: String, user: String },
}

impl HostRef {
    /// The host name, or `None` for the local machine.
    pub fn host(&self) -> Option<&str> {
        match self {
            HostRef::Local => None,
            HostRef::Sso { host } | HostRef::Alt { host, .. } => Some(host),
        }
    }

    /// Does reaching this target require the alternate-credential transport?
    pub fn is_alt_cred(&self) -> bool {
        matches!(self, HostRef::Alt { .. })
    }

    /// The identity a `(host, credential)` pair resolves to.
    ///
    /// The credential is read for its user name only; the password stays in the
    /// caller's hands. A credential without a host is not an alternate-cred
    /// target at all: WMI refuses credentialed *local* connections, so there is
    /// nothing such a pair could name.
    pub fn of(host: Option<&str>, cred: Option<&Credential>) -> HostRef {
        match (host, cred) {
            (None, _) => HostRef::Local,
            (Some(h), None) => HostRef::Sso {
                host: h.to_string(),
            },
            (Some(h), Some(c)) => HostRef::Alt {
                host: h.to_string(),
                user: c.qualified_user(),
            },
        }
    }

    /// A short label for a status bar: `this machine`, `\\HOST`, `\\HOST as U`.
    pub fn label(&self) -> String {
        match self {
            HostRef::Local => "this machine".to_string(),
            HostRef::Sso { host } => format!(r"\\{host}"),
            HostRef::Alt { host, user } => format!(r"\\{host} as {user}"),
        }
    }
}

/// Impersonation level requested on the DCOM proxy.
///
/// This is what the WMI service is allowed to do with the caller's identity,
/// and it is a real security decision, not a formality: `Delegate` hands the
/// target machine credentials it can spend on a *third* machine.
///
/// It applies wherever this crate hand-calls `CoSetProxyBlanket`, which is
/// **both** transports — [`crate::remote::RemoteConn`] and
/// [`crate::enumerate::DirectConn`]. The plan expected only the first, on the
/// grounds that the SSO path went through the `wmi` crate; it has not since
/// Phase 3. Measured on the local machine at `Identify`, via
/// `examples/impersonation.rs`:
///
/// | request | Identify | Impersonate |
/// |---|---|---|
/// | connect probe (`Win32_OperatingSystem`) | `WBEM_E_ACCESS_DENIED` | ok |
/// | `SELECT … FROM Win32_ComputerSystem` | `WBEM_E_ACCESS_DENIED` | 1 row |
/// | reflect `Win32_Process` schema | **ok, 45 properties** | ok |
/// | `StdRegProv.EnumKey` | `WBEM_E_PROVIDER_NOT_CAPABLE` | `ReturnValue=0` |
///
/// So the level reaches WMI, and what it gates is *providers*: a class
/// definition comes out of the repository and needs no impersonation, while
/// anything a provider has to answer is refused. `Delegate` behaved exactly
/// like `Impersonate` here, which is expected — locally there is no second hop
/// for it to differ on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, PartialOrd, Ord)]
pub enum Impersonation {
    /// The server may learn who the caller is and check ACLs, but may not act
    /// as them. Not enough for most of WMI — see the table above.
    Identify,
    /// The server may act as the caller on its own machine. WMI's usual level,
    /// and what the `wmi` crate connects at.
    #[default]
    Impersonate,
    /// The server may act as the caller on *other* machines too. Required for
    /// a double hop, and dangerous for exactly that reason.
    Delegate,
}

impl Impersonation {
    /// The `RPC_C_IMP_LEVEL_*` constant this maps to.
    pub(crate) fn level(self) -> RPC_C_IMP_LEVEL {
        match self {
            Impersonation::Identify => RPC_C_IMP_LEVEL_IDENTIFY,
            Impersonation::Impersonate => RPC_C_IMP_LEVEL_IMPERSONATE,
            Impersonation::Delegate => RPC_C_IMP_LEVEL_DELEGATE,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Impersonation::Identify => "Identify",
            Impersonation::Impersonate => "Impersonate",
            Impersonation::Delegate => "Delegate",
        }
    }

    /// Every level, in ascending order of what it gives away — for a UI that
    /// offers the choice.
    pub fn all() -> [Impersonation; 3] {
        [
            Impersonation::Identify,
            Impersonation::Impersonate,
            Impersonation::Delegate,
        ]
    }
}

/// What a host says about itself, read once at connect time.
///
/// Deliberately all strings. Every field is displayed rather than computed
/// with, and `LastBootUpTime` is a `CIM_DATETIME` whose offset is the *target's*
/// timezone — parsing it into a local `SystemTime` here would silently shift it
/// by the difference between two machines' clocks.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct HostInfo {
    /// `Win32_OperatingSystem.Caption`, e.g. `Microsoft Windows 11 Pro`.
    pub caption: String,
    /// `Win32_OperatingSystem.Version`, e.g. `10.0.26200`.
    pub version: String,
    /// `Win32_OperatingSystem.BuildNumber`, e.g. `26200`.
    pub build_number: String,
    /// `Win32_OperatingSystem.OSArchitecture`, e.g. `64-bit`.
    pub architecture: String,
    /// `Win32_OperatingSystem.LastBootUpTime`, raw `CIM_DATETIME`.
    pub last_boot: String,
    /// `Win32_ComputerSystemProduct.UUID` — the SMBIOS machine UUID, which is
    /// what tells two identically named hosts apart.
    pub uuid: String,
}

impl HostInfo {
    /// Did the probe learn anything at all? A host that answered the connect
    /// but not the probe is a real state (a locked-down namespace), and a
    /// caller must be able to tell it from "not asked yet".
    pub fn is_empty(&self) -> bool {
        self == &HostInfo::default()
    }

    /// `Microsoft Windows 11 Pro · 10.0.26200 · 64-bit`, skipping blanks.
    pub fn summary(&self) -> String {
        [
            self.caption.as_str(),
            self.version.as_str(),
            self.architecture.as_str(),
        ]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" \u{b7} ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cred(user: &str, domain: Option<&str>) -> Credential {
        Credential {
            user: user.to_string(),
            password: "irrelevant".into(),
            domain: domain.map(str::to_string),
        }
    }

    #[test]
    fn a_credential_without_a_host_is_still_the_local_machine() {
        // WMI rejects credentialed local connections, so there is no identity
        // for this pair to name -- and pretending there is would spawn a worker
        // that can never connect.
        assert_eq!(
            HostRef::of(None, Some(&cred("admin", None))),
            HostRef::Local
        );
    }

    #[test]
    fn sso_and_alt_cred_on_one_host_are_different_targets() {
        let sso = HostRef::of(Some("SRV1"), None);
        let alt = HostRef::of(Some("SRV1"), Some(&cred("admin", Some("CORP"))));
        assert_ne!(sso, alt);
        assert!(!sso.is_alt_cred());
        assert!(alt.is_alt_cred());
        assert_eq!(sso.host(), Some("SRV1"));
        assert_eq!(alt.host(), Some("SRV1"));
    }

    #[test]
    fn the_domain_is_part_of_the_identity() {
        let corp = HostRef::of(Some("SRV1"), Some(&cred("admin", Some("CORP"))));
        let local = HostRef::of(Some("SRV1"), Some(&cred("admin", None)));
        assert_ne!(corp, local);
        assert_eq!(corp.label(), r"\\SRV1 as CORP\admin");
        assert_eq!(local.label(), r"\\SRV1 as admin");
    }

    /// The password must never reach the key -- two connections that differ
    /// only by a typo'd password are the same target, and the wrong one fails
    /// loudly at connect rather than quietly occupying a second slot.
    #[test]
    fn the_password_is_not_part_of_the_identity() {
        let a = HostRef::of(Some("SRV1"), Some(&cred("admin", Some("CORP"))));
        let mut other = cred("admin", Some("CORP"));
        other.password = "something else entirely".into();
        assert_eq!(a, HostRef::of(Some("SRV1"), Some(&other)));
        assert!(!format!("{a:?}").contains("irrelevant"));
    }

    #[test]
    fn host_info_tells_unprobed_apart_from_probed() {
        assert!(HostInfo::default().is_empty());
        let info = HostInfo {
            caption: "Microsoft Windows 11 Pro".into(),
            version: "10.0.26200".into(),
            architecture: "64-bit".into(),
            ..Default::default()
        };
        assert!(!info.is_empty());
        assert_eq!(
            info.summary(),
            "Microsoft Windows 11 Pro \u{b7} 10.0.26200 \u{b7} 64-bit"
        );
        // A partial probe renders without stray separators.
        let partial = HostInfo {
            caption: "Windows".into(),
            ..Default::default()
        };
        assert_eq!(partial.summary(), "Windows");
    }

    #[test]
    fn impersonation_levels_map_to_distinct_rpc_constants() {
        let levels: Vec<u32> = Impersonation::all().iter().map(|i| i.level().0).collect();
        assert_eq!(levels, vec![2, 3, 4]); // IDENTIFY, IMPERSONATE, DELEGATE
        assert_eq!(Impersonation::default(), Impersonation::Impersonate);
    }
}
