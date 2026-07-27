//! Background WMI worker.
//!
//! COM apartments are thread-affine, so all WMI work happens on a single
//! dedicated thread that owns the [`COMLibrary`]. The UI talks to it purely
//! through channels: it pushes [`Request`]s and drains [`Response`]s each
//! frame without ever blocking on WMI.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows::Win32::System::Wmi::IWbemClassObject;
use wmi::{Variant, WMIConnection};

use crate::enumerate::{self, CancelToken, Completion, DirectConn, WorkerControl};
use crate::events::{assess, first_quoted, Risk, Subscription, SubscriptionReport};
use crate::method::{MethodArg, MethodOutcome, MethodTarget};
use crate::network::{tcp_state_name, Connection, NetworkSnapshot, Protocol};
use crate::providers::ProviderInfo;
use crate::remote::{Credential, RemoteConn};
use crate::schema::{ClassSchema, SearchIndex};
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
    ///
    /// `max_rows` caps the result. WQL has no `TOP`/`LIMIT`, so an unbounded
    /// `SELECT * FROM CIM_DataFile` really does walk the whole filesystem.
    ///
    /// `timeout` bounds it in the other dimension, and both are needed. A row
    /// cap only bites once rows arrive, and some providers deliver none for a
    /// very long time: measured on this machine, `Win32_Process` capped at 5
    /// answers in 36 ms, while `CIM_DataFile` capped at 200 returns *nothing*
    /// in 45 s, because that provider materialises its whole result before
    /// yielding the first row. With no deadline the only remaining escape is
    /// the user noticing and pressing cancel.
    ///
    /// A partial result always says why -- [`Completion::Truncated`],
    /// [`Completion::TimedOut`] or [`Completion::Cancelled`] -- rather than
    /// pretending to be whole.
    Query {
        id: u64,
        namespace: String,
        wql: String,
        max_rows: Option<usize>,
        timeout: Option<Duration>,
    },
    /// Take a snapshot of the live TCP/UDP connection table.
    NetworkSnapshot { id: u64 },
    /// Enumerate permanent WMI event subscriptions (persistence hunt).
    ListEventSubscriptions { id: u64 },
    /// Enumerate WMI providers and their host processes.
    ListProviders { id: u64 },
    /// Reflect the full schema (properties, qualifiers, methods) of a class.
    ClassSchema {
        id: u64,
        namespace: String,
        class: String,
    },
    /// Fetch the MOF text of a class or instance.
    ClassMof {
        id: u64,
        namespace: String,
        object_path: String,
    },
    /// List instances of a class as method-invocation targets.
    ListInstances {
        id: u64,
        namespace: String,
        class: String,
    },
    /// Invoke a WMI method (mutating; gated by the GUI).
    InvokeMethod {
        id: u64,
        namespace: String,
        class: String,
        object_path: String,
        method: String,
        is_static: bool,
        args: Vec<MethodArg>,
    },
    /// Build a class/property(/method) name index for global search.
    BuildSearchIndex {
        id: u64,
        namespace: String,
        include_methods: bool,
    },
    /// Point all subsequent connections at `host`, back to the local machine
    /// with `None`. With `cred`, authenticate using alternate credentials
    /// (raw DCOM); without, connect as the current user (SSO).
    SetHost {
        id: u64,
        host: Option<String>,
        cred: Option<Credential>,
    },
    /// Stop the enumeration running under `id`.
    ///
    /// [`WmiWorker::send`] raises the flag *before* queueing this message,
    /// because a `Cancel` that waited its turn in the channel would be waiting
    /// behind exactly the query it exists to stop.
    Cancel { id: u64 },
    /// Stop the worker thread.
    ///
    /// Like `Cancel`, the effect comes from a flag raised before the send, not
    /// from the message itself.
    Shutdown,
}

/// A tabular query result: `columns` is the ordered union of property names,
/// `rows` are already stringified and aligned to `columns`.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    /// Milliseconds spent binding the namespace.
    ///
    /// Reported apart from `elapsed_ms` because the bind happens on *every*
    /// request: folded together, a 3 ms query on a 40 ms connection would read
    /// as a 43 ms query and every timing shown to a user would be a lie.
    pub connect_ms: u64,
    /// Milliseconds spent enumerating, with the bind above excluded.
    pub elapsed_ms: u64,
    /// Why the enumeration stopped — whole, capped, or cancelled.
    pub completion: Completion,
}

