//! The Compare view: one query, two targets, one keyed row diff.
//!
//! Everything else in this application asks one machine a question. This asks
//! two, and the whole value of the answer is in the *alignment* -- which row on
//! A is the same row on B, and which of its columns moved. Three decisions carry
//! that, and each of them is a way this view can silently lie if it is got
//! wrong:
//!
//! * **The key.** Rows are matched on the class's own key columns
//!   ([`QueryResult::key_columns`]), falling back to `__RELPATH` and only then to
//!   whole-row identity. Whole-row identity is not a diff of a table, it is a
//!   set difference of strings: the moment any counter moves, every row is
//!   reported as removed-and-added, which is indistinguishable from useless. The
//!   key that was actually used is always named on screen, because a diff whose
//!   alignment you cannot see is a diff you cannot check.
//! * **The ignore list.** A column that moves on its own says nothing when it
//!   differs. The defaults are derived from the column *names* (there is no type
//!   information in a `QueryResult`), so they are a heuristic, and a heuristic
//!   that hides data has to be visible and overridable -- both lists are shown
//!   and both are editable.
//! * **Partiality.** A diff computed from a side that stopped early is not a
//!   diff. Every row the truncated side never read is reported as "only on the
//!   other one", which is a fabricated finding. So a partial side withholds the
//!   table by default and says what happened; running it anyway is possible, and
//!   is labelled as unsound everywhere it can be seen, including in the export.
//!
//! Two mechanical notes:
//!
//! * The value columns are **unclipped** ([`TableColumn::clip`]). A clipped
//!   column's clip rect is exactly its `max_rect` in egui 0.35
//!   (`clip_rect_margin == 0.0`), so the cell tint -- which by construction
//!   bleeds half an `item_spacing` past that rect to meet its neighbour -- would
//!   be discarded entirely rather than trimmed. Unclipped columns do not
//!   ellipsize on their own, so every cell truncates explicitly instead.
//! * A and B run on **their own workers** ([`WorkerRegistry`], one COM thread per
//!   [`HostRef`]). The app's single worker cannot serve two hosts: `SetHost` is a
//!   flush, so alternating through one worker would be a reconnect per side and,
//!   worse, a race in which the reply that arrives cannot say which host it came
//!   from.

use std::collections::BTreeSet;
use std::time::Duration;

use eframe::egui::{self, Color32, ComboBox, Frame, Label, Margin, RichText, TextStyle, Ui};
use serde::Serialize;

use vmiscope_core::diff::Row;
use vmiscope_core::export::query_to_csv;
use vmiscope_core::{
    diff_tables, HostInfo, HostRef, QueryResult, Request, Response, RowDelta, TableDiff,
    WorkerRegistry,
};

use crate::app::{VmiScopeApp, DEFAULT_NAMESPACE};
use crate::config::{Config, CredRef};
use crate::theme::icons;
use crate::theme::tokens::{muted, BAD, OK, R_MD, S2, S3, S4, S6, WARN};
use crate::util::save_file;
use crate::widgets::button::{accent, btn_primary, btn_secondary, segmented};
use crate::widgets::chip::dot_chip;
use crate::widgets::field::mono_input;
use crate::widgets::loading::{format_ms, spinner};
use crate::widgets::rule::{hrule, HAIRLINE};
use crate::widgets::table::{cell_background, DataTable, DataTableState, TableColumn};

/// The class the view opens on. `Win32_Service` because it is the plan's own
/// acceptance case: it has a real key (`Name`), it is present on every Windows
/// build, and it carries exactly one genuinely volatile column (`ProcessId`).
const DEFAULT_CLASS: &str = "Win32_Service";

/// Starting width of a value column, and the point past which shrinking one
/// stops leaving anything identifiable. Same figures as the Query results grid.
const COL_W: f32 = 150.0;
const COL_MIN: f32 = 48.0;

/// The sign column. Fixed: it holds at most two characters and it is the one
/// column the eye scans down, so it must not move when a neighbour is dragged.
const SIGN_W: f32 = 46.0;

/// Cell tint strength. A tint has to be readable behind body text on this
/// ground, which is a much lower number than it looks.
const CELL_TINT: f32 = 0.20;
/// Whole-row tint, for the rows that exist on one side only. Lighter than a
/// changed cell: it covers every column, so the same strength would band.
const ROW_TINT: f32 = 0.11;

/// Width of the picker combo. Wide enough for `\\HOSTNAME as DOMAIN\user`.
const PICKER_W: f32 = 260.0;

// ---------------------------------------------------------------------------
// The four states a row can be in
// ---------------------------------------------------------------------------

/// What happened to one row between A and B.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Sign {
    /// On both sides, and equal once the ignored columns are set aside.
    Same,
    /// On both sides, and something outside the ignore list moved.
    Changed,
    /// On A only -- gone on B.
    OnlyA,
    /// On B only -- new on B.
    OnlyB,
}

impl Sign {
    /// The mark in the sign column.
    ///
    /// ASCII, deliberately. `docs/REDESIGN.md` writes these as U+2260 and
    /// U+2212, and `check.ps1`'s I9 rules reject both: the glyph allow-list is a
    /// fixed typography set and neither is in it.
    ///
    /// **Not because the fonts lack them** -- both are in the cmap of Inter
    /// *and* of JetBrains Mono, measured from the two embedded files, so the
    /// usual I9 failure mode (a blank box in a face that never had the glyph)
    /// does not apply here. Widening the allow-list is a project-wide decision
    /// about the invariant rather than one this view gets to take, and the
    /// ASCII pair is what a diff reads as anyway.
    fn mark(self) -> &'static str {
        match self {
            Sign::Same => "=",
            Sign::Changed => "!=",
            Sign::OnlyA => "-",
            Sign::OnlyB => "+",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Sign::Same => "Identical",
            Sign::Changed => "Changed",
            Sign::OnlyA => "Only on A",
            Sign::OnlyB => "Only on B",
        }
    }

    /// The row's colour. A is the left-hand reference, so a row that is on A and
    /// not on B reads as a removal and a row that is only on B reads as an
    /// addition -- the same convention as the `-` and `+` in the sign column,
    /// and the same one every diff tool has trained the eye on.
    fn color(self) -> Color32 {
        match self {
            Sign::Same => muted(40),
            Sign::Changed => WARN,
            Sign::OnlyA => BAD,
            Sign::OnlyB => OK,
        }
    }

    /// Position in the legend, and the sort rank of the sign column.
    fn rank(self) -> usize {
        match self {
            Sign::Same => 0,
            Sign::Changed => 1,
            Sign::OnlyA => 2,
            Sign::OnlyB => 3,
        }
    }

    const ALL: [Sign; 4] = [Sign::Same, Sign::Changed, Sign::OnlyA, Sign::OnlyB];
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Whether the query is built from a class name or typed as WQL.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    Class,
    Wql,
}

/// What the last run against one side produced.
#[derive(Default)]
enum SideState {
    /// Nothing has been asked of this side yet.
    #[default]
    Idle,
    /// A query is in flight.
    Running,
    /// The query answered. It may still be *partial* -- see
    /// [`QueryResult::completion`], which is what the partial gate reads.
    Done(QueryResult),
    /// The connection or the query failed. Both are reported here because both
    /// mean the same thing to the diff: this side has no table.
    Failed { context: String, message: String },
    /// The run never left this process, and why.
    Refused(String),
}

/// One side of the comparison.
struct Side {
    /// The picker's choice. Survives runs and view switches.
    target: HostRef,
    /// The id of the `SetHost` this run had to issue, if the worker was cold.
    connect_id: Option<u64>,
    /// The id of the query this run issued.
    query_id: Option<u64>,
    state: SideState,
    /// What the target said about itself when the worker connected. Free: the
    /// connect probes it anyway, and "which build am I comparing" is the first
    /// question anyone asks of a two-host diff.
    info: Option<HostInfo>,
    connect_ms: Option<u64>,
}

impl Side {
    fn new(target: HostRef) -> Self {
        Self {
            target,
            connect_id: None,
            query_id: None,
            state: SideState::Idle,
            info: None,
            connect_ms: None,
        }
    }

    fn result(&self) -> Option<&QueryResult> {
        match &self.state {
            SideState::Done(r) => Some(r),
            _ => None,
        }
    }

    fn is_running(&self) -> bool {
        matches!(self.state, SideState::Running)
    }

    /// Why this side cannot take part in a diff, if it cannot.
    fn partial_note(&self) -> Option<String> {
        self.result().and_then(|r| r.completion.note())
    }
}

/// Which side a helper is acting on. Two of everything, named rather than
/// indexed, because "side 0" is unreadable at the call site.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Which {
    A,
    B,
}

impl Which {
    fn label(self) -> &'static str {
        match self {
            Which::A => "A",
            Which::B => "B",
        }
    }
}

/// The Compare view's state.
pub(crate) struct CompareView {
    /// One COM thread per target. Owned here rather than by the app because
    /// nothing else in the application talks to two machines at once.
    registry: WorkerRegistry,
    a: Side,
    b: Side,
    shape: Shape,
    class: String,
    wql: String,
    namespace: String,
    /// The query and namespace the *last run* used.
    ///
    /// Not the fields above: those are an editor, and they keep changing after a
    /// run. An export that named them would label the rows on screen with a
    /// query that never produced them -- which is the one thing a saved
    /// comparison must not do, because the file outlives every other clue about
    /// where it came from.
    ran_wql: String,
    ran_namespace: String,
    /// Key columns the user picked, or `None` for the derived key.
    key_override: Option<Vec<String>>,
    /// Ignored columns the user picked, or `None` for the volatile defaults.
    ignore_override: Option<BTreeSet<String>>,
    /// The user has explicitly accepted a diff built from a partial side.
    allow_partial: bool,
    /// The prepared diff. Rebuilt only when something it depends on moves, not
    /// per frame: it is O(rows) and the table underneath it is virtualised.
    view: Option<DiffView>,
    /// `view` needs rebuilding before the next paint.
    stale: bool,
    table: DataTableState,
}

