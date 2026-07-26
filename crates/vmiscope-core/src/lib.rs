//! Core WMI access layer for **VMI-Scope**.
//!
//! This crate is intentionally GUI-agnostic. It exposes:
//!  - [`value`]: conversion of raw WMI variants into display-friendly strings.
//!  - [`worker`]: a background thread that owns the COM apartment and answers
//!    [`Request`]s with [`Response`]s over channels, so the UI never blocks.

pub mod diff;
pub mod events;
pub mod export;
pub mod method;
pub mod monitor;
pub mod network;
pub mod providers;
pub mod reflect;
pub mod remote;
pub mod schema;
pub mod value;
pub mod worker;

pub use diff::{diff_providers, diff_subscriptions, ProviderDiff, SubscriptionDiff};
pub use events::{Risk, Subscription, SubscriptionReport};
pub use method::{param_kind, MethodArg, MethodOutcome, MethodTarget, ParamKind};
pub use monitor::{EventMonitor, MonitorMsg, DEFAULT_EVENT_QUERY};
pub use network::{Connection, NetworkSnapshot, Protocol};
pub use providers::ProviderInfo;
pub use remote::{Credential, RemoteConn};
pub use schema::{
    ClassKind, ClassSchema, MethodSchema, ParamSchema, PropertySchema, SearchHit, SearchIndex,
};
pub use worker::{QueryResult, Request, Response, WmiWorker};
