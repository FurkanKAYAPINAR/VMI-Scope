//! Export query results and the persistence report to CSV / JSON — the format
//! analysts drop into a ticket.

use crate::events::{Subscription, SubscriptionReport};
use crate::providers::ProviderInfo;
use crate::worker::QueryResult;
use serde::ser::{Serialize, SerializeMap, Serializer};
use std::borrow::Cow;

/// Providers as pretty JSON (for a baseline snapshot).
pub fn providers_to_json(providers: &[ProviderInfo]) -> String {
    serde_json::to_string_pretty(providers).unwrap_or_default()
}

/// Parse a provider snapshot back for baseline diffing.
pub fn providers_from_json(json: &str) -> anyhow::Result<Vec<ProviderInfo>> {
    Ok(serde_json::from_str(json)?)
}

/// Parse a saved subscription snapshot (the JSON written by
/// [`subscriptions_to_json`]) back into subscriptions, for baseline diffing.
pub fn subscriptions_from_json(json: &str) -> anyhow::Result<Vec<Subscription>> {
    Ok(serde_json::from_str(json)?)
}

/// Escape one CSV field (RFC 4180: quote if it contains `,`, `"`, or a newline).
fn csv_field(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn csv_row(cells: impl Iterator<Item = String>) -> String {
    let mut line = cells.collect::<Vec<_>>().join(",");
    line.push('\n');
    line
}

/// The header both query exporters write, which also fixes the width every row
/// is padded to.
///
/// [`QueryResult`] promises `rows` are aligned to `columns`, and
/// `worker::to_table` builds them that way, so a row of a different length is a
/// malformed result. An exporter is the wrong place to discover that: the file
/// has already left the process by the time anyone reads it, and the analyst
/// reading it is the one who pays for whatever was dropped on the way out. So
/// the table is squared off instead — the header grows to cover the longest
/// row, short rows are padded with empty cells — and every cell that existed
/// reaches the file, under a column of its own, in *both* formats.
///
/// Both exporters go through here for exactly that reason: CSV and JSON cannot
/// disagree about the shape of a table they do not measure separately.
fn export_header(r: &QueryResult) -> Vec<Cow<'_, str>> {
    let width = r
        .rows
        .iter()
        .map(Vec::len)
        .max()
        .unwrap_or(0)
        .max(r.columns.len());
    (0..width)
        .map(|i| match r.columns.get(i) {
            Some(c) => Cow::Borrowed(c.as_str()),
            // Reachable only for a malformed result. 1-based, so `column_9` is
            // the ninth column in the file rather than the tenth.
            None => Cow::Owned(format!("column_{}", i + 1)),
        })
        .collect()
}

/// One row's cells, padded out to `width`. Never truncates — `width` comes from
/// [`export_header`], which is at least as wide as the longest row.
fn export_cells(row: &[String], width: usize) -> impl Iterator<Item = &str> + '_ {
    (0..width).map(move |i| row.get(i).map(String::as_str).unwrap_or(""))
}

/// A JSON object written in the order its pairs were given, duplicates and all.
///
/// `serde_json::Map` is a `BTreeMap` here — the crate is built without
/// `preserve_order` — so collecting a row into one sorts the keys and silently
/// keeps just one cell per repeated column. Neither is acceptable in an export:
/// the CSV of the same table carries `QueryResult.columns` order and every
/// duplicate column, and two files describing one table differently is a bug
/// the analyst discovers, not the author.
///
/// Serialising the pairs straight into the map serializer keeps both properties
/// without touching the dependency graph. Turning on `serde_json/preserve_order`
/// was the other option and is worse twice over: Cargo unifies features, so it
/// would swap `Map` for an `IndexMap` in *every* crate in the build that shares
/// this `serde_json` (including downstream users of `vmiscope-core`), and an
/// `IndexMap` still collapses duplicate keys, so it fixes only half the bug.
///
/// Duplicate keys make this "SHOULD be unique" JSON (RFC 8259 §4) — most
/// parsers keep the last. That is a real cost, paid deliberately: the file
/// still contains every cell, and a reader that drops one can be told to look
/// at the CSV, whereas a cell this function never wrote is gone.
struct OrderedObject<'a>(Vec<(&'a str, &'a str)>);

impl Serialize for OrderedObject<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(self.0.len()))?;
        for (k, v) in &self.0 {
            map.serialize_entry(k, v)?;
        }
        map.end()
    }
}

/// A query result as CSV (header row + one row per instance).
pub fn query_to_csv(r: &QueryResult) -> String {
    let header = export_header(r);
    let mut out = csv_row(header.iter().map(|c| csv_field(c)));
    for row in &r.rows {
        out.push_str(&csv_row(export_cells(row, header.len()).map(csv_field)));
    }
    out
}

