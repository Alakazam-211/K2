//! Daemon glue for the K2 vault (`prd-k2-vault-v1.md` §5.1).
//!
//! - [`boot`] opens `~/.k2/vault/vault.db` once, from the `serve` path,
//!   after the single-instance lock (VS14). Nothing else opens a KeyStore.
//! - [`get`] hands the open vault to routes and consumers.
//! - Key store selection (§7.2, KV23, KV26): macOS → Keychain via
//!   Security.framework (no file fallback); Linux → the root helper's
//!   keeper when it answers protocol ≥ 4, else systemd encrypted
//!   credentials when the unit provides them (Q24), else the file key.
//!   A better store showing up later is adopted in place (re-wrap, then
//!   the file key is removed).

pub mod actor;
pub mod adopt;
pub mod human;
#[cfg(target_os = "macos")]
pub mod keychain;
pub mod keeper_client;
#[cfg(target_os = "macos")]
pub mod local_auth;
pub mod refstore;
pub mod spawn;
pub mod systemd_creds;
pub mod tunnel;

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use k2_vault::keystore::file::FileKeyStore;
use k2_vault::{KeyStore, LocalVault, VaultError};
use parking_lot::RwLock;

/// The open vault, or why it isn't open.
static VAULT: OnceLock<RwLock<Result<Arc<LocalVault>, VaultError>>> = OnceLock::new();

fn cell() -> &'static RwLock<Result<Arc<LocalVault>, VaultError>> {
    VAULT.get_or_init(|| RwLock::new(Err(VaultError::new("vault_not_open", 503, "the vault isn't open yet"))))
}

/// The open vault.
pub fn get() -> Result<Arc<LocalVault>, VaultError> {
    cell().read().clone()
}

/// Was the daemon made non-dumpable when the vault opened (Q18)?
static NON_DUMPABLE: OnceLock<bool> = OnceLock::new();

pub fn non_dumpable() -> bool {
    NON_DUMPABLE.get().copied().unwrap_or(false)
}

pub fn vault_dir(home: &Path) -> PathBuf {
    home.join("vault")
}

pub fn file_key_dir(home: &Path) -> PathBuf {
    vault_dir(home).join("key")
}

/// The key store this box should use now.
pub fn select_key_store(home: &Path) -> Arc<dyn KeyStore> {
    #[cfg(target_os = "macos")]
    {
        let _ = home;
        Arc::new(keychain::KeychainKeyStore::new())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let t = keeper_client::SudoHelper::detect();
        if let Some(t) = t {
            if k2_vault::keystore::keeper::probe(&t).is_some() {
                return Arc::new(k2_vault::keystore::keeper::KeeperKeyStore::new(t));
            }
        }
        if let Some(s) = systemd_creds::SystemdCredsKeyStore::detect() {
            return Arc::new(s);
        }
        Arc::new(FileKeyStore::new(file_key_dir(home)))
    }
}

/// Open the vault at boot. Called once from `async_main`, after the
/// single-instance lock. Failures leave the vault closed with a reason
/// that `/cli/vault/status` shows; the daemon keeps running.
pub fn boot(home: &Path) {
    let dir = vault_dir(home);
    let ks = select_key_store(home);
    let r = open_with_upgrade(&dir, home, ks);
    match &r {
        Ok(v) => {
            let a = adopt::thread_secrets(v, &k2_core::overlay::vault::vault_root());
            if !a.moved.is_empty() || !a.errors.is_empty() {
                k2_core::log_debug!("[vault] thread-secrets: moved {} kept {} errors {:?}", a.moved.len(), a.kept.len(), a.errors);
            }
            record_adoption(a);
            for (path, engine, subsystem, orphans) in [
                (crate::mail::secrets::legacy_path(), "stalwart", "mail", true),
                (crate::sql::secrets::legacy_path(), "postgres", "db", false),
            ] {
                let a = adopt::json_store(v, &path, engine, subsystem, orphans);
                if !a.moved.is_empty() || !a.errors.is_empty() {
                    k2_core::log_debug!("[vault] {}: moved {} errors {:?}", a.store, a.moved.len(), a.errors);
                }
                record_adoption(a);
            }
            match tunnel::adopt(v) {
                Ok(moved) => record_adoption(adopt::Adopted {
                    store: "tunnel.json token".into(),
                    moved: if moved { vec![tunnel::NAME.into()] } else { vec![] },
                    ..Default::default()
                }),
                Err(e) => record_adoption(adopt::Adopted {
                    store: "tunnel.json token".into(),
                    errors: vec![e.hint],
                    ..Default::default()
                }),
            }
            spawn::sweep_at_boot();
            let nd = k2_vault::secret::set_non_dumpable();
            let _ = NON_DUMPABLE.set(nd);
            k2_core::log_debug!(
                "[vault] open: key={} recovery={} non_dumpable={nd}",
                v.key_store().kind().as_str(),
                v.recovery_state().0
            );
        }
        Err(e) => k2_core::log_debug!("[vault] NOT open: {e}"),
    }
    *cell().write() = r;
}

