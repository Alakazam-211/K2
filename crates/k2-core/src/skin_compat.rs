//! The user store's side of the Supabase-compatible data layer
//! (prd-app-supabase-compat-v1 §5.3, §5.4, §6.1; lane A).
//!
//! - **Metadata.** Two JSON objects per App user: `app_metadata` (the
//!   owner, the App's agent or the server key set it; never the user) and
//!   `user_metadata` (the user may set it). 16 KiB each. Both go into the
//!   claims and into `auth.users`. Columns on `principals`, additive.
//! - **The App's users, as `auth.users` sees them** ([`app_users`]): every
//!   principal with a grant for the App, enabled or not (a disabled one is
//!   banned), with its email, metadata, App role and last sign-in.
//! - **Identity links** (§6.1): the pair (issuer, `sub`) a third-party token
//!   carried the first time it was matched to a user of an App. A later
//!   mismatch is refused; `k2 app identity unlink` clears it.
//!
//! The store stays the source of truth (R1): Postgres gets a projection.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The most a metadata object may hold, serialized (16 KiB, §5.3).
pub const METADATA_MAX_BYTES: usize = 16 * 1024;

pub(super) fn ensure_schema(conn: &Connection) -> Result<(), String> {
    // Additive, like `full_name`: an older K2 ignores the columns.
    let _ = conn.execute("ALTER TABLE principals ADD COLUMN app_metadata TEXT", []);
    let _ = conn.execute("ALTER TABLE principals ADD COLUMN user_metadata TEXT", []);
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS identity_links (
            app_service TEXT NOT NULL,
            issuer TEXT NOT NULL,
            sub TEXT NOT NULL,
            principal_id TEXT NOT NULL REFERENCES principals(id) ON DELETE CASCADE,
            created_at INTEGER NOT NULL,
            PRIMARY KEY (app_service, issuer, sub),
            UNIQUE (app_service, issuer, principal_id)
         );",
    )
    .map_err(|e| format!("skin.db identity_links: {e}"))
}

/// One user of one App, as the claims and `auth.users` see them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppUser {
    pub id: String,
    pub username: String,
    pub email: Option<String>,
    pub full_name: Option<String>,
    /// The App grant is enabled (a disabled user is banned in `auth.users`
    /// and refused on every door).
    pub enabled: bool,
    pub created_at: i64,
    /// The newest session the user signed in with (any App), if any.
    pub last_sign_in_at: Option<i64>,
    /// The App role's name, if the grant has one.
    pub role_name: Option<String>,
    pub app_metadata: Value,
    pub user_metadata: Value,
}

fn json_object(raw: Option<String>) -> Value {
    raw.and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Default::default()))
}

const APP_USER_SELECT: &str = "SELECT p.id, p.username, p.email, p.full_name, g.enabled, p.created_at,
        (SELECT max(t.created_at) FROM tokens t WHERE t.principal_id = p.id AND t.session = 1),
        r.name, p.app_metadata, p.user_metadata
     FROM grants g
     JOIN principals p ON p.id = g.subject_id
     LEFT JOIN roles r ON r.id = g.role_id
     WHERE g.subject_kind = 'principal' AND g.kind = 'app' AND g.target_id = ?1";

fn app_user_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<AppUser> {
    Ok(AppUser {
        id: r.get(0)?,
        username: r.get(1)?,
        email: r.get::<_, Option<String>>(2)?.filter(|e| !e.trim().is_empty()),
        full_name: r.get(3)?,
        enabled: r.get::<_, i64>(4)? != 0,
        created_at: r.get(5)?,
        last_sign_in_at: r.get(6)?,
        role_name: r.get(7)?,
        app_metadata: json_object(r.get(8)?),
        user_metadata: json_object(r.get(9)?),
    })
}

/// Every user of the App whose published service is `service_id` (its
/// grant's target), enabled or not, by id.
pub fn app_users(service_id: &str) -> Result<Vec<AppUser>, String> {
    super::with_conn(|c| app_users_in(c, service_id))
}

