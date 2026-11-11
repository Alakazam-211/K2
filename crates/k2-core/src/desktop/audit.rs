//! The desktop access log (S7): `~/.k2/desktop-audit.jsonl`, 0600, one
//! JSON object per line, rotated at [`ROTATE_BYTES`] to `.1` (the
//! `auth_audit` pattern).
//!
//! **Never frames or keystrokes.** A line has only the fixed fields of
//! [`AuditLine`] (who, role, ingress, IP, device, display, the viewer id,
//! the target login of a grant, a mode, a duration, an outcome); there is
//! no free-text or payload field, and [`ALLOWED_KEYS`] pins that.
//!
//! **Strict, not best effort** (vs-live R33): [`write`] returns the error,
//! and a viewer whose connect line can't be written is refused
//! (`audit_unavailable`). Management routes report a failed write too.

use std::io::{BufRead, Write as _};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use serde_json::{json, Value};

/// Rotate once the live file reaches this size.
pub const ROTATE_BYTES: u64 = 5 * 1024 * 1024;
pub const DEFAULT_LIMIT: usize = 50;
pub const MAX_LIMIT: usize = 1000;
/// Any one field is cut to this many characters.
const MAX_FIELD: usize = 128;

/// Every key a line may have.
pub const ALLOWED_KEYS: &[&str] = &[
    "ts", "event", "desktop", "who", "role", "ingress", "ip", "device", "display", "viewer", "target", "mode",
    "durationMs", "outcome",
];

/// What happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Enable,
    Disable,
    Policy,
    Grant,
    Revoke,
    Kick,
    Open,
    Ticket,
    Connect,
    Disconnect,
    ControlTaken,
    ControlHandedBack,
    ControlTimedOut,
    LocalHold,
    ModeChange,
    Refused,
    /// The controller read the host's clipboard (Copy theirs). Only that
    /// it happened, and its outcome (`ok` or why not); never the text.
    ClipboardRead,
    /// The controller put text on the host's clipboard (Send my
    /// clipboard). Never the text.
    ClipboardWrite,
}

impl Event {
    pub fn as_str(self) -> &'static str {
        match self {
            Event::Enable => "enable",
            Event::Disable => "disable",
            Event::Policy => "policy",
            Event::Grant => "grant",
            Event::Revoke => "revoke",
            Event::Kick => "kick",
            Event::Open => "open",
            Event::Ticket => "ticket",
            Event::Connect => "connect",
            Event::Disconnect => "disconnect",
            Event::ControlTaken => "control_taken",
            Event::ControlHandedBack => "control_handed_back",
            Event::ControlTimedOut => "control_timed_out",
            Event::LocalHold => "local_hold",
            Event::ModeChange => "mode_change",
            Event::Refused => "refused",
            Event::ClipboardRead => "clipboard_read",
            Event::ClipboardWrite => "clipboard_write",
        }
    }
}

/// One line. Empty strings and `None` are left out of the JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditLine {
    pub event: Event,
    pub desktop: String,
    /// `login:<name>`, `owner`, `shell`.
    pub who: String,
    pub role: String,
    /// `loopback` | `lan` | `tunnel` | `app`.
    pub ingress: String,
    pub ip: String,
    pub device: String,
    pub display: String,
    pub viewer: String,
    /// The login a grant, revoke or kick named.
    pub target: String,
    /// `view` | `control`.
    pub mode: String,
    pub duration_ms: Option<u64>,
    /// `ok`, or the refusal code.
    pub outcome: String,
}

impl AuditLine {
    pub fn new(event: Event, desktop: &str, who: &str) -> Self {
        AuditLine {
            event,
            desktop: desktop.to_string(),
            who: who.to_string(),
            role: String::new(),
            ingress: String::new(),
            ip: String::new(),
            device: String::new(),
            display: String::new(),
            viewer: String::new(),
            target: String::new(),
            mode: String::new(),
            duration_ms: None,
            outcome: "ok".to_string(),
        }
    }

