//! Process start/stop events, and the guarded lookup that adds a command line.
//!
//! # Why these events are shaped the way they are
//!
//! `Win32_ProcessStartTrace` and `Win32_ProcessStopTrace` derive from
//! `__ExtrinsicEvent`: the provider *pushes* them, so there is no `WITHIN` and
//! no snapshot polling, and therefore no window in which a process can live and
//! die unobserved. That is the entire reason this module exists — a polled
//! `__InstanceCreationEvent WITHIN 2` subscription was measured missing 67 of
//! 72 instant-exit processes on this machine (`docs/FINDINGS.md`).
//!
//! Being extrinsic has a concrete consequence for parsing: **there is no
//! `TargetInstance`**. Every property is a scalar sitting directly on the event
//! object, so the embedded-object drill-down in [`crate::monitor`] never fires
//! here and every field arrives through the scalar branch. The intrinsic
//! fallback does have a `TargetInstance`, and [`crate::procmon`] unwraps it
//! before calling [`ProcEvent::from_map`], so this module only ever sees a flat
//! map either way.
//!
//! # What the events do not carry
//!
//! There is no command line and no image path — `ProcessName` is the bare
//! executable name. Recovering a command line needs a follow-up
//! `Win32_Process` query, which is genuinely racy: the process may already be
//! gone, and a pid may have been reused. [`enrich_process`] does it anyway, but
//! only hands back an answer it can prove belongs to the same process.

use std::collections::HashMap;

use wmi::{Variant, WMIConnection};

use crate::value::{variant_to_bytes, variant_to_string, variant_to_u32, variant_to_u64};

/// 100 ns ticks between the FILETIME epoch (1601-01-01) and the Unix epoch.
const FILETIME_UNIX_EPOCH: u64 = 116_444_736_000_000_000;

/// Days between 1601-01-01 and 1970-01-01.
const DAYS_1601_TO_1970: i64 = 134_774;

/// How far apart the event clock and `Win32_Process.CreationDate` may be and
/// still describe the same process: 2 seconds.
///
/// They are not the same measurement. `TIME_CREATED` is stamped when the kernel
/// trace provider emits the event; `CreationDate` is the process object's own
/// creation time, recorded moments earlier and truncated to microseconds. A
/// tolerance is therefore required, and 2 s is far tighter than any plausible
/// pid reuse — Windows cycles pids through a large space, and a reuse *inside*
/// two seconds that also matched the parent pid and the image name would be the
/// same program launched twice by the same launcher, where the command line is
/// the one we would have wanted anyway.
const CREATION_TOLERANCE_100NS: u64 = 2 * 10_000_000;

/// Which end of a process lifetime an event describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
pub enum ProcKind {
    Start,
    Stop,
}

impl ProcKind {
    /// A one-character sign for a table's leading column.
    pub fn sign(self) -> &'static str {
        match self {
            ProcKind::Start => "+",
            ProcKind::Stop => "-",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ProcKind::Start => "start",
            ProcKind::Stop => "stop",
        }
    }
}

/// One process start or stop, as delivered by the monitor.
///
/// Fields mirror the trace classes exactly, so nothing here is derived or
/// guessed. `exit_status` is `None` on a start (the class does not declare it)
/// and on a stop whose provider left it NULL.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ProcEvent {
    pub kind: ProcKind,
    pub pid: u32,
    pub parent_pid: u32,
    /// Bare image name (`cmd.exe`) — the trace classes carry no path.
    pub name: String,
    pub session_id: u32,
    /// Raw binary owner SID. Empty when the event carried none.
    ///
    /// Kept raw rather than resolved here because resolution can block on a
    /// domain controller; see [`crate::sid`].
    pub sid: Vec<u8>,
    /// FILETIME: 100 ns ticks since 1601-01-01 UTC.
    pub time_created: u64,
    pub exit_status: Option<u32>,
}

