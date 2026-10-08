//! The daemon-owned hook installer
//! (prd-daemon-activity-and-thread-working-v1 DA7–DA11, DA18; A2, A3, A9, A10).
//!
//! Moved out of `src-tauri/src/agent_hooks.rs` so a headless daemon has
//! hooks too. The daemon runs [`run`] after boot is ready and every
//! 10 minutes (self-heal); `POST /cli/hooks/install` runs it on demand.
//!
//! What it writes, all under an explicit `home` (never `dirs::home_dir`):
//! - `~/.k2/hooks/notify.sh` — [`generate_hook_script`], stamped
//!   `# k2-hook-version: <daemon version>`. An installer older than the
//!   on-disk stamp writes nothing at all (A10).
//! - K2 entries in `~/.claude/settings.json`, `~/.cursor/hooks.json` and
//!   `~/.gemini/settings.json`.
//!
//! Config writes are safe (DA8 / A2): a file that doesn't parse, or whose
//! root or `hooks` isn't an object, is left byte-identical and reported
//! `config_unreadable`. A K2 entry is any hook command containing
//! `/.k2/hooks/notify.sh` (or the pre-0.40.37 `/.k2so/` path); K2 entries
//! are replaced in place and every other tool's entries are kept. A write
//! is temp-file + rename, and only when the parsed content changed.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Map, Value};

use super::envelope::HookSource;

/// Marks a K2 hook command.
pub const NOTIFY_FRAGMENT: &str = "/.k2/hooks/notify.sh";
/// The pre-0.40.37 path, still a K2 entry (replaced in place).
pub const LEGACY_NOTIFY_FRAGMENT: &str = "/.k2so/hooks/notify.sh";
/// The script's version stamp line prefix.
pub const VERSION_STAMP: &str = "# k2-hook-version: ";
/// `X-K2-Hook-Version` the script sends.
pub const HOOK_PROTOCOL: u32 = 2;

/// Q1: at or above this Claude Code version the full §4 set is
/// registered; below it (or unknown), the base set.
pub const CLAUDE_FULL_SET_FLOOR: (u32, u32, u32) = (2, 1, 292);

/// §4: every Claude event K2 registers on the floor version and up.
pub const CLAUDE_FULL_SET: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "PermissionDenied",
    "Notification",
    "Stop",
    "StopFailure",
    "SubagentStart",
    "SubagentStop",
    "PostCompact",
    "SessionEnd",
];

/// DA9: the set for an unknown or older Claude.
pub const CLAUDE_BASE_SET: &[&str] = &[
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "Stop",
    "SubagentStop",
    "SessionStart",
    "SessionEnd",
    "Notification",
];

/// DA10: tool events run `async: true` (full set only), so a slow daemon
/// never slows a tool call. Lifecycle events stay synchronous.
pub const CLAUDE_ASYNC_EVENTS: &[&str] =
    &["PreToolUse", "PostToolUse", "PostToolUseFailure", "SubagentStart"];

/// Cursor event → the word passed as `$1` (today's mapping).
pub const CURSOR_EVENTS: &[(&str, &str)] = &[
    ("beforeSubmitPrompt", "Start"),
    ("stop", "Stop"),
    ("beforeShellExecution", "PermissionRequest"),
    ("beforeMCPExecution", "PermissionRequest"),
];

/// Gemini CLI events.
pub const GEMINI_EVENTS: &[&str] = &["BeforeAgent", "AfterAgent", "AfterTool"];

/// Timeout on `claude --version` (DA9).
pub const VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// `~/.k2/hooks/notify.sh` under `home`.
pub fn script_path(home: &Path) -> PathBuf {
    home.join(".k2").join("hooks").join("notify.sh")
}

fn claude_settings_path(home: &Path) -> PathBuf {
    home.join(".claude").join("settings.json")
}
fn cursor_hooks_path(home: &Path) -> PathBuf {
    home.join(".cursor").join("hooks.json")
}
/// DA18: the file the installer writes, which is also the file status reads.
fn gemini_settings_path(home: &Path) -> PathBuf {
    home.join(".gemini").join("settings.json")
}

// ── The script ───────────────────────────────────────────────────────────

