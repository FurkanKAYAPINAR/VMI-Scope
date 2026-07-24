//! WMI provider → host-process mapping.
//!
//! Every WMI class is served by a *provider* (a COM component) that runs inside
//! a host process (`wmiprvse.exe` instances, or in-process). `Msft_Providers`
//! in `root\cimv2` exposes which provider is hosted by which PID — useful for
//! troubleshooting a hung/leaking `wmiprvse.exe` or spotting an odd provider.

/// One provider and the process currently hosting it.
#[derive(Debug, Clone, Default)]
pub struct ProviderInfo {
    pub provider: String,
    pub namespace: String,
    pub host_pid: u32,
    pub host_process: String,
    pub hosting_group: String,
}
