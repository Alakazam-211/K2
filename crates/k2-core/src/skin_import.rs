//! App users brought over from another user store (prd-app-supabase-compat-v1
//! §9.2, S2): `k2 app user import <app> --file users.jsonl`.
//!
//! - **The given id is kept** as the principal id. A moved App's rows point
//!   at its users by id (foreign keys to `auth.users(id)`), so an import that
//!   minted fresh ids would orphan every row.
//! - **bcrypt hashes are accepted as given** (`$2a$`, `$2b$`, `$2y$`; what
//!   Supabase's auth server writes). At sign-in [`verify_legacy`] checks one
//!   with the `bcrypt` crate and, on success, [`upgrade_after_login`] replaces
//!   it with Argon2 (compare-and-set: a password changed meanwhile wins). An
//!   Argon2 PHC string is accepted too.
//! - The person chose that password themselves, so an imported login is
//!   never "must change".
//! - **Dry run by default.** It reports counts and every clash (an id or
//!   email already in use by someone else, a bad line). `apply` writes, in
//!   one transaction, and only when there is no clash. Running it again
//!   updates changed rows and adds new ones (the cut-over's users delta).
//!   A K2 password set since the last import (an Argon2 hash) is kept.
//! - `app_metadata` and `user_metadata` are stored on the principal
//!   (additive columns, 16 KiB each, JSON objects). The identity lane
//!   (lane A) reads them into the claims and `auth.users`.
//! - A `banned_until` in the future imports the login with its App grant
//!   turned off.
//!
//! Who: the owner, or the App's own agent with "Let this agent manage its
//! Apps' users". An agent never touches a login it did not make for its own
//! workspace ([`Options::agent_workspace`]); such a row is a clash.

use std::collections::{BTreeMap, HashSet};

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value as J};

use super::{app_grant_shape, hash_password, normalize_email, normalize_username, now_secs, with_conn, HOST_APP_ID, USERNAME_MAX_LEN};

/// The largest `app_metadata` or `user_metadata`, as stored JSON.
pub const METADATA_MAX: usize = 16 * 1024;

/// Refusal code: the file has clashes and `apply` was asked.
pub const CLASHES: &str = "import_clashes";

// ── bcrypt at sign-in ────────────────────────────────────────────────

/// The highest bcrypt cost K2 checks (Supabase writes 10). Each step
/// doubles the work, so a cost of 31 in an imported file would stall every
/// sign-in of that person.
pub const BCRYPT_MAX_COST: u32 = 15;

/// A bcrypt hash as Supabase (and most bcrypt libraries) write it:
/// `$2a$`/`$2b$`/`$2y$`, a two-digit cost (4 to [`BCRYPT_MAX_COST`]), 53
/// characters of salt and hash.
pub fn is_bcrypt(hash: &str) -> bool {
    let h = hash.trim();
    h.len() == 60
        && h.is_ascii()
        && (h.starts_with("$2a$") || h.starts_with("$2b$") || h.starts_with("$2y$"))
        && h.as_bytes()[6] == b'$'
        && h[4..6].parse::<u32>().is_ok_and(|c| (4..=BCRYPT_MAX_COST).contains(&c))
}

/// An Argon2 PHC string (what K2 itself writes).
fn is_argon2(hash: &str) -> bool {
    hash.trim().starts_with("$argon2")
}

/// The password check for a stored hash that isn't Argon2: an imported
/// bcrypt hash. Anything else never verifies.
pub(super) fn verify_legacy(password: &str, hash: &str) -> bool {
    if !is_bcrypt(hash) {
        return false;
    }
    let _slot = super::guard::hash_slot();
    bcrypt::verify(password, hash.trim()).unwrap_or(false)
}

