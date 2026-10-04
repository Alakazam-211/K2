//! `/cli/zen/*` (prd-zen-mode-v1 Z15, vs-live Z59/Z62).
//!
//! `~/.k2/zen/` belongs to the OS user of this computer, so every route
//! answers ONLY the local daemon's owner token (Z15a): a Connect login onto
//! this daemon (any role), an app pass or an agent passport gets 403
//! `zen_local_only`. The policy rows are Member so the source-walk test
//! passes and `role_gate` lets the request reach this check; the check
//! itself lives here (Z59), after the dispatcher consumed the request.
//!
//! No route writes `grants.json` (Z19): it is reserved for v2, where only a
//! click in the K2 app may write it. `homes.json` and `.history/` are
//! written only as a side effect of `page/ensure`, `homes/sync`, `reset`
//! and the clean-save snapshot.

use std::collections::HashMap;
use std::sync::Mutex;

use serde::Deserialize;
use serde_json::{json, Value as J};

use k2_core::zen::store::{HomeEntry, NOT_SET_UP};
use k2_core::zen::{ZenError, ZenFile, ZenFiles};

use crate::cli_response::CliResponse;

/// Request bodies over this are refused with 413 (Z15).
pub const MAX_BODY: usize = 64 * 1024;

/// The POST rows. A GET on any of them is 405.
pub const POST_ROUTES: &[&str] = &[
    "/cli/zen/homes/sync",
    "/cli/zen/page/ensure",
    "/cli/zen/reload",
    "/cli/zen/reset",
];

/// The GET rows.
pub const GET_ROUTES: &[&str] = &[
    "/cli/zen/doctor",
    "/cli/zen/get",
    "/cli/zen/history",
    "/cli/zen/status",
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
    match e {
        ZenError::NotSetUp => resp(
            "404 Not Found",
            json!({ "ok": false, "error": "zen_not_set_up", "message": NOT_SET_UP }),
        ),
        ZenError::BadRequest(m) => resp("400 Bad Request", json!({ "ok": false, "error": "bad_request", "message": m })),
        ZenError::NotFound(m) => resp("404 Not Found", json!({ "ok": false, "error": "not_found", "message": m })),
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

/// `file=` and/or `home=` (id or name) → the file they name.
fn target(f: &ZenFiles, file: Option<&str>, home: Option<&str>) -> Result<Option<ZenFile>, ZenError> {
    match (file, home) {
        (Some(_), Some(_)) => Err(ZenError::BadRequest("pass file or home, not both".into())),
        (Some(file), None) => ZenFile::parse(file).map(Some),
        (None, Some(home)) => {
            let id = f.find_home(home).ok_or_else(|| {
                ZenError::NotFound(format!("no Home '{home}' on this computer; list them with k2 zen pages"))
            })?;
            Ok(Some(ZenFile::Page(id)))
        }
        (None, None) => Ok(None),
    }
}

fn body_json(body: &[u8]) -> Result<J, ZenError> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(json!({}));
    }
    serde_json::from_slice(body).map_err(|e| ZenError::BadRequest(format!("invalid JSON body: {e}")))
}

fn handle_get(params: &HashMap<String, String>) -> Result<J, ZenError> {
    let f = files();
    if !f.exists() {
        return Err(ZenError::NotSetUp);
    }
    // `get` serves the live state; refresh first so a save the watcher
    // hasn't debounced yet is already live (and announced once).
    refresh_and_emit()?;
    f.resolve(param(params, "home"))
}

fn handle_status() -> J {
    let f = files();
    let set_up = f.exists();
    let homes = f.read_homes();
    let ids = f.page_ids();
    let mut pages: Vec<J> = homes
        .iter()
        .map(|h| json!({ "id": h.id, "name": h.name, "hasFile": ids.contains(&h.id) }))
        .collect();
    for id in &ids {
        if !homes.iter().any(|h| &h.id == id) {
            pages.push(json!({ "id": id, "name": J::Null, "hasFile": true }));
        }
    }
    json!({
        "ok": true,
        "setUp": set_up,
        "path": f.root().display().to_string(),
        "pages": pages,
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
                "watching zen.toml and pages/ (250 ms debounce)"
            } else {
                "not running; edits apply on the next k2 zen reload or Zen open"
            },
        }));
    }
    let all_ok = checks.iter().all(|c| c["ok"].as_bool() == Some(true));
    json!({ "ok": all_ok, "setUp": f.exists(), "path": f.root().display().to_string(), "checks": checks })
}

