//! The claims object of one request (prd-app-supabase-compat-v1 §5.2) and
//! how it reaches Postgres. This is the shared contract lanes C and D code
//! against:
//!
//! ```json
//! {"sub": "<user id>", "role": "authenticated", "email": "a@example.com",
//!  "phone": "", "iss": "k2:app:<app id>", "aud": "authenticated",
//!  "session_id": "<k2 pass id>",
//!  "app_metadata": {"k2_app": "<app id>", "k2_role": "<App role>",
//!                   "provider": "email", "providers": ["email"], …},
//!  "user_metadata": {…}, "is_anonymous": false, "iat": 0, "exp": 0}
//! ```
//! Anon: `{"role":"anon","iss","iat","app_metadata"}`; service:
//! `{"role":"service_role","iss","iat","app_metadata"}`. A third-party
//! token's claims keep its own `iss`, `aud`, `iat`, `exp`.
//!
//! - **Doors** (lane C/D): log in as the App's `<db>_authenticator`
//!   ([`super::door_config::DoorConfig::authenticator`]), then in each
//!   transaction set the role ([`set_role_sql`], or `set_config('role', $1,
//!   true)`) and the claims ([`TX_CLAIMS_SQL`] with `$1` = the JSON, `$2` =
//!   `sub` or ''). `auth.*` reads those settings only on that login.
//! - **The brokered lane** (`/cli/db/query`, the rows doors): the migrator
//!   writes a claims lock before `SET ROLE authenticated` ([`for_brokered`]).
//! - **The App's own connection**: `k2.set_claims(token)`
//!   ([`super::identity_token`]).

use k2_core::deploy::store::AppRow;
use k2_core::skin::compat::AppUser;
use serde_json::{json, Map, Value};

/// `iss` on claims K2 makes for its own sessions: `k2:app:<app id>`,
/// stable for the App's life (a URL would change with its domains). A
/// third-party token's claims keep the token's own issuer.
pub fn issuer(app_id: &str) -> String {
    format!("k2:app:{app_id}")
}

/// How long claims K2 makes say they last (`exp - iat`): a K2 session is
/// checked on every request, so this is informational, as in Supabase.
pub const CLAIMS_TTL_SECS: i64 = 3600;

/// One transaction's claims, after the role: `$1` the claims JSON, `$2`
/// the `sub` ('' for none). Both are transaction-local.
pub const TX_CLAIMS_SQL: &str = "SELECT set_config('request.jwt.claims', $1, true), set_config('request.jwt.claim.sub', $2, true)";

/// The role a request runs as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoorRole {
    Anon,
    Authenticated,
    Service,
}

impl DoorRole {
    pub fn name(self) -> &'static str {
        match self {
            Self::Anon => "anon",
            Self::Authenticated => "authenticated",
            Self::Service => "service_role",
        }
    }
}

/// `SET LOCAL ROLE …` for a door transaction.
pub fn set_role_sql(role: DoorRole) -> &'static str {
    match role {
        DoorRole::Anon => "SET LOCAL ROLE anon",
        DoorRole::Authenticated => "SET LOCAL ROLE authenticated",
        DoorRole::Service => "SET LOCAL ROLE service_role",
    }
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// `app_metadata`: the user's own, then K2's keys (K2's win).
fn app_metadata(app_id: &str, user: Option<&AppUser>) -> Value {
    let mut m: Map<String, Value> = user.and_then(|u| u.app_metadata.as_object().cloned()).unwrap_or_default();
    m.entry("provider").or_insert(json!("email"));
    m.entry("providers").or_insert(json!(["email"]));
    m.insert("k2_app".into(), json!(app_id));
    if let Some(r) = user.and_then(|u| u.role_name.clone()) {
        m.insert("k2_role".into(), json!(r));
    }
    Value::Object(m)
}

/// A signed-in App user's claims (a K2 session, a device pass, the
/// person's pass on the gateway link, or a third-party token mapped to
/// this user under `sub user`).
pub fn for_user(app_id: &str, user: &AppUser, session_id: Option<&str>) -> Value {
    for_user_at(app_id, user, session_id, now())
}

