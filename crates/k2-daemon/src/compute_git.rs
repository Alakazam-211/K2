//! `k2 compute git-access` (0.46.9, prd-compute-git-access-v1, GH#78):
//! short-lived, read-only git tokens for a workspace's compute jobs, for
//! the GitHub repos a person granted, without `--secret` (so the jobs keep
//! `--cache`, warm slots and sccache).
//!
//! - **Setup** ([`setup`], the owner or an Admin, never a passport): a
//!   GitHub App's id and private key (preferred: 1-hour tokens for one repo,
//!   `contents: read`) or a fine-grained token (`github_pat_…`; GitHub
//!   can't mint short-lived tokens from it, so the node gets the token
//!   itself). The key or token goes into the vault, system namespace,
//!   never deliverable; `k2.db` keeps the kind and the app id.
//!   `gh auth token` and classic tokens are refused.
//! - **Grants** ([`grant`], [`revoke`], [`show`]): (workspace, repo, node
//!   scope). `all` = every node this server owns, now and later; a node
//!   another server owns only when named with `--shared`, and only with an
//!   App. A person grants (an agent gets `git_access_needs_person` and the
//!   card `compute.git-access`); an agent may revoke its own workspace's.
//!   A grant checks each repo with GitHub first.
//! - **Plans** ([`plan_repos_for`]): at assign, a node offering
//!   `git-access` gets `Plan.git_access` = the job's workspace's grants that
//!   cover it.
//! - **Release** ([`release`], from `compute_ws` on its own thread): every
//!   check again for each `git_cred_need`, a token minted (or reused) for
//!   exactly that workspace and repo, sealed to the node's one-time key, an
//!   event on the job (never the token).
//!
//! Tokens exist here only in memory (the cache below) and in the sealed
//! answer; nothing of a token is written to `k2.db`, an event or a log.

use std::collections::{BTreeSet, HashMap};
use std::sync::Mutex;

use k2_core::compute::git::{self as cgit, Github, Minted};
use k2_core::compute::proto::frames::{GitCred, GitCredNeed};
use k2_core::compute::proto::gitcred::{self, codes as gcodes};
use k2_core::compute::{self as compute, store};
use k2_vault::{Actor, Delivery, Kind, Namespace, PutMode, PutRequest, Requester, Scope, SecretBytes, SecretStore, Selector, Use};
use serde_json::json;

use crate::compute_routes::{audit, b, json_body, need_credential, no, owner_only, resolve_node_for, s, target_ws, Who, Ws, R};

/// `GET` grants and the credential (`k2 compute git-access`).
pub const SHOW: &str = "/cli/compute/git-access";
/// `POST` grant repos to a workspace's jobs.
pub const GRANT: &str = "/cli/compute/git-access/grant";
/// `POST` take grants back.
pub const REVOKE: &str = "/cli/compute/git-access/revoke";
/// `POST` set or clear the server's GitHub credential.
pub const SETUP: &str = "/cli/compute/git-access/setup";

/// An agent's grant (exit 3; the card `compute.git-access`).
pub const NEEDS_PERSON: &str = "git_access_needs_person";
pub const BAD_REPO: &str = "git_access_bad_repo";
pub const BAD_CREDENTIAL: &str = "git_access_bad_credential";
/// GitHub said no while checking a repo at grant (or the App at setup).
pub const CHECK_FAILED: &str = "git_access_check_failed";

const SUBSYSTEM: &str = "compute-git";
const APP_KEY: &str = "github-app-key";
const TOKEN: &str = "github-token";
/// A minted App token is reused until this close to its expiry.
const REUSE_MARGIN_SECS: i64 = 15 * 60;
/// At most this many repos in one grant or revoke.
const MAX_REPOS_PER_CALL: usize = 32;

// ── the GitHub API base (a test's fake, else GitHub) ─────────────────

static API_BASE: Mutex<Option<String>> = Mutex::new(None);

fn api_base() -> String {
    API_BASE.lock().unwrap_or_else(|p| p.into_inner()).clone().unwrap_or_else(|| "https://api.github.com".into())
}

/// Test seam: point the GitHub client at a fake (`None`: GitHub again).
#[doc(hidden)]
pub fn test_set_api_base(base: Option<String>) {
    *API_BASE.lock().unwrap_or_else(|p| p.into_inner()) = base;
    forget_all_tokens();
}

