//! The Process view's row model: what ran, what it did, and what became of it.
//!
//! The rule this file exists to enforce is that **an ended process does not
//! disappear**. It dims and stays. The Network view fades a closed connection
//! and then drops it after six seconds, which is right there -- a closed socket
//! is not evidence of much. A process that ran and exited is exactly the thing
//! you came to look at, and the question the view answers ("what ran on this
//! box while I wasn't watching?") is unanswerable if the answer scrolls itself
//! away.
//!
//! Memory is bounded instead by an explicit cap and an explicit Clear, never by
//! silent expiry -- and the cap drops ended rows only, oldest first, so a live
//! process is never evicted by history.
//!
//! The view that renders this lands in the next commit; until then several of
//! these are only read by the tests.
#![allow(dead_code)]

use std::collections::HashMap;

use vmiscope_core::{filetime_to_unix_secs, Enrichment, ProcEvent, ProcKind};

/// How long an ended row takes to reach [`DIM_FLOOR`].
pub(crate) const FADE_SECS: f64 = 6.0;

/// How dim an ended row settles at, and stays.
///
/// Not zero, and not close to it: the row has to remain readable, because the
/// point of keeping it is that someone will want to read it later. This is the
/// difference between "faded out" and "faded back".
pub(crate) const DIM_FLOOR: f32 = 0.35;

/// Default row cap. Roughly a day of ordinary desktop churn.
pub(crate) const DEFAULT_MAX_ROWS: usize = 5_000;

/// A broken-down instant, **in UTC**.
///
/// Named for its timezone rather than left implicit, because there is no honest
/// way to produce a local one here. Converting UTC to the machine's local time
/// needs the current DST rule for the current zone, and `std` carries neither a
/// zone database nor an API for the platform's. Everything downstream therefore
/// labels this `UTC` on screen; a timestamp that silently means something other
/// than what it is labelled is worse than one an operator has to offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Utc {
    pub(crate) year: i64,
    pub(crate) month: u32,
    pub(crate) day: u32,
    pub(crate) hour: u32,
    pub(crate) minute: u32,
    pub(crate) second: u32,
}

impl Utc {
    /// `HH:MM:SS` -- the time column, which has room for nothing more.
    pub(crate) fn hms(self) -> String {
        format!("{:02}:{:02}:{:02}", self.hour, self.minute, self.second)
    }

    /// `YYYY-MM-DD HH:MM:SS UTC` -- the tooltip, where the date matters.
    pub(crate) fn full(self) -> String {
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }
}

/// Break a Windows `FILETIME` down into a UTC calendar instant.
///
/// `ticks` is 100 ns since 1601-01-01 UTC, which is what
/// `ProcEvent::time_created` carries on both paths: the trace classes deliver
/// it directly, and the polled fallback derives it from the instance's
/// `CreationDate`, which `vmiscope_core::cim_datetime_to_filetime` has already
/// normalised out of its local offset. So this is UTC in, UTC out, with no zone
/// arithmetic anywhere in between.
///
/// `None` for anything at or before the Unix epoch: `ProcEvent` uses `0` for
/// "the provider gave us no creation time at all", and a process that started in
/// 1601 is not a thing that happens. That distinction is the point -- rendering
/// an unknown time as `1601-01-01` would be a fabricated fact on a screen whose
/// whole job is to say when something ran.
pub(crate) fn utc_from_filetime(ticks: u64) -> Option<Utc> {
    let unix = filetime_to_unix_secs(ticks);
    if unix <= 0.0 {
        return None;
    }
    let secs = unix as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    Some(Utc {
        year,
        month,
        day,
        hour: (rem / 3600) as u32,
        minute: ((rem % 3600) / 60) as u32,
        second: (rem % 60) as u32,
    })
}