impl Default for CompareView {
    fn default() -> Self {
        Self {
            registry: WorkerRegistry::new(),
            a: Side::new(HostRef::Local),
            b: Side::new(HostRef::Local),
            shape: Shape::Class,
            class: DEFAULT_CLASS.to_string(),
            wql: format!("SELECT * FROM {DEFAULT_CLASS}"),
            namespace: DEFAULT_NAMESPACE.to_string(),
            ran_wql: String::new(),
            ran_namespace: String::new(),
            key_override: None,
            ignore_override: None,
            allow_partial: false,
            view: None,
            stale: false,
            table: DataTableState::default(),
        }
    }
}

impl CompareView {
    fn side(&self, which: Which) -> &Side {
        match which {
            Which::A => &self.a,
            Which::B => &self.b,
        }
    }

    fn side_mut(&mut self, which: Which) -> &mut Side {
        match which {
            Which::A => &mut self.a,
            Which::B => &mut self.b,
        }
    }

    fn is_running(&self) -> bool {
        self.a.is_running() || self.b.is_running()
    }

    /// The query both sides run. One query, not two: a diff of two different
    /// questions is not a diff.
    fn effective_wql(&self) -> String {
        match self.shape {
            Shape::Class => format!("SELECT * FROM {}", self.class.trim()),
            Shape::Wql => self.wql.trim().to_string(),
        }
    }

    fn effective_namespace(&self) -> String {
        let ns = self.namespace.trim();
        if ns.is_empty() {
            DEFAULT_NAMESPACE.to_string()
        } else {
            ns.to_string()
        }
    }

    fn can_run(&self) -> bool {
        !self.is_running()
            && match self.shape {
                Shape::Class => !self.class.trim().is_empty(),
                Shape::Wql => !self.wql.trim().is_empty(),
            }
    }
}

// ---------------------------------------------------------------------------
// Target pickers
// ---------------------------------------------------------------------------

/// One row of a host picker.
struct Candidate {
    host: HostRef,
    label: String,
    /// The principal, as the targets list records it.
    principal: String,
    /// Can Compare actually open a worker for this target?
    reachable: bool,
}

/// Every target the pickers offer: this machine, then each saved target.
///
/// Alternate-credential targets are listed but **not selectable**. A password is
/// never persisted (by [`CredRef`]'s construction), Compare owns its own workers
/// rather than borrowing the connected one, and it has no password prompt -- so
/// selecting one could only ever produce a connection that fails. Listing it
/// disabled, with the reason, is the honest version of that.
fn candidates(config: &Config) -> Vec<Candidate> {
    let mut out = vec![Candidate {
        host: HostRef::Local,
        label: "This machine".to_string(),
        principal: "current user".to_string(),
        reachable: true,
    }];
    for t in &config.targets {
        let name = t.name.trim();
        if name.is_empty() {
            continue;
        }
        let (host, reachable) = match &t.cred_ref {
            CredRef::CurrentUser => (
                HostRef::Sso {
                    host: name.to_string(),
                },
                true,
            ),
            CredRef::Alt { user, domain } => (
                HostRef::Alt {
                    host: name.to_string(),
                    user: match domain {
                        Some(d) if !d.is_empty() => format!(r"{d}\{user}"),
                        _ => user.clone(),
                    },
                },
                false,
            ),
        };
        out.push(Candidate {
            label: format!(r"\\{name}"),
            principal: t.cred_ref.label(),
            host,
            reachable,
        });
    }
    out
}

// ---------------------------------------------------------------------------
// Keys and ignores
// ---------------------------------------------------------------------------

/// Where the key columns came from. Shown on screen: an alignment nobody can
/// see is an alignment nobody can check.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum KeySource {
    /// The user picked the columns.
    Manual,
    /// The class's own `key` properties, as the schema declares them.
    ClassKey,
    /// `__RELPATH` -- the object's relative path, which is stable across hosts
    /// (unlike `__PATH`, which embeds the machine name).
    RelPath,
    /// Nothing usable was declared, so identity is the whole row.
    WholeRow,
}

impl KeySource {
    fn note(self) -> &'static str {
        match self {
            KeySource::Manual => "manual",
            KeySource::ClassKey => "class key",
            KeySource::RelPath => "__RELPATH",
            KeySource::WholeRow => "whole row",
        }
    }
}

/// The columns a diff of `a` and `b` should key on, and where they came from.
///
/// The declared key is only usable if the query actually *returned* it: a
/// projection like `SELECT State FROM Win32_Service` still reports `Name` as the
/// class key, and keying on a column that is not in the result gives every row
/// the same empty key -- which pairs rows by arrival order and reports garbage
/// with total confidence. So a declared key that is missing from either side is
/// dropped, and the caller is told it was.
fn resolve_keys(
    a: &QueryResult,
    b: &QueryResult,
    over: Option<&Vec<String>>,
) -> (Vec<String>, KeySource, Option<String>) {
    if let Some(keys) = over {
        return (keys.clone(), KeySource::Manual, None);
    }

    let has = |r: &QueryResult, c: &String| r.columns.contains(c);
    let declared: Vec<String> = if !a.key_columns.is_empty() {
        a.key_columns.clone()
    } else {
        b.key_columns.clone()
    };
    let usable: Vec<String> = declared
        .iter()
        .filter(|c| has(a, c) && has(b, c))
        .cloned()
        .collect();
    if !usable.is_empty() && usable.len() == declared.len() {
        return (usable, KeySource::ClassKey, None);
    }

    // A partly-usable key is not a key: matching on some of a compound key
    // merges rows that the schema says are distinct.
    let dropped = declared
        .iter()
        .filter(|c| !(has(a, c) && has(b, c)))
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");

    let relpath = "__RELPATH".to_string();
    if has(a, &relpath) && has(b, &relpath) {
        let note = (!dropped.is_empty()).then(|| {
            format!("the class key ({dropped}) was not returned by this query, so rows are keyed on __RELPATH")
        });
        return (vec![relpath], KeySource::RelPath, note);
    }

    let note = Some(if dropped.is_empty() {
        "this query has no declared key and no __RELPATH, so identity is the whole row \u{2014} \
         any moving column reports as a removal plus an addition"
            .to_string()
    } else {
        format!(
            "the class key ({dropped}) was not returned and there is no __RELPATH, so identity \
             is the whole row"
        )
    });
    (Vec::new(), KeySource::WholeRow, note)
}

/// Does this column's value move on its own?
///
/// Name-based, because a `QueryResult` carries no types -- and deliberately
/// narrow, because a default that hides a real difference is worse than one that
/// leaves noise on screen. Every rule below is a class of column whose value is
/// *expected* to differ between two reads of the same healthy machine:
///
/// * `__PATH` embeds the machine name, so it differs between any two hosts by
///   construction. (`__RELPATH` does not, which is why that one is a key.)
/// * anything ending in `Time`, `Date` or carrying `Timestamp` -- boot times,
///   install dates, cumulative CPU ticks, perf-counter stamps.
/// * the process identity: `ProcessId`, `IDProcess`, a bare `Handle`.
/// * anything ending in `Count` or `Bytes`, or carrying `Usage`, `WorkingSet`,
///   `VirtualSize`, `Elapsed` or `Uptime` -- live counters and live memory.
/// * anything starting with `Percent` -- the perf classes' rate columns.
/// * free space: `FreeSpace`, `Available*`.
///
/// `Bytes` and `Count` are matched at the *end* of the name on purpose:
/// `BytesPerSector` is a fact about a disk, not a reading off it. `Size` on its
/// own is not matched at all for the same reason -- `Win32_LogicalDisk.Size` is
/// the disk, and hiding it would hide a real difference between two machines --
/// so the two process-memory columns that need it are named:
/// `VirtualSize`/`PeakVirtualSize` were measured moving on 18 of 365 processes
/// between two reads a fraction of a second apart.
fn is_volatile_column(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n == "__path"
        || n.ends_with("time")
        || n.ends_with("date")
        || n.contains("timestamp")
        || n.contains("processid")
        || n == "idprocess"
        || n == "handle"
        || n.ends_with("count")
        || n.ends_with("bytes")
        || n.contains("usage")
        || n.contains("workingset")
        || n.contains("virtualsize")
        || n.contains("elapsed")
        || n.contains("uptime")
        || n.starts_with("percent")
        || n.contains("freespace")
        || n.starts_with("available")
}

