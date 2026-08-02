//! The status line: the current error, a bounded history of past ones, and the
//! transient notices that say something finished.
//!
//! Three fields, three lifetimes, and the differences are the point. `error` is
//! the *current* condition -- one line in the status bar, and it has to stop
//! being true. `error_log` is the session's history and never forgets, which is
//! why clearing the banner loses nothing. `notice` is neither: it is a fact that
//! was true for a moment ("saved to ..."), so it expires on a clock.

use crate::app::VmiScopeApp;

/// How many errors the session log keeps.
const LOG_CAP: usize = 50;

/// How long a notice stays in the status bar.
///
/// Long enough to read a path, short enough that it is gone before it starts
/// looking like state. It exists because moving the save off the frame loop
/// (task 7.8) took away the only feedback an export had: the application used
/// to freeze until the write finished, and unfreezing was the confirmation.
const NOTICE_SECS: f64 = 6.0;

impl VmiScopeApp {
    /// Record an error: keep it as the latest, and accumulate into a bounded log
    /// (so a burst of errors doesn't lose all but the last).
    pub(crate) fn push_error(&mut self, msg: String) {
        self.error_log.insert(0, msg.clone());
        self.error_log.truncate(LOG_CAP);
        self.error = Some(msg);
    }

    /// Fold everything the IO thread finished into the app.
    ///
    /// Once a frame, like `handle_responses`. A file dialog no longer blocks the
    /// frame loop (`crate::io`), so its answer has to arrive the same way every
    /// other asynchronous answer does.
    pub(crate) fn drain_io(&mut self, now: f64) {
        for note in crate::io::drain() {
            match note {
                crate::io::Note::Saved(path) => {
                    self.push_notice(now, format!("Saved {}", path.display()));
                }
                crate::io::Note::Picked { what, text } => match what {
                    crate::io::PickFor::PersistenceBaseline => self.apply_baseline_file(&text),
                    crate::io::PickFor::ProvidersBaseline => {
                        self.apply_provider_baseline_file(&text);
                    }
                    crate::io::PickFor::SavedLibrary => self.apply_library_file(&text),
                },
                // Including the ones a `let _ = fs::write(..)` used to swallow.
                crate::io::Note::Failed(message) => self.push_error(message),
            }
        }
    }

    /// A operation succeeded, so the banner is no longer describing the present.
    ///
    /// Only `run_query` and the `QueryResult` arm used to do this, so any error
    /// raised by a *non*-query operation -- a denied provider list, a failed
    /// baseline load, a connect that timed out -- stayed in the status bar
    /// forever while every subsequent operation succeeded. It is the one
    /// component of the shell whose job is to say what is wrong now, so a
    /// permanently stale value in it is worse than an empty one.
    ///
    /// Nothing is lost: `error_log` still has it, and the `Log (n)` button in
    /// the status bar only exists because that log is non-empty.
    pub(crate) fn clear_error(&mut self) {
        self.error = None;
    }

    /// Say that something finished, for [`NOTICE_SECS`].
    pub(crate) fn push_notice(&mut self, now: f64, text: String) {
        self.notice = Some((text, now + NOTICE_SECS));
    }

    /// The notice to show right now, if it has not expired.
    ///
    /// Read rather than reaped: expiring it here would need a `&mut` in the
    /// middle of drawing the status bar, and a notice that outlives its welcome
    /// by one frame because nobody looked is not a bug worth that.
    pub(crate) fn live_notice(&self, now: f64) -> Option<&str> {
        match &self.notice {
            Some((text, until)) if *until > now => Some(text.as_str()),
            _ => None,
        }
    }
}
