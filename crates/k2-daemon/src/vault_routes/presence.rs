//! This computer's own user check, for the steps that need a person at this
//! computer and not only a K2 token:
//!
//! - while no recovery key is confirmed: setting the first vault password,
//!   showing the recovery words, a vault backup, a box-move export of the
//!   vault, and adding a vault backup key;
//! - with the owner token: making a login an owner, and setting a new
//!   password for an owner login (`/cli/users/set-role`,
//!   `/cli/users/set-password`).
//!
//! Each vault step takes a single-use token from `/cli/vault/confirm/finish`
//! with the step's action and its folder or key, minted only by one of
//! these checks:
//!
//! - **macOS**: LocalAuthentication (Touch ID or the Mac's login password),
//!   shown by the daemon on this Mac (`local-auth`).
//! - **Windows**: Windows Hello, else Windows' own credential prompt for the
//!   signed-in user (`windows-hello`, `vault::windows_presence`).
//! - **Linux** (a server, usually with no screen): the password of one of
//!   this server's owner Connect logins (`owner-login-password`). Owner
//!   logins are made or re-keyed with the owner token only after this
//!   check, so their passwords are ones a person chose. On a server with no
//!   owner login yet, the first one is made from a terminal outside K2
//!   (`terminal`, the vault socket's person check), for that change only.
//! - **An owner Connect login** (any OS): that login's own password
//!   (`connect-password`).
//!
//! Admin logins, agents and the vault password are not checks here. With
//! no check available the step is refused (409 `vault_presence_required`).
//!
//! The owner-login changes run the check inline when this computer has its
//! own prompt (the K2 app needs nothing more), or take the same kind of
//! token (`k2 users`, and any Linux server).

use k2_vault::{Actor, HumanProof, LocalVault, Role, VaultError};
use serde_json::{json, Value};

use super::{confirm_identity, err};
use crate::vault::actor::{Channel, VaultCaller};
use crate::vault::request_peer;
use k2_core::brand::CLI_NAME;
use k2_core::cli_name;

/// Setting the vault password while none is set.
pub const PASSWORD_SET: &str = "vault-password-set";
/// Showing the recovery words while no recovery key is confirmed (the K2
/// app's Set up recovery key and `k2 vault recovery show-once`).
pub const RECOVERY_SETUP: &str = "recovery-setup";
/// Writing a vault backup while no recovery key is confirmed; `secretId` =
/// the backup's folder.
pub const BACKUP_EXPORT_INITIAL: &str = "backup-export-initial";
/// A box-move export while no recovery key is confirmed; `secretId` =
/// `export:<move key fingerprint>`.
pub const BOX_MOVE_EXPORT: &str = "box-move-export";
/// Adding a vault backup key while no recovery key is confirmed;
/// `secretId` = `add:<age1… recipient>`.
pub const BACKUP_KEY_ADD: &str = "backup-key-add";
/// With the owner token: making a login an owner (`role:<username>`) or
/// setting an owner login's password (`password:<username>`).
pub const OWNER_LOGIN: &str = "owner-login";
/// Agent Isolation M2 B4: turning on a root-owned scope for every agent
/// session on this server (`k2 agent isolation layout on`); `secretId` =
/// [`ISOLATION_LAYOUT_BINDING`]. The owner token or an owner login.
pub const ISOLATION_LAYOUT: &str = "isolation-layout";
pub const ISOLATION_LAYOUT_BINDING: &str = "layout:on";

/// The vault's own steps (taken on the vault routes).
#[cfg_attr(not(test), allow(dead_code))]
pub const VAULT_STEPS: &[&str] = &[PASSWORD_SET, RECOVERY_SETUP, BACKUP_EXPORT_INITIAL, BOX_MOVE_EXPORT, BACKUP_KEY_ADD];
pub const ACTIONS: &[&str] = &[PASSWORD_SET, RECOVERY_SETUP, BACKUP_EXPORT_INITIAL, BOX_MOVE_EXPORT, BACKUP_KEY_ADD, OWNER_LOGIN, ISOLATION_LAYOUT];

pub const LOCAL_AUTH: &str = "local-auth";
pub const WINDOWS_HELLO: &str = "windows-hello";
pub const CONNECT_PASSWORD: &str = "connect-password";
pub const OWNER_LOGIN_PASSWORD: &str = "owner-login-password";
/// A terminal outside K2 on the vault socket: only for the first owner
/// login of a server that has no other check.
pub const TERMINAL: &str = "terminal";