/// The proleptic-Gregorian date `days` after 1970-01-01.
///
/// Howard Hinnant's era-based `civil_from_days`, the exact inverse of the
/// `days_from_civil` the core already uses for the other direction. Written out
/// rather than taken as a dependency for the same reason it is there: this is
/// one conversion, and a calendar crate would be a supply-chain entry for two
/// dozen lines of integer arithmetic.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11], March-based
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// One process, alive or not.
#[derive(Debug, Clone)]
pub(crate) struct TrackedProc {
    pub(crate) pid: u32,
    pub(crate) parent_pid: u32,
    pub(crate) name: String,
    pub(crate) session_id: u32,
    /// Resolved owner, or empty until the details message arrives.
    pub(crate) user: String,
    pub(crate) command_line: Enrichment,
    /// App time when the start event arrived.
    pub(crate) started_at: f64,
    /// The event's own `TIME_CREATED`, a FILETIME. `0` when the provider gave
    /// none.
    ///
    /// This is the row's only link to a wall clock, and it exists because
    /// `started_at` cannot be one: the frame clock starts at zero when the
    /// window opens, so `T+04:12` answers "how long ago" and nothing else. The
    /// question this view is for -- "what ran at 03:14?" -- needs the other
    /// axis. Core consumes the same field for the pid-reuse guard and used to
    /// drop it here; the log now keys on it *and* carries it.
    pub(crate) created_filetime: u64,
    /// App time when the stop event arrived, if it has.
    pub(crate) ended_at: Option<f64>,
    pub(crate) exit_status: Option<u32>,
    /// Sequence number from the monitor, so late-arriving detail can find its
    /// row even after the row has ended.
    pub(crate) seq: u64,
}

impl TrackedProc {
    pub(crate) fn is_alive(&self) -> bool {
        self.ended_at.is_none()
    }

    /// When this process started, on the wall clock, in UTC. `None` when the
    /// event carried no creation time -- see [`utc_from_filetime`].
    pub(crate) fn started_utc(&self) -> Option<Utc> {
        utc_from_filetime(self.created_filetime)
    }

    /// When it ended, on the same clock.
    ///
    /// Derived rather than carried: the stop event's own `TIME_CREATED` is the
    /// stop instant, but it is not kept -- `stop()` matches on pid, so the row
    /// never sees that event again. Adding the measured lifetime to the start is
    /// exact to the resolution of the frame clock the two app-times came from,
    /// which is the same resolution the Duration column already reports.
    pub(crate) fn ended_utc(&self) -> Option<Utc> {
        let ended = self.ended_at?;
        let elapsed = (ended - self.started_at).max(0.0);
        let ticks = self
            .created_filetime
            .checked_add((elapsed * 10_000_000.0) as u64)?;
        utc_from_filetime(ticks)
    }

    /// How long it ran, in seconds, or how long it has been running.
    pub(crate) fn lifetime(&self, now: f64) -> f64 {
        self.ended_at.unwrap_or(now) - self.started_at
    }

    /// Row opacity: full while alive, easing to [`DIM_FLOOR`] once ended and
    /// then holding there.
    pub(crate) fn alpha(&self, now: f64) -> f32 {
        let Some(ended) = self.ended_at else {
            return 1.0;
        };
        let t = ((now - ended) / FADE_SECS).clamp(0.0, 1.0) as f32;
        // Ease out, so the row dims quickly enough to read as an event and then
        // settles rather than crawling.
        let eased = 1.0 - (1.0 - t) * (1.0 - t);
        1.0 - eased * (1.0 - DIM_FLOOR)
    }

    /// A non-zero exit status is worth colouring; 0 and "still running" are not.
    pub(crate) fn failed(&self) -> bool {
        matches!(self.exit_status, Some(code) if code != 0)
    }
}

/// Identity of a row.
///
/// Keyed on the pid **and** its start time, never the pid alone: Windows
/// recycles pids aggressively, and a bare-pid key would let a new process
/// silently overwrite the record of the one that just exited -- losing exactly
/// the history this view exists to keep.
type Key = (u32, u64);

/// Every process the view knows about, in arrival order.
#[derive(Debug)]
pub(crate) struct ProcessLog {
    rows: Vec<TrackedProc>,
    /// Key -> index into `rows`.
    index: HashMap<Key, usize>,
    /// seq -> index, for detail that arrives after the event.
    by_seq: HashMap<u64, usize>,
    pub(crate) max_rows: usize,
    /// Ended rows dropped to stay under the cap. Surfaced in the UI, because a
    /// silently truncated history is worse than a stated one.
    pub(crate) dropped: usize,
}

