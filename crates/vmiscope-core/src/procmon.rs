//! Live process start/stop monitor, with a degraded fallback.
//!
//! # The two modes
//!
//! **Trace (wanted).** `Win32_ProcessStartTrace` and `Win32_ProcessStopTrace`
//! are `__ExtrinsicEvent`-derived: the WMI Kernel Trace Event Provider pushes
//! them, so there is no `WITHIN`, no polling, and no window in which a process
//! can live and die unseen.
//!
//! **Intrinsic (degraded).** `__InstanceCreationEvent`/`__InstanceDeletionEvent
//! WITHIN n` over `Win32_Process`. This is a *sampler*: it compares snapshots n
//! seconds apart, so a process that starts and exits between two samples is
//! never reported. Measured on this machine, that lost 67 of 72 instant-exit
//! processes — 93% — with a positive control proving the subscription itself
//! was healthy (`docs/FINDINGS.md`).
//!
//! # Why the fallback exists
//!
//! On this machine's UAC-filtered admin token the trace subscription is refused
//! outright with `WBEM_E_ACCESS_DENIED (0x80041003)`. The denial is specific to
//! that provider rather than to extrinsic events in general —
//! `Win32_VolumeChangeEvent` is also extrinsic and subscribes fine unelevated —
//! and it arrives from `ExecNotificationQuery`, not from the iterator, so it is
//! caught at the subscribe call.
//!
//! **Whether elevation lifts the denial is untested.** No elevated session was
//! ever available to check, and the design deliberately does not depend on the
//! answer: the monitor tries trace, takes whatever it gets, and reports the
//! mode it settled on so the UI can say plainly that it is degraded. It is
//! never a silent downgrade — a security tool that quietly stops seeing 93% of
//! short-lived processes is worse than one that refuses to start.
//!
//! # Threading
//!
//! One pump thread owns both subscriptions and its own COM connection. A second
//! thread does everything that can block — SID resolution (which may reach a
//! domain controller) and the `Win32_Process` enrichment query — because a pump
//! that stops to ask questions loses events.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use windows::Win32::Foundation::E_ACCESSDENIED;
use windows::Win32::System::Wmi::{
    IEnumWbemClassObject, IWbemClassObject, WBEM_E_ACCESS_DENIED, WBEM_E_PRIVILEGE_NOT_HELD,
    WBEM_S_FALSE,
};
use wmi::{IWbemClassWrapper, Variant, WMIConnection};

use crate::enumerate::DirectConn;
use crate::process::{enrich_process, Enrichment, ProcEvent, ProcInfo, ProcKind};
use crate::sid::SidResolver;

/// The namespace both modes live in.
pub const PROCESS_NAMESPACE: &str = "root\\CIMV2";

/// Extrinsic, provider-pushed. No `WITHIN`: there is nothing to poll.
pub const TRACE_START_QUERY: &str = "SELECT * FROM Win32_ProcessStartTrace";
pub const TRACE_STOP_QUERY: &str = "SELECT * FROM Win32_ProcessStopTrace";

/// `WITHIN n` for the degraded fallback. Two seconds matches the existing
/// event monitor's default; lowering it does not meaningfully help (measured
/// `WITHIN 1` still caught 0 of 15).
pub const DEFAULT_FALLBACK_WITHIN_SECS: u32 = 2;

/// Objects requested per `Next`. Large enough that a burst drains in a few
/// calls, small enough that the array costs nothing on the stack.
const PUMP_BATCH: usize = 32;

/// How long the pump sleeps when both subscriptions are empty.
///
/// This is a *pump* interval, not a sampling interval, and the difference is
/// the whole point: the enumerator queues events as the provider delivers them,
/// so a slower pump adds latency and never loses an event. It is also the
/// worst-case time between a stop request and the thread noticing.
const IDLE_POLL_MS: u64 = 50;

/// Beyond this many events waiting on the details thread, enrichment is shed.
///
/// Enrichment is a per-event WMI round trip and is strictly optional; the event
/// stream is not. Under a burst the queue is drained without querying, so the
/// pump is never throttled by the slowest part of the pipeline.
const ENRICH_BACKLOG_MAX: usize = 512;

