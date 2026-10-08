//! `/cli/vault/*` routes that USE a secret on the caller's behalf:
//! `fetch` (V4), `env-file` and `import` (V8). Split from
//! `vault_routes.rs` so delivery lands without touching management.
//!
//! `fetch` (§9.2 as amended by VS3–VS6, KV17): the agent names the secret;
//! K2 checks the URL against the secret's sites, puts the value only where
//! the secret's creator said (sealed injection), makes the call through
//! the egress gate, and returns the response with every echo of the value
//! replaced by `[k2v:NAME]`. Request bodies over 64 KiB, and `-o` output,
//! go through files inside the caller's own workspace.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use base64::Engine as _;
use k2_vault::fetch::MAX_INLINE_BODY;
use k2_vault::hosts::{Origin, OriginPattern};
use k2_vault::{LocalVault, Requester, SecretStore, Use, VaultError};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::vault::actor::VaultCaller;
use crate::vault_routes::{refuse, selector, Sel};

pub const FETCH: &str = "/cli/vault/fetch";

/// Routes this module serves.
pub const ROUTES: &[&str] = &[FETCH];
/// POST-only routes among them.
pub const POST_ROUTES: &[&str] = &[FETCH];

pub fn is_route(path: &str) -> bool {
    ROUTES.contains(&path)
}

pub fn is_post_route(path: &str) -> bool {
    POST_ROUTES.contains(&path)
}