impl ProcEvent {
    /// Build an event from a flat property map.
    ///
    /// `kind` comes from *which subscription* delivered the object, not from
    /// the object, because the two trace classes are distinguished by their
    /// class name and share every property but `ExitStatus`.
    ///
    /// Property names are matched case-insensitively, with aliases, for two
    /// reasons. WMI's own casing is not stable (`docs/FINDINGS.md`), and the
    /// degraded fallback path feeds this the properties of a `Win32_Process`
    /// instance — `ProcessId`/`Name`/`SessionId` — where the trace classes say
    /// `ProcessID`/`ProcessName`/`SessionID`.
    pub fn from_map(kind: ProcKind, props: &HashMap<String, Variant>) -> ProcEvent {
        let time_created = match prop(props, &["TIME_CREATED"]).map(variant_to_u64) {
            Some(t) if t != 0 => t,
            // The intrinsic fallback has no `TIME_CREATED` on the instance, so
            // the process's own creation time stands in. It is a CIM_DATETIME,
            // i.e. a different epoch *and* a different representation, hence
            // the conversion rather than a cast.
            _ => prop(props, &["CreationDate"])
                .map(variant_to_string)
                .and_then(|s| cim_datetime_to_filetime(&s))
                .unwrap_or(0),
        };

        ProcEvent {
            kind,
            pid: prop(props, &["ProcessID"]).map(variant_to_u32).unwrap_or(0),
            parent_pid: prop(props, &["ParentProcessID"])
                .map(variant_to_u32)
                .unwrap_or(0),
            name: prop(props, &["ProcessName", "Name"])
                .map(variant_to_string)
                .unwrap_or_default(),
            session_id: prop(props, &["SessionID"]).map(variant_to_u32).unwrap_or(0),
            // A NULL or absent `Sid` yields an empty vector, never a panic:
            // system processes and some providers legitimately omit it.
            sid: prop(props, &["Sid"])
                .map(variant_to_bytes)
                .unwrap_or_default(),
            time_created,
            exit_status: prop(props, &["ExitStatus"]).and_then(|v| match v {
                Variant::Empty | Variant::Null => None,
                other => Some(variant_to_u32(other)),
            }),
        }
    }
}

/// Look a property up by any of `names`, exact match first, then
/// case-insensitively. Event objects carry under a dozen properties, so the
/// linear scan costs nothing worth optimizing away.
fn prop<'a>(props: &'a HashMap<String, Variant>, names: &[&str]) -> Option<&'a Variant> {
    for name in names {
        if let Some(v) = props.get(*name) {
            return Some(v);
        }
    }
    for name in names {
        for (k, v) in props {
            if k.eq_ignore_ascii_case(name) {
                return Some(v);
            }
        }
    }
    None
}

/// What a follow-up `Win32_Process` lookup managed to add.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct ProcInfo {
    pub command_line: String,
    pub executable_path: String,
}

/// The outcome of enrichment for one event.
///
/// Four states, not two, because "we asked and the answer was no", "we never
/// asked" and "we are still asking" lead to different UI -- the first is final,
/// the others are not, and a view whose job is telling you what ran must not
/// collapse them.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub enum Enrichment {
    /// A `Win32_Process` row answered and every identity check passed.
    Found(ProcInfo),
    /// The process was already gone, or the row that answered belonged to a
    /// different process that had reused the pid. Never a guess.
    Unavailable,
    /// Not attempted: a stop event (the process is gone by definition), or the
    /// enrichment queue was too deep to be worth joining.
    Skipped,
    /// Asked for, no answer yet.
    ///
    /// The default, so a freshly built row starts here rather than in a state
    /// that claims something was decided.
    ///
    /// Distinct from [`Enrichment::Skipped`] because a row that is *about* to
    /// have a command line and a row that will never have one look identical
    /// to a reader otherwise -- and in a view whose whole job is telling you
    /// what ran, "we haven't looked yet" and "we looked and could not see"
    /// are not the same claim.
    #[default]
    Pending,
}

