//! The template hello on an agent's first start (prd-agent-templates-v1 §3,
//! T9).
//!
//! A template hire's first create renders the template's hello into
//! `.k2/agent/template.lock` as `hello_pending`. Whatever starts the agent
//! first sends it, once:
//!
//! - `POST /cli/agents/hire` with `launch: true` (a first create, or a
//!   re-run of a hire that didn't launch) takes it before it starts the
//!   session, then delivers it and the hirer's `onboard` after it, in order;
//! - any other start of the workspace's chat session (the app opening it,
//!   a wake, `ensure-canonical-session`, a later `k2 agent hire --launch`
//!   whose hire step had nothing to take) reaches [`on_session_registered`]
//!   from `v2_session_map::register`, the one point every new session
//!   passes, which takes it and delivers it.
//!
//! Taking it records `hello_sent_at` in the lock before anything is
//! delivered, so it never goes twice. A lock written before the field
//! existed has no `hello_pending`: an agent made then never gets a
//! surprise hello. Later starts send nothing.
//!
//! The hello goes as the owner, so the lock alone never decides it:
//!
//! - only a hello K2's hire wrote is taken: the hire records its sha256
//!   in the daemon's database (`hire::template::hello_record`), and the
//!   take matches the lock's text against it and removes the record. A
//!   lock put in a folder by hand, or changed after the hire, is never
//!   delivered;
//! - nothing is taken while the folder-trust gate holds or denies the
//!   workspace's folder ([`folder_trusted`]). The hello stays pending: no
//!   AI tool starts there until a person trusts the folder (the spawn door
//!   asks the same gate), and the first start after that sends it.

use k2_core::cli_folder_trust::gate;
use k2_core::hire::template::lock;
use k2_core::hire::{links, HireCaller};

use crate::hire_routes::Sender;

/// What the daemon would have delivered, in unit tests (no session starts
/// there): `(handle, hello, onboard)`.
#[cfg(test)]
pub(crate) static TEST_HELLOS: std::sync::Mutex<Vec<(String, String, Option<String>)>> = std::sync::Mutex::new(Vec::new());

/// The folder-trust gate lets K2 start programs in `path` now (look only:
/// no grant written, no card). Held, denied or not checkable: false. A
/// test build's non-temp HOME skip passes, as at the spawn door.
fn folder_trusted(path: &str) -> bool {
    match gate::check(path) {
        gate::Outcome::Granted { .. } => true,
        gate::Outcome::Skipped(why) if why == gate::SKIP_TEST_HOME => true,
        other => {
            k2_core::log_debug!("[template-hello] {path}: the folder isn't trusted now ({}); the hello waits", other.to_json()["state"]);
            false
        }
    }
}

/// Take the pending hello of the workspace `workspace_id` at `path`
/// (recorded as sent first). `None`: nothing to send, the folder isn't
/// trusted now (it stays pending), the hello isn't one K2's hire wrote,
/// or the lock couldn't be updated (then nothing is sent, so it can never
/// go twice).
pub(crate) fn take(workspace_id: &str, path: &str) -> Option<String> {
    if !lock::has_pending_hello(path) || !folder_trusted(path) {
        return None;
    }
    match lock::take_pending_hello(workspace_id, path) {
        Ok(h) => h,
        Err(e) => {
            k2_core::log_debug!("[template-hello] {path}: {}", e.hint);
            None
        }
    }
}

/// Start the agent's session if it isn't running, then deliver `hello` and
/// `onboard` after it, in order, off the caller's thread. A failure is
/// logged; the agent stands either way.
pub(crate) fn send(path: &str, handle: &str, hello: String, onboard: Option<String>, from: String, sender: Sender) {
    #[cfg(test)]
    {
        let _ = (path, from, sender);
        TEST_HELLOS.lock().unwrap_or_else(|p| p.into_inner()).push((handle.to_string(), hello, onboard));
    }
    #[cfg(not(test))]
    {
        let (path, handle) = (path.to_string(), handle.to_string());
        let label = handle.clone();
        let spawned = crate::rt::spawn("template-hello", move || {
            if let Err(e) = crate::canonical_session::ensure_canonical_session(&path) {
                k2_core::log_debug!("[template-hello] {handle}: launch failed: {e}");
                return;
            }
            for text in std::iter::once(hello).chain(onboard) {
                let (r, _) = crate::hire_routes::deliver_recorded(
                    &handle,
                    &text,
                    &from,
                    "",
                    true,
                    crate::workspace_msg::DEFAULT_WAKE_TIMEOUT,
                    "hire",
                    &sender,
                );
                if !r.success {
                    k2_core::log_debug!("[template-hello] {handle}: {}", r.reason.unwrap_or_default());
                    return;
                }
            }
        });
        if let Err(e) = spawned {
            k2_core::log_debug!("[template-hello] {label}: not started: {e}");
        }
    }
}

/// A workspace's chat session key is its bare project id (a UUID); a
/// heartbeat lane (`<id>:hb:<name>`), a tab or a sidecar is not.
fn is_chat_key(key: &str) -> bool {
    let b = key.as_bytes();
    b.len() == 36
        && key.bytes().enumerate().all(|(i, c)| if matches!(i, 8 | 13 | 18 | 23) { c == b'-' } else { c.is_ascii_hexdigit() })
}

/// `v2_session_map::register` saw a new session under `key`. When it is a
/// workspace's chat session running an agent (not a login shell), check
/// that workspace for a pending hello, off the caller's thread (register's
/// callers may hold locks).
pub(crate) fn on_session_registered(key: &str, program: Option<&str>) {
    if !is_chat_key(key) || program.is_none_or(|p| p.trim().is_empty()) {
        return;
    }
    #[cfg(not(test))]
    {
        let key = key.to_string();
        if let Err(e) = crate::rt::spawn("template-hello-start", move || {
            launch_hello(&key);
            // T3: the cards a hire couldn't file (no Thread then).
            crate::hire_permissions::file_pending_cards_when_ready(&key);
        }) {
            k2_core::log_debug!("[template-hello] check not started: {e}");
        }
    }
}

/// The start hook's body: send the workspace's pending hello, if any, as
/// the owner (the person the agent works for). True when one was taken.
pub(crate) fn launch_hello(workspace: &str) -> bool {
    let ws = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        links::resolve_workspace(&conn, workspace)
    };
    let Some(ws) = ws else { return false };
    let Some(hello) = take(&ws.id, &ws.path) else { return false };
    let from = k2_core::person_name::sender_identity_now(k2_core::person_name::Sender::OwnerToken);
    send(&ws.path, &ws.handle, hello, None, from, Sender::from_hire_caller(&HireCaller::Owner, None));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_hello_chat_keys_only() {
        assert!(is_chat_key("0b5f2b1e-3c4d-4e5f-8a9b-0c1d2e3f4a5b"));
        assert!(!is_chat_key("0b5f2b1e-3c4d-4e5f-8a9b-0c1d2e3f4a5b:hb:daily"));
        assert!(!is_chat_key("tab-12"));
        assert!(!is_chat_key("ops/reviewer"));
    }
}
