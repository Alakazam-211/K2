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

/// `GET /cli/push/device?deviceId=<id>`: read one device's stored
/// choices back (0.46.1 previews). The dispatcher resolves the caller
/// from the token, like the POSTs.
pub const DEVICE_PATH: &str = "/cli/push/device";

/// Push-domain GET dispatch. The two mutations are POST-only — reached
/// via the GET chain they answer an explicit 405. [`DEVICE_PATH`] needs
/// the caller's login, which only the dispatcher's push arm resolves, so
/// this chain (no caller) refuses it. Returns `None` if the path isn't a
/// push-domain route.
pub fn dispatch(path: &str, _params: &HashMap<String, String>) -> Option<CliResponse> {
    match path {
        "/cli/push/register-device" | "/cli/push/unregister-device" => {
            Some(CliResponse::method_not_allowed())
        }
        DEVICE_PATH => Some(CliResponse {
            status: "403 Forbidden",
            content_type: "application/json",
            body: serde_json::json!({ "error": "a login token is required" }).to_string(),
        }),
        _ => None,
    }
}

/// Dispatch a `/cli/push/*` POST body to its handler. `username` is
/// the daemon-resolved authed actor (`"owner"` | a connect-user name)
/// — resolved by the dispatcher arm from the session token, never
/// from the body. Unknown paths 404 (mirrors `dispatch_unit6_post`);
/// the GET-only [`DEVICE_PATH`] answers 405.
pub fn dispatch_post(path: &str, body: &[u8], username: &str) -> CliResponse {
    dispatch_post_for_session(path, body, username, None)
}

/// [`dispatch_post`] with the caller's Connect login session (0.46.3):
/// `session_sid` is `connect_users::session_sid(<caller token>)` for a
/// Connect session, `None` for the owner token. register-device links the
/// device to it, so the registration ends when that login ends (logout,
/// revoke, expiry). Resolved by the dispatcher, never from the body.
pub fn dispatch_post_for_session(
    path: &str,
    body: &[u8],
    username: &str,
    session_sid: Option<&str>,
) -> CliResponse {
    match path {
        "/cli/push/register-device" => handle_register(body, username, session_sid),
        "/cli/push/unregister-device" => handle_unregister(body, username),
        DEVICE_PATH => CliResponse::method_not_allowed(),
        _ => CliResponse::not_found(),
    }
}

/// The owner's server-wide preview switch (`pushMessagePreviews`, default
/// on). Off = no device gets message text, whatever it chose.
pub(crate) fn previews_allowed() -> bool {
    k2_core::app_settings::load().push_message_previews
}

/// A device row as the push routes answer it: the stored row (camelCase,
/// `previews` = this device's own choice) plus `previewsAllowed` (the
/// server master switch) and `ok: true`. The phone greys out its toggle
/// ("Turned off by the server owner") when `previewsAllowed` is false.
fn device_reply(device: &push_devices::PushDevice) -> CliResponse {
    let mut v = serde_json::to_value(device).unwrap_or_else(|_| serde_json::json!({}));
    if let Some(map) = v.as_object_mut() {
        map.insert("ok".to_string(), serde_json::json!(true));
        map.insert("previewsAllowed".to_string(), serde_json::json!(previews_allowed()));
        // 0.46.1: pushes carry `badgeKey` instead of the person's name; the
        // phone matches it to this login (a phone may hold two accounts on
        // one host). Stable for the person on this server.
        map.insert(
            "badgeKey".to_string(),
            serde_json::json!(k2_core::read_state::badge_key_for_shared(&device.username)),
        );
        // 0.46.3: true when the registration is tied to a Connect login
        // (it ends when that login signs out, is revoked or expires);
        // false for the owner token or a row from before 0.46.3.
        let linked = push_devices::device_session_sid(&device.device_id)
            .map(|s| s.is_some())
            .unwrap_or(false);
        map.insert("sessionLinked".to_string(), serde_json::json!(linked));
    }
    CliResponse::ok_json(v.to_string())
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
    /// Thread push message previews on this phone (0.46.1). Absent =
    /// keep the stored value (an older phone); a new device then reads
    /// as on. Anything but a JSON bool is a usage error.
    previews: Option<bool>,
}

