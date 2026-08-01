//! Chunked, cancellable WMI enumeration.
//!
//! A WQL query such as `SELECT * FROM CIM_DataFile` walks the whole filesystem.
//! Pulled the obvious way — one `IEnumWbemClassObject::Next(WBEM_INFINITE, ..)`
//! per object, as both the `wmi` crate's iterator and [`crate::remote`] do — it
//! parks the single COM thread inside a call that nothing can interrupt, and
//! every later request (including `Shutdown`) waits behind it.
//!
//! [`drain`] is the answer: batches of [`BATCH_SIZE`] objects, a **finite**
//! `lTimeout` per batch, and a look at two atomic flags between batches. The
//! COM call itself is still uninterruptible, so the worst-case stall is one
//! batch — [`BATCH_TIMEOUT_MS`] — rather than the lifetime of the query.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::CO_E_NOTINITIALIZED;
use windows::Win32::System::Com::{
    CoCreateInstance, CoSetProxyBlanket, CLSCTX_INPROC_SERVER, EOAC_NONE, RPC_C_AUTHN_LEVEL,
    RPC_C_AUTHN_LEVEL_CALL, RPC_C_AUTHN_LEVEL_PKT_PRIVACY, RPC_C_IMP_LEVEL_IMPERSONATE,
};
use windows::Win32::System::Rpc::{RPC_C_AUTHN_WINNT, RPC_C_AUTHZ_NONE};
use windows::Win32::System::Wmi::{
    IEnumWbemClassObject, IWbemClassObject, IWbemContext, IWbemLocator, IWbemServices, WbemLocator,
    WBEM_FLAG_CONNECT_USE_MAX_WAIT, WBEM_FLAG_FORWARD_ONLY, WBEM_FLAG_RETURN_IMMEDIATELY,
    WBEM_S_FALSE,
};
use wmi::WMIConnection;

/// Objects requested per `IEnumWbemClassObject::Next` call.
pub const BATCH_SIZE: usize = 64;

/// Per-batch `lTimeout` in milliseconds, and therefore the longest a cancelled
/// or shutting-down enumeration can keep the worker thread.
///
/// Never `WBEM_INFINITE`: on timeout `Next` returns `WBEM_S_TIMEDOUT`, which is
/// a *success* HRESULT carrying however many objects it managed to collect, and
/// that return is the only chance the loop gets to read the cancellation flags.
pub const BATCH_TIMEOUT_MS: i32 = 200;

/// Why an enumeration stopped.
///
/// A capped or cancelled result is a *partial* result and says so. A silently
/// short table is worse than no table at all: nothing downstream can tell the
/// difference between "the machine has 5,000 files" and "we stopped counting".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub enum Completion {
    /// The enumerator ran dry; every matching row is present.
    #[default]
    Complete,
    /// Stopped at `cap` rows with more still available.
    Truncated { cap: usize },
    /// Stopped because the request was cancelled or the worker is shutting down.
    Cancelled,
    /// Stopped because the deadline passed.
    ///
    /// A row cap cannot bound wall-clock time on its own, and the difference is
    /// not academic. Measured here: `SELECT * FROM Win32_Process` capped at 5
    /// returns in 36 ms, but `SELECT * FROM CIM_DataFile` capped at 200 returns
    /// *nothing at all* in 45 s -- that provider materialises its whole result
    /// before yielding a single row, so there is never anything to count and
    /// the cap can never fire. Only a deadline bounds it.
    TimedOut { after_ms: u64, rows: usize },
}

impl Completion {
    /// Is this a full result?
    pub fn is_complete(&self) -> bool {
        matches!(self, Completion::Complete)
    }

    /// A short human-readable reason, or `None` when the result is complete.
    pub fn note(&self) -> Option<String> {
        match self {
            Completion::Complete => None,
            Completion::Truncated { cap } => Some(format!("truncated at the {cap}-row cap")),
            Completion::Cancelled => Some("cancelled".to_string()),
            Completion::TimedOut { after_ms, rows } => Some(format!(
                "timed out after {:.1}s with {rows} row{}",
                *after_ms as f64 / 1000.0,
                if *rows == 1 { "" } else { "s" }
            )),
        }
    }
}

