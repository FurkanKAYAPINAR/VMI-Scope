//! Module-scope helpers shared by the views: sort comparators, table column
//! accessors, script generation, and the native save dialog.

use crate::app::ScriptLang;

use vmiscope_core::{Connection, Protocol, ProviderInfo, Risk, Subscription};

/// A dangerous-looking method name gets an extra warning in the confirm modal.
pub(crate) fn is_dangerous_method(method: &str) -> bool {
    let m = method.to_lowercase();
    [
        "create",
        "delete",
        "terminate",
        "reboot",
        "shutdown",
        "format",
        "change",
        "rename",
        "uninstall",
        "setpowerstate",
        "stopservice",
        "kill",
        "write",
        "set",
    ]
    .iter()
    .any(|k| m.contains(k))
}

/// Open a native "Save as" dialog and write `contents` to the chosen path.
///
/// **Returns immediately.** The dialog and the write both happen on the IO
/// thread (`crate::io`), so the frame loop keeps running -- the live views keep
/// polling and the process monitor keeps collecting -- while the dialog is up.
/// It used to block the UI thread for the whole time somebody spent browsing
/// their filesystem, which on a security tool means a gap in the evidence.
///
/// A failed write now reaches the error log instead of being dropped by a
/// `let _`.
pub(crate) fn save_file(default_name: &str, contents: &str) {
    crate::io::save_as(default_name, contents);
}

/// Produce a runnable script equivalent to the current namespace + WQL query.
///
/// The generation itself lives in `vmiscope_core::generate_script` (all four
/// languages); this maps the GUI's two-way persisted selector onto the two arms
/// the Code sub-tab drives through it, and applies the Settings-level
/// **include credentials block** (task 7.3) on top.
pub(crate) fn generate_script(
    lang: ScriptLang,
    namespace: &str,
    wql: &str,
    credentials: bool,
) -> String {
    let core_lang = match lang {
        ScriptLang::PowerShell => vmiscope_core::ScriptLang::PowerShell,
        ScriptLang::VbScript => vmiscope_core::ScriptLang::VbScript,
    };
    let script = vmiscope_core::generate_script(core_lang, namespace, wql);
    if credentials {
        with_credentials(lang, &script)
    } else {
        script
    }
}

/// The anchor line the PowerShell arm's credentials rewrite replaces.
///
/// Matched as a substring of what `vmiscope_core::generate_script` actually
/// emits, pinned by [`tests::the_credentials_anchors_still_exist_in_core`]. The
/// alternative -- re-implementing the generator here so it can take a
/// credential -- would mean two generators for four languages that have to stay
/// in step, which is a worse failure than a rewrite whose seam is tested.
const PS_ANCHOR: &str = "Get-CimInstance -Namespace $namespace";

/// The VBScript arm's anchor: the moniker bind, which alternate credentials
/// cannot go through at all. `GetObject("winmgmts:")` authenticates as the
/// caller by construction, so this is a replacement rather than an insertion.
const VBS_ANCHOR: &str = "Set objWMI   = GetObject";

/// Rewrite a generated script to authenticate as somebody else.
///
/// Not a prepended comment block: a header that declares `$credential` and is
/// then ignored by the call below it is worse than no header, because it reads
/// as working. Each arm replaces the line that actually binds WMI.
///
/// If the anchor is absent the script comes back untouched -- there is no
/// half-rewritten script, and the unit test below is what stops that silence
/// from being how anyone finds out.
fn with_credentials(lang: ScriptLang, script: &str) -> String {
    match lang {
        ScriptLang::PowerShell => {
            let Some(at) = script.find(PS_ANCHOR) else {
                return script.to_string();
            };
            let rewritten = script.replacen(
                PS_ANCHOR,
                "Get-CimInstance -CimSession $session -Namespace $namespace",
                1,
            );
            // `at` still indexes `rewritten`: everything before it is untouched
            // by a replacement that starts exactly there.
            format!(
                "{}{PS_PRELUDE}{}{PS_EPILOGUE}",
                &rewritten[..at],
                &rewritten[at..].trim_end()
            )
        }
        ScriptLang::VbScript => {
            let Some(at) = script.find(VBS_ANCHOR) else {
                return script.to_string();
            };
            // The whole bind line goes, not just the call: `Set objWMI = ...` is
            // re-issued by the block.
            let after = script[at..].find('\n').map_or(script.len(), |n| at + n + 1);
            format!("{}{VBS_BIND}{}", &script[..at], &script[after..])
        }
    }
}