/// After a successful sign-in: a bcrypt hash becomes Argon2 (only if it is
/// still the stored one). Returns the hash the login now stands on, which
/// the sign-in door keeps beside its one-time code: the new one when the
/// swap landed, else the one the password was checked against (so a
/// password changed meanwhile still kills the code).
pub(super) fn upgrade_after_login(username: &str, hash: Option<String>, password: &str) -> Option<String> {
    let old = hash?;
    if !is_bcrypt(&old) {
        return Some(old);
    }
    let new = match hash_password(password) {
        Ok(h) => h,
        Err(e) => {
            crate::log_debug!("[skin] imported password kept as bcrypt for now: {e}");
            return Some(old);
        }
    };
    let swapped = with_conn(|conn| {
        conn.execute(
            "UPDATE principals SET password_hash = ?1 WHERE username = ?2 AND password_hash = ?3",
            params![new, username, old],
        )
        .map_err(|e| format!("skin rehash: {e}"))
    });
    match swapped {
        Ok(1) => Some(new),
        Ok(_) => Some(old),
        Err(e) => {
            crate::log_debug!("[skin] {e}");
            Some(old)
        }
    }
}

// ── the file ─────────────────────────────────────────────────────────

/// One login to import, checked.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportRow {
    /// 1-based line in the file.
    pub line: usize,
    pub id: String,
    pub email: String,
    pub username: Option<String>,
    pub password_hash: Option<String>,
    pub app_metadata: Option<String>,
    pub user_metadata: Option<String>,
    pub created_at: Option<i64>,
    pub banned: bool,
}

/// A line that can't be imported, or a row that clashes.
#[derive(Debug, Clone, PartialEq)]
pub struct Clash {
    pub line: usize,
    pub id: Option<String>,
    pub email: Option<String>,
    pub reason: String,
}

impl Clash {
    fn json(&self) -> J {
        json!({ "line": self.line, "id": self.id, "email": self.email, "reason": self.reason })
    }
}

fn text<'a>(v: &'a J, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| v.get(*k).and_then(J::as_str)).map(str::trim).filter(|s| !s.is_empty())
}

fn metadata(v: &J, keys: &[&str], what: &str) -> Result<Option<String>, String> {
    let Some(m) = keys.iter().find_map(|k| v.get(*k)).filter(|m| !m.is_null()) else {
        return Ok(None);
    };
    if !m.is_object() {
        return Err(format!("{what} must be a JSON object"));
    }
    let s = m.to_string();
    if s.len() > METADATA_MAX {
        return Err(format!("{what} is {} bytes; at most {METADATA_MAX}", s.len()));
    }
    Ok(Some(s))
}

/// A Postgres or ISO 8601 time (`2024-03-01 10:00:00.12+00`,
/// `2024-03-01T10:00:00Z`) or epoch seconds, as epoch seconds.
pub fn parse_time(v: &J) -> Result<Option<i64>, String> {
    match v {
        J::Null => Ok(None),
        J::Number(n) => n.as_i64().map(Some).ok_or_else(|| format!("time {n} is not whole seconds")),
        J::String(s) if s.trim().is_empty() => Ok(None),
        J::String(s) if !s.is_ascii() => Err(format!("time {s:?} is not a timestamp")),
        J::String(s) => {
            let mut t = s.trim().replacen(' ', "T", 1);
            // `+00` / `-05` (Postgres' short offset) → `+00:00`.
            let tail = t.len().saturating_sub(3);
            if t.len() > 10 && (t[tail..].starts_with('+') || t[tail..].starts_with('-')) && t[tail + 1..].bytes().all(|b| b.is_ascii_digit()) {
                t.push_str(":00");
            }
            chrono::DateTime::parse_from_rfc3339(&t)
                .map(|d| Some(d.timestamp()))
                .map_err(|_| format!("time {s:?} is not a timestamp (2024-03-01T10:00:00Z)"))
        }
        other => Err(format!("time {other} is not a timestamp")),
    }
}

