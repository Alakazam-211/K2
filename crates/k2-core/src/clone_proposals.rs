//! Agent-proposed clones, held for a person (`clone_proposals`, 0181;
//! prd-clone-carries-threads-and-vault-v1 §4.4, §5.2, CL21).
//!
//! Agents never start a clone (Rosson R7: clone stays a person action).
//! An agent prepares one for its **own** workspace; a person approves it
//! (Owner/Admin, fresh vault confirm `clone-approve` bound to the id) and
//! runs it under their own credential and clone gate, or denies it. The
//! proposer may withdraw an open proposal. Open proposals expire after
//! seven days.
//!
//! ```text
//! open ──approve──▶ approved ──finish──▶ done | failed
//!   ├──deny──▶ denied
//!   ├──withdraw──▶ withdrawn
//!   └──7 days──▶ expired
//! ```
//!
//! [`CloneOptions`] is what travels in `options_json`: names and choices
//! only. It is rebuilt from validated fields, so nothing else (and never a
//! value) can be stored.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const STATUS_OPEN: &str = "open";
pub const STATUS_APPROVED: &str = "approved";
pub const STATUS_DENIED: &str = "denied";
pub const STATUS_WITHDRAWN: &str = "withdrawn";
pub const STATUS_EXPIRED: &str = "expired";
pub const STATUS_DONE: &str = "done";
pub const STATUS_FAILED: &str = "failed";

pub const STATUSES: &[&str] =
    &[STATUS_OPEN, STATUS_APPROVED, STATUS_DENIED, STATUS_WITHDRAWN, STATUS_EXPIRED, STATUS_DONE, STATUS_FAILED];

/// Seven days.
pub const TTL_SECS: i64 = 7 * 24 * 3600;

/// The longest `why` kept (the rest is cut).
pub const MAX_WHY: usize = 500;

/// Per-secret choice (§6.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SecretChoice {
    Copy,
    Blank,
    Skip,
}

impl SecretChoice {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "copy" => Some(Self::Copy),
            "blank" | "leave-blank" | "leave_blank" => Some(Self::Blank),
            "skip" => Some(Self::Skip),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Copy => "copy",
            Self::Blank => "blank",
            Self::Skip => "skip",
        }
    }
}

/// What the clone carries (names only, never a value).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CloneOptions {
    /// Bring the Thread (D3: default on).
    #[serde(default = "yes")]
    pub threads: bool,
    /// Only the last N days of Thread (None = all).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since_days: Option<u32>,
    /// NAME → choice. Names absent here take the per-kind default (§6.2).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub secrets: BTreeMap<String, SecretChoice>,
    /// Carry every chat transcript (the dialog's "Include all chat history").
    #[serde(default = "yes")]
    pub all_history: bool,
    /// Destination parent folder on the target (absent = the default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dest: Option<String>,
}

fn yes() -> bool {
    true
}

/// A secret name as the vault names them: `[A-Za-z_][A-Za-z0-9_.-]{0,127}`.
pub fn valid_secret_name(n: &str) -> bool {
    let b = n.as_bytes();
    !b.is_empty()
        && b.len() <= 128
        && (b[0].is_ascii_alphabetic() || b[0] == b'_')
        && b.iter().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))
}

impl CloneOptions {
    /// Validate and normalize. Errors name the bad input (never echo more).
    pub fn validated(mut self) -> Result<Self, String> {
        if let Some(d) = self.since_days {
            if d == 0 || d > 3650 {
                return Err("sinceDays is 1–3650".into());
            }
        }
        if self.secrets.len() > 200 {
            return Err("at most 200 secret choices".into());
        }
        for name in self.secrets.keys() {
            if !valid_secret_name(name) {
                return Err(format!("not a secret name: '{}'", name.chars().take(40).collect::<String>()));
            }
        }
        if let Some(dest) = self.dest.as_deref().map(str::trim) {
            if dest.is_empty() {
                self.dest = None;
            } else if !dest.starts_with('/') && !dest.starts_with('~') {
                return Err("dest is an absolute folder on the target".into());
            }
        }
        if !self.threads {
            self.since_days = None;
        }
        Ok(self)
    }

