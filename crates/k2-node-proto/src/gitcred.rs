//! Compute git access (protocol 8, prd-compute-git-access-v1, GH#78): a
//! short-lived, read-only git token for one granted repo, sealed from the
//! server to the node that asks for it.
//!
//! The plan names the repos a job may read (`Plan::git_access`, keys like
//! `github.com/acme/app`). When the node syncs one of them, or when git
//! inside the job asks its credential helper for one, the node sends
//! `git_cred_need` with a fresh X25519 public key; the server checks the
//! grant again, mints (or reuses) a token and answers `git_cred`:
//!
//! - `shared = X25519(e_c, e_n)`, both halves fresh and single-use;
//! - `info = signed_bytes("k2-compute-gitcred-v1", [controller_fp, node_fp,
//!   job_id, generation, attempt, plan_digest, req, repo, purpose, epk_n,
//!   epk_c])`;
//! - `key = HKDF-SHA256(salt = "k2-compute-gitcred-v1", ikm = shared, info)`;
//! - `ct = AES-256-GCM(key, nonce = 0^12, aad = info, payload)`.
//!
//! The same construction as [`crate::secret`] (a zero nonce is safe because
//! every key is used once), with its own domain, and the repo and purpose
//! in the binding, so an answer for one repo never opens as another's.

use aws_lc_rs::{aead, agreement, rand};
use serde::{Deserialize, Serialize};

use crate::crypto;
use crate::secret::{key_for_domain, wipe};

/// Domain, HKDF salt and AAD prefix.
pub const DOMAIN: &str = "k2-compute-gitcred-v1";
/// The offer feature: this node takes `Plan::git_access` and asks for git
/// tokens (protocol ≥ [`crate::frames::PROTOCOL_GIT`]).
pub const FEATURE_GIT_ACCESS: &str = "git-access";
/// At most this many repos in one plan.
pub const MAX_REPOS: usize = 32;
/// Largest token (GitHub's are about 40–100 bytes).
pub const MAX_TOKEN_BYTES: usize = 4096;
/// Largest sealed payload (`ct`, raw bytes).
pub const MAX_SEALED_BYTES: usize = 8 * 1024;
/// How long the node waits for the server's answer.
pub const NEED_TIMEOUT_SECS: u64 = 30;
/// The node keeps a token for one job at most this long…
pub const NODE_KEEP_SECS: i64 = 600;
/// …and never closer than this to its expiry.
pub const EXPIRY_MARGIN_SECS: i64 = 300;
/// The username git sends with a GitHub token (installation tokens need
/// it; fine-grained tokens take any name).
pub const USERNAME: &str = "x-access-token";
/// The only forge in v1.
pub const GITHUB_HOST: &str = "github.com";

/// Refusal codes (the node passes only these through).
pub mod codes {
    /// No live grant covers this workspace, repo and node.
    pub const NOT_GRANTED: &str = "git_access_not_granted";
    /// The server has no git credential set (`k2 compute git-access setup`).
    pub const NO_CREDENTIAL: &str = "git_access_no_credential";
    /// Not this job's current attempt, or it already ended.
    pub const STALE: &str = "git_access_stale";
    /// The link dropped, the wait ran out, or the forge didn't answer.
    pub const UNAVAILABLE: &str = "git_access_unavailable";
    /// A node another server owns, not named with `--shared`.
    pub const SHARED_NEEDS_OPTIN: &str = "git_access_shared_needs_optin";
    /// A personal access token never goes to a node another server owns.
    pub const PAT_SHARED: &str = "git_access_pat_shared";
    /// The server's vault is closed.
    pub const VAULT_LOCKED: &str = "git_access_vault_locked";
    /// The forge refused to mint (App not installed on the repo, …).
    pub const FORGE_REFUSED: &str = "git_access_forge_refused";

    /// Every code a node accepts from a server.
    pub const ALL: &[&str] = &[NOT_GRANTED, NO_CREDENTIAL, STALE, UNAVAILABLE, SHARED_NEEDS_OPTIN, PAT_SHARED, VAULT_LOCKED, FORGE_REFUSED];
}

/// Why a token is asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    /// The node's own fetch of the job's source (outside the job).
    Sync,
    /// git inside the job, through its credential helper.
    Job,
}

impl Purpose {
    pub fn as_str(self) -> &'static str {
        match self {
            Purpose::Sync => "sync",
            Purpose::Job => "job",
        }
    }
}

