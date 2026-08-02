//! WMI providers, the processes hosting them, and the quota that bounds those.
//!
//! Every WMI class is served by a *provider* (a COM component) that runs inside
//! a host process (`wmiprvse.exe` instances, or in-process in the WMI service).
//! `Msft_Providers` in `root\CIMV2` exposes which provider is hosted by which
//! PID — useful for troubleshooting a hung/leaking `wmiprvse.exe` or spotting
//! an odd provider.
//!
//! The raw numbers alone answer very little. 58 MB in a host process is either
//! nothing or the last reading before WMI kills it, and which one depends on a
//! ceiling that lives somewhere else entirely: `__ProviderHostQuotaConfiguration`
//! in the `root` namespace. So usage and ceiling are modelled together here —
//! [`HostStats`] against [`HostQuota`] — because a consumer handed only the
//! first would have to invent the second.

use serde::{Deserialize, Serialize};

/// One provider and the process currently hosting it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderInfo {
    pub provider: String,
    pub namespace: String,
    pub host_pid: u32,
    pub host_process: String,
    pub hosting_group: String,
    /// `__Win32Provider.HostingModel` for this provider, from the registration
    /// in its own namespace — `NetworkServiceHost`, `LocalSystemHost`,
    /// `Decoupled:NonCOM`, `WmiCore`, …
    ///
    /// **Not** a property of `Msft_Providers`, though `docs/REDESIGN.md` §5.11
    /// says it is: measured on Windows 11 26200, `SELECT HostingModel FROM
    /// Msft_Providers` is rejected with *Invalid query*, and the class declares
    /// no such property. The string exists, one class over, and is worth the
    /// extra lookup — it is what explains *why* a provider sits in the host it
    /// sits in.
    ///
    /// Empty when the provider has no `__Win32Provider` registration in that
    /// namespace, which is a real state rather than a failure: measured here,
    /// `DelegatorProvider` in `root\Microsoft\Windows\Storage\PT` has none.
    #[serde(default)]
    pub hosting_model: String,
    /// `Msft_Providers.HostingSpecification`, verbatim.
    ///
    /// A `uint32` whose meaning Microsoft does not document, so it is passed
    /// through undecoded rather than translated by guesswork. Observed on this
    /// machine it tracks [`ProviderInfo::hosting_model`] one-for-one
    /// (1=`WmiCore`, 5=`LocalSystemHost`, 10=`Decoupled:NonCOM`,
    /// 12=`NetworkServiceHost`, 13=`LocalServiceHost`), but that is eight rows
    /// on one build, which is an observation and not a mapping table.
    #[serde(default)]
    pub hosting_specification: u32,
    /// `Msft_Providers.User` — the account a decoupled provider registered
    /// itself under. Empty for the ordinary hosted case.
    #[serde(default)]
    pub user: String,
}

/// Live load of one provider host process, from
/// `Win32_PerfFormattedData_PerfProc_Process`.
///
/// Keyed by `pid`, never by name. The perf class names sibling hosts
/// `WmiPrvSE`, `WmiPrvSE#1`, `WmiPrvSE#2` … while `Win32_Process` calls all of
/// them `WmiPrvSE.exe`, so a name join either collapses every host into one row
/// or attributes one host's load to another. Measured on this machine: four
/// `WmiPrvSE` instances, one `Win32_Process.Name`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HostStats {
    /// `IDProcess` — the join key, and the only one.
    pub pid: u32,
    /// The perf counter's instance name (`WmiPrvSE#2`, `svchost#13`).
    ///
    /// Carried for display and because it is the thing that must *not* be
    /// joined on; showing it makes the distinction visible instead of folklore.
    /// The `#n` suffix is a reusable slot rather than an identity — measured
    /// here, `WmiPrvSE#3` was PID 43468, that host exited, and the same label
    /// came back on PID 37048 — so it must never be used as a key across
    /// samples either.
    pub instance: String,
    /// `PercentProcessorTime`, **summed over every logical processor**, so its
    /// range is `0..=100 × logical CPUs` and not `0..=100`.
    ///
    /// Measured, not assumed: on this 24-CPU machine the `_Total` instance
    /// reads 2414 and a single busy `find` reads 102. Rendering this as a
    /// machine-wide percentage without dividing by the CPU count would overstate
    /// a provider's CPU by 24×, so [`HostStats::cpu_of_machine`] is the way to
    /// display it.
    pub cpu_percent: u64,
    /// `PrivateBytes` — private committed bytes. The counter compared against
    /// [`HostQuota::memory_per_host`]; see that field for what is and is not
    /// verified about the comparison.
    pub private_bytes: u64,
    /// `WorkingSetPrivate` — private *resident* bytes. Always ≤
    /// [`HostStats::private_bytes`]; kept because it is the number Task Manager
    /// shows and a user comparing the two would otherwise think one is wrong.
    pub working_set_private: u64,
    pub handle_count: u32,
    pub thread_count: u32,
}

