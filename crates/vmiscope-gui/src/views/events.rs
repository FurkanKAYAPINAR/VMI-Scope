//! The Events view: one live WMI notification subscription, and what it delivers.
//!
//! The design's layout: a 300px subscription column -- query, polling interval,
//! delivery mode, Start/Stop, and a small stats card -- beside a live stream of
//! what has arrived. The stream is virtualised and capped; see [`EventLog`].
//!
//! # What this view is allowed to claim
//!
//! The mock's stats card carries four tiles. Three of them survived contact with
//! what a polling subscription can actually observe, and one did not:
//!
//! * **"Sink queue" is not observable.** There is no way to read a provider's
//!   pending count from `IWbemServices::ExecNotificationQuery` -- the sink is on
//!   WMI's side of the boundary and nothing comes back describing it. The tile
//!   is therefore the depth of the queue we *do* own: the `mpsc` channel between
//!   `EventMonitor`'s thread and this frame, sampled at the last drain. It is
//!   labelled "Queued" so it cannot be read as WMI's number, and it carries the
//!   peak alongside the instantaneous value, because a backlog that has already
//!   drained is exactly the thing an instantaneous zero would hide.
//! * **Delivery rate is real**, and is computed here from arrival times over a
//!   30-second sliding window. It is never a constant.
//! * **Permanent delivery ships disabled**, with the reason in its tooltip. See
//!   [`PERMANENT_WHY`].
//!
//! # Two things the plan asked for that the data does not carry
//!
//! `docs/REDESIGN.md` 4.11 has the event kind parsed from the event's `__CLASS`.
//! **It cannot be.** `wmi::IWbemClassWrapper::list_properties` calls `GetNames`
//! with `WBEM_FLAG_NONSYSTEM_ONLY`, so every `__`-prefixed system property is
//! filtered out before `monitor::flatten_event` ever sees it -- and
//! `MonitorMsg::Event`, a flat `Vec<(String, String)>`, is the only thing this
//! view receives. The *target* class is missing for a second reason on top of
//! that: `flatten_event` drills one level into `TargetInstance` and copies six
//! named scalars, none of which is a class name.
//!
//! Both are therefore derived from the subscription's own query and stamped onto
//! each row as it arrives -- a fact we hold rather than one we invent. Where the
//! query alone is ambiguous (a subscription to `__InstanceOperationEvent`
//! delivers all three kinds), the one per-event discriminator that *does* survive
//! the flattening is used: only a modification carries `PreviousInstance`.
//!
//! # Time
//!
//! Stamps are UTC and say so. The app has no local-time conversion anywhere --
//! `std::time` offers only the epoch, and nothing here links a timezone API --
//! so rendering "09:47:31" from a UTC clock would be off by the local offset and
//! silently wrong for the one job an event log has, which is lining up against
//! another log.

use std::collections::VecDeque;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use eframe::egui::{
    self, Align, Button, Color32, CornerRadius, Frame, Label, Layout, Margin, Pos2, Rect, RichText,
    Sense, Stroke, TextEdit, TextStyle, UiBuilder, Vec2,
};

use vmiscope_core::{EventMonitor, MonitorMsg};

use crate::app::VmiScopeApp;
use crate::theme::icons;
use crate::theme::tokens::{
    a300, muted, BAD, BG, DIVIDER, NEUTRAL, OK, R_MD, R_SM, S1, S2, S3, S4, SURFACE, TEXT, WARN,
};
use crate::util::save_file;
use crate::widgets::button::{accent, btn_primary, btn_secondary, focus_ring};
use crate::widgets::card::card;
use crate::widgets::field::{filter_box, mono_input};
use crate::widgets::kv::kv_grid_sized;
use crate::widgets::rule::{solid_hline, solid_vline, HAIRLINE};

// ---------------------------------------------------------------------------
// Metrics
// ---------------------------------------------------------------------------

/// Subscription column width. Exact, per task 4.8.
const CONFIG_W: f32 = 300.0;
/// The raw-event reveal's width. The same figure as the Query view's row detail
/// and for the same reason: WMI property names are long.
const DETAIL_W: f32 = 320.0;
/// The query textarea's opening height, in rows. The mock's 104px textarea at a
/// 1.7 line height is a shade under five lines of the monospace face.
const EDITOR_ROWS: usize = 5;
/// The interval field only ever holds a small integer.
const INTERVAL_W: f32 = 56.0;
/// Stream filter box width. Same figure as the Network and Process toolbars, so
/// the three line up.
const FILTER_W: f32 = 200.0;
/// What "Save log" measures, so the header's elastic note knows what to leave
/// for it. Measured off the rendered button rather than guessed low: an
/// under-reservation puts the count back under the filter box.
const SAVE_W: f32 = 96.0;

/// One stream row. Fixed, because [`egui::ScrollArea::show_rows`] virtualises on
/// a constant row height -- and a variable height here would mean laying out
/// every one of the retained rows to find the scroll extent.
const ROW_H: f32 = 26.0;
/// The kind column. Wide enough for "Modification".
const KIND_W: f32 = 74.0;
/// The class column, per the mock.
const CLASS_W: f32 = 210.0;
/// The trailing open-detail affordance.
const ICON_W: f32 = 18.0;
const ICON_SIZE: f32 = 12.0;

/// Stream text sizes: the stamp reads quieter than the payload.
const STAMP_SIZE: f32 = 11.5;
const KIND_SIZE: f32 = 10.5;
const BODY_SIZE: f32 = 12.0;
/// A 13.5px medium-weight column heading -- the mock's `--font-heading` at the
/// size it uses inside a panel, which no `TextStyle` covers (`Heading` is 19).
const HEADING_SIZE: f32 = 13.5;
/// The stats card's rows.
const STAT_SIZE: f32 = 11.5;
/// A field's label, per the mock's `.field > label`.
const LABEL_SIZE: f32 = 12.0;

/// Row rule strength, as a percentage of the body colour. The mock's 7%.
const ROW_RULE: u8 = 7;
/// Row hover strength. Matches the table kit, so a stream row and a table row
/// respond identically.
const HOVER_TINT: f32 = 0.04;

// ---------------------------------------------------------------------------
// Motion
// ---------------------------------------------------------------------------

/// How long a new row's accent flash lasts.
///
/// Driven from a per-row `created_at` and an eased age, **not** from
/// `Ui::animate_bool`: that returns its target value on the first frame it sees
/// a given `Id`, so a freshly created row would start at "already faded" and
/// never animate at all. Nothing in egui's animation API can express "this
/// element was born just now".
const FLASH_SECS: f64 = 0.18;
/// Peak strength of the flash tint. The mock's `accent 18%`.
const FLASH_TINT: f32 = 0.18;

/// The live dot's pulse period, per the mock's `noct-pulse 2.4s`.
///
/// Also driven by `input().time` rather than `animate_bool`, for a different
/// reason: `animate_bool` eases once between two states and stops. It cannot
/// loop, so there is no way to build a heartbeat out of it.
const PULSE_SECS: f64 = 2.4;
/// The dot at rest, and how far the pulse takes its opacity and radius.
const DOT_R: f32 = 3.5;
const PULSE_MIN_ALPHA: f32 = 0.3;
const PULSE_MIN_SCALE: f32 = 0.7;
/// The glow under the dot -- the mock's `box-shadow: 0 0 8px`, which egui has no
/// shadow primitive for on a circle. A second, wider, much fainter disc is the
/// closest honest approximation.
const GLOW_SCALE: f32 = 2.6;
const GLOW_ALPHA: f32 = 0.18;

/// Repaint cadence while the monitor runs.
///
/// The app asks for 250 ms so events keep draining on any view; that is far too
/// coarse for a 2.4 s pulse (10 steps) and useless for a 0.18 s flash (one
/// frame). This view therefore asks for its own while it is the one on screen.
const LIVE_FRAME: Duration = Duration::from_millis(33);

// ---------------------------------------------------------------------------
// Measurement
// ---------------------------------------------------------------------------

/// The delivery rate's sliding window.
const RATE_WINDOW: f64 = 30.0;
/// Below this much observation the rate is an em dash rather than a number: one
/// event 200 ms after Start is not "5 events per second".
const RATE_FLOOR: f64 = 1.0;

/// How many events the log retains.
///
/// The same figure as the Process view's row cap, and roughly 25 seconds at the
/// 200 ev/s this view is built to survive. Eviction is counted and shown, never
/// silent.
const LOG_CAP: usize = 5_000;

/// Fields a one-line summary will show before it stops.
const SUMMARY_FIELDS: usize = 3;

/// Properties that describe the *envelope* rather than the event, and would
/// crowd out the payload in a one-line summary. Both are still in the row's
/// detail panel.
const ENVELOPE: [&str; 2] = ["TIME_CREATED", "SECURITY_DESCRIPTOR"];

/// Fields worth leading a summary with, in this order. `flatten_event` sorts its
/// pairs alphabetically, so without a preference the summary would open with
/// `Caption` and `CommandLine` every time and push the name off the end.
const SUMMARY_FIRST: [&str; 4] = ["Name", "ProcessId", "Handle", "Caption"];

/// The largest polling interval the field will inject. Six hours; past that the
/// number is more likely a typo than an intention.
const MAX_WITHIN: u32 = 21_600;

