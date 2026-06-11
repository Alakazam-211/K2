//! Narrow terminal access for the companion polling loop.
//!
//! Companion's `run_terminal_polling` needs to read `GridUpdate`s and
//! scrollback for every terminal a WS client is subscribed to. The
//! Tauri app owns the `TerminalManager` inside its `AppState`; this
//! bridge hands k2so-core's companion module a clean handle without
//! dragging AppState into core.
//!
//! Registered once at app startup, same pattern as `settings_bridge`:
//!
//! ```ignore
//! k2_core::companion::terminal_bridge::set_provider(
//!     Box::new(TauriTerminalProvider::new(app_state.clone())),
//! );
//! ```
//!
//! No provider registered → the polling loop sees "terminal missing"
//! for every id, which is safe (just broadcasts nothing).

use parking_lot::Mutex;
use std::sync::OnceLock;

use crate::terminal::grid_types::GridUpdate;

/// The two terminal operations companion needs. Kept minimal —
/// a full `TerminalManager` API surface would over-couple.
/// Note: scrollback returns `Vec<String>` (plain text lines), which
/// matches the signature `TerminalManager::read_lines_with_scrollback`
/// exposes today.
pub trait TerminalProvider: Send + Sync {
    fn get_grid(&self, terminal_id: &str) -> Result<GridUpdate, String>;
    fn read_lines_with_scrollback(
        &self,
        terminal_id: &str,
        limit: usize,
        include_scrollback: bool,
    ) -> Result<Vec<String>, String>;
}

static PROVIDER: OnceLock<Mutex<Option<Box<dyn TerminalProvider>>>> = OnceLock::new();

fn slot() -> &'static Mutex<Option<Box<dyn TerminalProvider>>> {
    PROVIDER.get_or_init(|| Mutex::new(None))
}

pub fn set_provider(p: Box<dyn TerminalProvider>) {
    *slot().lock() = Some(p);
}

pub fn get_grid(terminal_id: &str) -> Result<GridUpdate, String> {
    slot()
        .lock()
        .as_ref()
        .ok_or_else(|| "terminal provider not registered".to_string())?
        .get_grid(terminal_id)
}

pub fn read_lines_with_scrollback(
    terminal_id: &str,
    limit: usize,
    include_scrollback: bool,
) -> Result<Vec<String>, String> {
    slot()
        .lock()
        .as_ref()
        .ok_or_else(|| "terminal provider not registered".to_string())?
        .read_lines_with_scrollback(terminal_id, limit, include_scrollback)
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex as PLMutex;

    /// Because the provider is a process-global OnceLock, parallel
    /// tests that both mutate it race. This mutex serializes every
    /// terminal_bridge test so the global-state assertions stay
    /// deterministic.
    static TEST_LOCK: PLMutex<()> = PLMutex::new(());

    #[test]
    fn unregistered_provider_returns_error() {
        let _g = TEST_LOCK.lock();
        *slot().lock() = None;
        assert!(get_grid("t-1").is_err());
        assert!(read_lines_with_scrollback("t-1", 10, true).is_err());
        // leave the slot cleared so the next test starts from a known state.
        *slot().lock() = None;
    }

    #[test]
    fn registered_provider_is_invoked() {
        let _g = TEST_LOCK.lock();
        struct Fake;
        impl TerminalProvider for Fake {
            fn get_grid(&self, _id: &str) -> Result<GridUpdate, String> {
                Err("fake".to_string())
            }
            fn read_lines_with_scrollback(
                &self,
                _id: &str,
                _limit: usize,
                _inc: bool,
            ) -> Result<Vec<String>, String> {
                Ok(vec![])
            }
        }
        set_provider(Box::new(Fake));
        assert!(get_grid("t-1").is_err());
        assert_eq!(
            read_lines_with_scrollback("t-1", 10, true).unwrap().len(),
            0
        );
        *slot().lock() = None;
    }
}
