//! Export query results and the persistence report to CSV / JSON — the format
//! analysts drop into a ticket.

use crate::events::{Subscription, SubscriptionReport};
use crate::providers::ProviderInfo;
use crate::worker::QueryResult;

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

/// A query result as CSV (header row + one row per instance).
pub fn query_to_csv(r: &QueryResult) -> String {
    let mut out = csv_row(r.columns.iter().map(|c| csv_field(c)));
    for row in &r.rows {
        out.push_str(&csv_row(row.iter().map(|c| csv_field(c))));
    }
    out
}

/// A query result as a JSON array of `{column: value}` objects.
pub fn query_to_json(r: &QueryResult) -> String {
    let objs: Vec<serde_json::Map<String, serde_json::Value>> = r
        .rows
        .iter()
        .map(|row| {
            r.columns
                .iter()
                .zip(row)
                .map(|(c, v)| (c.clone(), serde_json::Value::String(v.clone())))
                .collect()
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
/// of objects.
pub fn events_to_json(events: &[Vec<(String, String)>]) -> String {
    let objs: Vec<serde_json::Map<String, serde_json::Value>> = events
        .iter()
        .map(|ev| {
            ev.iter()
                .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                .collect()
        })
        .collect();
    serde_json::to_string_pretty(&objs).unwrap_or_default()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A self-contained HTML report of the persistence scan — the artifact an
/// analyst attaches to a ticket (risk colours + MITRE ATT&CK T1546.003).
pub fn subscriptions_to_html(report: &SubscriptionReport) -> String {
    use crate::events::Risk;
    let color = |r: Risk| match r {
        Risk::High => "#f06464",
        Risk::Medium => "#e1b95a",
        Risk::Low => "#96a596",
    };
    let mut rows = String::new();
    for s in &report.subscriptions {
        rows.push_str(&format!(
            "<tr><td><span class=\"pill\" style=\"background:{c}\">{risk}</span></td>\
             <td>{ct}</td><td>{cn}</td><td>{fn_}</td><td><code>{fq}</code></td>\
             <td><code>{act}</code></td><td>{why}</td></tr>",
            c = color(s.risk),
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
.pill{{color:#fff;padding:2px 8px;border-radius:10px;font-weight:600;font-size:12px}}\
.counts span{{margin-right:14px;font-weight:600}}</style></head><body>\
<h1>WMI Persistence Report</h1>\
<p class=\"sub\">root\\subscription event subscriptions — MITRE ATT&amp;CK \
<a href=\"https://attack.mitre.org/techniques/T1546/003/\">T1546.003</a> — generated by VMI-Scope</p>\
<p class=\"counts\"><span style=\"color:{ch}\">{high} high</span>\
<span style=\"color:{cm}\">{med} medium</span><span style=\"color:{cl}\">{low} low</span></p>\
<table><thead><tr><th>Risk</th><th>Consumer type</th><th>Consumer</th><th>Filter</th>\
<th>Filter query</th><th>Action</th><th>Why</th></tr></thead><tbody>{rows}</tbody></table>\
</body></html>",
        ch = color(Risk::High),
        cm = color(Risk::Medium),
        cl = color(Risk::Low),
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
    const GOLDEN_HTML: &str = concat!(
        r##"<!doctype html><html><head><meta charset="utf-8">"##,
        r##"<title>VMI-Scope — WMI Persistence Report</title><style>"##,
        r##"body{font-family:Segoe UI,Arial,sans-serif;margin:24px;color:#1c1c1c}"##,
        r##"h1{margin:0 0 4px}.sub{color:#666;margin:0 0 16px}"##,
        r##"table{border-collapse:collapse;width:100%;font-size:13px}"##,
        r##"th,td{border:1px solid #ddd;padding:6px 8px;text-align:left;vertical-align:top}"##,
        r##"th{background:#f4f4f4}"##,
        r##"code{font-family:Consolas,monospace;font-size:12px;word-break:break-all}"##,
        r##".pill{color:#fff;padding:2px 8px;border-radius:10px;font-weight:600;font-size:12px}"##,
        r##".counts span{margin-right:14px;font-weight:600}</style></head><body>"##,
        r##"<h1>WMI Persistence Report</h1>"##,
        r##"<p class="sub">root\subscription event subscriptions — MITRE ATT&amp;CK "##,
        r##"<a href="https://attack.mitre.org/techniques/T1546/003/">T1546.003</a>"##,
        r##" — generated by VMI-Scope</p>"##,
        r##"<p class="counts"><span style="color:#f06464">1 high</span>"##,
        r##"<span style="color:#e1b95a">1 medium</span>"##,
        r##"<span style="color:#96a596">1 low</span></p>"##,
        r##"<table><thead><tr><th>Risk</th><th>Consumer type</th><th>Consumer</th>"##,
        r##"<th>Filter</th><th>Filter query</th><th>Action</th><th>Why</th></tr></thead><tbody>"##,
        r##"<tr><td><span class="pill" style="background:#f06464">High</span></td>"##,
        r##"<td>CommandLineEventConsumer</td><td>Updater</td><td>ProcessWatcher</td>"##,
        r##"<td><code>SELECT * FROM __InstanceCreationEvent WITHIN 60</code></td>"##,
        r##"<td><code>cmd.exe /c &quot;powershell -enc SQBFAFgA&quot; &amp; del %0</code></td>"##,
        r##"<td>CommandLineEventConsumer launches a process; action contains 'powershell'</td></tr>"##,
        r##"<tr><td><span class="pill" style="background:#e1b95a">Medium</span></td>"##,
        r##"<td>LogFileEventConsumer</td><td>StagedLogger</td><td></td><td><code></code></td>"##,
        r##"<td><code>C:\ProgramData\&lt;host&gt;\stage.log</code></td>"##,
        r##"<td>UNBOUND consumer (staged, no binding); no risky indicators</td></tr>"##,
        r##"<tr><td><span class="pill" style="background:#96a596">Low</span></td>"##,
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
        }
    }

    fn fixture_providers() -> Vec<ProviderInfo> {
        vec![
            ProviderInfo {
                provider: "CIMWin32".into(),
                namespace: "root\\CIMV2".into(),
                host_pid: 4212,
                host_process: "wmiprvse.exe".into(),
                hosting_group: "LocalSystemHost".into(),
            },
            ProviderInfo {
                provider: "MS_NT_EVENTLOG_PROVIDER".into(),
                namespace: "root\\CIMV2".into(),
                host_pid: 0,
                host_process: String::new(),
                hosting_group: String::new(),
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

    #[test]
    fn json_object_keys_are_alphabetical_not_in_column_order() {
        let r = QueryResult {
            columns: vec!["Zeta".into(), "Alpha".into()],
            rows: vec![vec!["z".into(), "a".into()]],
            ..Default::default()
        };
        let json = query_to_json(&r);
        // `serde_json::Map` is a `BTreeMap` here (the `preserve_order` feature
        // is off), so the objects come out key-sorted — unlike the CSV, the
        // JSON does *not* carry the column order.
        assert!(json.find("\"Alpha\"").unwrap() < json.find("\"Zeta\"").unwrap());
        assert!(json.contains("\"Alpha\": \"a\""));
        assert!(json.contains("\"Zeta\": \"z\""));
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

    #[test]
    fn subscriptions_html_names_the_attack_id_and_the_risk_colours() {
        let html = subscriptions_to_html(&fixture_report());
        assert!(html.contains("T1546.003"));
        assert!(html.contains("https://attack.mitre.org/techniques/T1546/003/"));
        // One pill per subscription, each in its risk colour.
        assert!(html.contains("<span class=\"pill\" style=\"background:#f06464\">High</span>"));
        assert!(html.contains("<span class=\"pill\" style=\"background:#e1b95a\">Medium</span>"));
        assert!(html.contains("<span class=\"pill\" style=\"background:#96a596\">Low</span>"));
        assert_eq!(html.matches("class=\"pill\"").count(), 3);
        assert!(html.contains("1 high"));
        assert!(html.contains("1 medium"));
        assert!(html.contains("1 low"));
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
