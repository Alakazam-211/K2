//! Append-only auth audit log — `~/.k2/auth-audit.jsonl`.
//!
//! PRD `prd-connect-login-edge-only-v1.md` §5. One JSON object per line:
//!
//! ```json
//! {"ts":"<RFC3339>","event":"login","user":"<as submitted, trimmed, ≤64>",
//!  "outcome":"ok|bad_creds|locked|blocked_ingress|bad_attest:<reason>|rate_limited",
//!  "ingress":"loopback|lan|tunnel|edge:<kid>","ip":"<ip or ->","client":"web|api"}
//! ```
//!
//! Also `change-password`, `logout` and `set-password` events. The file is
//! 0600, appended under a process-wide lock, and rotated at
//! [`ROTATE_BYTES`] to `auth-audit.jsonl.1` (one generation kept, L2).
//! `tail(n)` reads the previous generation first so a fresh rotation does
//! not blank the view.

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// L2 — rotate once the live file reaches this size.
pub const ROTATE_BYTES: u64 = 5 * 1024 * 1024;
/// L3 — `GET /cli/users/audit?tail=N` defaults / bounds.
pub const DEFAULT_TAIL: usize = 50;
pub const MAX_TAIL: usize = 1000;
/// L1 — the submitted username is truncated to this many chars.
pub const MAX_USER_CHARS: usize = 64;

/// One audit record (all strings; the writer owns the timestamp).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEvent {
    /// `login` | `change-password` | `logout` | `set-password`.
    pub event: String,
    pub user: String,
    pub outcome: String,
    /// `loopback` | `lan` | `tunnel` | `edge:<kid>`.
    pub ingress: String,
    /// Client IP when known, else `-`.
    pub ip: String,
    /// `web` | `api`.
    pub client: String,
}

impl AuditEvent {
    pub fn new(
        event: &str,
        user: &str,
        outcome: impl Into<String>,
        ingress: impl Into<String>,
        ip: impl Into<String>,
        client: &str,
    ) -> Self {
        Self {
            event: event.to_string(),
            user: clean_user(user),
            outcome: outcome.into(),
            ingress: ingress.into(),
            ip: ip.into(),
            client: client.to_string(),
        }
    }
}

/// Trim + cap the submitted username (never store unbounded attacker input).
pub fn clean_user(raw: &str) -> String {
    let trimmed = raw.trim();
    let mut out: String = trimmed.chars().take(MAX_USER_CHARS).collect();
    // Keep the line single-line JSON-safe even before serde escaping.
    out.retain(|c| !c.is_control());
    out
}

fn config_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".k2")
}

/// `~/.k2/auth-audit.jsonl`
pub fn path() -> PathBuf {
    config_dir().join("auth-audit.jsonl")
}

/// `~/.k2/auth-audit.jsonl.1` (the single rotated generation).
pub fn rotated_path() -> PathBuf {
    config_dir().join("auth-audit.jsonl.1")
}

fn write_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[cfg(unix)]
fn restrict_mode(file: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Err(e) = std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600)) {
        crate::log_debug!("[auth_audit] WARN chmod 0600 {}: {e}", file.display());
    }
}

#[cfg(not(unix))]
fn restrict_mode(_file: &std::path::Path) {}

fn render_line(ev: &AuditEvent) -> String {
    serde_json::json!({
        "ts": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "event": ev.event,
        "user": ev.user,
        "outcome": ev.outcome,
        "ingress": ev.ingress,
        "ip": ev.ip,
        "client": ev.client,
    })
    .to_string()
}

/// Append one record. Never panics; a write failure is logged (the auth
/// decision has already been made — the audit trail is best-effort but
/// loud in the daemon log when it cannot be written).
pub fn record(ev: &AuditEvent) {
    if let Err(e) = try_record(ev) {
        crate::log_debug!("[auth_audit] WARN could not append audit record: {e}");
    }
}

/// [`record`] with the error surfaced (tests assert on it).
pub fn try_record(ev: &AuditEvent) -> Result<(), String> {
    let _g = write_lock().lock().unwrap_or_else(|p| p.into_inner());
    let dir = config_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    let file = path();
    // L2 rotation: size check BEFORE the append so the live file never
    // exceeds the cap by more than one line.
    if let Ok(meta) = std::fs::metadata(&file) {
        if meta.len() >= ROTATE_BYTES {
            let rotated = rotated_path();
            std::fs::rename(&file, &rotated)
                .map_err(|e| format!("rotate {} → {}: {e}", file.display(), rotated.display()))?;
        }
    }
    let created = !file.exists();
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file)
        .map_err(|e| format!("open {}: {e}", file.display()))?;
    if created {
        restrict_mode(&file);
    }
    let mut line = render_line(ev);
    line.push('\n');
    f.write_all(line.as_bytes())
        .map_err(|e| format!("append {}: {e}", file.display()))?;
    Ok(())
}

