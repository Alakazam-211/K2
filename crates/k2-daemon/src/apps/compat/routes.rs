//! `k2 app compat …`, `k2 app identity …`, `k2 app user meta` (lane A's
//! verbs, prd-app-supabase-compat-v1 §11.3).
//!
//! People (the owner, an Admin) and the App's own workspace agent with
//! "Build and run Apps" change the settings; Members read the status. K2 is
//! a service (R6): risky settings answer with a warning, not a wall.

use std::collections::HashMap;

use serde_json::{json, Value as J};

use super::super::routes::{app_for, json_body, need_build, need_credential, no, s, Who, R};
use super::door_config::{self, DoorConfig, Limits, Stored, SubMode, UnknownEmail};
use crate::sql::compat_ops;

pub const ON: &str = "/cli/app/compat/on";
pub const OFF: &str = "/cli/app/compat/off";
pub const STATUS: &str = "/cli/app/compat/status";
pub const SET: &str = "/cli/app/compat/set";
pub const SYNC_USERS: &str = "/cli/app/compat/sync-users";
pub const ISSUER_ADD: &str = "/cli/app/identity/issuer/add";
pub const ISSUER_RM: &str = "/cli/app/identity/issuer/rm";
pub const ISSUERS: &str = "/cli/app/identity/issuers";
pub const UNLINK: &str = "/cli/app/identity/unlink";
pub const USER_META: &str = "/cli/app/user/meta";

pub const ROUTES: &[&str] = &[ON, OFF, STATUS, SET, SYNC_USERS, ISSUER_ADD, ISSUER_RM, ISSUERS, UNLINK, USER_META];
pub const POST_ROUTES: &[&str] = &[ON, OFF, SET, SYNC_USERS, ISSUER_ADD, ISSUER_RM, UNLINK, USER_META];

pub mod codes {
    pub const USAGE: &str = "usage";
    /// The layer is off for the App.
    pub const OFF: &str = "compat_off";
    /// The layer is on for another database: off first.
    pub const OTHER_DB: &str = "compat_other_db";
    /// Postgres or the vault refused a step.
    pub const FAILED: &str = "compat_failed";
    pub const USER_NOT_FOUND: &str = "user_not_found";
}

fn usage(h: impl Into<String>) -> super::super::routes::Refusal {
    no(codes::USAGE, 400, h)
}

fn failed(h: impl Into<String>) -> super::super::routes::Refusal {
    no(codes::FAILED, 502, h)
}