/// The `Win32_Process` columns used to confirm identity and add detail.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcRow {
    pub command_line: String,
    pub executable_path: String,
    pub name: String,
    pub parent_pid: u32,
    /// `CreationDate` converted onto the event clock, or `None` when it was
    /// absent or unparseable.
    pub created_filetime: Option<u64>,
}

/// Best-effort `CommandLine` / `ExecutablePath` for a start event.
///
/// Racy by nature: the process may have exited before the query runs, and its
/// pid may already belong to something else. The guard in [`identity_matches`]
/// is what makes the race safe — a mismatch yields [`Enrichment::Unavailable`],
/// never a command line belonging to some other process. In a tool used to
/// answer "what ran on this box", a wrong command line is worse than none.
pub fn enrich_process(conn: &WMIConnection, ev: &ProcEvent) -> Enrichment {
    if ev.kind != ProcKind::Start {
        return Enrichment::Skipped;
    }
    let wql = format!(
        "SELECT CommandLine, ExecutablePath, CreationDate, ParentProcessId, Name \
         FROM Win32_Process WHERE ProcessId = {}",
        ev.pid
    );
    let rows: Vec<HashMap<String, Variant>> = match conn.raw_query(&wql) {
        Ok(r) => r,
        // A failed query is indistinguishable from a vanished process from the
        // caller's point of view, and neither is an error worth surfacing: an
        // instant-exit process is the common case, and it is exactly the case
        // this whole view exists to catch.
        Err(_) => return Enrichment::Unavailable,
    };

    for row in &rows {
        let candidate = row_from_map(row);
        if identity_matches(ev, &candidate) {
            return Enrichment::Found(ProcInfo {
                command_line: candidate.command_line,
                executable_path: candidate.executable_path,
            });
        }
    }
    Enrichment::Unavailable
}

/// Read a `Win32_Process` property map into the columns we check against.
pub fn row_from_map(row: &HashMap<String, Variant>) -> ProcRow {
    ProcRow {
        command_line: prop(row, &["CommandLine"])
            .map(variant_to_string)
            .unwrap_or_default(),
        executable_path: prop(row, &["ExecutablePath"])
            .map(variant_to_string)
            .unwrap_or_default(),
        name: prop(row, &["Name"])
            .map(variant_to_string)
            .unwrap_or_default(),
        parent_pid: prop(row, &["ParentProcessId"])
            .map(variant_to_u32)
            .unwrap_or(0),
        created_filetime: prop(row, &["CreationDate"])
            .map(variant_to_string)
            .filter(|s| !s.is_empty())
            .and_then(|s| cim_datetime_to_filetime(&s)),
    }
}

/// `DOMAIN\user` for a live process, via `Win32_Process.GetOwner`.
///
/// This is the *degraded* path's answer to a question the trace path answers
/// for free. `Win32_ProcessStartTrace` carries the owner's SID on the event
/// itself, with no race and no second call; the intrinsic `Win32_Process`
/// instance carries no owner at all, so the only route left is the one §9.2
/// criticizes — a follow-up `GetOwner` against a process that may already have
/// exited. It is used only where there is no alternative, and a miss returns
/// `None` rather than a guess.
///
/// No pid-reuse guard is applied here on purpose: unlike a command line, an
/// owner is a low-stakes attribution and the guard would need a second query to
/// establish the identity it was guarding. Callers pair it with an event whose
/// process was alive moments ago.
pub fn process_owner(conn: &WMIConnection, pid: u32) -> Option<String> {
    let path = format!("Win32_Process.Handle=\"{pid}\"");
    let out = conn.exec_method(&path, "GetOwner", None).ok()??;
    // A non-zero `ReturnValue` means the provider declined -- commonly for a
    // process running as another user, or one that exited mid-call.
    let rc = out
        .get_property("ReturnValue")
        .map(|v| variant_to_u32(&v))
        .unwrap_or(1);
    if rc != 0 {
        return None;
    }
    let get = |p: &str| {
        out.get_property(p)
            .map(|v| variant_to_string(&v))
            .unwrap_or_default()
    };
    let user = get("User");
    if user.is_empty() {
        return None;
    }
    let domain = get("Domain");
    Some(if domain.is_empty() {
        user
    } else {
        format!("{domain}\\{user}")
    })
}