/// Why the Permanent segment is inert.
const PERMANENT_WHY: &str = "Permanent delivery is deliberately disabled.\n\n\
     A temporary subscription lives only as long as this app is running. A \
     permanent one is written into the WMI repository as an __EventFilter, a \
     consumer and a __FilterToConsumerBinding, and survives reboots \u{2014} which \
     is exactly the artifact the Persistence view exists to find. Creating one \
     from here would plant evidence in the audit trail you opened this tool to \
     read.\n\n\
     Enabling it needs typed confirmation, an audit-log line and a visible \
     teardown control. None of those is built, so the honest state is off.";

/// Why "Queued" is not the mock's "Sink queue".
const QUEUED_WHY: &str = "Depth of this app's own event channel, sampled at the \
     last drain, with the highest depth seen since Start.\n\n\
     This is NOT WMI's sink queue: a notification query gives no way to read the \
     provider's pending count, so that number cannot be shown at all.";

/// What the class column is, and why it is not per-event.
const CLASS_WHY: &str = "The class this subscription watches, stamped on the row \
     when it arrived.\n\n\
     It is not read from the event: WMI's GetNames hides every __ system \
     property, so the delivered object carries no __CLASS, and the embedded \
     TargetInstance arrives as named scalars with no class name among them.";

/// What the stamp measures.
const STAMP_WHY: &str = "Arrival time at this app, in UTC. Not the time WMI \
     recorded on the event, and not local time \u{2014} the app has no timezone \
     conversion, and a UTC clock drawn as if it were local would be silently \
     wrong.";

// ---------------------------------------------------------------------------
// The subscription's shape, read out of its query
// ---------------------------------------------------------------------------

/// One lexical token of a WQL query, as a byte range into it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Tok {
    start: usize,
    end: usize,
    /// True for the *contents* of a quoted literal; the quotes are excluded.
    quoted: bool,
}

/// Scan `wql` into word and string tokens.
///
/// Everything the view derives from a query -- the event class, the watched
/// class, the polling interval, and where to write a new one -- needs to know
/// what is inside a string literal and what is not. `WHERE Name = 'within 5'`
/// is not a polling interval, and a substring search would say it was.
///
/// A doubled delimiter inside a literal is an escaped quote, which is WQL's
/// rule. An unterminated literal runs to the end of the text: that is a query
/// being typed, and guessing a terminator for it would move the tokens around
/// under the caret.
fn tokens(wql: &str) -> Vec<Tok> {
    let mut out: Vec<Tok> = Vec::new();
    let mut word: Option<usize> = None;
    let mut quote: Option<(char, usize)> = None;
    let mut chars = wql.char_indices().peekable();

    while let Some((i, c)) = chars.next() {
        if let Some((delim, start)) = quote {
            if c == delim {
                if chars.peek().map(|&(_, next)| next) == Some(delim) {
                    chars.next();
                    continue;
                }
                out.push(Tok {
                    start,
                    end: i,
                    quoted: true,
                });
                quote = None;
            }
            continue;
        }
        if c == '\'' || c == '"' {
            if let Some(start) = word.take() {
                out.push(Tok {
                    start,
                    end: i,
                    quoted: false,
                });
            }
            quote = Some((c, i + c.len_utf8()));
            continue;
        }
        if c.is_ascii_alphanumeric() || c == '_' {
            if word.is_none() {
                word = Some(i);
            }
        } else if let Some(start) = word.take() {
            out.push(Tok {
                start,
                end: i,
                quoted: false,
            });
        }
    }

    if let Some(start) = word {
        out.push(Tok {
            start,
            end: wql.len(),
            quoted: false,
        });
    }
    if let Some((_, start)) = quote {
        out.push(Tok {
            start,
            end: wql.len(),
            quoted: true,
        });
    }
    out
}

fn tok_text<'a>(wql: &'a str, tok: &Tok) -> &'a str {
    &wql[tok.start..tok.end]
}

/// Index of the unquoted token matching `keyword`, case-insensitively.
fn keyword_at(wql: &str, toks: &[Tok], keyword: &str) -> Option<usize> {
    toks.iter()
        .position(|t| !t.quoted && tok_text(wql, t).eq_ignore_ascii_case(keyword))
}

/// The token after `keyword`, if there is one.
fn after_keyword<'a>(wql: &'a str, toks: &[Tok], keyword: &str) -> Option<&'a str> {
    let at = keyword_at(wql, toks, keyword)?;
    toks.get(at + 1).map(|t| tok_text(wql, t))
}

/// The event class a notification query subscribes to.
fn event_class_of(wql: &str) -> Option<&str> {
    let toks = tokens(wql);
    after_keyword(wql, &toks, "from").filter(|c| !c.is_empty())
}

/// The class an `ISA` clause narrows the subscription to.
fn watched_class_of(wql: &str) -> Option<&str> {
    let toks = tokens(wql);
    after_keyword(wql, &toks, "isa").filter(|c| !c.is_empty())
}

/// What the row's class column should say for this query: the watched class if
/// the query names one, else the event class itself.
fn subject_of(wql: &str) -> String {
    watched_class_of(wql)
        .or_else(|| event_class_of(wql))
        .unwrap_or_default()
        .to_string()
}

/// The polling interval a query already carries.
fn within_of(wql: &str) -> Option<u32> {
    let toks = tokens(wql);
    after_keyword(wql, &toks, "within")?.parse().ok()
}

/// Is this one of WMI's intrinsic event classes -- the ones synthesised by
/// polling the repository, and therefore the ones a `WITHIN` applies to?
///
/// The three families are `__Instance*Event`, `__Class*Event` and
/// `__Namespace*Event`. `__ExtrinsicEvent` is explicitly not one of them
/// despite the shape of its name.
fn is_intrinsic(class: &str) -> bool {
    let c = class.to_ascii_lowercase();
    c != "__extrinsicevent"
        && c.ends_with("event")
        && ["__instance", "__class", "__namespace"]
            .iter()
            .any(|prefix| c.starts_with(prefix))
}

/// Can a polling interval be written into this query?
///
/// Either it already has one -- in which case the field edits what is there --
/// or the query subscribes to an intrinsic class, where `WITHIN` is required.
/// A `WITHIN` on anything else is not something this view will invent.
fn accepts_within(wql: &str) -> bool {
    within_of(wql).is_some() || event_class_of(wql).is_some_and(is_intrinsic)
}

/// Write `secs` into `wql`'s `WITHIN` clause, adding the clause if it is absent.
///
/// Three cases, and the third is the one a naive string replace gets wrong:
/// a query with no `WITHIN` at all needs the clause placed after the event class
/// and before any `WHERE`, which means finding the class token rather than
/// appending.
fn inject_within(wql: &str, secs: u32) -> String {
    let toks = tokens(wql);

    if let Some(at) = keyword_at(wql, &toks, "within") {
        return match toks.get(at + 1) {
            Some(next) if !next.quoted && tok_text(wql, next).parse::<u32>().is_ok() => {
                format!("{}{}{}", &wql[..next.start], secs, &wql[next.end..])
            }
            // `WITHIN` with nothing usable after it: put a number there rather
            // than rewrite whatever the next token is.
            _ => {
                let end = toks[at].end;
                format!("{} {}{}", &wql[..end], secs, &wql[end..])
            }
        };
    }

    let Some(from) = keyword_at(wql, &toks, "from") else {
        return wql.to_string();
    };
    let Some(class) = toks.get(from + 1) else {
        return wql.to_string();
    };
    format!("{} WITHIN {}{}", &wql[..class.end], secs, &wql[class.end..])
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

/// What kind of change an event reports.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum EventKind {
    Creation,
    Modification,
    Deletion,
    /// An instance operation whose kind the delivered properties do not pin
    /// down -- see the module comment.
    Operation,
    /// Provider-pushed rather than synthesised from a poll.
    Extrinsic,
}

impl EventKind {
    fn label(self) -> &'static str {
        match self {
            Self::Creation => "Creation",
            Self::Modification => "Modification",
            Self::Deletion => "Deletion",
            Self::Operation => "Operation",
            Self::Extrinsic => "Extrinsic",
        }
    }

    /// The design's four kind colours, plus a neutral for the extrinsic case.
    /// Modification takes the accent's 300 step rather than the accent itself:
    /// the accent only clears 3:1 against this ground, which is not enough for
    /// an 10.5px label.
    fn color(self, ui: &egui::Ui) -> Color32 {
        match self {
            Self::Creation => OK,
            Self::Modification => a300(crate::widgets::button::accent_ramp(ui)),
            Self::Deletion => BAD,
            Self::Operation | Self::Extrinsic => muted(55),
        }
    }

    fn tip(self, event_class: &str) -> String {
        match self {
            Self::Operation => format!(
                "{event_class} delivers creations, modifications and deletions on one \
                 subscription, and the delivered properties only distinguish a \
                 modification (it carries PreviousInstance). This one does not."
            ),
            Self::Extrinsic => format!("{event_class} \u{2014} pushed by a provider, not polled."),
            _ => format!("Derived from the subscription's class, {event_class}."),
        }
    }
}