fn github() -> Result<Github, (&'static str, String)> {
    Github::new(&api_base()).map_err(|e| (gcodes::UNAVAILABLE, e))
}

// ── the vault ────────────────────────────────────────────────────────

fn vault() -> Result<std::sync::Arc<k2_vault::LocalVault>, (&'static str, String)> {
    crate::vault::get().map_err(|e| (gcodes::VAULT_LOCKED, format!("the vault isn't open ({}); try again once it is", e.code)))
}

fn vault_put(name: &str, value: &[u8], what: &str) -> Result<(), (&'static str, String)> {
    let v = vault()?;
    let mut r = PutRequest::new(Namespace::System, Scope::engine(SUBSYSTEM), name, Kind::Generic);
    r.default_delivery = Some(Delivery::None);
    r.delivery_ceiling = Some(Delivery::None);
    r.origin = format!("system:{SUBSYSTEM}");
    r.description = format!("{what} for compute git access (k2 compute git-access); daemon-only.");
    r.mode = PutMode::Upsert;
    r.idempotent = true;
    v.put(r, SecretBytes::from_slice(value), &Actor::System(SUBSYSTEM.into())).map(|_| ()).map_err(|e| (gcodes::VAULT_LOCKED, format!("the vault refused ({})", e.code)))
}

fn vault_get(name: &str) -> Result<Option<String>, (&'static str, String)> {
    let v = vault()?;
    match v.resolve(&Selector::system(Scope::engine(SUBSYSTEM), name), Use::System, &Requester::System(SUBSYSTEM.into())) {
        Ok(r) => Ok(String::from_utf8(r.value.expose().to_vec()).ok()),
        Err(e) if e.status == 404 || e.code == "secret_not_active" => Ok(None),
        Err(e) => Err((gcodes::VAULT_LOCKED, format!("the vault refused ({})", e.code))),
    }
}

fn vault_drop(name: &str) {
    if let Ok(v) = crate::vault::get() {
        match v.destroy(&Selector::system(Scope::engine(SUBSYSTEM), name), &Actor::System(SUBSYSTEM.into())) {
            Ok(()) => {}
            Err(e) if e.status == 404 || e.code == "secret_destroyed" => {}
            Err(e) => k2_core::log_debug!("[compute-git] destroying {name}: WARN {}", e.code),
        }
    }
}

// ── tokens (memory only) ─────────────────────────────────────────────

/// Minted tokens by (workspace, repo): an App token is reused until close
/// to its expiry; a revoke drops (and for an App deletes) its own.
static TOKENS: Mutex<Option<HashMap<(String, String), Minted>>> = Mutex::new(None);

fn forget_all_tokens() {
    *TOKENS.lock().unwrap_or_else(|p| p.into_inner()) = None;
}

/// The tokens cached for `ws` and `repo`, taken out of the cache.
fn take_tokens(ws: &str, repo: &str) -> Vec<Minted> {
    let mut g = TOKENS.lock().unwrap_or_else(|p| p.into_inner());
    g.as_mut().and_then(|m| m.remove(&(ws.to_string(), repo.to_string()))).into_iter().collect()
}

