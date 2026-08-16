//! Phase 1 (prd-wiki-public-chat-api-loopback-v1) — daemon-held API key for
//! public wiki chat.
//!
//! When the owner enables public wiki chat on a workspace, the daemon
//! auto-mints (or reuses) a workspace-scoped API key with **host_sessions
//! only** (no canonical message, no sandboxes) and stores the raw `k2sk_`
//! secret under `~/.k2/wiki-public-chat/` (mode 0600). The secret is NEVER
//! returned to the renderer or CLI after mint — Phase 2's same-origin
//! `/api/chat` proxy loads it in-process via [`load_raw_key`].
//!
//! Coordination contract with Phase 2:
//! - Setting column: `projects.wiki_public_chat` (`wikiPublicChat` in JSON)
//! - ServeStatus fields: `publicChatEnabled`, `publicChatReady`, `publicChatError`
//! - Secret dir: [`store_dir`] (`~/.k2/wiki-public-chat/`)
//! - Key label prefix: [`KEY_LABEL_PREFIX`]

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::api_keys::{
    create_api_key, resolve_api_key, revoke_api_key, ApiCapabilities, API_KEY_PREFIX,
};
use crate::paths::k2_home;

/// Directory under `~/.k2` holding per-workspace wiki-chat key files.
pub const STORE_REL: &str = "wiki-public-chat";

/// Label prefix on auto-minted keys (`wiki-public-chat:<basename>`).
pub const KEY_LABEL_PREFIX: &str = "wiki-public-chat:";

/// Non-secret metadata for a workspace chat key (safe to log / status).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChatKeyMeta {
    pub key_id: String,
    pub workspace_path: String,
    pub label: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChatKeyFile {
    workspace_path: String,
    key_id: String,
    /// Raw `k2sk_…` — daemon-only. Never surface outside this module except
    /// via [`load_raw_key`] for the Phase 2 proxy.
    key: String,
    label: String,
    created_at: i64,
}

/// `~/.k2/wiki-public-chat/`.
pub fn store_dir() -> PathBuf {
    k2_home().join(STORE_REL)
}

fn path_key_hash(workspace_path: &str) -> String {
    let canonical = Path::new(workspace_path)
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(workspace_path));
    let hex = format!("{:x}", Sha256::digest(canonical.to_string_lossy().as_bytes()));
    // Short stable id for the filename (full hash is fine too; 32 hex keeps
    // listings readable).
    hex[..32].to_string()
}

fn secret_path(workspace_path: &str) -> PathBuf {
    store_dir().join(format!("{}.json", path_key_hash(workspace_path)))
}

/// Folder basename of `workspace_path` — the primary `/v1/w/<slug>/…` address
/// and the default grant slug (matches `resolve_workspace_slug` basename).
pub fn primary_slug(workspace_path: &str) -> Option<String> {
    let name = Path::new(workspace_path.trim_end_matches('/'))
        .file_name()?
        .to_string_lossy()
        .into_owned();
    if name.is_empty() || name.contains('/') || name == ".." {
        return None;
    }
    Some(name)
}