/// Hand-written rather than derived, because a derived `Default` leaves
/// `max_rows` at 0 -- and a zero cap evicts every ended row the instant it
/// arrives, which is the exact opposite of what this module exists to do. The
/// failure would be silent and would look like "the monitor isn't catching
/// exits".
impl Default for ProcessLog {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            index: HashMap::new(),
            by_seq: HashMap::new(),
            max_rows: DEFAULT_MAX_ROWS,
            dropped: 0,
        }
    }
}

impl ProcessLog {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn rows(&self) -> &[TrackedProc] {
        &self.rows
    }

    pub(crate) fn len(&self) -> usize {
        self.rows.len()
    }

    pub(crate) fn live_count(&self) -> usize {
        self.rows.iter().filter(|r| r.is_alive()).count()
    }

    /// Fold one monitor event in: a start appends a row, a stop closes the
    /// matching one.
    pub(crate) fn apply(&mut self, seq: u64, event: &ProcEvent, now: f64) {
        match event.kind {
            ProcKind::Start => self.start(seq, event, now),
            ProcKind::Stop => self.stop(event, now),
        }
    }

    fn start(&mut self, seq: u64, event: &ProcEvent, now: f64) {
        let key = (event.pid, event.time_created);
        if self.index.contains_key(&key) {
            return; // a duplicate delivery, not a second process
        }
        let at = self.rows.len();
        self.rows.push(TrackedProc {
            pid: event.pid,
            parent_pid: event.parent_pid,
            name: event.name.clone(),
            session_id: event.session_id,
            user: String::new(),
            command_line: Enrichment::Pending,
            started_at: now,
            created_filetime: event.time_created,
            ended_at: None,
            exit_status: None,
            seq,
        });
        self.index.insert(key, at);
        self.by_seq.insert(seq, at);
        self.evict();
    }

    /// Close the most recent live row for this pid.
    ///
    /// The stop event's `time_created` is the *stop* time, so it cannot be used
    /// to find the start; matching the newest live row with that pid is the
    /// best available identity, and it is right unless two processes with the
    /// same pid are alive at once, which cannot happen.
    fn stop(&mut self, event: &ProcEvent, now: f64) {
        if let Some(row) = self
            .rows
            .iter_mut()
            .rev()
            .find(|r| r.pid == event.pid && r.is_alive())
        {
            row.ended_at = Some(now);
            row.exit_status = event.exit_status;
        }
    }

    /// Attach owner and command line once the enrichment lands. Works after the
    /// row has ended -- the detail is often slower than the exit.
    ///
    /// Returns whether it found a row. The caller has nothing to do either way;
    /// the bool exists so [`the_seq_index_holds_starts_only`] can pin the
    /// invariant below rather than leaving it as something that happens to be
    /// true.
    ///
    /// # The invariant
    ///
    /// **`by_seq` maps the seq of a *start* event only.** [`Self::start`] is the
    /// one place that writes it, so a `Details` message carrying a stop event's
    /// seq matches nothing and is dropped in silence.
    ///
    /// That is correct today and it is worth writing down, because it is
    /// correct for a reason that lives in another crate: the monitor only
    /// queues enrichment for a start (`procmon.rs` sends `DetailsJob` from the
    /// start arm), and a stop is a process that is gone by definition, which is
    /// exactly what `Enrichment::Skipped` says on those rows. If the monitor
    /// ever enriched a stop, the detail would vanish here with no error and no
    /// log line -- so this is the sentence that has to be re-read when it does,
    /// and the fix would be to index the stop's seq onto the row it closed.
    pub(crate) fn attach(&mut self, seq: u64, user: String, enrichment: Enrichment) -> bool {
        let Some(&at) = self.by_seq.get(&seq) else {
            return false;
        };
        let Some(row) = self.rows.get_mut(at) else {
            return false;
        };
        if !user.is_empty() {
            row.user = user;
        }
        row.command_line = enrichment;
        true
    }

