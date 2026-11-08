//! The old Companion password hash in the macOS login keychain.
//!
//! The ngrok Companion server that used this hash is gone. An older
//! install may still have the item, so settings reset deletes it
//! ([`delete_password_hash`]). The item's write options stay for the
//! keychain tests, which pin how K2-owned items are written.

#[cfg(target_os = "macos")]
const SERVICE: &str = "K2SO-companion-auth";
#[cfg(target_os = "macos")]
const ACCOUNT: &str = "companion-password-hash";

/// How the companion hash item was written (and repaired): our `-T` ACL,
/// delete-then-add, K2-owned (never argv).
#[cfg(all(test, target_os = "macos"))]
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
/// `/usr/bin/security` — both the read and the write shelled through it,
/// so it is the requesting application the keychain sees on every access.
/// The daemon executable (`current_exe`) is added too.
#[cfg(all(test, target_os = "macos"))]
fn acl_trusted_apps() -> Vec<String> {
    let mut apps = vec!["/usr/bin/security".to_string()];
    if let Ok(exe) = std::env::current_exe() {
        let exe = dunce::canonicalize(&exe).unwrap_or(exe);
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
    let _ = crate::bg_proc::command("security")
        .args(["delete-generic-password", "-s", SERVICE, "-a", ACCOUNT])
        .output();
}

#[cfg(not(target_os = "macos"))]
pub fn delete_password_hash() {}
