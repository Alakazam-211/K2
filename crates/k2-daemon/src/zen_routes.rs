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

use std::collections::HashMap;
use std::sync::Mutex;

use serde::Deserialize;
use serde_json::{json, Value as J};

use k2_core::zen::widget_access::{Pause, PauseReason, WidgetPauses};
use k2_core::zen::store::NOT_SET_UP;
use k2_core::zen::{ZenError, ZenFile, ZenFiles};

use crate::cli_response::CliResponse;
use crate::sidecar_routes::Caller;

/// Request bodies over this are refused with 413 (Z15).
pub const MAX_BODY: usize = 64 * 1024;

/// The POST rows (G45, UW35). A GET on any of them is 405.
/// `/cli/zen/lib/fetch` is `zen_lib_routes`'s (B3, UWB15).
pub const POST_ROUTES: &[&str] = &[
    "/cli/zen/garden/delete",
    "/cli/zen/garden/new",
    "/cli/zen/garden/rename",
    "/cli/zen/garden/reorder",
    "/cli/zen/garden/sync",
    "/cli/zen/garden/template",
    "/cli/zen/lib/fetch",
    "/cli/zen/news/seen",
    "/cli/zen/reload",
    "/cli/zen/reset",
    "/cli/zen/setup",
    "/cli/zen/theme/new",
    "/cli/zen/theme/next",
    "/cli/zen/theme/prev",
    "/cli/zen/theme/set",
    "/cli/zen/widget/new",
    "/cli/zen/widget/pause",
    "/cli/zen/widget/resume",
];

/// The GET rows (G45, UW35, UWB23). `/cli/zen/lib/file` is B3's.
pub const GET_ROUTES: &[&str] = &[
    "/cli/zen/doctor",
    "/cli/zen/gardens",
    "/cli/zen/get",
    "/cli/zen/history",
    "/cli/zen/lib/file",
    "/cli/zen/news",
    "/cli/zen/status",
    "/cli/zen/sync",
    "/cli/zen/templates",
    "/cli/zen/theme/list",
    "/cli/zen/validate",
    "/cli/zen/widget/bundle",
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
        json!({ "error": "body_too_large", "message": "Zen request bodies must be under 64 KB." }),
    )
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

/// Re-read the folder (snapshotting clean new content) and emit ONE
/// `zen_changed` when the live state moved. Serialised: the watcher, the
/// routes and the CLI all funnel here, so each effective change is
/// announced once whoever sees it first. Returns whether it moved. The
/// fingerprint covers widget folders and runaway pauses (UW12).
pub fn refresh_and_emit() -> Result<bool, ZenError> {
    let mut last = LAST_FINGERPRINT.lock().unwrap_or_else(|e| e.into_inner());
    let f = files();
    let fp = f.refresh_with(&pauses())?;
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
        if let Ok(fp) = f.refresh_with(&pauses()) {
            *last = Some(fp);
        }
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

/// GET `/cli/zen/widget/bundle?widget=` (UW35): the live bundle with a
/// fresh nonce in every script.
fn handle_widget_bundle(params: &HashMap<String, String>) -> Result<J, ZenError> {
    let widget = param(params, "widget").ok_or_else(|| ZenError::BadRequest("widget/bundle needs widget=<name>".into()))?;
    let f = files();
    let st = f.widget_bundle(widget)?;
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
/// like `garden/new`.
fn handle_widget_new(body: &[u8]) -> Result<J, ZenError> {
    let b: NewWidgetBody = parse_body(body, "widget/new takes {name, from?}")?;
    let f = files();
    let mut out = f.new_widget(&b.name, nonempty(&b.from).as_deref())?;
    out["changed"] = json!(refresh_and_emit()?);
    let st = f.widget_status(b.name.trim());
    out["state"] = json!(st.state.as_str());
    out["errors"] = json!(st.errors);
    Ok(out)
}

/// Requests only a person may make: resuming a paused widget.
fn person_only(path: &str) -> bool {
    path == "/cli/zen/widget/resume"
}

const OWNER_ONLY: &str = "Only you can resume a paused widget, in the K2 app: click Resume on it. Agents, K2 terminals, Connect logins and app passes can't.";

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
    if person_only(path) && !owner_token {
        audit("zen.widget.refused", caller, format!("{path}: owner token only"), ingress);
        return err(ZenError::OwnerOnly(OWNER_ONLY.into()));
    }
    if !owner {
        return local_only();
    }
    let r = match path {
        "/cli/zen/get" => handle_get(params),
        "/cli/zen/gardens" => Ok(handle_gardens()),
        "/cli/zen/validate" => {
            let f = files();
            match param(params, "widget") {
                Some(w) => set_up(&f).and_then(|_| f.validate_widget(w)),
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
        "/cli/zen/templates" => {
            Ok(json!({ "ok": true, "templates": k2_core::zen::garden_catalog::template_list() }))
        }
        "/cli/zen/widgets" => {
            let f = files();
            set_up(&f).and_then(|_| refresh_and_emit()).and_then(|_| f.widgets_json(&pauses()))
        }
        "/cli/zen/widget/bundle" => {
            let f = files();
            set_up(&f).and_then(|_| refresh_and_emit()).and_then(|_| handle_widget_bundle(params))
        }
        "/cli/zen/widget/new" => handle_widget_new(body),
        "/cli/zen/widget/pause" => handle_widget_pause(body, caller, ingress),
        "/cli/zen/widget/resume" => handle_widget_resume(body, caller, ingress),
        "/cli/zen/theme/list" => refuse_home(params).and_then(|_| files().theme_list(param(params, "garden"))),
        "/cli/zen/theme/set" | "/cli/zen/theme/next" | "/cli/zen/theme/prev" => handle_theme_switch(path, body),
        "/cli/zen/theme/new" => handle_theme_new(body),
        "/cli/zen/status" => Ok(handle_status()),
        "/cli/zen/doctor" => Ok(handle_doctor()),
        "/cli/zen/setup" => handle_setup(body),
        "/cli/zen/garden/new" => handle_garden_new(body),
        "/cli/zen/garden/rename" => handle_garden_rename(body),
        "/cli/zen/garden/delete" => handle_garden_delete(body),
        "/cli/zen/garden/reorder" => handle_garden_reorder(body),
        "/cli/zen/garden/template" => handle_garden_template(body),
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
