//! `auth.users`, the daemon-written projection of the App user store
//! (prd-app-supabase-compat-v1 §5.4; lane A).
//!
//! - One database may serve several compat Apps: its `auth.users` is the
//!   union of their users (by id; ids are K2's principal ids, kept as they
//!   are, so an imported user keeps its Supabase id).
//! - **When it is written:** at turn-on; at daemon start (the whole table is
//!   checked; a difference is logged loudly and repaired, the store wins);
//!   on `k2 app compat sync-users`; when skin.db's access generation moves
//!   (a user added, changed, disabled or removed; [`spawn_watcher`]); and
//!   synchronously by a call that makes a user ([`after_user_change`])
//!   before it answers, so an insert that references the new id never
//!   races.
//! - A removed user's row is deleted, so the App's `ON DELETE` rules fire.
//!   A disabled user is banned (`banned_until = infinity`).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::sql::compat_ops::{self, UsersWrite};

/// One `auth.users` row as the daemon sends it (times in unix seconds).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UserRow {
    pub id: String,
    pub email: Option<String>,
    pub raw_app_meta_data: Value,
    pub raw_user_meta_data: Value,
    pub created_at: i64,
    pub last_sign_in_at: Option<i64>,
    pub email_confirmed: bool,
    pub banned: bool,
}

pub fn row_for(app_id: &str, u: &k2_core::skin::compat::AppUser) -> UserRow {
    let mut am: Map<String, Value> = u.app_metadata.as_object().cloned().unwrap_or_default();
    am.entry("provider").or_insert(json!("email"));
    am.entry("providers").or_insert(json!(["email"]));
    am.insert("k2_app".into(), json!(app_id));
    if let Some(r) = &u.role_name {
        am.insert("k2_role".into(), json!(r));
    }
    let mut um: Map<String, Value> = u.user_metadata.as_object().cloned().unwrap_or_default();
    if let Some(n) = u.full_name.as_ref().filter(|n| !n.trim().is_empty()) {
        um.entry("full_name").or_insert(json!(n));
    }
    UserRow {
        id: u.id.clone(),
        email: u.email.as_deref().map(str::to_lowercase),
        raw_app_meta_data: Value::Object(am),
        raw_user_meta_data: Value::Object(um),
        created_at: u.created_at,
        last_sign_in_at: u.last_sign_in_at,
        email_confirmed: u.email.is_some(),
        banned: !u.enabled,
    }
}

/// The rows database `db` should hold: every compat App on it, its users.
/// Ids that aren't uuids can't be `auth.users` ids: skipped and named.
pub(crate) fn wanted(db: &str) -> Result<(Vec<UserRow>, Vec<String>, String), String> {
    let apps = super::door_config::for_database(db)?;
    let mut rows: Vec<UserRow> = Vec::new();
    let mut skipped = Vec::new();
    let mut project = String::new();
    for s in &apps {
        let app = {
            let h = k2_core::db::shared();
            let c = h.lock();
            k2_core::deploy::store::get_app(&c, &s.config.app_id).map_err(|e| e.to_string())?
        };
        let Some(app) = app else { continue };
        project = app.owner_workspace.clone();
        for u in k2_core::skin::compat::app_users(&app.service_id)? {
            if uuid::Uuid::parse_str(&u.id).is_err() {
                skipped.push(u.username.clone());
                continue;
            }
            if !rows.iter().any(|r| r.id == u.id) {
                rows.push(row_for(&s.config.app_id, &u));
            }
        }
    }
    Ok((rows, skipped, project))
}

/// Make `db`'s `auth.users` equal the store (full: rows no App lists are
/// deleted). Returns what changed.
pub(crate) fn sync_database(db: &str) -> Result<UsersWrite, String> {
    let (rows, skipped, project) = wanted(db)?;
    if project.is_empty() {
        return Ok(UsersWrite::default());
    }
    for u in &skipped {
        eprintln!("[compat] {db}: user {u} has no uuid id; not in auth.users");
    }
    let json = serde_json::to_string(&rows).map_err(|e| e.to_string())?;
    let secrets = crate::sql::secrets::FileSecretStore::default();
    let w = compat_ops::write_users(crate::sql::routes::current_ops(), &secrets, &project, db, &json, true, &[]).map_err(|e| e.hint().to_string())?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    for s in super::door_config::for_database(db)? {
        let _ = super::door_config::mark_synced(&s.config.app_id, now);
    }
    Ok(w)
}

