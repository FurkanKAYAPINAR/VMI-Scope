//! Ground truth for Phase 6.1-6.3: prove `__RELPATH` arrives when
//! `include_system` is set, and diff two real `Win32_Service` snapshots keyed
//! on `Name`.
//!
//! Run with: `cargo run -p vmiscope-core --example compare`

use std::time::{Duration, Instant};

use vmiscope_core::{diff_tables, QueryResult, Request, Response, WmiWorker};

const NS: &str = "root\\CIMV2";

/// Send a query and block until its reply (or an error) comes back.
fn query(
    worker: &WmiWorker,
    id: u64,
    wql: &str,
    include_system: bool,
) -> Result<QueryResult, String> {
    worker.send(Request::Query {
        id,
        namespace: NS.into(),
        wql: wql.into(),
        max_rows: Some(5000),
        timeout: Some(Duration::from_secs(20)),
        include_system,
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        for msg in worker.poll() {
            if msg.id() != id {
                continue;
            }
            match msg {
                Response::QueryResult { result, .. } => return Ok(result),
                Response::Error { message, .. } => return Err(message),
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err("timed out waiting for reply".into())
}

fn key_col(qr: &QueryResult) -> String {
    qr.key_columns.join(", ")
}

fn main() {
    let worker = WmiWorker::spawn();

    // --- Part 1: __RELPATH arrives only when include_system is set -----------
    let stripped =
        query(&worker, 1, "SELECT * FROM Win32_Service", false).expect("stripped query failed");
    let full = query(&worker, 2, "SELECT * FROM Win32_Service", true)
        .expect("include_system query failed");

    let has = |qr: &QueryResult, c: &str| qr.columns.iter().any(|x| x == c);
    println!("== Part 1: system columns ==");
    println!(
        "  include_system=false: __RELPATH present? {}  (columns: {})",
        has(&stripped, "__RELPATH"),
        stripped.columns.len()
    );
    println!(
        "  include_system=true : __RELPATH={} __PATH={} __CLASS={}  (columns: {})",
        has(&full, "__RELPATH"),
        has(&full, "__PATH"),
        has(&full, "__CLASS"),
        full.columns.len()
    );
    println!("  key_columns reported by the worker: [{}]", key_col(&full));
    if let (Some(rel), Some(row)) = (
        full.columns.iter().position(|c| c == "__RELPATH"),
        full.rows.first(),
    ) {
        println!("  sample __RELPATH value: {}", row[rel]);
    }

    // --- Part 2: two real snapshots, keyed on Name ---------------------------
    // Auto-key comes from the worker; ProcessId is treated as volatile.
    let wql = "SELECT Name, State, StartMode, ProcessId FROM Win32_Service";
    let a = query(&worker, 3, wql, true).expect("snapshot A failed");
    let b = query(&worker, 4, wql, true).expect("snapshot B failed");
    let key = if a.key_columns.is_empty() {
        vec!["Name".to_string()]
    } else {
        a.key_columns.clone()
    };
    let ignore = vec!["ProcessId".to_string()];

    println!(
        "\n== Part 2: A vs B (same query twice), key=[{}] ==",
        key.join(", ")
    );
    let d = diff_tables(&a, &b, &key, &ignore);
    println!(
        "  A={} rows, B={} rows -> added={} removed={} changed={} unchanged={}",
        a.rows.len(),
        b.rows.len(),
        d.added.len(),
        d.removed.len(),
        d.changed.len(),
        d.unchanged
    );

    // --- Part 3: all services vs the running subset --------------------------
    // Real removals: services not in 'Running' state vanish from the B side.
    let running = query(
        &worker,
        5,
        "SELECT Name, State, StartMode, ProcessId FROM Win32_Service WHERE State='Running'",
        true,
    )
    .expect("running snapshot failed");
    let d2 = diff_tables(&a, &running, &key, &ignore);
    println!("\n== Part 3: all services vs WHERE State='Running' ==");
    println!(
        "  all={} running={} -> added={} removed={} changed={} unchanged={}",
        a.rows.len(),
        running.rows.len(),
        d2.added.len(),
        d2.removed.len(),
        d2.changed.len(),
        d2.unchanged
    );
    if let Some(r) = d2.removed.first() {
        println!("  sample removed row key: {:?}", r.key);
    }

    // --- Part 4: the diff serializes to JSON ---------------------------------
    let json = serde_json::to_string(&d2).expect("diff serializes");
    println!("\n== Part 4: TableDiff JSON is {} bytes ==", json.len());

    worker.send(Request::Shutdown);
}
