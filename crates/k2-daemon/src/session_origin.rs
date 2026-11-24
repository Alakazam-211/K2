//! How K2 started a session (0.46.18): the one thing
//! [`crate::sidecar_routes::caller_from_session`] reads to tell a person's
//! own shell tab from an agent.
//!
//! Before 0.46.18 the caller was classed by the program its session ran (a
//! known LLM CLI was an agent, anything else a shell tab). The program is
//! the agent's own choice (an AGENT.md `launch:` block can say `bash`, `npx`
//! or a wrapper), so the class now comes from the spawn instead:
//!
//! - [`SessionOrigin::PersonShellTab`]: a person's K2 client opened a plain
//!   shell tab. Only `/cli/sessions/v2/spawn` called with a person's
//!   credential (the owner token or a Connect login) can record it, only for
//!   a `tab-*` key, and only when K2 starts the plain login shell
//!   ([`k2_core::cli_folder_trust::needs_gate`] is false). That is the
//!   person's terminal; what they type in it is theirs.
//! - [`SessionOrigin::Agent`]: everything else K2 starts (the workspace
//!   agent's pinned chat, heartbeats, sidecars, hires, `k2 terminal spawn`,
//!   remote-session shells, `/v1` sessions, a person's LLM tab).
//!
//! Fail closed: a live session with no recorded origin is an agent. The
//! origin rides the k2-ptyd meta (`origin`), so an adopted session keeps
//! it across a daemon restart; a session an older daemon left in k2-ptyd
//! (no `origin` in its meta) is an agent's ([`from_meta`]) until the person
//! opens a new tab.

use std::collections::HashMap;
use std::sync::Mutex;

use k2_core::session::SessionId;

/// How K2 started a session. See the module doc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionOrigin {
    /// A person's K2 client opened a plain shell tab.
    PersonShellTab,
    /// K2 started it for an agent (or for anything that isn't a person's
    /// own shell tab).
    Agent,
}

impl SessionOrigin {
    /// The k2-ptyd meta value.
    pub fn as_str(self) -> &'static str {
        match self {
            SessionOrigin::PersonShellTab => "person_shell_tab",
            SessionOrigin::Agent => "agent",
        }
    }

    /// A k2-ptyd meta value; an unknown one is an agent (fail closed).
    pub fn parse(raw: &str) -> SessionOrigin {
        match raw {
            "person_shell_tab" => SessionOrigin::PersonShellTab,
            _ => SessionOrigin::Agent,
        }
    }
}

/// The k2-ptyd meta key.
pub const META_KEY: &str = "origin";

static ORIGINS: Mutex<Option<HashMap<SessionId, SessionOrigin>>> = Mutex::new(None);

/// Record how K2 started `id`. Called at spawn (and at adoption) before
/// the session is registered, so no caller ever sees it unclassified.
pub fn record(id: SessionId, origin: SessionOrigin) {
    let mut g = ORIGINS.lock().unwrap_or_else(|p| p.into_inner());
    g.get_or_insert_with(HashMap::new).insert(id, origin);
}

/// How K2 started `id`; `None` when nothing was recorded (callers treat
/// that as [`SessionOrigin::Agent`]).
pub fn of(id: &SessionId) -> Option<SessionOrigin> {
    let g = ORIGINS.lock().unwrap_or_else(|p| p.into_inner());
    g.as_ref().and_then(|m| m.get(id).copied())
}

/// The session ended (`v2_session_map::unregister`).
pub fn forget(id: &SessionId) {
    let mut g = ORIGINS.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(m) = g.as_mut() {
        m.remove(id);
    }
}

/// Is this a person's own shell tab? Only for a spawn a person's client
/// asked for (`opened_by_person`): a `tab-*` key running the plain login
/// shell (no command beyond it, no args but `-l`/`-i`, no env beyond K2's
/// own). Anything else, a harness, a wrapper, a canonical or heartbeat
/// key, is an agent.
pub fn for_spawn(
    opened_by_person: bool,
    agent_name: &str,
    program: Option<&str>,
    args: &[String],
    env: &HashMap<String, String>,
) -> SessionOrigin {
    if opened_by_person
        && k2_core::workspace_session_handles::is_tab_agent_name(agent_name)
        && !k2_core::cli_folder_trust::needs_gate(program, args, env)
    {
        SessionOrigin::PersonShellTab
    } else {
        SessionOrigin::Agent
    }
}