/// The pair of flags a running enumeration reads between batches.
///
/// Two, not one, because they have different scopes: `request` stops a single
/// query, `shutdown` stops the whole worker and is shared by every token.
#[derive(Clone, Debug)]
pub struct CancelToken {
    request: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
}

impl CancelToken {
    /// A token nothing can raise. For enumerations that are known to be small.
    pub fn never() -> Self {
        Self {
            request: Arc::new(AtomicBool::new(false)),
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Has either flag been raised?
    pub fn is_raised(&self) -> bool {
        self.request.load(Ordering::Relaxed) || self.shutdown.load(Ordering::Relaxed)
    }

    /// Raise this token's request flag directly (the worker uses
    /// [`WorkerControl::cancel`]; this exists for tests and for callers that
    /// already hold the token).
    pub fn cancel(&self) {
        self.request.store(true, Ordering::Relaxed);
    }
}

/// Out-of-band control plane for the WMI worker thread.
///
/// Shared, not owned by the worker, and that is the whole point: the worker is
/// inside an uninterruptible COM call precisely when someone wants to cancel
/// it, so a `Cancel` routed through the request channel would queue behind the
/// very query it is meant to stop. Raising a flag needs no cooperation from the
/// worker at all.
#[derive(Clone, Debug, Default)]
pub struct WorkerControl {
    shutdown: Arc<AtomicBool>,
    live: Arc<Mutex<HashMap<u64, Arc<AtomicBool>>>>,
}

impl WorkerControl {
    /// A fresh control plane with nothing cancelled.
    pub fn new() -> Self {
        Self::default()
    }

    /// Raise the flag for request `id`.
    ///
    /// Safe to call before the request starts: the flag is stored under `id`
    /// and [`WorkerControl::begin`] adopts it, so a cancel that overtakes a
    /// still-queued query is honoured rather than lost.
    pub fn cancel(&self, id: u64) {
        self.live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(id)
            .or_default()
            .store(true, Ordering::Relaxed);
    }

    /// Raise the priority stop flag. Every live token reads it, so all running
    /// enumerations abort within one batch.
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }

    /// Has shutdown been requested?
    pub fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::Relaxed)
    }

    /// Register `id` and hand back its token. Called on the worker thread as a
    /// cancellable request starts.
    pub fn begin(&self, id: u64) -> CancelToken {
        let flag = self.map().entry(id).or_default().clone();
        CancelToken {
            request: flag,
            shutdown: self.shutdown.clone(),
        }
    }

    /// Forget `id`'s flag. Called when the request finishes, and again when the
    /// queued `Cancel` message finally surfaces — which is what keeps a cancel
    /// for an already-finished id from leaking an entry.
    pub fn end(&self, id: u64) {
        self.map().remove(&id);
    }

    /// How many request flags are currently registered.
    pub fn live_count(&self) -> usize {
        self.map().len()
    }

    /// A poisoned lock must not disable cancellation — the map is a bag of
    /// flags, so there is no invariant left broken by a panicking holder.
    fn map(&self) -> MutexGuard<'_, HashMap<u64, Arc<AtomicBool>>> {
        self.live.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// How many objects the next batch should ask for, or `None` once the cap is
/// satisfied.
///
/// One row *past* the cap is requested on purpose. Stopping exactly at `cap`
/// would leave truncation a guess — a query returning exactly `max_rows` rows
/// is complete, not truncated — and the difference is the whole point of
/// reporting it.
fn next_batch(collected: usize, max_rows: Option<usize>) -> Option<usize> {
    match max_rows {
        None => Some(BATCH_SIZE),
        Some(cap) if collected > cap => None,
        Some(cap) => Some((cap - collected + 1).min(BATCH_SIZE)),
    }
}