/// Open with `ks`; when `ks` isn't the file store and the vault was made
/// on the file key, move it in place: re-wrap to the new key, anchor, then
/// remove the file key (§7.2 "Upgrades in place").
fn open_with_upgrade(dir: &Path, home: &Path, ks: Arc<dyn KeyStore>) -> Result<Arc<LocalVault>, VaultError> {
    let opts = k2_vault::OpenOptions::default();
    let file = Arc::new(FileKeyStore::new(file_key_dir(home)));
    let is_file = ks.kind() == k2_vault::KeyHome::File;
    let file_kek = if is_file { None } else { file.load().ok().flatten() };
    let vault_exists = dir.join("vault.db").exists();
    if let (Some(fk), true) = (file_kek, vault_exists) {
        match ks.load()? {
            // Same bytes (e.g. the file key sealed as a systemd credential):
            // nothing to re-wrap; the file copy just goes.
            Some(k) if k == fk => {
                let v = LocalVault::open(dir, ks.clone(), opts)?;
                v.reanchor();
                file.remove()?;
                return Ok(Arc::new(v));
            }
            other => {
                // The vault lives on the file key: re-wrap to the better
                // store's key (made now if it has none), verify by
                // reopening, then remove the file key.
                let v = LocalVault::open(dir, file.clone() as Arc<dyn KeyStore>, opts.clone())?;
                let new_kek = match other {
                    Some(k) => k,
                    None => ks.init()?,
                };
                v.rewrap_box(&new_kek, "system:vault", "rekey-store")?;
                drop(v);
                let v2 = LocalVault::open(dir, ks.clone(), opts)?;
                v2.reanchor();
                file.remove()?;
                return Ok(Arc::new(v2));
            }
        }
    }
    Ok(Arc::new(LocalVault::open(dir, ks, opts)?))
}

static ADOPTED: parking_lot::Mutex<Vec<adopt::Adopted>> = parking_lot::Mutex::new(Vec::new());

fn record_adoption(a: adopt::Adopted) {
    let mut g = ADOPTED.lock();
    g.retain(|x| x.store != a.store);
    g.push(a);
}

/// Which old stores moved into the vault (status `adopted`).
pub fn adopted() -> Vec<adopt::Adopted> {
    ADOPTED.lock().clone()
}

/// One vault for the whole test binary (temp dir, fake key store),
/// installed as the daemon's vault. Tests never touch a real key store.
#[cfg(test)]
pub fn ensure_test_vault() -> Arc<LocalVault> {
    static TEST_VAULT: OnceLock<(k2_vault::fsutil::TestDir, Arc<LocalVault>)> = OnceLock::new();
    let (_, v) = TEST_VAULT.get_or_init(|| {
        let dir = k2_vault::fsutil::TestDir::new("daemon-vault");
        let ks: Arc<dyn KeyStore> = Arc::new(k2_vault::keystore::fake::FakeKeyStore::new());
        let v = Arc::new(
            LocalVault::open(dir.path().join("vault"), ks, k2_vault::OpenOptions { kdf: k2_vault::confirm::KdfCost::Floor })
                .expect("open test vault"),
        );
        (dir, v)
    });
    *cell().write() = Ok(v.clone());
    v.clone()
}

// ── workspace helpers ───────────────────────────────────────────────

