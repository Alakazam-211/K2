//! The typed hook envelope behind `POST /hook/event`
//! (prd-daemon-activity-and-thread-working-v1 §7.1, DA5, DA11, DA12).
//!
//! `notify.sh` forwards the CLI's raw hook stdin plus a few `X-K2-*`
//! headers. [`parse`] turns that into a [`HookEnvelope`] and the raw body
//! is dropped right after: prompt text, tool input and output, and
//! `last_assistant_message` never leave this module. What survives is
//! ids, names, counts, and the redacted one-line tool description
//! ([`super::tool_line`]).
//!
//! The envelope is deliberately not `Serialize`: it is never written to
//! disk, a log, the bus, or a client.

use serde_json::Value;

/// Largest body `notify.sh` forwards and the daemon accepts (DA12).
pub const MAX_BODY_BYTES: usize = 1024 * 1024;

/// Which CLI's hook config called `notify.sh` (`X-K2-Hook-Source`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HookSource {
    Claude,
    Cursor,
    Gemini,
    Unknown,
}

impl HookSource {
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "claude" => Self::Claude,
            "cursor" => Self::Cursor,
            "gemini" => Self::Gemini,
            _ => Self::Unknown,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Cursor => "cursor",
            Self::Gemini => "gemini",
            Self::Unknown => "unknown",
        }
    }
}

/// The `X-K2-*` request headers, read by the transport (TCP dispatcher or
/// cell socket) before the body.
#[derive(Debug, Clone)]
pub struct HookHeaders {
    /// `X-K2-Pane`: the v2 session id the hook's PTY was spawned with.
    pub pane: String,
    /// `X-K2-Agent-Pid`: the hooked CLI's pid (`$PPID` in the hook command).
    pub agent_pid: Option<i32>,
    /// `X-K2-Hook-Source`.
    pub source: HookSource,
    /// `X-K2-Hook-Version` (script protocol, 2 today).
    pub hook_version: Option<u32>,
    /// `X-K2-Claude-Version` (`$CLAUDE_CODE_VERSION`), when set.
    pub cli_version: Option<String>,
    /// `X-K2-Hook-Truncated: 1`: stdin was over [`MAX_BODY_BYTES`]; the
    /// body is empty and only [`HookHeaders::event_hint`] names the event.
    pub truncated: bool,
    /// `X-K2-Hook-Event`: the script's own scan of the event name (or
    /// Cursor's `$1`). The body's `hook_event_name` wins when present.
    pub event_hint: Option<String>,
}

impl HookHeaders {
    /// Read the headers through `get` (a case-insensitive header lookup).
    pub fn from_lookup<'a>(get: impl Fn(&str) -> Option<&'a str>) -> Self {
        let nonempty = |name: &str| {
            get(name)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        Self {
            pane: nonempty("x-k2-pane").unwrap_or_default(),
            agent_pid: nonempty("x-k2-agent-pid")
                .and_then(|s| s.parse::<i32>().ok())
                .filter(|p| *p > 1),
            source: nonempty("x-k2-hook-source")
                .map(|s| HookSource::parse(&s))
                .unwrap_or(HookSource::Unknown),
            hook_version: nonempty("x-k2-hook-version").and_then(|s| s.parse().ok()),
            cli_version: nonempty("x-k2-claude-version").map(|s| s.chars().take(32).collect()),
            truncated: nonempty("x-k2-hook-truncated").is_some_and(|s| s == "1"),
            event_hint: nonempty("x-k2-hook-event").map(|s| s.chars().take(64).collect()),
        }
    }
}

/// One entry of a Claude `background_tasks[]` inventory. The
/// `description` and `command` fields are dropped at parse time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskEntry {
    pub id: String,
    /// `shell` | `subagent` | `monitor` | `workflow` | an unknown raw type.
    pub kind: String,
    pub status: String,
}