/// Classify one delivered event.
///
/// The query's class decides it wherever the query is specific. Where it is not
/// -- a subscription to a superclass -- the single discriminator that survives
/// the flattening is used: only a modification carries `PreviousInstance`.
fn kind_of(event_class: &str, pairs: &[(String, String)]) -> EventKind {
    let previous = pairs.iter().any(|(k, _)| k.starts_with("PreviousInstance"));
    match event_class.to_ascii_lowercase().as_str() {
        "__instancecreationevent" => EventKind::Creation,
        "__instancemodificationevent" => EventKind::Modification,
        "__instancedeletionevent" => EventKind::Deletion,
        c if c.starts_with("__") && c.ends_with("event") && c != "__extrinsicevent" => {
            if previous {
                EventKind::Modification
            } else {
                EventKind::Operation
            }
        }
        _ => EventKind::Extrinsic,
    }
}

fn leaf(key: &str) -> &str {
    key.rsplit('.').next().unwrap_or(key)
}

/// The fields a one-line summary chooses from, with a modification's two halves
/// folded together.
///
/// An intrinsic modification carries the same property twice, once under
/// `PreviousInstance` and once under `TargetInstance`, and printing both reads
/// as `Name = wsl \u{00b7} Name = wsl` -- measured, on a subscription to
/// `Win32_PerfFormattedData_PerfProc_Process`. Folded, the pair says what
/// actually happened: the old value, an arrow, the new one. A property that did
/// not change contributes its value once.
fn summary_fields(pairs: &[(String, String)]) -> Vec<(String, String)> {
    const PREVIOUS: &str = "PreviousInstance.";
    const TARGET: &str = "TargetInstance.";
    let mut out = Vec::new();

    for (key, value) in pairs {
        if ENVELOPE.contains(&key.as_str()) {
            continue;
        }
        if let Some(field) = key.strip_prefix(PREVIOUS) {
            // Folded in by its target twin below, unless there is no twin --
            // which is what a deletion looks like.
            if !pairs
                .iter()
                .any(|(k, _)| k.strip_prefix(TARGET) == Some(field))
            {
                out.push((format!("was {field}"), value.clone()));
            }
            continue;
        }
        let name = leaf(key);
        let before = pairs
            .iter()
            .find(|(k, _)| k.strip_prefix(PREVIOUS) == Some(name))
            .map(|(_, v)| v);
        let rendered = match before {
            Some(before) if before != value => format!("{before} \u{2192} {value}"),
            _ => value.clone(),
        };
        out.push((name.to_string(), rendered));
    }
    out
}

/// A one-line description of what arrived.
fn summarize(pairs: &[(String, String)]) -> String {
    let fields = summary_fields(pairs);
    let mut taken: Vec<usize> = Vec::new();
    let mut parts: Vec<String> = Vec::new();

    // Identity first. `flatten_event` sorts its pairs, so without a preference
    // every summary would open with `Caption` and push the name off the line.
    for wanted in SUMMARY_FIRST {
        if parts.len() >= SUMMARY_FIELDS {
            break;
        }
        if let Some(at) = fields.iter().position(|(name, _)| name == wanted) {
            taken.push(at);
            parts.push(format!("{wanted} = {}", fields[at].1));
        }
    }
    for (at, (name, value)) in fields.iter().enumerate() {
        if parts.len() >= SUMMARY_FIELDS {
            break;
        }
        if taken.contains(&at) {
            continue;
        }
        taken.push(at);
        parts.push(format!("{name} = {value}"));
    }
    parts.join(" \u{00b7} ")
}

/// One event that arrived.
struct EventRow {
    /// Stable identity. Indices shift as the log evicts; a selection keyed on
    /// one would silently open a different event.
    seq: u64,
    kind: EventKind,
    /// The class the subscription was watching when this arrived.
    class: String,
    /// The event class the subscription was on, for the kind's tooltip.
    event_class: String,
    summary: String,
    pairs: Vec<(String, String)>,
    /// App clock at arrival. The flash and the rate window read this.
    created_at: f64,
    /// Wall clock (UTC) at arrival, in milliseconds since the epoch.
    unix_ms: u64,
}

impl EventRow {
    /// Does this row match the stream filter? Everything on the row is
    /// searchable, including the properties only the detail panel shows.
    fn matches(&self, needle: &str) -> bool {
        if needle.is_empty() {
            return true;
        }
        self.class.to_lowercase().contains(needle)
            || self.summary.to_lowercase().contains(needle)
            || self.kind.label().to_lowercase().contains(needle)
            || self.pairs.iter().any(|(k, v)| {
                k.to_lowercase().contains(needle) || v.to_lowercase().contains(needle)
            })
    }
}

// ---------------------------------------------------------------------------
// The log
// ---------------------------------------------------------------------------

/// The retained event log: a capped ring, newest first.
///
/// This replaces a `Vec` that was front-inserted and then truncated to 500 on
/// every single event. Two things were wrong with that. The insert shifts the
/// whole vector, so the cost of an event grew with the history; and the truncate
/// dropped the oldest row *silently*, so a log that had lost half of what it saw
/// looked exactly like one that had not. A `VecDeque` push/pop pair is O(1) at
/// both ends and the eviction is counted.
pub(crate) struct EventLog {
    rows: VecDeque<EventRow>,
    cap: usize,
    /// Every event pushed since the last clear, including the evicted ones.
    received: u64,
    /// Rows the cap has dropped since the last clear.
    dropped: u64,
    next_seq: u64,
}

impl Default for EventLog {
    /// Hand-written: a derived `Default` would leave `cap` at zero, and a zero
    /// cap discards every event as it arrives -- which looks exactly like a
    /// subscription that is not delivering.
    fn default() -> Self {
        Self {
            rows: VecDeque::new(),
            cap: LOG_CAP,
            received: 0,
            dropped: 0,
            next_seq: 0,
        }
    }
}

impl EventLog {
    fn push(&mut self, mut row: EventRow) {
        row.seq = self.next_seq;
        self.next_seq += 1;
        if self.rows.len() == self.cap {
            self.rows.pop_back();
            self.dropped += 1;
        }
        self.rows.push_front(row);
        self.received += 1;
    }

    /// Rows held, newest first. Read by the status bar, which shows the same
    /// figure it always did.
    pub(crate) fn len(&self) -> usize {
        self.rows.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    fn iter(&self) -> impl Iterator<Item = &EventRow> {
        self.rows.iter()
    }

    fn find(&self, seq: u64) -> Option<&EventRow> {
        self.rows.iter().find(|r| r.seq == seq)
    }

    fn clear(&mut self) {
        self.rows.clear();
        self.received = 0;
        self.dropped = 0;
    }
}

// ---------------------------------------------------------------------------
// Delivery rate
// ---------------------------------------------------------------------------

/// Arrival stamps inside the sliding window.
///
/// The stamps are frame times, not the instant WMI produced the event: the
/// channel is drained once per frame, so a burst that arrives between two frames
/// lands on one stamp. Over a 30-second window at a 33 ms frame that quantisation
/// is below the noise, and it is the only clock this side of the channel has.
#[derive(Default)]
struct Rate {
    arrivals: VecDeque<f64>,
    /// App clock when measurement began (Start, or the last Clear).
    since: Option<f64>,
}

impl Rate {
    fn start(&mut self, now: f64) {
        self.arrivals.clear();
        self.since = Some(now);
    }

    fn stop(&mut self) {
        self.arrivals.clear();
        self.since = None;
    }

    fn mark(&mut self, at: f64) {
        self.arrivals.push_back(at);
    }

    fn prune(&mut self, now: f64) {
        while self
            .arrivals
            .front()
            .is_some_and(|&t| t < now - RATE_WINDOW)
        {
            self.arrivals.pop_front();
        }
    }

    /// Events per second over the window, or `None` until there is enough of a
    /// window to divide by.
    fn per_second(&self, now: f64) -> Option<f32> {
        let since = self.since?;
        let span = (now - since).min(RATE_WINDOW);
        if span < RATE_FLOOR {
            return None;
        }
        let counted = self
            .arrivals
            .iter()
            .filter(|&&t| t >= now - RATE_WINDOW)
            .count();
        Some(counted as f32 / span as f32)
    }
}

// ---------------------------------------------------------------------------
// View state
// ---------------------------------------------------------------------------

/// How the subscription is registered with WMI.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum Delivery {
    /// Lives as long as this process. The only one that ships.
    #[default]
    Temporary,
    /// Stored in the repository and surviving reboots. See [`PERMANENT_WHY`].
    ///
    /// Nothing constructs it, and the `allow` says so out loud rather than
    /// hiding it: the variant exists so the control has a second segment to
    /// disable and a name to explain, and the day it becomes constructible is
    /// the day the confirmation, the audit line and the teardown control have
    /// to exist too.
    #[allow(dead_code)]
    Permanent,
}

/// The Events view's own state.
///
/// The subscription itself (`monitor`, `monitor_wql`, `monitor_error`) and the
/// log stay on the app, because two other modules read them: the status bar
/// counts the log, and the Explorer's "Watch" action writes the query. Only what
/// belongs to this view alone lives here.
pub(crate) struct EventsView {
    /// The polling-interval field, as typed. A string rather than a number so a
    /// half-typed value does not snap back to something else under the caret.
    interval: String,
    delivery: Delivery,
    filter: String,
    /// `seq` of the row whose properties are open.
    selected: Option<u64>,
    rate: Rate,
    /// Depth of our own channel at the last drain, and the highest seen since
    /// Start. See [`QUEUED_WHY`].
    queued: usize,
    queued_peak: usize,
    /// App clock and wall clock when the running subscription started.
    started_at: Option<f64>,
    started_unix_ms: u64,
    /// The running subscription's classes and interval, resolved once at Start.
    /// Editing the query afterwards must not retroactively relabel rows that
    /// arrived under the old one, and the difference is what the "Stop and
    /// Start to apply" note is derived from.
    event_class: String,
    subject: String,
    started_within: Option<u32>,
}

