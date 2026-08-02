//! Live network-connection model.
//!
//! Connections are sourced from WMI itself: `MSFT_NetTCPConnection` and
//! `MSFT_NetUDPEndpoint` in `root\StandardCimv2`, joined to process names via
//! `Win32_Process`. See [`crate::worker`] for the query implementation.

use serde::Serialize;

/// Transport protocol of an endpoint/connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Protocol {
    Tcp,
    Udp,
}

impl Protocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Protocol::Tcp => "TCP",
            Protocol::Udp => "UDP",
        }
    }
}

/// A single connection (TCP) or endpoint (UDP).
#[derive(Debug, Clone, Serialize)]
pub struct Connection {
    pub proto: Protocol,
    pub local_addr: String,
    pub local_port: u16,
    pub remote_addr: String,
    pub remote_port: u16,
    /// TCP state name (e.g. `Established`); empty for UDP.
    pub state: String,
    pub pid: u32,
    pub process: String,
}

/// Is `addr` a public (routable) IP — i.e. not loopback/private/link-local?
pub fn is_public_ip(addr: &str) -> bool {
    let a = addr.trim().trim_start_matches('[').trim_end_matches(']');
    let a = a.split('%').next().unwrap_or(a); // strip IPv6 scope id
    if let Ok(v4) = a.parse::<std::net::Ipv4Addr>() {
        return !(v4.is_loopback()
            || v4.is_private()
            || v4.is_link_local()
            || v4.is_unspecified()
            || v4.is_multicast()
            || v4.is_broadcast()
            || v4.octets()[0] == 0);
    }
    if let Ok(v6) = a.parse::<std::net::Ipv6Addr>() {
        if v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() {
            return false;
        }
        let seg0 = v6.segments()[0];
        let link_local = (seg0 & 0xffc0) == 0xfe80;
        let unique_local = (seg0 & 0xfe00) == 0xfc00;
        return !(link_local || unique_local);
    }
    false
}

impl Connection {
    /// An established TCP connection to a public IP — the notable case
    /// (possible C2 / exfil) worth surfacing during a hunt.
    pub fn is_external(&self) -> bool {
        self.proto == Protocol::Tcp
            && self.state == "Established"
            && is_public_ip(&self.remote_addr)
    }

    /// Stable identity used to track a connection across snapshots (for fade).
    pub fn key(&self) -> String {
        format!(
            "{}|{}:{}|{}:{}|{}",
            self.proto.as_str(),
            self.local_addr,
            self.local_port,
            self.remote_addr,
            self.remote_port,
            self.pid,
        )
    }
}

/// One point-in-time reading of the whole connection table.
#[derive(Debug, Clone, Default, Serialize)]
pub struct NetworkSnapshot {
    pub connections: Vec<Connection>,
}

/// Map an `MSFT_NetTCPConnection.State` enum code to its name.
pub fn tcp_state_name(code: u32) -> &'static str {
    match code {
        1 => "Closed",
        2 => "Listen",
        3 => "SynSent",
        4 => "SynReceived",
        5 => "Established",
        6 => "FinWait1",
        7 => "FinWait2",
        8 => "CloseWait",
        9 => "Closing",
        10 => "LastAck",
        11 => "TimeWait",
        12 => "DeleteTCB",
        100 => "Bound",
        _ => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tcp_state_names() {
        assert_eq!(tcp_state_name(5), "Established");
        assert_eq!(tcp_state_name(2), "Listen");
        assert_eq!(tcp_state_name(100), "Bound");
        assert_eq!(tcp_state_name(999), "Unknown");
    }

    #[test]
    fn public_ip_classification() {
        assert!(is_public_ip("8.8.8.8"));
        assert!(is_public_ip("140.82.121.6"));
        assert!(!is_public_ip("127.0.0.1"));
        assert!(!is_public_ip("192.168.1.5"));
        assert!(!is_public_ip("10.0.0.1"));
        assert!(!is_public_ip("169.254.1.1"));
        assert!(!is_public_ip("::1"));
        assert!(!is_public_ip("fe80::1"));
        assert!(!is_public_ip("0.0.0.0"));
        assert!(is_public_ip("2606:4700:4700::1111"));
        assert!(!is_public_ip(""));
    }

    #[test]
    fn connection_key_is_stable_and_descriptive() {
        let c = Connection {
            proto: Protocol::Tcp,
            local_addr: "0.0.0.0".into(),
            local_port: 443,
            remote_addr: "1.2.3.4".into(),
            remote_port: 55000,
            state: "Established".into(),
            pid: 1234,
            process: "svc.exe".into(),
        };
        assert_eq!(c.key(), c.clone().key());
        assert!(c.key().contains("443"));
        assert!(c.key().contains("TCP"));
    }
}
