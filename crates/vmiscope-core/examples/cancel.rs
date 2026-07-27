//! Does chunked enumeration actually cap, cancel and exit?
//!
//! Measured against live WMI, because the interesting failures here are all
//! runtime behaviour: a cap that never triggers, a cancel that arrives after
//! the work is done anyway, or a shutdown that queues behind a runaway query.

use std::time::{Duration, Instant};
use vmiscope_core::{Completion, Request, Response, WmiWorker};

const NS: &str = r"root\cimv2";

/// Run one query and report how it finished, or what it was still doing when
/// the budget ran out.
fn run(
    label: &str,
    wql: &str,
    max_rows: Option<usize>,
    timeout: Option<Duration>,
    budget: Duration,
    cancel_after: Option<Duration>,
) {
    let worker = WmiWorker::spawn();
    let t0 = Instant::now();
    worker.send(Request::Query {
        id: 1,
        namespace: NS.into(),
        wql: wql.into(),
        max_rows,
        timeout,
    });
    let mut cancelled_at: Option<Instant> = None;
    loop {
        for msg in worker.poll() {
            match msg {
                Response::QueryResult { result, .. } => {
                    let since = cancelled_at.map(|c| c.elapsed().as_millis());
                    println!(
                        "{label:<24} {:>7} rows  exec {:>6} ms  connect {:>4} ms  {:?}{}",
                        result.rows.len(),
                        result.elapsed_ms,
                        result.connect_ms,
                        result.completion,
                        since
                            .map(|m| format!("  (reply {m} ms after cancel)"))
                            .unwrap_or_default(),
                    );
                    return;
                }
                Response::Error { message, .. } => {
                    println!(
                        "{label:<24} error: {}",
                        message.lines().next().unwrap_or("")
                    );
                    return;
                }
                _ => {}
            }
        }
        if let Some(after) = cancel_after {
            if cancelled_at.is_none() && t0.elapsed() >= after {
                worker.cancel(1);
                cancelled_at = Some(Instant::now());
            }
        }
        if t0.elapsed() > budget {
            println!("{label:<24} NO REPLY within {:?}", budget);
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn main() {
    // Does the mechanism work at all, on a provider that answers instantly?
    run(
        "process, cap 5",
        "SELECT * FROM Win32_Process",
        Some(5),
        None,
        Duration::from_secs(20),
        None,
    );
    run(
        "process, uncapped",
        "SELECT * FROM Win32_Process",
        None,
        None,
        Duration::from_secs(20),
        None,
    );

    // A provider that streams a lot, slowly.
    run(
        "datafile, cap only",
        "SELECT * FROM CIM_DataFile",
        Some(200),
        None,
        Duration::from_secs(12),
        None,
    );
    run(
        "datafile, 3s deadline",
        "SELECT * FROM CIM_DataFile",
        Some(200),
        Some(Duration::from_secs(3)),
        Duration::from_secs(20),
        None,
    );
    run(
        "datafile, cancel@1s",
        "SELECT * FROM CIM_DataFile",
        None,
        None,
        Duration::from_secs(45),
        Some(Duration::from_secs(1)),
    );

    // The original hang: drop the worker while a runaway query is in flight.
    // `Drop` joins the thread, so this only returns if shutdown jumps the queue.
    let t = Instant::now();
    {
        let worker = WmiWorker::spawn();
        worker.send(Request::Query {
            id: 9,
            namespace: NS.into(),
            wql: "SELECT * FROM CIM_DataFile".into(),
            max_rows: None,
            timeout: None,
        });
        std::thread::sleep(Duration::from_millis(500));
    }
    println!(
        "{:<24} dropped mid-query in {} ms",
        "exit",
        t.elapsed().as_millis()
    );
    let _ = Completion::Complete;
}
