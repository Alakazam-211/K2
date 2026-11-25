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
//! records link to that buyer's account (its side of the contract, which
//! K2 Web holds: when to call, retries, the answer, no password logged). A
//! retry after a lost answer gets 409 `already_claimed`; the dashboard
//! confirms the login is its own by signing in with it.
//!
//! Hosted servers only, once. Every one of these must hold, or nothing is
//! made:
//!
//! 0. **Our own brand and the hosted-claim marker.** A build of another
//!    brand answers a plain 404 whatever its files say. The marker is a
//!    file the provisioner writes as root, [`MARKER_PATH`], root-owned,
//!    mode 0600, in a root-owned folder no group or other may write: the
//!    daemon only looks at it (it can't read, change or remove it), and a
//!    box without it (a self-hosted server) answers a plain 404.
//! 1. **The tunnel only.** Loopback, LAN and App ingress get a plain 404.
//!    The tunnel listener itself is on 127.0.0.1, so a process on this box
//!    can reach it; what it can't do is use it, because of 2.
//! 2. **The dashboard's signature** (`X-K2-Edge-Sig`, a `k2-edge-dash-…`
//!    kid, [`k2_core::edge_attest::verify_dashboard`]): over this path, the
//!    exact body, and this daemon's own subdomain, fresh and never replayed.
//!    Only the BAKED dashboard keys count: `~/.k2/edge-keys.json` (in the
//!    daemon user's home, writable from this box) never adds one in a
//!    release build, and the `*.app.k2.dev` Worker's keys (which proxy any
//!    browser) never count. Unsigned or badly signed → the same plain 404.
//! 3. **Never claimed before.** The first claim that makes its logins
//!    burns the door: a record in the login store
//!    ([`connect_users::record_first_owner_claimed`]) that removing,
//!    demoting or disabling logins never clears. After it, a signed claim
//!    gets 409 `already_claimed`, for good, even with no owner left.
//! 4. **No owner login at all** (enabled or disabled), as a second check:
//!    409 `owner_exists`; the owner adds others in person.
//! 5. A new username (an existing login is never promoted or re-keyed) and
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

/// The hosted-claim marker: written by the provisioner as root (0600, in
/// the root helper's root-owned config folder) on a hosted server only.
pub const MARKER_PATH: &str = "/etc/k2-root-helper/hosted-claim";

/// Where the marker is looked for, and the owner it must have. A release
/// build: [`MARKER_PATH`], root. A test build (`test-util`): the same name
/// under `~/.k2/test-root-etc/`, owned by the test's own user, so tests
/// can make, break and remove it.
pub fn marker_location() -> (std::path::PathBuf, u32) {
    if k2_core::TEST_BUILD {
        (k2_core::paths::k2_home().join("test-root-etc").join("hosted-claim"), current_uid())
    } else {
        (std::path::PathBuf::from(MARKER_PATH), 0)
    }
}

#[cfg(unix)]
fn current_uid() -> u32 {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() }
}

#[cfg(not(unix))]
fn current_uid() -> u32 {
    0
}

/// Is `path` a marker this server can trust: a regular file (not a link)
/// owned by `owner_uid` with mode exactly 0600, in a folder owned by
/// `owner_uid` that no group or other can write? Only its metadata is read
/// (the daemon can't read a root-owned 0600 file, and needn't). `Err` is
/// the audit reason.
#[cfg(unix)]
pub fn marker_ok(path: &std::path::Path, owner_uid: u32) -> Result<(), &'static str> {
    use std::os::unix::fs::MetadataExt;
    let m = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err("no_marker"),
        Err(_) => return Err("marker_unreadable"),
    };
    if !m.file_type().is_file() {
        return Err("marker_not_file");
    }
    if m.uid() != owner_uid {
        return Err("marker_owner");
    }
    if m.mode() & 0o7777 != 0o600 {
        return Err("marker_mode");
    }
    let dir = path.parent().ok_or("marker_dir")?;
    match std::fs::symlink_metadata(dir) {
        Ok(d) if d.file_type().is_dir() && d.uid() == owner_uid && d.mode() & 0o022 == 0 => Ok(()),
        _ => Err("marker_dir"),
    }
}

/// No hosted claim off Unix (hosted servers are Linux).
#[cfg(not(unix))]
pub fn marker_ok(_path: &std::path::Path, _owner_uid: u32) -> Result<(), &'static str> {
    Err("no_marker")
}

/// What opens the door at all, before any signature is looked at: our own
/// brand, and a trusted marker.
pub struct Opener {
    pub ours: bool,
    pub marker: Result<(), &'static str>,
}

impl Opener {
    /// This build and this box, now.
    pub fn here() -> Self {
        let (path, uid) = marker_location();
        Opener { ours: k2_core::brand::IS_OURS, marker: marker_ok(&path, uid) }
    }
}

/// The door's own check, before the claim is read: the tunnel, a POST, our
/// brand, the marker, and the dashboard's signature over (`path`, `body`,
/// `expected_sub`). `Err` carries the audit reason; the wire answer is
/// always the same plain 404.
#[allow(clippy::too_many_arguments)]
pub fn admit(
    ingress: Ingress,
    is_post: bool,
    opener: &Opener,
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
    if !opener.ours {
        return Err("not_our_brand".to_string());
    }
    opener.marker.map_err(str::to_string)?;
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
    match connect_users::first_owner_claimed() {
        Ok(None) => {}
        Ok(Some(_)) => {
            audit(&req.username, &format!("deny already_claimed account={account}"), ingress);
            return json_err("409 Conflict", "already_claimed", "this server's first owner was already claimed; it can't be claimed again.");
        }
        Err(e) => {
            audit(&req.username, "deny store_error", ingress);
            return CliResponse::internal_error(e);
        }
    }
    match owner_login_count() {
        Ok(0) => {}
        Ok(_) => {
            audit(&req.username, &format!("deny owner_exists account={account}"), ingress);
            return json_err(
                "409 Conflict",
                "owner_exists",
                &format!(
                    "this server already has an owner login, so it can't be claimed. Its owner adds other owners in person ({} users set-role).",
                    k2_core::brand::CLI_NAME
                ),
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
    // Burn the door. If the record can't be written, the logins are taken
    // back: a claim either makes its logins AND closes for good, or makes
    // nothing.
    let note = format!("at={} user={user}{ops_line} account={account}", k2_core::edge_attest::now_unix());
    if let Err(e) = connect_users::record_first_owner_claimed(&note) {
        let undo: Vec<String> = std::iter::once(user.clone())
            .chain(ops.iter().cloned())
            .filter_map(|n| connect_users::remove_user(&n).err().map(|err| format!("{n}: {err}")))
            .collect();
        audit(&user, &format!("deny burn_failed undo={} account={account}", if undo.is_empty() { "ok" } else { "FAILED" }), ingress);
        return CliResponse::internal_error(if undo.is_empty() {
            format!("the claim could not be recorded ({e}); nothing was kept")
        } else {
            format!("the claim could not be recorded ({e}), and taking the logins back failed ({})", undo.join("; "))
        });
    }
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
