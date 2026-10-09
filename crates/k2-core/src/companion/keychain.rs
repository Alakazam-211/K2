//! macOS Keychain storage for the companion password hash.
//!
//! The hash itself is argon2id-protected, but keeping it in
//! `~/.k2so/settings.json` means any process able to read the user's home
//! directory can attempt an offline dictionary attack. Moving the hash to
//! the user's login Keychain restricts read access to the k2so binary
//! (and anything the user explicitly allows) and picks up the OS disk
//! encryption story for free.
//!
//! On non-macOS platforms the functions are no-ops — callers must fall
//! back to the legacy `settings.companion.password_hash` field.

#[cfg(target_os = "macos")]
const SERVICE: &str = "K2SO-companion-auth";
#[cfg(target_os = "macos")]
const ACCOUNT: &str = "companion-password-hash";

#[cfg(target_os = "macos")]
pub fn read_password_hash() -> Option<String> {
    // Test builds never touch the developer's real login keychain (a read
    // may also repair, i.e. rewrite, the item).
    if cfg!(any(test, feature = "test-util")) {
        return None;
    }
    // Never a dialog; an item 0.45.1/0.45.2 left with K2's partition only
    // is read in process and rewritten by `security` (repaired).
    crate::macos_keychain::read_k2_owned_text(SERVICE, ACCOUNT, &item_write_options(), "companion")
}

#[cfg(target_os = "macos")]
pub fn write_password_hash(hash: &str) -> Result<(), String> {
    // 0.40.7 — stamp an explicit trusted-application ACL so this item never
    // provokes the login-keychain password prompt, and the grant survives an
    // app-update re-sign. Both the item's creation (here) and the READ
    // ([`read_password_hash`]) go through `/usr/bin/security`, so that is the
    // process the keychain sees as the requester on every access; we also
    // trust the daemon's own executable for any future direct read. (Same
    // rationale as `tunnel::lease::acl_trusted_apps`, scoped to this item.)
    //
    // `security`'s `-U` updates the VALUE but does NOT reset the ACL, so to
    // (re)install the ACL — including upgrading a pre-0.40.7 item created
    // without one — we DELETE then plain-ADD (no `-U`). A delete of a
    // missing item is a harmless no-op.
    //
    // The hash is never on argv (`-w <hash>` showed it in `ps`): `security`
    // creates the item with the `-T` list and the hash on its stdin, and it
    // is read back (`crate::macos_keychain::write`).
    crate::macos_keychain::write(SERVICE, ACCOUNT, hash.as_bytes(), &item_write_options())
        .map_err(|e| format!("keychain write failed: {e}"))
}

/// How the companion hash item is written (and repaired): our `-T` ACL,
/// delete-then-add, K2-owned (never argv).
#[cfg(target_os = "macos")]
pub(crate) fn item_write_options() -> crate::macos_keychain::WriteOptions {
    crate::macos_keychain::WriteOptions {
        keychain: None,
        label: None,
        trusted_apps: acl_trusted_apps(),
        replace_acl: true,
        cli_owned: false,
    }
}

/// Trusted-application `-T` set for the companion password-hash item.
///
/// `/usr/bin/security` — both the read and the write shell through it, so it
/// is the requesting application the keychain sees on every access.
/// The daemon executable (`current_exe`) is added too for any future direct
/// Security-framework read. There is no renderer/app reader of this item
/// (it's daemon-internal companion auth), so no `k2` app-binary entry.
#[cfg(target_os = "macos")]
fn acl_trusted_apps() -> Vec<String> {
    let mut apps = vec!["/usr/bin/security".to_string()];
    if let Ok(exe) = std::env::current_exe() {
        let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
        apps.push(exe.to_string_lossy().to_string());
    }
    apps
}

#[cfg(target_os = "macos")]
pub fn delete_password_hash() {
    // Test builds (k2-core's own tests, and k2-daemon tests through the
    // `test-util` dev feature) must never touch the developer's real login
    // keychain: `settings/reset` tests reach this.
    if cfg!(any(test, feature = "test-util")) {
        return;
    }
    let _ = std::process::Command::new("security")
        .args(["delete-generic-password", "-s", SERVICE, "-a", ACCOUNT])
        .output();
}

#[cfg(not(target_os = "macos"))]
pub fn read_password_hash() -> Option<String> {
    None
}

#[cfg(not(target_os = "macos"))]
pub fn write_password_hash(_hash: &str) -> Result<(), String> {
    Err("Keychain storage is macOS-only".to_string())
}

#[cfg(not(target_os = "macos"))]
pub fn delete_password_hash() {}
