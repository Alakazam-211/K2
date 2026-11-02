//! People behind chat posts: whom an assistant workspace posts for, and
//! which person a deciding caller is.
//!
//! Sew stamps `on_behalf_of` on every post of an assistant member (S27's
//! `assistant:<person>` chat grant); there is no per-request "as" field.
//! So K2's `--on-behalf-of <user>` is a declaration the daemon checks against the
//! workspace's chat grant, and the person is named by the same Sew
//! external id S27 provisions (`k2:owner` or `k2:principal:<uuid>`).

use rusqlite::{params, Connection, OptionalExtension};

use crate::tool_hook::chat::{self, ChatScope};

/// A person an assistant posts for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    /// Sew external id: `k2:owner` or `k2:principal:<uuid>`.
    pub external_id: String,
    /// Their chat username (what `--on-behalf-of` names).
    pub username: String,
}

/// The owner person's external id.
pub const OWNER_EXTERNAL_ID: &str = "k2:owner";

/// Does `--on-behalf-of <given>` name this person? Case-insensitive, `@` optional.
pub fn names(p: &Person, given: &str) -> bool {
    let g = given.trim().trim_start_matches('@');
    !g.is_empty() && g.eq_ignore_ascii_case(&p.username)
}

/// The person `project_id`'s agent assists in chat, from its enabled
/// workspace `chat` grant on `conn` (skin.db). `None` = a coworker, or no
/// chat grant. A grant whose scope or person can't be read is an error
/// (fail closed: the caller refuses the write).
pub fn assistant_of_in(conn: &Connection, project_id: &str, owner_login: Option<&str>) -> Result<Option<Person>, String> {
    let scope: Option<Option<String>> = conn
        .query_row(
            "SELECT scope FROM grants WHERE kind = 'chat' AND subject_kind = 'workspace'
               AND lower(subject_id) = lower(?1) AND enabled = 1",
            params![project_id.trim()],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| format!("chat grant lookup: {e}"))?;
    let Some(scope) = scope else {
        return Ok(None);
    };
    let scope = scope.ok_or_else(|| "this workspace's chat grant has no scope".to_string())?;
    match ChatScope::parse_stored(&scope)? {
        ChatScope::AssistantOwner => Ok(Some(Person {
            external_id: OWNER_EXTERNAL_ID.to_string(),
            username: chat::owner_username(owner_login),
        })),
        ChatScope::AssistantOf(id) => {
            let p = chat::principal_by_id(conn, &id)?
                .ok_or_else(|| format!("the person this workspace assists ({id}) is gone"))?;
            Ok(Some(Person { external_id: format!("k2:principal:{id}"), username: p.username }))
        }
        ChatScope::Coworker | ChatScope::Member | ChatScope::Guest => Ok(None),
    }
}

/// [`assistant_of_in`] on the box's skin.db (no skin.db = no chat grant).
pub fn assistant_of(project_id: &str) -> Result<Option<Person>, String> {
    if !crate::tool_hook::skin_store_exists() {
        return Ok(None);
    }
    let owner_login = crate::person_name::owner_login_now();
    crate::skin::with_skin_conn(|conn| assistant_of_in(conn, project_id, owner_login.as_deref()))
}

/// Whether `project_id`'s agent has its own chat account: an enabled
/// workspace `chat` grant (coworker or assistant) on `conn` (skin.db).
pub fn has_chat_grant_in(conn: &Connection, project_id: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT 1 FROM grants WHERE kind = 'chat' AND subject_kind = 'workspace'
           AND lower(subject_id) = lower(?1) AND enabled = 1 LIMIT 1",
        params![project_id.trim()],
        |_| Ok(()),
    )
    .optional()
    .map(|row| row.is_some())
    .map_err(|e| format!("chat grant lookup: {e}"))
}

/// [`has_chat_grant_in`] on this box's skin store (`false` when there is
/// none yet: nobody has a chat grant).
pub fn has_chat_grant(project_id: &str) -> Result<bool, String> {
    if !crate::tool_hook::skin_store_exists() {
        return Ok(false);
    }
    crate::skin::with_skin_conn(|conn| has_chat_grant_in(conn, project_id))
}