/// The columns this diff disregards: the user's list, or the volatile default.
///
/// A key column is never in the derived list, whatever its name looks like.
/// `Win32_Process.Handle` is the PID *and* the class key, and the two readings
/// of it are opposite: as a value it churns, as a key it is the row's identity.
/// Ignoring it would also feed straight into `diff_tables`' whole-row fallback,
/// which builds its key out of the non-ignored columns -- so an ignored key
/// column can silently stop being part of the identity at all.
fn resolve_ignores(
    columns: &[String],
    keys: &[String],
    over: Option<&BTreeSet<String>>,
) -> Vec<String> {
    match over {
        Some(set) => set.iter().cloned().collect(),
        None => columns
            .iter()
            .filter(|c| is_volatile_column(c) && !keys.contains(c))
            .cloned()
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// The prepared diff
// ---------------------------------------------------------------------------

/// One cell of the diff table, resolved.
struct Cell {
    text: String,
    /// Does this cell carry the difference? Only a changed row's changed columns
    /// do; on a one-sided row the whole row is tinted instead.
    changed: bool,
}

/// One line of the diff table.
struct Line {
    sign: Sign,
    cells: Vec<Cell>,
}

/// A computed diff, resolved into exactly what the table paints.
struct DiffView {
    /// Display order: the key columns, then A's columns, then anything only B
    /// returned.
    columns: Vec<String>,
    keys: Vec<String>,
    key_source: KeySource,
    key_note: Option<String>,
    ignored: Vec<String>,
    lines: Vec<Line>,
    /// Per-[`Sign::rank`] counts, for the legend.
    counts: [usize; 4],
    /// Columns only one side returned. Not an error -- two Windows builds really
    /// do disagree about a class -- but it makes every matched row "changed",
    /// so it has to be said out loud.
    only_a_cols: Vec<String>,
    only_b_cols: Vec<String>,
    /// Was either side partial? Stamped into the export and shown above the
    /// table; a diff built on a truncated read is not a diff of the two tables.
    sound: bool,
    /// The raw diff, kept for the JSON export.
    diff: TableDiff,
}

/// Turn a positional [`QueryResult`] row into the name-aligned map the core's
/// differ works in. The same shape `diff::row_maps` builds, rebuilt here because
/// it is private and the identical rows have to be recovered from B's own order.
fn row_maps(qr: &QueryResult) -> Vec<Row> {
    qr.rows
        .iter()
        .map(|vals| {
            qr.columns
                .iter()
                .cloned()
                .zip(vals.iter().cloned())
                .collect()
        })
        .collect()
}

/// The display column order: keys first, then A's own order, then the columns
/// only B returned.
fn display_columns(a: &QueryResult, b: &QueryResult, keys: &[String]) -> Vec<String> {
    let mut cols: Vec<String> = Vec::new();
    for c in keys.iter().chain(a.columns.iter()).chain(b.columns.iter()) {
        if !cols.contains(c) {
            cols.push(c.clone());
        }
    }
    cols
}

/// One cell's text: the value, or an em dash when the column is absent on that
/// side. An absent column and an empty string are different facts, and a diff
/// that renders them the same hides the more interesting one.
fn cell_text(row: &Row, column: &str) -> String {
    match row.get(column) {
        Some(v) => v.clone(),
        None => "\u{2014}".to_string(),
    }
}

/// What the diff says about one row of B.
///
/// `Added` carries nothing: an added row *is* the B row being walked, so the
/// caller already holds every value it would carry. `Changed` carries the delta,
/// because that is where A's side of the row lives.
enum Verdict<'a> {
    Same,
    Added,
    Changed(&'a RowDelta),
}

/// Label every row of B from the diff, in B's own order.
///
/// [`TableDiff`] reports `added` and `changed` in B's order but does not report
/// the rows it *matched and found equal* -- it only counts them. Recovering them
/// is what lets the table show the whole comparison rather than only its
/// differences, and it is done by walking B alongside the two lists: a row that
/// equals the next unconsumed add is that add, a row that equals the next
/// unconsumed change is that change, and anything else is a row the differ
/// matched.
///
/// The equality check is what makes the cursors safe. Trusting the position
/// alone would mislabel every row after any divergence; with the check, a
/// divergence can only ever produce a row labelled `Same` that was in fact
/// equal to nothing the differ reported -- and duplicate rows with identical
/// content are indistinguishable on screen anyway.
fn label_b_rows<'d>(b_rows: &[Row], diff: &'d TableDiff) -> Vec<Verdict<'d>> {
    let (mut ai, mut ci) = (0usize, 0usize);
    b_rows
        .iter()
        .map(|row| {
            if diff.added.get(ai).is_some_and(|d| &d.values == row) {
                ai += 1;
                return Verdict::Added;
            }
            if diff.changed.get(ci).is_some_and(|d| &d.b == row) {
                ci += 1;
                return Verdict::Changed(&diff.changed[ci - 1]);
            }
            Verdict::Same
        })
        .collect()
}

/// Build everything the table paints from two results and the caller's key and
/// ignore lists.
fn build_view(
    a: &QueryResult,
    b: &QueryResult,
    keys: Vec<String>,
    key_source: KeySource,
    key_note: Option<String>,
    ignored: Vec<String>,
) -> DiffView {
    let diff = diff_tables(a, b, &keys, &ignored);
    let columns = display_columns(a, b, &keys);
    let b_rows = row_maps(b);

    let mut lines: Vec<Line> = Vec::with_capacity(b_rows.len() + diff.removed.len());
    let mut counts = [0usize; 4];

    for (row, verdict) in b_rows.iter().zip(label_b_rows(&b_rows, &diff)) {
        let line = match verdict {
            Verdict::Same => Line {
                sign: Sign::Same,
                cells: columns
                    .iter()
                    .map(|c| Cell {
                        text: cell_text(row, c),
                        changed: false,
                    })
                    .collect(),
            },
            Verdict::Added => Line {
                sign: Sign::OnlyB,
                cells: columns
                    .iter()
                    .map(|c| Cell {
                        text: cell_text(row, c),
                        changed: false,
                    })
                    .collect(),
            },
            Verdict::Changed(delta) => {
                let moved: BTreeSet<&str> =
                    delta.changed_columns.iter().map(String::as_str).collect();
                Line {
                    sign: Sign::Changed,
                    cells: columns
                        .iter()
                        .map(|c| {
                            if moved.contains(c.as_str()) {
                                Cell {
                                    // Both sides, in one cell: a changed row whose
                                    // old value you have to hover for is a changed
                                    // row you have to take on trust.
                                    text: format!(
                                        "{} \u{2192} {}",
                                        cell_text(&delta.a, c),
                                        cell_text(&delta.b, c)
                                    ),
                                    changed: true,
                                }
                            } else {
                                Cell {
                                    text: cell_text(&delta.b, c),
                                    changed: false,
                                }
                            }
                        })
                        .collect(),
                }
            }
        };
        counts[line.sign.rank()] += 1;
        lines.push(line);
    }

    for removed in &diff.removed {
        counts[Sign::OnlyA.rank()] += 1;
        lines.push(Line {
            sign: Sign::OnlyA,
            cells: columns
                .iter()
                .map(|c| Cell {
                    text: cell_text(&removed.values, c),
                    changed: false,
                })
                .collect(),
        });
    }

    let a_cols: BTreeSet<&String> = a.columns.iter().collect();
    let b_cols: BTreeSet<&String> = b.columns.iter().collect();
    let sound = a.completion.is_complete() && b.completion.is_complete();

    DiffView {
        only_a_cols: a_cols.difference(&b_cols).map(|c| (*c).clone()).collect(),
        only_b_cols: b_cols.difference(&a_cols).map(|c| (*c).clone()).collect(),
        columns,
        keys,
        key_source,
        key_note,
        ignored,
        lines,
        counts,
        sound,
        diff,
    }
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

/// The JSON compare file: the diff, plus everything needed to read it later.
///
/// A bare [`TableDiff`] is not a document -- it does not say which machines,
/// which query, what it keyed on, what it disregarded, or whether either side
/// was whole. Every one of those changes what the rows mean.
#[derive(Serialize)]
struct DiffExport<'a> {
    a: String,
    b: String,
    namespace: &'a str,
    wql: &'a str,
    key_columns: &'a [String],
    key_source: &'static str,
    ignored_columns: &'a [String],
    /// False when either side was truncated, timed out or was cancelled. The
    /// rows are still here; what is not here is the right to read them as a
    /// complete comparison.
    sound: bool,
    a_completion: Option<String>,
    b_completion: Option<String>,
    identical: usize,
    changed: usize,
    only_on_a: usize,
    only_on_b: usize,
    diff: &'a TableDiff,
}

/// The diff as CSV: one line per *side-row*, so nothing is folded together.
///
/// A changed row is two lines (`!=` on A, `!=` on B) rather than one line of
/// `old -> new` strings, because a spreadsheet column has to hold one value to
/// be sortable or filterable. Identical rows are included: the file is the whole
/// comparison, and a reader who wants only the differences can filter the first
/// column.
///
/// Built as a [`QueryResult`] and handed to the core's own exporter rather than
/// re-implementing RFC 4180 quoting here -- that escaping is already written and
/// already tested.
fn diff_to_csv(view: &DiffView) -> String {
    let mut columns = vec!["Diff".to_string(), "Side".to_string()];
    columns.extend(view.columns.iter().cloned());

    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut push = |sign: Sign, side: &str, row: &Row| {
        let mut cells = vec![sign.mark().to_string(), side.to_string()];
        cells.extend(view.columns.iter().map(|c| cell_text(row, c)));
        rows.push(cells);
    };

    for delta in &view.diff.changed {
        push(Sign::Changed, "A", &delta.a);
        push(Sign::Changed, "B", &delta.b);
    }
    for added in &view.diff.added {
        push(Sign::OnlyB, "B", &added.values);
    }
    for removed in &view.diff.removed {
        push(Sign::OnlyA, "A", &removed.values);
    }
    // The identical rows are only held as prepared lines, so they are written
    // from those. They are B's rows by construction -- A's are equal to them.
    for line in view.lines.iter().filter(|l| l.sign == Sign::Same) {
        let mut cells = vec![Sign::Same.mark().to_string(), "B".to_string()];
        cells.extend(line.cells.iter().map(|c| c.text.clone()));
        rows.push(cells);
    }

    query_to_csv(&QueryResult {
        columns,
        rows,
        ..Default::default()
    })
}

// ---------------------------------------------------------------------------
// Dispatch and replies
// ---------------------------------------------------------------------------

