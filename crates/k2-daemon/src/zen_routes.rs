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
//! No route writes `grants.json` (Z19): it is reserved for v2, where only a
//! click in the K2 app may write it. `.history/` is written only as a side
//! effect of the clean-save snapshot, `reset` and `garden/delete`.

use std::collections::HashMap;
use std::sync::Mutex;

use serde::Deserialize;
use serde_json::{json, Value as J};

use k2_core::zen::store::NOT_SET_UP;
use k2_core::zen::{ZenError, ZenFile, ZenFiles};

use crate::cli_response::CliResponse;

/// Request bodies over this are refused with 413 (Z15).
pub const MAX_BODY: usize = 64 * 1024;

/// The POST rows (G45). A GET on any of them is 405.
pub const POST_ROUTES: &[&str] = &[
    "/cli/zen/garden/delete",
    "/cli/zen/garden/new",
    "/cli/zen/garden/rename",
    "/cli/zen/garden/reorder",
    "/cli/zen/reload",
    "/cli/zen/reset",
    "/cli/zen/setup",
    "/cli/zen/theme/new",
    "/cli/zen/theme/next",
    "/cli/zen/theme/prev",
    "/cli/zen/theme/set",
];

/// The GET rows (G45).
pub const GET_ROUTES: &[&str] = &[
    "/cli/zen/doctor",
    "/cli/zen/gardens",
    "/cli/zen/get",
    "/cli/zen/history",
    "/cli/zen/status",
    "/cli/zen/theme/list",
    "/cli/zen/validate",
];

/// Fingerprint of the last state announced with `zen_changed`.
static LAST_FINGERPRINT: Mutex<Option<String>> = Mutex::new(None);

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
        ZenError::Io(m) => resp("500 Internal Server Error", json!({ "ok": false, "error": "io", "message": m })),
    }
}

fn files() -> ZenFiles {
    ZenFiles::local()
}

/// Re-read the folder (snapshotting clean new content) and emit ONE
/// `zen_changed` when the live state moved. Serialised: the watcher, the
/// routes and the CLI all funnel here, so each effective change is
/// announced once whoever sees it first. Returns whether it moved.
pub fn refresh_and_emit() -> Result<bool, ZenError> {
    let mut last = LAST_FINGERPRINT.lock().unwrap_or_else(|e| e.into_inner());
    let fp = files().refresh()?;
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
        if let Ok(fp) = files().refresh() {
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
    f.resolve(param(params, "garden"))
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
                "watching zen.toml, gardens/ and themes/ (250 ms debounce)"
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
    to: Option<String>,
}

fn handle_reset(body: &[u8]) -> Result<J, ZenError> {
    let b: ResetBody = parse_body(body, "reset takes {file?, garden?, theme?, to?}")?;
    let f = files();
    if !f.is_set_up() {
        return Err(ZenError::NotSetUp);
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
/// the dispatcher has already consumed the request (and refused a GET on a
/// POST row and an oversize body).
pub fn handle(path: &str, owner: bool, params: &HashMap<String, String>, body: &[u8]) -> CliResponse {
    if !GET_ROUTES.contains(&path) && !POST_ROUTES.contains(&path) {
        return resp("404 Not Found", json!({ "error": "unknown zen route", "path": path }));
    }
    if !owner {
        return local_only();
    }
    let r = match path {
        "/cli/zen/get" => handle_get(params),
        "/cli/zen/gardens" => Ok(handle_gardens()),
        "/cli/zen/validate" => {
            let f = files();
            set_up(&f)
                .and_then(|_| refuse_home(params))
                .and_then(|_| target(&f, param(params, "file"), param(params, "garden"), param(params, "theme"), false))
                .and_then(|t| f.validate(t.as_ref()))
        }
        "/cli/zen/history" => {
            let f = files();
            set_up(&f)
                .and_then(|_| refuse_home(params))
                .and_then(|_| target(&f, param(params, "file"), param(params, "garden"), param(params, "theme"), true))
                .and_then(|t| f.history(t.as_ref()))
        }
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
        "/cli/zen/reload" => handle_reload(),
        "/cli/zen/reset" => handle_reset(body),
        _ => unreachable!("checked against GET_ROUTES/POST_ROUTES above"),
    };
    match r {
        Ok(v) => ok(v),
        Err(e) => err(e),
    }
}