impl Default for EventsView {
    fn default() -> Self {
        Self {
            interval: within_of(vmiscope_core::DEFAULT_EVENT_QUERY)
                .map(|n| n.to_string())
                .unwrap_or_default(),
            delivery: Delivery::default(),
            filter: String::new(),
            selected: None,
            rate: Rate::default(),
            queued: 0,
            queued_peak: 0,
            started_at: None,
            started_unix_ms: 0,
            event_class: String::new(),
            subject: String::new(),
            started_within: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Time
// ---------------------------------------------------------------------------

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// `HH:MM:SS` UTC.
fn utc_clock(unix_ms: u64) -> String {
    let secs = unix_ms / 1000;
    format!(
        "{:02}:{:02}:{:02}",
        (secs / 3600) % 24,
        (secs / 60) % 60,
        secs % 60
    )
}

/// `HH:MM:SS.mmm` UTC -- the stream's stamp.
fn utc_stamp(unix_ms: u64) -> String {
    format!("{}.{:03}", utc_clock(unix_ms), unix_ms % 1000)
}

// ---------------------------------------------------------------------------
// Easing
// ---------------------------------------------------------------------------

/// How far through its flash a row of age `age` is, eased: 1 at birth, 0 once
/// [`FLASH_SECS`] has passed.
fn flash_strength(age: f64) -> f32 {
    if !(0.0..FLASH_SECS).contains(&age) {
        return 0.0;
    }
    1.0 - egui::emath::easing::cubic_out((age / FLASH_SECS) as f32)
}

/// The pulse's position in its cycle, 0 at the top of the beat and 1 at the
/// trough. A cosine is its own ease-in-out, which is what the mock's
/// `ease-in-out` keyframes describe.
fn pulse_phase(time: f64) -> f32 {
    let turns = (time % PULSE_SECS) / PULSE_SECS;
    (0.5 - 0.5 * (turns * std::f64::consts::TAU).cos()) as f32
}

// ---------------------------------------------------------------------------
// Plumbing
// ---------------------------------------------------------------------------

impl VmiScopeApp {
    /// Drain the monitor's channel into the log.
    ///
    /// Called every frame from `App::ui`, on whatever view is on screen: a
    /// subscription that only collected while its own tab was visible would
    /// answer "what happened while I was looking at something else" with
    /// silence.
    pub(crate) fn drain_events(&mut self, now: f64) {
        let Some(monitor) = self.monitor.as_ref() else {
            self.events.queued = 0;
            return;
        };

        let messages = monitor.poll();
        // The one queue depth that can honestly be reported: how many messages
        // were waiting in our own channel at the moment we looked.
        self.events.queued = messages.len();
        self.events.queued_peak = self.events.queued_peak.max(messages.len());

        if !messages.is_empty() {
            let unix_ms = unix_millis();
            for message in messages {
                match message {
                    MonitorMsg::Event(pairs) => {
                        self.events.rate.mark(now);
                        self.events_log.push(EventRow {
                            // Overwritten by `EventLog::push`, which owns the
                            // sequence -- the log is the only thing that can
                            // hand out an identity that stays unique.
                            seq: 0,
                            kind: kind_of(&self.events.event_class, &pairs),
                            class: self.events.subject.clone(),
                            event_class: self.events.event_class.clone(),
                            summary: summarize(&pairs),
                            pairs,
                            created_at: now,
                            unix_ms,
                        });
                    }
                    MonitorMsg::Error(e) => self.monitor_error = Some(e),
                }
            }
        }
        self.events.rate.prune(now);
    }

    fn start_monitor(&mut self, now: f64) {
        self.monitor_error = None;
        self.events.rate.start(now);
        self.events.queued = 0;
        self.events.queued_peak = 0;
        self.events.started_at = Some(now);
        self.events.started_unix_ms = unix_millis();
        self.events.event_class = event_class_of(&self.monitor_wql)
            .unwrap_or_default()
            .to_string();
        self.events.subject = subject_of(&self.monitor_wql);
        self.events.started_within = within_of(&self.monitor_wql);
        self.monitor = Some(EventMonitor::start(
            self.active_ns.clone(),
            self.monitor_wql.clone(),
        ));
    }

    fn stop_monitor(&mut self) {
        self.monitor = None;
        // The error belongs to the subscription being torn down; leaving it up
        // outlives the thing it describes.
        self.monitor_error = None;
        self.events.rate.stop();
        self.events.queued = 0;
    }
}

// ---------------------------------------------------------------------------
// The view
// ---------------------------------------------------------------------------

impl VmiScopeApp {
    pub(crate) fn ui_events(&mut self, ui: &mut egui::Ui, now: f64) {
        egui::Panel::left("vs_events_config")
            .exact_size(CONFIG_W)
            // `Panel::left` is constructed resizable, and the resize-hover
            // branch wins over the flag, so a fixed column needs both.
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::NONE.fill(BG))
            .show(ui, |ui| self.ui_events_config(ui, now));

        // The raw-property reveal: a right panel present only while a row is
        // open, the same shape the Query view uses for a result row.
        if self.events.selected.is_some() {
            egui::Panel::right("vs_events_detail")
                .exact_size(DETAIL_W)
                .resizable(false)
                .show_separator_line(false)
                .frame(Frame::NONE.fill(BG))
                .show(ui, |ui| self.ui_event_detail(ui));
        }

        egui::CentralPanel::default()
            .frame(Frame::NONE.fill(BG))
            .show(ui, |ui| {
                self.ui_events_stream_header(ui, now);
                self.ui_events_stream(ui, now);
            });

        // Both animations here are clock-driven, so the view has to ask for the
        // frames itself. The app's 250 ms cadence keeps events *arriving* on any
        // view; it is nowhere near enough to draw a 2.4 s pulse or a 0.18 s
        // flash.
        let flashing = self
            .events_log
            .iter()
            .next()
            .is_some_and(|row| flash_strength(now - row.created_at) > 0.0);
        if self.monitor.is_some() || flashing {
            ui.ctx().request_repaint_after(LIVE_FRAME);
        }
    }

    // -- subscription column ------------------------------------------------

    fn ui_events_config(&mut self, ui: &mut egui::Ui, now: f64) {
        column_edge_right(ui);
        Frame::NONE
            .inner_margin(Margin::same(S3 as i8))
            .show(ui, |ui| {
                heading(ui, "Subscription");
                self.ui_events_target(ui);
                ui.add_space(S2);

                self.ui_events_query(ui);
                ui.add_space(S2);
                self.ui_events_interval(ui);
                ui.add_space(S2);

                field_label(ui, "Delivery");
                delivery_segmented(ui, &mut self.events.delivery);
                ui.add_space(S3);

                self.ui_events_actions(ui, now);
                if let Some(error) = self.monitor_error.clone() {
                    ui.add_space(S2);
                    subscription_error(ui, &error);
                }
                ui.add_space(S3);
                self.ui_events_stats(ui, now);
            });
    }

    /// Which machine and namespace the subscription binds to.
    ///
    /// `EventMonitor::start` opens its own local COM connection, so it does not
    /// follow the app's remote host. Saying so is the whole point of this line:
    /// a monitor silently watching the wrong machine is the worst outcome this
    /// view has available to it.
    fn ui_events_target(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(icons::labelled_styled(
                ui,
                icons::DATABASE,
                &self.active_ns,
                TextStyle::Small,
                muted(50),
            ));
        });
        if !matches!(self.conn_status, crate::app::ConnStatus::Local) {
            ui.label(
                RichText::new(
                    "Local machine \u{2014} a notification query does not follow the connection",
                )
                .text_style(TextStyle::Small)
                .color(WARN),
            )
            .on_hover_text(
                "The monitor opens its own local COM connection. Remote notification \
                 queries are not implemented, so this stream is this machine's \
                 whatever the Machines view is pointed at.",
            );
        }
    }

    fn ui_events_query(&mut self, ui: &mut egui::Ui) {
        field_label(ui, "Event query");
        let output = TextEdit::multiline(&mut self.monitor_wql)
            .font(TextStyle::Monospace)
            .desired_rows(EDITOR_ROWS)
            .desired_width(f32::INFINITY)
            .background_color(SURFACE)
            .show(ui);
        focus_ring(ui, &output.response);
    }