    /// Plain words for the Thread notice and the CLI.
    pub fn summary(&self) -> String {
        let copy = self.secrets.values().filter(|c| **c == SecretChoice::Copy).count();
        let mut parts = vec![if self.threads {
            match self.since_days {
                Some(d) => format!("with the last {d} days of Thread"),
                None => "with Thread".to_string(),
            }
        } else {
            "without Thread".to_string()
        }];
        match copy {
            0 => parts.push("no secrets copied".into()),
            1 => parts.push("1 secret copied".into()),
            n => parts.push(format!("{n} secrets copied")),
        }
        parts.join(", ")
    }
}

/// `here` (a second copy on this server) or a host name.
pub fn valid_target(t: &str) -> bool {
    let t = t.trim();
    t == "here"
        || (!t.is_empty()
            && t.len() <= 253
            && t.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b':'))
            && !t.starts_with('-'))
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Proposal {
    pub id: String,
    pub project_id: String,
    pub proposed_by: String,
    pub target: String,
    pub options: CloneOptions,
    pub why: Option<String>,
    pub status: String,
    pub decided_by: Option<String>,
    pub decided_at: Option<i64>,
    pub created_at: i64,
    pub expires_at: i64,
}

pub struct NewProposal<'a> {
    pub project_id: &'a str,
    pub proposed_by: &'a str,
    pub target: &'a str,
    pub options: CloneOptions,
    pub why: Option<&'a str>,
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Proposal> {
    let raw: String = r.get("options_json")?;
    let options = serde_json::from_str(&raw).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;
    Ok(Proposal {
        id: r.get("id")?,
        project_id: r.get("project_id")?,
        proposed_by: r.get("proposed_by")?,
        target: r.get("target")?,
        options,
        why: r.get("why")?,
        status: r.get("status")?,
        decided_by: r.get("decided_by")?,
        decided_at: r.get("decided_at")?,
        created_at: r.get("created_at")?,
        expires_at: r.get("expires_at")?,
    })
}

const COLS: &str =
    "id, project_id, proposed_by, target, options_json, why, status, decided_by, decided_at, created_at, expires_at";

/// Insert an open proposal (`cp_<uuid>`), expiring `TTL_SECS` from `now`.
pub fn create(conn: &Connection, p: NewProposal<'_>, now: i64) -> Result<Proposal, String> {
    if !valid_target(p.target) {
        return Err("target is a host name or 'here'".into());
    }
    let options = p.options.validated()?;
    let id = format!("cp_{}", uuid::Uuid::new_v4().simple());
    let why = p.why.map(str::trim).filter(|w| !w.is_empty()).map(|w| w.chars().take(MAX_WHY).collect::<String>());
    let opts = serde_json::to_string(&options).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO clone_proposals (id, project_id, proposed_by, target, options_json, why, status, created_at, expires_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'open', ?7, ?8)",
        params![id, p.project_id, p.proposed_by, p.target.trim(), opts, why, now, now + TTL_SECS],
    )
    .map_err(|e| e.to_string())?;
    get(conn, &id).map_err(|e| e.to_string())?.ok_or_else(|| "proposal vanished".to_string())
}

pub fn get(conn: &Connection, id: &str) -> rusqlite::Result<Option<Proposal>> {
    conn.query_row(&format!("SELECT {COLS} FROM clone_proposals WHERE id = ?1"), params![id], row).optional()
}

/// Exact id, or a unique prefix of at least 6 characters.
pub fn find(conn: &Connection, token: &str) -> rusqlite::Result<Option<Proposal>> {
    let t = token.trim();
    if let Some(p) = get(conn, t)? {
        return Ok(Some(p));
    }
    if t.len() < 6 || !t.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') {
        return Ok(None);
    }
    let mut st = conn.prepare(&format!("SELECT {COLS} FROM clone_proposals WHERE id LIKE ?1 || '%' LIMIT 2"))?;
    let rows: Vec<Proposal> = st.query_map(params![t], row)?.collect::<rusqlite::Result<_>>()?;
    Ok(if rows.len() == 1 { rows.into_iter().next() } else { None })
}