    pub fn role(mut self, v: &str) -> Self {
        self.role = v.to_string();
        self
    }
    pub fn ingress(mut self, v: &str) -> Self {
        self.ingress = v.to_string();
        self
    }
    pub fn ip(mut self, v: &str) -> Self {
        self.ip = v.to_string();
        self
    }
    pub fn device(mut self, v: &str) -> Self {
        self.device = v.to_string();
        self
    }
    pub fn display(mut self, v: &str) -> Self {
        self.display = v.to_string();
        self
    }
    pub fn viewer(mut self, v: &str) -> Self {
        self.viewer = v.to_string();
        self
    }
    pub fn target(mut self, v: &str) -> Self {
        self.target = v.to_string();
        self
    }
    pub fn mode(mut self, v: &str) -> Self {
        self.mode = v.to_string();
        self
    }
    pub fn duration_ms(mut self, v: u64) -> Self {
        self.duration_ms = Some(v);
        self
    }
    pub fn outcome(mut self, v: &str) -> Self {
        self.outcome = v.to_string();
        self
    }

    /// The JSON line (without the newline).
    pub fn render(&self, ts: &str) -> Value {
        let mut m = serde_json::Map::new();
        m.insert("ts".into(), json!(ts));
        m.insert("event".into(), json!(self.event.as_str()));
        for (k, v) in [
            ("desktop", &self.desktop),
            ("who", &self.who),
            ("role", &self.role),
            ("ingress", &self.ingress),
            ("ip", &self.ip),
            ("device", &self.device),
            ("display", &self.display),
            ("viewer", &self.viewer),
            ("target", &self.target),
            ("mode", &self.mode),
            ("outcome", &self.outcome),
        ] {
            let v = clean(v);
            if !v.is_empty() {
                m.insert(k.into(), json!(v));
            }
        }
        if let Some(d) = self.duration_ms {
            m.insert("durationMs".into(), json!(d));
        }
        Value::Object(m)
    }
}

fn clean(raw: &str) -> String {
    let mut s: String = raw.trim().chars().take(MAX_FIELD).collect();
    s.retain(|c| !c.is_control());
    s
}

fn dir() -> PathBuf {
    crate::paths::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".k2")
}

/// `~/.k2/desktop-audit.jsonl`
pub fn path() -> PathBuf {
    dir().join("desktop-audit.jsonl")
}

/// The one rotated generation.
pub fn rotated_path() -> PathBuf {
    dir().join("desktop-audit.jsonl.1")
}

fn lock() -> &'static Mutex<()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(()))
}

#[cfg(unix)]
fn restrict(file: &std::path::Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600)).map_err(|e| format!("chmod 0600 {}: {e}", file.display()))
}

#[cfg(not(unix))]
fn restrict(file: &std::path::Path) -> Result<(), String> {
    crate::private_fs::make_private_file(file).map_err(|e| format!("make owner-only {}: {e}", file.display()))
}

/// Append one line. `Err` when it couldn't be written (the caller refuses
/// the action that needed it).
pub fn write(line: &AuditLine) -> Result<(), String> {
    crate::guard_test_store_home("desktop-audit.jsonl");
    let ts = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let mut text = line.render(&ts).to_string();
    text.push('\n');
    let _g = lock().lock().unwrap_or_else(|p| p.into_inner());
    let d = dir();
    std::fs::create_dir_all(&d).map_err(|e| format!("mkdir {}: {e}", d.display()))?;
    let file = path();
    if let Ok(meta) = std::fs::metadata(&file) {
        if meta.len() >= ROTATE_BYTES {
            std::fs::rename(&file, rotated_path()).map_err(|e| format!("rotate {}: {e}", file.display()))?;
        }
    }
    let created = !file.exists();
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file)
        .map_err(|e| format!("open {}: {e}", file.display()))?;
    if created {
        restrict(&file)?;
    }
    f.write_all(text.as_bytes()).map_err(|e| format!("append {}: {e}", file.display()))
}

