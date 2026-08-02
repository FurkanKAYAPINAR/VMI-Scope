//! Background WMI worker: one thread, one COM apartment, **one target**.
//!
//! COM apartments are thread-affine, so all WMI work happens on a dedicated
//! thread. The UI talks to it purely through channels: it pushes [`Request`]s
//! and drains [`Response`]s each frame without ever blocking on WMI.
//!
//! One target, because the host and credentials live in thread-locals here and
//! [`Request::SetHost`] flushes every cached connection when they change.
//! Talking to two machines therefore means two workers, which is what
//! [`crate::registry::WorkerRegistry`] is.
//!
//! Two rules hold everywhere below, and both are answers to bugs that were
//! measured rather than reasoned about:
//!
//! - **One door.** [`bind`] is the only way to reach WMI, so an operation
//!   cannot accidentally take a transport that ignores the configured
//!   credentials. Seven of them used to.
//! - **Every reply says where it came from.** Each carries the host it ran
//!   against and the namespace it read, stamped at execution time, so nothing
//!   downstream has to remember what was current when it asked.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows::Win32::System::Wmi::IWbemClassObject;
use wmi::Variant;

use crate::enumerate::{self, Bound, CancelToken, Completion, DirectConn, WorkerControl};
use crate::events::{assess, first_quoted, Risk, Subscription, SubscriptionReport};
use crate::host::{HostInfo, Impersonation};
use crate::method::{MethodArg, MethodOutcome, MethodTarget};
use crate::network::{tcp_state_name, Connection, NetworkSnapshot, Protocol};
use crate::providers::{host_pids, HostQuota, HostStats, ProviderHosts, ProviderInfo};
use crate::remote::{Credential, RemoteConn};
use crate::schema::{
    AssocInfo, ClassBrief, ClassKind, ClassSchema, NamespaceStats, SearchIndex, Tally,
};
use crate::value::{variant_to_string, variant_to_u32, variant_to_u64};

/// `root\CIMV2` — where the OS, process and provider classes live.
pub const CIMV2: &str = "root\\CIMV2";

/// `root\StandardCimv2` — where the TCP/UDP endpoint classes live.
pub const NET_NAMESPACE: &str = "root\\StandardCimv2";

/// `root` — where `__ProviderHostQuotaConfiguration` lives.
///
/// Not `root\CIMV2`, where the provider list itself comes from. The quota is a
/// property of the WMI installation, so it is a singleton one level up, and a
/// query for it in `CIMV2` returns nothing at all rather than an error.
pub const ROOT_NAMESPACE: &str = "root";

/// Every namespace a permanent-subscription scan visits.
///
/// `root\subscription` is the documented home; `root\default` is a classic
/// hiding spot, and a scan that only reads the documented one is a scan an
/// attacker has already read the documentation for.
pub const SUBSCRIPTION_NAMESPACES: [&str; 2] = ["root\\subscription", "root\\default"];

/// Wall-clock budget for enumerating one namespace's classes.
///
/// Generous because it is paid once and the first call of a session is the
/// expensive one: measured here, the very first `root\CIMV2` class enumeration
/// after a boot took 6.1 s while every later one took ~0.2 s. A budget tuned to
/// the warm case would turn a cold start into a failure.
pub const CLASS_ENUM_BUDGET: Duration = Duration::from_secs(30);

/// Wall-clock budget for one [`Request::NamespaceStats`], recursion included.
///
/// The budget is for the *whole* walk, not per namespace, because a recursive
/// rollup over `root` visits ~100 namespaces and a per-namespace budget would
/// multiply into minutes.
pub const NAMESPACE_STATS_BUDGET: Duration = Duration::from_secs(20);

/// Wall-clock budget for counting one class's instances.
///
/// Short on purpose. Counts are fired by a UI walking a class list, so this is
/// paid per row, and the pathological cases are not rare: `CIM_DataFile`
/// enumerates the filesystem and yields nothing at all for the first several
/// seconds, so a partial count is the *only* possible answer for it.
pub const INSTANCE_COUNT_BUDGET: Duration = Duration::from_secs(3);

/// Wall-clock budget for one [`Request::Associations`] lookup.
pub const ASSOCIATIONS_BUDGET: Duration = Duration::from_secs(10);

/// Wall-clock budget for the fixed-shape queries the worker issues on its own
/// behalf — the connection table, the provider list, a subscription scan.
///
/// These have no user watching a spinner and no cancel button of their own, so
/// they need a ceiling; 30 s because they are all small on a healthy machine
/// and the ceiling exists for the unhealthy one.
pub const HELPER_QUERY_BUDGET: Duration = Duration::from_secs(30);

/// Row cap and wall-clock budget for [`Request::ListInstances`].
///
/// The cap was always here; the budget was not, and the cap alone cannot bound
/// this. `CIM_DataFile` is a legal argument to "list the instances I could
/// invoke a method on", and it yields *zero* rows for its first several seconds
/// — so a row cap never fires and the worker sits inside the enumeration.
pub const INSTANCE_LIST_CAP: usize = 500;
pub const INSTANCE_LIST_BUDGET: Duration = Duration::from_secs(10);

/// Wall-clock budget for the whole provider-registration enrichment.
///
/// The `HostingModel` lookup is one bind plus one small query *per distinct
/// namespace in the provider list*, and that list is not bounded by anything we
/// control — a machine with decoupled providers scattered across the tree pays
/// per namespace. So the loop is bounded as a whole and degrades to an empty
/// `hosting_model` rather than delaying the rows that are already in hand.
/// Measured here: four namespaces, 4–18 ms each.
pub const PROVIDER_ENRICH_BUDGET: Duration = Duration::from_secs(10);

/// Above this many distinct host PIDs, ask the perf class for everything and
/// filter locally instead of building a WQL `OR` chain that long.
///
/// The cap is about the query text, not about cost: measured on this machine,
/// a five-PID filter takes 347–476 ms and the *unfiltered* enumeration of all
/// 393 processes takes 380–388 ms. The perf provider materialises every counter
/// instance either way, so the `WHERE` saves marshalling and nothing else.
pub const PERF_PID_FILTER_CAP: usize = 64;