/// Decide the outcome and drop the probe row that [`next_batch`] asked for.
///
/// Order matters where two conditions are true at once. Cancellation wins
/// because the user asked and should not be told a different story; the
/// deadline comes next because a timed-out result is partial for a reason the
/// cap cannot explain; truncation is last.
fn settle<T>(
    rows: &mut Vec<T>,
    max_rows: Option<usize>,
    cancelled: bool,
    timed_out: Option<u64>,
) -> Completion {
    let mut over = None;
    if let Some(cap) = max_rows {
        if rows.len() > cap {
            over = Some(cap);
            rows.truncate(cap);
        }
    }
    if cancelled {
        return Completion::Cancelled;
    }
    if let Some(after_ms) = timed_out {
        return Completion::TimedOut {
            after_ms,
            rows: rows.len(),
        };
    }
    match over {
        Some(cap) => Completion::Truncated { cap },
        None => Completion::Complete,
    }
}

/// Pull every object out of `en` in bounded batches, converting each with
/// `make`, and stop early on the cap or on either cancellation flag.
///
/// The cap check comes before the cancellation check so that a query which
/// exactly fills its budget is reported as `Truncated`, not `Cancelled`, when
/// both happen to be true on the same turn.
pub(crate) fn drain<T>(
    en: &IEnumWbemClassObject,
    max_rows: Option<usize>,
    deadline: Option<Duration>,
    cancel: &CancelToken,
    mut make: impl FnMut(&IWbemClassObject) -> anyhow::Result<T>,
) -> anyhow::Result<(Vec<T>, Completion)> {
    let mut rows: Vec<T> = Vec::new();
    let mut cancelled = false;
    let mut timed_out = None;
    let started = Instant::now();

    loop {
        let Some(want) = next_batch(rows.len(), max_rows) else {
            break;
        };
        if cancel.is_raised() {
            cancelled = true;
            break;
        }
        // Checked per batch, like the flags, so the worst case is one batch
        // timeout of overshoot rather than one provider.
        if let Some(limit) = deadline {
            let spent = started.elapsed();
            if spent >= limit {
                timed_out = Some(spent.as_millis() as u64);
                break;
            }
        }

        let mut objs: [Option<IWbemClassObject>; BATCH_SIZE] = std::array::from_fn(|_| None);
        let mut returned = 0u32;
        // `Next` takes the requested count from the slice length.
        let hr = unsafe { en.Next(BATCH_TIMEOUT_MS, &mut objs[..want], &mut returned) };
        hr.ok()?;

        for slot in objs.iter_mut().take(returned as usize) {
            if let Some(obj) = slot.take() {
                rows.push(make(&obj)?);
            }
        }

        // Three success codes land here. `WBEM_S_FALSE` is the only one that
        // means "no more rows, ever"; `WBEM_S_NO_ERROR` means the batch was
        // filled and `WBEM_S_TIMEDOUT` means it was not filled *yet*. Treating
        // a short batch as the end — which a `returned == 0` test would do —
        // would silently truncate every slow provider.
        if hr.0 == WBEM_S_FALSE.0 {
            break;
        }
    }

    let completion = settle(&mut rows, max_rows, cancelled, timed_out);
    Ok((rows, completion))
}

/// A raw `IWbemServices` bound to one namespace.
///
/// The `wmi` crate keeps its own `IWbemServices` `pub(crate)` and its query
/// iterator hard-codes `WBEM_INFINITE`, so a chunked enumeration cannot be
/// built on top of it — hence this thin parallel connector. It covers the
/// local and current-user-SSO cases; alternate credentials keep going through
/// [`crate::remote::RemoteConn`], which additionally has to re-blanket every
/// proxy it hands out.
pub(crate) struct DirectConn {
    svc: IWbemServices,
}

impl DirectConn {
    /// Bind `namespace` on `host` (or the local machine when `host` is `None`).
    pub(crate) fn open(host: Option<&str>, namespace: &str) -> anyhow::Result<Self> {
        let resource = match host {
            Some(h) => format!(r"\\{h}\{namespace}"),
            None => namespace.to_string(),
        };
        // Match the `wmi` crate's levels exactly so switching a request onto
        // this path cannot change what the caller is allowed to read.
        let auth = if host.is_some() {
            RPC_C_AUTHN_LEVEL_PKT_PRIVACY
        } else {
            RPC_C_AUTHN_LEVEL_CALL
        };

        let loc = create_locator()?;
        unsafe {
            // Empty BSTRs are null pointers, i.e. "current user", "no locale",
            // "no authority" — the documented way to ask for SSO.
            let svc: IWbemServices = loc.ConnectServer(
                &windows::core::BSTR::from(resource),
                &windows::core::BSTR::new(),
                &windows::core::BSTR::new(),
                &windows::core::BSTR::new(),
                WBEM_FLAG_CONNECT_USE_MAX_WAIT.0,
                &windows::core::BSTR::new(),
                None::<&IWbemContext>,
            )?;
            set_blanket(&svc, auth)?;
            Ok(Self { svc })
        }
    }

