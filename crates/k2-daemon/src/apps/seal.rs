//! The App seal key (K-M3, 0.46.3): one stable random secret per App, so
//! an App that seals its sessions (Hearth/Meet: AES-256-GCM over a sqlite
//! store in `$K2_APP_DATA`) keeps everyone signed in across deploys,
//! restarts, crash restarts and rollbacks.
//!
//! - **What:** 32 random bytes from the OS, as 43 base64url characters
//!   without padding (no newline).
//! - **Where it lives:** the vault, a server-scope secret named
//!   `K2_APP_SEAL_KEY.<app-id>.<6 hex>`, found by the App id (not its
//!   service id), so an uninstall without `--purge` and a reinstall keep
//!   it, like its data. The suffix is new for each key K2 makes: a purged
//!   App's destroyed key keeps its name in the vault as a tombstone.
//! - **How the App gets it:** the same way it gets its App key: a vault
//!   grant to the App's published service, delivered as the file
//!   `$K2_SECRETS_DIR/K2_APP_SEAL_KEY` (guarded container, process
//!   exception and the process engine alike). Never an environment
//!   variable, never a route body, never a log line.
//! - **When:** made the first time the App starts (any deploy pipeline
//!   run, so Apps installed before 0.46.3 get one on their next deploy or
//!   restart) and never again. A deploy, restart or rollback reads the
//!   same key: it belongs to the App, not to a Seed.
//! - **Only its own App:** the grant names one service; no other App's
//!   start resolves it.
//! - **Rotate:** only `k2 app seal rotate <app> --yes` (the owner, or an
//!   agent with switch 2 for its own App). Every session sealed with the
//!   old key stops opening: everyone is signed out. It reaches the App at
//!   its next start (`k2 app restart <app>`).
//! - **Purge:** `k2 app uninstall --purge` destroys it; a plain uninstall
//!   only drops the grant.
//! - With the vault closed the App starts without one (sessions in memory,
//!   as before 0.46.3); K2 logs that, without any value.

use std::collections::HashMap;

use base64::Engine;
use k2_core::deploy::{self, codes};
use k2_vault::{
    Actor, Consumer, Delivery, GrantRequest, Kind, ListFilter, LocalVault, Namespace, PutMode, PutRequest, Scope, SecretBytes, SecretMeta,
    SecretStatus, SecretStore, Selector, VaultError,
};
use serde_json::{json, Value as J};

use super::routes::{actor, app_for, json_body, need_credential, no, s, Refusal, Who, R};

/// The file name the App reads in `$K2_SECRETS_DIR`.
pub const SEAL_FILE: &str = "K2_APP_SEAL_KEY";

pub const ROUTES: &[&str] = &["/cli/app/seal", "/cli/app/seal/rotate"];
pub const POST_ROUTES: &[&str] = &["/cli/app/seal/rotate"];

/// What the rotate warning says (route and CLI).
pub const SIGNOUT_WARNING: &str = "rotating the seal key signs EVERYONE out of this App (members and guests): \
     the sessions it sealed can't be opened any more. Their passwords still work.";

/// Every vault name of an App's seal keys starts with this.
pub fn name_prefix(app_id: &str) -> String {
    format!("{SEAL_FILE}.{app_id}.")
}

fn by_name(name: &str) -> Selector {
    Selector::user(Scope::server(), name.to_string())
}

/// The App's seal key that isn't destroyed (the newest, should there be
/// more than one).
fn current(vault: &LocalVault, app_id: &str) -> Result<Option<SecretMeta>, VaultError> {
    let prefix = name_prefix(app_id);
    let all = vault.list(&ListFilter { scope: Some(Scope::server()), ..Default::default() }, &Actor::Owner)?;
    Ok(all
        .into_iter()
        .filter(|m| m.namespace == Namespace::User && m.name.starts_with(&prefix) && m.name.len() > prefix.len())
        .max_by(|a, b| a.created_at.cmp(&b.created_at)))
}

/// A fresh key: 32 bytes from the OS, base64url without padding.
pub fn generate() -> Result<SecretBytes, VaultError> {
    let mut b = [0u8; 32];
    getrandom::getrandom(&mut b).map_err(|e| VaultError::new("random_failed", 500, format!("random: {e}")))?;
    let s = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b);
    b.iter_mut().for_each(|x| *x = 0);
    Ok(SecretBytes::from_slice(s.as_bytes()))
}

