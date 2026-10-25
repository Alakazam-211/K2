//! Agent ties (prd-agent-app-users-v1 §4–§5.2, R2, Rosson 2026-10-24).
//!
//! A **tie** says "an agent of `workspace_id` could know or control this
//! App user's credentials": an agent created them, set their password or
//! email, or sent them an invite; or an App key (which lives in the App's
//! runtime, readable by that workspace's agent) created them or set their
//! password. While a user is tied, everything they sign in to and every
//! live session is held to the **ceiling** ([`clamp`]):
//!
//! - App targets: the box login (`host`) only when `host_ok` (they already
//!   had it when the tie was set), and any App of `workspace_id`;
//! - rooms: only `workspace_id`, minus [`super::PERSON_ONLY_CAPS`];
//! - no chat-only box login and no chat pass (a chat grant is box-wide).
//!
//! The owner may widen a tied user at any time; the extra access just
//! waits (**pending reset**) until that person uses an owner-minted reset
//! link (`owner_reset`), the only thing that drops the tie.
//!
//! skin.db step 6 adds the table (and `app_user_invites`, used by the
//! invite and owner-reset slices): a numbered step, so a K2 older than
//! this refuses the file instead of ignoring the ties and minting
//! uncapped sessions.

use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

use rusqlite::{params, Connection, OptionalExtension};

use super::{now_secs, with_conn, RoomPolicy, SkinGrant, SkinRole, HOST_APP_ID, PERSON_ONLY_CAPS};

pub const REASON_CREATED: &str = "created";
pub const REASON_PASSWORD: &str = "password";
pub const REASON_EMAIL: &str = "email";
pub const REASON_INVITE: &str = "invite";
pub const REASON_APP_KEY: &str = "app_key";
/// An App's own forgot-password link was used (the App's page saw it).
pub const REASON_APP_RESET: &str = "app_reset";
/// The owner marked this login as the workspace's agent (`k2 app user agent`).
pub const REASON_AGENT_LOGIN: &str = "agent_login";
/// Carried over from App caps' first tie table (step 6).
pub const REASON_UPGRADE: &str = "upgrade";

