//! Cross-platform OS keychain bridge for K2 Connect (PRD §1, §3 of
//! `.k2so/prds/k2-connect-client-ux.md`, build order step #3).
//!
//! "Remember password" for a remote [`ConnectHost`] stores its auth token
//! in the OS keychain — NEVER in `~/.k2so/connect-hosts.json`, which holds
//! only non-secret fields. The renderer keys each token by the host's
//! stable client-generated id (`account = host.id`) under a fixed
//! service name, so resolving a remembered token on boot is a single
//! `k2_secret_get(SERVICE, host.id)`.
//!
//! Backed by the cross-platform [`keyring`] crate:
//!   - macOS   → Keychain Services (`apple-native`)
//!   - Linux   → Secret Service via libsecret/D-Bus (`sync-secret-service`)
//!     ⚠ requires the system `libsecret` runtime + a running Secret
//!       Service provider (gnome-keyring / KWallet). See Cargo.toml.
//!   - Windows → Credential Manager (`windows-native`)
//!
//! SECURITY: secret VALUES are never logged. Error paths log only the
//! service/account identifiers and the keyring error category — never the
//! secret itself. The keychain entry is the single source of truth for a
//! remembered remote token.

use keyring::Entry;

/// Build a keyring [`Entry`] for `(service, account)`, mapping the
/// keyring error to a renderer-friendly string. Logs the identifiers
/// (never a secret) on failure.
fn entry(service: &str, account: &str) -> Result<Entry, String> {
    Entry::new(service, account).map_err(|e| {
        log_debug!(
            "[secrets] keyring entry init failed (service={service}, account={account}): {e}"
        );
        format!("keychain unavailable: {e}")
    })
}

/// Store `secret` in the OS keychain under `(service, account)`.
///
/// Used by "Remember password" — `account` is the [`ConnectHost`] id, so
/// the token round-trips back via [`k2_secret_get`] keyed by the same id.
/// The secret value is NEVER logged.
#[tauri::command]
pub fn k2_secret_set(service: String, account: String, secret: String) -> Result<(), String> {
    let e = entry(&service, &account)?;
    e.set_password(&secret).map_err(|err| {
        // err's Display never includes the secret, but be explicit: log
        // only the identifiers + category, not `secret`.
        log_debug!("[secrets] set failed (service={service}, account={account}): {err}");
        format!("failed to store secret: {err}")
    })?;
    log_debug!("[secrets] stored secret (service={service}, account={account})");
    Ok(())
}

/// Fetch the secret stored under `(service, account)`.
///
/// Returns `Ok(None)` when no entry exists (a host whose token was never
/// remembered, or was forgotten) so the renderer can cleanly fall back to
/// the full-screen sign-in. Any OTHER keyring error is surfaced as `Err`.
/// The returned value is NEVER logged.
#[tauri::command]
pub fn k2_secret_get(service: String, account: String) -> Result<Option<String>, String> {
    let e = entry(&service, &account)?;
    match e.get_password() {
        Ok(secret) => Ok(Some(secret)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(err) => {
            log_debug!("[secrets] get failed (service={service}, account={account}): {err}");
            Err(format!("failed to read secret: {err}"))
        }
    }
}

/// Delete the secret stored under `(service, account)`.
///
/// Idempotent: a missing entry is treated as success so toggling
/// "Remember password" off (or removing a host) never errors when the
/// token was already gone.
#[tauri::command]
pub fn k2_secret_delete(service: String, account: String) -> Result<(), String> {
    let e = entry(&service, &account)?;
    match e.delete_credential() {
        Ok(()) => {
            log_debug!("[secrets] deleted secret (service={service}, account={account})");
            Ok(())
        }
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(err) => {
            log_debug!("[secrets] delete failed (service={service}, account={account}): {err}");
            Err(format!("failed to delete secret: {err}"))
        }
    }
}

/// Persist the K2 Connect sign-in session (`dev.k2.connect.account` /
/// `session`) with the trusted-application ACL applied UP FRONT.
///
/// The generic [`k2_secret_set`] goes through the `keyring` crate, whose
/// macOS backend gives the item a default ACL trusting only the creating app
/// (`k2`). The daemon then reads that item via the `security` CLI on its next
/// boot — not on the ACL — which raises exactly one login-keychain prompt
/// before it can re-stamp the ACL (see `k2_core::tunnel::lease`). Routing the
/// sign-in write through the SAME `-T` ACL writer the daemon uses means the
/// item is BORN trusting `/usr/bin/security` + both bundle binaries, so the
/// daemon's first read is already covered and that one prompt never appears —
/// and it stays prompt-free across future app re-signings.
///
/// macOS-only ACL path; other platforms fall back to the plain keyring blob
/// write (same bytes, same `(service, account)`), since the `-T` mechanism is
/// a login-keychain / `security` CLI feature with no cross-platform analogue.
/// The stored bytes are identical to what the renderer wrote before, so
/// `readAccountSession` / the daemon read path are unaffected.
#[tauri::command]
pub fn k2_account_session_set(refresh_token: String, email: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        // Best-effort inside the daemon-shared writer (logs + swallows a
        // keychain failure); the caller keeps the in-memory session regardless.
        k2_core::tunnel::lease::write_account_session(&k2_core::tunnel::lease::AccountSession {
            refresh_token,
            email: Some(email),
        });
        log_debug!("[secrets] stored K2 Connect account session with trusted-app ACL");
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        // Same blob shape the renderer's readAccountSession expects.
        let blob = serde_json::json!({
            "refreshToken": refresh_token,
            "email": email,
        })
        .to_string();
        let e = entry("dev.k2.connect.account", "session")?;
        e.set_password(&blob).map_err(|err| {
            log_debug!("[secrets] account session set failed: {err}");
            format!("failed to store secret: {err}")
        })?;
        Ok(())
    }
}