/// PowerShell: a `CimSession` is the only way `Get-CimInstance` takes a
/// credential, so the block builds one and the call below is rewritten onto it.
const PS_PRELUDE: &str = "\
# Alternate credentials (Settings -> Code generation -> Include credentials block).\n\
$computer   = '.'                # the host to run against\n\
$credential = Get-Credential     # prompts; never store a password in a script\n\
$session    = New-CimSession -ComputerName $computer -Credential $credential\n\
\n";

/// And a session that is not closed leaks a DCOM connection per run.
const PS_EPILOGUE: &str = "\nRemove-CimSession $session\n";

/// VBScript: `GetObject(\"winmgmts:\")` authenticates as the caller and has no
/// credential parameter at all, so the bind is replaced by `SWbemLocator`,
/// which does. The impersonation level is set explicitly because the locator
/// path does not inherit the moniker's default.
const VBS_BIND: &str = "\
' Alternate credentials (Settings -> Code generation -> Include credentials block).\n\
strComputer = \".\"\n\
strUser     = \"\"                ' DOMAIN\\user; leave empty for the current user\n\
strPassword = \"\"\n\
Set objLocator = CreateObject(\"WbemScripting.SWbemLocator\")\n\
Set objWMI     = objLocator.ConnectServer(strComputer, strNamespace, strUser, strPassword)\n\
objWMI.Security_.ImpersonationLevel = 3   ' wbemImpersonationLevelImpersonate\n";

/// What a Network cell shows where the field does not apply: a UDP endpoint has
/// no peer, so it has neither a remote address nor a remote port.
pub(crate) const NET_NONE: &str = "*";

/// What a Network cell shows where the field applies but was not reported —
/// today only a TCP row that arrived without a state name.
pub(crate) const NET_UNKNOWN: &str = "\u{2014}";

/// The text of a network table cell, in header order.
///
/// This is the **only** definition of that text: `views::network` renders every
/// cell through it and `DataTable` sorts on it, so the two cannot disagree.
/// They used to. Column 5 returned an empty string for a UDP row while the cell
/// painted [`NET_NONE`], and column 1 returned an empty state while the cell
/// painted [`NET_UNKNOWN`] — so those rows sorted on text that was nowhere on
/// screen, and the caret pointed the wrong way for a reason the user could not
/// see. Adding a placeholder to a cell without adding it here is the whole bug,
/// and the way to not do it again is to have one string.
pub(crate) fn net_col_value(c: &Connection, col: usize) -> String {
    match col {
        0 => c.proto.as_str().to_string(),
        1 => {
            if c.state.is_empty() {
                NET_UNKNOWN.to_string()
            } else {
                c.state.clone()
            }
        }
        2 => c.local_addr.clone(),
        3 => c.local_port.to_string(),
        4 => {
            if c.remote_addr.is_empty() {
                NET_NONE.to_string()
            } else {
                c.remote_addr.clone()
            }
        }
        5 => {
            if c.proto == Protocol::Udp {
                NET_NONE.to_string()
            } else {
                c.remote_port.to_string()
            }
        }
        6 => c.pid.to_string(),
        7 => c.process.clone(),
        _ => String::new(),
    }
}

/// What the Risk column's header says about its own sort.
///
/// Column 0 is the one place in the app where the sort key is deliberately
/// *not* the cell text: `High`/`Medium`/`Low` sorted alphabetically is
/// `High, Low, Medium`, which is no order at all for a hunt. It sorts by
/// severity instead — but an ascending caret over a column that reads
/// `Low, Medium, High` looks like the caret is upside down, so the header says
/// which order it means rather than leaving the user to work it out.
pub(crate) const RISK_SORT_NOTE: &str =
    "Sorts by severity, not alphabetically: ascending runs Low \u{2192} Medium \u{2192} High.";

