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
//! Nothing reads or writes `grants.json` (Z19). Zen v2 custom-widget grants
//! (prd-zen-user-widgets-v2 UWB3–UWB6) are signed rows in the daemon's
//! database (`k2_core::zen::grants`, migration 0138). Giving power
//! (`widget/grant`, `widget/sending` on, `widget/resume`, `garden/new` with
//! a grant) takes ONLY the owner token: a passport (agent or K2 shell tab),
//! a Connect login, an API key or an app pass gets 403 `owner_only`. Taking
//! power away (`widget/revoke`, `widget/sending` off) is allowed to
//! anything Zen accepts. Every grant change writes an audit line. Limit
//! (R1, parked into the vault research): an agent that reads the disk owner
//! token on purpose still passes, as for every owner-only route.
//! `.history/` is written only as a side effect of the clean-save snapshot,
//! `reset`, `garden/template` and `garden/delete`.

use std::collections::HashMap;
use std::sync::Mutex;

use serde::Deserialize;
use serde_json::{json, Value as J};

use k2_core::zen::grants::{self, GrantEntry, GrantKey, GrantRecord, GrantSnapshot, Pause, PauseReason, Scope};
use k2_core::zen::store::NOT_SET_UP;
use k2_core::zen::{ZenError, ZenFile, ZenFiles};

use crate::cli_response::CliResponse;
use crate::sidecar_routes::Caller;

/// Request bodies over this are refused with 413 (Z15).
pub const MAX_BODY: usize = 64 * 1024;

/// The POST rows (G45, UW35, UWB6). A GET on any of them is 405.
/// `/cli/zen/lib/fetch` is `zen_lib_routes`'s (B3, UWB15).
pub const POST_ROUTES: &[&str] = &[
    "/cli/zen/garden/delete",
    "/cli/zen/garden/new",
    "/cli/zen/garden/rename",
    "/cli/zen/garden/reorder",
    "/cli/zen/garden/template",
    "/cli/zen/lib/fetch",
    "/cli/zen/reload",
    "/cli/zen/reset",
    "/cli/zen/setup",
    "/cli/zen/theme/new",
    "/cli/zen/theme/next",
    "/cli/zen/theme/prev",
    "/cli/zen/theme/set",
    "/cli/zen/widget/grant",
    "/cli/zen/widget/new",
    "/cli/zen/widget/resume",
    "/cli/zen/widget/revoke",
    "/cli/zen/widget/sending",
];