pub fn for_user_at(app_id: &str, user: &AppUser, session_id: Option<&str>, iat: i64) -> Value {
    json!({
        "sub": user.id,
        "role": "authenticated",
        "email": user.email.as_deref().map(str::to_lowercase),
        "phone": "",
        "iss": issuer(app_id),
        "aud": "authenticated",
        "session_id": session_id,
        "app_metadata": app_metadata(app_id, Some(user)),
        "user_metadata": user.user_metadata,
        "is_anonymous": false,
        "iat": iat,
        "exp": iat + CLAIMS_TTL_SECS,
    })
}

/// The anon lane's claims (`{"role":"anon"}` plus the App).
pub fn anon(app_id: &str) -> Value {
    json!({ "role": "anon", "iss": issuer(app_id), "iat": now(), "app_metadata": app_metadata(app_id, None) })
}

/// The server key's claims.
pub fn service(app_id: &str) -> Value {
    json!({ "role": "service_role", "iss": issuer(app_id), "iat": now(), "app_metadata": app_metadata(app_id, None) })
}

/// `$2` of [`TX_CLAIMS_SQL`].
pub fn sub_of(claims: &Value) -> String {
    claims.get("sub").and_then(Value::as_str).unwrap_or("").to_string()
}

/// The brokered lane on database `db` for a guest `principal` holding pass
/// `pass_id`: when an App has the layer on for `db`, the claims to lock and
/// that the lane runs as `authenticated`. Prefers the App the pass was
/// minted for; else the first App on `db` the person is a user of. `None`:
/// no App has the layer on there (the lane stays as it was).
pub(crate) fn for_brokered(db: &str, principal: &str, pass_id: &str) -> Option<Value> {
    let apps = super::door_config::for_database(db).ok()?;
    if apps.is_empty() {
        return None;
    }
    let pass_app = k2_core::skin::compat::pass_app_service(pass_id).ok().flatten();
    let rows: Vec<AppRow> = {
        let h = k2_core::db::shared();
        let c = h.lock();
        apps.iter().filter_map(|s| k2_core::deploy::store::get_app(&c, &s.config.app_id).ok().flatten()).collect()
    };
    let mut ordered: Vec<&AppRow> = rows.iter().filter(|a| pass_app.as_deref() == Some(a.service_id.as_str())).collect();
    ordered.extend(rows.iter().filter(|a| pass_app.as_deref() != Some(a.service_id.as_str())));
    for app in ordered {
        if let Ok(Some(u)) = k2_core::skin::compat::app_user(&app.service_id, principal) {
            return Some(for_user(&app.app_id, &u, Some(pass_id)));
        }
    }
    // A layer is on for this database but the person is no user of those
    // Apps: authenticated with only `sub` (they matched no App row).
    let app = rows.first()?;
    Some(json!({ "sub": principal, "role": "authenticated", "iss": issuer(&app.app_id), "aud": "authenticated", "session_id": pass_id, "app_metadata": app_metadata(&app.app_id, None) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user() -> AppUser {
        AppUser {
            id: "11111111-1111-1111-1111-111111111111".into(),
            username: "ann".into(),
            email: Some("Ann@Example.com".into()),
            full_name: None,
            enabled: true,
            created_at: 1,
            last_sign_in_at: None,
            role_name: Some("staff".into()),
            app_metadata: json!({"plan": "pro", "k2_app": "spoof"}),
            user_metadata: json!({"theme": "dark"}),
        }
    }

    #[test]
    fn the_claims_have_the_documented_shape() {
        let c = for_user_at("crm", &user(), Some("sess-1"), 100);
        assert_eq!(
            c,
            json!({
                "sub": "11111111-1111-1111-1111-111111111111", "role": "authenticated", "email": "ann@example.com",
                "phone": "", "iss": "k2:app:crm", "aud": "authenticated", "session_id": "sess-1",
                "app_metadata": {"plan": "pro", "k2_app": "crm", "k2_role": "staff", "provider": "email", "providers": ["email"]},
                "user_metadata": {"theme": "dark"}, "is_anonymous": false, "iat": 100, "exp": 3700
            })
        );
        assert_eq!(anon("crm")["role"], "anon");
        assert_eq!(service("crm")["role"], "service_role");
        assert_eq!(sub_of(&c), "11111111-1111-1111-1111-111111111111");
        assert_eq!(sub_of(&anon("crm")), "");
        assert_eq!(set_role_sql(DoorRole::Service), "SET LOCAL ROLE service_role");
    }
}
