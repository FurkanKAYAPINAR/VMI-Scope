//! What the alternate-credential path can be proved to do on one machine.
//!
//! WMI refuses credentialed *local* connections, so a successful alt-cred
//! session cannot exist here and nothing can measure what one returns. Two
//! things can still be established, and both are worth more than they look:
//!
//! 1. **The credentials reach DCOM.** A bogus credential is rejected by the
//!    remote end with a WMI/RPC error rather than crashing on the
//!    `COAUTHIDENTITY` buffers — at every impersonation level, which is the
//!    only local evidence that the level reaches the blanket call at all.
//!
//! 2. **Nothing silently falls back to SSO.** This is the important one. With
//!    the worker in alternate-credential mode and credentials that cannot
//!    connect, *every* request must fail. Any request that comes back with data
//!    answered from a connection built as the current user — which is exactly
//!    the bug tasks 5.6 and 5.9 exist to remove, and the shape it has when it
//!    is present: not an error, a plausible answer about the wrong computer.
//!
//! Run with: `cargo run -p vmiscope-core --example altcred`

use std::time::{Duration, Instant};

use vmiscope_core::{Credential, Impersonation, RemoteConn, Request, Response, WmiWorker};

fn bogus() -> Credential {
    Credential {
        user: "bogus_user".into(),
        password: "bogus_pass".into(),
        domain: Some("BOGUSDOM".into()),
    }
}

/// Every request shape the worker answers, aimed at things that all exist on a
/// healthy local machine — so a reply carrying data means the request was
/// served locally rather than refused.
fn every_request_shape(next: &mut impl FnMut() -> u64) -> Vec<(u64, &'static str, Request)> {
    let ns = || "root\\CIMV2".to_string();
    let mut out = Vec::new();
    let mut add = |name: &'static str, req: Request| out.push((0, name, req));
    add(
        "ListChildNamespaces",
        Request::ListChildNamespaces {
            id: 0,
            namespace: "root".into(),
        },
    );
    add(
        "ListClasses",
        Request::ListClasses {
            id: 0,
            namespace: ns(),
        },
    );
    add(
        "NamespaceStats",
        Request::NamespaceStats {
            id: 0,
            namespace: ns(),
            recursive: false,
        },
    );
    add(
        "InstanceCount",
        Request::InstanceCount {
            id: 0,
            namespace: ns(),
            class: "Win32_Process".into(),
            deep: false,
        },
    );
    add(
        "Associations",
        Request::Associations {
            id: 0,
            namespace: ns(),
            class: "Win32_Process".into(),
        },
    );
    add(
        "Query",
        Request::Query {
            id: 0,
            namespace: ns(),
            wql: "SELECT Name FROM Win32_ComputerSystem".into(),
            max_rows: Some(5),
            timeout: Some(Duration::from_secs(5)),
            include_system: false,
        },
    );
    add("NetworkSnapshot", Request::NetworkSnapshot { id: 0 });
    add(
        "ListEventSubscriptions",
        Request::ListEventSubscriptions { id: 0 },
    );
    add("ListProviders", Request::ListProviders { id: 0 });
    add(
        "ClassSchema",
        Request::ClassSchema {
            id: 0,
            namespace: ns(),
            class: "Win32_Process".into(),
        },
    );
    add(
        "ClassMof",
        Request::ClassMof {
            id: 0,
            namespace: ns(),
            object_path: "Win32_Process".into(),
        },
    );
    add(
        "ListInstances",
        Request::ListInstances {
            id: 0,
            namespace: ns(),
            class: "Win32_Service".into(),
        },
    );
    add(
        "InvokeMethod",
        Request::InvokeMethod {
            id: 0,
            namespace: ns(),
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
        },
    );
    add(
        "BuildSearchIndex",
        Request::BuildSearchIndex {
            id: 0,
            namespace: ns(),
            include_methods: false,
        },
    );

    // Stamp real ids now that the shapes are built.
    for entry in out.iter_mut() {
        let id = next();
        entry.0 = id;
        set_id(&mut entry.2, id);
    }
    out
}

fn set_id(req: &mut Request, new: u64) {
    match req {
        Request::ListChildNamespaces { id, .. }
        | Request::ListClasses { id, .. }
        | Request::NamespaceStats { id, .. }
        | Request::InstanceCount { id, .. }
        | Request::Associations { id, .. }
        | Request::Query { id, .. }
        | Request::NetworkSnapshot { id }
        | Request::ListEventSubscriptions { id }
        | Request::ListProviders { id }
        | Request::ClassSchema { id, .. }
        | Request::ClassMof { id, .. }
        | Request::ListInstances { id, .. }
        | Request::InvokeMethod { id, .. }
        | Request::BuildSearchIndex { id, .. }
        | Request::SetHost { id, .. }
        | Request::Cancel { id } => *id = new,
        Request::Shutdown => {}
    }
}

fn main() {
    let host = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "localhost".into());

    println!("== 1. do the credentials reach DCOM, at every impersonation level? ==");
    for imp in Impersonation::all() {
        let t0 = Instant::now();
        let outcome = RemoteConn::connect(&host, "root\\cimv2", &bogus(), imp);
        let ms = t0.elapsed().as_millis();
        match outcome {
            Ok(_) => println!(
                "  {:<12} UNEXPECTED: connected with bogus creds",
                imp.as_str()
            ),
            Err(e) => println!(
                "  {:<12} rejected in {ms} ms: {}",
                imp.as_str(),
                e.to_string().lines().next().unwrap_or_default()
            ),
        }
    }
    println!("  no crash -> the COAUTHIDENTITY buffers outlive the call at all three levels");

    println!("\n== 2. does anything fall back to SSO when the credentials cannot connect? ==");
    let worker = WmiWorker::spawn();
    let mut n = 0u64;
    let mut next = || {
        n += 1;
        n
    };

    let connect_id = next();
    worker.send(Request::SetHost {
        id: connect_id,
        host: Some(host.clone()),
        cred: Some(bogus()),
        impersonation: Impersonation::Impersonate,
    });

    let requests = every_request_shape(&mut next);
    let expected: Vec<(u64, &str)> = requests.iter().map(|(id, name, _)| (*id, *name)).collect();
    for (_, _, req) in requests {
        worker.send(req);
    }

    let mut leaked: Vec<&str> = Vec::new();
    let mut refused = 0usize;
    let mut seen = 0usize;
    let deadline = Instant::now() + Duration::from_secs(120);
    while seen <= expected.len() && Instant::now() < deadline {
        for resp in worker.poll() {
            seen += 1;
            let id = resp.id();
            let label = expected
                .iter()
                .find(|(rid, _)| *rid == id)
                .map(|(_, name)| *name)
                .unwrap_or("SetHost");
            match &resp {
                Response::Error { message, .. } => {
                    refused += 1;
                    println!(
                        "  {label:<24} refused: {}",
                        message.lines().next().unwrap_or_default()
                    );
                }
                other => {
                    if label != "SetHost" {
                        leaked.push(label);
                    }
                    println!("  {label:<24} ANSWERED (host stamp: {:?})", other.host());
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    println!(
        "\n  {refused} refused, {} answered, {seen} of {} replies seen",
        leaked.len(),
        expected.len() + 1
    );
    if leaked.is_empty() {
        println!("  PASS: no request answered from a current-user connection");
    } else {
        println!("  FAIL: these answered as the current user: {leaked:?}");
    }
}