pub fn app_users_in(c: &Connection, service_id: &str) -> Result<Vec<AppUser>, String> {
    let mut st = c.prepare(&format!("{APP_USER_SELECT} ORDER BY p.id")).map_err(|e| format!("app users: {e}"))?;
    let rows = st.query_map(params![service_id], app_user_row).map_err(|e| format!("app users: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("app users: {e}"))
}

/// The App user with this id, if the principal has a grant for the App.
pub fn app_user(service_id: &str, principal_id: &str) -> Result<Option<AppUser>, String> {
    super::with_conn(|c| {
        c.query_row(&format!("{APP_USER_SELECT} AND p.id = ?2"), params![service_id, principal_id], app_user_row)
            .optional()
            .map_err(|e| format!("app user: {e}"))
    })
}

/// The App user whose email is `email` (lower-cased match, §6.1).
pub fn app_user_by_email(service_id: &str, email: &str) -> Result<Option<AppUser>, String> {
    let e = email.trim().to_lowercase();
    if e.is_empty() {
        return Ok(None);
    }
    super::with_conn(|c| {
        c.query_row(&format!("{APP_USER_SELECT} AND lower(trim(p.email)) = ?2"), params![service_id, e], app_user_row)
            .optional()
            .map_err(|e| format!("app user: {e}"))
    })
}

/// Check one metadata object: a JSON object, at most [`METADATA_MAX_BYTES`].
pub fn metadata_problem(v: &Value) -> Option<String> {
    if !v.is_object() {
        return Some("metadata must be a JSON object".into());
    }
    let n = serde_json::to_string(v).map(|s| s.len()).unwrap_or(usize::MAX);
    (n > METADATA_MAX_BYTES).then(|| format!("metadata is {n} bytes; at most {METADATA_MAX_BYTES}"))
}

/// Replace a user's `app_metadata` and/or `user_metadata` (each `None` =
/// keep). Refuses a value that isn't an object or is too big. Returns
/// whether the user exists.
pub fn set_metadata(principal_id: &str, app: Option<&Value>, user: Option<&Value>) -> Result<bool, String> {
    for v in [app, user].into_iter().flatten() {
        if let Some(p) = metadata_problem(v) {
            return Err(p);
        }
    }
    super::with_conn(|c| {
        let exists = c
            .query_row("SELECT 1 FROM principals WHERE id = ?1", params![principal_id], |_| Ok(()))
            .optional()
            .map_err(|e| format!("metadata: {e}"))?
            .is_some();
        if !exists {
            return Ok(false);
        }
        if let Some(v) = app {
            c.execute("UPDATE principals SET app_metadata = ?2 WHERE id = ?1", params![principal_id, v.to_string()])
                .map_err(|e| format!("metadata: {e}"))?;
        }
        if let Some(v) = user {
            c.execute("UPDATE principals SET user_metadata = ?2 WHERE id = ?1", params![principal_id, v.to_string()])
                .map_err(|e| format!("metadata: {e}"))?;
        }
        Ok(true)
    })
}

/// What [`match_identity`] decided for a third-party token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkOutcome {
    /// Matched and (on a first match) recorded.
    Ok { first: bool },
    /// The `sub` is recorded for another user of this App, or this user
    /// already has another `sub` from this issuer (§6.1).
    Changed { recorded_principal: Option<String>, recorded_sub: Option<String> },
}

/// §6.1: the token's (issuer, `sub`) against the user its email mapped to.
/// First match: recorded. Later: must be the same pair.
pub fn match_identity(service_id: &str, issuer: &str, sub: &str, principal_id: &str, now: i64) -> Result<LinkOutcome, String> {
    super::with_conn(|c| {
        let by_sub: Option<String> = c
            .query_row(
                "SELECT principal_id FROM identity_links WHERE app_service = ?1 AND issuer = ?2 AND sub = ?3",
                params![service_id, issuer, sub],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| format!("identity link: {e}"))?;
        let by_user: Option<String> = c
            .query_row(
                "SELECT sub FROM identity_links WHERE app_service = ?1 AND issuer = ?2 AND principal_id = ?3",
                params![service_id, issuer, principal_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| format!("identity link: {e}"))?;
        match (by_sub, by_user) {
            (Some(p), Some(s)) if p == principal_id && s == sub => Ok(LinkOutcome::Ok { first: false }),
            (None, None) => {
                c.execute(
                    "INSERT INTO identity_links (app_service, issuer, sub, principal_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![service_id, issuer, sub, principal_id, now],
                )
                .map_err(|e| format!("identity link: {e}"))?;
                Ok(LinkOutcome::Ok { first: true })
            }
            (p, s) => Ok(LinkOutcome::Changed { recorded_principal: p, recorded_sub: s }),
        }
    })
}

/// The App a session pass was minted for (`tokens.app_service_id`).
pub fn pass_app_service(token_id: &str) -> Result<Option<String>, String> {
    super::with_conn(|c| {
        c.query_row("SELECT app_service_id FROM tokens WHERE id = ?1", params![token_id], |r| r.get::<_, Option<String>>(0))
            .optional()
            .map(Option::flatten)
            .map_err(|e| format!("pass app: {e}"))
    })
}

/// `k2 app identity unlink <app> <user>`: forget the user's recorded
/// pairs for this App (every issuer). Returns how many.
pub fn unlink(service_id: &str, principal_id: &str) -> Result<usize, String> {
    super::with_conn(|c| {
        c.execute("DELETE FROM identity_links WHERE app_service = ?1 AND principal_id = ?2", params![service_id, principal_id])
            .map_err(|e| format!("identity unlink: {e}"))
    })
}

/// The user's recorded `sub` from `issuer` on this App, if any.
pub fn linked_sub(service_id: &str, issuer: &str, principal_id: &str) -> Result<Option<String>, String> {
    super::with_conn(|c| {
        c.query_row(
            "SELECT sub FROM identity_links WHERE app_service = ?1 AND issuer = ?2 AND principal_id = ?3",
            params![service_id, issuer, principal_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| format!("identity link: {e}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn app_user_fixture(app: &str) -> (String, String) {
        let p = super::super::add_principal_with_email("meta-ann", Some("Ann@Example.com")).expect("principal");
        super::super::with_conn(|c| {
            c.execute(
                "INSERT INTO grants (id, subject_kind, subject_id, kind, target_id, role_id, scope, created_at, enabled)
                 VALUES (?1, 'principal', ?2, 'app', ?3, NULL, '[]', 1, 1)",
                params![uuid::Uuid::new_v4().to_string(), p.id, app],
            )
            .map_err(|e| e.to_string())
        })
        .expect("grant");
        (p.id, p.username)
    }

    #[test]
    fn metadata_is_an_object_of_at_most_16_kib_and_reaches_the_app_user() {
        let _home = crate::test_env::TempHome::new();
        let (id, _) = app_user_fixture("svc-meta");
        assert!(set_metadata(&id, Some(&json!([1])), None).unwrap_err().contains("object"));
        let big = json!({ "x": "y".repeat(METADATA_MAX_BYTES) });
        assert!(set_metadata(&id, None, Some(&big)).unwrap_err().contains("at most"));
        assert!(set_metadata(&id, Some(&json!({"plan": "pro"})), Some(&json!({"theme": "dark"}))).unwrap());
        assert!(!set_metadata("nobody", Some(&json!({})), None).unwrap());
        let u = app_user_by_email("svc-meta", "  ann@EXAMPLE.com ").unwrap().expect("by email");
        assert_eq!(u.id, id);
        assert_eq!(u.app_metadata, json!({"plan": "pro"}));
        assert_eq!(u.user_metadata, json!({"theme": "dark"}));
        assert!(u.enabled);
        assert!(app_user_by_email("svc-other", "ann@example.com").unwrap().is_none(), "another App's user is not this App's");
        assert_eq!(app_users("svc-meta").unwrap().len(), 1);
    }

    #[test]
    fn an_identity_link_is_recorded_once_and_a_change_is_refused() {
        let _home = crate::test_env::TempHome::new();
        let (id, _) = app_user_fixture("svc-link");
        let other = super::super::add_principal_with_email("meta-bob", Some("bob@example.com")).expect("bob");
        let iss = "https://issuer.example";
        assert_eq!(match_identity("svc-link", iss, "fb-1", &id, 10).unwrap(), LinkOutcome::Ok { first: true });
        assert_eq!(match_identity("svc-link", iss, "fb-1", &id, 11).unwrap(), LinkOutcome::Ok { first: false });
        assert!(matches!(match_identity("svc-link", iss, "fb-2", &id, 12).unwrap(), LinkOutcome::Changed { recorded_sub: Some(s), .. } if s == "fb-1"));
        assert!(matches!(match_identity("svc-link", iss, "fb-1", &other.id, 13).unwrap(), LinkOutcome::Changed { recorded_principal: Some(p), .. } if p == id));
        assert_eq!(linked_sub("svc-link", iss, &id).unwrap().as_deref(), Some("fb-1"));
        assert_eq!(unlink("svc-link", &id).unwrap(), 1);
        assert_eq!(match_identity("svc-link", iss, "fb-2", &id, 14).unwrap(), LinkOutcome::Ok { first: true });
    }
}