/// A parsed hook event (§7.1). Never serialized out.
#[derive(Debug, Clone)]
pub struct HookEnvelope {
    pub pane: String,
    pub agent_pid: Option<i32>,
    pub source: HookSource,
    /// `hook_event_name` (or the header hint for a truncated body).
    pub event: String,
    pub session_id: Option<String>,
    pub prompt_id: Option<String>,
    /// Set only inside a subagent (Claude).
    pub agent_id: Option<String>,
    pub agent_type: Option<String>,
    pub tool_name: Option<String>,
    pub tool_use_id: Option<String>,
    /// TW8: built and redacted at parse time; the raw input is dropped.
    pub tool_line: Option<String>,
    /// `SessionStart.source` / `UserPromptSubmit.source` / `PostCompact.trigger`
    /// / `SessionEnd.reason`.
    pub source_field: Option<String>,
    pub notification_type: Option<String>,
    pub is_interrupt: Option<bool>,
    /// `None` when the event carried no inventory (only `Stop` /
    /// `SubagentStop` do).
    pub background_tasks: Option<Vec<TaskEntry>>,
    pub session_crons: Option<usize>,
    /// The `[thread:<addr>]` address in a `UserPromptSubmit` prompt, if any.
    /// The prompt text itself is dropped.
    pub prompt_thread_addr: Option<String>,
    /// `<task-id>`s from a `<task-notification>` prompt.
    pub task_notification_ids: Vec<String>,
    /// A background task this lead `PostToolUse` launched, read from
    /// `tool_response` (`backgroundTaskId` = a `run_in_background` shell,
    /// `isAsync` + `agentId` = an async subagent, a Monitor `taskId`).
    /// `kind` is `shell` | `subagent` | `monitor`; `status` is `running`.
    pub launched_task: Option<TaskEntry>,
    /// The task a `TaskStop` / `KillShell` call stopped (`tool_input`
    /// `task_id` / `shell_id` / `bash_id`). Claude sends no notification
    /// for a stopped task, so the roster forgets it here.
    pub stopped_task_id: Option<String>,
    /// The hook's working directory (a path, not content).
    pub cwd: Option<String>,
    /// The CLI version the hook reported (`X-K2-Claude-Version`).
    pub cli_version: Option<String>,
    /// The body was over 1 MiB: a name-only envelope.
    pub truncated: bool,
}

/// Why a body didn't parse. Every case is answered 204 and counted
/// (`parse_error`); an agent is never blocked by it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvelopeError {
    MissingPane,
    NotJson,
    NotAnObject,
    NoEventName,
}

impl EnvelopeError {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::MissingPane => "missing_pane",
            Self::NotJson => "not_json",
            Self::NotAnObject => "not_an_object",
            Self::NoEventName => "no_event_name",
        }
    }
}

