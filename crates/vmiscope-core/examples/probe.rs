//! Quick reality check against the live WMI service.
//! Run with: `cargo run -p vmiscope-core --example probe`

use vmiscope_core::{ParamSchema, Request, Response, WmiWorker};

/// Render a parameter list as `name: type [direction]`, comma separated.
fn signature(params: &[ParamSchema]) -> String {
    params
        .iter()
        .map(|p| format!("{}: {} [{}]", p.name, p.cim_type, p.direction()))
        .collect::<Vec<_>>()
        .join(", ")
}

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
        max_rows: None,
        timeout: None,
    });
    worker.send(Request::NetworkSnapshot { id: id() });
    worker.send(Request::ListEventSubscriptions { id: id() });
    // Class reflection: one representative of every `ClassKind`, plus the two
    // classes the parameter-direction and static-method rules hinge on.
    for class in [
        "Win32_Process",
        "Win32_LogicalDiskToPartition",
        "__InstanceCreationEvent",
        "CIM_Process",
        "Win32_PerfFormattedData_PerfProc_Process",
        "Win32_WMISetting",
        "Win32_USBHub",
        "Win32_OperatingSystem",
    ] {
        worker.send(Request::ClassSchema {
            id: id(),
            namespace: "root\\cimv2".into(),
            class: class.into(),
        });
    }
    worker.send(Request::ClassMof {
        id: id(),
        namespace: "root\\cimv2".into(),
        object_path: "Win32_Process".into(),
    });
    worker.send(Request::ListInstances {
        id: id(),
        namespace: "root\\cimv2".into(),
        class: "Win32_Service".into(),
    });
    worker.send(Request::BuildSearchIndex {
        id: id(),
        namespace: "root\\cimv2".into(),
        include_methods: false,
    });
    // Read-only method: enumerate HKLM\SOFTWARE subkeys via StdRegProv.
    worker.send(Request::InvokeMethod {
        id: id(),
        namespace: "root\\cimv2".into(),
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
    // Same call, but as a caller who does *not* know the method is static and
    // has no instance to offer — the shape a missing `Static` qualifier
    // produces. It must still run, against the class path.
    worker.send(Request::InvokeMethod {
        id: id(),
        namespace: "root\\cimv2".into(),
        class: "StdRegProv".into(),
        object_path: String::new(),
        method: "EnumKey".into(),
        is_static: false,
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
    // The other half of the fallback: a static method aimed at an instance
    // path. WMI rejects it, we retry on the class path. The command line is
    // deliberately unreachable, so the retry returns 9 ("path not found")
    // instead of starting anything.
    worker.send(Request::InvokeMethod {
        id: id(),
        namespace: "root\\cimv2".into(),
        class: "Win32_Process".into(),
        object_path: "Win32_Process.Handle=\"0\"".into(),
        method: "Create".into(),
        is_static: false,
        args: vec![vmiscope_core::MethodArg {
            name: "CommandLine".into(),
            kind: vmiscope_core::ParamKind::Str,
            value: "Z:\\vmiscope-no-such-file.exe".into(),
        }],
    });

    // Give the worker time and drain replies.
    let expected = next_id as usize;
    let mut received = 0;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while received < expected && std::time::Instant::now() < deadline {
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
                    namespace,
                    classes,
                    completion,
                    elapsed_ms,
                    ..
                } => {
                    println!(
                        "\n== classes in {namespace} ({}, {elapsed_ms} ms{}) ==",
                        classes.len(),
                        completion
                            .note()
                            .map(|n| format!(", {n}"))
                            .unwrap_or_default()
                    );
                    for c in classes
                        .iter()
                        .filter(|c| c.name.starts_with("Win32_"))
                        .take(15)
                    {
                        println!(
                            "  {:<44} {:<24} {}",
                            c.name,
                            c.kind.labels().join("|"),
                            c.provider.clone().unwrap_or_default()
                        );
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
                            .as_ref()
                            .map(|s| format!(" : {s}"))
                            .unwrap_or_default()
                    );
                    let labels = schema.kind.labels();
                    println!(
                        "  kind:       {}",
                        if labels.is_empty() {
                            "-".into()
                        } else {
                            labels.join(" | ")
                        }
                    );
                    println!(
                        "  derivation: {}",
                        if schema.derivation.is_empty() {
                            "-".into()
                        } else {
                            schema.derivation.join(" > ")
                        }
                    );
                    println!("  qualifiers ({}):", schema.qualifiers.len());
                    for (name, value) in &schema.qualifiers {
                        let short: String = value.chars().take(60).collect();
                        println!("     {name:<20} = {short}");
                    }
                    for p in schema.properties.iter().take(4) {
                        let flags = format!(
                            "{}{}{}",
                            if p.is_key { "K" } else { "" },
                            if p.is_read { "R" } else { "" },
                            if p.is_write { "W" } else { "" }
                        );
                        println!("  {:<22} {:<10} {}", p.name, p.cim_type, flags);
                    }
                    for m in schema.methods.iter().take(8) {
                        let tag = match (m.declared_static, m.is_static) {
                            (true, _) => "  [static]",
                            (false, true) => "  [static: no key / singleton]",
                            _ => "",
                        };
                        println!(
                            "  method {}({}) -> ({}){tag}",
                            m.name,
                            signature(&m.in_params),
                            signature(&m.out_params),
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
                Response::SearchIndex { index, .. } => {
                    let props: usize = index.properties.values().map(|v| v.len()).sum();
                    println!(
                        "\n== search index: {} classes, {props} property names ==",
                        index.classes.len()
                    );
                }
                Response::Instances { class, targets, .. } => {
                    println!("\n== instances of {class} ({}) ==", targets.len());
                    for t in targets.iter().take(4) {
                        println!("  {} -> {}", t.label, t.path);
                    }
                }
                Response::MethodDone {
                    class,
                    method,
                    outcome,
                    ..
                } => {
                    println!(
                        "\n== invoke {class}.{method} -> ReturnValue={:?} ==",
                        outcome.return_value
                    );
                    for (k, v) in outcome.outputs.iter().take(3) {
                        let short: String = v.chars().take(80).collect();
                        println!("  {k} = {short}");
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
    println!("\ndone ({received}/{expected} responses)");
}
