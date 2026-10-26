//! Which workspace's agent added a guest (security review, 0.46.4). The
//! owner's guests carry no stamp. An agent with the skin-manage switch may
//! act on a guest with no rooms, role or App yet only when its own
//! workspace's agent added it; an unstamped guest stays the owner's. Copy
//! of box logins onto an App leaves out guests another workspace's agent
//! added. The table is additive in skin.db (created at open, no numbered
//! step), and a guest's row goes with it.

use rusqlite::{params, Connection, OptionalExtension};

use super::{now_secs, with_conn};

/// skin.db DDL (run by the idempotent open block). Ties live in
/// `principal_agent_ties`, which skin.db step 6 owns ([`super::ties`], the
/// single tie store); `principal_person_claims` is gone (step 6 folds the
/// claims in and drops it).
pub(super) fn ensure_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS principal_agent_creators (
            principal_id TEXT PRIMARY KEY REFERENCES principals(id) ON DELETE CASCADE,
            workspace_id TEXT NOT NULL,
            created_at INTEGER NOT NULL
         );",
    )
    .map_err(|e| format!("skin.db principal_agent_creators: {e}"))
}

// -- agent ties (0.46.4 security review B1; agent-app-users R2) ---------
//
// A guest is "tied" while an agent or an App key could know or control its
// credentials: [`super::ties`] is the one record (who, which workspace,
// why). A tied guest is not "a person" for person-only actions (answering a
// card) and is held to its workspace's access. Only the owner lifts a tie:
// a used owner reset link, or [`clear_agent_tie`] (an owner claim, which
// also wipes the password the agent may know). The creator stamps stay a
// pure record of who added the guest.

/// The guest's tie: the workspace (`projects.id`) its tie holds it to, or
/// `None` for a person (the owner's untouched guest, or one an owner reset
/// or claim has taken over).
pub fn agent_tie(principal_id: &str) -> Result<Option<String>, String> {
    super::ties::get(principal_id).map(|t| t.map(|t| t.workspace_id))
}

/// Tie a guest an agent of `workspace_id` could now control (`reason`: one
/// of [`super::ties`]'s reasons). A person tied to another workspace is
/// refused (`tied_elsewhere`): one workspace per tie until an owner reset.
pub fn tie(principal_id: &str, workspace_id: &str, set_by: &str, reason: &str) -> Result<(), String> {
    super::ties::set(principal_id, super::ties::TieWrite { workspace_id, set_by, reason })
}

/// Who may lift a tie: ONLY the owner token (`owner-token`) or an Owner
/// Connect login (`user:<name>` whose role is Owner). Never the guest
/// itself (`self:…`: a reset link can reach the agent), never an Admin or
/// Member login, an App key, a Garden widget or an agent.
pub fn person_may_claim(by: &str) -> bool {
    let by = by.trim();
    by == "owner-token"
        || by.strip_prefix("user:").is_some_and(|n| crate::connect_users::role_for_user(n) == Some(crate::connect_users::Role::Owner))
}

/// The owner takes a tied guest over (an owner claim that requires a
/// reset): the tie is dropped, and in the same transaction the password
/// (which the agent may know) is wiped and every session and device
/// sign-in ends, so the person signs in again only after the owner gives
/// them a new password or a reset link. `by` must be the owner token or an
/// Owner login; anything else is refused and nothing is written.
pub fn clear_agent_tie(principal_id: &str, by: &str) -> Result<(), String> {
    let (pid, by) = (principal_id.trim().to_string(), by.trim().to_string());
    if pid.is_empty() || !person_may_claim(&by) {
        return Err("only the owner (the owner token or an Owner login) clears a guest's agent tie".into());
    }
    super::ties::owner_claim(&pid, &by)
}

#[cfg(test)]
mod tie_tests {
    use super::*;

    #[test]
    fn a_tied_guest_stays_tied_until_the_owner_claims_it() {
        let _home = crate::test_env::TempHome::new();
        let owners = super::super::add_principal("owners-guest").unwrap();
        let bots = super::super::add_principal("bots-guest").unwrap();
        super::super::set_principal_password("bots-guest", Some("agent-knows-1")).unwrap();
        stamp(&bots.id, "ws-agent").unwrap();
        tie(&bots.id, "ws-agent", "agent:bot", super::super::ties::REASON_CREATED).unwrap();
        assert_eq!(agent_tie(&owners.id).unwrap(), None, "the owner's guest is a person");
        assert_eq!(agent_tie(&bots.id).unwrap().as_deref(), Some("ws-agent"), "an agent-added guest is tied");
        // Only the owner token or an Owner login: never the guest itself, an
        // Admin or Member login, an App key, a Garden or an agent.
        let self_claim = format!("self:{}", bots.id);
        for by in ["agent:bot", "app-key:k1", "garden:board", "user:no-such-login", self_claim.as_str(), "", "owner"] {
            assert!(clear_agent_tie(&bots.id, by).is_err(), "{by:?} may not claim");
        }
        assert_eq!(agent_tie(&bots.id).unwrap().as_deref(), Some("ws-agent"));
        let (_, live) = super::super::create_session_token("bots-guest").unwrap();
        clear_agent_tie(&bots.id, "owner-token").unwrap();
        assert_eq!(agent_tie(&bots.id).unwrap(), None, "the owner took it over");
        assert!(super::super::resolve_skin_token(&live).is_none(), "the claim ends every session");
        assert!(!super::super::verify_principal_password("bots-guest", "agent-knows-1"), "the password the agent knew is gone");
        // The creator stamp stays a record of who added the guest.
        assert_eq!(workspace_of(&bots.id).unwrap().as_deref(), Some("ws-agent"));
        // A re-key ties it again; the first workspace is kept.
        tie(&bots.id, "ws-other", "agent:other", super::super::ties::REASON_PASSWORD).unwrap();
        assert_eq!(agent_tie(&bots.id).unwrap().as_deref(), Some("ws-other"), "re-keyed: tied again");
        let err = tie(&bots.id, "ws-third", "agent:third", super::super::ties::REASON_EMAIL).unwrap_err();
        assert!(err.starts_with(super::super::ties::TIED_ELSEWHERE), "{err}");
        assert_eq!(agent_tie(&bots.id).unwrap().as_deref(), Some("ws-other"), "a second workspace is refused, never folded in");
        // A tie never writes a creator stamp.
        tie(&owners.id, "ws-app", "app-key:k1", super::super::ties::REASON_APP_KEY).unwrap();
        assert_eq!(agent_tie(&owners.id).unwrap().as_deref(), Some("ws-app"));
        assert_eq!(workspace_of(&owners.id).unwrap(), None, "no creator stamp for an App key's guest");
    }
}