pub const CODE: &str = "vault_presence_required";

pub fn is_action(action: &str) -> bool {
    ACTIONS.contains(&action)
}

/// A value from a request, safe to show in a prompt or a refusal.
fn shown(s: &str) -> String {
    s.chars().filter(|ch| !ch.is_control()).take(120).collect()
}

/// What the person approves, in plain words (the prompt's text).
pub fn reason(action: &str, secret: Option<&str>) -> String {
    let s = secret.map(shown).unwrap_or_default();
    match action {
        PASSWORD_SET => "set your first vault password".into(),
        RECOVERY_SETUP => "show your vault recovery words".into(),
        BACKUP_EXPORT_INITIAL if !s.is_empty() => format!("make a vault backup in {s}"),
        BACKUP_EXPORT_INITIAL => "make a vault backup".into(),
        BOX_MOVE_EXPORT => match s.strip_prefix("export:") {
            Some(fp) => format!("export your vault to another server (move key {fp})"),
            None => "export your vault to another server".into(),
        },
        BACKUP_KEY_ADD => match secret.and_then(|x| x.strip_prefix("add:")).and_then(|r| k2_vault::agefmt::Recipient::parse(r.trim()).ok()) {
            Some(r) => format!("add a vault backup key ({})", r.fingerprint()),
            None => "add a vault backup key".into(),
        },
        ISOLATION_LAYOUT => "turn on a root-owned scope for every agent session on this server".into(),
        _ => {
            if let Some(u) = s.strip_prefix("role:") {
                format!("make login {u} an owner of this server")
            } else if let Some(u) = s.strip_prefix("password:") {
                format!("set a new password for owner login {u}")
            } else {
                "change an owner login".into()
            }
        }
    }
}

/// The prompt's whole text: the step, and who asks where known.
pub fn prompt(action: &str, secret: Option<&str>) -> String {
    let what = if action == OWNER_LOGIN || action == ISOLATION_LAYOUT { k2_core::brand::PRODUCT_NAME.to_string() } else { format!("{} vault", k2_core::brand::PRODUCT_NAME) };
    let asker = request_peer::asker().map(|a| format!(" (asked from {a})")).unwrap_or_default();
    let t = format!("{what}: {}{asker}", reason(action, secret));
    t.chars().filter(|ch| !ch.is_control()).take(240).collect()
}

/// The step in a refusal ("setting the first vault password needs …").
fn step(action: &str) -> &'static str {
    match action {
        PASSWORD_SET => "setting the first vault password",
        RECOVERY_SETUP => "showing the recovery words before a recovery key is confirmed",
        BACKUP_EXPORT_INITIAL => "a vault backup before a recovery key is confirmed",
        BOX_MOVE_EXPORT => "a box-move export of the vault before a recovery key is confirmed",
        BACKUP_KEY_ADD => "adding a vault backup key before a recovery key is confirmed",
        ISOLATION_LAYOUT => "turning on a root-owned scope for every agent session",
        _ => "making a login an owner, or setting an owner login's password, with the owner token",
    }
}

fn command(action: &str) -> &'static str {
    match action {
        PASSWORD_SET => concat!(cli_name!(), " vault confirm-password set"),
        RECOVERY_SETUP => concat!(cli_name!(), " vault recovery show-once (or Settings → Vault → Set up recovery key)"),
        BACKUP_EXPORT_INITIAL => concat!(cli_name!(), " vault backup <folder>"),
        BOX_MOVE_EXPORT => concat!(cli_name!(), " migrate export-secrets"),
        BACKUP_KEY_ADD => concat!(cli_name!(), " vault backup key <age1…>"),
        ISOLATION_LAYOUT => concat!(k2_core::cli_name!(), " agent isolation layout on"),
        _ => concat!("the ", cli_name!(), " users command"),
    }
}

/// No proof came with the step (or one from another check).
pub fn required(action: &str) -> VaultError {
    err(
        CODE,
        403,
        format!(
            "{} needs this computer's own user check first: Touch ID or the Mac's password, Windows Hello, or on a Linux server an owner login's sign-in password. Run {} yourself; it asks for the check. Agents: don't run it; tell your owner.",
            step(action),
            command(action)
        ),
    )
}