/// A query result as a JSON array of `{column: value}` objects, in the caller's
/// column order and with the same cells the CSV carries.
pub fn query_to_json(r: &QueryResult) -> String {
    let header = export_header(r);
    let objs: Vec<OrderedObject> = r
        .rows
        .iter()
        .map(|row| {
            OrderedObject(
                header
                    .iter()
                    .map(Cow::as_ref)
                    .zip(export_cells(row, header.len()))
                    .collect(),
            )
        })
        .collect();
    serde_json::to_string_pretty(&objs).unwrap_or_default()
}

/// The persistence report as CSV.
pub fn subscriptions_to_csv(report: &SubscriptionReport) -> String {
    let mut out = csv_row(
        [
            "risk",
            "consumer_type",
            "consumer_name",
            "filter_name",
            "filter_query",
            "action",
            "reasons",
        ]
        .into_iter()
        .map(|s| s.to_string()),
    );
    for s in &report.subscriptions {
        out.push_str(&csv_row(
            [
                s.risk.as_str().to_string(),
                s.consumer_type.clone(),
                s.consumer_name.clone(),
                s.filter_name.clone(),
                s.filter_query.clone(),
                s.action.clone(),
                s.reasons.join("; "),
            ]
            .into_iter()
            .map(|c| csv_field(&c)),
        ));
    }
    out
}

/// The persistence report as pretty JSON.
pub fn subscriptions_to_json(report: &SubscriptionReport) -> String {
    serde_json::to_string_pretty(&report.subscriptions).unwrap_or_default()
}

/// A captured event log (each event = `(field, value)` pairs) as a JSON array
/// of objects, each keeping the field order the event arrived in.
pub fn events_to_json(events: &[Vec<(String, String)>]) -> String {
    let objs: Vec<OrderedObject> = events
        .iter()
        .map(|ev| OrderedObject(ev.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect()))
        .collect();
    serde_json::to_string_pretty(&objs).unwrap_or_default()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The CSS class carrying a risk level's colour.
///
/// One class per level, and the colour itself lives in the stylesheet, because
/// the report used to paint the pills with an inline `style="background:#..."`.
/// An inline style is the highest-priority origin in the cascade short of
/// `!important`, so that report could not be re-themed by a stylesheet at all —
/// changing a risk colour, for any reason including contrast or a house palette,
/// meant editing Rust and rebuilding.
fn risk_class(r: crate::events::Risk) -> &'static str {
    use crate::events::Risk;
    match r {
        Risk::High => "risk-high",
        Risk::Medium => "risk-medium",
        Risk::Low => "risk-low",
    }
}

