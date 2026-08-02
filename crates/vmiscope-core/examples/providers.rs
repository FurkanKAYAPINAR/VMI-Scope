//! Providers, their host processes, and how close those hosts are to the
//! quota that will kill them — the ground truth behind tasks 5.11–5.13.
//!
//! Prints, from the live machine:
//!  - every `Msft_Providers` row with the widened columns,
//!  - one line per *distinct host PID*, joined on `IDProcess`,
//!  - the `__ProviderHostQuotaConfiguration` ceilings and the usage against
//!    them,
//!  - the sibling-host check: what a name join would have done instead.
//!
//! Run with: `cargo run -p vmiscope-core --example providers`

use std::time::{Duration, Instant};

use vmiscope_core::{ProviderHosts, ProviderInfo, Request, Response, WmiWorker};

fn mb(bytes: u64) -> f64 {
    bytes as f64 / 1_048_576.0
}

fn main() {
    let worker = WmiWorker::spawn();
    worker.send(Request::ListProviders { id: 1 });

    let deadline = Instant::now() + Duration::from_secs(90);
    let mut answered = false;
    while !answered && Instant::now() < deadline {
        for resp in worker.poll() {
            match resp {
                Response::Providers {
                    providers,
                    hosts,
                    namespace,
                    elapsed_ms,
                    ..
                } => {
                    answered = true;
                    report(&namespace, elapsed_ms, &providers, &hosts);
                }
                Response::Error {
                    context, message, ..
                } => {
                    answered = true;
                    println!("FAILED [{context}] {message}");
                }
                other => println!("unexpected: {other:?}"),
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    if !answered {
        println!("no reply within the deadline");
    }

    cancellation_still_replies(&worker);
}

/// The provider list grew from one fixed query into a walk over every namespace
/// that registers a provider, so it became cancellable with task 5.11. What
/// that has to mean in practice: the caller still gets an answer, and it is not
/// a half-list dressed up as a whole one.
fn cancellation_still_replies(worker: &WmiWorker) {
    println!("\n== cancelled mid-flight ==");
    worker.send(Request::ListProviders { id: 2 });
    worker.cancel(2);

    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if let Some(resp) = worker.poll().into_iter().next() {
            match resp {
                Response::Providers {
                    providers, hosts, ..
                } => println!(
                    "  finished before the flag was read: {} providers, {} hosts",
                    providers.len(),
                    hosts.stats.len()
                ),
                Response::Error {
                    context, message, ..
                } => println!("  refused rather than truncated: [{context}] {message}"),
                other => println!("  unexpected: {other:?}"),
            }
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    println!("  NO REPLY -- a cancelled request stranded its caller");
}

fn report(namespace: &str, elapsed_ms: u64, providers: &[ProviderInfo], hosts: &ProviderHosts) {
    println!("== Msft_Providers in {namespace} ({elapsed_ms} ms for everything) ==");
    println!(
        "{:<34} {:<38} {:>7}  {:<14} {:<26} {:<20} {:>4}  user",
        "provider", "namespace", "pid", "process", "HostingGroup", "HostingModel", "spec"
    );
    for p in providers {
        println!(
            "{:<34} {:<38} {:>7}  {:<14} {:<26} {:<20} {:>4}  {}",
            p.provider,
            p.namespace,
            p.host_pid,
            p.host_process,
            p.hosting_group,
            p.hosting_model,
            p.hosting_specification,
            p.user
        );
    }

    println!("\n== host processes, joined on IDProcess ==");
    println!("  logical CPUs: {}", hosts.logical_cpus);
    println!(
        "{:>7}  {:<14} {:>7} {:>9}  {:>12} {:>12} {:>8} {:>8}",
        "pid", "perf instance", "cpu", "of machine", "private", "ws private", "handles", "threads"
    );
    for h in &hosts.stats {
        let share = match h.cpu_of_machine(hosts.logical_cpus) {
            Some(pct) => format!("{pct:.2} %"),
            None => "—".into(),
        };
        println!(
            "{:>7}  {:<14} {:>7} {:>9}  {:>9.1} MB {:>9.1} MB {:>8} {:>8}",
            h.pid,
            h.instance,
            h.cpu_percent,
            share,
            mb(h.private_bytes),
            mb(h.working_set_private),
            h.handle_count,
            h.thread_count
        );
    }

    println!("\n== __ProviderHostQuotaConfiguration (root) ==");
    match hosts.quota {
        None => println!("  not readable"),
        Some(q) => {
            println!(
                "  MemoryPerHost {} ({:.0} MB)   HandlesPerHost {}   ThreadsPerHost {}",
                q.memory_per_host,
                mb(q.memory_per_host),
                q.handles_per_host,
                q.threads_per_host
            );
            println!(
                "  MemoryAllHosts {} ({:.0} MB)   ProcessLimitAllHosts {}",
                q.memory_all_hosts,
                mb(q.memory_all_hosts),
                q.process_limit_all_hosts
            );
            println!("\n== usage against the ceiling ==");
            println!(
                "{:>7}  {:<14} {:>9} {:>9} {:>9}   nearest ceiling",
                "pid", "instance", "memory", "handles", "threads"
            );
            for h in &hosts.stats {
                let pct = |f: Option<f32>| match f {
                    Some(f) => format!("{:.1} %", f * 100.0),
                    None => "—".into(),
                };
                let worst = match q.pressure(h) {
                    Some((kind, f)) => format!("{} at {:.1} %", kind.as_str(), f * 100.0),
                    None => "no quota".into(),
                };
                println!(
                    "{:>7}  {:<14} {:>9} {:>9} {:>9}   {worst}",
                    h.pid,
                    h.instance,
                    pct(q.memory_fraction(h)),
                    pct(q.handle_fraction(h)),
                    pct(q.thread_fraction(h)),
                );
            }
        }
    }

    println!("\n== the join, checked ==");
    // The claim under test: `Win32_Process.Name` cannot separate the hosts, the
    // perf instance name is a different string again, and only the PID works.
    let mut by_process: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for h in &hosts.stats {
        let name = providers
            .iter()
            .find(|p| p.host_pid == h.pid)
            .map(|p| p.host_process.as_str())
            .unwrap_or("");
        *by_process.entry(name).or_default() += 1;
    }
    for (name, n) in &by_process {
        println!("  Win32_Process.Name {name:?} covers {n} distinct host process(es)");
    }
    for h in &hosts.stats {
        let owners: Vec<&str> = providers
            .iter()
            .filter(|p| p.host_pid == h.pid)
            .map(|p| p.provider.as_str())
            .collect();
        println!(
            "  pid {:>7} = perf {:<14} hosts {:?}",
            h.pid, h.instance, owners
        );
    }

    if !hosts.is_complete() {
        println!("\n== unreadable ==");
        for u in &hosts.unreadable {
            println!("  {u}");
        }
    }
}
