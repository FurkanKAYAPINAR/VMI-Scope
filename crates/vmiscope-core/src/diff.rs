//! Snapshot diffing — compare a saved baseline against the current scan to
//! surface what *changed*. The DFIR question this answers: "which WMI event
//! subscriptions appeared (or changed) since my known-good baseline?"

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use crate::events::Subscription;
use crate::providers::ProviderInfo;
use crate::worker::QueryResult;

/// The delta between a baseline and the current set of subscriptions.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct SubscriptionDiff {
    /// Present now, absent in the baseline — the alert case.
    pub added: Vec<Subscription>,
    /// Present in the baseline, gone now.
    pub removed: Vec<Subscription>,
    /// Same identity, but the action / query / consumer type changed.
    pub changed: Vec<Subscription>,
    pub unchanged: usize,
}

impl SubscriptionDiff {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

/// Identity of a subscription for diffing: filter + consumer names.
fn key(s: &Subscription) -> (String, String) {
    (s.filter_name.clone(), s.consumer_name.clone())
}

fn materially_differs(a: &Subscription, b: &Subscription) -> bool {
    a.action != b.action || a.filter_query != b.filter_query || a.consumer_type != b.consumer_type
}

/// Diff `current` against a `baseline`.
pub fn diff_subscriptions(baseline: &[Subscription], current: &[Subscription]) -> SubscriptionDiff {
    let base: HashMap<(String, String), &Subscription> =
        baseline.iter().map(|s| (key(s), s)).collect();
    let cur_keys: HashMap<(String, String), ()> = current.iter().map(|s| (key(s), ())).collect();

    let mut diff = SubscriptionDiff::default();
    for c in current {
        match base.get(&key(c)) {
            None => diff.added.push(c.clone()),
            Some(b) if materially_differs(b, c) => diff.changed.push(c.clone()),
            Some(_) => diff.unchanged += 1,
        }
    }
    diff.removed = baseline
        .iter()
        .filter(|b| !cur_keys.contains_key(&key(b)))
        .cloned()
        .collect();
    diff
}

/// The delta between a baseline and the current provider set.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ProviderDiff {
    pub added: Vec<ProviderInfo>,
    pub removed: Vec<ProviderInfo>,
    /// Same provider, but something about how it is hosted changed.
    pub changed: Vec<ProviderInfo>,
    pub unchanged: usize,
}

impl ProviderDiff {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

fn prov_key(p: &ProviderInfo) -> (String, String) {
    (p.provider.clone(), p.namespace.clone())
}

/// Has anything about *how* this provider is hosted changed?
///
/// The PID is the noisy half — provider hosts idle out and respawn on their
/// own, so a baseline taken an hour ago disagrees about most PIDs and says
/// nothing by doing so. The three columns task 5.11 added are the quiet half
/// and the interesting one: `HostingModel` and `HostingSpecification` moving is
/// a provider re-registering itself with different isolation, and `User`
/// moving is one re-registering under a different account. Both are things a
/// baseline diff exists to notice, and neither was visible before the columns
/// were read.
fn prov_materially_differs(a: &ProviderInfo, b: &ProviderInfo) -> bool {
    a.host_pid != b.host_pid
        || a.host_process != b.host_process
        || a.hosting_model != b.hosting_model
        || a.hosting_specification != b.hosting_specification
        || a.user != b.user
}

/// Diff `current` providers against a `baseline`.
pub fn diff_providers(baseline: &[ProviderInfo], current: &[ProviderInfo]) -> ProviderDiff {
    let base: HashMap<(String, String), &ProviderInfo> =
        baseline.iter().map(|p| (prov_key(p), p)).collect();
    let cur_keys: HashMap<(String, String), ()> =
        current.iter().map(|p| (prov_key(p), ())).collect();

    let mut diff = ProviderDiff::default();
    for c in current {
        match base.get(&prov_key(c)) {
            None => diff.added.push(c.clone()),
            Some(b) if prov_materially_differs(b, c) => diff.changed.push(c.clone()),
            Some(_) => diff.unchanged += 1,
        }
    }
    diff.removed = baseline
        .iter()
        .filter(|b| !cur_keys.contains_key(&prov_key(b)))
        .cloned()
        .collect();
    diff
}

/// One row of a table snapshot as `column -> value`.
///
/// A map rather than the positional `Vec<String>` a [`QueryResult`] stores,
/// because a diff must survive the two sides disagreeing about which columns
/// exist — a projection that dropped a column, or two hosts on different Windows
/// builds. Alignment is by name, never by index.
pub type Row = BTreeMap<String, String>;

/// A row that exists on exactly one side of a diff.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct DiffRow {
    /// Identity of the row, one entry per key column in the caller's order.
    pub key: Vec<String>,
    /// Every column of the row, aligned by name.
    pub values: Row,
}