/// A token for `ws`'s jobs to read `repo` (a key): a cached App token,
/// else a new one; a fine-grained token as it is. `Err((code, message))`.
pub fn mint(ws: &str, repo: &str) -> Result<(Minted, &'static str), (&'static str, String)> {
    let cred = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        cgit::credential(&conn)
    }
    .ok_or((gcodes::NO_CREDENTIAL, "this server has no GitHub credential for compute git access; a person runs: k2 compute git-access setup".to_string()))?;
    let (owner, name) = gitcred::owner_repo(repo).ok_or((gcodes::NOT_GRANTED, format!("{repo} isn't a GitHub repo key")))?;
    let now = compute::now();
    if cred.is_app() {
        if let Some(m) = TOKENS.lock().unwrap_or_else(|p| p.into_inner()).as_ref().and_then(|m| m.get(&(ws.to_string(), repo.to_string()))) {
            if m.expires_at - now > REUSE_MARGIN_SECS {
                return Ok((m.clone(), "app"));
            }
        }
        let pem = vault_get(APP_KEY)?.ok_or((gcodes::NO_CREDENTIAL, "the GitHub App's key is missing from the vault; a person runs k2 compute git-access setup again".to_string()))?;
        let key = cgit::app_key(&pem).map_err(|e| (gcodes::NO_CREDENTIAL, e))?;
        let app_id = cred.app_id.clone().unwrap_or_default();
        let jwt = cgit::app_jwt(&app_id, &key, now).map_err(|e| (gcodes::UNAVAILABLE, e))?;
        let gh = github()?;
        let forge = |(refused, m): (bool, String)| if refused { (gcodes::FORGE_REFUSED, m) } else { (gcodes::UNAVAILABLE, m) };
        let inst = gh.installation(&jwt, owner, name).map_err(|e| {
            let (c, m) = forge(e);
            (c, format!("{m} (is the GitHub App installed on {owner}/{name}?)"))
        })?;
        let minted = gh.installation_token(&jwt, inst, name).map_err(forge)?;
        TOKENS.lock().unwrap_or_else(|p| p.into_inner()).get_or_insert_with(HashMap::new).insert((ws.to_string(), repo.to_string()), minted.clone());
        Ok((minted, "app"))
    } else {
        let t = vault_get(TOKEN)?.ok_or((gcodes::NO_CREDENTIAL, "the fine-grained token is missing from the vault; a person runs k2 compute git-access setup again".to_string()))?;
        Ok((Minted { token: t, expires_at: now + cgit::TOKEN_NOMINAL_SECS }, "token"))
    }
}

/// Does the credential reach `repo`? An App: mint (and keep) a token; a
/// fine-grained token: `GET /repos/…` with it.
fn check_repo(ws: &str, repo: &str) -> R<()> {
    let is_app = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        cgit::credential(&conn).map(|c| c.is_app())
    };
    let fail = |m: String| no(CHECK_FAILED, 502, m);
    match is_app {
        None => Err(no(gcodes::NO_CREDENTIAL, 409, "set the server's GitHub credential first (a person, from a terminal outside K2): k2 compute git-access setup --github-app <app id> --key-file <app.pem>, or --github-token-file <file> for a fine-grained token")),
        Some(true) => mint(ws, repo).map(|_| ()).map_err(|(c, m)| if c == gcodes::VAULT_LOCKED { no(gcodes::VAULT_LOCKED, 503, m) } else { fail(format!("{repo}: {m}")) }),
        Some(false) => {
            let (owner, name) = gitcred::owner_repo(repo).ok_or_else(|| no(BAD_REPO, 400, format!("{repo} isn't a GitHub repo")))?;
            let t = vault_get(TOKEN).map_err(|(_, m)| no(gcodes::VAULT_LOCKED, 503, m))?.ok_or_else(|| no(gcodes::NO_CREDENTIAL, 409, "the fine-grained token is missing from the vault; set it again"))?;
            let gh = github().map_err(|(_, m)| fail(m))?;
            gh.can_read(&t, owner, name).map_err(|(_, m)| fail(format!("{repo}: {m} (does the fine-grained token include {owner}/{name}, with Contents read?)")))
        }
    }
}

// ── who ──────────────────────────────────────────────────────────────

/// A person here: the owner token or an Owner/Admin login (never a
/// passport: an agent, or a K2 shell tab, which follows the agent rules).
fn person(who: &Who) -> bool {
    who.is_admin()
}