fn valid_owner(o: &str) -> bool {
    (1..=39).contains(&o.len()) && o.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') && !o.starts_with('-')
}

fn valid_repo(r: &str) -> bool {
    (1..=100).contains(&r.len()) && r != "." && r != ".." && r.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// `owner/repo` (any case, an optional `.git` or `/` at the end) on
/// `host` → the repo key `github.com/owner/repo` (lowercase). Only
/// github.com in v1.
pub fn repo_key(host: &str, path: &str) -> Option<String> {
    if !host.eq_ignore_ascii_case(GITHUB_HOST) {
        return None;
    }
    let p = path.trim_start_matches('/').trim_end_matches('/');
    let p = p.strip_suffix(".git").unwrap_or(p);
    let (owner, repo) = p.split_once('/')?;
    if !valid_owner(owner) || !valid_repo(repo) {
        return None;
    }
    Some(format!("{GITHUB_HOST}/{}/{}", owner.to_ascii_lowercase(), repo.to_ascii_lowercase()))
}

/// A repo as a person writes it: `owner/repo`, `github.com/owner/repo`, or
/// `https://github.com/owner/repo[.git]`.
pub fn parse_repo_arg(s: &str) -> Option<String> {
    let s = s.trim();
    let rest = s.strip_prefix("https://").unwrap_or(s);
    match rest.split_once('/') {
        Some((h, p)) if h.eq_ignore_ascii_case(GITHUB_HOST) => repo_key(h, p),
        _ if !s.contains("://") => repo_key(GITHUB_HOST, rest),
        _ => None,
    }
}

/// The repo key of a fetchable remote (`https://github.com/owner/repo[.git]`
/// as the server normalizes it), or `None` for any other remote.
pub fn repo_of_remote(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    if rest.contains(['?', '#', '@']) {
        return None;
    }
    let (host, path) = rest.split_once('/')?;
    repo_key(host, path)
}

/// `github.com/owner/repo` → (`owner`, `repo`).
pub fn owner_repo(key: &str) -> Option<(&str, &str)> {
    let p = key.strip_prefix("github.com/")?;
    p.split_once('/')
}

/// The https URL git uses for a repo key.
pub fn https_url(key: &str) -> String {
    format!("https://{key}")
}

/// Is `list` a valid `Plan::git_access` (keys, sorted, unique, capped)?
pub fn valid_plan_list(list: &[String]) -> bool {
    list.len() <= MAX_REPOS && list.iter().all(|k| owner_repo(k).and_then(|(o, r)| repo_key(GITHUB_HOST, &format!("{o}/{r}"))).as_deref() == Some(k.as_str())) && list.windows(2).all(|w| w[0] < w[1])
}

/// What a seal binds (besides both public halves).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub controller_fp: String,
    pub node_fp: String,
    pub job_id: String,
    pub generation: u32,
    pub attempt: u32,
    pub plan_digest: String,
    pub req: u64,
    pub repo: String,
    pub purpose: Purpose,
}

impl Binding {
    fn info(&self, epk_n: &[u8], epk_c: &[u8]) -> Vec<u8> {
        let (generation, attempt, req) = (self.generation.to_string(), self.attempt.to_string(), self.req.to_string());
        crypto::signed_bytes(
            DOMAIN,
            &[
                self.controller_fp.as_bytes(),
                self.node_fp.as_bytes(),
                self.job_id.as_bytes(),
                generation.as_bytes(),
                attempt.as_bytes(),
                self.plan_digest.as_bytes(),
                req.as_bytes(),
                self.repo.as_bytes(),
                self.purpose.as_str().as_bytes(),
                epk_n,
                epk_c,
            ],
        )
    }
}

/// One opened credential. The password is zeroed on drop; `Debug` never
/// prints it.
#[derive(Clone, PartialEq, Eq)]
pub struct Cred {
    pub username: String,
    pub password: String,
    /// Unix seconds (server clock).
    pub expires_at: i64,
}

impl std::fmt::Debug for Cred {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Cred({}, expires {})", self.username, self.expires_at)
    }
}

impl Drop for Cred {
    fn drop(&mut self) {
        // SAFETY: zero bytes keep the String valid UTF-8.
        wipe(unsafe { self.password.as_bytes_mut() });
    }
}

