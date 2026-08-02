//! Persisted user config — preferences, query history and the saved-query
//! library.
//!
//! Stored as JSON at `%APPDATA%\VMI-Scope\config.json`.
//!
//! # Versioning
//!
//! v1 (shipped through v0.6.0) carried only `history` and `saved`. v2 added the
//! Settings surface: theme, connection, results and code-generation preferences.
//! v3 gives history and saved queries a shape of their own — a history entry
//! grows a namespace and the run's real timings, a saved query grows a folder, a
//! favourite flag, an author and the metrics of its last run.
//!
//! The file gained a `version` field at v2 so a future migration can tell an old
//! file apart from a new one *by its shape's intent*, not by guessing from which
//! keys happen to be present. A file written before v2 has no `version` key at
//! all, which `serde` reads back as `1` (see [`assumed_v1`]) — that absence is
//! the signal, and it is why the field is not merely `#[serde(default)]` like the
//! rest. Every other new field defaults through the struct's [`Default`], and
//! [`HistoryEntry`] reads a bare string as well as an object, so a v1 file loads
//! with its history and saved queries intact and every new preference at the same
//! value a fresh install would use.
//!
//! # Writing
//!
//! [`Config::save`] writes the whole file synchronously. That is fine for a
//! settings toggle and wrong for something that happens on every query run, so
//! [`Config::save_debounced`] marks the config dirty and [`Config::poll_save`] —
//! called once a frame — coalesces a burst of changes into one write. See
//! [`SaveClock`].

use std::cell::Cell;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Deserializer, Serialize};

use crate::theme::{Accent, Density};

/// The current on-disk schema version. Bump this and add a `migrate` arm when a
/// field changes meaning (a pure addition needs no bump — `serde(default)`
/// covers it).
const CONFIG_VERSION: u32 = 3;

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

/// How many queries the history keeps.
const HISTORY_CAP: usize = 25;

/// How long a coalesced write waits behind the one before it.
///
/// Two seconds: long enough that a burst of runs collapses into a single write,
/// short enough that a crash costs at most the last couple of seconds of
/// history. It is a *cooldown*, not a trailing-edge delay — the first change
/// after a quiet period is written at once, so the common case (one query, then
/// thinking) still hits the disk immediately.
const SAVE_DEBOUNCE: Duration = Duration::from_secs(2);

/// A file with no `version` key predates versioning, so it is a v1 file.
fn assumed_v1() -> u32 {
    1
}

/// Seconds since the Unix epoch, or 0 on a clock that predates it.
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Who is running this process, as `DOMAIN\user`.
///
/// Read from the environment at save time and stored, rather than resolved at
/// display time: the point of the field is to say who wrote a query, and a query
/// imported from someone else's library must keep *their* name.
fn current_author() -> String {
    let user = std::env::var("USERNAME").unwrap_or_default();
    let domain = std::env::var("USERDOMAIN").unwrap_or_default();
    if user.is_empty() {
        String::new()
    } else if domain.is_empty() {
        user
    } else {
        format!("{domain}\\{user}")
    }
}

// ---------------------------------------------------------------------------
// Write debounce
// ---------------------------------------------------------------------------

/// What a [`SaveClock`] owes at a given instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Due {
    /// Nothing has changed since the last write.
    Nothing,
    /// A write is owed and the cooldown has passed.
    Now,
    /// A write is owed but must wait this long.
    In(Duration),
}

/// The cooldown behind [`Config::save_debounced`].
///
/// Split out from `Config` so the policy can be unit-tested against synthetic
/// instants without going near the filesystem.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct SaveClock {
    dirty: bool,
    last_write: Option<Instant>,
}