fn repos_in(v: &serde_json::Value) -> R<Vec<String>> {
    let raw: Vec<String> = match v.get("github") {
        None | Some(serde_json::Value::Null) => Vec::new(),
        Some(serde_json::Value::String(s)) => s.split(',').map(str::trim).filter(|x| !x.is_empty()).map(str::to_string).collect(),
        Some(serde_json::Value::Array(a)) => a.iter().filter_map(|x| x.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        Some(_) => return Err(no(BAD_REPO, 400, "--github <owner>/<repo>[,<owner>/<repo>…]")),
    };
    if raw.len() > MAX_REPOS_PER_CALL {
        return Err(no(BAD_REPO, 400, format!("at most {MAX_REPOS_PER_CALL} repos at a time")));
    }
    let mut out = BTreeSet::new();
    for r in raw {
        let k = gitcred::parse_repo_arg(&r).ok_or_else(|| no(BAD_REPO, 400, format!("{r:?} isn't a GitHub repo: write <owner>/<repo> (github.com only in this release)")))?;
        out.insert(k);
    }
    Ok(out.into_iter().collect())
}

fn short(repo: &str) -> String {
    gitcred::owner_repo(repo).map(|(o, r)| format!("{o}/{r}")).unwrap_or_else(|| repo.to_string())
}

/// `nodes` in a request: `all` (or nothing) = `*`; else node names.
fn nodes_in(v: &serde_json::Value) -> Vec<String> {
    let list: Vec<String> = match v.get("nodes") {
        Some(serde_json::Value::String(s)) => s.split(',').map(str::trim).filter(|x| !x.is_empty()).map(str::to_string).collect(),
        Some(serde_json::Value::Array(a)) => a.iter().filter_map(|x| x.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        _ => Vec::new(),
    };
    if list.is_empty() || list.iter().any(|n| n == "all") {
        vec![cgit::ALL_NODES.to_string()]
    } else {
        list
    }
}

/// What a person reads before a token goes to another server's machine.
fn shared_warning(n: &store::NodeRow) -> String {
    format!(
        "'{}' belongs to another K2 server ({}): while a job uses a token there, that machine's admin (root) and its k2-node user can read it. \
         Tokens are read-only, for one repo, and live one hour",
        n.name,
        n.owner_label.as_deref().unwrap_or("its owner")
    )
}

// ── views ────────────────────────────────────────────────────────────

fn node_label(conn: &rusqlite::Connection, id: &str) -> String {
    if id == cgit::ALL_NODES {
        return "all".into();
    }
    store::node_by_id(conn, id).map(|n| n.name).unwrap_or_else(|| format!("{id} (removed)"))
}

fn view(ws: &Ws) -> serde_json::Value {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let cred = cgit::credential(&conn);
    let grants: Vec<serde_json::Value> = cgit::grants_of(&conn, &ws.id)
        .iter()
        .map(|g| {
            json!({
                "repo": short(&g.repo),
                "key": g.repo,
                "nodes": node_label(&conn, &g.nodes),
                "shared": g.shared,
                "grantedBy": g.granted_by,
                "grantedAt": g.granted_at,
            })
        })
        .collect();
    json!({
        "workspace": ws.handle,
        "credential": cred.map(|c| json!({"forge": "github", "kind": c.kind, "appId": c.app_id, "setBy": c.set_by, "setAt": c.set_at})),
        "grants": grants,
        "note": "jobs read these repos with short-lived, read-only tokens through git's credential helper; they keep --cache and warm slots (k2 study compute, GIT ACCESS)",
    })
}

/// `GET /cli/compute/git-access[?workspace=WS]`.
pub(crate) fn show(who: &Who, params: &HashMap<String, String>) -> R<serde_json::Value> {
    need_credential(who)?;
    let ws = target_ws(who, params.get("workspace").map(String::as_str))?;
    Ok(view(&ws))
}

// ── setup ────────────────────────────────────────────────────────────

/// `POST /cli/compute/git-access/setup {app_id, key_pem} | {token} | {clear}`:
/// the owner or an Admin only (a raw credential is typed outside K2).
pub(crate) fn setup(who: &Who, body: &[u8]) -> R<serde_json::Value> {
    need_credential(who)?;
    if !person(who) {
        return Err(owner_only());
    }
    let v = json_body(body)?;
    let vault_err = |(c, m): (&'static str, String)| no(c, 503, m);
    if b(&v, "clear") == Some(true) {
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            cgit::clear_credential(&conn).map_err(|e| no("internal", 500, e))?;
        }
        vault_drop(APP_KEY);
        vault_drop(TOKEN);
        forget_all_tokens();
        audit(who, "git_access_setup", None, None, None, json!({"cleared": true}));
        return Ok(json!({"credential": null, "note": "cleared: every later ask for a git token is refused (git_access_no_credential); grants stay"}));
    }
    let (app_id, key, token) = (s(&v, "app_id"), s(&v, "key_pem"), s(&v, "token"));
    let out = match (app_id, key, token) {
        (Some(id), Some(pem), None) => {
            if !cgit::valid_app_id(id) {
                return Err(no(BAD_CREDENTIAL, 400, "--github-app takes the App's numeric id (its settings page: App ID)"));
            }
            let k = cgit::app_key(pem).map_err(|e| no(BAD_CREDENTIAL, 400, format!("{e} (download a private key from the App's settings page: a .pem file)")))?;
            let jwt = cgit::app_jwt(id, &k, compute::now()).map_err(|e| no(BAD_CREDENTIAL, 400, e))?;
            let gh = github().map_err(|(_, m)| no(CHECK_FAILED, 502, m))?;
            let slug = gh.app_slug(&jwt).map_err(|(_, m)| no(CHECK_FAILED, 502, format!("GitHub didn't accept the App's key: {m}")))?;
            vault_put(APP_KEY, pem.as_bytes(), "A GitHub App's private key").map_err(vault_err)?;
            vault_drop(TOKEN);
            let db = k2_core::db::shared();
            let conn = db.lock();
            cgit::set_credential(&conn, "app", Some(id), &who.label(), compute::now()).map_err(|e| no("internal", 500, e))?;
            drop(conn);
            json!({"credential": {"kind": "app", "appId": id, "app": slug}, "note": "1-hour, read-only tokens for one repo each. Install the App on the repos (Contents: read), then: k2 compute git-access grant --github <owner>/<repo>"})
        }
        (None, None, Some(t)) => {
            if let Some(why) = cgit::token_refusal(t) {
                return Err(no(BAD_CREDENTIAL, 400, why));
            }
            vault_put(TOKEN, t.trim().as_bytes(), "A fine-grained GitHub token").map_err(vault_err)?;
            vault_drop(APP_KEY);
            let db = k2_core::db::shared();
            let conn = db.lock();
            cgit::set_credential(&conn, "token", None, &who.label(), compute::now()).map_err(|e| no("internal", 500, e))?;
            drop(conn);
            json!({
                "credential": {"kind": "token"},
                "note": "GitHub can't mint short-lived tokens from a fine-grained token, so nodes get this token itself (in memory, for each git call; never on another server's node). Its limits are the ones you set on GitHub: keep it read-only, to these repos, with an expiry. A GitHub App gives 1-hour tokens instead",
            })
        }
        _ => {
            return Err(no(
                BAD_CREDENTIAL,
                400,
                "say one: --github-app <app id> --key-file <app.pem> (preferred), --github-token-file <file> (a fine-grained token, github_pat_…), or --clear",
            ))
        }
    };
    forget_all_tokens();
    audit(who, "git_access_setup", None, None, None, json!({"kind": out["credential"]["kind"]}));
    Ok(out)
}

// ── grant / revoke ───────────────────────────────────────────────────

/// `POST /cli/compute/git-access/grant {workspace?, github, nodes?, shared?}`.
pub(crate) fn grant(who: &Who, body: &[u8]) -> R<serde_json::Value> {
    need_credential(who)?;
    let v = json_body(body)?;
    let repos = repos_in(&v)?;
    if repos.is_empty() {
        return Err(no(BAD_REPO, 400, "name the repos: --github <owner>/<repo>[,<owner>/<repo>…]"));
    }
    if !person(who) {
        if who.passport().is_none() {
            return Err(owner_only());
        }
        let ws = target_ws(who, s(&v, "workspace"))?;
        let list: Vec<String> = repos.iter().map(|r| short(r)).collect();
        return Err(no(
            NEEDS_PERSON,
            403,
            format!(
                "giving {}'s compute jobs read access to {} sends short-lived tokens for them to its nodes, so only a person does it: the owner or an Admin, \
                 from a terminal outside K2 (k2 compute git-access grant --github {} --workspace {})",
                ws.handle,
                list.join(", "),
                list.join(","),
                ws.handle
            ),
        )
        .with("workspace", ws.handle.clone())
        .with("repos", list.join(",")));
    }
    let ws = target_ws(who, s(&v, "workspace"))?;
    let shared_ok = b(&v, "shared") == Some(true);
    let cred = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        cgit::credential(&conn)
    };
    // Each named node: owned, or another server's with --shared (and an App).
    let mut scopes: Vec<(String, bool)> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    for name in nodes_in(&v) {
        if name == cgit::ALL_NODES {
            scopes.push((name, false));
            continue;
        }
        let n = resolve_node_for(who, Some(&name), Some(&ws.id))?;
        if n.owned_for_secrets() {
            scopes.push((n.id, false));
            continue;
        }
        if !shared_ok {
            return Err(no(
                gcodes::SHARED_NEEDS_OPTIN,
                400,
                format!("{}. Grant it there only on purpose: add --shared (k2 compute git-access grant … --nodes {} --shared)", shared_warning(&n), n.name),
            )
            .with("node", n.name.clone()));
        }
        if !cred.as_ref().is_some_and(|c| c.is_app()) {
            return Err(no(
                gcodes::PAT_SHARED,
                400,
                format!("a fine-grained token never goes to another server's node ('{}'): its own limits are all it has. Set up a GitHub App (1-hour tokens for one repo): k2 compute git-access setup --github-app <id> --key-file <app.pem>", n.name),
            ));
        }
        warnings.push(shared_warning(&n));
        scopes.push((n.id, true));
    }
    for r in &repos {
        check_repo(&ws.id, r)?;
    }
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        for r in &repos {
            for (scope, shared) in &scopes {
                cgit::add_grant(&conn, &ws.id, r, scope, *shared, &who.label(), compute::now()).map_err(|e| no("internal", 500, e))?;
            }
        }
    }
    audit(who, "git_access_grant", None, Some(&ws.id), None, json!({"repos": repos, "nodes": scopes.iter().map(|(s, _)| s).collect::<Vec<_>>()}));
    let mut out = view(&ws);
    out["granted"] = repos.iter().map(|r| short(r)).collect::<Vec<_>>().into();
    out["warnings"] = warnings.into();
    if cred.as_ref().is_some_and(|c| !c.is_app()) {
        out["tokenNote"] = "a fine-grained token: nodes get the token itself (GitHub can't mint short-lived ones from it); a GitHub App gives 1-hour tokens".into();
    }
    Ok(out)
}