/// Step 6: the ties and the invite / owner-reset links. A file an earlier
/// 0.46.4 build opened may already hold App caps' first `principal_agent_ties`
/// (`principal_id, tied_to, tied_at`) and `principal_person_claims`: their
/// rows move into the one tie table (a claimed guest stays a person) and
/// both old tables go. Guests an agent added before ties existed (a creator
/// stamp, no tie) are tied to that workspace.
pub(super) fn step6_ties_and_invites(conn: &Connection) -> Result<(), String> {
    let old_shape = super::column_exists(conn, "principal_agent_ties", "tied_to")?;
    if old_shape {
        conn.execute_batch("ALTER TABLE principal_agent_ties RENAME TO principal_agent_ties_v1;")
            .map_err(|e| format!("skin.db step 6 (old ties): {e}"))?;
    }
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS principal_agent_ties (
            principal_id TEXT PRIMARY KEY REFERENCES principals(id) ON DELETE CASCADE,
            workspace_id TEXT NOT NULL,
            host_ok      INTEGER NOT NULL DEFAULT 0,
            reasons      TEXT NOT NULL DEFAULT '[]',
            set_by       TEXT NOT NULL,
            set_at       INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS app_user_invites (
            id           TEXT PRIMARY KEY,
            principal_id TEXT NOT NULL REFERENCES principals(id) ON DELETE CASCADE,
            app          TEXT NOT NULL,
            purpose      TEXT NOT NULL CHECK (purpose IN ('invite','reset','owner_reset')),
            token_hash   TEXT NOT NULL UNIQUE,
            sent_to      TEXT,
            set_email    INTEGER NOT NULL DEFAULT 0,
            ttl_secs     INTEGER NOT NULL DEFAULT 0,
            tie_workspace TEXT,
            tie_reason   TEXT,
            outbound_id  TEXT,
            created_by   TEXT NOT NULL,
            created_at   INTEGER NOT NULL,
            expires_at   INTEGER,
            used_at      INTEGER
         );
         CREATE INDEX IF NOT EXISTS app_user_invites_principal ON app_user_invites(principal_id, app);",
    )
    .map_err(|e| format!("skin.db step 6: {e}"))?;
    let claims = table_exists(conn, "principal_person_claims")?;
    let unclaimed = if claims { "AND x.principal_id NOT IN (SELECT principal_id FROM principal_person_claims)" } else { "" };
    if old_shape {
        conn.execute(
            &format!(
                "INSERT OR IGNORE INTO principal_agent_ties (principal_id, workspace_id, host_ok, reasons, set_by, set_at)
                 SELECT x.principal_id, x.tied_to, 0, '[\"{REASON_UPGRADE}\"]', 'upgrade:0.46.4', x.tied_at
                   FROM principal_agent_ties_v1 x WHERE 1 = 1 {unclaimed}"
            ),
            [],
        )
        .map_err(|e| format!("skin.db step 6 (move old ties): {e}"))?;
        conn.execute_batch("DROP TABLE principal_agent_ties_v1;").map_err(|e| format!("skin.db step 6: {e}"))?;
    }
    // Agent-added guests from before ties: tied to the adding workspace,
    // keeping the box login they were given then.
    if super::table_exists_conn(conn, "principal_agent_creators")? {
        conn.execute(
            &format!(
                "INSERT OR IGNORE INTO principal_agent_ties (principal_id, workspace_id, host_ok, reasons, set_by, set_at)
                 SELECT x.principal_id, x.workspace_id,
                        EXISTS (SELECT 1 FROM grants g WHERE g.subject_kind = 'principal' AND g.subject_id = x.principal_id
                                  AND g.kind = 'app' AND g.target_id = '{HOST_APP_ID}' AND g.enabled != 0),
                        '[\"created\"]', 'upgrade:0.46.4', x.created_at
                   FROM principal_agent_creators x WHERE 1 = 1 {unclaimed}"
            ),
            [],
        )
        .map_err(|e| format!("skin.db step 6 (stamped guests): {e}"))?;
    }
    if claims {
        conn.execute_batch("DROP TABLE principal_person_claims;").map_err(|e| format!("skin.db step 6: {e}"))?;
    }
    Ok(())
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool, String> {
    super::table_exists_conn(conn, name)
}

/// The owner takes a tied guest over (see `creators::clear_agent_tie`, which
/// checks `by`): one IMMEDIATE transaction drops the tie, wipes the
/// password, ends every session and device sign-in, and voids the links an
/// App or agent could see.
pub(super) fn owner_claim(principal_id: &str, by: &str) -> Result<(), String> {
    with_conn(|conn| {
        let tx = super::immediate_tx(conn)?;
        drop_conn(&tx, principal_id)?;
        tx.execute("UPDATE principals SET password_hash = NULL WHERE id = ?1", params![principal_id])
            .map_err(|e| format!("owner claim: {e}"))?;
        super::revoke_sessions_for_principal(&tx, principal_id)?;
        tx.execute(
            "DELETE FROM app_user_invites WHERE principal_id = ?1 AND used_at IS NULL AND purpose != 'owner_reset'",
            params![principal_id],
        )
        .map_err(|e| format!("owner claim: {e}"))?;
        tx.commit().map_err(|e| format!("owner claim commit: {e}"))?;
        crate::log_debug!("[skin] owner claim of {principal_id} by {by}: tie dropped, password wiped");
        Ok(())
    })
}

/// One tie.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tie {
    pub principal_id: String,
    pub workspace_id: String,
    pub host_ok: bool,
    pub reasons: Vec<String>,
    pub set_by: String,
    pub set_at: i64,
}

/// What a write ties a user to.
#[derive(Debug, Clone, Copy)]
pub struct TieWrite<'a> {
    pub workspace_id: &'a str,
    /// `agent:<handle>` or `app-key:<id>`.
    pub set_by: &'a str,
    pub reason: &'a str,
}

fn map_tie(r: &rusqlite::Row<'_>) -> rusqlite::Result<Tie> {
    let reasons: String = r.get(3)?;
    Ok(Tie {
        principal_id: r.get(0)?,
        workspace_id: r.get(1)?,
        host_ok: r.get::<_, i64>(2)? != 0,
        reasons: serde_json::from_str(&reasons).unwrap_or_default(),
        set_by: r.get(4)?,
        set_at: r.get(5)?,
    })
}

const TIE_SELECT: &str = "SELECT principal_id, workspace_id, host_ok, reasons, set_by, set_at FROM principal_agent_ties";

/// Does `principal_id` hold an enabled box login (`host` grant) now?
pub(super) fn holds_host_conn(conn: &Connection, principal_id: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM grants WHERE subject_kind = 'principal' AND subject_id = ?1
                         AND kind = 'app' AND target_id = ?2 AND enabled != 0)",
        params![principal_id, HOST_APP_ID],
        |r| r.get::<_, bool>(0),
    )
    .map_err(|e| format!("tie host lookup: {e}"))
}