/// One JSON line (Supabase's `auth.users` column names, or the short ones)
/// as an [`ImportRow`].
pub fn parse_row(line: usize, v: &J, now: i64) -> Result<ImportRow, Clash> {
    let bad = |id: Option<&str>, email: Option<&str>, reason: String| Clash {
        line,
        id: id.map(str::to_string),
        email: email.map(str::to_string),
        reason,
    };
    if !v.is_object() {
        return Err(bad(None, None, "not a JSON object".into()));
    }
    let raw_id = text(v, &["id"]);
    let raw_email = text(v, &["email"]);
    let id = match raw_id.map(uuid::Uuid::parse_str) {
        Some(Ok(u)) => u.hyphenated().to_string(),
        Some(Err(_)) => return Err(bad(raw_id, raw_email, "id is not a UUID".into())),
        None => return Err(bad(None, raw_email, "no id".into())),
    };
    let email = match raw_email.map(normalize_email) {
        Some(Ok(Some(e))) => e,
        Some(_) => return Err(bad(Some(&id), raw_email, "email is not an email address".into())),
        None => return Err(bad(Some(&id), None, "no email (App users sign in by email or username)".into())),
    };
    let e = |reason: String| bad(Some(&id), Some(&email), reason);
    let username = match text(v, &["username"]) {
        Some(u) => Some(normalize_username(u).map_err(|m| e(m))?),
        None => None,
    };
    let password_hash = match text(v, &["encrypted_password", "password_hash"]) {
        Some(h) if is_bcrypt(h) || is_argon2(h) => Some(h.to_string()),
        Some(_) => return Err(e("encrypted_password is neither bcrypt ($2a$/$2b$/$2y$) nor Argon2".into())),
        None => None,
    };
    let app_metadata = metadata(v, &["raw_app_meta_data", "app_metadata"], "app_metadata").map_err(e)?;
    let user_metadata = metadata(v, &["raw_user_meta_data", "user_metadata"], "user_metadata").map_err(e)?;
    let created_at = parse_time(v.get("created_at").unwrap_or(&J::Null)).map_err(e)?;
    let banned = parse_time(v.get("banned_until").unwrap_or(&J::Null)).map_err(e)?.is_some_and(|t| t > now);
    Ok(ImportRow { line, id, email, username, password_hash, app_metadata, user_metadata, created_at, banned })
}

/// Every line of a JSONL file (blank lines skipped).
pub fn parse_jsonl(text_in: &str, now: i64) -> (Vec<ImportRow>, Vec<Clash>) {
    let (mut rows, mut bad) = (Vec::new(), Vec::new());
    for (i, l) in text_in.lines().enumerate() {
        if l.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<J>(l) {
            Ok(v) => match parse_row(i + 1, &v, now) {
                Ok(r) => rows.push(r),
                Err(c) => bad.push(c),
            },
            Err(err) => bad.push(Clash { line: i + 1, id: None, email: None, reason: format!("not JSON: {err}") }),
        }
    }
    (rows, bad)
}

/// A username from an email's local part: `Ada.Lovelace+x@…` → `ada-lovelace-x`.
pub fn username_from_email(email: &str) -> String {
    let local = email.split('@').next().unwrap_or("");
    let mut out = String::new();
    for c in local.chars() {
        let c = c.to_ascii_lowercase();
        let keep = c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-';
        let c = if keep { c } else { '-' };
        if !(c == '-' && out.ends_with('-')) {
            out.push(c);
        }
    }
    let mut out = out.trim_matches('-').to_string();
    if out.len() < 2 {
        out = format!("user-{out}").trim_end_matches('-').to_string();
    }
    out.truncate(USERNAME_MAX_LEN - 6);
    out
}

// ── the import ───────────────────────────────────────────────────────

/// What the import does and who asks.
#[derive(Debug, Clone, Copy)]
pub struct Options<'a> {
    /// The App's published service id (never `host`).
    pub app: &'a str,
    /// A role (id or name) for new App grants; else the grant has no rooms.
    pub role: Option<&'a str>,
    pub apply: bool,
    /// The agent's workspace when an agent imports: new logins are stamped
    /// and tied to it, and an existing login it didn't make there clashes.
    pub agent_workspace: Option<&'a str>,
    /// The audit label (`owner`, `agent:<handle>`).
    pub actor: &'a str,
    /// Names an agent may not give a login (`creators::name_reserved`),
    /// read before the store is locked.
    pub reserved: &'a [String],
}

/// The principal columns the identity lane reads (additive; a duplicate
/// ALTER is ignored, as every `principals` column before it).
pub(super) fn ensure_metadata_columns(conn: &Connection) {
    let _ = conn.execute("ALTER TABLE principals ADD COLUMN app_metadata TEXT", []);
    let _ = conn.execute("ALTER TABLE principals ADD COLUMN user_metadata TEXT", []);
}

