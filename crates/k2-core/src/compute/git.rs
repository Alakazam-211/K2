//! Compute git access, the controller's store and forge client (0.46.10,
//! prd-compute-git-access-v1, GH#78).
//!
//! - **Grants** (`compute_git_grants`, migration 0272): which workspace's
//!   jobs may read which repo on which nodes. `nodes` is `*` (every node
//!   this server owns, now and later) or one node id; a node another server
//!   owns is covered only by a row that names it with `shared`.
//! - **The credential** (`compute_git_credentials`): facts only (kind, app
//!   id). The App's private key or the fine-grained token is in the vault
//!   (the daemon keeps it, system namespace).
//! - **GitHub** ([`Github`]): an App's 9-minute RS256 JWT, its installation
//!   for a repo, an installation token for exactly that repo with
//!   `contents: read`, and deleting a token at revoke. A fine-grained
//!   token is checked against the repo and handed out as it is (GitHub
//!   can't mint short-lived tokens from one).

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use super::proto::gitcred;
use super::store::NodeRow;

/// The only forge in v1.
pub const FORGE_GITHUB: &str = "github";
/// A grant on every node this server owns.
pub const ALL_NODES: &str = "*";
/// How long an App JWT lives (GitHub allows 10 minutes).
pub const JWT_SECS: i64 = 540;
/// A fine-grained token's nominal life for the node's keep rule: GitHub
/// doesn't say when it expires, so the node asks again after this.
pub const TOKEN_NOMINAL_SECS: i64 = 3600;

/// One grant row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GrantRow {
    pub workspace_id: String,
    pub repo: String,
    /// `*` or a node id.
    pub nodes: String,
    pub shared: bool,
    pub granted_by: String,
    pub granted_at: i64,
}

/// The server's credential (facts only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CredRow {
    /// `app` or `token`.
    pub kind: String,
    pub app_id: Option<String>,
    pub set_by: String,
    pub set_at: i64,
}

impl CredRow {
    pub fn is_app(&self) -> bool {
        self.kind == "app"
    }
}

fn grant_of(r: &rusqlite::Row) -> rusqlite::Result<GrantRow> {
    Ok(GrantRow {
        workspace_id: r.get(0)?,
        repo: r.get(1)?,
        nodes: r.get(2)?,
        shared: r.get::<_, i64>(3)? != 0,
        granted_by: r.get(4)?,
        granted_at: r.get(5)?,
    })
}

const GRANT_COLS: &str = "workspace_id, repo, nodes, shared, granted_by, granted_at";

/// Add (or refresh) one grant. `nodes`: `*` or a node id.
pub fn add_grant(conn: &Connection, ws: &str, repo: &str, nodes: &str, shared: bool, by: &str, at: i64) -> Result<(), String> {
    conn.execute(
        "INSERT INTO compute_git_grants (workspace_id, forge, repo, nodes, shared, granted_by, granted_at) VALUES (?1, 'github', ?2, ?3, ?4, ?5, ?6) \
         ON CONFLICT(workspace_id, repo, nodes) DO UPDATE SET shared = excluded.shared, granted_by = excluded.granted_by, granted_at = excluded.granted_at",
        params![ws, repo, nodes, shared as i64, by, at],
    )
    .map_err(|e| format!("git grant: {e}"))?;
    Ok(())
}

/// Remove grants of `ws` for `repo` (`nodes`: one scope, or every scope
/// when `None`). Returns the rows removed.
pub fn remove_grants(conn: &Connection, ws: &str, repo: &str, nodes: Option<&str>) -> Result<Vec<GrantRow>, String> {
    let gone: Vec<GrantRow> = grants_of(conn, ws).into_iter().filter(|g| g.repo == repo && nodes.is_none_or(|n| g.nodes == n)).collect();
    for g in &gone {
        conn.execute("DELETE FROM compute_git_grants WHERE workspace_id = ?1 AND repo = ?2 AND nodes = ?3", params![g.workspace_id, g.repo, g.nodes])
            .map_err(|e| format!("git revoke: {e}"))?;
    }
    Ok(gone)
}

