//! Daemon-side `/cli/push/*` routes + the C4 dispatch triggers —
//! Companion C4 (prd-companion-v2 §4; feedback PRD §8.4 Option B).
//!
//! Registration: the companion upserts its device on every app launch
//! (`register-device`, keyed by a stable deviceId — which also absorbs
//! APNs/FCM token rotation) and removes it when the user toggles push
//! off (`unregister-device`). Both are JSON-bodied POSTs dispatched by
//! an isolated arm in `routes::dispatcher` (token_ok auth — ANY authed
//! user: owner or connect-user; the arm resolves the acting username
//! from the session token, NEVER the request body — the D3
//! discipline — and passes it here for attribution). GETs on these
//! paths 405 through `crate::cli::dispatch`
//! (feedback_post_only_route_guards house rule).
//!
//! Dispatch: [`notify_feedback_created`] / [`notify_project_message_created`]
//! are called next to the existing `/events` emits in feedback_routes /
//! project_group_routes. They build a CONTENT-FREE [`PushEvent`] (§4.5
//! invariant: generic title + attribution line + IDs only — ask/message
//! TEXT never rides APNs/FCM) and hand it to the process-wide
//! [`k2_core::push::K2Cloud`] adapter on a detached thread
//! (fire-and-forget, the PushTarget contract — no retry, no queue;
//! failures are logged). When no gateway URL + token is configured the
//! adapter is never constructed and every notify is a no-op: DORMANT —
//! the shipped-but-inactive state until the relay gateway deploys.
//! V1 fans out to ALL registered devices (per-user subscriptions
//! later).

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use crate::cli_response::CliResponse;
use k2_core::push::{K2Cloud, PushEvent, PushTarget};
use k2_core::push_devices;

// ── Route dispatch ────────────────────────────────────────────────────

/// Push-domain GET dispatch. Both routes are POST-only mutations —
/// reached via the GET chain they answer an explicit 405. Returns
/// `None` if the path isn't a push-domain route.
pub fn dispatch(path: &str, _params: &HashMap<String, String>) -> Option<CliResponse> {
    match path {
        "/cli/push/register-device" | "/cli/push/unregister-device" => {
            Some(CliResponse::method_not_allowed())
        }
        _ => None,
    }
}

/// Dispatch a `/cli/push/*` POST body to its handler. `username` is
/// the daemon-resolved authed actor (`"owner"` | a connect-user name)
/// — resolved by the dispatcher arm from the session token, never
/// from the body. Unknown paths 404 (mirrors `dispatch_unit6_post`).
pub fn dispatch_post(path: &str, body: &[u8], username: &str) -> CliResponse {
    match path {
        "/cli/push/register-device" => handle_register(body, username),
        "/cli/push/unregister-device" => handle_unregister(body),
        _ => CliResponse::not_found(),
    }
}

/// Stable usage-error shape (code `usage` → CLI exit 2; the
/// feedback_routes idiom).
fn usage_error(hint: impl std::fmt::Display) -> CliResponse {
    CliResponse {
        status: "400 Bad Request",
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": { "code": "usage", "hint": hint.to_string() },
        })
        .to_string(),
    }
}

/// `POST /cli/push/register-device` body.
#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct RegisterBody {
    device_id: String,
    platform: String,
    token: String,
}

/// Handler for `POST /cli/push/register-device` — upsert keyed by
/// deviceId (re-sent every app launch; rotation lands for free).
fn handle_register(body: &[u8], username: &str) -> CliResponse {
    let b: RegisterBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return usage_error(format!("invalid JSON body: {e}")),
    };
    match push_devices::upsert_device(&b.device_id, &b.platform, &b.token, username) {
        Ok(device) => {
            let mut v = serde_json::to_value(&device).unwrap_or_else(|_| serde_json::json!({}));
            if let Some(map) = v.as_object_mut() {
                map.insert("ok".to_string(), serde_json::json!(true));
            }
            CliResponse::ok_json(v.to_string())
        }
        Err(e) => usage_error(e),
    }
}

/// `POST /cli/push/unregister-device` body.
#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct UnregisterBody {
    device_id: String,
}