/// Newest first. `project_id` narrows to one workspace; `status` to one
/// state (None = every state).
pub fn list(conn: &Connection, project_id: Option<&str>, status: Option<&str>, limit: u32) -> rusqlite::Result<Vec<Proposal>> {
    let mut st = conn.prepare(&format!(
        "SELECT {COLS} FROM clone_proposals WHERE (?1 IS NULL OR project_id = ?1) AND (?2 IS NULL OR status = ?2) \
         ORDER BY created_at DESC, id DESC LIMIT ?3"
    ))?;
    let rows = st.query_map(params![project_id, status, limit], row)?.collect();
    rows
}

/// Close an open proposal (`approved`, `denied`, `withdrawn`). Refusals:
/// `not_found`, `expired` (it was due; it is marked expired now),
/// `not_open`.
pub fn decide(conn: &Connection, id: &str, status: &str, by: &str, now: i64) -> Result<Proposal, &'static str> {
    if ![STATUS_APPROVED, STATUS_DENIED, STATUS_WITHDRAWN].contains(&status) {
        return Err("bad_status");
    }
    let p = get(conn, id).map_err(|_| "db")?.ok_or("not_found")?;
    if p.status != STATUS_OPEN {
        return Err("not_open");
    }
    if p.expires_at <= now {
        let _ = conn.execute(
            "UPDATE clone_proposals SET status = 'expired', decided_at = ?2 WHERE id = ?1 AND status = 'open'",
            params![id, now],
        );
        return Err("expired");
    }
    let n = conn
        .execute(
            "UPDATE clone_proposals SET status = ?2, decided_by = ?3, decided_at = ?4 WHERE id = ?1 AND status = 'open'",
            params![id, status, by, now],
        )
        .map_err(|_| "db")?;
    if n != 1 {
        return Err("not_open");
    }
    get(conn, id).map_err(|_| "db")?.ok_or("not_found")
}

/// Record how an approved proposal's clone went (`done` / `failed`).
pub fn finish(conn: &Connection, id: &str, ok: bool) -> Result<Proposal, &'static str> {
    let status = if ok { STATUS_DONE } else { STATUS_FAILED };
    let n = conn
        .execute("UPDATE clone_proposals SET status = ?2 WHERE id = ?1 AND status = 'approved'", params![id, status])
        .map_err(|_| "db")?;
    if n != 1 {
        return Err(if get(conn, id).map_err(|_| "db")?.is_some() { "not_approved" } else { "not_found" });
    }
    get(conn, id).map_err(|_| "db")?.ok_or("not_found")
}