struct Existing {
    username: String,
    email: Option<String>,
    password_hash: Option<String>,
    app_metadata: Option<String>,
    user_metadata: Option<String>,
}

fn existing_by_id(conn: &Connection, id: &str) -> Result<Option<Existing>, String> {
    conn.query_row(
        "SELECT username, email, password_hash, app_metadata, user_metadata FROM principals WHERE id = ?1",
        params![id],
        |r| Ok(Existing { username: r.get(0)?, email: r.get(1)?, password_hash: r.get(2)?, app_metadata: r.get(3)?, user_metadata: r.get(4)? }),
    )
    .optional()
    .map_err(|e| format!("skin import lookup: {e}"))
}

fn id_of_email(conn: &Connection, email: &str) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT id FROM principals WHERE email = ?1 COLLATE NOCASE AND email IS NOT NULL AND trim(email) != ''",
        params![email],
        |r| r.get(0),
    )
    .optional()
    .map_err(|e| format!("skin import email lookup: {e}"))
}

fn id_of_username(conn: &Connection, username: &str) -> Result<Option<String>, String> {
    conn.query_row("SELECT id FROM principals WHERE username = ?1", params![username], |r| r.get(0))
        .optional()
        .map_err(|e| format!("skin import name lookup: {e}"))
}

/// May an agent of `ws` touch this existing login? Only one it made for
/// its own workspace that holds no box login.
fn agent_may_touch(conn: &Connection, id: &str, ws: &str) -> Result<bool, String> {
    if super::creators::workspace_of_conn(conn, id)?.as_deref() != Some(ws) {
        return Ok(false);
    }
    Ok(!super::ties::holds_host_conn(conn, id)?)
}

/// What one row will do.
enum Step {
    New { username: String, derived: bool },
    Update { changed: bool, keep_k2_password: bool },
}