/// Why a subscription could not be established.
///
/// A typed variant rather than a stringified HRESULT because exactly one of
/// these has a specific answer for the operator, and `0x80041003` in a status
/// bar does not give it to them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MonitorError {
    /// The kernel trace provider refused the subscription.
    ///
    /// Measured on a UAC-filtered admin token; whether an elevated token lifts
    /// it is **unverified** — no elevated session has ever been observed. So
    /// this names what happened, not what will fix it.
    NeedsElevation { query: String, hresult: i32 },
    /// The namespace could not be bound at all.
    Connect { message: String },
    /// Anything else, with the HRESULT preserved for a bug report.
    Wmi { hresult: i32, message: String },
}

impl std::fmt::Display for MonitorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MonitorError::NeedsElevation { query, hresult } => write!(
                f,
                "access denied ({hresult:#010x}) subscribing to `{query}` \
                 - the WMI Kernel Trace Event Provider refused this token"
            ),
            MonitorError::Connect { message } => {
                write!(f, "cannot bind {PROCESS_NAMESPACE}: {message}")
            }
            MonitorError::Wmi { hresult, message } => write!(f, "{message} ({hresult:#010x})"),
        }
    }
}

impl std::error::Error for MonitorError {}

/// Which subscription the monitor settled on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MonitorMode {
    /// Extrinsic trace events: complete, no polling gap.
    Trace,
    /// Degraded: polled intrinsic events, carrying the refusal that caused it.
    Intrinsic {
        within_secs: u32,
        reason: MonitorError,
    },
}

impl MonitorMode {
    /// Is this the lossy mode? The UI must say so when it is.
    pub fn is_degraded(&self) -> bool {
        matches!(self, MonitorMode::Intrinsic { .. })
    }

    /// One line naming the mode and, when degraded, what it costs.
    pub fn summary(&self) -> String {
        match self {
            MonitorMode::Trace => {
                "extrinsic Win32_ProcessStartTrace/StopTrace - no polling gap".to_string()
            }
            MonitorMode::Intrinsic {
                within_secs,
                reason,
            } => format!(
                "degraded: polled __InstanceCreationEvent/__InstanceDeletionEvent WITHIN \
                 {within_secs} - short-lived processes will be missed (measured ~93% of \
                 instant-exit processes); {reason}"
            ),
        }
    }
}

/// A message from the process monitor.
#[derive(Debug, Clone)]
pub enum ProcMsg {
    /// Always first: which subscription was established.
    Mode(MonitorMode),
    /// One process start or stop. `seq` is a monotonic per-monitor event id.
    Event { seq: u64, event: ProcEvent },
    /// Everything that had to be looked up after the fact for event `seq`.
    ///
    /// Separate from [`ProcMsg::Event`] because it arrives later, on another
    /// thread, and may arrive after the process has already stopped. A consumer
    /// that has already marked the row ended must still attach it.
    Details {
        seq: u64,
        /// `DOMAIN\user`, SDDL when unresolvable, empty when the event carried
        /// no SID at all.
        user: String,
        enrichment: Enrichment,
    },
    /// A non-fatal problem worth telling the operator about.
    Error(String),
}

/// A unit of work for the details thread.
struct DetailsJob {
    seq: u64,
    event: ProcEvent,
    /// Already known, so no query is needed. The intrinsic fallback gets the
    /// command line for free on the `TargetInstance`; the trace path does not.
    known: Option<ProcInfo>,
}

/// Handle to a running process monitor. Dropping it stops the threads.
pub struct ProcessMonitor {
    rx: Receiver<ProcMsg>,
    stop: Arc<AtomicBool>,
    mode: Arc<Mutex<Option<MonitorMode>>>,
    handle: Option<JoinHandle<()>>,
}

impl ProcessMonitor {
    /// Start monitoring with the default fallback interval.
    pub fn start() -> ProcessMonitor {
        ProcessMonitor::start_with(DEFAULT_FALLBACK_WITHIN_SECS)
    }

