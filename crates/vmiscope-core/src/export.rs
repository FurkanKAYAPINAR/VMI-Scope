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

    #[test]
    fn csv_quotes_fields_with_commas_and_quotes() {
        let r = QueryResult {
            columns: vec!["Name".into(), "Note".into()],
            rows: vec![vec!["a,b".into(), "say \"hi\"".into()]],
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
        };
        let json = query_to_json(&r);
        assert!(json.contains("\"A\": \"1\""));
        assert!(json.trim_start().starts_with('['));
    }
}
