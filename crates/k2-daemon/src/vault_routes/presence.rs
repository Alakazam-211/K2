//! The computer's own user check for three first-setup steps of a vault:
//! setting the first vault password, showing the recovery words while no
//! recovery key is confirmed, and writing a vault backup before one is.
//!
//! Each step takes a single-use token from `/cli/vault/confirm/finish`
//! with the step's action, minted only by one of these checks:
//!
//! - **macOS**: LocalAuthentication (Touch ID or the Mac's login password),
//!   shown by the daemon on this Mac (`local-auth`).
//! - **Windows**: Windows Hello, else Windows' own credential prompt for the
//!   signed-in user (`windows-hello`, `vault::windows_presence`).
//! - **Linux** (a server, usually with no screen): the password of one of
//!   this server's owner Connect logins (`owner-login-password`).
//! - **A Connect login** (any OS): that login's own password
//!   (`connect-password`).
//!
//! The vault password is not one of them, and neither is the person check
//! on the vault socket. With no check available the steps are refused
//! (`vault_presence_required`); there is no other way to them.

use k2_vault::{Actor, HumanProof, LocalVault, VaultError};
use serde_json::{json, Value};

use super::{confirm_identity, err, human_admin};
use crate::vault::actor::{Channel, VaultCaller};

/// Setting the vault password while none is set.
pub const PASSWORD_SET: &str = "vault-password-set";
/// Showing the recovery words while no recovery key is confirmed (the K2
/// app's Set up recovery key and `k2 vault recovery show-once`).
pub const RECOVERY_SETUP: &str = "recovery-setup";
/// Writing a vault backup while no recovery key is confirmed; `secretId` =
/// the backup's folder.
pub const BACKUP_EXPORT_INITIAL: &str = "backup-export-initial";

pub const ACTIONS: &[&str] = &[PASSWORD_SET, RECOVERY_SETUP, BACKUP_EXPORT_INITIAL];

pub const LOCAL_AUTH: &str = "local-auth";
pub const WINDOWS_HELLO: &str = "windows-hello";
pub const CONNECT_PASSWORD: &str = "connect-password";
pub const OWNER_LOGIN_PASSWORD: &str = "owner-login-password";

pub const CODE: &str = "vault_presence_required";

pub fn is_action(action: &str) -> bool {
    ACTIONS.contains(&action)
}

/// The step's words on the prompt (after "K2 vault: ").
pub fn reason(action: &str) -> &'static str {
    match action {
        PASSWORD_SET => "set the vault password",
        RECOVERY_SETUP => "show the vault's recovery words",
        _ => "write an encrypted copy of the vault",
    }
}

/// The step in a refusal ("setting the first vault password needs …").
fn step(action: &str) -> &'static str {
    match action {
        PASSWORD_SET => "setting the first vault password",
        RECOVERY_SETUP => "showing the recovery words before a recovery key is confirmed",
        _ => "a vault backup before a recovery key is confirmed",
    }
}