/// Running jobs of `ws` that were handed a token for `repo`.
fn running_with(ws: &str, repo: &str) -> Vec<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let Ok(mut st) = conn.prepare(
        "SELECT DISTINCT e.job_id FROM compute_events e JOIN compute_jobs j ON j.id = e.job_id \
         WHERE e.kind = 'git_cred_release' AND e.workspace_id = ?1 AND j.state IN ('assigned','preparing','running','finishing') \
         AND json_extract(e.detail_json, '$.repo') = ?2 ORDER BY e.job_id",
    ) else {
        return Vec::new();
    };
    st.query_map(rusqlite::params![ws, repo], |r| r.get::<_, String>(0)).map(|rows| rows.filter_map(Result::ok).collect()).unwrap_or_default()
}

/// `POST /cli/compute/git-access/revoke {workspace?, github | all, nodes?}`:
/// a person, or an agent for its own workspace (it only narrows).
pub(crate) fn revoke(who: &Who, body: &[u8]) -> R<serde_json::Value> {
    need_credential(who)?;
    if !person(who) && who.passport().is_none() {
        return Err(owner_only());
    }
    let v = json_body(body)?;
    let ws = target_ws(who, s(&v, "workspace"))?;
    let all = b(&v, "all") == Some(true);
    let repos = if all {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let set: BTreeSet<String> = cgit::grants_of(&conn, &ws.id).into_iter().map(|g| g.repo).collect();
        set.into_iter().collect()
    } else {
        repos_in(&v)?
    };
    if repos.is_empty() && !all {
        return Err(no(BAD_REPO, 400, "name the repos (--github <owner>/<repo>[,…]) or --all"));
    }
    // Scopes to remove: every scope, or the named ones (`all` = `*`).
    let named: Option<Vec<String>> = match v.get("nodes") {
        None | Some(serde_json::Value::Null) => None,
        _ => {
            let mut ids = Vec::new();
            for n in nodes_in(&v) {
                ids.push(if n == cgit::ALL_NODES { n } else { resolve_node_for(who, Some(&n), Some(&ws.id))?.id });
            }
            Some(ids)
        }
    };
    let mut removed = 0usize;
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        for r in &repos {
            match &named {
                None => removed += cgit::remove_grants(&conn, &ws.id, r, None).map_err(|e| no("internal", 500, e))?.len(),
                Some(ids) => {
                    for id in ids {
                        removed += cgit::remove_grants(&conn, &ws.id, r, Some(id)).map_err(|e| no("internal", 500, e))?.len();
                    }
                }
            }
        }
    }
    // Tokens already minted for these repos stop now (an App's are deleted
    // at GitHub); a fine-grained token stays valid there until rotated.
    let is_app = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        cgit::credential(&conn).map(|c| c.is_app())
    };
    let mut deleted = 0usize;
    let mut running: Vec<serde_json::Value> = Vec::new();
    for r in &repos {
        let still = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            cgit::grants_of(&conn, &ws.id).iter().any(|g| &g.repo == r)
        };
        if still {
            continue;
        }
        for m in take_tokens(&ws.id, r) {
            if is_app == Some(true) {
                if let Ok(gh) = github() {
                    if gh.delete_token(&m.token).is_ok() {
                        deleted += 1;
                    }
                }
            }
        }
        for j in running_with(&ws.id, r) {
            running.push(json!({"job": compute::message::short(&j), "repo": short(r)}));
        }
    }
    audit(who, "git_access_revoke", None, Some(&ws.id), None, json!({"repos": repos, "removed": removed}));
    let mut out = view(&ws);
    out["revoked"] = repos.iter().map(|r| short(r)).collect::<Vec<_>>().into();
    out["removed"] = removed.into();
    out["tokensDeleted"] = deleted.into();
    out["runningJobs"] = running.into();
    out["note"] = if is_app == Some(false) {
        "new asks are refused now; a fine-grained token already handed out stays valid at GitHub until you rotate or delete it there".into()
    } else {
        "new asks are refused now and the tokens already handed out were deleted at GitHub; running jobs keep running, their next git call for these repos fails".into()
    };
    Ok(out)
}

