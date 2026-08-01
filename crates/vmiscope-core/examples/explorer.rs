//! Ground truth for the Explorer's core capabilities: class kinds, namespace
//! class counts, instance counts and associations.
//!
//! Run with: `cargo run -p vmiscope-core --example explorer`
//!
//! Every number this prints was measured, not assumed. The class enumeration
//! is timed three times over because the first call of a session pays for
//! warming the WMI repository and is not representative of anything a user
//! sees twice.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use vmiscope_core::{ClassKind, Request, Response, Tally, WmiWorker};

const NS: &str = "root\\CIMV2";

fn main() {
    let worker = WmiWorker::spawn();
    let mut next_id = 0u64;
    let mut id = || {
        next_id += 1;
        next_id
    };

    // -- 3.9 -------------------------------------------------------------
    // Three back-to-back enumerations of the same namespace. The worker runs
    // them in order on its single COM thread, so these are three independent
    // round trips, not one cached answer.
    for _ in 0..3 {
        worker.send(Request::ListClasses {
            id: id(),
            namespace: NS.into(),
        });
    }

    // -- 3.10 ------------------------------------------------------------
    worker.send(Request::NamespaceStats {
        id: id(),
        namespace: NS.into(),
        recursive: false,
    });
    // A subtree small enough to finish, and one that is not: `root` has more
    // namespaces than the budget allows, and must say so rather than quietly
    // returning the part it managed.
    worker.send(Request::NamespaceStats {
        id: id(),
        namespace: "root\\Interop".into(),
        recursive: true,
    });
    worker.send(Request::NamespaceStats {
        id: id(),
        namespace: "root".into(),
        recursive: true,
    });

    // -- 3.11 / 3.12 -----------------------------------------------------
    // Small and bounded; large and unbounded; then one class per skip-list
    // reason, which must come back as an em dash rather than a zero.
    for (class, deep) in [
        ("Win32_LogicalDisk", false),
        ("Win32_Process", false),
        ("CIM_DataFile", false),
        ("CIM_Process", false),          // abstract
        ("Win32_SessionProcess", false), // association
        // `__Event`-derived. It also reports `abstract = TRUE`, inherited from
        // `__Event` with `PropagatesToSubclass` set — as every event class in
        // this namespace does — which is why the event reason has to outrank
        // the abstract one to ever be seen.
        ("Win32_ProcessStartTrace", false),
    ] {
        worker.send(Request::InstanceCount {
            id: id(),
            namespace: NS.into(),
            class: class.into(),
            deep,
        });
    }

    // -- 3.13 ------------------------------------------------------------
    for class in ["Win32_Process", "Win32_LogicalDisk"] {
        worker.send(Request::Associations {
            id: id(),
            namespace: NS.into(),
            class: class.into(),
        });
    }

    let expected = next_id as usize;
    let mut received = 0;
    let deadline = Instant::now() + Duration::from_secs(180);
    while received < expected && Instant::now() < deadline {
        for resp in worker.poll() {
            received += 1;
            match resp {
                Response::Classes {
                    namespace,
                    classes,
                    completion,
                    elapsed_ms,
                    ..
                } => {
                    println!(
                        "\n== classes in {namespace}: {} in {elapsed_ms} ms{} ==",
                        classes.len(),
                        completion
                            .note()
                            .map(|n| format!(" [{n}]"))
                            .unwrap_or_default()
                    );
                    // How much of the badge column came back populated. A kind
                    // that is silently empty for every row would still print a
                    // plausible-looking class list.
                    let mut by_flag: BTreeMap<&str, usize> = BTreeMap::new();
                    let mut plain = 0usize;
                    let mut with_provider = 0usize;
                    for c in &classes {
                        if c.kind.is_empty() {
                            plain += 1;
                        }
                        for label in c.kind.labels() {
                            *by_flag.entry(label).or_default() += 1;
                        }
                        if c.provider.is_some() {
                            with_provider += 1;
                        }
                    }
                    println!("  kinds: {by_flag:?}  (no flags: {plain})");
                    println!("  provider qualifier present on {with_provider}");
                    for name in [
                        "Win32_Process",
                        "Win32_SessionProcess",
                        "CIM_Process",
                        "__InstanceCreationEvent",
                        "Win32_WMISetting",
                        "Win32_PerfFormattedData_PerfProc_Process",
                    ] {
                        if let Some(c) = classes.iter().find(|c| c.name == name) {
                            println!(
                                "  {:<44} {:<28} {}",
                                c.name,
                                c.kind.labels().join("|"),
                                c.provider.clone().unwrap_or_else(|| "-".into())
                            );
                        }
                    }
                }

                Response::NamespaceStats {
                    stats, elapsed_ms, ..
                } => {
                    println!(
                        "\n== namespace stats: {} ({}) in {elapsed_ms} ms ==",
                        stats.namespace,
                        if stats.recursive {
                            "recursive"
                        } else {
                            "shallow"
                        }
                    );
                    println!(
                        "  classes here: {}   children: {}",
                        stats.classes, stats.children
                    );
                    println!(
                        "  rollup: {} classes over {} namespaces ({} unreadable){}",
                        stats.total_classes,
                        stats.namespaces,
                        stats.unreadable,
                        stats
                            .completion
                            .note()
                            .map(|n| format!("  [{n}]"))
                            .unwrap_or_default()
                    );
                }

                Response::InstanceCount {
                    class,
                    tally,
                    elapsed_ms,
                    ..
                } => {
                    let detail = match &tally {
                        Tally::Skipped(reason) => format!("skipped: {}", reason.note()),
                        Tally::Counted { completion, .. } => {
                            completion.note().unwrap_or_else(|| "exact".to_string())
                        }
                    };
                    println!(
                        "\n== instances of {class}: {:>8}   ({detail}, {elapsed_ms} ms) ==",
                        tally.badge()
                    );
                }

                Response::Associations {
                    class,
                    associations,
                    completion,
                    elapsed_ms,
                    ..
                } => {
                    println!(
                        "\n== associations of {class}: {} in {elapsed_ms} ms{} ==",
                        associations.len(),
                        completion
                            .note()
                            .map(|n| format!(" [{n}]"))
                            .unwrap_or_default()
                    );
                    for a in &associations {
                        println!(
                            "  {:<40} {:<14} -> {:<40} {}",
                            if a.assoc_class.is_empty() {
                                "-"
                            } else {
                                &a.assoc_class
                            },
                            if a.role.is_empty() { "-" } else { &a.role },
                            a.target_class,
                            a.note
                        );
                    }
                }

                Response::Error {
                    context, message, ..
                } => eprintln!("\n!! ERROR [{context}]: {message}"),

                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }

    // A kind nobody set is indistinguishable from a class with no flags, so
    // assert the classification actually ran rather than trusting the print.
    debug_assert_ne!(ClassKind::NONE, ClassKind::ABSTRACT);
    println!("\ndone ({received}/{expected} responses)");
}
