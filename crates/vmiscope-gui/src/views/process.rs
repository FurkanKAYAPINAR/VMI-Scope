//! The Process tab: what ran on this box, and what became of it.
//!
//! The rule this view is built around lives in [`crate::state::processes`]: an
//! ended process dims and **stays**. The Network tab fades a closed connection
//! and then drops it, and is right to -- a closed socket is not evidence of
//! much. A process that ran and exited is the entire question here, so nothing
//! on this screen expires. Rows leave exactly two ways, both of them stated:
//! the explicit Clear, and the row cap, which is surfaced the moment it bites.
//!
//! # Honesty
//!
//! Two of the columns can be empty for reasons that are not "there was nothing
//! there", and the difference is load-bearing in a tool used to answer "what
//! ran on this box":
//!
//! * the **command line** of a process this token does not own comes back as a
//!   NULL `CommandLine` in the degraded mode (`docs/FINDINGS.md`), which is
//!   *unreadable*, not *empty*, and is drawn as such;
//! * an empty **user** is an owner that could not be resolved, never a process
//!   without one.
//!
//! And the degraded banner is permanent while it applies, because the polled
//! fallback misses ~93% of instant-exit processes and that is precisely the
//! class of process someone reading this view came for.

use std::collections::HashMap;

use eframe::egui;

use crate::app::VmiScopeApp;
use crate::state::processes::{ProcessLog, TrackedProc};
use crate::theme::icons;
use crate::theme::tokens::{muted, BAD, DIVIDER, NEUTRAL, OK, S1, WARN};
use crate::util::save_file;
use crate::widgets::button::{btn_ghost, btn_primary, btn_secondary, segmented};
use crate::widgets::card::card;
use crate::widgets::chip::dot_chip;
use crate::widgets::field::{combo, filter_box};
use crate::widgets::rule::{hrule, vrule};
use crate::widgets::table::{DataTable, DataTableState, TableColumn};

use vmiscope_core::{Enrichment, MonitorMode, ProcKind, ProcMsg, ProcessMonitor};

/// `filter_box` fills whatever width it is handed, and a filter that runs the
/// width of a 4K window is harder to read than one the size of what you type
/// into it. Same figure as the Network tab's, so the two toolbars line up.
const FILTER_W: f32 = 240.0;

/// One level of parent-child indent, in points.
const INDENT: f32 = 12.0;

/// Where the indent stops. Deep chains are real (`svchost` under `services`
/// under `wininit`), but past a handful of levels the name column is mostly
/// whitespace and the tree stops being readable as a tree.
const MAX_DEPTH: usize = 6;

/// Strength of a cell that describes *missing* data rather than carrying data.
const NOTE: u8 = 45;

/// Strength of the banner's supporting lines.
const BANNER_BODY: u8 = 70;
const BANNER_NOTE: u8 = 55;

/// What the time column measures.
///
/// The app's own clock, because that is the only clock the row model keeps:
/// `ProcEvent::time_created` is a real FILETIME, but it is consumed by the
/// pid-reuse guard in core and never reaches [`TrackedProc`], so a wall-clock
/// stamp is not available to this view.
const TIME_HINT: &str = "elapsed since VMI-Scope started";

/// Which lifecycle states the table shows.
///
/// One control rather than two checkboxes: "live only" and "ended only" as
/// independent flags have a fourth state that means "show nothing", and a
/// filter combination that can only ever be a mistake should not be reachable.
#[derive(PartialEq, Eq, Clone, Copy, Default)]
enum Lifecycle {
    #[default]
    All,
    Live,
    Ended,
}

/// Which exporter a toolbar click asked for.
///
/// The buttons sit in the toolbar but the rows they export are not filtered
/// until further down the frame, so the click is recorded and acted on later.
#[derive(PartialEq, Eq, Clone, Copy)]
enum Export {
    Csv,
    Json,
}

/// The Process view's own state: its subscription, its retained log, and the
/// filters over it.
///
/// One struct rather than a dozen fields on the app, because unlike the older
/// tabs this view owns a live subscription whose lifetime is its own.
pub(crate) struct ProcessView {
    /// The running monitor, if there is one. Dropping it stops the pump.
    monitor: Option<ProcessMonitor>,
    /// Everything seen so far, ended rows included. See
    /// [`crate::state::processes`].
    log: ProcessLog,
    /// Which subscription the monitor settled on. `None` until it reports.
    /// Deliberately **not** cleared when the monitor stops: the caveat belongs
    /// to the rows that were collected under it, and they are still on screen.
    mode: Option<MonitorMode>,
    /// The last non-fatal monitor error.
    error: Option<String>,
    /// The user stopped the monitor. Kept so the view does not helpfully
    /// restart the subscription it was just asked to end.
    stopped: bool,
    filter: String,
    lifecycle: Lifecycle,
    failed_only: bool,
    /// `None` is every session.
    session: Option<u32>,
    tree: bool,
    /// (column index, ascending). `None` = newest first.
    sort: Option<(usize, bool)>,
}

impl Default for ProcessView {
    fn default() -> Self {
        Self {
            monitor: None,
            // `ProcessLog::new`, never `ProcessLog::default`: the derived
            // default leaves `max_rows` at 0, which would evict every ended row
            // the instant it arrived -- the one thing this view exists not to
            // do.
            log: ProcessLog::new(),
            mode: None,
            error: None,
            stopped: false,
            filter: String::new(),
            lifecycle: Lifecycle::default(),
            failed_only: false,
            session: None,
            tree: false,
            sort: None,
        }
    }
}

impl VmiScopeApp {
    // ------------------------------------------------------------------
    // Plumbing
    // ------------------------------------------------------------------

