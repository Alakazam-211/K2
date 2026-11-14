//! `POST /cli/usage/subscriptions/refresh` that never waits on a vendor
//! (speed pass P5). A child of [`super`]: it uses the probe's own helpers.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
#[cfg(test)]
use std::time::Instant;

use chrono::Utc;
use serde_json::Value;

use super::{airgap_response, airgap_on, fresh_cache, handle_get, handle_refresh, refresh_shared};
use crate::cli_response::CliResponse;

/// The boot-status key that says `POST /cli/usage/subscriptions/refresh`
/// answers at once (speed pass P5). A client on an older daemon waits.
pub const FEATURE_REFRESH_ASYNC: &str = "usage-refresh-async";

/// `POST /cli/usage/subscriptions/refresh`. Without `wait=1` it answers at
/// once ([`handle_refresh_async`]); `wait=1` keeps the answer that waits
/// for the check ([`handle_refresh`]).
pub fn handle_refresh_route(query: &str) -> CliResponse {
    if wants_wait(query) {
        handle_refresh()
    } else {
        handle_refresh_async()
    }
}

pub(super) fn wants_wait(query: &str) -> bool {
    query
        .trim_start_matches('?')
        .split('&')
        .any(|kv| matches!(kv, "wait=1" | "wait=true"))
}

/// The refresh that never waits on a vendor: start a check in the
/// background (unless the numbers are under 15 s old or a check this route
/// started is still running) and answer the saved numbers right away,
/// with `refreshing: true` while that check runs. When it ends the daemon
/// sends `subscription_usage_changed` (`source: "refresh_done"`), and the
/// next read has `refreshing: false`.
pub fn handle_refresh_async() -> CliResponse {
    if airgap_on() {
        return airgap_response();
    }
    start_person_refresh();
    handle_get()
}

/// How long a person's background check is waited on before its mark
/// clears (each vendor has its own, shorter, timeout).
const PERSON_REFRESH_CAP: Duration = Duration::from_secs(60);

fn refresh_pending_count() -> &'static AtomicUsize {
    static PENDING: AtomicUsize = AtomicUsize::new(0);
    &PENDING
}

/// A check the refresh route started is still running.
pub fn refresh_pending() -> bool {
    refresh_pending_count().load(Ordering::SeqCst) > 0
}

/// The served document plus `refreshing` (an object only; the air-gap
/// answer has no check to wait for).
pub(super) fn with_refreshing(mut doc: Value) -> Value {
    if let Some(obj) = doc.as_object_mut() {
        obj.insert("refreshing".into(), Value::Bool(refresh_pending()));
    }
    doc
}

/// Start the person's check without waiting for it.
fn start_person_refresh() {
    if crate::usage_store::single_record_on() {
        crate::llm_accounts_runtime::spawn_adopt_live_logins();
        use crate::usage_refresh::{request_many, Check, Outcome, Why};
        let running: Vec<String> = request_many(&crate::usage_store::live_token_ids(), Why::Person, Duration::ZERO)
            .into_iter()
            .filter(|(_, o)| matches!(o, Outcome::Ran(Check::Pending) | Outcome::Joined(Check::Pending)))
            .map(|(id, _)| id)
            .collect();
        if running.is_empty() {
            return;
        }
        spawn_refresh_waiter(move || crate::usage_refresh::wait_running(&running, PERSON_REFRESH_CAP));
        return;
    }
    if refresh_pending() || fresh_cache(Utc::now()).is_some() {
        return;
    }
    spawn_refresh_waiter(|| {
        if let Err(e) = refresh_shared(None) {
            k2_core::log_debug!("[usage] background refresh failed: {e}");
        }
    });
}

/// Run `work` on its own thread with the pending mark held; when it ends,
/// drop the mark and announce, so a client clears its "refreshing" mark
/// even when no number changed.
fn spawn_refresh_waiter(work: impl FnOnce() + Send + 'static) {
    refresh_pending_count().fetch_add(1, Ordering::SeqCst);
    #[cfg(test)]
    let (pin, scoped) = (crate::usage_store::current_pin(), k2_core::db::scoped_handle_for_test());
    let spawned = std::thread::Builder::new().name("usage-refresh".into()).spawn(move || {
        #[cfg(test)]
        let _pin = pin.map(crate::usage_store::pin);
        #[cfg(test)]
        match scoped {
            Some(handle) => k2_core::db::with_scoped_handle(handle, work),
            None => work(),
        }
        #[cfg(not(test))]
        work();
        refresh_pending_count().fetch_sub(1, Ordering::SeqCst);
        crate::session_events::emit_subscription_usage_changed(None, None, "refresh_done");
    });
    if spawned.is_err() {
        k2_core::log_debug!("[usage] could not start a background refresh");
        refresh_pending_count().fetch_sub(1, Ordering::SeqCst);
    }
}

/// Wait until no check the refresh route started is running (tests).
#[cfg(test)]
pub(crate) fn wait_refresh_idle(cap: Duration) -> bool {
    let deadline = Instant::now() + cap;
    while refresh_pending() {
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    true
}