/// A token that isn't for this step (another step, folder or key, another
/// caller, used already or expired).
fn again(action: &str) -> VaultError {
    err(
        CODE,
        403,
        format!(
            "{} needs this computer's own user check again: a check works once, within 60 seconds, for this exact step only. Run {} yourself; it asks again. Agents: don't run it; tell your owner.",
            step(action),
            command(action)
        ),
    )
}

/// No check this caller can use exists on this computer.
pub fn unavailable(action: &str) -> VaultError {
    err(
        CODE,
        409,
        format!(
            "{} needs this computer's own user check, and none is available to this caller here. The owner does it in person: at the Mac or Windows PC itself, or on a Linux server with an owner login the owner set up. Agents: don't run it, and don't change logins for it; tell your owner.",
            step(action)
        ),
    )
}

/// This computer's prompt can't be shown or answered right now (no one at
/// its screen, an SSH-only session).
#[cfg_attr(any(test, not(any(target_os = "macos", windows))), allow(dead_code))]
fn headless(action: &str, why: &str) -> VaultError {
    err(
        CODE,
        409,
        format!(
            "{} needs this computer's own user check, and {why}. Do it at the computer itself, or sign in to {} Connect with an owner login and do it there: that asks for the login's own password. Agents: don't run it; tell your owner.",
            step(action),
            k2_core::brand::PRODUCT_NAME
        ),
    )
}

/// These steps are the owner's: the owner token, or an owner login.
fn owner_human(c: &VaultCaller) -> Result<(), VaultError> {
    if c.is_passport() || !c.actor.is_owner_human() {
        return Err(err("owner_only", 403, "this step is for the server's owner: an owner login, or the owner at this computer. Agents: don't run it; tell your owner."));
    }
    Ok(())
}

/// The checks this caller can use for `action`, here.
pub fn methods(c: &VaultCaller, action: &str) -> Vec<&'static str> {
    if c.is_passport() {
        return vec![];
    }
    match &c.actor {
        // An owner login proves itself with its own password. It changes
        // owner logins as a person already (no owner-token gate there).
        Actor::Login { role: Role::Owner, .. } if action != OWNER_LOGIN => vec![CONNECT_PASSWORD],
        Actor::Owner if c.channel != Channel::Login => match os_method() {
            Some(m) => vec![m],
            // A Linux server with no owner login: a terminal outside K2
            // for its first owner login, and for turning on agent scopes
            // (a tightening; without it such a server could never).
            None if action == OWNER_LOGIN || action == ISOLATION_LAYOUT => vec![TERMINAL],
            None => vec![],
        },
        _ => vec![],
    }
}

/// This computer's own check for the owner at it.
fn os_method() -> Option<&'static str> {
    #[cfg(test)]
    {
        fake::get().and_then(|f| f.method)
    }
    #[cfg(not(test))]
    {
        if let Some(f) = fake::from_env() {
            return f.method;
        }
        platform_method(!owner_logins().is_empty())
    }
}

/// Per OS: macOS and Windows have their own prompt; anything else (a Linux
/// server) uses an owner login's password, when there is one.
#[cfg_attr(test, allow(dead_code))]
fn platform_method(has_owner_login: bool) -> Option<&'static str> {
    if cfg!(target_os = "macos") {
        Some(LOCAL_AUTH)
    } else if cfg!(windows) {
        Some(WINDOWS_HELLO)
    } else if has_owner_login {
        Some(OWNER_LOGIN_PASSWORD)
    } else {
        None
    }
}

/// This server's owner Connect logins that can sign in (not disabled).
pub(crate) fn owner_logins() -> Vec<String> {
    k2_core::connect_users::list_users()
        .unwrap_or_default()
        .into_iter()
        .filter(|u| !u.disabled && u.role == k2_core::connect_users::Role::Owner)
        .map(|u| u.username)
        .collect()
}