/// A unit of work for the WMI thread. `id` lets the UI correlate the reply
/// with the widget that asked (namespaces resolve out of order otherwise).
#[derive(Debug, Clone)]
pub enum Request {
    /// Enumerate the direct child namespaces of `namespace` (via `__NAMESPACE`).
    ListChildNamespaces { id: u64, namespace: String },
    /// Enumerate the classes defined in `namespace` as [`ClassBrief`]s.
    ///
    /// Each row carries its [`crate::schema::ClassKind`] and provider, read off
    /// the class-definition object the enumeration already produced — no second
    /// round trip per class. Cancellable and bounded by [`CLASS_ENUM_BUDGET`].
    ListClasses { id: u64, namespace: String },
    /// Count the classes in `namespace`, and optionally in its whole subtree.
    ///
    /// The count comes from `CreateClassEnum` with every object discarded
    /// unread — see `enumerate::count`. `recursive` walks
    /// `SELECT Name FROM __NAMESPACE` depth-first, because WMI has no
    /// server-side rollup: a subtree total is N enumerations or it is nothing.
    NamespaceStats {
        id: u64,
        namespace: String,
        recursive: bool,
    },
    /// Count the instances of one class.
    ///
    /// `deep` includes subclasses (`WBEM_FLAG_DEEP`); false counts only
    /// instances of `class` itself. Bounded by [`INSTANCE_COUNT_BUDGET`] and
    /// cancellable, and classes on the skip-list
    /// ([`ClassKind::count_skip_reason`]) are answered without touching WMI.
    ///
    /// Unlike [`Request::Query`] there is no way to ask for an unbounded run.
    /// A query is something a user typed and is watching; a count is fired by
    /// the UI, per row, and an unbounded one would be a hang with no author.
    InstanceCount {
        id: u64,
        namespace: String,
        class: String,
        deep: bool,
    },
    /// List the relationships `class` participates in.
    ///
    /// `REFERENCES OF {class} WHERE SchemaOnly` for the association classes,
    /// `ASSOCIATORS OF {class} WHERE SchemaOnly` for the classes at the far
    /// end, both through raw `ExecQuery`.
    Associations {
        id: u64,
        namespace: String,
        class: String,
    },
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
    ///
    /// `include_system` folds the object's identity columns (`__RELPATH`,
    /// `__PATH`, `__CLASS`) into every row. They are stripped by default because
    /// the enumeration flag hides them and they are noise for a plain table, but
    /// a snapshot diff needs `__RELPATH` as the stable key for classes with no
    /// key property of their own — without it a diff can only compare whole
    /// rows, which is useless.
    Query {
        id: u64,
        namespace: String,
        wql: String,
        max_rows: Option<usize>,
        timeout: Option<Duration>,
        include_system: bool,
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
    ///
    /// `impersonation` reaches WMI on **both** transports, because both set
    /// their proxy blanket by hand. It is a real setting with real
    /// consequences — `Identify` refuses nearly everything a provider serves;
    /// see [`crate::host::Impersonation`] for the measurements.
    ///
    /// One worker now serves one host ([`crate::registry::WorkerRegistry`]), so
    /// this is normally sent once, immediately after the thread is spawned,
    /// rather than used to steer a shared worker between machines.
    SetHost {
        id: u64,
        host: Option<String>,
        cred: Option<Credential>,
        impersonation: Impersonation,
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
    /// The subset of `columns` that forms this class's key, when the WQL targets
    /// a single class and that class declares `key` properties.
    ///
    /// Empty when the query spans no single class (a meta or `ASSOCIATORS OF`
    /// query), when the class is keyless (`StdRegProv`, `Win32_OperatingSystem`),
    /// or when the class could not be reflected. A diff falls back to `__RELPATH`
    /// (present only when the query was run with `include_system`) and then to
    /// whole-row identity in that order.
    pub key_columns: Vec<String>,
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
///
/// **Every variant carries `host`, and every variant whose result belongs to a
/// namespace names that too** — stamped where the work happened rather than
/// remembered by the caller. (`SearchIndex` is the exception only in shape: its
/// namespace is a field of the index itself.) A UI that
/// tracked "the current host" alongside the reply would be describing the host
/// that is current *now*, not the one that answered — and the gap between those
/// two is exactly one `SetHost`. The rule this encodes: a result must be able
/// to say what it is a result *of*, without help.
#[derive(Debug, Clone, serde::Serialize)]
pub enum Response {
    ChildNamespaces {
        id: u64,
        host: Option<String>,
        namespace: String,
        children: Vec<String>,
        elapsed_ms: u64,
    },
    Classes {
        id: u64,
        host: Option<String>,
        namespace: String,
        classes: Vec<ClassBrief>,
        /// Why the enumeration stopped. A class list that was cut short must
        /// not read as "this namespace has 812 classes".
        completion: Completion,
        elapsed_ms: u64,
    },
    NamespaceStats {
        id: u64,
        host: Option<String>,
        namespace: String,
        stats: NamespaceStats,
        elapsed_ms: u64,
    },
    InstanceCount {
        id: u64,
        host: Option<String>,
        namespace: String,
        class: String,
        /// Counted (exactly or partially), or deliberately skipped.
        tally: Tally,
        elapsed_ms: u64,
    },
    Associations {
        id: u64,
        host: Option<String>,
        namespace: String,
        class: String,
        associations: Vec<AssocInfo>,
        completion: Completion,
        elapsed_ms: u64,
    },
    QueryResult {
        id: u64,
        host: Option<String>,
        namespace: String,
        wql: String,
        /// Carries its own `connect_ms` / `elapsed_ms` / `completion`.
        result: QueryResult,
    },
    Network {
        id: u64,
        host: Option<String>,
        /// Where the endpoint rows came from ([`NET_NAMESPACE`]). The process
        /// names joined onto them come from [`CIMV2`] on the same host.
        namespace: String,
        snapshot: NetworkSnapshot,
        elapsed_ms: u64,
    },
    EventSubscriptions {
        id: u64,
        host: Option<String>,
        /// The primary namespace scanned. The scan also covers the rest of
        /// [`SUBSCRIPTION_NAMESPACES`]; this names where a subscription is
        /// *supposed* to live.
        namespace: String,
        report: SubscriptionReport,
        elapsed_ms: u64,
    },
    Providers {
        id: u64,
        host: Option<String>,
        /// Where the provider rows came from ([`CIMV2`]). Their `HostingModel`
        /// comes from each provider's *own* namespace and the quota from
        /// [`ROOT_NAMESPACE`], both on the same host.
        namespace: String,
        providers: Vec<ProviderInfo>,
        /// Live load of the processes hosting them, and the quota they run
        /// against. Separate from `providers` because several providers share
        /// one host: folding the stats into each row would report the same
        /// 58 MB three times and make a baseline diff flap on CPU noise.
        hosts: ProviderHosts,
        elapsed_ms: u64,
    },
    Schema {
        id: u64,
        host: Option<String>,
        namespace: String,
        class: String,
        schema: ClassSchema,
        elapsed_ms: u64,
    },
    Mof {
        id: u64,
        host: Option<String>,
        namespace: String,
        object_path: String,
        mof: String,
    },
    Instances {
        id: u64,
        host: Option<String>,
        namespace: String,
        class: String,
        targets: Vec<MethodTarget>,
        /// Why the listing stopped — it is capped and deadline-bounded, and a
        /// picker showing 500 of 40,000 instances must not imply there are 500.
        completion: Completion,
    },
    MethodDone {
        id: u64,
        host: Option<String>,
        namespace: String,
        class: String,
        method: String,
        outcome: MethodOutcome,
    },
    SearchIndex {
        id: u64,
        host: Option<String>,
        index: SearchIndex,
        elapsed_ms: u64,
    },
    /// The target answered. `connect_ms` is the bind, `probe_ms` the two
    /// identity queries — reported apart because they fail for different
    /// reasons: a bad name or a firewall shows up in the first, a namespace ACL
    /// in the second.
    HostConnected {
        id: u64,
        host: Option<String>,
        connect_ms: u64,
        probe_ms: u64,
        info: HostInfo,
    },
    Error {
        id: u64,
        host: Option<String>,
        context: String,
        message: String,
    },
}

impl Response {
    /// The id of the request this answers.
    pub fn id(&self) -> u64 {
        match self {
            Response::ChildNamespaces { id, .. }
            | Response::Classes { id, .. }
            | Response::NamespaceStats { id, .. }
            | Response::InstanceCount { id, .. }
            | Response::Associations { id, .. }
            | Response::QueryResult { id, .. }
            | Response::Network { id, .. }
            | Response::EventSubscriptions { id, .. }
            | Response::Providers { id, .. }
            | Response::Schema { id, .. }
            | Response::Mof { id, .. }
            | Response::Instances { id, .. }
            | Response::MethodDone { id, .. }
            | Response::SearchIndex { id, .. }
            | Response::HostConnected { id, .. }
            | Response::Error { id, .. } => *id,
        }
    }

    /// The host this result was produced on, `None` for the local machine.
    pub fn host(&self) -> Option<&str> {
        match self {
            Response::ChildNamespaces { host, .. }
            | Response::Classes { host, .. }
            | Response::NamespaceStats { host, .. }
            | Response::InstanceCount { host, .. }
            | Response::Associations { host, .. }
            | Response::QueryResult { host, .. }
            | Response::Network { host, .. }
            | Response::EventSubscriptions { host, .. }
            | Response::Providers { host, .. }
            | Response::Schema { host, .. }
            | Response::Mof { host, .. }
            | Response::Instances { host, .. }
            | Response::MethodDone { host, .. }
            | Response::SearchIndex { host, .. }
            | Response::HostConnected { host, .. }
            | Response::Error { host, .. } => host.as_deref(),
        }
    }
}

/// Handle to the background WMI thread. Dropping it shuts the thread down.
///
/// A fresh worker targets the local machine as the current user. Point it
/// somewhere else with [`Request::SetHost`] — or let
/// [`crate::registry::WorkerRegistry`] own one per host and do that for you.
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
/// COM is still bootstrapped through the `wmi` crate (an MTA via
/// `CoIncrementMTAUsage` plus the default `CoInitializeSecurity`, once per
/// thread — see `enumerate::create_locator`), but no request travels on one of
/// its connections any more. Every interface here is thread-affine, so all
/// connections are created and used on this thread and never handed to another.
fn run(rx: Receiver<Request>, tx: Sender<Response>, control: WorkerControl) {
    for req in rx {
        // The flag, not the message, is what makes shutdown prompt: by the
        // time `Request::Shutdown` reaches the front of the queue there may be
        // a hundred requests ahead of it that were sent first.
        if control.is_shutdown() {
            break;
        }

        // The host stamp, read *here* — after every earlier request in the
        // queue has finished, and therefore after any `SetHost` among them, but
        // before this one runs. Reading it in the UI when the reply arrives
        // would name whichever host is current by then; reading it when the
        // request was *sent* would name the host the user had selected at the
        // time. Only this point is the host the answer actually came from.
        let host = current_host();

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
                        host,
                        namespace,
                        children,
                        elapsed_ms: ms(t0),
                    },
                    Err(e) => Response::Error {
                        id,
                        host,
                        context: format!("List namespaces under {namespace}"),
                        message: e.to_string(),
                    },
                };
                let _ = tx.send(resp);
            }

            Request::ListClasses { id, namespace } => {
                let t0 = Instant::now();
                let cancel = control.begin(id);
                let outcome = q_class_briefs(&namespace, &cancel);
                control.end(id);
                let resp = match outcome {
                    Ok((classes, completion)) => {
                        remember_kinds(&namespace, &classes);
                        Response::Classes {
                            id,
                            host,
                            namespace,
                            classes,
                            completion,
                            elapsed_ms: ms(t0),
                        }
                    }
                    Err(e) => Response::Error {
                        id,
                        host,
                        context: format!("List classes in {namespace}"),
                        message: e.to_string(),
                    },
                };
                let _ = tx.send(resp);
            }

            Request::NamespaceStats {
                id,
                namespace,
                recursive,
            } => {
                let t0 = Instant::now();
                let cancel = control.begin(id);
                let outcome = namespace_stats(&namespace, recursive, &cancel);
                control.end(id);
                let resp = match outcome {
                    Ok(stats) => Response::NamespaceStats {
                        id,
                        host,
                        namespace,
                        stats,
                        elapsed_ms: ms(t0),
                    },
                    Err(e) => Response::Error {
                        id,
                        host,
                        context: format!("Count classes in {namespace}"),
                        message: e.to_string(),
                    },
                };
                let _ = tx.send(resp);
            }

            Request::InstanceCount {
                id,
                namespace,
                class,
                deep,
            } => {
                let t0 = Instant::now();
                let cancel = control.begin(id);
                let outcome = instance_count(&namespace, &class, deep, &cancel);
                control.end(id);
                let resp = match outcome {
                    Ok(tally) => Response::InstanceCount {
                        id,
                        host,
                        namespace,
                        class,
                        tally,
                        elapsed_ms: ms(t0),
                    },
                    Err(e) => Response::Error {
                        id,
                        host,
                        context: format!("Count instances of {class} in {namespace}"),
                        message: e.to_string(),
                    },
                };
                let _ = tx.send(resp);
            }

            Request::Associations {
                id,
                namespace,
                class,
            } => {
                let t0 = Instant::now();
                let cancel = control.begin(id);
                let outcome = class_associations(&namespace, &class, &cancel);
                control.end(id);
                let resp = match outcome {
                    Ok((associations, completion)) => Response::Associations {
                        id,
                        host,
                        namespace,
                        class,
                        associations,
                        completion,
                        elapsed_ms: ms(t0),
                    },
                    Err(e) => Response::Error {
                        id,
                        host,
                        context: format!("Associations of {class} in {namespace}"),
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
                include_system,
            } => {
                let cancel = control.begin(id);
                let outcome =
                    run_query(&namespace, &wql, max_rows, timeout, include_system, &cancel);
                control.end(id);
                let resp = match outcome {
                    Ok(result) => Response::QueryResult {
                        id,
                        host,
                        namespace,
                        wql,
                        result,
                    },
                    Err(e) => Response::Error {
                        id,
                        host,
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
                        host,
                        namespace: NET_NAMESPACE.into(),
                        snapshot,
                        elapsed_ms: ms(t0),
                    },
                    Err(e) => Response::Error {
                        id,
                        host,
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
                        host,
                        namespace: SUBSCRIPTION_NAMESPACES[0].into(),
                        report,
                        elapsed_ms: ms(t0),
                    },
                    Err(e) => Response::Error {
                        id,
                        host,
                        context: "Enumerate event subscriptions".into(),
                        message: e.to_string(),
                    },
                };
                let _ = tx.send(resp);
            }

            Request::ListProviders { id } => {
                let t0 = Instant::now();
                // Cancellable since 5.11: this used to be one fixed query and
                // is now a walk over every namespace that registers a provider,
                // plus a perf enumeration. Anything that grows with the target
                // machine needs a way out.
                let cancel = control.begin(id);
                let outcome = list_providers(&cancel);
                control.end(id);
                let resp = match outcome {
                    Ok((providers, hosts)) => Response::Providers {
                        id,
                        host,
                        namespace: CIMV2.into(),
                        providers,
                        hosts,
                        elapsed_ms: ms(t0),
                    },
                    Err(e) => Response::Error {
                        id,
                        host,
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
                let resp = match bind(&namespace)
                    .and_then(|c| c.get_object(&class))
                    .and_then(|o| crate::reflect::read_class_schema(&o, &class))
                {
                    Ok(schema) => Response::Schema {
                        id,
                        host,
                        namespace,
                        class,
                        schema,
                        elapsed_ms: ms(t0),
                    },
                    Err(e) => Response::Error {
                        id,
                        host,
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
                let resp = match bind(&namespace)
                    .and_then(|c| c.get_object(&object_path))
                    .and_then(|o| crate::reflect::object_mof(&o))
                {
                    Ok(mof) => Response::Mof {
                        id,
                        host,
                        namespace,
                        object_path,
                        mof,
                    },
                    Err(e) => Response::Error {
                        id,
                        host,
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
                let cancel = control.begin(id);
                let outcome = bind(&namespace).and_then(|c| {
                    crate::method::list_instances(
                        &c,
                        &class,
                        Some(INSTANCE_LIST_CAP),
                        Some(INSTANCE_LIST_BUDGET),
                        &cancel,
                    )
                });
                control.end(id);
                let resp = match outcome {
                    Ok((targets, completion)) => Response::Instances {
                        id,
                        host,
                        namespace,
                        class,
                        targets,
                        completion,
                    },
                    Err(e) => Response::Error {
                        id,
                        host,
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
                let resp = match bind(&namespace).and_then(|c| {
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
                        host,
                        namespace,
                        class,
                        method,
                        outcome,
                    },
                    Err(e) => Response::Error {
                        id,
                        host,
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
                let cancel = control.begin(id);
                let outcome = build_search_index(&namespace, include_methods, &cancel);
                control.end(id);
                let resp = match outcome {
                    Ok(index) => Response::SearchIndex {
                        id,
                        host,
                        index,
                        elapsed_ms: ms(t0),
                    },
                    Err(e) => Response::Error {
                        id,
                        host,
                        context: format!("Build search index for {namespace}"),
                        message: e.to_string(),
                    },
                };
                let _ = tx.send(resp);
            }

            // A failed connect leaves the worker pointed at the target it
            // failed to reach, on purpose. Falling back to the local machine
            // would leave a worker that answers — plausibly, and about the
            // wrong computer — after the connection it exists for did not
            // happen. Every later request now fails with the same connection
            // error until the caller says otherwise, and `SetHost` with no host
            // is always the way back.
            Request::SetHost {
                id,
                host,
                cred,
                impersonation,
            } => {
                set_target(host.clone(), cred, impersonation);
                let resp = match connect_and_probe() {
                    Ok((connect_ms, probe_ms, info)) => Response::HostConnected {
                        id,
                        // The *new* host: this reply is about the switch, so it
                        // is stamped with what was switched to.
                        host,
                        connect_ms,
                        probe_ms,
                        info,
                    },
                    Err(e) => Response::Error {
                        id,
                        host,
                        context: "Connect to host".into(),
                        message: e.to_string(),
                    },
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
    /// Impersonation level for the alternate-credential transport.
    static IMP: RefCell<Impersonation> = const { RefCell::new(Impersonation::Impersonate) };
    /// Per-namespace raw-DCOM connections, used only in alternate-credential mode.
    ///
    /// `Rc` so a binding can be handed out and the cache's borrow released
    /// immediately. Holding the `RefCell` borrow for the length of an operation
    /// was what made a recursive namespace walk impossible under alternate
    /// credentials: the second `bind` was a second `borrow_mut`, which is a
    /// panic, so those requests had to refuse to run at all.
    static REMOTE: RefCell<HashMap<String, Rc<RemoteConn>>> = RefCell::new(HashMap::new());
    /// `(namespace, class)` -> kind, both keys lowercased.
    ///
    /// The schema cache task 3.9 asks for, in its smallest useful form. It is
    /// filled wholesale and for free by every [`Request::ListClasses`], and
    /// read by [`Request::InstanceCount`] to decide whether a class is on the
    /// skip-list. Without it, deciding "is this abstract?" costs a `GetObject`
    /// per class — the exact round trip the enumeration already paid for.
    static KIND_CACHE: RefCell<HashMap<(String, String), ClassKind>> = RefCell::new(HashMap::new());
}

/// Remember every kind a class enumeration just established.
fn remember_kinds(namespace: &str, classes: &[ClassBrief]) {
    let ns = namespace.to_lowercase();
    KIND_CACHE.with(|c| {
        let mut c = c.borrow_mut();
        for brief in classes {
            c.insert((ns.clone(), brief.name.to_lowercase()), brief.kind);
        }
    });
}

/// The cached kind of one class, if a class enumeration has already seen it.
fn cached_kind(namespace: &str, class: &str) -> Option<ClassKind> {
    KIND_CACHE.with(|c| {
        c.borrow()
            .get(&(namespace.to_lowercase(), class.to_lowercase()))
            .copied()
    })
}

/// Point this worker at a target. Clears everything cached about the old one.
fn set_target(host: Option<String>, cred: Option<Credential>, imp: Impersonation) {
    HOST.with(|h| *h.borrow_mut() = host);
    CRED.with(|c| *c.borrow_mut() = cred);
    IMP.with(|i| *i.borrow_mut() = imp);
    REMOTE.with(|m| m.borrow_mut().clear());
    // A class kind is a fact about a *machine's* repository, not about a class
    // name. Carrying it across a host switch would badge the new target with
    // the old one's schema.
    KIND_CACHE.with(|m| m.borrow_mut().clear());
}

/// The host all connections currently target, or `None` for this machine.
fn current_host() -> Option<String> {
    HOST.with(|h| h.borrow().clone())
}

/// Are we in alternate-credential mode (raw DCOM), or local/SSO?
fn is_alt_cred() -> bool {
    CRED.with(|c| c.borrow().is_some())
}

/// The cached raw-DCOM connection for `namespace`, opening one if needed.
///
/// The cache borrow is released before the connection is returned, so a caller
/// may hold one binding while asking for another.
fn remote_conn(namespace: &str) -> anyhow::Result<Rc<RemoteConn>> {
    if let Some(existing) = REMOTE.with(|c| c.borrow().get(namespace).cloned()) {
        return Ok(existing);
    }
    let host =
        current_host().ok_or_else(|| anyhow::anyhow!("alternate credentials require a host"))?;
    let cred = CRED
        .with(|c| c.borrow().clone())
        .ok_or_else(|| anyhow::anyhow!("no credentials set"))?;
    let imp = IMP.with(|i| *i.borrow());
    let conn = Rc::new(RemoteConn::connect(&host, namespace, &cred, imp)?);
    REMOTE.with(|c| c.borrow_mut().insert(namespace.to_string(), conn.clone()));
    Ok(conn)
}

/// **The** credential dispatcher: bind `namespace` on the transport this
/// worker's credentials require.
///
/// Every WMI operation this module performs starts here. That is a structural
/// claim, not a stylistic one. What it replaced was a set of paths that called
/// the `wmi` crate directly — the one transport that cannot carry a credential
/// — so under alternate credentials they ran as the current user and said
/// nothing about it. Two were known when this was written. Counting them by
/// making the wrong path fail found **seven**, one of which invoked a method:
/// see `docs/FINDINGS.md` and `examples/altcred.rs`.
///
/// One door is what makes that a closed class rather than seven fixes. A path
/// added tomorrow cannot pick the wrong transport, because there is no other
/// transport to pick.
fn bind(namespace: &str) -> anyhow::Result<Bound> {
    if is_alt_cred() {
        Ok(Bound::Remote(remote_conn(namespace)?))
    } else {
        Ok(Bound::Direct(DirectConn::open_with(
            current_host().as_deref(),
            namespace,
            IMP.with(|i| *i.borrow()),
        )?))
    }
}

/// Bind the target and ask it who it is: `(connect_ms, probe_ms, info)`.
///
/// The two timings are separate because they answer different questions and
/// fail for different reasons. `connect_ms` is the DCOM bind — the round trip a
/// "test connection" button is really measuring, and where a wrong name, a
/// closed port or a rejected credential surfaces. `probe_ms` is two ordinary
/// queries on top of it, which is what a *usable* connection costs and where a
/// namespace ACL surfaces instead.
///
/// The probe result used to be discarded — the connect ran
/// `SELECT Name FROM Win32_ComputerSystem` purely to see whether it would throw.
/// The same round trip can answer which OS, which build and which machine, so
/// it does.
fn connect_and_probe() -> anyhow::Result<(u64, u64, HostInfo)> {
    let t_connect = Instant::now();
    let conn = bind(CIMV2)?;
    let connect_ms = ms(t_connect);

    let t_probe = Instant::now();
    let mut info = HostInfo::default();
    // The OS query is the one that must succeed: it is the reachability proof
    // the connect step used to stand on.
    let en = conn.exec_enum(
        "SELECT Caption, Version, BuildNumber, OSArchitecture, LastBootUpTime \
         FROM Win32_OperatingSystem",
    )?;
    let (rows, _) = enumerate::drain(
        &en,
        Some(1),
        Some(HELPER_QUERY_BUDGET),
        &CancelToken::never(),
        |o| unsafe { crate::remote::object_to_map(o, false) },
    )?;
    if let Some(os) = rows.first() {
        let get = |k: &str| os.get(k).map(variant_to_string).unwrap_or_default();
        info.caption = get("Caption");
        info.version = get("Version");
        info.build_number = get("BuildNumber");
        info.architecture = get("OSArchitecture");
        info.last_boot = get("LastBootUpTime");
    }
    // The machine UUID is a nice-to-have from a *different* provider, so a
    // failure here is not a failure to connect. Two hosts with the same name
    // are a real situation and this is what tells them apart, but a host that
    // will not answer it is still a host we are connected to.
    if let Ok(rows) = q_maps(CIMV2, "SELECT UUID FROM Win32_ComputerSystemProduct") {
        if let Some(p) = rows.first() {
            info.uuid = p.get("UUID").map(variant_to_string).unwrap_or_default();
        }
    }
    Ok((connect_ms, ms(t_probe), info))
}

/// Run one of the worker's own fixed-shape queries and flatten the rows.
///
/// A partial answer to one of these is worse than an error: half a connection
/// table, read as the whole one, is a hunt that misses the connection it was
/// looking for. So the deadline and the cancellation flag are reported as
/// failures here, unlike on [`Request::Query`] where the user asked for the
/// query, is watching it, and is told exactly how it ended.
fn q_maps(namespace: &str, wql: &str) -> anyhow::Result<Vec<HashMap<String, Variant>>> {
    q_maps_within(namespace, wql, HELPER_QUERY_BUDGET, &CancelToken::never())
}

/// [`q_maps`] with the caller's own deadline and cancellation flag.
///
/// Exists for the loops: a request that issues one query per namespace cannot
/// hand each of them the full [`HELPER_QUERY_BUDGET`], or its own ceiling
/// becomes that budget multiplied by however many namespaces the target
/// machine happens to have.
fn q_maps_within(
    namespace: &str,
    wql: &str,
    budget: Duration,
    cancel: &CancelToken,
) -> anyhow::Result<Vec<HashMap<String, Variant>>> {
    let conn = bind(namespace)?;
    let en = conn.exec_enum(wql)?;
    let (rows, completion) = enumerate::drain(&en, None, Some(budget), cancel, |o| unsafe {
        crate::remote::object_to_map(o, false)
    })?;
    match completion.note() {
        None => Ok(rows),
        Some(why) => anyhow::bail!("{wql} in {namespace}: {why}"),
    }
}

/// Enumerate a namespace's classes as briefs.
///
/// `CreateClassEnum` on both transports, not `SELECT * FROM meta_class`: it
/// reaches the same class-definition objects without the query engine parsing
/// WQL, building a projection and evaluating a trivial filter to get there. The
/// alternate-credential path used the query form only because it was the one
/// call that re-blanketed its enumerator; [`RemoteConn::class_enum`] does that
/// too now.
fn q_class_briefs(
    namespace: &str,
    cancel: &CancelToken,
) -> anyhow::Result<(Vec<ClassBrief>, Completion)> {
    let conn = bind(namespace)?;
    let en = conn.class_enum(None, true)?;
    let (mut classes, completion) =
        enumerate::drain(&en, None, Some(CLASS_ENUM_BUDGET), cancel, |o| {
            Ok(crate::reflect::class_brief(o))
        })?;
    classes.retain(|c| !c.name.is_empty());
    classes.sort_by(|a, b| a.name.cmp(&b.name));
    classes.dedup_by(|a, b| a.name == b.name);
    Ok((classes, completion))
}

/// Count the classes in one namespace without reading a single object.
fn count_classes(
    conn: &Bound,
    deadline: Option<Duration>,
    cancel: &CancelToken,
) -> anyhow::Result<(usize, Completion)> {
    let en = conn.class_enum(None, true)?;
    enumerate::count(&en, deadline, cancel)
}

/// The direct child namespaces of `namespace`, fully qualified.
///
/// Separate from [`list_child_namespaces`] because this one runs inside a
/// recursive walk: it takes a binding the caller already holds and a share of
/// the walk's remaining budget, so a rollup over `root` cannot spend a fresh
/// full budget on each of its ~100 namespaces.
fn child_namespaces(
    conn: &Bound,
    namespace: &str,
    deadline: Option<Duration>,
    cancel: &CancelToken,
) -> anyhow::Result<(Vec<String>, Completion)> {
    let en = conn.exec_enum("SELECT Name FROM __NAMESPACE")?;
    let (names, completion) = enumerate::drain(&en, None, deadline, cancel, |o| {
        Ok(crate::reflect::string_property(o, "Name"))
    })?;
    let mut children: Vec<String> = names
        .into_iter()
        .filter(|n| !n.is_empty())
        .map(|n| format!("{namespace}\\{n}"))
        .collect();
    children.sort_unstable();
    children.dedup();
    Ok((children, completion))
}

/// Class counts for a namespace, optionally rolled up over its subtree.
///
/// The walk is depth-first and iterative, and the budget is checked before
/// every namespace rather than only between batches: binding a namespace is
/// itself a round trip, so a rollup over `root` can spend most of its time in
/// `ConnectServer` calls that [`enumerate::drain`] never sees.
///
/// A namespace that cannot be bound or enumerated increments `unreadable`
/// instead of failing the request. `root\SECURITY` denies access to a normal
/// token, and a tree that refuses to show any counts because one node is
/// private would be useless.
fn namespace_stats(
    namespace: &str,
    recursive: bool,
    cancel: &CancelToken,
) -> anyhow::Result<NamespaceStats> {
    let started = Instant::now();
    let mut stats = NamespaceStats {
        namespace: namespace.to_string(),
        recursive,
        ..Default::default()
    };

    let mut stack = vec![namespace.to_string()];
    let mut root = true;
    while let Some(ns) = stack.pop() {
        if cancel.is_raised() {
            stats.completion = Completion::Cancelled;
            break;
        }
        let Some(left) = NAMESPACE_STATS_BUDGET.checked_sub(started.elapsed()) else {
            stats.completion = Completion::TimedOut {
                after_ms: started.elapsed().as_millis() as u64,
                rows: stats.total_classes,
            };
            break;
        };

        let conn = match bind(&ns) {
            Ok(conn) => conn,
            // The *root* failing is not a partial result, it is no result —
            // a bad namespace has to surface as an error, not as a rollup of
            // zero.
            Err(e) if root => return Err(e),
            Err(_) => {
                stats.unreadable += 1;
                continue;
            }
        };

        match count_classes(&conn, Some(left), cancel) {
            Ok((n, c)) => {
                stats.namespaces += 1;
                stats.total_classes += n;
                if root {
                    stats.classes = n;
                }
                // A partial count anywhere makes the rollup partial. Keep the
                // first reason: it is the one that explains the rest.
                if !c.is_complete() && stats.completion.is_complete() {
                    stats.completion = c;
                }
            }
            Err(_) => stats.unreadable += 1,
        }

        // The root's children are counted even for a non-recursive request:
        // the tree wants to know whether a node is expandable.
        if recursive || root {
            let left = NAMESPACE_STATS_BUDGET
                .checked_sub(started.elapsed())
                .unwrap_or_default();
            if let Ok((kids, _)) = child_namespaces(&conn, &ns, Some(left), cancel) {
                if root {
                    stats.children = kids.len();
                }
                if recursive {
                    stack.extend(kids);
                }
            }
        }
        root = false;
    }

    Ok(stats)
}

/// The kind of one class, from the cache when a class enumeration has already
/// established it, else from a single `GetObject`.
fn class_kind(conn: &Bound, namespace: &str, class: &str) -> anyhow::Result<ClassKind> {
    if let Some(kind) = cached_kind(namespace, class) {
        return Ok(kind);
    }
    let obj = conn.get_object(class)?;
    let brief = crate::reflect::class_brief(&obj);
    KIND_CACHE.with(|c| {
        c.borrow_mut()
            .insert((namespace.to_lowercase(), class.to_lowercase()), brief.kind)
    });
    Ok(brief.kind)
}

/// Count the instances of one class, or decline to.
fn instance_count(
    namespace: &str,
    class: &str,
    deep: bool,
    cancel: &CancelToken,
) -> anyhow::Result<Tally> {
    let conn = bind(namespace)?;
    if let Some(reason) = class_kind(&conn, namespace, class)?.count_skip_reason() {
        return Ok(Tally::Skipped(reason));
    }
    let en = conn.instance_enum(class, deep)?;
    let (instances, completion) = enumerate::count(&en, Some(INSTANCE_COUNT_BUDGET), cancel)?;
    Ok(Tally::Counted {
        instances,
        completion,
    })
}

/// Decompose the association classes one `REFERENCES OF` returned into rows.
///
/// `subject` is the class the query was aimed at; the reference property that
/// points at it is the role, and every other reference on the association is a
/// far end. `note` is stamped by the caller, which is the only thing that knows
/// whether `subject` was the class asked about or one of its ancestors.
fn assoc_rows(defs: &[crate::reflect::AssocClassDef], subject: &str, note: &str) -> Vec<AssocInfo> {
    let mut out = Vec::new();
    for def in defs {
        if def.class.is_empty() {
            continue;
        }
        let ours: Vec<&(String, String)> = def
            .endpoints
            .iter()
            .filter(|(_, t)| t.eq_ignore_ascii_case(subject))
            .collect();
        if ours.is_empty() {
            // WMI returned an association that references `subject`, but no
            // reference property names it. Report the endpoints rather than
            // drop the row: an association we cannot decompose is still one
            // the class participates in.
            for (role, target) in &def.endpoints {
                out.push(AssocInfo {
                    assoc_class: def.class.clone(),
                    role: role.clone(),
                    target_class: target.clone(),
                    note: join_notes(note, "role not resolved"),
                });
            }
            continue;
        }
        for (role, _) in &ours {
            let far: Vec<&(String, String)> = def
                .endpoints
                .iter()
                .filter(|(name, _)| name != role)
                .collect();
            // A degenerate association with a single reference — it points at
            // us and nowhere else, so we are our own far end.
            let far = if far.is_empty() { ours.clone() } else { far };
            for (_, target) in far {
                // `Win32_SubDirectory` and `CIM_BasedOn` relate a class to
                // itself. The far end is *not* empty in that case — it is the
                // other role on the same class — so this has to be decided per
                // row from the target, never from the shape of the endpoint
                // list.
                let self_ref = target.eq_ignore_ascii_case(subject);
                out.push(AssocInfo {
                    assoc_class: def.class.clone(),
                    role: role.clone(),
                    target_class: target.clone(),
                    note: join_notes(note, if self_ref { "self-referencing" } else { "" }),
                });
            }
        }
    }
    out
}

fn join_notes(a: &str, b: &str) -> String {
    match (a.is_empty(), b.is_empty()) {
        (true, true) => String::new(),
        (false, true) => a.to_string(),
        (true, false) => b.to_string(),
        (false, false) => format!("{a}; {b}"),
    }
}

/// Every relationship `class` takes part in, inherited ones included.
///
/// Three facts about WMI shape this, all measured rather than assumed.
///
/// **`REFERENCES OF` does not walk the derivation chain.** Measured on
/// `root\CIMV2`: `REFERENCES OF {Win32_Process}` returns exactly
/// `Win32_SessionProcess`, `Win32_NamedJobObjectProcess`,
/// `Win32_SystemProcesses`, while `REFERENCES OF {CIM_Process}` returns a
/// disjoint three — `CIM_ProcessThread`, `CIM_ProcessExecutable`,
/// `CIM_OSProcess`. A `Win32_Process` is a `CIM_Process`, so it can stand at
/// the end of all six, but only one query per ancestor finds them. Hence the
/// loop over `__Derivation`; the plan's "≥ 4 associations for `Win32_Process`"
/// is unreachable without it and correct with it.
///
/// **`SchemaOnly` is what makes this affordable.** Without it both forms
/// enumerate live *instances* — `ASSOCIATORS OF {Win32_Process}` on a busy
/// machine is thousands of objects to answer a question about the schema.
///
/// **The endpoint class lives in the `CIMTYPE` qualifier, not the type code.**
/// `Get` reports `CIM_REFERENCE` for every reference property alike; only
/// `CIMTYPE` carries `ref:Win32_LogonSession`.
///
/// `ASSOCIATORS OF` runs alongside as a cross-check, so an endpoint WMI reports
/// but whose association could not be decomposed still appears, labelled.
///
/// All of it reads `__CLASS` by name off the raw object. §5.5 of the plan
/// listed this task as blocked on system properties surviving the query path;
/// they do not survive it, and this never uses it.
fn class_associations(
    namespace: &str,
    class: &str,
    cancel: &CancelToken,
) -> anyhow::Result<(Vec<AssocInfo>, Completion)> {
    let conn = bind(namespace)?;
    let started = Instant::now();

    // The class itself first, then each ancestor nearest-first.
    let mut lineage = vec![class.to_string()];
    if let Ok(obj) = conn.get_object(class) {
        lineage.extend(crate::reflect::class_derivation(&obj));
    }

    let mut out: Vec<AssocInfo> = Vec::new();
    let mut completion = Completion::Complete;
    let mut endpoints: Vec<String> = Vec::new();

    for (depth, subject) in lineage.iter().enumerate() {
        // One shared budget across every query, so a deep hierarchy cannot
        // multiply the cost of the request by its own length.
        let Some(left) = ASSOCIATIONS_BUDGET.checked_sub(started.elapsed()) else {
            completion = Completion::TimedOut {
                after_ms: started.elapsed().as_millis() as u64,
                rows: out.len(),
            };
            break;
        };
        if cancel.is_raised() {
            completion = Completion::Cancelled;
            break;
        }
        let note = if depth == 0 {
            String::new()
        } else {
            format!("inherited via {subject}")
        };

        let en = conn.exec_enum(&format!("REFERENCES OF {{{subject}}} WHERE SchemaOnly"))?;
        let (defs, c) = enumerate::drain(&en, None, Some(left), cancel, |o| {
            Ok(crate::reflect::assoc_class_def(o))
        })?;
        if !c.is_complete() && completion.is_complete() {
            completion = c;
        }
        out.extend(assoc_rows(&defs, subject, &note));

        let left = ASSOCIATIONS_BUDGET
            .checked_sub(started.elapsed())
            .unwrap_or_default();
        let en = conn.exec_enum(&format!("ASSOCIATORS OF {{{subject}}} WHERE SchemaOnly"))?;
        if let Ok((names, _)) = enumerate::drain(&en, None, Some(left), cancel, |o| {
            Ok(crate::reflect::class_name(o))
        }) {
            endpoints.extend(names.into_iter().filter(|n| !n.is_empty()));
        }
    }

    for endpoint in endpoints {
        if !out
            .iter()
            .any(|a| a.target_class.eq_ignore_ascii_case(&endpoint))
        {
            out.push(AssocInfo {
                assoc_class: String::new(),
                role: String::new(),
                target_class: endpoint,
                note: "reported by ASSOCIATORS OF only".into(),
            });
        }
    }

    out.sort_by(|a, b| {
        (&a.assoc_class, &a.role, &a.target_class).cmp(&(&b.assoc_class, &b.role, &b.target_class))
    });
    out.dedup();
    Ok((out, completion))
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
    include_system: bool,
    cancel: &CancelToken,
) -> anyhow::Result<QueryResult> {
    // Both transports flatten an object identically; only the way the
    // enumerator is obtained differs. `include_system` rides through the closure
    // so the identity columns are read on the same object, without a second
    // pass.
    let to_map =
        |obj: &IWbemClassObject| unsafe { crate::remote::object_to_map(obj, include_system) };

    let t_connect = Instant::now();
    let conn = bind(namespace)?;
    let connect_ms = ms(t_connect);

    let t_exec = Instant::now();
    let en = conn.exec_enum(wql)?;
    let (rows, completion) = enumerate::drain(&en, max_rows, deadline, cancel, to_map)?;
    let elapsed_ms = ms(t_exec);

    let mut table = to_table(rows);
    table.connect_ms = connect_ms;
    table.elapsed_ms = elapsed_ms;
    table.completion = completion;
    // Key columns are a fact about the *class*, not the rows, so they are read
    // after the timed enumeration and never counted against `elapsed_ms`. A
    // failure to reflect them is not a failure of the query: the table is still
    // whole, it just has no declared key, and the diff falls back accordingly.
    table.key_columns = single_class_from_wql(wql)
        .and_then(|class| class_key_columns(&conn, &class).ok())
        .unwrap_or_default();
    Ok(table)
}

/// The single class a WQL `SELECT` targets, or `None` for anything else.
///
/// WMI has no joins, so a data query names exactly one class after `FROM`; that
/// token is the class. Everything else — `ASSOCIATORS OF`, `REFERENCES OF`, a
/// subquery, `meta_class` reflection — has no single instance class to key on,
/// and returns `None` rather than a guess. The parse is deliberately strict:
/// the token after `FROM` must be a bare identifier, so a quoted object path or
/// a parenthesised subquery is rejected instead of mistaken for a class name.
fn single_class_from_wql(wql: &str) -> Option<String> {
    let tokens: Vec<&str> = wql.split_whitespace().collect();
    let from = tokens.iter().position(|t| t.eq_ignore_ascii_case("from"))?;
    let raw = tokens.get(from + 1)?;
    // A class butting straight against a clause with no space ("Win32_Service"
    // is fine, but be defensive about "Win32_Service;") is trimmed to its
    // identifier core.
    let name = raw.trim_matches(|c: char| !(c.is_alphanumeric() || c == '_'));
    if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    // `meta_class` is the reflection pseudo-class, not an instance class.
    if name.eq_ignore_ascii_case("meta_class") {
        return None;
    }
    Some(name.to_string())
}

/// The `key`-qualified property names of `class`, in declared order.
///
/// One `GetObject` plus the reflection the schema panel already does — no
/// enumeration, so it is bounded by nothing but that single round trip. Keyless
/// classes (`StdRegProv`, singletons) legitimately return an empty list.
fn class_key_columns(conn: &Bound, class: &str) -> anyhow::Result<Vec<String>> {
    let obj = conn.get_object(class)?;
    let schema = crate::reflect::read_class_schema(&obj, class)?;
    Ok(schema
        .properties
        .iter()
        .filter(|p| p.is_key)
        .map(|p| p.name.clone())
        .collect())
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

/// Turn one endpoint row into a [`Connection`], joining the owning process by
/// **PID**.
///
/// Two things here are load-bearing. The join key is `OwningProcess`, never a
/// name, and the ports come back as `uint16` widened to `u32` by the variant
/// layer, so they have to be narrowed again rather than parsed. A row whose
/// PID is absent from the map keeps an empty process name — the endpoint is
/// real either way, and dropping it would hide exactly the connection whose
/// owner could not be read.
fn to_connection(
    r: &HashMap<String, Variant>,
    proto: Protocol,
    pid_name: &HashMap<u32, String>,
) -> Connection {
    let pid = r.get("OwningProcess").map(variant_to_u32).unwrap_or(0);
    let text = |k: &str| r.get(k).map(variant_to_string).unwrap_or_default();
    let port = |k: &str| r.get(k).map(variant_to_u32).unwrap_or(0) as u16;
    let tcp = proto == Protocol::Tcp;
    Connection {
        proto,
        local_addr: text("LocalAddress"),
        local_port: port("LocalPort"),
        // UDP is connectionless: it has no peer and no state, and inventing
        // `0.0.0.0:0` for one would put a fake row in a hunt.
        remote_addr: if tcp {
            text("RemoteAddress")
        } else {
            String::new()
        },
        remote_port: if tcp { port("RemotePort") } else { 0 },
        state: if tcp {
            tcp_state_name(r.get("State").map(variant_to_u32).unwrap_or(0)).to_string()
        } else {
            String::new()
        },
        pid,
        process: pid_name.get(&pid).cloned().unwrap_or_default(),
    }
}

/// Snapshot the TCP/UDP connection table and resolve owning process names.
///
/// TCP/UDP tables live in [`NET_NAMESPACE`]; process names come from
/// `Win32_Process` in [`CIMV2`]. A missing endpoint class (older Windows)
/// degrades gracefully rather than failing the whole snapshot.
///
/// **Both halves go through the credential dispatcher.** The process-name half
/// used to call the `wmi` crate directly, which cannot carry a credential: on a
/// credentialed remote the endpoint rows came from the target while the PID→name
/// join ran against *this* machine's process table. The visible symptom was an
/// empty process column, which reads as "WMI didn't tell us" rather than "we
/// asked the wrong computer"; the invisible one was worse, since a PID that
/// happened to exist locally would have joined a wrong name onto a remote
/// connection.
fn list_connections() -> anyhow::Result<NetworkSnapshot> {
    // `Request::NetworkSnapshot` has no cancellation of its own yet, so this
    // path keeps the behaviour it had; the parameter exists for the provider
    // list, which does.
    let pid_name = process_names(&CancelToken::never())?;
    let mut connections = Vec::new();

    if let Ok(rows) = q_maps(
        NET_NAMESPACE,
        "SELECT LocalAddress,LocalPort,RemoteAddress,RemotePort,State,OwningProcess \
         FROM MSFT_NetTCPConnection",
    ) {
        connections.extend(
            rows.iter()
                .map(|r| to_connection(r, Protocol::Tcp, &pid_name)),
        );
    }

    if let Ok(rows) = q_maps(
        NET_NAMESPACE,
        "SELECT LocalAddress,LocalPort,OwningProcess FROM MSFT_NetUDPEndpoint",
    ) {
        connections.extend(
            rows.iter()
                .map(|r| to_connection(r, Protocol::Udp, &pid_name)),
        );
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
    let mut unreadable = Vec::new();
    for ns in SUBSCRIPTION_NAMESPACES {
        match scan_subscriptions_in(ns) {
            Ok(subs) => subscriptions.extend(subs),
            Err(e) => unreadable.push(format!("{ns}: {e}")),
        }
    }
    // Nothing scanned is not a clean bill of health, and this is the exact
    // shape the mistake takes: every namespace refused, every error swallowed
    // by an `if let Ok`, and an empty report handed back as a *successful*
    // scan. "No persistence found" and "we could not look" are opposite
    // answers, and a responder reading a green panel cannot tell them apart.
    // Measured, not imagined: with the worker on a credentialed target it
    // could not reach, this was the one request out of fourteen that still
    // answered (see `examples/altcred.rs`).
    if subscriptions.is_empty() && unreadable.len() == SUBSCRIPTION_NAMESPACES.len() {
        anyhow::bail!(
            "no subscription namespace could be scanned ({})",
            unreadable.join("; ")
        );
    }
    // Most-suspicious first.
    subscriptions.sort_by(|a, b| b.risk.cmp(&a.risk));
    Ok(SubscriptionReport {
        subscriptions,
        unreadable,
    })
}

/// Scan one namespace, or say why it could not be scanned.
///
/// All three enumerations are load-bearing, so any of them failing fails the
/// namespace rather than degrading a row. A scan that read the bindings but not
/// the consumers is worse than no scan: it produces rows with an empty
/// `consumer_type`, which [`assess`] can only score **Low** — a report full of
/// reassuring entries that describe nothing.
fn scan_subscriptions_in(namespace: &str) -> anyhow::Result<Vec<Subscription>> {
    let conn = bind(namespace)?;

    // Filters: Name -> query.
    let mut filters: HashMap<String, String> = HashMap::new();
    for r in scan_query(&conn, namespace, "SELECT Name, Query FROM __EventFilter")? {
        let name = r.get("Name").map(variant_to_string).unwrap_or_default();
        if !name.is_empty() {
            let query = r.get("Query").map(variant_to_string).unwrap_or_default();
            filters.insert(name, query);
        }
    }

    // Consumers: Name -> (concrete class, best-effort action).
    //
    // This walk was skipped entirely under alternate credentials, and the
    // consequence was not a missing column — it was a wrong verdict. Every
    // subscription came back with an empty `consumer_type` and an empty
    // `action`, and `assess("", query, "")` has nothing left to fire on, so it
    // returns **Low**. A `CommandLineEventConsumer` running an encoded
    // PowerShell payload — the textbook T1546.003 — was reported as low risk on
    // exactly the credentialed remote scan a responder would run during an
    // incident. A silent false negative on the flagship feature.
    //
    // The fix is to read the objects instead of flattened maps: `__CLASS` is a
    // system property and the enumeration flags hide it, so a map-returning
    // query cannot answer "which kind of consumer is this?" on either
    // transport. Both now take the same path.
    let mut consumers: HashMap<String, (String, String)> = HashMap::new();
    let (objects, completion) = conn.exec_objects(
        "SELECT * FROM __EventConsumer",
        None,
        Some(HELPER_QUERY_BUDGET),
        &CancelToken::never(),
    )?;
    if let Some(why) = completion.note() {
        anyhow::bail!("__EventConsumer in {namespace}: {why}");
    }
    for obj in &objects {
        let get = |p: &str| crate::reflect::string_property(obj, p);
        let name = get("Name");
        if name.is_empty() {
            continue;
        }
        let class = crate::reflect::class_name(obj);
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

    let mut subs = Vec::new();
    let mut bound_filters = std::collections::HashSet::new();
    let mut bound_consumers = std::collections::HashSet::new();

    // Bindings.
    for r in scan_query(
        &conn,
        namespace,
        "SELECT Filter, Consumer FROM __FilterToConsumerBinding",
    )? {
        let filter_name = first_quoted(&r.get("Filter").map(variant_to_string).unwrap_or_default());
        let consumer_name =
            first_quoted(&r.get("Consumer").map(variant_to_string).unwrap_or_default());
        bound_filters.insert(filter_name.clone());
        bound_consumers.insert(consumer_name.clone());
        let filter_query = filters.get(&filter_name).cloned().unwrap_or_default();
        let (consumer_type, action) = consumers.get(&consumer_name).cloned().unwrap_or_default();
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

    Ok(subs)
}

/// One query of a subscription scan, on a binding the caller already holds.
///
/// Separate from [`q_maps`] only because it reuses that binding: a scan issues
/// three queries per namespace, and `q_maps` opens a connection per call.
fn scan_query(
    conn: &Bound,
    namespace: &str,
    wql: &str,
) -> anyhow::Result<Vec<HashMap<String, Variant>>> {
    let en = conn.exec_enum(wql)?;
    let (rows, completion) = enumerate::drain(
        &en,
        None,
        Some(HELPER_QUERY_BUDGET),
        &CancelToken::never(),
        |o| unsafe { crate::remote::object_to_map(o, false) },
    )?;
    match completion.note() {
        None => Ok(rows),
        Some(why) => anyhow::bail!("{wql} in {namespace}: {why}"),
    }
}

/// PID → process name, from `Win32_Process` **on the connected host**.
fn process_names(cancel: &CancelToken) -> anyhow::Result<HashMap<u32, String>> {
    let procs = q_maps_within(
        CIMV2,
        "SELECT ProcessId, Name FROM Win32_Process",
        HELPER_QUERY_BUDGET,
        cancel,
    )?;
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
///
/// The longest-running request that is not a user's query — it reflects every
/// class in a namespace — so it is drained in batches like the rest rather than
/// pulled through an iterator that can only be checked one object at a time.
fn build_search_index(
    namespace: &str,
    include_methods: bool,
    cancel: &CancelToken,
) -> anyhow::Result<SearchIndex> {
    let conn = bind(namespace)?;
    let mut index = SearchIndex {
        namespace: namespace.to_string(),
        has_methods: include_methods,
        ..Default::default()
    };
    let en = conn.class_enum(None, true)?;
    let (entries, _) = enumerate::drain(&en, None, Some(CLASS_ENUM_BUDGET), cancel, |obj| {
        let class = crate::reflect::class_name(obj);
        let properties = wmi::IWbemClassWrapper::new(obj.clone())
            .list_properties()
            .unwrap_or_default();
        let methods = if include_methods {
            crate::reflect::enum_method_names(obj)
        } else {
            Vec::new()
        };
        Ok((class, properties, methods))
    })?;
    for (class, properties, methods) in entries {
        if class.is_empty() {
            continue;
        }
        index.properties.insert(class.clone(), properties);
        if !methods.is_empty() {
            index.methods.insert(class.clone(), methods);
        }
        index.classes.push(class);
    }
    index.classes.sort_unstable();
    index.classes.dedup();
    Ok(index)
}

/// Read a property under either the case WMI declares it or the case the plan
/// wrote it — `Msft_Providers` declares `provider` lowercase and `User` upper,
/// which is not a pattern anyone will remember correctly at every call site.
fn prop<'a>(row: &'a HashMap<String, Variant>, a: &str, b: &str) -> Option<&'a Variant> {
    row.get(a).or_else(|| row.get(b))
}

/// List WMI providers (`Msft_Providers`) and the processes hosting them.
///
/// Four sources, because the answer is not in one class:
///
/// 1. `Msft_Providers` in [`CIMV2`] — which provider, in which namespace, in
///    which PID, under which account.
/// 2. `Win32_Process` — the PID's image name.
/// 3. `__Win32Provider` in each provider's *own* namespace — the
///    `HostingModel` string. §5.11 of the plan places this on `Msft_Providers`;
///    it is not there (see [`ProviderInfo::hosting_model`]).
/// 4. [`provider_hosts`] — the live load and the quota bounding it.
///
/// Only the first is allowed to fail the request. A provider list without image
/// names or hosting models is degraded; a provider list that is not the
/// provider list is wrong.
fn list_providers(cancel: &CancelToken) -> anyhow::Result<(Vec<ProviderInfo>, ProviderHosts)> {
    let names = process_names(cancel).unwrap_or_default();
    let rows = q_maps_within(
        CIMV2,
        "SELECT provider, Namespace, HostProcessIdentifier, HostingGroup, \
         HostingSpecification, User FROM Msft_Providers",
        HELPER_QUERY_BUDGET,
        cancel,
    )?;
    let mut providers: Vec<ProviderInfo> = rows
        .into_iter()
        .map(|r| {
            let host_pid = r
                .get("HostProcessIdentifier")
                .map(variant_to_u32)
                .unwrap_or(0);
            ProviderInfo {
                provider: prop(&r, "provider", "Provider")
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
                hosting_model: String::new(),
                hosting_specification: r
                    .get("HostingSpecification")
                    .map(variant_to_u32)
                    .unwrap_or(0),
                user: prop(&r, "User", "user")
                    .map(variant_to_string)
                    .unwrap_or_default(),
            }
        })
        .collect();
    providers.sort_by(|a, b| a.provider.to_lowercase().cmp(&b.provider.to_lowercase()));

    let mut unreadable = Vec::new();
    add_hosting_models(&mut providers, &mut unreadable, cancel);
    let hosts = provider_hosts(&providers, unreadable, cancel);
    Ok((providers, hosts))
}

/// Fill in each provider's `HostingModel` from its registration.
///
/// One `__Win32Provider` query per distinct namespace, not per provider: five
/// of the eight providers on this machine live in `root\CIMV2`, so the
/// per-provider form would pay for that namespace five times.
///
/// Failures are recorded and skipped. A namespace whose `__Win32Provider` is
/// unreadable is common — `root\SECURITY` refuses an ordinary token — and the
/// providers hosted there are still worth listing without the string.
fn add_hosting_models(
    providers: &mut [ProviderInfo],
    unreadable: &mut Vec<String>,
    cancel: &CancelToken,
) {
    let mut namespaces: Vec<String> = providers
        .iter()
        .map(|p| p.namespace.clone())
        .filter(|n| !n.is_empty())
        .collect();
    namespaces.sort_by_key(|n| n.to_lowercase());
    namespaces.dedup_by_key(|n| n.to_lowercase());

    let started = Instant::now();
    // `(namespace, provider name)` both lowercased: WMI matches class and
    // property names case-insensitively, and `Msft_Providers` and
    // `__Win32Provider` do not agree on the case of a provider's own name.
    let mut models: HashMap<(String, String), String> = HashMap::new();
    for ns in namespaces {
        if cancel.is_raised() {
            unreadable.push("hosting models: cancelled".into());
            break;
        }
        let Some(left) = PROVIDER_ENRICH_BUDGET.checked_sub(started.elapsed()) else {
            unreadable.push(format!(
                "hosting models: budget of {} s spent before {ns}",
                PROVIDER_ENRICH_BUDGET.as_secs()
            ));
            break;
        };
        match q_maps_within(
            &ns,
            "SELECT Name, HostingModel FROM __Win32Provider",
            left,
            cancel,
        ) {
            Ok(rows) => {
                for r in rows {
                    let name = r.get("Name").map(variant_to_string).unwrap_or_default();
                    if name.is_empty() {
                        continue;
                    }
                    models.insert(
                        (ns.to_lowercase(), name.to_lowercase()),
                        r.get("HostingModel")
                            .map(variant_to_string)
                            .unwrap_or_default(),
                    );
                }
            }
            Err(e) => unreadable.push(format!("{ns}: __Win32Provider: {e}")),
        }
    }

    for p in providers.iter_mut() {
        if let Some(model) = models.get(&(p.namespace.to_lowercase(), p.provider.to_lowercase())) {
            p.hosting_model.clone_from(model);
        }
    }
}

/// Live load of every process hosting a provider, plus the quota it runs
/// against.
///
/// The join is `IDProcess`, and nothing else, for two measured reasons.
///
/// **One name, many hosts.** `Win32_Process` calls every live host on this
/// machine `WmiPrvSE.exe`; the perf class calls them `WmiPrvSE`, `WmiPrvSE#1`,
/// `WmiPrvSE#2`, `WmiPrvSE#3`. Joining on the `Win32_Process` name folds three
/// distinct processes into one row and attributes one host's leak to a sibling.
///
/// **And the perf name is a slot, not an identity.** Measured across samples
/// minutes apart on this machine: `WmiPrvSE#3` was PID 43468, that host exited,
/// and `WmiPrvSE#3` came back as PID 37048. The suffix is reused. Anything that
/// remembered "`WmiPrvSE#3` is the one leaking" would later be reading a
/// different process and calling it the same one.
///
/// The filter is by PID rather than the plan's `WHERE Name LIKE 'WmiPrvSE%'`
/// for the same reason it must not join by name: measured here, two of eight
/// providers are hosted in the WMI service itself (`svchost#13`, PID 2788), and
/// the name filter reports nothing at all for them.
fn provider_hosts(
    providers: &[ProviderInfo],
    mut unreadable: Vec<String>,
    cancel: &CancelToken,
) -> ProviderHosts {
    let pids = host_pids(providers);
    let mut hosts = ProviderHosts::default();

    match q_maps_within(ROOT_NAMESPACE, QUOTA_WQL, HELPER_QUERY_BUDGET, cancel) {
        Ok(rows) => match rows.first() {
            Some(r) => hosts.quota = Some(to_quota(r)),
            // The class exists on every Windows install; an empty result means
            // the singleton is genuinely absent, which is not the same as a
            // quota of zero and must not be rendered as one.
            None => unreadable.push(format!(
                "{ROOT_NAMESPACE}: __ProviderHostQuotaConfiguration: no instance"
            )),
        },
        Err(e) => unreadable.push(format!(
            "{ROOT_NAMESPACE}: __ProviderHostQuotaConfiguration: {e}"
        )),
    }

    // Needed to turn `PercentProcessorTime` into a share of the machine; a
    // failure costs the percentage, not the row.
    match q_maps_within(
        CIMV2,
        "SELECT NumberOfLogicalProcessors FROM Win32_ComputerSystem",
        HELPER_QUERY_BUDGET,
        cancel,
    ) {
        Ok(rows) => {
            hosts.logical_cpus = rows
                .first()
                .and_then(|r| r.get("NumberOfLogicalProcessors"))
                .map(variant_to_u32)
                .unwrap_or(0);
        }
        Err(e) => unreadable.push(format!("{CIMV2}: Win32_ComputerSystem: {e}")),
    }

    if !pids.is_empty() {
        match q_maps_within(CIMV2, &perf_wql(&pids), HELPER_QUERY_BUDGET, cancel) {
            Ok(rows) => {
                hosts.stats = rows
                    .iter()
                    .map(to_host_stats)
                    // The `WHERE` is an optimisation, not the filter: above
                    // `PERF_PID_FILTER_CAP` there is no `WHERE` at all, and a
                    // provider is always free to return more than it was asked
                    // for.
                    .filter(|h| pids.binary_search(&h.pid).is_ok())
                    .collect();
                hosts.stats.sort_by_key(|h| h.pid);
            }
            Err(e) => unreadable.push(format!(
                "{CIMV2}: Win32_PerfFormattedData_PerfProc_Process: {e}"
            )),
        }
    }

    hosts.unreadable = unreadable;
    hosts
}

/// The quota singleton. `MemoryAllHosts`/`ProcessLimitAllHosts` are read
/// alongside the three §5.13 asks for because they are properties of the same
/// object already in hand, and they answer the other half of the question:
/// whether the *machine* is out of provider-host budget rather than one host.
const QUOTA_WQL: &str = "SELECT MemoryPerHost, HandlesPerHost, ThreadsPerHost, MemoryAllHosts, \
                         ProcessLimitAllHosts FROM __ProviderHostQuotaConfiguration";

fn to_quota(r: &HashMap<String, Variant>) -> HostQuota {
    HostQuota {
        memory_per_host: r.get("MemoryPerHost").map(variant_to_u64).unwrap_or(0),
        handles_per_host: r.get("HandlesPerHost").map(variant_to_u32).unwrap_or(0),
        threads_per_host: r.get("ThreadsPerHost").map(variant_to_u32).unwrap_or(0),
        memory_all_hosts: r.get("MemoryAllHosts").map(variant_to_u64).unwrap_or(0),
        process_limit_all_hosts: r
            .get("ProcessLimitAllHosts")
            .map(variant_to_u32)
            .unwrap_or(0),
    }
}

fn to_host_stats(r: &HashMap<String, Variant>) -> HostStats {
    HostStats {
        pid: r.get("IDProcess").map(variant_to_u32).unwrap_or(0),
        instance: r.get("Name").map(variant_to_string).unwrap_or_default(),
        cpu_percent: r
            .get("PercentProcessorTime")
            .map(variant_to_u64)
            .unwrap_or(0),
        private_bytes: r.get("PrivateBytes").map(variant_to_u64).unwrap_or(0),
        working_set_private: r.get("WorkingSetPrivate").map(variant_to_u64).unwrap_or(0),
        handle_count: r.get("HandleCount").map(variant_to_u32).unwrap_or(0),
        thread_count: r.get("ThreadCount").map(variant_to_u32).unwrap_or(0),
    }
}

/// The perf query for a specific set of host PIDs.
///
/// WQL has no `IN`, so a PID set is an `OR` chain. Past
/// [`PERF_PID_FILTER_CAP`] the chain is dropped entirely and the caller filters
/// what comes back — the perf provider builds every counter instance regardless
/// of the `WHERE`, so the clause only ever saved marshalling, and a
/// several-hundred-term predicate is a worse thing to send than a few hundred
/// rows are to receive.
fn perf_wql(pids: &[u32]) -> String {
    const SELECT: &str = "SELECT Name, IDProcess, PercentProcessorTime, PrivateBytes, \
                          WorkingSetPrivate, HandleCount, ThreadCount \
                          FROM Win32_PerfFormattedData_PerfProc_Process";
    if pids.is_empty() || pids.len() > PERF_PID_FILTER_CAP {
        return SELECT.to_string();
    }
    let terms: Vec<String> = pids.iter().map(|p| format!("IDProcess={p}")).collect();
    format!("{SELECT} WHERE {}", terms.join(" OR "))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::reflect::AssocClassDef;

    fn def(class: &str, endpoints: &[(&str, &str)]) -> AssocClassDef {
        AssocClassDef {
            class: class.to_string(),
            endpoints: endpoints
                .iter()
                .map(|(r, t)| (r.to_string(), t.to_string()))
                .collect(),
        }
    }

    /// The shape almost every association has: two references, one of them
    /// ours. Copied from the live `root\CIMV2` definition.
    #[test]
    fn an_association_resolves_our_role_and_the_far_end() {
        let rows = assoc_rows(
            &[def(
                "Win32_SessionProcess",
                &[
                    ("Antecedent", "Win32_LogonSession"),
                    ("Dependent", "Win32_Process"),
                ],
            )],
            "Win32_Process",
            "",
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].assoc_class, "Win32_SessionProcess");
        assert_eq!(rows[0].role, "Dependent");
        assert_eq!(rows[0].target_class, "Win32_LogonSession");
        assert!(rows[0].note.is_empty());
    }

    /// An association reached through an ancestor says so, because the user is
    /// looking at `Win32_Process` and the relationship is declared on
    /// `CIM_Process`.
    #[test]
    fn an_inherited_association_carries_its_provenance() {
        let rows = assoc_rows(
            &[def(
                "CIM_ProcessExecutable",
                &[("Antecedent", "CIM_DataFile"), ("Dependent", "CIM_Process")],
            )],
            "CIM_Process",
            "inherited via CIM_Process",
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].target_class, "CIM_DataFile");
        assert_eq!(rows[0].note, "inherited via CIM_Process");
    }

    /// Both ends the same class: there is no "other" end, and dropping the row
    /// would hide the relationship entirely.
    #[test]
    fn a_self_referencing_association_still_produces_rows() {
        let rows = assoc_rows(
            &[def(
                "Win32_SubDirectory",
                &[
                    ("GroupComponent", "Win32_Directory"),
                    ("PartComponent", "Win32_Directory"),
                ],
            )],
            "Win32_Directory",
            "",
        );
        // One row per role, each pointing back at the same class.
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.target_class == "Win32_Directory"));
        assert!(rows.iter().all(|r| r.note == "self-referencing"));
        let roles: Vec<&str> = rows.iter().map(|r| r.role.as_str()).collect();
        assert_eq!(roles, vec!["GroupComponent", "PartComponent"]);
    }

    /// WMI returned an association that references us, but no reference
    /// property names us. Report it rather than silently drop it.
    #[test]
    fn an_unresolved_role_is_reported_not_dropped() {
        let rows = assoc_rows(
            &[def(
                "Odd_Association",
                &[("Left", "A_Class"), ("Right", "B_Class")],
            )],
            "Win32_Process",
            "",
        );
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.note == "role not resolved"));
    }

    /// The predicate is per-PID. A `Name LIKE 'WmiPrvSE%'` filter would be
    /// shorter and would miss the two providers this machine hosts inside the
    /// WMI service, whose counter instance is `svchost#13`.
    #[test]
    fn the_perf_filter_names_pids_and_never_process_names() {
        let wql = perf_wql(&[2788, 10772]);
        assert!(
            wql.contains("WHERE IDProcess=2788 OR IDProcess=10772"),
            "{wql}"
        );
        assert!(!wql.contains("Name="), "{wql}");
        assert!(!wql.to_lowercase().contains("like"), "{wql}");
        // The projection is what `to_host_stats` reads; a missing column is a
        // silent zero, not an error.
        for col in [
            "IDProcess",
            "PercentProcessorTime",
            "PrivateBytes",
            "WorkingSetPrivate",
            "HandleCount",
            "ThreadCount",
        ] {
            assert!(wql.contains(col), "{col} missing from {wql}");
        }
    }

    /// Past the cap the `OR` chain is dropped rather than grown, and the caller
    /// filters instead. Empty likewise: no PIDs must not produce `WHERE `.
    #[test]
    fn an_oversized_pid_set_drops_the_predicate() {
        let many: Vec<u32> = (1..=(PERF_PID_FILTER_CAP as u32 + 1)).collect();
        assert!(!perf_wql(&many).contains("WHERE"));
        assert!(!perf_wql(&[]).contains("WHERE"));
        let at_cap: Vec<u32> = (1..=PERF_PID_FILTER_CAP as u32).collect();
        assert!(perf_wql(&at_cap).contains("WHERE"));
    }

    /// `Msft_Providers` declares `provider` in lower case and `User` in upper,
    /// and WMI is free to hand back either — the lookup has to accept both or
    /// a real column reads as empty.
    #[test]
    fn a_property_is_found_under_either_case() {
        let r = row(&[("User", Variant::String("HOST\\root".into()))]);
        assert_eq!(
            prop(&r, "User", "user").map(variant_to_string),
            Some("HOST\\root".to_string())
        );
        let lower = row(&[("user", Variant::String("HOST\\root".into()))]);
        assert_eq!(
            prop(&lower, "User", "user").map(variant_to_string),
            Some("HOST\\root".to_string())
        );
        assert!(prop(&row(&[]), "User", "user").is_none());
    }

    #[test]
    fn notes_join_without_stray_separators() {
        assert_eq!(join_notes("", ""), "");
        assert_eq!(join_notes("a", ""), "a");
        assert_eq!(join_notes("", "b"), "b");
        assert_eq!(join_notes("a", "b"), "a; b");
    }

    fn row(pairs: &[(&str, Variant)]) -> HashMap<String, Variant> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    fn one_process() -> HashMap<u32, String> {
        HashMap::from([(4321, "svchost.exe".to_string())])
    }

    /// The join is on `OwningProcess`, and the ports arrive as widened
    /// `uint16`s. Both are easy to get subtly wrong and neither would show up
    /// as an error -- a mis-narrowed port is just a different port.
    #[test]
    fn a_tcp_row_joins_its_process_by_pid() {
        let c = to_connection(
            &row(&[
                ("LocalAddress", Variant::String("10.0.0.5".into())),
                ("LocalPort", Variant::UI4(49_800)),
                ("RemoteAddress", Variant::String("140.82.121.6".into())),
                ("RemotePort", Variant::UI4(443)),
                ("State", Variant::UI4(5)),
                ("OwningProcess", Variant::UI4(4321)),
            ]),
            Protocol::Tcp,
            &one_process(),
        );
        assert_eq!(c.pid, 4321);
        assert_eq!(c.process, "svchost.exe");
        assert_eq!(c.local_port, 49_800);
        assert_eq!(c.remote_port, 443);
        assert_eq!(c.state, "Established");
        assert!(c.is_external());
    }

    /// UDP is connectionless. Carrying `0.0.0.0:0` and a state name over from
    /// the TCP shape would put endpoints in the table that do not exist.
    #[test]
    fn a_udp_row_has_no_peer_and_no_state() {
        let c = to_connection(
            &row(&[
                ("LocalAddress", Variant::String("0.0.0.0".into())),
                ("LocalPort", Variant::UI4(5353)),
                // A provider is free to return these; UDP still has no peer.
                ("RemoteAddress", Variant::String("1.2.3.4".into())),
                ("RemotePort", Variant::UI4(9)),
                ("State", Variant::UI4(5)),
                ("OwningProcess", Variant::UI4(4321)),
            ]),
            Protocol::Udp,
            &one_process(),
        );
        assert_eq!(c.local_port, 5353);
        assert!(c.remote_addr.is_empty());
        assert_eq!(c.remote_port, 0);
        assert!(c.state.is_empty());
        assert_eq!(c.process, "svchost.exe");
        assert!(!c.is_external());
    }

    /// A PID the process table does not contain leaves the name **blank**.
    ///
    /// This is the shape the credential bug had: the endpoints came from the
    /// remote host and the process table from this one, so nearly every PID
    /// missed and the column emptied. Blank has to stay blank -- inventing a
    /// name for an unmatched PID would turn a visible gap into an invisible
    /// lie, and the row itself must survive, because the connection whose
    /// owner cannot be read is the interesting one.
    #[test]
    fn an_unknown_pid_leaves_the_process_name_empty_and_keeps_the_row() {
        let c = to_connection(
            &row(&[
                ("LocalAddress", Variant::String("10.0.0.5".into())),
                ("LocalPort", Variant::UI4(445)),
                ("OwningProcess", Variant::UI4(999)),
                ("State", Variant::UI4(2)),
            ]),
            Protocol::Tcp,
            &one_process(),
        );
        assert_eq!(c.pid, 999);
        assert_eq!(c.process, "");
        assert_eq!(c.state, "Listen");
        assert_eq!(c.local_port, 445);
    }

    /// An endpoint row with no `OwningProcess` at all still becomes a row.
    #[test]
    fn a_row_without_an_owning_process_still_lists_the_endpoint() {
        let c = to_connection(
            &row(&[("LocalAddress", Variant::String("::".into()))]),
            Protocol::Udp,
            &one_process(),
        );
        assert_eq!(c.pid, 0);
        assert_eq!(c.process, "");
        assert_eq!(c.local_addr, "::");
    }

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

    #[test]
    fn single_class_is_pulled_from_the_from_clause() {
        assert_eq!(
            single_class_from_wql("SELECT * FROM Win32_Service"),
            Some("Win32_Service".to_string())
        );
        // Case-insensitive keyword, a WHERE clause, and extra whitespace.
        assert_eq!(
            single_class_from_wql("select Name  from   Win32_Service where State='Running'"),
            Some("Win32_Service".to_string())
        );
        // A trailing separator with no space is trimmed to the identifier.
        assert_eq!(
            single_class_from_wql("SELECT * FROM Win32_Service;"),
            Some("Win32_Service".to_string())
        );
    }

    #[test]
    fn queries_with_no_single_instance_class_yield_no_key() {
        // No FROM at all.
        assert_eq!(
            single_class_from_wql("ASSOCIATORS OF {Win32_Service.Name='Spooler'}"),
            None
        );
        // The reflection pseudo-class is not an instance class.
        assert_eq!(single_class_from_wql("SELECT * FROM meta_class"), None);
        // A quoted object path after FROM is not a bare class identifier.
        assert_eq!(single_class_from_wql("SELECT * FROM \"root\\cimv2\""), None);
        // FROM with nothing after it.
        assert_eq!(single_class_from_wql("SELECT * FROM"), None);
    }
}