/// Make the App's key if it has none, and grant it to `service_id` as
/// the file [`SEAL_FILE`] (replacing a seal grant of another name).
/// `Ok(true)` = a key was made now.
pub fn ensure(app_id: &str, service_id: &str) -> Result<bool, VaultError> {
    let vault = crate::vault::get()?;
    let (name, created) = match current(&vault, app_id)? {
        Some(m) => (m.name, false),
        None => {
            let name = format!("{}{}", name_prefix(app_id), &uuid::Uuid::new_v4().simple().to_string()[..6]);
            let mut put = PutRequest::new(Namespace::User, Scope::server(), name.clone(), Kind::Generic);
            put.mode = PutMode::Create;
            put.default_delivery = Some(Delivery::Fetch);
            put.delivery_ceiling = Some(Delivery::File);
            put.description =
                format!("Seal key for App {app_id} (made by K2; the App reads $K2_SECRETS_DIR/{SEAL_FILE}; rotate: k2 app seal rotate {app_id})");
            put.origin = "k2 app seal".into();
            vault.put(put, generate()?, &Actor::Owner)?;
            (name, true)
        }
    };
    let consumer = Consumer::published_service(service_id.to_string());
    let grants = vault.grants_to(&consumer)?;
    if grants.iter().any(|(g, n)| g.file_name.as_deref() == Some(SEAL_FILE) && n == &name) {
        return Ok(created);
    }
    for (g, _) in grants.iter().filter(|(g, _)| g.file_name.as_deref() == Some(SEAL_FILE)) {
        vault.revoke(&g.id, &Actor::Owner)?;
    }
    vault.grant(
        GrantRequest {
            selector: by_name(&name),
            consumer,
            delivery: Delivery::File,
            env_name: None,
            file_name: Some(SEAL_FILE.into()),
            hosts_narrowed: None,
            expires_at: None,
            version: None,
        },
        &Actor::Owner,
    )?;
    Ok(created)
}

/// The deploy pipeline's call before every start: never fails the start
/// (an App without a key keeps its sessions in memory, as before).
pub fn ensure_for_start(app_id: &str, service_id: &str) {
    match ensure(app_id, service_id) {
        Ok(true) => k2_core::log_debug!("[apps] {app_id}: made its seal key ($K2_SECRETS_DIR/{SEAL_FILE})"),
        Ok(false) => {}
        Err(e) => k2_core::log_debug!("[apps] {app_id}: no seal key this start ({}: {})", e.code, e.hint),
    }
}

/// A new key under the same name (the grant follows the latest version).
/// Returns the new version number.
pub fn rotate(app_id: &str, service_id: &str) -> Result<u32, VaultError> {
    ensure(app_id, service_id)?;
    let vault = crate::vault::get()?;
    let name = current(&vault, app_id)?.map(|m| m.name).ok_or_else(|| VaultError::not_found("the App's seal key is gone"))?;
    let out = vault.rotate(&by_name(&name), generate()?, &Actor::Owner)?;
    Ok(out.meta.latest_version)
}

/// Uninstall: drop the grant to the App's service; `purge` also destroys
/// the key (a reinstall then gets a new one).
pub fn forget(app_id: &str, service_id: &str, purge: bool) {
    let Ok(vault) = crate::vault::get() else { return };
    if let Ok(grants) = vault.grants_to(&Consumer::published_service(service_id.to_string())) {
        for (g, _) in grants.iter().filter(|(g, _)| g.file_name.as_deref() == Some(SEAL_FILE)) {
            let _ = vault.revoke(&g.id, &Actor::Owner);
        }
    }
    if purge {
        if let Ok(Some(m)) = current(&vault, app_id) {
            if let Err(e) = vault.destroy(&by_name(&m.name), &Actor::Owner) {
                k2_core::log_debug!("[apps] {app_id}: seal key not destroyed ({})", e.code);
            }
        }
    }
}