    /// Is the process subscription up? Read by the frame loop, which has to
    /// keep asking for repaints while it is.
    pub(crate) fn proc_running(&self) -> bool {
        self.proc.monitor.is_some()
    }

    /// The status bar's line for this view.
    ///
    /// Composed here rather than in `shell::statusbar` because the state it
    /// counts is private to this module -- and because the mode belongs in it:
    /// a bar that says "watching" while the monitor is sampling would undo the
    /// banner three inches above it.
    pub(crate) fn proc_status(&self) -> String {
        let total = self.proc.log.len();
        let live = self.proc.log.live_count();
        let state = match (self.proc.monitor.is_some(), self.proc.mode.as_ref()) {
            (false, _) => "stopped",
            (true, None) => "connecting",
            (true, Some(mode)) if mode.is_degraded() => "watching (degraded)",
            (true, Some(_)) => "watching (trace)",
        };
        format!(
            "{live} live \u{00b7} {} kept \u{00b7} {state}",
            total - live
        )
    }

    /// Fold everything the monitor has produced since the last frame into the
    /// log.
    ///
    /// Called every frame from `eframe::App::ui`, whatever view is on screen:
    /// the question this log answers is "what ran while I wasn't watching",
    /// which cannot be answered by a subscription that only collects while its
    /// own tab is visible.
    pub(crate) fn drain_processes(&mut self, now: f64) {
        // Collected first: `poll` borrows the monitor, and applying a message
        // borrows the log that sits beside it.
        let msgs = match self.proc.monitor.as_ref() {
            Some(monitor) => monitor.poll(),
            None => return,
        };
        for msg in msgs {
            match msg {
                ProcMsg::Mode(mode) => self.proc.mode = Some(mode),
                ProcMsg::Event { seq, event } => self.proc.log.apply(seq, &event, now),
                ProcMsg::Details {
                    seq,
                    user,
                    enrichment,
                } => self.proc.log.attach(seq, user, enrichment),
                ProcMsg::Error(e) => {
                    // Both places: the view shows it in context, and the status
                    // bar's log keeps it after the user has moved on.
                    self.proc.error = Some(e.clone());
                    self.push_error(format!("Process monitor: {e}"));
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // UI
    // ------------------------------------------------------------------

    pub(crate) fn ui_process(&mut self, ui: &mut egui::Ui, now: f64) {
        // The subscription starts the first time someone opens this view, not
        // at boot: it is a real WMI subscription plus a pump thread, and a user
        // who never opens the tab should not be paying for either.
        if !self.proc.stopped && self.proc.monitor.is_none() {
            self.proc.monitor = Some(ProcessMonitor::start());
        }

        let export = self.ui_process_toolbar(ui);
        self.ui_process_filters(ui);
        if let Some(mode) = self.proc.mode.as_ref() {
            if mode.is_degraded() {
                degraded_banner(ui, mode);
            }
        }
        if let Some(e) = self.proc.error.as_ref() {
            ui.label(icons::labelled_styled(
                ui,
                icons::WARNING_CIRCLE,
                e,
                egui::TextStyle::Body,
                BAD,
            ));
        }
        self.ui_process_counts(ui);
        hrule(ui);

        // --- rows -------------------------------------------------------
        //
        // Everything below reads `self.proc` immutably, so the filter state is
        // copied out first and the sort is handed to the table on loan.
        let needle = self.proc.filter.to_lowercase();
        let lifecycle = self.proc.lifecycle;
        let failed_only = self.proc.failed_only;
        let session = self.proc.session;
        let tree = self.proc.tree;

        let all = self.proc.log.rows();
        // Computed over *every* row, not just the visible ones: a child whose
        // parent is filtered out is still a child, and promoting it to the root
        // would be a claim the data does not make.
        let depth = if tree { depths(all) } else { Vec::new() };

        let mut rows: Vec<(&TrackedProc, usize)> = Vec::new();
        for (at, t) in all.iter().enumerate() {
            if keep(t, &needle, lifecycle, failed_only, session) {
                rows.push((t, depth.get(at).copied().unwrap_or(0)));
            }
        }
        if self.proc.sort.is_none() && !tree {
            // The default order: newest first. The log is oldest-first, which
            // would put every new event off the bottom of a scrolled table.
            // Ended rows keep their place in the sequence either way -- nothing
            // here sorts the living above the dead, which is the one ordering
            // that would bury the history this view exists to keep.
            //
            // The tree is the exception, and has to be: an indent only reads as
            // ancestry when the ancestor is above the thing indented under it,
            // so turning the tree on drops back to causal order. An explicit
            // sort still wins over both.
            rows.reverse();
        }

        if rows.is_empty() {
            ui.weak(empty_note(&self.proc.log, self.proc.monitor.is_some()));
        }

        let mut table = DataTableState {
            sort: self.proc.sort,
            selected: None,
        };
        let out = DataTable::new("proc-table")
            .columns([
                // The sign is a glance-level start/stop marker; it duplicates
                // the lifecycle filter, so it is not a sort target.
                TableColumn::exact("", 24.0).sortable(false),
                TableColumn::initial("Time", 78.0).at_least(56.0),
                TableColumn::initial("PID", 62.0)
                    .at_least(48.0)
                    .numeric(true),
                TableColumn::initial("Process", 152.0).at_least(60.0),
                TableColumn::initial("User", 148.0).at_least(60.0),
                TableColumn::initial("Session", 68.0)
                    .at_least(52.0)
                    .numeric(true),
                TableColumn::initial("PPID", 62.0)
                    .at_least(48.0)
                    .numeric(true),
                TableColumn::initial("Duration", 78.0).at_least(56.0),
                TableColumn::initial("Exit", 84.0).at_least(56.0),
                TableColumn::remainder("Command line").at_least(120.0),
            ])
            .sort_key(|row, col| col_value(rows[row].0, col, now))
            .show(ui, &mut table, rows.len(), |row| {
                let (t, depth) = rows[row.data_index()];
                let alpha = t.alpha(now);
                // A started row leans OK, an ended one settles toward neutral,
                // and a non-zero exit is BAD whatever else is true of it.
                // `failed()` draws that line: still running has not failed, and
                // neither has exit 0.
                let color = if t.failed() {
                    BAD
                } else if t.is_alive() {
                    OK
                } else {
                    NEUTRAL[3]
                };
                row.set_alpha(alpha);
                row.set_color(color);

                row.text(if t.is_alive() {
                    ProcKind::Start.sign()
                } else {
                    ProcKind::Stop.sign()
                });
                row.text(stamp(t.started_at)).on_hover_text(TIME_HINT);
                row.text(t.pid.to_string());
                name_cell(row, t, depth, color.gamma_multiply(alpha));
                if t.user.is_empty() {
                    // Not "no owner": an owner that could not be resolved. The
                    // polled fallback has no SID to resolve at all.
                    row.colored("\u{2014}", muted(NOTE))
                        .on_hover_text(USER_UNKNOWN);
                } else {
                    row.text(t.user.as_str());
                }
                row.text(t.session_id.to_string());
                row.text(t.parent_pid.to_string());
                row.text(duration(t.lifetime(now)));
                let (exit, why) = exit_cell(t);
                let cell = row.text(exit);
                if let Some(why) = why {
                    cell.on_hover_text(why);
                }
                match cmd_of(&t.command_line) {
                    Cmd::Line(line) => {
                        // The tooltip carries the image path too, which the
                        // cell has no room for and which is the thing you
                        // actually want when a bare `rundll32.exe` shows up.
                        let tip = match &t.command_line {
                            Enrichment::Found(info) if !info.executable_path.is_empty() => {
                                format!("{line}\n\n{}", info.executable_path)
                            }
                            _ => line.to_string(),
                        };
                        row.path(line).on_hover_text(tip);
                    }
                    Cmd::Missing(label, why) => {
                        row.colored(label, muted(NOTE)).on_hover_text(why);
                    }
                }
            });

        match export {
            Some(Export::Csv) => {
                save_file("vmiscope_processes.csv", &to_csv(&rows, &out.order, now))
            }
            Some(Export::Json) => {
                save_file("vmiscope_processes.json", &to_json(&rows, &out.order, now))
            }
            None => {}
        }
        self.proc.sort = table.sort;
    }

    /// Row one: what the monitor is doing, and what can be done to the log.
    fn ui_process_toolbar(&mut self, ui: &mut egui::Ui) -> Option<Export> {
        let mut export = None;
        ui.horizontal(|ui| {
            ui.strong("Process starts and stops");
            if self.proc.monitor.is_some() {
                // Stop is the primary for the same reason Pause is on the
                // Network tab: once it is running, ending it is the only
                // decision left.
                if btn_primary(ui, icons::labelled(ui, icons::STOP, "Stop")).clicked() {
                    self.proc.stopped = true;
                    self.proc.monitor = None;
                    self.proc.error = None;
                }
            } else if btn_primary(ui, icons::labelled(ui, icons::PLAY, "Start")).clicked() {
                self.proc.stopped = false;
                self.proc.error = None;
            }
            let ended = self.proc.log.len() - self.proc.log.live_count();
            if ended > 0
                && btn_ghost(ui, icons::labelled(ui, icons::TRASH, "Clear ended"))
                    .on_hover_text("Forget every ended row. Live processes are left alone.")
                    .clicked()
            {
                self.proc.log.clear_ended();
            }
            if self.proc.log.len() > 0 {
                if btn_secondary(ui, icons::labelled(ui, icons::DOWNLOAD_SIMPLE, "CSV")).clicked() {
                    export = Some(Export::Csv);
                }
                if btn_secondary(ui, icons::labelled(ui, icons::DOWNLOAD_SIMPLE, "JSON")).clicked()
                {
                    export = Some(Export::Json);
                }
            }
            vrule(ui, DIVIDER);
            ui.checkbox(&mut self.proc.tree, "tree").on_hover_text(
                "Indent each row under its parent, using the ParentProcessID the event \
                 already carries -- nothing is re-requested. Rows fall back to oldest-first \
                 while it is on, because an indent only reads as ancestry when the parent \
                 is above the row indented under it.",
            );
        });
        export
    }

    /// Row two: the filters, which compose.
    fn ui_process_filters(&mut self, ui: &mut egui::Ui) {
        // Built before the combo so the log is no longer borrowed when the
        // selection is handed over mutably.
        let mut sessions: Vec<u32> = self.proc.log.rows().iter().map(|t| t.session_id).collect();
        sessions.sort_unstable();
        sessions.dedup();
        let labels: Vec<String> = sessions.iter().map(|s| format!("session {s}")).collect();
        let mut options: Vec<(Option<u32>, &str)> = vec![(None, "all sessions")];
        options.extend(
            sessions
                .iter()
                .zip(labels.iter())
                .map(|(id, label)| (Some(*id), label.as_str())),
        );

        ui.horizontal(|ui| {
            ui.scope(|ui| {
                ui.set_max_width(FILTER_W);
                filter_box(ui, &mut self.proc.filter, "filter process / user / command");
            });
            segmented(
                ui,
                &mut self.proc.lifecycle,
                &[
                    (Lifecycle::All, "All"),
                    (Lifecycle::Live, "Live"),
                    (Lifecycle::Ended, "Ended"),
                ],
            );
            ui.checkbox(&mut self.proc.failed_only, "non-zero exit")
                .on_hover_text(
                    "Only processes that exited with a status other than 0. A running \
                     process has not failed, and neither has one that exited cleanly.",
                );
            combo(ui, "proc-session", &mut self.proc.session, &options);
        });
    }

    /// Row three: the counts, including the two the user has to be told about
    /// -- how much history is held, and how much of it the cap has taken.
    fn ui_process_counts(&mut self, ui: &mut egui::Ui) {
        let total = self.proc.log.len();
        let live = self.proc.log.live_count();
        let failed = self.proc.log.rows().iter().filter(|t| t.failed()).count();
        let dropped = self.proc.log.dropped;
        let cap = self.proc.log.max_rows;
        ui.horizontal(|ui| {
            dot_chip(ui, OK, &format!("{live} live"));
            dot_chip(ui, NEUTRAL[5], &format!("{} ended", total - live));
            if failed > 0 {
                dot_chip(ui, BAD, &format!("{failed} non-zero exit"));
            }
            if dropped > 0 {
                dot_chip(ui, WARN, &format!("{dropped} evicted")).on_hover_text(format!(
                    "The log holds {cap} rows. Past that, the oldest ENDED rows are \
                         dropped, oldest first -- never a live one. Export or clear before \
                         this number grows if the early history matters.",
                ));
            }
            ui.weak(format!("{total} of {cap} rows"));
        });
    }
}

// ---------------------------------------------------------------------------
// The degraded banner
// ---------------------------------------------------------------------------

/// Why an empty user column is not an ownerless process.
const USER_UNKNOWN: &str = "Owner not resolved. The trace events carry a SID; the polled \
                            fallback delivers a Win32_Process instance, which has no SID \
                            property at all, so the owner has to come from GetOwner -- and \
                            that call is refused for a process this token does not own.";

/// Says plainly, and for as long as it applies, that the view is sampling
/// rather than observing.
///
/// Never drawn in trace mode: [`MonitorMode::is_degraded`] gates it, and a
/// warning that appears when nothing is wrong is a warning nobody reads.
fn degraded_banner(ui: &mut egui::Ui, mode: &MonitorMode) {
    card(ui, |ui| {
        ui.label(icons::labelled_styled(
            ui,
            icons::WARNING,
            "Degraded: short-lived processes are being missed",
            egui::TextStyle::Body,
            WARN,
        ));
        ui.add_space(S1);
        // The mode carries the measured cost and the refusal that caused it,
        // so the banner does not restate either from memory.
        ui.label(egui::RichText::new(mode.summary()).color(muted(BANNER_BODY)));
        ui.add_space(S1);
        ui.label(egui::RichText::new(advice()).color(muted(BANNER_NOTE)));
    });
}

/// What to do about it -- which depends on something the app can actually
/// check.
///
/// There is deliberately no "Restart elevated" button. Relaunching would have
/// to go out through a UAC prompt and come back as a different process, and
/// whether an elevated token lifts *this* denial has never been observed on
/// this machine (`docs/REDESIGN.md` 9.3). A button that promises an unverified
/// fix is worse than a sentence that names the step and its uncertainty.
fn advice() -> &'static str {
    if elevated() {
        "This process already holds an elevated token and the WMI Kernel Trace Event \
         Provider refused it anyway, so elevation is not the missing piece here. Read \
         every row below as a sample, not as an inventory."
    } else {
        "This token is not elevated. Closing VMI-Scope and starting it again with \
         Run as administrator is the documented next step -- though whether an elevated \
         token lifts this particular denial has never been observed here, so it may \
         change nothing. Until it does, read every row below as a sample, not as an \
         inventory."
    }
}

/// Whether this process holds an elevated token.
///
/// Asked once: the call opens and queries a process token, and the answer
/// cannot change for the life of the process.
fn elevated() -> bool {
    static ELEVATED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ELEVATED.get_or_init(vmiscope_core::is_elevated)
}

// ---------------------------------------------------------------------------
// Cells
// ---------------------------------------------------------------------------

/// The process-name cell, indented under its parent when the tree is on.
///
/// Hand-built rather than `row.text`, because the indent and the child marker
/// have to share one cell with the name -- and because the marker is an icon,
/// which needs the icon family and therefore its own section.
fn name_cell(
    row: &mut crate::widgets::table::RowCtx<'_, '_, '_>,
    t: &TrackedProc,
    depth: usize,
    color: egui::Color32,
) {
    let name = t.name.clone();
    row.cell(move |ui| {
        if depth > 0 {
            ui.add_space(depth as f32 * INDENT);
            ui.label(icons::glyph(icons::ARROW_ELBOW_DOWN_RIGHT).color(color));
        }
        ui.add(egui::Label::new(egui::RichText::new(name).color(color)).truncate());
    });
}

/// The exit-status cell: its text, and the tooltip it needs when the text
/// alone would mislead.
fn exit_cell(t: &TrackedProc) -> (String, Option<String>) {
    match (t.is_alive(), t.exit_status) {
        // Still running. Not "unknown" -- there is nothing to know yet.
        (true, _) => ("\u{2014}".to_string(), None),
        // Ended, but the provider left the status NULL. The polled fallback
        // does this on every row: its stop event is a Win32_Process deletion,
        // which carries no ExitStatus at all. Rendering that as 0 would invent
        // a clean exit.
        (false, None) => (
            "unknown".to_string(),
            Some("The stop event carried no ExitStatus. The polled fallback never does.".into()),
        ),
        // Small codes are the ones people recognise; an NTSTATUS-shaped code is
        // unreadable in decimal, so it is shown in hex with both in the tooltip.
        (false, Some(code)) if code <= 0xff => (code.to_string(), None),
        (false, Some(code)) => (
            format!("{code:#010x}"),
            Some(format!("{code} ({code:#010x})")),
        ),
    }
}

/// What the command-line cell can honestly say.
enum Cmd<'a> {
    /// A real command line.
    Line(&'a str),
    /// Why there is none: a short label, and the long form for the tooltip.
    Missing(&'static str, &'static str),
}

/// Three ways to have no command line, and they are three different facts.
fn cmd_of(enrichment: &Enrichment) -> Cmd<'_> {
    match enrichment {
        Enrichment::Found(info) if !info.command_line.is_empty() => Cmd::Line(&info.command_line),
        // Answered, with a NULL CommandLine. WMI does this for every process
        // the subscribing token does not own -- all of session 0 -- and it is
        // the reason this cell is not simply left blank.
        Enrichment::Found(_) => Cmd::Missing(
            "unreadable",
            "WMI answered with a NULL CommandLine, which it does for any process this \
             token does not own. The process has a command line; this session cannot \
             read it.",
        ),
        Enrichment::Unavailable => Cmd::Missing(
            "unavailable",
            "The process was already gone when it was asked, or the pid had been reused \
             and the identity check refused to attribute someone else's command line to \
             this row.",
        ),
        Enrichment::Pending => Cmd::Missing(
            "looking\u{2026}",
            "The lookup is in flight. This is the state a row is born in, and it \
             usually lasts a fraction of a second.",
        ),
        Enrichment::Skipped => Cmd::Missing(
            "not queried",
            "No lookup was made: a stop event, where the process is gone by definition, \
             or an enrichment queue too deep to join without holding up the event stream.",
        ),
    }
}

/// `T+MM:SS`, or `T+H:MM:SS` past the hour. See [`TIME_HINT`].
fn stamp(t: f64) -> String {
    let s = t.max(0.0) as u64;
    let (h, m, sec) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 {
        format!("T+{h}:{m:02}:{sec:02}")
    } else {
        format!("T+{m:02}:{sec:02}")
    }
}

/// How long it ran, at a precision that suits the magnitude.
///
/// Sub-second matters here and nowhere else in the app: an instant-exit process
/// is the thing this view is for, and "0s" would report it as not having run.
fn duration(secs: f64) -> String {
    let s = secs.max(0.0);
    if s < 10.0 {
        format!("{s:.2}s")
    } else if s < 60.0 {
        format!("{s:.1}s")
    } else if s < 3600.0 {
        format!("{}m{:02}s", (s / 60.0) as u64, (s % 60.0) as u64)
    } else {
        format!(
            "{}h{:02}m",
            (s / 3600.0) as u64,
            ((s % 3600.0) / 60.0) as u64
        )
    }
}

/// What the view says when the table is empty, which is a different sentence
/// depending on why it is empty.
fn empty_note(log: &ProcessLog, running: bool) -> &'static str {
    if log.len() > 0 {
        "No rows match the filters."
    } else if running {
        "Watching. Nothing has started or stopped yet."
    } else {
        "The monitor is stopped \u{2014} click Start."
    }
}

// ---------------------------------------------------------------------------
// Sorting and export
// ---------------------------------------------------------------------------

/// The sort key for a cell, in the header order.
///
/// Numbers are handed over as bare numbers rather than as their formatted
/// display strings: `smart_cmp` compares numerically when both sides parse, so
/// a duration sorts by length and not by whether it happens to be spelled in
/// minutes.
fn col_value(t: &TrackedProc, col: usize, now: f64) -> String {
    match col {
        0 => if t.is_alive() {
            ProcKind::Start.sign()
        } else {
            ProcKind::Stop.sign()
        }
        .to_string(),
        1 => t.started_at.to_string(),
        2 => t.pid.to_string(),
        3 => t.name.clone(),
        4 => t.user.clone(),
        5 => t.session_id.to_string(),
        6 => t.parent_pid.to_string(),
        7 => t.lifetime(now).to_string(),
        // Empty rather than 0 for a row with no status, so "never reported"
        // does not sort in among the clean exits.
        8 => t.exit_status.map(|c| c.to_string()).unwrap_or_default(),
        9 => match cmd_of(&t.command_line) {
            Cmd::Line(line) => line.to_string(),
            Cmd::Missing(label, _) => label.to_string(),
        },
        _ => String::new(),
    }
}

/// How deep under an ancestor each row sits, by row index.
///
/// Two passes rather than one accumulating pass, because arrival order is
/// **not** causal order: the polled fallback hands a batch of `Win32_Process`
/// instances over in whatever order the enumerator produced them, so a child
/// routinely lands before its parent. A single forward pass that only knew
/// about rows it had already seen found no ancestry at all on a real burst of
/// `ping` and its `conhost` children -- which is how this was caught.
///
/// The walk is bounded by [`MAX_DEPTH`], which is also what makes it safe: pid
/// reuse can produce two rows that each claim the other as parent, and a bound
/// is a cheaper defence than a visited set.
fn depths(all: &[TrackedProc]) -> Vec<usize> {
    let mut by_pid: HashMap<u32, usize> = HashMap::with_capacity(all.len());
    for (at, t) in all.iter().enumerate() {
        // Last writer wins: with a recycled pid, the most recent row is the
        // better guess at whose parent it is.
        by_pid.insert(t.pid, at);
    }
    (0..all.len())
        .map(|start| {
            let mut depth = 0;
            let mut at = start;
            while depth < MAX_DEPTH {
                match by_pid.get(&all[at].parent_pid) {
                    // A "parent" that started after its child is a recycled
                    // pid, not an ancestor. The chain stops rather than
                    // inventing one.
                    Some(&up) if up != at && all[up].started_at <= all[at].started_at => {
                        depth += 1;
                        at = up;
                    }
                    _ => break,
                }
            }
            depth
        })
        .collect()
}

/// Does this row survive the filters? They compose: every clause has to hold.
fn keep(
    t: &TrackedProc,
    needle: &str,
    lifecycle: Lifecycle,
    failed_only: bool,
    session: Option<u32>,
) -> bool {
    let lifecycle_ok = match lifecycle {
        Lifecycle::All => true,
        Lifecycle::Live => t.is_alive(),
        Lifecycle::Ended => !t.is_alive(),
    };
    if !lifecycle_ok || (failed_only && !t.failed()) {
        return false;
    }
    if session.is_some_and(|s| t.session_id != s) {
        return false;
    }
    if needle.is_empty() {
        return true;
    }
    // The text filter searches the command line only when there is one to
    // search: matching the word "unavailable" against rows that are merely
    // missing data would be a search over the UI's own vocabulary.
    let command = match cmd_of(&t.command_line) {
        Cmd::Line(line) => line,
        Cmd::Missing(..) => "",
    };
    t.name.to_lowercase().contains(needle)
        || t.user.to_lowercase().contains(needle)
        || command.to_lowercase().contains(needle)
}

/// One CSV field, RFC 4180.
///
/// `vmiscope_core::export` has these same three lines, but private, and core is
/// not this task's to change.
fn csv_field(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// The log as CSV, in the order shown.
///
/// `command_line_state` is a column of its own on purpose: a reader looking at
/// a blank `command_line` has to be able to tell "no arguments" from "this
/// session was not allowed to read them", which is the same distinction the
/// table draws.
fn to_csv(rows: &[(&TrackedProc, usize)], order: &[usize], now: f64) -> String {
    let mut out = String::from(
        "state,started_t_plus_secs,pid,process,user,session_id,parent_pid,\
         duration_secs,exit_status,command_line,command_line_state\n",
    );
    for &i in order {
        let t = rows[i].0;
        let (command, state) = match cmd_of(&t.command_line) {
            Cmd::Line(line) => (line, "found"),
            Cmd::Missing(label, _) => ("", label),
        };
        let cells = [
            if t.is_alive() { "running" } else { "ended" }.to_string(),
            format!("{:.3}", t.started_at),
            t.pid.to_string(),
            t.name.clone(),
            t.user.clone(),
            t.session_id.to_string(),
            t.parent_pid.to_string(),
            format!("{:.3}", t.lifetime(now)),
            t.exit_status.map(|c| c.to_string()).unwrap_or_default(),
            command.to_string(),
            state.to_string(),
        ];
        out.push_str(
            &cells
                .iter()
                .map(|c| csv_field(c))
                .collect::<Vec<_>>()
                .join(","),
        );
        out.push('\n');
    }
    out
}

/// One JSON object per row, in the order shown.
///
/// `user` and `command_line` are nullable rather than empty strings for the
/// same reason the table draws them differently: `null` is "not known", and an
/// empty string would claim it was known to be empty.
#[derive(serde::Serialize)]
struct ExportRow<'a> {
    state: &'a str,
    started_t_plus_secs: f64,
    pid: u32,
    process: &'a str,
    user: Option<&'a str>,
    session_id: u32,
    parent_pid: u32,
    duration_secs: f64,
    exit_status: Option<u32>,
    command_line: Option<&'a str>,
    command_line_state: &'a str,
}

fn to_json(rows: &[(&TrackedProc, usize)], order: &[usize], now: f64) -> String {
    let out: Vec<ExportRow<'_>> = order
        .iter()
        .map(|&i| {
            let t = rows[i].0;
            let (command, state) = match cmd_of(&t.command_line) {
                Cmd::Line(line) => (Some(line), "found"),
                Cmd::Missing(label, _) => (None, label),
            };
            ExportRow {
                state: if t.is_alive() { "running" } else { "ended" },
                started_t_plus_secs: t.started_at,
                pid: t.pid,
                process: &t.name,
                user: (!t.user.is_empty()).then_some(t.user.as_str()),
                session_id: t.session_id,
                parent_pid: t.parent_pid,
                duration_secs: t.lifetime(now),
                exit_status: t.exit_status,
                command_line: command,
                command_line_state: state,
            }
        })
        .collect();
    serde_json::to_string_pretty(&out).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    use vmiscope_core::{ProcEvent, ProcInfo};

    /// One event, in the shape the monitor delivers. `time_created` is keyed to
    /// the pid so no two of these look like the same process to the log's
    /// duplicate guard.
    fn ev(kind: ProcKind, pid: u32, exit: Option<u32>) -> ProcEvent {
        ProcEvent {
            kind,
            pid,
            parent_pid: 4,
            name: format!("p{pid}.exe"),
            session_id: 1,
            sid: Vec::new(),
            time_created: u64::from(pid),
            exit_status: exit,
        }
    }

    fn start(log: &mut ProcessLog, seq: u64, pid: u32, at: f64) {
        log.apply(seq, &ev(ProcKind::Start, pid, None), at);
    }

    fn stop(log: &mut ProcessLog, seq: u64, pid: u32, exit: Option<u32>, at: f64) {
        log.apply(seq, &ev(ProcKind::Stop, pid, exit), at);
    }

    /// The distinction the whole view rests on: a NULL command line is a
    /// statement about permission, not an empty argument list, and the two must
    /// not render the same way.
    #[test]
    fn an_unreadable_command_line_is_not_an_empty_one() {
        let readable = Enrichment::Found(ProcInfo {
            command_line: "cmd /c exit".into(),
            executable_path: String::new(),
        });
        assert!(matches!(cmd_of(&readable), Cmd::Line("cmd /c exit")));

        // Answered, but with a NULL CommandLine -- the degraded mode's normal
        // result for a process this token does not own.
        let null = Enrichment::Found(ProcInfo::default());
        let (unreadable, unavailable, skipped) = (
            cmd_of(&null),
            cmd_of(&Enrichment::Unavailable),
            cmd_of(&Enrichment::Skipped),
        );
        for cell in [&unreadable, &unavailable, &skipped] {
            assert!(
                matches!(cell, Cmd::Missing(..)),
                "a missing command line must never render as content"
            );
        }
        let label = |c: &Cmd<'_>| match c {
            Cmd::Line(_) => "",
            Cmd::Missing(l, _) => l,
        };
        // Three facts, three labels. Collapsing any pair would report "we asked
        // and were refused" as "we never asked", or vice versa.
        assert_ne!(label(&unreadable), label(&unavailable));
        assert_ne!(label(&unavailable), label(&skipped));
        assert_ne!(label(&unreadable), label(&skipped));
    }

