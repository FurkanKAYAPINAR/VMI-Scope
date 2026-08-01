//! Persisted user config — preferences, query history and saved queries.
//!
//! Stored as JSON at `%APPDATA%\VMI-Scope\config.json`.
//!
//! # Versioning
//!
//! v1 (shipped through v0.6.0) carried only `history` and `saved`. v2 adds the
//! Settings surface: theme, connection, results and code-generation preferences.
//! The file gained a `version` field so a future migration can tell an old file
//! apart from a new one *by its shape's intent*, not by guessing from which keys
//! happen to be present. A file written before v2 has no `version` key at all,
//! which `serde` reads back as `1` (see [`assumed_v1`]) — that absence is the
//! signal, and it is why the field is not merely `#[serde(default)]` like the
//! rest. Every other new field defaults through the struct's [`Default`], so an
//! old file loads with its history and saved queries intact and every new
//! preference at the same value a fresh install would use.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::theme::{Accent, Density};

/// The current on-disk schema version. Bump this and add a `migrate` arm when a
/// field changes meaning (a pure addition needs no bump — `serde(default)`
/// covers it).
const CONFIG_VERSION: u32 = 2;

/// The default namespace the Explorer opens to. Mirrors `app::DEFAULT_NAMESPACE`
/// deliberately rather than importing it: this is the *persisted* default a user
/// can override, and coupling the config layer to the app's boot constant would
/// invert the dependency for no gain.
const DEFAULT_NAMESPACE: &str = "root\\CIMV2";

/// Row cap default.
///
/// WQL has no `TOP`, so without a cap `SELECT * FROM CIM_DataFile` tries to
/// return every file on the machine. This is only the value a fresh config
/// starts at; `state::requests::run_query` reads the setting.
const DEFAULT_ROW_LIMIT: usize = 5_000;

/// Operation-timeout default, in seconds.
///
/// Needed on top of the row cap, because a cap only bites once rows arrive and
/// some providers deliver none for a long time -- `CIM_DataFile` returned
/// nothing at all in twelve seconds when measured. Also only a starting value.
const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Generated-script wrap column default.
const DEFAULT_LINE_WIDTH: u32 = 100;

/// A file with no `version` key predates versioning, so it is a v1 file.
fn assumed_v1() -> u32 {
    1
}

/// DCOM impersonation level for a remote connection.
///
/// Only reachable on the alternate-credentials path, where `remote.rs`
/// hand-calls `CoSetProxyBlanket`; the SSO path goes through the `wmi` crate,
/// which does not expose it. Persisted here so the choice survives, but honoured
/// only once core task 5.10 parameterises the blanket call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) enum Impersonation {
    Identify,
    #[default]
    Impersonate,
    Delegate,
}

impl Impersonation {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Identify => "Identify",
            Self::Impersonate => "Impersonate",
            Self::Delegate => "Delegate",
        }
    }
}

/// Target language for the script generator's *default* selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) enum CodeLang {
    #[default]
    PowerShell,
    VbScript,
}

/// How byte sizes are rendered where the UI shows one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) enum ByteFormat {
    /// Powers of 1024: KiB, MiB, GiB.
    #[default]
    Binary,
    /// Powers of 1000: KB, MB, GB.
    Decimal,
}

impl ByteFormat {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Binary => "Binary (KiB)",
            Self::Decimal => "Decimal (KB)",
        }
    }
}

/// A user-named query.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct SavedQuery {
    pub(crate) name: String,
    pub(crate) namespace: String,
    pub(crate) wql: String,
}

