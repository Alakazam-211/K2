//! `/cli/zen/*` (prd-zen-mode-v1 Z15, vs-live Z59/Z62; prd-zen-gardens-v1
//! G12–G16, vs-live G41/G45).
//!
//! `~/.k2/zen/` belongs to the OS user of this computer, so every route
//! answers ONLY the local daemon's owner token (Z15a): a Connect login onto
//! this daemon (any role), an app pass or an agent passport gets 403
//! `zen_local_only`. The policy rows are Member so the source-walk test
//! passes and `role_gate` lets the request reach this check; the check
//! itself lives here (Z59), after the dispatcher consumed the request.
//!
//! Gardens (G13): `setup` is the only route that creates the folder; the
//! `garden/*` routes are the only writers of `gardens.json`. Every change
//! to the list, a page, a theme or the active theme is announced with ONE
//! payload-free `zen_changed` (G15): the fingerprint covers the list.
//!
//! Nothing reads or writes `grants.json` (Z19). Custom widgets need no
//! grants (Rosson 2026-10-08: "If the widget exists, it should be able to
//! interact with agents"): a widget in your own Garden runs with every
//! Garden-safe cap it asks for (`k2_core::zen::widget_access`). The one
//! state left is the runaway guard's pause, held here in memory:
//! `widget/pause` (taking power away) is allowed to anything Zen accepts;
//! `widget/resume` is a person's click and takes ONLY the owner token (a
//! passport, a Connect login, an API key or an app pass gets 403
//! `owner_only`). Both write an audit line.
//! `.history/` is written only as a side effect of the clean-save snapshot,
//! `reset`, `garden/template` and `garden/delete`.
//!
//! Garden Lock (prd-garden-lock-and-sync-v1): `garden/lock` takes ONLY the
//! owner token (agents can neither lock nor unlock). A session passport
//! (an agent, or a K2 terminal) may make the Garden writes in
//! [`AGENT_GARDEN_WRITES`]; one that names a locked Garden is 423
//! `garden_locked`, with an audit line. The person's app (owner token, no
//! passport) still renames, rethemes, resets and deletes a locked Garden.
//! An edit to a locked Garden's file is put back on the next refresh
//! (`ZenFiles::enforce_locks`), audited as `zen.garden.lock_refused`.
//!
//! Agents run Gardens end to end (0.46.0, 0181): `garden/put`,
//! `widget/put` and the proposal queue (`proposals*`) are
//! [`crate::zen_agent_routes`]'s. Any local passport reaches them (an
//! agent without "Manage Gardens and data sources" proposes; with it, it
//! writes); a put naming a locked Garden is 423 like every agent Garden
//! write. With the switch a local passport may also make a Garden
//! (`garden/new`).

use std::collections::HashMap;
use std::sync::Mutex;

use serde::Deserialize;
use serde_json::{json, Value as J};

use k2_core::garden_data::authors::{self, CallerKind, StampOutcome, StampSource};
use k2_core::garden_data::MadeBy;
use k2_core::zen::widget_access::{Pause, PauseReason, WidgetPauses};
use k2_core::zen::widget_store::WidgetState;
use k2_core::zen::store::NOT_SET_UP;
use k2_core::zen::{ZenError, ZenFile, ZenFiles};

use crate::cli_response::CliResponse;
use crate::sidecar_routes::Caller;

/// Request bodies over this are refused with 413 (Z15). Room for one
/// widget code file (256 KiB, `agent_edit::MAX_PUT_BYTES`) in a JSON put.
pub const MAX_BODY: usize = 320 * 1024;

/// The POST rows (G45, UW35). A GET on any of them is 405.
/// `/cli/zen/lib/fetch` is `zen_lib_routes`'s (B3, UWB15).
pub const POST_ROUTES: &[&str] = &[
    "/cli/zen/garden/check",
    "/cli/zen/garden/delete",
    "/cli/zen/garden/lock",
    "/cli/zen/garden/new",
    "/cli/zen/garden/put",
    "/cli/zen/garden/rename",
    "/cli/zen/garden/reorder",
    "/cli/zen/garden/sync",
    "/cli/zen/garden/template",
    "/cli/zen/lib/fetch",
    "/cli/zen/news/seen",
    "/cli/zen/proposals/approve",
    "/cli/zen/proposals/reject",
    "/cli/zen/reload",
    "/cli/zen/reset",
    "/cli/zen/setup",
    "/cli/zen/theme/new",
    "/cli/zen/theme/next",
    "/cli/zen/theme/prev",
    "/cli/zen/theme/set",
    "/cli/zen/widget/check",
    "/cli/zen/widget/claim",
    "/cli/zen/widget/errors/clear",
    "/cli/zen/widget/folder/check",
    "/cli/zen/widget/folder/put",
    "/cli/zen/widget/new",
    "/cli/zen/widget/pause",
    "/cli/zen/widget/preview",
    "/cli/zen/widget/put",
    "/cli/zen/widget/resume",
    "/cli/zen/widget/run",
    "/cli/zen/widget/sign",
];

/// The GET rows (G45, UW35, UWB23). `/cli/zen/lib/file` is B3's.
pub const GET_ROUTES: &[&str] = &[
    "/cli/zen/doctor",
    "/cli/zen/gardens",
    "/cli/zen/get",
    "/cli/zen/history",
    "/cli/zen/inspect",
    "/cli/zen/lib/file",
    "/cli/zen/news",
    "/cli/zen/proposals",
    "/cli/zen/proposals/show",
    "/cli/zen/status",
    "/cli/zen/sync",
    "/cli/zen/templates",
    "/cli/zen/theme/list",
    "/cli/zen/validate",
    "/cli/zen/widget/bundle",
    "/cli/zen/widget/errors",
    "/cli/zen/widget/show",
    "/cli/zen/widgets",
];


/// Fingerprint of the last state announced with `zen_changed`.
static LAST_FINGERPRINT: Mutex<Option<String>> = Mutex::new(None);

/// The runaway guard's pauses, by Garden and placement (R6). In memory: a
/// daemon restart clears them, as a window reload clears the renderer's
/// post counters.
static PAUSES: Mutex<WidgetPauses> = Mutex::new(WidgetPauses::new());

fn resp(status: &'static str, body: J) -> CliResponse {
    CliResponse { status, content_type: "application/json", body: body.to_string() }
}

fn ok(body: J) -> CliResponse {
    resp("200 OK", body)
}

pub fn local_only() -> CliResponse {
    resp(
        "403 Forbidden",
        json!({
            "error": "zen_local_only",
            "message": "Zen belongs to the person on this computer. Only this computer's own K2 can read or change it.",
        }),
    )
}

pub fn too_large() -> CliResponse {
    resp(
        "413 Payload Too Large",
        json!({ "error": "body_too_large", "message": "Zen request bodies must be under 320 KB." }),
    )
}

/// The body cap for one `/cli/zen/*` path: [`MAX_BODY`], or a whole staged
/// widget folder for the dev loop's folder routes (0.46.1 S3).
pub fn max_body(path: &str) -> usize {
    if crate::zen_agent_routes::FOLDER_ROUTES.contains(&path) {
        k2_core::zen::widget_folder::MAX_FOLDER_BODY
    } else {
        MAX_BODY
    }
}