    /// A running process has no exit status *yet*; an ended one whose provider
    /// left it NULL has one that was never reported. Neither is a clean exit.
    #[test]
    fn a_missing_exit_status_is_never_shown_as_zero() {
        let mut log = ProcessLog::new();
        start(&mut log, 1, 100, 0.0);
        let (text, why) = exit_cell(&log.rows()[0]);
        assert_eq!(text, "\u{2014}", "a running process has no status yet");
        assert!(why.is_none());

        // Ended, with the status the polled fallback always leaves NULL.
        stop(&mut log, 2, 100, None, 1.0);
        let (text, why) = exit_cell(&log.rows()[0]);
        assert_eq!(text, "unknown");
        assert!(why.is_some(), "'unknown' has to say why");

        start(&mut log, 3, 101, 0.0);
        stop(&mut log, 4, 101, Some(0), 1.0);
        assert_eq!(exit_cell(&log.rows()[1]).0, "0");

        // An NTSTATUS-shaped code in decimal is unreadable; in hex it is
        // recognisable as an access violation.
        start(&mut log, 5, 102, 0.0);
        stop(&mut log, 6, 102, Some(0xc000_0005), 1.0);
        assert_eq!(exit_cell(&log.rows()[2]).0, "0xc0000005");
    }

    /// Filters compose: every clause has to hold, and none of them may quietly
    /// widen another.
    #[test]
    fn filters_compose() {
        let mut log = ProcessLog::new();
        start(&mut log, 1, 10, 0.0);
        start(&mut log, 2, 11, 0.0);
        stop(&mut log, 3, 11, Some(1), 1.0);
        log.attach(
            1,
            "CORP\\a.demir".into(),
            Enrichment::Found(ProcInfo {
                command_line: "p10.exe --serve".into(),
                executable_path: String::new(),
            }),
        );
        let rows = log.rows();
        let (live, dead) = (&rows[0], &rows[1]);

        assert!(keep(live, "", Lifecycle::Live, false, None));
        assert!(!keep(dead, "", Lifecycle::Live, false, None));
        assert!(keep(dead, "", Lifecycle::Ended, false, None));

        // Non-zero exit only: the live row is not a failure, whatever else is
        // true of it.
        assert!(!keep(live, "", Lifecycle::All, true, None));
        assert!(keep(dead, "", Lifecycle::All, true, None));

        // Session, and the text filter over name / user / command line.
        assert!(keep(live, "", Lifecycle::All, false, Some(1)));
        assert!(!keep(live, "", Lifecycle::All, false, Some(2)));
        assert!(keep(live, "p10", Lifecycle::All, false, None));
        assert!(keep(live, "a.demir", Lifecycle::All, false, None));
        assert!(keep(live, "--serve", Lifecycle::All, false, None));

        // Composed: live AND matching AND in session 1.
        assert!(keep(live, "p10", Lifecycle::Live, false, Some(1)));
        assert!(!keep(live, "p10", Lifecycle::Ended, false, Some(1)));
        assert!(!keep(live, "p99", Lifecycle::Live, false, Some(1)));

        // The text filter must not match the UI's own words for missing data:
        // p11 has no enrichment at all.
        assert!(!keep(dead, "not queried", Lifecycle::All, false, None));
    }