/// A row present on both sides whose non-ignored values differ.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct RowDelta {
    /// Identity of the row, shared by both sides (that is what matched them).
    pub key: Vec<String>,
    /// The `a`-side (baseline) row.
    pub a: Row,
    /// The `b`-side (current) row.
    pub b: Row,
    /// Columns whose value differs between `a` and `b`. Ignored columns never
    /// appear here; a column present on one side only counts as differing.
    pub changed_columns: Vec<String>,
}

/// The delta between two tabular snapshots, keyed on `key_cols`.
///
/// `added`/`removed` follow the input order of the side they came from (`b` for
/// added, `a` for removed); `changed` follows `b`'s order. Nothing here is
/// sorted, so a caller that wants a stable rendering order gets the one the
/// query already produced.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct TableDiff {
    /// In `b` but not `a` — appeared since the baseline.
    pub added: Vec<DiffRow>,
    /// In `a` but not `b` — gone since the baseline.
    pub removed: Vec<DiffRow>,
    /// On both sides, values differ (after ignores).
    pub changed: Vec<RowDelta>,
    /// On both sides, values identical (after ignores).
    pub unchanged: usize,
}

impl TableDiff {
    /// Did nothing move? (No adds, removes, or changes.)
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

/// Turn a [`QueryResult`]'s positional rows into name-aligned maps.
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

/// The key of one row: the value of each key column, missing columns as empty.
fn key_of(row: &Row, key_cols: &[String]) -> Vec<String> {
    key_cols
        .iter()
        .map(|c| row.get(c).cloned().unwrap_or_default())
        .collect()
}

/// Every column that differs between two rows, ignored columns excluded.
///
/// A column present on one side only counts as a difference: `Some(v)` versus
/// `None` is a real change (a property that stopped being returned), not a
/// non-event.
fn changed_columns(a: &Row, b: &Row, ignore: &BTreeSet<&str>) -> Vec<String> {
    let mut cols: BTreeSet<&String> = BTreeSet::new();
    cols.extend(a.keys());
    cols.extend(b.keys());
    cols.into_iter()
        .filter(|c| !ignore.contains(c.as_str()))
        .filter(|c| a.get(*c) != b.get(*c))
        .cloned()
        .collect()
}

/// Diff two tabular snapshots, matching rows by `key_cols` and disregarding
/// `ignore_cols` when deciding whether a matched row changed.
///
/// **Empty `key_cols` falls back to whole-row identity** — every non-ignored
/// column becomes part of the key. Keying on nothing would collapse every row
/// into a single bucket, which is never what a caller means; a caller with no
/// declared key wants "same row = same values", which is exactly this fallback.
///
/// **Duplicate keys are legal and handled deterministically.** A query need not
/// be keyed on a real WMI key, so two rows can share a key. Same-key rows are
/// paired by arrival order — the i-th `a` row with the i-th `b` row — and any
/// surplus on either side becomes an add or a remove. This never panics and
/// never silently drops a row.
pub fn diff_tables(
    a: &QueryResult,
    b: &QueryResult,
    key_cols: &[String],
    ignore_cols: &[String],
) -> TableDiff {
    let a_rows = row_maps(a);
    let b_rows = row_maps(b);
    let ignore: BTreeSet<&str> = ignore_cols.iter().map(String::as_str).collect();

    // The effective key: the caller's columns, or — when none were given — every
    // column either side has that is not ignored, i.e. whole-row identity.
    let effective_key: Vec<String> = if key_cols.is_empty() {
        let mut cols: BTreeSet<String> = BTreeSet::new();
        cols.extend(a.columns.iter().cloned());
        cols.extend(b.columns.iter().cloned());
        cols.into_iter()
            .filter(|c| !ignore.contains(c.as_str()))
            .collect()
    } else {
        key_cols.to_vec()
    };

    // Bucket the `a` rows by key, preserving arrival order within each bucket so
    // duplicate keys pair up positionally against `b`.
    let mut a_by_key: HashMap<Vec<String>, VecDeque<usize>> = HashMap::new();
    for (i, row) in a_rows.iter().enumerate() {
        a_by_key
            .entry(key_of(row, &effective_key))
            .or_default()
            .push_back(i);
    }

    let mut diff = TableDiff::default();
    let mut matched_a = vec![false; a_rows.len()];

    for b_row in &b_rows {
        let key = key_of(b_row, &effective_key);
        match a_by_key.get_mut(&key).and_then(VecDeque::pop_front) {
            Some(ai) => {
                matched_a[ai] = true;
                let changed = changed_columns(&a_rows[ai], b_row, &ignore);
                if changed.is_empty() {
                    diff.unchanged += 1;
                } else {
                    diff.changed.push(RowDelta {
                        key,
                        a: a_rows[ai].clone(),
                        b: b_row.clone(),
                        changed_columns: changed,
                    });
                }
            }
            None => diff.added.push(DiffRow {
                key,
                values: b_row.clone(),
            }),
        }
    }

    for (i, a_row) in a_rows.iter().enumerate() {
        if !matched_a[i] {
            diff.removed.push(DiffRow {
                key: key_of(a_row, &effective_key),
                values: a_row.clone(),
            });
        }
    }

    diff
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::Risk;

    // ---- diff_tables ----------------------------------------------------

    /// Build a snapshot from a header and rows, the way a real query result
    /// arrives: `columns` first, then each row aligned to it.
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

    fn cols(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn table_diff_classifies_added_removed_changed_unchanged() {
        // Keyed on Name. svc-a unchanged, svc-b changed State, svc-c only on the
        // baseline (removed), svc-d only on current (added).
        let a = table(
            &["Name", "State"],
            &[
                &["svc-a", "Running"],
                &["svc-b", "Running"],
                &["svc-c", "Stopped"],
            ],
        );
        let b = table(
            &["Name", "State"],
            &[
                &["svc-a", "Running"],
                &["svc-b", "Stopped"],
                &["svc-d", "Running"],
            ],
        );
        let d = diff_tables(&a, &b, &cols(&["Name"]), &[]);
        assert_eq!(d.unchanged, 1);
        assert_eq!(d.changed.len(), 1);
        assert_eq!(d.changed[0].key, vec!["svc-b".to_string()]);
        assert_eq!(d.changed[0].changed_columns, vec!["State".to_string()]);
        assert_eq!(d.added.len(), 1);
        assert_eq!(d.added[0].key, vec!["svc-d".to_string()]);
        assert_eq!(d.removed.len(), 1);
        assert_eq!(d.removed[0].key, vec!["svc-c".to_string()]);
        assert!(!d.is_empty());
    }

    #[test]
    fn table_diff_ignores_a_volatile_column() {
        // ProcessId flips on every read but is not a real change. With it
        // ignored, the row is unchanged; without, it is a false positive.
        let a = table(
            &["Name", "State", "ProcessId"],
            &[&["spooler", "Running", "1234"]],
        );
        let b = table(
            &["Name", "State", "ProcessId"],
            &[&["spooler", "Running", "5678"]],
        );

        let noisy = diff_tables(&a, &b, &cols(&["Name"]), &[]);
        assert_eq!(
            noisy.changed.len(),
            1,
            "PID churn must register without an ignore"
        );
        assert_eq!(
            noisy.changed[0].changed_columns,
            vec!["ProcessId".to_string()]
        );

        let quiet = diff_tables(&a, &b, &cols(&["Name"]), &cols(&["ProcessId"]));
        assert_eq!(quiet.changed.len(), 0);
        assert_eq!(quiet.unchanged, 1);
        assert!(quiet.is_empty());
    }

    #[test]
    fn table_diff_survives_differing_column_sets() {
        // `b` gained a Description column `a` never had. On the shared key the
        // rows match; the extra column is a per-row change (absent -> present),
        // not an add/remove.
        let a = table(&["Name", "State"], &[&["w32time", "Running"]]);
        let b = table(
            &["Name", "State", "Description"],
            &[&["w32time", "Running", "Windows Time"]],
        );
        let d = diff_tables(&a, &b, &cols(&["Name"]), &[]);
        assert_eq!(d.added.len(), 0);
        assert_eq!(d.removed.len(), 0);
        assert_eq!(d.changed.len(), 1);
        assert_eq!(
            d.changed[0].changed_columns,
            vec!["Description".to_string()]
        );
        // The B side carries the new column; the A side does not.
        assert_eq!(
            d.changed[0].b.get("Description").map(String::as_str),
            Some("Windows Time")
        );
        assert_eq!(d.changed[0].a.get("Description"), None);
    }

    #[test]
    fn table_diff_pairs_duplicate_keys_by_order() {
        // Two rows share the key "dup". They are paired positionally: first with
        // first (changed), second with second (unchanged). A third `a` duplicate
        // with no partner is a removal.
        let a = table(
            &["Name", "V"],
            &[&["dup", "1"], &["dup", "2"], &["dup", "3"]],
        );
        let b = table(&["Name", "V"], &[&["dup", "9"], &["dup", "2"]]);
        let d = diff_tables(&a, &b, &cols(&["Name"]), &[]);
        assert_eq!(d.changed.len(), 1, "first pair differs (1 vs 9)");
        assert_eq!(d.changed[0].a.get("V").map(String::as_str), Some("1"));
        assert_eq!(d.changed[0].b.get("V").map(String::as_str), Some("9"));
        assert_eq!(d.unchanged, 1, "second pair matches (2 vs 2)");
        assert_eq!(d.removed.len(), 1, "the third `a` duplicate has no partner");
        assert_eq!(d.removed[0].values.get("V").map(String::as_str), Some("3"));
        assert_eq!(d.added.len(), 0);
    }

    #[test]
    fn table_diff_empty_key_falls_back_to_whole_row() {
        // No key columns: identity is the whole (non-ignored) row. Identical rows
        // are unchanged; any value difference makes the row a removal + an add,
        // because with no key there is nothing to match a changed row against.
        let a = table(&["A", "B"], &[&["1", "x"], &["2", "y"]]);
        let b = table(&["A", "B"], &[&["1", "x"], &["2", "z"]]);
        let d = diff_tables(&a, &b, &[], &[]);
        assert_eq!(d.unchanged, 1, "the [1,x] row is byte-identical");
        assert_eq!(
            d.added.len(),
            1,
            "[2,z] has no [2,*] match under whole-row key"
        );
        assert_eq!(d.removed.len(), 1, "[2,y] is left unmatched");
        assert_eq!(d.changed.len(), 0);
    }

    #[test]
    fn table_diff_of_identical_snapshots_is_all_unchanged() {
        let t = table(&["Name", "State"], &[&["a", "Running"], &["b", "Stopped"]]);
        let d = diff_tables(&t, &t, &cols(&["Name"]), &[]);
        assert_eq!(d.unchanged, 2);
        assert!(d.is_empty());
    }

    #[test]
    fn table_diff_serializes_to_json() {
        // AC 6.4: a full compare result round-trips through JSON.
        let a = table(&["Name", "State"], &[&["svc", "Running"]]);
        let b = table(&["Name", "State"], &[&["svc", "Stopped"]]);
        let d = diff_tables(&a, &b, &cols(&["Name"]), &[]);
        let json = serde_json::to_string(&d).expect("TableDiff serializes");
        assert!(json.contains("\"changed\""));
        assert!(json.contains("\"changed_columns\""));
        assert!(json.contains("State"));
        let sub = SubscriptionDiff::default();
        let prov = ProviderDiff::default();
        assert!(serde_json::to_string(&sub).is_ok());
        assert!(serde_json::to_string(&prov).is_ok());
    }

    #[test]
    fn provider_diff_detects_host_change() {
        let base = vec![ProviderInfo {
            provider: "CIMWin32".into(),
            namespace: "root\\CIMV2".into(),
            host_pid: 100,
            host_process: "wmiprvse.exe".into(),
            ..Default::default()
        }];
        let current = vec![ProviderInfo {
            provider: "CIMWin32".into(),
            namespace: "root\\CIMV2".into(),
            host_pid: 200, // moved to a different host process
            host_process: "wmiprvse.exe".into(),
            ..Default::default()
        }];
        let d = diff_providers(&base, &current);
        assert_eq!(d.changed.len(), 1);
        assert_eq!(d.added.len(), 0);
        assert_eq!(d.removed.len(), 0);
    }

    /// The same provider, same host, but re-registered under an account and an
    /// isolation model it did not have before. Nothing the pre-5.11 diff read
    /// moved, so it reported "unchanged" for the only change worth an alert.
    #[test]
    fn provider_diff_notices_a_re_registration_that_never_moved_host() {
        let mut base = ProviderInfo {
            provider: "Evil".into(),
            namespace: "root\\CIMV2".into(),
            host_pid: 100,
            host_process: "wmiprvse.exe".into(),
            hosting_group: "DefaultNetworkServiceHost".into(),
            hosting_model: "NetworkServiceHost".into(),
            hosting_specification: 12,
            user: String::new(),
        };
        let unchanged = diff_providers(std::slice::from_ref(&base), std::slice::from_ref(&base));
        assert_eq!(unchanged.unchanged, 1);
        assert!(unchanged.is_empty());

        let mut current = base.clone();
        current.hosting_model = "Decoupled:Com".into();
        current.hosting_specification = 10;
        current.user = "HOST\\attacker".into();
        let d = diff_providers(std::slice::from_ref(&base), std::slice::from_ref(&current));
        assert_eq!(d.changed.len(), 1);
        assert_eq!(d.changed[0].user, "HOST\\attacker");

        // The account alone is enough.
        base.user = "HOST\\svc".into();
        let mut only_user = base.clone();
        only_user.user = "HOST\\attacker".into();
        let d = diff_providers(
            std::slice::from_ref(&base),
            std::slice::from_ref(&only_user),
        );
        assert_eq!(d.changed.len(), 1);
    }

    fn sub(filter: &str, consumer: &str, action: &str) -> Subscription {
        Subscription {
            filter_name: filter.into(),
            filter_query: "SELECT * FROM __InstanceCreationEvent".into(),
            consumer_type: "CommandLineEventConsumer".into(),
            consumer_name: consumer.into(),
            action: action.into(),
            risk: Risk::High,
            reasons: vec![],
            bound: true,
        }
    }

    #[test]
    fn detects_added_removed_changed_unchanged() {
        let baseline = vec![sub("F1", "C1", "cmd /c a"), sub("F2", "C2", "cmd /c b")];
        let current = vec![
            sub("F1", "C1", "cmd /c a"),        // unchanged
            sub("F2", "C2", "cmd /c EVIL"),     // changed action
            sub("F3", "C3", "powershell -enc"), // added
        ];
        let d = diff_subscriptions(&baseline, &current);
        assert_eq!(d.added.len(), 1);
        assert_eq!(d.added[0].filter_name, "F3");
        assert_eq!(d.changed.len(), 1);
        assert_eq!(d.changed[0].filter_name, "F2");
        assert_eq!(d.removed.len(), 0);
        assert_eq!(d.unchanged, 1);
        assert!(!d.is_empty());
    }

    #[test]
    fn identical_sets_are_empty_diff() {
        let s = vec![sub("F", "C", "x")];
        assert!(diff_subscriptions(&s, &s).is_empty());
    }

    #[test]
    fn subscription_removed_from_the_baseline_is_reported() {
        let baseline = vec![sub("F1", "C1", "cmd /c a"), sub("F2", "C2", "cmd /c b")];
        let current = vec![sub("F1", "C1", "cmd /c a")];
        let d = diff_subscriptions(&baseline, &current);
        assert_eq!(d.removed.len(), 1);
        assert_eq!(d.removed[0].filter_name, "F2");
        assert_eq!(d.added.len(), 0);
        assert_eq!(d.changed.len(), 0);
        assert_eq!(d.unchanged, 1);
        assert!(!d.is_empty());
    }

    #[test]
    fn subscription_diff_of_two_empty_sets_is_empty() {
        let d = diff_subscriptions(&[], &[]);
        assert!(d.is_empty());
        assert_eq!(d.unchanged, 0);
    }

    #[test]
    fn subscription_diff_against_an_empty_side() {
        let s = vec![sub("F1", "C1", "a"), sub("F2", "C2", "b")];
        // No baseline: everything is new.
        let fresh = diff_subscriptions(&[], &s);
        assert_eq!(fresh.added.len(), 2);
        assert_eq!(fresh.removed.len(), 0);
        assert_eq!(fresh.unchanged, 0);
        // Nothing left: everything is gone.
        let wiped = diff_subscriptions(&s, &[]);
        assert_eq!(wiped.added.len(), 0);
        assert_eq!(wiped.removed.len(), 2);
        assert_eq!(wiped.unchanged, 0);
    }

    #[test]
    fn subscription_diff_keys_on_filter_and_consumer_names_only() {
        // Same identity, different consumer type — a change, not an add/remove.
        let mut current = sub("F1", "C1", "cmd /c a");
        current.consumer_type = "ActiveScriptEventConsumer".into();
        let d = diff_subscriptions(&[sub("F1", "C1", "cmd /c a")], &[current]);
        assert_eq!(d.changed.len(), 1);
        assert_eq!(d.added.len(), 0);
        assert_eq!(d.removed.len(), 0);
    }

    #[test]
    fn subscription_diff_output_follows_the_input_order() {
        let baseline = vec![sub("B1", "C1", "a"), sub("B2", "C2", "b")];
        let current = vec![sub("N1", "X1", "a"), sub("N2", "X2", "b")];
        let d = diff_subscriptions(&baseline, &current);
        // `added` walks `current`, `removed` walks `baseline` — the HashMaps are
        // only ever looked up, never iterated, so both orders are stable.
        assert_eq!(
            d.added
                .iter()
                .map(|s| s.filter_name.as_str())
                .collect::<Vec<_>>(),
            ["N1", "N2"]
        );
        assert_eq!(
            d.removed
                .iter()
                .map(|s| s.filter_name.as_str())
                .collect::<Vec<_>>(),
            ["B1", "B2"]
        );
    }

    fn prov(provider: &str, pid: u32) -> ProviderInfo {
        ProviderInfo {
            provider: provider.into(),
            namespace: "root\\CIMV2".into(),
            host_pid: pid,
            host_process: "wmiprvse.exe".into(),
            ..Default::default()
        }
    }

    #[test]
    fn provider_diff_detects_added_removed_and_unchanged() {
        let baseline = vec![prov("CIMWin32", 100), prov("WinMgmt", 100)];
        let current = vec![
            prov("CIMWin32", 100), // unchanged
            prov("Nope", 300),     // added
        ];
        let d = diff_providers(&baseline, &current);
        assert_eq!(d.added.len(), 1);
        assert_eq!(d.added[0].provider, "Nope");
        assert_eq!(d.removed.len(), 1);
        assert_eq!(d.removed[0].provider, "WinMgmt");
        assert_eq!(d.changed.len(), 0);
        assert_eq!(d.unchanged, 1);
        assert!(!d.is_empty());
    }

    #[test]
    fn provider_diff_detects_a_host_process_rename() {
        let baseline = vec![prov("CIMWin32", 100)];
        let mut moved = prov("CIMWin32", 100);
        moved.host_process = "svchost.exe".into();
        let d = diff_providers(&baseline, &[moved]);
        assert_eq!(d.changed.len(), 1);
        assert_eq!(d.unchanged, 0);
    }

    #[test]
    fn provider_diff_of_two_empty_sets_is_empty() {
        let d = diff_providers(&[], &[]);
        assert!(d.is_empty());
        assert_eq!(d.unchanged, 0);
    }

    #[test]
    fn provider_diff_against_an_empty_side() {
        let p = vec![prov("CIMWin32", 100), prov("WinMgmt", 200)];
        let fresh = diff_providers(&[], &p);
        assert_eq!(fresh.added.len(), 2);
        assert_eq!(fresh.removed.len(), 0);
        let wiped = diff_providers(&p, &[]);
        assert_eq!(wiped.removed.len(), 2);
        assert_eq!(wiped.added.len(), 0);
    }

    #[test]
    fn provider_diff_keys_on_provider_and_namespace() {
        // Same provider name in a different namespace is a different provider.
        let mut other_ns = prov("CIMWin32", 100);
        other_ns.namespace = "root\\StandardCimv2".into();
        let d = diff_providers(&[prov("CIMWin32", 100)], &[other_ns]);
        assert_eq!(d.added.len(), 1);
        assert_eq!(d.removed.len(), 1);
        assert_eq!(d.unchanged, 0);
    }

    #[test]
    fn identical_provider_sets_are_empty_diff() {
        let p = vec![prov("CIMWin32", 100)];
        let d = diff_providers(&p, &p);
        assert!(d.is_empty());
        assert_eq!(d.unchanged, 1);
    }
}