/// Handler for `POST /cli/push/register-device` — upsert keyed by
/// deviceId (re-sent every app launch; rotation lands for free). The
/// reply is the stored row plus `previewsAllowed` ([`device_reply`]).
///
/// 0.46.3: the row is linked to the caller's login session
/// (`session_sid`); when that login ends the row goes. Every register
/// re-links, so the phone's per-launch register follows a new login.
fn handle_register(body: &[u8], username: &str, session_sid: Option<&str>) -> CliResponse {
    let b: RegisterBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return usage_error(format!("invalid JSON body: {e}")),
    };
    match push_devices::upsert_device_for_session(
        &b.device_id,
        &b.platform,
        &b.token,
        username,
        b.previews,
        session_sid,
    ) {
        Ok(device) => device_reply(&device),
        Err(e) => usage_error(e),
    }
}

/// Handler for `GET /cli/push/device?deviceId=<id>`: the device's stored
/// row (`previews`) and the server's `previewsAllowed`. Same rule as
/// unregister (TP12): only the device's own person or an Admin+ sees it;
/// anyone else gets the same 404 as an unknown id (no oracle).
pub fn handle_device_get(params: &HashMap<String, String>, username: &str) -> CliResponse {
    let id = params.get("deviceId").or_else(|| params.get("device_id")).map(|s| s.trim()).unwrap_or("");
    if id.is_empty() {
        return usage_error("missing 'deviceId'");
    }
    let row = match push_devices::get_device(id) {
        Ok(r) => r,
        Err(e) => return usage_error(e),
    };
    let not_found = || CliResponse {
        status: "404 Not Found",
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": { "code": "not_found", "hint": "no such device for this login" },
        })
        .to_string(),
    };
    let Some(row) = row else {
        return not_found();
    };
    if !may_manage_device(&row, username) {
        return not_found();
    }
    device_reply(&row)
}