impl SaveClock {
    fn due(self, now: Instant) -> Due {
        if !self.dirty {
            return Due::Nothing;
        }
        match self.last_write {
            // Never written this session: the first change goes straight out.
            None => Due::Now,
            Some(at) => {
                let since = now.saturating_duration_since(at);
                match SAVE_DEBOUNCE.checked_sub(since) {
                    Some(left) if !left.is_zero() => Due::In(left),
                    _ => Due::Now,
                }
            }
        }
    }
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

// ---------------------------------------------------------------------------
// History
// ---------------------------------------------------------------------------

/// One run of one query.
///
/// The metrics are `Option` because they are only known when the reply lands,
/// and a query that errored never gets any. A `0 ms / 0 rows` default would read
/// as "instant and empty", which is the one thing it must not say.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct HistoryEntry {
    pub(crate) wql: String,
    /// The namespace the query ran against. Empty for an entry migrated from a
    /// v1/v2 file, which stored the query text and nothing else — clicking such
    /// an entry therefore leaves the active namespace alone rather than guessing
    /// one for it.
    pub(crate) namespace: String,
    /// Measured enumeration time, excluding the namespace bind.
    pub(crate) elapsed_ms: Option<u64>,
    pub(crate) rows: Option<usize>,
    /// Unix seconds at dispatch. `None` for a migrated v1/v2 entry.
    pub(crate) at: Option<u64>,
}

impl HistoryEntry {
    /// A fresh entry for a query being dispatched now: no metrics yet.
    fn started(wql: String, namespace: String) -> Self {
        Self {
            wql,
            namespace,
            elapsed_ms: None,
            rows: None,
            at: Some(unix_now()),
        }
    }
}

/// Accepts both shapes a `history` element has ever had: the v1/v2 bare string,
/// and the v3 object.
///
/// Written by hand rather than with `#[serde(untagged)]` on the entry itself,
/// because untagged would also silently accept an object missing `wql` (falling
/// through to the string arm's error) and report the failure against the wrong
/// variant.
impl<'de> Deserialize<'de> for HistoryEntry {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Fields {
            wql: String,
            #[serde(default)]
            namespace: String,
            #[serde(default)]
            elapsed_ms: Option<u64>,
            #[serde(default)]
            rows: Option<usize>,
            #[serde(default)]
            at: Option<u64>,
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Plain(String),
            Full(Fields),
        }

        Ok(match Repr::deserialize(deserializer)? {
            Repr::Plain(wql) => Self {
                wql,
                namespace: String::new(),
                elapsed_ms: None,
                rows: None,
                at: None,
            },
            Repr::Full(f) => Self {
                wql: f.wql,
                namespace: f.namespace,
                elapsed_ms: f.elapsed_ms,
                rows: f.rows,
                at: f.at,
            },
        })
    }
}

// ---------------------------------------------------------------------------
// Saved queries
// ---------------------------------------------------------------------------

/// A user-named query.
///
/// v3 additions all carry `#[serde(default)]`, so a v1/v2 entry loads with no
/// folder, unfavourited, no author and no metrics — the honest reading of a file
/// that never recorded them.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub(crate) struct SavedQuery {
    pub(crate) name: String,
    pub(crate) namespace: String,
    pub(crate) wql: String,
    /// Free-text grouping. Empty means the implicit "Ungrouped" bucket; there is
    /// no folder registry, because a folder that can exist while empty is a
    /// second thing to keep in sync with the queries.
    #[serde(default)]
    pub(crate) folder: String,
    #[serde(default)]
    pub(crate) fav: bool,
    /// `DOMAIN\user` at save time. Empty when the environment named neither.
    #[serde(default)]
    pub(crate) author: String,
    /// Metrics of the last run of this exact text in this exact namespace.
    #[serde(default)]
    pub(crate) last_ms: Option<u64>,
    #[serde(default)]
    pub(crate) last_rows: Option<usize>,
}

// ---------------------------------------------------------------------------
// The file
// ---------------------------------------------------------------------------

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

    // --- History and library ---
    pub(crate) history: Vec<HistoryEntry>,
    pub(crate) saved: Vec<SavedQuery>,

    /// Write cooldown. Not persisted — it is a fact about this process, not
    /// about the file.
    #[serde(skip)]
    save_clock: Cell<SaveClock>,
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
            save_clock: Cell::new(SaveClock::default()),
        }
    }
}

fn config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(base).join("VMI-Scope"))
}

fn config_path() -> Option<PathBuf> {
    // The unit tests below exercise the real `save_query` / `merge_library` /
    // `push_history`, all of which write. Without this they would overwrite the
    // developer's own `config.json` on every `cargo test` -- a test suite that
    // destroys real user data to prove a sort order is not a trade worth making.
    // `save` still runs its whole path, minus the file itself, so the write
    // debounce is exercised exactly as it is in the app.
    if cfg!(test) {
        return None;
    }
    Some(config_dir()?.join("config.json"))
}