/// The `notify.sh` body (DA11, A3). Stamped with `daemon_version`.
///
/// It reads ALL of stdin, forwards at most 1 MiB as the `POST /hook/event`
/// body (a larger payload goes as headers only, `X-K2-Hook-Truncated: 1`
/// plus the event name), tries the per-cell socket with the scoped Bearer
/// and then loopback TCP with the disk owner token, prints nothing, and
/// always exits 0. The payload is held in memory only, never in a file.
pub fn generate_hook_script(daemon_version: &str) -> String {
    format!(
        r#"#!/bin/bash
# K2 agent lifecycle hook. DO NOT EDIT: managed by the K2 daemon.
{VERSION_STAMP}{daemon_version}
#
# Agent CLIs (Claude Code, Cursor, Gemini) run this on lifecycle events.
# It forwards the CLI's raw hook JSON to the daemon (POST /hook/event).
# It prints nothing and always exits 0, so it can never block the agent.
# The hook command sets K2_HOOK_AGENT_PID=$PPID (the CLI's own pid) and
# K2_HOOK_SOURCE; the daemon checks the pid really is this pane's agent.

export LC_ALL=C
MAX_BODY={max_body}
PANE="${{K2_PANE_ID:-${{K2SO_PANE_ID:-}}}}"
SOURCE="${{K2_HOOK_SOURCE:-claude}}"
AGENT_PID="${{K2_HOOK_AGENT_PID:-}}"
SOCK="${{K2_HOOK_SOCK:-${{K2SO_HOOK_SOCK:-}}}}"
SCOPED="${{K2_HOOK_TOKEN:-${{K2SO_HOOK_TOKEN:-}}}}"

# The payload: JSON as $1 (notify-style CLIs) or on stdin. Always drain
# stdin so the CLI never sees a broken pipe.
HINT=""
BODY=""
if [ -n "${{1:-}}" ] && [ "${{1:0:1}}" = "{{" ]; then
    BODY="$1"
else
    HINT="${{1:-}}"
    if [ ! -t 0 ]; then
        BODY="$(cat 2>/dev/null; printf x)"
        BODY="${{BODY%x}}"
    fi
fi

[ -z "$PANE" ] && exit 0

TRUNCATED=""
if [ "${{#BODY}}" -gt "$MAX_BODY" ]; then
    TRUNCATED=1
    for key in hook_event_name type event eventType; do
        if [[ "$BODY" =~ \"$key\"[[:space:]]*:[[:space:]]*\"([^\"]*)\" ]]; then
            HINT="${{BASH_REMATCH[1]}}"
            break
        fi
    done
    BODY=""
fi

HDR=(-H "Content-Type: application/json"
     -H "X-K2-Pane: $PANE"
     -H "X-K2-Hook-Source: $SOURCE"
     -H "X-K2-Hook-Version: {protocol}")
[ -n "$AGENT_PID" ] && HDR+=(-H "X-K2-Agent-Pid: $AGENT_PID")
[ -n "${{CLAUDE_CODE_VERSION:-}}" ] && HDR+=(-H "X-K2-Claude-Version: $CLAUDE_CODE_VERSION")
[ -n "$HINT" ] && HDR+=(-H "X-K2-Hook-Event: $HINT")
[ -n "$TRUNCATED" ] && HDR+=(-H "X-K2-Hook-Truncated: 1")

# Tokens never ride curl's argv (any OS user can read argv with `ps`):
# the Authorization header goes in a curl config on fd 3 (a here-string;
# stdin carries the payload). Tokens hold no `"` or `\`, so no escaping.

# Per-cell socket first, with the scoped token as a Bearer header.
if [ -n "$SOCK" ] && [ -S "$SOCK" ]; then
    if printf '%s' "$BODY" | curl -sf -K /dev/fd/3 -X POST --unix-socket "$SOCK" \
        --connect-timeout 1 --max-time 2 "${{HDR[@]}}" \
        --data-binary @- "http://localhost/hook/event" >/dev/null 2>&1 \
        3<<<"header = \"Authorization: Bearer $SCOPED\""; then
        exit 0
    fi
fi

# Loopback TCP with the owner token from disk (read at exec time, so a
# daemon restart with a new port or token never strands a long session).
PORT="$(cat "$HOME/.k2/heartbeat.port" 2>/dev/null)"
[ -z "$PORT" ] && exit 0
TOKEN="$(cat "$HOME/.k2/heartbeat.token" 2>/dev/null)"
printf '%s' "$BODY" | curl -s -K /dev/fd/3 -X POST --connect-timeout 1 --max-time 2 \
    "${{HDR[@]}}" \
    --data-binary @- "http://127.0.0.1:$PORT/hook/event" >/dev/null 2>&1 \
    3<<<"header = \"Authorization: Bearer $TOKEN\""
exit 0
"#,
        max_body = super::envelope::MAX_BODY_BYTES,
        protocol = HOOK_PROTOCOL,
    )
}

/// The version stamped in a script body, if any.
pub fn script_stamp(body: &str) -> Option<String> {
    body.lines()
        .find_map(|l| l.strip_prefix(VERSION_STAMP))
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// The registered command for one CLI (A3): the `[ -x ]` guard is kept,
/// `$PPID` (expanded by the `sh -c` the CLI starts) is the CLI's own pid.
pub fn hook_command(script: &Path, source: HookSource, cursor_arg: Option<&str>) -> String {
    let p = shell_escape(&script.to_string_lossy());
    let args = match cursor_arg {
        Some(a) => a.to_string(),
        None => "\"$@\"".to_string(),
    };
    format!(
        "[ -x {p} ] && K2_HOOK_AGENT_PID=$PPID K2_HOOK_SOURCE={} {p} {args} || true",
        source.as_str()
    )
}

fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

// ── Versions ─────────────────────────────────────────────────────────────

/// The first `x.y.z` in `s` (`"2.1.292 (Claude Code)"` → `(2,1,292)`).
pub fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    for word in s.split(|c: char| !(c.is_ascii_digit() || c == '.')) {
        let mut parts = word.split('.');
        let (Some(a), Some(b), Some(c)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        if let (Ok(a), Ok(b), Ok(c)) = (a.parse(), b.parse(), c.parse()) {
            return Some((a, b, c));
        }
    }
    None
}

/// The Claude events to register for `claude_version`, and whether that is
/// the full set (Q1).
pub fn claude_events_for(claude_version: Option<&str>) -> (&'static [&'static str], bool) {
    match claude_version.and_then(parse_version) {
        Some(v) if v >= CLAUDE_FULL_SET_FLOOR => (CLAUDE_FULL_SET, true),
        _ => (CLAUDE_BASE_SET, false),
    }
}

/// `claude --version`, resolved the way spawns resolve programs (A9): the
/// augmented login PATH, then [`crate::terminal::agent_spawn_guard`]. Under
/// a test shim dir only the shim runs; under a temp HOME a real `claude`
/// is never started. `None` when Claude isn't installed, refused, slow, or
/// prints no version.
pub fn probe_claude_version() -> Option<String> {
    let inherited = crate::terminal::login_path::process_path();
    let search = crate::terminal::login_path::augmented_path(&inherited);
    let guard = crate::terminal::agent_spawn_guard::GuardEnv::from_process();
    probe_version_with("claude", &search, &guard, VERSION_PROBE_TIMEOUT)
}

/// [`probe_claude_version`] with every input explicit (tests).
pub fn probe_version_with(
    program: &str,
    search_path: &str,
    guard: &crate::terminal::agent_spawn_guard::GuardEnv,
    timeout: Duration,
) -> Option<String> {
    let resolved = match crate::terminal::agent_spawn_guard::resolve_program(program, search_path, guard) {
        Ok(p) => p,
        Err(msg) => {
            crate::log_debug!("[agent-hooks] version probe skipped: {msg}");
            return None;
        }
    };
    let mut child = Command::new(&resolved)
        .arg("--version")
        .env("PATH", search_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut out = String::new();
    child.stdout.take()?.take(4096).read_to_string(&mut out).ok()?;
    parse_version(&out).map(|(a, b, c)| format!("{a}.{b}.{c}"))
}

// ── Install gate (A10) ───────────────────────────────────────────────────

/// Whether this daemon may install hooks into `home`. `Err` carries the
/// reason it won't. `K2_HOOK_INSTALL=0` skips; so does a debug build whose
/// `HOME` is the user's real passwd home (a worktree or dev daemon would
/// otherwise rewrite the real `~/.claude/settings.json`). A debug build
/// under a temp HOME installs into that temp HOME.
pub fn install_gate(
    env_install: Option<&str>,
    is_debug_build: bool,
    home: &Path,
    passwd_home: Option<&Path>,
) -> Result<(), &'static str> {
    if let Some(v) = env_install {
        if matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "false" | "no" | "off") {
            return Err("K2_HOOK_INSTALL=0");
        }
    }
    if is_debug_build {
        let same = match passwd_home {
            Some(real) => same_path(real, home),
            // No passwd entry to compare with (Windows): a debug build
            // never installs on its own.
            None => true,
        };
        if same {
            return Err("debug build on the real home");
        }
    }
    Ok(())
}

/// [`install_gate`] for this process.
pub fn install_gate_for_process(home: &Path) -> Result<(), &'static str> {
    install_gate(
        std::env::var("K2_HOOK_INSTALL").ok().as_deref(),
        cfg!(debug_assertions),
        home,
        passwd_home().as_deref(),
    )
}

fn same_path(a: &Path, b: &Path) -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    a == b || canon(a) == canon(b)
}

/// The current user's home from the passwd database (never `$HOME`).
#[cfg(unix)]
pub fn passwd_home() -> Option<PathBuf> {
    use std::ffi::CStr;
    // SAFETY: getpwuid_r writes into the caller's buffers; we check the
    // result pointer before reading it.
    unsafe {
        let mut pwd: libc::passwd = std::mem::zeroed();
        let mut buf = vec![0 as libc::c_char; 4096];
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        let rc = libc::getpwuid_r(libc::getuid(), &mut pwd, buf.as_mut_ptr(), buf.len(), &mut result);
        if rc != 0 || result.is_null() || pwd.pw_dir.is_null() {
            return None;
        }
        let dir = CStr::from_ptr(pwd.pw_dir).to_string_lossy().into_owned();
        (!dir.is_empty()).then(|| PathBuf::from(dir))
    }
}

#[cfg(not(unix))]
pub fn passwd_home() -> Option<PathBuf> {
    None
}

// ── Reports ──────────────────────────────────────────────────────────────