/// Mark due open proposals expired; returns them (to tell the proposers).
pub fn expire_due(conn: &Connection, now: i64) -> rusqlite::Result<Vec<Proposal>> {
    let due: Vec<Proposal> = {
        let mut st = conn.prepare(&format!(
            "SELECT {COLS} FROM clone_proposals WHERE status = 'open' AND expires_at <= ?1"
        ))?;
        let rows = st.query_map(params![now], row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    for p in &due {
        conn.execute(
            "UPDATE clone_proposals SET status = 'expired', decided_at = ?2 WHERE id = ?1 AND status = 'open'",
            params![p.id, now],
        )?;
    }
    Ok(due)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn_with_project() -> (Connection, String) {
        let conn = crate::db::isolated_test_connection();
        conn.execute_batch("PRAGMA foreign_keys = ON;").expect("fk");
        let id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO projects (id, path, name) VALUES (?1, ?2, 'sales')",
            params![id, format!("/tmp/cc-{id}")],
        )
        .expect("project");
        (conn, id)
    }

    fn opts(copy: &[&str]) -> CloneOptions {
        let mut o = CloneOptions { threads: true, all_history: true, ..Default::default() };
        for n in copy {
            o.secrets.insert((*n).to_string(), SecretChoice::Copy);
        }
        o
    }

    #[test]
    fn create_list_decide_finish_round_trip() {
        let (conn, pid) = conn_with_project();
        let p = create(
            &conn,
            NewProposal { project_id: &pid, proposed_by: "agent:sales", target: "luzz.k2.dev", options: opts(&["STRIPE_KEY"]), why: Some("move") },
            1000,
        )
        .expect("create");
        assert!(p.id.starts_with("cp_"), "{}", p.id);
        assert_eq!(p.status, STATUS_OPEN);
        assert_eq!(p.expires_at, 1000 + TTL_SECS);
        assert_eq!(p.options.secrets.get("STRIPE_KEY"), Some(&SecretChoice::Copy));
        assert_eq!(list(&conn, Some(&pid), Some(STATUS_OPEN), 10).expect("list").len(), 1);
        assert_eq!(find(&conn, &p.id[..10]).expect("find").expect("prefix").id, p.id);
        let a = decide(&conn, &p.id, STATUS_APPROVED, "owner", 2000).expect("approve");
        assert_eq!(a.status, STATUS_APPROVED);
        assert_eq!(a.decided_by.as_deref(), Some("owner"));
        assert_eq!(decide(&conn, &p.id, STATUS_DENIED, "owner", 2001), Err("not_open"));
        assert_eq!(finish(&conn, &p.id, true).expect("finish").status, STATUS_DONE);
        assert_eq!(finish(&conn, &p.id, true), Err("not_approved"));
    }

    #[test]
    fn open_proposals_expire_after_seven_days() {
        let (conn, pid) = conn_with_project();
        let p = create(&conn, NewProposal { project_id: &pid, proposed_by: "a", target: "here", options: opts(&[]), why: None }, 0)
            .expect("create");
        assert_eq!(decide(&conn, &p.id, STATUS_APPROVED, "owner", TTL_SECS), Err("expired"));
        assert_eq!(get(&conn, &p.id).expect("get").expect("row").status, STATUS_EXPIRED);
        let q = create(&conn, NewProposal { project_id: &pid, proposed_by: "a", target: "here", options: opts(&[]), why: None }, 0)
            .expect("create");
        let gone = expire_due(&conn, TTL_SECS + 1).expect("sweep");
        assert_eq!(gone.iter().map(|g| g.id.as_str()).collect::<Vec<_>>(), vec![q.id.as_str()]);
    }

    #[test]
    fn options_hold_names_only_and_refuse_bad_input() {
        assert!(CloneOptions { since_days: Some(0), ..opts(&[]) }.validated().is_err());
        let mut bad = opts(&[]);
        bad.secrets.insert("sk_live=abc def".into(), SecretChoice::Copy);
        assert!(bad.validated().is_err());
        assert!(CloneOptions { dest: Some("relative/dir".into()), ..opts(&[]) }.validated().is_err());
        assert!(!valid_target("bad host"));
        assert!(valid_target("luzz.k2.dev"));
        assert!(valid_target("here"));
        // Unknown keys in stored JSON are dropped on the next read: the
        // options struct is the only shape that is ever written.
        let v: CloneOptions = serde_json::from_str(r#"{"threads":false,"value":"sk_live_x"}"#).expect("parse");
        assert!(!serde_json::to_string(&v).expect("ser").contains("sk_live_x"));
        assert_eq!(opts(&["A", "B"]).summary(), "with Thread, 2 secrets copied");
    }

    #[test]
    fn proposals_go_with_their_workspace() {
        let (conn, pid) = conn_with_project();
        create(&conn, NewProposal { project_id: &pid, proposed_by: "a", target: "here", options: opts(&[]), why: None }, 0)
            .expect("create");
        conn.execute("DELETE FROM projects WHERE id = ?1", params![pid]).expect("delete");
        assert!(list(&conn, None, None, 10).expect("list").is_empty());
    }
}