impl HostStats {
    /// CPU as a share of the whole machine, given its logical processor count.
    ///
    /// `None` when the CPU count is unknown (0), because the alternative —
    /// showing the raw counter and calling it a percentage — is off by the
    /// number of cores.
    pub fn cpu_of_machine(&self, logical_cpus: u32) -> Option<f32> {
        if logical_cpus == 0 {
            return None;
        }
        Some(self.cpu_percent as f32 / logical_cpus as f32)
    }
}

/// Which ceiling a host is closest to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuotaKind {
    Memory,
    Handles,
    Threads,
}

impl QuotaKind {
    pub fn as_str(self) -> &'static str {
        match self {
            QuotaKind::Memory => "memory",
            QuotaKind::Handles => "handles",
            QuotaKind::Threads => "threads",
        }
    }
}

/// The ceilings WMI enforces on provider hosts, from
/// `__ProviderHostQuotaConfiguration` in the `root` namespace.
///
/// This is the point of the whole view. A host at 300 MB is unremarkable; a
/// host at 300 MB of a 512 MB ceiling is a provider that will be terminated
/// shortly, and the raw number cannot tell those apart.
///
/// Measured on this machine (Windows 11 26200, defaults): `MemoryPerHost`
/// 536870912, `MemoryAllHosts` 1073741824, `HandlesPerHost` 4096,
/// `ThreadsPerHost` 256, `ProcessLimitAllHosts` 32.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostQuota {
    /// Memory ceiling for a single host process, in bytes.
    ///
    /// **Which counter WMI measures against this is unverified.** The value is
    /// compared here against `PrivateBytes` (private commit), the closest
    /// available reading — measured equal to `Win32_Process.PrivatePageCount`
    /// on the same PID at the same moment. Confirming it would mean lowering
    /// the quota until a host is actually killed and reading the counter WMI
    /// logged, which is a machine-configuration change this has not made.
    pub memory_per_host: u64,
    /// Handle ceiling for a single host process.
    pub handles_per_host: u32,
    /// Thread ceiling for a single host process.
    pub threads_per_host: u32,
    /// Memory ceiling across *all* host processes combined, in bytes.
    pub memory_all_hosts: u64,
    /// Maximum number of provider host processes.
    pub process_limit_all_hosts: u32,
}

impl HostQuota {
    /// Fraction of the per-host memory ceiling in use (`1.0` = at the ceiling).
    ///
    /// `None` when no ceiling is configured. A zero ceiling means *unlimited*
    /// in WMI's configuration, so dividing by it would be both a panic and a
    /// lie; a caller must render "no quota", not "0 %".
    pub fn memory_fraction(&self, s: &HostStats) -> Option<f32> {
        fraction(s.private_bytes, self.memory_per_host)
    }

    /// Fraction of the per-host handle ceiling in use.
    pub fn handle_fraction(&self, s: &HostStats) -> Option<f32> {
        fraction(s.handle_count as u64, self.handles_per_host as u64)
    }

    /// Fraction of the per-host thread ceiling in use.
    pub fn thread_fraction(&self, s: &HostStats) -> Option<f32> {
        fraction(s.thread_count as u64, self.threads_per_host as u64)
    }

    /// The ceiling this host is nearest to, and how near.
    ///
    /// The *maximum* of the three, not an average or a sum: a host is killed by
    /// whichever quota it hits first, so a process at 4 % memory and 98 %
    /// handles is in trouble, and any measure that blends them says it is fine.
    pub fn pressure(&self, s: &HostStats) -> Option<(QuotaKind, f32)> {
        [
            (QuotaKind::Memory, self.memory_fraction(s)),
            (QuotaKind::Handles, self.handle_fraction(s)),
            (QuotaKind::Threads, self.thread_fraction(s)),
        ]
        .into_iter()
        .filter_map(|(k, f)| f.map(|f| (k, f)))
        .max_by(|a, b| a.1.total_cmp(&b.1))
    }
}

/// `used / ceiling`, or `None` when there is no ceiling to divide by.
fn fraction(used: u64, ceiling: u64) -> Option<f32> {
    if ceiling == 0 {
        return None;
    }
    Some(used as f32 / ceiling as f32)
}

