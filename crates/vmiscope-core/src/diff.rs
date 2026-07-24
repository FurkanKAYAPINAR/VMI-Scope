//! Snapshot diffing — compare a saved baseline against the current scan to
//! surface what *changed*. The DFIR question this answers: "which WMI event
//! subscriptions appeared (or changed) since my known-good baseline?"

use std::collections::HashMap;

use crate::events::Subscription;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::Risk;

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
}