/// The GET rows (G45, UW35, UWB6, UWB23). `/cli/zen/lib/file` is B3's.
pub const GET_ROUTES: &[&str] = &[
    "/cli/zen/doctor",
    "/cli/zen/gardens",
    "/cli/zen/get",
    "/cli/zen/history",
    "/cli/zen/lib/file",
    "/cli/zen/status",
    "/cli/zen/templates",
    "/cli/zen/theme/list",
    "/cli/zen/validate",
    "/cli/zen/widget/bundle",
    "/cli/zen/widget/grants",
    "/cli/zen/widgets",
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

/// `~/.k2/zen-grant.key` for this Zen folder (UWB4).
fn key_path(f: &ZenFiles) -> std::path::PathBuf {
    grants::key_path_for(f.root())
}

/// Every live grant row and the key, read once per request. Fails closed:
/// no database (a unit test without one) means no grants, so widgets show
/// K2's review card.
fn grant_snapshot(f: &ZenFiles) -> Result<GrantSnapshot, ZenError> {
    let Some(db) = k2_core::db::try_shared() else {
        return Ok(GrantSnapshot::empty());
    };
    let conn = db.lock();
    GrantSnapshot::load(&conn, &key_path(f)).map_err(|e| ZenError::Io(format!("zen_widget_grants: {e}")))
}

/// Re-read the folder (snapshotting clean new content) and emit ONE
/// `zen_changed` when the live state moved. Serialised: the watcher, the
/// routes and the CLI all funnel here, so each effective change is
/// announced once whoever sees it first. Returns whether it moved. The
/// fingerprint covers widget folders and grants (UW12).
pub fn refresh_and_emit() -> Result<bool, ZenError> {
    let mut last = LAST_FINGERPRINT.lock().unwrap_or_else(|e| e.into_inner());
    let f = files();
    let fp = f.refresh_with(&grant_snapshot(&f)?)?;
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
        if let Ok(fp) = grant_snapshot(&f).and_then(|s| f.refresh_with(&s)) {
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
    f.resolve_with(param(params, "garden"), &grant_snapshot(&f)?)
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
        checks.push(grants_check(&f));
    }
    let all_ok = checks.iter().all(|c| c["ok"].as_bool() == Some(true));
    json!({ "ok": all_ok, "setUp": f.is_set_up(), "path": f.root().display().to_string(), "checks": checks })
}

/// The doctor's `grants` check (UWB4): the key, and rows that don't verify.
fn grants_check(f: &ZenFiles) -> J {
    let snap = match grant_snapshot(f) {
        Ok(s) => s,
        Err(e) => return json!({ "name": "grants", "ok": false, "detail": e.to_string() }),
    };
    let bad = snap
        .rows
        .iter()
        .filter(|r| !snap.key.as_ref().is_some_and(|k| r.key_id == k.id() && grants::verify(k, &r.record, &r.sig)))
        .count();
    let (ok, detail) = match (&snap.key_error, &snap.key, snap.rows.len(), bad) {
        (Some(e), _, n, _) => (false, format!("{n} grant(s); the key can't be read ({e}), so none of them count")),
        (None, None, 0, _) => (true, "no widget grants yet".to_string()),
        (None, None, n, _) => (false, format!("{n} grant(s) but no key file; they don't count until allowed again")),
        (None, Some(_), n, 0) => (true, format!("{n} widget grant(s), each signed")),
        (None, Some(_), n, b) => (false, format!("{b} of {n} grant(s) don't check out (edited by hand or another key); they show K2's review card")),
    };
    json!({ "name": "grants", "ok": ok, "detail": detail })
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
    /// UWB22: a catalog Garden whose template needs a grant gets it in the
    /// same owner click. Owner token only.
    grant: Option<NewGardenGrant>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewGardenGrant {
    scope: J,
    #[serde(default = "yes")]
    sending: bool,
    /// The rows the scope resolved to in the dialog (UWA8); may be empty.
    #[serde(default)]
    entries: Vec<GrantEntry>,
}

fn yes() -> bool {
    true
}

fn handle_garden_new(body: &[u8], caller: &Caller, ingress: &str) -> Result<J, ZenError> {
    let b: NewGardenBody = parse_body(body, "garden/new takes {name, template?, seedHome?, at?, grant?}")?;
    let f = files();
    // A grant: check everything before the Garden exists, so a refusal
    // leaves nothing behind.
    let planned = match &b.grant {
        None => None,
        Some(g) => {
            if !f.is_set_up() {
                return Err(ZenError::NotSetUp);
            }
            let tid = k2_core::zen::store::template_choice(nonempty(&b.template).as_deref())?;
            let entry = k2_core::zen::garden_catalog::garden_catalog()
                .iter()
                .find(|e| e.template_id == tid)
                .and_then(|e| e.meta.grant.clone())
                .ok_or_else(|| {
                    ZenError::BadRequest(format!("template '{tid}' has no widget to allow; drop grant"))
                })?;
            let page = k2_core::zen::template_page(tid)
                .ok_or_else(|| ZenError::Io(format!("built-in template {tid} is missing")))?;
            let placement = page["widgets"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|w| w["kind"] == "custom" && w["widget"] == entry.widget.as_str())
                .and_then(|w| w["id"].as_str().map(str::to_string))
                .ok_or_else(|| ZenError::Io(format!("template {tid} doesn't place {}", entry.widget)))?;
            let scope = Scope::from_json(&g.scope).map_err(ZenError::BadRequest)?;
            check_entries(&g.entries, &scope, &Default::default(), 0)?;
            let st = f.widget_bundle(&entry.widget)?;
            let caps: Vec<String> = entry.caps.iter().filter(|c| st.requested().contains(c)).cloned().collect();
            Some((placement, entry.widget, caps, scope, g.entries.clone(), g.sending, st.hash().to_string()))
        }
    };
    let g = f.new_garden(&b.name, nonempty(&b.template).as_deref(), nonempty(&b.seed_home).as_deref(), b.at)?;
    let grant = match planned {
        None => J::Null,
        Some((placement, widget, caps, scope, entries, sending, hash)) => {
            let row = GrantRecord {
                garden: g.id.clone(),
                placement,
                widget,
                caps,
                scope,
                entries,
                ask: Default::default(),
                sending,
                granted_at: now(),
            };
            json!(store_grant(&f, row, &hash, caller, ingress)?)
        }
    };
    let changed = refresh_and_emit()?;
    let (index, _) = f.garden(&g.id)?;
    let file = ZenFile::Garden(g.id.clone());
    Ok(json!({
        "ok": true,
        "garden": f.garden_json(index, &g, &f.read_active()),
        "file": file.label(),
        "path": f.path_of(&file).display().to_string(),
        "grant": grant,
        "changed": changed,
    }))
}

// ── Custom widgets (prd-zen-user-widgets-v2 UW35, UWB3–UWB9, UWB23) ─────

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

/// The bound rows a grant records (UWA8): 1 to 200 well-formed, distinct
/// pairs (`min` 0 for a catalog Garden's create click), exactly one for an
/// agent scope; a placement that asks for one agent takes only an agent
/// scope.
fn check_entries(
    entries: &[GrantEntry],
    scope: &Scope,
    ask: &grants::PlacementAsk,
    min: usize,
) -> Result<(), ZenError> {
    if entries.len() < min || entries.len() > 200 {
        return Err(ZenError::BadRequest(format!("entries holds {min} to 200 {{server, room}} pairs")));
    }
    for e in entries {
        let ok = |s: &str| !s.trim().is_empty() && s.len() <= 200 && !s.chars().any(char::is_control);
        if !ok(&e.server) || !ok(&e.room) {
            return Err(ZenError::BadRequest("each entry is {server, room}: non-empty text".into()));
        }
    }
    let mut sorted = entries.to_vec();
    sorted.sort();
    sorted.dedup();
    if sorted.len() != entries.len() {
        return Err(ZenError::BadRequest("entries lists a {server, room} twice".into()));
    }
    if matches!(scope, Scope::Agent(_)) && entries.len() > 1 {
        return Err(ZenError::BadRequest("an agent scope binds exactly one entry".into()));
    }
    if ask.agent.is_some() && !matches!(scope, Scope::Agent(_)) {
        return Err(ZenError::BadRequest(
            "this placement asks for one agent (its agent prop); grant it an agent scope".into(),
        ));
    }
    Ok(())
}

/// Sign and store a grant, prune rows whose placement is gone, write the
/// audit line, and answer the view the renderer reads.
fn store_grant(
    f: &ZenFiles,
    record: GrantRecord,
    hash: &str,
    caller: &Caller,
    ingress: &str,
) -> Result<grants::GrantView, ZenError> {
    let key = GrantKey::load_or_create(&key_path(f)).map_err(|e| ZenError::Io(format!("grant key: {e}")))?;
    let db = k2_core::db::try_shared().ok_or_else(|| ZenError::Io("K2's database isn't open".into()))?;
    let placements = f.custom_placements();
    let conn = db.lock();
    let summary = format!(
        "granted {}/{} widget={} hash={} caps={} scope={} sending={}",
        record.garden,
        record.placement,
        record.widget,
        hash,
        record.caps.join(","),
        record.scope.to_json(),
        record.sending
    );
    let row = grants::put(&conn, &key, record, hash, caller_kind(caller))
        .map_err(|e| ZenError::Io(format!("zen_widget_grants: {e}")))?;
    let pruned = grants::prune(&conn, |r| {
        placements.iter().any(|p| {
            p.garden == r.record.garden
                && p.placement == r.record.placement
                && (p.widget == r.record.widget || grants::carries(&r.record.widget, &p.widget))
        })
    })
    .map_err(|e| ZenError::Io(format!("zen_widget_grants: {e}")))?;
    drop(conn);
    audit("zen.widget.grant", caller, format!("{summary} pruned={pruned}"), ingress);
    let r = &row.record;
    let requested = f.widget_status(&r.widget).requested();
    let q = grants::GrantQuery {
        garden: &r.garden,
        placement: &r.placement,
        widget: &r.widget,
        hash,
        requested: &requested,
        ask: &r.ask,
    };
    Ok(grants::effective(Some(&row), Some(&key), &q))
}

/// A custom placement on a Garden's live page: `(garden id, widget, ask)`.
fn find_placement(f: &ZenFiles, garden: &str, placement: &str) -> Result<(String, String, grants::PlacementAsk), ZenError> {
    let (_, g) = f.garden(garden)?;
    let p = f
        .custom_placements()
        .into_iter()
        .find(|p| p.garden == g.id && p.placement == placement)
        .ok_or_else(|| ZenError::NotFound(format!("Garden '{}' has no custom widget '{placement}'", g.name)))?;
    Ok((g.id, p.widget, p.ask))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantBody {
    garden: String,
    placement: String,
    widget: String,
    caps: Vec<String>,
    scope: J,
    entries: Vec<GrantEntry>,
    #[serde(default = "yes")]
    sending: bool,
    hash: String,
}

/// POST `/cli/zen/widget/grant` (UW35 as amended by UWB4–UWB7): owner
/// token only (checked in [`handle`]).
fn handle_widget_grant(body: &[u8], caller: &Caller, ingress: &str) -> Result<J, ZenError> {
    let b: GrantBody =
        parse_body(body, "widget/grant takes {garden, placement, widget, caps, scope, entries, sending?, hash}")?;
    let f = files();
    set_up(&f)?;
    let (garden, widget, ask) = find_placement(&f, &b.garden, &b.placement)?;
    if widget != b.widget {
        return Err(ZenError::BadRequest(format!(
            "placement '{}' shows '{widget}', not '{}'",
            b.placement, b.widget
        )));
    }
    let st = f.widget_bundle(&widget)?;
    if st.hash() != b.hash {
        return Err(ZenError::WidgetChanged(
            "The widget changed while you were reviewing it. Review it again.".into(),
        ));
    }
    let requested = st.requested();
    let mut seen: Vec<&str> = Vec::new();
    for c in &b.caps {
        if !k2_core::zen::USER_WIDGET_CAPS.contains(&c.as_str()) {
            return Err(ZenError::BadRequest(format!("'{c}' isn't available to custom widgets")));
        }
        if !requested.contains(c) {
            return Err(ZenError::BadRequest(format!("this widget doesn't ask for '{c}'")));
        }
        if seen.contains(&c.as_str()) {
            return Err(ZenError::BadRequest(format!("'{c}' is listed twice")));
        }
        seen.push(c);
    }
    if b.caps.is_empty() {
        return Err(ZenError::BadRequest("caps is empty: there is nothing to allow".into()));
    }
    let scope = Scope::from_json(&b.scope).map_err(ZenError::BadRequest)?;
    check_entries(&b.entries, &scope, &ask, 1)?;
    let record = GrantRecord {
        garden,
        placement: b.placement,
        widget,
        caps: b.caps,
        scope,
        entries: b.entries,
        ask,
        sending: b.sending,
        granted_at: now(),
    };
    let view = store_grant(&f, record, &b.hash, caller, ingress)?;
    let changed = refresh_and_emit()?;
    Ok(json!({ "ok": true, "grant": view, "changed": changed }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RevokeBody {
    garden: Option<String>,
    placement: Option<String>,
    widget: Option<String>,
}

/// POST `/cli/zen/widget/revoke` (UW25): taking power away is always
/// allowed to anything Zen accepts.
fn handle_widget_revoke(body: &[u8], caller: &Caller, ingress: &str) -> Result<J, ZenError> {
    let b: RevokeBody = parse_body(body, "widget/revoke takes {garden, placement} or {widget}")?;
    let f = files();
    set_up(&f)?;
    let db = k2_core::db::try_shared().ok_or_else(|| ZenError::Io("K2's database isn't open".into()))?;
    let revoked: Vec<(String, String)> = match (nonempty(&b.garden), nonempty(&b.placement), nonempty(&b.widget)) {
        (Some(g), Some(p), None) => {
            let id = f.find_garden(&g).map(|(_, e)| e.id).unwrap_or(g);
            let conn = db.lock();
            let gone = grants::revoke(&conn, &id, &p, caller_kind(caller))
                .map_err(|e| ZenError::Io(format!("zen_widget_grants: {e}")))?;
            if gone { vec![(id, p)] } else { Vec::new() }
        }
        (None, None, Some(w)) => {
            let conn = db.lock();
            grants::revoke_widget(&conn, &w, caller_kind(caller)).map_err(|e| ZenError::Io(format!("zen_widget_grants: {e}")))?
        }
        _ => return Err(ZenError::BadRequest("widget/revoke takes {garden, placement} or {widget}".into())),
    };
    let list: Vec<J> = revoked.iter().map(|(g, p)| json!({ "garden": g, "placement": p })).collect();
    audit("zen.widget.revoke", caller, format!("revoked {}", J::Array(list.clone())), ingress);
    let changed = if revoked.is_empty() { false } else { refresh_and_emit()? };
    Ok(json!({ "ok": true, "revoked": list, "changed": changed }))
}

/// A live row that checks out under the key, or why not.
fn valid_row(f: &ZenFiles, garden: &str, placement: &str) -> Result<(String, GrantKey), ZenError> {
    let id = f.garden(garden)?.1.id;
    let key = GrantKey::load(&key_path(f))
        .ok()
        .flatten()
        .ok_or_else(|| ZenError::WidgetChanged("This grant doesn't check out; allow the widget again.".into()))?;
    let db = k2_core::db::try_shared().ok_or_else(|| ZenError::Io("K2's database isn't open".into()))?;
    let conn = db.lock();
    let row = grants::load_row(&conn, &id, placement)
        .map_err(|e| ZenError::Io(format!("zen_widget_grants: {e}")))?
        .ok_or_else(|| ZenError::NotFound(format!("no grant for '{placement}' in this Garden")))?;
    if row.key_id != key.id() || !grants::verify(&key, &row.record, &row.sig) {
        return Err(ZenError::WidgetChanged("This grant doesn't check out; allow the widget again.".into()));
    }
    Ok((id, key))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SendingBody {
    garden: String,
    placement: String,
    on: bool,
    reason: Option<String>,
}

/// POST `/cli/zen/widget/sending` (UWB9): off is allowed like revoke (the
/// runaway guard sends `reason: "runaway"`, which also pauses the widget);
/// on is a grant, owner token only (checked in [`handle`]).
fn handle_widget_sending(body: &[u8], caller: &Caller, ingress: &str) -> Result<J, ZenError> {
    let b: SendingBody = parse_body(body, "widget/sending takes {garden, placement, on, reason?}")?;
    let pause = match b.reason.as_deref() {
        None | Some("user") => None,
        Some("runaway") if !b.on => Some(Pause { at: now(), reason: PauseReason::Runaway }),
        Some("runaway") => return Err(ZenError::BadRequest("reason runaway goes with on: false".into())),
        Some(other) => return Err(ZenError::BadRequest(format!("unknown reason '{other}'; use runaway or user"))),
    };
    let f = files();
    set_up(&f)?;
    let (id, key) = valid_row(&f, &b.garden, &b.placement)?;
    let db = k2_core::db::try_shared().ok_or_else(|| ZenError::Io("K2's database isn't open".into()))?;
    let conn = db.lock();
    grants::set_sending(&conn, &key, &id, &b.placement, b.on, pause.clone())
        .map_err(|e| ZenError::Io(format!("zen_widget_grants: {e}")))?;
    drop(conn);
    audit(
        "zen.widget.sending",
        caller,
        format!("{id}/{} sending={} paused={}", b.placement, b.on, pause.is_some()),
        ingress,
    );
    let changed = refresh_and_emit()?;
    Ok(json!({ "ok": true, "sending": b.on, "paused": pause, "changed": changed }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResumeBody {
    garden: String,
    placement: String,
}

/// POST `/cli/zen/widget/resume` (UWB9): clear a runaway pause and turn
/// Sending back on. Owner token only (checked in [`handle`]).
fn handle_widget_resume(body: &[u8], caller: &Caller, ingress: &str) -> Result<J, ZenError> {
    let b: ResumeBody = parse_body(body, "widget/resume takes {garden, placement}")?;
    let f = files();
    set_up(&f)?;
    let (id, key) = valid_row(&f, &b.garden, &b.placement)?;
    let db = k2_core::db::try_shared().ok_or_else(|| ZenError::Io("K2's database isn't open".into()))?;
    let conn = db.lock();
    grants::set_sending(&conn, &key, &id, &b.placement, true, None)
        .map_err(|e| ZenError::Io(format!("zen_widget_grants: {e}")))?;
    drop(conn);
    audit("zen.widget.resume", caller, format!("{id}/{} resumed", b.placement), ingress);
    let changed = refresh_and_emit()?;
    Ok(json!({ "ok": true, "sending": true, "paused": J::Null, "changed": changed }))
}

/// GET `/cli/zen/widget/grants` (UWB3c): every live grant whose placement
/// is still on a Garden, with its state now (the Settings list).
fn handle_widget_grants() -> Result<J, ZenError> {
    let f = files();
    set_up(&f)?;
    let snap = grant_snapshot(&f)?;
    let placements = f.custom_placements();
    let rows: Vec<J> = snap
        .rows
        .iter()
        .filter_map(|r| {
            let p = placements.iter().find(|p| p.garden == r.record.garden && p.placement == r.record.placement)?;
            let st = f.widget_status(&p.widget);
            let requested = st.requested();
            let q = grants::GrantQuery {
                garden: &p.garden,
                placement: &p.placement,
                widget: &p.widget,
                hash: st.hash(),
                requested: &requested,
                ask: &p.ask,
            };
            let v = snap.view(&q);
            Some(json!({
                "garden": p.garden,
                "gardenName": p.garden_name,
                "placement": p.placement,
                "widget": p.widget,
                "state": v.state,
                "caps": v.caps,
                "scope": r.record.scope,
                "entries": r.record.entries,
                "sending": r.record.sending,
                "paused": r.paused,
                "grantedAt": r.record.granted_at,
            }))
        })
        .collect();
    Ok(json!({ "ok": true, "grants": rows }))
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

/// Requests that give a widget power (UWB3): only the owner token.
fn gives_power(path: &str, body: &[u8]) -> bool {
    match path {
        "/cli/zen/widget/grant" | "/cli/zen/widget/resume" => true,
        "/cli/zen/widget/sending" => body_json(body).ok().is_none_or(|v| v["on"] != J::Bool(false)),
        "/cli/zen/garden/new" => body_json(body).ok().is_some_and(|v| v.get("grant").is_some_and(|g| !g.is_null())),
        _ => false,
    }
}

const OWNER_ONLY: &str = "Only you can allow a widget, in the K2 app: open the Garden and click Review. Agents, K2 terminals, Connect logins and app passes can't.";

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
/// the owner-only grant rule and the audit line (UWB3). The dispatcher has
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
    if gives_power(path, body) && !owner_token {
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
            set_up(&f).and_then(|_| refresh_and_emit()).and_then(|_| grant_snapshot(&f)).and_then(|s| f.widgets_json(&s))
        }
        "/cli/zen/widget/bundle" => {
            let f = files();
            set_up(&f).and_then(|_| refresh_and_emit()).and_then(|_| handle_widget_bundle(params))
        }
        "/cli/zen/widget/grants" => handle_widget_grants(),
        "/cli/zen/widget/new" => handle_widget_new(body),
        "/cli/zen/widget/grant" => handle_widget_grant(body, caller, ingress),
        "/cli/zen/widget/revoke" => handle_widget_revoke(body, caller, ingress),
        "/cli/zen/widget/sending" => handle_widget_sending(body, caller, ingress),
        "/cli/zen/widget/resume" => handle_widget_resume(body, caller, ingress),
        "/cli/zen/theme/list" => refuse_home(params).and_then(|_| files().theme_list(param(params, "garden"))),
        "/cli/zen/theme/set" | "/cli/zen/theme/next" | "/cli/zen/theme/prev" => handle_theme_switch(path, body),
        "/cli/zen/theme/new" => handle_theme_new(body),
        "/cli/zen/status" => Ok(handle_status()),
        "/cli/zen/doctor" => Ok(handle_doctor()),
        "/cli/zen/setup" => handle_setup(body),
        "/cli/zen/garden/new" => handle_garden_new(body, caller, ingress),
        "/cli/zen/garden/rename" => handle_garden_rename(body),
        "/cli/zen/garden/delete" => handle_garden_delete(body),
        "/cli/zen/garden/reorder" => handle_garden_reorder(body),
        "/cli/zen/garden/template" => handle_garden_template(body),
        "/cli/zen/reload" => handle_reload(),
        "/cli/zen/reset" => handle_reset(body),
        _ => unreachable!("checked against GET_ROUTES/POST_ROUTES above"),
    };
    match r {
        Ok(v) => ok(v),
        Err(e) => err(e),
    }
}
