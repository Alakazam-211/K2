//! The server's Send feedback to K2 switches (PF8, PF10a, PF19, and the
//! agent opt-in of 0.46.17):
//!
//! - `enabled` (default on): off hides Send and refuses `send` with
//!   `disabled`. Air-gap forces it off whatever the file says.
//! - `agents`: agents may report **K2 CLI problems** on their own (the main
//!   switch; the field name is kept from 0.46.11 so older clients still
//!   read something true).
//! - `agentPlatform`: agents may also report anything else about K2. Only
//!   with `agents` on.
//! - `agentAttach`: the parts of the K2 details an agent may attach
//!   ([`ATTACH_PARTS`], the person's dialog sections, each its own choice).
//!   The versions always go with every report; nothing else unless listed.
//! - `agentsAskedAt` / `agentsAskedBy`: when, and by whom (`owner` or a
//!   login's username), a person answered the agent question.
//! - `agentDiagnostics`: the 0.46.11 switch, kept in the file for an older
//!   daemon after a downgrade (written as "any part allowed"); never read
//!   for a decision any more.
//!
//! **The migration (0.46.17, a deliberate behaviour change):** a file
//! without `agentsAskedAt` reads as everything **off** for agents, whatever
//! `agents` says. Until 0.46.16 agents were on by default; from 0.46.17 an
//! agent may report only after a person said yes (onboarding, or the
//! prompt after the update). A missing file is the same: off until asked.
//!
//! A small JSON file under `~/.k2` (server state, never synced from a
//! client), written only through `/cli/product-feedback/settings`, by a
//! person (the owner or an admin).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::fs_atomic::atomic_write;
use crate::product_name;

/// The parts of the K2 details an agent may attach, as `(id, label)`: the
/// person's dialog sections (`super::diagnostics`), without the versions
/// (always sent) and "Where you were" (a person's page, never an agent's).
pub const ATTACH_PARTS: &[(&str, &str)] = &[
    (super::diagnostics::S_COMPUTER, "Computer size and free space"),
    (super::diagnostics::S_STATE, "The names of settings that are on, and counts"),
    (super::diagnostics::S_TOOLS, "Which AI tools are installed"),
    (super::diagnostics::S_ERRORS, "Failure-looking lines from the last 24 hours"),
    (super::diagnostics::S_TAIL, concat!("The end of ", product_name!(), "'s log")),
];

/// The section that goes with every agent report's attached details.
pub const ALWAYS_PART: &str = super::diagnostics::S_VERSIONS;

/// What an agent report is about.
pub const SCOPE_CLI: &str = "cli";
pub const SCOPE_PLATFORM: &str = "platform";
pub const SCOPES: &[&str] = &[SCOPE_CLI, SCOPE_PLATFORM];

pub fn is_attach_part(id: &str) -> bool {
    ATTACH_PARTS.iter().any(|(p, _)| *p == id)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub enabled: bool,
    /// The CLI switch (see the module docs). Read through [`Settings::agent_policy`].
    pub agents: bool,
    /// Legacy (0.46.11): written for an older daemon, never decided on.
    pub agent_diagnostics: bool,
    pub agent_platform: bool,
    pub agent_attach: Vec<String>,
    /// Milliseconds since the epoch; `None` = never asked (all off).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agents_asked_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agents_asked_by: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            enabled: true,
            agents: false,
            agent_diagnostics: false,
            agent_platform: false,
            agent_attach: Vec::new(),
            agents_asked_at: None,
            agents_asked_by: None,
        }
    }
}

/// What agents may do, after the migration rule.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AgentPolicy {
    /// Report K2 CLI problems.
    pub cli: bool,
    /// Report anything else about K2 (only with `cli`).
    pub platform: bool,
    /// The [`ATTACH_PARTS`] ids allowed, in their order (empty unless `cli`).
    pub attach: Vec<String>,
}

impl AgentPolicy {
    /// Whether an agent may send a report of `scope`.
    pub fn allows(&self, scope: &str) -> bool {
        match scope {
            SCOPE_CLI => self.cli,
            SCOPE_PLATFORM => self.platform,
            _ => false,
        }
    }

    /// The scopes allowed, `cli` first.
    pub fn scopes(&self) -> Vec<&'static str> {
        SCOPES.iter().copied().filter(|s| self.allows(s)).collect()
    }
}

impl Settings {
    /// Whether a person answered the agent question.
    pub fn agents_asked(&self) -> bool {
        self.agents_asked_at.is_some()
    }

