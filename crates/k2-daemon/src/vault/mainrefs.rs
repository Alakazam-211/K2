//! Main-DB plaintext moves into the vault (§10.4, migration 0148, KV22,
//! KV36, VS9):
//! - `projects.anthropic_api_key` → `user` secret `ANTHROPIC_API_KEY` in
//!   that workspace (sites: api.anthropic.com);
//! - `api_keys.anthropic_api_key` → server-scope `user` secret
//!   `API_KEY_<id8>_LLM`, bound to the key's `base_url` host when set
//!   (KV36), else its provider's API host;
//! - preset env values with sensitive names → server-scope secrets,
//!   replaced in place by `k2v://server/<name>`.
//!
//! Order per value: vault put + read back, then one main-DB transaction
//! sets the reference and nulls the plaintext. When anything moved, the
//! main DB is rewritten (secure_delete, WAL truncate, VACUUM) before
//! routes open.

use std::sync::Arc;

use k2_vault::{Actor, Delivery, Kind, LocalVault, Namespace, PutMode, PutRequest, Scope, VaultError};

use super::adopt::{put_and_verify, Adopted};

fn provider_host(provider: Option<&str>, base_url: Option<&str>) -> Vec<String> {
    if let Some(u) = base_url.filter(|u| !u.trim().is_empty()) {
        if let Ok(p) = url::Url::parse(u.trim()) {
            if let Some(h) = p.host_str() {
                let scheme = if p.scheme() == "http" { "http" } else { "https" };
                let port = p.port().map(|x| format!(":{x}")).unwrap_or_default();
                return vec![format!("{scheme}://{h}{port}")];
            }
        }
    }
    let h = match provider.unwrap_or("anthropic") {
        "openai" | "codex" => "api.openai.com",
        "xai" | "grok" => "api.x.ai",
        "gemini" | "google" => "generativelanguage.googleapis.com",
        _ => "api.anthropic.com",
    };
    vec![h.to_string()]
}

fn put_key(vault: &LocalVault, scope: Scope, name: &str, value: &str, hosts: Vec<String>, origin: &str) -> Result<(), VaultError> {
    let mut r = PutRequest::new(Namespace::User, scope, name, Kind::ApiKey);
    r.hosts = hosts;
    r.default_delivery = Some(Delivery::Fetch);
    r.delivery_ceiling = Some(Delivery::Env);
    r.origin = origin.into();
    r.mode = PutMode::Upsert;
    r.idempotent = true;
    r.human_given_override = Some(true);
    r.description = "moved from K2's settings when the vault arrived".into();
    put_and_verify(vault, r, value.as_bytes(), &Actor::System("adopt".into())).map(|_| ())
}

/// The holder k2-core calls for new keys and to read references. It uses
/// whichever vault is open (a restore swaps it).
pub struct Holder;

fn open_vault() -> Result<Arc<LocalVault>, String> {
    super::get().map_err(|e| format!("vault: {}", e.code))
}

impl k2_core::secret_refs::SecretRefs for Holder {
    fn store_workspace_key(&self, project_id: &str, key: &str) -> Result<String, String> {
        let v = open_vault()?;
        put_key(&v, Scope::workspace(project_id), "ANTHROPIC_API_KEY", key, vec!["api.anthropic.com".into()], "ui")
            .map_err(|e| format!("vault: {}", e.code))?;
        Ok(format!("k2v://ws/{project_id}/ANTHROPIC_API_KEY"))
    }

    fn store_api_key_credential(&self, api_key_id: &str, key: &str, provider: Option<&str>, base_url: Option<&str>) -> Result<String, String> {
        let short: String = api_key_id.chars().filter(|c| c.is_ascii_alphanumeric()).take(8).collect();
        let name = format!("API_KEY_{short}_LLM");
        let v = open_vault()?;
        put_key(&v, Scope::server(), &name, key, provider_host(provider, base_url), "cli-owner")
            .map_err(|e| format!("vault: {}", e.code))?;
        Ok(format!("k2v://server/{name}"))
    }

