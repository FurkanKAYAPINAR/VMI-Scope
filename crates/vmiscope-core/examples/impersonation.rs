//! Is the impersonation level on the proxy blanket observable?
//!
//! `docs/REDESIGN.md` task 5.10 says the level is reachable only where
//! `remote.rs` hand-calls `CoSetProxyBlanket`, because the SSO path goes
//! through the `wmi` crate, which does not expose it. That stopped being true
//! when the SSO path moved onto `enumerate::DirectConn`, which sets its own
//! blanket — so the level can be set there too. Whether WMI *notices* is a
//! different question, and the only way to answer it is to try.
//!
//! Every level below is exercised against the local machine through the
//! ordinary request path, so what is measured is what a user would get.
//!
//! Run with: `cargo run -p vmiscope-core --example impersonation`

use std::time::{Duration, Instant};

use vmiscope_core::{Impersonation, Request, Response, WmiWorker};

fn drain(worker: &WmiWorker, want: usize, secs: u64) -> Vec<Response> {
    let mut out = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(secs);
    while out.len() < want && Instant::now() < deadline {
        out.extend(worker.poll());
        std::thread::sleep(Duration::from_millis(25));
    }
    out
}

fn one_line(r: &Response) -> String {
    match r {
        Response::HostConnected {
            connect_ms,
            probe_ms,
            info,
            ..
        } => format!(
            "connected ({connect_ms} ms + {probe_ms} ms probe) {}",
            info.summary()
        ),
        Response::QueryResult { result, .. } => format!("{} row(s)", result.rows.len()),
        Response::Schema { schema, .. } => format!("{} properties", schema.properties.len()),
        Response::MethodDone { outcome, .. } => {
            format!("ReturnValue={:?}", outcome.return_value)
        }
        Response::Error {
            context, message, ..
        } => {
            format!("ERROR [{context}] {}", message.lines().next().unwrap_or(""))
        }
        other => format!("{other:?}"),
    }
}

fn main() {
    for imp in Impersonation::all() {
        println!("== local machine at {} ==", imp.as_str());
        let worker = WmiWorker::spawn();
        worker.send(Request::SetHost {
            id: 1,
            host: None,
            cred: None,
            impersonation: imp,
        });
        for r in drain(&worker, 1, 30) {
            println!("  SetHost       {}", one_line(&r));
        }

        // A query reaches WMI through an enumerator, which the SSO path
        // deliberately does not re-blanket; a schema read and a method
        // invocation go through the service proxy itself, which it does. If the
        // level bites anywhere, those two are where.
        worker.send(Request::Query {
            id: 2,
            namespace: "root\\CIMV2".into(),
            wql: "SELECT Name FROM Win32_ComputerSystem".into(),
            max_rows: Some(1),
            timeout: Some(Duration::from_secs(10)),
            include_system: false,
        });
        worker.send(Request::ClassSchema {
            id: 3,
            namespace: "root\\CIMV2".into(),
            class: "Win32_Process".into(),
        });
        worker.send(Request::InvokeMethod {
            id: 4,
            namespace: "root\\CIMV2".into(),
            class: "StdRegProv".into(),
            object_path: String::new(),
            method: "EnumKey".into(),
            is_static: true,
            args: vec![
                vmiscope_core::MethodArg {
                    name: "hDefKey".into(),
                    kind: vmiscope_core::ParamKind::Uint,
                    value: "2147483650".into(),
                },
                vmiscope_core::MethodArg {
                    name: "sSubKeyName".into(),
                    kind: vmiscope_core::ParamKind::Str,
                    value: "SOFTWARE".into(),
                },
            ],
        });
        let mut replies = drain(&worker, 3, 60);
        replies.sort_by_key(|r| r.id());
        for r in &replies {
            let what = match r.id() {
                2 => "query",
                3 => "schema",
                _ => "invoke",
            };
            println!("  {what:<13} {}", one_line(r));
        }
        println!();
    }
}
