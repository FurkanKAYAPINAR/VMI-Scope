//! The error log: the latest error plus a bounded history of recent ones.

use crate::app::VmiScopeApp;

impl VmiScopeApp {
    /// Record an error: keep it as the latest, and accumulate into a bounded log
    /// (so a burst of errors doesn't lose all but the last).
    pub(crate) fn push_error(&mut self, msg: String) {
        self.error_log.insert(0, msg.clone());
        self.error_log.truncate(50);
        self.error = Some(msg);
    }
}
