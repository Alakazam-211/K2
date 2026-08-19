//! Bounded notify `EventHandler` + Access-event filter (Linux RSS).
//!
//! `notify` 7 only implements `EventHandler` for the unbounded
//! `mpsc::Sender`. A full inotify/FSEvents burst then grows without
//! bound. Charter-watch and fs-live share a `sync_channel` +
//! `try_send` handler that drops on full.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};

use notify::{Event, EventHandler, EventKind};

use k2_core::log_debug;

/// Capacity for charter-watch / fs-live notify channels.
pub(crate) const NOTIFY_CHANNEL_BOUND: usize = 256;

/// `try_send` handler. Full channel increments [`Self::dropped`] and
/// logs occasionally instead of blocking the notify backend thread.
pub(crate) struct DroppingHandler {
    tx: SyncSender<notify::Result<Event>>,
    dropped: AtomicUsize,
}

impl DroppingHandler {
    pub(crate) fn new(tx: SyncSender<notify::Result<Event>>) -> Self {
        Self {
            tx,
            dropped: AtomicUsize::new(0),
        }
    }

    #[cfg(test)]
    pub(crate) fn dropped(&self) -> usize {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl EventHandler for DroppingHandler {
    fn handle_event(&mut self, event: notify::Result<Event>) {
        match self.tx.try_send(event) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                let n = self.dropped.fetch_add(1, Ordering::Relaxed) + 1;
                if n == 1 || n.is_power_of_two() {
                    log_debug!("[daemon/notify] dropped {n} events (channel full)");
                }
            }
            Err(TrySendError::Disconnected(_)) => {}
        }
    }
}

/// Whether a notify kind should be treated as a real source change.
///
/// `Access(_)` (inotify OPEN/CLOSE, including Close(Write) after a
/// `File::open`) and `Other` are ignored. `Any` is kept so FSEvents
/// "something happened" saves are not dropped.
pub(crate) fn should_observe(kind: EventKind) -> bool {
    match kind {
        EventKind::Access(_) | EventKind::Other => false,
        EventKind::Create(_) | EventKind::Remove(_) | EventKind::Modify(_) | EventKind::Any => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::ModifyKind;
    use std::sync::mpsc;

    #[test]
    fn dropping_handler_drops_on_full() {
        let (tx, _rx) = mpsc::sync_channel(2);
        let mut handler = DroppingHandler::new(tx);
        let ev = Event::new(EventKind::Modify(ModifyKind::Any));
        for _ in 0..5 {
            handler.handle_event(Ok(ev.clone()));
        }
        let dropped = handler.dropped();
        assert!(
            dropped >= 3,
            "expected at least 3 drops after filling sync_channel(2) with 5 sends, got {dropped}"
        );
    }
}
