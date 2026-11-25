//! `POST /cli/users/claim-first-owner`: a server's first owner login, made
//! by the person whose k2.dev account the server is linked to (0.46.18).
//!
//! Since 0.46.15 the owner token can't make an owner login without this
//! computer's own user check (`presence::OWNER_LOGIN`), and a Linux server
//! with no owner login has no password to check. A hosted server is linked
//! to the buyer's k2.dev account, so its first owner comes from there: the
//! k2.dev dashboard's backend sends the first owner login (the username and
//! initial password it keeps for the buyer's "reveal") here over the
//! server's K2 Connect tunnel, signed with the dashboard's edge key, right
//! after provisioning. K2 Web signs a claim only for a server row its own
//! records link to that buyer's account (its side of the contract,
//! `.k2/prds/contract-first-owner-claim-v1.md`). A retry after a lost
//! answer gets 409 `owner_exists`; the dashboard confirms it is its own
//! login by signing in with it.
//!
//! Every one of these must hold, or nothing is made:
//!
//! 1. **The tunnel only.** Loopback, LAN and App ingress get a plain 404:
//!    the owner token, agents, the CLI and every on-box caller never reach
//!    it.
//! 2. **The dashboard's signature** (`X-K2-Edge-Sig`, a `k2-edge-dash-…`
//!    kid, [`k2_core::edge_attest::verify_dashboard`]): over this path, the
//!    exact body, and this daemon's own subdomain, fresh and never replayed.
//!    Only the BAKED dashboard keys count: `~/.k2/edge-keys.json` (in the
//!    daemon user's home, writable from this box) never adds one in a
//!    release build, and the `*.app.k2.dev` Worker's keys (which proxy any
//!    browser) never count. Unsigned or badly signed → the same plain 404.
//! 3. **No owner login at all** (enabled or disabled). Once one exists this
//!    answers 409 `owner_exists` for good; the owner adds others in person.
//! 4. A new username (an existing login is never promoted or re-keyed) and
//!    a password that meets the server's policy.
//!
//! With `opsUsername` (k2.dev's website-management login, `k2cloud`) the
//! same claim also makes that login an owner, with a password this server
//! makes and returns once (`opsPassword`) — the portal then works as a
//! k2cloud session, as before. Both names must be new; if the second
//! can't be made, the first is taken back (the server stays claimable).
//!
//! One claim at a time (a process lock around the check and the write).
//! Every attempt is audited (`first-owner-claim`, with the k2.dev account
//! the dashboard names); the password never is.

use std::sync::Mutex;

use k2_core::connect_users::{self, Role};

use crate::cli_response::CliResponse;
use crate::routes::dispatcher::Ingress;

pub const PATH: &str = "/cli/users/claim-first-owner";
pub const AUDIT_EVENT: &str = "first-owner-claim";

/// Serializes the zero-owner check and the write.
static CLAIM_LOCK: Mutex<()> = Mutex::new(());

#[derive(serde::Deserialize)]
struct ClaimReq {
    username: String,
    password: String,
    /// The k2.dev account the dashboard signed this for (audit only; the
    /// signature is what vouches for it).
    #[serde(default, rename = "k2Account")]
    k2_account: Option<String>,
    /// The website-management login (k2.dev's `k2cloud`), made an owner in
    /// the same claim with a password this server makes and returns.
    #[serde(default, rename = "opsUsername")]
    ops_username: Option<String>,
    #[serde(default, rename = "firstName")]
    first_name: Option<String>,
    #[serde(default, rename = "lastName")]
    last_name: Option<String>,
}

/// The username a claim body names (for refusal audit lines), else `-`.
pub fn body_username(body: &[u8]) -> String {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("username").and_then(|u| u.as_str()).map(str::to_string))
        .unwrap_or_else(|| "-".to_string())
}

fn audit(user: &str, outcome: &str, ingress: &str) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(AUDIT_EVENT, user, outcome, ingress, "-", "api"));
}

/// A refusal before the claim is even read (off the tunnel, or no valid
/// dashboard signature): audited with its reason; the wire answer is the
/// caller's plain 404.
pub fn audit_refused(body: &[u8], reason: &str, ingress: &str) {
    audit(&body_username(body), &format!("deny {reason}"), ingress);
}

/// The door's own check, before the claim is read: the tunnel, a POST, and
/// the dashboard's signature over (`path`, `body`, `expected_sub`). `Err`
/// carries the audit reason; the wire answer is always the same plain 404.
pub fn admit(
    ingress: Ingress,
    is_post: bool,
    header: Option<&str>,
    path: &str,
    body: &[u8],
    expected_sub: &str,
    now: i64,
) -> Result<k2_core::edge_attest::Attestation, String> {
    match ingress {
        Ingress::Tunnel => {}
        Ingress::Loopback | Ingress::Lan | Ingress::App => return Err("off_tunnel".to_string()),
    }
    if !is_post {
        return Err("not_post".to_string());
    }
    k2_core::edge_attest::verify_dashboard(header, "POST", path, body, expected_sub, now).map_err(|e| format!("bad_attest:{}", e.reason()))
}

