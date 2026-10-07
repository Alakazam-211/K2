//! Transcript records → activity signals (DA27, A16, A19).
//!
//! Pure: one [`TranscriptReader`] per followed session turns each JSONL
//! line into zero or more [`TranscriptSignal`]s. Nothing here reads a
//! file; `transcript_follow` delivers the lines and the daemon's
//! `activity_transcript` feeds the signals to the row
//! ([`super::Evidence::Transcript`]).
//!
//! What a harness writes, and what it means:
//! - **Claude** (`~/.claude/projects/<slug>/<id>.jsonl`): a user prompt →
//!   turn start; an `attachment.type = queued_command` → mid-turn input
//!   (working evidence, Thread binding); an assistant `tool_use` → tool;
//!   a `tool_result` → tool done; an assistant record with
//!   `stop_reason: end_turn` and no `tool_use` still waiting for its
//!   result → turn end (a cross-check: the row waits 5 s for a hook
//!   first); a user record starting `[Request interrupted by user` →
//!   interrupt. `isSidechain: true` records are ignored (A19).
//! - **Codex** (rollout): `event_msg` `task_started` → turn start,
//!   `task_complete` → turn end, `turn_aborted` → interrupt;
//!   `response_item` `function_call` / `custom_tool_call` → tool, their
//!   outputs → tool done, `reasoning` → thinking.
//! - **Grok** (`chat_history.jsonl`): a `user` record (not
//!   `synthetic_reason`) → turn start; `assistant` with `tool_calls` →
//!   tool, without → turn end; `tool_result` → tool done; `reasoning` →
//!   thinking; `backend_tool_call` → tool.
//! - **Gemini**: a `user` record → turn start. Its hooks carry the rest.
//!
//! DA5: prompt text, tool input and thinking text never leave this
//! module. A signal carries at most the `[thread:<addr>]` address and the
//! redacted one-line tool description (TW8).

use serde_json::{json, Value};

use crate::agent_hooks::envelope::thread_addr;
use crate::agent_hooks::tool_line;

/// One piece of transcript evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptSignal {
    /// A user prompt began a turn. `thread_addr` is the `[thread:<addr>]`
    /// stamp it carried, if any.
    TurnStart { thread_addr: Option<String> },
    /// Input that arrived mid-turn (Claude `queued_command`; a stamped
    /// Codex user message). Working evidence and a Thread binding signal.
    Queued { thread_addr: Option<String> },
    /// The agent called a tool. `line` is the redacted TW8 description.
    Tool { line: String },
    /// A tool returned.
    ToolDone,
    /// A reasoning / thinking record.
    Thinking,
    /// The turn ended. `cross_check` marks Claude's `end_turn`, which the
    /// row trusts only after 5 s with no hook (DA27); `record_at` is the
    /// record's own timestamp (unix ms), to drop a record older than the
    /// last hook.
    TurnEnd { record_at: Option<i64>, cross_check: bool },
    /// The user interrupted the turn (Claude `[Request interrupted by
    /// user`, Codex `turn_aborted`).
    Interrupt,
}