/// TP12: the device's own person (its login, or a login of the same
/// person) or an Admin+.
fn may_manage_device(row: &push_devices::PushDevice, username: &str) -> bool {
    let is_admin = username == k2_core::notify_prefs::OWNER
        || k2_core::connect_users::role_for_user(username)
            .is_some_and(|r| r >= k2_core::connect_users::Role::Admin);
    if is_admin {
        return true;
    }
    match k2_core::notify_prefs::People::load() {
        Ok(people) => people.person_key(&row.username) == people.person_key(username),
        Err(_) => row.username == username,
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

/// `project-group:message-created` → one mobile push per PERSON
/// (thread-read-v1, 0.46.1), called next to the
/// `ProjectGroupMessageCreated` emit in `project_group_routes`:
/// - never to the author's own person (`author_person`: the poster's
///   person key, `None` for an agent's line). Before 0.46.1 only the owner
///   was skipped, so a Connect user's own post buzzed their own phone;
/// - never to a person who muted the chat;
/// - only to people whose server bell is on, removed/disabled logins never;
/// - each carries that person's `badgeKey` (never their name) and `unreadTotal` /
///   `waitingTotal` (strings). Content-free: project name + group id.
pub fn notify_project_message_created(project_name: &str, group_id: &str, author_person: Option<&str>) {
    if airgap_blocks() {
        note_airgap_drop();
        return;
    }
    for (person, logins, totals) in project_recipients(group_id, author_person) {
        dispatch_push(PushEvent::ProjectPost(k2_core::push::ProjectPush {
            project_name: project_name.to_string(),
            group_id: group_id.to_string(),
            principal: person,
            device_usernames: logins,
            unread_total: totals.total.to_string(),
            waiting_total: totals.waiting_total.to_string(),
        }));
    }
}

/// Who gets a project chat push: (person, their device logins, their
/// totals now), bell-filtered, author and muted people left out.
pub(crate) fn project_recipients(
    group_id: &str,
    author_person: Option<&str>,
) -> Vec<(String, Vec<String>, k2_core::read_state::Totals)> {
    let people = match k2_core::notify_prefs::People::load() {
        Ok(p) => p,
        Err(e) => {
            k2_core::log_debug!("[push] people load failed: {e}");
            return Vec::new();
        }
    };
    let devices = push_devices::list_devices().unwrap_or_default();
    let db = k2_core::db::shared();
    let conn = db.lock();
    let prefs = if k2_core::notify_prefs::tables_exist(&conn) {
        k2_core::notify_prefs::PrefsSnapshot::load(&conn).unwrap_or_default()
    } else {
        k2_core::notify_prefs::PrefsSnapshot::default()
    };
    let read_ready = k2_core::read_state::tables_exist(&conn);
    let now = k2_core::read_state::now_secs();
    let mut out = Vec::new();
    for (person, group) in group_by_person(&people, k2_core::notify_prefs::bell_allowed_devices(&people, &prefs, devices)) {
        if author_person == Some(person.as_str()) {
            continue;
        }
        let mut totals = k2_core::read_state::Totals::default();
        if read_ready {
            match k2_core::read_state::is_muted(&conn, &person, k2_core::read_state::KIND_PROJECT, group_id, now) {
                Ok(true) => continue,
                Ok(false) => {}
                Err(e) => k2_core::log_debug!("[push] mute check for {person}: {e}"),
            }
            let p = k2_core::read_state::Person::of_login(&people, &person);
            match k2_core::read_state::totals(&conn, &p, now) {
                Ok(t) => totals = t,
                Err(e) => k2_core::log_debug!("[push] totals for {person}: {e}"),
            }
        }
        let mut logins: Vec<String> = group.into_iter().map(|d| d.username).collect();
        logins.sort();
        logins.dedup();
        out.push((person, logins, totals));
    }
    out
}

/// Take one call from the badge budget (thread-read-v1): its own bucket,
/// never the Thread or ticket/project one. Dormant/test: always granted.
pub(crate) fn take_badge_budget(now: Instant) -> bool {
    #[cfg(test)]
    {
        let _ = now;
        true
    }
    #[cfg(not(test))]
    {
        if current_cloud().is_none() {
            return true;
        }
        budget::global().try_take(budget::BudgetClass::Badge, budget::BADGE_PER_HOUR, now)
    }
}

/// True when pushes go out on the per-daemon relay path (a paired tunnel):
/// the only path the relay takes a silent badge push on.
pub(crate) fn per_daemon_path() -> bool {
    current_config().is_some_and(|c| c.tunnel.is_some())
}

/// Send one silent badge push to one person's devices. Air-gap is checked
/// again here; a relay refusal (429 included) is logged and dropped,
/// never retried: the next badge push carries the full totals.
pub(crate) fn deliver_badge(event: PushEvent) {
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
        let PushEvent::Badge(b) = &event else {
            return;
        };
        let principal = b.principal.clone();
        let logins = b.device_usernames.clone();
        std::thread::spawn(move || {
            let devices = push_devices::list_devices_for_usernames(&logins).unwrap_or_default();
            if let Err(e) = cloud.dispatch(&event, &k2_core::read_state::badge_key_for_shared(&principal), &devices) {
                k2_core::log_debug!("[push] badge dispatch dropped ({}): {e}", cloud.label());
            }
        });
    }
}

/// Sew chat S30: an agent's post waits in the chat approval queue → push to
/// the people who may approve. CONTENT-FREE: agent name + `out_…` id only,
/// never the body, channel or person it's for.
pub fn notify_chat_approval_requested(agent_name: &str, outbox_id: &str, approvers: &[String]) {
    dispatch_push(PushEvent::ChatApprovalRequested {
        agent_name: agent_name.to_string(),
        outbox_id: outbox_id.to_string(),
        approvers: approvers.to_vec(),
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
                if let Err(e) = cloud.dispatch(&event, &k2_core::read_state::badge_key_for_shared(&principal), &devices) {
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

/// Take `calls` gateway calls from the Thread bucket (§6.3), all or
/// nothing: 30/h on the per-daemon relay path, 5/h on the shared token.
/// A dispatch is one call, or two when one person's phones split on
/// previews. Dormant/test: always granted (nothing leaves anyway).
pub(crate) fn take_thread_budget(now: Instant, calls: usize) -> bool {
    #[cfg(test)]
    {
        let _ = (now, calls);
        true
    }
    #[cfg(not(test))]
    {
        let Some(cloud) = current_cloud() else {
            return true;
        };
        let per_hour = k2_core::push::RelayPath::of(cloud.config(), unix_now()).thread_per_hour();
        budget::global().try_take_n(budget::BudgetClass::Thread, per_hour, now, calls)
    }
}

/// Send one coalesced Thread push (one person's devices) — air-gap is
/// checked again here, then a detached send. Test: the capture sink.
/// `K2Cloud::dispatch` sends a preview only to devices with `previews` on
/// and the generic body to the rest; the coalescer already took one
/// budget call per payload variant from the same device rows (a phone
/// that re-registers in between can at most add one call).
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
            if let Err(e) = cloud.dispatch(&event, &k2_core::read_state::badge_key_for_shared(&principal), &devices) {
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

    /// 0.46.1 previews contract: a new device defaults to `previews:true`;
    /// `previews:false` stores off; a register WITHOUT the field (an older
    /// phone) keeps the stored value; every reply carries `previews` and
    /// `previewsAllowed`; a non-bool `previews` is a usage error.
    #[test]
    fn register_device_previews_default_on_and_absent_keeps_the_stored_value() {
        let _home = crate::test_support::TempHome::new();
        let id = did("previews");
        let reg = |body: serde_json::Value| ok_json(post_json("/cli/push/register-device", body, "owner"));
        let v = reg(serde_json::json!({ "deviceId": id, "platform": "apns", "token": tok("pv-1") }));
        assert_eq!(v["previews"], true, "new device: previews on by default: {v}");
        assert_eq!(v["previewsAllowed"], true, "the owner's switch defaults on: {v}");
        let v = reg(serde_json::json!({ "deviceId": id, "platform": "apns", "token": tok("pv-2"), "previews": false }));
        assert_eq!(v["previews"], false);
        let v = reg(serde_json::json!({ "deviceId": id, "platform": "apns", "token": tok("pv-3") }));
        assert_eq!(v["previews"], false, "absent keeps the stored off");
        assert!(!push_devices::get_device(&id).expect("get").expect("row").previews);
        let v = reg(serde_json::json!({ "deviceId": id, "platform": "apns", "token": tok("pv-4"), "previews": true }));
        assert_eq!(v["previews"], true);

        // The owner's master switch shows up as previewsAllowed:false; the
        // device keeps its own choice.
        k2_core::app_settings::update(serde_json::json!({ "pushMessagePreviews": false })).expect("switch off");
        let v = reg(serde_json::json!({ "deviceId": id, "platform": "apns", "token": tok("pv-5") }));
        assert_eq!((v["previews"].clone(), v["previewsAllowed"].clone()), (serde_json::json!(true), serde_json::json!(false)));
        k2_core::app_settings::update(serde_json::json!({ "pushMessagePreviews": true })).expect("switch on");

        let resp = post_json(
            "/cli/push/register-device",
            serde_json::json!({ "deviceId": id, "platform": "apns", "token": tok("pv-6"), "previews": "no" }),
            "owner",
        );
        assert_eq!(resp.status, "400 Bad Request", "previews must be a bool: {}", resp.body);
        push_devices::remove_device(&id).expect("cleanup");
    }

    /// `GET /cli/push/device?deviceId=` reads the stored choice back: the
    /// device's own person and an Admin see it; another Member gets the
    /// same 404 as an unknown id. POST to it is 405; the caller-less GET
    /// chain refuses it.
    #[test]
    fn device_get_reads_previews_back_for_its_person_only() {
        let _home = crate::test_support::TempHome::new();
        k2_core::connect_users::add_user("pvmo", "password123").expect("mo");
        k2_core::connect_users::add_user("pvgina", "password123").expect("gina");
        k2_core::connect_users::set_role("pvgina", k2_core::connect_users::Role::Admin).expect("admin");
        let id = did("get");
        ok_json(post_json(
            "/cli/push/register-device",
            serde_json::json!({ "deviceId": id, "platform": "fcm", "token": tok("get"), "previews": false }),
            "pvmo",
        ));
        let q = |who: &str| {
            let mut params = HashMap::new();
            params.insert("deviceId".to_string(), id.clone());
            handle_device_get(&params, who)
        };
        let v = ok_json(q("pvmo"));
        assert_eq!((v["deviceId"].as_str(), v["previews"].clone(), v["previewsAllowed"].clone()),
            (Some(id.as_str()), serde_json::json!(false), serde_json::json!(true)));
        assert_eq!(ok_json(q("pvgina"))["previews"], false, "an Admin can read it");
        assert_eq!(ok_json(q("owner"))["previews"], false, "the owner can");
        k2_core::connect_users::add_user("pvzed", "password123").expect("zed");
        let resp = q("pvzed");
        assert_eq!(resp.status, "404 Not Found", "another Member: no oracle: {}", resp.body);
        let unknown = {
            let mut params = HashMap::new();
            params.insert("deviceId".to_string(), did("nope"));
            handle_device_get(&params, "owner")
        };
        assert_eq!(unknown.status, "404 Not Found");
        assert_eq!(resp.body, unknown.body, "same answer as an unknown id");
        assert_eq!(handle_device_get(&HashMap::new(), "owner").status, "400 Bad Request", "deviceId required");
        assert_eq!(dispatch_post(DEVICE_PATH, b"{}", "owner").status, "405 Method Not Allowed");
        let resp = dispatch(DEVICE_PATH, &HashMap::new()).expect("claimed");
        assert_eq!(resp.status, "403 Forbidden", "no caller on the plain GET chain");
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
        let _home = crate::test_support::TempHome::new();
        let _db = k2_core::db::scoped_for_test();
        push_devices::upsert_device(&did("notify"), "apns", &tok("notify"), "owner").expect("owner phone");
        let fid = format!("fb-notify-{}", uuid::Uuid::new_v4());
        let gid = k2_core::project_groups::create_group(&format!("notify-{}", uuid::Uuid::new_v4())).expect("group").id;
        let mark = test_push_capture::mark();
        notify_feedback_created("cortana", &fid, &[]);
        notify_project_message_created("Apollo", &gid, None);
        let events = test_push_capture::since(mark);
        let mine: Vec<_> = events
            .iter()
            .filter(|e| match e {
                PushEvent::FeedbackCreated { feedback_id, .. } => feedback_id == &fid,
                PushEvent::ProjectPost(p) => p.group_id == gid,
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
            serde_json::json!({ "kind": "project", "groupId": gid, "unreadTotal": "0", "waitingTotal": "0" }),
            "content-free: ids and the person's totals only"
        );
    }

    /// thread-read-v1: a project chat push is per person; the author's own
    /// person never gets it (a Connect user's own post used to buzz their
    /// own phone), a person who muted the chat never gets it, and each
    /// push carries that person's totals.
    #[test]
    fn project_pushes_skip_the_author_and_muted_people_and_carry_totals() {
        let _home = crate::test_support::TempHome::new();
        let _db = k2_core::db::scoped_for_test();
        k2_core::connect_users::add_user("prjalice", "password123").expect("alice");
        k2_core::connect_users::add_user("prjbob", "password123").expect("bob");
        for (id, user) in [("pa1", "owner"), ("pa2", "prjalice"), ("pa3", "prjbob")] {
            push_devices::upsert_device(id, "apns", &format!("tok-{id}"), user).expect("device");
        }
        let gid = k2_core::project_groups::create_group(&format!("selfpush-{}", uuid::Uuid::new_v4())).expect("group").id;
        let m = k2_core::project_groups::post_message_as(&gid, "prjalice", "person", "hi team").expect("post");
        let principals = |events: &[PushEvent]| -> Vec<(String, String)> {
            let mut v: Vec<(String, String)> = events
                .iter()
                .filter_map(|e| match e {
                    PushEvent::ProjectPost(p) if p.group_id == gid => Some((p.principal.clone(), p.unread_total.clone())),
                    _ => None,
                })
                .collect();
            v.sort();
            v
        };
        let mark = test_push_capture::mark();
        notify_project_message_created("Apollo", &gid, Some("prjalice"));
        assert_eq!(
            principals(&test_push_capture::since(mark)),
            vec![("owner".to_string(), "1".to_string()), ("prjbob".to_string(), "1".to_string())],
            "alice never gets her own post; the others have it unread (the chat is newer than them)"
        );
        assert_eq!(m.seq, 1);
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::read_state::set_mute(&conn, "owner", k2_core::read_state::KIND_PROJECT, &gid, true, None).expect("mute");
        }
        let mark = test_push_capture::mark();
        notify_project_message_created("Apollo", &gid, None);
        assert_eq!(
            principals(&test_push_capture::since(mark)),
            vec![("prjalice".to_string(), "0".to_string()), ("prjbob".to_string(), "1".to_string())],
            "a muted chat never pushes; an agent's line goes to every other person"
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

    /// 0.46.3: register-device from a Connect login links the phone to
    /// that login (`sessionLinked: true`); `POST /cli/auth/logout` of that
    /// login drops it, while the same person's other login keeps its phone
    /// and the owner's phone (`sessionLinked: false`) is untouched.
    #[test]
    fn push_logout_ends_only_that_logins_registration() {
        let _home = crate::test_support::TempHome::new();
        k2_core::connect_users::add_user("pushlogout", "password123").expect("add");
        let a = k2_core::connect_users::create_session("pushlogout");
        let b = k2_core::connect_users::create_session("pushlogout");
        let (ida, idb, ido) = (did("lo-a"), did("lo-b"), did("lo-owner"));
        let reg = |id: &str, user: &str, sid: Option<String>| {
            ok_json(dispatch_post_for_session(
                "/cli/push/register-device",
                serde_json::json!({ "deviceId": id, "platform": "apns", "token": tok(id) }).to_string().as_bytes(),
                user,
                sid.as_deref(),
            ))
        };
        let v = reg(&ida, "pushlogout", Some(k2_core::connect_users::session_sid(&a)));
        assert_eq!(v["sessionLinked"], true, "a login's registration is linked: {v}");
        assert!(v.get("sessionSid").is_none() && !v.to_string().contains(&a), "never echoes the token or sid: {v}");
        reg(&idb, "pushlogout", Some(k2_core::connect_users::session_sid(&b)));
        let v = reg(&ido, "owner", None);
        assert_eq!(v["sessionLinked"], false, "the owner token has no login: {v}");

        let (resp, user) = crate::connect_users_routes::handle_logout_with_user(&a);
        assert_eq!(resp.status, "200 OK");
        assert_eq!(user.as_deref(), Some("pushlogout"));
        let ids: Vec<String> = push_devices::list_devices().expect("list").into_iter().map(|d| d.device_id).collect();
        assert!(!ids.contains(&ida), "the logged-out phone stops getting pushes");
        assert!(ids.contains(&idb), "the same person's other login keeps its phone");
        assert!(ids.contains(&ido), "the owner's phone is untouched");
        remove_device(&idb);
        remove_device(&ido);
    }

    fn remove_device(id: &str) {
        push_devices::remove_device(id).expect("cleanup");
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
        notify_project_message_created("Apollo", &fid, None);
        set_test_airgap(false);
        let after: Vec<_> = test_push_capture::since(mark)
            .into_iter()
            .filter(|e| match e {
                PushEvent::FeedbackCommented { feedback_id, .. } => feedback_id == &fid,
                PushEvent::ProjectMessageCreated { group_id, .. } => group_id == &fid,
                PushEvent::ProjectPost(p) => p.group_id == fid,
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