    /// The polling-interval field, which writes into the query's `WITHIN`.
    ///
    /// Two directions, and both are needed. Typing here rewrites the clause.
    /// Whenever the field is *not* being edited it re-reads the clause instead,
    /// so editing the query above -- or arriving here from the Explorer's
    /// "Watch" action, which writes a whole query -- cannot leave the field
    /// contradicting the thing it claims to control.
    fn ui_events_interval(&mut self, ui: &mut egui::Ui) {
        let injectable = accepts_within(&self.monitor_wql);
        field_label(ui, "Polling interval (WITHIN)");

        let mut changed = false;
        let response = ui
            .horizontal(|ui| {
                let field = ui
                    .scope(|ui| {
                        ui.set_max_width(INTERVAL_W);
                        ui.spacing_mut().text_edit_width = INTERVAL_W;
                        ui.add_enabled_ui(injectable, |ui| {
                            mono_input(ui, &mut self.events.interval, "n")
                        })
                        .inner
                    })
                    .inner;
                changed = field.changed();
                ui.label(
                    RichText::new("seconds")
                        .text_style(TextStyle::Small)
                        .color(muted(50)),
                );
                field
            })
            .inner;

        if !injectable {
            ui.label(
                RichText::new("not applied \u{2014} this query is not intrinsic")
                    .text_style(TextStyle::Small)
                    .color(muted(40)),
            )
            .on_hover_text(
                "WITHIN is the repository polling interval for the intrinsic event \
                 classes (__Instance*Event, __Class*Event, __Namespace*Event). This \
                 query subscribes to something else, so nothing is written into it.",
            );
            return;
        }

        if changed {
            match self.events.interval.trim().parse::<u32>() {
                Ok(secs) if (1..=MAX_WITHIN).contains(&secs) => {
                    self.monitor_wql = inject_within(&self.monitor_wql, secs);
                }
                _ => {}
            }
        } else if !response.has_focus() {
            if let Some(secs) = within_of(&self.monitor_wql) {
                let text = secs.to_string();
                if self.events.interval != text {
                    self.events.interval = text;
                }
            }
        }
    }

    /// Does the text in the box still describe the subscription that is running?
    ///
    /// `EventMonitor` cannot be asked what it was handed, so this compares the
    /// three things resolved at Start against the same three read out of the box
    /// now. Comparing the raw text instead would fire on a reformatted line
    /// break, which changes nothing about the subscription.
    fn subscription_drifted(&self) -> bool {
        self.monitor.is_some()
            && (event_class_of(&self.monitor_wql).unwrap_or_default() != self.events.event_class
                || subject_of(&self.monitor_wql) != self.events.subject
                || within_of(&self.monitor_wql) != self.events.started_within)
    }

    fn ui_events_actions(&mut self, ui: &mut egui::Ui, now: f64) {
        let running = self.monitor.is_some();
        if self.subscription_drifted() {
            ui.label(icons::labelled_styled(
                ui,
                icons::WARNING_CIRCLE,
                "Edited \u{2014} Stop and Start to apply",
                TextStyle::Small,
                WARN,
            ))
            .on_hover_text(
                "The running subscription was registered with WMI when Start was \
                 pressed. Editing the query here does not reach it, and the rows \
                 below still belong to the one that is running.",
            );
            ui.add_space(S1);
        }
        ui.horizontal(|ui| {
            // Stop is the primary once running, for the same reason Pause is on
            // the Network tab: ending it is the only decision left to make.
            let label = if running { "Stop" } else { "Start" };
            let icon = if running { icons::STOP } else { icons::PLAY };
            if btn_primary(ui, icons::labelled(ui, icon, label)).clicked() {
                if running {
                    self.stop_monitor();
                } else {
                    self.start_monitor(now);
                }
            }
            if btn_secondary(ui, icons::labelled(ui, icons::X, "Clear"))
                .on_hover_text("Forget every event held. The subscription keeps running.")
                .clicked()
            {
                self.events_log.clear();
                self.events.selected = None;
                if running {
                    self.events.rate.start(now);
                    self.events.queued_peak = 0;
                }
            }
        });
    }

    fn ui_events_stats(&mut self, ui: &mut egui::Ui, now: f64) {
        let received = self.events_log.received;
        let dropped = self.events_log.dropped;
        let rate = self.events.rate.per_second(now);
        let running = self.monitor.is_some();

        card(ui, |ui| {
            stat_row(
                ui,
                "Events received",
                &received.to_string(),
                muted(80),
                None,
            );
            if dropped > 0 {
                stat_row(
                    ui,
                    "Dropped (cap)",
                    &dropped.to_string(),
                    WARN,
                    Some(&format!(
                        "The log holds the newest {LOG_CAP}. This many older events \
                         have been evicted since the last Clear."
                    )),
                );
            }
            let (rate_text, rate_color) = match rate {
                Some(r) if r > 0.0 => (format!("{r:.2} / s"), OK),
                Some(_) => ("0 / s".to_string(), muted(80)),
                None => ("\u{2014}".to_string(), muted(35)),
            };
            stat_row(
                ui,
                "Delivery rate",
                &rate_text,
                rate_color,
                Some(&format!(
                    "Arrivals per second over the last {RATE_WINDOW:.0} s, counted here \
                     from the times events reached this app. An em dash means less \
                     than {RATE_FLOOR:.0} s of observation."
                )),
            );
            stat_row(
                ui,
                "Queued",
                &if running {
                    format!(
                        "{} \u{00b7} peak {}",
                        self.events.queued, self.events.queued_peak
                    )
                } else {
                    "\u{2014}".to_string()
                },
                if self.events.queued > 0 {
                    WARN
                } else {
                    muted(80)
                },
                Some(QUEUED_WHY),
            );
            stat_row(
                ui,
                "Delivery",
                &delivery_note(&self.monitor_wql),
                muted(80),
                Some(
                    "Read from the query's event class. Intrinsic events are \
                     synthesised by WMI polling the repository at the WITHIN \
                     interval; extrinsic ones are pushed by a provider as they \
                     happen.",
                ),
            );
        });
    }

    // -- stream -------------------------------------------------------------

    fn ui_events_stream_header(&mut self, ui: &mut egui::Ui, now: f64) {
        let running = self.monitor.is_some();
        let mut save = false;

        Frame::NONE
            .inner_margin(Margin::symmetric(S3 as i8, S2 as i8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    live_dot(ui, now, running, self.monitor_error.is_some());
                    heading(ui, "Event stream");
                    // The note is the only elastic thing in this row, so it is
                    // the one that has to give: a right-to-left group takes
                    // whatever is left after it, and with the detail reveal open
                    // the two collided (captured). Reserving the controls'
                    // width up front is what keeps the count from running under
                    // the filter box.
                    let reserved = FILTER_W
                        + if self.events_log.is_empty() {
                            0.0
                        } else {
                            SAVE_W
                        }
                        + S3 * 2.0;
                    ui.scope(|ui| {
                        ui.set_max_width((ui.available_width() - reserved).max(0.0));
                        ui.add(
                            Label::new(
                                RichText::new(self.stream_note())
                                    .text_style(TextStyle::Small)
                                    .color(muted(45)),
                            )
                            .truncate()
                            .selectable(false),
                        )
                        .on_hover_text(STAMP_WHY);
                    });

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        // Nothing to write is not the same as a button that
                        // writes an empty file, so the action is only offered
                        // once there is a log.
                        if !self.events_log.is_empty()
                            && btn_secondary(
                                ui,
                                icons::labelled(ui, icons::DOWNLOAD_SIMPLE, "Save log"),
                            )
                            .on_hover_text("Write the events shown, newest first, as JSON")
                            .clicked()
                        {
                            save = true;
                        }
                        ui.scope(|ui| {
                            ui.set_max_width(FILTER_W);
                            ui.spacing_mut().text_edit_width = FILTER_W;
                            filter_box(ui, &mut self.events.filter, "filter the stream");
                        });
                    });
                });
            });
        header_edge(ui);

        if save {
            let events: Vec<Vec<(String, String)>> = self
                .events_log
                .iter()
                .filter(|row| row.matches(&self.events.filter.to_lowercase()))
                .map(|row| row.pairs.clone())
                .collect();
            save_file(
                "wmi_events.json",
                &vmiscope_core::export::events_to_json(&events),
            );
        }
    }

    /// "N events since HH:MM:SS UTC", or whatever is true instead.
    fn stream_note(&self) -> String {
        let received = self.events_log.received;
        match self.events.started_at {
            Some(_) => format!(
                "{received} events since {} UTC",
                utc_clock(self.events.started_unix_ms)
            ),
            None => "not started".to_string(),
        }
    }

    fn ui_events_stream(&mut self, ui: &mut egui::Ui, now: f64) {
        let needle = self.events.filter.to_lowercase();
        let rows: Vec<&EventRow> = self
            .events_log
            .iter()
            .filter(|row| row.matches(&needle))
            .collect();

        if rows.is_empty() {
            Frame::NONE
                .inner_margin(Margin::same(S4 as i8))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(empty_note(
                            self.monitor.is_some(),
                            self.events.started_at.is_some(),
                            self.events_log.len(),
                        ))
                        .color(muted(40)),
                    );
                });
            return;
        }

        let accent = accent(ui);
        let selected = self.events.selected;
        let mut opened: Option<u64> = None;

        ui.scope(|ui| {
            // `show_rows` derives its geometry from `row_height + item_spacing.y`,
            // and it has to be told here rather than inside: the value is read
            // from the parent before the closure runs. Rows are separated by
            // their own rule, not by a gap.
            ui.spacing_mut().item_spacing.y = 0.0;
            egui::ScrollArea::vertical()
                .id_salt("event-stream")
                .auto_shrink([false, false])
                .show_rows(ui, ROW_H, rows.len(), |ui, range| {
                    for at in range {
                        let row = rows[at];
                        if stream_row(ui, row, now, accent, selected == Some(row.seq)) {
                            opened = Some(row.seq);
                        }
                    }
                });
        });

        if let Some(seq) = opened {
            // Clicking the open row closes it, which is the only way back from a
            // reveal that has no close button of its own on the row.
            self.events.selected = (self.events.selected != Some(seq)).then_some(seq);
        }
    }

    // -- detail reveal ------------------------------------------------------

    fn ui_event_detail(&mut self, ui: &mut egui::Ui) {
        let Some(seq) = self.events.selected else {
            return;
        };
        let Some(row) = self.events_log.find(seq) else {
            // The row aged out from under the reveal. Close it rather than show
            // an empty panel that looks like a rendering fault.
            self.events.selected = None;
            return;
        };

        let kind_color = row.kind.color(ui);
        let mut close = false;

        column_edge_left(ui);
        Frame::NONE
            .inner_margin(Margin::same(S3 as i8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    heading(ui, "Event");
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if crate::widgets::button::btn_icon(ui, icons::X)
                            .on_hover_text("Close")
                            .clicked()
                        {
                            close = true;
                        }
                    });
                });
                ui.label(
                    RichText::new(utc_stamp(row.unix_ms))
                        .text_style(TextStyle::Monospace)
                        .size(STAMP_SIZE)
                        .color(muted(55)),
                )
                .on_hover_text(STAMP_WHY);
                ui.label(
                    RichText::new(row.kind.label())
                        .text_style(TextStyle::Small)
                        .color(kind_color),
                )
                .on_hover_text(row.kind.tip(&row.event_class));
                ui.label(
                    RichText::new(&row.class)
                        .text_style(TextStyle::Monospace)
                        .size(BODY_SIZE),
                )
                .on_hover_text(CLASS_WHY);

                ui.add_space(S3);
                ui.label(
                    RichText::new("PROPERTIES AS DELIVERED")
                        .text_style(TextStyle::Small)
                        .color(muted(45)),
                )
                .on_hover_text(
                    "Every pair the monitor received. WMI's GetNames hides the __ \
                     system properties, so __CLASS, __PATH and __RELPATH are not \
                     among them \u{2014} they were never delivered, not dropped here.",
                );
                ui.add_space(S1);
                egui::ScrollArea::vertical()
                    .id_salt("event-props")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        kv_grid_sized(
                            ui,
                            "event-detail",
                            120.0,
                            row.pairs.iter().map(|(k, v)| (k.as_str(), v.as_str())),
                        );
                    });
            });

        if close {
            self.events.selected = None;
        }
    }
}