impl VmiScopeApp {
    /// Drain both compare workers. Called every frame from `eframe::App::ui`,
    /// not only while the view is on screen: a run that finished behind the
    /// user's back must still land, or coming back to the view would show a
    /// spinner for a query that completed minutes ago.
    pub(crate) fn compare_poll(&mut self, ctx: &egui::Context) {
        for (_target, resp) in self.compare.registry.poll() {
            self.compare_apply(resp);
        }
        if self.compare.is_running() {
            ctx.request_repaint_after(Duration::from_millis(30));
        }
    }

    /// Route one reply to the side that asked for it.
    fn compare_apply(&mut self, resp: Response) {
        let id = resp.id();
        let Some(which) = [Which::A, Which::B].into_iter().find(|w| {
            let s = self.compare.side(*w);
            s.connect_id == Some(id) || s.query_id == Some(id)
        }) else {
            // A reply for a run that has already been superseded. Dropped
            // deliberately: applying it would overwrite the current run's state
            // with an older one's.
            return;
        };

        match resp {
            Response::HostConnected {
                connect_ms, info, ..
            } => {
                let side = self.compare.side_mut(which);
                side.connect_id = None;
                side.connect_ms = Some(connect_ms);
                side.info = Some(info);
            }
            Response::QueryResult { result, .. } => {
                let side = self.compare.side_mut(which);
                side.query_id = None;
                side.state = SideState::Done(result);
                self.compare.stale = true;
            }
            Response::Error {
                context, message, ..
            } => {
                let side = self.compare.side_mut(which);
                side.connect_id = None;
                side.query_id = None;
                side.state = SideState::Failed {
                    context: context.clone(),
                    message: message.clone(),
                };
                self.compare.stale = true;
                // The side card carries the failure; the log carries it too,
                // because every other failed request in this application ends up
                // there and a compare failure is not a special case.
                self.push_error(format!("Compare {}: {context}\n{message}", which.label()));
            }
            // The compare workers are only ever sent `SetHost` and `Query`, so
            // nothing else can arrive under an id either side owns.
            _ => {}
        }
    }

    /// The status bar's line for this view: the four counts, or the reason there
    /// are none.
    pub(crate) fn compare_status(&self) -> String {
        if self.compare.is_running() {
            return "Comparing\u{2026}".to_string();
        }
        for which in [Which::A, Which::B] {
            match &self.compare.side(which).state {
                SideState::Failed { .. } => {
                    return format!("Side {} did not answer", which.label())
                }
                SideState::Refused(_) => return format!("Side {} was not run", which.label()),
                _ => {}
            }
        }
        match self.compare.view.as_ref() {
            Some(view) => {
                let counts = view.counts;
                let line = format!(
                    "{} identical \u{00b7} {} changed \u{00b7} {} only on A \u{00b7} {} only on B",
                    counts[Sign::Same.rank()],
                    counts[Sign::Changed.rank()],
                    counts[Sign::OnlyA.rank()],
                    counts[Sign::OnlyB.rank()],
                );
                if view.sound {
                    line
                } else {
                    format!("{line} \u{00b7} partial")
                }
            }
            // Both sides may well have answered; the diff is withheld because
            // one of them is not a whole table. Saying "no comparison run"
            // there would describe the wrong problem.
            None if [Which::A, Which::B]
                .into_iter()
                .any(|w| self.compare.side(w).partial_note().is_some()) =>
            {
                "Partial read \u{2014} the diff is withheld".to_string()
            }
            None => "No comparison run".to_string(),
        }
    }

    /// Re-run the last comparison. Bound to the title bar's refresh.
    pub(crate) fn compare_refresh(&mut self) {
        if self.compare.can_run() {
            self.compare_run();
        }
    }

    /// Run the query against both sides.
    fn compare_run(&mut self) {
        self.compare.allow_partial = false;
        self.compare.view = None;
        self.compare.stale = false;
        self.compare.table = DataTableState::default();
        // The key and ignore overrides are dropped: they name columns of the
        // *previous* result, and carrying a key that the new query does not
        // return is the exact mis-keying `resolve_keys` exists to refuse.
        self.compare.key_override = None;
        self.compare.ignore_override = None;

        let wql = self.compare.effective_wql();
        let namespace = self.compare.effective_namespace();
        self.compare_dispatch(Which::A, &wql, &namespace);
        self.compare_dispatch(Which::B, &wql, &namespace);
        // Stamped from the run, not read back off the editor when the export
        // happens: the two drift the moment anyone types.
        self.compare.ran_wql = wql;
        self.compare.ran_namespace = namespace;
    }

    /// Open (if needed) and query one side.
    fn compare_dispatch(&mut self, which: Which, wql: &str, namespace: &str) {
        let target = self.compare.side(which).target.clone();

        if target.is_alt_cred() {
            let side = self.compare.side_mut(which);
            side.state = SideState::Refused(
                "alternate credentials are not persisted, and Compare has no password prompt \
                 \u{2014} connect from Machines instead"
                    .to_string(),
            );
            return;
        }

        // A worker is opened once and kept: `SetHost` flushes every cached
        // connection, so re-opening on each run would pay a reconnect per run
        // for nothing.
        let connect_id = if self.compare.registry.is_open(&target) {
            None
        } else {
            let id = self.alloc_id();
            self.compare.registry.open(id, &target, None);
            Some(id)
        };

        let id = self.alloc_id();
        let sent = self.compare.registry.send(
            &target,
            Request::Query {
                id,
                namespace: namespace.to_string(),
                wql: wql.to_string(),
                // Both bounds come from Settings, exactly as the Query view's
                // runs do -- and both are why the partial gate exists.
                max_rows: Some(self.config.row_limit),
                timeout: Some(Duration::from_secs(self.config.operation_timeout_secs)),
                // The identity columns, which is the whole reason this request
                // shape grew the flag: `__RELPATH` is the key for every class
                // that declares none of its own.
                include_system: true,
            },
        );

        let side = self.compare.side_mut(which);
        side.connect_id = connect_id;
        if sent {
            side.query_id = Some(id);
            side.state = SideState::Running;
        } else {
            side.query_id = None;
            side.state = SideState::Refused("no worker is open for this target".to_string());
        }
    }

    /// Rebuild the prepared diff from whatever both sides currently hold.
    fn compare_rebuild(&mut self) {
        self.compare.stale = false;
        self.compare.view = None;

        let (Some(a), Some(b)) = (self.compare.a.result(), self.compare.b.result()) else {
            return;
        };
        // The partial gate. A side that stopped early did not read the rows it
        // did not read; diffing against it reports every one of them as a
        // difference that the two machines do not have.
        let whole = a.completion.is_complete() && b.completion.is_complete();
        if !(whole || self.compare.allow_partial) {
            return;
        }

        let (keys, source, note) = resolve_keys(a, b, self.compare.key_override.as_ref());
        let columns = display_columns(a, b, &keys);
        let ignored = resolve_ignores(&columns, &keys, self.compare.ignore_override.as_ref());
        self.compare.view = Some(build_view(a, b, keys, source, note, ignored));
    }
}

// ---------------------------------------------------------------------------
// UI
// ---------------------------------------------------------------------------

impl VmiScopeApp {
    pub(crate) fn ui_compare(&mut self, ui: &mut Ui) {
        if self.compare.stale {
            self.compare_rebuild();
        }

        self.ui_compare_header(ui);
        self.ui_compare_sides(ui);
        self.ui_compare_query(ui);
        hrule(ui);
        self.ui_compare_state(ui);
        self.ui_compare_table(ui);
    }

