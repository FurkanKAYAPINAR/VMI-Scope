//! WMI **event-subscription** enumeration — permanent-subscription persistence
//! hunting.
//!
//! Permanent WMI event subscriptions live in `root\subscription` as a triple:
//! an `__EventFilter` (the trigger, a WQL query), an `__EventConsumer`
//! (the action — run a command, run a script, write a log…), and a
//! `__FilterToConsumerBinding` wiring the two together. A filter+consumer that
//! runs code on a system event is one of the most common *fileless*
//! persistence techniques (MITRE ATT&CK T1546.003).

use serde::{Deserialize, Serialize};

/// How suspicious a subscription looks, from a defender's point of view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Risk {
    Low,
    Medium,
    High,
}

impl Risk {
    pub fn as_str(self) -> &'static str {
        match self {
            Risk::Low => "Low",
            Risk::Medium => "Medium",
            Risk::High => "High",
        }
    }
}

/// A filter→consumer binding, flattened with its trigger and action and scored.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subscription {
    pub filter_name: String,
    pub filter_query: String,
    pub consumer_type: String,
    pub consumer_name: String,
    /// Command line / executable / script the consumer runs (best-effort).
    pub action: String,
    pub risk: Risk,
    pub reasons: Vec<String>,
    /// False for filters/consumers that exist but aren't wired by a binding.
    pub bound: bool,
}

/// The whole `root\subscription` picture.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SubscriptionReport {
    pub subscriptions: Vec<Subscription>,
    /// Namespaces the scan could not read, and why.
    ///
    /// An empty subscription list means one of two opposite things — nothing is
    /// installed, or nothing could be looked at — and a persistence hunt that
    /// conflates them reports "clean" for a machine it never managed to
    /// question. `#[serde(default)]` so a baseline saved before this field
    /// existed still loads.
    #[serde(default)]
    pub unreadable: Vec<String>,
}

impl SubscriptionReport {
    pub fn count(&self, risk: Risk) -> usize {
        self.subscriptions.iter().filter(|s| s.risk == risk).count()
    }

    /// Was every namespace readable? A caller showing "0 subscriptions" should
    /// say so only when this is true.
    pub fn is_complete(&self) -> bool {
        self.unreadable.is_empty()
    }
}

/// Extract the first double-quoted token from a WMI object-path reference,
/// e.g. `...:__EventFilter.Name="SCM Event Log Filter"` → `SCM Event Log Filter`.
pub fn first_quoted(reference: &str) -> String {
    if let Some(a) = reference.find('"') {
        if let Some(b) = reference[a + 1..].find('"') {
            return reference[a + 1..a + 1 + b].to_string();
        }
    }
    String::new()
}

/// Score a subscription and explain why. Deterministic, defender-oriented.
pub fn assess(consumer_type: &str, query: &str, action: &str) -> (Risk, Vec<String>) {
    let ct = consumer_type.to_lowercase();
    let act = action.to_lowercase();
    let q = query.to_lowercase();
    let mut risk = Risk::Low;
    let mut reasons = Vec::new();

    if ct.contains("activescript") {
        risk = Risk::High;
        reasons.push("ActiveScriptEventConsumer executes script code".into());
    } else if ct.contains("commandline") {
        risk = Risk::High;
        reasons.push("CommandLineEventConsumer launches a process".into());
    }

    for kw in [
        "powershell",
        "-enc",
        "-encodedcommand",
        "frombase64",
        "iex",
        "invoke-expression",
        "regsvr32",
        "rundll32",
        "mshta",
        "wscript",
        "cscript",
        "cmd.exe",
        "http://",
        "https://",
    ] {
        if act.contains(kw) {
            risk = Risk::High;
            reasons.push(format!("action contains '{kw}'"));
        }
    }

    if q.contains("__instancecreationevent")
        || q.contains("__instancemodificationevent")
        || q.contains("__instanceoperationevent")
    {
        risk = risk.max(Risk::Medium);
        reasons.push("intrinsic event trigger (fires on system changes)".into());
    }
    for kw in [
        "win32_process",
        "logon",
        "win32_service",
        "__timerevent",
        "localtime",
        "startup",
    ] {
        if q.contains(kw) {
            risk = risk.max(Risk::Medium);
            reasons.push(format!("filter targets '{kw}'"));
        }
    }

    if reasons.is_empty() {
        reasons.push("no risky indicators".into());
    }
    (risk, reasons)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_quoted_extracts_the_key_value() {
        assert_eq!(
            first_quoted(r#"\\.\root\subscription:__EventFilter.Name="SCM Filter""#),
            "SCM Filter"
        );
        assert_eq!(first_quoted("no quotes here"), "");
        assert_eq!(first_quoted(r#"Class.Name="first""second""#), "first");
    }

    #[test]
    fn commandline_consumer_with_encoded_payload_is_high() {
        let (risk, reasons) = assess(
            "CommandLineEventConsumer",
            "SELECT * FROM __InstanceCreationEvent WHERE TargetInstance ISA 'Win32_Process'",
            "powershell.exe -enc SQBFAFgA",
        );
        assert_eq!(risk, Risk::High);
        assert!(reasons.iter().any(|r| r.contains("CommandLine")));
    }

    #[test]
    fn benign_scm_subscription_is_low() {
        let (risk, _) = assess(
            "NTEventLogEventConsumer",
            "select * from MSFT_SCMEventLogEvent",
            "",
        );
        assert_eq!(risk, Risk::Low);
    }

    /// What a skipped consumer walk costs, in the only currency that matters
    /// here: the verdict.
    ///
    /// `consumer_type` and `action` are the only inputs that can reach `High`,
    /// so a scan that could not read the consumers scores every subscription on
    /// its filter query alone — and a filter query with no keywords in it is
    /// `Low`. The subscription below is a textbook T1546.003 implant, and
    /// without the consumer walk it is reported as the least interesting row in
    /// the table. That is a false negative, not a missing column, which is why
    /// the walk is no longer allowed to be silently skipped on any transport.
    #[test]
    fn a_consumer_that_was_never_read_downgrades_a_real_implant_to_low() {
        let filter = "SELECT * FROM MSFT_SCMEventLogEvent";
        let (seen, why) = assess(
            "CommandLineEventConsumer",
            filter,
            "powershell.exe -nop -w hidden -enc SQBFAFgA",
        );
        assert_eq!(seen, Risk::High);
        assert!(why.iter().any(|r| r.contains("CommandLineEventConsumer")));

        // The same subscription, as it looked when the consumer walk was
        // skipped: type and action blank, everything else identical.
        let (unseen, why) = assess("", filter, "");
        assert_eq!(unseen, Risk::Low);
        assert_eq!(why, vec!["no risky indicators".to_string()]);
    }

    #[test]
    fn intrinsic_trigger_is_at_least_medium() {
        let (risk, _) = assess(
            "LogFileEventConsumer",
            "SELECT * FROM __InstanceModificationEvent WHERE TargetInstance ISA 'Win32_Service'",
            "C:\\log.txt",
        );
        assert!(risk >= Risk::Medium);
    }
}