// ---------------------------------------------------------------------------
// Row painting
// ---------------------------------------------------------------------------

/// One stream row. Returns true when it was clicked.
///
/// Hand-built rather than handed to the table kit: this list has no header, no
/// resizable columns and no sort, and every one of those is something the table
/// would insist on. What it does need -- a per-row background that is painted
/// *before* the content -- is a `Ui::new_child` over an already-allocated rect,
/// the same placement idiom the shell uses.
fn stream_row(
    ui: &mut egui::Ui,
    row: &EventRow,
    now: f64,
    accent: Color32,
    selected: bool,
) -> bool {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::click());

    // The flash. `created_at` is the row's own birth time and the fade is eased
    // off it, which is the only way an element that did not exist last frame can
    // animate at all -- see FLASH_SECS.
    let age = now - row.created_at;
    let flash = flash_strength(age);
    let painter = ui.painter();
    if flash > 0.0 {
        painter.rect_filled(
            rect,
            CornerRadius::ZERO,
            accent.gamma_multiply(FLASH_TINT * flash),
        );
    }
    if selected {
        painter.rect_filled(
            rect,
            CornerRadius::ZERO,
            accent.gamma_multiply(HOVER_TINT * 3.0),
        );
    }
    if response.hovered() {
        painter.rect_filled(rect, CornerRadius::ZERO, TEXT.gamma_multiply(HOVER_TINT));
    }
    solid_hline(
        painter,
        Rect::from_min_size(
            Pos2::new(rect.left(), rect.bottom() - HAIRLINE),
            Vec2::new(rect.width(), HAIRLINE),
        ),
        muted(ROW_RULE),
    );

    // The mock's `noct-row` also ramps the row's *content* from zero opacity.
    // That is dropped here, and it was measured rather than guessed: an
    // intrinsic subscription delivers a whole poll's worth of events on one
    // frame, so at 260 ev/s every visible row shares a birth time and the ramp
    // blanks the entire stream for 0.18 s at a stroke (captured -- see the
    // commit's verification notes). A row you cannot read is worse than a row
    // that simply appears; the accent wash alone says "new", and text at full
    // strength over an 18% tint stays legible the whole way through.
    let inner = rect.shrink2(Vec2::new(S3, 0.0));
    let icon_rect = Rect::from_min_max(
        Pos2::new(inner.right() - ICON_W, inner.top()),
        inner.right_bottom(),
    );
    let body = Rect::from_min_max(
        inner.min,
        Pos2::new((icon_rect.left() - S2).max(inner.left()), inner.bottom()),
    );

    let mut cells = ui.new_child(
        UiBuilder::new()
            .max_rect(body)
            .layout(Layout::left_to_right(Align::Center)),
    );
    cells.spacing_mut().item_spacing.x = S3;
    cells.add(
        Label::new(
            RichText::new(utc_stamp(row.unix_ms))
                .text_style(TextStyle::Monospace)
                .size(STAMP_SIZE)
                .color(muted(40)),
        )
        .selectable(false),
    );
    fixed_cell(&mut cells, KIND_W, |ui| {
        ui.add(
            Label::new(
                RichText::new(row.kind.label())
                    .text_style(TextStyle::Small)
                    .size(KIND_SIZE)
                    .color(row.kind.color(ui)),
            )
            .truncate()
            .selectable(false),
        );
    });
    fixed_cell(&mut cells, CLASS_W, |ui| {
        ui.add(
            Label::new(
                RichText::new(&row.class)
                    .text_style(TextStyle::Monospace)
                    .size(BODY_SIZE)
                    .color(TEXT),
            )
            .truncate()
            .selectable(false),
        );
    });
    cells.add(
        Label::new(
            RichText::new(&row.summary)
                .text_style(TextStyle::Monospace)
                .size(BODY_SIZE)
                .color(muted(62)),
        )
        .truncate()
        .selectable(false),
    );

    let open_tint = if selected || response.hovered() {
        muted(70)
    } else {
        muted(30)
    };
    let mut icon = ui.new_child(
        UiBuilder::new()
            .max_rect(icon_rect)
            .layout(Layout::right_to_left(Align::Center)),
    );
    icon.add(
        Label::new(
            icons::glyph(icons::ARROW_SQUARE_OUT)
                .size(ICON_SIZE)
                .color(open_tint),
        )
        .selectable(false),
    );

    response.clicked()
}

/// A cell of exactly `width`, whatever its content measures.
fn fixed_cell(ui: &mut egui::Ui, width: f32, add: impl FnOnce(&mut egui::Ui)) {
    ui.allocate_ui_with_layout(
        Vec2::new(width, ui.available_height()),
        Layout::left_to_right(Align::Center),
        add,
    );
}

// ---------------------------------------------------------------------------
// Small parts
// ---------------------------------------------------------------------------

/// The design's 13.5px medium-weight panel heading.
fn heading(ui: &mut egui::Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .family(egui::FontFamily::Name(
                crate::theme::fonts::UI_MEDIUM.into(),
            ))
            .size(HEADING_SIZE)
            .color(TEXT),
    );
}

/// A field's label: the mock's `.field > label`.
fn field_label(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(LABEL_SIZE).color(muted(70)));
    ui.add_space(S1);
}

/// One row of the stats card: muted key, monospace value, optional tooltip.
fn stat_row(ui: &mut egui::Ui, key: &str, value: &str, color: Color32, tip: Option<&str>) {
    let response = ui
        .horizontal(|ui| {
            ui.label(RichText::new(key).size(STAT_SIZE).color(muted(50)));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new(value)
                        .text_style(TextStyle::Monospace)
                        .size(STAT_SIZE)
                        .color(color),
                );
            });
        })
        .response;
    if let Some(tip) = tip {
        response.on_hover_text(tip);
    }
}

/// What the "Delivery" stat says about this query.
fn delivery_note(wql: &str) -> String {
    let class = event_class_of(wql).unwrap_or_default();
    if class.is_empty() {
        return "\u{2014}".to_string();
    }
    if is_intrinsic(class) {
        match within_of(wql) {
            Some(secs) => format!("polled / {secs} s"),
            None => "polled".to_string(),
        }
    } else {
        "pushed".to_string()
    }
}

/// The pulsing live dot.
///
/// The pulse comes from `input().time`, not `Ui::animate_bool`. `animate_bool`
/// eases once between two states and then holds: there is no way to make it
/// loop, so a heartbeat built on it beats exactly once.
fn live_dot(ui: &mut egui::Ui, time: f64, running: bool, errored: bool) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(DOT_R * 2.0), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let color = match (running, errored) {
        (_, true) => BAD,
        (true, false) => OK,
        (false, false) => NEUTRAL[5],
    };
    let (alpha, scale) = if running {
        let phase = pulse_phase(time);
        (
            1.0 - (1.0 - PULSE_MIN_ALPHA) * phase,
            1.0 - (1.0 - PULSE_MIN_SCALE) * phase,
        )
    } else {
        (1.0, 1.0)
    };

    let painter = ui.painter();
    if running {
        painter.circle_filled(
            rect.center(),
            DOT_R * GLOW_SCALE * scale,
            color.gamma_multiply(GLOW_ALPHA * alpha),
        );
    }
    painter.circle_filled(rect.center(), DOT_R * scale, color.gamma_multiply(alpha));
}

