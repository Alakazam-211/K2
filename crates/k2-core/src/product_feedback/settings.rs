//! The server's Send feedback to K2 switches (PF8, PF10a, PF19):
//!
//! - `enabled` (default on): off hides Send and refuses `send` with
//!   `disabled`. Air-gap forces it off whatever the file says.
//! - `agents` (default on): off refuses agents with `agents_off`.
//! - `agentDiagnostics` (default **off**): on lets an agent include the K2
//!   details; only a person (the owner or an admin) may set it.
//!
//! A small JSON file under `~/.k2` (server state, never synced from a
//! client), written only through `/cli/product-feedback/settings`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::fs_atomic::atomic_write;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub enabled: bool,
    pub agents: bool,
    pub agent_diagnostics: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { enabled: true, agents: true, agent_diagnostics: false }
    }
}

pub fn path() -> PathBuf {
    crate::paths::k2_home().join("product-feedback.json")
}

/// The switches. A missing or unreadable file is the defaults.
pub fn load() -> Settings {
    std::fs::read(path())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save(s: &Settings) -> std::io::Result<()> {
    let p = path();
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let body = serde_json::to_vec_pretty(s).map_err(std::io::Error::other)?;
    atomic_write(&p, &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_on_on_off_and_round_trip() {
        let _home = crate::test_env::TempHome::new();
        assert_eq!(load(), Settings { enabled: true, agents: true, agent_diagnostics: false });
        let s = Settings { enabled: false, agents: false, agent_diagnostics: true };
        save(&s).unwrap();
        assert_eq!(load(), s);
        let raw = std::fs::read_to_string(path()).unwrap();
        assert!(raw.contains("\"agentDiagnostics\": true"), "{raw}");
        // A partial file keeps the other defaults.
        std::fs::write(path(), r#"{"agents": false}"#).unwrap();
        assert_eq!(load(), Settings { enabled: true, agents: false, agent_diagnostics: false });
    }
}