/// Every compat database, once. `loud`: a repaired difference is logged as
/// one (the boot check); lines are returned either way.
pub(crate) fn sync_all(loud: bool) -> Vec<String> {
    let mut dbs: Vec<String> = match super::door_config::all() {
        Ok(a) => a.into_iter().map(|s| s.config.database).collect(),
        Err(e) => return vec![format!("compat: {e}")],
    };
    dbs.sort();
    dbs.dedup();
    dbs.into_iter()
        .map(|db| match sync_database(&db) {
            Ok(w) if w.changed() && loud => format!(
                "{db}: auth.users differed from the App users and was repaired ({} added, {} changed, {} removed)",
                w.inserted, w.updated, w.deleted
            ),
            Ok(w) => format!("{db}: auth.users in step ({} added, {} changed, {} removed)", w.inserted, w.updated, w.deleted),
            Err(e) => format!("{db}: auth.users NOT synced: {e}"),
        })
        .collect()
}

/// A call that made, changed or removed a user of the App whose service
/// is `service_id`: write the projection now, before answering. A no-op
/// when no App with that service has the layer on.
pub fn after_user_change(service_id: &str) {
    let dbs: Vec<String> = match super::door_config::all() {
        Ok(a) => {
            let h = k2_core::db::shared();
            let c = h.lock();
            a.into_iter()
                .filter(|s| k2_core::deploy::store::get_app(&c, &s.config.app_id).ok().flatten().is_some_and(|app| app.service_id == service_id))
                .map(|s| s.config.database)
                .collect()
        }
        Err(_) => return,
    };
    for db in dbs {
        if let Err(e) = sync_database(&db) {
            eprintln!("[compat] {db}: auth.users not written after a user change: {e}");
        }
    }
}

static WATCHING: AtomicBool = AtomicBool::new(false);
static SEEN: AtomicU64 = AtomicU64::new(u64::MAX);

/// Once per daemon: the boot check (loud), then a write whenever skin.db's
/// access generation moves (checked every 2 s), and a full check every 15
/// minutes (last sign-in times aren't access writes).
pub fn spawn_watcher() {
    if WATCHING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::Builder::new()
        .name("compat-users".into())
        .spawn(|| {
            let mut ticks: u64 = 0;
            loop {
                let any = super::door_config::all().map(|a| !a.is_empty()).unwrap_or(false);
                let gen = k2_core::skin::write_generation();
                let first = SEEN.load(Ordering::SeqCst) == u64::MAX;
                if any && (first || gen != SEEN.load(Ordering::SeqCst) || ticks % 450 == 0) {
                    SEEN.store(gen, Ordering::SeqCst);
                    let _ = std::panic::catch_unwind(|| {
                        for line in sync_all(first) {
                            if first || !line.contains("in step (0 added, 0 changed, 0 removed)") {
                                eprintln!("[compat] {line}");
                            }
                        }
                    });
                } else if !any {
                    SEEN.store(gen, Ordering::SeqCst);
                }
                ticks += 1;
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
        })
        .map(|_| ())
        .unwrap_or_else(|e| eprintln!("[compat] users watcher: {e}"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_carries_the_users_metadata_and_k2s_keys() {
        let u = k2_core::skin::compat::AppUser {
            id: "11111111-1111-1111-1111-111111111111".into(),
            username: "ann".into(),
            email: Some("Ann@Example.com".into()),
            full_name: Some("Ann Lee".into()),
            enabled: false,
            created_at: 10,
            last_sign_in_at: Some(20),
            role_name: Some("staff".into()),
            app_metadata: json!({"tier": "gold", "k2_app": "spoof"}),
            user_metadata: json!({"theme": "dark"}),
        };
        let r = row_for("crm", &u);
        assert_eq!(r.email.as_deref(), Some("ann@example.com"));
        assert_eq!(r.raw_app_meta_data, json!({"tier": "gold", "k2_app": "crm", "k2_role": "staff", "provider": "email", "providers": ["email"]}));
        assert_eq!(r.raw_user_meta_data, json!({"theme": "dark", "full_name": "Ann Lee"}));
        assert!(r.banned && r.email_confirmed);
        assert_eq!((r.created_at, r.last_sign_in_at), (10, Some(20)));
    }
}