fn json_err(status: &'static str, code: &str, message: &str) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({ "error": { "code": code, "message": message } }).to_string(),
    }
}

/// Logins with the owner role, enabled or not.
fn owner_login_count() -> Result<usize, String> {
    Ok(connect_users::list_users()?.into_iter().filter(|u| u.role == Role::Owner).count())
}

/// The claim itself, after the dispatcher checked the tunnel and the
/// dashboard signature (`ingress` = `edge:<kid>`, for the audit).
pub fn handle(body: &[u8], ingress: &str) -> CliResponse {
    let req: ClaimReq = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => {
            audit(&body_username(body), "deny bad_request", ingress);
            return CliResponse::bad_request(format!("invalid JSON body: {e}"));
        }
    };
    let account = req.k2_account.as_deref().map(str::trim).filter(|a| !a.is_empty()).unwrap_or("-").to_string();
    let _g = CLAIM_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    match owner_login_count() {
        Ok(0) => {}
        Ok(_) => {
            audit(&req.username, &format!("deny owner_exists account={account}"), ingress);
            return json_err(
                "409 Conflict",
                "owner_exists",
                "this server already has an owner login, so it can't be claimed. Its owner adds other owners in person (k2 users set-role).",
            );
        }
        Err(e) => {
            audit(&req.username, "deny store_error", ingress);
            return CliResponse::internal_error(e);
        }
    }
    let user = match connect_users::normalize_username(&req.username) {
        Ok(u) => u,
        Err(e) => {
            audit(&req.username, &format!("deny invalid account={account}"), ingress);
            return CliResponse::bad_request(e);
        }
    };
    let ops = match req.ops_username.as_deref().map(str::trim).filter(|o| !o.is_empty()) {
        None => None,
        Some(o) => match connect_users::normalize_username(o) {
            Ok(o) if o == user => {
                audit(&user, &format!("deny invalid account={account}"), ingress);
                return CliResponse::bad_request("opsUsername must differ from username");
            }
            Ok(o) => Some(o),
            Err(e) => {
                audit(&user, &format!("deny invalid account={account}"), ingress);
                return CliResponse::bad_request(format!("opsUsername: {e}"));
            }
        },
    };
    for name in std::iter::once(&user).chain(ops.iter()) {
        if connect_users::role_for_user(name).is_some() {
            audit(&user, &format!("deny exists:{name} account={account}"), ingress);
            return json_err("409 Conflict", "username_taken", &format!("a login named {name} already exists on this server: nothing was made"));
        }
    }
    let new = connect_users::NewLogin { role: Some(Role::Owner), must_change_password: false, shares: Default::default(), fixed_password: false };
    if let Err(e) = connect_users::add_user_with_role(&user, &req.password, req.first_name.as_deref(), req.last_name.as_deref(), &new) {
        audit(&user, &format!("deny invalid account={account}"), ingress);
        return CliResponse::bad_request(e);
    }
    // The website-management login: an owner too, with a password this
    // server makes (returned once, in this answer only). If it can't be
    // made, the first owner is taken back so the server stays claimable.
    let ops_password = match &ops {
        None => None,
        Some(o) => {
            let pw = k2_core::skin::generate_password();
            if let Err(e) = connect_users::add_user_with_role(o, &pw, None, None, &new) {
                let undo = connect_users::remove_user(&user);
                audit(&user, &format!("deny ops_failed undo={} account={account}", if undo.is_ok() { "ok" } else { "FAILED" }), ingress);
                return match undo {
                    Ok(()) => CliResponse::internal_error(format!("the website-management login could not be made ({e}); nothing was kept")),
                    Err(u) => CliResponse::internal_error(format!("the website-management login could not be made ({e}), and taking back {user} failed ({u})")),
                };
            }
            Some(pw)
        }
    };
    let ops_line = ops.as_deref().map(|o| format!(" ops={o}")).unwrap_or_default();
    audit(&user, &format!("ok account={account}{ops_line}"), ingress);
    let mut out = serde_json::json!({ "success": true, "username": user, "role": "owner" });
    if let (Some(o), Some(pw)) = (ops, ops_password) {
        out["opsUsername"] = serde_json::Value::String(o);
        out["opsPassword"] = serde_json::Value::String(pw);
    }
    CliResponse::ok_json(out.to_string())
}

#[cfg(test)]
#[path = "first_owner_claim_tests.rs"]
mod tests;
