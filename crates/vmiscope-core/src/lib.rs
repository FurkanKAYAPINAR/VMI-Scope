//! Core WMI access layer for **VMI-Scope**.
//!
//! This crate is intentionally GUI-agnostic. It exposes:
//!  - [`value`]: conversion of raw WMI variants into display-friendly strings.
//!  - [`worker`]: a background thread that owns the COM apartment and answers
//!    [`Request`]s with [`Response`]s over channels, so the UI never blocks.
//!  - [`enumerate`]: chunked, cancellable enumeration — the reason an
//!    unbounded query can no longer wedge that thread.

pub mod diff;
pub mod elevation;
pub mod enumerate;
pub mod events;
pub mod export;
pub mod host;
pub mod method;
pub mod monitor;
pub mod network;
pub mod process;
pub mod procmon;
pub mod providers;
pub mod reflect;
pub mod registry;
pub mod remote;
pub mod schema;
pub mod script;
pub mod sid;
pub mod value;
pub mod worker;

pub use diff::{
    diff_providers, diff_subscriptions, diff_tables, DiffRow, ProviderDiff, RowDelta,
    SubscriptionDiff, TableDiff,
};
pub use elevation::is_elevated;
pub use enumerate::{CancelToken, Completion, WorkerControl, BATCH_SIZE, BATCH_TIMEOUT_MS};
pub use events::{Risk, Subscription, SubscriptionReport};
pub use host::{HostInfo, HostRef, Impersonation};
pub use method::{param_kind, MethodArg, MethodOutcome, MethodTarget, ParamKind};
pub use monitor::{EventMonitor, MonitorMsg, DEFAULT_EVENT_QUERY};
pub use network::{Connection, NetworkSnapshot, Protocol};
pub use process::{
    cim_datetime_to_filetime, enrich_process, filetime_to_unix_secs, process_owner, Enrichment,
    ProcEvent, ProcInfo, ProcKind, ProcRow,
};
pub use procmon::{
    MonitorError, MonitorMode, ProcMsg, ProcessMonitor, DEFAULT_FALLBACK_WITHIN_SECS,
    TRACE_START_QUERY, TRACE_STOP_QUERY,
};
pub use providers::{host_pids, HostQuota, HostStats, ProviderHosts, ProviderInfo, QuotaKind};
pub use registry::WorkerRegistry;
pub use remote::{Credential, RemoteConn};
pub use schema::{
    AssocInfo, ClassBrief, ClassKind, ClassSchema, MethodSchema, NamespaceStats, ParamSchema,
    PropertySchema, SearchHit, SearchIndex, SkipReason, Tally,
};
pub use script::{generate_script, ScriptLang};
pub use sid::{resolve_sid, SidResolver};
pub use worker::{
    QueryResult, Request, Response, WmiWorker, ASSOCIATIONS_BUDGET, CIMV2, CLASS_ENUM_BUDGET,
    HELPER_QUERY_BUDGET, INSTANCE_COUNT_BUDGET, INSTANCE_LIST_BUDGET, INSTANCE_LIST_CAP,
    NAMESPACE_STATS_BUDGET, NET_NAMESPACE, PERF_PID_FILTER_CAP, PROVIDER_ENRICH_BUDGET,
    ROOT_NAMESPACE, SUBSCRIPTION_NAMESPACES,
};
