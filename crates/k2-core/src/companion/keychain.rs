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
    // `-U` overwrites an existing item; args are passed directly so the hash
    // (which contains `$`) is never shell-interpreted.
    let output = std::process::Command::new("security")
        .args([
            "add-generic-password",
            "-U",
            "-s", SERVICE,
            "-a", ACCOUNT,
            "-w", hash,
        ])
        .output()
        .map_err(|e| format!("keychain spawn failed: {}", e))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(format!("keychain write failed: {}", err.trim()));
    }
    Ok(())
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