/// The whole persisted config.
///
/// `#[serde(default)]` at the container level fills any missing field from
/// [`Config::default`], which is what makes a v1 file load cleanly. `version` is
/// the one exception: its own default reports `1`, so a file without the key is
/// recognised as pre-v2 and migrated on load.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Config {
    #[serde(default = "assumed_v1")]
    pub(crate) version: u32,

    // --- Interface ---
    pub(crate) accent: Accent,
    pub(crate) density: Density,
    /// `--decorated`: OS chrome instead of the custom title bar. The CLI flag
    /// still wins at boot; the field lets the Settings toggle (task 2.25)
    /// persist the choice.
    pub(crate) decorated: bool,

    // --- Connection ---
    pub(crate) default_namespace: String,
    pub(crate) impersonation: Impersonation,

    // --- Results ---
    pub(crate) row_limit: usize,
    pub(crate) operation_timeout_secs: u64,
    pub(crate) show_system_classes: bool,
    pub(crate) byte_format: ByteFormat,
    pub(crate) live_polling: bool,
    /// Opt-in status-bar provider host stats (task 5.14); costs a WMI query per
    /// refresh, so it defaults off.
    pub(crate) show_provider_stats: bool,

    // --- Code generation ---
    pub(crate) default_lang: CodeLang,
    pub(crate) line_width: u32,

    // --- History (v1) ---
    pub(crate) history: Vec<String>,
    pub(crate) saved: Vec<SavedQuery>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            accent: Accent::default(),
            density: Density::default(),
            decorated: false,
            default_namespace: DEFAULT_NAMESPACE.to_string(),
            impersonation: Impersonation::default(),
            row_limit: DEFAULT_ROW_LIMIT,
            operation_timeout_secs: DEFAULT_TIMEOUT_SECS,
            show_system_classes: false,
            byte_format: ByteFormat::default(),
            // Live views polling is the resting state a security monitor expects;
            // the Network view still has its own per-view pause on top of this.
            live_polling: true,
            show_provider_stats: false,
            default_lang: CodeLang::default(),
            line_width: DEFAULT_LINE_WIDTH,
            history: Vec::new(),
            saved: Vec::new(),
        }
    }
}

fn config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(base).join("VMI-Scope"))
}

fn config_path() -> Option<PathBuf> {
    Some(config_dir()?.join("config.json"))
}

/// Append a line to the audit log (`%APPDATA%\VMI-Scope\audit.log`) — used to
/// record every mutating (method-execution) call for a security tool.
pub(crate) fn append_audit(line: &str) {
    let Some(dir) = config_dir() else { return };
    let _ = std::fs::create_dir_all(&dir);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("audit.log"))
    {
        let _ = writeln!(f, "{ts}\t{line}");
    }
}

impl Config {
    pub(crate) fn load() -> Config {
        let mut cfg: Config = config_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        // Persist the upgrade so the next load is a straight v2 read rather than
        // another migrate. A fresh install is already at `CONFIG_VERSION`, so it
        // does not touch the disk here.
        if cfg.migrate() {
            cfg.save();
        }
        cfg
    }

    /// Bring an older config up to [`CONFIG_VERSION`] in place. Returns whether
    /// anything changed, so the caller only writes when it must.
    ///
    /// v1 → v2 is a pure field addition: `serde` already supplied v2 defaults
    /// for the missing keys, so the migration is just stamping the version.
    /// Kept as its own method (not folded into `load`) so it can be tested
    /// without touching the filesystem.
    fn migrate(&mut self) -> bool {
        if self.version < CONFIG_VERSION {
            self.version = CONFIG_VERSION;
            true
        } else {
            false
        }
    }

    pub(crate) fn save(&self) {
        if let Some(p) = config_path() {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(s) = serde_json::to_string_pretty(self) {
                let _ = std::fs::write(p, s);
            }
        }
    }

    /// Record a run query at the front of the history (deduped, capped).
    pub(crate) fn push_history(&mut self, wql: &str) {
        let wql = wql.trim();
        if wql.is_empty() {
            return;
        }
        self.history.retain(|q| q != wql);
        self.history.insert(0, wql.to_string());
        self.history.truncate(25);
        self.save();
    }

    /// Save (or replace) a named query.
    pub(crate) fn save_query(&mut self, name: String, namespace: String, wql: String) {
        self.saved.retain(|q| q.name != name);
        self.saved.push(SavedQuery {
            name,
            namespace,
            wql,
        });
        self.saved
            .sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        self.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real v1 `config.json` as written by v0.6.0: two keys, nothing else.
    const V1: &str = r#"{
  "history": [
    "SELECT * FROM Win32_OperatingSystem",
    "SELECT * FROM Win32_Process"
  ],
  "saved": [
    {
      "name": "Processes",
      "namespace": "root\\CIMV2",
      "wql": "SELECT * FROM Win32_Process"
    }
  ]
}"#;

