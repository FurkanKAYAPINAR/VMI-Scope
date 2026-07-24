//! Live WMI event monitor.
//!
//! A WMI notification query (`SELECT ... FROM __InstanceCreationEvent WITHIN n
//! WHERE ...`) blocks until an event arrives, so the monitor runs on its **own**
//! dedicated thread with its own COM connection and streams events back over a
//! channel. It never touches the main worker thread or the UI thread.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use wmi::{IWbemClassWrapper, Variant, WMIConnection};

use crate::value::variant_to_string;

/// A useful default: fires within ~2s of any process starting.
pub const DEFAULT_EVENT_QUERY: &str =
    "SELECT * FROM __InstanceCreationEvent WITHIN 2 WHERE TargetInstance ISA 'Win32_Process'";

/// A message from the monitor thread.
#[derive(Debug, Clone)]
pub enum MonitorMsg {
    /// One event, flattened to `(field, value)` pairs.
    Event(Vec<(String, String)>),
    Error(String),
}

/// Curated fields pulled out of an embedded `TargetInstance` object.
const EMBEDDED_FIELDS: [&str; 6] = [
    "Name",
    "ProcessId",
    "ParentProcessId",
    "ExecutablePath",
    "CommandLine",
    "Caption",
];

/// Flatten an event object into readable `(field, value)` pairs, drilling one
/// level into embedded objects (e.g. `TargetInstance`).
///
/// We read through the reflective wrapper rather than deserializing into a map,
/// because the `HashMap<String, Variant>` path rejects embedded objects.
fn flatten_event(wrapper: &IWbemClassWrapper) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for name in wrapper.list_properties().unwrap_or_default() {
        match wrapper.get_property(&name) {
            Ok(Variant::Object(inner)) => {
                for p in EMBEDDED_FIELDS {
                    if let Ok(pv) = inner.get_property(p) {
                        let s = variant_to_string(&pv);
                        if !s.is_empty() {
                            out.push((format!("{name}.{p}"), s));
                        }
                    }
                }
            }
            Ok(v) => {
                let s = variant_to_string(&v);
                if !s.is_empty() && !s.starts_with('<') {
                    out.push((name, s));
                }
            }
            Err(_) => {}
        }
    }
    out.sort();
    out
}

/// Handle to a running event monitor. Dropping it signals the thread to stop.
pub struct EventMonitor {
    rx: Receiver<MonitorMsg>,
    stop: Arc<AtomicBool>,
    _handle: Option<JoinHandle<()>>,
}

impl EventMonitor {
    /// Start monitoring `wql` in `namespace` on a fresh thread.
    pub fn start(namespace: String, wql: String) -> EventMonitor {
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        let handle = thread::Builder::new()
            .name("wmi-events".into())
            .spawn(move || {
                let conn = match WMIConnection::with_namespace_path(&namespace) {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = tx.send(MonitorMsg::Error(e.to_string()));
                        return;
                    }
                };
                let iter = match conn.exec_notification_query(&wql) {
                    Ok(i) => i,
                    Err(e) => {
                        let _ = tx.send(MonitorMsg::Error(e.to_string()));
                        return;
                    }
                };
                for ev in iter {
                    if stop_thread.load(Ordering::Relaxed) {
                        break;
                    }
                    let msg = match ev {
                        Ok(wrapper) => MonitorMsg::Event(flatten_event(&wrapper)),
                        Err(e) => MonitorMsg::Error(e.to_string()),
                    };
                    if tx.send(msg).is_err() {
                        break; // receiver dropped
                    }
                }
            })
            .expect("failed to spawn event monitor thread");
        EventMonitor {
            rx,
            stop,
            _handle: Some(handle),
        }
    }

    /// Drain any events received since the last poll.
    pub fn poll(&self) -> Vec<MonitorMsg> {
        self.rx.try_iter().collect()
    }
}

impl Drop for EventMonitor {
    fn drop(&mut self) {
        // Signal stop. We deliberately do NOT join: the notification `Next()`
        // can block until the next event; the thread exits on its own once one
        // arrives (or when the process ends). No UI stall either way.
        self.stop.store(true, Ordering::Relaxed);
    }
}
