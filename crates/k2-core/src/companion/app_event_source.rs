//! Inbound events: host UI → companion.
//!
//! Companion's WebSocket server mirrors a handful of Tauri-emitted
//! events (`agent:lifecycle`, `agent:reply`, `sync:projects`) out to
//! any connected mobile client. Historically this was wired via
//! `tauri::AppHandle::listen` inside companion/mod.rs — a direct
//! Tauri dep which blocks companion from moving into core.
//!
//! This bridge inverts the control: the Tauri app implements
//! `AppEventSource`, registers it at startup, and — when it emits one
//! of the watched events — also pushes the payload into any
//! registered companion handler. Companion's WS fan-out reads from
//! there.
//!
//! For scope: we don't try to replicate Tauri's full listen semantics
//! (multi-window isolation, one-shot listeners). Companion only needs
//! broadcast fan-out of fire-and-forget events.

use parking_lot::Mutex;
use std::sync::OnceLock;

pub type AppEventHandler = Box<dyn Fn(&str, &str) + Send + Sync>;

pub trait AppEventSource: Send + Sync {
    /// Register a handler that fires for every occurrence of any of
    /// `events` in the host app. Handler receives (event_name,
    /// payload_as_json_string).
    fn subscribe(&self, events: &[&'static str], handler: AppEventHandler);
}

static SOURCE: OnceLock<Mutex<Option<Box<dyn AppEventSource>>>> = OnceLock::new();

fn slot() -> &'static Mutex<Option<Box<dyn AppEventSource>>> {
    SOURCE.get_or_init(|| Mutex::new(None))
}

pub fn set_source(s: Box<dyn AppEventSource>) {
    *slot().lock() = Some(s);
}

/// Called by companion to register a broadcast-to-WS handler. No-op if
/// no source registered (daemon-only context where there's no Tauri
/// app to subscribe to).
pub fn subscribe(events: &[&'static str], handler: AppEventHandler) {
    if let Some(s) = slot().lock().as_ref() {
        s.subscribe(events, handler);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex as PLMutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    static TEST_LOCK: PLMutex<()> = PLMutex::new(());

    #[test]
    fn subscribe_without_source_is_silent_noop() {
        let _g = TEST_LOCK.lock();
        *slot().lock() = None;
        subscribe(
            &["agent:lifecycle"],
            Box::new(|_, _| panic!("should not be called")),
        );
    }

    #[test]
    fn registered_source_receives_subscribe_call() {
        let _g = TEST_LOCK.lock();
        let calls = Arc::new(AtomicUsize::new(0));
        struct Fake(Arc<AtomicUsize>);
        impl AppEventSource for Fake {
            fn subscribe(&self, events: &[&'static str], _handler: AppEventHandler) {
                assert_eq!(events, &["agent:lifecycle", "agent:reply"]);
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        set_source(Box::new(Fake(calls.clone())));
        subscribe(
            &["agent:lifecycle", "agent:reply"],
            Box::new(|_e, _p| {}),
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        *slot().lock() = None;
    }
}