/// Handler for `POST /cli/push/unregister-device`. Removing an
/// already-gone device is a fine no-op (`removed:false`) — the
/// companion's Settings toggle fires blind.
fn handle_unregister(body: &[u8]) -> CliResponse {
    let b: UnregisterBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return usage_error(format!("invalid JSON body: {e}")),
    };
    if b.device_id.trim().is_empty() {
        return usage_error("missing 'deviceId'");
    }
    match push_devices::remove_device(&b.device_id) {
        Ok(removed) => CliResponse::ok_json(
            serde_json::json!({ "ok": true, "removed": removed }).to_string(),
        ),
        Err(e) => usage_error(e),
    }
}

// ── Dispatch triggers (V1) ────────────────────────────────────────────

/// `feedback:created` → mobile push. Called next to the
/// `HookEvent::FeedbackCreated` emit in `feedback_routes::handle_create`.
/// When `assignees` is non-empty, only those usernames' devices receive
/// the push; empty = fan out to everyone on the server (legacy).
pub fn notify_feedback_created(agent_name: &str, feedback_id: &str, assignees: &[String]) {
    dispatch_push(PushEvent::FeedbackCreated {
        agent_name: agent_name.to_string(),
        feedback_id: feedback_id.to_string(),
        assignees: assignees.to_vec(),
    });
}

/// Ticket thread reply → mobile push to assignees (or everyone if
/// unassigned). Content-free: ticket id only.
pub fn notify_feedback_commented(feedback_id: &str, assignees: &[String]) {
    dispatch_push(PushEvent::FeedbackCommented {
        feedback_id: feedback_id.to_string(),
        assignees: assignees.to_vec(),
    });
}

/// `project-group:message-created` with author != "owner" → mobile
/// push. Called next to the `ProjectGroupMessageCreated` emit in
/// `project_group_routes` — the CALLER applies the author gate (the
/// rate-sanity rule: never push the owner their own message; V1 keeps
/// it simple otherwise).
pub fn notify_project_message_created(project_name: &str, group_id: &str) {
    dispatch_push(PushEvent::ProjectMessageCreated {
        project_name: project_name.to_string(),
        group_id: group_id.to_string(),
    });
}

/// The process-wide push target, resolved ONCE from config
/// (`K2_PUSH_GATEWAY_URL`/`K2_PUSH_GATEWAY_TOKEN` env, falling back to
/// the `pushGatewayUrl`/`pushGatewayToken` settings — see
/// `K2Cloud::from_config`). `None` = DORMANT: leaving dormancy takes a
/// daemon restart after configuring, per the PRD's "startup no-op"
/// contract.
// Unreachable under cfg(test): dispatch_push short-circuits into the
// capture sink before the configured target is ever consulted.
#[cfg_attr(test, allow(dead_code))]
fn configured_target() -> Option<Arc<dyn PushTarget>> {
    static TARGET: OnceLock<Option<Arc<dyn PushTarget>>> = OnceLock::new();
    TARGET
        .get_or_init(|| K2Cloud::from_config().map(|c| Arc::new(c) as Arc<dyn PushTarget>))
        .clone()
}

fn dispatch_push(event: PushEvent) {
    // Test seam: unit tests capture the event instead of touching
    // config or spawning senders (no live network in tests).
    #[cfg(test)]
    {
        test_push_capture::capture(&event);
        return;
    }
    #[cfg(not(test))]
    {
        dispatch_with(configured_target(), event);
    }
}

/// Fire-and-forget core, target injected for testability: `None` =
/// DORMANT no-op (returns `None`); `Some` sends on a detached thread —
/// the route handler never waits on the gateway, and per the
/// `PushTarget` contract a failure is logged, never retried or queued.
fn dispatch_with(
    target: Option<Arc<dyn PushTarget>>,
    event: PushEvent,
) -> Option<std::thread::JoinHandle<()>> {
    let target = target?;
    Some(std::thread::spawn(move || {
        if let Err(e) = target.send(&event) {
            k2_core::log_debug!("[push] dispatch failed ({}): {e}", target.label());
        }
    }))
}

/// Test-only capture sink for [`dispatch_push`]. Process-global and
/// additive (the agent-hooks capture-sink idiom): trigger tests take a
/// [`mark`](test_push_capture::mark), drive a route, then assert on
/// [`since`](test_push_capture::since) filtered by their own unique
/// ids — cross-test traffic is invisible.
#[cfg(test)]
pub(crate) mod test_push_capture {
    use k2_core::push::PushEvent;
    use std::sync::Mutex;

    static CAPTURED: Mutex<Vec<PushEvent>> = Mutex::new(Vec::new());

    pub(crate) fn capture(event: &PushEvent) {
        CAPTURED.lock().expect("push capture lock").push(event.clone());
    }