/// The delivery segmented control.
///
/// Hand-built rather than `widgets::button::segmented`, because one of its two
/// options has to ship **disabled with a reason**, and the kit's control has no
/// way to express that -- every option there is selectable by construction.
/// Everything else about the shape is the kit's: one bordered group, a hairline
/// seam, an inset accent ring on the selection.
fn delivery_segmented(ui: &mut egui::Ui, current: &mut Delivery) {
    let a = accent(ui);
    Frame::NONE
        .stroke(Stroke::new(HAIRLINE, DIVIDER))
        .corner_radius(R_MD)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            {
                let widgets = &mut ui.visuals_mut().widgets;
                for state in [
                    &mut widgets.inactive,
                    &mut widgets.hovered,
                    &mut widgets.active,
                ] {
                    state.bg_stroke = Stroke::NONE;
                    state.corner_radius = R_SM;
                }
                widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
                widgets.hovered.weak_bg_fill = TEXT.gamma_multiply(0.07);
                widgets.active.weak_bg_fill = TEXT.gamma_multiply(0.14);
            }
            let half = (ui.available_width() - HAIRLINE) * 0.5;
            let height = ui.spacing().interact_size.y;

            ui.horizontal(|ui| {
                let selected = *current == Delivery::Temporary;
                let temporary = ui.add_sized(
                    Vec2::new(half, height),
                    Button::new(RichText::new("Temporary").color(if selected { a } else { TEXT })),
                );
                if temporary.clicked() {
                    *current = Delivery::Temporary;
                }
                if selected {
                    ui.painter().rect_stroke(
                        temporary.rect,
                        R_SM,
                        Stroke::new(HAIRLINE, a),
                        egui::StrokeKind::Inside,
                    );
                }
                focus_ring(ui, &temporary);
                temporary.on_hover_text(
                    "Lives as long as VMI-Scope is running. Nothing is written to the \
                     repository.",
                );

                let permanent = ui
                    .add_enabled(
                        false,
                        Button::new(RichText::new("Permanent")).min_size(Vec2::new(half, height)),
                    )
                    .on_disabled_hover_text(PERMANENT_WHY);
                solid_vline(
                    ui.painter(),
                    Rect::from_min_max(
                        permanent.rect.left_top(),
                        permanent.rect.left_bottom() + Vec2::new(HAIRLINE, 0.0),
                    ),
                    DIVIDER,
                );
            });
        });
}

/// The subscription's error, and what can honestly be said about it.
fn subscription_error(ui: &mut egui::Ui, error: &str) {
    ui.label(icons::labelled_styled(
        ui,
        icons::WARNING_CIRCLE,
        error.lines().next().unwrap_or("subscription error"),
        TextStyle::Small,
        BAD,
    ))
    .on_hover_text(error.to_string());
    ui.label(
        RichText::new(
            "If it failed at subscribe time the stream has ended \u{2014} Stop, then Start.",
        )
        .text_style(TextStyle::Small)
        .color(muted(45)),
    )
    .on_hover_text(
        "A connect or subscribe failure ends the monitor thread; a per-event error \
         does not. The channel looks identical either way, so this view will not \
         guess which one happened.",
    );
}

/// What the stream says when it has nothing to show, which is a different
/// sentence depending on why.
fn empty_note(running: bool, started: bool, held: usize) -> &'static str {
    match (running, started, held) {
        (_, _, held) if held > 0 => "No event held matches this filter.",
        (true, _, _) => "Subscribed. Nothing has matched yet.",
        (false, true, _) => "Stopped. The log was cleared.",
        (false, false, _) => "Not started \u{2014} check the query and press Start.",
    }
}

/// Paint the config column's own right edge, flush with its outer rect. The
/// panel draws no separator of its own; see `ui_events`.
fn column_edge_right(ui: &egui::Ui) {
    let r = ui.max_rect();
    solid_vline(
        ui.painter(),
        Rect::from_min_max(
            Pos2::new(r.right() - HAIRLINE, r.top()),
            Pos2::new(r.right(), r.bottom()),
        ),
        DIVIDER,
    );
}

/// The same, on a right-hand panel's left edge.
fn column_edge_left(ui: &egui::Ui) {
    let r = ui.max_rect();
    solid_vline(
        ui.painter(),
        Rect::from_min_max(r.left_top(), Pos2::new(r.left() + HAIRLINE, r.bottom())),
        DIVIDER,
    );
}