/// The origin for an adopted session, from its k2-ptyd meta. A session an
/// older daemon (0.46.5–0.46.17) left in k2-ptyd has no `origin` there and
/// is always an agent's, whatever its key or program (fail closed): its
/// shape can't prove a person opened it. A person gets a shell tab again by
/// opening a new tab, which records its origin.
pub fn from_meta(meta: &serde_json::Value) -> SessionOrigin {
    match meta.get(META_KEY).and_then(serde_json::Value::as_str) {
        Some(raw) => SessionOrigin::parse(raw),
        None => SessionOrigin::Agent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn only_a_persons_plain_tab_is_a_shell_tab() {
        let none = HashMap::new();
        assert_eq!(for_spawn(true, "tab-1", None, &[], &none), SessionOrigin::PersonShellTab);
        assert_eq!(for_spawn(true, "tab-1", Some("$SHELL"), &args(&["-l"]), &none), SessionOrigin::PersonShellTab);
        // Not the person's client: whatever it runs.
        assert_eq!(for_spawn(false, "tab-1", None, &[], &none), SessionOrigin::Agent);
        // Not a tab's key: the pinned chat, a heartbeat, a terminal.
        for key in ["8f0c1d9e-0000-4000-8000-000000000000", "p:hb:daily", "terminal-0a1b2c3d", "remote-shell-g-1", "api-x-1"] {
            assert_eq!(for_spawn(true, key, None, &[], &none), SessionOrigin::Agent, "{key}");
        }
        // A launcher, a wrapper, a harness, a shell with a script or env.
        for (cmd, a) in [
            ("/usr/bin/npx", vec![]),
            ("/opt/agent/run.sh", vec![]),
            ("claude", vec![]),
            ("$SHELL", vec!["-c", "claude -p hi"]),
        ] {
            assert_eq!(for_spawn(true, "tab-1", Some(cmd), &args(&a), &none), SessionOrigin::Agent, "{cmd} {a:?}");
        }
        let env = HashMap::from([("PROMPT_COMMAND".to_string(), "claude".to_string())]);
        assert_eq!(for_spawn(true, "tab-1", None, &[], &env), SessionOrigin::Agent, "env beyond K2's own");
    }

    #[test]
    fn the_record_is_per_session_and_missing_is_none() {
        let (a, b) = (SessionId::new(), SessionId::new());
        record(a, SessionOrigin::PersonShellTab);
        record(b, SessionOrigin::Agent);
        assert_eq!(of(&a), Some(SessionOrigin::PersonShellTab));
        assert_eq!(of(&b), Some(SessionOrigin::Agent));
        assert_eq!(of(&SessionId::new()), None, "never recorded");
        forget(&a);
        assert_eq!(of(&a), None, "forgotten when the session ends");
    }

    #[test]
    fn an_adopted_session_keeps_its_recorded_origin() {
        let meta = json!({ "key": "tab-1", "spawn_site": "v2_spawn", "origin": "person_shell_tab" });
        assert_eq!(from_meta(&meta), SessionOrigin::PersonShellTab);
        let meta = json!({ "key": "tab-1", "spawn_site": "v2_spawn", "origin": "agent" });
        assert_eq!(from_meta(&meta), SessionOrigin::Agent);
        // An unknown value fails closed.
        let meta = json!({ "key": "tab-1", "spawn_site": "v2_spawn", "origin": "owner" });
        assert_eq!(from_meta(&meta), SessionOrigin::Agent);
    }

    /// An older daemon's session (no `origin` in its meta) is an agent's,
    /// whatever its key, spawn site or program: even the exact shape a
    /// person's client made (a plain shell under `tab-*` from `v2_spawn`).
    #[test]
    fn a_session_with_no_origin_is_an_agent_whatever_its_key_or_program() {
        for meta in [
            json!({ "key": "tab-1", "spawn_site": "v2_spawn" }),
            json!({ "key": "tab-1", "spawn_site": "v2_spawn", "program": null, "args": [] }),
            json!({ "key": "tab-1", "spawn_site": "spawn", "program": "bash" }),
            json!({ "key": "8f0c1d9e-0000-4000-8000-000000000000", "spawn_site": "v2_spawn", "program": "bash" }),
            json!({ "key": "p:hb:daily", "spawn_site": "spawn", "program": "/usr/bin/npx" }),
            json!({ "key": "terminal-0a1b2c3d", "spawn_site": "spawn", "program": "/opt/agent/run.sh" }),
            json!({ "key": "tab-1" }),
            json!({}),
            json!(null),
        ] {
            assert_eq!(from_meta(&meta), SessionOrigin::Agent, "{meta}");
        }
    }
}
