//! Outbound events: companion → host UI.
//!
//! Companion emits UI notifications like `companion:tunnel_activated`
//! when the ngrok tunnel comes up. The Tauri app listens for these
//! and shows a toast / updates the Settings panel. Daemon builds would
//! fan them out over WebSocket instead.
//!
//! Tiny one-method trait, set-once global. No provider registered →
//! emits are silently dropped.

use parking_lot::Mutex;
use std::sync::OnceLock;

pub trait CompanionEventSink: Send + Sync {
    fn emit(&self, event: &str, payload: serde_json::Value);
}

static SINK: OnceLock<Mutex<Option<Box<dyn CompanionEventSink>>>> = OnceLock::new();

fn slot() -> &'static Mutex<Option<Box<dyn CompanionEventSink>>> {
    SINK.get_or_init(|| Mutex::new(None))
}

pub fn set_sink(s: Box<dyn CompanionEventSink>) {
    *slot().lock() = Some(s);
}

pub fn emit(event: &str, payload: serde_json::Value) {
    if let Some(s) = slot().lock().as_ref() {
        s.emit(event, payload);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex as PLMutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// Serialize global-state assertions — the sink is a
    /// process-global OnceLock.
    static TEST_LOCK: PLMutex<()> = PLMutex::new(());

    #[test]
    fn emit_without_sink_is_silent_noop() {
        let _g = TEST_LOCK.lock();
        *slot().lock() = None;
        emit("companion:test", serde_json::json!({"k": "v"}));
    }

    #[test]
    fn registered_sink_gets_event_and_payload() {
        let _g = TEST_LOCK.lock();
        let count = Arc::new(AtomicUsize::new(0));
        let count_clone = count.clone();
        struct Fake(Arc<AtomicUsize>);
        impl CompanionEventSink for Fake {
            fn emit(&self, event: &str, _payload: serde_json::Value) {
                assert_eq!(event, "companion:tunnel_activated");
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        set_sink(Box::new(Fake(count_clone)));
        emit(
            "companion:tunnel_activated",
            serde_json::json!({"tunnelUrl": "https://x.ngrok.app"}),
        );
        assert_eq!(count.load(Ordering::SeqCst), 1);
        *slot().lock() = None;
    }
}