/// Stamp `principal_id` as added by the agent of `workspace_id`.
pub fn stamp(principal_id: &str, workspace_id: &str) -> Result<(), String> {
    let (pid, ws) = (principal_id.trim().to_string(), workspace_id.trim().to_string());
    if pid.is_empty() || ws.is_empty() {
        return Err("a creator stamp needs a guest and a workspace".into());
    }
    with_conn(|conn| stamp_conn(conn, &pid, &ws))
}

/// [`stamp`] on an open connection (inside the caller's transaction).
pub(super) fn stamp_conn(conn: &Connection, principal_id: &str, workspace_id: &str) -> Result<(), String> {
    let (pid, ws) = (principal_id.trim(), workspace_id.trim());
    if pid.is_empty() || ws.is_empty() {
        return Err("a creator stamp needs a guest and a workspace".into());
    }
    conn.execute(
        "INSERT INTO principal_agent_creators (principal_id, workspace_id, created_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(principal_id) DO UPDATE SET workspace_id = excluded.workspace_id",
        params![pid, ws, now_secs()],
    )
    .map_err(|e| format!("creator stamp: {e}"))?;
    Ok(())
}

/// The workspace whose agent added `principal_id` (`None`: the owner's).
pub(super) fn workspace_of_conn(conn: &Connection, principal_id: &str) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT workspace_id FROM principal_agent_creators WHERE principal_id = ?1",
        params![principal_id],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .map_err(|e| format!("creator stamp lookup: {e}"))
}

/// [`workspace_of_conn`] on its own connection.
pub fn workspace_of(principal_id: &str) -> Result<Option<String>, String> {
    with_conn(|conn| workspace_of_conn(conn, principal_id))
}

/// The usernames of `app`'s guests (any `kind=app` grant on that published
/// service id, enabled or not) tied to `workspace_id`: their [`agent_tie`]
/// names it, or the owner marked them as that workspace's agent login
/// (`principals.agent_workspace`). Sorted. The hand-over guard refuses
/// while any exists: moving the App would leave them tied to a workspace
/// that no longer maintains it (and, once ties are ceilings, sign them out).
pub fn app_guests_tied_to(app: &str, workspace_id: &str) -> Result<Vec<String>, String> {
    let (app, ws) = (app.trim().to_string(), workspace_id.trim().to_string());
    if app.is_empty() || ws.is_empty() {
        return Ok(vec![]);
    }
    with_conn(|conn| {
        let rows: Vec<(String, String, Option<String>)> = {
            let mut st = conn
                .prepare(
                    "SELECT DISTINCT p.id, p.username, p.agent_workspace FROM principals p JOIN grants g
                       ON g.subject_kind = 'principal' AND g.subject_id = p.id AND g.kind = 'app' AND g.target_id = ?1
                      ORDER BY p.username",
                )
                .map_err(|e| format!("tied guests: {e}"))?;
            let it = st.query_map(params![app], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map_err(|e| format!("tied guests: {e}"))?;
            it.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| format!("tied guests: {e}"))?
        };
        let mut out = Vec::new();
        for (pid, username, marked) in rows {
            let marked_here = marked.as_deref().map(str::trim) == Some(ws.as_str());
            let tie: Option<String> = if claimed(conn, &pid)? {
                None
            } else {
                let tied: Option<String> = conn
                    .query_row("SELECT tied_to FROM principal_agent_ties WHERE principal_id = ?1", params![pid], |r| r.get(0))
                    .optional()
                    .map_err(|e| format!("agent tie lookup: {e}"))?;
                match tied {
                    Some(t) => Some(t),
                    None => workspace_of_conn(conn, &pid)?,
                }
            };
            if marked_here || tie.as_deref() == Some(ws.as_str()) {
                out.push(username);
            }
        }
        Ok(out)
    })
}