    /// Title, the A-vs-B line, Run and Export.
    fn ui_compare_header(&mut self, ui: &mut Ui) {
        let mut run = false;
        let mut export_json = false;
        let mut export_csv = false;
        let has_view = self.compare.view.is_some();
        let can_run = self.compare.can_run();

        Frame::NONE
            .inner_margin(Margin::symmetric(S4 as i8, S3 as i8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(icons::labelled_styled(
                        ui,
                        icons::GIT_DIFF,
                        "Compare",
                        TextStyle::Body,
                        accent(ui),
                    ));
                    ui.label(
                        RichText::new(format!(
                            "{} vs {}",
                            self.compare.a.target.label(),
                            self.compare.b.target.label()
                        ))
                        .text_style(TextStyle::Small)
                        .color(muted(50)),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.menu_button(icons::labelled(ui, icons::EXPORT, "Export diff"), |ui| {
                            if has_view {
                                if ui
                                    .button(icons::labelled(
                                        ui,
                                        icons::BRACKETS_CURLY,
                                        "Diff as JSON",
                                    ))
                                    .clicked()
                                {
                                    export_json = true;
                                    ui.close();
                                }
                                if ui
                                    .button(icons::labelled(ui, icons::FILE_CSV, "Diff as CSV"))
                                    .clicked()
                                {
                                    export_csv = true;
                                    ui.close();
                                }
                            } else {
                                ui.label(RichText::new("Run a comparison first").color(muted(40)));
                            }
                        });
                        let label = if self.compare.is_running() {
                            "Running"
                        } else {
                            "Run"
                        };
                        let resp = ui.add_enabled_ui(can_run, |ui| {
                            btn_primary(ui, icons::labelled(ui, icons::PLAY, label))
                        });
                        if resp.inner.clicked() {
                            run = true;
                        }
                    });
                });
            });

        if run {
            self.compare_run();
        }
        if export_json || export_csv {
            if let Some(text) = self.compare_export_text(export_json) {
                let name = if export_json {
                    "compare.json"
                } else {
                    "compare.csv"
                };
                save_file(name, &text);
            }
        }
    }

    /// The exported document, without the file dialog.
    ///
    /// Separate from the save so the composition can be exercised: the JSON's
    /// worth is in the context around the diff -- which hosts, which query, what
    /// it keyed on, whether either side was whole -- and none of that is visible
    /// from a call that ends in a native dialog.
    fn compare_export_text(&self, as_json: bool) -> Option<String> {
        let view = self.compare.view.as_ref()?;
        Some(if as_json {
            let doc = DiffExport {
                a: self.compare.a.target.label(),
                b: self.compare.b.target.label(),
                namespace: &self.compare.ran_namespace,
                wql: &self.compare.ran_wql,
                key_columns: &view.keys,
                key_source: view.key_source.note(),
                ignored_columns: &view.ignored,
                sound: view.sound,
                a_completion: self.compare.a.partial_note(),
                b_completion: self.compare.b.partial_note(),
                identical: view.counts[Sign::Same.rank()],
                changed: view.counts[Sign::Changed.rank()],
                only_on_a: view.counts[Sign::OnlyA.rank()],
                only_on_b: view.counts[Sign::OnlyB.rank()],
                diff: &view.diff,
            };
            serde_json::to_string_pretty(&doc).unwrap_or_default()
        } else {
            diff_to_csv(view)
        })
    }

    /// The two target pickers, side by side, each with its own status line.
    fn ui_compare_sides(&mut self, ui: &mut Ui) {
        let candidates = candidates(&self.config);
        Frame::NONE
            .inner_margin(Margin::symmetric(S4 as i8, 0))
            .show(ui, |ui| {
                ui.columns(2, |cols| {
                    self.ui_compare_side(&mut cols[0], Which::A, &candidates);
                    self.ui_compare_side(&mut cols[1], Which::B, &candidates);
                });
            });
    }

    fn ui_compare_side(&mut self, ui: &mut Ui, which: Which, candidates: &[Candidate]) {
        let title = format!("Side {}", which.label());
        ui.label(
            RichText::new(title)
                .text_style(TextStyle::Small)
                .color(muted(55)),
        );

        let current = self.compare.side(which).target.clone();
        let selected = candidates
            .iter()
            .find(|c| c.host == current)
            .map(|c| c.label.clone())
            // A target the user has since forgotten in Machines: keep showing
            // what this side is actually pointed at rather than silently
            // re-pointing it somewhere else.
            .unwrap_or_else(|| current.label());

        let mut picked: Option<HostRef> = None;
        ComboBox::from_id_salt(format!("vs_cmp_target_{}", which.label()))
            .width(PICKER_W)
            .selected_text(RichText::new(selected).text_style(TextStyle::Monospace))
            .show_ui(ui, |ui| {
                for c in candidates {
                    let row = ui.add_enabled(
                        c.reachable,
                        egui::Button::selectable(
                            c.host == current,
                            RichText::new(format!("{}  \u{00b7}  {}", c.label, c.principal))
                                .text_style(TextStyle::Monospace),
                        ),
                    );
                    if !c.reachable {
                        row.on_hover_text(
                            "Alternate credentials are not persisted \u{2014} a password lives \
                             only in the Machines form and in the worker that already holds it, \
                             so Compare cannot open its own connection as this principal.",
                        );
                    } else if row.clicked() {
                        picked = Some(c.host.clone());
                    }
                }
            });
        if let Some(host) = picked {
            let side = self.compare.side_mut(which);
            if side.target != host {
                side.target = host;
                // The old result belongs to the old machine.
                side.state = SideState::Idle;
                side.info = None;
                side.connect_ms = None;
                self.compare.view = None;
            }
        }

        self.ui_compare_side_status(ui, which);
    }

    fn ui_compare_side_status(&mut self, ui: &mut Ui, which: Which) {
        let side = self.compare.side(which);
        ui.add_space(S2);
        match &side.state {
            SideState::Idle => {
                dot_chip(ui, muted(35), "not run");
            }
            SideState::Running => {
                spinner(ui, "querying\u{2026}");
            }
            SideState::Done(result) => {
                let partial = result.completion.note();
                ui.horizontal(|ui| {
                    let color = if partial.is_some() { WARN } else { OK };
                    dot_chip(
                        ui,
                        color,
                        &format!(
                            "{} row{} \u{00b7} {}",
                            result.rows.len(),
                            if result.rows.len() == 1 { "" } else { "s" },
                            format_ms(result.elapsed_ms)
                        ),
                    );
                });
                if let Some(note) = partial {
                    ui.label(RichText::new(note).text_style(TextStyle::Small).color(WARN));
                }
            }
            SideState::Failed { context, message } => {
                ui.horizontal(|ui| {
                    dot_chip(ui, BAD, "failed");
                })
                .response
                .on_hover_text(format!("{context}\n{message}"));
                ui.add(
                    Label::new(
                        RichText::new(first_line(message))
                            .text_style(TextStyle::Small)
                            .color(muted(60)),
                    )
                    .truncate(),
                )
                .on_hover_text(format!("{context}\n{message}"));
            }
            SideState::Refused(reason) => {
                dot_chip(ui, muted(45), "refused");
                ui.add(
                    Label::new(
                        RichText::new(reason.as_str())
                            .text_style(TextStyle::Small)
                            .color(muted(60)),
                    )
                    .wrap(),
                );
            }
        }

        // What the target says it is. The connect probes this anyway, and the
        // first question of any two-host diff is which builds are being
        // compared.
        //
        // A side whose worker was already open never issued a `SetHost` and so
        // never got a probe of its own; when both sides name the same target --
        // which is the smoke test, and the same worker -- the other side's probe
        // is a probe of this one's machine, so it is shown rather than left
        // blank.
        let other = self.compare.side(match which {
            Which::A => Which::B,
            Which::B => Which::A,
        });
        let info = side.info.as_ref().or_else(|| {
            (other.target == side.target)
                .then_some(other.info.as_ref())
                .flatten()
        });
        if let Some(info) = info {
            let summary = info.summary();
            if !summary.is_empty() {
                ui.add(
                    Label::new(
                        RichText::new(summary)
                            .text_style(TextStyle::Small)
                            .color(muted(38)),
                    )
                    .truncate(),
                );
            }
        }
    }

    /// The class or WQL picker, and the namespace both sides read.
    fn ui_compare_query(&mut self, ui: &mut Ui) {
        let mut run = false;
        Frame::NONE
            .inner_margin(Margin::symmetric(S4 as i8, S2 as i8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    segmented(
                        ui,
                        &mut self.compare.shape,
                        &[(Shape::Class, "Class"), (Shape::Wql, "WQL")],
                    );
                    ui.label(
                        RichText::new("namespace")
                            .text_style(TextStyle::Small)
                            .color(muted(45)),
                    );
                    ui.scope(|ui| {
                        ui.set_width(NS_W);
                        mono_input(ui, &mut self.compare.namespace, DEFAULT_NAMESPACE);
                    });
                    let typed = match self.compare.shape {
                        Shape::Class => {
                            ui.label(
                                RichText::new("class")
                                    .text_style(TextStyle::Small)
                                    .color(muted(45)),
                            );
                            mono_input(ui, &mut self.compare.class, DEFAULT_CLASS)
                        }
                        Shape::Wql => {
                            mono_input(ui, &mut self.compare.wql, "SELECT * FROM Win32_Service")
                        }
                    };
                    // Enter runs, the same as it does in every other single-line
                    // field in the app. `lost_focus` is how a `TextEdit` reports
                    // it: the key is consumed by the field before any shortcut
                    // layer can see it.
                    if typed.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        run = true;
                    }
                });
                if self.compare.shape == Shape::Class {
                    ui.label(
                        RichText::new(self.compare.effective_wql())
                            .text_style(TextStyle::Monospace)
                            .size(11.0)
                            .color(muted(38)),
                    );
                }
            });
        if run && self.compare.can_run() {
            self.compare_run();
        }
    }

    /// Everything between the query row and the table: the reasons there is no
    /// diff, the warnings about the one there is, and the legend.
    fn ui_compare_state(&mut self, ui: &mut Ui) {
        Frame::NONE
            .inner_margin(Margin::symmetric(S4 as i8, 0))
            .show(ui, |ui| {
                // A side with no table is the whole story: say which one, and
                // what it said, rather than showing an empty grid.
                for which in [Which::A, Which::B] {
                    let side = self.compare.side(which);
                    match &side.state {
                        SideState::Failed { context, message } => banner(
                            ui,
                            BAD,
                            icons::WARNING_CIRCLE,
                            &format!("Side {} did not answer", which.label()),
                            &format!("{context}\n{message}"),
                        ),
                        SideState::Refused(reason) => banner(
                            ui,
                            WARN,
                            icons::PROHIBIT,
                            &format!("Side {} was not run", which.label()),
                            reason,
                        ),
                        _ => {}
                    }
                }

                // A partial side, and the one affordance that gets past it.
                let partial: Vec<String> = [Which::A, Which::B]
                    .into_iter()
                    .filter_map(|w| {
                        self.compare
                            .side(w)
                            .partial_note()
                            .map(|n| format!("side {}: {n}", w.label()))
                    })
                    .collect();
                if !partial.is_empty() {
                    let mut proceed = false;
                    banner(
                        ui,
                        WARN,
                        icons::WARNING,
                        "One side is not a whole table",
                        &format!(
                            "{}. Every row it never read would be reported as a difference \
                             between the two machines, which is a finding neither machine has. \
                             Raise the row limit or the timeout in Settings and run it again.",
                            partial.join("; ")
                        ),
                    );
                    if !self.compare.allow_partial {
                        ui.horizontal(|ui| {
                            if btn_secondary(
                                ui,
                                icons::labelled(ui, icons::EYE, "Diff the partial results anyway"),
                            )
                            .on_hover_text(
                                "The rows that were read are still real. The comparison of them \
                                 is not, and everything it produces \u{2014} including the \
                                 export \u{2014} is marked unsound.",
                            )
                            .clicked()
                            {
                                proceed = true;
                            }
                        });
                    }
                    if proceed {
                        self.compare.allow_partial = true;
                        self.compare.stale = true;
                    }
                }

                let Some(view) = self.compare.view.as_ref() else {
                    return;
                };

                if !view.sound {
                    banner(
                        ui,
                        WARN,
                        icons::WARNING,
                        "Partial comparison",
                        "One side stopped early. This is a comparison of what was read, not of \
                         the two machines.",
                    );
                }

                // Two builds really can disagree about a class. The differ
                // aligns by name and survives it, but a column present on one
                // side only counts as a difference on every matched row -- so
                // an unexplained wall of Changed would otherwise be the first
                // thing on screen.
                if !view.only_a_cols.is_empty() || !view.only_b_cols.is_empty() {
                    let mut ignore_them = false;
                    banner(
                        ui,
                        WARN,
                        icons::WARNING_CIRCLE,
                        "The two sides returned different columns",
                        &format!(
                            "only on A: {} \u{00b7} only on B: {}",
                            list_or_none(&view.only_a_cols),
                            list_or_none(&view.only_b_cols)
                        ),
                    );
                    ui.horizontal(|ui| {
                        if btn_secondary(
                            ui,
                            icons::labelled(
                                ui,
                                icons::FUNNEL_SIMPLE,
                                "Compare shared columns only",
                            ),
                        )
                        .on_hover_text(
                            "Adds the columns only one side returned to the ignore list, so the \
                             diff is over the columns both machines have.",
                        )
                        .clicked()
                        {
                            ignore_them = true;
                        }
                    });
                    if ignore_them {
                        let mut set: BTreeSet<String> = view.ignored.iter().cloned().collect();
                        set.extend(view.only_a_cols.iter().cloned());
                        set.extend(view.only_b_cols.iter().cloned());
                        self.compare.ignore_override = Some(set);
                        self.compare.stale = true;
                    }
                }

                if let Some(note) = &view.key_note {
                    ui.add(
                        Label::new(
                            RichText::new(note.as_str())
                                .text_style(TextStyle::Small)
                                .color(WARN),
                        )
                        .wrap(),
                    );
                }

                self.ui_compare_legend(ui);
            });
    }

    /// The legend, and the two controls that decide what the diff means.
    fn ui_compare_legend(&mut self, ui: &mut Ui) {
        let Some(view) = self.compare.view.as_ref() else {
            return;
        };
        let counts = view.counts;
        let keys = view.keys.clone();
        let key_source = view.key_source;
        let ignored = view.ignored.clone();
        let columns = view.columns.clone();

        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = S3;
            for sign in Sign::ALL {
                dot_chip(
                    ui,
                    sign.color(),
                    &format!("{} {}", counts[sign.rank()], sign.label()),
                );
            }
        });

        ui.add_space(S2);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = S3;

            // Keys.
            let key_text = if keys.is_empty() {
                "whole row".to_string()
            } else {
                keys.join(", ")
            };
            let mut new_keys: Option<Option<Vec<String>>> = None;
            ComboBox::from_id_salt("vs_cmp_keys")
                .selected_text(
                    RichText::new(format!("key: {key_text}  ({})", key_source.note()))
                        .text_style(TextStyle::Small),
                )
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(false, "Reset to the derived key")
                        .clicked()
                    {
                        new_keys = Some(None);
                    }
                    for column in &columns {
                        let on = keys.contains(column);
                        if ui.selectable_label(on, column.as_str()).clicked() {
                            let mut next = keys.clone();
                            if on {
                                next.retain(|c| c != column);
                            } else {
                                next.push(column.clone());
                            }
                            new_keys = Some(Some(next));
                        }
                    }
                });

            // Ignores.
            let mut new_ignores: Option<Option<BTreeSet<String>>> = None;
            ComboBox::from_id_salt("vs_cmp_ignores")
                .selected_text(
                    RichText::new(format!("ignoring: {}", ignored.len()))
                        .text_style(TextStyle::Small),
                )
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(false, "Reset to the volatile defaults")
                        .clicked()
                    {
                        new_ignores = Some(None);
                    }
                    for column in &columns {
                        let on = ignored.contains(column);
                        if ui.selectable_label(on, column.as_str()).clicked() {
                            let mut next: BTreeSet<String> = ignored.iter().cloned().collect();
                            if on {
                                next.remove(column);
                            } else {
                                next.insert(column.clone());
                            }
                            new_ignores = Some(Some(next));
                        }
                    }
                });
            if !ignored.is_empty() {
                ui.label(
                    RichText::new(ignored.join(", "))
                        .text_style(TextStyle::Small)
                        .color(muted(38)),
                )
                .on_hover_text(
                    "These columns are disregarded when deciding whether a matched row changed. \
                     They are still shown.",
                );
            }

            if let Some(keys) = new_keys {
                self.compare.key_override = keys;
                self.compare.stale = true;
            }
            if let Some(ignores) = new_ignores {
                self.compare.ignore_override = ignores;
                self.compare.stale = true;
            }
        });
    }

    fn ui_compare_table(&mut self, ui: &mut Ui) {
        Frame::NONE
            .inner_margin(Margin::symmetric(S4 as i8, S2 as i8))
            .show(ui, |ui| {
                let Some(view) = self.compare.view.as_ref() else {
                    if !self.compare.is_running() && !self.has_compare_blocker() {
                        empty_state(
                            ui,
                            icons::GIT_DIFF,
                            "No comparison yet",
                            "Pick two targets and a class, then press Run. The same target on \
                             both sides is a legitimate check: it should come back identical \
                             except for the columns that move on their own.",
                        );
                    }
                    return;
                };
                if view.lines.is_empty() {
                    empty_state(
                        ui,
                        icons::LIST_BULLETS,
                        "Nothing to compare",
                        "The query returned no rows on either side.",
                    );
                    return;
                }

                let lines = &view.lines;
                let mut table = self.compare.table;
                DataTable::new("compare-diff")
                    .column(
                        // Tinted, so it must be unclipped for the same reason the
                        // value columns are.
                        TableColumn::exact("Diff", SIGN_W).clip(false),
                    )
                    .columns(view.columns.iter().map(|name| {
                        TableColumn::initial(name.as_str(), COL_W)
                            .at_least(COL_MIN)
                            // THE trap: a clipped column's clip rect is exactly
                            // its `max_rect`, and `cell_background` paints half
                            // an item-spacing outside that. Clipped, every tint
                            // in this table would be discarded rather than
                            // trimmed -- the cells would look identical to
                            // unchanged ones.
                            .clip(false)
                    }))
                    .sort_key(|row, col| match col {
                        0 => lines[row].sign.rank().to_string(),
                        c => lines[row]
                            .cells
                            .get(c - 1)
                            .map(|cell| cell.text.clone())
                            .unwrap_or_default(),
                    })
                    .show(ui, &mut table, lines.len(), |row| {
                        let line = &lines[row.data_index()];
                        let sign = line.sign;
                        diff_cell(
                            row,
                            sign.mark(),
                            sign.color(),
                            sign.color().gamma_multiply(CELL_TINT),
                        );
                        for cell in &line.cells {
                            // A one-sided row is tinted across its whole width
                            // (the row itself is the finding); a changed row
                            // tints only the columns that moved.
                            let tint = match (sign, cell.changed) {
                                (Sign::Same, _) => None,
                                (Sign::Changed, false) => None,
                                (Sign::Changed, true) => Some(WARN.gamma_multiply(CELL_TINT)),
                                (other, _) => Some(other.color().gamma_multiply(ROW_TINT)),
                            };
                            let fg = if cell.changed { WARN } else { muted(85) };
                            match tint {
                                Some(tint) => diff_cell(row, &cell.text, fg, tint),
                                None => {
                                    row.path(RichText::new(cell.text.as_str()).color(muted(85)));
                                }
                            }
                        }
                    });
                self.compare.table = table;
            });
    }

    /// Is there a stated reason the table is absent? (A failure, a refusal or a
    /// partial side each print their own card; the empty state must not print a
    /// second, contradictory explanation underneath.)
    fn has_compare_blocker(&self) -> bool {
        [Which::A, Which::B].into_iter().any(|w| {
            let side = self.compare.side(w);
            matches!(side.state, SideState::Failed { .. } | SideState::Refused(_))
                || side.partial_note().is_some()
        })
    }
}