    /// The migration's whole promise: an old file loses nothing, and every
    /// setting it never had comes back as a sane default rather than a zero.
    #[test]
    fn v1_config_loads_without_losing_history_or_saved() {
        let cfg: Config = serde_json::from_str(V1).expect("a v1 config.json must still parse");

        // The two things a v1 file actually carried survive verbatim.
        assert_eq!(
            cfg.history,
            vec![
                "SELECT * FROM Win32_OperatingSystem".to_string(),
                "SELECT * FROM Win32_Process".to_string(),
            ]
        );
        assert_eq!(cfg.saved.len(), 1);
        assert_eq!(cfg.saved[0].name, "Processes");
        assert_eq!(cfg.saved[0].namespace, "root\\CIMV2");
        assert_eq!(cfg.saved[0].wql, "SELECT * FROM Win32_Process");

        // No `version` key means a pre-v2 file.
        assert_eq!(cfg.version, 1);

        // The v2 additions must not read back as 0/empty — an uncapped row
        // limit or a blank namespace would be a real regression, not a default.
        assert_eq!(cfg.accent, Accent::Steel);
        assert_eq!(cfg.density, Density::Compact);
        assert_eq!(cfg.default_namespace, "root\\CIMV2");
        assert_eq!(cfg.row_limit, DEFAULT_ROW_LIMIT);
        assert_eq!(cfg.operation_timeout_secs, DEFAULT_TIMEOUT_SECS);
        assert_eq!(cfg.line_width, DEFAULT_LINE_WIDTH);
        assert_eq!(cfg.default_lang, CodeLang::PowerShell);
        assert_eq!(cfg.impersonation, Impersonation::Impersonate);
        assert_eq!(cfg.byte_format, ByteFormat::Binary);
        assert!(cfg.live_polling);
        assert!(!cfg.show_system_classes);
        assert!(!cfg.show_provider_stats);
        assert!(!cfg.decorated);
    }

    /// Loading a v1 file stamps it as current; doing it again is a no-op, so a
    /// steady-state config never rewrites itself on every launch.
    #[test]
    fn migration_stamps_current_version_and_is_idempotent() {
        let mut cfg: Config = serde_json::from_str(V1).unwrap();
        assert_eq!(cfg.version, 1);
        assert!(cfg.migrate(), "a v1 file must be migrated");
        assert_eq!(cfg.version, CONFIG_VERSION);
        assert!(!cfg.migrate(), "a current file must not migrate again");
    }

    /// A fresh install is born at the current version and must never trigger a
    /// migration write.
    #[test]
    fn a_fresh_config_is_already_current() {
        let mut cfg = Config::default();
        assert_eq!(cfg.version, CONFIG_VERSION);
        assert!(!cfg.migrate());
    }

    /// A full round-trip through JSON keeps every field — the guarantee that
    /// `save` then `load` is lossless.
    #[test]
    fn round_trips_through_json() {
        let cfg = Config {
            accent: Accent::Amber,
            density: Density::Comfortable,
            default_lang: CodeLang::VbScript,
            row_limit: 250,
            live_polling: false,
            history: vec!["SELECT * FROM Win32_Service".to_string()],
            ..Default::default()
        };

        let json = serde_json::to_string_pretty(&cfg).unwrap();
        let back: Config = serde_json::from_str(&json).unwrap();

        assert_eq!(back.version, CONFIG_VERSION);
        assert_eq!(back.accent, Accent::Amber);
        assert_eq!(back.density, Density::Comfortable);
        assert_eq!(back.default_lang, CodeLang::VbScript);
        assert_eq!(back.row_limit, 250);
        assert!(!back.live_polling);
        assert_eq!(back.history, cfg.history);
    }
}
