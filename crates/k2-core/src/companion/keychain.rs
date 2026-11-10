//! The old Companion password hash in the macOS login keychain.
//!
//! The ngrok Companion server that used this hash is gone. An older
//! install may still have the item, so settings reset deletes it
//! ([`delete_password_hash`]). Nothing writes it any more.

#[cfg(target_os = "macos")]
const SERVICE: &str = "K2SO-companion-auth";
#[cfg(target_os = "macos")]
const ACCOUNT: &str = "companion-password-hash";

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
