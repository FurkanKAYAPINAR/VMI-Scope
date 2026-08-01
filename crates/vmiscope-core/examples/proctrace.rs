//! Process-monitor reality check: elevation, the trace subscription, the typed
//! denial, and the degraded fallback.
//!
//! Run with: `cargo run -p vmiscope-core --example proctrace`
//!
//! What it proves, in order:
//!  1. what `is_elevated()` says about this token;
//!  2. what the *raw* `Win32_ProcessStartTrace` subscription does, and that the
//!     answer arrives from the subscribe call rather than from the iterator;
//!  3. which mode `ProcessMonitor` settled on, and what that costs;
//!  4. that events, resolved user names and command lines actually arrive;
//!  5. the instant-exit gap, measured live rather than quoted.

use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use vmiscope_core::{
    is_elevated, resolve_sid, Enrichment, MonitorMode, ProcMsg, ProcessMonitor, SidResolver,
    TRACE_START_QUERY,
};

/// Long enough to be sampled by a `WITHIN 2` subscription.
const LIVE_SECS: &str = "4";

fn main() {
    println!("== token ==");
    println!("is_elevated(): {}", is_elevated());

    println!();
    println!("== raw trace subscription ==");
    // Straight at the `wmi` crate, so the denial is shown where it happens
    // rather than after the monitor has already recovered from it.
    match wmi::WMIConnection::with_namespace_path("root\\CIMV2") {
        Ok(conn) => match conn.exec_notification_query(TRACE_START_QUERY) {
            Ok(_) => println!("{TRACE_START_QUERY}\n  -> subscribed"),
            Err(e) => println!("{TRACE_START_QUERY}\n  -> refused at subscribe time: {e}"),
        },
        Err(e) => println!("cannot connect: {e}"),
    }

    println!();
    println!("== SID resolution ==");
    // S-1-5-18 (LOCAL SYSTEM) and a well-formed SID that belongs to nobody.
    let system = [1u8, 1, 0, 0, 0, 0, 0, 5, 18, 0, 0, 0];
    let mut nowhere = vec![1u8, 5, 0, 0, 0, 0, 0, 5];
    for sub in [21u32, 1_111_111_111, 2_222_222_222, 3_333_333_333, 4_444] {
        nowhere.extend_from_slice(&sub.to_le_bytes());
    }
    println!("S-1-5-18        -> {:?}", resolve_sid(&system));
    println!("unknown domain  -> {:?}", resolve_sid(&nowhere));
    println!("absent          -> {:?}", resolve_sid(&[]));
    let mut cache = SidResolver::new();
    cache.resolve(&system);
    cache.resolve(&system);
    println!(
        "cache entries after two identical lookups: {}",
        cache.cached()
    );

    println!();
    println!("== ProcessMonitor ==");
    let mon = ProcessMonitor::start();

    // Wait for the mode to settle before spawning anything, so nothing is
    // spawned into a window where no subscription exists yet.
    let settle = Instant::now();
    while mon.mode().is_none() && settle.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(50));
    }
    match mon.mode() {
        Some(m) => {
            println!(
                "mode: {}",
                if m.is_degraded() { "DEGRADED" } else { "trace" }
            );
            println!("  {}", m.summary());
            if let MonitorMode::Intrinsic { reason, .. } = &m {
                println!("  typed error: {reason:?}");
            }
        }
        None => {
            println!("monitor never reported a mode");
            for msg in mon.poll() {
                if let ProcMsg::Error(e) = msg {
                    println!("  error: {e}");
                }
            }
            return;
        }
    }

    // Two kinds of child on purpose: long-lived ones a polled subscription can
    // see, and instant-exit ones it mostly cannot. The difference between the
    // two counts is the blind spot, measured rather than quoted.
    println!();
    println!("spawning 3 long-lived + 12 instant-exit children");
    let mut kids = Vec::new();
    for _ in 0..3 {
        if let Ok(c) = Command::new("ping")
            .args(["-n", LIVE_SECS, "127.0.0.1"])
            .stdout(Stdio::null())
            .spawn()
        {
            kids.push(c);
        }
        std::thread::sleep(Duration::from_millis(400));
    }
    let mut instant = 0;
    for _ in 0..12 {
        if Command::new("cmd")
            .args(["/c", "exit"])
            .stdout(Stdio::null())
            .spawn()
            .and_then(|mut c| c.wait())
            .is_ok()
        {
            instant += 1;
        }
    }

    // Collect for long enough that a `WITHIN 2` sampler gets several chances.
    let deadline = Instant::now() + Duration::from_secs(12);
    let mut events: Vec<(u64, String)> = Vec::new();
    let mut details: HashMap<u64, String> = HashMap::new();
    let mut starts = 0;
    let mut stops = 0;
    let mut caught_instant = 0;
    let mut errors = Vec::new();

    while Instant::now() < deadline {
        for msg in mon.poll() {
            match msg {
                ProcMsg::Mode(_) => {}
                ProcMsg::Event { seq, event } => {
                    match event.kind {
                        vmiscope_core::ProcKind::Start => starts += 1,
                        vmiscope_core::ProcKind::Stop => stops += 1,
                    }
                    // Matched on the parent pid, not just the image name: this
                    // machine starts `cmd.exe` under the task scheduler on its
                    // own, and counting those would inflate the catch rate.
                    if event.kind == vmiscope_core::ProcKind::Start
                        && event.parent_pid == std::process::id()
                        && event.name.eq_ignore_ascii_case("cmd.exe")
                    {
                        caught_instant += 1;
                    }
                    events.push((
                        seq,
                        format!(
                            "{} pid={:<6} ppid={:<6} sess={} {:<20} exit={}",
                            event.kind.sign(),
                            event.pid,
                            event.parent_pid,
                            event.session_id,
                            event.name,
                            event
                                .exit_status
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| "-".into()),
                        ),
                    ));
                }
                ProcMsg::Details {
                    seq,
                    user,
                    enrichment,
                } => {
                    let cmd = match enrichment {
                        Enrichment::Found(info) if !info.command_line.is_empty() => {
                            info.command_line
                        }
                        Enrichment::Found(info) if !info.executable_path.is_empty() => {
                            info.executable_path
                        }
                        Enrichment::Found(_) => "<empty>".into(),
                        Enrichment::Unavailable => "<gone or pid reused>".into(),
                        Enrichment::Skipped => "<not attempted>".into(),
                    };
                    details.insert(
                        seq,
                        format!(
                            "user={:<28} cmd={}",
                            if user.is_empty() { "-" } else { &user },
                            cmd
                        ),
                    );
                }
                ProcMsg::Error(e) => errors.push(e),
            }
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    for mut k in kids {
        let _ = k.wait();
    }

    println!();
    println!("== events ({} total) ==", events.len());
    for (seq, line) in events.iter().take(24) {
        println!(
            "  {line}  {}",
            details.get(seq).map(String::as_str).unwrap_or("...")
        );
    }
    if events.len() > 24 {
        println!("  ... {} more", events.len() - 24);
    }

    println!();
    println!("== summary ==");
    println!(
        "starts: {starts}   stops: {stops}   details attached: {}",
        details.len()
    );
    println!("instant-exit children spawned: {instant}, caught: {caught_instant}");
    for e in &errors {
        println!("error: {e}");
    }
}