impl TranscriptSignal {
    /// The Thread address this signal can bind (TW2 / A22), if any.
    pub fn thread_addr(&self) -> Option<&str> {
        match self {
            Self::TurnStart { thread_addr } | Self::Queued { thread_addr } => thread_addr.as_deref(),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Harness {
    Claude,
    Codex,
    Grok,
    Gemini,
}

/// Claude's interrupt record prefix (A16).
const INTERRUPT_PREFIX: &str = "[Request interrupted by user";

/// Tool calls remembered while they wait for a result (Claude).
const PENDING_TOOLS_CAP: usize = 64;

/// Per-session transcript state.
#[derive(Debug, Clone)]
pub struct TranscriptReader {
    harness: Harness,
    /// Claude `tool_use` ids with no `tool_result` yet. A turn end is not
    /// read while one dangles (DA27).
    pending_tools: Vec<String>,
}

impl TranscriptReader {
    /// A reader for `harness`, or `None` when it has no transcript decoder.
    pub fn new(harness: &str) -> Option<Self> {
        let harness = match harness.trim() {
            "claude" => Harness::Claude,
            "codex" => Harness::Codex,
            "grok" => Harness::Grok,
            "gemini" => Harness::Gemini,
            _ => return None,
        };
        Some(Self { harness, pending_tools: Vec::new() })
    }

    /// Does `harness` have a transcript the activity store can follow?
    pub fn supports(harness: &str) -> bool {
        Self::new(harness).is_some()
    }

    /// The follower re-positioned (a rewrite): forget in-flight state.
    pub fn reset(&mut self) {
        self.pending_tools.clear();
    }

    /// Signals from one JSONL line. Unknown or malformed lines give none.
    pub fn push_line(&mut self, line: &str) -> Vec<TranscriptSignal> {
        let line = line.trim();
        if line.is_empty() {
            return Vec::new();
        }
        let Ok(rec) = serde_json::from_str::<Value>(line) else {
            return Vec::new();
        };
        match self.harness {
            Harness::Claude => self.claude(&rec),
            Harness::Codex => codex(&rec),
            Harness::Grok => grok(&rec),
            Harness::Gemini => gemini(&rec),
        }
    }

    fn claude(&mut self, rec: &Value) -> Vec<TranscriptSignal> {
        if flag(rec, "isSidechain") {
            return Vec::new();
        }
        let kind = str_at(rec, "type");
        if kind == "attachment" {
            let att = rec.get("attachment").unwrap_or(&Value::Null);
            if str_at(att, "type") == "queued_command" {
                let addr = all_text(att.get("prompt")).as_deref().and_then(thread_addr);
                return vec![TranscriptSignal::Queued { thread_addr: addr }];
            }
            return Vec::new();
        }
        if flag(rec, "isMeta") || flag(rec, "isCompactSummary") {
            return Vec::new();
        }
        let message = rec.get("message").unwrap_or(&Value::Null);
        let content = message.get("content").or_else(|| rec.get("content"));
        match kind {
            "user" => self.claude_user(content),
            "assistant" => self.claude_assistant(rec, message, content),
            _ => Vec::new(),
        }
    }

    fn claude_user(&mut self, content: Option<&Value>) -> Vec<TranscriptSignal> {
        let mut results = false;
        let mut texts: Vec<&str> = Vec::new();
        match content {
            Some(Value::String(s)) => texts.push(s),
            Some(Value::Array(parts)) => {
                for part in parts {
                    match str_at(part, "type") {
                        "tool_result" => {
                            results = true;
                            let id = part.get("tool_use_id").and_then(Value::as_str).unwrap_or("");
                            self.pending_tools.retain(|t| t != id);
                        }
                        "text" => texts.extend(part.get("text").and_then(Value::as_str)),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        if texts.iter().any(|t| t.trim_start().starts_with(INTERRUPT_PREFIX)) {
            self.pending_tools.clear();
            return vec![TranscriptSignal::Interrupt];
        }
        if results {
            return vec![TranscriptSignal::ToolDone];
        }
        let text = texts.join("\n");
        let trimmed = text.trim_start();
        // A local slash command's echo is not a turn.
        if trimmed.is_empty() || trimmed.starts_with("<command-") || trimmed.starts_with("<local-command-") {
            return Vec::new();
        }
        // A new prompt: any call still dangling belonged to an earlier turn.
        self.pending_tools.clear();
        vec![TranscriptSignal::TurnStart { thread_addr: thread_addr(trimmed) }]
    }

    fn claude_assistant(&mut self, rec: &Value, message: &Value, content: Option<&Value>) -> Vec<TranscriptSignal> {
        let mut out = Vec::new();
        if let Some(Value::Array(parts)) = content {
            for part in parts {
                match str_at(part, "type") {
                    "tool_use" => {
                        let id = part.get("id").and_then(Value::as_str).unwrap_or("");
                        let name = part.get("name").and_then(Value::as_str).unwrap_or("");
                        if !id.is_empty() && !self.pending_tools.iter().any(|t| t == id) {
                            if self.pending_tools.len() >= PENDING_TOOLS_CAP {
                                self.pending_tools.remove(0);
                            }
                            self.pending_tools.push(id.to_string());
                        }
                        let input = part.get("input").unwrap_or(&Value::Null);
                        out.push(TranscriptSignal::Tool { line: tool_line::describe(name, input) });
                    }
                    "thinking" | "redacted_thinking" => out.push(TranscriptSignal::Thinking),
                    _ => {}
                }
            }
        }
        if str_at(message, "stop_reason") == "end_turn" && self.pending_tools.is_empty() {
            out.push(TranscriptSignal::TurnEnd { record_at: record_ms(rec), cross_check: true });
        }
        out
    }
}

fn codex(rec: &Value) -> Vec<TranscriptSignal> {
    let payload = rec.get("payload").unwrap_or(&Value::Null);
    let ptype = str_at(payload, "type");
    match str_at(rec, "type") {
        "event_msg" => match ptype {
            "task_started" => vec![TranscriptSignal::TurnStart { thread_addr: None }],
            "task_complete" => vec![TranscriptSignal::TurnEnd { record_at: record_ms(rec), cross_check: false }],
            "turn_aborted" => vec![TranscriptSignal::Interrupt],
            "user_message" => stamped_queue(payload.get("message").and_then(Value::as_str)),
            _ => Vec::new(),
        },
        "response_item" => match ptype {
            "function_call" | "custom_tool_call" => {
                let name = str_at(payload, "name");
                if name.is_empty() {
                    return Vec::new();
                }
                let args = payload.get("arguments").or_else(|| payload.get("input")).unwrap_or(&Value::Null);
                vec![TranscriptSignal::Tool { line: foreign_tool_line(name, args) }]
            }
            "function_call_output" | "custom_tool_call_output" => vec![TranscriptSignal::ToolDone],
            "reasoning" => vec![TranscriptSignal::Thinking],
            "message" if str_at(payload, "role") == "user" => {
                stamped_queue(all_text(payload.get("content")).as_deref())
            }
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// A Codex user message counts only when it carries a Thread stamp
/// (`task_started` is the turn start).
fn stamped_queue(text: Option<&str>) -> Vec<TranscriptSignal> {
    match text.and_then(thread_addr) {
        Some(addr) => vec![TranscriptSignal::Queued { thread_addr: Some(addr) }],
        None => Vec::new(),
    }
}

fn grok(rec: &Value) -> Vec<TranscriptSignal> {
    match str_at(rec, "type") {
        "user" if rec.get("synthetic_reason").is_none() => {
            let addr = all_text(rec.get("content")).as_deref().and_then(thread_addr);
            vec![TranscriptSignal::TurnStart { thread_addr: addr }]
        }
        "assistant" => {
            let calls = rec.get("tool_calls").and_then(Value::as_array).filter(|c| !c.is_empty());
            match calls {
                Some(calls) => calls
                    .iter()
                    .filter_map(|c| {
                        let name = c.get("name").and_then(Value::as_str)?;
                        let args = c.get("arguments").or_else(|| c.get("input")).unwrap_or(&Value::Null);
                        Some(TranscriptSignal::Tool { line: foreign_tool_line(name, args) })
                    })
                    .collect(),
                None => vec![TranscriptSignal::TurnEnd { record_at: None, cross_check: false }],
            }
        }
        "backend_tool_call" | "tool_call" => {
            let name = rec.get("name").and_then(Value::as_str).unwrap_or("tool");
            vec![TranscriptSignal::Tool { line: foreign_tool_line(name, &Value::Null) }]
        }
        "tool_result" => vec![TranscriptSignal::ToolDone],
        "reasoning" => vec![TranscriptSignal::Thinking],
        _ => Vec::new(),
    }
}

fn gemini(rec: &Value) -> Vec<TranscriptSignal> {
    if str_at(rec, "type") == "user" {
        let addr = all_text(rec.get("content")).as_deref().and_then(thread_addr);
        return vec![TranscriptSignal::TurnStart { thread_addr: addr }];
    }
    Vec::new()
}

/// TW8 for Codex and Grok tool calls: map the harness's tool to the
/// Claude tool it acts like, then describe (and redact) it the same way.
pub fn foreign_tool_line(name: &str, args: &Value) -> String {
    // Arguments usually arrive as a JSON string.
    let parsed;
    let args = match args {
        Value::String(s) => {
            parsed = serde_json::from_str::<Value>(s).unwrap_or_else(|_| json!({ "input": s }));
            &parsed
        }
        other => other,
    };
    let field = |keys: &[&str]| -> String {
        keys.iter()
            .find_map(|k| args.get(*k))
            .map(|v| match v {
                Value::String(s) => s.clone(),
                // Codex `shell`: ["bash", "-lc", "<cmd>"] → the command.
                Value::Array(parts) => parts.last().and_then(Value::as_str).unwrap_or("").to_string(),
                _ => String::new(),
            })
            .unwrap_or_default()
    };
    let as_claude = |tool: &str, key: &str, value: String| tool_line::describe(tool, &json!({ key: value }));
    match name {
        "exec_command" | "shell" | "local_shell" | "run_terminal_cmd" | "run_command" | "bash" | "run_shell_command" => {
            as_claude("Bash", "command", field(&["cmd", "command"]))
        }
        "read_file" | "view" | "view_file" | "read" => {
            as_claude("Read", "file_path", field(&["target_file", "file_path", "path"]))
        }
        "edit_file" | "write_file" | "search_replace" | "write" | "create_file" | "str_replace" => {
            as_claude("Edit", "file_path", field(&["target_file", "file_path", "path"]))
        }
        "apply_patch" => match patch_target(&field(&["input", "patch"])) {
            Some(path) => as_claude("Edit", "file_path", path),
            None => tool_line::describe("apply_patch", &Value::Null),
        },
        "grep" | "grep_search" | "codebase_search" | "file_search" | "search" | "glob" => {
            as_claude("Grep", "pattern", field(&["pattern", "query"]))
        }
        "web_search" => tool_line::describe("WebSearch", &Value::Null),
        other => tool_line::describe(other, &Value::Null),
    }
}

/// The first file an `apply_patch` body touches.
fn patch_target(patch: &str) -> Option<String> {
    patch.lines().find_map(|l| {
        ["*** Update File: ", "*** Add File: ", "*** Delete File: "]
            .iter()
            .find_map(|p| l.strip_prefix(p))
            .map(|p| p.trim().to_string())
    })
}

fn str_at<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

fn flag(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool) == Some(true)
}

/// Every text in a string or an array of text parts, joined.
fn all_text(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) => Some(s.clone()),
        Value::Array(parts) => {
            let texts: Vec<&str> = parts
                .iter()
                .filter_map(|p| p.as_str().or_else(|| p.get("text").and_then(Value::as_str)))
                .collect();
            (!texts.is_empty()).then(|| texts.join("\n"))
        }
        _ => None,
    }
}

/// The record's RFC 3339 `timestamp`, as unix ms.
fn record_ms(rec: &Value) -> Option<i64> {
    let ts = rec.get("timestamp").and_then(Value::as_str)?;
    chrono::DateTime::parse_from_rfc3339(ts).ok().map(|d| d.timestamp_millis())
}

#[cfg(test)]
mod tests {
    use super::*;
    use TranscriptSignal as S;

    fn read(harness: &str, lines: &[&str]) -> Vec<TranscriptSignal> {
        let mut r = TranscriptReader::new(harness).expect("decoder");
        lines.iter().flat_map(|l| r.push_line(l)).collect()
    }

    fn fixture(name: &str) -> Vec<&'static str> {
        super::tests_fixture(name)
    }

    /// T-S3b: Codex rollout fixtures → task_started / task_complete /
    /// turn_aborted, tool calls with a redacted line, reasoning.
    #[test]
    fn codex_rollouts_mark_turns() {
        assert_eq!(
            read("codex", &fixture("codex-plain")),
            vec![
                S::TurnStart { thread_addr: None },
                S::Thinking,
                S::TurnEnd { record_at: Some(1_790_000_004_000), cross_check: false },
            ]
        );
        let tool = read("codex", &fixture("codex-tool"));
        assert_eq!(tool.first(), Some(&S::TurnStart { thread_addr: None }));
        assert!(tool.contains(&S::Queued { thread_addr: Some("ws/thread-1".into()) }), "{tool:?}");
        assert!(tool.contains(&S::Tool { line: "Running `cargo test`".into() }), "{tool:?}");
        assert!(tool.contains(&S::Tool { line: "Editing `lib.rs`".into() }), "{tool:?}");
        assert_eq!(tool.iter().filter(|s| **s == S::ToolDone).count(), 2);
        assert!(matches!(tool.last(), Some(S::TurnEnd { cross_check: false, .. })));
        let aborted = read("codex", &fixture("codex-interrupt"));
        assert_eq!(aborted.last(), Some(&S::Interrupt));
        assert!(!aborted.iter().any(|s| matches!(s, S::TurnEnd { .. })));
    }

    /// T-S3c: Grok fixtures → a final `assistant` with no `tool_calls` is
    /// the turn end; one with calls is a tool.
    #[test]
    fn grok_final_assistant_ends_the_turn() {
        assert_eq!(
            read("grok", &fixture("grok-plain")),
            vec![S::TurnStart { thread_addr: None }, S::Thinking, S::TurnEnd { record_at: None, cross_check: false }]
        );
        let tool = read("grok", &fixture("grok-tool"));
        assert_eq!(tool.first(), Some(&S::TurnStart { thread_addr: Some("ws/thread-2".into()) }));
        assert!(tool.contains(&S::Tool { line: "Reading `main.rs`".into() }), "{tool:?}");
        assert!(tool.contains(&S::Tool { line: "Running `ls -la`".into() }), "{tool:?}");
        assert!(tool.contains(&S::ToolDone));
        assert_eq!(tool.iter().filter(|s| matches!(s, S::TurnEnd { .. })).count(), 1);
        assert!(matches!(tool.last(), Some(S::TurnEnd { .. })));
        // A synthetic user record is not a turn.
        assert!(read("grok", &[r#"{"type":"user","synthetic_reason":"x","content":"y"}"#]).is_empty());
    }

    /// Claude: prompt → working; a dangling `tool_use` holds the turn end
    /// back; `end_turn` after the results ends it; sidechains and local
    /// commands are not turns; `queued_command` is mid-turn input (A19).
    #[test]
    fn claude_transcript_drives_both_directions() {
        let got = read("claude", &fixture("claude-no-hooks"));
        assert_eq!(
            got,
            vec![
                S::TurnStart { thread_addr: None },
                S::Thinking,
                S::Tool { line: "Running `cargo build`".into() },
                S::Queued { thread_addr: Some("ws/thread-3".into()) },
                S::ToolDone,
                S::TurnEnd { record_at: Some(1_790_000_009_000), cross_check: true },
            ]
        );
    }

    #[test]
    fn claude_end_turn_with_a_dangling_tool_use_keeps_working() {
        let lines = [
            r#"{"type":"user","message":{"role":"user","content":"go"}}"#,
            r#"{"type":"assistant","message":{"id":"m","stop_reason":"tool_use","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"sleep 99"}}]}}"#,
            r#"{"type":"assistant","message":{"id":"m","stop_reason":"end_turn","content":[{"type":"text","text":"still going"}]}}"#,
        ];
        let got = read("claude", &lines);
        assert!(!got.iter().any(|s| matches!(s, S::TurnEnd { .. })), "{got:?}");
    }

    /// A16: the interrupt record (either wording) is an interrupt.
    #[test]
    fn claude_interrupt_record_is_an_interrupt() {
        let got = read("claude", &fixture("claude-interrupt"));
        assert_eq!(got.iter().filter(|s| **s == S::Interrupt).count(), 2, "{got:?}");
        assert!(!got.iter().any(|s| matches!(s, S::TurnEnd { .. })));
    }

    #[test]
    fn claude_filters_sidechain_meta_and_local_commands() {
        let got = read(
            "claude",
            &[
                r#"{"type":"user","isSidechain":true,"message":{"role":"user","content":"sub prompt"}}"#,
                r#"{"type":"assistant","isSidechain":true,"message":{"stop_reason":"end_turn","content":[{"type":"text","text":"x"}]}}"#,
                r#"{"type":"user","isMeta":true,"message":{"role":"user","content":"caveat"}}"#,
                r#"{"type":"user","message":{"role":"user","content":"<command-name>/clear</command-name>"}}"#,
                r#"{"type":"attachment","attachment":{"type":"file","path":"x"}}"#,
                "not json",
            ],
        );
        assert!(got.is_empty(), "{got:?}");
    }

    #[test]
    fn foreign_tool_lines_are_redacted_like_claude_ones() {
        assert_eq!(
            foreign_tool_line("exec_command", &json!(r#"{"cmd":"GITHUB_TOKEN=ghp_abc cargo test"}"#)),
            "Running `GITHUB_TOKEN=*** cargo test`"
        );
        assert_eq!(
            foreign_tool_line("shell", &json!({"command": ["bash", "-lc", "ls /home/u/x"]})),
            "Running `ls ~/x`"
        );
        assert_eq!(
            foreign_tool_line("apply_patch", &json!("*** Begin Patch\n*** Update File: src/a/b.rs\n-x\n")),
            "Editing `b.rs`"
        );
        assert_eq!(foreign_tool_line("mystery_tool", &Value::Null), "Using mystery_tool");
    }

    /// §11: the repo is public. Transcript fixtures are synthetic: no real
    /// home paths, no tokens, no addresses.
    #[test]
    fn transcript_fixtures_are_scrubbed() {
        let marks = [
            "/Users/", "k2skn_", "k2sk_", "k2rs_", "ghp_", "gho_", "github_pat_", "xoxb-", "xoxp-",
            "xoxa-", "AKIA", "Bearer ", "-----BEGIN", "@",
        ];
        for name in [
            "codex-plain", "codex-tool", "codex-interrupt", "grok-plain", "grok-tool", "claude-no-hooks",
            "claude-interrupt",
        ] {
            let text = fixture(name).join("\n");
            for mark in marks {
                assert!(!text.contains(mark), "{name}: fixture contains {mark:?}");
            }
            assert!(text.contains("Synthetic"), "{name}: synthetic fixtures say so");
        }
    }

    #[test]
    fn only_known_harnesses_have_a_reader() {
        for h in ["claude", "codex", "grok", "gemini"] {
            assert!(TranscriptReader::supports(h), "{h}");
        }
        for h in ["hermes", "cursor", "shell", "pi", ""] {
            assert!(!TranscriptReader::supports(h), "{h}");
        }
    }
}

/// The row under transcript and screen evidence (T-S3b–d, T-S3h, A16).
#[cfg(test)]
mod row_tests {
    use super::super::row::{Display, LeadState, Reason, Row, RowFacts, TRANSCRIPT_END_GRACE_MS};
    use super::super::{Evidence, TitleSignal};
    use super::*;
    use crate::agent_hooks::envelope::{self, HookHeaders, HookSource};

    fn row(harness: &str) -> Row {
        Row::new(
            RowFacts { session_id: "s".into(), agent_name: "tab-x".into(), harness: harness.into(), ..Default::default() },
            0,
        )
    }

    fn hook(r: &mut Row, body: &str, at: i64) {
        let env = envelope::parse(
            &HookHeaders {
                pane: "s".into(),
                agent_pid: Some(100),
                source: HookSource::Claude,
                hook_version: Some(2),
                cli_version: Some("2.1.292".into()),
                truncated: false,
                event_hint: None,
            },
            body.as_bytes(),
        )
        .expect("synthetic hook parses");
        r.apply(Evidence::Hook(&env), at);
    }

    fn sig(r: &mut Row, s: TranscriptSignal, at: i64) -> super::super::Change {
        r.apply(Evidence::Transcript(&s), at)
    }

    fn feed(r: &mut Row, reader: &mut TranscriptReader, lines: &[&str], start: i64) -> i64 {
        let mut t = start;
        for l in lines {
            for s in reader.push_line(l) {
                sig(r, s, t);
            }
            t += 100;
        }
        t
    }

    /// T-S3d: Claude with no hooks: the transcript drives both directions,
    /// and the row says `evidence: transcript`.
    #[test]
    fn claude_without_hooks_runs_on_the_transcript() {
        let mut r = row("claude");
        let mut reader = TranscriptReader::new("claude").expect("reader");
        let lines = super::tests_fixture("claude-no-hooks");
        // Up to (not including) the end_turn record: working.
        let t = feed(&mut r, &mut reader, &lines[..lines.len() - 1], 1_000);
        assert_eq!(r.lead.state, LeadState::Working);
        assert_eq!(r.display, Display::Working);
        assert_eq!(r.to_json()["evidenceSource"], "transcript");
        assert!(r.confirmed);
        // The end_turn record: idle at once (no hooks → no grace).
        feed(&mut r, &mut reader, &lines[lines.len() - 1..], t);
        assert_eq!(r.display, Display::Idle);
        assert_eq!(r.reason, Reason::TranscriptTurnEnd);
        assert_eq!(r.to_json()["evidenceSource"], "transcript");
        // A later title can't override transcript truth (DA28).
        r.apply(Evidence::Title(TitleSignal::Working), t + 1_000);
        assert_eq!(r.display, Display::Idle);
    }

    /// DA27: with hooks, `end_turn` waits 5 s for a hook; a hook in that
    /// window (or a later working record) cancels it; a record older than
    /// the last hook is stale.
    #[test]
    fn claude_end_turn_is_a_five_second_cross_check_with_hooks() {
        let mut r = row("claude");
        hook(&mut r, r#"{"hook_event_name":"UserPromptSubmit","prompt_id":"p1"}"#, 1_000);
        assert_eq!(r.display, Display::Working);
        let end = TranscriptSignal::TurnEnd { record_at: Some(1_500), cross_check: true };
        sig(&mut r, end.clone(), 2_000);
        assert!(r.transcript_hot());
        assert_eq!(r.next_deadline(), Some(2_000 + TRANSCRIPT_END_GRACE_MS));
        r.tick(2_000 + TRANSCRIPT_END_GRACE_MS - 1);
        assert_eq!(r.display, Display::Working, "still inside the grace");
        let ch = r.tick(2_000 + TRANSCRIPT_END_GRACE_MS);
        assert_eq!(r.display, Display::Idle);
        assert_eq!(r.reason, Reason::TranscriptTurnEnd);
        assert_eq!(ch.turn_ended.map(|t| t.reason), Some(Reason::TranscriptTurnEnd));
        // The latch: a late async tool event for p1 doesn't revive it.
        hook(&mut r, r#"{"hook_event_name":"PostToolUse","prompt_id":"p1","tool_name":"Bash","tool_use_id":"t9"}"#, 8_000);
        assert_eq!(r.display, Display::Idle);

        // A hook inside the grace cancels the end.
        let mut r = row("claude");
        hook(&mut r, r#"{"hook_event_name":"UserPromptSubmit","prompt_id":"p1"}"#, 1_000);
        sig(&mut r, end.clone(), 2_000);
        hook(&mut r, r#"{"hook_event_name":"PreToolUse","prompt_id":"p1","tool_name":"Bash","tool_use_id":"t1"}"#, 3_000);
        r.tick(10_000);
        assert_eq!(r.display, Display::Working);

        // A later working record cancels it too.
        let mut r = row("claude");
        hook(&mut r, r#"{"hook_event_name":"UserPromptSubmit","prompt_id":"p1"}"#, 1_000);
        sig(&mut r, end.clone(), 2_000);
        sig(&mut r, TranscriptSignal::Tool { line: "Running `x`".into() }, 2_500);
        r.tick(10_000);
        assert_eq!(r.display, Display::Working);

        // A stale record (stamped before the last hook) arms nothing.
        let mut r = row("claude");
        hook(&mut r, r#"{"hook_event_name":"UserPromptSubmit","prompt_id":"p2"}"#, 5_000);
        sig(&mut r, TranscriptSignal::TurnEnd { record_at: Some(4_000), cross_check: true }, 5_100);
        assert!(r.pending_transcript_end.is_none());
        r.tick(20_000);
        assert_eq!(r.display, Display::Working);
    }

    /// A16: the interrupt record cancels a hooked Claude turn (no keys
    /// needed); with hooks, transcript tool records are evidence only.
    #[test]
    fn claude_interrupt_record_cancels_with_hooks() {
        let mut r = row("claude");
        hook(&mut r, r#"{"hook_event_name":"UserPromptSubmit","prompt_id":"p1"}"#, 1_000);
        hook(&mut r, r#"{"hook_event_name":"Stop","prompt_id":"p1"}"#, 2_000);
        assert_eq!(r.display, Display::Idle);
        // A tool record after the hook's Stop does not drive a hooked row.
        sig(&mut r, TranscriptSignal::Tool { line: "Running `x`".into() }, 2_100);
        assert_eq!(r.display, Display::Idle);
        hook(&mut r, r#"{"hook_event_name":"UserPromptSubmit","prompt_id":"p2"}"#, 3_000);
        let ch = sig(&mut r, TranscriptSignal::Interrupt, 4_000);
        assert_eq!(r.display, Display::Idle);
        assert_eq!(r.reason, Reason::Interrupted);
        assert_eq!(ch.turn_ended.map(|t| t.reason), Some(Reason::Interrupted));
    }

    /// T-S3b (row): Codex's rollout alone runs the row.
    #[test]
    fn codex_rollout_drives_the_row() {
        let mut r = row("codex");
        let mut reader = TranscriptReader::new("codex").expect("reader");
        let mut t = feed(&mut r, &mut reader, &super::tests_fixture("codex-interrupt"), 1_000);
        assert_eq!(r.display, Display::Idle);
        assert_eq!(r.reason, Reason::Interrupted);
        let tool = super::tests_fixture("codex-tool");
        t = feed(&mut r, &mut reader, &tool[..4], t);
        assert_eq!(r.display, Display::Working, "task_started → working");
        feed(&mut r, &mut reader, &tool[4..], t);
        assert_eq!(r.display, Display::Idle);
        assert_eq!(r.reason, Reason::TranscriptTurnEnd);
        assert_eq!(r.to_json()["evidenceSource"], "transcript");
    }

    /// T-S3c (row): Grok's final assistant record ends the turn.
    #[test]
    fn grok_history_drives_the_row() {
        let mut r = row("grok");
        let mut reader = TranscriptReader::new("grok").expect("reader");
        let lines = super::tests_fixture("grok-tool");
        let t = feed(&mut r, &mut reader, &lines[..lines.len() - 1], 1_000);
        assert_eq!(r.display, Display::Working);
        feed(&mut r, &mut reader, &lines[lines.len() - 1..], t);
        assert_eq!(r.display, Display::Idle);
        assert_eq!(r.reason, Reason::TranscriptTurnEnd);
    }

    /// T-S3h (row): screen evidence is working; gone is idle; it never
    /// overrides hook or transcript truth.
    #[test]
    fn screen_marker_counts_only_without_better_evidence() {
        let mut r = row("hermes");
        r.apply(Evidence::Screen(true), 1_000);
        assert_eq!(r.display, Display::Working);
        assert_eq!(r.to_json()["evidenceSource"], "screen");
        r.apply(Evidence::Screen(false), 5_000);
        assert_eq!(r.display, Display::Idle);
        assert_eq!(r.reason, Reason::TurnDone);

        let mut r = row("cursor");
        sig(&mut r, TranscriptSignal::TurnEnd { record_at: None, cross_check: false }, 500);
        r.apply(Evidence::Screen(true), 1_000);
        assert_eq!(r.display, Display::Idle, "transcript evidence outranks the screen");
    }
}

/// Synthetic transcript fixtures (§11), one record per line.
#[cfg(test)]
fn tests_fixture(name: &str) -> Vec<&'static str> {
    let text = match name {
        "codex-plain" => include_str!("fixtures/codex-0.154/plain.jsonl"),
        "codex-tool" => include_str!("fixtures/codex-0.154/tool.jsonl"),
        "codex-interrupt" => include_str!("fixtures/codex-0.154/interrupt.jsonl"),
        "grok-plain" => include_str!("fixtures/grok-1.0.46/plain.jsonl"),
        "grok-tool" => include_str!("fixtures/grok-1.0.46/tool.jsonl"),
        "claude-no-hooks" => include_str!("fixtures/claude-2.1.292/transcript-no-hooks.jsonl"),
        "claude-interrupt" => include_str!("fixtures/claude-2.1.292/transcript-interrupt.jsonl"),
        other => panic!("no fixture {other}"),
    };
    text.lines().filter(|l| !l.trim().is_empty()).collect()
}
