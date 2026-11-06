//! App client logs (migration 0269, 0.46.9, feature `app-client-logs-v1`).
//!
//! A signed-in App user's device sends a small log bundle with their own
//! session pass (`diagnostics.send`, `POST /cli/skin/client-logs`): a
//! panic message, the last lines of its log ring buffer, the app version
//! and the OS. No third-party crash SDK: the bundle stays on the App's own
//! server. The daemon caps, rate-limits and redacts it before it lands
//! here ([`clean_bundle`]); the App's owner and its agent read it with
//! `k2 app logs <app> --client`.
//!
//! Rows go after the App's retention ([`DEFAULT_KEEP_DAYS`] unless the
//! owner sets 1 to [`MAX_KEEP_DAYS`]), past [`MAX_ROWS_PER_APP`] (oldest
//! first), and with the person's account deletion ([`forget_person`]).

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

pub const DEFAULT_KEEP_DAYS: i64 = 14;
pub const MIN_KEEP_DAYS: i64 = 1;
pub const MAX_KEEP_DAYS: i64 = 90;
/// Rows kept per App; a new bundle past it drops the oldest.
pub const MAX_ROWS_PER_APP: i64 = 2_000;
/// The request body a bundle fits in (JSON escaping included).
pub const MAX_BODY: usize = 128 * 1024;
/// Text kept per bundle (message plus lines); older lines go first.
pub const MAX_TEXT_BYTES: usize = 64 * 1024;
/// The panic or error message.
pub const MAX_MESSAGE_CHARS: usize = 4_000;
/// Lines per bundle (the newest are kept).
pub const MAX_LINES: usize = 1_000;
/// One line (longer ones are cut).
pub const MAX_LINE_CHARS: usize = 1_000;
/// appVersion, os, device.
pub const MAX_FIELD_CHARS: usize = 64;
pub const KIND_CRASH: &str = "crash";
pub const KIND_LOG: &str = "log";
/// What replaces a secret-looking value.
pub const REDACTED: &str = "***";

/// One stored bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bundle {
    pub id: String,
    pub app: String,
    pub username: String,
    pub received_at: i64,
    pub client_at: Option<i64>,
    pub kind: String,
    pub app_version: String,
    pub os: String,
    pub device: Option<String>,
    pub message: Option<String>,
    pub lines: Vec<String>,
    pub bytes: i64,
    /// How many values the server replaced with `***`.
    pub redacted: i64,
    /// Lines or text were cut to fit.
    pub truncated: bool,
}

/// A bundle as a device sent it, before [`clean_bundle`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Incoming {
    pub kind: String,
    pub app_version: String,
    pub os: String,
    pub device: Option<String>,
    pub message: Option<String>,
    pub lines: Vec<String>,
    pub client_at: Option<i64>,
}

/// What [`clean_bundle`] made of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clean {
    pub kind: String,
    pub app_version: String,
    pub os: String,
    pub device: Option<String>,
    pub message: Option<String>,
    pub lines: Vec<String>,
    pub client_at: Option<i64>,
    pub bytes: usize,
    pub redacted: usize,
    pub truncated: bool,
}

fn cut(s: &str, max: usize) -> (String, bool) {
    if s.chars().count() <= max {
        return (s.to_string(), false);
    }
    (s.chars().take(max).collect(), true)
}

/// Control characters (but tab) become spaces: a line never moves a
/// terminal's cursor or colors when an agent reads it.
fn printable(s: &str) -> String {
    s.chars().map(|c| if c.is_control() && c != '\t' { ' ' } else { c }).collect()
}

fn field(s: &str) -> String {
    let t = printable(s.trim());
    cut(t.trim(), MAX_FIELD_CHARS).0
}

