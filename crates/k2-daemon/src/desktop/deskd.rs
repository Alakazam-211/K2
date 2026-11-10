//! The daemon's side of `k2-deskd` ("K2 Screen"), behind a small trait so
//! the routes run headless before deskd exists (D3 builds the real one:
//! the LaunchAgent on macOS, the child over pipes on Linux).
//!
//! [`NotBuilt`] is what every build has until then: it reports
//! `unavailable` with why, lists no displays, and its enable/disable do
//! nothing (the switch itself is the daemon's, in `k2_core::desktop`).
//! Tests install a fake with [`set_for_test`].

use std::sync::{Arc, OnceLock, RwLock};

use k2_core::desktop::wire::Display;
use k2_core::desktop::MachineState;

/// What the routes need from deskd.
pub trait Deskd: Send + Sync {
    /// `macos` | `linux` | `windows`.
    fn backend(&self) -> &'static str;
    /// The machine's state right now.
    fn state(&self) -> MachineState;
    /// The switch went on: install / arm deskd.
    fn enable(&self) -> Result<(), String>;
    /// The switch went off (viewers are already closed): stop deskd.
    fn disable(&self) -> Result<(), String>;
    /// The displays a viewer may pick.
    fn displays(&self) -> Vec<Display>;
}

/// This OS's backend name.
pub fn os_backend() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(windows) {
        "windows"
    } else {
        "linux"
    }
}

/// No deskd in this build yet.
pub struct NotBuilt;

impl Deskd for NotBuilt {
    fn backend(&self) -> &'static str {
        os_backend()
    }
    fn state(&self) -> MachineState {
        let why = if cfg!(windows) {
            "Remote desktop isn't on Windows yet."
        } else {
            "K2 Screen isn't in this build of K2 yet, so there is no picture to show."
        };
        MachineState::Unavailable(why.into())
    }
    fn enable(&self) -> Result<(), String> {
        Ok(())
    }
    fn disable(&self) -> Result<(), String> {
        Ok(())
    }
    fn displays(&self) -> Vec<Display> {
        Vec::new()
    }
}

fn slot() -> &'static RwLock<Arc<dyn Deskd>> {
    static S: OnceLock<RwLock<Arc<dyn Deskd>>> = OnceLock::new();
    S.get_or_init(|| RwLock::new(Arc::new(NotBuilt)))
}

/// The deskd backend in use.
pub fn current() -> Arc<dyn Deskd> {
    slot().read().unwrap_or_else(|p| p.into_inner()).clone()
}

/// Tests (and D3's real backend at boot): put `d` in place. Returns the
/// one it replaced.
pub fn install(d: Arc<dyn Deskd>) -> Arc<dyn Deskd> {
    std::mem::replace(&mut *slot().write().unwrap_or_else(|p| p.into_inner()), d)
}

/// [`install`] for tests, restored when the guard drops.
pub fn set_for_test(d: Arc<dyn Deskd>) -> TestDeskd {
    TestDeskd { prev: Some(install(d)) }
}

pub struct TestDeskd {
    prev: Option<Arc<dyn Deskd>>,
}

impl Drop for TestDeskd {
    fn drop(&mut self) {
        if let Some(p) = self.prev.take() {
            install(p);
        }
    }
}