/// Namespace field width. Wide enough for `root\StandardCimv2` in the mono face.
const NS_W: f32 = 170.0;

/// One tinted, truncating cell.
///
/// Truncation is explicit because the column is unclipped: `egui_extras` forces
/// `TextWrapMode::Truncate` only on clipped columns, so without this a long
/// `PathName` would run straight over its neighbour.
fn diff_cell(
    row: &mut crate::widgets::table::RowCtx<'_, '_, '_>,
    text: &str,
    fg: Color32,
    tint: Color32,
) {
    let text = text.to_owned();
    let full = text.clone();
    row.cell(move |ui| {
        cell_background(ui, tint);
        ui.add(Label::new(RichText::new(text).color(fg)).truncate());
    })
    .on_hover_text(full);
}

/// A titled, tinted note above the table.
///
/// In a **debug build** this frame is one of the places egui paints its orange
/// `Unaligned` rule: `Ui::register_rect` flags any `Ui` whose edges are off the
/// 1/32-point grid, and the density scale is fractional by design (5.6, 8.4,
/// 11.2), so a cursor a few `add_space` calls deep is always off it. It shows up
/// here rather than in the other views only because nothing is drawn over it --
/// when the diff is withheld this banner is the last thing on screen. Snapping
/// the frame's top onto the grid does not remove it (the height comes from font
/// metrics and is fractional too), and snapping the height as well merely adds a
/// second flagged `Ui`; both were measured. `cfg!(debug_assertions)` gates the
/// whole overlay, so nothing of it reaches a release build.
fn banner(ui: &mut Ui, color: Color32, icon: &str, title: &str, body: &str) {
    ui.add_space(S2);
    Frame::NONE
        .fill(color.gamma_multiply(0.10))
        .stroke(egui::Stroke::new(HAIRLINE, color.gamma_multiply(0.45)))
        .corner_radius(R_MD)
        .inner_margin(Margin::symmetric(S3 as i8, S2 as i8))
        .show(ui, |ui| {
            // Full width whatever the text length. A `Frame` sizes to its
            // content, so a short message would otherwise produce a small
            // floating box in the middle of an otherwise empty pane -- which
            // reads as a tooltip that failed to close rather than as the
            // explanation for why the table is missing.
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = S2;
                ui.label(icons::labelled_styled(
                    ui,
                    icon,
                    title,
                    TextStyle::Body,
                    color,
                ));
                ui.label(
                    RichText::new(body)
                        .text_style(TextStyle::Small)
                        .color(muted(70)),
                );
            });
        });
    ui.add_space(S2);
}