/// [`too_large`] for `path`'s own cap.
pub fn too_large_for(path: &str) -> CliResponse {
    if crate::zen_agent_routes::FOLDER_ROUTES.contains(&path) {
        resp(
            "413 Payload Too Large",
            json!({ "error": "body_too_large", "message": "A staged widget folder must be under 6 MB (4 MB of files)." }),
        )
    } else {
        too_large()
    }
}

/// A `ZenError` as its JSON answer (shared with `zen_agent_routes`).
pub(crate) fn error_response(e: ZenError) -> CliResponse {
    err(e)
}

fn err(e: ZenError) -> CliResponse {
    let message = e.to_string();
    match e {
        ZenError::NotSetUp => resp(
            "404 Not Found",
            json!({ "ok": false, "error": "zen_not_set_up", "message": NOT_SET_UP }),
        ),
        ZenError::BadRequest(m) => resp("400 Bad Request", json!({ "ok": false, "error": "bad_request", "message": m })),
        ZenError::NotFound(m) => resp("404 Not Found", json!({ "ok": false, "error": "not_found", "message": m })),
        ZenError::UnknownTheme { name, known } => resp(
            "404 Not Found",
            json!({ "ok": false, "error": "unknown_theme", "theme": name, "themes": known, "message": message }),
        ),
        ZenError::UnknownGarden { garden, known } => resp(
            "404 Not Found",
            json!({ "ok": false, "error": "unknown_garden", "garden": garden, "gardens": known, "message": message }),
        ),
        ZenError::Conflict(m) => resp("409 Conflict", json!({ "ok": false, "error": "theme_exists", "message": m })),
        ZenError::GardenExists(m) => resp("409 Conflict", json!({ "ok": false, "error": "garden_exists", "message": m })),
        ZenError::LastGarden => resp("409 Conflict", json!({ "ok": false, "error": "last_garden", "message": message })),
        ZenError::GardenHasChanges { garden, keys } => resp(
            "409 Conflict",
            json!({ "ok": false, "error": "garden_has_changes", "garden": garden, "keys": keys, "message": message }),
        ),
        ZenError::UnknownWidget(m) => resp("404 Not Found", json!({ "ok": false, "error": "unknown_widget", "message": m })),
        ZenError::WidgetExists(m) => resp("409 Conflict", json!({ "ok": false, "error": "widget_exists", "message": m })),
        ZenError::WidgetBroken(m) => resp("409 Conflict", json!({ "ok": false, "error": "widget_broken", "message": m })),
        ZenError::WidgetChanged(m) => resp("409 Conflict", json!({ "ok": false, "error": "widget_changed", "message": m })),
        ZenError::OwnerOnly(m) => resp("403 Forbidden", json!({ "ok": false, "error": "owner_only", "message": m })),
        ZenError::GardenLocked { garden, name } => resp(
            "423 Locked",
            json!({ "ok": false, "error": "garden_locked", "garden": garden, "name": name, "message": message }),
        ),
        ZenError::Io(m) => resp("500 Internal Server Error", json!({ "ok": false, "error": "io", "message": m })),
    }
}

fn files() -> ZenFiles {
    ZenFiles::local()
}

/// The runaway pauses now, read once per request.
fn pauses() -> WidgetPauses {
    PAUSES.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Is this placement paused (any reason)? Garden data's widget door
/// refuses data writes while it is (contract-garden-data-v1 §7.1 step 5).
pub fn placement_pause(garden: &str, placement: &str) -> Option<Pause> {
    PAUSES.lock().unwrap_or_else(|e| e.into_inner()).get(garden, placement).cloned()
}

/// Garden data's runaway guard tripped for a placement (`budget::runaway`,
/// PRD §6.3): pause it (`PauseReason::Writes`) in every window until the
/// person's Resume, with one `zen_changed`. `garden` is the Garden id.
pub fn pause_for_writes(garden: &str, placement: &str) {
    PAUSES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(garden, placement, Pause { at: now(), reason: PauseReason::Writes });
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        "zen.data.pause",
        "daemon",
        format!("{garden}/{placement} reason=writes"),
        "local",
        "-",
        "cli",
    ));
    if let Err(e) = refresh_and_emit() {
        k2_core::log_debug!("[daemon/zen] pause_for_writes refresh: {e}");
    }
}

/// Re-read the folder (snapshotting clean new content) and emit ONE
/// `zen_changed` when the live state moved. Serialised: the watcher, the
/// routes and the CLI all funnel here, so each effective change is
/// announced once whoever sees it first. Returns whether it moved. The
/// fingerprint covers widget folders and runaway pauses (UW12).
pub fn refresh_and_emit() -> Result<bool, ZenError> {
    let mut last = LAST_FINGERPRINT.lock().unwrap_or_else(|e| e.into_inner());
    let f = files();
    let (fp, refused) = f.refresh_with_report(&pauses())?;
    audit_lock_refusals(&refused);
    let changed = last.as_deref() != Some(fp.as_str());
    if changed {
        let first = last.is_none();
        *last = Some(fp);
        // The first fingerprint after boot is a baseline, not a change.
        if !first {
            let _ = crate::session_events::emit(crate::session_events::SessionEvent::ZenChanged {});
        }
        return Ok(!first);
    }
    Ok(false)
}

/// Record the current state as the baseline without announcing it (boot,
/// watcher start). A no-op when Zen isn't set up.
pub fn prime() {
    let mut last = LAST_FINGERPRINT.lock().unwrap_or_else(|e| e.into_inner());
    if last.is_none() {
        let f = files();
        if let Ok((fp, refused)) = f.refresh_with_report(&pauses()) {
            audit_lock_refusals(&refused);
            *last = Some(fp);
        }
    }
}

/// One audit line per edit to a locked Garden's file that `refresh` put
/// back (prd-garden-lock-and-sync-v1 §5.4). The daemon can't tell who
/// saved the file, so the actor is `file_edit`.
fn audit_lock_refusals(refused: &[k2_core::zen::lock::LockRefusal]) {
    for r in refused {
        k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
            "zen.garden.lock_refused",
            "file_edit",
            format!(
                "{} ({}): edit put back; kept at {}",
                r.garden,
                r.name,
                r.kept.as_deref().unwrap_or("- (the file was deleted)")
            ),
            "local",
            "-",
            "cli",
        ));
    }
}

fn param<'a>(params: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    params.get(key).map(|s| s.trim()).filter(|s| !s.is_empty())
}

/// A `home=` query is the pre-Gardens shape: refuse it loudly so a stale
/// client can't silently read the first Garden.
fn refuse_home(params: &HashMap<String, String>) -> Result<(), ZenError> {
    if params.contains_key("home") {
        return Err(ZenError::BadRequest("Zen pages are Gardens now: pass garden=<id or name>, not home=".into()));
    }
    Ok(())
}