/// One line with secrets replaced. Returns the line and how many values
/// went. The free-text redactor of K2's activity lines (Bearer values,
/// `key=value` secrets, known token prefixes like `k2skn_`, URL user info
/// and query strings, long random-looking runs, PEM blocks), then any
/// value the vault holds.
pub fn redact_line(line: &str) -> (String, usize) {
    let once = crate::agent_hooks::tool_line::redact(line);
    let twice = crate::redaction::redact(&once);
    let n = twice.matches(REDACTED).count().saturating_sub(line.matches(REDACTED).count())
        + twice.matches("[k2v:").count().saturating_sub(line.matches("[k2v:").count());
    (twice, n)
}

/// Cap, clean and redact a bundle. `Err` names the field a device got
/// wrong (a usage refusal).
pub fn clean_bundle(b: Incoming) -> Result<Clean, String> {
    let kind = match b.kind.trim() {
        "" | KIND_LOG => KIND_LOG,
        KIND_CRASH => KIND_CRASH,
        _ => return Err("kind is crash or log".to_string()),
    };
    let app_version = field(&b.app_version);
    let os = field(&b.os);
    if app_version.is_empty() || os.is_empty() {
        return Err("send appVersion and os (each up to 64 characters)".to_string());
    }
    let device = b.device.as_deref().map(field).filter(|d| !d.is_empty());
    let mut redacted = 0usize;
    let mut truncated = false;
    let message = match b.message.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
        Some(m) => {
            let (m, cut_it) = cut(&printable(m), MAX_MESSAGE_CHARS);
            truncated |= cut_it;
            // A panic message can span lines: redact each.
            let mut out = Vec::new();
            for l in m.split('\n') {
                let (r, n) = redact_line(l);
                redacted += n;
                out.push(r);
            }
            Some(out.join("\n"))
        }
        None => None,
    };
    if message.is_none() && b.lines.iter().all(|l| l.trim().is_empty()) {
        return Err("send a message or some lines".to_string());
    }
    let mut lines: Vec<String> = b.lines;
    if lines.len() > MAX_LINES {
        lines.drain(..lines.len() - MAX_LINES);
        truncated = true;
    }
    let mut out = Vec::with_capacity(lines.len());
    for l in &lines {
        let (l, cut_it) = cut(&printable(l), MAX_LINE_CHARS);
        truncated |= cut_it;
        let (r, n) = redact_line(&l);
        redacted += n;
        out.push(r);
    }
    // The newest lines win: drop the oldest until the text fits.
    let msg_len = message.as_deref().map_or(0, str::len);
    let mut total: usize = msg_len + out.iter().map(String::len).sum::<usize>();
    let mut drop = 0;
    while total > MAX_TEXT_BYTES && drop < out.len() {
        total -= out[drop].len();
        drop += 1;
    }
    if drop > 0 {
        out.drain(..drop);
        truncated = true;
    }
    Ok(Clean {
        kind: kind.to_string(),
        app_version,
        os,
        device,
        message,
        lines: out,
        client_at: b.client_at.filter(|t| *t > 0),
        bytes: total,
        redacted,
        truncated,
    })
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Store one cleaned bundle; past [`MAX_ROWS_PER_APP`] the App's oldest go.
/// Returns its id.
pub fn insert(conn: &Connection, app: &str, principal_id: &str, username: &str, c: &Clean) -> rusqlite::Result<String> {
    let id = format!("cl_{}", &uuid::Uuid::new_v4().simple().to_string()[..20]);
    let lines = serde_json::to_string(&c.lines).unwrap_or_else(|_| "[]".to_string());
    conn.execute(
        "INSERT INTO app_client_logs (id, app_id, principal_id, username, received_at, client_at, kind, app_version, os, device, message, lines, bytes, redacted, truncated)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        params![
            id,
            app,
            principal_id,
            username,
            now_secs(),
            c.client_at,
            c.kind,
            c.app_version,
            c.os,
            c.device,
            c.message,
            lines,
            c.bytes as i64,
            c.redacted as i64,
            c.truncated as i64
        ],
    )?;
    conn.execute(
        "DELETE FROM app_client_logs WHERE app_id = ?1 AND id IN (
            SELECT id FROM app_client_logs WHERE app_id = ?1 ORDER BY received_at DESC, id DESC LIMIT -1 OFFSET ?2)",
        params![app, MAX_ROWS_PER_APP],
    )?;
    Ok(id)
}