pub fn handle(
    vault: &Arc<LocalVault>,
    path: &str,
    c: &VaultCaller,
    body: &[u8],
    _params: &HashMap<String, String>,
    _ingress: &str,
) -> CliResponse {
    let r = match path {
        FETCH => fetch(vault, c, body),
        _ => Err(VaultError::not_found("no such vault route")),
    };
    match r {
        Ok(v) => CliResponse::ok_json(v.to_string()),
        Err(e) => refuse(&e),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FetchSecret {
    #[serde(flatten)]
    sel: Sel,
    /// Pick from the secret's sealed `allowedDst` (`header:X`, `query:k`).
    dst: Option<String>,
    fmt: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FetchBody {
    #[serde(default)]
    secrets: Vec<FetchSecret>,
    /// Shorthand for one secret by name.
    secret: Option<String>,
    method: Option<String>,
    url: Option<String>,
    #[serde(default)]
    headers: Vec<(String, String)>,
    body_b64: Option<String>,
    /// A file inside the caller's workspace (bodies over 64 KiB).
    body_file: Option<String>,
    #[serde(default)]
    max_bytes: usize,
    #[serde(default)]
    timeout_secs: u64,
    #[serde(default)]
    include_headers: bool,
    /// Write the (redacted) body to this workspace file instead.
    out: Option<String>,
}

fn rate() -> &'static crate::vault_fetch::RateLimiter {
    static R: OnceLock<crate::vault_fetch::RateLimiter> = OnceLock::new();
    R.get_or_init(crate::vault_fetch::RateLimiter::for_fetch)
}

/// A path inside the workspace folder (no `..` escape, no symlink escape
/// for the parent). Returns the full path.
pub fn jailed_path(workspace_root: &Path, rel: &str) -> Result<PathBuf, VaultError> {
    let bad = || VaultError::bad_request("path_outside_workspace", "use a path inside your workspace");
    let rel = rel.trim();
    if rel.is_empty() || rel.contains('\0') {
        return Err(bad());
    }
    let p = Path::new(rel);
    let joined = if p.is_absolute() { p.to_path_buf() } else { workspace_root.join(p) };
    let root = std::fs::canonicalize(workspace_root).map_err(|_| bad())?;
    let parent = joined.parent().ok_or_else(bad)?;
    let parent = std::fs::canonicalize(parent).map_err(|_| bad())?;
    if !parent.starts_with(&root) {
        return Err(bad());
    }
    let name = joined.file_name().ok_or_else(bad)?;
    if name == ".." || name == "." {
        return Err(bad());
    }
    Ok(parent.join(name))
}

fn fetch(vault: &Arc<LocalVault>, c: &VaultCaller, body: &[u8]) -> Result<Value, VaultError> {
    let a = c.actor.agent().ok_or_else(|| {
        VaultError::forbidden("use_fetch_as_agent", "k2 vault fetch runs for an agent session; people call the site themselves")
    })?;
    let b: FetchBody = serde_json::from_slice(body).map_err(|_| VaultError::bad_request("bad_json", "bad request body"))?;
    let key = format!("{}/{}", a.workspace_id, c.session_id.clone().unwrap_or_default());
    rate().check(&key)?;
    let url = b.url.clone().ok_or_else(|| VaultError::bad_request("url_required", "give the URL to call"))?;
    let (_, origin): (url::Url, Origin) = Origin::parse_url(&url)?;
    let mut wanted = b.secrets;
    if let Some(n) = b.secret.as_deref().filter(|n| !n.is_empty()) {
        wanted.push(FetchSecret { sel: Sel { name: Some(n.to_string()), ..Default::default() }, dst: None, fmt: None });
    }
    if wanted.is_empty() {
        return Err(VaultError::bad_request("secret_required", "name the secret: --secret NAME"));
    }
    let who = Requester::Session { workspace_id: a.workspace_id.clone(), session_id: c.session_id.clone() };
    let mut injections = vec![];
    for w in &wanted {
        let sel = selector(c, &w.sel)?;
        let r = vault.resolve(&sel, Use::Fetch(&origin), &who)?;
        let (dst, fmt) = r.inject.choose(w.dst.as_deref(), w.fmt.as_deref())?;
        let allowed: Vec<OriginPattern> = k2_vault::hosts::parse_stored(&r.meta.hosts);
        injections.push(crate::vault_fetch::Injection { name: r.meta.name.clone(), value: r.value, dst, fmt, allowed });
    }
    let ws_root = crate::vault::workspace_path(&a.workspace_id).map(PathBuf::from);
    let req_body = match (&b.body_b64, &b.body_file) {
        (Some(_), Some(_)) => return Err(VaultError::bad_request("bad_body", "send the body inline or as a file, not both")),
        (Some(s), None) => {
            let v = base64::engine::general_purpose::STANDARD
                .decode(s.trim())
                .map_err(|_| VaultError::bad_request("bad_body", "bodyB64 isn't base64"))?;
            if v.len() > MAX_INLINE_BODY {
                return Err(VaultError::bad_request("body_too_large", "bodies over 64 KiB go as a file (-d @file)"));
            }
            Some(v)
        }
        (None, Some(f)) => {
            let root = ws_root.as_ref().ok_or_else(|| VaultError::not_found("workspace folder not found"))?;
            let p = jailed_path(root, f)?;
            let meta = std::fs::metadata(&p).map_err(|_| VaultError::not_found("body file not found"))?;
            if meta.len() > 10 * 1024 * 1024 {
                return Err(VaultError::bad_request("body_too_large", "request bodies are at most 10 MiB"));
            }
            Some(std::fs::read(&p).map_err(|_| VaultError::not_found("body file not readable"))?)
        }
        (None, None) => None,
    };
    let spec = crate::vault_fetch::FetchSpec {
        method: b.method.clone().unwrap_or_else(|| if req_body.is_some() { "POST".into() } else { "GET".into() }),
        url,
        headers: b.headers.clone(),
        body: req_body,
        max_bytes: b.max_bytes,
        timeout: Duration::from_secs(b.timeout_secs),
        include_headers: b.include_headers,
    };
    let redactor = vault.redactor();
    let res = crate::vault_fetch::execute(spec, injections, &redactor)?;
    let headers: Vec<Value> = res.headers.iter().map(|(k, v)| json!([k, v])).collect();
    if let Some(out) = b.out.as_deref() {
        let root = ws_root.as_ref().ok_or_else(|| VaultError::not_found("workspace folder not found"))?;
        let p = jailed_path(root, out)?;
        write_no_follow(&p, &res.body)?;
        return Ok(json!({
            "status": res.status,
            "headers": headers,
            "truncated": res.truncated,
            "redirected": res.redirected,
            "out": p.display().to_string(),
            "bytes": res.body.len(),
        }));
    }
    let (text, b64) = match String::from_utf8(res.body.clone()) {
        Ok(t) => (Some(t), None),
        Err(_) => (None, Some(base64::engine::general_purpose::STANDARD.encode(&res.body))),
    };
    Ok(json!({
        "status": res.status,
        "headers": headers,
        "truncated": res.truncated,
        "redirected": res.redirected,
        "body": text,
        "bodyB64": b64,
    }))
}

fn write_no_follow(p: &Path, bytes: &[u8]) -> Result<(), VaultError> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(p)
        .map_err(|e| VaultError::bad_request("out_not_writable", format!("can't write the output file ({})", e.kind())))?;
    f.write_all(bytes).map_err(|e| VaultError::internal(format!("write: {}", e.kind())))
}

/// A published service's id from its label (`name`) or id. Agents see
/// only their own workspace's services.
pub fn published_service_id(c: &VaultCaller, label: &str) -> Result<String, VaultError> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT id, project_id FROM published_services WHERE id = ?1 OR name = ?1 ORDER BY (id = ?1) DESC LIMIT 1",
            rusqlite::params![label],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    let (id, project) = row.ok_or_else(|| VaultError::not_found(format!("no published service {label}")))?;
    if let Some(ws) = c.workspace_id() {
        if ws != project {
            return Err(VaultError::not_found(format!("no published service {label} in this workspace")));
        }
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// End to end through the route: the agent names the secret, the
    /// upstream gets it in the sealed header, the echo comes back
    /// redacted, another host is refused before any connection.
    #[test]
    fn fetch_route_injects_sealed_header_and_redacts_the_echo() {
        use std::io::{Read, Write};
        use std::sync::atomic::{AtomicUsize, Ordering};

        use k2_vault::confirm::KdfCost;
        use k2_vault::keystore::fake::FakeKeyStore;
        use k2_vault::{Actor, AgentActor, KeyStore, Kind, Namespace, OpenOptions, PutRequest, Scope, SecretBytes};

        const VAL: &str = "k2test_ROUTEFETCH_9a8b7c6d5e4f3a2b1c";
        crate::test_support::with_temp_home(|| {
            let t = k2_vault::fsutil::TestDir::new("fetch-route");
            let ks: Arc<dyn KeyStore> = Arc::new(FakeKeyStore::new());
            let v = Arc::new(LocalVault::open(t.path().join("vault"), ks, OpenOptions { kdf: KdfCost::Floor }).expect("open"));
            let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
            let port = l.local_addr().expect("addr").port();
            let conns = Arc::new(AtomicUsize::new(0));
            let c2 = conns.clone();
            std::thread::spawn(move || {
                for s in l.incoming() {
                    let Ok(mut s) = s else { return };
                    c2.fetch_add(1, Ordering::SeqCst);
                    let mut buf = vec![0u8; 8192];
                    let mut got = Vec::new();
                    while !got.windows(4).any(|w| w == b"\r\n\r\n") {
                        let n = s.read(&mut buf).unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        got.extend_from_slice(&buf[..n]);
                    }
                    let head = String::from_utf8_lossy(&got).to_string();
                    let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", head.len(), head);
                    let _ = s.write_all(resp.as_bytes());
                }
            });
            let ws = "ws-fetch";
            let mut r = PutRequest::new(Namespace::User, Scope::workspace(ws), "STRIPE_KEY", Kind::ApiKey);
            r.hosts = vec![format!("http://127.0.0.1:{port}")];
            v.put(r, SecretBytes::from_slice(VAL.as_bytes()), &Actor::Owner).expect("put");
            let c = VaultCaller {
                actor: Actor::Agent(AgentActor { workspace_id: ws.into(), session_id: Some("s1".into()), shell: false, vault_manage: false }),
                channel: crate::vault::actor::Channel::Plain,
                session_id: Some("s1".into()),
            };
            let body = json!({ "secret": "STRIPE_KEY", "url": format!("http://127.0.0.1:{port}/v1/charges") }).to_string();
            let r = handle(&v, FETCH, &c, body.as_bytes(), &HashMap::new(), "test");
            assert_eq!(r.status, "200 OK", "{}", r.body);
            assert!(!r.body.contains(VAL), "the value never comes back: {}", r.body);
            let out: Value = serde_json::from_str(&r.body).expect("json");
            let echoed = out["body"].as_str().expect("text body");
            assert!(echoed.to_ascii_lowercase().contains("authorization: [k2v:stripe_key]") || echoed.contains("[k2v:STRIPE_KEY]"), "{echoed}");
            assert_eq!(conns.load(Ordering::SeqCst), 1);
            // Another host: refused before any connection.
            let body = json!({ "secret": "STRIPE_KEY", "url": "http://127.0.0.1:1/x" }).to_string();
            let r = handle(&v, FETCH, &c, body.as_bytes(), &HashMap::new(), "test");
            assert!(r.status.starts_with("403"), "{}", r.body);
            assert!(r.body.contains("host_not_allowed"), "{}", r.body);
            // A person can't use fetch.
            let owner = VaultCaller { actor: Actor::Owner, channel: crate::vault::actor::Channel::Plain, session_id: None };
            let r = handle(&v, FETCH, &owner, body.as_bytes(), &HashMap::new(), "test");
            assert!(r.status.starts_with("403"), "{}", r.body);
        });
    }

    #[test]
    fn jail_keeps_paths_inside_the_workspace() {
        let t = k2_vault::fsutil::TestDir::new("jail");
        let ws = t.path().join("ws");
        std::fs::create_dir_all(ws.join("sub")).expect("dirs");
        jailed_path(&ws, "out.json").expect("inside");
        jailed_path(&ws, "sub/out.json").expect("inside");
        assert!(jailed_path(&ws, "../escape.json").is_err());
        assert!(jailed_path(&ws, "/etc/passwd").is_err());
        std::os::unix::fs::symlink(t.path(), ws.join("link")).expect("symlink");
        assert!(jailed_path(&ws, "link/x").is_err(), "a symlinked folder can't lead outside");
    }
}