/// Tie `principal_id` (inside the caller's transaction). `host_ok` is
/// whether they hold the box login at this moment. On a second write the
/// first workspace is kept, `host_ok` only ever goes down, and the reason
/// joins the list.
pub(super) fn set_conn(conn: &Connection, principal_id: &str, w: TieWrite<'_>) -> Result<(), String> {
    let ws = w.workspace_id.trim();
    if principal_id.trim().is_empty() || ws.is_empty() {
        return Err("a tie needs a guest and a workspace".into());
    }
    let host_ok = holds_host_conn(conn, principal_id)?;
    let existing = get_conn(conn, principal_id)?;
    // A tied person gets no chat pass (a chat grant is box-wide): end the
    // ones already out (security review 2026-10-25, item 14).
    if existing.is_none() && super::principal_has_chat_grant_conn(conn, principal_id)? {
        crate::sew_revoke::enqueue(
            conn,
            &crate::sew_revoke::SewSessionRevoke::ExternalId(crate::sew_revoke::principal_external_id(principal_id)),
        )?;
    }
    match existing {
        None => {
            conn.execute(
                "INSERT INTO principal_agent_ties (principal_id, workspace_id, host_ok, reasons, set_by, set_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![principal_id, ws, host_ok as i64, serde_json::json!([w.reason]).to_string(), w.set_by, now_secs()],
            )
            .map_err(|e| format!("tie insert: {e}"))?;
        }
        Some(t) => {
            let mut reasons = t.reasons.clone();
            if !reasons.iter().any(|r| r == w.reason) {
                reasons.push(w.reason.to_string());
            }
            conn.execute(
                "UPDATE principal_agent_ties SET host_ok = ?2, reasons = ?3 WHERE principal_id = ?1",
                params![principal_id, (t.host_ok && host_ok) as i64, serde_json::to_string(&reasons).unwrap_or_else(|_| "[]".into())],
            )
            .map_err(|e| format!("tie update: {e}"))?;
        }
    }
    Ok(())
}

/// [`set_conn`] on its own connection (an IMMEDIATE transaction).
pub fn set(principal_id: &str, w: TieWrite<'_>) -> Result<(), String> {
    with_conn(|conn| {
        let tx = super::immediate_tx(conn)?;
        set_conn(&tx, principal_id, w)?;
        tx.commit().map_err(|e| format!("tie commit: {e}"))
    })
}

pub(super) fn get_conn(conn: &Connection, principal_id: &str) -> Result<Option<Tie>, String> {
    conn.query_row(&format!("{TIE_SELECT} WHERE principal_id = ?1"), params![principal_id], map_tie)
        .optional()
        .map_err(|e| format!("tie lookup: {e}"))
}

/// The tie on `principal_id`, if any.
pub fn get(principal_id: &str) -> Result<Option<Tie>, String> {
    with_conn(|conn| get_conn(conn, principal_id))
}