/// A comma list, or an em dash when there is nothing in it.
fn list_or_none(items: &[String]) -> String {
    if items.is_empty() {
        "\u{2014}".to_string()
    } else {
        items.join(", ")
    }
}

/// The first line of a multi-line error, for a one-line status.
fn first_line(message: &str) -> &str {
    message.lines().next().unwrap_or(message)
}

/// The shared empty state: a large muted glyph, a heading, and one line saying
/// what would fill it.
fn empty_state(ui: &mut Ui, icon: &str, title: &str, note: &str) {
    ui.add_space(S6);
    ui.vertical_centered(|ui| {
        ui.label(icons::glyph(icon).size(28.0).color(muted(20)));
        ui.add_space(S2);
        ui.label(RichText::new(title).color(muted(55)));
        ui.add(
            Label::new(
                RichText::new(note)
                    .text_style(TextStyle::Small)
                    .color(muted(38)),
            )
            .wrap(),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use vmiscope_core::Completion;

    fn table(columns: &[&str], rows: &[&[&str]]) -> QueryResult {
        QueryResult {
            columns: columns.iter().map(|s| s.to_string()).collect(),
            rows: rows
                .iter()
                .map(|r| r.iter().map(|s| s.to_string()).collect())
                .collect(),
            ..Default::default()
        }
    }

    fn keyed(columns: &[&str], rows: &[&[&str]], keys: &[&str]) -> QueryResult {
        QueryResult {
            key_columns: keys.iter().map(|s| s.to_string()).collect(),
            ..table(columns, rows)
        }
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    /// Build the view the way the app does: derived key, derived ignores.
    fn view_of(a: &QueryResult, b: &QueryResult) -> DiffView {
        let (keys, source, note) = resolve_keys(a, b, None);
        let columns = display_columns(a, b, &keys);
        let ignored = resolve_ignores(&columns, &keys, None);
        build_view(a, b, keys, source, note, ignored)
    }

    fn signs(view: &DiffView) -> Vec<Sign> {
        view.lines.iter().map(|l| l.sign).collect()
    }

    fn cell(view: &DiffView, line: usize, column: &str) -> String {
        let col = view
            .columns
            .iter()
            .position(|c| c == column)
            .expect("column");
        view.lines[line].cells[col].text.clone()
    }

    // -- pickers -----------------------------------------------------------

    /// The pickers are built from the saved targets, and the one thing they must
    /// get right is which of those Compare can actually reach: an
    /// alternate-credential target has no persisted password, so opening a
    /// worker for it could only ever produce a connection that fails.
    #[test]
    fn the_pickers_offer_this_machine_first_and_lock_alt_cred_targets() {
        // Built by pushing rather than with struct-update syntax: `Config` has a
        // private field (its write clock), so `..Default::default()` is not
        // legal from outside that module.
        let mut config = Config::default();
        config.targets.push(crate::config::Target {
            name: "SRV1".into(),
            cred_ref: CredRef::CurrentUser,
            ..Default::default()
        });
        config.targets.push(crate::config::Target {
            name: "SRV2".into(),
            cred_ref: CredRef::Alt {
                user: "admin".into(),
                domain: Some("CORP".into()),
            },
            ..Default::default()
        });
        let list = candidates(&config);
        assert_eq!(list.len(), 3);

        assert_eq!(list[0].host, HostRef::Local);
        assert!(list[0].reachable);

        assert_eq!(
            list[1].host,
            HostRef::Sso {
                host: "SRV1".into()
            }
        );
        assert!(list[1].reachable);

        // The identity has to be the same one `HostRef::of` would build from the
        // real credential, or Compare would open a *second* worker for a machine
        // the app is already talking to.
        assert_eq!(
            list[2].host,
            HostRef::Alt {
                host: "SRV2".into(),
                user: r"CORP\admin".into()
            }
        );
        assert!(!list[2].reachable);
    }

    // -- keys --------------------------------------------------------------

    /// Task 6.8's acceptance: `Win32_Service` keys on `Name`, not on whole-row
    /// equality.
    #[test]
    fn a_declared_class_key_is_used() {
        let a = keyed(&["Name", "State"], &[&["spooler", "Running"]], &["Name"]);
        let (keys, source, note) = resolve_keys(&a, &a, None);
        assert_eq!(keys, strings(&["Name"]));
        assert_eq!(source, KeySource::ClassKey);
        assert!(note.is_none());
    }

    /// The failure this guards: a projection that drops the key column still
    /// reports one. Keying on a column the result does not contain gives every
    /// row the same (empty) key, which pairs rows by arrival order and reports
    /// nonsense with total confidence.
    #[test]
    fn a_key_the_query_did_not_return_falls_back_to_relpath() {
        let a = keyed(
            &["State", "__RELPATH"],
            &[&["Running", r#"Win32_Service.Name="spooler""#]],
            &["Name"],
        );
        let (keys, source, note) = resolve_keys(&a, &a, None);
        assert_eq!(keys, strings(&["__RELPATH"]));
        assert_eq!(source, KeySource::RelPath);
        assert!(note.expect("the fall-back is explained").contains("Name"));
    }

    #[test]
    fn no_key_and_no_relpath_is_whole_row_and_says_so() {
        let a = table(&["State"], &[&["Running"]]);
        let (keys, source, note) = resolve_keys(&a, &a, None);
        assert!(keys.is_empty());
        assert_eq!(source, KeySource::WholeRow);
        assert!(note.is_some());
    }

    #[test]
    fn a_manual_key_overrides_the_derived_one() {
        let a = keyed(&["Name", "State"], &[&["spooler", "Running"]], &["Name"]);
        let (keys, source, _) = resolve_keys(&a, &a, Some(&strings(&["State"])));
        assert_eq!(keys, strings(&["State"]));
        assert_eq!(source, KeySource::Manual);
    }

    /// Half a compound key is not a key: it merges rows the schema says are
    /// distinct, which is a wrong answer rather than a partial one.
    #[test]
    fn a_partly_returned_compound_key_is_not_used() {
        let a = keyed(
            &["Antecedent", "__RELPATH"],
            &[&["x", "y"]],
            &["Antecedent", "Dependent"],
        );
        let (keys, source, _) = resolve_keys(&a, &a, None);
        assert_eq!(keys, strings(&["__RELPATH"]));
        assert_eq!(source, KeySource::RelPath);
    }

    // -- ignores -----------------------------------------------------------

    #[test]
    fn volatile_columns_are_recognised() {
        for name in [
            "__PATH",
            "ProcessId",
            "ParentProcessId",
            "IDProcess",
            "Handle",
            "HandleCount",
            "ThreadCount",
            "InstallDate",
            "LastBootUpTime",
            "KernelModeTime",
            "Timestamp_Sys100NS",
            "PageFileUsage",
            "WorkingSetSize",
            "PrivateBytes",
            "PercentProcessorTime",
            "FreeSpace",
            "AvailableBytes",
            "ElapsedTime",
            "SystemUpTime",
            "VirtualSize",
            "PeakVirtualSize",
        ] {
            assert!(is_volatile_column(name), "{name} should be volatile");
        }
    }

    /// The other half, and the more important one: a default that hides a real
    /// difference between two machines is worse than one that leaves noise on
    /// screen.
    #[test]
    fn stable_columns_are_left_alone() {
        for name in [
            "Name",
            "State",
            "Status",
            "StartMode",
            "StartName",
            "PathName",
            "Description",
            "Caption",
            "ServiceType",
            "ErrorControl",
            "__RELPATH",
            "__CLASS",
            "Version",
            "BytesPerSector",
            "Size",
            "TotalPhysicalMemory",
        ] {
            assert!(!is_volatile_column(name), "{name} should not be volatile");
        }
    }

    #[test]
    fn the_ignore_list_can_be_replaced_wholesale() {
        let columns = strings(&["Name", "ProcessId", "State"]);
        assert_eq!(
            resolve_ignores(&columns, &[], None),
            strings(&["ProcessId"])
        );
        let mine: BTreeSet<String> = ["State".to_string()].into_iter().collect();
        assert_eq!(
            resolve_ignores(&columns, &[], Some(&mine)),
            strings(&["State"])
        );
    }

    /// `Win32_Process.Handle` is the PID and the class key at once. As a value
    /// it churns; as a key it is the row's identity, and `diff_tables` builds
    /// its whole-row fallback key out of the *non-ignored* columns -- so an
    /// ignored key column can stop being part of the identity altogether.
    #[test]
    fn a_key_column_is_never_ignored_by_default() {
        let columns = strings(&["Handle", "Name", "ThreadCount"]);
        assert_eq!(
            resolve_ignores(&columns, &strings(&["Handle"]), None),
            strings(&["ThreadCount"])
        );
        // Without the key it is volatile like any other PID column.
        assert_eq!(
            resolve_ignores(&columns, &[], None),
            strings(&["Handle", "ThreadCount"])
        );
    }

    // -- the prepared view -------------------------------------------------

    /// The four states, in one table, keyed on `Name`.
    #[test]
    fn every_sign_is_produced_and_counted() {
        let a = keyed(
            &["Name", "State"],
            &[
                &["a", "Running"],
                &["b", "Running"],
                &["c", "Stopped"], // only on A
            ],
            &["Name"],
        );
        let b = keyed(
            &["Name", "State"],
            &[
                &["a", "Running"], // identical
                &["b", "Stopped"], // changed
                &["d", "Running"], // only on B
            ],
            &["Name"],
        );
        let view = view_of(&a, &b);

        assert_eq!(
            signs(&view),
            vec![Sign::Same, Sign::Changed, Sign::OnlyB, Sign::OnlyA]
        );
        assert_eq!(view.counts[Sign::Same.rank()], 1);
        assert_eq!(view.counts[Sign::Changed.rank()], 1);
        assert_eq!(view.counts[Sign::OnlyB.rank()], 1);
        assert_eq!(view.counts[Sign::OnlyA.rank()], 1);
        assert!(view.sound);

        // The changed cell carries both sides; its neighbours do not.
        assert_eq!(cell(&view, 1, "State"), "Running \u{2192} Stopped");
        assert_eq!(cell(&view, 1, "Name"), "b");
        let state_col = view.columns.iter().position(|c| c == "State").unwrap();
        assert!(view.lines[1].cells[state_col].changed);
        assert!(!view.lines[0].cells[state_col].changed);
        // The A-only row shows A's values, not blanks.
        assert_eq!(cell(&view, 3, "State"), "Stopped");
    }

    /// The whole point of the ignore list: the same machine read twice differs
    /// only in the columns that move on their own, and must read as identical.
    #[test]
    fn a_volatile_column_alone_does_not_make_a_row_changed() {
        let a = keyed(
            &["Name", "State", "ProcessId"],
            &[&["spooler", "Running", "1234"]],
            &["Name"],
        );
        let b = keyed(
            &["Name", "State", "ProcessId"],
            &[&["spooler", "Running", "9999"]],
            &["Name"],
        );
        let view = view_of(&a, &b);
        assert_eq!(signs(&view), vec![Sign::Same]);
        assert_eq!(view.ignored, strings(&["ProcessId"]));
        // Ignored, not hidden: the column is still on screen, showing B's value.
        assert_eq!(cell(&view, 0, "ProcessId"), "9999");
    }

    /// Identical rows are recovered from B's order, so duplicate keys -- which
    /// the differ pairs positionally -- still line up with the right verdict.
    #[test]
    fn duplicate_keys_keep_their_place_in_the_table() {
        let a = keyed(
            &["Name", "V"],
            &[&["dup", "1"], &["dup", "2"], &["dup", "3"]],
            &["Name"],
        );
        let b = keyed(&["Name", "V"], &[&["dup", "9"], &["dup", "2"]], &["Name"]);
        let view = view_of(&a, &b);
        assert_eq!(signs(&view), vec![Sign::Changed, Sign::Same, Sign::OnlyA]);
        assert_eq!(cell(&view, 0, "V"), "1 \u{2192} 9");
        assert_eq!(cell(&view, 1, "V"), "2");
        assert_eq!(cell(&view, 2, "V"), "3");
    }

    /// Differing column sets: the differ aligns by name, the view reports the
    /// asymmetry, and the missing side renders as an em dash rather than as an
    /// empty string that would read as a real blank value.
    #[test]
    fn a_column_only_one_side_returned_is_reported_and_dashed() {
        let a = keyed(&["Name", "State"], &[&["w32time", "Running"]], &["Name"]);
        let b = keyed(
            &["Name", "State", "Description"],
            &[&["w32time", "Running", "Windows Time"]],
            &["Name"],
        );
        let view = view_of(&a, &b);
        assert_eq!(view.only_b_cols, strings(&["Description"]));
        assert!(view.only_a_cols.is_empty());
        assert_eq!(signs(&view), vec![Sign::Changed]);
        assert_eq!(
            cell(&view, 0, "Description"),
            "\u{2014} \u{2192} Windows Time"
        );
    }

    /// A truncated side taints the view it produces, and the taint has to reach
    /// the exported file -- that is the only copy of the diff that outlives the
    /// banner on screen.
    #[test]
    fn a_partial_side_marks_the_view_and_the_export_unsound() {
        let a = keyed(&["Name"], &[&["a"]], &["Name"]);
        let mut b = keyed(&["Name"], &[&["a"]], &["Name"]);
        b.completion = Completion::Truncated { cap: 1 };
        let view = view_of(&a, &b);
        assert!(!view.sound);

        let doc = DiffExport {
            a: "this machine".into(),
            b: "this machine".into(),
            namespace: "root\\CIMV2",
            wql: "SELECT * FROM Win32_Service",
            key_columns: &view.keys,
            key_source: view.key_source.note(),
            ignored_columns: &view.ignored,
            sound: view.sound,
            a_completion: None,
            b_completion: b.completion.note(),
            identical: view.counts[Sign::Same.rank()],
            changed: view.counts[Sign::Changed.rank()],
            only_on_a: view.counts[Sign::OnlyA.rank()],
            only_on_b: view.counts[Sign::OnlyB.rank()],
            diff: &view.diff,
        };
        let json = serde_json::to_string(&doc).expect("the export serializes");
        let back: serde_json::Value = serde_json::from_str(&json).expect("it parses back");
        assert_eq!(back["sound"], serde_json::Value::Bool(false));
        assert!(back["b_completion"].as_str().unwrap().contains("truncated"));
    }

    // -- export ------------------------------------------------------------

    /// Task 6.6's acceptance: the export round-trips -- every field the file
    /// needs to be readable later comes back out of it.
    #[test]
    fn the_json_export_round_trips() {
        let a = keyed(
            &["Name", "State"],
            &[&["a", "Running"], &["c", "Stopped"]],
            &["Name"],
        );
        let b = keyed(
            &["Name", "State"],
            &[&["a", "Stopped"], &["d", "Running"]],
            &["Name"],
        );
        let view = view_of(&a, &b);
        let doc = DiffExport {
            a: r"\\SRV1".into(),
            b: r"\\SRV2".into(),
            namespace: "root\\CIMV2",
            wql: "SELECT * FROM Win32_Service",
            key_columns: &view.keys,
            key_source: view.key_source.note(),
            ignored_columns: &view.ignored,
            sound: view.sound,
            a_completion: None,
            b_completion: None,
            identical: view.counts[Sign::Same.rank()],
            changed: view.counts[Sign::Changed.rank()],
            only_on_a: view.counts[Sign::OnlyA.rank()],
            only_on_b: view.counts[Sign::OnlyB.rank()],
            diff: &view.diff,
        };
        let json = serde_json::to_string_pretty(&doc).expect("serializes");
        let back: serde_json::Value = serde_json::from_str(&json).expect("parses back");

        assert_eq!(back["a"], r"\\SRV1");
        assert_eq!(back["b"], r"\\SRV2");
        assert_eq!(back["key_columns"][0], "Name");
        assert_eq!(back["key_source"], "class key");
        assert_eq!(back["sound"], serde_json::Value::Bool(true));
        assert_eq!(back["changed"], 1);
        assert_eq!(back["only_on_a"], 1);
        assert_eq!(back["only_on_b"], 1);
        assert_eq!(back["diff"]["changed"][0]["key"][0], "a");
        assert_eq!(back["diff"]["changed"][0]["changed_columns"][0], "State");
        assert_eq!(back["diff"]["changed"][0]["a"]["State"], "Running");
        assert_eq!(back["diff"]["changed"][0]["b"]["State"], "Stopped");
        assert_eq!(back["diff"]["added"][0]["values"]["Name"], "d");
        assert_eq!(back["diff"]["removed"][0]["values"]["Name"], "c");
    }

    /// The CSV is one line per side-row, so a changed row is two lines and
    /// every cell in the file holds one value.
    #[test]
    fn the_csv_writes_both_sides_of_a_changed_row() {
        let a = keyed(
            &["Name", "State"],
            &[&["a", "Running"], &["b", "Running"]],
            &["Name"],
        );
        let b = keyed(
            &["Name", "State"],
            &[&["a", "Running"], &["b", "Stopped"]],
            &["Name"],
        );
        let csv = diff_to_csv(&view_of(&a, &b));
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines[0], "Diff,Side,Name,State");
        assert_eq!(lines[1], "!=,A,b,Running");
        assert_eq!(lines[2], "!=,B,b,Stopped");
        assert_eq!(lines[3], "=,B,a,Running");
        assert_eq!(lines.len(), 4);
    }

    // -- column order ------------------------------------------------------

    #[test]
    fn the_key_leads_the_columns_and_b_only_columns_trail() {
        let a = table(&["State", "Name"], &[]);
        let b = table(&["State", "Name", "Extra"], &[]);
        assert_eq!(
            display_columns(&a, &b, &strings(&["Name"])),
            strings(&["Name", "State", "Extra"])
        );
    }
}