/// The daemon's own form of a step's `secretId` (what its token is bound
/// to), checked: the folder, the key, the login. `None` for the steps with
/// no secretId.
pub fn binding(action: &str, secret_id: Option<&str>) -> Result<Option<String>, VaultError> {
    let sid = secret_id.map(str::trim).unwrap_or("");
    let bad = |m: &str| err("bad_secret_id", 400, m.to_string());
    match action {
        PASSWORD_SET | RECOVERY_SETUP => Ok(None),
        BACKUP_EXPORT_INITIAL => match std::fs::canonicalize(sid) {
            Ok(d) if !sid.is_empty() && d.is_dir() => Ok(Some(d.display().to_string())),
            _ => Err(bad("secretId is the backup's folder, and it must exist")),
        },
        BOX_MOVE_EXPORT => match sid.strip_prefix("export:") {
            Some(fp) if !fp.is_empty() && fp.len() <= 96 && fp.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, ':' | '-' | '_')) => {
                Ok(Some(format!("export:{fp}")))
            }
            _ => Err(bad("secretId is export:<the new box's move key fingerprint>")),
        },
        BACKUP_KEY_ADD => match sid.strip_prefix("add:") {
            Some(r) => k2_vault::agefmt::Recipient::parse(r.trim()).map(|r| Some(format!("add:{}", r.encode()))).map_err(|_| bad("secretId is add:<age1… recipient>")),
            None => Err(bad("secretId is add:<age1… recipient>")),
        },
        ISOLATION_LAYOUT if sid == ISOLATION_LAYOUT_BINDING => Ok(Some(ISOLATION_LAYOUT_BINDING.to_string())),
        ISOLATION_LAYOUT => Err(bad("secretId is layout:on")),
        _ => {
            let (kind, user) = sid.split_once(':').ok_or_else(|| bad("secretId is role:<username> or password:<username>"))?;
            if !matches!(kind, "role" | "password") {
                return Err(bad("secretId is role:<username> or password:<username>"));
            }
            let user = k2_core::connect_users::normalize_username(user).map_err(|e| bad(&e))?;
            Ok(Some(format!("{kind}:{user}")))
        }
    }
}

/// What a token is bound to beyond the step: the caller, and (for the
/// vault's own steps, minted and taken on the same transport) this
/// request's peer.
pub(super) fn bound_stamp(c: &VaultCaller, action: &str) -> String {
    let stamp = c.actor.stamp();
    if action == OWNER_LOGIN || action == ISOLATION_LAYOUT {
        // Minted on the vault socket or TCP, taken on the users routes or
        // the agent-isolation layout route (TCP): the owner only.
        stamp
    } else {
        format!("{stamp}{}", request_peer::bind())
    }
}

/// `/cli/vault/confirm/start` for one of these steps.
pub fn start(c: &VaultCaller, action: &str, secret_id: Option<&str>) -> Result<Value, VaultError> {
    owner_human(c)?;
    let m = methods(c, action);
    if m.is_empty() {
        return Err(unavailable(action));
    }
    let (server, mut login) = confirm_identity(c);
    let mut owners = None;
    if m.contains(&OWNER_LOGIN_PASSWORD) {
        // The client names the login it asks for; with several, it asks
        // which (`ownerLogins` says how many there are).
        let o = owner_logins();
        login = (o.len() == 1).then(|| o[0].clone());
        owners = Some(o.len());
    }
    let text = binding(action, secret_id).ok().flatten();
    Ok(json!({
        "methods": m,
        "expiresIn": 60,
        "confirmText": reason(action, text.as_deref().or(secret_id)),
        "server": server,
        "login": login,
        "ownerLogins": owners,
    }))
}