#[derive(Deserialize)]
struct EnsureBody {
    #[serde(rename = "homeId")]
    home_id: String,
    name: String,
}

fn handle_ensure(body: &[u8]) -> Result<J, ZenError> {
    let b: EnsureBody = serde_json::from_value(body_json(body)?)
        .map_err(|e| ZenError::BadRequest(format!("page/ensure needs {{homeId, name}}: {e}")))?;
    let f = files();
    let outcome = f.ensure_page(&b.home_id, &b.name)?;
    // Z61: the watcher starts on the first ensure when the folder was
    // missing at boot. Z68: seed `k2-zen` into every workspace once.
    crate::zen_watch::start();
    if outcome.created_folder {
        let seeded = k2_core::workspace::skill_regen::seed_zen_skill_everywhere();
        k2_core::log_debug!("[daemon/zen] set up; k2-zen skill in {seeded} workspace(s)");
    }
    let changed = refresh_and_emit()?;
    Ok(json!({
        "ok": true,
        "path": f.root().display().to_string(),
        "file": outcome.file,
        "createdFolder": outcome.created_folder,
        "createdZen": outcome.created_zen,
        "createdPage": outcome.created_page,
        "changed": changed,
    }))
}

#[derive(Deserialize)]
struct SyncBody {
    homes: Vec<HomeEntry>,
}

fn handle_sync(body: &[u8]) -> Result<J, ZenError> {
    let b: SyncBody = serde_json::from_value(body_json(body)?)
        .map_err(|e| ZenError::BadRequest(format!("homes/sync needs {{homes:[{{id, name}}]}}: {e}")))?;
    let (written, archived) = files().sync_homes(b.homes)?;
    let changed = if written && !archived.is_empty() { refresh_and_emit()? } else { false };
    Ok(json!({
        "ok": true,
        "written": written,
        "archived": archived,
        "changed": changed,
        "reason": if written { J::Null } else { json!("zen_not_set_up") },
    }))
}

fn handle_reload() -> Result<J, ZenError> {
    let f = files();
    if !f.exists() {
        return Err(ZenError::NotSetUp);
    }
    let changed = refresh_and_emit()?;
    let v = f.validate(None)?;
    Ok(json!({ "ok": true, "changed": changed, "errors": v["errors"], "warnings": v["warnings"] }))
}

#[derive(Deserialize, Default)]
struct ResetBody {
    file: Option<String>,
    home: Option<String>,
    to: Option<String>,
}

fn handle_reset(body: &[u8]) -> Result<J, ZenError> {
    let b: ResetBody = serde_json::from_value(body_json(body)?)
        .map_err(|e| ZenError::BadRequest(format!("reset takes {{file?, home?, to?}}: {e}")))?;
    let f = files();
    if !f.exists() {
        return Err(ZenError::NotSetUp);
    }
    let nonempty = |s: &Option<String>| s.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let file = target(&f, nonempty(&b.file).as_deref(), nonempty(&b.home).as_deref())?
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
        "/cli/zen/validate" => {
            let f = files();
            target(&f, param(params, "file"), param(params, "home")).and_then(|t| f.validate(t.as_ref()))
        }
        "/cli/zen/history" => {
            let f = files();
            target(&f, param(params, "file"), param(params, "home")).and_then(|t| f.history(t.as_ref()))
        }
        "/cli/zen/status" => Ok(handle_status()),
        "/cli/zen/doctor" => Ok(handle_doctor()),
        "/cli/zen/page/ensure" => handle_ensure(body),
        "/cli/zen/homes/sync" => handle_sync(body),
        "/cli/zen/reload" => handle_reload(),
        "/cli/zen/reset" => handle_reset(body),
        _ => unreachable!("checked against GET_ROUTES/POST_ROUTES above"),
    };
    match r {
        Ok(v) => ok(v),
        Err(e) => err(e),
    }
}