/// The display/sort string for a persistence table column. Column 0 (risk) maps
/// to a numeric severity so sorting orders by danger, not alphabetically; see
/// [`RISK_SORT_NOTE`], which is what the header tells the user about it.
pub(crate) fn sub_col_value(s: &Subscription, col: usize) -> String {
    match col {
        0 => match s.risk {
            Risk::High => "3",
            Risk::Medium => "2",
            Risk::Low => "1",
        }
        .to_string(),
        1 => s.consumer_type.clone(),
        2 => s.consumer_name.clone(),
        3 => s.filter_name.clone(),
        4 => {
            if s.action.is_empty() {
                s.filter_query.clone()
            } else {
                s.action.clone()
            }
        }
        5 => s.reasons.join("; "),
        _ => String::new(),
    }
}

/// The display/sort string for a providers table column.
pub(crate) fn prov_col_value(p: &ProviderInfo, col: usize) -> String {
    match col {
        0 => p.provider.clone(),
        1 => p.namespace.clone(),
        2 => p.host_pid.to_string(),
        3 => p.host_process.clone(),
        4 => p.hosting_group.clone(),
        _ => String::new(),
    }
}

/// Compare two cells numerically when both parse as numbers, else case-insensitively.
pub(crate) fn smart_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    match (a.parse::<f64>(), b.parse::<f64>()) {
        (Ok(x), Ok(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
        _ => a.to_lowercase().cmp(&b.to_lowercase()),
    }
}

// `toggle_sort` lived here until the Explorer's results grid -- the last
// hand-rolled `TableBuilder` -- moved onto `DataTable`, which owns the
// tri-state cycle itself as `widgets::table::cycle_sort`.

#[cfg(test)]
mod tests {
    use super::*;

    fn conn(proto: Protocol, remote_addr: &str, remote_port: u16, state: &str) -> Connection {
        Connection {
            proto,
            local_addr: "0.0.0.0".into(),
            local_port: 445,
            remote_addr: remote_addr.into(),
            remote_port,
            state: state.into(),
            pid: 4,
            process: "System".into(),
        }
    }

    /// The bug this function was rewritten for: a cell that paints a placeholder
    /// must sort on that placeholder. A UDP endpoint has no peer, so both the
    /// remote address and the remote port read `*` -- and used to sort on `""`
    /// and on the raw `0`, neither of which is anywhere on screen.
    #[test]
    fn a_udp_row_sorts_on_the_text_it_shows() {
        let udp = conn(Protocol::Udp, "", 0, "");
        assert_eq!(net_col_value(&udp, 4), NET_NONE, "remote address");
        assert_eq!(net_col_value(&udp, 5), NET_NONE, "remote port");
        // Not the raw zero the struct carries: `0` would sort in among real
        // ephemeral ports and claim a peer on port zero.
        assert_ne!(net_col_value(&udp, 5), "0");
    }

    /// A listening TCP socket has no peer either, and a state-less TCP row shows
    /// an em dash. Both are placeholders, and both have to be the sort key.
    #[test]
    fn tcp_placeholders_are_the_sort_key_too() {
        let listening = conn(Protocol::Tcp, "", 0, "Listen");
        assert_eq!(net_col_value(&listening, 4), NET_NONE);
        // The port is real on a TCP row even when it is zero, so it is NOT
        // replaced: only the address is unknown.
        assert_eq!(net_col_value(&listening, 5), "0");

        let stateless = conn(Protocol::Tcp, "8.8.8.8", 53, "");
        assert_eq!(net_col_value(&stateless, 1), NET_UNKNOWN);
        assert_eq!(
            net_col_value(&conn(Protocol::Tcp, "", 0, "Listen"), 1),
            "Listen"
        );
    }

    /// Every column has to answer, and out-of-range has to answer emptily rather
    /// than panicking: `DataTable` calls this for whatever column was clicked.
    #[test]
    fn every_column_answers_and_the_range_is_bounded() {
        let c = conn(Protocol::Tcp, "1.1.1.1", 443, "Established");
        let cells: Vec<String> = (0..8).map(|col| net_col_value(&c, col)).collect();
        assert_eq!(
            cells,
            vec![
                "TCP",
                "Established",
                "0.0.0.0",
                "445",
                "1.1.1.1",
                "443",
                "4",
                "System"
            ]
        );
        assert_eq!(net_col_value(&c, 8), "");
    }

    /// The risk column sorts by danger. This is the one deliberate
    /// key/display divergence in the app, and `RISK_SORT_NOTE` is what makes it
    /// visible -- so pin the ordering the note promises.
    #[test]
    fn risk_sorts_by_severity_ascending_toward_high() {
        let of = |risk| {
            sub_col_value(
                &Subscription {
                    filter_name: String::new(),
                    filter_query: String::new(),
                    consumer_type: String::new(),
                    consumer_name: String::new(),
                    action: String::new(),
                    risk,
                    reasons: Vec::new(),
                    bound: true,
                },
                0,
            )
        };
        assert!(smart_cmp(&of(Risk::Low), &of(Risk::Medium)).is_lt());
        assert!(smart_cmp(&of(Risk::Medium), &of(Risk::High)).is_lt());
        assert!(
            RISK_SORT_NOTE.contains("severity"),
            "the header note must say what the order is"
        );
    }

    /// The whole rewrite rests on two substrings of another crate's output. If
    /// the core's generator is reworded, the credentials block silently stops
    /// being applied and the Settings toggle silently goes back to being
    /// decorative -- so the seam is pinned here rather than trusted.
    #[test]
    fn the_credentials_anchors_still_exist_in_core() {
        let ps = vmiscope_core::generate_script(
            vmiscope_core::ScriptLang::PowerShell,
            "root\\CIMV2",
            "SELECT * FROM Win32_Process",
        );
        assert!(
            ps.contains(PS_ANCHOR),
            "core no longer emits {PS_ANCHOR:?}:\n{ps}"
        );
        let vbs = vmiscope_core::generate_script(
            vmiscope_core::ScriptLang::VbScript,
            "root\\CIMV2",
            "SELECT * FROM Win32_Process",
        );
        assert!(
            vbs.contains(VBS_ANCHOR),
            "core no longer emits {VBS_ANCHOR:?}:\n{vbs}"
        );
    }

    /// Task 7.3's acceptance for the credentials block: with it off the script
    /// is exactly what core produced, and with it on the script *authenticates*
    /// -- not merely declares a variable it then ignores.
    #[test]
    fn the_credentials_block_changes_how_the_script_binds() {
        let ns = "root\\CIMV2";
        let q = "SELECT * FROM Win32_Process";

        let plain = generate_script(ScriptLang::PowerShell, ns, q, false);
        assert_eq!(
            plain,
            vmiscope_core::generate_script(vmiscope_core::ScriptLang::PowerShell, ns, q),
            "off must be a passthrough"
        );
        assert!(!plain.contains("Get-Credential"));

        let creds = generate_script(ScriptLang::PowerShell, ns, q, true);
        assert!(creds.contains("New-CimSession"), "{creds}");
        // The point: the *call* uses the session. A header alone would be a lie.
        assert!(
            creds.contains("Get-CimInstance -CimSession $session"),
            "{creds}"
        );
        assert!(!creds.contains(PS_ANCHOR), "the original bind survived");
        assert!(creds.contains("Remove-CimSession"), "the session leaks");
        // The query itself is untouched -- the here-string is why nothing here
        // may reflow the body.
        assert!(creds.contains(q), "{creds}");

        let vbs = generate_script(ScriptLang::VbScript, ns, q, true);
        assert!(vbs.contains("SWbemLocator"), "{vbs}");
        assert!(vbs.contains("ConnectServer"), "{vbs}");
        assert!(
            !vbs.contains("GetObject(\"winmgmts:"),
            "the moniker bind -- which cannot take a credential -- survived:\n{vbs}"
        );
        // Everything after the bind still runs against `objWMI`.
        assert!(vbs.contains("objWMI.ExecQuery(strQuery)"), "{vbs}");
        assert!(vbs.contains("For Each objItem In colItems"), "{vbs}");
    }

    /// `smart_cmp` is what every table sorts through, so a numeric column must
    /// not order `10` before `9` -- and a mixed column has to fall back to text
    /// rather than panicking on the parse.
    #[test]
    fn smart_cmp_is_numeric_when_it_can_be_and_textual_when_it_cannot() {
        assert!(smart_cmp("9", "10").is_lt());
        assert!(smart_cmp("10", "9").is_gt());
        assert!(smart_cmp("Established", "established").is_eq());
        // A placeholder against a number: no panic, and a total order.
        assert!(smart_cmp(NET_NONE, "443").is_lt());
        assert!(smart_cmp("443", NET_NONE).is_gt());
    }
}