/// A reply from the WMI thread. `id` echoes the originating request's `id`.
///
/// Where a variant carries `elapsed_ms`, it is the wall time of the whole
/// operation *including* the namespace bind. Only [`Response::QueryResult`]
/// splits the two, because only the query path has a single bind to attribute
/// the cost to — the security scans bind several namespaces each, so one
/// "connect" figure would be meaningless.
#[derive(Debug, Clone)]
pub enum Response {
    ChildNamespaces {
        id: u64,
        namespace: String,
        children: Vec<String>,
        elapsed_ms: u64,
    },
    Classes {
        id: u64,
        namespace: String,
        classes: Vec<String>,
        elapsed_ms: u64,
    },
    QueryResult {
        id: u64,
        namespace: String,
        wql: String,
        /// Carries its own `connect_ms` / `elapsed_ms` / `completion`.
        result: QueryResult,
    },
    Network {
        id: u64,
        snapshot: NetworkSnapshot,
        elapsed_ms: u64,
    },
    EventSubscriptions {
        id: u64,
        report: SubscriptionReport,
        elapsed_ms: u64,
    },
    Providers {
        id: u64,
        providers: Vec<ProviderInfo>,
        elapsed_ms: u64,
    },
    Schema {
        id: u64,
        namespace: String,
        class: String,
        schema: ClassSchema,
        elapsed_ms: u64,
    },
    Mof {
        id: u64,
        object_path: String,
        mof: String,
    },
    Instances {
        id: u64,
        class: String,
        targets: Vec<MethodTarget>,
    },
    MethodDone {
        id: u64,
        class: String,
        method: String,
        outcome: MethodOutcome,
    },
    SearchIndex {
        id: u64,
        index: SearchIndex,
        elapsed_ms: u64,
    },
    HostConnected {
        id: u64,
        host: Option<String>,
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
    control: WorkerControl,
    handle: Option<JoinHandle<()>>,
}

impl WmiWorker {
    /// Spawn the worker thread and return a handle to it.
    pub fn spawn() -> Self {
        let (req_tx, req_rx) = mpsc::channel::<Request>();
        let (res_tx, res_rx) = mpsc::channel::<Response>();
        let control = WorkerControl::new();
        let worker_control = control.clone();
        let handle = thread::Builder::new()
            .name("wmi-worker".into())
            .spawn(move || run(req_rx, res_tx, worker_control))
            .expect("failed to spawn wmi worker thread");
        Self {
            tx: req_tx,
            rx: res_rx,
            control,
            handle: Some(handle),
        }
    }

    /// Queue a request. Non-blocking; the reply arrives later via [`WmiWorker::poll`].
    ///
    /// [`Request::Cancel`] and [`Request::Shutdown`] additionally raise their
    /// flag before the send. Both exist to interrupt work that is *already
    /// running*, and a message sitting in a FIFO behind that work cannot do it.
    pub fn send(&self, req: Request) {
        match &req {
            Request::Cancel { id } => self.control.cancel(*id),
            Request::Shutdown => self.control.shutdown(),
            _ => {}
        }
        let _ = self.tx.send(req);
    }

    /// Cancel the request with `id`; equivalent to sending [`Request::Cancel`].
    ///
    /// A cancelled query still replies, with the rows it had and
    /// [`Completion::Cancelled`] — silence would strand the caller's spinner.
    pub fn cancel(&self, id: u64) {
        self.send(Request::Cancel { id });
    }

