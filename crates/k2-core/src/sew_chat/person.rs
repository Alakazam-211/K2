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
