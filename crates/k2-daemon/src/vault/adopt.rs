//! First-boot moves of old secret stores into the vault (§10.1, §10.4,
//! VS9). Every move follows the same order, idempotent on restart:
//!
//! 1. `put` keyed by (namespace, scope, name), `idempotent` (a retry with
//!    the same value adds no version);
//! 2. read the value back from the vault and compare byte for byte;
//! 3. only then overwrite and remove the old copy (best effort on
//!    copy-on-write disks; status says "removed", not "shredded").
//!
//! Any mismatch keeps the old file and is reported in status.

use std::path::{Path, PathBuf};

use k2_vault::{
    Actor, Delivery, Kind, LocalVault, Namespace, PutMode, PutRequest, Requester, Scope, SecretBytes, SecretStore,
    Selector, Use, VaultError,
};

/// What one adoption pass did.
#[derive(Debug, Default, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Adopted {
    pub store: String,
    pub moved: Vec<String>,
    pub kept: Vec<String>,
    pub errors: Vec<String>,
}

/// Overwrite then unlink a file holding a moved value.
pub fn remove_old_copy(path: &Path) -> std::io::Result<()> {
    use std::io::Write;
    if let Ok(meta) = std::fs::metadata(path) {
        if let Ok(mut f) = std::fs::OpenOptions::new().write(true).open(path) {
            let zeros = vec![0u8; meta.len() as usize];
            let _ = f.write_all(&zeros);
            let _ = f.sync_all();
        }
    }
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Put `value` and verify it reads back identically.
pub fn put_and_verify(vault: &LocalVault, req: PutRequest, value: &[u8], actor: &Actor) -> Result<String, VaultError> {
    let sel = Selector::Name { namespace: req.namespace, scope: req.scope.clone(), name: req.name.clone() };
    let out = vault.put(req, SecretBytes::from_slice(value), actor)?;
    let back = vault.resolve(&sel, Use::System, &Requester::System("adopt".into()))?;
    if back.value.expose() != value {
        return Err(VaultError::internal("read-back differs"));
    }
    Ok(out.meta.id)
}

/// `~/.k2/thread-secrets/<workspace id>/<NAME>` → workspace-scope `user`
/// secrets: person-given, no sites yet, delivered as a file (VS24),
/// ceiling env. The workspace gets one inbox note.
pub fn thread_secrets(vault: &LocalVault, root: &Path) -> Adopted {
    let mut out = Adopted { store: "thread-secrets".into(), ..Default::default() };
    let Ok(rd) = std::fs::read_dir(root) else { return out };
    for ws in rd.flatten() {
        let ws_path = ws.path();
        if !ws_path.is_dir() {
            continue;
        }
        let ws_id = ws.file_name().to_string_lossy().to_string();
        let mut names = vec![];
        let Ok(files) = std::fs::read_dir(&ws_path) else { continue };
        for f in files.flatten() {
            let p: PathBuf = f.path();
            if !p.is_file() {
                continue;
            }
            let name = f.file_name().to_string_lossy().to_string();
            let label = format!("{ws_id}/{name}");
            let bytes = match std::fs::read(&p) {
                Ok(b) => b,
                Err(e) => {
                    out.errors.push(format!("{label}: {}", e.kind()));
                    continue;
                }
            };
            if bytes.is_empty() {
                out.kept.push(label);
                continue;
            }
            let mut req = PutRequest::new(Namespace::User, Scope::workspace(ws_id.clone()), name.clone(), Kind::Generic);
            req.default_delivery = Some(Delivery::File);
            req.delivery_ceiling = Some(Delivery::Env);
            req.origin = "adopt:thread-secrets".into();
            req.description = "moved from a Thread secret card (before the vault)".into();
            req.mode = PutMode::Upsert;
            req.idempotent = true;
            req.human_given_override = Some(true);
            match put_and_verify(vault, req, &bytes, &Actor::System("adopt".into())) {
                Ok(_) => match remove_old_copy(&p) {
                    Ok(()) => {
                        out.moved.push(label);
                        names.push(name);
                    }
                    Err(e) => out.errors.push(format!("{label}: moved but not removed ({})", e.kind())),
                },
                Err(e) => {
                    out.kept.push(label.clone());
                    out.errors.push(format!("{label}: {}", e.code));
                }
            }
        }
        let _ = std::fs::remove_dir(&ws_path);
        if !names.is_empty() {
            if let Some(path) = super::workspace_path(&ws_id) {
                let label = super::workspace_label(&ws_id);
                let lines: Vec<String> = names
                    .iter()
                    .map(|n| format!("- {n} → k2v://ws/{label}/{n} — read it at $K2_SECRETS_DIR/{n} in new sessions"))
                    .collect();
                let body = format!(
                    "The secrets people gave you through Thread cards now live in the K2 vault (encrypted).\n\n{}\n\nThey have no sites yet, so `k2 vault fetch` refuses them until the owner adds one (k2 vault hosts NAME --add <host>).\nMore: k2 study vault",
                    lines.join("\n")
                );
                let _ = k2_core::inbox::compose(Path::new(&path), "Your Thread secrets moved into the vault", &body, Some("normal"), Some("vault"), Some("k2"));
            }
        }
    }
    let _ = std::fs::remove_dir(root);
    out
}

/// A flat `ref → secret` JSON store (`mail-secrets.json`,
/// `db-secrets.json`) → `system` secrets in scope `engine:<engine>`, same
/// names. All keys move and verify before the file goes; any failure
/// keeps the whole file. `map_account_orphans` (mail, KV13): an orphaned
/// `mailsec_account-<row>_<hex>` also lands as `account-<row>` when no
/// exact entry exists.
pub fn json_store(vault: &LocalVault, path: &Path, engine: &str, subsystem: &str, map_account_orphans: bool) -> Adopted {
    let mut out = Adopted { store: path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default(), ..Default::default() };
    if !path.exists() {
        return out;
    }
    let map = match crate::mail::secrets::FileSecretStore::legacy_map(path) {
        Ok(m) => m,
        Err(e) => {
            out.errors.push(format!("read: {e}"));
            return out;
        }
    };
    let mut entries: Vec<(String, String)> = map
        .iter()
        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
        .collect();
    if map_account_orphans {
        let exact: std::collections::HashSet<String> = entries.iter().map(|(k, _)| k.clone()).collect();
        let mut extra: Vec<(String, String)> = vec![];
        for (k, v) in &entries {
            if let Some(rest) = k.strip_prefix("mailsec_account-") {
                if let Some((row, _hex)) = rest.rsplit_once('_') {
                    let key = format!("account-{row}");
                    if !exact.contains(&key) && !extra.iter().any(|(e, _)| e == &key) {
                        extra.push((key, v.clone()));
                    }
                }
            }
        }
        entries.extend(extra);
    }
    let actor = Actor::System(subsystem.to_string());
    let mut all_ok = true;
    for (k, v) in &entries {
        if v.is_empty() {
            continue;
        }
        let mut req = PutRequest::new(Namespace::System, Scope::engine(engine), k.clone(), Kind::Password);
        req.default_delivery = Some(Delivery::None);
        req.delivery_ceiling = Some(Delivery::Env);
        req.origin = format!("adopt:{}", out.store);
        req.mode = PutMode::Upsert;
        req.idempotent = true;
        match put_and_verify(vault, req, v.as_bytes(), &actor) {
            Ok(_) => out.moved.push(k.clone()),
            Err(e) => {
                all_ok = false;
                out.errors.push(format!("{k}: {}", e.code));
            }
        }
    }
    if all_ok {
        match remove_old_copy(path) {
            Ok(()) => {
                let marker = path.with_extension("json.moved");
                let _ = k2_vault::fsutil::write_private_atomic(
                    &marker,
                    format!("moved into the K2 vault at {}\n", chrono::Utc::now().to_rfc3339()).as_bytes(),
                );
            }
            Err(e) => out.errors.push(format!("moved but not removed ({})", e.kind())),
        }
    } else {
        out.kept = out.moved.clone();
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn mail_store_moves_with_orphan_mapping_and_db_store_moves() {
        let t = TestDir::new("adopt-json");
        let ks: Arc<dyn KeyStore> = Arc::new(FakeKeyStore::new());
        let v = LocalVault::open(t.path().join("vault"), ks, OpenOptions { kdf: KdfCost::Floor }).expect("open");
        let p = t.path().join("mail-secrets.json");
        std::fs::write(
            &p,
            r#"{"mailsec_admin_0123456789ab":"admin-pw-1","mailsec_account-77_aaaaaaaaaaaa":"acct-77-pw","account-5":"acct-5-pw","ext-inbox-9":"oauth-tok"}"#,
        )
        .expect("write");
        let r = json_store(&v, &p, "stalwart", "mail", true);
        assert!(r.errors.is_empty(), "{r:?}");
        assert!(!p.exists(), "old file removed");
        assert!(t.path().join("mail-secrets.json.moved").exists());
        let get = |name: &str| {
            v.resolve(&Selector::system(Scope::engine("stalwart"), name), Use::System, &Requester::System("mail".into()))
                .expect(name)
                .value
                .expose()
                .to_vec()
        };
        assert_eq!(get("mailsec_admin_0123456789ab"), b"admin-pw-1");
        assert_eq!(get("account-77"), b"acct-77-pw", "orphan mapped to the exact key");
        assert_eq!(get("account-5"), b"acct-5-pw");
        assert_eq!(get("ext-inbox-9"), b"oauth-tok");
        // A broken file stays put.
        let bad = t.path().join("db-secrets.json");
        std::fs::write(&bad, "not json").expect("write");
        let r2 = json_store(&v, &bad, "postgres", "db", false);
        assert!(!r2.errors.is_empty());
        assert!(bad.exists(), "kept on error");
    }

    use std::sync::Arc;

    use k2_vault::confirm::KdfCost;
    use k2_vault::fsutil::TestDir;
    use k2_vault::keystore::fake::FakeKeyStore;
    use k2_vault::{KeyStore, OpenOptions, ScopeKind};

    use super::*;

    #[test]
    fn thread_secrets_move_byte_for_byte_then_go() {
        let t = TestDir::new("adopt-thread");
        let ks: Arc<dyn KeyStore> = Arc::new(FakeKeyStore::new());
        let v = LocalVault::open(t.path().join("vault"), ks, OpenOptions { kdf: KdfCost::Floor }).expect("open");
        let root = t.path().join("thread-secrets");
        let ws = "0b5e8c1e-1f2a-4c1d-9b5e-123456789abc";
        std::fs::create_dir_all(root.join(ws)).expect("dir");
        std::fs::write(root.join(ws).join("API_TOKEN"), b"tok-ADOPT-SENTINEL-123").expect("write");
        let r = thread_secrets(&v, &root);
        assert_eq!(r.moved, vec![format!("{ws}/API_TOKEN")], "{r:?}");
        assert!(r.errors.is_empty(), "{r:?}");
        assert!(!root.exists(), "old folder removed");
        let m = v.meta(&Selector::user(Scope::workspace(ws), "API_TOKEN"), &Actor::Owner).expect("meta");
        assert!(m.human_given);
        assert_eq!(m.default_delivery, Delivery::File);
        assert_eq!(m.delivery_ceiling, Delivery::Env);
        assert_eq!(m.scope.kind, ScopeKind::Workspace);
        assert!(m.hosts.is_empty());
        // Restart: idempotent (nothing left to move, no new version).
        std::fs::create_dir_all(root.join(ws)).expect("dir");
        std::fs::write(root.join(ws).join("API_TOKEN"), b"tok-ADOPT-SENTINEL-123").expect("write");
        let r2 = thread_secrets(&v, &root);
        assert_eq!(r2.moved.len(), 1);
        let m2 = v.meta(&Selector::user(Scope::workspace(ws), "API_TOKEN"), &Actor::Owner).expect("meta");
        assert_eq!(m2.latest_version, 1, "a retry with the same value adds no version");
    }
}
