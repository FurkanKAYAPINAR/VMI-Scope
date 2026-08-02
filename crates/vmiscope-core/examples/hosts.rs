//! Multi-host: one COM thread per target, and where a result says it came from.
//!
//! Two targets exist on any single machine — the local machine reached with no
//! host at all, and the same machine reached by name over DCOM as the current
//! user. They are genuinely different code paths (`root\CIMV2` versus
//! `\\HOST\root\CIMV2`, and a different authentication level on the proxy), so
//! they make a real two-host test out of one box.
//!
//! Run with: `cargo run -p vmiscope-core --example hosts`

use std::time::{Duration, Instant};

use vmiscope_core::{HostRef, Request, Response, WmiWorker, WorkerRegistry};

fn drain_for(reg: &WorkerRegistry, want: usize, secs: u64) -> Vec<(HostRef, Response)> {
    let mut out = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(secs);
    while out.len() < want && Instant::now() < deadline {
        out.extend(reg.poll());
        std::thread::sleep(Duration::from_millis(25));
    }
    out
}

fn main() {
    let name = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "localhost".into());
    let local = HostRef::Local;
    let by_name = HostRef::Sso { host: name.clone() };

    println!("== connect both targets ==");
    let mut reg = WorkerRegistry::new();
    reg.open(1, &local, None);
    reg.open(2, &by_name, None);
    println!("  {} COM threads live", reg.len());

    for (target, resp) in drain_for(&reg, 2, 60) {
        match resp {
            Response::HostConnected {
                connect_ms,
                probe_ms,
                info,
                host,
                ..
            } => {
                println!(
                    "  {:<26} connect {connect_ms:>4} ms  probe {probe_ms:>4} ms  stamp {:?}",
                    target.label(),
                    host
                );
                println!("      {}", info.summary());
                println!(
                    "      build {}  booted {}  uuid {}",
                    info.build_number, info.last_boot, info.uuid
                );
            }
            Response::Error {
                context, message, ..
            } => {
                println!("  {:<26} FAILED [{context}] {message}", target.label())
            }
            other => println!("  unexpected: {other:?}"),
        }
    }

    println!("\n== interleave work across both, with no SetHost between ==");
    let wql = "SELECT Name, NumberOfLogicalProcessors FROM Win32_ComputerSystem";
    for (i, target) in [&local, &by_name, &local, &by_name].iter().enumerate() {
        let sent = reg.send(
            target,
            Request::Query {
                id: 100 + i as u64,
                namespace: "root\\CIMV2".into(),
                wql: wql.into(),
                max_rows: Some(1),
                timeout: Some(Duration::from_secs(10)),
                include_system: false,
            },
        );
        assert!(sent, "no worker for {}", target.label());
    }
    let mut replies = drain_for(&reg, 4, 60);
    replies.sort_by_key(|(_, r)| r.id());
    for (target, resp) in &replies {
        if let Response::QueryResult {
            id, host, result, ..
        } = resp
        {
            println!(
                "  id {id} on {:<26} host stamp {:<20} {} row(s), {} ms connect + {} ms query",
                target.label(),
                format!("{host:?}"),
                result.rows.len(),
                result.connect_ms,
                result.elapsed_ms
            );
        } else {
            println!("  id {} on {:<26} {resp:?}", resp.id(), target.label());
        }
    }

    println!("\n== one worker, a SetHost queued between two queries ==");
    // The case task 5.1 is about: all three requests are queued before any of
    // them runs, so "the host that is current when the reply is read" is the
    // same for all three and wrong for two of them.
    let worker = WmiWorker::spawn();
    let q = |id: u64| Request::Query {
        id,
        namespace: "root\\CIMV2".into(),
        wql: "SELECT Name FROM Win32_ComputerSystem".into(),
        max_rows: Some(1),
        timeout: Some(Duration::from_secs(10)),
        include_system: false,
    };
    worker.send(q(1));
    worker.send(Request::SetHost {
        id: 2,
        host: Some(name.clone()),
        cred: None,
        impersonation: vmiscope_core::Impersonation::default(),
    });
    worker.send(q(3));

    let mut seen = 0;
    let deadline = Instant::now() + Duration::from_secs(60);
    while seen < 3 && Instant::now() < deadline {
        for resp in worker.poll() {
            seen += 1;
            println!("  reply id {} stamped {:?}", resp.id(), resp.host());
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    println!("  (the host in force when all three replies are read is {name:?})");
}