/// `/cli/vault/confirm/finish` for one of these steps: run the check and
/// mint a token bound to (action, the daemon's form of `secret_id`, caller,
/// this request's peer).
#[allow(clippy::too_many_arguments)]
pub fn finish(
    vault: &LocalVault,
    c: &VaultCaller,
    action: &str,
    secret_id: Option<&str>,
    method: &str,
    username: Option<&str>,
    password: Option<&str>,
    ingress: &str,
) -> Result<String, VaultError> {
    owner_human(c)?;
    let m = methods(c, action);
    if m.is_empty() {
        return Err(unavailable(action));
    }
    if !m.contains(&method) {
        return Err(err("bad_method", 400, format!("this step takes this computer's own user check: method {}", m.join(" or "))));
    }
    let bound = binding(action, secret_id)?;
    let secret_id = bound.as_deref();
    let stamp = c.actor.stamp();
    let res = match method {
        LOCAL_AUTH | WINDOWS_HELLO => os_check(action, method, &prompt(action, secret_id)),
        CONNECT_PASSWORD => match &c.actor {
            Actor::Login { username, .. } => super::check_connect_password(vault, username, password.unwrap_or("")),
            _ => Err(err("bad_method", 400, "connect-password is for a Connect login")),
        },
        OWNER_LOGIN_PASSWORD => owner_login_check(vault, action, username, password.unwrap_or("")),
        _ => {
            // TERMINAL: the first owner login of a server with no other
            // check, from a terminal outside K2.
            if c.channel == Channel::Tty {
                Ok(())
            } else {
                Err(err(
                    CODE,
                    403,
                    format!(
                        "this check is a terminal outside {}: run the {} users command yourself, from your own SSH login or terminal. Agents: don't run it; tell your owner.",
                        k2_core::brand::PRODUCT_NAME,
                        k2_core::brand::CLI_NAME
                    ),
                ))
            }
        }
    };
    match res {
        Ok(()) => {
            let t = vault.confirm.mint(action, secret_id, &bound_stamp(c, action), method);
            crate::vault::auth_audit("vault-confirm", &stamp, &format!("ok {action} via {method}"), ingress);
            Ok(t)
        }
        Err(e) => {
            crate::vault::auth_audit("vault-confirm", &stamp, &format!("deny {action} via {method}: {}", e.code), ingress);
            Err(e)
        }
    }
}

/// The step itself: take its token, which one of this caller's checks
/// must have minted for this exact step.
pub fn take(vault: &LocalVault, c: &VaultCaller, action: &str, secret: Option<&str>, token: Option<&str>) -> Result<HumanProof, VaultError> {
    owner_human(c)?;
    let m = methods(c, action);
    if m.is_empty() {
        return Err(unavailable(action));
    }
    let t = token.map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| required(action))?;
    let proof = vault.confirm.take(t, action, secret, &bound_stamp(c, action)).map_err(|_| again(action))?;
    match &proof {
        HumanProof::Confirmed { method } if m.contains(&method.as_str()) => Ok(proof),
        // 0.46.17: the owner's phone answered this step's card with an owner
        // login's own password (`phone`): never minted for an owner-login
        // change, nor from the vault password.
        HumanProof::Confirmed { method } if method == super::phone::METHOD && super::phone::eligible(action) => Ok(proof),
        _ => Err(again(action)),
    }
}

/// With the owner token: making a login an owner, or setting an owner
/// login's password (`binding` = `role:<user>` / `password:<user>`). A
/// token from `/cli/vault/confirm/finish` (the CLI, any Linux server), or,
/// with none, this computer's own prompt right now (a Mac or a Windows PC:
/// the K2 app needs nothing more).
pub fn owner_login_change(binding: &str, token: Option<&str>, ingress: &str) -> Result<(), VaultError> {
    owner_login_change_on(None, binding, token, ingress)
}

/// [`owner_login_change`] on a given vault (tests); `None`: the daemon's.
pub(crate) fn owner_login_change_on(vault: Option<&LocalVault>, binding: &str, token: Option<&str>, ingress: &str) -> Result<(), VaultError> {
    let c = VaultCaller { actor: Actor::Owner, channel: Channel::Plain, session_id: None };
    let res = match token.map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => match vault {
            Some(v) => take(v, &c, OWNER_LOGIN, Some(binding), Some(t)).map(|_| ()),
            None => crate::vault::get().and_then(|v| take(&v, &c, OWNER_LOGIN, Some(binding), Some(t)).map(|_| ())),
        },
        None => match methods(&c, OWNER_LOGIN).into_iter().find(|m| matches!(*m, LOCAL_AUTH | WINDOWS_HELLO)) {
            Some(method) => os_check(OWNER_LOGIN, method, &prompt(OWNER_LOGIN, Some(binding))),
            None => Err(required(OWNER_LOGIN)),
        },
    };
    let outcome = match &res {
        Ok(()) => format!("ok {binding}"),
        Err(e) => format!("deny {binding}: {}", e.code),
    };
    crate::vault::auth_audit("owner-login-check", "owner", &outcome, ingress);
    res
}

