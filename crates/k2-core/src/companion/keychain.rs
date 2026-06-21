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
    let output = std::process::Command::new("security")
        .args(["find-generic-password", "-s", SERVICE, "-a", ACCOUNT, "-w"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8(output.stdout).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(target_os = "macos")]
pub fn write_password_hash(hash: &str) -> Result<(), String> {
    // 0.40.7 — stamp an explicit trusted-application ACL so this item never
    // provokes the login-keychain password prompt, and the grant survives an
    // app-update re-sign. Both the WRITE (here) and the READ
    // ([`read_password_hash`]) go through `/usr/bin/security`, so that is the
    // process the keychain sees as the requester on every access; we also
    // trust the daemon's own executable for any future direct read. (Same
    // rationale as `tunnel::lease::acl_trusted_apps`, scoped to this item.)
    //
    // `security`'s `-U` updates the VALUE but does NOT reset the ACL, so to
    // (re)install the ACL — including upgrading a pre-0.40.7 item created
    // without one — we DELETE then plain-ADD (no `-U`). A delete of a
    // missing item is a harmless no-op.
    let _ = std::process::Command::new("security")
        .args(["delete-generic-password", "-s", SERVICE, "-a", ACCOUNT])
        .output();

    let mut cmd = std::process::Command::new("security");
    cmd.args(["add-generic-password", "-s", SERVICE, "-a", ACCOUNT]);
    for app in acl_trusted_apps() {
        cmd.arg("-T").arg(app);
    }
    // `-w <hash>` LAST; args are passed directly to exec (never shell-parsed)
    // so the hash's `$` is safe.
    cmd.arg("-w").arg(hash);
    let output = cmd
        .output()
        .map_err(|e| format!("keychain spawn failed: {}", e))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(format!("keychain write failed: {}", err.trim()));
    }
    Ok(())
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
