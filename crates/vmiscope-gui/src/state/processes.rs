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

use vmiscope_core::{Enrichment, ProcEvent, ProcKind};

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
#[derive(Debug, Default)]
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

impl ProcessLog {
    pub(crate) fn new() -> Self {
        Self {
            max_rows: DEFAULT_MAX_ROWS,
            ..Default::default()
        }
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
            command_line: Enrichment::Skipped,
            started_at: now,
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
    pub(crate) fn attach(&mut self, seq: u64, user: String, enrichment: Enrichment) {
        if let Some(&at) = self.by_seq.get(&seq) {
            if let Some(row) = self.rows.get_mut(at) {
                if !user.is_empty() {
                    row.user = user;
                }
                row.command_line = enrichment;
            }
        }
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

    /// A duplicate delivery of the same start is not a second process.
    #[test]
    fn a_duplicate_start_is_ignored() {
        let mut log = ProcessLog::new();
        log.apply(1, &ev(ProcKind::Start, 3, 77), 0.0);
        log.apply(2, &ev(ProcKind::Start, 3, 77), 0.1);
        assert_eq!(log.len(), 1);
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