    /// What agents may do. **Not asked = all off** (the 0.46.17 migration:
    /// an old file's `agents: true` counts for nothing until a person
    /// answers).
    pub fn agent_policy(&self) -> AgentPolicy {
        if !self.agents_asked() || !self.agents {
            return AgentPolicy::default();
        }
        AgentPolicy {
            cli: true,
            platform: self.agent_platform,
            attach: ATTACH_PARTS
                .iter()
                .map(|(id, _)| *id)
                .filter(|id| self.agent_attach.iter().any(|a| a == id))
                .map(str::to_string)
                .collect(),
        }
    }

    /// Store a person's answer: the three agent switches, stamped with when
    /// and who. `platform` without `cli` is stored off; unknown parts are
    /// dropped (the route refuses them first).
    pub fn answer(&mut self, cli: bool, platform: bool, attach: &[String], by: &str, now_ms: i64) {
        self.agents = cli;
        self.agent_platform = cli && platform;
        self.agent_attach = ATTACH_PARTS
            .iter()
            .map(|(id, _)| *id)
            .filter(|id| attach.iter().any(|a| a == id))
            .map(str::to_string)
            .collect();
        self.agent_diagnostics = cli && !self.agent_attach.is_empty();
        self.agents_asked_at = Some(now_ms);
        self.agents_asked_by = Some(by.to_string());
    }
}

/// Agents may report problems with `send-feedback` now: sending is on and a
/// person allowed agents (at least CLI problems). The same rule as the
/// AGENTS.md section (`teach::agents_md_section`) and the refusal's
/// no-card feedback line (`permission::request::PermissionRefusal`'s
/// `feedback`, 0.46.18).
pub fn agents_may_report() -> bool {
    let s = load();
    s.enabled && s.agent_policy().cli
}

pub fn path() -> PathBuf {
    crate::paths::k2_home().join("product-feedback.json")
}

/// The switches. A missing or unreadable file is the defaults (agents off
/// until asked).
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

    fn parts(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn defaults_and_round_trip() {
        let _home = crate::test_env::TempHome::new();
        let d = load();
        assert_eq!(d, Settings::default());
        assert!(d.enabled);
        assert_eq!(d.agent_policy(), AgentPolicy::default(), "a new install: agents off until asked");
        let mut s = Settings::default();
        s.answer(true, true, &parts(&["log-tail", "computer"]), "owner", 42);
        save(&s).unwrap();
        assert_eq!(load(), s);
        let raw = std::fs::read_to_string(path()).unwrap();
        for want in ["\"agents\": true", "\"agentPlatform\": true", "\"agentsAskedAt\": 42", "\"agentsAskedBy\": \"owner\"", "\"agentDiagnostics\": true"] {
            assert!(raw.contains(want), "{want} in {raw}");
        }
        assert_eq!(
            load().agent_policy(),
            AgentPolicy { cli: true, platform: true, attach: parts(&["computer", "log-tail"]) },
            "parts in their own order"
        );
    }

    #[test]
    fn an_old_file_reads_as_everything_off_for_agents() {
        let _home = crate::test_env::TempHome::new();
        // 0.46.11–0.46.16 wrote exactly these three keys, agents on by default.
        std::fs::write(path(), r#"{"enabled": true, "agents": true, "agentDiagnostics": true}"#).unwrap();
        let s = load();
        assert!(s.agents && s.agent_diagnostics, "kept as read, for an older daemon");
        assert!(!s.agents_asked());
        assert_eq!(s.agent_policy(), AgentPolicy::default(), "not asked = all off");
        assert!(!s.agent_policy().allows(SCOPE_CLI) && !s.agent_policy().allows(SCOPE_PLATFORM));
        // A partial file keeps the other defaults.
        std::fs::write(path(), r#"{"agents": false}"#).unwrap();
        assert_eq!(load(), Settings::default());
    }

    #[test]
    fn platform_needs_cli_and_parts_are_known_ones() {
        let mut s = Settings::default();
        s.answer(false, true, &parts(&["computer"]), "al", 1);
        assert_eq!(s.agent_policy(), AgentPolicy::default());
        assert!(!s.agent_platform && !s.agent_diagnostics && s.agents_asked());
        s.answer(true, false, &parts(&["versions", "context", "nope", "agent-tools"]), "owner", 2);
        let p = s.agent_policy();
        assert_eq!(p, AgentPolicy { cli: true, platform: false, attach: parts(&["agent-tools"]) });
        assert_eq!(p.scopes(), vec!["cli"]);
        assert!(p.allows("cli") && !p.allows("platform") && !p.allows("other"));
    }
}
