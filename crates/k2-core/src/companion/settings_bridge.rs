//! Narrow settings bridge for the companion runtime.
//!
//! The companion server — now in k2so-core — needs to read the user's
//! companion credentials (username, password hash, migration-flag) from
//! the app's settings.json. That file's full parser (`AppSettings` +
//! friends, ~400 lines + Tauri cache-invalidation emits) still lives in
//! `src-tauri/src/commands/settings.rs`. A full settings-module
//! migration is planned (see `.k2so/notes/corners-cut-0.33.0.md` item
//! #18) but not this commit.
//!
//! Meanwhile, this module exposes a tiny three-method trait that the
//! Tauri app implements at startup. Everything k2so-core's companion
//! code needs is behind this indirection — no back-reach into
//! `crate::commands::*` from core.
//!
//! Set the provider once in src-tauri's `setup()`:
//!
//! ```ignore
//! k2_core::companion::settings_bridge::set_provider(
//!     Box::new(TauriCompanionSettingsProvider)
//! );
//! ```
//!
//! If no provider is registered, calls degrade to sensible empty
//! defaults — this keeps the daemon runnable for ping-only smoke tests
//! before everything is wired.

use parking_lot::Mutex;
use std::sync::OnceLock;

/// The shape companion needs from the user's settings. Explicitly NOT
/// `AppSettings` because we don't want to drag the full settings
/// schema into k2so-core — this struct grows only when companion's
/// needs grow.
#[derive(Debug, Clone, Default)]
pub struct CompanionSettingsSnapshot {
    pub username: String,
    pub password_hash: String,
    /// True once the legacy on-disk hash has been migrated into the
    /// macOS Keychain.
    pub password_set: bool,
    /// ngrok auth token (paid plan for reserved-domain persistence).
    pub ngrok_auth_token: String,
    /// Reserved domain for the tunnel (paid feature). Empty string
    /// means use a rotating free-tier URL.
    pub ngrok_domain: String,
    /// Origins allowed to hit the tunnel's browser-facing endpoints.
    pub cors_origins: Vec<String>,
    /// Opt-in: allow remote-spawn of shell commands. A real exposure
    /// increase — the startup banner warns about this.
    pub allow_remote_spawn: bool,
    /// Whether the companion tunnel should be auto-started on host
    /// boot. Read by the daemon's first-boot hook (Phase 2 Unit 1) so
    /// the tunnel comes up without requiring Tauri to be running.
    pub auto_start: bool,
}

/// Implemented by the Tauri app. `&self` methods so an `Arc` can live
/// in the OnceLock without interior mutability surprises.
pub trait CompanionSettingsProvider: Send + Sync {
    fn read(&self) -> CompanionSettingsSnapshot;
    fn clear_password_hash_after_migration(&self);
}

static PROVIDER: OnceLock<Mutex<Option<Box<dyn CompanionSettingsProvider>>>> =
    OnceLock::new();

fn slot() -> &'static Mutex<Option<Box<dyn CompanionSettingsProvider>>> {
    PROVIDER.get_or_init(|| Mutex::new(None))
}

/// Register the Tauri app's settings provider. Idempotent — second
/// call overwrites the first, which is useful in tests.
pub fn set_provider(p: Box<dyn CompanionSettingsProvider>) {
    *slot().lock() = Some(p);
}

/// Read the currently-configured companion settings. Returns empty
/// defaults if no provider has been registered (dev / smoke-test
/// scenarios where the Tauri app hasn't started yet).
pub fn read_settings() -> CompanionSettingsSnapshot {
    slot()
        .lock()
        .as_ref()
        .map(|p| p.read())
        .unwrap_or_default()
}

/// Forward the post-migration flag-write. No-op if no provider is
/// registered.
pub fn clear_password_hash_after_migration() {
    if let Some(p) = slot().lock().as_ref() {
        p.clear_password_hash_after_migration();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn read_without_provider_returns_defaults() {
        // Clear any provider set by prior tests.
        *slot().lock() = None;
        let s = read_settings();
        assert_eq!(s.username, "");
        assert_eq!(s.password_hash, "");
        assert!(!s.password_set);
    }

    #[test]
    fn set_provider_is_used_by_read() {
        struct Fake;
        impl CompanionSettingsProvider for Fake {
            fn read(&self) -> CompanionSettingsSnapshot {
                CompanionSettingsSnapshot {
                    username: "rosson".into(),
                    password_hash: "$argon2id$...".into(),
                    password_set: true,
                    ..CompanionSettingsSnapshot::default()
                }
            }
            fn clear_password_hash_after_migration(&self) {}
        }
        set_provider(Box::new(Fake));
        let s = read_settings();
        assert_eq!(s.username, "rosson");
        assert_eq!(s.password_hash, "$argon2id$...");
        assert!(s.password_set);
    }

    #[test]
    fn clear_password_hash_delegates_to_provider() {
        let counter = Arc::new(AtomicUsize::new(0));
        struct Fake(Arc<AtomicUsize>);
        impl CompanionSettingsProvider for Fake {
            fn read(&self) -> CompanionSettingsSnapshot {
                CompanionSettingsSnapshot::default()
            }
            fn clear_password_hash_after_migration(&self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        set_provider(Box::new(Fake(counter.clone())));
        clear_password_hash_after_migration();
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }
}