    fn resolve(&self, sref: &str) -> Option<String> {
        let r = k2_vault::refs::VaultRef::parse(sref).ok()?;
        let scope = match r.scope_kind {
            k2_vault::ScopeKind::Server => Scope::server(),
            k2_vault::ScopeKind::Workspace => Scope::workspace(r.scope_ref.clone()),
            _ => return None,
        };
        use k2_vault::SecretStore as _;
        open_vault()
            .ok()?
            .resolve(
                &k2_vault::Selector::user(scope, r.name),
                k2_vault::Use::System,
                &k2_vault::Requester::System("llm-staging".into()),
            )
            .ok()
            .and_then(|v| String::from_utf8(v.value.expose().to_vec()).ok())
    }

    fn delete(&self, sref: &str) {
        if let Ok(r) = k2_vault::refs::VaultRef::parse(sref) {
            let scope = match r.scope_kind {
                k2_vault::ScopeKind::Server => Scope::server(),
                k2_vault::ScopeKind::Workspace => Scope::workspace(r.scope_ref.clone()),
                _ => return,
            };
            use k2_vault::SecretStore as _;
            let Ok(v) = open_vault() else { return };
            let _ = v.destroy(&k2_vault::Selector::user(scope, r.name), &Actor::System("settings".into()));
        }
    }
}