/// Everything about the *hosts* behind a provider list: their live load, the
/// ceilings they run against, and what could not be read.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderHosts {
    /// One entry per distinct host PID in the provider list, load included.
    pub stats: Vec<HostStats>,
    /// The machine's provider-host quota, or `None` when it could not be read.
    ///
    /// `None` and "no quota configured" are deliberately different: the first
    /// is an absent answer, the second is a [`HostQuota`] whose fields are 0.
    pub quota: Option<HostQuota>,
    /// Logical processors on the target, for [`HostStats::cpu_of_machine`].
    /// `0` when unknown.
    pub logical_cpus: u32,
    /// What this scan could not read, and why.
    ///
    /// Same reasoning as [`crate::events::SubscriptionReport::unreadable`]: a
    /// provider row with no stats means either "that host has no counters" or
    /// "the perf query failed", and a view that renders both as a blank cell
    /// has quietly turned a failure into a measurement.
    #[serde(default)]
    pub unreadable: Vec<String>,
}

impl ProviderHosts {
    /// Load of the host with `pid`.
    ///
    /// By PID. See [`HostStats`] for why there is no by-name variant.
    pub fn stats_for(&self, pid: u32) -> Option<&HostStats> {
        self.stats.iter().find(|h| h.pid == pid)
    }

    /// The ceiling `pid` is nearest to, and how near — `None` when either the
    /// host or the quota is unknown.
    pub fn pressure_for(&self, pid: u32) -> Option<(QuotaKind, f32)> {
        self.quota?.pressure(self.stats_for(pid)?)
    }

    /// Did every part of this scan answer?
    pub fn is_complete(&self) -> bool {
        self.unreadable.is_empty()
    }
}