/// Agent Isolation M2 B4: turning on a root-owned scope for every agent
/// session (`k2 agent isolation layout on`), by the owner token or an owner
/// login (`c`). The same checks as [`owner_login_change`]: a token from
/// `/cli/vault/confirm/finish` for [`ISOLATION_LAYOUT`] /
/// [`ISOLATION_LAYOUT_BINDING`], minted for this caller (an owner login with
/// its own password; the owner token with this computer's check, an owner
/// login's password on a Linux server, or a terminal outside K2 on a server
/// with no owner login), or, with none, this computer's own prompt right
/// now where it has one. An admin login and a passport are refused.
pub fn isolation_layout_change(c: &VaultCaller, token: Option<&str>, ingress: &str) -> Result<(), VaultError> {
    isolation_layout_change_on(None, c, token, ingress)
}

/// [`isolation_layout_change`] on a given vault (tests); `None`: the daemon's.
pub(crate) fn isolation_layout_change_on(vault: Option<&LocalVault>, c: &VaultCaller, token: Option<&str>, ingress: &str) -> Result<(), VaultError> {
    let res = owner_human(c).and_then(|()| match token.map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => match vault {
            Some(v) => take(v, c, ISOLATION_LAYOUT, Some(ISOLATION_LAYOUT_BINDING), Some(t)).map(|_| ()),
            None => crate::vault::get().and_then(|v| take(&v, c, ISOLATION_LAYOUT, Some(ISOLATION_LAYOUT_BINDING), Some(t)).map(|_| ())),
        },
        None => match methods(c, ISOLATION_LAYOUT).into_iter().find(|m| matches!(*m, LOCAL_AUTH | WINDOWS_HELLO)) {
            Some(method) => os_check(ISOLATION_LAYOUT, method, &prompt(ISOLATION_LAYOUT, Some(ISOLATION_LAYOUT_BINDING))),
            None if methods(c, ISOLATION_LAYOUT).is_empty() => Err(unavailable(ISOLATION_LAYOUT)),
            None => Err(required(ISOLATION_LAYOUT)),
        },
    });
    let outcome = match &res {
        Ok(()) => format!("ok {ISOLATION_LAYOUT_BINDING}"),
        Err(e) => format!("deny {ISOLATION_LAYOUT_BINDING}: {}", e.code),
    };
    crate::vault::auth_audit("isolation-layout-check", &c.actor.stamp(), &outcome, ingress);
    res
}

/// An owner login's sign-in password. `username` names the login; with
/// none given, the server's only owner login.
pub(super) fn owner_login_check(vault: &LocalVault, action: &str, username: Option<&str>, password: &str) -> Result<(), VaultError> {
    let owners = owner_logins();
    let user = match username.map(str::trim).filter(|u| !u.is_empty()) {
        Some(u) => {
            let u = k2_core::connect_users::normalize_username(u).unwrap_or_else(|_| u.to_lowercase());
            if !owners.contains(&u) {
                return Err(err("confirm_failed", 403, format!("{u} isn't an owner login on this server ({CLI_NAME} users list)")));
            }
            u
        }
        None if owners.len() == 1 => owners[0].clone(),
        None if owners.is_empty() => return Err(unavailable(action)),
        None => return Err(err("username_required", 400, "this server has more than one owner login: say which (username)")),
    };
    super::check_connect_password(vault, &user, password)
}

/// Run this computer's own prompt for `action`.
fn os_check(action: &str, method: &str, shown: &str) -> Result<(), VaultError> {
    #[cfg(test)]
    {
        let _ = (action, method);
        fake::note_prompt(shown);
        match fake::get() {
            Some(f) if f.approve => Ok(()),
            _ => Err(err("confirm_failed", 403, "not approved")),
        }
    }
    #[cfg(not(test))]
    {
        if let Some(f) = fake::from_env() {
            return if f.approve { Ok(()) } else { Err(err("confirm_failed", 403, "not approved")) };
        }
        match method {
            LOCAL_AUTH => mac_check(action, shown),
            _ => windows_hello(action, shown),
        }
    }
}

#[cfg(target_os = "macos")]
#[cfg_attr(test, allow(dead_code))]
fn mac_check(action: &str, shown: &str) -> Result<(), VaultError> {
    use crate::vault::local_auth::{ask, Answer};
    match ask(shown) {
        Answer::Approved => Ok(()),
        Answer::Declined(h) => Err(err("confirm_failed", 403, h)),
        Answer::Unavailable(h) => Err(headless(action, &h)),
    }
}

#[cfg(not(target_os = "macos"))]
#[cfg_attr(test, allow(dead_code))]
fn mac_check(_action: &str, _shown: &str) -> Result<(), VaultError> {
    Err(err("bad_method", 400, "local-auth is only on a Mac"))
}