/// Who K2 stamps on a chat grant it made because "Manage Chat / Voice
/// Hosting" is on.
pub const AUTO_ACCOUNT_BY: &str = "auto:chat_manage";

/// Why [`ensure_chat_manage_account`] looks at a workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoAccount {
    /// A person just turned the switch on: that gesture also brings back
    /// an account someone removed earlier.
    SwitchedOn,
    /// A sweep or a chat call: an account a person removed stays removed
    /// (K2's member mirror still has its row) until they turn the switch
    /// on again or the agent asks on a card.
    Sweep,
}

/// "Manage Chat / Voice Hosting" on means the agent has its own chat
/// account (Rosson 2026-11-02: an agent with the switch manages end to
/// end, so no second ask). Makes the workspace's `coworker` grant when the
/// switch is on, chat is on, and it has no grant at all. Never touches an
/// existing grant: an assistant stays one, and a grant a person turned off
/// stays off. Turning the switch off keeps the account. `Ok(true)` = made.
pub fn ensure_chat_manage_account(project_id: &str, why: AutoAccount) -> Result<bool, String> {
    if !crate::tool_hook::skin_store_exists() || !crate::workspace::settings::agents_can_manage_chat(project_id) {
        return Ok(false);
    }
    let ext = format!("k2:workspace:{}", project_id.trim().to_ascii_lowercase());
    let wanted = crate::skin::with_skin_conn(|conn| {
        if !crate::tool_hook::outbox::target_enabled(conn, crate::tool_hook::CHAT_KIND)? {
            return Ok(false);
        }
        let any_grant = conn
            .query_row(
                "SELECT 1 FROM grants WHERE kind = 'chat' AND subject_kind = 'workspace' AND lower(subject_id) = lower(?1) LIMIT 1",
                params![project_id.trim()],
                |_| Ok(()),
            )
            .optional()
            .map_err(|e| format!("chat grant lookup: {e}"))?
            .is_some();
        if any_grant {
            return Ok(false);
        }
        let removed = why == AutoAccount::Sweep && crate::tool_hook::members::by_external_id(conn, &ext)?.is_some();
        Ok(!removed)
    })?;
    if !wanted {
        return Ok(false);
    }
    crate::skin::create_grant(&crate::skin::SkinGrantWrite {
        subject_kind: "workspace".into(),
        subject_id: project_id.trim().to_string(),
        kind: crate::tool_hook::CHAT_KIND.into(),
        target_id: crate::tool_hook::CHAT_TARGET_ID.into(),
        role: None,
        scope: Some("coworker".into()),
        created_by: Some(AUTO_ACCOUNT_BY.into()),
    })?;
    Ok(true)
}

/// The sweep: every workspace with the switch on (one turned on before
/// this release, or before chat was enabled). One workspace's failure
/// doesn't stop the others; returns how many accounts it made.
pub fn ensure_chat_manage_accounts() -> Result<usize, String> {
    if !crate::tool_hook::skin_store_exists() {
        return Ok(0);
    }
    let ids: Vec<String> = {
        let db = crate::db::shared();
        let conn = db.lock();
        let mut st = conn.prepare("SELECT id FROM projects WHERE agents_can_manage_chat = 1").map_err(|e| e.to_string())?;
        let rows = st.query_map([], |r| r.get::<_, String>(0)).map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())?
    };
    let mut made = 0;
    for id in ids {
        match ensure_chat_manage_account(&id, AutoAccount::Sweep) {
            Ok(true) => made += 1,
            Ok(false) => {}
            Err(e) => crate::log_debug!("[chat] the automatic chat account for {id}: {e}"),
        }
    }
    Ok(made)
}

/// The Sew external id of the person behind a Connect login: an
/// owner-linked login is the owner (`k2:owner`, S27: one person, one
/// member); any other login is `k2:connect:<login>` (never an assistant's
/// person, so it can't collide with one).
pub fn person_of_login(username: &str, owner_linked: bool) -> String {
    if owner_linked {
        OWNER_EXTERNAL_ID.to_string()
    } else {
        format!("k2:connect:{}", username.trim().to_ascii_lowercase())
    }
}

