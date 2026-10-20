//! The one time source (contracts rev 4).
//!
//! Product code reads time only through a [`Clock`], so tests can move time. This module holds
//! the only `SystemTime::now()` call in the crate.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::time::Timestamp;

/// A source of wall-clock time that tests can replace.
pub trait Clock: Send + Sync {
    /// Milliseconds since the Unix epoch.
    fn now_unix_ms(&self) -> u64;

    /// Waits `ms` milliseconds. `0` means "yield once".
    fn sleep_ms(&self, ms: u64);

    /// The current time as a [`Timestamp`].
    fn now(&self) -> Timestamp {
        Timestamp::from_unix_ms_saturating(self.now_unix_ms())
    }
}

/// The real clock: `SystemTime::now()` and `std::thread::sleep`.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_unix_ms(&self) -> u64 {
        match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(d) => u64::try_from(d.as_millis()).unwrap_or(u64::MAX),
            // A clock before 1970 reads as 0; the id generator then refuses it loudly.
            Err(_) => 0,
        }
    }

    fn sleep_ms(&self, ms: u64) {
        if ms == 0 {
            std::thread::yield_now();
        } else {
            std::thread::sleep(std::time::Duration::from_millis(ms));
        }
    }
}

impl<C: Clock + ?Sized> Clock for Arc<C> {
    fn now_unix_ms(&self) -> u64 {
        (**self).now_unix_ms()
    }
    fn sleep_ms(&self, ms: u64) {
        (**self).sleep_ms(ms)
    }
}

impl<C: Clock + ?Sized> Clock for Box<C> {
    fn now_unix_ms(&self) -> u64 {
        (**self).now_unix_ms()
    }
    fn sleep_ms(&self, ms: u64) {
        (**self).sleep_ms(ms)
    }
}

impl<C: Clock + ?Sized> Clock for &C {
    fn now_unix_ms(&self) -> u64 {
        (**self).now_unix_ms()
    }
    fn sleep_ms(&self, ms: u64) {
        (**self).sleep_ms(ms)
    }
}
