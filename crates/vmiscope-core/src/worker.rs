//! Background WMI worker.
//!
//! COM apartments are thread-affine, so all WMI work happens on a single
//! dedicated thread that owns the [`COMLibrary`]. The UI talks to it purely
//! through channels: it pushes [`Request`]s and drains [`Response`]s each
//! frame without ever blocking on WMI.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use wmi::{Variant, WMIConnection};

use crate::network::{tcp_state_name, Connection, NetworkSnapshot, Protocol};
use crate::value::{variant_to_string, variant_to_u32};

/// A unit of work for the WMI thread. `id` lets the UI correlate the reply
/// with the widget that asked (namespaces resolve out of order otherwise).
#[derive(Debug, Clone)]
pub enum Request {
    /// Enumerate the direct child namespaces of `namespace` (via `__NAMESPACE`).
    ListChildNamespaces { id: u64, namespace: String },
    /// Enumerate the class names defined in `namespace`.
    ListClasses { id: u64, namespace: String },
    /// Run an arbitrary WQL query in `namespace`.
    Query {
        id: u64,
        namespace: String,
        wql: String,
    },
    /// Take a snapshot of the live TCP/UDP connection table.
    NetworkSnapshot { id: u64 },
    /// Stop the worker thread.
    Shutdown,
}

/// A tabular query result: `columns` is the ordered union of property names,
/// `rows` are already stringified and aligned to `columns`.
#[derive(Debug, Clone, Default)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

/// A reply from the WMI thread. `id` echoes the originating request's `id`.
#[derive(Debug, Clone)]
pub enum Response {
    ChildNamespaces {
        id: u64,
        namespace: String,
        children: Vec<String>,
    },
    Classes {
        id: u64,
        namespace: String,
        classes: Vec<String>,
    },
    QueryResult {
        id: u64,
        namespace: String,
        wql: String,
        result: QueryResult,
    },
    Network {
        id: u64,
        snapshot: NetworkSnapshot,
    },
    Error {
        id: u64,
        context: String,
        message: String,
    },
}

/// Handle to the background WMI thread. Dropping it shuts the thread down.
pub struct WmiWorker {
    tx: Sender<Request>,
    rx: Receiver<Response>,
    handle: Option<JoinHandle<()>>,
}

impl WmiWorker {
    /// Spawn the worker thread and return a handle to it.
    pub fn spawn() -> Self {
        let (req_tx, req_rx) = mpsc::channel::<Request>();
        let (res_tx, res_rx) = mpsc::channel::<Response>();
        let handle = thread::Builder::new()
            .name("wmi-worker".into())
            .spawn(move || run(req_rx, res_tx))
            .expect("failed to spawn wmi worker thread");
        Self {
            tx: req_tx,
            rx: res_rx,
            handle: Some(handle),
        }
    }

    /// Queue a request. Non-blocking; the reply arrives later via [`WmiWorker::poll`].
    pub fn send(&self, req: Request) {
        let _ = self.tx.send(req);
    }

    /// Drain all currently available responses without blocking.
    pub fn poll(&self) -> Vec<Response> {
        self.rx.try_iter().collect()
    }
}