/// Is this Connect login linked to the owner (`ownerAccounts`)? Unreadable
/// people settings → `true` (fail closed: treated as the owner, so the
/// owner's own-post rule still holds).
pub fn login_is_owner(username: &str) -> bool {
    match crate::notify_prefs::People::load() {
        Ok(p) => p.person_key(username) == crate::notify_prefs::OWNER,
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE principals (id TEXT PRIMARY KEY, username TEXT NOT NULL UNIQUE, email TEXT, full_name TEXT,
               created_at INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE grants (id TEXT PRIMARY KEY, subject_kind TEXT NOT NULL, subject_id TEXT NOT NULL,
               kind TEXT NOT NULL, target_id TEXT NOT NULL, role_id TEXT, scope TEXT, created_at INTEGER NOT NULL DEFAULT 0,
               enabled INTEGER NOT NULL DEFAULT 1);",
        )
        .unwrap();
        c
    }

    const P: &str = "11111111-2222-4333-8444-555555555555";

    fn grant(c: &Connection, ws: &str, scope: &str, enabled: i64) {
        c.execute(
            "INSERT INTO grants (id, subject_kind, subject_id, kind, target_id, scope, enabled)
             VALUES (?1, 'workspace', ?2, 'chat', 'sew', ?3, ?4)",
            params![format!("g-{ws}"), ws, scope, enabled],
        )
        .unwrap();
    }

    #[test]
    fn coworkers_and_ungranted_workspaces_assist_nobody() {
        let c = conn();
        assert_eq!(assistant_of_in(&c, "w-none", None).unwrap(), None);
        grant(&c, "w-co", "coworker", 1);
        assert_eq!(assistant_of_in(&c, "W-CO", None).unwrap(), None, "case-insensitive id, coworker = nobody");
    }

    #[test]
    fn a_chat_account_is_an_enabled_workspace_chat_grant_of_either_scope() {
        let c = conn();
        assert!(!has_chat_grant_in(&c, "w-none").unwrap());
        grant(&c, "w-co", "coworker", 1);
        assert!(has_chat_grant_in(&c, "W-CO").unwrap(), "case-insensitive id");
        grant(&c, "w-as", &format!("assistant:{P}"), 1);
        assert!(has_chat_grant_in(&c, "w-as").unwrap(), "an assistant has its own account too");
        grant(&c, "w-off", "coworker", 0);
        assert!(!has_chat_grant_in(&c, "w-off").unwrap(), "a turned-off grant is no account");
    }

    #[test]
    fn assistants_resolve_their_person_and_disabled_grants_do_not() {
        let c = conn();
        c.execute("INSERT INTO principals (id, username) VALUES (?1, 'ada')", params![P]).unwrap();
        grant(&c, "w-ada", &format!("assistant:{P}"), 1);
        let p = assistant_of_in(&c, "w-ada", None).unwrap().unwrap();
        assert_eq!(p, Person { external_id: format!("k2:principal:{P}"), username: "ada".into() });
        assert!(names(&p, "@Ada") && names(&p, "ada") && !names(&p, "bob") && !names(&p, "@"));
        grant(&c, "w-own", "assistant:owner", 1);
        let o = assistant_of_in(&c, "w-own", Some("rosson.long")).unwrap().unwrap();
        assert_eq!(o, Person { external_id: "k2:owner".into(), username: "rosson-long".into() });
        assert_eq!(assistant_of_in(&c, "w-own", None).unwrap().unwrap().username, "owner");
        grant(&c, "w-off", "assistant:owner", 0);
        assert_eq!(assistant_of_in(&c, "w-off", None).unwrap(), None, "a disabled grant is no grant");
    }

    #[test]
    fn unreadable_grants_fail_closed() {
        let c = conn();
        grant(&c, "w-bad", "assistant:not-a-uuid", 1);
        assert!(assistant_of_in(&c, "w-bad", None).is_err());
        grant(&c, "w-gone", &format!("assistant:{P}"), 1);
        assert!(assistant_of_in(&c, "w-gone", None).is_err(), "the person row is missing");
    }

    #[test]
    fn logins_map_to_people() {
        assert_eq!(person_of_login("Rosson", true), "k2:owner");
        assert_eq!(person_of_login("Ada", false), "k2:connect:ada");
    }
}
