//! Terminal-ID format helpers for agent chat + heartbeat sessions.
//!
//! Format reference (must match `src/renderer/lib/terminal-id.ts`):
//!
//! - `agent-chat:<project_id>:<agent>`                — chat tab session
//! - `agent-chat:wt:<workspace_id>`                   — worktree-scoped chat
//! - `agent-chat:<project_id>:<agent>:hb:<heartbeat>` — per-heartbeat session
//!
//! `:` is the separator so hyphenated agent / heartbeat names parse cleanly.
//!
//! Legacy formats (pre-0.36.0):
//!
//! - `agent-chat-<agent>`         — collided across projects sharing an agent
//! - `agent-chat-wt-<workspace>`  — already namespaced; renamed for consistency
//!
//! Both legacy forms are recognised by [`parse`] for one release window so
//! unmigrated rows / stale CLI references resolve correctly while the
//! migration drains.

use serde::{Deserialize, Serialize};

const PREFIX: &str = "agent-chat:";
const LEGACY_PREFIX: &str = "agent-chat-";
const WORKTREE_TAG: &str = "wt";
const HEARTBEAT_TAG: &str = "hb";

/// What kind of session a terminal id refers to. Returned by [`parse`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TerminalIdKind {
    /// Chat tab session bound to (project_id, agent).
    AgentChat { project_id: String, agent: String },
    /// Per-heartbeat session bound to (project_id, agent, heartbeat).
    HeartbeatChat {
        project_id: String,
        agent: String,
        heartbeat: String,
    },
    /// Worktree-scoped session.
    Worktree { workspace_id: String },
    /// Legacy unscoped form (`agent-chat-<agent>`) — pre-0.36.0.
    LegacyAgentChat { agent: String },
    /// Legacy worktree form (`agent-chat-wt-<workspace>`) — pre-0.36.0.
    LegacyWorktree { workspace_id: String },
}

/// Build the terminal id for an agent's Chat tab.
///
/// **0.37.5:** the agent name is no longer part of the canonical
/// shape — post-unification the workspace's project_id alone is
/// unique. The `agent` parameter is kept for back-compat at call
/// sites but it's IGNORED in the produced id. Mirrors the renderer
/// `terminal-id.ts agentChatId()` helper. Pre-0.37.5 the id was
/// `agent-chat:<project_id>:<agent>`; rebuilding it that way
/// caused the renderer-side `k2so_agents_lock` call to write the
/// wrong shape to `workspace_sessions.terminal_id`, bypassing the
/// SQL migration 0042 (C3PO 5c80bef1).
pub fn agent_chat_id(project_id: &str, agent: &str) -> String {
    let _ = agent;
    format!("{PREFIX}{project_id}")
}

/// Build the terminal id for a worktree-scoped chat session.
pub fn worktree_chat_id(workspace_id: &str) -> String {
    format!("{PREFIX}{WORKTREE_TAG}:{workspace_id}")
}

/// Build the terminal id for a per-heartbeat session.
pub fn heartbeat_chat_id(project_id: &str, agent: &str, heartbeat: &str) -> String {
    format!("{PREFIX}{project_id}:{agent}:{HEARTBEAT_TAG}:{heartbeat}")
}

/// Whether this id is in the legacy pre-0.36.0 form.
pub fn is_legacy(id: &str) -> bool {
    id.starts_with(LEGACY_PREFIX) && !id.starts_with(PREFIX)
}

/// Parse a terminal id into its kind. Recognises both new (`:`-delimited)
/// and legacy (`-`-delimited) forms.
///
/// Returns `None` for ids that don't match any known agent-chat shape.
pub fn parse(id: &str) -> Option<TerminalIdKind> {
    if let Some(rest) = id.strip_prefix(PREFIX) {
        return parse_namespaced(rest);
    }
    if let Some(rest) = id.strip_prefix(LEGACY_PREFIX) {
        return parse_legacy(rest);
    }
    None
}

fn parse_namespaced(rest: &str) -> Option<TerminalIdKind> {
    // First segment is either WORKTREE_TAG or project_id.
    let mut parts = rest.splitn(2, ':');
    let head = parts.next()?;
    let tail = match parts.next() {
        Some(t) => t,
        None => {
            // 0.37.5 bare-pid form: `agent-chat:<projectId>` (no
            // agent suffix). Post-unification the project_id alone
            // is canonical.
            if head.is_empty() {
                return None;
            }
            return Some(TerminalIdKind::AgentChat {
                project_id: head.to_string(),
                agent: String::new(),
            });
        }
    };

    if head == WORKTREE_TAG {
        if tail.is_empty() {
            return None;
        }
        return Some(TerminalIdKind::Worktree {
            workspace_id: tail.to_string(),
        });
    }

    // head = project_id, tail = "<agent>" or "<agent>:hb:<heartbeat>"
    let project_id = head.to_string();

    // Heartbeat form: split tail on ":hb:" to handle agents that contain ':'
    // (none today, but the format reserves them).
    let hb_marker = format!(":{HEARTBEAT_TAG}:");
    if let Some(idx) = tail.find(&hb_marker) {
        let agent = tail[..idx].to_string();
        let heartbeat = tail[idx + hb_marker.len()..].to_string();
        if agent.is_empty() || heartbeat.is_empty() {
            return None;
        }
        return Some(TerminalIdKind::HeartbeatChat {
            project_id,
            agent,
            heartbeat,
        });
    }

    if tail.is_empty() {
        return None;
    }
    Some(TerminalIdKind::AgentChat {
        project_id,
        agent: tail.to_string(),
    })
}

