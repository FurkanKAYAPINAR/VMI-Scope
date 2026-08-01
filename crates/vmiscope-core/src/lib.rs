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
pub mod method;
pub mod monitor;
pub mod network;
pub mod process;
pub mod procmon;
pub mod providers;
pub mod reflect;
pub mod remote;
pub mod schema;
pub mod sid;
pub mod value;
pub mod worker;

pub use diff::{diff_providers, diff_subscriptions, ProviderDiff, SubscriptionDiff};
pub use elevation::is_elevated;
pub use enumerate::{CancelToken, Completion, WorkerControl, BATCH_SIZE, BATCH_TIMEOUT_MS};
pub use events::{Risk, Subscription, SubscriptionReport};
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
pub use providers::ProviderInfo;
pub use remote::{Credential, RemoteConn};
pub use schema::{
    ClassKind, ClassSchema, MethodSchema, ParamSchema, PropertySchema, SearchHit, SearchIndex,
};
pub use sid::{resolve_sid, SidResolver};
pub use worker::{QueryResult, Request, Response, WmiWorker};
