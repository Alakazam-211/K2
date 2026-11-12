//! The daemon's side of `k2-deskd` ("K2 Screen"), behind a small trait so
//! the routes run headless before deskd exists (D3 builds the real one:
//! the LaunchAgent on macOS, the child over pipes on Linux).
//!
//! [`NotBuilt`] is what every build has until then: it reports
//! `unavailable` with why, lists no displays, and its enable/disable do
//! nothing (the switch itself is the daemon's, in `k2_core::desktop`).
//! Tests install a fake with [`set_for_test`].

use std::collections::HashMap;
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
    /// The stream's side (`/cli/desktop/stream`, D6); `None` = no picture
    /// here (the stream answers `desktop_unavailable`).
    fn stream(&self) -> Option<Arc<dyn super::stream::StreamDeskd>> {
        None
    }
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

// ── one K2 Screen per desktop (LD-22) ────────────────────────────────

/// Makes the K2 Screen of a virtual desktop (boot installs the real one:
/// a K2 Screen child per display, Linux).
pub type Factory = Arc<dyn Fn(&str) -> Arc<dyn Deskd> + Send + Sync>;

fn per_desktop() -> &'static RwLock<HashMap<String, Arc<dyn Deskd>>> {
    static S: OnceLock<RwLock<HashMap<String, Arc<dyn Deskd>>>> = OnceLock::new();
    S.get_or_init(|| RwLock::new(HashMap::new()))
}

fn factory() -> &'static RwLock<Option<Factory>> {
    static F: OnceLock<RwLock<Option<Factory>>> = OnceLock::new();
    F.get_or_init(|| RwLock::new(None))
}

/// Boot: how a virtual desktop's K2 Screen is made.
pub fn set_virtual_factory(f: Factory) {
    *factory().write().unwrap_or_else(|p| p.into_inner()) = Some(f);
}

/// The K2 Screen of a desktop: [`current`] for `console`, one per id for
/// a virtual desktop (made on first use; [`NotBuilt`] with no factory).
pub fn for_desktop(id: &str) -> Arc<dyn Deskd> {
    if !k2_core::desktop::virtual_desktop::is_virtual(id) {
        return current();
    }
    if let Some(d) = per_desktop().read().unwrap_or_else(|p| p.into_inner()).get(id) {
        return d.clone();
    }
    let made = factory().read().unwrap_or_else(|p| p.into_inner()).clone().map_or_else(|| Arc::new(NotBuilt) as Arc<dyn Deskd>, |f| f(id));
    per_desktop().write().unwrap_or_else(|p| p.into_inner()).entry(id.to_string()).or_insert(made).clone()
}

/// Tests: a fake K2 Screen for one virtual desktop until the guard drops.
pub fn set_for_test_desktop(id: &str, d: Arc<dyn Deskd>) -> TestDesktopDeskd {
    let prev = per_desktop().write().unwrap_or_else(|p| p.into_inner()).insert(id.to_string(), d);
    TestDesktopDeskd { id: id.to_string(), prev }
}

pub struct TestDesktopDeskd {
    id: String,
    prev: Option<Arc<dyn Deskd>>,
}

impl Drop for TestDesktopDeskd {
    fn drop(&mut self) {
        let mut m = per_desktop().write().unwrap_or_else(|p| p.into_inner());
        match self.prev.take() {
            Some(p) => {
                m.insert(self.id.clone(), p);
            }
            None => {
                m.remove(&self.id);
            }
        }
    }
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
