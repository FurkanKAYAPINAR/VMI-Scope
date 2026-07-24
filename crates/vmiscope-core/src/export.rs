//! Export query results and the persistence report to CSV / JSON — the format
//! analysts drop into a ticket.

use crate::events::SubscriptionReport;
use crate::worker::QueryResult;

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