/// A self-contained HTML report of the persistence scan — the artifact an
/// analyst attaches to a ticket (risk colours + MITRE ATT&CK T1546.003).
///
/// Self-contained is a hard requirement, not a convenience: the file is emailed
/// and opened from wherever it lands, so it may not reference an external
/// stylesheet, font or image. The risk colours are therefore declared once, in
/// the single `<style>` block at the top, as a `--risk` custom property per
/// level — which both keeps each colour in exactly one place and leaves one
/// overridable hook for a reader who wants a different palette.
pub fn subscriptions_to_html(report: &SubscriptionReport) -> String {
    use crate::events::Risk;
    let mut rows = String::new();
    for s in &report.subscriptions {
        rows.push_str(&format!(
            "<tr><td><span class=\"pill {c}\">{risk}</span></td>\
             <td>{ct}</td><td>{cn}</td><td>{fn_}</td><td><code>{fq}</code></td>\
             <td><code>{act}</code></td><td>{why}</td></tr>",
            c = risk_class(s.risk),
            risk = s.risk.as_str(),
            ct = html_escape(&s.consumer_type),
            cn = html_escape(&s.consumer_name),
            fn_ = html_escape(&s.filter_name),
            fq = html_escape(&s.filter_query),
            act = html_escape(&s.action),
            why = html_escape(&s.reasons.join("; ")),
        ));
    }
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
<title>VMI-Scope — WMI Persistence Report</title><style>\
body{{font-family:Segoe UI,Arial,sans-serif;margin:24px;color:#1c1c1c}}\
h1{{margin:0 0 4px}}.sub{{color:#666;margin:0 0 16px}}\
table{{border-collapse:collapse;width:100%;font-size:13px}}\
th,td{{border:1px solid #ddd;padding:6px 8px;text-align:left;vertical-align:top}}\
th{{background:#f4f4f4}}code{{font-family:Consolas,monospace;font-size:12px;word-break:break-all}}\
.risk-high{{--risk:#f06464}}.risk-medium{{--risk:#e1b95a}}.risk-low{{--risk:#96a596}}\
.pill{{background:var(--risk);color:#fff;padding:2px 8px;border-radius:10px;font-weight:600;font-size:12px}}\
.counts span{{margin-right:14px;font-weight:600;color:var(--risk)}}</style></head><body>\
<h1>WMI Persistence Report</h1>\
<p class=\"sub\">root\\subscription event subscriptions — MITRE ATT&amp;CK \
<a href=\"https://attack.mitre.org/techniques/T1546/003/\">T1546.003</a> — generated by VMI-Scope</p>\
<p class=\"counts\"><span class=\"{ch}\">{high} high</span>\
<span class=\"{cm}\">{med} medium</span><span class=\"{cl}\">{low} low</span></p>\
<table><thead><tr><th>Risk</th><th>Consumer type</th><th>Consumer</th><th>Filter</th>\
<th>Filter query</th><th>Action</th><th>Why</th></tr></thead><tbody>{rows}</tbody></table>\
</body></html>",
        ch = risk_class(Risk::High),
        cm = risk_class(Risk::Medium),
        cl = risk_class(Risk::Low),
        high = report.count(Risk::High),
        med = report.count(Risk::Medium),
        low = report.count(Risk::Low),
        rows = rows,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::Risk;

    /// The exact document [`subscriptions_to_html`] must keep producing for
    /// [`fixture_report`]. Split only for line length — `concat!` joins the
    /// pieces back into one byte-identical string.
    ///
    /// CHANGED: this fixture was updated when the risk colours moved out of
    /// inline `style` attributes. Three edits, all of them presentation and
    /// none of them content — every string, the ATT&CK id and link, the counts
    /// and all three rows are the bytes they were:
    ///
    ///  1. the stylesheet gained `.risk-high|-medium|-low{--risk:#...}` and the
    ///     `.pill` / `.counts span` rules now read `var(--risk)`;
    ///  2. `<span style="color:#f06464">1 high</span>` became
    ///     `<span class="risk-high">1 high</span>`, and likewise for the other
    ///     two counts;
    ///  3. `<span class="pill" style="background:#f06464">` became
    ///     `<span class="pill risk-high">`, and likewise per row.
    ///
    /// The rendered colours are unchanged; what changed is that a stylesheet
    /// can now override them, which an inline style made impossible.
    const GOLDEN_HTML: &str = concat!(
        r##"<!doctype html><html><head><meta charset="utf-8">"##,
        r##"<title>VMI-Scope — WMI Persistence Report</title><style>"##,
        r##"body{font-family:Segoe UI,Arial,sans-serif;margin:24px;color:#1c1c1c}"##,
        r##"h1{margin:0 0 4px}.sub{color:#666;margin:0 0 16px}"##,
        r##"table{border-collapse:collapse;width:100%;font-size:13px}"##,
        r##"th,td{border:1px solid #ddd;padding:6px 8px;text-align:left;vertical-align:top}"##,
        r##"th{background:#f4f4f4}"##,
        r##"code{font-family:Consolas,monospace;font-size:12px;word-break:break-all}"##,
        r##".risk-high{--risk:#f06464}.risk-medium{--risk:#e1b95a}.risk-low{--risk:#96a596}"##,
        r##".pill{background:var(--risk);color:#fff;padding:2px 8px;border-radius:10px;"##,
        r##"font-weight:600;font-size:12px}"##,
        r##".counts span{margin-right:14px;font-weight:600;color:var(--risk)}</style></head><body>"##,
        r##"<h1>WMI Persistence Report</h1>"##,
        r##"<p class="sub">root\subscription event subscriptions — MITRE ATT&amp;CK "##,
        r##"<a href="https://attack.mitre.org/techniques/T1546/003/">T1546.003</a>"##,
        r##" — generated by VMI-Scope</p>"##,
        r##"<p class="counts"><span class="risk-high">1 high</span>"##,
        r##"<span class="risk-medium">1 medium</span>"##,
        r##"<span class="risk-low">1 low</span></p>"##,
        r##"<table><thead><tr><th>Risk</th><th>Consumer type</th><th>Consumer</th>"##,
        r##"<th>Filter</th><th>Filter query</th><th>Action</th><th>Why</th></tr></thead><tbody>"##,
        r##"<tr><td><span class="pill risk-high">High</span></td>"##,
        r##"<td>CommandLineEventConsumer</td><td>Updater</td><td>ProcessWatcher</td>"##,
        r##"<td><code>SELECT * FROM __InstanceCreationEvent WITHIN 60</code></td>"##,
        r##"<td><code>cmd.exe /c &quot;powershell -enc SQBFAFgA&quot; &amp; del %0</code></td>"##,
        r##"<td>CommandLineEventConsumer launches a process; action contains 'powershell'</td></tr>"##,
        r##"<tr><td><span class="pill risk-medium">Medium</span></td>"##,
        r##"<td>LogFileEventConsumer</td><td>StagedLogger</td><td></td><td><code></code></td>"##,
        r##"<td><code>C:\ProgramData\&lt;host&gt;\stage.log</code></td>"##,
        r##"<td>UNBOUND consumer (staged, no binding); no risky indicators</td></tr>"##,
        r##"<tr><td><span class="pill risk-low">Low</span></td>"##,
        r##"<td></td><td></td><td>LonelyFilter</td>"##,
        r##"<td><code>select * from MSFT_SCMEventLogEvent</code></td><td><code></code></td>"##,
        r##"<td>unbound filter (no binding)</td></tr>"##,
        r##"</tbody></table></body></html>"##,
    );

    /// A fixed three-row report covering the three shapes a scan produces: a
    /// bound High-risk CommandLine consumer, an orphan (unbound) consumer and
    /// an orphan filter. Also carries `&`, `"`, `<` and `>` so the escaping of
    /// every exporter is exercised by the same data.
    fn fixture_report() -> SubscriptionReport {
        SubscriptionReport {
            subscriptions: vec![
                Subscription {
                    filter_name: "ProcessWatcher".into(),
                    filter_query: "SELECT * FROM __InstanceCreationEvent WITHIN 60".into(),
                    consumer_type: "CommandLineEventConsumer".into(),
                    consumer_name: "Updater".into(),
                    action: "cmd.exe /c \"powershell -enc SQBFAFgA\" & del %0".into(),
                    risk: Risk::High,
                    reasons: vec![
                        "CommandLineEventConsumer launches a process".into(),
                        "action contains 'powershell'".into(),
                    ],
                    bound: true,
                },
                Subscription {
                    filter_name: String::new(),
                    filter_query: String::new(),
                    consumer_type: "LogFileEventConsumer".into(),
                    consumer_name: "StagedLogger".into(),
                    action: "C:\\ProgramData\\<host>\\stage.log".into(),
                    risk: Risk::Medium,
                    reasons: vec![
                        "UNBOUND consumer (staged, no binding)".into(),
                        "no risky indicators".into(),
                    ],
                    bound: false,
                },
                Subscription {
                    filter_name: "LonelyFilter".into(),
                    filter_query: "select * from MSFT_SCMEventLogEvent".into(),
                    consumer_type: String::new(),
                    consumer_name: String::new(),
                    action: String::new(),
                    risk: Risk::Low,
                    reasons: vec!["unbound filter (no binding)".into()],
                    bound: false,
                },
            ],
            ..Default::default()
        }
    }

    fn fixture_providers() -> Vec<ProviderInfo> {
        vec![
            ProviderInfo {
                provider: "CIMWin32".into(),
                namespace: "root\\CIMV2".into(),
                host_pid: 4212,
                host_process: "wmiprvse.exe".into(),
                hosting_group: "DefaultNetworkServiceHost".into(),
                hosting_model: "NetworkServiceHost".into(),
                hosting_specification: 12,
                user: String::new(),
            },
            ProviderInfo {
                provider: "MS_NT_EVENTLOG_PROVIDER".into(),
                namespace: "root\\CIMV2".into(),
                host_pid: 0,
                host_process: String::new(),
                hosting_group: String::new(),
                hosting_model: String::new(),
                hosting_specification: 0,
                user: String::new(),
            },
        ]
    }

    #[test]
    fn csv_quotes_fields_with_commas_and_quotes() {
        let r = QueryResult {
            columns: vec!["Name".into(), "Note".into()],
            rows: vec![vec!["a,b".into(), "say \"hi\"".into()]],
            ..Default::default()
        };
        let csv = query_to_csv(&r);
        assert!(csv.starts_with("Name,Note\n"));
        assert!(csv.contains("\"a,b\""));
        assert!(csv.contains("\"say \"\"hi\"\"\""));
    }

    #[test]
    fn html_escapes_and_includes_attack_id() {
        use crate::events::{Risk, Subscription};
        let report = SubscriptionReport {
            subscriptions: vec![Subscription {
                filter_name: "F".into(),
                filter_query: "SELECT * FROM x".into(),
                consumer_type: "CommandLineEventConsumer".into(),
                consumer_name: "<evil>".into(),
                action: "cmd & run".into(),
                risk: Risk::High,
                reasons: vec!["bad".into()],
                bound: true,
            }],
            ..Default::default()
        };
        let html = subscriptions_to_html(&report);
        assert!(html.contains("T1546.003"));
        assert!(html.contains("&lt;evil&gt;")); // escaped
        assert!(html.contains("cmd &amp; run"));
        assert!(html.contains("1 high"));
    }

    #[test]
    fn json_is_array_of_objects() {
        let r = QueryResult {
            columns: vec!["A".into()],
            rows: vec![vec!["1".into()], vec!["2".into()]],
            ..Default::default()
        };
        let json = query_to_json(&r);
        assert!(json.contains("\"A\": \"1\""));
        assert!(json.trim_start().starts_with('['));
    }

    #[test]
    fn csv_quotes_fields_containing_newlines() {
        let r = QueryResult {
            columns: vec!["Desc".into()],
            rows: vec![vec!["line one\nline two".into()], vec!["cr\r\nlf".into()]],
            ..Default::default()
        };
        assert_eq!(
            query_to_csv(&r),
            "Desc\n\"line one\nline two\"\n\"cr\r\nlf\"\n"
        );
    }

    #[test]
    fn csv_keeps_the_column_order_it_was_given() {
        let r = QueryResult {
            columns: vec!["Zeta".into(), "Alpha".into(), "Mu".into()],
            rows: vec![vec!["z".into(), "a".into(), "m".into()]],
            ..Default::default()
        };
        assert_eq!(query_to_csv(&r), "Zeta,Alpha,Mu\nz,a,m\n");
    }

    #[test]
    fn empty_result_is_a_bare_header_and_an_empty_array() {
        let header_only = QueryResult {
            columns: vec!["Name".into(), "ProcessId".into()],
            rows: vec![],
            ..Default::default()
        };
        assert_eq!(query_to_csv(&header_only), "Name,ProcessId\n");
        assert_eq!(query_to_json(&header_only), "[]");

        // No columns either: CSV still emits the (empty) header line.
        let nothing = QueryResult::default();
        assert_eq!(query_to_csv(&nothing), "\n");
        assert_eq!(query_to_json(&nothing), "[]");
    }

    /// CHANGED (new): `query_to_json` used to `zip` the columns with the row,
    /// so a row longer than the header was clipped to the header and a row
    /// shorter than the header dropped the trailing columns — while
    /// `query_to_csv` emitted whatever the row held. Both now square the table
    /// off against the longest row, so no cell is dropped by either and a
    /// spreadsheet still sees one field per header column.
    #[test]
    fn a_row_longer_than_the_header_keeps_its_extra_cells() {
        let r = QueryResult {
            columns: vec!["A".into(), "B".into()],
            rows: vec![vec!["1".into(), "2".into(), "3".into()]],
            ..Default::default()
        };
        // The extra cell gets a synthesised, 1-based column name rather than
        // landing under no header at all.
        assert_eq!(query_to_csv(&r), "A,B,column_3\n1,2,3\n");
        let json = query_to_json(&r);
        assert!(json.contains("\"A\": \"1\""));
        assert!(json.contains("\"B\": \"2\""));
        assert!(json.contains("\"column_3\": \"3\""));
    }

    #[test]
    fn a_row_shorter_than_the_header_is_padded_not_truncated() {
        let r = QueryResult {
            columns: vec!["A".into(), "B".into(), "C".into()],
            rows: vec![vec!["1".into()]],
            ..Default::default()
        };
        assert_eq!(query_to_csv(&r), "A,B,C\n1,,\n");
        let json = query_to_json(&r);
        assert!(json.contains("\"A\": \"1\""));
        assert!(json.contains("\"B\": \"\""));
        assert!(json.contains("\"C\": \"\""));
    }

    /// The guard against the two exporters drifting apart again. `query_to_csv`
    /// and `query_to_json` describe the same table, so parsing both back must
    /// yield the same header and the same cells in the same order — whichever
    /// of the two someone edits next.
    ///
    /// The fixtures deliberately avoid `,`, `"` and newlines so `split(',')` is
    /// a correct CSV parse here; quoting has its own tests above.
    #[test]
    fn csv_and_json_describe_the_same_table() {
        let tables = [
            // Well-formed.
            QueryResult {
                columns: vec!["Zeta".into(), "Alpha".into()],
                rows: vec![vec!["z".into(), "a".into()], vec!["z2".into(), "a2".into()]],
                ..Default::default()
            },
            // Row longer than the header.
            QueryResult {
                columns: vec!["A".into()],
                rows: vec![vec!["1".into(), "over".into()]],
                ..Default::default()
            },
            // Row shorter than the header.
            QueryResult {
                columns: vec!["A".into(), "B".into()],
                rows: vec![vec!["1".into()]],
                ..Default::default()
            },
            // Both, in one table.
            QueryResult {
                columns: vec!["A".into(), "B".into()],
                rows: vec![vec!["1".into()], vec!["1".into(), "2".into(), "3".into()]],
                ..Default::default()
            },
        ];

        for (i, r) in tables.iter().enumerate() {
            let csv = query_to_csv(r);
            let mut lines = csv.lines();
            let header: Vec<String> = lines
                .next()
                .expect("a header line")
                .split(',')
                .map(str::to_string)
                .collect();
            let csv_rows: Vec<Vec<String>> = lines
                .map(|l| l.split(',').map(str::to_string).collect())
                .collect();

            let objs = read_back_in_written_order(&query_to_json(r));
            assert_eq!(objs.len(), csv_rows.len(), "table {i}: row count");
            for (obj, csv_row) in objs.iter().zip(&csv_rows) {
                let keys: Vec<String> = obj.iter().map(|(k, _)| k.clone()).collect();
                let values: Vec<String> = obj.iter().map(|(_, v)| v.clone()).collect();
                assert_eq!(keys, header, "table {i}: JSON keys vs CSV header");
                assert_eq!(&values, csv_row, "table {i}: JSON values vs CSV row");
            }
        }
    }

    /// Read a pretty-printed JSON array of flat string objects back *in written
    /// order*, which `serde_json::from_str` cannot do: `Value`'s map is a
    /// `BTreeMap` here, so parsing re-sorts the keys and collapses duplicates —
    /// hiding exactly what these tests exist to check.
    ///
    /// Only handles the shape these exporters emit: one `"key": "value"` per
    /// line, no nesting, and no `": "` or `"` inside a key or value. The
    /// fixtures that use it are written to stay inside that.
    fn read_back_in_written_order(json: &str) -> Vec<Vec<(String, String)>> {
        let mut out: Vec<Vec<(String, String)>> = Vec::new();
        let mut open: Option<Vec<(String, String)>> = None;
        for line in json.lines() {
            let t = line.trim().trim_end_matches(',');
            match t {
                "[" | "]" => {}
                "{}" => out.push(Vec::new()),
                "{" => open = Some(Vec::new()),
                "}" => out.push(open.take().expect("an object is open")),
                _ => {
                    let (k, v) = t.split_once(": ").expect("a \"key\": \"value\" line");
                    open.as_mut().expect("an object is open").push((
                        k.trim_matches('"').to_string(),
                        v.trim_matches('"').to_string(),
                    ));
                }
            }
        }
        assert!(open.is_none(), "an object was left open");
        out
    }

    /// CHANGED: this test used to be `json_object_keys_are_alphabetical_not_in
    /// _column_order` and pinned the opposite assertion — that the objects came
    /// out key-sorted because `serde_json::Map` is a `BTreeMap` without the
    /// `preserve_order` feature. That was the bug, pinned rather than fixed
    /// while Phase 0 was meant to be behaviour-neutral: the CSV of the same
    /// table carried `columns` order, so the two exports of one result
    /// disagreed on layout. Rows are now serialised as ordered pairs.
    #[test]
    fn json_object_keys_follow_the_column_order() {
        let r = QueryResult {
            columns: vec!["Zeta".into(), "Alpha".into()],
            rows: vec![vec!["z".into(), "a".into()]],
            ..Default::default()
        };
        let json = query_to_json(&r);
        assert!(json.find("\"Zeta\"").unwrap() < json.find("\"Alpha\"").unwrap());
        assert!(json.contains("\"Alpha\": \"a\""));
        assert!(json.contains("\"Zeta\": \"z\""));
        // The header the CSV writes is the key order the JSON writes.
        assert!(query_to_csv(&r).starts_with("Zeta,Alpha\n"));
    }

    /// CHANGED (new): events had the same key-sorting bug as query rows —
    /// `Class` sorted ahead of `ProcessId` no matter which order the event
    /// carried. An event log is a chronological record of fields as they
    /// arrived; re-alphabetising it loses the provider's own ordering.
    #[test]
    fn events_json_keeps_the_field_order_the_event_carried() {
        let events = vec![vec![
            ("ProcessId".to_string(), "4321".to_string()),
            ("Class".to_string(), "Win32_Process".to_string()),
            ("Arrived".to_string(), "12:00:01".to_string()),
        ]];
        let json = events_to_json(&events);
        let at = |k: &str| json.find(&format!("\"{k}\"")).expect("key present");
        assert!(at("ProcessId") < at("Class"));
        assert!(at("Class") < at("Arrived"));
    }

    /// A duplicate column name is malformed input — `worker::to_table` dedupes
    /// through a `BTreeSet` and cannot produce one — but the CSV emits both
    /// cells, so the JSON must too. Collecting into a `serde_json::Map` kept
    /// only the last, which is a cell deleted from a file an analyst reads.
    ///
    /// The duplicate key makes this "SHOULD be unique" JSON (RFC 8259 §4), so
    /// the assertion is deliberately on the *text*: a `serde_json::Value` parsed
    /// back would collapse the pair again and prove nothing.
    #[test]
    fn json_keeps_every_cell_when_two_columns_share_a_name() {
        let r = QueryResult {
            columns: vec!["Name".into(), "Name".into()],
            rows: vec![vec!["first".into(), "second".into()]],
            ..Default::default()
        };
        let json = query_to_json(&r);
        assert_eq!(json.matches("\"Name\"").count(), 2);
        assert!(json.contains("\"Name\": \"first\""));
        assert!(json.contains("\"Name\": \"second\""));
        assert_eq!(query_to_csv(&r), "Name,Name\nfirst,second\n");
    }

    #[test]
    fn subscriptions_csv_has_a_fixed_header_and_one_row_each() {
        let csv = subscriptions_to_csv(&fixture_report());
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 4);
        assert_eq!(
            lines[0],
            "risk,consumer_type,consumer_name,filter_name,filter_query,action,reasons"
        );
        // The action carries a double quote, so the field is quoted and the
        // quotes are doubled; the reasons are joined with "; ".
        assert_eq!(
            lines[1],
            "High,CommandLineEventConsumer,Updater,ProcessWatcher,\
             SELECT * FROM __InstanceCreationEvent WITHIN 60,\
             \"cmd.exe /c \"\"powershell -enc SQBFAFgA\"\" & del %0\",\
             CommandLineEventConsumer launches a process; action contains 'powershell'"
        );
        // Orphan consumer: no filter columns, and a reason list that carries a
        // comma, so that field is quoted too.
        assert_eq!(
            lines[2],
            "Medium,LogFileEventConsumer,StagedLogger,,,C:\\ProgramData\\<host>\\stage.log,\
             \"UNBOUND consumer (staged, no binding); no risky indicators\""
        );
        // Orphan filter: no consumer columns and no action.
        assert_eq!(
            lines[3],
            "Low,,,LonelyFilter,select * from MSFT_SCMEventLogEvent,,unbound filter (no binding)"
        );
    }

    /// CHANGED: this used to assert the pills carried
    /// `style="background:#f06464"` and the count spans `style="color:#..."`.
    /// The colours moved into the one `<style>` block as a `--risk` custom
    /// property per level, so risk now travels as a class and the report can be
    /// restyled without rebuilding. The rendered colours are the same three.
    #[test]
    fn subscriptions_html_names_the_attack_id_and_the_risk_classes() {
        let html = subscriptions_to_html(&fixture_report());
        assert!(html.contains("T1546.003"));
        assert!(html.contains("https://attack.mitre.org/techniques/T1546/003/"));
        // One pill per subscription, each carrying its risk level's class.
        assert!(html.contains("<span class=\"pill risk-high\">High</span>"));
        assert!(html.contains("<span class=\"pill risk-medium\">Medium</span>"));
        assert!(html.contains("<span class=\"pill risk-low\">Low</span>"));
        assert_eq!(html.matches("class=\"pill ").count(), 3);
        assert!(html.contains("<span class=\"risk-high\">1 high</span>"));
        assert!(html.contains("<span class=\"risk-medium\">1 medium</span>"));
        assert!(html.contains("<span class=\"risk-low\">1 low</span>"));
        // Each colour is declared exactly once, in the stylesheet.
        for (class, hex) in [
            ("risk-high", "#f06464"),
            ("risk-medium", "#e1b95a"),
            ("risk-low", "#96a596"),
        ] {
            assert!(html.contains(&format!(".{class}{{--risk:{hex}}}")));
            assert_eq!(
                html.matches(hex).count(),
                1,
                "{hex} declared more than once"
            );
        }
        // Nothing paints a risk colour inline any more, or the stylesheet could
        // not override it.
        assert!(!html.contains("style=\""));
        // Self-contained: no external stylesheet, script, font or image.
        assert!(!html.contains("<link"));
        assert!(!html.contains("<script"));
        assert!(!html.contains("<img"));
        // Escaping: none of the fixture's markup characters survive raw.
        assert!(html.contains("&quot;powershell -enc SQBFAFgA&quot; &amp; del %0"));
        assert!(html.contains("C:\\ProgramData\\&lt;host&gt;\\stage.log"));
    }

    #[test]
    fn subscriptions_html_matches_the_golden_report() {
        assert_eq!(subscriptions_to_html(&fixture_report()), GOLDEN_HTML);
    }

    #[test]
    fn subscriptions_json_round_trips_losslessly() {
        let report = fixture_report();
        let json = subscriptions_to_json(&report);
        let back = subscriptions_from_json(&json).expect("the snapshot parses back");
        assert_eq!(back.len(), report.subscriptions.len());
        // `Subscription` has no `PartialEq`, so re-serialising is the all-field
        // comparison: equal JSON means every field survived the round trip.
        let again = subscriptions_to_json(&SubscriptionReport {
            subscriptions: back,
            ..Default::default()
        });
        assert_eq!(again, json);
    }

    #[test]
    fn subscriptions_json_keeps_risk_and_orphan_flags() {
        let json = subscriptions_to_json(&fixture_report());
        let back = subscriptions_from_json(&json).expect("the snapshot parses back");
        assert_eq!(back[0].risk, Risk::High);
        assert!(back[0].bound);
        assert_eq!(back[1].risk, Risk::Medium);
        assert!(!back[1].bound); // orphan consumer
        assert!(back[1].filter_name.is_empty());
        assert_eq!(back[2].risk, Risk::Low);
        assert!(!back[2].bound); // orphan filter
        assert!(back[2].consumer_name.is_empty());
    }

    #[test]
    fn subscriptions_json_rejects_junk() {
        assert!(subscriptions_from_json("not json").is_err());
        assert!(subscriptions_from_json("{}").is_err());
    }

    #[test]
    fn providers_json_round_trips_losslessly() {
        let providers = fixture_providers();
        let json = providers_to_json(&providers);
        assert!(json.trim_start().starts_with('['));
        assert!(json.contains("\"provider\": \"CIMWin32\""));
        assert!(json.contains("\"host_pid\": 4212"));

        let back = providers_from_json(&json).expect("the baseline parses back");
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].provider, "CIMWin32");
        assert_eq!(back[0].namespace, "root\\CIMV2");
        assert_eq!(back[0].host_pid, 4212);
        assert_eq!(back[1].host_pid, 0);
        assert!(back[1].host_process.is_empty());
        // `ProviderInfo` has no `PartialEq` either — same trick as above.
        assert_eq!(providers_to_json(&back), json);
    }

    /// A baseline written before task 5.11 widened `ProviderInfo` has none of
    /// the new keys. It must still load, or every saved baseline on disk turns
    /// into a parse error the day the struct grows — which is the failure mode
    /// `#[serde(default)]` on the new fields exists to prevent.
    #[test]
    fn a_pre_5_11_provider_baseline_still_loads() {
        let old = r#"[
          {
            "provider": "CIMWin32",
            "namespace": "root\\CIMV2",
            "host_pid": 4212,
            "host_process": "wmiprvse.exe",
            "hosting_group": "DefaultNetworkServiceHost"
          }
        ]"#;
        let back = providers_from_json(old).expect("an old baseline parses");
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].provider, "CIMWin32");
        // Absent, not invented.
        assert!(back[0].hosting_model.is_empty());
        assert!(back[0].user.is_empty());
        assert_eq!(back[0].hosting_specification, 0);
    }

    #[test]
    fn providers_json_handles_the_empty_snapshot_and_junk() {
        assert_eq!(providers_to_json(&[]), "[]");
        assert!(providers_from_json("[]").expect("empty parses").is_empty());
        assert!(providers_from_json("nope").is_err());
    }

    #[test]
    fn events_json_is_one_object_per_event() {
        let events = vec![
            vec![
                ("Class".to_string(), "Win32_Process".to_string()),
                ("Handle".to_string(), "4321".to_string()),
            ],
            vec![("Class".to_string(), "Win32_Service".to_string())],
        ];
        let json = events_to_json(&events);
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let arr = parsed.as_array().expect("an array");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["Class"], "Win32_Process");
        assert_eq!(arr[0]["Handle"], "4321");
        // Events are ragged: the second one only has the field it carried.
        assert_eq!(arr[1].as_object().expect("an object").len(), 1);
        assert_eq!(events_to_json(&[]), "[]");
    }
}
