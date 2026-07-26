//! Persisted user config — query history and saved queries.
//!
//! Stored as JSON at `%APPDATA%\VMI-Scope\config.json`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A user-named query.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SavedQuery {
    pub name: String,
    pub namespace: String,
    pub wql: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub history: Vec<String>,
    #[serde(default)]
    pub saved: Vec<SavedQuery>,
}

fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(base).join("VMI-Scope").join("config.json"))
}

impl Config {
    pub fn load() -> Config {
        config_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
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
    pub fn push_history(&mut self, wql: &str) {
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
    pub fn save_query(&mut self, name: String, namespace: String, wql: String) {
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