impl Cred {
    /// The `Authorization` header for an HTTPS git fetch (basic auth).
    pub fn basic_header(&self) -> String {
        use base64::Engine;
        let mut raw = format!("{}:{}", self.username, self.password).into_bytes();
        let h = format!("Authorization: Basic {}", base64::engine::general_purpose::STANDARD.encode(&raw));
        wipe(&mut raw);
        h
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Payload<'a> {
    username: std::borrow::Cow<'a, str>,
    password: std::borrow::Cow<'a, str>,
    expires_at: i64,
}

fn check_text(s: &str) -> bool {
    !s.is_empty() && s.len() <= MAX_TOKEN_BYTES && s.chars().all(|c| c.is_ascii_graphic())
}

/// The server's side: seal `cred` to the node's `epk_n` (base64).
/// Returns `(epk_c, ct)`, both base64.
pub fn seal(cred: &Cred, epk_n_b64: &str, b: &Binding) -> Result<(String, String), &'static str> {
    let epk_n = crypto::unb64(epk_n_b64).map_err(|_| codes::STALE)?;
    if epk_n.len() != 32 {
        return Err(codes::STALE);
    }
    if !check_text(&cred.username) || !check_text(&cred.password) {
        return Err(codes::UNAVAILABLE);
    }
    let p = Payload { username: cred.username.as_str().into(), password: cred.password.as_str().into(), expires_at: cred.expires_at };
    let mut body = serde_json::to_vec(&p).map_err(|_| codes::UNAVAILABLE)?;
    let rng = rand::SystemRandom::new();
    let r = (|| -> Result<(String, String), ()> {
        let e_c = agreement::EphemeralPrivateKey::generate(&agreement::X25519, &rng).map_err(|_| ())?;
        let epk_c = e_c.compute_public_key().map_err(|_| ())?.as_ref().to_vec();
        let info = b.info(&epk_n, &epk_c);
        let key = agreement::agree_ephemeral(e_c, &agreement::UnparsedPublicKey::new(&agreement::X25519, &epk_n), (), |shared| key_for_domain(DOMAIN, shared, &info))?;
        let mut in_out = body.clone();
        key.seal_in_place_append_tag(aead::Nonce::assume_unique_for_key([0u8; 12]), aead::Aad::from(&info), &mut in_out).map_err(|_| ())?;
        let ct = crypto::b64(&in_out);
        wipe(&mut in_out);
        Ok((crypto::b64(&epk_c), ct))
    })();
    wipe(&mut body);
    r.map_err(|_| codes::STALE)
}

/// The node's one-time key for one `git_cred_need`.
pub struct NodeKey {
    key: agreement::EphemeralPrivateKey,
    public: Vec<u8>,
}

impl std::fmt::Debug for NodeKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NodeKey({})", crypto::b64(&self.public))
    }
}

impl NodeKey {
    pub fn generate() -> Result<Self, String> {
        let rng = rand::SystemRandom::new();
        let key = agreement::EphemeralPrivateKey::generate(&agreement::X25519, &rng).map_err(|_| "X25519 key generation failed".to_string())?;
        let public = key.compute_public_key().map_err(|_| "X25519 public key failed".to_string())?.as_ref().to_vec();
        Ok(Self { key, public })
    }

    pub fn public_b64(&self) -> String {
        crypto::b64(&self.public)
    }