/// What happened to `notify.sh`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScriptState {
    Written,
    Unchanged,
    /// The on-disk stamp is newer than this daemon: nothing was written (A10).
    SkippedNewer,
    Error,
}

/// What happened to one CLI config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigState {
    Written,
    Unchanged,
    /// Didn't parse / root or `hooks` not an object: left untouched (A2).
    ConfigUnreadable,
    /// The CLI isn't set up on this machine (no config dir): skipped.
    Absent,
    /// Skipped because a newer daemon owns the hooks (A10).
    SkippedNewer,
    Error,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptReport {
    pub path: String,
    pub state: ScriptState,
    /// The stamp found on disk before this run.
    pub on_disk_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CliReport {
    pub cli: &'static str,
    pub path: String,
    pub state: ConfigState,
    /// Events with a K2 entry after this run.
    pub events: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallReport {
    /// `install` | `remove`.
    pub action: &'static str,
    pub daemon_version: String,
    /// The detected Claude version (`None` = unknown → base set).
    pub claude_version: Option<String>,
    /// `full` | `base`.
    pub claude_event_set: &'static str,
    pub script: ScriptReport,
    pub clis: Vec<CliReport>,
}

impl InstallReport {
    /// Per-CLI problems a person should see (`hooks_install_failed`).
    pub fn failures(&self) -> Vec<Value> {
        let mut out = Vec::new();
        if self.script.state == ScriptState::Error {
            out.push(json!({ "cli": "notify-script", "error": self.script.error.clone().unwrap_or_default() }));
        }
        for c in &self.clis {
            match c.state {
                ConfigState::ConfigUnreadable => out.push(json!({
                    "cli": c.cli, "error": "config_unreadable", "path": c.path,
                })),
                ConfigState::Error => out.push(json!({
                    "cli": c.cli, "error": c.error.clone().unwrap_or_default(), "path": c.path,
                })),
                _ => {}
            }
        }
        out
    }
}

/// One installer run.
#[derive(Debug, Clone)]
pub struct InstallRequest<'a> {
    pub home: &'a Path,
    /// This daemon's version (the script stamp).
    pub daemon_version: &'a str,
    /// From [`probe_claude_version`] (the caller runs the probe).
    pub claude_version: Option<String>,
    /// Remove K2 entries instead of installing (`k2 hooks uninstall`, or
    /// the `agent_hooks` setting off). The script is left in place.
    pub remove: bool,
}

/// Install (or remove) K2's hooks under `req.home`.
pub fn run(req: &InstallRequest<'_>) -> InstallReport {
    let (claude_events, full) = claude_events_for(req.claude_version.as_deref());
    let script = script_path(req.home);
    let mut report = InstallReport {
        action: if req.remove { "remove" } else { "install" },
        daemon_version: req.daemon_version.to_string(),
        claude_version: req.claude_version.clone(),
        claude_event_set: if full { "full" } else { "base" },
        script: ScriptReport {
            path: script.to_string_lossy().into_owned(),
            state: ScriptState::Unchanged,
            on_disk_version: None,
            error: None,
        },
        clis: Vec::new(),
    };

    let mut skip_newer = false;
    if !req.remove {
        match write_script(&script, req.daemon_version) {
            Ok((state, on_disk)) => {
                skip_newer = state == ScriptState::SkippedNewer;
                report.script.state = state;
                report.script.on_disk_version = on_disk;
            }
            Err(e) => {
                report.script.state = ScriptState::Error;
                report.script.error = Some(e);
            }
        }
    } else {
        report.script.on_disk_version = std::fs::read_to_string(&script)
            .ok()
            .and_then(|b| script_stamp(&b));
    }

    let claude_present = req.claude_version.is_some() || req.home.join(".claude").is_dir();
    let plans: [(&'static str, PathBuf, bool, Desired); 3] = [
        (
            "claude",
            claude_settings_path(req.home),
            claude_present,
            Desired::Nested(
                claude_events
                    .iter()
                    .map(|e| {
                        let async_ = full && CLAUDE_ASYNC_EVENTS.contains(e);
                        (e.to_string(), claude_group(&hook_command(&script, HookSource::Claude, None), async_))
                    })
                    .collect(),
            ),
        ),
        (
            "cursor",
            cursor_hooks_path(req.home),
            req.home.join(".cursor").is_dir(),
            Desired::Flat(
                CURSOR_EVENTS
                    .iter()
                    .map(|(e, arg)| {
                        (
                            e.to_string(),
                            json!({ "command": hook_command(&script, HookSource::Cursor, Some(arg)) }),
                        )
                    })
                    .collect(),
            ),
        ),
        (
            "gemini",
            gemini_settings_path(req.home),
            req.home.join(".gemini").is_dir(),
            Desired::Nested(
                GEMINI_EVENTS
                    .iter()
                    .map(|e| {
                        (e.to_string(), claude_group(&hook_command(&script, HookSource::Gemini, None), false))
                    })
                    .collect(),
            ),
        ),
    ];

    for (cli, path, present, desired) in plans {
        let path_str = path.to_string_lossy().into_owned();
        let state_only = |state: ConfigState| CliReport {
            cli,
            path: path_str.clone(),
            state,
            events: Vec::new(),
            error: None,
        };
        if skip_newer {
            report.clis.push(state_only(ConfigState::SkippedNewer));
            continue;
        }
        if !req.remove && !present && !path.exists() {
            report.clis.push(state_only(ConfigState::Absent));
            continue;
        }
        if req.remove && !path.exists() {
            report.clis.push(state_only(ConfigState::Absent));
            continue;
        }
        let desired = if req.remove { desired.emptied() } else { desired };
        match edit_config(&path, |root| apply(root, &desired)) {
            Ok((state, root)) => report.clis.push(CliReport {
                cli,
                path: path_str,
                state,
                events: k2_events(&root, desired.is_flat()),
                error: None,
            }),
            Err(EditError::Unreadable) => report.clis.push(state_only(ConfigState::ConfigUnreadable)),
            Err(EditError::Io(e)) => {
                let mut r = state_only(ConfigState::Error);
                r.error = Some(e);
                report.clis.push(r);
            }
        }
    }
    report
}

/// Write `notify.sh` when this daemon's version is ≥ the on-disk stamp and
/// the bytes differ. Returns the state and the stamp found on disk.
fn write_script(path: &Path, version: &str) -> Result<(ScriptState, Option<String>), String> {
    let existing = std::fs::read_to_string(path).ok();
    let on_disk = existing.as_deref().and_then(script_stamp);
    if let Some(theirs) = on_disk.as_deref() {
        if let (Some(t), Some(m)) = (parse_version(theirs), parse_version(version)) {
            if t > m {
                return Ok((ScriptState::SkippedNewer, on_disk));
            }
        }
    }
    let body = generate_hook_script(version);
    let state = if existing.as_deref() == Some(body.as_str()) {
        ScriptState::Unchanged
    } else {
        crate::fs_atomic::atomic_write_str(path, &body).map_err(|e| e.to_string())?;
        ScriptState::Written
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path).map_err(|e| e.to_string())?.permissions().mode();
        if mode & 0o777 != 0o755 {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
                .map_err(|e| e.to_string())?;
        }
    }
    Ok((state, on_disk))
}

// ── Config editing ───────────────────────────────────────────────────────

/// The K2 entries one config should hold.
enum Desired {
    /// Claude / Gemini: `hooks.<Event>` = `[ {matcher?, hooks:[{type,command}]} ]`.
    Nested(Vec<(String, Value)>),
    /// Cursor: `hooks.<event>` = `[ {command} ]`.
    Flat(Vec<(String, Value)>),
}

impl Desired {
    fn emptied(self) -> Self {
        match self {
            Self::Nested(_) => Self::Nested(Vec::new()),
            Self::Flat(_) => Self::Flat(Vec::new()),
        }
    }
    fn is_flat(&self) -> bool {
        matches!(self, Self::Flat(_))
    }
}

fn claude_group(command: &str, async_: bool) -> Value {
    let mut hook = json!({ "type": "command", "command": command });
    if async_ {
        hook["async"] = Value::Bool(true);
    }
    json!({ "hooks": [hook] })
}

fn is_k2_command(cmd: &str) -> bool {
    cmd.contains(NOTIFY_FRAGMENT) || cmd.contains(LEGACY_NOTIFY_FRAGMENT)
}

fn hook_is_k2(hook: &Value) -> bool {
    hook.get("command").and_then(Value::as_str).is_some_and(is_k2_command)
}

enum EditError {
    Unreadable,
    Io(String),
}

/// Read → mutate → write-if-changed. A missing file starts as `{}`.
/// Returns the state and the resulting root.
fn edit_config(
    path: &Path,
    mutate: impl FnOnce(&mut Map<String, Value>) -> Result<(), EditError>,
) -> Result<(ConfigState, Value), EditError> {
    let original: Option<Value> = match std::fs::read(path) {
        Ok(bytes) => {
            let v: Value = serde_json::from_slice(&bytes).map_err(|_| EditError::Unreadable)?;
            Some(v)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(EditError::Io(e.to_string())),
    };
    let mut root = match &original {
        Some(Value::Object(m)) => m.clone(),
        Some(_) => return Err(EditError::Unreadable),
        None => Map::new(),
    };
    if root.get("hooks").is_some_and(|h| !h.is_object()) {
        return Err(EditError::Unreadable);
    }
    mutate(&mut root)?;
    let after = Value::Object(root);
    let unchanged = match &original {
        Some(orig) => *orig == after,
        // A missing file that would only hold `{}` (an uninstall) is not created.
        None => after.as_object().is_some_and(|m| m.is_empty()),
    };
    if unchanged {
        return Ok((ConfigState::Unchanged, after));
    }
    let mut text = serde_json::to_string_pretty(&after).map_err(|e| EditError::Io(e.to_string()))?;
    text.push('\n');
    // Write through a symlinked config (dotfiles) instead of replacing the link.
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    crate::fs_atomic::atomic_write_str(&target, &text).map_err(|e| EditError::Io(e.to_string()))?;
    Ok((ConfigState::Written, after))
}

/// Make `root.hooks` hold exactly `desired`'s K2 entries, in place.
fn apply(root: &mut Map<String, Value>, desired: &Desired) -> Result<(), EditError> {
    let (wanted, flat) = match desired {
        Desired::Nested(w) => (w, false),
        Desired::Flat(w) => (w, true),
    };
    // Cursor ≤0.40.145 K2 wrote events at the file root; clear those.
    if flat {
        let legacy: Vec<String> = root
            .iter()
            .filter(|(k, v)| *k != "hooks" && *k != "version" && v.is_array())
            .map(|(k, _)| k.clone())
            .collect();
        for k in legacy {
            if let Some(Value::Array(arr)) = root.get_mut(&k) {
                if replace_k2(arr, None, true) && arr.is_empty() {
                    root.remove(&k);
                }
            }
        }
    }
    let has_hooks = root.contains_key("hooks");
    if wanted.is_empty() && !has_hooks {
        return Ok(());
    }
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or(EditError::Unreadable)?;
    // Events that hold K2 entries but are no longer wanted.
    let stale: Vec<String> = hooks
        .keys()
        .filter(|k| !wanted.iter().any(|(w, _)| w == *k))
        .cloned()
        .collect();
    for k in stale {
        let Some(v) = hooks.get_mut(&k) else { continue };
        let Some(arr) = v.as_array_mut() else { continue };
        if replace_k2(arr, None, flat) && arr.is_empty() {
            hooks.remove(&k);
        }
    }
    for (event, entry) in wanted {
        match hooks.get_mut(event) {
            Some(Value::Array(arr)) => {
                replace_k2(arr, Some(entry.clone()), flat);
            }
            // Another tool's non-array value: not ours to rewrite.
            Some(_) => return Err(EditError::Unreadable),
            None => {
                hooks.insert(event.clone(), Value::Array(vec![entry.clone()]));
            }
        }
    }
    if flat && !wanted.is_empty() && !root.contains_key("version") {
        root.insert("version".to_string(), json!(1));
    }
    // An uninstall may leave `hooks: {}`; the key is the user's, so it stays.
    Ok(())
}

/// Drop every K2 hook from `arr` and put `new` where the first one was (or
/// at the end). A group left with no hooks is dropped. Returns whether a
/// K2 entry was found.
fn replace_k2(arr: &mut Vec<Value>, new: Option<Value>, flat: bool) -> bool {
    let mut first: Option<usize> = None;
    let mut out: Vec<Value> = Vec::with_capacity(arr.len() + 1);
    for item in arr.drain(..) {
        if flat {
            if hook_is_k2(&item) {
                first.get_or_insert(out.len());
                continue;
            }
            out.push(item);
            continue;
        }
        let Some(hooks) = item.get("hooks").and_then(Value::as_array) else {
            out.push(item);
            continue;
        };
        let kept: Vec<Value> = hooks.iter().filter(|h| !hook_is_k2(h)).cloned().collect();
        if kept.len() == hooks.len() {
            out.push(item);
            continue;
        }
        // A mixed group keeps its other hooks (and matcher); the K2 group
        // goes right after it.
        first.get_or_insert(if kept.is_empty() { out.len() } else { out.len() + 1 });
        if !kept.is_empty() {
            let mut group = item.clone();
            group["hooks"] = Value::Array(kept);
            out.push(group);
        }
    }
    let found = first.is_some();
    if let Some(entry) = new {
        let at = first.unwrap_or(out.len());
        out.insert(at, entry);
    }
    *arr = out;
    found
}

/// Event names holding a K2 entry in `root.hooks`, sorted.
fn k2_events(root: &Value, flat: bool) -> Vec<String> {
    let Some(hooks) = root.get("hooks").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut out: Vec<String> = hooks
        .iter()
        .filter(|(_, v)| {
            v.as_array().is_some_and(|arr| {
                arr.iter().any(|item| {
                    if flat {
                        hook_is_k2(item)
                    } else {
                        item.get("hooks")
                            .and_then(Value::as_array)
                            .is_some_and(|h| h.iter().any(hook_is_k2))
                    }
                })
            })
        })
        .map(|(k, _)| k.clone())
        .collect();
    out.sort();
    out
}

// ── Read-only status (`/cli/hooks/status`) ───────────────────────────────

/// Per-CLI injection state read from disk: `{path, exists, injected,
/// events, configUnreadable}`, plus the script's path, presence and stamp.
/// DA18: Gemini is read from `~/.gemini/settings.json`, the file the
/// installer writes.
pub fn check_hook_injections(home: &Path) -> Value {
    let check = |cli: &str, path: PathBuf, flat: bool| -> Value {
        let path_str = path.to_string_lossy().into_owned();
        let Ok(bytes) = std::fs::read(&path) else {
            return json!({ "cli": cli, "path": path_str, "exists": false, "injected": false, "events": [], "configUnreadable": false });
        };
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(v) if v.is_object() => {
                let events = k2_events(&v, flat);
                json!({
                    "cli": cli,
                    "path": path_str,
                    "exists": true,
                    "injected": !events.is_empty(),
                    "events": events,
                    "configUnreadable": false,
                })
            }
            _ => json!({ "cli": cli, "path": path_str, "exists": true, "injected": false, "events": [], "configUnreadable": true }),
        }
    };
    let script = script_path(home);
    let stamp = std::fs::read_to_string(&script).ok().and_then(|b| script_stamp(&b));
    json!({
        "notify_script": {
            "path": script.to_string_lossy(),
            "exists": script.exists(),
            "version": stamp,
        },
        "claude": check("claude", claude_settings_path(home), false),
        "cursor": check("cursor", cursor_hooks_path(home), true),
        "gemini": check("gemini", gemini_settings_path(home), false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::agent_spawn_guard::GuardEnv;

    struct TempHome(PathBuf);
    impl TempHome {
        fn new(tag: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let p = std::env::temp_dir().join(format!("k2-hooks-{tag}-{}-{nanos}", std::process::id()));
            std::fs::create_dir_all(&p).expect("temp home");
            Self(p)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempHome {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn req(home: &Path, version: &str, claude: Option<&str>) -> InstallRequest<'static> {
        InstallRequest {
            home: Box::leak(home.to_path_buf().into_boxed_path()),
            daemon_version: Box::leak(version.to_string().into_boxed_str()),
            claude_version: claude.map(str::to_string),
            remove: false,
        }
    }

    fn read_json(p: &Path) -> Value {
        serde_json::from_slice(&std::fs::read(p).expect("read config")).expect("valid json")
    }

    fn mtime(p: &Path) -> std::time::SystemTime {
        std::fs::metadata(p).expect("meta").modified().expect("mtime")
    }

    /// Every file under `root`, relative.
    fn tree(root: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).expect("read_dir").flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    out.push(p.strip_prefix(root).expect("under root").to_path_buf());
                }
            }
        }
        out.sort();
        out
    }

    const SUPERSET: &str = "/opt/superset/hook.sh UserPromptSubmit";

    /// T-S1a: a Superset hook is kept, an unparsable file is untouched, and
    /// a second run writes nothing (mtime unchanged).
    #[test]
    fn t_s1a_keeps_other_tools_skips_unparsable_and_is_idempotent() {
        let home = TempHome::new("a");
        let h = home.path();
        std::fs::create_dir_all(h.join(".claude")).unwrap();
        std::fs::create_dir_all(h.join(".gemini")).unwrap();
        std::fs::write(
            claude_settings_path(h),
            serde_json::to_string_pretty(&json!({
                "model": "opus",
                "hooks": { "UserPromptSubmit": [ { "hooks": [ { "type": "command", "command": SUPERSET } ] } ] }
            }))
            .unwrap(),
        )
        .unwrap();
        let broken = b"{ \"hooks\": { oops ";
        std::fs::write(gemini_settings_path(h), broken).unwrap();

        let r = run(&req(h, "0.44.3", Some("2.1.292 (Claude Code)")));
        assert_eq!(r.script.state, ScriptState::Written);
        assert_eq!(r.claude_event_set, "full");
        let claude = r.clis.iter().find(|c| c.cli == "claude").unwrap();
        assert_eq!(claude.state, ConfigState::Written);
        let gemini = r.clis.iter().find(|c| c.cli == "gemini").unwrap();
        assert_eq!(gemini.state, ConfigState::ConfigUnreadable);
        assert_eq!(std::fs::read(gemini_settings_path(h)).unwrap(), broken, "unparsable file is byte-identical");
        assert_eq!(r.clis.iter().find(|c| c.cli == "cursor").unwrap().state, ConfigState::Absent);
        assert!(!h.join(".cursor").exists(), "no config dir is invented for an absent CLI");

        let v = read_json(&claude_settings_path(h));
        assert_eq!(v["model"], "opus");
        let ups = v["hooks"]["UserPromptSubmit"].as_array().unwrap();
        assert_eq!(ups.len(), 2);
        assert_eq!(ups[0]["hooks"][0]["command"], SUPERSET, "Superset's entry kept, first");
        assert!(is_k2_command(ups[1]["hooks"][0]["command"].as_str().unwrap()));
        let mut keys: Vec<&String> = v["hooks"].as_object().unwrap().keys().collect();
        keys.sort();
        let mut want: Vec<&str> = CLAUDE_FULL_SET.to_vec();
        want.sort();
        assert_eq!(keys, want);
        assert_eq!(v["hooks"]["PreToolUse"][0]["hooks"][0]["async"], true);
        assert!(v["hooks"]["Stop"][0]["hooks"][0].get("async").is_none(), "lifecycle stays sync");
        let cmd = v["hooks"]["Stop"][0]["hooks"][0]["command"].as_str().unwrap();
        assert!(cmd.starts_with("[ -x '") && cmd.contains("K2_HOOK_AGENT_PID=$PPID K2_HOOK_SOURCE=claude") && cmd.ends_with("\"$@\" || true"), "{cmd}");

        let before_cfg = mtime(&claude_settings_path(h));
        let before_script = mtime(&script_path(h));
        std::thread::sleep(Duration::from_millis(20));
        let r2 = run(&req(h, "0.44.3", Some("2.1.292")));
        assert_eq!(r2.script.state, ScriptState::Unchanged);
        assert!(r2.clis.iter().all(|c| c.state != ConfigState::Written), "{:?}", r2.clis);
        assert_eq!(mtime(&claude_settings_path(h)), before_cfg);
        assert_eq!(mtime(&script_path(h)), before_script);
    }

    /// T-S1a+ (A2): malformed stays byte-identical, an old-format K2 entry
    /// is upgraded in place, uninstall keeps Superset.
    #[test]
    fn t_s1a_plus_upgrades_in_place_and_uninstalls_only_k2() {
        let home = TempHome::new("a2");
        let h = home.path();
        std::fs::create_dir_all(h.join(".claude")).unwrap();
        let old_cmd = format!("[ -x '{}' ] && '{}' \"$@\" || true", h.join(".k2so/hooks/notify.sh").display(), h.join(".k2so/hooks/notify.sh").display());
        std::fs::write(
            claude_settings_path(h),
            serde_json::to_string(&json!({
                "hooks": {
                    "Stop": [
                        { "hooks": [ { "type": "command", "command": old_cmd } ] },
                        { "hooks": [ { "type": "command", "command": SUPERSET } ] }
                    ],
                    "PostToolUse": [
                        { "matcher": "Bash", "hooks": [ { "type": "command", "command": SUPERSET }, { "type": "command", "command": old_cmd } ] }
                    ]
                }
            }))
            .unwrap(),
        )
        .unwrap();
        run(&req(h, "0.44.3", None));
        let v = read_json(&claude_settings_path(h));
        let stop = v["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2, "{stop:?}");
        let new_cmd = stop[0]["hooks"][0]["command"].as_str().unwrap();
        assert!(new_cmd.contains(NOTIFY_FRAGMENT) && new_cmd.contains("K2_HOOK_AGENT_PID"), "upgraded in place, first slot: {new_cmd}");
        assert_eq!(stop[1]["hooks"][0]["command"], SUPERSET);
        // Mixed group: the K2 hook leaves the matcher group, Superset stays in it.
        let ptu = v["hooks"]["PostToolUse"].as_array().unwrap();
        assert_eq!(ptu.len(), 2);
        assert_eq!(ptu[0]["hooks"][0]["command"], SUPERSET.to_string());
        assert_eq!(ptu[0]["matcher"], "Bash");
        assert!(is_k2_command(ptu[1]["hooks"][0]["command"].as_str().unwrap()));
        // Base set (unknown version): no full-set-only event.
        assert!(v["hooks"].get("StopFailure").is_none());
        assert!(v["hooks"]["PreToolUse"][0]["hooks"][0].get("async").is_none(), "no async below the floor");

        let mut rm = req(h, "0.44.3", None);
        rm.remove = true;
        let r = run(&rm);
        assert_eq!(r.action, "remove");
        let v = read_json(&claude_settings_path(h));
        let hooks = v["hooks"].as_object().unwrap();
        assert_eq!(hooks.len(), 2, "only the events holding Superset survive: {hooks:?}");
        assert_eq!(hooks["Stop"][0]["hooks"][0]["command"], SUPERSET);
        assert_eq!(hooks["PostToolUse"][0]["hooks"][0]["command"], SUPERSET);
        assert!(!serde_json::to_string(&v).unwrap().contains("notify.sh"));
        assert!(script_path(h).exists(), "uninstall leaves the script");
    }

    #[test]
    fn non_object_root_or_hooks_is_unreadable_and_untouched() {
        let home = TempHome::new("obj");
        let h = home.path();
        std::fs::create_dir_all(h.join(".claude")).unwrap();
        for body in [&b"[1,2]"[..], &b"{\"hooks\": []}"[..], &b"{\"hooks\": {\"Stop\": \"x\"}}"[..]] {
            std::fs::write(claude_settings_path(h), body).unwrap();
            let r = run(&req(h, "0.44.3", None));
            assert_eq!(r.clis[0].state, ConfigState::ConfigUnreadable, "{}", String::from_utf8_lossy(body));
            assert_eq!(std::fs::read(claude_settings_path(h)).unwrap(), body);
            assert_eq!(r.failures().len(), 1);
        }
    }

    /// T-S1j (A10): an installer older than the on-disk stamp writes nothing.
    #[test]
    fn t_s1j_older_installer_writes_nothing() {
        let home = TempHome::new("j");
        let h = home.path();
        std::fs::create_dir_all(h.join(".claude")).unwrap();
        run(&req(h, "0.45.0", Some("2.1.292")));
        let script_before = std::fs::read(script_path(h)).unwrap();
        // Drop the K2 entries so an older run would visibly re-add them.
        std::fs::write(claude_settings_path(h), b"{}").unwrap();
        let r = run(&req(h, "0.44.3", None));
        assert_eq!(r.script.state, ScriptState::SkippedNewer);
        assert_eq!(r.script.on_disk_version.as_deref(), Some("0.45.0"));
        assert!(r.clis.iter().all(|c| c.state == ConfigState::SkippedNewer));
        assert_eq!(std::fs::read(script_path(h)).unwrap(), script_before);
        assert_eq!(std::fs::read(claude_settings_path(h)).unwrap(), b"{}");
        // Same or newer version does write.
        assert_eq!(run(&req(h, "0.45.1", None)).script.state, ScriptState::Written);
    }

    #[test]
    fn cursor_entries_live_under_hooks_and_legacy_root_entries_are_cleared() {
        let home = TempHome::new("cur");
        let h = home.path();
        std::fs::create_dir_all(h.join(".cursor")).unwrap();
        let legacy = format!("[ -x '{p}' ] && '{p}' Start || true", p = h.join(".k2/hooks/notify.sh").display());
        std::fs::write(
            cursor_hooks_path(h),
            serde_json::to_string(&json!({ "beforeSubmitPrompt": [ { "command": legacy } ], "hooks": { "afterFileEdit": [ { "command": "./fmt.sh" } ] } })).unwrap(),
        )
        .unwrap();
        let r = run(&req(h, "0.44.3", None));
        assert_eq!(r.clis.iter().find(|c| c.cli == "cursor").unwrap().state, ConfigState::Written);
        let v = read_json(&cursor_hooks_path(h));
        assert!(v.get("beforeSubmitPrompt").is_none(), "root-level legacy entry cleared: {v}");
        assert_eq!(v["version"], 1);
        assert_eq!(v["hooks"]["afterFileEdit"][0]["command"], "./fmt.sh");
        let cmd = v["hooks"]["beforeSubmitPrompt"][0]["command"].as_str().unwrap();
        assert!(cmd.contains("K2_HOOK_SOURCE=cursor") && cmd.contains(" Start || true"), "{cmd}");
        let status = check_hook_injections(h);
        assert_eq!(status["cursor"]["injected"], true);
        assert_eq!(status["cursor"]["events"].as_array().unwrap().len(), CURSOR_EVENTS.len());
    }

    /// DA18: status reads Gemini from `~/.gemini/settings.json`.
    #[test]
    fn gemini_status_reads_the_file_the_installer_writes() {
        let home = TempHome::new("gem");
        let h = home.path();
        std::fs::create_dir_all(h.join(".gemini")).unwrap();
        run(&req(h, "0.44.3", None));
        let s = check_hook_injections(h);
        assert_eq!(s["gemini"]["path"], gemini_settings_path(h).to_string_lossy().as_ref());
        assert_eq!(s["gemini"]["injected"], true);
        assert_eq!(s["gemini"]["events"], json!(["AfterAgent", "AfterTool", "BeforeAgent"]));
        assert_eq!(s["notify_script"]["version"], "0.44.3");
    }

    /// A42: the installer never writes outside `home`.
    #[test]
    fn installer_writes_only_under_home() {
        let outside = TempHome::new("outside");
        let home = TempHome::new("inside");
        let h = home.path();
        for d in [".claude", ".cursor", ".gemini"] {
            std::fs::create_dir_all(h.join(d)).unwrap();
        }
        let before = tree(outside.path());
        run(&req(h, "0.44.3", Some("2.1.292")));
        let mut rm = req(h, "0.44.3", None);
        rm.remove = true;
        run(&rm);
        assert_eq!(tree(outside.path()), before);
        for f in tree(h) {
            assert!(
                [".claude", ".cursor", ".gemini", ".k2"].iter().any(|d| f.starts_with(d)),
                "unexpected file {}",
                f.display()
            );
        }
    }

    #[test]
    fn version_sets_follow_the_floor() {
        assert_eq!(claude_events_for(Some("2.1.292 (Claude Code)")), (CLAUDE_FULL_SET, true));
        assert_eq!(claude_events_for(Some("2.2.0")), (CLAUDE_FULL_SET, true));
        assert_eq!(claude_events_for(Some("2.1.291")), (CLAUDE_BASE_SET, false));
        assert_eq!(claude_events_for(Some("garbage")), (CLAUDE_BASE_SET, false));
        assert_eq!(claude_events_for(None), (CLAUDE_BASE_SET, false));
        assert_eq!(parse_version("claude 10.20.30-beta"), Some((10, 20, 30)));
        // T-S1h (static half): the floor version's set still carries the
        // turn start and end. The live half is fixture capture (§11).
        let (full, _) = claude_events_for(Some("2.1.292"));
        assert!(full.contains(&"UserPromptSubmit") && full.contains(&"Stop"));
        for e in CLAUDE_BASE_SET {
            assert!(CLAUDE_FULL_SET.contains(e), "{e} is in the base set but not the full set");
        }
    }

    #[test]
    fn install_gate_rules() {
        let real = Path::new("/home/real");
        assert_eq!(install_gate(Some("0"), false, real, Some(real)), Err("K2_HOOK_INSTALL=0"));
        assert_eq!(install_gate(Some("off"), false, real, Some(real)), Err("K2_HOOK_INSTALL=0"));
        assert_eq!(install_gate(None, true, real, Some(real)), Err("debug build on the real home"));
        assert_eq!(install_gate(None, true, Path::new("/tmp/x"), Some(real)), Ok(()));
        assert_eq!(install_gate(None, false, real, Some(real)), Ok(()), "release builds install");
        assert_eq!(install_gate(Some("1"), false, real, None), Ok(()));
        assert_eq!(install_gate(None, true, Path::new("/tmp/x"), None), Err("debug build on the real home"));
    }

    #[cfg(unix)]
    fn write_exe(path: &Path, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(path, body).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// T-S1g (A9): a real-looking `claude` on PATH under a temp HOME is
    /// never started by the probe; a test shim is.
    #[cfg(unix)]
    #[test]
    fn t_s1g_probe_never_starts_a_real_claude_under_temp_home() {
        let bin = TempHome::new("realbin");
        let marker = bin.path().join("ran");
        write_exe(
            &bin.path().join("claude"),
            &format!("#!/bin/sh\ntouch '{}'\necho '9.9.9 (Claude Code)'\n", marker.display()),
        );
        let home = TempHome::new("probehome");
        let guard = GuardEnv {
            shim_dirs: None,
            home: Some(home.path().to_path_buf()),
            temp_dirs: vec![std::env::temp_dir()],
            allow_real: false,
            test_build: false,
        };
        let search = bin.path().to_string_lossy().into_owned();
        assert_eq!(probe_version_with("claude", &search, &guard, Duration::from_secs(5)), None);
        assert!(!marker.exists(), "the probe started a real claude");

        let shims = TempHome::new("shims");
        write_exe(&shims.path().join("claude"), "#!/bin/sh\necho '2.1.292 (Claude Code)'\n");
        let guard = GuardEnv { shim_dirs: Some(vec![shims.path().to_path_buf()]), ..guard };
        assert_eq!(
            probe_version_with("claude", &search, &guard, Duration::from_secs(5)).as_deref(),
            Some("2.1.292")
        );
        assert!(!marker.exists());
    }

    #[cfg(unix)]
    #[test]
    fn probe_times_out_on_a_hung_cli() {
        let shims = TempHome::new("slow");
        write_exe(&shims.path().join("claude"), "#!/bin/sh\nsleep 30\n");
        let guard = GuardEnv {
            shim_dirs: Some(vec![shims.path().to_path_buf()]),
            home: None,
            temp_dirs: vec![],
            allow_real: false,
            test_build: false,
        };
        let t = Instant::now();
        assert_eq!(probe_version_with("claude", "", &guard, Duration::from_millis(300)), None);
        assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
    }

    /// A one-shot HTTP server on a Unix socket or loopback TCP: accepts
    /// one request, answers `status`, hands back the raw request bytes.
    #[cfg(unix)]
    fn serve_one(listener: ServerSock, status: &'static str) -> std::thread::JoinHandle<Vec<u8>> {
        use std::io::Write;
        std::thread::spawn(move || {
            let mut conn: Box<dyn ReadWrite> = match listener {
                ServerSock::Unix(l) => Box::new(l.accept().expect("accept").0),
                ServerSock::Tcp(l) => Box::new(l.accept().expect("accept").0),
            };
            let mut raw = Vec::new();
            let mut chunk = [0u8; 65536];
            loop {
                if let Some(end) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&raw[..end]).to_ascii_lowercase();
                    let len = head
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().expect("len")))
                        .unwrap_or(0);
                    if raw.len() >= end + 4 + len {
                        break;
                    }
                }
                let n = conn.read(&mut chunk).expect("read");
                if n == 0 {
                    break;
                }
                raw.extend_from_slice(&chunk[..n]);
            }
            conn.write_all(format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\n\r\n").as_bytes()).expect("reply");
            raw
        })
    }

    #[cfg(unix)]
    trait ReadWrite: Read + std::io::Write {}
    #[cfg(unix)]
    impl<T: Read + std::io::Write> ReadWrite for T {}

    #[cfg(unix)]
    enum ServerSock {
        Unix(std::os::unix::net::UnixListener),
        Tcp(std::net::TcpListener),
    }

    /// Run the generated script under bash with `stdin`, returning
    /// (exit code, stdout, elapsed).
    #[cfg(unix)]
    fn run_script(home: &Path, env: &[(&str, &str)], stdin: &[u8]) -> (i32, Vec<u8>, Duration) {
        use std::io::Write;
        let script = script_path(home);
        let mut cmd = Command::new("/bin/bash");
        cmd.arg(&script)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (k, v) in env {
            cmd.env(k, v);
        }
        let t = Instant::now();
        let mut child = cmd.spawn().expect("spawn bash");
        child.stdin.take().expect("stdin").write_all(stdin).expect("write stdin");
        let out = child.wait_with_output().expect("wait");
        (out.status.code().expect("exit code"), out.stdout, t.elapsed())
    }

    /// Split a raw request into (lower-cased head, body).
    fn split_request(raw: &[u8]) -> (String, Vec<u8>) {
        let end = raw.windows(4).position(|w| w == b"\r\n\r\n").expect("request head");
        (String::from_utf8_lossy(&raw[..end]).to_ascii_lowercase(), raw[end + 4..].to_vec())
    }

    /// T-S1b: the script posts once over the cell socket with the headers,
    /// the body byte-identical, nothing on stdout, exit 0.
    #[cfg(unix)]
    #[test]
    fn t_s1b_script_posts_the_raw_payload_over_the_cell_socket() {
        let home = TempHome::new("s1b");
        write_script(&script_path(home.path()), "0.44.3").expect("script");
        let sock = PathBuf::from(format!("/tmp/k2s1b-{}-{}.sock", std::process::id(), Instant::now().elapsed().as_nanos() % 100000));
        let _ = std::fs::remove_file(&sock);
        let server = serve_one(ServerSock::Unix(std::os::unix::net::UnixListener::bind(&sock).expect("bind")), "204 No Content");
        let payload = "{\"hook_event_name\":\"PreToolUse\",\"session_id\":\"s1\",\"tool_name\":\"Bash\",\"tool_input\":{\"command\":\"echo \\\"h\u{e9}llo\\\"\"}}\n\n";
        let (code, stdout, _) = run_script(
            home.path(),
            &[
                ("K2_PANE_ID", "pane-s1b"),
                ("K2_HOOK_SOCK", sock.to_str().expect("utf8")),
                ("K2_HOOK_TOKEN", "sid.scopedsecret"),
                ("K2_HOOK_AGENT_PID", "4242"),
                ("K2_HOOK_SOURCE", "claude"),
                ("CLAUDE_CODE_VERSION", "2.1.292"),
            ],
            payload.as_bytes(),
        );
        let raw = server.join().expect("server thread");
        let _ = std::fs::remove_file(&sock);
        assert_eq!(code, 0);
        assert!(stdout.is_empty(), "stdout: {:?}", String::from_utf8_lossy(&stdout));
        let (head, body) = split_request(&raw);
        assert!(head.starts_with("post /hook/event http/1.1"), "{head}");
        for h in [
            "x-k2-pane: pane-s1b",
            "x-k2-agent-pid: 4242",
            "x-k2-hook-source: claude",
            "x-k2-hook-version: 2",
            "x-k2-claude-version: 2.1.292",
            "authorization: bearer sid.scopedsecret",
        ] {
            assert!(head.contains(h), "missing {h} in {head}");
        }
        assert!(!head.contains("x-k2-hook-truncated"));
        assert_eq!(body, payload.as_bytes(), "body must be byte-identical");
    }

    /// No socket: loopback TCP with the disk owner token. Over 1 MiB: a
    /// header-only post naming the event.
    #[cfg(unix)]
    #[test]
    fn t_s1b_script_falls_back_to_tcp_and_truncates_huge_bodies() {
        let home = TempHome::new("s1b-tcp");
        write_script(&script_path(home.path()), "0.44.3").expect("script");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        std::fs::write(home.path().join(".k2/heartbeat.port"), port.to_string()).unwrap();
        std::fs::write(home.path().join(".k2/heartbeat.token"), "owner-tok").unwrap();
        let server = serve_one(ServerSock::Tcp(listener), "204 No Content");
        let mut payload = String::from("{\"hook_event_name\":\"PostToolUse\",\"tool_response\":\"");
        payload.push_str(&"x".repeat(super::super::envelope::MAX_BODY_BYTES + 10));
        payload.push_str("\"}");
        let (code, stdout, _) = run_script(
            home.path(),
            &[("K2SO_PANE_ID", "pane-tcp"), ("K2_HOOK_AGENT_PID", "77")],
            payload.as_bytes(),
        );
        let raw = server.join().expect("server");
        assert_eq!(code, 0);
        assert!(stdout.is_empty());
        let (head, body) = split_request(&raw);
        assert!(head.contains("authorization: bearer owner-tok"), "{head}");
        assert!(head.contains("x-k2-pane: pane-tcp"));
        assert!(head.contains("x-k2-hook-truncated: 1"));
        assert!(head.contains("x-k2-hook-event: posttooluse"), "{head}");
        assert!(body.is_empty(), "a truncated post carries no body");
    }

    /// The daemon down: exit 0 fast, nothing printed. No pane: no post.
    #[cfg(unix)]
    #[test]
    fn t_s1b_script_exits_zero_quickly_with_the_daemon_down() {
        let home = TempHome::new("s1b-down");
        write_script(&script_path(home.path()), "0.44.3").expect("script");
        // A port nobody listens on (bind then drop).
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        std::fs::write(home.path().join(".k2/heartbeat.port"), port.to_string()).unwrap();
        let (code, stdout, took) = run_script(
            home.path(),
            &[("K2_PANE_ID", "p"), ("K2_HOOK_SOCK", "/tmp/k2-no-such.sock")],
            b"{\"hook_event_name\":\"Stop\"}",
        );
        assert_eq!(code, 0);
        assert!(stdout.is_empty());
        assert!(took < Duration::from_secs(3), "{took:?}");
        let (code, stdout, _) = run_script(home.path(), &[], b"{\"hook_event_name\":\"Stop\"}");
        assert_eq!((code, stdout.is_empty()), (0, true));
    }

    /// 0.45.1: neither the scoped passport nor the owner token is on
    /// curl's argv (world-readable via `ps`), yet both still reach the
    /// daemon as the Authorization header. A `curl` shim first on PATH
    /// logs every argv it is given, then runs the real curl.
    #[cfg(unix)]
    #[test]
    fn t_s1b_tokens_never_on_curl_argv() {
        let home = TempHome::new("s1b-argv");
        write_script(&script_path(home.path()), "0.44.3").expect("script");
        let shim_dir = home.path().join("shim");
        std::fs::create_dir_all(&shim_dir).unwrap();
        let argv_log = home.path().join("curl-argv.log");
        let shim = shim_dir.join("curl");
        std::fs::write(
            &shim,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexec /usr/bin/curl \"$@\"\n",
                argv_log.display()
            ),
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path_env = format!("{}:/usr/bin:/bin", shim_dir.display());

        // Arm 1: the per-cell socket with the scoped passport.
        let sock = PathBuf::from(format!("/tmp/k2argv-{}-{}.sock", std::process::id(), Instant::now().elapsed().as_nanos() % 100000));
        let _ = std::fs::remove_file(&sock);
        let server = serve_one(ServerSock::Unix(std::os::unix::net::UnixListener::bind(&sock).expect("bind")), "204 No Content");
        let (code, _, _) = run_script(
            home.path(),
            &[
                ("PATH", &path_env),
                ("K2_PANE_ID", "pane-argv"),
                ("K2_HOOK_SOCK", sock.to_str().expect("utf8")),
                ("K2_HOOK_TOKEN", "sid.scoped-argv-secret"),
            ],
            b"{\"hook_event_name\":\"Stop\"}",
        );
        let raw = server.join().expect("server thread");
        let _ = std::fs::remove_file(&sock);
        assert_eq!(code, 0);
        let (head, body) = split_request(&raw);
        assert!(head.contains("authorization: bearer sid.scoped-argv-secret"), "{head}");
        assert_eq!(body, b"{\"hook_event_name\":\"Stop\"}", "stdin payload still the body");

        // Arm 2: loopback TCP with the disk owner token.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        std::fs::write(home.path().join(".k2/heartbeat.port"), port.to_string()).unwrap();
        std::fs::write(home.path().join(".k2/heartbeat.token"), "owner-argv-secret").unwrap();
        let server = serve_one(ServerSock::Tcp(listener), "204 No Content");
        let (code, _, _) = run_script(
            home.path(),
            &[("PATH", &path_env), ("K2_PANE_ID", "pane-argv")],
            b"{\"hook_event_name\":\"Stop\"}",
        );
        let raw = server.join().expect("server");
        assert_eq!(code, 0);
        let (head, _) = split_request(&raw);
        assert!(head.contains("authorization: bearer owner-argv-secret"), "{head}");

        let log = std::fs::read_to_string(&argv_log).expect("the shim must have run");
        assert_eq!(log.lines().count(), 2, "one curl per arm: {log}");
        assert!(!log.contains("scoped-argv-secret"), "scoped token on curl argv: {log}");
        assert!(!log.contains("owner-argv-secret"), "owner token on curl argv: {log}");
        assert!(!log.to_ascii_lowercase().contains("authorization"), "{log}");
    }

    #[test]
    fn script_carries_its_stamp_and_never_echoes() {
        let s = generate_hook_script("0.44.3");
        assert_eq!(script_stamp(&s).as_deref(), Some("0.44.3"));
        assert!(s.contains("/hook/event") && !s.contains("/hook/complete"));
        assert!(s.contains("X-K2-Hook-Version: 2"));
        assert!(!s.contains("mktemp"), "the payload must never touch disk (DA5)");
    }
}