    /// A sub-second process is the thing this view exists to catch, so it must
    /// not be rendered as having taken no time at all.
    #[test]
    fn an_instant_exit_still_has_a_duration() {
        assert_eq!(duration(0.04), "0.04s");
        assert_eq!(duration(9.5), "9.50s");
        assert_eq!(duration(42.25), "42.2s");
        assert_eq!(duration(90.0), "1m30s");
        assert_eq!(duration(3_930.0), "1h05m");
        // A clock that ran backwards between two frames must not print a
        // negative lifetime.
        assert_eq!(duration(-1.0), "0.00s");
    }

    #[test]
    fn the_time_stamp_grows_a_field_at_the_hour() {
        assert_eq!(stamp(0.0), "T+00:00");
        assert_eq!(stamp(65.4), "T+01:05");
        assert_eq!(stamp(3_725.0), "T+1:02:05");
    }

    /// The export carries the same distinction the table draws, or it is not
    /// the same evidence.
    #[test]
    fn the_export_keeps_unreadable_apart_from_empty() {
        let mut log = ProcessLog::new();
        start(&mut log, 1, 10, 0.0);
        start(&mut log, 2, 11, 0.0);
        log.attach(
            1,
            "CORP\\a.demir".into(),
            Enrichment::Found(ProcInfo {
                command_line: "p10.exe --serve, now".into(),
                executable_path: String::new(),
            }),
        );
        // Answered with a NULL CommandLine.
        log.attach(2, String::new(), Enrichment::Found(ProcInfo::default()));

        let rows: Vec<(&TrackedProc, usize)> = log.rows().iter().map(|t| (t, 0)).collect();
        let order = [0, 1];

        let csv = to_csv(&rows, &order, 2.0);
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 3, "a header and one row each");
        assert!(lines[0].ends_with("command_line,command_line_state"));
        // The comma inside the command line is quoted, not a column break.
        assert!(lines[1].contains("\"p10.exe --serve, now\",found"));
        assert!(lines[1].contains("CORP\\a.demir"));
        assert!(
            lines[2].ends_with(",,unreadable"),
            "an unreadable command line must be blank AND labelled: {}",
            lines[2]
        );