fn read_file(p: &std::path::Path, out: &mut Vec<Value>) -> Result<(), String> {
    let f = match std::fs::File::open(p) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("open {}: {e}", p.display())),
    };
    for line in std::io::BufReader::new(f).lines() {
        let line = line.map_err(|e| format!("read {}: {e}", p.display()))?;
        // A torn last line during rotation is skipped, never fatal.
        if let Ok(v) = serde_json::from_str::<Value>(&line) {
            out.push(v);
        }
    }
    Ok(())
}

/// Clamp a requested limit into `1..=MAX_LIMIT` (`None` → default).
pub fn clamp_limit(requested: Option<usize>) -> usize {
    requested.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

/// Lines newest first, at most `limit`, only those at or after
/// `since_rfc3339` when given (RFC 3339 compares as text in UTC `Z` form).
pub fn read(since_rfc3339: Option<&str>, limit: usize) -> Result<Vec<Value>, String> {
    let mut all = Vec::new();
    read_file(&rotated_path(), &mut all)?;
    read_file(&path(), &mut all)?;
    let limit = limit.clamp(1, MAX_LIMIT);
    Ok(all
        .into_iter()
        .rev()
        .filter(|v| match since_rfc3339 {
            Some(s) => v.get("ts").and_then(Value::as_str).is_some_and(|t| t >= s),
            None => true,
        })
        .take(limit)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_has_only_the_fixed_fields() {
        let l = AuditLine::new(Event::Connect, "console", "login:rosson")
            .role("owner")
            .ingress("tunnel")
            .ip("203.0.113.9")
            .device("MacBook")
            .display("1")
            .viewer("dv_1")
            .target("mel")
            .mode("control")
            .duration_ms(12)
            .outcome("ok");
        let v = l.render("2026-11-10T00:00:00.000Z");
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        for k in &keys {
            assert!(ALLOWED_KEYS.contains(k), "unexpected key {k}");
        }
        assert_eq!(keys.len(), ALLOWED_KEYS.len(), "every field rendered: {keys:?}");
        // The struct itself has no place for a keystroke, a clipboard or a
        // frame: every field is one of these, by name.
        assert_eq!(v["event"], "connect");
        assert_eq!(v["durationMs"], 12);
    }

    #[test]
    fn empty_fields_are_left_out_and_fields_are_capped() {
        let v = AuditLine::new(Event::Enable, "console", "owner").render("t");
        assert_eq!(v, json!({"ts": "t", "event": "enable", "desktop": "console", "who": "owner", "outcome": "ok"}));
        let long = "x".repeat(500);
        let v = AuditLine::new(Event::Grant, "console", &format!("{long}\n")).render("t");
        assert_eq!(v["who"].as_str().unwrap().len(), MAX_FIELD);
    }

    #[test]
    fn write_then_read_newest_first_with_since_and_limit() {
        let _home = crate::test_env::TempHome::new();
        write(&AuditLine::new(Event::Enable, "console", "owner")).expect("write 1");
        write(&AuditLine::new(Event::Grant, "console", "owner").target("mel")).expect("write 2");
        write(&AuditLine::new(Event::Disable, "console", "owner")).expect("write 3");
        let all = read(None, 10).expect("read");
        let events: Vec<&str> = all.iter().map(|v| v["event"].as_str().unwrap()).collect();
        assert_eq!(events, vec!["disable", "grant", "enable"]);
        assert_eq!(read(None, 1).unwrap().len(), 1);
        assert!(read(Some("2999-01-01T00:00:00.000Z"), 10).unwrap().is_empty());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(path()).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_write_is_an_error_not_a_log_line() {
        let _home = crate::test_env::TempHome::new();
        // The log's path is a folder: it can't be appended to.
        std::fs::create_dir_all(path()).unwrap();
        assert!(write(&AuditLine::new(Event::Connect, "console", "login:x")).is_err());
    }
}