/// Where the saved-query library lives, for the Saved view's header.
///
/// It is the config file: the library is a field of it, not a file of its own.
/// Saying so in the UI is the point — "Local library · N queries · <path>" is
/// only honest if the path is the one that would actually be edited.
pub(crate) fn library_path() -> Option<PathBuf> {
    config_path()
}

/// Append a line to the audit log (`%APPDATA%\VMI-Scope\audit.log`) — used to
/// record every mutating (method-execution) call for a security tool.
pub(crate) fn append_audit(line: &str) {
    let Some(dir) = config_dir() else { return };
    let _ = std::fs::create_dir_all(&dir);
    let ts = unix_now();
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
        // Persist the upgrade so the next load is a straight v3 read rather than
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
    /// Both hops so far are pure additions — `serde` already supplied the
    /// defaults for the missing keys and [`HistoryEntry`] already read the old
    /// bare-string history — so the migration is just stamping the version.
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

    /// Write the file now.
    ///
    /// For a change the user just made deliberately — a Settings toggle, a saved
    /// query. Anything that happens as a side effect of ordinary use should go
    /// through [`Config::save_debounced`] instead.
    pub(crate) fn save(&self) {
        if let Some(p) = config_path() {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(s) = serde_json::to_string_pretty(self) {
                let _ = std::fs::write(p, s);
            }
        }
        self.save_clock.set(SaveClock {
            dirty: false,
            last_write: Some(Instant::now()),
        });
    }

    /// Mark the config changed without touching the disk.
    ///
    /// The write happens in [`Config::poll_save`], at most once per
    /// [`SAVE_DEBOUNCE`]. This exists because `push_history` used to call
    /// [`Config::save`] on **every** query run: a serialize-and-write of the
    /// whole file, on the UI thread, for a single pushed string (task 4.7).
    pub(crate) fn save_debounced(&self) {
        let mut clock = self.save_clock.get();
        clock.dirty = true;
        self.save_clock.set(clock);
    }

    /// Perform a debounced write if one is due.
    ///
    /// Returns how long the caller must wait before the next attempt would do
    /// anything, so a frame loop that is about to go idle can schedule itself a
    /// wake-up rather than leaving the change unwritten until the next input
    /// event. `None` means nothing is owed.
    pub(crate) fn poll_save(&self) -> Option<Duration> {
        match self.save_clock.get().due(Instant::now()) {
            Due::Nothing => None,
            Due::Now => {
                self.save();
                None
            }
            Due::In(left) => Some(left),
        }
    }

    /// Record a dispatched query at the front of the history (deduped, capped).
    ///
    /// The metrics are filled in later by [`Config::note_query_run`], when the
    /// reply that carries them arrives.
    pub(crate) fn push_history(&mut self, wql: &str, namespace: &str) {
        let wql = wql.trim();
        if wql.is_empty() {
            return;
        }
        // Deduped on the pair, not on the text: the same WQL against
        // `root\CIMV2` and `root\subscription` are two different queries, and
        // collapsing them would lose the namespace of whichever ran first.
        self.history
            .retain(|h| !(h.wql == wql && h.namespace == namespace));
        self.history.insert(
            0,
            HistoryEntry::started(wql.to_string(), namespace.to_string()),
        );
        self.history.truncate(HISTORY_CAP);
        self.save_debounced();
    }

    /// Attach a completed run's measured timings to the history entry it belongs
    /// to, and to any saved query with the same text in the same namespace.
    ///
    /// Returns whether anything matched, which is only of interest to the tests
    /// — the caller has nothing to do either way.
    pub(crate) fn note_query_run(
        &mut self,
        wql: &str,
        namespace: &str,
        elapsed_ms: u64,
        rows: usize,
    ) -> bool {
        let wql = wql.trim();
        let mut touched = false;
        if let Some(entry) = self
            .history
            .iter_mut()
            .find(|h| h.wql == wql && h.namespace == namespace)
        {
            entry.elapsed_ms = Some(elapsed_ms);
            entry.rows = Some(rows);
            touched = true;
        }
        for saved in self
            .saved
            .iter_mut()
            .filter(|q| q.wql.trim() == wql && q.namespace == namespace)
        {
            saved.last_ms = Some(elapsed_ms);
            saved.last_rows = Some(rows);
            touched = true;
        }
        if touched {
            self.save_debounced();
        }
        touched
    }

    /// Save (or replace) a named query.
    ///
    /// Saving over an existing name keeps that entry's folder and favourite
    /// flag: those are how the user filed it, and re-saving the text is not a
    /// request to unfile it. The metrics come from the history entry for the
    /// same query, so a card shows the run it was saved from rather than a
    /// placeholder.
    pub(crate) fn save_query(&mut self, name: String, namespace: String, wql: String) {
        let (last_ms, last_rows) = self
            .history
            .iter()
            .find(|h| h.wql == wql.trim() && h.namespace == namespace)
            .map_or((None, None), |h| (h.elapsed_ms, h.rows));
        let previous = self.saved.iter().find(|q| q.name == name);
        let folder = previous.map(|q| q.folder.clone()).unwrap_or_default();
        let fav = previous.is_some_and(|q| q.fav);

        self.saved.retain(|q| q.name != name);
        self.saved.push(SavedQuery {
            name,
            namespace,
            wql,
            folder,
            fav,
            author: current_author(),
            last_ms,
            last_rows,
        });
        self.sort_saved();
        self.save();
    }

    /// Order the library: favourites first, then by folder, then by name. The
    /// card grid renders in this order, so it is the library's order rather than
    /// the view's.
    fn sort_saved(&mut self) {
        self.saved.sort_by(|a, b| {
            b.fav
                .cmp(&a.fav)
                .then_with(|| a.folder.to_lowercase().cmp(&b.folder.to_lowercase()))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
    }

    /// File a saved query under `folder` (empty to unfile it).
    ///
    /// Task 4.15 adds the field and 4.18 filters on it, but the plan names
    /// nothing that *sets* one -- which would have left `folder` decorative, the
    /// one thing this project's own rules refuse. Folders are created by naming
    /// them here: there is no registry, so a folder exists exactly as long as a
    /// query is in it.
    pub(crate) fn set_folder(&mut self, name: &str, folder: &str) {
        if let Some(q) = self.saved.iter_mut().find(|q| q.name == name) {
            q.folder = folder.trim().to_string();
        }
        self.sort_saved();
        self.save();
    }

    /// Toggle a saved query's favourite flag by name.
    pub(crate) fn toggle_favourite(&mut self, name: &str) {
        if let Some(q) = self.saved.iter_mut().find(|q| q.name == name) {
            q.fav = !q.fav;
        }
        self.sort_saved();
        self.save();
    }

    /// Remove a saved query by name.
    pub(crate) fn delete_saved(&mut self, name: &str) {
        self.saved.retain(|q| q.name != name);
        self.save();
    }

    /// Every folder in use, sorted, with the empty (ungrouped) bucket dropped.
    pub(crate) fn folders(&self) -> Vec<String> {
        let mut folders: Vec<String> = self
            .saved
            .iter()
            .filter(|q| !q.folder.is_empty())
            .map(|q| q.folder.clone())
            .collect();
        folders.sort_by_key(|f| f.to_lowercase());
        folders.dedup();
        folders
    }

    /// The library as a standalone JSON document, for export.
    pub(crate) fn library_to_json(&self) -> String {
        serde_json::to_string_pretty(&self.saved).unwrap_or_else(|_| "[]".to_string())
    }

    /// Parse an exported library.
    pub(crate) fn library_from_json(text: &str) -> Result<Vec<SavedQuery>, String> {
        serde_json::from_str(text).map_err(|e| e.to_string())
    }

    /// Merge an imported library in, replacing by name. Returns
    /// `(added, replaced)`.
    ///
    /// Name is the identity because it is what the user typed and what the cards
    /// are read by. An import that silently duplicated every query under the
    /// same name would make a round trip lossy in the one direction that
    /// matters.
    pub(crate) fn merge_library(&mut self, incoming: Vec<SavedQuery>) -> (usize, usize) {
        let (mut added, mut replaced) = (0, 0);
        for query in incoming {
            if query.name.trim().is_empty() {
                continue;
            }
            match self.saved.iter_mut().find(|q| q.name == query.name) {
                Some(existing) => {
                    *existing = query;
                    replaced += 1;
                }
                None => {
                    self.saved.push(query);
                    added += 1;
                }
            }
        }
        self.sort_saved();
        self.save();
        (added, replaced)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real v1 `config.json` as written by v0.6.0: two keys, nothing else, and
    /// a history of bare strings.
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
        assert_eq!(cfg.history.len(), 2);
        assert_eq!(cfg.history[0].wql, "SELECT * FROM Win32_OperatingSystem");
        assert_eq!(cfg.history[1].wql, "SELECT * FROM Win32_Process");
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

    /// Task 4.6's acceptance: an old config's plain strings load with `None`
    /// metadata rather than with a fabricated `0 ms / 0 rows`.
    #[test]
    fn v1_history_strings_load_with_no_metadata() {
        let cfg: Config = serde_json::from_str(V1).unwrap();
        for entry in &cfg.history {
            assert_eq!(entry.elapsed_ms, None, "{entry:?} invented a duration");
            assert_eq!(entry.rows, None, "{entry:?} invented a row count");
            assert_eq!(entry.at, None, "{entry:?} invented a timestamp");
            assert!(
                entry.namespace.is_empty(),
                "{entry:?} invented a namespace it was never stored with"
            );
        }
    }

    /// Task 4.15's acceptance: old saved queries migrate with an empty folder,
    /// `fav = false`, no author and no metrics.
    #[test]
    fn v1_saved_queries_migrate_unfiled_and_unmeasured() {
        let cfg: Config = serde_json::from_str(V1).unwrap();
        let q = &cfg.saved[0];
        assert!(q.folder.is_empty());
        assert!(!q.fav);
        assert!(q.author.is_empty());
        assert_eq!(q.last_ms, None);
        assert_eq!(q.last_rows, None);
    }

    /// A v3 history entry round-trips as an object while the v1 string form
    /// still reads — the two shapes have to coexist in one array, because a file
    /// written by v3 after loading a v1 file contains exactly that.
    #[test]
    fn both_history_shapes_parse_from_one_array() {
        let json = r#"[
            "SELECT * FROM Win32_Service",
            {"wql":"SELECT * FROM Win32_Process","namespace":"root\\CIMV2",
             "elapsed_ms":412,"rows":187,"at":1750000000}
        ]"#;
        let entries: Vec<HistoryEntry> = serde_json::from_str(json).unwrap();
        assert_eq!(entries[0].wql, "SELECT * FROM Win32_Service");
        assert_eq!(entries[0].elapsed_ms, None);
        assert_eq!(entries[1].namespace, "root\\CIMV2");
        assert_eq!(entries[1].elapsed_ms, Some(412));
        assert_eq!(entries[1].rows, Some(187));

        // And the object form survives a save/load cycle unchanged.
        let back: Vec<HistoryEntry> =
            serde_json::from_str(&serde_json::to_string(&entries).unwrap()).unwrap();
        assert_eq!(back, entries);
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
        let mut cfg = Config {
            accent: Accent::Amber,
            density: Density::Comfortable,
            default_lang: CodeLang::VbScript,
            row_limit: 250,
            live_polling: false,
            ..Default::default()
        };
        cfg.push_history("SELECT * FROM Win32_Service", "root\\CIMV2");

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

    // -- history -----------------------------------------------------------

    /// History dedupes on the (query, namespace) pair. The same text against two
    /// namespaces is two entries, because it is two questions with two answers.
    #[test]
    fn history_dedupes_on_query_and_namespace_together() {
        let mut cfg = Config::default();
        cfg.push_history("SELECT * FROM __EventFilter", "root\\subscription");
        cfg.push_history("SELECT * FROM __EventFilter", "root\\CIMV2");
        cfg.push_history("SELECT * FROM __EventFilter", "root\\subscription");

        assert_eq!(cfg.history.len(), 2);
        // The re-run moved back to the front rather than adding a third entry.
        assert_eq!(cfg.history[0].namespace, "root\\subscription");
        assert_eq!(cfg.history[1].namespace, "root\\CIMV2");
    }

    #[test]
    fn history_is_capped_and_ignores_blank_queries() {
        let mut cfg = Config::default();
        for n in 0..(HISTORY_CAP + 10) {
            cfg.push_history(&format!("SELECT * FROM C{n}"), "root\\CIMV2");
        }
        assert_eq!(cfg.history.len(), HISTORY_CAP);
        cfg.push_history("   ", "root\\CIMV2");
        assert_eq!(cfg.history.len(), HISTORY_CAP);
    }

    /// The reply's real timings land on the entry the run created, and on a
    /// saved query with the same text — never on one in another namespace.
    #[test]
    fn a_completed_run_stamps_its_history_entry_and_saved_query() {
        let mut cfg = Config::default();
        cfg.saved.push(SavedQuery {
            name: "Processes".into(),
            namespace: "root\\CIMV2".into(),
            wql: "SELECT * FROM Win32_Process".into(),
            ..Default::default()
        });
        cfg.saved.push(SavedQuery {
            name: "Elsewhere".into(),
            namespace: "root\\StandardCimv2".into(),
            wql: "SELECT * FROM Win32_Process".into(),
            ..Default::default()
        });
        cfg.push_history("SELECT * FROM Win32_Process", "root\\CIMV2");

        assert!(cfg.note_query_run("SELECT * FROM Win32_Process", "root\\CIMV2", 412, 187));

        assert_eq!(cfg.history[0].elapsed_ms, Some(412));
        assert_eq!(cfg.history[0].rows, Some(187));
        assert_eq!(cfg.saved[0].last_ms, Some(412));
        assert_eq!(cfg.saved[0].last_rows, Some(187));
        assert_eq!(
            cfg.saved[1].last_ms, None,
            "a query in another namespace took another namespace's timing"
        );

        // A run nobody recorded matches nothing and changes nothing.
        assert!(!cfg.note_query_run("SELECT * FROM Win32_Share", "root\\CIMV2", 9, 0));
    }

    // -- library -----------------------------------------------------------

    /// Re-saving over a name keeps how the user filed it, and picks up the
    /// metrics of the run it was saved from.
    #[test]
    fn resaving_keeps_the_folder_and_favourite_and_takes_the_last_run() {
        let mut cfg = Config::default();
        cfg.saved.push(SavedQuery {
            name: "Processes".into(),
            namespace: "root\\CIMV2".into(),
            wql: "SELECT * FROM Win32_Process".into(),
            folder: "Triage".into(),
            fav: true,
            ..Default::default()
        });
        cfg.push_history("SELECT Name FROM Win32_Process", "root\\CIMV2");
        cfg.note_query_run("SELECT Name FROM Win32_Process", "root\\CIMV2", 61, 187);

        cfg.save_query(
            "Processes".into(),
            "root\\CIMV2".into(),
            "SELECT Name FROM Win32_Process".into(),
        );

        assert_eq!(cfg.saved.len(), 1, "the name was duplicated");
        let q = &cfg.saved[0];
        assert_eq!(q.folder, "Triage");
        assert!(q.fav);
        assert_eq!(q.last_ms, Some(61));
        assert_eq!(q.last_rows, Some(187));
    }

    /// Task 4.17's acceptance: export then import is lossless.
    #[test]
    fn the_library_round_trips_through_its_export_format() {
        let cfg = Config {
            saved: vec![
                SavedQuery {
                    name: "Autoruns".into(),
                    namespace: "root\\subscription".into(),
                    wql: "SELECT * FROM __FilterToConsumerBinding".into(),
                    folder: "Hunting".into(),
                    fav: true,
                    author: "CORP\\ana".into(),
                    last_ms: Some(88),
                    last_rows: Some(3),
                },
                SavedQuery {
                    name: "Services".into(),
                    namespace: "root\\CIMV2".into(),
                    wql: "SELECT Name, State FROM Win32_Service".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        let json = cfg.library_to_json();
        let back = Config::library_from_json(&json).expect("exported library must re-parse");
        assert_eq!(back, cfg.saved);

        // Importing it into an empty library reproduces it exactly.
        let mut fresh = Config::default();
        let (added, replaced) = fresh.merge_library(back);
        assert_eq!((added, replaced), (2, 0));
        assert_eq!(fresh.saved.len(), 2);
    }

    /// An import replaces by name rather than duplicating, and a nameless entry
    /// is dropped instead of creating an unclickable card.
    #[test]
    fn import_replaces_by_name_and_drops_nameless_entries() {
        let mut cfg = Config::default();
        cfg.saved.push(SavedQuery {
            name: "Services".into(),
            namespace: "root\\CIMV2".into(),
            wql: "SELECT * FROM Win32_Service".into(),
            ..Default::default()
        });

        let (added, replaced) = cfg.merge_library(vec![
            SavedQuery {
                name: "Services".into(),
                namespace: "root\\CIMV2".into(),
                wql: "SELECT Name FROM Win32_Service".into(),
                folder: "Ops".into(),
                ..Default::default()
            },
            SavedQuery {
                name: "  ".into(),
                ..Default::default()
            },
        ]);

        assert_eq!((added, replaced), (0, 1));
        assert_eq!(cfg.saved.len(), 1);
        assert_eq!(cfg.saved[0].wql, "SELECT Name FROM Win32_Service");
        assert_eq!(cfg.saved[0].folder, "Ops");
    }

    /// Favourites lead, then folders, then names — and `folders()` reports each
    /// folder once, without inventing one for the ungrouped queries.
    #[test]
    fn the_library_sorts_favourites_first_and_lists_each_folder_once() {
        let mut cfg = Config {
            saved: vec![
                SavedQuery {
                    name: "zeta".into(),
                    folder: "Ops".into(),
                    ..Default::default()
                },
                SavedQuery {
                    name: "alpha".into(),
                    ..Default::default()
                },
                SavedQuery {
                    name: "beta".into(),
                    folder: "Ops".into(),
                    fav: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        cfg.sort_saved();

        let names: Vec<&str> = cfg.saved.iter().map(|q| q.name.as_str()).collect();
        assert_eq!(names, vec!["beta", "alpha", "zeta"]);
        assert_eq!(cfg.folders(), vec!["Ops".to_string()]);
    }

    // -- write debounce ----------------------------------------------------

    /// Task 4.7's acceptance, as a policy rather than as a filesystem test: a
    /// clean config owes nothing, the first change goes out at once, and every
    /// change inside the cooldown is folded into a single later write. Ten rapid
    /// runs therefore cost two writes, not ten.
    #[test]
    fn a_burst_of_changes_costs_two_writes() {
        let start = Instant::now();
        let mut clock = SaveClock::default();
        assert_eq!(
            clock.due(start),
            Due::Nothing,
            "a clean config owes nothing"
        );

        let mut writes = 0;
        // Ten runs, 40 ms apart: comfortably inside one two-second cooldown.
        for n in 0..10u32 {
            let now = start + Duration::from_millis(u64::from(n) * 40);
            clock.dirty = true;
            if clock.due(now) == Due::Now {
                clock = SaveClock {
                    dirty: false,
                    last_write: Some(now),
                };
                writes += 1;
            }
        }
        assert_eq!(writes, 1, "the burst should have collapsed to one write");

        // The pending change is still owed, and lands once the cooldown expires.
        assert!(matches!(
            clock.due(start + Duration::from_millis(400)),
            Due::In(_)
        ));
        assert_eq!(clock.due(start + SAVE_DEBOUNCE), Due::Now);
        writes += 1;
        assert!(writes <= 2, "10 rapid queries produced {writes} writes");
    }

    /// The wait reported to the caller is the time actually left, so a frame
    /// loop scheduling a wake-up from it does not spin.
    #[test]
    fn the_reported_wait_is_the_time_remaining() {
        let start = Instant::now();
        let clock = SaveClock {
            dirty: true,
            last_write: Some(start),
        };
        match clock.due(start + Duration::from_millis(500)) {
            Due::In(left) => assert_eq!(left, SAVE_DEBOUNCE - Duration::from_millis(500)),
            other => panic!("expected a wait, got {other:?}"),
        }
    }
}