/// `{present, granted, version, updatedAt, file}`: never the value.
pub fn status(app_id: &str, service_id: &str) -> J {
    let vault = match crate::vault::get() {
        Ok(v) => v,
        Err(e) => return json!({ "vaultOpen": false, "error": e.code }),
    };
    let meta = current(&vault, app_id).ok().flatten();
    let granted = meta.as_ref().is_some_and(|m| {
        vault
            .grants_to(&Consumer::published_service(service_id.to_string()))
            .map(|gs| gs.iter().any(|(g, n)| g.file_name.as_deref() == Some(SEAL_FILE) && n == &m.name))
            .unwrap_or(false)
    });
    json!({
        "vaultOpen": true,
        "present": meta.is_some(),
        "disabled": meta.as_ref().is_some_and(|m| m.status != SecretStatus::Active),
        "granted": granted,
        "secret": meta.as_ref().map(|m| m.name.clone()),
        "version": meta.as_ref().map(|m| m.latest_version),
        "updatedAt": meta.as_ref().map(|m| m.updated_at.clone()),
        "file": format!("$K2_SECRETS_DIR/{SEAL_FILE}"),
    })
}

// ── routes ───────────────────────────────────────────────────────────

pub(crate) fn handle(path: &str, who: &Who, body: &[u8], params: &HashMap<String, String>) -> R<J> {
    match path {
        "/cli/app/seal" => show(who, params),
        "/cli/app/seal/rotate" => rotate_route(who, body),
        _ => Err(no("not_found", 404, "no such App seal route")),
    }
}

fn vault_refusal(e: VaultError) -> Refusal {
    no(if e.status == 503 { "vault_closed" } else { "vault_error" }, e.status.max(400), format!("the vault: {} ({})", e.hint, e.code))
}

/// GET `/cli/app/seal?app=` → whether the App has a seal key (no value).
fn show(who: &Who, params: &HashMap<String, String>) -> R<J> {
    need_credential(who)?;
    let app = app_for(who, params.get("app").map(String::as_str))?;
    Ok(json!({ "ok": true, "app": app.app_id, "seal": status(&app.app_id, &app.service_id) }))
}

