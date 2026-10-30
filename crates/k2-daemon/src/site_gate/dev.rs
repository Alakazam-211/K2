//! The gate's calls into dev mode's process lane (`publish_runtime`, DM-6 and
//! vs-live PA-11). Dev mode installs them at boot; until it does, nothing is
//! a dev process (`NotDev`) and a touch does nothing.

use std::sync::RwLock;

/// What the gate should do for a visitor who passed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DevWake {
    /// Running: dial it.
    Up,
    /// On its way: answer K2's "Starting…" page.
    Starting,
    /// It couldn't start (why is logged and shown in `ps`, never to a visitor).
    Failed(String),
    /// Not a dev process: dial it as usual.
    NotDev,
}

/// `touch` is called for every request that passes the gate on a dev label,
/// and while a spliced WebSocket carries data; it must be cheap. `wake` is
/// called once a visitor passed, before the dial; it must never block. A
/// visitor who fails the gate calls neither.
#[derive(Clone, Copy)]
pub struct DevHooks {
    pub touch: fn(&str),
    pub wake: fn(&str) -> DevWake,
}

static HOOKS: RwLock<Option<DevHooks>> = RwLock::new(None);

pub fn install(h: DevHooks) {
    *HOOKS.write().unwrap_or_else(|p| p.into_inner()) = Some(h);
}

pub(super) fn touch(label: &str) {
    if let Some(h) = *HOOKS.read().unwrap_or_else(|p| p.into_inner()) {
        (h.touch)(label);
    }
}

pub(super) fn wake(label: &str) -> DevWake {
    match *HOOKS.read().unwrap_or_else(|p| p.into_inner()) {
        Some(h) => (h.wake)(label),
        None => DevWake::NotDev,
    }
}
