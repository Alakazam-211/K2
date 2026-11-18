//! The shell CLI's tool table (`cli/src/cli-tools-policy.sh`, between
//! `BEGIN_CLI_TOOL_POLICY` and `END_CLI_TOOL_POLICY`): which catalog tool
//! a command line is ([`tool_id`], `_cli_tool_id_for_verb`) and whether
//! that tool needs the session's own passport ([`is_id`],
//! `_cli_tool_default_mode`).
//!
//! The shell's router asks it twice before any verb runs, and only does
//! something in a session with `HOOK_SOCK` set:
//!
//! - `_cli_check_session_passport "$1"`: the tool of the first word alone
//!   (no sub-verb). An `id` tool checks for a shared Codex server and a
//!   dead passport.
//! - `_cli_apply_auth_for_verb "$1" "$2"`: the tool of the first two
//!   words. An `id` tool with no passport is refused.
//!
//! Both are pure word matches; `tests/tools_parity.rs` runs the shell's
//! functions over every word they name and compares.

/// `_cli_tool_id_for_verb VERB SUB`: the catalog tool id, or `""` for an
/// ungoverned verb.
pub fn tool_id(verb: &str, sub: &str) -> &'static str {
    match verb {
        "agent" => match sub {
            "team" | "teams" | "instances" | "instance" => "agent-teams",
            "identity" => "agent-identity",
            "harness" | "refresh" => "agent-harness",
            _ => "",
        },
        "setup" => match sub {
            "chief-of-staff" => "agent-harness",
            _ => "",
        },
        "workspace" => match sub {
            "set-agent-identity" => "agent-identity",
            _ => "",
        },
        "help" | "--help" | "-h" | "glossary" | "help-deprecated" | "help-internal"
        | "help-advanced" => "help",
        "--schema" | "schema" => "schema",
        "study" => "study",
        "whats-new" => "whats-new",
        "checkin" | "done" | "status" => "checkin",
        "activity" => "activity",
        "read" => "read",
        "skills" => "skills",
        "msg" | "talk" | "thread" => "msg",
        "inbox" => "inbox",
        "wiki" => "wiki",
        "respond" => "respond",
        "connections" | "reserve" | "release" => "connections",
        "sidecar" => "sidecar",
        "compute" | "ci" => "compute",
        "changes" => "changes",
        "sim" => "sim",
        "desktop" => match sub {
            "env" => "desktop",
            _ => "",
        },
        "doc" | "docs" => "docs",
        "hostchat" | "voice" => "hostchat",
        "chat" => "chat",
        "sources" | "source" => "sources",
        "llm" => "llm-accounts",
        "usage" => "usage",
        "vault" => "vault",
        "migrate" => "migrate",
        "job" | "jobs" => "jobs",
        "project" => "project",
        "organization" => match sub {
            "" | "help" | "--help" | "-h" => "",
            _ => "organization",
        },
        "heartbeat" | "remind" | "reminders" => "heartbeat",
        "mail" | "hostmail" | "calendar" | "cal" => "mail",
        "db" => "db",
        "store" | "board" => "store",
        "dns" | "domain" | "cert" | "firewall" | "front" => "dns",
        "publish" => "publish",
        "connect" => "connect-account",
        "sessions" | "terminal" => "sessions-spawn",
        "fs" => "fs",
        "daemon" => "daemon-admin",
        "users" => "users-admin",
        "api-key" => "api-key",
        "app" | "skin" | "skin-token" | "capabilities" => "skin",
        "remote-session" => "remote-session",
        "settings" => "settings",
        "browser" => "browser-open",
        _ => "",
    }
}

/// The tools `_cli_tool_default_mode` calls `id`. Every other tool, and
/// `""`, is `open`.
pub const ID_TOOLS: &[&str] = &[
    "msg",
    "inbox",
    "respond",
    "awareness",
    "connections",
    "heartbeat",
    "sidecar",
    "project",
    "organization",
    "mail",
    "db",
    "store",
    "dns",
    "publish",
    "connect-account",
    "sessions-spawn",
    "fs",
    "daemon-admin",
    "users-admin",
    "api-key",
    "skin",
    "remote-session",
    "browser-open",
    "settings",
    "llm-accounts",
    "compute",
    "vault",
    "sources",
    "agent-teams",
    "agent-identity",
    "agent-harness",
    "changes",
    "usage",
    "hostchat",
    "jobs",
    "sim",
    "desktop",
    "chat",
    "migrate",
    "docs",
];

/// `_cli_tool_default_mode TOOL` is `id`.
pub fn is_id(tool: &str) -> bool {
    ID_TOOLS.contains(&tool)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_few_rows() {
        assert_eq!(tool_id("mail", "send"), "mail");
        assert_eq!(tool_id("agent", ""), "");
        assert_eq!(tool_id("agent", "team"), "agent-teams");
        assert_eq!(tool_id("organization", "--help"), "");
        assert_eq!(tool_id("organization", "bulletin"), "organization");
        assert_eq!(tool_id("nope", "x"), "");
        assert!(is_id("mail") && !is_id("help") && !is_id("") && !is_id("tools-policy"));
    }
}