fn command(action: &str) -> &'static str {
    match action {
        PASSWORD_SET => "k2 vault confirm-password set",
        RECOVERY_SETUP => "k2 vault recovery show-once (or Settings → Vault → Set up recovery key)",
        _ => "k2 vault backup <folder>",
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

/// No check this caller can use exists on this computer.
pub fn unavailable(action: &str) -> VaultError {
    err(
        CODE,
        409,
        format!(
            "{} needs this computer's own user check, and none is available here, so it can't be done yet. On a Linux server, add an owner login first (k2 users add <you> --role owner) and run it again; on a Mac or a Windows PC, run it while signed in at that computer. Agents: don't run it; tell your owner.",
            step(action)
        ),
    )
}

/// The checks this caller can use for these steps, here.
pub fn methods(c: &VaultCaller) -> Vec<&'static str> {
    if c.is_passport() {
        return vec![];
    }
    match &c.actor {
        Actor::Login { .. } => vec![CONNECT_PASSWORD],
        Actor::Owner if c.channel != Channel::Login => os_method().into_iter().collect(),
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

/// `/cli/vault/confirm/start` for one of these steps.
pub fn start(c: &VaultCaller, action: &str) -> Result<Value, VaultError> {
    human_admin(c)?;
    let m = methods(c);
    if m.is_empty() {
        return Err(unavailable(action));
    }
    let (server, mut login) = confirm_identity(c);
    if m.contains(&OWNER_LOGIN_PASSWORD) {
        // The CLI names the login it asks for; with several, it asks which.
        let owners = owner_logins();
        login = (owners.len() == 1).then(|| owners[0].clone());
    }
    Ok(json!({ "methods": m, "expiresIn": 60, "confirmText": reason(action), "server": server, "login": login }))
}

/// `/cli/vault/confirm/finish` for one of these steps: run the check and
/// mint a token bound to (action, secret, caller).
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
    human_admin(c)?;
    let m = methods(c);
    if m.is_empty() {
        return Err(unavailable(action));
    }
    if !m.contains(&method) {
        return Err(err("bad_method", 400, format!("this step takes this computer's own user check: method {}", m.join(" or "))));
    }
    let stamp = c.actor.stamp();
    let shown = format!("{} vault: {}", k2_core::brand::PRODUCT_NAME, reason(action));
    let res = match method {
        LOCAL_AUTH | WINDOWS_HELLO => os_check(method, &shown),
        CONNECT_PASSWORD => match &c.actor {
            Actor::Login { username, .. } => super::check_connect_password(vault, username, password.unwrap_or("")),
            _ => Err(err("bad_method", 400, "connect-password is for a Connect login")),
        },
        _ => owner_login_check(vault, username, password.unwrap_or("")),
    };
    match res {
        Ok(()) => {
            let t = vault.confirm.mint(action, secret_id, &stamp, method);
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
/// must have minted.
pub fn take(vault: &LocalVault, c: &VaultCaller, action: &str, secret: Option<&str>, token: Option<&str>) -> Result<HumanProof, VaultError> {
    let m = methods(c);
    if m.is_empty() {
        return Err(unavailable(action));
    }
    let t = token.ok_or_else(|| required(action))?;
    let proof = vault.confirm.take(t, action, secret, &c.actor.stamp())?;
    match &proof {
        HumanProof::Confirmed { method } if m.contains(&method.as_str()) => Ok(proof),
        _ => Err(required(action)),
    }
}

/// An owner login's sign-in password. `username` names the login; with
/// none given, the server's only owner login.
fn owner_login_check(vault: &LocalVault, username: Option<&str>, password: &str) -> Result<(), VaultError> {
    let owners = owner_logins();
    let user = match username.map(str::trim).filter(|u| !u.is_empty()) {
        Some(u) => {
            let u = k2_core::connect_users::normalize_username(u).unwrap_or_else(|_| u.to_lowercase());
            if !owners.contains(&u) {
                return Err(err("confirm_failed", 403, format!("{u} isn't an owner login on this server (k2 users list)")));
            }
            u
        }
        None if owners.len() == 1 => owners[0].clone(),
        None if owners.is_empty() => return Err(err(CODE, 409, "this server has no owner login to check (k2 users add <you> --role owner)")),
        None => return Err(err("username_required", 400, "this server has more than one owner login: say which (username)")),
    };
    super::check_connect_password(vault, &user, password)
}

fn os_check(method: &str, shown: &str) -> Result<(), VaultError> {
    #[cfg(test)]
    if let Some(f) = fake::get() {
        return if f.approve { Ok(()) } else { Err(err("confirm_failed", 403, "not approved")) };
    }
    match method {
        LOCAL_AUTH => super::local_auth(shown),
        _ => windows_hello(shown),
    }
}

#[cfg(windows)]
fn windows_hello(shown: &str) -> Result<(), VaultError> {
    use crate::vault::windows_presence::{confirm, Answer};
    match confirm(shown) {
        Answer::Approved => Ok(()),
        Answer::Declined(h) => Err(err("confirm_failed", 403, h)),
        Answer::Unavailable(h) => Err(err(CODE, 409, format!("{h}; this step needs this PC's own user check (Windows Hello or your Windows password)"))),
    }
}

#[cfg(not(windows))]
fn windows_hello(_shown: &str) -> Result<(), VaultError> {
    Err(err("bad_method", 400, "windows-hello is only on Windows"))
}

/// Test seam: this computer's check, per test thread (no real prompt in
/// tests, and tests running side by side don't see each other's).
#[cfg(test)]
pub mod fake {
    use std::cell::Cell;

    #[derive(Debug, Clone, Copy)]
    pub struct Fake {
        /// The check this computer offers the owner (`None`: none).
        pub method: Option<&'static str>,
        /// The person's answer to a `local-auth` / `windows-hello` prompt.
        pub approve: bool,
    }

    thread_local! {
        static FAKE: Cell<Option<Fake>> = const { Cell::new(None) };
    }

    pub fn set(f: Option<Fake>) {
        FAKE.with(|c| c.set(f));
    }

    pub fn get() -> Option<Fake> {
        FAKE.with(Cell::get)
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
}