/// Move every plaintext value; returns what moved.
pub fn adopt(vault: &Arc<LocalVault>) -> Adopted {
    let mut out = Adopted { store: "main DB keys (0148)".into(), ..Default::default() };
    let db = k2_core::db::shared();
    // Workspace keys.
    let rows: Vec<(String, String)> = {
        let conn = db.lock();
        let mut st = match conn.prepare(
            "SELECT id, anthropic_api_key FROM projects WHERE anthropic_api_key IS NOT NULL AND TRIM(anthropic_api_key) <> ''",
        ) {
            Ok(s) => s,
            Err(e) => {
                out.errors.push(format!("projects: {e}"));
                return out;
            }
        };
        let it = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)));
        match it {
            Ok(i) => i.flatten().collect(),
            Err(_) => vec![],
        }
    };
    for (id, key) in rows {
        let r = put_key(vault, Scope::workspace(id.clone()), "ANTHROPIC_API_KEY", key.trim(), vec!["api.anthropic.com".into()], "adopt:projects")
            .and_then(|_| {
                let conn = db.lock();
                conn.execute(
                    "UPDATE projects SET anthropic_secret_ref = ?1, anthropic_api_key = NULL WHERE id = ?2",
                    rusqlite::params![format!("k2v://ws/{id}/ANTHROPIC_API_KEY"), id],
                )
                .map(|_| ())
                .map_err(VaultError::from)
            });
        match r {
            Ok(()) => out.moved.push(format!("projects/{id}")),
            Err(e) => out.errors.push(format!("projects/{id}: {}", e.code)),
        }
    }
    // API-key BYO credentials.
    let rows: Vec<(String, String, Option<String>, Option<String>)> = {
        let conn = db.lock();
        let st = conn.prepare(
            "SELECT id, anthropic_api_key, provider, base_url FROM api_keys WHERE anthropic_api_key IS NOT NULL AND TRIM(anthropic_api_key) <> ''",
        );
        match st {
            Ok(mut s) => match s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))) {
                Ok(i) => i.flatten().collect(),
                Err(_) => vec![],
            },
            Err(_) => vec![],
        }
    };
    let holder = Holder;
    for (id, key, provider, base_url) in rows {
        use k2_core::secret_refs::SecretRefs as _;
        let r = holder
            .store_api_key_credential(&id, key.trim(), provider.as_deref(), base_url.as_deref())
            .map_err(VaultError::internal)
            .and_then(|sref| {
                let conn = db.lock();
                conn.execute(
                    "UPDATE api_keys SET anthropic_secret_ref = ?1, anthropic_api_key = NULL WHERE id = ?2",
                    rusqlite::params![sref, id],
                )
                .map(|_| ())
                .map_err(VaultError::from)
            });
        match r {
            Ok(()) => out.moved.push(format!("api_keys/{id}")),
            Err(e) => out.errors.push(format!("api_keys/{id}: {}", e.code)),
        }
    }
    // Preset env values with sensitive names.
    let presets: Vec<(String, String)> = {
        let conn = db.lock();
        let st = conn.prepare("SELECT id, env FROM agent_presets WHERE env IS NOT NULL AND TRIM(env) <> ''");
        match st {
            Ok(mut s) => match s.query_map([], |r| Ok((r.get(0)?, r.get(1)?))) {
                Ok(i) => i.flatten().collect(),
                Err(_) => vec![],
            },
            Err(_) => vec![],
        }
    };
    for (pid, env_json) in presets {
        let Ok(serde_json::Value::Object(mut map)) = serde_json::from_str::<serde_json::Value>(&env_json) else { continue };
        let mut changed = false;
        let keys: Vec<String> = map.keys().cloned().collect();
        for k in keys {
            let Some(v) = map.get(&k).and_then(|v| v.as_str()).map(str::to_string) else { continue };
            if !k2_vault::delivery::is_sensitive_name(&k) || v.trim().is_empty() || k2_vault::refs::VaultRef::is_ref(&v) {
                continue;
            }
            let short: String = pid.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(16).collect();
            let name: String = format!("preset-{short}-{k}").chars().take(64).collect();
            let mut r = PutRequest::new(Namespace::User, Scope::server(), name.clone(), Kind::Generic);
            r.default_delivery = Some(Delivery::Env);
            r.delivery_ceiling = Some(Delivery::Env);
            r.origin = "adopt:agent_presets".into();
            r.mode = PutMode::Upsert;
            r.idempotent = true;
            r.human_given_override = Some(true);
            match put_and_verify(vault, r, v.as_bytes(), &Actor::System("adopt".into())) {
                Ok(_) => {
                    map.insert(k.clone(), serde_json::Value::String(format!("k2v://server/{name}")));
                    changed = true;
                    out.moved.push(format!("agent_presets/{pid}/{k}"));
                }
                Err(e) => out.errors.push(format!("agent_presets/{pid}/{k}: {}", e.code)),
            }
        }
        if changed {
            let conn = db.lock();
            if let Err(e) = conn.execute(
                "UPDATE agent_presets SET env = ?1 WHERE id = ?2",
                rusqlite::params![serde_json::Value::Object(map).to_string(), pid],
            ) {
                out.errors.push(format!("agent_presets/{pid}: {e}"));
            }
        }
    }
    if !out.moved.is_empty() {
        // VS9: nulling a column leaves old bytes in free pages and the WAL.
        let conn = db.lock();
        let _ = conn.pragma_update(None, "secure_delete", "ON");
        let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
        if let Err(e) = conn.execute_batch("VACUUM;") {
            out.errors.push(format!("vacuum: {e}"));
        }
        let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_host_binds_the_key() {
        assert_eq!(provider_host(None, Some("https://openrouter.ai/api/v1")), vec!["https://openrouter.ai".to_string()]);
        assert_eq!(provider_host(Some("openai"), None), vec!["api.openai.com".to_string()]);
        assert_eq!(provider_host(None, None), vec!["api.anthropic.com".to_string()]);
    }

    /// VS9: after the move the sentinel is nowhere in the main DB.
    #[test]
    fn workspace_key_moves_and_the_main_db_keeps_only_a_reference() {
        let v = super::super::ensure_test_vault();
        let db = k2_core::db::shared();
        let id = uuid::Uuid::new_v4().to_string();
        let sentinel = format!("k2test-MAINDB-SENTINEL-{}", &id[..8]);
        {
            let conn = db.lock();
            conn.execute(
                "INSERT INTO projects (id, path, name) VALUES (?1, ?2, ?3)",
                rusqlite::params![id, format!("/tmp/k2-vault-test-{id}"), format!("ws-{}", &id[..8])],
            )
            .expect("project");
            conn.execute("UPDATE projects SET anthropic_api_key = ?1 WHERE id = ?2", rusqlite::params![sentinel, id]).expect("key");
        }
        let out = adopt(&v);
        assert!(out.moved.iter().any(|m| m == &format!("projects/{id}")), "{out:?}");
        let (plain, sref): (Option<String>, Option<String>) = db
            .lock()
            .query_row("SELECT anthropic_api_key, anthropic_secret_ref FROM projects WHERE id = ?1", [&id], |r| Ok((r.get(0)?, r.get(1)?)))
            .expect("row");
        assert!(plain.is_none());
        assert_eq!(sref.as_deref(), Some(format!("k2v://ws/{id}/ANTHROPIC_API_KEY").as_str()));
        // Readers resolve through the holder.
        let _ = k2_core::secret_refs::install(Box::new(Holder));
        let path = format!("/tmp/k2-vault-test-{id}");
        assert_eq!(k2_core::workspace::settings::get_workspace_api_key(&path).as_deref(), Some(sentinel.as_str()));
    }
}