    /// Drain all currently available responses without blocking.
    pub fn poll(&self) -> Vec<Response> {
        self.rx.try_iter().collect()
    }
}

impl Drop for WmiWorker {
    fn drop(&mut self) {
        // Flag first, message second. `Drop` joins the thread, so if the
        // shutdown had to travel through the request channel it would land
        // behind whatever is running -- and closing the window during a
        // `CIM_DataFile` query would hang the application until the filesystem
        // walk finished. The flag is read every batch instead.
        self.control.shutdown();
        let _ = self.tx.send(Request::Shutdown);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// Milliseconds elapsed since `start`, saturating into `u64`.
fn ms(start: Instant) -> u64 {
    start.elapsed().as_millis() as u64
}

/// The worker thread's main loop.
///
/// `wmi` 0.18 initializes COM implicitly (an MTA via `CoIncrementMTAUsage`)
/// the first time a connection is created. [`WMIConnection`] is `!Send`, so
/// all connections are created and used here, never handed to another thread.
fn run(rx: Receiver<Request>, tx: Sender<Response>, control: WorkerControl) {
    for req in rx {
        // The flag, not the message, is what makes shutdown prompt: by the
        // time `Request::Shutdown` reaches the front of the queue there may be
        // a hundred requests ahead of it that were sent first.
        if control.is_shutdown() {
            break;
        }

        match req {
            Request::Shutdown => break,

            // The work was already done by `WmiWorker::send`, which raised the
            // flag. Arriving here only means the request has drained past, so
            // the flag can be forgotten -- otherwise cancelling an id that had
            // already finished would leave an entry behind for good.
            Request::Cancel { id } => control.end(id),

            Request::ListChildNamespaces { id, namespace } => {
                let t0 = Instant::now();
                let resp = match list_child_namespaces(&namespace) {
                    Ok(children) => Response::ChildNamespaces {
                        id,
                        namespace,
                        children,
                        elapsed_ms: ms(t0),
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
                let t0 = Instant::now();
                let resp = match q_class_names(&namespace, &control) {
                    Ok(classes) => Response::Classes {
                        id,
                        namespace,
                        classes,
                        elapsed_ms: ms(t0),
                    },
                    Err(e) => Response::Error {
                        id,
                        context: format!("List classes in {namespace}"),
                        message: e.to_string(),
                    },
                };
                let _ = tx.send(resp);
            }

            Request::Query {
                id,
                namespace,
                wql,
                max_rows,
                timeout,
            } => {
                let cancel = control.begin(id);
                let outcome = run_query(&namespace, &wql, max_rows, timeout, &cancel);
                control.end(id);
                let resp = match outcome {
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
                let t0 = Instant::now();
                let resp = match list_connections() {
                    Ok(snapshot) => Response::Network {
                        id,
                        snapshot,
                        elapsed_ms: ms(t0),
                    },
                    Err(e) => Response::Error {
                        id,
                        context: "Network snapshot".into(),
                        message: e.to_string(),
                    },
                };
                let _ = tx.send(resp);
            }

            Request::ListEventSubscriptions { id } => {
                let t0 = Instant::now();
                let resp = match list_event_subscriptions() {
                    Ok(report) => Response::EventSubscriptions {
                        id,
                        report,
                        elapsed_ms: ms(t0),
                    },
                    Err(e) => Response::Error {
                        id,
                        context: "Enumerate event subscriptions".into(),
                        message: e.to_string(),
                    },
                };
                let _ = tx.send(resp);
            }

            Request::ListProviders { id } => {
                let t0 = Instant::now();
                let resp = match list_providers() {
                    Ok(providers) => Response::Providers {
                        id,
                        providers,
                        elapsed_ms: ms(t0),
                    },
                    Err(e) => Response::Error {
                        id,
                        context: "Enumerate WMI providers".into(),
                        message: e.to_string(),
                    },
                };
                let _ = tx.send(resp);
            }

            Request::ClassSchema {
                id,
                namespace,
                class,
            } => {
                let t0 = Instant::now();
                let resp = match connect(&namespace)
                    .and_then(|c| crate::reflect::read_class_schema(&c, &class))
                {
                    Ok(schema) => Response::Schema {
                        id,
                        namespace,
                        class,
                        schema,
                        elapsed_ms: ms(t0),
                    },
                    Err(e) => Response::Error {
                        id,
                        context: format!("Reflect schema of {class}"),
                        message: e.to_string(),
                    },
                };
                let _ = tx.send(resp);
            }

            Request::ClassMof {
                id,
                namespace,
                object_path,
            } => {
                let resp = match connect(&namespace)
                    .and_then(|c| crate::reflect::class_mof(&c, &object_path))
                {
                    Ok(mof) => Response::Mof {
                        id,
                        object_path,
                        mof,
                    },
                    Err(e) => Response::Error {
                        id,
                        context: format!("MOF of {object_path}"),
                        message: e.to_string(),
                    },
                };
                let _ = tx.send(resp);
            }

            Request::ListInstances {
                id,
                namespace,
                class,
            } => {
                let resp = match connect(&namespace)
                    .and_then(|c| crate::method::list_instances(&c, &class))
                {
                    Ok(targets) => Response::Instances { id, class, targets },
                    Err(e) => Response::Error {
                        id,
                        context: format!("List instances of {class}"),
                        message: e.to_string(),
                    },
                };
                let _ = tx.send(resp);
            }

            Request::InvokeMethod {
                id,
                namespace,
                class,
                object_path,
                method,
                is_static,
                args,
            } => {
                let resp = match connect(&namespace).and_then(|c| {
                    crate::method::invoke_method(
                        &c,
                        &class,
                        &object_path,
                        &method,
                        is_static,
                        &args,
                    )
                }) {
                    Ok(outcome) => Response::MethodDone {
                        id,
                        class,
                        method,
                        outcome,
                    },
                    Err(e) => Response::Error {
                        id,
                        context: format!("Invoke {class}.{method}"),
                        message: e.to_string(),
                    },
                };
                let _ = tx.send(resp);
            }

            Request::BuildSearchIndex {
                id,
                namespace,
                include_methods,
            } => {
                let t0 = Instant::now();
                let resp = match build_search_index(&namespace, include_methods, &control) {
                    Ok(index) => Response::SearchIndex {
                        id,
                        index,
                        elapsed_ms: ms(t0),
                    },
                    Err(e) => Response::Error {
                        id,
                        context: format!("Build search index for {namespace}"),
                        message: e.to_string(),
                    },
                };
                let _ = tx.send(resp);
            }

            Request::SetHost { id, host, cred } => {
                HOST.with(|h| *h.borrow_mut() = host.clone());
                CRED.with(|c| *c.borrow_mut() = cred.clone());
                REMOTE.with(|m| m.borrow_mut().clear());
                // Verify the target is reachable (this also exercises the
                // credential path — bogus creds fail here).
                let probe = q_maps("root\\CIMV2", "SELECT Name FROM Win32_ComputerSystem");
                let resp = match probe {
                    Ok(_) => Response::HostConnected { id, host },
                    Err(e) => {
                        // Revert to local so the app stays usable.
                        HOST.with(|h| *h.borrow_mut() = None);
                        CRED.with(|c| *c.borrow_mut() = None);
                        REMOTE.with(|m| m.borrow_mut().clear());
                        Response::Error {
                            id,
                            context: "Connect to host".into(),
                            message: e.to_string(),
                        }
                    }
                };
                let _ = tx.send(resp);
            }
        }
    }
}

thread_local! {
    /// The remote host all connections target, or `None` for the local machine.
    /// Thread-local because it lives entirely on the (single) worker thread.
    static HOST: RefCell<Option<String>> = const { RefCell::new(None) };
    /// Alternate credentials for the remote host, if any (else current-user SSO).
    static CRED: RefCell<Option<Credential>> = const { RefCell::new(None) };
    /// Per-namespace raw-DCOM connections, used only in alternate-credential mode.
    static REMOTE: RefCell<HashMap<String, RemoteConn>> = RefCell::new(HashMap::new());
}

/// Connect to a namespace on the current target host. When a host is set the
/// connection is made as the *current user* (SSO) — see [`Request::SetHost`].
/// Kept per-request for simplicity; connections are cheap relative to the
/// user-driven cadence of an explorer.
fn connect(namespace: &str) -> anyhow::Result<WMIConnection> {
    match current_host() {
        Some(server) => Ok(WMIConnection::with_credentials_and_namespace(
            &server, namespace, None, None, None,
        )?),
        None => Ok(WMIConnection::with_namespace_path(namespace)?),
    }
}

/// The host all connections currently target, or `None` for this machine.
fn current_host() -> Option<String> {
    HOST.with(|h| h.borrow().clone())
}

/// Are we in alternate-credential mode (raw DCOM), or local/SSO (`wmi` crate)?
fn is_alt_cred() -> bool {
    CRED.with(|c| c.borrow().is_some())
}

/// Run a closure against the cached raw-DCOM connection for `namespace`.
fn with_remote<T>(
    namespace: &str,
    f: impl FnOnce(&RemoteConn) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    REMOTE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if !cache.contains_key(namespace) {
            let host = HOST
                .with(|h| h.borrow().clone())
                .ok_or_else(|| anyhow::anyhow!("alternate credentials require a host"))?;
            let cred = CRED
                .with(|c| c.borrow().clone())
                .ok_or_else(|| anyhow::anyhow!("no credentials set"))?;
            cache.insert(
                namespace.to_string(),
                RemoteConn::connect(&host, namespace, &cred)?,
            );
        }
        f(cache.get(namespace).unwrap())
    })
}

/// Run a WQL query, returning raw property maps. Dispatches local/SSO
/// (`wmi` crate) vs alternate-credential (raw DCOM).
fn q_maps(namespace: &str, wql: &str) -> anyhow::Result<Vec<HashMap<String, Variant>>> {
    if is_alt_cred() {
        with_remote(namespace, |r| r.exec_maps(wql))
    } else {
        Ok(connect(namespace)?.raw_query(wql)?)
    }
}

/// Make sure the raw-DCOM connection for `namespace` exists.
///
/// Called before timing an alternate-credential query so the cost of building
/// the connection lands in `connect_ms` instead of being charged to the query.
fn ensure_remote(namespace: &str) -> anyhow::Result<()> {
    with_remote(namespace, |_| Ok(()))
}

/// Enumerate class names, dispatching local/SSO vs alternate-credential.
fn q_class_names(namespace: &str, control: &WorkerControl) -> anyhow::Result<Vec<String>> {
    if is_alt_cred() {
        with_remote(namespace, |r| r.list_class_names())
    } else {
        list_classes_local(namespace, control)
    }
}

fn list_child_namespaces(namespace: &str) -> anyhow::Result<Vec<String>> {
    let rows = q_maps(namespace, "SELECT Name FROM __NAMESPACE")?;
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

fn list_classes_local(namespace: &str, control: &WorkerControl) -> anyhow::Result<Vec<String>> {
    let conn = connect(namespace)?;
    // `meta_class` enumerates class *definitions*. The generic `HashMap` path
    // hides WMI system properties (`WBEM_FLAG_NONSYSTEM_ONLY`), so we drop to
    // the low-level `exec_query` and read each object's `__Class` directly via
    // the reflective wrapper.
    let mut classes: Vec<String> = Vec::new();
    for item in conn.exec_query("SELECT * FROM meta_class")? {
        // This iterator is the `wmi` crate's, which pulls one object per
        // `Next(WBEM_INFINITE, ..)`, so it cannot be chunked from here. It is
        // bounded (~1,400 classes in `root\CIMV2`) but not instant, and exit
        // should not wait for it either.
        if control.is_shutdown() {
            anyhow::bail!("worker is shutting down");
        }
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

/// Run a WQL query as a chunked, cancellable enumeration.
///
/// Two things separate this from the old one-shot `raw_query` path. It pulls
/// objects in batches with a finite per-batch timeout, so the request can be
/// cancelled and the worker can be told to exit mid-query; and it honours
/// `max_rows`, reporting truncation as a fact rather than handing back a short
/// table that looks complete.
///
/// The bind is timed separately from the enumeration — see
/// [`QueryResult::connect_ms`].
fn run_query(
    namespace: &str,
    wql: &str,
    max_rows: Option<usize>,
    deadline: Option<Duration>,
    cancel: &CancelToken,
) -> anyhow::Result<QueryResult> {
    // Both transports flatten an object identically; only the way the
    // enumerator is obtained differs.
    let to_map = |obj: &IWbemClassObject| unsafe { crate::remote::object_to_map(obj) };
    let alt_cred = is_alt_cred();

    let t_connect = Instant::now();
    let direct = if alt_cred {
        ensure_remote(namespace)?;
        None
    } else {
        Some(DirectConn::open(current_host().as_deref(), namespace)?)
    };
    let connect_ms = ms(t_connect);

    let t_exec = Instant::now();
    let (rows, completion) = match &direct {
        Some(conn) => {
            let en = conn.exec_enum(wql)?;
            enumerate::drain(&en, max_rows, deadline, cancel, to_map)?
        }
        None => with_remote(namespace, |r| {
            let en = r.exec_enum(wql)?;
            enumerate::drain(&en, max_rows, deadline, cancel, to_map)
        })?,
    };
    let elapsed_ms = ms(t_exec);

    let mut table = to_table(rows);
    table.connect_ms = connect_ms;
    table.elapsed_ms = elapsed_ms;
    table.completion = completion;
    Ok(table)
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
        ..Default::default()
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

    let mut connections = Vec::new();

    // TCP connections.
    if let Ok(rows) = q_maps(
        "root\\StandardCimv2",
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
    if let Ok(rows) = q_maps(
        "root\\StandardCimv2",
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

/// Enumerate permanent WMI event subscriptions and score each for persistence.
///
/// Scans `root\subscription` (the primary home) **and** `root\default` (a classic
/// hiding spot), walks each `__FilterToConsumerBinding`, and additionally
/// surfaces **orphan** filters/consumers — objects staged without a binding, a
/// known evasion against binding-only tools.
fn list_event_subscriptions() -> anyhow::Result<SubscriptionReport> {
    let mut subscriptions = Vec::new();
    for ns in ["root\\subscription", "root\\default"] {
        subscriptions.extend(scan_subscriptions_in(ns));
    }
    // Most-suspicious first.
    subscriptions.sort_by(|a, b| b.risk.cmp(&a.risk));
    Ok(SubscriptionReport { subscriptions })
}

fn scan_subscriptions_in(namespace: &str) -> Vec<Subscription> {
    // Filters: Name -> query.
    let mut filters: HashMap<String, String> = HashMap::new();
    if let Ok(rows) = q_maps(namespace, "SELECT Name, Query FROM __EventFilter") {
        for r in rows {
            let name = r.get("Name").map(variant_to_string).unwrap_or_default();
            if !name.is_empty() {
                let query = r.get("Query").map(variant_to_string).unwrap_or_default();
                filters.insert(name, query);
            }
        }
    }

    // Consumers: Name -> (concrete class, best-effort action). Read via the
    // reflective wrapper (the class is a system property). Local/SSO only.
    let mut consumers: HashMap<String, (String, String)> = HashMap::new();
    if !is_alt_cred() {
        if let Ok(conn) = connect(namespace) {
            if let Ok(iter) = conn.exec_query("SELECT * FROM __EventConsumer") {
                for item in iter.flatten() {
                    let class = item.class().unwrap_or_default();
                    let get = |p: &str| {
                        item.get_property(p)
                            .ok()
                            .map(|v| variant_to_string(&v))
                            .unwrap_or_default()
                    };
                    let name = get("Name");
                    if name.is_empty() {
                        continue;
                    }
                    let action = [
                        "CommandLineTemplate",
                        "ExecutablePath",
                        "ScriptFileName",
                        "ScriptText",
                    ]
                    .into_iter()
                    .map(&get)
                    .find(|s| !s.is_empty())
                    .unwrap_or_default();
                    consumers.insert(name, (class, action));
                }
            }
        }
    }

    let mut subs = Vec::new();
    let mut bound_filters = std::collections::HashSet::new();
    let mut bound_consumers = std::collections::HashSet::new();

    // Bindings.
    if let Ok(rows) = q_maps(
        namespace,
        "SELECT Filter, Consumer FROM __FilterToConsumerBinding",
    ) {
        for r in rows {
            let filter_name =
                first_quoted(&r.get("Filter").map(variant_to_string).unwrap_or_default());
            let consumer_name =
                first_quoted(&r.get("Consumer").map(variant_to_string).unwrap_or_default());
            bound_filters.insert(filter_name.clone());
            bound_consumers.insert(consumer_name.clone());
            let filter_query = filters.get(&filter_name).cloned().unwrap_or_default();
            let (consumer_type, action) =
                consumers.get(&consumer_name).cloned().unwrap_or_default();
            let (risk, reasons) = assess(&consumer_type, &filter_query, &action);
            subs.push(Subscription {
                filter_name,
                filter_query,
                consumer_type,
                consumer_name,
                action,
                risk,
                reasons,
                bound: true,
            });
        }
    }

    // Orphan consumers — staged code with no binding (evasion signal).
    for (name, (class, action)) in &consumers {
        if !bound_consumers.contains(name) {
            let (risk, mut reasons) = assess(class, "", action);
            reasons.insert(0, "UNBOUND consumer (staged, no binding)".into());
            subs.push(Subscription {
                filter_name: String::new(),
                filter_query: String::new(),
                consumer_type: class.clone(),
                consumer_name: name.clone(),
                action: action.clone(),
                risk: risk.max(Risk::Medium),
                reasons,
                bound: false,
            });
        }
    }

    // Orphan filters — present but unused.
    for (name, query) in &filters {
        if !bound_filters.contains(name) {
            subs.push(Subscription {
                filter_name: name.clone(),
                filter_query: query.clone(),
                consumer_type: String::new(),
                consumer_name: String::new(),
                action: String::new(),
                risk: Risk::Low,
                reasons: vec!["unbound filter (no binding)".into()],
                bound: false,
            });
        }
    }

    subs
}

/// PID → process name, from `Win32_Process`.
fn process_names() -> anyhow::Result<HashMap<u32, String>> {
    let procs = q_maps("root\\CIMV2", "SELECT ProcessId, Name FROM Win32_Process")?;
    let mut map = HashMap::new();
    for p in procs {
        let pid = p.get("ProcessId").map(variant_to_u32).unwrap_or(0);
        if pid != 0 {
            map.insert(
                pid,
                p.get("Name").map(variant_to_string).unwrap_or_default(),
            );
        }
    }
    Ok(map)
}

/// Build a class/property(/method) name index for a namespace (for search).
fn build_search_index(
    namespace: &str,
    include_methods: bool,
    control: &WorkerControl,
) -> anyhow::Result<SearchIndex> {
    let conn = connect(namespace)?;
    let mut index = SearchIndex {
        namespace: namespace.to_string(),
        has_methods: include_methods,
        ..Default::default()
    };
    for item in conn.exec_query("SELECT * FROM meta_class")? {
        // The longest-running request that is not a query: reflecting every
        // class in a namespace. Same caveat as `list_classes_local` -- the
        // `wmi` iterator cannot be chunked, so the check is per object.
        if control.is_shutdown() {
            anyhow::bail!("worker is shutting down");
        }
        let Ok(obj) = item else { continue };
        let class = obj.class().unwrap_or_default();
        if class.is_empty() {
            continue;
        }
        index
            .properties
            .insert(class.clone(), obj.list_properties().unwrap_or_default());
        if include_methods {
            let methods = crate::reflect::enum_method_names(&obj.inner);
            if !methods.is_empty() {
                index.methods.insert(class.clone(), methods);
            }
        }
        index.classes.push(class);
    }
    index.classes.sort_unstable();
    Ok(index)
}

/// List WMI providers (`Msft_Providers`) and the processes hosting them.
fn list_providers() -> anyhow::Result<Vec<ProviderInfo>> {
    let names = process_names().unwrap_or_default();
    let rows = q_maps(
        "root\\CIMV2",
        "SELECT provider, Namespace, HostProcessIdentifier, HostingGroup FROM Msft_Providers",
    )?;
    let mut providers: Vec<ProviderInfo> = rows
        .into_iter()
        .map(|r| {
            let host_pid = r
                .get("HostProcessIdentifier")
                .map(variant_to_u32)
                .unwrap_or(0);
            ProviderInfo {
                provider: r
                    .get("provider")
                    .or_else(|| r.get("Provider"))
                    .map(variant_to_string)
                    .unwrap_or_default(),
                namespace: r
                    .get("Namespace")
                    .map(variant_to_string)
                    .unwrap_or_default(),
                host_pid,
                host_process: names.get(&host_pid).cloned().unwrap_or_default(),
                hosting_group: r
                    .get("HostingGroup")
                    .map(variant_to_string)
                    .unwrap_or_default(),
            }
        })
        .collect();
    providers.sort_by(|a, b| a.provider.to_lowercase().cmp(&b.provider.to_lowercase()));
    Ok(providers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_table_unions_columns_and_aligns_sparse_rows() {
        let mut r1 = HashMap::new();
        r1.insert("A".to_string(), Variant::UI4(1));
        r1.insert("B".to_string(), Variant::String("x".into()));
        let mut r2 = HashMap::new();
        r2.insert("A".to_string(), Variant::UI4(2)); // no "B"

        let table = to_table(vec![r1, r2]);
        assert_eq!(table.columns, vec!["A".to_string(), "B".to_string()]);
        assert_eq!(table.rows.len(), 2);
        // Every row is aligned to the column count, sparse cells blanked.
        for row in &table.rows {
            assert_eq!(row.len(), 2);
        }
        let b_col = table.columns.iter().position(|c| c == "B").unwrap();
        assert!(table.rows.iter().any(|r| r[b_col].is_empty()));
    }
}