/// Every grant of `ws`, by repo then scope.
pub fn grants_of(conn: &Connection, ws: &str) -> Vec<GrantRow> {
    let Ok(mut st) = conn.prepare(&format!("SELECT {GRANT_COLS} FROM compute_git_grants WHERE workspace_id = ?1 ORDER BY repo, nodes")) else { return Vec::new() };
    st.query_map(params![ws], grant_of).map(|rows| rows.filter_map(Result::ok).collect()).unwrap_or_default()
}

/// Does `g` cover `node`? `*` covers every node this server owns; a row
/// naming the node covers it when this server owns it, or when it is
/// another server's and the row says `shared`.
pub fn covers(g: &GrantRow, node: &NodeRow) -> bool {
    if g.nodes == ALL_NODES {
        return node.owned_for_secrets();
    }
    g.nodes == node.id && (node.owned_for_secrets() || g.shared)
}

/// The grant that lets `ws`'s jobs read `repo` on `node` right now.
pub fn covering(conn: &Connection, ws: &str, repo: &str, node: &NodeRow) -> Option<GrantRow> {
    grants_of(conn, ws).into_iter().find(|g| g.repo == repo && covers(g, node))
}

/// `Plan::git_access` for a job of `ws` on `node`: the covered repos,
/// sorted, unique, at most [`gitcred::MAX_REPOS`].
pub fn plan_repos(conn: &Connection, ws: &str, node: &NodeRow) -> Vec<String> {
    let mut v: Vec<String> = grants_of(conn, ws).into_iter().filter(|g| covers(g, node)).map(|g| g.repo).collect();
    v.sort();
    v.dedup();
    v.truncate(gitcred::MAX_REPOS);
    v
}

/// The server's credential, if set.
pub fn credential(conn: &Connection) -> Option<CredRow> {
    conn.query_row("SELECT kind, app_id, set_by, set_at FROM compute_git_credentials WHERE forge = 'github'", [], |r| {
        Ok(CredRow { kind: r.get(0)?, app_id: r.get(1)?, set_by: r.get(2)?, set_at: r.get(3)? })
    })
    .optional()
    .ok()
    .flatten()
}

pub fn set_credential(conn: &Connection, kind: &str, app_id: Option<&str>, by: &str, at: i64) -> Result<(), String> {
    conn.execute(
        "INSERT INTO compute_git_credentials (forge, kind, app_id, set_by, set_at) VALUES ('github', ?1, ?2, ?3, ?4) \
         ON CONFLICT(forge) DO UPDATE SET kind = excluded.kind, app_id = excluded.app_id, set_by = excluded.set_by, set_at = excluded.set_at",
        params![kind, app_id, by, at],
    )
    .map_err(|e| format!("git credential: {e}"))?;
    Ok(())
}

pub fn clear_credential(conn: &Connection) -> Result<(), String> {
    conn.execute("DELETE FROM compute_git_credentials WHERE forge = 'github'", []).map_err(|e| format!("git credential: {e}"))?;
    Ok(())
}

// ── credentials as people hand them in ───────────────────────────────

/// Why a token can't be the server's credential, if it can't: only a
/// fine-grained personal access token (`github_pat_…`). A classic token
/// (`ghp_`) or someone's login (`gho_`, `ghu_`) reaches every repo its
/// person can, with write access and no expiry.
pub fn token_refusal(t: &str) -> Option<&'static str> {
    let t = t.trim();
    if t.starts_with("github_pat_") && t.len() > 20 && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    Some(if t.starts_with("ghp_") {
        "a classic token reaches every repo you can, with write access: make a fine-grained token (github_pat_…) with read-only Contents on just these repos"
    } else if t.starts_with("gho_") || t.starts_with("ghu_") {
        "that's a person's own GitHub login (gh auth token): every repo, read and write, no expiry. Use a GitHub App or a fine-grained token instead"
    } else {
        "not a fine-grained personal access token (github_pat_…)"
    })
}

/// An App's private key (PEM, PKCS#1 `RSA PRIVATE KEY` as GitHub gives
/// it, or PKCS#8 `PRIVATE KEY`), checked by loading it.
pub fn app_key(pem: &str) -> Result<aws_lc_rs::signature::RsaKeyPair, String> {
    let (label, der) = pem_der(pem).ok_or("not a PEM private key")?;
    let k = match label.as_str() {
        "RSA PRIVATE KEY" => aws_lc_rs::signature::RsaKeyPair::from_der(&der),
        "PRIVATE KEY" => aws_lc_rs::signature::RsaKeyPair::from_pkcs8(&der),
        _ => return Err(format!("a {label} isn't an RSA private key")),
    };
    k.map_err(|e| format!("the App's private key doesn't load: {e}"))
}