/// The hairline under the stream header.
fn header_edge(ui: &mut egui::Ui) {
    let (_, rect) = ui.allocate_space(Vec2::new(ui.available_width(), HAIRLINE));
    solid_hline(ui.painter(), rect, DIVIDER);
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- the WQL scanner ---------------------------------------------------

    #[test]
    fn a_literal_hides_its_contents_from_every_derivation() {
        // Every one of these words is a keyword outside a literal and a value
        // inside one. A substring search would find all four.
        let wql = "SELECT * FROM __InstanceCreationEvent WITHIN 2 \
                   WHERE TargetInstance.Name = 'within from isa 99'";
        assert_eq!(event_class_of(wql), Some("__InstanceCreationEvent"));
        assert_eq!(within_of(wql), Some(2));
        assert_eq!(watched_class_of(wql), None);
    }

    #[test]
    fn a_doubled_quote_is_an_escape_not_a_terminator() {
        let wql = "SELECT * FROM X WHERE Name = 'it''s here' AND Other ISA 'Win32_Service'";
        assert_eq!(watched_class_of(wql), Some("Win32_Service"));
    }

    #[test]
    fn the_watched_class_beats_the_event_class_as_the_subject() {
        let wql = "SELECT * FROM __InstanceCreationEvent WITHIN 2 \
                   WHERE TargetInstance ISA 'Win32_Process'";
        assert_eq!(subject_of(wql), "Win32_Process");
        // With no ISA there is nothing better than the event class itself.
        assert_eq!(
            subject_of("SELECT * FROM Win32_ProcessStartTrace"),
            "Win32_ProcessStartTrace"
        );
    }

    // -- WITHIN injection (task 4.8's acceptance criterion) -----------------

    #[test]
    fn the_interval_is_written_into_the_existing_clause() {
        let wql = "SELECT * FROM __InstanceCreationEvent WITHIN 2 \
                   WHERE TargetInstance ISA 'Win32_Process'";
        let out = inject_within(wql, 15);
        assert_eq!(
            out,
            "SELECT * FROM __InstanceCreationEvent WITHIN 15 \
             WHERE TargetInstance ISA 'Win32_Process'"
        );
        assert_eq!(within_of(&out), Some(15));
    }

    #[test]
    fn injection_survives_line_breaks_and_lower_case() {
        let wql = "SELECT * FROM __InstanceModificationEvent\n  within 2\n WHERE TargetInstance ISA 'Win32_Service'";
        let out = inject_within(wql, 30);
        assert_eq!(within_of(&out), Some(30));
        assert!(out.contains("within 30"), "{out}");
        // The rest of the text is untouched, newlines included.
        assert_eq!(out.lines().count(), 3);
    }

    #[test]
    fn a_query_without_a_clause_gets_one_in_the_right_place() {
        let out = inject_within(
            "SELECT * FROM __InstanceDeletionEvent WHERE TargetInstance ISA 'Win32_Process'",
            5,
        );
        assert_eq!(
            out,
            "SELECT * FROM __InstanceDeletionEvent WITHIN 5 \
             WHERE TargetInstance ISA 'Win32_Process'"
        );
        assert_eq!(within_of(&out), Some(5));
    }

    #[test]
    fn a_query_with_no_from_is_left_exactly_as_it_is() {
        // Nowhere to put the clause, so nothing is guessed.
        let broken = "SELECT * WHERE TargetInstance ISA 'Win32_Process'";
        assert_eq!(inject_within(broken, 5), broken);
    }

    #[test]
    fn only_intrinsic_queries_accept_an_interval() {
        assert!(accepts_within(
            "SELECT * FROM __InstanceCreationEvent WHERE TargetInstance ISA 'Win32_Process'"
        ));
        assert!(!accepts_within("SELECT * FROM Win32_ProcessStartTrace"));
        // ... unless one is already there, in which case the field edits it.
        assert!(accepts_within(
            "SELECT * FROM Win32_ProcessStartTrace WITHIN 4"
        ));
        assert!(!is_intrinsic("__ExtrinsicEvent"));
        assert!(is_intrinsic("__NamespaceOperationEvent"));
    }

    // -- kinds --------------------------------------------------------------

    fn pairs(keys: &[&str]) -> Vec<(String, String)> {
        keys.iter()
            .map(|k| ((*k).to_string(), "x".to_string()))
            .collect()
    }

    #[test]
    fn the_kind_comes_from_the_subscription_class() {
        let p = pairs(&["TargetInstance.Name"]);
        assert_eq!(kind_of("__InstanceCreationEvent", &p), EventKind::Creation);
        assert_eq!(kind_of("__InstanceDeletionEvent", &p), EventKind::Deletion);
        assert_eq!(
            kind_of("__InstanceModificationEvent", &p),
            EventKind::Modification
        );
        assert_eq!(kind_of("Win32_ProcessStartTrace", &p), EventKind::Extrinsic);
    }

    /// A subscription to the superclass delivers all three kinds on one stream,
    /// and `__CLASS` is not in the payload to tell them apart. `PreviousInstance`
    /// is the only discriminator that survives, and it only identifies one of
    /// the three -- so the other two must not be guessed at.
    #[test]
    fn an_operation_event_is_only_narrowed_where_the_payload_allows_it() {
        assert_eq!(
            kind_of(
                "__InstanceOperationEvent",
                &pairs(&["TargetInstance.Name", "PreviousInstance.Name"])
            ),
            EventKind::Modification
        );
        assert_eq!(
            kind_of("__InstanceOperationEvent", &pairs(&["TargetInstance.Name"])),
            EventKind::Operation
        );
    }

    // -- the summary --------------------------------------------------------

    #[test]
    fn the_summary_leads_with_the_identity_not_the_alphabet() {
        // `flatten_event` sorts its pairs, so Caption and CommandLine come
        // first in the data and would push the name off a three-field line.
        let ev = vec![
            ("TIME_CREATED".to_string(), "133".to_string()),
            ("TargetInstance.Caption".to_string(), "cmd.exe".to_string()),
            (
                "TargetInstance.CommandLine".to_string(),
                "cmd /c whoami".to_string(),
            ),
            ("TargetInstance.Name".to_string(), "cmd.exe".to_string()),
            ("TargetInstance.ProcessId".to_string(), "9016".to_string()),
        ];
        let line = summarize(&ev);
        assert!(line.starts_with("Name = cmd.exe"), "{line}");
        assert!(line.contains("ProcessId = 9016"), "{line}");
        assert!(
            !line.contains("TIME_CREATED"),
            "the envelope crowded out the payload: {line}"
        );
    }

    #[test]
    fn an_event_with_nothing_familiar_still_summarises() {
        let ev = vec![("Foo".to_string(), "1".to_string())];
        assert_eq!(summarize(&ev), "Foo = 1");
        assert_eq!(summarize(&[]), "");
    }

    /// Measured on a real `__InstanceModificationEvent` subscription: the same
    /// property arrives twice, and printing both reads as "Name = wsl · Name =
    /// wsl". The pair has to fold into the one thing worth saying.
    #[test]
    fn a_modification_folds_its_two_halves_into_one_field() {
        let changed = vec![
            ("PreviousInstance.Name".to_string(), "Running".to_string()),
            ("TargetInstance.Name".to_string(), "Stopped".to_string()),
        ];
        assert_eq!(summarize(&changed), "Name = Running \u{2192} Stopped");

        let unchanged = vec![
            ("PreviousInstance.Name".to_string(), "wsl".to_string()),
            ("TargetInstance.Name".to_string(), "wsl".to_string()),
        ];
        assert_eq!(summarize(&unchanged), "Name = wsl");

        // A previous half with no twin is a property that went away, and is
        // still worth saying.
        let gone = vec![("PreviousInstance.Handle".to_string(), "42".to_string())];
        assert_eq!(summarize(&gone), "was Handle = 42");
    }

    // -- the ring -----------------------------------------------------------

    fn row(at: f64) -> EventRow {
        EventRow {
            seq: 0,
            kind: EventKind::Creation,
            class: "Win32_Process".into(),
            event_class: "__InstanceCreationEvent".into(),
            summary: "Name = cmd.exe".into(),
            pairs: Vec::new(),
            created_at: at,
            unix_ms: 0,
        }
    }

    #[test]
    fn the_ring_holds_its_cap_newest_first_and_counts_what_it_drops() {
        let mut log = EventLog {
            cap: 3,
            ..EventLog::default()
        };
        for i in 0..10 {
            log.push(row(i as f64));
        }
        assert_eq!(log.len(), 3);
        assert_eq!(log.received, 10, "the total must survive eviction");
        assert_eq!(log.dropped, 7);

        let held: Vec<f64> = log.iter().map(|r| r.created_at).collect();
        assert_eq!(held, vec![9.0, 8.0, 7.0], "newest first");
    }

    #[test]
    fn a_default_log_keeps_events() {
        // A derived Default would leave the cap at zero, and a zero cap discards
        // every event as it arrives -- indistinguishable from a dead
        // subscription.
        let mut log = EventLog::default();
        assert_eq!(log.cap, LOG_CAP);
        log.push(row(0.0));
        assert_eq!(log.len(), 1);
        assert!(!log.is_empty());
    }

    #[test]
    fn a_selection_survives_eviction_or_is_told_it_did_not() {
        let mut log = EventLog {
            cap: 2,
            ..EventLog::default()
        };
        log.push(row(0.0));
        let first = log.iter().next().expect("a row").seq;
        assert!(log.find(first).is_some());
        log.push(row(1.0));
        log.push(row(2.0));
        assert!(
            log.find(first).is_none(),
            "an evicted seq must not resolve to some other row"
        );
    }

    #[test]
    fn clearing_resets_the_counters_too() {
        let mut log = EventLog {
            cap: 1,
            ..EventLog::default()
        };
        log.push(row(0.0));
        log.push(row(1.0));
        assert_eq!(log.dropped, 1);
        log.clear();
        assert_eq!(log.len(), 0);
        assert_eq!(log.received, 0);
        assert_eq!(log.dropped, 0);
    }

    // -- rate ---------------------------------------------------------------

    #[test]
    fn the_rate_is_measured_not_assumed() {
        let mut rate = Rate::default();
        assert_eq!(rate.per_second(0.0), None, "not running");

        rate.start(0.0);
        assert_eq!(rate.per_second(0.5), None, "too little to divide by");

        for i in 0..10 {
            rate.mark(i as f64 * 0.1);
        }
        // Ten arrivals over two seconds of observation.
        let r = rate.per_second(2.0).expect("a rate");
        assert!((r - 5.0).abs() < 1e-3, "{r}");
    }

    #[test]
    fn the_window_slides_so_a_burst_does_not_stay_on_the_number_forever() {
        let mut rate = Rate::default();
        rate.start(0.0);
        for i in 0..100 {
            rate.mark(i as f64 * 0.01);
        }
        let during = rate.per_second(1.0).expect("a rate");
        assert!(during > 90.0, "{during}");

        // A minute later, with the burst outside the window and nothing since.
        let now = 60.0;
        rate.prune(now);
        assert_eq!(rate.arrivals.len(), 0);
        assert_eq!(rate.per_second(now), Some(0.0));
    }

    // -- motion -------------------------------------------------------------

    /// The flash has to *start* at full strength and decay. This is the property
    /// `animate_bool` cannot provide: for an `Id` it has not seen before it
    /// returns the target value immediately, so a row born this frame would be
    /// handed "already finished".
    #[test]
    fn the_flash_starts_full_and_decays_to_nothing() {
        assert_eq!(flash_strength(0.0), 1.0);
        assert_eq!(flash_strength(FLASH_SECS), 0.0);
        assert_eq!(flash_strength(FLASH_SECS * 10.0), 0.0);
        assert_eq!(flash_strength(-1.0), 0.0, "a clock that went backwards");

        let mut previous = f32::MAX;
        for step in 0..=18 {
            let now = flash_strength(step as f64 * 0.01);
            assert!(now <= previous, "the flash brightened at step {step}");
            previous = now;
        }
        // Eased, not linear: cubic_out is steepest at the start.
        assert!(flash_strength(FLASH_SECS * 0.5) < 0.5);
    }

    /// The pulse has to loop. `animate_bool` eases once between two states and
    /// then holds, so a heartbeat built on it beats exactly once.
    #[test]
    fn the_pulse_loops() {
        for t in [0.0, 0.3, 1.2, 2.0] {
            let phase = pulse_phase(t);
            assert!((0.0..=1.0).contains(&phase), "{t} -> {phase}");
        }
        assert!(pulse_phase(0.0) < 0.01, "the beat starts at the top");
        assert!(pulse_phase(PULSE_SECS / 2.0) > 0.99, "and troughs halfway");
        // Two full periods apart is the same point in the cycle.
        assert!((pulse_phase(0.7) - pulse_phase(0.7 + PULSE_SECS * 2.0)).abs() < 1e-3);
        // And it is not a constant, which is the whole failure mode.
        assert!((pulse_phase(0.2) - pulse_phase(0.9)).abs() > 0.1);
    }

    // -- clock --------------------------------------------------------------

    #[test]
    fn the_stamp_is_utc_to_the_millisecond() {
        // 1970-01-01T01:02:03.456Z
        let ms: u64 = (3600 + 2 * 60 + 3) * 1000 + 456;
        assert_eq!(utc_clock(ms), "01:02:03");
        assert_eq!(utc_stamp(ms), "01:02:03.456");
        // And it wraps by day rather than counting hours since 1970.
        assert_eq!(utc_clock(ms + 86_400_000), "01:02:03");
    }

    // -- copy ---------------------------------------------------------------

    #[test]
    fn the_delivery_note_reads_off_the_query() {
        assert_eq!(
            delivery_note("SELECT * FROM __InstanceCreationEvent WITHIN 2 WHERE X ISA 'Y'"),
            "polled / 2 s"
        );
        assert_eq!(
            delivery_note("SELECT * FROM Win32_ProcessStartTrace"),
            "pushed"
        );
        assert_eq!(delivery_note("nonsense"), "\u{2014}");
    }
}