/// Clamp a requested tail size into `1..=MAX_TAIL` (`None` → default).
pub fn clamp_tail(requested: Option<usize>) -> usize {
    requested.unwrap_or(DEFAULT_TAIL).clamp(1, MAX_TAIL)
}

/// The last `n` records (oldest first), spanning the rotated generation
/// when the live file is shorter than `n`. Lines that fail to parse as
/// JSON are skipped (a torn last line during rotation) — never fatal.
pub fn tail(n: usize) -> Result<Vec<serde_json::Value>, String> {
    let n = n.clamp(1, MAX_TAIL);
    let _g = write_lock().lock().unwrap_or_else(|p| p.into_inner());
    let mut lines: Vec<String> = Vec::new();
    for file in [rotated_path(), path()] {
        if !file.exists() {
            continue;
        }
        let raw = std::fs::read_to_string(&file)
            .map_err(|e| format!("read {}: {e}", file.display()))?;
        lines.extend(raw.lines().filter(|l| !l.trim().is_empty()).map(str::to_string));
    }
    let start = lines.len().saturating_sub(n);
    Ok(lines[start..]
        .iter()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunnel::test_support::with_temp_home;

    fn ev(user: &str, outcome: &str) -> AuditEvent {
        AuditEvent::new("login", user, outcome, "tunnel", "-", "api")
    }

    #[test]
    fn record_appends_json_lines_and_tail_reads_them_oldest_first() {
        with_temp_home(|| {
            try_record(&ev("alice", "ok")).expect("append 1");
            try_record(&ev("bob", "bad_creds")).expect("append 2");
            try_record(&AuditEvent::new(
                "logout",
                "alice",
                "ok",
                "edge:k2-edge-test",
                "203.0.113.9",
                "web",
            ))
            .expect("append 3");
            let all = tail(10).expect("tail");
            assert_eq!(all.len(), 3);
            assert_eq!(all[0]["user"], "alice");
            assert_eq!(all[0]["outcome"], "ok");
            assert_eq!(all[0]["event"], "login");
            assert_eq!(all[1]["user"], "bob");
            assert_eq!(all[2]["ingress"], "edge:k2-edge-test");
            assert_eq!(all[2]["ip"], "203.0.113.9");
            assert_eq!(all[2]["client"], "web");
            assert!(all[0]["ts"].as_str().unwrap().ends_with('Z'));
            let last = tail(1).expect("tail 1");
            assert_eq!(last.len(), 1);
            assert_eq!(last[0]["event"], "logout");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = std::fs::metadata(path()).unwrap().permissions().mode() & 0o777;
                assert_eq!(mode, 0o600, "audit log must be 0600");
            }
        });
    }

    #[test]
    fn user_is_trimmed_capped_and_control_free() {
        let long = format!("  {}\n", "x".repeat(200));
        let u = clean_user(&long);
        assert_eq!(u.len(), MAX_USER_CHARS);
        assert!(!u.contains('\n'));
        assert_eq!(clean_user("  Bob  "), "Bob");
    }

    #[test]
    fn clamp_tail_bounds() {
        assert_eq!(clamp_tail(None), DEFAULT_TAIL);
        assert_eq!(clamp_tail(Some(0)), 1);
        assert_eq!(clamp_tail(Some(7)), 7);
        assert_eq!(clamp_tail(Some(999_999)), MAX_TAIL);
    }

    #[test]
    fn rotates_at_cap_and_tail_spans_generations() {
        with_temp_home(|| {
            try_record(&ev("first", "ok")).expect("append");
            // Inflate the live file past the cap, then append: the live
            // file must rotate to `.1` and the new line lands in a fresh file.
            {
                let mut f = std::fs::OpenOptions::new().append(true).open(path()).unwrap();
                let filler = vec![b'x'; ROTATE_BYTES as usize];
                f.write_all(&filler).unwrap();
                f.write_all(b"\n").unwrap();
            }
            try_record(&ev("second", "ok")).expect("append after cap");
            assert!(rotated_path().exists(), "rotated generation must exist");
            let live = std::fs::read_to_string(path()).unwrap();
            assert_eq!(live.lines().count(), 1, "live file starts fresh");
            assert!(live.contains("\"second\""));
            // Tail spans: the rotated file's parseable line + the live line.
            let all = tail(10).expect("tail");
            assert_eq!(all.len(), 2, "filler line is skipped; both records seen");
            assert_eq!(all[0]["user"], "first");
            assert_eq!(all[1]["user"], "second");
        });
    }
}