#[cfg(windows)]
#[cfg_attr(test, allow(dead_code))]
fn windows_hello(action: &str, shown: &str) -> Result<(), VaultError> {
    use crate::vault::windows_presence::{confirm, Answer};
    match confirm(shown) {
        Answer::Approved => Ok(()),
        Answer::Declined(h) => Err(err("confirm_failed", 403, h)),
        Answer::Unavailable(h) => Err(headless(action, &h)),
    }
}

#[cfg(not(windows))]
#[cfg_attr(test, allow(dead_code))]
fn windows_hello(_action: &str, _shown: &str) -> Result<(), VaultError> {
    Err(err("bad_method", 400, "windows-hello is only on Windows"))
}

/// Test seams: this computer's check, never a real prompt.
///
/// - Unit tests: per test thread (tests running side by side don't see
///   each other's).
/// - A test-build daemon (the integration tests' daemons,
///   `k2_core::TEST_BUILD`): `K2_TEST_PRESENCE` = `approve` (unset: the
///   person at the computer approves), `decline`, or `none` (no check of
///   its own, like a Linux server). A release build never reads it.
pub mod fake {
    #[derive(Debug, Clone, Copy)]
    pub struct Fake {
        /// The check this computer offers the owner (`None`: none).
        pub method: Option<&'static str>,
        /// The person's answer to a `local-auth` / `windows-hello` prompt.
        pub approve: bool,
    }

    pub const ENV: &str = "K2_TEST_PRESENCE";

    #[cfg(test)]
    thread_local! {
        static FAKE: std::cell::Cell<Option<Fake>> = const { std::cell::Cell::new(None) };
    }

    #[cfg(test)]
    pub fn set(f: Option<Fake>) {
        FAKE.with(|c| c.set(f));
    }

    #[cfg(test)]
    pub fn get() -> Option<Fake> {
        FAKE.with(std::cell::Cell::get)
    }

    #[cfg(test)]
    thread_local! {
        static PROMPT: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
    }

    /// The text the last prompt on this test thread showed.
    #[cfg(test)]
    pub fn last_prompt() -> Option<String> {
        PROMPT.with(|p| p.borrow().clone())
    }

    #[cfg(test)]
    pub(super) fn note_prompt(t: &str) {
        PROMPT.with(|p| *p.borrow_mut() = Some(t.to_string()));
    }

    /// A test-build daemon's check (see the module doc); `None` in a
    /// release build.
    #[cfg_attr(test, allow(dead_code))]
    pub fn from_env() -> Option<Fake> {
        if !k2_core::TEST_BUILD {
            return None;
        }
        Some(parse(std::env::var(ENV).ok().as_deref()))
    }