/// POST `/cli/app/seal/rotate {app, yes}`: the owner (or a human in a K2
/// shell tab), or an agent with switch 2 for its own App. Without `yes`:
/// 409 with the sign-out warning; nothing changes.
fn rotate_route(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    if !(super::access::owner_human(who) || super::access::self_widens(who)) {
        return Err(no(
            codes::OWNER_ONLY,
            403,
            format!(
                "rotating an App's seal key signs everyone out, so only the owner (or an agent whose workspace has \
                 \"…and manage them end to end\") may: k2 app seal rotate {} --yes",
                app.app_id
            ),
        ));
    }
    if v.get("yes").and_then(J::as_bool) != Some(true) {
        let mut r = no("signout_confirm_required", 409, format!("{SIGNOUT_WARNING} Send yes: true (k2 app seal rotate {} --yes) to go ahead.", app.app_id));
        r.extra = Some(json!({ "warning": SIGNOUT_WARNING }));
        return Err(r);
    }
    let version = rotate(&app.app_id, &app.service_id).map_err(vault_refusal)?;
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if let Err(e) = deploy::audit::append(
            &conn,
            deploy::audit::SUBJECT_APP,
            &app.app_id,
            "seal_rotate",
            &actor(who),
            None,
            &json!({ "version": version }),
        ) {
            k2_core::log_debug!("[apps] audit {} seal_rotate: {e}", app.app_id);
        }
    }
    Ok(json!({
        "ok": true,
        "app": app.app_id,
        "rotated": true,
        "version": version,
        "warning": SIGNOUT_WARNING,
        "next": format!("it reaches the App at its next start: k2 app restart {}", app.app_id),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value_of(service_id: &str) -> Option<String> {
        let g = crate::vault::publish_env::granted(service_id, "ws-seal", std::path::Path::new("/nonexistent-seal-cwd"));
        g.files.iter().find(|(n, _)| n == SEAL_FILE).map(|(_, b)| String::from_utf8(b.clone()).expect("utf-8 key"))
    }

    #[test]
    fn keys_are_43_base64url_and_unique() {
        let a = generate().unwrap();
        let b = generate().unwrap();
        let (a, b) = (std::str::from_utf8(a.expose()).unwrap().to_string(), std::str::from_utf8(b.expose()).unwrap().to_string());
        assert_ne!(a, b);
        for k in [&a, &b] {
            assert_eq!(k.len(), 43, "{k}");
            assert!(k.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'), "{k}");
            let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(k).unwrap();
            assert_eq!(raw.len(), 32);
        }
    }

    /// Stable across starts (ensure again), only for its own App, rotate
    /// changes it, uninstall drops the grant and purge destroys it.
    #[test]
    fn one_stable_key_per_app_granted_as_a_file() {
        let _home = crate::test_support::TempHome::new();
        let _db = k2_core::db::scoped_for_test();
        crate::vault::ensure_test_vault();
        // Unique App ids: the test vault is shared by the test binary.
        let tag = &uuid::Uuid::new_v4().simple().to_string()[..8];
        let (app_a, app_b) = (format!("seal-a-{tag}"), format!("seal-b-{tag}"));
        let (svc_a, svc_b) = (format!("svc-{app_a}"), format!("svc-{app_b}"));
        assert!(ensure(&app_a, &svc_a).unwrap(), "first start makes it");
        let k1 = value_of(&svc_a).expect("granted as K2_APP_SEAL_KEY");
        assert_eq!(k1.len(), 43);
        // Every later start (deploy, restart, rollback) reads the same key.
        for _ in 0..3 {
            assert!(!ensure(&app_a, &svc_a).unwrap(), "never made twice");
            assert_eq!(value_of(&svc_a).as_deref(), Some(k1.as_str()));
        }
        // Another App: its own key, and never A's.
        assert_eq!(value_of(&svc_b), None, "B has nothing before its first start");
        ensure(&app_b, &svc_b).unwrap();
        let kb = value_of(&svc_b).unwrap();
        assert_ne!(kb, k1);
        assert_eq!(value_of(&svc_a).as_deref(), Some(k1.as_str()), "A unchanged");
        // Status names it, never its value.
        let st = status(&app_a, &svc_a);
        assert_eq!(st["present"], true, "{st}");
        assert_eq!(st["granted"], true, "{st}");
        assert_eq!(st["disabled"], false, "{st}");
        assert_eq!(st["version"], 1, "{st}");
        assert!(st["secret"].as_str().unwrap().starts_with(&name_prefix(&app_a)), "{st}");
        assert!(!st.to_string().contains(&k1), "status never carries the key");
        // The vault's redactor knows the value: anything that tried to
        // log it would print a mask.
        let line = format!("start with {k1}");
        assert!(!crate::vault::redact_str(&line).contains(&k1), "redacted on K2's surfaces");
        // Rotate: a new value, same grant; B untouched.
        let v2 = rotate(&app_a, &svc_a).unwrap();
        assert_eq!(v2, 2);
        let k2 = value_of(&svc_a).unwrap();
        assert_ne!(k2, k1, "rotate changes it");
        assert!(!ensure(&app_a, &svc_a).unwrap());
        assert_eq!(value_of(&svc_a).as_deref(), Some(k2.as_str()), "and it stays the new one");
        assert_eq!(value_of(&svc_b).as_deref(), Some(kb.as_str()));
        // A reinstall (new service id) after a plain uninstall keeps it.
        forget(&app_a, &svc_a, false);
        assert_eq!(value_of(&svc_a), None, "uninstall drops the grant");
        let svc_a2 = format!("{svc_a}-again");
        assert!(!ensure(&app_a, &svc_a2).unwrap());
        assert_eq!(value_of(&svc_a2).as_deref(), Some(k2.as_str()), "kept across a reinstall, like the data");
        // Purge destroys it: the next install makes a new one.
        forget(&app_a, &svc_a2, true);
        assert_eq!(value_of(&svc_a2), None);
        assert_eq!(status(&app_a, &svc_a2)["present"], false);
        assert!(ensure(&app_a, &svc_a2).unwrap(), "made again after a purge");
        let k3 = value_of(&svc_a2).unwrap();
        assert!(k3 != k1 && k3 != k2);
        assert_eq!(value_of(&svc_b).as_deref(), Some(kb.as_str()), "B never touched");
    }
}
