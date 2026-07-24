//! Core WMI access layer for **VMI-Scope**.
//!
//! This crate is intentionally GUI-agnostic. It exposes:
//!  - [`value`]: conversion of raw WMI variants into display-friendly strings.
//!  - [`worker`]: a background thread that owns the COM apartment and answers
//!    [`Request`]s with [`Response`]s over channels, so the UI never blocks.

pub mod network;
pub mod value;
pub mod worker;

pub use network::{Connection, NetworkSnapshot, Protocol};
pub use worker::{QueryResult, Request, Response, WmiWorker};