/// Every tie.
pub fn list() -> Result<Vec<Tie>, String> {
    with_conn(|conn| {
        let mut st = conn.prepare(TIE_SELECT).map_err(|e| format!("tie list: {e}"))?;
        let rows = st.query_map([], map_tie).map_err(|e| format!("tie list: {e}"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| format!("tie list: {e}"))
    })
}

/// Drop the tie (an owner reset was used), inside the caller's transaction.
pub(super) fn drop_conn(conn: &Connection, principal_id: &str) -> Result<bool, String> {
    conn.execute("DELETE FROM principal_agent_ties WHERE principal_id = ?1", params![principal_id])
        .map(|n| n > 0)
        .map_err(|e| format!("tie delete: {e}"))
}

// ── the ceiling ──────────────────────────────────────────────────────

/// Outside the tied user's ceiling. Login answers the bad-password 401;
/// resolve answers `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Denied;

/// Is App `app` (whose workspace is `app_workspace`; `None` for `host` or
/// an App that no longer exists) inside `tie`'s ceiling?
pub fn app_inside(tie: &Tie, app: &str, app_workspace: Option<&str>) -> bool {
    if app == HOST_APP_ID {
        return tie.host_ok;
    }
    app_workspace == Some(tie.workspace_id.as_str())
}

/// What a tied user may use of `policy` on `app` (§5.2): no tie → the
/// policy as is; an App outside the ceiling → [`Denied`]; inside → only
/// the tie's room, without person-only caps (an empty result is allowed:
/// the session is Thread-dark).
pub fn clamp(tie: Option<&Tie>, app: &str, app_workspace: Option<&str>, policy: RoomPolicy) -> Result<RoomPolicy, Denied> {
    let Some(tie) = tie else { return Ok(policy) };
    if !app_inside(tie, app, app_workspace) {
        return Err(Denied);
    }
    let mut out = RoomPolicy::new();
    for (room, caps) in policy {
        if room != tie.workspace_id {
            continue;
        }
        let caps: Vec<String> = caps.into_iter().filter(|c| !PERSON_ONLY_CAPS.contains(&c.as_str())).collect();
        out.insert(room, caps);
    }
    Ok(out)
}

/// The room policy a grant gives (its role's per-room policy, or its scope
/// rooms with Thread-only caps), from already-read roles. A role that is
/// gone reads as nothing.
fn grant_policy(g: &SkinGrant, roles: &[SkinRole]) -> RoomPolicy {
    match g.role_id.as_deref() {
        Some(rid) => roles.iter().find(|r| r.id == rid).map(|r| r.room_policy.clone()).unwrap_or_default(),
        None => super::thread_only_policy(&super::parse_rooms_json(g.scope.as_deref())),
    }
}

/// Pending reset (display only): do `principal_id`'s live grants reach
/// past `tie`'s ceiling? A non-App grant (chat, mailbox, database, cli),
/// the box login without `host_ok`, an App of another workspace, or a
/// room or person-only cap beyond what [`clamp`] keeps. The legacy box
/// login role on `principals.role_id` rides the host grant.
pub fn pending(
    tie: &Tie,
    principal_id: &str,
    grants: &[SkinGrant],
    roles: &[SkinRole],
    app_workspace: &dyn Fn(&str) -> Option<String>,
) -> bool {
    for g in grants.iter().filter(|g| g.subject_kind == "principal" && g.subject_id == principal_id && g.enabled) {
        if g.kind != "app" {
            return true;
        }
        let ws = if g.target_id == HOST_APP_ID { None } else { app_workspace(&g.target_id) };
        let policy = grant_policy(g, roles);
        match clamp(Some(tie), &g.target_id, ws.as_deref(), policy.clone()) {
            Err(Denied) => return true,
            Ok(kept) if kept != policy => return true,
            Ok(_) => {}
        }
    }
    false
}

// ── App → workspace, outside the skin lock (A21) ─────────────────────

thread_local! {
    static SKIN_LOCK_HELD: Cell<bool> = const { Cell::new(false) };
}

/// Times the App → workspace lookup ran while this thread held the skin
/// mutex (the lock-order rule, T3.9). Always 0 in a correct build.
pub static LOCK_ORDER_VIOLATIONS: AtomicUsize = AtomicUsize::new(0);

/// Marks the skin mutex held on this thread for the guard's life (set by
/// `with_conn`).
pub(super) struct HeldGuard(bool);

impl HeldGuard {
    pub(super) fn enter() -> Self {
        HeldGuard(SKIN_LOCK_HELD.with(|h| h.replace(true)))
    }
}

impl Drop for HeldGuard {
    fn drop(&mut self) {
        let prev = self.0;
        SKIN_LOCK_HELD.with(|h| h.set(prev));
    }
}

/// Does this thread hold the skin mutex right now?
pub fn skin_lock_held() -> bool {
    SKIN_LOCK_HELD.with(|h| h.get())
}

/// The workspace (`projects.id`) of published App `app`; `None` for
/// `host`, an empty id, or an App that no longer exists. Reads the daemon
/// DB, so it must never run while the skin mutex is held (the skin mutex
/// never waits on the daemon DB mutex): callers look it up before
/// `with_conn` (mint) or after it (resolve).
pub fn app_workspace(app: &str) -> Option<String> {
    let app = app.trim();
    if app.is_empty() || app == HOST_APP_ID {
        return None;
    }
    if skin_lock_held() {
        LOCK_ORDER_VIOLATIONS.fetch_add(1, Ordering::SeqCst);
        debug_assert!(false, "app_workspace({app}) ran under the skin mutex (lock order: never skin → daemon DB)");
    }
    let db = crate::db::shared();
    let conn = db.lock();
    crate::published_services::get_by_id(&conn, app).ok().flatten().map(|s| s.project_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tie(ws: &str, host_ok: bool) -> Tie {
        Tie {
            principal_id: "p1".into(),
            workspace_id: ws.into(),
            host_ok,
            reasons: vec![REASON_CREATED.into()],
            set_by: "agent:sales".into(),
            set_at: 1,
        }
    }

    fn policy(lines: &[(&str, &[&str])]) -> RoomPolicy {
        lines.iter().map(|(r, caps)| (r.to_string(), caps.iter().map(|c| c.to_string()).collect())).collect()
    }

    #[test]
    fn clamp_is_the_ceiling() {
        let wide = policy(&[("ws", &["thread:read", "files:read", "files:download", "files:agentconfig"]), ("sales", &["thread:read"])]);
        // No tie: unchanged.
        assert_eq!(clamp(None, "svc-x", Some("other"), wide.clone()), Ok(wide.clone()));
        // An App of the tie's workspace: only that room, person-only caps gone.
        let kept = clamp(Some(&tie("ws", false)), "svc-a", Some("ws"), wide.clone()).unwrap();
        assert_eq!(kept, policy(&[("ws", &["thread:read", "files:read"])]));
        // Another workspace's App, an App that's gone: denied.
        assert_eq!(clamp(Some(&tie("ws", false)), "svc-b", Some("sales"), wide.clone()), Err(Denied));
        assert_eq!(clamp(Some(&tie("ws", false)), "svc-gone", None, wide.clone()), Err(Denied));
        // The box login only with host_ok.
        assert_eq!(clamp(Some(&tie("ws", false)), HOST_APP_ID, None, wide.clone()), Err(Denied));
        assert_eq!(clamp(Some(&tie("ws", true)), HOST_APP_ID, None, wide.clone()).unwrap(), kept);
        // Nothing in the tie's room: an empty (Thread-dark) policy, allowed.
        assert_eq!(clamp(Some(&tie("ws", true)), HOST_APP_ID, None, policy(&[("sales", &["thread:read"])])), Ok(RoomPolicy::new()));
    }

    fn grant(kind: &str, target: &str, role: Option<&str>, scope: Option<&str>) -> SkinGrant {
        SkinGrant {
            id: format!("g-{kind}-{target}"),
            subject_kind: "principal".into(),
            subject_id: "p1".into(),
            kind: kind.into(),
            target_id: target.into(),
            role_id: role.map(str::to_string),
            scope: scope.map(str::to_string),
            created_at: 1,
            enabled: true,
            created_by: None,
        }
    }

    fn role(id: &str, p: RoomPolicy) -> SkinRole {
        SkinRole {
            id: id.into(),
            name: id.into(),
            caps: Vec::new(),
            rooms: p.keys().cloned().collect(),
            room_handles: Vec::new(),
            room_access: Vec::new(),
            created_at: 1,
            app_id: None,
            room_policy: p,
            room_paths: Default::default(),
        }
    }

    #[test]
    fn pending_means_grants_reach_past_the_ceiling() {
        let ws_of = |app: &str| match app {
            "svc-a" => Some("ws".to_string()),
            "svc-b" => Some("sales".to_string()),
            _ => None,
        };
        let roles = vec![
            role("r-in", policy(&[("ws", &["thread:read"])])),
            role("r-wide", policy(&[("ws", &["thread:read"]), ("sales", &["thread:read"])])),
            role("r-dl", policy(&[("ws", &["files:download"])])),
        ];
        let t = tie("ws", false);
        let inside = vec![grant("app", "svc-a", Some("r-in"), None)];
        assert!(!pending(&t, "p1", &inside, &roles, &ws_of), "inside the ceiling");
        for (what, extra) in [
            ("box login without host_ok", grant("app", HOST_APP_ID, None, Some("[]"))),
            ("another workspace's App", grant("app", "svc-b", Some("r-in"), None)),
            ("a role reaching another room", grant("app", "svc-a", Some("r-wide"), None)),
            ("a person-only cap", grant("app", "svc-a", Some("r-dl"), None)),
            ("a chat grant", grant("chat", "sew", None, Some("member"))),
            ("role-less rooms elsewhere", grant("app", "svc-a", None, Some(r#"["sales"]"#))),
        ] {
            let mut gs = inside.clone();
            gs.retain(|g| g.target_id != extra.target_id || g.kind != extra.kind);
            gs.push(extra);
            assert!(pending(&t, "p1", &gs, &roles, &ws_of), "{what} is pending");
        }
        // A disabled grant is no grant.
        let mut off = grant("chat", "sew", None, Some("member"));
        off.enabled = false;
        assert!(!pending(&t, "p1", &[inside[0].clone(), off], &roles, &ws_of));
        // Another principal's grants don't count.
        let mut theirs = grant("chat", "sew", None, Some("member"));
        theirs.subject_id = "p2".into();
        assert!(!pending(&t, "p1", &[inside[0].clone(), theirs], &roles, &ws_of));
    }
}