// ── plans and the relay hint ─────────────────────────────────────────

/// `Plan.git_access` for `job` on its node with `features`: its
/// workspace's grants covering that node, only when the node offers
/// `git-access`.
pub fn plan_repos_for(conn: &rusqlite::Connection, job: &store::JobRow, features: &[String]) -> Vec<String> {
    if !features.iter().any(|f| f == gitcred::FEATURE_GIT_ACCESS) {
        return Vec::new();
    }
    match store::node_by_id(conn, &job.node_id) {
        Some(n) => cgit::plan_repos(conn, &job.workspace_id, &n),
        None => Vec::new(),
    }
}

/// The `relay_cap` hint for a bundle of `remote`: the grant command when
/// it's on GitHub.
pub fn relay_hint(remote: Option<&str>) -> String {
    match remote.and_then(gitcred::repo_of_remote) {
        Some(r) => format!(". For a private GitHub repo, let the node fetch it itself: k2 compute git-access grant --github {} (a person)", short(&r)),
        None => String::new(),
    }
}

// ── release ──────────────────────────────────────────────────────────

/// A refusal to release: `(code, message)`.
pub type Refused = (&'static str, String);

/// (job, generation, repo) releases in flight.
static IN_FLIGHT: Mutex<Option<BTreeSet<(String, u32, String)>>> = Mutex::new(None);

/// A taken in-flight place; dropping it frees it.
pub struct Place((String, u32, String));

impl Drop for Place {
    fn drop(&mut self) {
        let mut g = IN_FLIGHT.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(s) = g.as_mut() {
            s.remove(&self.0);
        }
    }
}

/// One release per (job, generation, repo) at a time; `None` = dropped.
pub fn begin(n: &GitCredNeed) -> Option<Place> {
    let key = (n.job_id.clone(), n.generation, n.repo.clone());
    let mut g = IN_FLIGHT.lock().unwrap_or_else(|p| p.into_inner());
    let set = g.get_or_insert_with(BTreeSet::new);
    if !set.insert(key.clone()) {
        return None;
    }
    Some(Place(key))
}

/// Check everything for one `git_cred_need` and seal a token. `Err(None)`:
/// not a job of this node (dropped, no answer, no row).
pub fn release(node_id: &str, n: &GitCredNeed) -> Result<(GitCred, &'static str, i64), Option<Refused>> {
    let stale = |m: &str| Some((gcodes::STALE, m.to_string()));
    let (job, node) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        (store::job_by_id(&conn, &n.job_id), store::node_by_id(&conn, node_id))
    };
    let (Some(job), Some(node)) = (job, node) else { return Err(None) };
    if job.node_id != node_id {
        return Err(None);
    }
    if job.generation != n.generation as i64 || job.attempt != n.attempt as i64 || !k2_core::compute::proto::crypto::ct_eq(job.plan_digest.as_bytes(), n.plan_digest.as_bytes()) {
        return Err(stale("not this job's current attempt on this node"));
    }
    if !matches!(job.state.as_str(), "assigned" | "preparing" | "running") {
        return Err(stale(&format!("the job is {}", job.state)));
    }
    let plan = job.plan().ok_or_else(|| stale("the job's plan is unreadable"))?;
    if !plan.git_access.iter().any(|r| r == &n.repo) {
        return Err(Some((gcodes::NOT_GRANTED, format!("{} isn't in this job's plan (a person runs: k2 compute git-access grant --github {})", n.repo, short(&n.repo)))));
    }
    // The live grant, now (a revoke since the plan was written refuses).
    let (grant, cred) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        (cgit::covering(&conn, &job.workspace_id, &n.repo, &node), cgit::credential(&conn))
    };
    if grant.is_none() {
        let why = if node.owned_for_secrets() {
            format!("{} isn't granted to this workspace's jobs on '{}' any more (revoked)", short(&n.repo), node.name)
        } else {
            format!("{} isn't granted to this workspace's jobs on '{}' (another server's node: it must be named with --shared)", short(&n.repo), node.name)
        };
        return Err(Some((gcodes::NOT_GRANTED, why)));
    }
    if !node.owned_for_secrets() && !cred.as_ref().is_some_and(|c| c.is_app()) {
        return Err(Some((gcodes::PAT_SHARED, "a fine-grained token never goes to another server's node".into())));
    }
    let key = compute::controller_key().map_err(|e| Some((gcodes::UNAVAILABLE, format!("controller key: {e}"))))?;
    let (minted, kind) = mint(&job.workspace_id, &n.repo).map_err(|(c, m)| Some((c, m)))?;
    let cred = gitcred::Cred { username: gitcred::USERNAME.into(), password: minted.token.clone(), expires_at: minted.expires_at };
    let binding = gitcred::Binding {
        controller_fp: key.fingerprint(),
        node_fp: node.fingerprint.clone(),
        job_id: job.id.clone(),
        generation: n.generation,
        attempt: n.attempt,
        plan_digest: job.plan_digest.clone(),
        req: n.req,
        repo: n.repo.clone(),
        purpose: n.purpose,
    };
    let (epk, ct) = gitcred::seal(&cred, &n.epk, &binding).map_err(|c| Some((c, "the node's key for this ask is unusable".to_string())))?;
    Ok((GitCred { job_id: job.id.clone(), generation: n.generation, req: n.req, epk, ct }, kind, minted.expires_at))
}