fn pem_der(pem: &str) -> Option<(String, Vec<u8>)> {
    use base64::Engine;
    let begin = pem.find("-----BEGIN ")?;
    let rest = &pem[begin + 11..];
    let label_end = rest.find("-----")?;
    let label = rest[..label_end].to_string();
    let body_start = label_end + 5;
    let end = rest.find(&format!("-----END {label}-----"))?;
    let b64: String = rest[body_start..end].chars().filter(|c| !c.is_whitespace()).collect();
    let der = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    Some((label, der))
}

/// Is this a GitHub App id (digits)?
pub fn valid_app_id(s: &str) -> bool {
    (1..=20).contains(&s.len()) && s.chars().all(|c| c.is_ascii_digit())
}

/// An App's JWT: RS256 over `{"iat": now-60, "exp": now+540, "iss": app_id}`.
pub fn app_jwt(app_id: &str, key: &aws_lc_rs::signature::RsaKeyPair, now: i64) -> Result<String, String> {
    use base64::Engine;
    let enc = |b: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b);
    let header = enc(br#"{"alg":"RS256","typ":"JWT"}"#);
    let claims = enc(serde_json::json!({"iat": now - 60, "exp": now + JWT_SECS, "iss": app_id}).to_string().as_bytes());
    let input = format!("{header}.{claims}");
    let mut sig = vec![0u8; key.public_modulus_len()];
    key.sign(&aws_lc_rs::signature::RSA_PKCS1_SHA256, &aws_lc_rs::rand::SystemRandom::new(), input.as_bytes(), &mut sig).map_err(|_| "signing the App's JWT failed".to_string())?;
    Ok(format!("{input}.{}", enc(&sig)))
}

/// A minted (or handed-out) credential and when it stops working.
#[derive(Clone, PartialEq, Eq)]
pub struct Minted {
    pub token: String,
    pub expires_at: i64,
}

impl std::fmt::Debug for Minted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Minted(expires {})", self.expires_at)
    }
}

/// What went wrong talking to GitHub: `(refused, message)`, `refused` =
/// GitHub answered no (a 4xx), else it couldn't be reached or answered oddly.
pub type ForgeError = (bool, String);

/// The GitHub API (`api_base`: `https://api.github.com`, or a test's).
pub struct Github {
    pub api_base: String,
    http: reqwest::blocking::Client,
}

