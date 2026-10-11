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
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::cli_response::CliResponse;
use k2_core::push::{K2Cloud, PushEvent, PushTarget};
#[cfg(not(test))]
use k2_core::push::budget;
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
        "/cli/push/unregister-device" => handle_unregister(body, username),
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
///
/// TP12: only the device's own person (its login, or a login that maps to
/// the same person key) or an Admin+ may remove it. Anyone else gets the
/// same `removed:false` as a miss — no oracle for device ids.
fn handle_unregister(body: &[u8], username: &str) -> CliResponse {
    let b: UnregisterBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return usage_error(format!("invalid JSON body: {e}")),
    };
    if b.device_id.trim().is_empty() {
        return usage_error("missing 'deviceId'");
    }
    let row = match push_devices::list_devices() {
        Ok(all) => all.into_iter().find(|d| d.device_id == b.device_id.trim()),
        Err(e) => return usage_error(e),
    };
    let Some(row) = row else {
        return CliResponse::ok_json(serde_json::json!({ "ok": true, "removed": false }).to_string());
    };
    let people = match k2_core::notify_prefs::People::load() {
        Ok(p) => p,
        Err(e) => return usage_error(e),
    };
    let is_admin = username == k2_core::notify_prefs::OWNER
        || k2_core::connect_users::role_for_user(username)
            .is_some_and(|r| r >= k2_core::connect_users::Role::Admin);
    let own = people.person_key(&row.username) == people.person_key(username);
    if !is_admin && !own {
        return CliResponse::ok_json(serde_json::json!({ "ok": true, "removed": false }).to_string());
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

/// How long a resolved gateway config is reused before it is read again
/// (settings JSON + `tunnel.json`, maybe the token vault). Short, so a
/// pairing or a settings change applies without a restart.
const CONFIG_TTL: std::time::Duration = std::time::Duration::from_secs(60);

/// The push gateway as configured NOW (TP6): a paired tunnel defaults to
/// `https://push.k2.dev` with per-daemon auth; the env/settings URL and
/// shared token still override and back it up. `None` = DORMANT.
/// Cached for [`CONFIG_TTL`].
// Unreachable under cfg(test): every send short-circuits into the capture
// sink before the configured target is consulted.
#[cfg_attr(test, allow(dead_code))]
pub(crate) fn current_cloud() -> Option<Arc<K2Cloud>> {
    static CACHE: Mutex<Option<(Instant, Option<Arc<K2Cloud>>)>> = Mutex::new(None);
    k2_core::push::set_instance_id(crate::boot_status::instance_id());
    let mut slot = CACHE.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((at, cloud)) = slot.as_ref() {
        if at.elapsed() < CONFIG_TTL {
            return cloud.clone();
        }
    }
    let cloud = K2Cloud::from_config().map(Arc::new);
    *slot = Some((Instant::now(), cloud.clone()));
    cloud
}

/// The resolved gateway config for status reporting (§5.6 `push`). `None`
/// under test (no gateway is ever configured there).
pub(crate) fn current_config() -> Option<k2_core::push::GatewayConfig> {
    #[cfg(test)]
    {
        None
    }
    #[cfg(not(test))]
    {
        current_cloud().map(|c| c.config().clone())
    }
}

/// The production air-gap probe: [`k2_core::airgap::enabled`], read on
/// EVERY send because air-gap can flip at runtime (TP5).
pub(crate) fn prod_airgap_on() -> bool {
    k2_core::airgap::enabled()
}

#[cfg(test)]
thread_local! {
    /// Test-only air-gap for THIS thread's sends. Flipping the real
    /// process-wide flag would silence every other test's pushes.
    static TEST_AIRGAP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Test-only: turn this thread's air-gap on or off.
#[cfg(test)]
pub(crate) fn set_test_airgap(on: bool) {
    TEST_AIRGAP.with(|c| c.set(on));
}

/// The air-gap check every push passes, all kinds (TP5).
pub(crate) fn airgap_blocks() -> bool {
    #[cfg(test)]
    {
        TEST_AIRGAP.with(|c| c.get())
    }
    #[cfg(not(test))]
    {
        prod_airgap_on()
    }
}

/// Log an air-gap drop once per boot (TP5), not once per push.
pub(crate) fn note_airgap_drop() {
    static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        k2_core::log_debug!("[push] air-gap is on: no push of any kind leaves this daemon");
    }
}

/// Log a budget drop at most once a minute (§6.3).
#[cfg_attr(test, allow(dead_code))]
pub(crate) fn note_budget_drop(class: k2_core::push::budget::BudgetClass) {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|p| p.into_inner());
    if last.is_none_or(|t| t.elapsed() >= std::time::Duration::from_secs(60)) {
        *last = Some(Instant::now());
        k2_core::log_debug!("[push] local {class:?} budget spent; dropping pushes (not queued)");
    }
}