    pub fn parse(v: Option<&str>) -> Fake {
        match v {
            None | Some("approve") => Fake { method: Some(super::LOCAL_AUTH), approve: true },
            Some("decline") => Fake { method: Some(super::LOCAL_AUTH), approve: false },
            Some("none") => Fake { method: None, approve: false },
            Some(other) => panic!("{ENV}={other:?}: use approve, decline or none"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_os_offers_its_own_check_and_linux_needs_an_owner_login() {
        if cfg!(target_os = "macos") {
            assert_eq!(platform_method(false), Some(LOCAL_AUTH));
        } else if cfg!(windows) {
            assert_eq!(platform_method(false), Some(WINDOWS_HELLO));
        } else {
            assert_eq!(platform_method(true), Some(OWNER_LOGIN_PASSWORD));
            assert_eq!(platform_method(false), None, "no owner login: no check, so the steps are refused");
        }
    }

    #[test]
    fn owner_logins_are_the_enabled_owner_role_logins() {
        use k2_core::connect_users::{add_user, set_disabled, set_role, Role};
        let _home = crate::test_support::TempHome::new();
        assert!(owner_logins().is_empty());
        add_user("rosa", "password123").expect("rosa");
        set_role("rosa", Role::Owner).expect("owner");
        add_user("maxi", "password123").expect("maxi");
        set_role("maxi", Role::Admin).expect("admin");
        add_user("oldo", "password123").expect("oldo");
        set_role("oldo", Role::Owner).expect("owner 2");
        set_disabled("oldo", true).expect("disable");
        assert_eq!(owner_logins(), vec!["rosa".to_string()]);
    }

    #[test]
    fn the_prompt_names_the_step_and_the_asker() {
        let r = k2_vault::agefmt::Identity::generate().recipient();
        let cases = [
            (PASSWORD_SET, None, "set your first vault password".to_string()),
            (RECOVERY_SETUP, None, "show your vault recovery words".to_string()),
            (BACKUP_EXPORT_INITIAL, Some("/tmp/bk".to_string()), "make a vault backup in /tmp/bk".to_string()),
            (BOX_MOVE_EXPORT, Some("export:ab12cd".to_string()), "export your vault to another server (move key ab12cd)".to_string()),
            (BACKUP_KEY_ADD, Some(format!("add:{}", r.encode())), format!("add a vault backup key ({})", r.fingerprint())),
            (OWNER_LOGIN, Some("role:eve".to_string()), "make login eve an owner of this server".to_string()),
            (OWNER_LOGIN, Some("password:eve".to_string()), "set a new password for owner login eve".to_string()),
        ];
        for (a, s, want) in cases {
            assert_eq!(reason(a, s.as_deref()), want, "{a}");
        }
        let p = request_peer::scoped(request_peer::Peer { bind: "@sid:9".into(), asker: Some("agent scout in a K2 terminal".into()) }, || prompt(RECOVERY_SETUP, None));
        assert_eq!(p, format!("{} vault: show your vault recovery words (asked from agent scout in a K2 terminal)", k2_core::brand::PRODUCT_NAME));
        assert_eq!(prompt(OWNER_LOGIN, Some("role:eve")), format!("{}: make login eve an owner of this server", k2_core::brand::PRODUCT_NAME));
        // No control characters reach the prompt.
        assert!(!reason(BACKUP_EXPORT_INITIAL, Some("/tmp/a\nb\u{1b}[2J")).chars().any(char::is_control));
    }

    #[test]
    fn bindings_are_the_daemons_own_form() {
        let r = k2_vault::agefmt::Identity::generate().recipient().encode();
        assert_eq!(binding(PASSWORD_SET, Some("ignored")).expect("none"), None);
        assert_eq!(binding(BACKUP_KEY_ADD, Some(&format!("add: {r} "))).expect("key"), Some(format!("add:{r}")));
        assert_eq!(binding(BACKUP_KEY_ADD, Some("remove:x")).expect_err("not an add").code, "bad_secret_id");
        assert_eq!(binding(BOX_MOVE_EXPORT, Some("export:ab12")).expect("fp"), Some("export:ab12".into()));
        assert_eq!(binding(BOX_MOVE_EXPORT, Some("export:a b")).expect_err("bad fp").code, "bad_secret_id");
        assert_eq!(binding(OWNER_LOGIN, Some("role:Eve")).expect("login"), Some("role:eve".into()));
        assert_eq!(binding(OWNER_LOGIN, Some("admin:eve")).expect_err("kind").code, "bad_secret_id");
        assert_eq!(binding(BACKUP_EXPORT_INITIAL, Some("/no/such/folder/k2")).expect_err("folder").code, "bad_secret_id");
    }

    /// A Mac with no one at its screen (an SSH-only session): 409 with the
    /// Connect-login way, not a declined check.
    #[test]
    fn a_prompt_nobody_can_answer_is_a_409_with_the_connect_login_way() {
        let e = headless(RECOVERY_SETUP, "this Mac couldn't show its Touch ID or password prompt (no one at its screen?)");
        assert_eq!((e.code, e.status), (CODE, 409));
        assert!(e.hint.contains("Connect with an owner login") && e.hint.contains("no one at its screen"), "{}", e.hint);
        assert!(e.hint.contains("tell your owner"), "{}", e.hint);
    }

    #[test]
    fn a_test_build_daemon_reads_its_check_from_the_env() {
        assert!(matches!(fake::parse(None), fake::Fake { method: Some(LOCAL_AUTH), approve: true }));
        assert!(matches!(fake::parse(Some("decline")), fake::Fake { method: Some(LOCAL_AUTH), approve: false }));
        assert!(matches!(fake::parse(Some("none")), fake::Fake { method: None, .. }));
        assert!(std::panic::catch_unwind(|| fake::parse(Some("maybe"))).is_err(), "a typo fails loudly");
    }
}