    /// Start a WQL enumeration. `FORWARD_ONLY | RETURN_IMMEDIATELY` makes this
    /// semi-synchronous: `ExecQuery` returns before the provider has finished,
    /// which is what lets [`drain`] interleave flag checks with the work.
    pub(crate) fn exec_enum(&self, wql: &str) -> anyhow::Result<IEnumWbemClassObject> {
        unsafe {
            Ok(self.svc.ExecQuery(
                &windows::core::BSTR::from("WQL"),
                &windows::core::BSTR::from(wql),
                WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
                None::<&IWbemContext>,
            )?)
        }
        // The enumerator is deliberately *not* re-blanketed: on the SSO path
        // the process-wide default from `CoInitializeSecurity` applies, which
        // is exactly what the `wmi` crate relies on. Only the explicit
        // `COAUTHIDENTITY` of the alternate-credential path has to be pushed
        // onto each new proxy by hand.
    }

    /// Open an event subscription and hand back the raw enumerator.
    ///
    /// The same reason [`DirectConn`] exists at all applies twice over here.
    /// `wmi`'s `exec_notification_query` returns an opaque iterator whose
    /// `next()` pulls one object per `Next(WBEM_INFINITE, ..)`, and this
    /// monitor needs **two** subscriptions merged on **one** thread — which is
    /// impossible with a blocking pull, because whichever stream is quiet parks
    /// the thread and starves the other. Owning the enumerator allows a finite
    /// (in practice zero) `lTimeout`, so both can be drained in turn and the
    /// stop flag can be read between rounds.
    ///
    /// The error is returned unmapped, as a `windows::core::Error`: the access
    /// denial that gates `Win32_ProcessStartTrace` surfaces from
    /// `ExecNotificationQuery` itself, and [`crate::procmon`] needs the HRESULT
    /// intact to classify it.
    pub(crate) fn exec_notification(
        &self,
        wql: &str,
    ) -> windows::core::Result<IEnumWbemClassObject> {
        unsafe {
            self.svc.ExecNotificationQuery(
                &windows::core::BSTR::from("WQL"),
                &windows::core::BSTR::from(wql),
                WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
                None::<&IWbemContext>,
            )
        }
    }
}

unsafe fn set_blanket(svc: &IWbemServices, auth: RPC_C_AUTHN_LEVEL) -> anyhow::Result<()> {
    CoSetProxyBlanket(
        svc,
        RPC_C_AUTHN_WINNT,
        RPC_C_AUTHZ_NONE,
        None,
        auth,
        RPC_C_IMP_LEVEL_IMPERSONATE,
        None,
        EOAC_NONE,
    )?;
    Ok(())
}