/// The ticket/project send chokepoint: air-gap → (test: capture) →
/// the `Other` budget → a detached send. Never waits on the gateway.
fn dispatch_push(event: PushEvent) {
    if airgap_blocks() {
        note_airgap_drop();
        return;
    }
    // Test seam: unit tests capture the event instead of touching
    // config or spawning senders (no live network in tests).
    #[cfg(test)]
    {
        test_push_capture::capture(&event);
    }
    #[cfg(not(test))]
    {
        let Some(cloud) = current_cloud() else {
            return;
        };
        // Off the route thread: resolving people/devices reads files.
        std::thread::spawn(move || {
            for (principal, devices) in ticket_or_project_targets(&event) {
                // One dispatch per person (TP29), each taking one token
                // from the ticket/project bucket (never the Thread one).
                if !budget::global().try_take(budget::BudgetClass::Other, budget::OTHER_PER_HOUR, Instant::now()) {
                    note_budget_drop(budget::BudgetClass::Other);
                    return;
                }
                if airgap_blocks() {
                    note_airgap_drop();
                    return;
                }
                if let Err(e) = cloud.dispatch(&event, &principal, &devices) {
                    k2_core::log_debug!("[push] dispatch failed ({}): {e}", cloud.label());
                }
            }
        });
    }
}

/// Ticket/project recipients (§10): the event's targets (assignees, or
/// every device), minus removed/disabled logins and people whose server
/// bell is off, grouped by person.
#[cfg_attr(test, allow(dead_code))]
pub(crate) fn ticket_or_project_targets(
    event: &PushEvent,
) -> Vec<(String, Vec<k2_core::push_devices::PushDevice>)> {
    let devices = match event.target_usernames() {
        Some(users) => push_devices::list_devices_for_usernames(users),
        None => push_devices::list_devices(),
    }
    .unwrap_or_default();
    let people = match k2_core::notify_prefs::People::load() {
        Ok(p) => p,
        Err(e) => {
            k2_core::log_debug!("[push] people load failed: {e}");
            return Vec::new();
        }
    };
    let prefs = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if k2_core::notify_prefs::tables_exist(&conn) {
            k2_core::notify_prefs::PrefsSnapshot::load(&conn).unwrap_or_default()
        } else {
            k2_core::notify_prefs::PrefsSnapshot::default()
        }
    };
    group_by_person(&people, k2_core::notify_prefs::bell_allowed_devices(&people, &prefs, devices))
}

/// Group devices by their person key (sorted by person).
pub(crate) fn group_by_person(
    people: &k2_core::notify_prefs::People,
    devices: Vec<k2_core::push_devices::PushDevice>,
) -> Vec<(String, Vec<k2_core::push_devices::PushDevice>)> {
    let mut out: std::collections::BTreeMap<String, Vec<k2_core::push_devices::PushDevice>> =
        Default::default();
    for d in devices {
        if let Some(person) = people.device_person(&d.username) {
            out.entry(person).or_default().push(d);
        }
    }
    out.into_iter().collect()
}

/// Take one dispatch from the Thread bucket (§6.3): 30/h on the
/// per-daemon relay path, 5/h on the shared token. Dormant/test: always
/// granted (nothing leaves anyway).
pub(crate) fn take_thread_budget(now: Instant) -> bool {
    #[cfg(test)]
    {
        let _ = now;
        true
    }
    #[cfg(not(test))]
    {
        let Some(cloud) = current_cloud() else {
            return true;
        };
        let per_hour = k2_core::push::RelayPath::of(cloud.config(), unix_now()).thread_per_hour();
        budget::global().try_take(budget::BudgetClass::Thread, per_hour, now)
    }
}