        let json = to_json(&rows, &order, 2.0);
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let arr = parsed.as_array().expect("an array");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["command_line"], "p10.exe --serve, now");
        assert_eq!(arr[0]["command_line_state"], "found");
        assert_eq!(arr[0]["user"], "CORP\\a.demir");
        assert_eq!(arr[0]["state"], "running");
        // Null, not "": the difference between "no arguments" and "not
        // readable", which is the whole point of the column beside it.
        assert!(arr[1]["command_line"].is_null());
        assert_eq!(arr[1]["command_line_state"], "unreadable");
        assert!(arr[1]["user"].is_null(), "an unresolved owner is not \"\"");
    }

    /// The bug this test exists for, found by looking at a real capture: the
    /// polled fallback delivers a batch in enumerator order, so a child lands
    /// before its parent and a forward-only pass sees no ancestry whatsoever.
    #[test]
    fn ancestry_does_not_depend_on_arrival_order() {
        let mut log = ProcessLog::new();
        // conhost (child) first, then the ping that spawned it -- exactly the
        // order the fallback produced on the machine this was measured on.
        log.apply(
            1,
            &ProcEvent {
                parent_pid: 500,
                ..ev(ProcKind::Start, 600, None)
            },
            0.0,
        );
        log.apply(
            2,
            &ProcEvent {
                parent_pid: 4,
                ..ev(ProcKind::Start, 500, None)
            },
            0.0,
        );
        let d = depths(log.rows());
        assert_eq!(
            d[0], 1,
            "the child must be indented under a later-seen parent"
        );
        assert_eq!(d[1], 0, "the parent is a root: pid 4 is not in the log");
    }

    /// Pid reuse can produce two rows that each name the other, and an unbounded
    /// walk would spin on them forever.
    #[test]
    fn ancestry_is_bounded_and_refuses_a_later_parent() {
        let mut log = ProcessLog::new();
        log.apply(
            1,
            &ProcEvent {
                parent_pid: 2,
                ..ev(ProcKind::Start, 1, None)
            },
            0.0,
        );
        log.apply(
            2,
            &ProcEvent {
                parent_pid: 1,
                ..ev(ProcKind::Start, 2, None)
            },
            0.0,
        );
        for d in depths(log.rows()) {
            assert!(d <= MAX_DEPTH, "the walk has to stop somewhere");
        }

        // A "parent" that started later than its child is a recycled pid, and
        // attributing the child to it would invent an ancestry.
        let mut log = ProcessLog::new();
        log.apply(
            1,
            &ProcEvent {
                parent_pid: 900,
                ..ev(ProcKind::Start, 10, None)
            },
            0.0,
        );
        log.apply(
            2,
            &ProcEvent {
                parent_pid: 4,
                ..ev(ProcKind::Start, 900, None)
            },
            30.0,
        );
        assert_eq!(depths(log.rows())[0], 0);
    }

    /// A chain deeper than the cap is indented to the cap and no further.
    #[test]
    fn ancestry_stops_at_the_indent_cap() {
        let mut log = ProcessLog::new();
        for pid in 1..=(MAX_DEPTH as u32 + 3) {
            log.apply(
                u64::from(pid),
                &ProcEvent {
                    parent_pid: pid.saturating_sub(1),
                    ..ev(ProcKind::Start, pid, None)
                },
                f64::from(pid),
            );
        }
        let d = depths(log.rows());
        assert_eq!(d[0], 0, "the root of the chain");
        assert_eq!(d[1], 1);
        assert_eq!(*d.last().expect("rows"), MAX_DEPTH);
    }

    /// Sorting has to be numeric where the display string is not: "10" must not
    /// sort before "9", and a duration must sort by length rather than by the
    /// unit it happens to be spelled in.
    #[test]
    fn numeric_columns_sort_as_numbers() {
        let mut log = ProcessLog::new();
        start(&mut log, 1, 9, 0.0);
        start(&mut log, 2, 10, 0.0);
        let rows = log.rows();
        assert_eq!(col_value(&rows[0], 2, 1.0), "9");
        assert_eq!(col_value(&rows[1], 2, 1.0), "10");
        assert_eq!(
            crate::util::smart_cmp(&col_value(&rows[0], 2, 1.0), &col_value(&rows[1], 2, 1.0)),
            std::cmp::Ordering::Less
        );
        // Column 8 for a row with no exit status is empty, so it cannot sort in
        // among the zeroes.
        assert_eq!(col_value(&rows[0], 8, 1.0), "");
    }
}