/// `file=`, `garden=` (id or name) or `theme=` → the file they name.
/// `deleted` lets `garden=` name a Garden that only lives in history.
fn target(
    f: &ZenFiles,
    file: Option<&str>,
    garden: Option<&str>,
    theme: Option<&str>,
    deleted: bool,
) -> Result<Option<ZenFile>, ZenError> {
    match (file, garden, theme) {
        (Some(file), None, None) => ZenFile::parse(file).map(Some),
        (None, Some(garden), None) => f.garden_file(garden, deleted).map(Some),
        (None, None, Some(theme)) => {
            if !k2_core::zen::valid_theme_name(theme) {
                return Err(ZenError::BadRequest(format!("'{theme}' is not a theme name")));
            }
            Ok(Some(ZenFile::Theme(theme.to_string())))
        }
        (None, None, None) => Ok(None),
        _ => Err(ZenError::BadRequest("pass one of file, garden or theme".into())),
    }
}

fn body_json(body: &[u8]) -> Result<J, ZenError> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(json!({}));
    }
    serde_json::from_slice(body).map_err(|e| ZenError::BadRequest(format!("invalid JSON body: {e}")))
}

fn parse_body<T: serde::de::DeserializeOwned>(body: &[u8], shape: &str) -> Result<T, ZenError> {
    serde_json::from_value(body_json(body)?).map_err(|e| ZenError::BadRequest(format!("{shape}: {e}")))
}

fn nonempty(s: &Option<String>) -> Option<String> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

fn set_up(f: &ZenFiles) -> Result<(), ZenError> {
    if f.is_set_up() {
        Ok(())
    } else {
        Err(ZenError::NotSetUp)
    }
}

fn handle_get(params: &HashMap<String, String>) -> Result<J, ZenError> {
    refuse_home(params)?;
    let f = files();
    if !f.is_set_up() {
        return Err(ZenError::NotSetUp);
    }
    // `get` serves the live state; refresh first so a save the watcher
    // hasn't debounced yet is already live (and announced once).
    refresh_and_emit()?;
    crate::zen_sync_routes::get(&f, param(params, "garden"), param(params, "preview"), &pauses())
}

/// GET `/cli/zen/gardens` (G13): 200 with `setUp:false` and no Gardens when
/// Zen isn't set up (never 404), so the renderer can tell "not set up"
/// from "down".
fn handle_gardens() -> J {
    let f = files();
    let set_up = f.is_set_up();
    json!({
        "ok": true,
        "setUp": set_up,
        "gardens": if set_up { f.gardens_json() } else { Vec::new() },
    })
}

fn handle_status() -> J {
    let f = files();
    let set_up = f.is_set_up();
    json!({
        "ok": true,
        "setUp": set_up,
        "path": f.root().display().to_string(),
        "gardens": if set_up { f.gardens_json() } else { Vec::new() },
        "widgets": if set_up { f.widget_names() } else { Vec::new() },
        "watching": crate::zen_watch::is_running(),
        "message": if set_up { J::Null } else { json!(NOT_SET_UP) },
    })
}