/// Send one coalesced Thread push (one person's devices) — air-gap is
/// checked again here, then a detached send. Test: the capture sink.
pub(crate) fn deliver_thread(event: PushEvent) {
    if airgap_blocks() {
        note_airgap_drop();
        return;
    }
    #[cfg(test)]
    {
        test_push_capture::capture(&event);
    }
    #[cfg(not(test))]
    {
        let Some(cloud) = current_cloud() else {
            return;
        };
        let PushEvent::ThreadPost(t) = &event else {
            return;
        };
        let principal = t.principal.clone();
        let logins = t.device_usernames.clone();
        std::thread::spawn(move || {
            let devices = push_devices::list_devices_for_usernames(&logins).unwrap_or_default();
            if let Err(e) = cloud.dispatch(&event, &principal, &devices) {
                k2_core::log_debug!("[push] thread dispatch failed ({}): {e}", cloud.label());
            }
        });
    }
}

#[cfg_attr(test, allow(dead_code))]
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg_attr(not(test), allow(dead_code))]
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

    /// TP12: a Member cannot unregister someone else's phone (and learns
    /// nothing: the answer is the same as a miss); the owner of the device
    /// and an Admin can.
    #[test]
    fn unregister_is_limited_to_the_devices_person_or_an_admin() {
        let _home = crate::test_support::TempHome::new();
        k2_core::connect_users::add_user("tp12mo", "password123").expect("mo");
        k2_core::connect_users::add_user("tp12gina", "password123").expect("gina");
        k2_core::connect_users::set_role("tp12gina", k2_core::connect_users::Role::Admin).expect("admin");
        let id = did("tp12");
        ok_json(post_json(
            "/cli/push/register-device",
            serde_json::json!({ "deviceId": id, "platform": "apns", "token": tok("tp12") }),
            "owner",
        ));
        let v = ok_json(post_json("/cli/push/unregister-device", serde_json::json!({ "deviceId": id }), "tp12mo"));
        assert_eq!(v["removed"], false, "a Member can't remove the owner's phone");
        assert!(push_devices::list_devices().expect("list").iter().any(|d| d.device_id == id), "still there");
        let v = ok_json(post_json("/cli/push/unregister-device", serde_json::json!({ "deviceId": id }), "tp12gina"));
        assert_eq!(v["removed"], true, "an Admin can");
        // The device's own login can.
        let id2 = did("tp12-own");
        ok_json(post_json(
            "/cli/push/register-device",
            serde_json::json!({ "deviceId": id2, "platform": "fcm", "token": tok("tp12-own") }),
            "tp12mo",
        ));
        let v = ok_json(post_json("/cli/push/unregister-device", serde_json::json!({ "deviceId": id2 }), "tp12mo"));
        assert_eq!(v["removed"], true);
    }

    /// TP11: removing a login drops its phones and its notification rows.
    #[test]
    fn removing_a_user_drops_their_devices() {
        let _home = crate::test_support::TempHome::new();
        k2_core::connect_users::add_user("tp11gone", "password123").expect("add");
        let id = did("tp11");
        ok_json(post_json(
            "/cli/push/register-device",
            serde_json::json!({ "deviceId": id, "platform": "apns", "token": tok("tp11") }),
            "tp11gone",
        ));
        k2_core::connect_users::remove_user("tp11gone").expect("remove");
        assert!(
            !push_devices::list_devices().expect("list").iter().any(|d| d.device_id == id),
            "a removed login's phone must not keep getting pushes"
        );
    }

    /// TP5: air-gap is checked on EVERY send, for every kind — flipped
    /// between two sends, the second is dropped, and turning it off lets
    /// the next one through again.
    #[test]
    fn airgap_is_checked_on_every_send() {
        let fid = format!("fb-airgap-{}", uuid::Uuid::new_v4());
        let ours = |events: Vec<PushEvent>| {
            events
                .into_iter()
                .filter(|e| matches!(e, PushEvent::FeedbackCommented { feedback_id, .. } if feedback_id == &fid))
                .count()
        };
        let mark = test_push_capture::mark();
        notify_feedback_commented(&fid, &[]);
        assert_eq!(ours(test_push_capture::since(mark)), 1, "air-gap off: sent");

        set_test_airgap(true);
        let mark = test_push_capture::mark();
        notify_feedback_commented(&fid, &[]);
        notify_project_message_created("Apollo", &fid);
        set_test_airgap(false);
        let after: Vec<_> = test_push_capture::since(mark)
            .into_iter()
            .filter(|e| match e {
                PushEvent::FeedbackCommented { feedback_id, .. } => feedback_id == &fid,
                PushEvent::ProjectMessageCreated { group_id, .. } => group_id == &fid,
                _ => false,
            })
            .collect();
        assert!(after.is_empty(), "air-gap on: zero sends of any kind, got {after:?}");

        let mark = test_push_capture::mark();
        notify_feedback_commented(&fid, &[]);
        assert_eq!(ours(test_push_capture::since(mark)), 1, "air-gap off again: sent");
    }

    /// The production probe is the real air-gap flag, read live: the
    /// runtime setting mirror (`airgap::set_setting_enabled`) and the env
    /// both flip it without a restart.
    #[test]
    fn prod_airgap_probe_reads_the_live_flag() {
        let _env = k2_core::test_env::EnvVar::remove("K2_AIRGAP");
        let _home = crate::test_support::TempHome::new();
        k2_core::airgap::set_setting_enabled(false);
        assert!(!prod_airgap_on() || k2_core::airgap::baked());
        k2_core::airgap::set_setting_enabled(true);
        assert!(prod_airgap_on(), "runtime setting flip is seen on the next send");
        k2_core::airgap::set_setting_enabled(false);
        let _on = k2_core::test_env::EnvVar::set("K2_AIRGAP", "1");
        assert!(prod_airgap_on(), "env flip is seen too");
    }

    /// §10 / Q5: the server bell silences ticket and project pushes too;
    /// disabled logins get none; each person is its own dispatch (TP29).
    #[test]
    fn ticket_and_project_targets_follow_the_bell_per_person() {
        let _home = crate::test_support::TempHome::new();
        let _db = k2_core::db::scoped_for_test();
        for (u, role) in [("bellmo", None), ("bellgina", None), ("belldis", None), ("bellz", Some(k2_core::connect_users::Role::Admin))] {
            k2_core::connect_users::add_user(u, "password123").expect("user");
            if let Some(r) = role {
                k2_core::connect_users::set_role(u, r).expect("role");
            }
        }
        k2_core::connect_users::set_disabled("belldis", true).expect("disable");
        k2_core::app_settings::update(serde_json::json!({ "ownerAccounts": ["bellz"] })).expect("link");
        for (id, user) in [("b1", "owner"), ("b2", "bellz"), ("b3", "bellmo"), ("b4", "bellgina"), ("b5", "belldis")] {
            push_devices::upsert_device(id, "apns", &format!("tok-{id}"), user).expect("device");
        }
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::notify_prefs::set_server(&conn, "bellgina", false, "bellgina").expect("bell off");
        }
        let groups = ticket_or_project_targets(&PushEvent::ProjectMessageCreated {
            project_name: "Apollo".into(),
            group_id: "g".into(),
        });
        let shape: Vec<(String, Vec<String>)> = groups
            .into_iter()
            .map(|(p, d)| (p, d.into_iter().map(|x| x.username).collect()))
            .collect();
        assert_eq!(
            shape,
            vec![
                ("bellmo".to_string(), vec!["bellmo".to_string()]),
                ("owner".to_string(), vec!["owner".to_string(), "bellz".to_string()]),
            ],
            "bell-off gina and disabled dis are skipped; the linked login is the owner"
        );
        // Assignee targeting still narrows, and still follows the bell.
        let groups = ticket_or_project_targets(&PushEvent::FeedbackCommented {
            feedback_id: "f".into(),
            assignees: vec!["bellgina".into(), "bellmo".into()],
        });
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].0, "bellmo");
    }
}