/// Plan (and with `apply`, write) the import of `rows` into `opts.app`.
/// `Ok` is the report; `Err` is the report of a refused apply (clashes) or
/// a store failure as `{"error": …}`.
pub fn import(rows: &[ImportRow], bad_lines: &[Clash], opts: Options<'_>) -> Result<J, J> {
    let app = opts.app.trim();
    if app.is_empty() || app == HOST_APP_ID {
        return Err(json!({ "error": "import App users into a published App (not the box login)" }));
    }
    let result = with_conn(|conn| {
        ensure_metadata_columns(conn);
        let tx = conn.unchecked_transaction().map_err(|e| format!("skin import begin: {e}"))?;
        let (role_id, scope) = match opts.role {
            Some(token) => app_grant_shape(&tx, app, Some(token), None)?,
            None => (None, Some("[]".to_string())),
        };
        let mut clashes: Vec<Clash> = bad_lines.to_vec();
        let (mut seen_ids, mut seen_emails) = (HashSet::new(), HashSet::new());
        let mut taken_names: HashSet<String> = HashSet::new();
        let mut steps: Vec<(&ImportRow, Step)> = Vec::new();
        for r in rows {
            let clash = |reason: &str| Clash { line: r.line, id: Some(r.id.clone()), email: Some(r.email.clone()), reason: reason.to_string() };
            if !seen_ids.insert(r.id.clone()) {
                clashes.push(clash("the same id is on an earlier line"));
                continue;
            }
            if !seen_emails.insert(r.email.clone()) {
                clashes.push(clash("the same email is on an earlier line"));
                continue;
            }
            if id_of_email(&tx, &r.email)?.is_some_and(|o| o != r.id) {
                clashes.push(clash("email_taken: another login on this box has this email"));
                continue;
            }
            match existing_by_id(&tx, &r.id)? {
                Some(ex) => {
                    if let Some(ws) = opts.agent_workspace {
                        if !agent_may_touch(&tx, &r.id, ws)? {
                            clashes.push(clash("outside_workspace: a login with this id exists and your workspace didn't make it (the owner imports it)"));
                            continue;
                        }
                    }
                    if r.username.as_deref().is_some_and(|u| u != ex.username) {
                        clashes.push(clash("username_differs: this id is already a login with another username"));
                        continue;
                    }
                    taken_names.insert(ex.username.clone());
                    let keep_k2_password = ex.password_hash.as_deref().is_some_and(is_argon2) && r.password_hash.as_deref() != ex.password_hash.as_deref();
                    let hash_changes = !keep_k2_password && r.password_hash.is_some() && r.password_hash != ex.password_hash;
                    let changed = hash_changes
                        || ex.email.as_deref() != Some(r.email.as_str())
                        || (r.app_metadata.is_some() && r.app_metadata != ex.app_metadata)
                        || (r.user_metadata.is_some() && r.user_metadata != ex.user_metadata);
                    steps.push((r, Step::Update { changed, keep_k2_password }));
                }
                None => {
                    let (base, derived) = match &r.username {
                        Some(u) => (u.clone(), false),
                        None => (username_from_email(&r.email), true),
                    };
                    let reserved = |n: &str| opts.agent_workspace.is_some() && opts.reserved.iter().any(|x| x.eq_ignore_ascii_case(n));
                    let free = |n: &str, taken: &HashSet<String>| -> Result<bool, String> {
                        Ok(!taken.contains(n) && !reserved(n) && id_of_username(&tx, n)?.is_none())
                    };
                    let name = if free(&base, &taken_names)? {
                        base
                    } else if !derived {
                        clashes.push(clash(if reserved(&base) { "login_name_reserved: an agent can't give a login this name" } else { "username_taken: another login on this box has this username" }));
                        continue;
                    } else {
                        let mut n = 2;
                        loop {
                            let c = format!("{base}-{n}");
                            if free(&c, &taken_names)? {
                                break c;
                            }
                            n += 1;
                        }
                    };
                    taken_names.insert(name.clone());
                    steps.push((r, Step::New { username: name, derived }));
                }
            }
        }
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        let mut derived_names = Vec::new();
        for (r, s) in &steps {
            match s {
                Step::New { username, derived } => {
                    *counts.entry("new").or_default() += 1;
                    if *derived {
                        derived_names.push(json!({ "id": r.id, "username": username }));
                    }
                }
                Step::Update { changed: true, .. } => *counts.entry("updated").or_default() += 1,
                Step::Update { changed: false, .. } => *counts.entry("unchanged").or_default() += 1,
            }
            if let Step::Update { keep_k2_password: true, .. } = s {
                *counts.entry("keptK2Password").or_default() += 1;
            }
            if r.password_hash.is_none() {
                *counts.entry("passwordless").or_default() += 1;
            }
            if r.banned {
                *counts.entry("disabled").or_default() += 1;
            }
        }
        let n = |k: &str| counts.get(k).copied().unwrap_or(0);
        let mut report = json!({
            "app": app,
            "apply": opts.apply,
            "total": rows.len() + bad_lines.len(),
            "new": n("new"),
            "updated": n("updated"),
            "unchanged": n("unchanged"),
            "keptK2Password": n("keptK2Password"),
            "passwordless": n("passwordless"),
            "disabled": n("disabled"),
            "derivedUsernames": derived_names,
            "clashes": clashes.iter().map(Clash::json).collect::<Vec<_>>(),
            "written": false,
        });
        if !opts.apply {
            return Ok(Ok(report));
        }
        if !clashes.is_empty() {
            report["error"] = json!({
                "code": CLASHES,
                "hint": format!("{} line(s) clash: nothing was written. Fix them (or remove those lines) and run again", clashes.len()),
            });
            return Ok(Err(report));
        }
        let now = now_secs();
        for (r, s) in &steps {
            write_one(&tx, r, s, app, role_id.as_deref(), scope.as_deref(), now, opts)?;
        }
        tx.commit().map_err(|e| format!("skin import commit: {e}"))?;
        report["written"] = json!(true);
        Ok(Ok(report))
    });
    match result {
        Ok(Ok(report)) => {
            if report["written"] == json!(true) {
                crate::workspace::context_layers::refresh_skin_roster_after_people_change();
                crate::tool_hook::kick_reconcile();
                super::members::changed(None);
            }
            Ok(report)
        }
        Ok(Err(report)) => Err(report),
        Err(e) => Err(json!({ "error": e })),
    }
}

