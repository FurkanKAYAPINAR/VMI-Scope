//! Live network-connection model.
//!
//! Connections are sourced from WMI itself: `MSFT_NetTCPConnection` and
//! `MSFT_NetUDPEndpoint` in `root\StandardCimv2`, joined to process names via
//! `Win32_Process`. See [`crate::worker`] for the query implementation.

/// Transport protocol of an endpoint/connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone)]
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

impl Connection {
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
#[derive(Debug, Clone, Default)]
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