/// The distinct, real host PIDs in a provider list, ascending.
///
/// PID 0 is dropped: `Msft_Providers` reports it for a provider that is
/// registered but not currently loaded, and the perf class's PID 0 is `Idle`
/// plus `_Total`. Joining them would give every unloaded provider the CPU of
/// the idle process.
pub fn host_pids(providers: &[ProviderInfo]) -> Vec<u32> {
    let mut pids: Vec<u32> = providers
        .iter()
        .map(|p| p.host_pid)
        .filter(|&p| p != 0)
        .collect();
    pids.sort_unstable();
    pids.dedup();
    pids
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prov(name: &str, ns: &str, pid: u32) -> ProviderInfo {
        ProviderInfo {
            provider: name.into(),
            namespace: ns.into(),
            host_pid: pid,
            host_process: "WmiPrvSE.exe".into(),
            ..Default::default()
        }
    }

    fn host(pid: u32, instance: &str, handles: u32, threads: u32, private: u64) -> HostStats {
        HostStats {
            pid,
            instance: instance.into(),
            cpu_percent: 0,
            private_bytes: private,
            working_set_private: private / 2,
            handle_count: handles,
            thread_count: threads,
        }
    }

    /// The bug this whole join exists to avoid, written down as a test. Two
    /// live hosts on this machine really are called `WmiPrvSE` and `WmiPrvSE#1`
    /// in the perf class and `WmiPrvSE.exe` in `Win32_Process`; a lookup keyed
    /// on either name form reaches the wrong one.
    #[test]
    fn sibling_hosts_are_told_apart_by_pid_not_name() {
        let hosts = ProviderHosts {
            stats: vec![
                host(10772, "WmiPrvSE", 593, 10, 42_524_672),
                host(2396, "WmiPrvSE#1", 177, 5, 4_337_664),
            ],
            ..Default::default()
        };
        assert_eq!(hosts.stats_for(10772).unwrap().handle_count, 593);
        assert_eq!(hosts.stats_for(2396).unwrap().handle_count, 177);
        assert!(hosts.stats_for(99999).is_none());

        // The name the two share in `Win32_Process` cannot separate them, and
        // the names the perf class gives them are not the process name at all.
        let names: Vec<&str> = hosts.stats.iter().map(|h| h.instance.as_str()).collect();
        assert_eq!(names, vec!["WmiPrvSE", "WmiPrvSE#1"]);
        assert!(names.iter().all(|n| *n != "WmiPrvSE.exe"));
    }

    #[test]
    fn host_pids_are_distinct_and_drop_the_unloaded() {
        let providers = vec![
            prov("CIMWin32", "root\\CIMV2", 10772),
            prov("SCM Event Provider", "root\\CIMV2", 2788),
            // Two providers, one host: the list must not query it twice.
            prov("Msft_ProviderSubSystem", "root\\CIMV2", 2788),
            // Registered but not loaded.
            prov("MS_NT_EVENTLOG_PROVIDER", "root\\CIMV2", 0),
        ];
        assert_eq!(host_pids(&providers), vec![2788, 10772]);
        assert!(host_pids(&[]).is_empty());
    }

    /// The measured defaults from this machine, and the reading that makes them
    /// mean something.
    #[test]
    fn usage_reads_against_the_real_ceilings() {
        let quota = HostQuota {
            memory_per_host: 536_870_912,
            handles_per_host: 4096,
            threads_per_host: 256,
            memory_all_hosts: 1_073_741_824,
            process_limit_all_hosts: 32,
        };
        let s = host(33520, "WmiPrvSE#2", 463, 11, 58_957_824);
        // 58,957,824 / 536,870,912 = 0.1098…
        let mem = quota.memory_fraction(&s).unwrap();
        assert!((mem - 0.1098).abs() < 0.001, "memory fraction was {mem}");
        assert!((quota.handle_fraction(&s).unwrap() - 0.113).abs() < 0.001);
        assert!((quota.thread_fraction(&s).unwrap() - 0.043).abs() < 0.001);
    }

    /// A host is killed by whichever ceiling it reaches first, so the reported
    /// pressure has to be the worst of the three — not their average, which
    /// would read this leak as 34 % and comfortable.
    #[test]
    fn pressure_reports_the_nearest_ceiling_not_the_blend() {
        let quota = HostQuota {
            memory_per_host: 536_870_912,
            handles_per_host: 4096,
            threads_per_host: 256,
            ..Default::default()
        };
        let leaking_handles = host(4242, "WmiPrvSE#9", 4000, 12, 21_000_000);
        let (kind, frac) = quota.pressure(&leaking_handles).unwrap();
        assert_eq!(kind, QuotaKind::Handles);
        assert!(frac > 0.97, "handle pressure was {frac}");

        let leaking_memory = host(4243, "WmiPrvSE#10", 50, 12, 500_000_000);
        assert_eq!(
            quota.pressure(&leaking_memory).unwrap().0,
            QuotaKind::Memory
        );
    }

    /// Zero means "no ceiling" in WMI's own configuration, so it must not
    /// render as a full bar — or as any bar.
    #[test]
    fn an_unset_ceiling_is_absent_rather_than_zero() {
        let none = HostQuota::default();
        let s = host(1, "WmiPrvSE", 100, 10, 1_000_000);
        assert!(none.memory_fraction(&s).is_none());
        assert!(none.handle_fraction(&s).is_none());
        assert!(none.thread_fraction(&s).is_none());
        assert!(none.pressure(&s).is_none());

        // A partial configuration still answers for the parts it sets.
        let handles_only = HostQuota {
            handles_per_host: 4096,
            ..Default::default()
        };
        assert!(handles_only.memory_fraction(&s).is_none());
        assert_eq!(handles_only.pressure(&s).unwrap().0, QuotaKind::Handles);
    }

    /// The counter is per-machine-summed, so 24 cores at 5 is 0.2 % of the box.
    #[test]
    fn cpu_is_divided_by_the_cpu_count_or_withheld() {
        let mut s = host(43468, "WmiPrvSE#3", 264, 9, 7_962_624);
        s.cpu_percent = 5;
        assert!((s.cpu_of_machine(24).unwrap() - 0.2083).abs() < 0.001);
        // Unknown CPU count: withhold rather than overstate by 24x.
        assert!(s.cpu_of_machine(0).is_none());
    }

    /// No quota read at all is not the same as a quota of zero, and neither is
    /// the same as a host we have no counters for.
    #[test]
    fn absent_answers_stay_absent() {
        let hosts = ProviderHosts {
            stats: vec![host(10772, "WmiPrvSE", 593, 10, 42_524_672)],
            quota: None,
            logical_cpus: 24,
            unreadable: vec!["root: __ProviderHostQuotaConfiguration: denied".into()],
        };
        assert!(hosts.pressure_for(10772).is_none());
        assert!(!hosts.is_complete());

        let quiet = ProviderHosts {
            quota: Some(HostQuota {
                handles_per_host: 4096,
                ..Default::default()
            }),
            ..Default::default()
        };
        // Quota known, host unknown -- still no answer, and not a zero.
        assert!(quiet.pressure_for(10772).is_none());
        assert!(quiet.is_complete());
    }
}