#[allow(clippy::too_many_arguments)]
fn write_one(tx: &Connection, r: &ImportRow, s: &Step, app: &str, role_id: Option<&str>, scope: Option<&str>, now: i64, opts: Options<'_>) -> Result<(), String> {
    match s {
        Step::New { username, .. } => {
            tx.execute(
                "INSERT INTO principals (id, username, created_at, email, password_hash, app_metadata, user_metadata)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![r.id, username, r.created_at.unwrap_or(now), r.email, r.password_hash, r.app_metadata, r.user_metadata],
            )
            .map_err(|e| format!("skin import insert (line {}): {e}", r.line))?;
            if let Some(ws) = opts.agent_workspace {
                super::creators::stamp_conn(tx, &r.id, ws)?;
                super::ties::set_conn(tx, &r.id, super::ties::TieWrite { workspace_id: ws, set_by: opts.actor, reason: super::ties::REASON_CREATED })?;
            }
        }
        Step::Update { changed: false, .. } => {}
        Step::Update { keep_k2_password, .. } => {
            tx.execute(
                "UPDATE principals SET email = ?2,
                   password_hash = CASE WHEN ?3 IS NULL OR ?4 THEN password_hash ELSE ?3 END,
                   app_metadata = coalesce(?5, app_metadata), user_metadata = coalesce(?6, user_metadata)
                 WHERE id = ?1",
                params![r.id, r.email, r.password_hash, *keep_k2_password, r.app_metadata, r.user_metadata],
            )
            .map_err(|e| format!("skin import update (line {}): {e}", r.line))?;
        }
    }
    // The App grant: added when missing; a ban turns it off. An existing
    // grant's role and rooms are left as the owner set them.
    tx.execute(
        "INSERT INTO grants (id, subject_kind, subject_id, kind, target_id, role_id, scope, created_at, enabled, created_by)
         VALUES (?1, 'principal', ?2, 'app', ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(subject_kind, subject_id, kind, target_id) DO NOTHING",
        params![uuid::Uuid::new_v4().to_string(), r.id, app, role_id, scope, now, !r.banned, opts.actor],
    )
    .map_err(|e| format!("skin import grant (line {}): {e}", r.line))?;
    if r.banned {
        tx.execute(
            "UPDATE grants SET enabled = 0 WHERE subject_kind = 'principal' AND subject_id = ?1 AND kind = 'app' AND target_id = ?2",
            params![r.id, app],
        )
        .map_err(|e| format!("skin import ban (line {}): {e}", r.line))?;
    }
    Ok(())
}

/// The stored metadata of one login (`None` when unset), for the identity
/// lane and tests.
pub fn metadata_of(principal_id: &str) -> Result<(Option<String>, Option<String>), String> {
    with_conn(|conn| {
        ensure_metadata_columns(conn);
        conn.query_row("SELECT app_metadata, user_metadata FROM principals WHERE id = ?1", params![principal_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()
            .map_err(|e| format!("skin metadata lookup: {e}"))
            .map(Option::unwrap_or_default)
    })
}

/// The stored password hash of one login (tests: was it rehashed?).
#[doc(hidden)]
pub fn stored_hash_for_tests(username: &str) -> Option<String> {
    with_conn(|conn| {
        conn.query_row("SELECT password_hash FROM principals WHERE username = ?1", params![username], |r| r.get(0))
            .optional()
            .map_err(|e| e.to_string())
            .map(Option::flatten)
    })
    .ok()
    .flatten()
}

/// A `$2a$` hash (cost 4) of `password`, as Supabase writes them (tests of
/// the import, here and in the daemon).
#[doc(hidden)]
pub fn bcrypt_for_tests(password: &str) -> String {
    bcrypt::hash_with_result(password, 4).map(|h| h.format_for_version(bcrypt::Version::TwoA)).unwrap_or_default()
}

#[cfg(test)]
#[path = "skin_import_tests.rs"]
mod tests;