/// Is `row` the same process the event described?
///
/// Three independent checks, all of which must hold:
///
/// * **parent pid** — free, and already wrong for most reuse cases;
/// * **image name** — case-insensitive, because WMI's casing is not stable;
/// * **creation time** — within [`CREATION_TOLERANCE_100NS`].
///
/// The third is the one that is easy to get wrong and easy not to notice. The
/// event's `TIME_CREATED` is a `uint64` FILETIME (100 ns ticks since 1601)
/// while `CreationDate` is a `CIM_DATETIME` string in local time with a UTC
/// offset. Comparing them without converting does not fail loudly — it compares
/// a 18-digit integer against a parse of `"20260801143000.123456+180"` and
/// quietly never matches, which reads as "enrichment does not work" rather than
/// as a bug. An unparseable or absent `CreationDate` is a failed check, not a
/// waived one.
pub fn identity_matches(ev: &ProcEvent, row: &ProcRow) -> bool {
    if row.parent_pid != ev.parent_pid {
        return false;
    }
    if !row.name.eq_ignore_ascii_case(&ev.name) {
        return false;
    }
    match row.created_filetime {
        Some(created) => created.abs_diff(ev.time_created) <= CREATION_TOLERANCE_100NS,
        None => false,
    }
}

/// Convert a `CIM_DATETIME` (`yyyymmddHHMMSS.ffffff±UUU`) to a FILETIME.
///
/// Returns `None` for anything malformed, including the `+***` "offset unknown"
/// form: without the offset the value cannot be placed on a UTC timeline, and a
/// timestamp that might be hours wrong is worse than no timestamp at all when
/// it is being used as an identity check.
pub fn cim_datetime_to_filetime(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    if b.len() < 22 || b[14] != b'.' {
        return None;
    }
    let num = |from: usize, to: usize| -> Option<i64> { s.get(from..to)?.parse::<i64>().ok() };

    let year = num(0, 4)?;
    let month = num(4, 6)?;
    let day = num(6, 8)?;
    let hour = num(8, 10)?;
    let minute = num(10, 12)?;
    let second = num(12, 14)?;
    let micros = num(15, 21)?;

    let sign = match b[21] {
        b'+' => 1i64,
        b'-' => -1i64,
        _ => return None,
    };
    // `***` means the provider does not know the offset. Rejected on purpose.
    let offset_minutes = num(22, s.len())?;

    if !(1601..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
        || !(0..=1440).contains(&offset_minutes)
    {
        return None;
    }

    let days = days_from_civil(year, month as u32, day as u32) + DAYS_1601_TO_1970;
    let local_secs = days * 86_400 + hour * 3600 + minute * 60 + second;
    let utc_secs = local_secs - sign * offset_minutes * 60;
    if utc_secs < 0 {
        return None;
    }
    Some(utc_secs as u64 * 10_000_000 + micros as u64 * 10)
}

/// Seconds since the Unix epoch for a FILETIME. For rendering only.
///
/// Saturates at zero: a FILETIME before 1970 is not a real process start time,
/// and a negative "seconds since the epoch" would render as a date in the
/// 1960s rather than as the missing value it actually is.
pub fn filetime_to_unix_secs(ft: u64) -> f64 {
    if ft <= FILETIME_UNIX_EPOCH {
        return 0.0;
    }
    (ft - FILETIME_UNIX_EPOCH) as f64 / 10_000_000.0
}

/// Days from 1970-01-01 to `y-m-d`, proleptic Gregorian.
///
/// Howard Hinnant's era-based `days_from_civil`. Written out rather than pulled
/// in as a dependency: this crate takes no dependency it does not need, and the
/// whole calendar problem here is one conversion in one direction.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = (m as i64 + 9) % 12; // March = 0
    let doy = (153 * mp + 2) / 5 + d as i64 - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic `Win32_ProcessStartTrace` property map, in the exact shapes
    /// WMI delivers: `uint32` as `VT_I4`, `uint64` as `VT_BSTR`, `uint8[]` as a
    /// `VT_ARRAY | VT_UI1`.
    fn start_map() -> HashMap<String, Variant> {
        HashMap::from([
            ("ProcessID".to_string(), Variant::I4(4242)),
            ("ParentProcessID".to_string(), Variant::I4(1000)),
            ("ProcessName".to_string(), Variant::String("cmd.exe".into())),
            ("SessionID".to_string(), Variant::I4(1)),
            (
                "Sid".to_string(),
                Variant::Array(vec![
                    Variant::UI1(1),
                    Variant::UI1(1),
                    Variant::UI1(0),
                    Variant::UI1(0),
                    Variant::UI1(0),
                    Variant::UI1(0),
                    Variant::UI1(0),
                    Variant::UI1(5),
                    Variant::UI1(18),
                    Variant::UI1(0),
                    Variant::UI1(0),
                    Variant::UI1(0),
                ]),
            ),
            (
                "TIME_CREATED".to_string(),
                Variant::String("133997760000000000".into()),
            ),
            (
                "SECURITY_DESCRIPTOR".to_string(),
                Variant::Null, // present and NULL, exactly as observed
            ),
        ])
    }

    #[test]
    fn a_start_trace_map_parses_from_flat_scalars() {
        let ev = ProcEvent::from_map(ProcKind::Start, &start_map());
        assert_eq!(ev.kind, ProcKind::Start);
        assert_eq!(ev.pid, 4242);
        assert_eq!(ev.parent_pid, 1000);
        assert_eq!(ev.name, "cmd.exe");
        assert_eq!(ev.session_id, 1);
        assert_eq!(ev.sid, vec![1u8, 1, 0, 0, 0, 0, 0, 5, 18, 0, 0, 0]);
        assert_eq!(ev.time_created, 133_997_760_000_000_000);
        // A start trace declares no `ExitStatus` at all.
        assert_eq!(ev.exit_status, None);
    }

    #[test]
    fn a_stop_trace_carries_an_exit_status() {
        let mut m = start_map();
        m.insert("ExitStatus".to_string(), Variant::I4(1));
        let ev = ProcEvent::from_map(ProcKind::Stop, &m);
        assert_eq!(ev.kind, ProcKind::Stop);
        assert_eq!(ev.exit_status, Some(1));
        // Zero is a real exit status, not an absent one.
        m.insert("ExitStatus".to_string(), Variant::I4(0));
        assert_eq!(
            ProcEvent::from_map(ProcKind::Stop, &m).exit_status,
            Some(0),
            "a clean exit must be Some(0), not None"
        );
    }

    #[test]
    fn a_missing_or_null_sid_is_empty_not_a_panic() {
        let mut m = start_map();
        m.remove("Sid");
        assert!(ProcEvent::from_map(ProcKind::Start, &m).sid.is_empty());

        m.insert("Sid".to_string(), Variant::Null);
        assert!(ProcEvent::from_map(ProcKind::Start, &m).sid.is_empty());

        m.insert("Sid".to_string(), Variant::Empty);
        assert!(ProcEvent::from_map(ProcKind::Start, &m).sid.is_empty());
    }

    #[test]
    fn an_empty_map_yields_zeroes_rather_than_failing() {
        let ev = ProcEvent::from_map(ProcKind::Start, &HashMap::new());
        assert_eq!(ev.pid, 0);
        assert_eq!(ev.name, "");
        assert_eq!(ev.time_created, 0);
        assert_eq!(ev.exit_status, None);
    }

    #[test]
    fn the_intrinsic_fallbacks_property_names_and_epoch_both_work() {
        // What the degraded path hands over: `Win32_Process` property names,
        // and a CIM_DATETIME where the trace class had a FILETIME.
        let m = HashMap::from([
            ("ProcessId".to_string(), Variant::I4(9)),
            ("ParentProcessId".to_string(), Variant::I4(4)),
            ("Name".to_string(), Variant::String("powershell.exe".into())),
            ("SessionId".to_string(), Variant::I4(1)),
            (
                "CreationDate".to_string(),
                Variant::String("19700101000000.000000+000".into()),
            ),
        ]);
        let ev = ProcEvent::from_map(ProcKind::Start, &m);
        assert_eq!(ev.pid, 9);
        assert_eq!(ev.parent_pid, 4);
        assert_eq!(ev.name, "powershell.exe");
        assert_eq!(ev.session_id, 1);
        assert_eq!(ev.time_created, FILETIME_UNIX_EPOCH);
    }

    #[test]
    fn property_names_are_matched_case_insensitively() {
        let m = HashMap::from([
            ("PROCESSID".to_string(), Variant::I4(11)),
            ("processname".to_string(), Variant::String("a.exe".into())),
        ]);
        let ev = ProcEvent::from_map(ProcKind::Start, &m);
        assert_eq!(ev.pid, 11);
        assert_eq!(ev.name, "a.exe");
    }

    #[test]
    fn cim_datetime_anchors_at_both_epochs() {
        assert_eq!(
            cim_datetime_to_filetime("16010101000000.000000+000"),
            Some(0)
        );
        assert_eq!(
            cim_datetime_to_filetime("19700101000000.000000+000"),
            Some(FILETIME_UNIX_EPOCH)
        );
    }

    #[test]
    fn cim_datetime_applies_the_utc_offset() {
        // 01:00 at UTC+60 is 00:00 UTC -- the same instant as the anchor above.
        assert_eq!(
            cim_datetime_to_filetime("19700101010000.000000+060"),
            Some(FILETIME_UNIX_EPOCH)
        );
        // 23:00 on the previous day at UTC-60 is also 00:00 UTC.
        assert_eq!(
            cim_datetime_to_filetime("19691231230000.000000-060"),
            Some(FILETIME_UNIX_EPOCH)
        );
    }

    #[test]
    fn cim_datetime_keeps_sub_second_precision() {
        // 1 microsecond = 10 FILETIME ticks.
        assert_eq!(
            cim_datetime_to_filetime("19700101000000.000001+000"),
            Some(FILETIME_UNIX_EPOCH + 10)
        );
        // A leap day, to prove the calendar arithmetic is real.
        assert_eq!(
            cim_datetime_to_filetime("20240229120000.000000+000"),
            cim_datetime_to_filetime("20240228120000.000000+000").map(|t| t + 864_000_000_000)
        );
    }

    #[test]
    fn cim_datetime_rejects_what_it_cannot_place_on_a_timeline() {
        assert_eq!(cim_datetime_to_filetime(""), None);
        assert_eq!(cim_datetime_to_filetime("20260801143000"), None);
        // The "offset unknown" form. Accepting it would invent an offset.
        assert_eq!(cim_datetime_to_filetime("20260801143000.000000+***"), None);
        assert_eq!(cim_datetime_to_filetime("20261301143000.000000+000"), None);
        assert_eq!(cim_datetime_to_filetime("nonsense-------.------+000"), None);
    }

    #[test]
    fn filetime_renders_as_unix_seconds() {
        assert_eq!(filetime_to_unix_secs(FILETIME_UNIX_EPOCH), 0.0);
        assert_eq!(filetime_to_unix_secs(FILETIME_UNIX_EPOCH + 10_000_000), 1.0);
        // Before the Unix epoch, and zero, both clamp rather than going negative.
        assert_eq!(filetime_to_unix_secs(0), 0.0);
    }

    fn event(pid: u32, ppid: u32, name: &str, ft: u64) -> ProcEvent {
        ProcEvent {
            kind: ProcKind::Start,
            pid,
            parent_pid: ppid,
            name: name.to_string(),
            session_id: 1,
            sid: Vec::new(),
            time_created: ft,
            exit_status: None,
        }
    }

    fn row(ppid: u32, name: &str, created: Option<u64>) -> ProcRow {
        ProcRow {
            command_line: "cmd.exe /c whoami".into(),
            executable_path: r"C:\Windows\System32\cmd.exe".into(),
            name: name.to_string(),
            parent_pid: ppid,
            created_filetime: created,
        }
    }

    #[test]
    fn identity_accepts_the_same_process() {
        let ev = event(4242, 1000, "cmd.exe", FILETIME_UNIX_EPOCH);
        assert!(identity_matches(
            &ev,
            &row(1000, "cmd.exe", Some(FILETIME_UNIX_EPOCH))
        ));
        // Casing varies across WMI providers, so it cannot be load-bearing.
        assert!(identity_matches(
            &ev,
            &row(1000, "CMD.EXE", Some(FILETIME_UNIX_EPOCH))
        ));
        // Within tolerance in both directions.
        assert!(identity_matches(
            &ev,
            &row(1000, "cmd.exe", Some(FILETIME_UNIX_EPOCH + 10_000_000))
        ));
        assert!(identity_matches(
            &ev,
            &row(1000, "cmd.exe", Some(FILETIME_UNIX_EPOCH - 10_000_000))
        ));
    }

    #[test]
    fn identity_rejects_a_pid_reused_by_something_else() {
        let ev = event(4242, 1000, "cmd.exe", FILETIME_UNIX_EPOCH);
        // Different parent.
        assert!(!identity_matches(
            &ev,
            &row(1001, "cmd.exe", Some(FILETIME_UNIX_EPOCH))
        ));
        // Different image.
        assert!(!identity_matches(
            &ev,
            &row(1000, "rundll32.exe", Some(FILETIME_UNIX_EPOCH))
        ));
        // Same parent and image, but minutes later: a genuine reuse.
        assert!(!identity_matches(
            &ev,
            &row(1000, "cmd.exe", Some(FILETIME_UNIX_EPOCH + 600_000_000_000))
        ));
    }

    #[test]
    fn identity_rejects_an_unverifiable_creation_time() {
        // The failure mode this guard exists for: a `CreationDate` that could
        // not be converted must not be treated as "close enough".
        let ev = event(4242, 1000, "cmd.exe", FILETIME_UNIX_EPOCH);
        assert!(!identity_matches(&ev, &row(1000, "cmd.exe", None)));
    }

    #[test]
    fn a_row_map_reads_through_the_same_conversion() {
        let m = HashMap::from([
            (
                "CommandLine".to_string(),
                Variant::String("cmd.exe /c exit".into()),
            ),
            (
                "ExecutablePath".to_string(),
                Variant::String(r"C:\Windows\System32\cmd.exe".into()),
            ),
            ("Name".to_string(), Variant::String("cmd.exe".into())),
            ("ParentProcessId".to_string(), Variant::I4(1000)),
            (
                "CreationDate".to_string(),
                Variant::String("19700101000000.000000+000".into()),
            ),
        ]);
        let r = row_from_map(&m);
        assert_eq!(r.parent_pid, 1000);
        assert_eq!(r.created_filetime, Some(FILETIME_UNIX_EPOCH));
        assert_eq!(r.command_line, "cmd.exe /c exit");
        assert!(identity_matches(
            &event(1, 1000, "cmd.exe", FILETIME_UNIX_EPOCH),
            &r
        ));
    }

    #[test]
    fn a_row_with_no_creation_date_cannot_be_confirmed() {
        let m = HashMap::from([("Name".to_string(), Variant::String("cmd.exe".into()))]);
        assert_eq!(row_from_map(&m).created_filetime, None);
    }
}