/// The event for a release or refusal (names only, never a token).
pub fn record(node_id: &str, n: &GitCredNeed, r: &Result<(&'static str, i64), Refused>) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let ws = store::job_by_id(&conn, &n.job_id).map(|j| j.workspace_id);
    let (kind, detail) = match r {
        Ok((k, exp)) => ("git_cred_release", json!({"generation": n.generation, "repo": n.repo, "purpose": n.purpose.as_str(), "kind": k, "expiresAt": exp})),
        Err((code, msg)) => ("git_cred_refused", json!({"generation": n.generation, "repo": n.repo, "purpose": n.purpose.as_str(), "code": code, "detail": msg})),
    };
    store::record_event(&conn, "controller", kind, Some(node_id), ws.as_deref(), Some(&n.job_id), detail);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repos_and_nodes_parse_from_a_list_or_a_comma_string() {
        let v = json!({"github": "Acme/App, acme/lib.git,,acme/app", "nodes": "all"});
        assert_eq!(repos_in(&v).expect("repos"), vec!["github.com/acme/app", "github.com/acme/lib"]);
        assert_eq!(nodes_in(&v), vec!["*"]);
        assert_eq!(nodes_in(&json!({"nodes": ["k2n9", " k2n10 "]})), vec!["k2n9", "k2n10"]);
        assert_eq!(nodes_in(&json!({})), vec!["*"], "no nodes = every owned node");
        assert_eq!(repos_in(&json!({"github": "gitlab.com/acme/app"})).expect_err("gitlab").code, BAD_REPO);
        assert_eq!(repos_in(&json!({"github": 3})).expect_err("number").code, BAD_REPO);
        assert_eq!(relay_hint(Some("https://github.com/Acme/App.git")), ". For a private GitHub repo, let the node fetch it itself: k2 compute git-access grant --github acme/app (a person)");
        assert_eq!(relay_hint(Some("https://example.com/a/b")), "");
        assert_eq!(relay_hint(None), "");
    }
}