    pub(crate) fn mark() -> usize {
        CAPTURED.lock().expect("push capture lock").len()
    }

    pub(crate) fn since(mark: usize) -> Vec<PushEvent> {
        CAPTURED.lock().expect("push capture lock")[mark..].to_vec()
    }
}

// ──────────────────────────────────────────────────────────────────────
// Inline unit tests — Companion C4 push routes + dispatch core
// ──────────────────────────────────────────────────────────────────────
//
// Mirrors feedback_routes' test module: `db::shared()` auto-inits the
// PROCESS-GLOBAL in-memory DB shared across the test binary — every
// test uses its own unique device/token ids.

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn did(label: &str) -> String {
        format!("dev-routes-{label}-{}", uuid::Uuid::new_v4())
    }

    fn tok(label: &str) -> String {
        format!("tok-routes-{label}-{}", uuid::Uuid::new_v4())
    }

    fn post_json(path: &str, body: serde_json::Value, username: &str) -> CliResponse {
        dispatch_post(path, body.to_string().as_bytes(), username)
    }

    fn ok_json(resp: CliResponse) -> serde_json::Value {
        assert_eq!(resp.status, "200 OK", "body={}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(v["ok"], true, "body={v}");
        v
    }

    /// GET on the POST-only mutations answers an explicit 405 —
    /// through the module dispatch AND the full `cli::dispatch` chain
    /// (feedback_post_only_route_guards house rule); unknown push
    /// paths stay unclaimed on GET and 404 on POST.
    #[test]
    fn push_mutations_405_on_get_and_post_404_unknown() {
        let params = HashMap::new();
        for route in ["/cli/push/register-device", "/cli/push/unregister-device"] {
            let resp = dispatch(route, &params).expect("route claimed");
            assert_eq!(resp.status, "405 Method Not Allowed", "route={route}");
            let resp = crate::cli::dispatch(route, &params);
            assert_eq!(
                resp.status, "405 Method Not Allowed",
                "route={route} through the full GET chain"
            );
        }
        assert!(
            dispatch("/cli/push/list-devices", &params).is_none(),
            "unknown push paths are not claimed on GET"
        );
        let resp = dispatch_post("/cli/push/nope", b"{}", "owner");
        assert_eq!(resp.status, "404 Not Found");
    }

    /// register-device round-trip: insert, then re-register the SAME
    /// deviceId with a rotated token and different actor → ONE row,
    /// updated in place, created_at preserved.
    #[test]
    fn register_device_upserts_and_records_resolved_username() {
        let id = did("upsert");
        let t1 = tok("upsert-1");
        let v = ok_json(post_json(
            "/cli/push/register-device",
            serde_json::json!({ "deviceId": id, "platform": "apns", "token": t1 }),
            "owner",
        ));
        assert_eq!(v["deviceId"], id.as_str());
        assert_eq!(v["platform"], "apns");
        assert_eq!(v["token"], t1.as_str());
        assert_eq!(v["username"], "owner", "attribution = the RESOLVED actor");
        let created = v["createdAt"].as_i64().expect("createdAt");
        assert!(created > 0);

        let t2 = tok("upsert-2");
        let v2 = ok_json(post_json(
            "/cli/push/register-device",
            serde_json::json!({ "deviceId": id, "platform": "fcm", "token": t2 }),
            "rosson",
        ));
        assert_eq!(v2["platform"], "fcm");
        assert_eq!(v2["token"], t2.as_str(), "token rotation lands via the upsert");
        assert_eq!(v2["username"], "rosson");
        assert_eq!(v2["createdAt"].as_i64(), Some(created), "created_at survives");

        let mine: Vec<_> = push_devices::list_devices()
            .expect("list")
            .into_iter()
            .filter(|d| d.device_id == id)
            .collect();
        assert_eq!(mine.len(), 1, "one row per deviceId");
        push_devices::remove_device(&id).expect("cleanup");
    }

    #[test]
    fn register_device_validation_is_loud() {
        for (body, why) in [
            (serde_json::json!({ "platform": "apns", "token": "t" }), "missing deviceId"),
            (serde_json::json!({ "deviceId": "d", "platform": "apns" }), "missing token"),
            (
                serde_json::json!({ "deviceId": "d", "platform": "webpush", "token": "t" }),
                "bad platform",
            ),
        ] {
            let resp = post_json("/cli/push/register-device", body, "owner");
            assert_eq!(resp.status, "400 Bad Request", "{why}: body={}", resp.body);
            let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
            assert_eq!(v["error"]["code"], "usage", "{why}");
        }
        // Malformed JSON is a usage error too, never a 500.
        let resp = dispatch_post("/cli/push/register-device", b"{nope", "owner");
        assert_eq!(resp.status, "400 Bad Request");
    }

    #[test]
    fn unregister_device_removes_and_tolerates_misses() {
        let id = did("unreg");
        ok_json(post_json(
            "/cli/push/register-device",
            serde_json::json!({ "deviceId": id, "platform": "fcm", "token": tok("unreg") }),
            "owner",
        ));
        let v = ok_json(post_json(
            "/cli/push/unregister-device",
            serde_json::json!({ "deviceId": id }),
            "owner",
        ));
        assert_eq!(v["removed"], true);
        let v = ok_json(post_json(
            "/cli/push/unregister-device",
            serde_json::json!({ "deviceId": id }),
            "owner",
        ));
        assert_eq!(v["removed"], false, "already-gone unregister is a fine no-op");
        assert!(
            !push_devices::list_devices().expect("list").iter().any(|d| d.device_id == id),
            "device gone"
        );

        let resp = post_json("/cli/push/unregister-device", serde_json::json!({}), "owner");
        assert_eq!(resp.status, "400 Bad Request", "missing deviceId is loud");
    }

    /// Recording PushTarget for the dispatch core.
    struct Recorder(Arc<Mutex<Vec<PushEvent>>>);

    impl PushTarget for Recorder {
        fn send(&self, event: &PushEvent) -> Result<(), k2_core::push::PushError> {
            self.0.lock().expect("recorder lock").push(event.clone());
            Ok(())
        }

        fn label(&self) -> String {
            "Recorder".to_string()
        }
    }

    /// The dormancy contract: no configured target → `dispatch_with`
    /// is a pure no-op (no thread, no send); a configured target gets
    /// the event on a detached thread.
    #[test]
    fn dispatch_with_is_dormant_without_a_target_and_sends_with_one() {
        assert!(
            dispatch_with(
                None,
                PushEvent::FeedbackCreated {
                    agent_name: "a".into(),
                    feedback_id: "f".into(),
                    assignees: vec![],
                },
            )
            .is_none(),
            "unconfigured push must be a silent no-op (DORMANT)"
        );

        let seen = Arc::new(Mutex::new(Vec::new()));
        let handle = dispatch_with(
            Some(Arc::new(Recorder(seen.clone()))),
            PushEvent::ProjectMessageCreated {
                project_name: "Apollo".into(),
                group_id: "pg-1".into(),
            },
        )
        .expect("configured target spawns a send");
        handle.join().expect("sender thread");
        let events = seen.lock().expect("recorder lock");
        assert_eq!(events.len(), 1);
        match &events[0] {
            PushEvent::ProjectMessageCreated { project_name, group_id } => {
                assert_eq!(project_name, "Apollo");
                assert_eq!(group_id, "pg-1");
            }
            other => panic!("wrong event dispatched: {other:?}"),
        }
    }

    /// The notify helpers shape content-free events (IDs + names
    /// only) — asserted through the same capture sink the route
    /// trigger tests use.
    #[test]
    fn notify_helpers_shape_content_free_events() {
        let fid = format!("fb-notify-{}", uuid::Uuid::new_v4());
        let gid = format!("pg-notify-{}", uuid::Uuid::new_v4());
        let mark = test_push_capture::mark();
        notify_feedback_created("cortana", &fid, &[]);
        notify_project_message_created("Apollo", &gid);
        let events = test_push_capture::since(mark);
        let mine: Vec<_> = events
            .iter()
            .filter(|e| match e {
                PushEvent::FeedbackCreated { feedback_id, .. } => feedback_id == &fid,
                PushEvent::ProjectMessageCreated { group_id, .. } => group_id == &gid,
                _ => false,
            })
            .collect();
        assert_eq!(mine.len(), 2, "both notifies captured: {events:?}");
        assert_eq!(mine[0].title(), "K2");
        assert_eq!(mine[0].body(), "cortana needs you on a ticket");
        assert_eq!(
            mine[0].data(),
            serde_json::json!({ "kind": "ticket", "feedbackId": fid })
        );
        assert_eq!(mine[1].body(), "New message in Apollo");
        assert_eq!(
            mine[1].data(),
            serde_json::json!({ "kind": "project", "groupId": gid })
        );
    }
}
