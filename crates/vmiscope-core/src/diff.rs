//! Snapshot diffing — compare a saved baseline against the current scan to
//! surface what *changed*. The DFIR question this answers: "which WMI event
//! subscriptions appeared (or changed) since my known-good baseline?"

use std::collections::HashMap;

use crate::events::Subscription;
use crate::providers::ProviderInfo;

/// The delta between a baseline and the current set of subscriptions.
#[derive(Debug, Clone, Default)]
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
#[derive(Debug, Clone, Default)]
pub struct ProviderDiff {
    pub added: Vec<ProviderInfo>,
    pub removed: Vec<ProviderInfo>,
    /// Same provider, but host PID / host process changed.
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
            Some(b) if b.host_pid != c.host_pid || b.host_process != c.host_process => {
                diff.changed.push(c.clone())
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::Risk;

    #[test]
    fn provider_diff_detects_host_change() {
        let base = vec![ProviderInfo {
            provider: "CIMWin32".into(),
            namespace: "root\\CIMV2".into(),
            host_pid: 100,
            host_process: "wmiprvse.exe".into(),
            hosting_group: String::new(),
        }];
        let current = vec![ProviderInfo {
            provider: "CIMWin32".into(),
            namespace: "root\\CIMV2".into(),
            host_pid: 200, // moved to a different host process
            host_process: "wmiprvse.exe".into(),
            hosting_group: String::new(),
        }];
        let d = diff_providers(&base, &current);
        assert_eq!(d.changed.len(), 1);
        assert_eq!(d.added.len(), 0);
        assert_eq!(d.removed.len(), 0);
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
            hosting_group: String::new(),
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