impl Drop for WmiWorker {
    fn drop(&mut self) {
        let _ = self.tx.send(Request::Shutdown);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// The worker thread's main loop.
///
/// `wmi` 0.18 initializes COM implicitly (an MTA via `CoIncrementMTAUsage`)
/// the first time a connection is created. [`WMIConnection`] is `!Send`, so
/// all connections are created and used here, never handed to another thread.
fn run(rx: Receiver<Request>, tx: Sender<Response>) {
    for req in rx {
        match req {
            Request::Shutdown => break,

            Request::ListChildNamespaces { id, namespace } => {
                let resp = match list_child_namespaces(&namespace) {
                    Ok(children) => Response::ChildNamespaces {
                        id,
                        namespace,
                        children,
                    },
                    Err(e) => Response::Error {
                        id,
                        context: format!("List namespaces under {namespace}"),
                        message: e.to_string(),
                    },
                };
                let _ = tx.send(resp);
            }

            Request::ListClasses { id, namespace } => {
                let resp = match list_classes(&namespace) {
                    Ok(classes) => Response::Classes {
                        id,
                        namespace,
                        classes,
                    },
                    Err(e) => Response::Error {
                        id,
                        context: format!("List classes in {namespace}"),
                        message: e.to_string(),
                    },
                };
                let _ = tx.send(resp);
            }

            Request::Query { id, namespace, wql } => {
                let resp = match run_query(&namespace, &wql) {
                    Ok(result) => Response::QueryResult {
                        id,
                        namespace,
                        wql,
                        result,
                    },
                    Err(e) => Response::Error {
                        id,
                        context: format!("Query in {namespace}: {wql}"),
                        message: e.to_string(),
                    },
                };
                let _ = tx.send(resp);
            }

            Request::NetworkSnapshot { id } => {
                let resp = match list_connections() {
                    Ok(snapshot) => Response::Network { id, snapshot },
                    Err(e) => Response::Error {
                        id,
                        context: "Network snapshot".into(),
                        message: e.to_string(),
                    },
                };
                let _ = tx.send(resp);
            }
        }
    }
}

/// Connect to a namespace. Kept per-request for simplicity; connections are
/// cheap relative to the user-driven cadence of an explorer.
fn connect(namespace: &str) -> anyhow::Result<WMIConnection> {
    Ok(WMIConnection::with_namespace_path(namespace)?)
}

fn list_child_namespaces(namespace: &str) -> anyhow::Result<Vec<String>> {
    let conn = connect(namespace)?;
    let rows: Vec<HashMap<String, Variant>> = conn.raw_query("SELECT Name FROM __NAMESPACE")?;
    let mut names: Vec<String> = rows
        .into_iter()
        .filter_map(|mut r| r.remove("Name"))
        .map(|v| variant_to_string(&v))
        .filter(|s| !s.is_empty())
        .map(|child| format!("{namespace}\\{child}"))
        .collect();
    names.sort_unstable();
    names.dedup();
    Ok(names)
}

fn list_classes(namespace: &str) -> anyhow::Result<Vec<String>> {
    let conn = connect(namespace)?;
    // `meta_class` enumerates class *definitions*. The generic `HashMap` path
    // hides WMI system properties (`WBEM_FLAG_NONSYSTEM_ONLY`), so we drop to
    // the low-level `exec_query` and read each object's `__Class` directly via
    // the reflective wrapper.
    let mut classes: Vec<String> = Vec::new();
    for item in conn.exec_query("SELECT * FROM meta_class")? {
        let obj = item?;
        if let Ok(name) = obj.class() {
            if !name.is_empty() {
                classes.push(name);
            }
        }
    }
    classes.sort_unstable();
    classes.dedup();
    Ok(classes)
}

fn run_query(namespace: &str, wql: &str) -> anyhow::Result<QueryResult> {
    let conn = connect(namespace)?;
    let rows: Vec<HashMap<String, Variant>> = conn.raw_query(wql)?;
    Ok(to_table(rows))
}

/// Flatten a list of property maps into a column-aligned table. Columns are
/// the sorted union of every row's keys so sparse properties still line up.
fn to_table(rows: Vec<HashMap<String, Variant>>) -> QueryResult {
    let mut columns: Vec<String> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for row in &rows {
        for key in row.keys() {
            if seen.insert(key.clone()) {
                columns.push(key.clone());
            }
        }
    }
    columns.sort_unstable();

    let table_rows: Vec<Vec<String>> = rows
        .into_iter()
        .map(|row| {
            columns
                .iter()
                .map(|c| row.get(c).map(variant_to_string).unwrap_or_default())
                .collect()
        })
        .collect();

    QueryResult {
        columns,
        rows: table_rows,
    }
}

/// Snapshot the TCP/UDP connection table and resolve owning process names.
///
/// TCP/UDP tables live in `root\StandardCimv2`; process names come from
/// `Win32_Process` in `root\CIMV2`. A missing endpoint class (older Windows)
/// degrades gracefully rather than failing the whole snapshot.
fn list_connections() -> anyhow::Result<NetworkSnapshot> {
    // PID -> process name.
    let cimv2 = connect("root\\CIMV2")?;
    let procs: Vec<HashMap<String, Variant>> =
        cimv2.raw_query("SELECT ProcessId, Name FROM Win32_Process")?;
    let mut pid_name: HashMap<u32, String> = HashMap::new();
    for p in procs {
        let pid = p.get("ProcessId").map(variant_to_u32).unwrap_or(0);
        if pid != 0 {
            let name = p.get("Name").map(variant_to_string).unwrap_or_default();
            pid_name.insert(pid, name);
        }
    }
    let name_of = |pid: u32| pid_name.get(&pid).cloned().unwrap_or_default();

    let net = connect("root\\StandardCimv2")?;
    let mut connections = Vec::new();

    // TCP connections.
    if let Ok(rows) = net.raw_query::<HashMap<String, Variant>>(
        "SELECT LocalAddress,LocalPort,RemoteAddress,RemotePort,State,OwningProcess \
         FROM MSFT_NetTCPConnection",
    ) {
        for r in rows {
            let pid = r.get("OwningProcess").map(variant_to_u32).unwrap_or(0);
            let state = tcp_state_name(r.get("State").map(variant_to_u32).unwrap_or(0)).to_string();
            connections.push(Connection {
                proto: Protocol::Tcp,
                local_addr: r
                    .get("LocalAddress")
                    .map(variant_to_string)
                    .unwrap_or_default(),
                local_port: r.get("LocalPort").map(variant_to_u32).unwrap_or(0) as u16,
                remote_addr: r
                    .get("RemoteAddress")
                    .map(variant_to_string)
                    .unwrap_or_default(),
                remote_port: r.get("RemotePort").map(variant_to_u32).unwrap_or(0) as u16,
                state,
                pid,
                process: name_of(pid),
            });
        }
    }

    // UDP endpoints (connectionless: no remote/state).
    if let Ok(rows) = net.raw_query::<HashMap<String, Variant>>(
        "SELECT LocalAddress,LocalPort,OwningProcess FROM MSFT_NetUDPEndpoint",
    ) {
        for r in rows {
            let pid = r.get("OwningProcess").map(variant_to_u32).unwrap_or(0);
            connections.push(Connection {
                proto: Protocol::Udp,
                local_addr: r
                    .get("LocalAddress")
                    .map(variant_to_string)
                    .unwrap_or_default(),
                local_port: r.get("LocalPort").map(variant_to_u32).unwrap_or(0) as u16,
                remote_addr: String::new(),
                remote_port: 0,
                state: String::new(),
                pid,
                process: name_of(pid),
            });
        }
    }

    Ok(NetworkSnapshot { connections })
}