/// Workspace token (name, handle, path or id) → projects.id.
pub fn workspace_id(token: &str) -> Option<String> {
    let t = token.trim();
    if t.is_empty() {
        return None;
    }
    let path = crate::workspace_msg::resolve_workspace(t)?;
    crate::canonical_session::lookup_project_id(&path)
}

/// projects.id → the handle people see (`sales`).
pub fn workspace_label(id: &str) -> String {
    k2_core::workspace_session_handles::workspace_address_name_shared(id).unwrap_or_else(|_| id.to_string())
}

/// projects.id → workspace folder.
pub fn workspace_path(id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT path FROM projects WHERE id = ?1", rusqlite::params![id], |r| r.get::<_, String>(0)).ok()
}

/// A `k2v://` reference as people write it (workspace handle, not id).
pub fn display_ref(meta: &k2_vault::SecretMeta) -> Option<String> {
    use k2_vault::ScopeKind;
    if meta.namespace != k2_vault::Namespace::User {
        return None;
    }
    let scope_ref = match meta.scope.kind {
        ScopeKind::Workspace => workspace_label(&meta.scope.id),
        ScopeKind::Server => String::new(),
        _ => meta.scope.id.clone(),
    };
    Some(
        k2_vault::refs::VaultRef { scope_kind: meta.scope.kind, scope_ref, name: meta.name.clone(), version: None }
            .render(),
    )
}

/// Parse a `k2v://` reference into a selector (workspace names → ids).
pub fn selector_for_ref(s: &str) -> Result<k2_vault::Selector, VaultError> {
    use k2_vault::{Scope, ScopeKind, Selector};
    let r = k2_vault::refs::VaultRef::parse(s)?;
    let scope = match r.scope_kind {
        ScopeKind::Server => Scope::server(),
        ScopeKind::Workspace => {
            let id = workspace_id(&r.scope_ref)
                .ok_or_else(|| VaultError::not_found(format!("no workspace named {}", r.scope_ref)))?;
            Scope::workspace(id)
        }
        ScopeKind::Session => Scope::session(r.scope_ref.clone()),
        _ => return Err(VaultError::bad_request("scope_reserved", "that scope isn't available yet")),
    };
    Ok(Selector::user(scope, r.name))
}

/// The redactor for K2's own surfaces (KV33). Empty when the vault is
/// closed.
pub fn redactor() -> Arc<k2_vault::redact::Redactor> {
    match get() {
        Ok(v) => v.redactor(),
        Err(_) => Arc::new(k2_vault::redact::Redactor::empty()),
    }
}

/// Redact a string for a K2 surface.
pub fn redact_str(s: &str) -> String {
    let r = redactor();
    if r.is_empty() {
        s.to_string()
    } else {
        r.redact_str(s)
    }
}

/// `auth-audit.jsonl` line for reveal-class actions and their denials
/// (KV32), so `k2 users audit` / Settings → Logs show them.
pub fn auth_audit(event: &str, user: &str, outcome: &str, ingress: &str) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        event,
        user,
        outcome.to_string(),
        ingress,
        "-",
        "vault",
    ));
}

#[cfg(test)]
mod tests {
    /// VS14: only the serve path opens the KeyStore. Every call site of
    /// `LocalVault::open` / `load_or_init` in the daemon is in this file.
    #[test]
    fn only_boot_opens_the_key_store() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = vec![];
        let mut stack = vec![root.clone()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).expect("read src dir") {
                let p = e.expect("entry").path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if p.extension().and_then(|x| x.to_str()) != Some("rs") {
                    continue;
                }
                let rel = p.strip_prefix(&root).expect("rel").to_string_lossy().to_string();
                if rel == "vault/mod.rs" || rel.ends_with("tests.rs") || rel.ends_with("/tests") {
                    continue;
                }
                let src = std::fs::read_to_string(&p).expect("read");
                // Test modules may open their own temp vaults.
                let prod = src.split("#[cfg(test)]").next().unwrap_or("");
                for needle in ["LocalVault::open(", ".load_or_init(", "LocalVault::adopt_with_identity("] {
                    if prod.contains(needle) {
                        offenders.push(format!("{rel}: {needle}"));
                    }
                }
            }
        }
        assert!(offenders.is_empty(), "only vault/mod.rs may open the key store: {offenders:?}");
    }
}
