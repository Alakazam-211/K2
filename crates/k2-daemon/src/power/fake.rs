//! A counting [`PowerOs`] for tests. Touches nothing real: it records
//! holds, releases, wake calls and approval prompts. The daemon binary
//! compiles it but only the tests and the lib use it.

#![allow(dead_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;

use super::{PowerOs, PowerSource, WakeSupport};

pub struct FakePowerOs {
    support: WakeSupport,
    needs_approval: bool,
    accept_approval: bool,
    pub source: Mutex<PowerSource>,
    taken: AtomicUsize,
    released: Arc<AtomicUsize>,
    prompts: AtomicUsize,
    /// `Some(t)` = set_wake(t); `None` = clear_wake().
    wakes: Mutex<Vec<Option<DateTime<Utc>>>>,
}

/// The fake assertion: counts its own release.
struct FakeGuard(Arc<AtomicUsize>);

impl Drop for FakeGuard {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

impl FakePowerOs {
    fn with(support: WakeSupport, needs_approval: bool, accept_approval: bool) -> Self {
        Self {
            support,
            needs_approval,
            accept_approval,
            source: Mutex::new(PowerSource { on_ac: Some(true), battery_percent: None }),
            taken: AtomicUsize::new(0),
            released: Arc::new(AtomicUsize::new(0)),
            prompts: AtomicUsize::new(0),
            wakes: Mutex::new(Vec::new()),
        }
    }

    /// Can schedule wakes, no approval needed.
    pub fn ready() -> Self {
        Self::with(WakeSupport::Ready, false, true)
    }

    /// Cannot schedule wakes.
    pub fn unavailable(why: &str) -> Self {
        Self::with(WakeSupport::Unavailable(why.to_string()), false, false)
    }

    /// macOS without the helper: turning wake on shows the dialog, which
    /// the "user" accepts or declines.
    pub fn needs_approval(accept: bool) -> Self {
        Self::with(WakeSupport::Unavailable("wake helper not installed".into()), true, accept)
    }

    pub fn holds_taken(&self) -> usize {
        self.taken.load(Ordering::SeqCst)
    }

    pub fn holds_released(&self) -> usize {
        self.released.load(Ordering::SeqCst)
    }

    pub fn approval_prompts(&self) -> usize {
        self.prompts.load(Ordering::SeqCst)
    }

    pub fn wake_calls(&self) -> Vec<Option<DateTime<Utc>>> {
        self.wakes.lock().clone()
    }
}

impl PowerOs for FakePowerOs {
    fn hold_awake(&self, _reason: &str) -> Result<Box<dyn Send>, String> {
        self.taken.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(FakeGuard(Arc::clone(&self.released))))
    }

    fn wake_support(&self) -> WakeSupport {
        self.support.clone()
    }

    fn set_wake(&self, at: DateTime<Utc>) -> Result<(), String> {
        self.wakes.lock().push(Some(at));
        Ok(())
    }

    fn clear_wake(&self) -> Result<(), String> {
        self.wakes.lock().push(None);
        Ok(())
    }

    fn power_source(&self) -> PowerSource {
        *self.source.lock()
    }

    fn wake_needs_approval(&self) -> bool {
        self.needs_approval
    }

    fn install_wake_helper(&self) -> Result<(), String> {
        self.prompts.fetch_add(1, Ordering::SeqCst);
        if self.accept_approval {
            Ok(())
        } else {
            Err("User canceled.".to_string())
        }
    }
}
