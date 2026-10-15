//! The per-server "Local Grok Bots" setting: two switches, both OFF by
//! default.
//!
//! - **Show** — detect Grok Bot on this computer, list its agents and read
//!   their chat.
//! - **Send** — let a person signed in to this server send messages to a
//!   Grok Bot agent as themselves. Each send runs a billed Grok turn.
//!   Send only takes effect while Show is on.
//!
//! Stored as its own small JSON file under `~/.k2` (not in the K2 settings
//! JSON every client mirrors) so it is server state, never synced from a
//! client, and owner/admin only through `/cli/grokbots/settings`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::fs_atomic::atomic_write;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LocalGrokBotsSettings {
    pub show: bool,
    pub send: bool,
}

impl LocalGrokBotsSettings {
    /// Send is honoured only while Show is on.
    pub fn send_allowed(&self) -> bool {
        self.show && self.send
    }
}

fn path() -> PathBuf {
    crate::paths::k2_home()
        .join("external-agents")
        .join("grokbots.json")
}

/// Read the setting. A missing or unreadable file is the default (both off).
pub fn load() -> LocalGrokBotsSettings {
    std::fs::read(path())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Write the setting atomically.
pub fn save(s: &LocalGrokBotsSettings) -> std::io::Result<()> {
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
    use crate::test_env::TempHome;

    #[test]
    fn both_switches_are_off_by_default_and_round_trip() {
        let _home = TempHome::new();
        let s = load();
        assert_eq!(
            s,
            LocalGrokBotsSettings {
                show: false,
                send: false
            }
        );
        assert!(!s.send_allowed());
        save(&LocalGrokBotsSettings {
            show: true,
            send: true,
        })
        .expect("save");
        let back = load();
        assert!(back.show && back.send && back.send_allowed());
        save(&LocalGrokBotsSettings {
            show: false,
            send: true,
        })
        .expect("save");
        assert!(!load().send_allowed(), "send needs show");
    }
}