    /// Start monitoring, trying the extrinsic trace subscription first and
    /// falling back to intrinsic `WITHIN within_secs` if it is refused.
    pub fn start_with(within_secs: u32) -> ProcessMonitor {
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let mode: Arc<Mutex<Option<MonitorMode>>> = Arc::new(Mutex::new(None));

        let thread_stop = stop.clone();
        let thread_mode = mode.clone();
        let handle = thread::Builder::new()
            .name("proc-monitor".into())
            .spawn(move || run(tx, thread_stop, thread_mode, within_secs))
            .expect("failed to spawn process monitor thread");

        ProcessMonitor {
            rx,
            stop,
            mode,
            handle: Some(handle),
        }
    }

    /// The mode the monitor settled on, or `None` while it is still connecting.
    ///
    /// Also delivered as the first [`ProcMsg`]; this accessor exists so a UI can
    /// ask at any later frame without having had to remember the message.
    pub fn mode(&self) -> Option<MonitorMode> {
        self.mode.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Drain everything received since the last poll. Never blocks.
    pub fn poll(&self) -> Vec<ProcMsg> {
        self.rx.try_iter().collect()
    }

    /// Ask the pump to stop. Idempotent; [`Drop`] does it too.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for ProcessMonitor {
    fn drop(&mut self) {
        self.stop();
        // Joining is safe here, unlike in [`crate::monitor::EventMonitor`],
        // which cannot join because its thread is parked in an infinite `Next`
        // until the next event happens to arrive. This pump never blocks for
        // longer than `IDLE_POLL_MS`, so the wait is bounded and short.
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// The pump thread.
fn run(
    tx: Sender<ProcMsg>,
    stop: Arc<AtomicBool>,
    mode_slot: Arc<Mutex<Option<MonitorMode>>>,
    within_secs: u32,
) {
    let conn = match DirectConn::open(None, PROCESS_NAMESPACE) {
        Ok(c) => c,
        Err(e) => {
            let err = MonitorError::Connect {
                message: e.to_string(),
            };
            let _ = tx.send(ProcMsg::Error(err.to_string()));
            return;
        }
    };

    let (mode, start_en, stop_en) = match subscribe(&conn, within_secs) {
        Ok(v) => v,
        Err(e) => {
            let _ = tx.send(ProcMsg::Error(e.to_string()));
            return;
        }
    };
    let trace = mode == MonitorMode::Trace;
    *mode_slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(mode.clone());
    if tx.send(ProcMsg::Mode(mode)).is_err() {
        return;
    }

    // Details run on their own thread with their own COM connection, so a slow
    // `LookupAccountSidW` or `Win32_Process` query can never stall the pump.
    let backlog = Arc::new(AtomicUsize::new(0));
    let (job_tx, job_rx) = mpsc::channel::<DetailsJob>();
    let details_tx = tx.clone();
    let details_backlog = backlog.clone();
    let details = thread::Builder::new()
        .name("proc-details".into())
        .spawn(move || run_details(job_rx, details_tx, details_backlog));
    if let Err(e) = &details {
        let _ = tx.send(ProcMsg::Error(format!(
            "details thread could not start ({e}); command lines and user names \
             will be unavailable"
        )));
    }

    let mut seq: u64 = 0;
    while !stop.load(Ordering::Relaxed) {
        let mut saw_any = false;
        for (en, kind) in [(&start_en, ProcKind::Start), (&stop_en, ProcKind::Stop)] {
            match take_available(en) {
                Ok(objs) => {
                    saw_any |= !objs.is_empty();
                    for obj in objs {
                        let Some((event, known)) = read_event(&obj, kind, trace) else {
                            continue;
                        };
                        seq += 1;
                        if tx
                            .send(ProcMsg::Event {
                                seq,
                                event: event.clone(),
                            })
                            .is_err()
                        {
                            return; // receiver gone
                        }
                        // Unbounded and never blocking: shedding happens on the
                        // consuming side, where the queue depth is known.
                        backlog.fetch_add(1, Ordering::Relaxed);
                        if job_tx.send(DetailsJob { seq, event, known }).is_err() {
                            backlog.fetch_sub(1, Ordering::Relaxed);
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.send(ProcMsg::Error(format!(
                        "{} subscription ended: {e}",
                        kind.as_str()
                    )));
                    return;
                }
            }
        }
        if !saw_any {
            thread::sleep(Duration::from_millis(IDLE_POLL_MS));
        }
    }
    // Dropping `job_tx` closes the details channel, so that thread finishes its
    // current job and exits on its own. It is deliberately not joined: it may
    // be inside a WMI call, and shutdown must stay bounded.
}

/// Establish both subscriptions, falling back to intrinsic on a refusal.
///
/// Both subscriptions must succeed for trace mode. A half-fallback — trace
/// starts with intrinsic stops — would be worse than either mode alone: every
/// row would appear promptly and then linger for up to `n` seconds after the
/// process had already gone.
fn subscribe(
    conn: &DirectConn,
    within_secs: u32,
) -> Result<(MonitorMode, IEnumWbemClassObject, IEnumWbemClassObject), MonitorError> {
    match try_pair(conn, TRACE_START_QUERY, TRACE_STOP_QUERY) {
        Ok((a, b)) => Ok((MonitorMode::Trace, a, b)),
        Err(reason) => {
            // Only a privilege refusal is worth degrading for. Anything else
            // (a malformed query, a broken repository) would fail identically
            // on the fallback, and pretending otherwise would hide the real
            // fault behind a plausible-looking downgrade.
            if !matches!(reason, MonitorError::NeedsElevation { .. }) {
                return Err(reason);
            }
            let (a, b) = try_pair(
                conn,
                &intrinsic_query("__InstanceCreationEvent", within_secs),
                &intrinsic_query("__InstanceDeletionEvent", within_secs),
            )?;
            Ok((
                MonitorMode::Intrinsic {
                    within_secs,
                    reason,
                },
                a,
                b,
            ))
        }
    }
}

fn try_pair(
    conn: &DirectConn,
    start: &str,
    stop: &str,
) -> Result<(IEnumWbemClassObject, IEnumWbemClassObject), MonitorError> {
    let a = conn
        .exec_notification(start)
        .map_err(|e| classify(start, &e))?;
    let b = conn
        .exec_notification(stop)
        .map_err(|e| classify(stop, &e))?;
    Ok((a, b))
}

/// The degraded query. `WITHIN n` is what makes it a sampler, and what makes it
/// lossy: the provider compares snapshots n seconds apart.
fn intrinsic_query(class: &str, within_secs: u32) -> String {
    format!("SELECT * FROM {class} WITHIN {within_secs} WHERE TargetInstance ISA 'Win32_Process'")
}

/// Turn a subscribe-time HRESULT into a typed error.
///
/// Three codes mean the same thing to an operator. `WBEM_E_ACCESS_DENIED` is
/// what this machine actually returns; `E_ACCESSDENIED` is what a DCOM-layer
/// refusal looks like; `WBEM_E_PRIVILEGE_NOT_HELD` is what WMI returns when a
/// required privilege is present but disabled. Collapsing them is honest —
/// each one means "this token was not allowed to do that".
fn classify(query: &str, e: &windows::core::Error) -> MonitorError {
    let hresult = e.code().0;
    if hresult == WBEM_E_ACCESS_DENIED.0
        || hresult == WBEM_E_PRIVILEGE_NOT_HELD.0
        || hresult == E_ACCESSDENIED.0
    {
        MonitorError::NeedsElevation {
            query: query.to_string(),
            hresult,
        }
    } else {
        MonitorError::Wmi {
            hresult,
            message: e.message(),
        }
    }
}

/// Pull everything already queued on `en`, without waiting.
///
/// `lTimeout = 0` is what makes two subscriptions shareable by one thread. With
/// a blocking timeout, whichever stream is quiet would park the thread and
/// starve the other; with zero, each is drained to empty in turn and the caller
/// decides when to sleep.
fn take_available(en: &IEnumWbemClassObject) -> windows::core::Result<Vec<IWbemClassObject>> {
    let mut out = Vec::new();
    loop {
        let mut objs: [Option<IWbemClassObject>; PUMP_BATCH] = std::array::from_fn(|_| None);
        let mut returned = 0u32;
        let hr = unsafe { en.Next(0, &mut objs, &mut returned) };
        hr.ok()?;
        for slot in objs.iter_mut().take(returned as usize) {
            if let Some(obj) = slot.take() {
                out.push(obj);
            }
        }
        // A short batch means the queue is empty for now — for a notification
        // enumerator that is `WBEM_S_TIMEDOUT`, not end-of-stream. Only
        // `WBEM_S_FALSE` would mean the subscription is finished, which for an
        // event query effectively never happens.
        if hr.0 == WBEM_S_FALSE.0 || (returned as usize) < PUMP_BATCH {
            return Ok(out);
        }
    }
}

/// Read one event object into a [`ProcEvent`], plus anything it already knew.
///
/// The two modes differ here and nowhere else. A trace event is flat: every
/// property is a scalar on the event object itself, because `__ExtrinsicEvent`
/// has no `TargetInstance`. An intrinsic event is the opposite — everything of
/// interest is inside the embedded `Win32_Process` instance, which as a bonus
/// carries the `CommandLine` and `ExecutablePath` that the trace classes do not
/// have, so the degraded mode needs no enrichment query at all.
fn read_event(
    obj: &IWbemClassObject,
    kind: ProcKind,
    trace: bool,
) -> Option<(ProcEvent, Option<ProcInfo>)> {
    let wrapper = IWbemClassWrapper::new(obj.clone());
    if trace {
        return Some((ProcEvent::from_map(kind, &props_of(&wrapper)), None));
    }
    let Ok(Variant::Object(inner)) = wrapper.get_property("TargetInstance") else {
        return None;
    };
    let props = props_of(&inner);
    let event = ProcEvent::from_map(kind, &props);
    let info = ProcInfo {
        command_line: props
            .get("CommandLine")
            .map(crate::value::variant_to_string)
            .unwrap_or_default(),
        executable_path: props
            .get("ExecutablePath")
            .map(crate::value::variant_to_string)
            .unwrap_or_default(),
    };
    // Both blank is not "we know it has no command line" — it is WMI declining
    // to show one, which it does for every process this token does not own.
    // Reporting that as `Found` with empty strings would claim knowledge we do
    // not have, so it is handed back as unknown and the details thread asks
    // properly (and reports `Unavailable` when the answer is still no).
    let known = (!info.command_line.is_empty() || !info.executable_path.is_empty()).then_some(info);
    Some((event, known))
}

/// Flatten one WMI object into a property map.
fn props_of(w: &IWbemClassWrapper) -> HashMap<String, Variant> {
    let mut map = HashMap::new();
    for name in w.list_properties().unwrap_or_default() {
        if let Ok(v) = w.get_property(&name) {
            map.insert(name, v);
        }
    }
    map
}

/// The details thread: SID resolution and the enrichment query.
///
/// Both are things the pump must never do. `LookupAccountSidW` can reach a
/// domain controller, and enrichment is a full WMI round trip per event.
fn run_details(jobs: Receiver<DetailsJob>, tx: Sender<ProcMsg>, backlog: Arc<AtomicUsize>) {
    // Its own connection: `WMIConnection` is `!Send` and COM apartments are
    // thread-affine, so it cannot be borrowed from the pump.
    let conn = WMIConnection::with_namespace_path(PROCESS_NAMESPACE).ok();
    let mut resolver = SidResolver::new();

    for job in jobs {
        // The depth *after* taking this job. Read here rather than on the
        // sending side because only the consumer knows how far behind it is.
        let depth = backlog.fetch_sub(1, Ordering::Relaxed).saturating_sub(1);
        let affordable = depth <= ENRICH_BACKLOG_MAX;

        // Cheap and cached: the same handful of SIDs repeat forever.
        let mut user = resolver.resolve(&job.event.sid);
        // Blank means the event carried no SID, which in practice means the
        // degraded path: `Win32_Process` has no owner property, only a
        // `GetOwner` method. Falling back to it costs a round trip and races
        // the process exiting, which is exactly why the trace classes carrying
        // the SID inline is the better design — but a blank user column in the
        // only mode an unelevated operator can reach is worse.
        if user.is_empty() && job.event.kind == ProcKind::Start && affordable {
            if let Some(c) = &conn {
                user = crate::process::process_owner(c, job.event.pid).unwrap_or_default();
            }
        }

        let enrichment = match (&job.known, &conn) {
            // The intrinsic path already carried it on the TargetInstance.
            (Some(info), _) => Enrichment::Found(info.clone()),
            (None, Some(c)) if affordable => enrich_process(c, &job.event),
            _ => Enrichment::Skipped,
        };

        if tx
            .send(ProcMsg::Details {
                seq: job.seq,
                user,
                enrichment,
            })
            .is_err()
        {
            return; // receiver gone
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_trace_queries_carry_no_within_clause() {
        // `WITHIN` on an extrinsic class is meaningless at best; its presence
        // would be a sign someone had copied the intrinsic query.
        assert!(!TRACE_START_QUERY.contains("WITHIN"));
        assert!(!TRACE_STOP_QUERY.contains("WITHIN"));
        assert!(TRACE_START_QUERY.contains("Win32_ProcessStartTrace"));
        assert!(TRACE_STOP_QUERY.contains("Win32_ProcessStopTrace"));
    }

    #[test]
    fn the_intrinsic_queries_do_carry_one() {
        let q = intrinsic_query("__InstanceCreationEvent", 2);
        assert_eq!(
            q,
            "SELECT * FROM __InstanceCreationEvent WITHIN 2 \
             WHERE TargetInstance ISA 'Win32_Process'"
        );
        assert!(intrinsic_query("__InstanceDeletionEvent", 5).contains("WITHIN 5"));
    }

    #[test]
    fn the_denial_maps_to_a_typed_variant_not_a_raw_hresult() {
        let denied =
            windows::core::Error::from_hresult(windows::core::HRESULT(WBEM_E_ACCESS_DENIED.0));
        let err = classify(TRACE_START_QUERY, &denied);
        assert!(matches!(err, MonitorError::NeedsElevation { .. }));
        match err {
            MonitorError::NeedsElevation { hresult, query } => {
                // 0x80041003, the value measured on this machine.
                assert_eq!(hresult as u32, 0x8004_1003);
                assert_eq!(query, TRACE_START_QUERY);
            }
            other => panic!("expected NeedsElevation, got {other:?}"),
        }
    }

    #[test]
    fn a_dcom_level_denial_and_a_missing_privilege_map_the_same_way() {
        for code in [E_ACCESSDENIED.0, WBEM_E_PRIVILEGE_NOT_HELD.0] {
            let e = windows::core::Error::from_hresult(windows::core::HRESULT(code));
            assert!(
                matches!(
                    classify(TRACE_START_QUERY, &e),
                    MonitorError::NeedsElevation { .. }
                ),
                "{code:#010x} should be treated as a privilege problem"
            );
        }
    }

    #[test]
    fn an_unrelated_failure_keeps_its_hresult_and_does_not_trigger_fallback() {
        // WBEM_E_INVALID_CLASS -- a real bug, not a privilege problem. Falling
        // back here would hide it behind a plausible-looking degraded mode.
        let e = windows::core::Error::from_hresult(windows::core::HRESULT(0x8004_1010u32 as i32));
        match classify(TRACE_START_QUERY, &e) {
            MonitorError::Wmi { hresult, .. } => assert_eq!(hresult as u32, 0x8004_1010),
            other => panic!("expected Wmi, got {other:?}"),
        }
    }

    #[test]
    fn degraded_mode_states_what_it_costs() {
        let trace = MonitorMode::Trace;
        assert!(!trace.is_degraded());
        assert!(!trace.summary().contains("degraded"));

        let fallback = MonitorMode::Intrinsic {
            within_secs: 2,
            reason: MonitorError::NeedsElevation {
                query: TRACE_START_QUERY.into(),
                hresult: WBEM_E_ACCESS_DENIED.0,
            },
        };
        assert!(fallback.is_degraded());
        let s = fallback.summary();
        assert!(s.contains("degraded"));
        assert!(s.contains("WITHIN 2"));
        // The measured cost has to be in the message, not in a doc comment.
        assert!(s.contains("93%"));
    }

    #[test]
    fn errors_render_without_leaking_a_bare_hresult_as_the_whole_message() {
        let e = MonitorError::NeedsElevation {
            query: TRACE_START_QUERY.into(),
            hresult: WBEM_E_ACCESS_DENIED.0,
        };
        let s = e.to_string();
        assert!(s.contains("access denied"));
        assert!(s.contains("0x80041003"));
        assert!(s.contains("Kernel Trace Event Provider"));
    }
}