    /// Open the server's answer. Consumes the key (single use).
    pub fn open(self, epk_c_b64: &str, ct_b64: &str, b: &Binding) -> Result<Cred, String> {
        let bad = || "the sealed git credential doesn't open (wrong key or binding, or tampered)".to_string();
        let epk_c = crypto::unb64(epk_c_b64).map_err(|_| bad())?;
        let mut ct = crypto::unb64(ct_b64).map_err(|_| bad())?;
        if epk_c.len() != 32 || ct.len() < 17 || ct.len() > MAX_SEALED_BYTES {
            return Err(bad());
        }
        let info = b.info(&self.public, &epk_c);
        let key = agreement::agree_ephemeral(self.key, &agreement::UnparsedPublicKey::new(&agreement::X25519, &epk_c), (), |shared| key_for_domain(DOMAIN, shared, &info)).map_err(|_| bad())?;
        let r = match key.open_in_place(aead::Nonce::assume_unique_for_key([0u8; 12]), aead::Aad::from(&info), &mut ct) {
            Ok(plain) => match serde_json::from_slice::<Payload>(plain) {
                Ok(p) if check_text(&p.username) && check_text(&p.password) => {
                    Ok(Cred { username: p.username.into_owned(), password: p.password.into_owned(), expires_at: p.expires_at })
                }
                _ => Err("the git credential payload doesn't parse".to_string()),
            },
            Err(_) => Err(bad()),
        };
        wipe(&mut ct);
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "ghs_k2testGITCRED0123456789abcdefABCDEF";

    fn bind() -> Binding {
        Binding {
            controller_fp: "c".repeat(64),
            node_fp: "n".repeat(64),
            job_id: "j1".into(),
            generation: 2,
            attempt: 1,
            plan_digest: "sha256:abc".into(),
            req: 7,
            repo: "github.com/acme/app".into(),
            purpose: Purpose::Job,
        }
    }

    fn cred() -> Cred {
        Cred { username: USERNAME.into(), password: TOKEN.into(), expires_at: 1_900_000_000 }
    }

    #[test]
    fn seal_open_round_trip_and_debug_hides_the_token() {
        let n = NodeKey::generate().unwrap();
        let (epk_c, ct) = seal(&cred(), &n.public_b64(), &bind()).unwrap();
        assert!(!ct.contains(TOKEN), "ct carries the token");
        assert!(!crypto::unb64(&ct).unwrap().windows(TOKEN.len()).any(|w| w == TOKEN.as_bytes()), "ct carries the token");
        let c = n.open(&epk_c, &ct, &bind()).unwrap();
        assert_eq!(c, cred());
        assert!(!format!("{c:?}").contains(TOKEN), "Debug prints the token");
        assert_eq!(c.basic_header(), "Authorization: Basic eC1hY2Nlc3MtdG9rZW46Z2hzX2sydGVzdEdJVENSRUQwMTIzNDU2Nzg5YWJjZGVmQUJDREVG");
    }

    #[test]
    fn every_binding_field_and_any_tamper_fails() {
        let mutations: Vec<Box<dyn Fn(&mut Binding)>> = vec![
            Box::new(|b| b.controller_fp.push('x')),
            Box::new(|b| b.node_fp.push('x')),
            Box::new(|b| b.job_id = "j2".into()),
            Box::new(|b| b.generation += 1),
            Box::new(|b| b.attempt += 1),
            Box::new(|b| b.plan_digest.push('x')),
            Box::new(|b| b.req += 1),
            Box::new(|b| b.repo = "github.com/acme/other".into()),
            Box::new(|b| b.purpose = Purpose::Sync),
        ];
        for (i, m) in mutations.iter().enumerate() {
            let n = NodeKey::generate().unwrap();
            let (epk_c, ct) = seal(&cred(), &n.public_b64(), &bind()).unwrap();
            let mut other = bind();
            m(&mut other);
            assert!(n.open(&epk_c, &ct, &other).is_err(), "mutation {i} still opened");
        }
        let n = NodeKey::generate().unwrap();
        let (epk_c, ct) = seal(&cred(), &n.public_b64(), &bind()).unwrap();
        let mut raw = crypto::unb64(&ct).unwrap();
        raw[3] ^= 1;
        assert!(n.open(&epk_c, &crypto::b64(&raw), &bind()).is_err(), "tampered ct opened");
        // A secret-env seal with the same keys never opens as a git one.
        let n = NodeKey::generate().unwrap();
        let sb = crate::secret::Binding { controller_fp: "c".repeat(64), node_fp: "n".repeat(64), job_id: "j1".into(), generation: 2, attempt: 1, plan_digest: "sha256:abc".into() };
        let (epk_c, ct) = crate::secret::seal(&[("TOKEN".into(), TOKEN.as_bytes().to_vec())], &n.public_b64(), &sb).unwrap();
        assert!(n.open(&epk_c, &ct, &bind()).is_err(), "a secret-env seal opened as a git credential");
    }

    #[test]
    fn the_git_frames_go_the_right_way_and_keep_old_plans_digests() {
        use crate::frames::{Direction, Frame, GitCred, GitCredNeed, GitCredRefused};
        let need = Frame::GitCredNeed(GitCredNeed { job_id: "j".into(), generation: 1, attempt: 1, plan_digest: "d".into(), req: 1, repo: "github.com/acme/app".into(), purpose: Purpose::Sync, epk: "e".into() });
        assert_eq!((need.direction(), need.kind()), (Direction::NodeToController, "git_cred_need"));
        let wire = serde_json::to_value(&need).unwrap();
        assert_eq!((wire["t"].as_str(), wire["purpose"].as_str()), (Some("git_cred_need"), Some("sync")));
        let cred = Frame::GitCred(GitCred { job_id: "j".into(), generation: 1, req: 1, epk: "e".into(), ct: "c".into() });
        assert_eq!((cred.direction(), cred.kind()), (Direction::ControllerToNode, "git_cred"));
        let no = Frame::GitCredRefused(GitCredRefused { job_id: "j".into(), generation: 1, req: 1, code: codes::NOT_GRANTED.into(), message: "m".into() });
        assert_eq!((no.direction(), no.kind()), (Direction::ControllerToNode, "git_cred_refused"));
        const { assert!(crate::frames::PROTOCOL >= crate::frames::PROTOCOL_GIT && crate::frames::PROTOCOL_GIT > crate::host::PROTOCOL_HOST) };
        // An empty `git_access` is absent from the wire: older digests hold.
        let p: crate::frames::Plan = serde_json::from_value(serde_json::json!({
            "job_id": "j", "node_id": "n", "workspace_id": "w", "workspace_label": "k2", "requested_by": "x", "argv": ["true"],
            "env": {}, "cwd": null, "limits": {"max_secs": 1, "cpu_millis": null, "mem_bytes": null, "disk_bytes": 0, "log_cap_bytes": 1},
            "src": null, "exclusive": false, "created_at": 1
        }))
        .unwrap();
        assert!(serde_json::to_value(&p).unwrap().get("git_access").is_none());
        let mut q = p.clone();
        q.git_access = vec!["github.com/acme/app".into()];
        assert_ne!(p.digest().unwrap(), q.digest().unwrap(), "the repos are part of the digest");
    }

    #[test]
    fn seal_refuses_bad_text_and_keys() {
        let n = NodeKey::generate().unwrap();
        let mut c = cred();
        c.password = "has space".into();
        assert_eq!(seal(&c, &n.public_b64(), &bind()), Err(codes::UNAVAILABLE));
        c.password = "x".repeat(MAX_TOKEN_BYTES + 1);
        assert_eq!(seal(&c, &n.public_b64(), &bind()), Err(codes::UNAVAILABLE));
        assert_eq!(seal(&cred(), "short", &bind()), Err(codes::STALE));
    }

    #[test]
    fn repo_keys_are_normalized_and_github_only() {
        assert_eq!(parse_repo_arg("Acme/App").as_deref(), Some("github.com/acme/app"));
        assert_eq!(parse_repo_arg("github.com/acme/app.git").as_deref(), Some("github.com/acme/app"));
        assert_eq!(parse_repo_arg("https://GitHub.com/acme/app.git").as_deref(), Some("github.com/acme/app"));
        assert_eq!(parse_repo_arg("https://gitlab.com/acme/app"), None);
        assert_eq!(parse_repo_arg("acme"), None);
        assert_eq!(parse_repo_arg("acme/app/extra"), None);
        assert_eq!(parse_repo_arg("-acme/app"), None);
        assert_eq!(parse_repo_arg("acme/.."), None);
        assert_eq!(parse_repo_arg("acme/a b"), None);
        assert_eq!(repo_of_remote("https://github.com/Acme/App.git").as_deref(), Some("github.com/acme/app"));
        assert_eq!(repo_of_remote("https://github.com/acme/app/").as_deref(), Some("github.com/acme/app"));
        assert_eq!(repo_of_remote("https://user@github.com/acme/app"), None);
        assert_eq!(repo_of_remote("http://github.com/acme/app"), None);
        assert_eq!(repo_of_remote("https://example.com/acme/app"), None);
        assert_eq!(repo_key("github.com", "/acme/app.git").as_deref(), Some("github.com/acme/app"));
        assert_eq!(owner_repo("github.com/acme/app"), Some(("acme", "app")));
        assert_eq!(https_url("github.com/acme/app"), "https://github.com/acme/app");
        assert!(valid_plan_list(&["github.com/acme/app".into(), "github.com/acme/lib".into()]));
        assert!(!valid_plan_list(&["github.com/acme/lib".into(), "github.com/acme/app".into()]), "unsorted");
        assert!(!valid_plan_list(&["github.com/Acme/app".into()]), "not lowercase");
        assert!(!valid_plan_list(&["gitlab.com/acme/app".into()]));
    }
}