fn parse_legacy(rest: &str) -> Option<TerminalIdKind> {
    if let Some(workspace_id) = rest.strip_prefix("wt-") {
        if workspace_id.is_empty() {
            return None;
        }
        return Some(TerminalIdKind::LegacyWorktree {
            workspace_id: workspace_id.to_string(),
        });
    }
    if rest.is_empty() {
        return None;
    }
    Some(TerminalIdKind::LegacyAgentChat {
        agent: rest.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_agent_chat_id() {
        // **0.37.5:** agent name no longer in the canonical id.
        // Reverting `agent_chat_id` to the pre-0.37.5 prefix shape
        // MUST flip these assertions to "FAIL".
        assert_eq!(agent_chat_id("p_abc", "manager"), "agent-chat:p_abc");
        assert_eq!(agent_chat_id("p_abc", "scout"), "agent-chat:p_abc");
    }

    #[test]
    fn builds_worktree_id() {
        assert_eq!(worktree_chat_id("ws_xyz"), "agent-chat:wt:ws_xyz");
    }

    #[test]
    fn builds_heartbeat_id() {
        assert_eq!(
            heartbeat_chat_id("p_abc", "manager", "triage"),
            "agent-chat:p_abc:manager:hb:triage"
        );
    }

    #[test]
    fn parses_agent_chat() {
        assert_eq!(
            parse("agent-chat:p_abc:manager"),
            Some(TerminalIdKind::AgentChat {
                project_id: "p_abc".to_string(),
                agent: "manager".to_string(),
            })
        );
    }

    #[test]
    fn parses_heartbeat_with_hyphenated_names() {
        // Agent "pod-leader", heartbeat "daily-brief" — exercises that
        // hyphens in components don't confuse the `:hb:` marker split.
        assert_eq!(
            parse("agent-chat:p_xyz:pod-leader:hb:daily-brief"),
            Some(TerminalIdKind::HeartbeatChat {
                project_id: "p_xyz".to_string(),
                agent: "pod-leader".to_string(),
                heartbeat: "daily-brief".to_string(),
            })
        );
    }

    #[test]
    fn parses_worktree() {
        assert_eq!(
            parse("agent-chat:wt:ws_abc"),
            Some(TerminalIdKind::Worktree {
                workspace_id: "ws_abc".to_string(),
            })
        );
    }

    #[test]
    fn parses_legacy_unscoped() {
        assert_eq!(
            parse("agent-chat-manager"),
            Some(TerminalIdKind::LegacyAgentChat {
                agent: "manager".to_string(),
            })
        );
    }

    #[test]
    fn parses_legacy_worktree() {
        assert_eq!(
            parse("agent-chat-wt-ws_abc"),
            Some(TerminalIdKind::LegacyWorktree {
                workspace_id: "ws_abc".to_string(),
            })
        );
    }

    #[test]
    fn rejects_non_agent_ids() {
        assert_eq!(parse("term-7"), None);
        assert_eq!(parse(""), None);
    }

    #[test]
    fn rejects_malformed_ids() {
        // bare prefix with no body
        assert_eq!(parse("agent-chat:"), None);
        // worktree tag with no workspace id
        assert_eq!(parse("agent-chat:wt:"), None);
        // namespaced form missing agent
        assert_eq!(parse("agent-chat:p_abc:"), None);
        // heartbeat form with empty heartbeat name
        assert_eq!(parse("agent-chat:p_abc:manager:hb:"), None);
        // heartbeat form with empty agent name
        assert_eq!(parse("agent-chat:p_abc::hb:triage"), None);
    }

    #[test]
    fn is_legacy_recognises_both_old_forms() {
        assert!(is_legacy("agent-chat-manager"));
        assert!(is_legacy("agent-chat-wt-ws_abc"));
        // New form is NOT legacy
        assert!(!is_legacy("agent-chat:p_abc:manager"));
        assert!(!is_legacy("agent-chat:wt:ws_abc"));
        // Random ids
        assert!(!is_legacy("term-7"));
    }

    #[test]
    fn parses_bare_pid_agent_chat() {
        // 0.37.5: `agent-chat:<project_id>` (no agent suffix).
        assert_eq!(
            parse("agent-chat:p_abc"),
            Some(TerminalIdKind::AgentChat {
                project_id: "p_abc".to_string(),
                agent: String::new(),
            })
        );
    }

    #[test]
    fn round_trip_agent_chat() {
        // **0.37.5:** `agent_chat_id` ignores the agent argument
        // and produces the bare-pid form. Round-trip parses back
        // to AgentChat with empty agent.
        let id = agent_chat_id("p_1", "alice");
        assert_eq!(id, "agent-chat:p_1");
        match parse(&id) {
            Some(TerminalIdKind::AgentChat { project_id, agent }) => {
                assert_eq!(project_id, "p_1");
                assert_eq!(agent, "");
            }
            other => panic!("expected AgentChat, got {other:?}"),
        }
    }

    #[test]
    fn round_trip_heartbeat() {
        let id = heartbeat_chat_id("p_1", "manager", "triage");
        match parse(&id) {
            Some(TerminalIdKind::HeartbeatChat {
                project_id,
                agent,
                heartbeat,
            }) => {
                assert_eq!(project_id, "p_1");
                assert_eq!(agent, "manager");
                assert_eq!(heartbeat, "triage");
            }
            other => panic!("expected HeartbeatChat, got {other:?}"),
        }
    }
}