/// Read the persisted K2 Connect session back for the renderer, WITHOUT
/// touching the keyring crate on macOS.
///
/// Counterpart of [`k2_account_session_set`] and the reason it exists:
/// since 0.40.31 the session item is CREATED by the `security` CLI, which
/// gives it an `apple-tool:` partition list. macOS checks the partition
/// list before the `-T` ACL for in-process Security-framework reads, so a
/// keyring-crate read from the signed app raises a login-keychain password
/// prompt — and the rotation writer's delete + re-add wipes any
/// user-granted allowance, re-prompting on every Settings visit (the
/// 0.40.31 regression). Reading through the daemon's `security`-CLI reader
/// (`/usr/bin/security` is both in the ACL and in the apple-tool
/// partition) is silent, and it already probes every layout: current blob
/// → legacy-service blob → legacy bare keys.
///
/// Returns the same JSON blob shape the renderer stores
/// (`{"refreshToken","email"}`), or `None` when signed out. The value is
/// NEVER logged.
#[tauri::command]
pub fn k2_account_session_get() -> Result<Option<String>, String> {
    #[cfg(target_os = "macos")]
    {
        Ok(k2_core::tunnel::lease::read_account_session().map(|sess| {
            serde_json::json!({
                "refreshToken": sess.refresh_token,
                "email": sess.email.unwrap_or_default(),
            })
            .to_string()
        }))
    }
    #[cfg(not(target_os = "macos"))]
    {
        // The blob k2_account_session_set wrote via keyring; no partition
        // lists outside macOS, so the crate read is the right path here.
        let e = entry("dev.k2.connect.account", "session")?;
        match e.get_password() {
            Ok(blob) => Ok(Some(blob)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(err) => {
                log_debug!("[secrets] account session get failed: {err}");
                Err(format!("failed to read secret: {err}"))
            }
        }
    }
}

/// Sign-out: remove the persisted session — current blob AND every legacy
/// layout under both service names. macOS deletes through the `security`
/// CLI (same partition-list reason as [`k2_account_session_get`]); other
/// platforms delete the keyring entries directly. Idempotent: missing
/// items are success.
#[tauri::command]
pub fn k2_account_session_clear() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        k2_core::tunnel::lease::clear_account_session();
        log_debug!("[secrets] cleared K2 Connect account session");
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        for service in ["dev.k2.connect.account", "com.k2so.connect.account"] {
            for account in ["session", "session-refresh-token", "session-email"] {
                let e = entry(service, account)?;
                match e.delete_credential() {
                    Ok(()) | Err(keyring::Error::NoEntry) => {}
                    Err(err) => {
                        log_debug!(
                            "[secrets] account session clear failed (service={service}, account={account}): {err}"
                        );
                        return Err(format!("failed to delete secret: {err}"));
                    }
                }
            }
        }
        Ok(())
    }
}

// Every test below exercises the REAL macOS keychain, so the whole module
// is mac-gated — a bare #[cfg(test)] leaves `use super::*` + the helper
// dead on Linux, which the release Linux build gate rejects (-D warnings).
#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    // A unique service name per test process so parallel CI runs (and
    // re-runs) never collide on a real OS keychain entry. The macOS
    // Keychain on a dev box is a shared resource — namespace by pid + a
    // nanosecond stamp.
    fn unique_service() -> String {
        format!(
            "k2so-connect-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        )
    }

    // These tests touch the REAL OS keychain. On a headless Linux CI
    // box with no Secret Service provider running, `Entry::new` /
    // set/get will error — which is itself a valid signal (keychain
    // unavailable), but would fail the assertions. Gate on macOS where
    // the login keychain is always present in this project's CI.
    #[cfg(target_os = "macos")]
    #[test]
    fn set_get_delete_round_trips() {
        let service = unique_service();
        let account = "host-abc123";
        let secret = "bearer-token-xyz";

        // Clean slate.
        let _ = k2_secret_delete(service.clone(), account.to_string());

        // Missing → None (never an error).
        assert_eq!(
            k2_secret_get(service.clone(), account.to_string()).unwrap(),
            None,
            "absent secret must resolve to None, not error"
        );

        // Set then read back the exact value.
        k2_secret_set(service.clone(), account.to_string(), secret.to_string()).unwrap();
        assert_eq!(
            k2_secret_get(service.clone(), account.to_string()).unwrap(),
            Some(secret.to_string()),
            "stored secret must round-trip byte-for-byte"
        );

        // Delete then it's gone again.
        k2_secret_delete(service.clone(), account.to_string()).unwrap();
        assert_eq!(
            k2_secret_get(service.clone(), account.to_string()).unwrap(),
            None,
            "deleted secret must resolve to None"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn delete_is_idempotent_on_missing_entry() {
        let service = unique_service();
        // Deleting a never-set entry must NOT error (toggle-off path).
        k2_secret_delete(service, "host-never-set".to_string()).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn overwrite_updates_value() {
        let service = unique_service();
        let account = "host-overwrite";
        let _ = k2_secret_delete(service.clone(), account.to_string());
        k2_secret_set(service.clone(), account.to_string(), "first".to_string()).unwrap();
        k2_secret_set(service.clone(), account.to_string(), "second".to_string()).unwrap();
        assert_eq!(
            k2_secret_get(service.clone(), account.to_string()).unwrap(),
            Some("second".to_string()),
            "set must overwrite the prior value"
        );
        let _ = k2_secret_delete(service, account.to_string());
    }
}