/// Slugs to put on the key grant: folder basename + registered display name
/// when different (so either URL form authorizes).
fn grant_slugs(workspace_path: &str) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(base) = primary_slug(workspace_path) {
        out.push(base);
    }
    // Registered display name (exact slug match on /v1).
    let name: Option<String> = {
        let db = crate::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT name FROM projects WHERE path = ?1",
            rusqlite::params![workspace_path],
            |r| r.get::<_, String>(0),
        )
        .ok()
    };
    if let Some(n) = name {
        let t = n.trim().to_string();
        if !t.is_empty() && !out.iter().any(|s| s == &t) {
            // Only add if it is a single path segment (no `/`) — grant is
            // exact-match slugs only.
            if !t.contains('/') && t != ".." {
                out.push(t);
            }
        }
    }
    let extras: Vec<String> = {
        let db = crate::db::shared();
        let conn = db.lock();
        crate::workspace::handle::slug_candidates_for_path(&conn, workspace_path)
    };
    for s in extras {
        if !s.contains('/') && s != ".." && !out.iter().any(|x| x == &s) {
            out.push(s);
        }
    }
    out
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn write_secret_file(path: &Path, file: &ChatKeyFile) -> Result<(), String> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|e| format!("create wiki-public-chat dir: {e}"))?;
    let bytes = serde_json::to_vec_pretty(file).map_err(|e| format!("serialize key file: {e}"))?;
    crate::fs_atomic::atomic_write(path, &bytes).map_err(|e| format!("write key file: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
        let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

fn read_secret_file(path: &Path) -> Option<ChatKeyFile> {
    let raw = fs::read_to_string(path).ok()?;
    let file: ChatKeyFile = serde_json::from_str(&raw).ok()?;
    if !file.key.starts_with(API_KEY_PREFIX) || file.key_id.is_empty() {
        return None;
    }
    Some(file)
}

/// Does the stored secret still resolve to a live principal with host_sessions
/// and a grant that covers at least one of our workspace slugs?
fn stored_key_still_valid(file: &ChatKeyFile, workspace_path: &str) -> bool {
    let Some(principal) = resolve_api_key(&file.key) else {
        return false;
    };
    if principal.id != file.key_id {
        return false;
    }
    if !principal.capabilities.host_sessions {
        return false;
    }
    let slugs = grant_slugs(workspace_path);
    if slugs.is_empty() {
        return false;
    }
    slugs.iter().any(|s| principal.authorizes_workspace(s))
}

/// Ensure a daemon-held host_sessions-only API key exists for this workspace.
/// Reuses a valid stored key; otherwise mints a new one and writes the secret
/// file. Returns non-secret metadata only.
///
/// The raw key is NEVER returned. Callers that need it (Phase 2 proxy only)
/// use [`load_raw_key`].
pub fn ensure_chat_key(workspace_path: &str) -> Result<ChatKeyMeta, String> {
    let path = secret_path(workspace_path);
    if let Some(existing) = read_secret_file(&path) {
        if stored_key_still_valid(&existing, workspace_path) {
            return Ok(ChatKeyMeta {
                key_id: existing.key_id,
                workspace_path: existing.workspace_path,
                label: existing.label,
                created_at: existing.created_at,
            });
        }
        // Stale/revoked — drop file and re-mint (best-effort revoke old id).
        let _ = revoke_api_key(&existing.key_id);
        let _ = fs::remove_file(&path);
    }

    let slugs = grant_slugs(workspace_path);
    if slugs.is_empty() {
        return Err(
            "cannot mint wiki public chat key: workspace path has no usable slug".into(),
        );
    }
    let grant = serde_json::to_string(&slugs).map_err(|e| format!("grant json: {e}"))?;
    let basename = primary_slug(workspace_path).unwrap_or_else(|| "workspace".into());
    let label = format!("{KEY_LABEL_PREFIX}{basename}");
    let caps = ApiCapabilities::new_key_default(); // host_sessions only

    let (key_id, raw) = create_api_key(
        &label,
        None, // no BYO LLM on the public chat proxy key
        Some(&grant),
        None,
        None,
        Some(caps),
    )?;

    let created_at = now_secs();
    let file = ChatKeyFile {
        workspace_path: workspace_path.to_string(),
        key_id: key_id.clone(),
        key: raw,
        label: label.clone(),
        created_at,
    };
    // Write secret; if write fails, revoke the orphaned key so we don't leak
    // unrecoverable secrets.
    if let Err(e) = write_secret_file(&path, &file) {
        let _ = revoke_api_key(&key_id);
        return Err(e);
    }

    crate::log_debug!(
        "[wiki-public-chat] ensured key {key_id} for workspace (label {label:?}); secret stored under ~/.k2/{STORE_REL}/"
    );

    Ok(ChatKeyMeta {
        key_id,
        workspace_path: workspace_path.to_string(),
        label,
        created_at,
    })
}

/// Load the raw `k2sk_` for Phase 2's loopback proxy. Returns `None` if no
/// file, corrupt file, or the key no longer resolves. NEVER log the result.
pub fn load_raw_key(workspace_path: &str) -> Option<String> {
    let file = read_secret_file(&secret_path(workspace_path))?;
    if !stored_key_still_valid(&file, workspace_path) {
        return None;
    }
    Some(file.key)
}

/// Non-secret: is a usable chat key on file for this workspace?
pub fn chat_key_ready(workspace_path: &str) -> bool {
    load_raw_key(workspace_path).is_some()
}

/// Non-secret metadata if a key file exists (does not re-mint).
pub fn chat_key_meta(workspace_path: &str) -> Option<ChatKeyMeta> {
    let file = read_secret_file(&secret_path(workspace_path))?;
    Some(ChatKeyMeta {
        key_id: file.key_id,
        workspace_path: file.workspace_path,
        label: file.label,
        created_at: file.created_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};
    use uuid::Uuid;

    /// Serialize HOME-mutating tests so they don't stomp each other.
    fn home_lock() -> std::sync::MutexGuard<'static, ()> {
        static L: OnceLock<Mutex<()>> = OnceLock::new();
        L.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    struct HomeGuard {
        home: PathBuf,
        prev: Option<std::ffi::OsString>,
    }

    impl HomeGuard {
        fn new() -> Self {
            let home = std::env::temp_dir().join(format!(
                "k2-wiki-chat-home-{}-{}",
                std::process::id(),
                Uuid::new_v4()
            ));
            fs::create_dir_all(&home).expect("create temp HOME");
            let prev = std::env::var_os("HOME");
            std::env::set_var("HOME", &home);
            Self { home, prev }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            let _ = fs::remove_dir_all(&self.home);
        }
    }

    fn unique_path(label: &str) -> String {
        format!(
            "/tmp/k2-wiki-chat-{}-{}-{}",
            label,
            std::process::id(),
            Uuid::new_v4()
        )
    }

    fn insert_project(path: &str, name: &str) {
        let db = crate::db::shared();
        let conn = db.lock();
        let id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, name, path],
        )
        .expect("insert project");
    }

    #[test]
    fn ensure_mints_host_sessions_only_and_load_round_trips() {
        let _g = home_lock();
        let _home = HomeGuard::new();

        let path = unique_path("mint");
        // Real dir so canonicalize is stable for the secret filename.
        fs::create_dir_all(&path).expect("mkdir");
        insert_project(&path, "Wiki Chat Mint");

        assert!(!chat_key_ready(&path), "no key before ensure");

        let meta = ensure_chat_key(&path).expect("ensure");
        assert!(!meta.key_id.is_empty());
        assert!(meta.label.starts_with(KEY_LABEL_PREFIX));
        assert!(chat_key_ready(&path));

        let raw = load_raw_key(&path).expect("load raw");
        assert!(raw.starts_with(API_KEY_PREFIX));
        // Status/list surfaces never get the raw key — only this loader.
        assert!(raw.starts_with(API_KEY_PREFIX));
        let principal = resolve_api_key(&raw).expect("resolve");
        assert_eq!(principal.id, meta.key_id);
        assert!(principal.capabilities.host_sessions);
        assert!(!principal.capabilities.canonical_message);
        assert!(!principal.capabilities.sandboxes);

        // Grant covers basename.
        let base = primary_slug(&path).expect("basename");
        assert!(
            principal.authorizes_workspace(&base),
            "grant must authorize folder basename"
        );

        // Second ensure reuses (same key id).
        let meta2 = ensure_chat_key(&path).expect("reuse");
        assert_eq!(meta2.key_id, meta.key_id);

        // Secret file is under the temp HOME only.
        assert!(
            store_dir().starts_with(&_home.home),
            "secrets must land under test HOME"
        );

        let _ = fs::remove_dir_all(&path);
    }

    #[test]
    fn ensure_remints_after_revoke() {
        let _g = home_lock();
        let _home = HomeGuard::new();

        let path = unique_path("remint");
        fs::create_dir_all(&path).expect("mkdir");
        insert_project(&path, "Remint");

        let meta1 = ensure_chat_key(&path).expect("first");
        revoke_api_key(&meta1.key_id).expect("revoke");
        assert!(!chat_key_ready(&path), "revoked key is not ready");

        let meta2 = ensure_chat_key(&path).expect("remint");
        assert_ne!(meta1.key_id, meta2.key_id);
        assert!(chat_key_ready(&path));

        let _ = fs::remove_dir_all(&path);
    }
}