/// Parse one hook post. `body` is the raw stdin JSON (empty when
/// `headers.truncated`).
pub fn parse(headers: &HookHeaders, body: &[u8]) -> Result<HookEnvelope, EnvelopeError> {
    if headers.pane.is_empty() {
        return Err(EnvelopeError::MissingPane);
    }
    let mut env = HookEnvelope {
        pane: headers.pane.clone(),
        agent_pid: headers.agent_pid,
        source: headers.source,
        event: String::new(),
        session_id: None,
        prompt_id: None,
        agent_id: None,
        agent_type: None,
        tool_name: None,
        tool_use_id: None,
        tool_line: None,
        source_field: None,
        notification_type: None,
        is_interrupt: None,
        background_tasks: None,
        session_crons: None,
        prompt_thread_addr: None,
        task_notification_ids: Vec::new(),
        launched_task: None,
        stopped_task_id: None,
        cwd: None,
        cli_version: headers.cli_version.clone(),
        truncated: headers.truncated,
    };
    if headers.truncated {
        env.event = headers.event_hint.clone().ok_or(EnvelopeError::NoEventName)?;
        return Ok(env);
    }
    let value: Value = serde_json::from_slice(body).map_err(|_| EnvelopeError::NotJson)?;
    let obj = value.as_object().ok_or(EnvelopeError::NotAnObject)?;
    let s = |k: &str| {
        obj.get(k)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    env.event = ["hook_event_name", "type", "event", "eventType"]
        .iter()
        .find_map(|k| s(k))
        .or_else(|| headers.event_hint.clone())
        .ok_or(EnvelopeError::NoEventName)?;
    env.session_id = s("session_id");
    env.prompt_id = s("prompt_id");
    env.agent_id = s("agent_id");
    env.agent_type = s("agent_type");
    env.tool_name = s("tool_name");
    env.tool_use_id = s("tool_use_id");
    env.cwd = s("cwd");
    env.notification_type = s("notification_type");
    env.is_interrupt = obj.get("is_interrupt").and_then(Value::as_bool);
    env.source_field = s("source").or_else(|| s("trigger")).or_else(|| s("reason"));
    if let Some(name) = env.tool_name.as_deref() {
        let input = obj.get("tool_input").cloned().unwrap_or(Value::Null);
        env.tool_line = Some(super::tool_line::describe(name, &input));
        if env.event == "PostToolUse" {
            env.launched_task = obj.get("tool_response").and_then(|r| launched_task(name, r));
            env.stopped_task_id = stopped_task_id(name, &input);
        }
    }
    if let Some(tasks) = obj.get("background_tasks").and_then(Value::as_array) {
        env.background_tasks = Some(
            tasks
                .iter()
                .filter_map(|t| {
                    let field = |k: &str| t.get(k).and_then(Value::as_str).map(str::to_string);
                    Some(TaskEntry {
                        id: field("id")?,
                        kind: field("type").unwrap_or_else(|| "unknown".to_string()),
                        status: field("status").unwrap_or_default(),
                    })
                })
                .collect(),
        );
    }
    env.session_crons = obj
        .get("session_crons")
        .and_then(Value::as_array)
        .map(Vec::len);
    if let Some(prompt) = obj.get("prompt").and_then(Value::as_str) {
        env.prompt_thread_addr = thread_addr(prompt);
        env.task_notification_ids = task_notification_ids(prompt);
    }
    Ok(env)
}

/// The address inside the first `[thread:<addr>]` stamp
/// (`workspace_msg::format_message`), if the text carries one.
pub fn thread_addr(text: &str) -> Option<String> {
    let start = text.find("[thread:")? + "[thread:".len();
    let rest = &text[start..];
    let end = rest.find(']')?;
    let addr = rest[..end].trim();
    if addr.is_empty() || addr.len() > 256 || addr.contains(char::is_whitespace) {
        return None;
    }
    Some(addr.to_string())
}

/// `<task-id>` values from a `<task-notification>` prompt (Claude's
/// background-task completion message). Only a block that carries a
/// `<status>` before any `<summary>` is a completion; a Monitor's
/// ongoing `<event>` turn has none and is not counted (S2).
pub fn task_notification_ids(text: &str) -> Vec<String> {
    const OPEN: &str = "<task-notification>";
    if !text.contains(OPEN) {
        return Vec::new();
    }
    let mut ids = Vec::new();
    for block in text.split(OPEN).skip(1) {
        let block = block.split("</task-notification>").next().unwrap_or(block);
        let status_at = block.find("<status>");
        let summary_at = block.find("<summary>");
        let completed = match (status_at, summary_at) {
            (Some(st), Some(su)) => st < su,
            (Some(_), None) => true,
            (None, _) => false,
        };
        if !completed {
            continue;
        }
        let Some(start) = block.find("<task-id>") else { continue };
        let after = &block[start + "<task-id>".len()..];
        let Some(end) = after.find("</task-id>") else { continue };
        let id = after[..end].trim();
        if !id.is_empty() && id.len() <= 128 {
            ids.push(id.to_string());
        }
    }
    ids
}

fn short_id(v: Option<&Value>) -> Option<String> {
    let id = v?.as_str()?.trim();
    (!id.is_empty() && id.len() <= 128).then(|| id.to_string())
}

/// A background task a lead `PostToolUse` launched, from its
/// `tool_response` (see [`HookEnvelope::launched_task`]).
fn launched_task(tool_name: &str, response: &Value) -> Option<TaskEntry> {
    let entry = |id: String, kind: &str| TaskEntry { id, kind: kind.to_string(), status: "running".to_string() };
    if let Some(id) = short_id(response.get("backgroundTaskId")) {
        return Some(entry(id, "shell"));
    }
    if response.get("isAsync").and_then(Value::as_bool) == Some(true) {
        if let Some(id) = short_id(response.get("agentId")) {
            return Some(entry(id, "subagent"));
        }
    }
    if tool_name == "Monitor" {
        if let Some(id) = short_id(response.get("taskId")) {
            return Some(entry(id, "monitor"));
        }
    }
    None
}

/// The task a `TaskStop` / `KillShell` / `KillBash` call stopped.
fn stopped_task_id(tool_name: &str, input: &Value) -> Option<String> {
    if !matches!(tool_name, "TaskStop" | "KillShell" | "KillBash") {
        return None;
    }
    ["task_id", "shell_id", "bash_id"].iter().find_map(|k| short_id(input.get(*k)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn headers(pane: &str) -> HookHeaders {
        HookHeaders {
            pane: pane.to_string(),
            agent_pid: Some(4242),
            source: HookSource::Claude,
            hook_version: Some(2),
            cli_version: Some("2.1.292".to_string()),
            truncated: false,
            event_hint: None,
        }
    }

    fn parse_json(v: Value) -> HookEnvelope {
        parse(&headers("pane-1"), v.to_string().as_bytes()).expect("parses")
    }

    #[test]
    fn pre_tool_use_keeps_ids_and_a_redacted_line_only() {
        let env = parse_json(json!({
            "hook_event_name": "PreToolUse",
            "session_id": "conv-1",
            "prompt_id": "p-1",
            "cwd": "/home/u/ws",
            "tool_name": "Bash",
            "tool_use_id": "tu-1",
            "tool_input": { "command": "API_KEY=sk-secret cargo test" },
        }));
        assert_eq!(env.event, "PreToolUse");
        assert_eq!(env.session_id.as_deref(), Some("conv-1"));
        assert_eq!(env.prompt_id.as_deref(), Some("p-1"));
        assert_eq!(env.tool_use_id.as_deref(), Some("tu-1"));
        assert_eq!(env.tool_line.as_deref(), Some("Running `API_KEY=*** cargo test`"));
        let dbg = format!("{env:?}");
        assert!(!dbg.contains("sk-secret"), "raw tool input survived: {dbg}");
    }

    #[test]
    fn prompt_text_is_dropped_but_thread_stamp_and_task_ids_survive() {
        let env = parse_json(json!({
            "hook_event_name": "UserPromptSubmit",
            "prompt": "[from sam] [thread:sales/reviewer] please run the private plan",
        }));
        assert_eq!(env.prompt_thread_addr.as_deref(), Some("sales/reviewer"));
        assert!(!format!("{env:?}").contains("private plan"));

        let env = parse_json(json!({
            "hook_event_name": "UserPromptSubmit",
            "prompt": "<task-notification><task-id>bash_1</task-id><status>completed</status></task-notification>\n<task-notification><task-id>bash_2</task-id><status>killed</status><summary>s</summary></task-notification>\n<task-notification><task-id>mon_1</task-id><summary>tick</summary><event>line</event><status>x</status></task-notification>\n<task-notification><task-id>mon_2</task-id><event>line</event></task-notification>",
        }));
        // A Monitor's ongoing event (no status, or status after the
        // summary) is not a completion.
        assert_eq!(env.task_notification_ids, vec!["bash_1".to_string(), "bash_2".to_string()]);
        assert_eq!(env.prompt_thread_addr, None);
    }

    #[test]
    fn post_tool_use_reads_background_launches_and_stops() {
        let launched = |tool: &str, resp: Value| {
            parse_json(json!({
                "hook_event_name": "PostToolUse",
                "tool_name": tool,
                "tool_use_id": "tu",
                "tool_input": {},
                "tool_response": resp,
            }))
            .launched_task
        };
        let entry = |id: &str, kind: &str| Some(TaskEntry { id: id.into(), kind: kind.into(), status: "running".into() });
        assert_eq!(launched("Bash", json!({"backgroundTaskId": "bash_7", "stdout": "x"})), entry("bash_7", "shell"));
        assert_eq!(launched("Task", json!({"isAsync": true, "agentId": "a-9"})), entry("a-9", "subagent"));
        assert_eq!(launched("Task", json!({"isAsync": false, "agentId": "a-9"})), None);
        assert_eq!(launched("Monitor", json!({"taskId": "m-1"})), entry("m-1", "monitor"));
        assert_eq!(launched("Bash", json!({"stdout": "plain"})), None);

        // A PreToolUse never reports a launch, even with a response key.
        let pre = parse_json(json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_response": {"backgroundTaskId": "bash_7"},
        }));
        assert_eq!(pre.launched_task, None);

        let stop = parse_json(json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "KillShell",
            "tool_input": {"shell_id": "bash_7"},
        }));
        assert_eq!(stop.stopped_task_id.as_deref(), Some("bash_7"));
    }

    #[test]
    fn stop_inventory_keeps_id_type_status_only() {
        let env = parse_json(json!({
            "hook_event_name": "Stop",
            "last_assistant_message": "the secret answer",
            "background_tasks": [
                { "id": "t1", "type": "shell", "status": "running", "command": "npm run dev", "description": "dev server" },
                { "id": "t2", "type": "subagent", "status": "running", "agent_type": "Explore" },
                { "type": "shell" },
            ],
            "session_crons": [{ "id": "c1" }],
        }));
        let tasks = env.background_tasks.clone().expect("inventory");
        assert_eq!(
            tasks,
            vec![
                TaskEntry { id: "t1".into(), kind: "shell".into(), status: "running".into() },
                TaskEntry { id: "t2".into(), kind: "subagent".into(), status: "running".into() },
            ]
        );
        assert_eq!(env.session_crons, Some(1));
        let dbg = format!("{env:?}");
        assert!(!dbg.contains("secret answer") && !dbg.contains("npm run dev"), "{dbg}");
    }

    #[test]
    fn truncated_body_is_a_name_only_envelope() {
        let mut h = headers("pane-1");
        h.truncated = true;
        h.event_hint = Some("PostToolUse".to_string());
        let env = parse(&h, b"").expect("name-only envelope");
        assert_eq!(env.event, "PostToolUse");
        assert!(env.truncated);
        assert_eq!(env.tool_line, None);

        h.event_hint = None;
        assert_eq!(parse(&h, b"").unwrap_err(), EnvelopeError::NoEventName);
    }

    #[test]
    fn bad_bodies_are_typed_errors() {
        assert_eq!(parse(&headers(""), b"{}").unwrap_err(), EnvelopeError::MissingPane);
        assert_eq!(parse(&headers("p"), b"not json").unwrap_err(), EnvelopeError::NotJson);
        assert_eq!(parse(&headers("p"), b"[1]").unwrap_err(), EnvelopeError::NotAnObject);
        assert_eq!(parse(&headers("p"), b"{\"x\":1}").unwrap_err(), EnvelopeError::NoEventName);
    }

    #[test]
    fn cursor_event_name_falls_back_to_the_header_hint() {
        let mut h = headers("p");
        h.source = HookSource::Cursor;
        h.event_hint = Some("Start".to_string());
        let env = parse(&h, b"{\"conversation_id\":\"c\"}").expect("parses");
        assert_eq!(env.event, "Start");
    }

    #[test]
    fn headers_parse_case_insensitively_and_reject_junk_pids() {
        let pairs = [
            ("x-k2-pane", "sess-1"),
            ("x-k2-agent-pid", "1"),
            ("x-k2-hook-source", "Gemini"),
            ("x-k2-hook-version", "2"),
            ("x-k2-hook-truncated", "1"),
            ("x-k2-hook-event", "AfterTool"),
        ];
        let h = HookHeaders::from_lookup(|n| {
            pairs.iter().find(|(k, _)| k.eq_ignore_ascii_case(n)).map(|(_, v)| *v)
        });
        assert_eq!(h.pane, "sess-1");
        assert_eq!(h.agent_pid, None, "pid 1 (init) is never an agent");
        assert_eq!(h.source, HookSource::Gemini);
        assert_eq!(h.hook_version, Some(2));
        assert!(h.truncated);
        assert_eq!(h.event_hint.as_deref(), Some("AfterTool"));
    }
}