impl Github {
    pub fn new(api_base: &str) -> Result<Self, String> {
        let http = reqwest::blocking::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("k2-daemon/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| format!("http client: {e}"))?;
        Ok(Self { api_base: api_base.trim_end_matches('/').to_string(), http })
    }

    fn call(&self, method: reqwest::Method, path: &str, auth: &str, body: Option<serde_json::Value>) -> Result<(u16, serde_json::Value), ForgeError> {
        crate::airgap::refuse().map_err(|e| (true, e))?;
        let url = format!("{}{path}", self.api_base);
        let mut rq = self.http.request(method.clone(), &url).header("Authorization", auth).header("Accept", "application/vnd.github+json").header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(b) = body {
            rq = rq.header("Content-Type", "application/json").body(b.to_string());
        }
        let resp = rq.send().map_err(|e| (false, format!("{method} {path}: {}", e.without_url())))?;
        let status = resp.status().as_u16();
        let text = resp.text().unwrap_or_default();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
        if (400..500).contains(&status) {
            let msg = v.get("message").and_then(|m| m.as_str()).unwrap_or("refused");
            return Err((true, format!("GitHub answered {status} to {method} {path}: {msg}")));
        }
        if !(200..300).contains(&status) {
            return Err((false, format!("GitHub answered {status} to {method} {path}")));
        }
        Ok((status, v))
    }

    /// `GET /app` with the JWT: the App's slug (setup's check).
    pub fn app_slug(&self, jwt: &str) -> Result<String, ForgeError> {
        let (_, v) = self.call(reqwest::Method::GET, "/app", &format!("Bearer {jwt}"), None)?;
        Ok(v.get("slug").and_then(|s| s.as_str()).unwrap_or("").to_string())
    }

    /// The App's installation that covers `owner/repo`.
    pub fn installation(&self, jwt: &str, owner: &str, repo: &str) -> Result<u64, ForgeError> {
        let (_, v) = self.call(reqwest::Method::GET, &format!("/repos/{owner}/{repo}/installation"), &format!("Bearer {jwt}"), None)?;
        v.get("id").and_then(|i| i.as_u64()).ok_or((false, "GitHub's installation answer has no id".into()))
    }

    /// An installation token for exactly `repo`, `contents: read`.
    pub fn installation_token(&self, jwt: &str, installation: u64, repo: &str) -> Result<Minted, ForgeError> {
        let body = serde_json::json!({"repositories": [repo], "permissions": {"contents": "read"}});
        let (_, v) = self.call(reqwest::Method::POST, &format!("/app/installations/{installation}/access_tokens"), &format!("Bearer {jwt}"), Some(body))?;
        let token = v.get("token").and_then(|t| t.as_str()).filter(|t| !t.is_empty()).ok_or((false, "GitHub's token answer has no token".to_string()))?;
        let exp = v
            .get("expires_at")
            .and_then(|e| e.as_str())
            .and_then(|e| chrono::DateTime::parse_from_rfc3339(e).ok())
            .map(|t| t.timestamp())
            .ok_or((false, "GitHub's token answer has no expiry".to_string()))?;
        // Never trust more than GitHub's own hour.
        let exp = exp.min(super::now() + 3600);
        Ok(Minted { token: token.to_string(), expires_at: exp })
    }

    /// May `token` read `owner/repo`? (`GET /repos/{owner}/{repo}`.)
    pub fn can_read(&self, token: &str, owner: &str, repo: &str) -> Result<(), ForgeError> {
        self.call(reqwest::Method::GET, &format!("/repos/{owner}/{repo}"), &format!("Bearer {token}"), None).map(|_| ())
    }

    /// Revoke an installation token now (`DELETE /installation/token`).
    pub fn delete_token(&self, token: &str) -> Result<(), ForgeError> {
        self.call(reqwest::Method::DELETE, "/installation/token", &format!("Bearer {token}"), None).map(|_| ())
    }
}

/// Test helpers for downstream crates' fake GitHub (the daemon's
/// integration tests): a fresh App key and a JWT check.
#[cfg(any(test, feature = "test-util"))]
pub mod testing {
    /// A fresh 2048-bit App key as a PKCS#8 PEM, and its public key.
    pub fn app_key_pem() -> (String, Vec<u8>) {
        use aws_lc_rs::encoding::AsDer;
        use aws_lc_rs::signature::KeyPair;
        use base64::Engine;
        let kp = aws_lc_rs::rsa::KeyPair::generate(aws_lc_rs::rsa::KeySize::Rsa2048).expect("RSA key");
        let der = kp.as_der().expect("PKCS#8");
        let b64 = base64::engine::general_purpose::STANDARD.encode(der.as_ref());
        let lines: Vec<String> = b64.as_bytes().chunks(64).map(|c| String::from_utf8_lossy(c).into_owned()).collect();
        (format!("-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n", lines.join("\n")), kp.public_key().as_ref().to_vec())
    }

    /// The claims of `jwt` when its RS256 signature checks with `public`.
    pub fn verify_jwt(jwt: &str, public: &[u8]) -> Option<serde_json::Value> {
        use base64::Engine;
        let parts: Vec<&str> = jwt.split('.').collect();
        if parts.len() != 3 {
            return None;
        }
        let dec = |s: &str| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(s).ok();
        let (claims, sig) = (dec(parts[1])?, dec(parts[2])?);
        let key = aws_lc_rs::signature::UnparsedPublicKey::new(&aws_lc_rs::signature::RSA_PKCS1_2048_8192_SHA256, public);
        key.verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &sig).ok()?;
        serde_json::from_slice(&claims).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(conn: &Connection, id: &str, role: &str, owner_label: Option<&str>) -> NodeRow {
        let labels = std::collections::BTreeMap::new();
        crate::compute::store::insert_pending_node(conn, id, id, &format!("{id:0>64}"), "pem", &labels, "owner", "123456", 1).unwrap();
        let mut n = crate::compute::store::node_by_id(conn, id).unwrap();
        n.role = role.into();
        n.owner_label = owner_label.map(str::to_string);
        n
    }