/// Filters for [`list`].
#[derive(Debug, Clone, Default)]
pub struct Query {
    pub username: Option<String>,
    pub kind: Option<String>,
    /// Only bundles received at or after this time.
    pub since: Option<i64>,
    pub limit: usize,
}

/// One App's bundles, newest first.
pub fn list(conn: &Connection, app: &str, q: &Query) -> rusqlite::Result<Vec<Bundle>> {
    let limit = q.limit.clamp(1, 500) as i64;
    let mut st = conn.prepare(
        "SELECT id, app_id, username, received_at, client_at, kind, app_version, os, device, message, lines, bytes, redacted, truncated
           FROM app_client_logs
          WHERE app_id = ?1 AND (?2 IS NULL OR username = ?2 COLLATE NOCASE) AND (?3 IS NULL OR kind = ?3) AND received_at >= ?4
          ORDER BY received_at DESC, id DESC LIMIT ?5",
    )?;
    let rows = st.query_map(params![app, q.username, q.kind, q.since.unwrap_or(0), limit], |r| {
        let lines: String = r.get(10)?;
        Ok(Bundle {
            id: r.get(0)?,
            app: r.get(1)?,
            username: r.get(2)?,
            received_at: r.get(3)?,
            client_at: r.get(4)?,
            kind: r.get(5)?,
            app_version: r.get(6)?,
            os: r.get(7)?,
            device: r.get(8)?,
            message: r.get(9)?,
            lines: serde_json::from_str(&lines).unwrap_or_default(),
            bytes: r.get(11)?,
            redacted: r.get(12)?,
            truncated: r.get::<_, i64>(13)? != 0,
        })
    })?;
    rows.collect()
}

/// A person's bundles in one App go (their account deletion). Returns how many.
pub fn forget_person(conn: &Connection, app: &str, principal_id: &str) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM app_client_logs WHERE app_id = ?1 AND principal_id = ?2", params![app, principal_id])
}

/// Every App's bundles older than its retention go. Returns how many.
pub fn sweep(conn: &Connection, now: i64) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM app_client_logs WHERE received_at < ?1 - 86400 * COALESCE(
            (SELECT keep_days FROM app_client_log_settings s WHERE s.app_id = app_client_logs.app_id), ?2)",
        params![now, DEFAULT_KEEP_DAYS],
    )
}

/// One App's retention in days, and whether someone set it.
pub fn keep_days(conn: &Connection, app: &str) -> rusqlite::Result<(i64, bool)> {
    let v: Option<i64> =
        conn.query_row("SELECT keep_days FROM app_client_log_settings WHERE app_id = ?1", params![app], |r| r.get(0)).optional()?;
    Ok(match v {
        Some(d) => (d, true),
        None => (DEFAULT_KEEP_DAYS, false),
    })
}