    /// Forget every ended row. The one way history leaves, and the user has to
    /// ask for it.
    pub(crate) fn clear_ended(&mut self) {
        self.rows.retain(TrackedProc::is_alive);
        self.dropped = 0;
        self.reindex();
    }

    pub(crate) fn clear(&mut self) {
        self.rows.clear();
        self.dropped = 0;
        self.reindex();
    }

    /// Drop the oldest **ended** rows until the cap is met.
    ///
    /// Never a live row, however old: a long-running service started before the
    /// window opened is not history, it is the current state of the machine.
    fn evict(&mut self) {
        if self.rows.len() <= self.max_rows {
            return;
        }
        let excess = self.rows.len() - self.max_rows;
        let mut removed = 0;
        self.rows.retain(|r| {
            if removed < excess && !r.is_alive() {
                removed += 1;
                false
            } else {
                true
            }
        });
        self.dropped += removed;
        if removed > 0 {
            self.reindex();
        }
    }

    fn reindex(&mut self) {
        self.index.clear();
        self.by_seq.clear();
        for (at, row) in self.rows.iter().enumerate() {
            self.by_seq.insert(row.seq, at);
        }
        // `index` is only consulted for duplicate suppression on start, and a
        // rebuilt one needs the same key; the start time is not kept on the row
        // (it is WMI's clock, not ours), so duplicates are only suppressed
        // within a generation. That is the correct trade: a duplicate after an
        // eviction would be a row we no longer had anyway.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Most tests do not care about the exit status; the one that does uses
    /// [`ev_exit`].
    /// `alpha` is f32 arithmetic, so the floor is reached to within rounding
    /// rather than exactly. Comparing for equality would make the test about
    /// float representation instead of about the fade settling.
    fn at_floor(alpha: f32) -> bool {
        (alpha - DIM_FLOOR).abs() < 1e-4
    }

    fn ev(kind: ProcKind, pid: u32, created: u64) -> ProcEvent {
        ev_exit(kind, pid, created, None)
    }

    fn ev_exit(kind: ProcKind, pid: u32, created: u64, exit: Option<u32>) -> ProcEvent {
        ProcEvent {
            kind,
            pid,
            parent_pid: 4,
            name: format!("p{pid}.exe"),
            session_id: 1,
            sid: Vec::new(),
            time_created: created,
            exit_status: exit,
        }
    }

    /// The whole point of the view: a process that ended is still on screen.
    #[test]
    fn an_ended_process_is_never_removed() {
        let mut log = ProcessLog::new();
        log.apply(1, &ev(ProcKind::Start, 100, 1), 0.0);
        log.apply(2, &ev(ProcKind::Stop, 100, 2), 1.0);

        // An hour later it is still there, dimmed and holding.
        assert_eq!(log.len(), 1);
        let row = &log.rows()[0];
        assert!(!row.is_alive());
        assert!(
            at_floor(row.alpha(3600.0)),
            "an hour on it should sit at the floor, got {}",
            row.alpha(3600.0)
        );
    }

    /// Dim, but still readable -- and it stops dimming.
    #[test]
    fn the_fade_settles_at_the_floor() {
        let mut log = ProcessLog::new();
        log.apply(1, &ev(ProcKind::Start, 1, 1), 0.0);
        log.apply(2, &ev(ProcKind::Stop, 1, 2), 10.0);
        let row = &log.rows()[0];

        assert_eq!(row.alpha(10.0), 1.0, "still full at the moment it ends");
        let mid = row.alpha(10.0 + FADE_SECS / 2.0);
        assert!(mid < 1.0 && mid > DIM_FLOOR, "mid-fade, got {mid}");
        assert!(at_floor(row.alpha(10.0 + FADE_SECS)));
        assert!(
            at_floor(row.alpha(10.0 + FADE_SECS * 100.0)),
            "the fade must stop at the floor, not keep going"
        );
        assert!(
            row.alpha(10.0 + FADE_SECS * 100.0) >= DIM_FLOOR,
            "a row must never fade past the floor and become unreadable"
        );
    }

    /// Windows recycles pids fast. A bare-pid key would overwrite the record of
    /// the process that just exited -- destroying the history this view keeps.
    #[test]
    fn a_recycled_pid_gets_its_own_row() {
        let mut log = ProcessLog::new();
        log.apply(1, &ev(ProcKind::Start, 4242, 1000), 0.0);
        log.apply(2, &ev(ProcKind::Stop, 4242, 1100), 1.0);
        log.apply(3, &ev(ProcKind::Start, 4242, 2000), 2.0);

        assert_eq!(log.len(), 2, "the reused pid overwrote the first row");
        assert!(!log.rows()[0].is_alive());
        assert!(log.rows()[1].is_alive());
    }

    /// A stop must close the live row, not an already-closed one.
    #[test]
    fn a_stop_closes_the_live_row() {
        let mut log = ProcessLog::new();
        log.apply(1, &ev(ProcKind::Start, 7, 1000), 0.0);
        log.apply(2, &ev(ProcKind::Stop, 7, 1100), 1.0);
        log.apply(3, &ev(ProcKind::Start, 7, 2000), 2.0);
        log.apply(4, &ev(ProcKind::Stop, 7, 2100), 3.0);

        assert_eq!(log.rows()[0].ended_at, Some(1.0));
        assert_eq!(log.rows()[1].ended_at, Some(3.0));
    }

    /// A long-running service started before the window opened is current
    /// state, not history, and must survive any amount of churn.
    #[test]
    fn eviction_never_takes_a_live_row() {
        let mut log = ProcessLog::new();
        log.max_rows = 10;

        log.apply(0, &ev(ProcKind::Start, 1, 1), 0.0); // the survivor
        for i in 1..40u32 {
            log.apply(i as u64, &ev(ProcKind::Start, 100 + i, i as u64), i as f64);
            log.apply(0, &ev(ProcKind::Stop, 100 + i, 0), i as f64 + 0.1);
        }

        assert!(log.len() <= 10);
        assert!(
            log.dropped > 0,
            "nothing was evicted, so the test proves nothing"
        );
        assert!(
            log.rows().iter().any(|r| r.pid == 1 && r.is_alive()),
            "the live row was evicted"
        );
    }

    /// Clearing is the only way history leaves, and it leaves the living alone.
    #[test]
    fn clear_ended_keeps_the_living() {
        let mut log = ProcessLog::new();
        log.apply(1, &ev(ProcKind::Start, 1, 1), 0.0);
        log.apply(2, &ev(ProcKind::Start, 2, 2), 0.0);
        log.apply(3, &ev(ProcKind::Stop, 2, 3), 1.0);

        log.clear_ended();
        assert_eq!(log.len(), 1);
        assert_eq!(log.rows()[0].pid, 1);
    }

    /// Enrichment is often slower than the exit, so it has to land on a row
    /// that has already ended.
    #[test]
    fn detail_attaches_after_the_process_has_ended() {
        let mut log = ProcessLog::new();
        log.apply(9, &ev(ProcKind::Start, 55, 1), 0.0);
        log.apply(0, &ev(ProcKind::Stop, 55, 2), 0.5);

        log.attach(
            9,
            "CORP\\a.demir".into(),
            Enrichment::Found(vmiscope_core::ProcInfo {
                command_line: "cmd /c whoami".into(),
                executable_path: "C:\\Windows\\System32\\cmd.exe".into(),
            }),
        );

        let row = &log.rows()[0];
        assert_eq!(row.user, "CORP\\a.demir");
        assert!(matches!(row.command_line, Enrichment::Found(_)));
        assert!(!row.is_alive(), "attaching must not revive the row");
    }

    /// A derived `Default` would leave the cap at zero, and a zero cap drops
    /// every ended row the moment it ends -- silently, and looking exactly like
    /// a monitor that never sees exits.
    #[test]
    fn a_default_log_keeps_history() {
        let mut log = ProcessLog::default();
        assert_eq!(log.max_rows, DEFAULT_MAX_ROWS);

        log.apply(1, &ev(ProcKind::Start, 1, 1), 0.0);
        log.apply(2, &ev(ProcKind::Stop, 1, 2), 1.0);
        assert_eq!(log.len(), 1, "the default cap threw the history away");
        assert_eq!(log.dropped, 0);
    }

    /// A duplicate delivery of the same start is not a second process.
    #[test]
    fn a_duplicate_start_is_ignored() {
        let mut log = ProcessLog::new();
        log.apply(1, &ev(ProcKind::Start, 3, 77), 0.0);
        log.apply(2, &ev(ProcKind::Start, 3, 77), 0.1);
        assert_eq!(log.len(), 1);
    }

    /// The whole of bug 5: a row has to be able to say *when*, not only "how
    /// long ago the window opened". `time_created` is a real FILETIME and it
    /// used to be consumed by the key and thrown away.
    #[test]
    fn a_row_carries_the_events_wall_clock() {
        // 2024-06-17 12:00:00 UTC as a FILETIME (100 ns since 1601), built from
        // its Unix seconds (1_718_625_600) rather than written out, so the
        // expectation below is not just a transcription of the same number.
        const JUN_17_2024: u64 = 116_444_736_000_000_000 + 1_718_625_600 * 10_000_000;
        let mut log = ProcessLog::new();
        log.apply(1, &ev(ProcKind::Start, 100, JUN_17_2024), 0.0);

        let row = &log.rows()[0];
        assert_eq!(
            row.created_filetime, JUN_17_2024,
            "the FILETIME was dropped"
        );
        let started = row.started_utc().expect("a real creation time converts");
        assert_eq!(
            (started.year, started.month, started.day),
            (2024, 6, 17),
            "{started:?}"
        );
        assert_eq!(started.hms(), "12:00:00");
        assert_eq!(started.full(), "2024-06-17 12:00:00 UTC");

        // The end is the start plus the measured lifetime, on the same clock.
        log.apply(2, &ev(ProcKind::Stop, 100, 0), 90.0);
        let ended = log.rows()[0].ended_utc().expect("an ended row has an end");
        assert_eq!(ended.hms(), "12:01:30");
    }

    /// A provider that reported no creation time must yield *nothing*, not
    /// 1601-01-01. This view exists to say when something ran; a fabricated
    /// date is the one wrong answer it must never give.
    #[test]
    fn an_absent_creation_time_is_not_a_date_in_1601() {
        let mut log = ProcessLog::new();
        log.apply(1, &ev(ProcKind::Start, 7, 0), 0.0);
        assert_eq!(log.rows()[0].created_filetime, 0);
        assert!(log.rows()[0].started_utc().is_none());
        assert!(log.rows()[0].ended_utc().is_none(), "still running anyway");

        log.apply(2, &ev(ProcKind::Stop, 7, 0), 5.0);
        assert!(
            log.rows()[0].ended_utc().is_none(),
            "an end derived from an unknown start is still unknown"
        );

        // Anything at or before the Unix epoch is the same non-answer.
        assert!(utc_from_filetime(0).is_none());
        assert!(utc_from_filetime(116_444_736_000_000_000).is_none());
    }

    /// The calendar arithmetic, on the dates that break naive versions: a leap
    /// day, a century that is not a leap year, a century that is, and the two
    /// ends of a year.
    #[test]
    fn the_calendar_handles_leap_years_and_year_ends() {
        // FILETIME for a given UTC date, computed forwards so the test does not
        // just restate the implementation's own arithmetic.
        let ft = |unix_secs: u64| 116_444_736_000_000_000 + unix_secs * 10_000_000;
        let ymd = |unix_secs: u64| {
            let u = utc_from_filetime(ft(unix_secs)).expect("after the epoch");
            (u.year, u.month, u.day, u.hour, u.minute, u.second)
        };

        assert_eq!(ymd(1), (1970, 1, 1, 0, 0, 1), "the first second");
        // 2000-02-29: a leap day in a century that IS divisible by 400.
        assert_eq!(ymd(951_782_400), (2000, 2, 29, 0, 0, 0));
        // 2100 is not a leap year (divisible by 100, not by 400), so
        // 2100-03-01 immediately follows 02-28. A `y % 4 == 0` leap rule puts
        // this one day out, which is exactly the bug the era-based form avoids.
        assert_eq!(ymd(4_107_456_000), (2100, 2, 28, 0, 0, 0));
        assert_eq!(ymd(4_107_542_400), (2100, 3, 1, 0, 0, 0));
        // The last second of a year, and the first of the next.
        assert_eq!(ymd(1_735_689_599), (2024, 12, 31, 23, 59, 59));
        assert_eq!(ymd(1_735_689_600), (2025, 1, 1, 0, 0, 0));
    }

    /// Bug 6, pinned rather than assumed: `by_seq` is written by `start` alone,
    /// so a `Details` message keyed by a stop event's seq matches nothing.
    ///
    /// This is the *current, correct* behaviour -- the monitor only enriches
    /// starts. The test exists so that if that ever changes, the silence has a
    /// failing assertion in front of it instead of a missing command line
    /// nobody can explain.
    #[test]
    fn the_seq_index_holds_starts_only() {
        let mut log = ProcessLog::new();
        log.apply(10, &ev(ProcKind::Start, 42, 500), 0.0);
        log.apply(11, &ev(ProcKind::Stop, 42, 600), 1.0);

        let found = Enrichment::Found(vmiscope_core::ProcInfo {
            command_line: "cmd /c whoami".into(),
            executable_path: String::new(),
        });

        assert!(
            log.attach(10, "CORP\\a".into(), found.clone()),
            "the START seq must land"
        );
        assert!(
            !log.attach(11, "CORP\\b".into(), found),
            "the STOP seq indexes nothing -- if this starts passing, read \
             ProcessLog::attach's invariant"
        );

        // And the start's detail survived the stop's failed attempt.
        assert_eq!(log.rows()[0].user, "CORP\\a");
    }

    /// Eviction rebuilds `by_seq`, and a rebuild that lost the mapping would
    /// silently stop attaching detail to every surviving row.
    #[test]
    fn the_seq_index_survives_an_eviction() {
        let mut log = ProcessLog::new();
        log.max_rows = 3;
        for pid in 1..=6u32 {
            log.apply(
                u64::from(pid),
                &ev(ProcKind::Start, pid, u64::from(pid)),
                0.0,
            );
            log.apply(100 + u64::from(pid), &ev(ProcKind::Stop, pid, 0), 0.1);
        }
        assert!(
            log.dropped > 0,
            "nothing was evicted, so this proves nothing"
        );

        let survivor = log.rows().last().expect("rows").seq;
        assert!(
            log.attach(survivor, "CORP\\c".into(), Enrichment::Unavailable),
            "a surviving row lost its seq mapping"
        );
    }

    /// A non-zero exit is worth flagging; zero and "still running" are not.
    #[test]
    fn only_a_non_zero_exit_counts_as_failure() {
        let mut log = ProcessLog::new();
        log.apply(1, &ev(ProcKind::Start, 1, 1), 0.0);
        assert!(!log.rows()[0].failed(), "a running process has not failed");

        log.apply(2, &ev(ProcKind::Stop, 1, 2), 1.0);
        assert!(
            !log.rows()[0].failed(),
            "exit status absent is not a failure"
        );

        let mut log = ProcessLog::new();
        log.apply(1, &ev(ProcKind::Start, 2, 1), 0.0);
        let mut stop = ev_exit(ProcKind::Stop, 2, 2, Some(0));
        log.apply(2, &stop, 1.0);
        assert!(!log.rows()[0].failed(), "exit 0 is not a failure");

        let mut log = ProcessLog::new();
        log.apply(1, &ev(ProcKind::Start, 3, 1), 0.0);
        stop = ev_exit(ProcKind::Stop, 3, 2, Some(1));
        log.apply(2, &stop, 1.0);
        assert!(log.rows()[0].failed());
    }
}