    #[test]
    fn grants_cover_owned_nodes_by_star_and_shared_nodes_only_by_name_with_optin() {
        let conn = crate::db::isolated_test_connection();
        let own = node(&conn, "own-1", "owner", None);
        let shared = node(&conn, "ten-1", "tenant", Some("other.k2.dev"));
        add_grant(&conn, "w", "github.com/acme/app", ALL_NODES, false, "owner", 1).unwrap();
        add_grant(&conn, "w", "github.com/acme/lib", "ten-1", false, "owner", 1).unwrap();
        add_grant(&conn, "w", "github.com/acme/tool", "ten-1", true, "owner", 1).unwrap();
        assert_eq!(plan_repos(&conn, "w", &own), vec!["github.com/acme/app"]);
        assert_eq!(plan_repos(&conn, "w", &shared), vec!["github.com/acme/tool"], "* never reaches a shared node; a named one needs shared");
        assert_eq!(plan_repos(&conn, "other", &own), Vec::<String>::new(), "another workspace has none");
        assert!(covering(&conn, "w", "github.com/acme/app", &own).is_some());
        assert!(covering(&conn, "w", "github.com/acme/app", &shared).is_none());
        // Re-granting refreshes; revoking one scope keeps the others.
        add_grant(&conn, "w", "github.com/acme/app", "own-1", false, "owner", 2).unwrap();
        let gone = remove_grants(&conn, "w", "github.com/acme/app", Some(ALL_NODES)).unwrap();
        assert_eq!(gone.len(), 1);
        assert_eq!(plan_repos(&conn, "w", &own), vec!["github.com/acme/app"], "the node-named grant stays");
        assert_eq!(remove_grants(&conn, "w", "github.com/acme/app", None).unwrap().len(), 1);
        assert!(plan_repos(&conn, "w", &own).is_empty());
    }

    #[test]
    fn credential_round_trip() {
        let conn = crate::db::isolated_test_connection();
        assert_eq!(credential(&conn), None);
        set_credential(&conn, "app", Some("123"), "owner", 5).unwrap();
        assert_eq!(credential(&conn).map(|c| (c.kind, c.app_id)), Some(("app".into(), Some("123".into()))));
        set_credential(&conn, "token", None, "owner", 6).unwrap();
        assert!(!credential(&conn).unwrap().is_app());
        clear_credential(&conn).unwrap();
        assert_eq!(credential(&conn), None);
    }

    #[test]
    fn only_fine_grained_tokens_are_taken() {
        assert_eq!(token_refusal("github_pat_11ABCDEFG0k2test_abcdefghijklmnopqrstuvwxyz"), None);
        assert!(token_refusal("ghp_k2testClassicToken0123456789").unwrap().contains("classic"));
        assert!(token_refusal("gho_k2testLoginToken0123456789").unwrap().contains("login"));
        assert!(token_refusal("ghu_k2testUserToServer0123456789").unwrap().contains("login"));
        assert!(token_refusal("github_pat_").is_some());
        assert!(token_refusal("hunter2").is_some());
        assert!(valid_app_id("123456") && !valid_app_id("12a") && !valid_app_id(""));
    }

    #[test]
    fn an_app_key_loads_and_signs_a_jwt_github_can_check() {
        use base64::Engine;
        let (pem, public) = testing::app_key_pem();
        let key = app_key(&pem).expect("a PKCS#8 key loads");
        assert!(app_key("-----BEGIN EC PRIVATE KEY-----\nAAAA\n-----END EC PRIVATE KEY-----\n").is_err());
        assert!(app_key("not a key").is_err());
        let jwt = app_jwt("4242", &key, 1_800_000_000).unwrap();
        let claims = testing::verify_jwt(&jwt, &public).expect("the signature checks with the App's public key");
        assert_eq!(claims, serde_json::json!({"iat": 1_800_000_000 - 60, "exp": 1_800_000_000 + JWT_SECS, "iss": "4242"}));
        let (_, other) = testing::app_key_pem();
        assert!(testing::verify_jwt(&jwt, &other).is_none(), "another key's public half doesn't check it");
        let header: serde_json::Value = serde_json::from_slice(&base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(jwt.split('.').next().unwrap()).unwrap()).unwrap();
        assert_eq!(header, serde_json::json!({"alg": "RS256", "typ": "JWT"}));
    }
}
