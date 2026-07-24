//! Quick reality check against the live WMI service.
//! Run with: `cargo run -p vmiscope-core --example probe`

use vmiscope_core::{Request, Response, WmiWorker};

fn main() {
    let worker = WmiWorker::spawn();
    let mut next_id = 0u64;
    let mut id = || {
        next_id += 1;
        next_id
    };

    worker.send(Request::ListChildNamespaces {
        id: id(),
        namespace: "root".into(),
    });
    worker.send(Request::ListClasses {
        id: id(),
        namespace: "root\\cimv2".into(),
    });
    worker.send(Request::Query {
        id: id(),
        namespace: "root\\cimv2".into(),
        wql: "SELECT Caption, Version, BuildNumber FROM Win32_OperatingSystem".into(),
    });
    worker.send(Request::NetworkSnapshot { id: id() });
    worker.send(Request::ListEventSubscriptions { id: id() });
    worker.send(Request::ClassSchema {
        id: id(),
        namespace: "root\\cimv2".into(),
        class: "Win32_Process".into(),
    });
    worker.send(Request::ClassMof {
        id: id(),
        namespace: "root\\cimv2".into(),
        object_path: "Win32_Process".into(),
    });

    // Give the worker time and drain replies.
    let mut received = 0;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while received < 7 && std::time::Instant::now() < deadline {
        for resp in worker.poll() {
            received += 1;
            match resp {
                Response::ChildNamespaces {
                    namespace,
                    children,
                    ..
                } => {
                    println!(
                        "\n== child namespaces of {namespace} ({}) ==",
                        children.len()
                    );
                    for c in children.iter().take(15) {
                        println!("  {c}");
                    }
                }
                Response::Classes {
                    namespace, classes, ..
                } => {
                    println!("\n== classes in {namespace} ({}) ==", classes.len());
                    for c in classes.iter().filter(|c| c.starts_with("Win32_")).take(15) {
                        println!("  {c}");
                    }
                }
                Response::QueryResult { result, wql, .. } => {
                    println!("\n== query: {wql} ==");
                    println!("  columns: {:?}", result.columns);
                    for row in &result.rows {
                        println!("  row: {:?}", row);
                    }
                }
                Response::Network { snapshot, .. } => {
                    let tcp = snapshot
                        .connections
                        .iter()
                        .filter(|c| matches!(c.proto, vmiscope_core::Protocol::Tcp))
                        .count();
                    println!(
                        "\n== network snapshot ({} endpoints, {tcp} tcp) ==",
                        snapshot.connections.len()
                    );
                    for c in snapshot
                        .connections
                        .iter()
                        .filter(|c| !c.state.is_empty())
                        .take(12)
                    {
                        println!(
                            "  {:<4} {:<22} -> {:<22} {:<12} pid={:<6} {}",
                            c.proto.as_str(),
                            format!("{}:{}", c.local_addr, c.local_port),
                            format!("{}:{}", c.remote_addr, c.remote_port),
                            c.state,
                            c.pid,
                            c.process
                        );
                    }
                }
                Response::EventSubscriptions { report, .. } => {
                    println!(
                        "\n== event subscriptions ({} bound) ==",
                        report.subscriptions.len()
                    );
                    for s in &report.subscriptions {
                        println!(
                            "  [{}] {} -> {} ({})  action={:?}  why={:?}",
                            s.risk.as_str(),
                            s.filter_name,
                            s.consumer_name,
                            s.consumer_type,
                            s.action,
                            s.reasons
                        );
                    }
                }
                Response::Schema { class, schema, .. } => {
                    println!(
                        "\n== schema: {class} ({} props, {} methods){} ==",
                        schema.properties.len(),
                        schema.methods.len(),
                        schema
                            .super_class
                            .map(|s| format!(" : {s}"))
                            .unwrap_or_default()
                    );
                    for p in schema.properties.iter().take(6) {
                        let flags = format!(
                            "{}{}{}",
                            if p.is_key { "K" } else { "" },
                            if p.is_read { "R" } else { "" },
                            if p.is_write { "W" } else { "" }
                        );
                        println!("  {:<22} {:<10} {}", p.name, p.cim_type, flags);
                    }
                    for m in schema.methods.iter().take(6) {
                        println!(
                            "  method {}({} in, {} out){}",
                            m.name,
                            m.in_params.len(),
                            m.out_params.len(),
                            if m.is_static { " [static]" } else { "" }
                        );
                    }
                }
                Response::Mof {
                    object_path, mof, ..
                } => {
                    println!("\n== MOF: {object_path} ({} chars) ==", mof.len());
                    for line in mof.lines().take(6) {
                        println!("  {line}");
                    }
                }
                Response::Error {
                    context, message, ..
                } => {
                    eprintln!("\n!! ERROR [{context}]: {message}");
                }
                _ => {}
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    println!("\ndone ({received}/3 responses)");
}