/// Set one App's retention (1 to [`MAX_KEEP_DAYS`] days; the caller checks).
pub fn set_keep_days(conn: &Connection, app: &str, days: i64, actor: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO app_client_log_settings (app_id, keep_days, updated_at, updated_by) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(app_id) DO UPDATE SET keep_days = ?2, updated_at = ?3, updated_by = ?4",
        params![app, days, now_secs(), actor],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(include_str!("../drizzle_sql/0269_app_client_logs.sql")).unwrap();
        c
    }

    fn incoming(lines: Vec<String>) -> Incoming {
        Incoming { kind: "crash".into(), app_version: "1.2.3".into(), os: "iOS 27.0".into(), message: Some("panicked at 'boom'".into()), lines, ..Default::default() }
    }

    #[test]
    fn tokens_and_secrets_are_redacted_on_the_server() {
        let c = clean_bundle(incoming(vec![
            "GET /cli/skin/me Authorization: Bearer k2skn_abcdefabcdefabcdefabcdef".into(),
            "login ok pass=k2skn_0123456789abcdef0123456789".into(),
            "password=hunter2-hunter2".into(),
            "plain line".into(),
        ]))
        .unwrap();
        let all = c.lines.join("\n");
        assert!(!all.contains("k2skn_"), "{all}");
        assert!(!all.contains("hunter2"), "{all}");
        assert!(all.contains("plain line"), "{all}");
        assert!(c.redacted >= 3, "{c:?}");
    }

    #[test]
    fn the_newest_lines_win_and_long_lines_are_cut() {
        let many: Vec<String> = (0..1_500).map(|i| format!("line {i}")).collect();
        let c = clean_bundle(incoming(many)).unwrap();
        assert!(c.truncated);
        assert_eq!(c.lines.len(), MAX_LINES);
        assert_eq!(c.lines.last().unwrap(), "line 1499");
        let long = clean_bundle(incoming(vec!["x ".repeat(5_000)])).unwrap();
        assert!(long.truncated && long.lines[0].chars().count() <= MAX_LINE_CHARS);
        let big: Vec<String> = (0..200).map(|i| format!("{i} {}", "word ".repeat(150))).collect();
        let fit = clean_bundle(incoming(big)).unwrap();
        assert!(fit.bytes <= MAX_TEXT_BYTES && fit.truncated, "{}", fit.bytes);
        assert!(fit.lines.last().unwrap().starts_with("199 "), "the newest stays");
    }

    #[test]
    fn a_bundle_needs_a_version_an_os_and_some_text() {
        assert!(clean_bundle(Incoming { app_version: "1".into(), os: "".into(), message: Some("m".into()), ..Default::default() }).is_err());
        assert!(clean_bundle(Incoming { app_version: "1".into(), os: "macOS".into(), ..Default::default() }).is_err());
        assert!(clean_bundle(Incoming { kind: "trace".into(), app_version: "1".into(), os: "x".into(), message: Some("m".into()), ..Default::default() }).is_err());
        let ok = clean_bundle(Incoming { app_version: "1\u{1b}[31m".into(), os: "x".into(), message: Some("m".into()), ..Default::default() }).unwrap();
        assert_eq!(ok.kind, KIND_LOG);
        assert!(!ok.app_version.contains('\u{1b}'), "{:?}", ok.app_version);
    }

    #[test]
    fn retention_the_row_cap_and_a_persons_deletion() {
        let c = conn();
        let b = clean_bundle(incoming(vec!["a".into()])).unwrap();
        let id = insert(&c, "jobs", "p1", "dana", &b).unwrap();
        insert(&c, "jobs", "p2", "eli", &b).unwrap();
        insert(&c, "crm", "p1", "dana", &b).unwrap();
        let rows = list(&c, "jobs", &Query { limit: 50, ..Default::default() }).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().any(|r| r.id == id));
        let dana = list(&c, "jobs", &Query { username: Some("DANA".into()), limit: 50, ..Default::default() }).unwrap();
        assert_eq!(dana.len(), 1);
        assert_eq!(forget_person(&c, "jobs", "p1").unwrap(), 1);
        assert_eq!(list(&c, "crm", &Query { limit: 50, ..Default::default() }).unwrap().len(), 1, "another App's rows stay");
        assert_eq!(keep_days(&c, "jobs").unwrap(), (DEFAULT_KEEP_DAYS, false));
        set_keep_days(&c, "jobs", 2, "owner").unwrap();
        assert_eq!(keep_days(&c, "jobs").unwrap(), (2, true));
        let now = now_secs();
        assert_eq!(sweep(&c, now + 3 * 86_400).unwrap(), 1, "jobs keeps 2 days");
        assert_eq!(sweep(&c, now + 10 * 86_400).unwrap(), 0, "crm keeps the default 14");
        assert_eq!(sweep(&c, now + 15 * 86_400).unwrap(), 1);
        for _ in 0..(MAX_ROWS_PER_APP + 3) {
            insert(&c, "big", "p", "u", &b).unwrap();
        }
        let n: i64 = c.query_row("SELECT COUNT(*) FROM app_client_logs WHERE app_id = 'big'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, MAX_ROWS_PER_APP);
    }
}
