//! Live event-monitor reality check.
//! Run with: `cargo run -p vmiscope-core --example monitor`

use vmiscope_core::{EventMonitor, MonitorMsg};

fn main() {
    let query =
        "SELECT * FROM __InstanceCreationEvent WITHIN 1 WHERE TargetInstance ISA 'Win32_Process'";
    println!("monitoring: {query}");
    let mon = EventMonitor::start("root\\cimv2".into(), query.into());

    // Trigger events with processes that live long enough to be sampled.
    let mut kids = Vec::new();
    for _ in 0..4 {
        if let Ok(c) = std::process::Command::new("ping")
            .args(["-n", "4", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
        {
            kids.push(c);
        }
        std::thread::sleep(std::time::Duration::from_millis(600));
    }

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut count = 0;
    while std::time::Instant::now() < deadline && count < 6 {
        for msg in mon.poll() {
            match msg {
                MonitorMsg::Event(pairs) => {
                    count += 1;
                    let name = pairs
                        .iter()
                        .find(|(k, _)| k.ends_with(".Name"))
                        .map(|(_, v)| v.as_str())
                        .unwrap_or("?");
                    println!("EVENT: {name}  ({} fields)", pairs.len());
                }
                MonitorMsg::Error(e) => eprintln!("ERR: {e}"),
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    println!("done ({count} events)");
}