fn handle_doctor() -> J {
    let f = files();
    let mut checks = f.doctor_checks();
    if f.exists() {
        checks.push(json!({
            "name": "watcher",
            "ok": crate::zen_watch::is_running(),
            "detail": if crate::zen_watch::is_running() {
                "watching zen.toml, gardens/, themes/ and widgets/ (250 ms debounce)"
            } else {
                "not running; edits apply on the next k2 zen reload or Zen open"
            },
        }));
    }
    let all_ok = checks.iter().all(|c| c["ok"].as_bool() == Some(true));
    json!({ "ok": all_ok, "setUp": f.is_set_up(), "path": f.root().display().to_string(), "checks": checks })
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct SetupBody {}

/// POST `/cli/zen/setup` (G14): the only route that creates the folder.
/// Idempotent. Starts the watcher and, the first time, seeds `k2-zen`.
fn handle_setup(body: &[u8]) -> Result<J, ZenError> {
    let _: SetupBody = parse_body(body, "setup takes {}")?;
    let f = files();
    let out = f.setup()?;
    // Z61: the watcher starts on setup when the folder was missing (or had
    // no Gardens) at boot. Z68: seed `k2-zen` into every workspace once.
    crate::zen_watch::start();
    if out.created_folder || out.created_default {
        let seeded = k2_core::workspace::skill_regen::seed_zen_skill_everywhere();
        k2_core::log_debug!("[daemon/zen] set up; k2-zen skill in {seeded} workspace(s)");
    }
    let changed = refresh_and_emit()?;
    Ok(json!({
        "ok": true,
        "path": f.root().display().to_string(),
        "createdFolder": out.created_folder,
        "createdDefault": out.created_default,
        // Per-Home Zen never shipped, so nothing is ever migrated.
        "migrated": J::Null,
        "gardens": f.gardens_json(),
        "changed": changed,
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewGardenBody {
    name: String,
    template: Option<String>,
    #[serde(rename = "seedHome")]
    seed_home: Option<String>,
    at: Option<usize>,
}

fn handle_garden_new(body: &[u8]) -> Result<J, ZenError> {
    let b: NewGardenBody = parse_body(body, "garden/new takes {name, template?, seedHome?, at?}")?;
    let f = files();
    let g = f.new_garden(&b.name, nonempty(&b.template).as_deref(), nonempty(&b.seed_home).as_deref(), b.at)?;
    let changed = refresh_and_emit()?;
    let (index, _) = f.garden(&g.id)?;
    let file = ZenFile::Garden(g.id.clone());
    Ok(json!({
        "ok": true,
        "garden": f.garden_json(index, &g, &f.read_active()),
        "file": file.label(),
        "path": f.path_of(&file).display().to_string(),
        "changed": changed,
    }))
}

// ── Custom widgets (prd-zen-user-widgets-v2 UW35, UWB9, UWB23) ──────────

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Who called, for the audit line.
fn caller_kind(caller: &Caller) -> &'static str {
    match caller {
        Caller::Owner => "owner_token",
        Caller::Login { .. } => "connect_login",
        Caller::Shell { .. } => "passport_shell",
        Caller::Agent(_) => "passport_agent",
        Caller::WrongCredential => "wrong_credential",
        Caller::Invalid => "none",
    }
}

fn audit(event: &str, caller: &Caller, outcome: String, ingress: &str) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        event,
        caller_kind(caller),
        outcome,
        ingress,
        "-",
        "cli",
    ));
}

/// A custom placement on a Garden's live page: `(garden id, widget)`.
fn find_placement(f: &ZenFiles, garden: &str, placement: &str) -> Result<(String, String), ZenError> {
    let (_, g) = f.garden(garden)?;
    let p = f
        .custom_placements()
        .into_iter()
        .find(|p| p.garden == g.id && p.placement == placement)
        .ok_or_else(|| ZenError::NotFound(format!("Garden '{}' has no custom widget '{placement}'", g.name)))?;
    Ok((g.id, p.widget))
}

/// POST `/cli/zen/widget/check` (0.46.1): may this placement's widget make
/// a call that needs `cap`?
pub const WIDGET_CHECK_ROUTE: &str = "/cli/zen/widget/check";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckBody {
    garden: String,
    placement: String,
    /// The folder name or `k2:<name>@<n>` the placement should hold.
    widget: String,
    /// The bundle hash the widget's frame runs.
    hash: String,
    /// A widget cap (`thread:post` today).
    cap: String,
}

/// POST `/cli/zen/widget/check` (0.46.1): the app asks this computer's
/// daemon, which holds the Garden, before a widget's Thread post or card
/// answer goes to an agent on ANOTHER server (that server can't see this
/// computer's placements). Same rule as the inline check on a post to an
/// agent here (`overlay_routes::widget_post_gate`,
/// `widget_access::check_placement_cap`): `{ok, caps}`, or 403
/// `widget_not_approved` / `widget_cap_missing`. Owner token only, like
/// every Zen route.
fn handle_widget_check(body: &[u8], caller: &Caller, ingress: &str) -> CliResponse {
    let b: CheckBody = match parse_body(body, "widget/check takes {garden, placement, widget, hash, cap}") {
        Ok(b) => b,
        Err(e) => return err(e),
    };
    if !k2_core::zen::USER_WIDGET_CAPS.contains(&b.cap.as_str()) {
        return err(ZenError::BadRequest(format!("'{}' isn't a widget cap", b.cap)));
    }
    let f = files();
    match k2_core::zen::widget_access::check_placement_cap(&f, &b.garden, &b.placement, &b.widget, &b.hash, &b.cap) {
        Ok(caps) => ok(json!({ "ok": true, "caps": caps })),
        Err(r) => {
            audit(
                "zen.widget.check_refused",
                caller,
                format!("{}/{} widget={} cap={} code={}", b.garden, b.placement, b.widget, b.cap, r.code),
                ingress,
            );
            resp("403 Forbidden", json!({ "ok": false, "error": r.code, "message": r.message }))
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PauseBody {
    garden: String,
    placement: String,
    /// Only `runaway` today (the renderer's guard, R6).
    reason: String,
}

/// POST `/cli/zen/widget/pause` (R6): the renderer's runaway guard tripped
/// (over 120 posts in 10 minutes, or 20 identical texts to one agent).
/// Posting pauses for that placement in every window until Resume. Taking
/// power away, so anything Zen accepts may call it. Pauses of placements
/// that are gone are dropped on the way.
fn handle_widget_pause(body: &[u8], caller: &Caller, ingress: &str) -> Result<J, ZenError> {
    let b: PauseBody = parse_body(body, "widget/pause takes {garden, placement, reason}")?;
    if b.reason != "runaway" {
        return Err(ZenError::BadRequest(format!("unknown reason '{}'; use runaway", b.reason)));
    }
    let f = files();
    set_up(&f)?;
    let (id, widget) = find_placement(&f, &b.garden, &b.placement)?;
    let pause = Pause { at: now(), reason: PauseReason::Runaway };
    let live = f.custom_placements();
    {
        let mut all = PAUSES.lock().unwrap_or_else(|e| e.into_inner());
        all.retain(|g, p| live.iter().any(|x| x.garden == g && x.placement == p));
        all.insert(&id, &b.placement, pause.clone());
    }
    audit("zen.widget.pause", caller, format!("{id}/{} widget={widget} reason=runaway", b.placement), ingress);
    let changed = refresh_and_emit()?;
    Ok(json!({ "ok": true, "paused": pause, "changed": changed }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunBody {
    widget: String,
    hash: String,
    event: String,
    message: Option<String>,
    stack: Option<String>,
    /// Garden v3 (GV49): `error` or `refusal`, and fixed fields that are
    /// all a `sources:read` widget's report keeps.
    kind: Option<String>,
    class: Option<String>,
    code: Option<String>,
    line: Option<u64>,
    /// 0.46.1 S3: the source address a refused `sources.*` call named
    /// (kept only when a placement's config names it).
    #[serde(default)]
    address: Option<String>,
}

/// Error reports kept per widget per minute; the frame host already stops a
/// widget at 20 a minute, this bounds the log against anything else.
const RUN_ERRORS_PER_MINUTE: usize = 30;
static RUN_ERRORS: Mutex<Option<HashMap<String, Vec<std::time::Instant>>>> = Mutex::new(None);

/// POST `/cli/zen/widget/run` (the renderer, decision-k2-runtime-v1 stage 0
/// (d)): a frame's `ready` or uncaught error, for the bundle hash it ran.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClearErrorsBody {
    widget: String,
    #[serde(default)]
    all: bool,
}

/// POST `widget/errors/clear {widget, all?}` (0.46.3): drop the runtime
/// errors validate no longer counts (or every one with `all`). The person
/// or any local agent (it only trims the widget's own runtime log); every
/// clear is audited.
fn handle_widget_errors_clear(body: &[u8], caller: &Caller, ingress: &str) -> Result<J, ZenError> {
    let b: ClearErrorsBody = parse_body(body, "widget/errors/clear takes {widget, all?}")?;
    let v = files().clear_widget_errors(&b.widget, b.all)?;
    audit(
        "zen.widget.errors_cleared",
        caller,
        format!("widgets/{}: cleared {} runtime error(s){}", v["widget"].as_str().unwrap_or(&b.widget), v["cleared"], if b.all { " (all)" } else { "" }),
        ingress,
    );
    Ok(v)
}

fn handle_widget_run(body: &[u8]) -> Result<J, ZenError> {
    let b: RunBody = parse_body(body, "widget/run takes {widget, hash, event, message?, stack?, kind?, class?, code?, line?, address?}")?;
    if b.event == "error" {
        let now = std::time::Instant::now();
        let mut g = RUN_ERRORS.lock().unwrap_or_else(|e| e.into_inner());
        let seen = g.get_or_insert_with(HashMap::new).entry(b.widget.clone()).or_default();
        seen.retain(|t| now.duration_since(*t) < std::time::Duration::from_secs(60));
        if seen.len() >= RUN_ERRORS_PER_MINUTE {
            return Ok(json!({ "ok": true, "logged": false, "dropped": true }));
        }
        seen.push(now);
    }
    let detail = k2_core::zen::widget_store::runs::RunDetail {
        message: b.message.as_deref(),
        stack: b.stack.as_deref(),
        kind: b.kind.as_deref(),
        class: b.class.as_deref(),
        code: b.code.as_deref(),
        line: b.line,
        address: b.address.as_deref(),
    };
    files().record_widget_run_with(&b.widget, &b.hash, &b.event, &detail)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResumeBody {
    garden: String,
    placement: String,
}

/// POST `/cli/zen/widget/resume` (R6): the person's one click on "Paused:
/// too many posts. Resume". Owner token only (checked in [`handle`]).
fn handle_widget_resume(body: &[u8], caller: &Caller, ingress: &str) -> Result<J, ZenError> {
    let b: ResumeBody = parse_body(body, "widget/resume takes {garden, placement}")?;
    let f = files();
    set_up(&f)?;
    let id = f.garden(&b.garden)?.1.id;
    let was = PAUSES.lock().unwrap_or_else(|e| e.into_inner()).remove(&id, &b.placement);
    audit("zen.widget.resume", caller, format!("{id}/{} resumed={was}", b.placement), ingress);
    let changed = if was { refresh_and_emit()? } else { false };
    Ok(json!({ "ok": true, "paused": J::Null, "resumed": was, "changed": changed }))
}

/// GET `/cli/zen/widget/bundle?widget=&garden=` (UW35): the live bundle
/// with a fresh nonce in every script. With `garden=` naming a locked
/// Garden, its frozen copy (prd-garden-lock-and-sync-v1).
fn handle_widget_bundle(params: &HashMap<String, String>) -> Result<J, ZenError> {
    let widget = param(params, "widget").ok_or_else(|| ZenError::BadRequest("widget/bundle needs widget=<name>".into()))?;
    let f = files();
    let st = f.widget_bundle_in(param(params, "garden"), widget)?;
    let b = st.bundle.as_ref().ok_or_else(|| ZenError::WidgetBroken(format!("{widget} has no good version")))?;
    let nonce = k2_core::zen::bundle::new_nonce();
    let html = b.render(&nonce);
    Ok(json!({
        "ok": true,
        "widget": widget,
        "hash": b.hash,
        "nonce": nonce,
        "bytes": html.len(),
        "state": st.state.as_str(),
        "html": html,
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewWidgetBody {
    name: String,
    from: Option<String>,
}

/// POST `/cli/zen/widget/new {name, from?}` (UW35): a user-file write,
/// like `garden/new`. Garden data (contract-garden-data-v1 §4.3, BG3): a
/// clean new folder's hash is stamped as made by the caller (the owner
/// token → you; an agent or K2 terminal passport → that workspace).
fn handle_widget_new(body: &[u8], caller: &Caller, ingress: &str) -> Result<J, ZenError> {
    let b: NewWidgetBody = parse_body(body, "widget/new takes {name, from?}")?;
    let f = files();
    let mut out = f.new_widget(&b.name, nonempty(&b.from).as_deref())?;
    out["changed"] = json!(refresh_and_emit()?);
    let name = b.name.trim();
    let st = f.widget_status(name);
    out["state"] = json!(st.state.as_str());
    out["errors"] = json!(st.errors);
    if st.state == WidgetState::Ok && !st.hash().is_empty() {
        let (made, stamped) = stamp_widget(name, st.hash(), caller, StampSource::New)?;
        audit("zen.widget.sign", caller, format!("{name} {} source=new stamped={stamped}", short(st.hash())), ingress);
        out["madeBy"] = made;
        out["stamped"] = json!(stamped);
    }
    Ok(out)
}

// ── Garden data authorship (contract-garden-data-v1 §4.3, §3.5; BG3) ────

/// The maker a caller stamps as, and how it called. `None` for a caller
/// that can't sign (a Connect login, app pass, sandboxed agent).
fn signer(caller: &Caller) -> Option<(MadeBy, CallerKind)> {
    match caller {
        Caller::Owner => Some((MadeBy::Owner, CallerKind::Owner)),
        Caller::Agent(a) if !a.sandboxed => {
            Some((MadeBy::Agent { project_id: a.project_id.clone() }, CallerKind::Agent))
        }
        Caller::Shell { project_id } => Some((MadeBy::Agent { project_id: project_id.clone() }, CallerKind::Shell)),
        _ => None,
    }
}

fn short(hash: &str) -> &str {
    &hash[..hash.len().min(12)]
}

fn data_err(e: k2_core::garden_data::DataError) -> ZenError {
    match e {
        k2_core::garden_data::DataError::BadRequest(m) => ZenError::BadRequest(m),
        other => ZenError::Io(other.to_string()),
    }
}

/// Stamp `(widget, hash)` as the caller (first signer wins). Answers the
/// maker now (wire form) and whether this call wrote it.
fn stamp_widget(widget: &str, hash: &str, caller: &Caller, source: StampSource) -> Result<(J, bool), ZenError> {
    let (author, kind) =
        signer(caller).ok_or_else(|| ZenError::OwnerOnly("only you or an agent on this computer can sign a widget".into()))?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let now = chrono::Utc::now().timestamp();
    let out = authors::stamp(&conn, widget, hash, &author, source, kind, now).map_err(data_err)?;
    let maker = authors::maker_of(&conn, widget, hash).map_err(data_err)?;
    Ok((crate::garden_data::call::made_by_wire(&maker), out == StampOutcome::Stamped))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WidgetNameBody {
    widget: String,
}

/// The live, clean bundle hash of a widget folder (sign) or its live hash
/// (claim; a folder with errors runs its last good version).
fn live_hash(f: &ZenFiles, widget: &str, clean: bool) -> Result<String, ZenError> {
    let st = f.widget_status(widget);
    if !st.exists {
        return Err(ZenError::UnknownWidget(format!("no widget folder '{widget}'")));
    }
    if clean && st.state != WidgetState::Ok {
        return Err(ZenError::WidgetBroken(format!(
            "{widget} has errors; fix them (k2 zen validate --widget {widget}) and sign again"
        )));
    }
    match st.bundle.as_ref() {
        Some(b) => Ok(b.hash.clone()),
        None => Err(ZenError::WidgetBroken(format!("{widget} has no good version"))),
    }
}

/// POST `/cli/zen/widget/sign {widget}`: stamp the live hash of a clean
/// widget folder as the caller. The owner token or a local agent/shell
/// passport (a K2 terminal is agent-level). Re-signing a hash that already
/// has a maker changes nothing (`stamped: false`).
fn handle_widget_sign(body: &[u8], caller: &Caller, ingress: &str) -> Result<J, ZenError> {
    let b: WidgetNameBody = parse_body(body, "widget/sign takes {widget}")?;
    let widget = b.widget.trim();
    if authors::is_builtin(widget) {
        return Ok(json!({ "ok": true, "widget": widget, "hash": J::Null, "madeBy": { "kind": "k2" }, "stamped": false }));
    }
    let f = files();
    set_up(&f)?;
    refresh_and_emit()?;
    let hash = live_hash(&f, widget, true)?;
    let (made, stamped) = stamp_widget(widget, &hash, caller, StampSource::Sign)?;
    audit("zen.widget.sign", caller, format!("{widget} {} source=sign stamped={stamped}", short(&hash)), ingress);
    Ok(json!({ "ok": true, "widget": widget, "hash": hash, "madeBy": made, "stamped": stamped }))
}

/// POST `/cli/zen/widget/claim {widget}`: the person's "It's mine". Owner
/// token only (checked in [`handle`]); overwrites the live hash's stamp.
fn handle_widget_claim(body: &[u8], caller: &Caller, ingress: &str) -> Result<J, ZenError> {
    let b: WidgetNameBody = parse_body(body, "widget/claim takes {widget}")?;
    let widget = b.widget.trim();
    let f = files();
    set_up(&f)?;
    refresh_and_emit()?;
    let hash = live_hash(&f, widget, false)?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    authors::claim(&conn, widget, &hash, chrono::Utc::now().timestamp()).map_err(data_err)?;
    let maker = authors::maker_of(&conn, widget, &hash).map_err(data_err)?;
    audit("zen.widget.claim", caller, format!("{widget} {}", short(&hash)), ingress);
    Ok(json!({ "ok": true, "widget": widget, "hash": hash, "madeBy": crate::garden_data::call::made_by_wire(&maker) }))
}

/// `GET /cli/zen/widgets` rows gain `madeBy`: the live version's maker
/// (creator rights, contract-garden-data-v1 §4.2), or null when the widget
/// has no good version. Settings shows It's mine from it before any data
/// exists.
pub(crate) fn with_made_by(v: &mut J) -> Result<(), ZenError> {
    let Some(rows) = v["widgets"].as_array_mut() else { return Ok(()) };
    let db = k2_core::db::shared();
    let conn = db.lock();
    for row in rows {
        let name = row["name"].as_str().unwrap_or_default().to_string();
        let hash = row["hash"].as_str().unwrap_or_default().to_string();
        row["madeBy"] = if hash.is_empty() {
            J::Null
        } else {
            let maker = authors::maker_of(&conn, &name, &hash).map_err(data_err)?;
            crate::garden_data::call::made_by_wire(&maker)
        };
    }
    Ok(())
}

/// Passport-admitted widget writes (contract-garden-data-v1 §4.3): an
/// agent or K2 terminal may make a widget folder and sign one. Never
/// lock-checked: a widget folder isn't a Garden.
pub const AGENT_WIDGET_WRITES: &[&str] = &["/cli/zen/widget/errors/clear", "/cli/zen/widget/new", "/cli/zen/widget/sign"];

const CLAIM_OWNER_ONLY: &str = "Only you can claim a widget (It's mine), in Settings \u{2192} Gardens \u{2192} Local Data or from a terminal outside K2. Agents and K2 terminals can't.";

/// Requests only a person may make: resuming a paused widget.
fn person_only(path: &str) -> bool {
    path == "/cli/zen/widget/resume"
}

const OWNER_ONLY: &str = "Only you can resume a paused widget, in the K2 app: click Resume on it. Agents, K2 terminals, Connect logins and app passes can't.";

// ── Garden Lock (prd-garden-lock-and-sync-v1 §5.2, §5.3, §6) ───────────

/// `POST /cli/zen/garden/lock {garden, locked}`: the person only.
pub const LOCK_ROUTE: &str = "/cli/zen/garden/lock";

const LOCK_OWNER_ONLY: &str = "Only the person can lock or unlock a Garden, in Settings \u{2192} Gardens. Agents and K2 terminals can't, either way.";

/// The Garden writes a session passport may make (`k2 zen garden …`,
/// `k2 zen reset`, `k2 zen theme set|next|prev` from inside a K2
/// terminal). Each is refused with 423 `garden_locked` when the Garden it
/// names is locked; otherwise it runs as it always did (agents reached
/// these with the disk owner token before Lock). Every other Zen route
/// still answers a passport 403 `zen_local_only`.
pub const AGENT_GARDEN_WRITES: &[&str] = &[
    "/cli/zen/garden/delete",
    "/cli/zen/garden/rename",
    "/cli/zen/garden/reorder",
    "/cli/zen/garden/template",
    "/cli/zen/reset",
    "/cli/zen/theme/next",
    "/cli/zen/theme/prev",
    "/cli/zen/theme/set",
];

/// Who called a Zen route. Zen paths aren't agent verbs, so
/// `caller_from_tcp` reports a session passport as `WrongCredential`; Lock
/// needs to know it is a passport (an agent, or a K2 terminal), so a valid
/// passport is classified from its session here.
pub fn zen_caller(path: &str, query: &str, bearer: Option<&str>, owner_token: &str) -> Caller {
    let c = crate::sidecar_routes::caller_from_tcp(path, query, bearer, owner_token);
    if c != Caller::WrongCredential {
        return c;
    }
    let presented = crate::routes::http::extract_token(query)
        .filter(|s| !s.is_empty())
        .or_else(|| bearer.map(str::trim).filter(|s| !s.is_empty()))
        .unwrap_or("");
    match crate::session_token::validate_hook(presented) {
        Some(v) => crate::sidecar_routes::caller_from_hook(&v),
        None => c,
    }
}

/// A passport the daemon treats as an agent on Zen writes (§5.3): any
/// harness session or K2 terminal on this computer. A sandboxed agent and
/// an API cell never reach Zen.
fn is_local_passport(caller: &Caller) -> bool {
    match caller {
        Caller::Agent(a) => !a.sandboxed,
        Caller::Shell { .. } => true,
        _ => false,
    }
}

/// The Garden an agent write names, if any: `garden`, or a `file` that is
/// `gardens/<id>.toml` (reset).
fn garden_named(body: &[u8]) -> Option<String> {
    let v = body_json(body).ok()?;
    if let Some(g) = v["garden"].as_str().map(str::trim).filter(|s| !s.is_empty()) {
        return Some(g.to_string());
    }
    match v["file"].as_str().map(ZenFile::parse) {
        Some(Ok(ZenFile::Garden(id))) => Some(id),
        _ => None,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LockBody {
    garden: String,
    locked: bool,
}

/// `POST /cli/zen/garden/lock` (§5.2). The caller is checked in [`handle`].
fn handle_garden_lock(body: &[u8], caller: &Caller, ingress: &str) -> Result<J, ZenError> {
    let b: LockBody = parse_body(body, "garden/lock takes {garden, locked}")?;
    let f = files();
    set_up(&f)?;
    let out = f.set_garden_lock(&b.garden, b.locked)?;
    audit(
        if out.locked { "zen.garden.lock" } else { "zen.garden.unlock" },
        caller,
        format!("{} ({}) changed={} widgets={}", out.garden, out.name, out.changed, out.widgets.join(",")),
        ingress,
    );
    let announced = refresh_and_emit()?;
    let list = f.sync_list()?;
    let row = list["gardens"]
        .as_array()
        .and_then(|a| a.iter().find(|g| g["id"] == out.garden.as_str()).cloned())
        .unwrap_or(J::Null);
    Ok(json!({
        "ok": true,
        "garden": row,
        "locked": out.locked,
        "changed": out.changed,
        "widgets": out.widgets,
        "announced": announced,
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameGardenBody {
    garden: String,
    name: String,
}

fn handle_garden_rename(body: &[u8]) -> Result<J, ZenError> {
    let b: RenameGardenBody = parse_body(body, "garden/rename takes {garden, name}")?;
    let f = files();
    let (g, renamed) = f.rename_garden(&b.garden, &b.name)?;
    let changed = if renamed { refresh_and_emit()? } else { false };
    let (index, _) = f.garden(&g.id)?;
    Ok(json!({ "ok": true, "garden": f.garden_json(index, &g, &f.read_active()), "changed": changed }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GardenBody {
    garden: String,
}

fn handle_garden_delete(body: &[u8]) -> Result<J, ZenError> {
    let b: GardenBody = parse_body(body, "garden/delete takes {garden}")?;
    let f = files();
    let out = f.delete_garden(&b.garden)?;
    let changed = refresh_and_emit()?;
    Ok(json!({
        "ok": true,
        "deleted": out.deleted.id,
        "name": out.deleted.name,
        "snapshot": out.snapshot,
        "gardens": f.gardens_json(),
        "changed": changed,
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReorderGardenBody {
    garden: String,
    to: i64,
}

fn handle_garden_reorder(body: &[u8]) -> Result<J, ZenError> {
    let b: ReorderGardenBody = parse_body(body, "garden/reorder takes {garden, to}")?;
    let f = files();
    let (_, moved) = f.reorder_garden(&b.garden, b.to)?;
    let changed = if moved { refresh_and_emit()? } else { false };
    Ok(json!({ "ok": true, "gardens": f.gardens_json(), "changed": changed }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TemplateGardenBody {
    garden: String,
    template: String,
    #[serde(default)]
    force: bool,
}

/// POST `/cli/zen/garden/template` (Rosson 2026-10-04, "Start with the
/// default"): turn one Garden into `texting` (Garden 1's page) or `blank`,
/// same id, name and place. The old file is kept in history first; a file
/// with its own changes is 409 `garden_has_changes` unless `force`. A
/// Garden already on the template is a no-op (`changed:false`, no event).
fn handle_garden_template(body: &[u8]) -> Result<J, ZenError> {
    let b: TemplateGardenBody = parse_body(body, "garden/template takes {garden, template, force?}")?;
    let f = files();
    let out = f.set_garden_template(&b.garden, &b.template, b.force)?;
    let changed = if out.changed { refresh_and_emit()? } else { false };
    let (index, g) = f.garden(&out.garden.id)?;
    let file = ZenFile::Garden(g.id.clone());
    Ok(json!({
        "ok": true,
        "garden": f.garden_json(index, &g, &f.read_active()),
        "template": out.template,
        "file": file.label(),
        "path": f.path_of(&file).display().to_string(),
        "snapshot": out.snapshot,
        "replaced": out.replaced,
        "changed": changed,
    }))
}

fn handle_reload() -> Result<J, ZenError> {
    let f = files();
    if !f.is_set_up() {
        return Err(ZenError::NotSetUp);
    }
    let changed = refresh_and_emit()?;
    let v = f.validate(None)?;
    Ok(json!({ "ok": true, "changed": changed, "errors": v["errors"], "warnings": v["warnings"] }))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ResetBody {
    file: Option<String>,
    garden: Option<String>,
    theme: Option<String>,
    /// A widget folder (UW11): restores its code files.
    widget: Option<String>,
    to: Option<String>,
}

fn handle_reset(body: &[u8]) -> Result<J, ZenError> {
    let b: ResetBody = parse_body(body, "reset takes {file?, garden?, theme?, widget?, to?}")?;
    let f = files();
    if !f.is_set_up() {
        return Err(ZenError::NotSetUp);
    }
    if let Some(w) = nonempty(&b.widget) {
        if b.file.is_some() || b.garden.is_some() || b.theme.is_some() {
            return Err(ZenError::BadRequest("pass one of file, garden, theme or widget".into()));
        }
        let mut out = f.reset_widget(&w, nonempty(&b.to).as_deref())?;
        out["changed"] = json!(refresh_and_emit()?);
        return Ok(out);
    }
    let file = target(&f, nonempty(&b.file).as_deref(), nonempty(&b.garden).as_deref(), nonempty(&b.theme).as_deref(), false)?
        .unwrap_or(ZenFile::Zen);
    let out = f.reset(&file, nonempty(&b.to).as_deref())?;
    let changed = refresh_and_emit()?;
    Ok(json!({
        "ok": true,
        "file": out.file,
        "restored": out.restored,
        "snapshot": out.snapshot,
        "changed": changed,
    }))
}

/// `{name?, garden?, clear?}` for `theme/set`; `{garden?}` for next/prev.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ThemeBody {
    name: Option<String>,
    garden: Option<String>,
    #[serde(default)]
    clear: bool,
}

fn switched(out: k2_core::zen::store::ThemeSwitch) -> Result<J, ZenError> {
    let changed = refresh_and_emit()?;
    Ok(json!({ "ok": true, "theme": out.theme, "scope": out.scope, "garden": out.garden, "changed": changed }))
}

/// POST `/cli/zen/theme/set|next|prev` (Omarchy addition 2, G16). The
/// daemon owns the active theme: one for the computer, plus an optional
/// pick per Garden. A switch emits one `zen_changed`.
fn handle_theme_switch(path: &str, body: &[u8]) -> Result<J, ZenError> {
    let b: ThemeBody = parse_body(body, "theme routes take {name?, garden?, clear?}")?;
    let f = files();
    if !f.is_set_up() {
        return Err(ZenError::NotSetUp);
    }
    let (name, garden) = (nonempty(&b.name), nonempty(&b.garden));
    match path {
        "/cli/zen/theme/set" => {
            if b.clear {
                if name.is_some() || garden.is_none() {
                    return Err(ZenError::BadRequest("clear takes a garden and no name".into()));
                }
                return switched(f.set_theme(None, garden.as_deref())?);
            }
            let name = name.ok_or_else(|| ZenError::BadRequest("theme/set needs {name}".into()))?;
            switched(f.set_theme(Some(&name), garden.as_deref())?)
        }
        _ => {
            if name.is_some() || b.clear {
                return Err(ZenError::BadRequest("theme/next and theme/prev take only {garden?}".into()));
            }
            let step = if path == "/cli/zen/theme/next" { 1 } else { -1 };
            switched(f.cycle_theme(step, garden.as_deref())?)
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewThemeBody {
    name: String,
    from: Option<String>,
}

fn handle_theme_new(body: &[u8]) -> Result<J, ZenError> {
    let b: NewThemeBody = parse_body(body, "theme/new needs {name, from?}")?;
    let f = files();
    let out = f.new_theme(b.name.trim(), nonempty(&b.from).as_deref())?;
    let changed = refresh_and_emit()?;
    Ok(json!({
        "ok": true,
        "name": out.name,
        "file": out.file,
        "path": out.path,
        "from": out.from,
        "copiedImage": out.copied_image,
        "changed": changed,
    }))
}

/// One `/cli/zen/*` request. `owner` is `token_is_owner` for this request;
/// `caller` is who presented what (`sidecar_routes::caller_from_tcp`), for
/// the person-only Resume rule and the audit line. The dispatcher has
/// already consumed the request (and refused a GET on a POST row and an
/// oversize body).
pub fn handle(
    path: &str,
    owner: bool,
    caller: &Caller,
    ingress: &str,
    params: &HashMap<String, String>,
    body: &[u8],
) -> CliResponse {
    // B3's `/cli/zen/lib/*` (UWB15): first-use library downloads and the
    // CDN cache. It answers `None` for every other path and applies Zen's
    // owner-token rule itself.
    if let Some(r) = crate::zen_lib_routes::handle(path, owner, params, body) {
        return r;
    }
    if !GET_ROUTES.contains(&path) && !POST_ROUTES.contains(&path) {
        return resp("404 Not Found", json!({ "error": "unknown zen route", "path": path }));
    }
    let owner_token = owner && *caller == Caller::Owner;
    if path == LOCK_ROUTE && !owner_token {
        // §5.2: agents can neither lock nor unlock (Rosson ruling 6).
        audit("zen.garden.lock_refused", caller, format!("{path}: the person only"), ingress);
        return err(ZenError::OwnerOnly(LOCK_OWNER_ONLY.into()));
    }
    if person_only(path) && !owner_token {
        audit("zen.widget.refused", caller, format!("{path}: owner token only"), ingress);
        return err(ZenError::OwnerOnly(OWNER_ONLY.into()));
    }
    if path == "/cli/zen/widget/claim" && !owner_token {
        audit("zen.widget.refused", caller, format!("{path}: owner token only"), ingress);
        return err(ZenError::OwnerOnly(CLAIM_OWNER_ONLY.into()));
    }
    if !owner {
        // §5.3: a passport (an agent or a K2 terminal) may make the Garden
        // writes it always could, unless the Garden is locked; and it may
        // make and sign widget folders (Garden data authorship, no Lock).
        // 0181: the put/proposal routes (the handler decides write or
        // propose) and, with "Manage Gardens and data sources", a new
        // Garden.
        let local = is_local_passport(caller);
        let widget_write = local && AGENT_WIDGET_WRITES.contains(&path);
        let agent_route = local && crate::zen_agent_routes::is_route(path);
        let managed_new = local && path == "/cli/zen/garden/new" && crate::gardens_manage::caller_may(caller);
        if !widget_write && !agent_route && !managed_new && !(local && AGENT_GARDEN_WRITES.contains(&path)) {
            return local_only();
        }
        if widget_write || managed_new || (agent_route && path != "/cli/zen/garden/put") {
            // Not a Garden: no lock check.
        } else if let Some(sel) = garden_named(body) {
            let f = files();
            if let Err(e) = f.refuse_if_locked(&sel) {
                audit("zen.garden.lock_refused", caller, format!("{path}: garden {sel} is locked"), ingress);
                return err(e);
            }
        }
    }
    if path == WIDGET_CHECK_ROUTE {
        return handle_widget_check(body, caller, ingress);
    }
    let r = match path {
        "/cli/zen/get" => handle_get(params),
        "/cli/zen/gardens" => Ok(handle_gardens()),
        "/cli/zen/validate" => {
            let f = files();
            match param(params, "widget") {
                Some(w) => set_up(&f).and_then(|_| f.validate_widget(w)).and_then(|mut v| {
                    // `--types[=all]`: the optional tsc-rs stage, warnings only.
                    if let Some(t) = param(params, "types") {
                        let types = f.widget_types(w, t == "all")?;
                        if let (Some(ws), Some(tw)) = (v["warnings"].as_array_mut(), types["warnings"].as_array()) {
                            ws.extend(tw.iter().cloned());
                        }
                        v["types"] = types;
                    }
                    Ok(v)
                }),
                None => set_up(&f)
                    .and_then(|_| refuse_home(params))
                    .and_then(|_| target(&f, param(params, "file"), param(params, "garden"), param(params, "theme"), false))
                    .and_then(|t| f.validate(t.as_ref())),
            }
        }
        "/cli/zen/history" => {
            let f = files();
            match param(params, "widget") {
                Some(w) => set_up(&f).and_then(|_| f.widget_history(w)),
                None => set_up(&f)
                    .and_then(|_| refuse_home(params))
                    .and_then(|_| target(&f, param(params, "file"), param(params, "garden"), param(params, "theme"), true))
                    .and_then(|t| f.history(t.as_ref())),
            }
        }
        // A received Garden bundle (zip) or folder, statically: never run,
        // never installed, every quoted string escaped and capped. Zen
        // needn't be set up: an agent reviews before anything is added.
        "/cli/zen/inspect" => match param(params, "path") {
            None => Err(ZenError::BadRequest("inspect needs path=<bundle or folder>".into())),
            Some(p) if !std::path::Path::new(p).is_absolute() => {
                Err(ZenError::BadRequest("inspect needs an absolute path".into()))
            }
            Some(p) => k2_core::jscheck::inspect::inspect(std::path::Path::new(p)).map_err(ZenError::BadRequest),
        },
        "/cli/zen/templates" => {
            Ok(json!({ "ok": true, "templates": k2_core::zen::garden_catalog::template_list() }))
        }
        "/cli/zen/widgets" => {
            let f = files();
            set_up(&f)
                .and_then(|_| refresh_and_emit())
                .and_then(|_| f.widgets_json(&pauses()))
                .and_then(|mut v| with_made_by(&mut v).map(|_| v))
        }
        "/cli/zen/widget/bundle" => {
            let f = files();
            set_up(&f).and_then(|_| refresh_and_emit()).and_then(|_| handle_widget_bundle(params))
        }
        "/cli/zen/widget/new" => handle_widget_new(body, caller, ingress),
        "/cli/zen/widget/run" => handle_widget_run(body),
        "/cli/zen/widget/errors/clear" => handle_widget_errors_clear(body, caller, ingress),
        "/cli/zen/widget/errors" => match param(params, "widget") {
            None => Err(ZenError::BadRequest("widget/errors needs widget=<name>".into())),
            Some(w) => {
                let since = match param(params, "since") {
                    None => Ok(None),
                    Some(s) => s.parse::<u64>().map(Some).map_err(|_| ZenError::BadRequest("since is a sequence number".into())),
                };
                since.and_then(|s| files().widget_errors(w, s))
            }
        },
        // Garden v3 (contract-sources-v1 §4.2): placements, caps, taint, sources.
        "/cli/zen/widget/show" => match param(params, "widget") {
            None => Err(ZenError::BadRequest("widget/show needs widget=<name>".into())),
            Some(w) => files().widget_show(w, &pauses()),
        },
        "/cli/zen/widget/pause" => handle_widget_pause(body, caller, ingress),
        "/cli/zen/widget/resume" => handle_widget_resume(body, caller, ingress),
        // Garden data authorship (contract-garden-data-v1 §4.3, BG3):
        // sign = the owner token or a local agent/shell passport; claim =
        // the owner token only (checked above).
        "/cli/zen/widget/sign" => handle_widget_sign(body, caller, ingress),
        "/cli/zen/widget/claim" => handle_widget_claim(body, caller, ingress),
        "/cli/zen/theme/list" => refuse_home(params).and_then(|_| files().theme_list(param(params, "garden"))),
        "/cli/zen/theme/set" | "/cli/zen/theme/next" | "/cli/zen/theme/prev" => handle_theme_switch(path, body),
        "/cli/zen/theme/new" => handle_theme_new(body),
        "/cli/zen/status" => Ok(handle_status()),
        "/cli/zen/doctor" => Ok(handle_doctor()),
        "/cli/zen/setup" => handle_setup(body),
        "/cli/zen/garden/new" => {
            let r = handle_garden_new(body);
            if !owner {
                if let Ok(v) = &r {
                    crate::gardens_manage::audit(
                        "zen-garden-new",
                        &caller.audit_user(),
                        v["garden"]["id"].as_str().or_else(|| v["id"].as_str()).unwrap_or("-"),
                        None,
                        None,
                    );
                }
            }
            r
        }
        "/cli/zen/garden/rename" => handle_garden_rename(body),
        "/cli/zen/garden/delete" => handle_garden_delete(body),
        "/cli/zen/garden/reorder" => handle_garden_reorder(body),
        "/cli/zen/garden/template" => handle_garden_template(body),
        LOCK_ROUTE => handle_garden_lock(body, caller, ingress),
        // Agents run Gardens end to end (0181): puts and the proposal queue.
        p if crate::zen_agent_routes::is_route(p) => {
            return crate::zen_agent_routes::handle(p, owner_token, caller, params, body);
        }
        "/cli/zen/reload" => handle_reload(),
        "/cli/zen/reset" => handle_reset(body),
        // Garden sync (prd-zen-garden-sync-defaults-v1 GS31/GS32): a change
        // takes the owner token from the owner itself, never a passport.
        p if crate::zen_sync_routes::is_post_route(p) && !owner_token => return local_only(),
        p if crate::zen_sync_routes::is_route(p) => crate::zen_sync_routes::handle(p, params, body),
        _ => unreachable!("checked against GET_ROUTES/POST_ROUTES above"),
    };
    match r {
        Ok(v) => ok(v),
        Err(e) => err(e),
    }
}