fn ops_refusal(e: crate::sql::ops::OpsError) -> super::super::routes::Refusal {
    let status = e.status().get(..3).and_then(|s| s.parse::<u16>().ok()).unwrap_or(500);
    no(e.code(), status, e.hint().to_string())
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn by(who: &Who) -> String {
    super::super::routes::actor(who)
}

fn audit(app: &str, event: &str, who: &Who, payload: J) {
    let h = k2_core::db::shared();
    let c = h.lock();
    if let Err(e) = k2_core::deploy::audit::append(&c, k2_core::deploy::audit::SUBJECT_APP, app, event, &by(who), None, &payload) {
        k2_core::log_debug!("[compat] audit {app} {event}: {e}");
    }
}

pub(crate) fn handle(path: &str, who: &Who, body: &[u8], params: &HashMap<String, String>) -> R<J> {
    need_credential(who)?;
    match path {
        ON => on(who, body),
        OFF => off(who, body),
        STATUS => status(who, params),
        SET => set(who, body),
        SYNC_USERS => sync_users(who, body),
        ISSUER_ADD => issuer_add(who, body),
        ISSUER_RM => issuer_rm(who, body),
        ISSUERS => issuers(who, params),
        UNLINK => unlink(who, body),
        USER_META => user_meta(who, body),
        _ => Err(no("not_found", 404, "no such compat route")),
    }
}

fn loaded(app_id: &str) -> R<Stored> {
    door_config::load(app_id)
        .map_err(failed)?
        .ok_or_else(|| no(codes::OFF, 409, format!("the Supabase-compatible layer is off for {app_id} (k2 app compat on {app_id} --db <db>)")))
}

/// Warnings for the App's current settings (R6: tools and warnings).
fn warnings(cfg: &DoorConfig) -> Vec<String> {
    let mut w = Vec::new();
    match compat_ops::exposure(crate::sql::routes::current_ops(), &cfg.database, &cfg.schemas) {
        Ok(x) => {
            if cfg.anon {
                w.push(format!(
                    "the anon lane is on: anyone with the publishable key reaches {} table(s) or view(s) anon can read and {} it can write",
                    x.anon_read, x.anon_write
                ));
            }
            if x.no_rls_readable > 0 {
                w.push(format!(
                    "{} table(s) that anon or authenticated can read have no row security (ALTER TABLE … ENABLE ROW LEVEL SECURITY)",
                    x.no_rls_readable
                ));
            }
        }
        Err(e) => w.push(format!("couldn't count what the standard roles reach: {}", e.hint())),
    }
    if cfg.service_key_public {
        w.push("the server key is accepted on the public host: anyone who holds it bypasses every row rule".into());
    }
    if cfg.unknown_email == UnknownEmail::Authenticated {
        w.push("a valid third-party token whose email is no App user runs as authenticated (Supabase's behaviour)".into());
    }
    if cfg.origins.iter().any(|o| !o.starts_with("https://")) {
        w.push("an allowed origin is not https".into());
    }
    if !cfg.origins.is_empty() {
        w.push("cookies across sites need SameSite=None: a listed origin on another site makes the session cookie cross-site".into());
    }
    let others: Vec<String> = door_config::all().unwrap_or_default().into_iter().map(|s| s.config.database).collect();
    let n = compat_ops::neighbours_in_cluster(&cfg.database, &others);
    if !n.is_empty() {
        w.push(format!(
            "the standard role names are shared by every database in this cluster, and {} is also a compat database here: a moved App is best given its own cluster",
            n.join(", ")
        ));
    }
    w
}

fn view(s: &Stored) -> J {
    let c = &s.config;
    json!({
        "app": c.app_id, "database": c.database, "authenticator": c.authenticator, "anon": c.anon,
        "serviceKeyPublic": c.service_key_public, "origins": c.origins, "schemas": c.schemas, "limits": c.limits,
        "sub": c.sub_mode, "unknownEmail": c.unknown_email, "usersSyncedAt": s.users_synced_at, "since": s.created_at, "by": s.created_by,
    })
}

/// POST `/cli/app/compat/on {app, db}`.
fn on(who: &Who, body: &[u8]) -> R<J> {
    need_build(who)?;
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    super::super::suspend::refuse_if_suspended(&app.app_id)?;
    super::super::ownership::refuse_if_parked(&app)?;
    let db = s(&v, "db").ok_or_else(|| usage("name the database: --db <name> (one the App's workspace owns; k2 db list)"))?.to_string();
    let (db_id, db_name) = compat_ops::owned_db(&app.owner_workspace, &db).map_err(ops_refusal)?;
    let existing = door_config::load(&app.app_id).map_err(failed)?;
    if let Some(e) = &existing {
        if e.config.database != db_name {
            return Err(no(
                codes::OTHER_DB,
                409,
                format!("the layer is on for {} with {}: k2 app compat off {} first", app.app_id, e.config.database, app.app_id),
            ));
        }
    }
    let secrets = crate::sql::secrets::FileSecretStore::default();
    let refs = existing.as_ref().map(|e| (e.config.authenticator_secret_ref.as_str(), e.claims_key_ref.as_str()));
    let others: Vec<String> = door_config::all().map_err(failed)?.into_iter().map(|s| s.config.database).collect();
    let made = compat_ops::turn_on(crate::sql::routes::current_ops(), &secrets, &db_name, refs, &others).map_err(ops_refusal)?;
    if !made.stripped.is_empty() {
        eprintln!("[compat] {}: memberships of the standard roles that weren't K2's were revoked: {}", db_name, made.stripped);
    }
    let t = now();
    let stored = match existing {
        Some(mut e) => {
            e.config.authenticator = made.authenticator.clone();
            e.config.authenticator_secret_ref = made.authenticator_secret_ref.clone();
            e.claims_key_ref = made.claims_key_ref.clone();
            e
        }
        None => Stored {
            config: DoorConfig {
                app_id: app.app_id.clone(),
                database: db_name.clone(),
                database_id: db_id,
                authenticator: made.authenticator.clone(),
                authenticator_secret_ref: made.authenticator_secret_ref.clone(),
                anon: false,
                service_key_public: false,
                origins: Vec::new(),
                schemas: vec!["public".into()],
                limits: Limits::default(),
                sub_mode: SubMode::Token,
                unknown_email: UnknownEmail::Refuse,
            },
            claims_key_ref: made.claims_key_ref.clone(),
            users_synced_at: None,
            created_by: by(who),
            created_at: t,
        },
    };
    door_config::upsert(&stored, t).map_err(failed)?;
    let users = super::users_sync::sync_database(&db_name).map_err(|e| failed(format!("the layer is on, but auth.users wasn't written: {e} (k2 app compat sync-users {})", app.app_id)))?;
    audit(&app.app_id, "compat.on", who, json!({ "db": db_name, "authenticator": made.authenticator, "rolesStripped": made.stripped }));
    let stored = door_config::load(&app.app_id).map_err(failed)?.unwrap_or(stored);
    let mut w = warnings(&stored.config);
    if !made.stripped.is_empty() {
        w.push(format!("memberships of the standard roles that weren't K2's were revoked: {}", made.stripped));
    }
    Ok(json!({ "ok": true, "compat": view(&stored), "users": users, "rolesStripped": made.stripped, "warnings": w }))
}

/// POST `/cli/app/compat/off {app}`. The door login stops (unless another
/// compat App uses the same database); the `auth` schema and `auth.users`
/// stay, since the App's foreign keys point at them.
fn off(who: &Who, body: &[u8]) -> R<J> {
    need_build(who)?;
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    let s0 = loaded(&app.app_id)?;
    let shared = door_config::for_database(&s0.config.database).map_err(failed)?.iter().any(|o| o.config.app_id != app.app_id);
    if !shared {
        compat_ops::turn_off(crate::sql::routes::current_ops(), &s0.config.database, &s0.config.authenticator).map_err(ops_refusal)?;
    }
    door_config::remove(&app.app_id).map_err(failed)?;
    audit(&app.app_id, "compat.off", who, json!({ "db": s0.config.database, "loginKept": shared }));
    Ok(json!({ "ok": true, "app": app.app_id, "database": s0.config.database, "loginKept": shared,
               "note": "auth.users and the auth schema stay in the database; the doors answer 404 for this App" }))
}

/// GET `/cli/app/compat/status?app=`.
fn status(who: &Who, params: &HashMap<String, String>) -> R<J> {
    let app = app_for(who, params.get("app").map(String::as_str))?;
    let Some(st) = door_config::load(&app.app_id).map_err(failed)? else {
        return Ok(json!({ "ok": true, "app": app.app_id, "on": false }));
    };
    let issuers: Vec<J> = super::issuers::list(&app.app_id)
        .map_err(failed)?
        .into_iter()
        .map(|i| json!({ "key": i.key, "issuer": i.issuer, "audience": i.audience, "jwks": i.jwks_url, "keys": super::issuers::key_status(&i.jwks_url) }))
        .collect();
    let mut w = warnings(&st.config);
    for i in &issuers {
        if let Some(x) = i["keys"]["warning"].as_str() {
            w.push(format!("{}'s signing keys didn't refresh: {x}", i["issuer"].as_str().unwrap_or("")));
        }
    }
    Ok(json!({ "ok": true, "app": app.app_id, "on": true, "compat": view(&st), "issuers": issuers, "warnings": w }))
}

fn on_off(v: &str) -> R<bool> {
    match v {
        "on" | "true" | "yes" => Ok(true),
        "off" | "false" | "no" => Ok(false),
        _ => Err(usage(format!("{v}: on or off"))),
    }
}

fn list(v: &str) -> Vec<String> {
    if v == "none" {
        return Vec::new();
    }
    v.split(',').map(str::trim).filter(|x| !x.is_empty()).map(str::to_string).collect()
}

/// POST `/cli/app/compat/set {app, key, value}`.
fn set(who: &Who, body: &[u8]) -> R<J> {
    need_build(who)?;
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    let mut st = loaded(&app.app_id)?;
    let key = s(&v, "key").ok_or_else(|| usage("name the setting: anon, service-key-public, origins, schemas, max-rows, sub, unknown-email"))?;
    let value = s(&v, "value").ok_or_else(|| usage(format!("give {key} a value")))?;
    let c = &mut st.config;
    match key {
        "anon" => c.anon = on_off(value)?,
        "service-key-public" => c.service_key_public = on_off(value)?,
        "origins" => {
            let o = list(value);
            if let Some(bad) = o.iter().find(|x| url::Url::parse(x).map(|u| u.origin().ascii_serialization() != x.trim_end_matches('/')).unwrap_or(true)) {
                return Err(usage(format!("{bad} is not an origin (scheme://host[:port])")));
            }
            c.origins = o;
        }
        "schemas" => {
            let sc = list(value);
            if sc.is_empty() || sc.iter().any(|x| !k2_root_proto::pg_major::role_name_ok(x) || x == "auth" || x == "k2" || x == "storage") {
                return Err(usage("schemas: a comma list of your own schema names (not auth, k2 or storage)"));
            }
            c.schemas = sc;
        }
        "max-rows" => {
            c.limits.max_rows = value.parse::<u32>().ok().filter(|n| (1..=100_000).contains(n)).ok_or_else(|| usage("max-rows: 1 to 100000"))?;
        }
        "sub" => {
            c.sub_mode = match value {
                "token" => SubMode::Token,
                "user" => SubMode::User,
                _ => return Err(usage("sub: token (the issuer's id, the cut-over setting) or user (the App user's id)")),
            }
        }
        "unknown-email" => {
            c.unknown_email = match value {
                "refuse" => UnknownEmail::Refuse,
                "authenticated" => UnknownEmail::Authenticated,
                _ => return Err(usage("unknown-email: refuse or authenticated")),
            }
        }
        other => return Err(usage(format!("{other}: not a setting (anon, service-key-public, origins, schemas, max-rows, sub, unknown-email)"))),
    }
    door_config::upsert(&st, now()).map_err(failed)?;
    audit(&app.app_id, "compat.set", who, json!({ "key": key, "value": value }));
    Ok(json!({ "ok": true, "compat": view(&st), "warnings": warnings(&st.config) }))
}

/// POST `/cli/app/compat/sync-users {app}`.
fn sync_users(who: &Who, body: &[u8]) -> R<J> {
    need_build(who)?;
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    let st = loaded(&app.app_id)?;
    let w = super::users_sync::sync_database(&st.config.database).map_err(failed)?;
    if w.changed() {
        eprintln!("[compat] {}: sync-users repaired auth.users ({} added, {} changed, {} removed)", st.config.database, w.inserted, w.updated, w.deleted);
    }
    Ok(json!({ "ok": true, "database": st.config.database, "users": w }))
}

fn issuer_add(who: &Who, body: &[u8]) -> R<J> {
    need_build(who)?;
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    loaded(&app.app_id)?;
    let i = super::issuers::Issuer {
        app_id: app.app_id.clone(),
        key: s(&v, "key").unwrap_or("").to_string(),
        issuer: s(&v, "issuer").unwrap_or("").to_string(),
        audience: s(&v, "audience").unwrap_or("").to_string(),
        jwks_url: s(&v, "jwks").unwrap_or("").to_string(),
    };
    super::issuers::add(&i, &by(who), now()).map_err(usage)?;
    audit(&app.app_id, "identity.issuer.add", who, json!({ "key": i.key, "issuer": i.issuer, "audience": i.audience, "jwks": i.jwks_url }));
    Ok(json!({ "ok": true, "issuer": i }))
}

fn issuer_rm(who: &Who, body: &[u8]) -> R<J> {
    need_build(who)?;
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    let which = s(&v, "issuer").ok_or_else(|| usage("name the issuer (its iss or its key)"))?;
    if !super::issuers::remove(&app.app_id, which).map_err(failed)? {
        return Err(no("not_found", 404, format!("{} trusts no issuer {which}", app.app_id)));
    }
    audit(&app.app_id, "identity.issuer.rm", who, json!({ "issuer": which }));
    Ok(json!({ "ok": true, "removed": which }))
}

fn issuers(who: &Who, params: &HashMap<String, String>) -> R<J> {
    let app = app_for(who, params.get("app").map(String::as_str))?;
    let rows: Vec<J> = super::issuers::list(&app.app_id)
        .map_err(failed)?
        .into_iter()
        .map(|i| json!({ "key": i.key, "issuer": i.issuer, "audience": i.audience, "jwks": i.jwks_url, "keys": super::issuers::key_status(&i.jwks_url) }))
        .collect();
    Ok(json!({ "ok": true, "app": app.app_id, "issuers": rows }))
}

/// One of the App's users, by username, email or id.
fn app_user(service_id: &str, who: &str) -> R<k2_core::skin::compat::AppUser> {
    let w = who.trim().to_lowercase();
    k2_core::skin::compat::app_users(service_id)
        .map_err(failed)?
        .into_iter()
        .find(|u| u.id == who || u.username.to_lowercase() == w || u.email.as_deref().is_some_and(|e| e.to_lowercase() == w))
        .ok_or_else(|| no(codes::USER_NOT_FOUND, 404, format!("{who} is not a user of this App (k2 app guests)")))
}

fn unlink(who: &Who, body: &[u8]) -> R<J> {
    need_build(who)?;
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    let u = app_user(&app.service_id, s(&v, "user").ok_or_else(|| usage("name the user"))?)?;
    let n = k2_core::skin::compat::unlink(&app.service_id, &u.id).map_err(failed)?;
    audit(&app.app_id, "identity.unlink", who, json!({ "user": u.id, "cleared": n }));
    Ok(json!({ "ok": true, "user": u.username, "cleared": n }))
}

/// POST `/cli/app/user/meta {app, user, appMetadata?, userMetadata?}`: the
/// owner or the App's agent sets either object (each replaces the old
/// one); without either, it reads them.
fn user_meta(who: &Who, body: &[u8]) -> R<J> {
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    let u = app_user(&app.service_id, s(&v, "user").ok_or_else(|| usage("name the user"))?)?;
    let am = v.get("appMetadata").filter(|x| !x.is_null());
    let um = v.get("userMetadata").filter(|x| !x.is_null());
    if am.is_none() && um.is_none() {
        return Ok(json!({ "ok": true, "user": u.username, "id": u.id, "appMetadata": u.app_metadata, "userMetadata": u.user_metadata }));
    }
    need_build(who)?;
    k2_core::skin::compat::set_metadata(&u.id, am, um).map_err(usage)?;
    super::users_sync::after_user_change(&app.service_id);
    audit(&app.app_id, "user.meta", who, json!({ "user": u.id, "app": am.is_some(), "user_": um.is_some() }));
    let u = app_user(&app.service_id, &u.id)?;
    Ok(json!({ "ok": true, "user": u.username, "id": u.id, "appMetadata": u.app_metadata, "userMetadata": u.user_metadata }))
}

/// Removing the App: the layer's row and issuers go; the door login stops
/// unless another compat App shares the database. Never fails the caller.
pub(crate) fn forget_for_uninstall(app_id: &str) {
    let Ok(Some(st)) = door_config::load(app_id) else { return };
    let shared = door_config::for_database(&st.config.database).unwrap_or_default().iter().any(|o| o.config.app_id != app_id);
    if !shared {
        if let Err(e) = compat_ops::turn_off(crate::sql::routes::current_ops(), &st.config.database, &st.config.authenticator) {
            k2_core::log_debug!("[compat] {app_id}: door login not stopped: {}", e.hint());
        }
    }
    if let Err(e) = door_config::remove(app_id) {
        k2_core::log_debug!("[compat] {app_id}: {e}");
    }
}