/// Create the WBEM locator, bootstrapping COM on this thread if needed.
///
/// The bootstrap is delegated to `WMIConnection::new()` rather than
/// re-implemented: it already performs `CoIncrementMTAUsage` plus the default
/// `CoInitializeSecurity` and tolerates `RPC_E_TOO_LATE`, and having two
/// different COM security policies in one process would be a bug waiting to
/// happen. It runs at most once per thread, because COM stays initialized.
fn create_locator() -> anyhow::Result<IWbemLocator> {
    match unsafe { CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER) } {
        Ok(loc) => Ok(loc),
        Err(e) if e.code() == CO_E_NOTINITIALIZED => {
            let _bootstrap = WMIConnection::new()?;
            Ok(unsafe { CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER)? })
        }
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uncapped_batches_are_full_size() {
        assert_eq!(next_batch(0, None), Some(BATCH_SIZE));
        assert_eq!(next_batch(100_000, None), Some(BATCH_SIZE));
    }

    #[test]
    fn a_cap_asks_for_exactly_one_probe_row() {
        // 5 wanted -> ask for 6, so a 6th row proves there were more.
        assert_eq!(next_batch(0, Some(5)), Some(6));
        assert_eq!(next_batch(4, Some(5)), Some(2));
        assert_eq!(next_batch(5, Some(5)), Some(1));
    }

    #[test]
    fn a_cap_larger_than_a_batch_is_clamped() {
        assert_eq!(next_batch(0, Some(5_000)), Some(BATCH_SIZE));
        assert_eq!(next_batch(4_990, Some(5_000)), Some(11));
    }

    #[test]
    fn the_probe_row_ends_the_loop() {
        assert_eq!(next_batch(6, Some(5)), None);
        assert_eq!(next_batch(1, Some(0)), None);
        // A zero cap still costs one round trip: that is how truncation is
        // told apart from an empty result set.
        assert_eq!(next_batch(0, Some(0)), Some(1));
    }

    #[test]
    fn settle_drops_the_probe_row_and_reports_truncation() {
        let mut rows: Vec<u32> = (0..6).collect();
        assert_eq!(
            settle(&mut rows, Some(5), false, None),
            Completion::Truncated { cap: 5 }
        );
        assert_eq!(rows, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn exactly_the_cap_is_complete_not_truncated() {
        let mut rows: Vec<u32> = (0..5).collect();
        assert_eq!(
            settle(&mut rows, Some(5), false, None),
            Completion::Complete
        );
        assert_eq!(rows.len(), 5);
    }

    #[test]
    fn no_cap_is_never_truncated() {
        let mut rows: Vec<u32> = (0..1000).collect();
        assert_eq!(settle(&mut rows, None, false, None), Completion::Complete);
        assert_eq!(rows.len(), 1000);
    }

    #[test]
    fn cancellation_outranks_truncation_and_keeps_the_partial_rows() {
        let mut rows: Vec<u32> = (0..3).collect();
        assert_eq!(
            settle(&mut rows, Some(5), true, None),
            Completion::Cancelled
        );
        assert_eq!(rows.len(), 3);
    }

    #[test]
    fn completion_notes_are_only_written_for_partial_results() {
        assert!(Completion::Complete.note().is_none());
        assert!(Completion::Complete.is_complete());
        assert_eq!(
            Completion::Truncated { cap: 5000 }.note().unwrap(),
            "truncated at the 5000-row cap"
        );
        assert_eq!(Completion::Cancelled.note().unwrap(), "cancelled");
    }

    #[test]
    fn a_token_starts_down_and_stays_up() {
        let control = WorkerControl::new();
        let token = control.begin(7);
        assert!(!token.is_raised());
        control.cancel(7);
        assert!(token.is_raised());
        // Ending the request forgets the flag, but the token already handed out
        // keeps reading the same `Arc` -- an in-flight loop is never un-cancelled.
        control.end(7);
        assert!(token.is_raised());
        assert_eq!(control.live_count(), 0);
    }

    #[test]
    fn a_cancel_that_overtakes_its_request_is_not_lost() {
        let control = WorkerControl::new();
        control.cancel(9); // request 9 is still sitting in the channel
        let token = control.begin(9);
        assert!(token.is_raised());
    }

    #[test]
    fn cancelling_one_request_leaves_the_others_running() {
        let control = WorkerControl::new();
        let a = control.begin(1);
        let b = control.begin(2);
        control.cancel(1);
        assert!(a.is_raised());
        assert!(!b.is_raised());
        assert_eq!(control.live_count(), 2);
    }

    #[test]
    fn shutdown_raises_every_token_including_later_ones() {
        let control = WorkerControl::new();
        let early = control.begin(1);
        control.shutdown();
        let late = control.begin(2);
        assert!(control.is_shutdown());
        assert!(early.is_raised());
        assert!(late.is_raised());
    }

    #[test]
    fn a_never_token_is_independent_of_any_control() {
        let control = WorkerControl::new();
        control.shutdown();
        assert!(!CancelToken::never().is_raised());
    }

    #[test]
    fn ending_an_unknown_id_is_harmless() {
        let control = WorkerControl::new();
        control.end(42);
        control.cancel(42);
        control.end(42);
        assert_eq!(control.live_count(), 0);
    }
}
