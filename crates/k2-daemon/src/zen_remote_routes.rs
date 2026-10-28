//! `/cli/zen/remote/*`: the remote Garden door (prd-gardens-on-phone-v1
//! §3, slices GP1 + GP2; vs-live GP-A1…GP-A6, GP-A22; phone contract
//! `contract-gardens-phone-v1.md`).
//!
//! Every other `/cli/zen/*` route answers only this computer's owner token
//! (`zen_routes`). This door is the one way a **paired phone** (K2
//! Companion) reaches the Gardens, and it is open to exactly one person:
//!
//! - **Who** ([`gate`]): the owner token (K2's harness and the CLI), or a
//!   live **device session** (`connect_users::validate_device_session`)
//!   whose login is the **owner person**: a login the owner linked
//!   (`zen_routes::login_is_owner_person`, the GP-B1 rule). The owner
//!   person on a password login (the web, another server's K2) gets 403
//!   `garden_device_only`; anyone else (another login of any role, an agent
//!   or K2 terminal passport, an App pass, an API key) gets 403
//!   `garden_person_only`. An ended device session already got 403
//!   `device_session_ended` from the dispatcher before this module ran.
//! - **What** (v1, 0.46.6): read the Garden list ([`GARDENS`], the Homes
//!   sync pull shape), a page ([`PAGE`], `GET /cli/zen/get`'s answer
//!   without `sources`, plus how the phone runs each placement), the sealed
//!   document per custom placement ([`FRAME`], built by the same Rust
//!   wrapper as the headless shot, `k2_core::zen::widget_shot`), download
//!   and CDN libraries ([`LIB`]), and run a widget's data verbs ([`CALL`],
//!   [`BLOB_PUT`], [`BLOB_GET`]) with the daemon's own checks (placement,
//!   live hash, caps, the signer gate, pause, budgets) through the same
//!   runner as `/cli/zen/call`. Reports, the runaway pause, the person's
//!   Resume and the Agents widget's order (read-only) round it out.
//! - **Never**: layout edits, theme picks, renames, the lock, Garden files,
//!   agent order changes; `later` / `never` rows of the catalog's `phone`
//!   column (`k2_core::contract::phone`).
//!
//! **One envelope** (GP-A3): every refusal is `{ok: false, error: {code,
//! hint, …}}`. On [`CALL`] a refusal's `code` is always a catalog error
//! code (the closed list), so the phone hands it to the frame as is.
//!
//! **Audit** (GP-A22): every request writes one `zen.remote.*` line with
//! `client=phone` (or `owner-token`), the login and the device id; never
//! args or values.
//!
//! **No secrets**: no answer carries a credential, a path or any of the
//! catalog's `bannedFields`; the page drops `sources` (file names); the
//! frame gets no token (it never talks to the daemon, the phone does).
//!
//! Routing: one dispatcher arm ahead of the generic Zen arm, POST rows 405
//! a GET, policy rows are Member (so `role_gate` lets every credential
//! through to [`gate`], as Zen rows do). No migration; no new event: the
//! phone re-reads on the app socket's `zen_changed`,
//! `zen_agents_order_changed` and `garden_data_changed` (§3.3).

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{json, Value as J};

use k2_core::contract::{catalog, phone, Phone};
use k2_core::zen::stdlib::{self, LibRef, LibSource};
use k2_core::zen::widget_shot::{self, Scheme};
use k2_core::zen::{ZenError, ZenFiles};

use crate::cli_response::CliResponse;
use crate::garden_data::call::CallBody;

pub const GARDENS: &str = "/cli/zen/remote/gardens";
pub const PAGE: &str = "/cli/zen/remote/page";
pub const FRAME: &str = "/cli/zen/remote/frame";
pub const LIB: &str = "/cli/zen/remote/lib";
pub const LIB_FETCH: &str = "/cli/zen/remote/lib/fetch";
pub const CALL: &str = "/cli/zen/remote/call";
pub const BLOB_PUT: &str = "/cli/zen/remote/blob/put";
pub const BLOB_GET: &str = "/cli/zen/remote/blob/get";
pub const REPORT: &str = "/cli/zen/remote/report";
pub const PAUSE: &str = "/cli/zen/remote/pause";
pub const RESUME: &str = "/cli/zen/remote/resume";
pub const AGENTS_ORDER: &str = "/cli/zen/remote/agents-order";

/// The POST rows: a GET is 405.
pub const POST_ROUTES: &[&str] = &[BLOB_PUT, CALL, LIB_FETCH, PAUSE, REPORT, RESUME];
/// The GET rows.
pub const GET_ROUTES: &[&str] = &[AGENTS_ORDER, BLOB_GET, FRAME, GARDENS, LIB, PAGE];

/// `/boot-status` `features`: a phone draws Gardens only for a server that
/// lists it.
pub const FEATURE: &str = "gardens-remote-v1";

/// Request bodies: 64 KiB, except a blob chunk (the widget door's own cap).
pub const MAX_BODY: usize = 64 * 1024;

pub fn is_route(path: &str) -> bool {
    POST_ROUTES.contains(&path) || GET_ROUTES.contains(&path)
}

pub fn is_post_route(path: &str) -> bool {
    POST_ROUTES.contains(&path)
}

pub fn max_body(path: &str) -> usize {
    if path == BLOB_PUT {
        crate::zen_data_routes::max_body(crate::garden_data::blob_routes::WIDGET_PUT)
    } else {
        MAX_BODY
    }
}

// ── who ─────────────────────────────────────────────────────────────

/// Who presented what, as this door sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteWho {
    /// This daemon's owner token (K2's tests, the CLI).
    Owner,
    /// A live device session: a paired phone.
    Phone { login: String, device: String },
    /// A live Connect login that is not a device session (a password login:
    /// the web client, another server's K2, a script).
    Login { login: String },
    /// Anything else that isn't a person on a phone: an agent or K2 terminal
    /// passport, an App pass, an API key.
    Other { kind: &'static str },
    /// No valid credential.
    Invalid,
}

/// Classify the request's credential (query `token=` first, then Bearer,
/// as every route reads it).
pub fn who_from_tcp(query: &str, bearer: Option<&str>, owner_token: &str) -> RemoteWho {
    let presented = crate::routes::http::extract_token(query)
        .filter(|s| !s.is_empty())
        .or_else(|| bearer.map(str::trim).filter(|s| !s.is_empty()))
        .unwrap_or("");
    who_of(presented, owner_token)
}

/// [`who_from_tcp`] for one presented credential.
pub fn who_of(presented: &str, owner_token: &str) -> RemoteWho {
    if presented.is_empty() {
        return RemoteWho::Invalid;
    }
    if crate::routes::http::ct_eq_token(presented, owner_token) {
        return RemoteWho::Owner;
    }
    if k2_core::skin::is_skin_token(presented) {
        return RemoteWho::Other { kind: "app-pass" };
    }
    if presented.starts_with("k2sk_") {
        return RemoteWho::Other { kind: "api-key" };
    }
    if let Some((login, _, device)) = k2_core::connect_users::validate_device_session_device(presented) {
        return RemoteWho::Phone { login, device };
    }
    if let Some(login) = k2_core::connect_users::validate_session(presented) {
        return RemoteWho::Login { login };
    }
    if crate::session_token::validate_hook(presented).is_some() {
        return RemoteWho::Other { kind: "passport" };
    }
    RemoteWho::Invalid
}

/// The person the door answers, after [`gate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    /// `phone` or `owner-token`.
    pub client: &'static str,
    /// The login (`owner` for the owner token).
    pub login: String,
    /// The phone's install id (`-` for the owner token).
    pub device: String,
}

/// The door's one rule (§3.1, GP-A1, GP-A2): the owner token, or a paired
/// phone of the owner person. Checked on every request (a device session
/// can end, and a link can be removed, between two calls).
pub fn gate(who: &RemoteWho) -> Result<Person, CliResponse> {
    match who {
        RemoteWho::Owner => Ok(Person { client: "owner-token", login: "owner".into(), device: "-".into() }),
        RemoteWho::Phone { login, device } if crate::zen_routes::login_is_owner_person(login) => {
            Ok(Person { client: "phone", login: login.clone(), device: device.clone() })
        }
        RemoteWho::Login { login } if crate::zen_routes::login_is_owner_person(login) => Err(refuse(
            "403 Forbidden",
            "garden_device_only",
            "Gardens open on a phone you paired with this server (Settings → Admin Access → Pair a phone), not on a password sign-in.",
        )),
        _ => Err(refuse(
            "403 Forbidden",
            "garden_person_only",
            "Gardens on a phone are only for this server's owner, on a phone they paired.",
        )),
    }
}

fn who_label(who: &RemoteWho) -> String {
    match who {
        RemoteWho::Owner => "owner".into(),
        RemoteWho::Phone { login, .. } | RemoteWho::Login { login } => login.clone(),
        RemoteWho::Other { kind } => (*kind).to_string(),
        RemoteWho::Invalid => "-".into(),
    }
}

// ── envelope and audit ──────────────────────────────────────────────

/// `{ok: false, error: {code, hint}}`.
pub fn refuse(status: &'static str, code: &str, hint: impl Into<String>) -> CliResponse {
    let hint: String = hint.into();
    CliResponse { status, content_type: "application/json", body: json!({ "ok": false, "error": { "code": code, "hint": hint } }).to_string() }
}

fn ok(body: J) -> CliResponse {
    CliResponse { status: "200 OK", content_type: "application/json", body: body.to_string() }
}

/// Any answer from a reused handler in the door's envelope (GP-A3): Zen's
/// `{error: "<code>", message, …extra}` and Garden data's `{error: {code,
/// hint, …}}` both become `{ok: false, error: {code, hint, …extra}}`. A 2xx
/// answer passes unchanged.
pub fn envelope(r: CliResponse) -> CliResponse {
    if r.status.starts_with('2') {
        return r;
    }
    let v: J = serde_json::from_str(&r.body).unwrap_or_else(|_| json!({ "error": "failed", "message": r.body.clone() }));
    let mut e = serde_json::Map::new();
    match &v["error"] {
        J::Object(o) => {
            e.extend(o.clone());
        }
        J::String(code) => {
            e.insert("code".into(), json!(code));
            e.insert("hint".into(), v["message"].clone());
            if let Some(o) = v.as_object() {
                for (k, x) in o {
                    if !matches!(k.as_str(), "ok" | "error" | "message") {
                        e.insert(k.clone(), x.clone());
                    }
                }
            }
        }
        _ => {
            e.insert("code".into(), json!("failed"));
            e.insert("hint".into(), json!("K2 couldn't answer this"));
        }
    }
    if !e.contains_key("hint") || e["hint"].is_null() {
        e.insert("hint".into(), json!(""));
    }
    CliResponse { status: r.status, content_type: "application/json", body: json!({ "ok": false, "error": J::Object(e) }).to_string() }
}

fn zen_err(e: ZenError) -> CliResponse {
    envelope(crate::zen_routes::error_response(e))
}

/// One audit line per request (GP-A22): who, from where; never args or
/// values.
fn audit(event: &str, p: &Person, outcome: String) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        event,
        &p.login,
        format!("client={} device={} {outcome}", p.client, p.device),
        "remote-garden",
        "-",
        p.client,
    ));
}

// ── the door ────────────────────────────────────────────────────────

/// One `/cli/zen/remote/*` request. The transport already 405'd a GET on a
/// POST row and refused an oversized body.
pub fn handle(path: &str, who: &RemoteWho, params: &HashMap<String, String>, body: &[u8]) -> CliResponse {
    let person = match gate(who) {
        Ok(p) => p,
        Err(r) => {
            k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
                "zen.remote.refused",
                &who_label(who),
                format!("{path}: {}", serde_json::from_str::<J>(&r.body).ok().and_then(|v| v["error"]["code"].as_str().map(str::to_string)).unwrap_or_default()),
                "remote-garden",
                "-",
                "cli",
            ));
            return r;
        }
    };
    match path {
        GARDENS => gardens(&person, params),
        PAGE => page(&person, params),
        FRAME => frame(&person, params),
        LIB | LIB_FETCH => lib(&person, path, params, body),
        CALL => call(&person, body),
        BLOB_PUT | BLOB_GET => blob(&person, path, params, body),
        REPORT => report(&person, body),
        PAUSE => pause(&person, body),
        RESUME => resume(&person, body),
        AGENTS_ORDER => agents_order(&person, params),
        _ => refuse("404 Not Found", "not_found", format!("no remote Garden route {path}")),
    }
}

fn param<'a>(params: &'a HashMap<String, String>, k: &str) -> Option<&'a str> {
    params.get(k).map(|s| s.trim()).filter(|s| !s.is_empty())
}

fn need<'a>(params: &'a HashMap<String, String>, k: &str) -> Result<&'a str, CliResponse> {
    param(params, k).ok_or_else(|| refuse("400 Bad Request", "usage", format!("{k} is required")))
}

fn scheme_param(params: &HashMap<String, String>) -> Result<Option<Scheme>, CliResponse> {
    match param(params, "scheme") {
        None => Ok(None),
        Some(s) => Scheme::parse(s).map(Some).ok_or_else(|| refuse("400 Bad Request", "usage", "scheme is light or dark")),
    }
}

fn files() -> ZenFiles {
    ZenFiles::local()
}

/// Refresh first (a save the watcher hasn't debounced yet is live, and
/// announced once), as `GET /cli/zen/get` does.
fn refreshed() -> Result<ZenFiles, CliResponse> {
    let f = files();
    if f.is_set_up() {
        crate::zen_routes::refresh_and_emit().map_err(zen_err)?;
    }
    Ok(f)
}

/// The page answer for `garden` (an id or a name), with the daemon's pauses.
fn resolved(f: &ZenFiles, garden: &str) -> Result<J, CliResponse> {
    if !f.is_set_up() {
        return Err(zen_err(ZenError::NotSetUp));
    }
    f.resolve_with(Some(garden), &crate::zen_routes::current_pauses()).map_err(zen_err)
}

// ── GET gardens ─────────────────────────────────────────────────────

/// The feed's cursor for a list: a hash of what the phone shows (order,
/// names, templates, themes, locks and each page's version).
fn cursor_of(gardens: &[J]) -> String {
    format!("g-{}", k2_core::zen::schema::fnv_hex(J::Array(gardens.to_vec()).to_string().as_bytes()))
}

/// `GET /cli/zen/remote/gardens[?since=<cursor>]` (§3.2): the whole list
/// (`full: true`), or nothing when `since` is the current cursor.
fn gardens(p: &Person, params: &HashMap<String, String>) -> CliResponse {
    let f = match refreshed() {
        Ok(f) => f,
        Err(r) => return r,
    };
    let set_up = f.is_set_up();
    let mut list = Vec::new();
    if set_up {
        let pauses = crate::zen_routes::current_pauses();
        for g in f.gardens_json() {
            let id = g["id"].as_str().unwrap_or_default().to_string();
            let version = match f.resolve_with(Some(&id), &pauses) {
                Ok(v) => v["version"].clone(),
                Err(e) => return zen_err(e),
            };
            list.push(json!({
                "id": id,
                "name": g["name"],
                "index": g["index"],
                "template": g["template"],
                "theme": g["theme"],
                "locked": g["locked"],
                "version": version,
            }));
        }
    }
    let cursor = cursor_of(&list);
    let full = param(params, "since") != Some(cursor.as_str());
    audit("zen.remote.gardens", p, format!("gardens={} full={full}", list.len()));
    ok(json!({
        "ok": true,
        "serverId": crate::boot_status::server_id(),
        "person": "owner",
        "setUp": set_up,
        "cursor": cursor,
        "full": full,
        "gardens": if full { list } else { Vec::new() },
    }))
}

// ── GET page ────────────────────────────────────────────────────────

/// How the phone runs one placement (§4.1).
fn run_of(kind: &str) -> (&'static str, Option<&'static str>) {
    match kind {
        "custom" => ("frame", None),
        "agents" | "conversation" | "garden-empty" => ("native", None),
        _ => ("placeholder", Some("desktop_only")),
    }
}

/// The hello's `features` on a phone (GP-A5): the desktop frame's, minus
/// every feature whose rows are all `later`/`never` on a phone.
pub fn phone_features() -> Vec<&'static str> {
    phone::phone_features(catalog(), widget_shot::FRAME_FEATURES)
}

/// `GET /cli/zen/remote/page?garden=<id>[&scheme=]` (§3.2): `GET
/// /cli/zen/get`'s answer for that Garden without `sources` (file names),
/// plus `remote {widgets: [{id, run, reason?}], features}`. The page's
/// `theme.tokens` carries both schemes; `scheme` is checked and only
/// matters to the frame.
fn page(p: &Person, params: &HashMap<String, String>) -> CliResponse {
    let garden = match need(params, "garden") {
        Ok(g) => g,
        Err(r) => return r,
    };
    if let Err(r) = scheme_param(params) {
        return r;
    }
    let f = match refreshed() {
        Ok(f) => f,
        Err(r) => return r,
    };
    let mut v = match resolved(&f, garden) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Some(o) = v.as_object_mut() {
        o.remove("sources");
    }
    let widgets: Vec<J> = v["page"]["widgets"]
        .as_array()
        .map(|ws| {
            ws.iter()
                .map(|w| {
                    let (run, reason) = run_of(w["kind"].as_str().unwrap_or_default());
                    let mut row = json!({ "id": w["id"], "run": run });
                    if let Some(r) = reason {
                        row["reason"] = json!(r);
                    }
                    row
                })
                .collect()
        })
        .unwrap_or_default();
    v["remote"] = json!({ "widgets": widgets, "features": phone_features() });
    audit("zen.remote.page", p, format!("garden={}", v["garden"]["id"].as_str().unwrap_or(garden)));
    ok(v)
}

// ── GET frame ───────────────────────────────────────────────────────

/// One library slot's row (§4.2, contract §4.2).
fn lib_row(n: usize, key: &J) -> Result<J, String> {
    let slot = widget_shot::remote_lib_slot(n);
    match LibRef::from_json(key)? {
        LibRef::Cdn { url, integrity } => Ok(json!({
            "slot": slot,
            "key": url,
            "source": "cdn",
            "url": url,
            "integrity": integrity,
        })),
        r @ LibRef::Named { .. } => {
            let m = stdlib::zen_lib_manifest();
            let e = stdlib::find(m, &r).ok_or_else(|| format!("no library {key} in K2"))?;
            Ok(json!({
                "slot": slot,
                "key": e.key(),
                "id": e.id,
                "version": e.version,
                "source": match e.source { LibSource::Bundled => "bundled", LibSource::Download => "download" },
                "files": e.files.iter().map(|f| json!({ "name": f.name, "sha256": f.sha256, "bytes": f.bytes })).collect::<Vec<_>>(),
            }))
        }
    }
}

/// `GET /cli/zen/remote/frame?garden=&placement=[&scheme=]` (§3.2, §4.2,
/// GP-A5): the sealed document for one custom placement, built by the
/// desktop wrapper's Rust twin, with one `__K2_LIB_<n>__` slot per library,
/// and the hello the phone posts with the port.
fn frame(p: &Person, params: &HashMap<String, String>) -> CliResponse {
    let (garden, placement) = match (need(params, "garden"), need(params, "placement")) {
        (Ok(g), Ok(pl)) => (g, pl),
        (Err(r), _) | (_, Err(r)) => return r,
    };
    let want = match scheme_param(params) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let f = match refreshed() {
        Ok(f) => f,
        Err(r) => return r,
    };
    let v = match resolved(&f, garden) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let gid = v["garden"]["id"].as_str().unwrap_or_default().to_string();
    let Some(w) = v["page"]["widgets"].as_array().and_then(|ws| ws.iter().find(|w| w["id"] == placement)).cloned() else {
        return refuse("404 Not Found", "unknown_placement", format!("no placement {placement} on this Garden"));
    };
    if w["kind"] != "custom" {
        let (run, _) = run_of(w["kind"].as_str().unwrap_or_default());
        return refuse("409 Conflict", "not_a_frame", format!("{placement} is drawn by the phone ({run}), not a frame"));
    }
    let widget = w["widget"].as_str().unwrap_or_default().to_string();
    let st = match f.widget_bundle_in(Some(&gid), &widget) {
        Ok(st) => st,
        Err(e) => return zen_err(e),
    };
    let (Some(bundle), Some(manifest)) = (st.bundle.as_ref(), st.manifest.as_ref()) else {
        return refuse("409 Conflict", "widget_broken", format!("{widget} has no good version on the computer"));
    };
    if w["hash"].as_str() != Some(bundle.hash.as_str()) {
        return refuse("409 Conflict", "stale_widget", "the widget changed while its page was read; read the page again");
    }
    // The libraries: exactly the page's resolved `libKeys` (0.46.5 WL7).
    let keys: Vec<J> = match w["libKeys"].as_array() {
        Some(k) => k.clone(),
        None => {
            let r = stdlib::resolve(stdlib::zen_lib_manifest(), &manifest.libs);
            if let Some(e) = r.errors().next() {
                return refuse("409 Conflict", "widget_broken", e.message.clone());
            }
            r.keys
        }
    };
    let mut libs = Vec::new();
    for (n, k) in keys.iter().enumerate() {
        match lib_row(n, k) {
            Ok(row) => libs.push(row),
            Err(m) => return refuse("409 Conflict", "widget_broken", m),
        }
    }
    let (vars, scheme) = widget_shot::page_frame_vars(&v["theme"], want);
    let own = w["colorScheme"] == "own";
    let nonce = k2_core::zen::bundle::new_nonce();
    let srcdoc = widget_shot::remote_frame_doc(&bundle.render(&nonce), &nonce, &vars, scheme, libs.len(), own);
    let hello = json!({
        "k2": "hello",
        "v": 1,
        "caps": w["caps"],
        "features": phone_features(),
        "widget": { "id": placement, "name": w["name"], "garden": gid },
        "config": w["props"].get("config").cloned().unwrap_or_else(|| json!({})),
        "motion": { "reduced": v["motion"]["reduced"].as_bool().unwrap_or(false) },
    });
    audit("zen.remote.frame", p, format!("garden={gid} placement={placement} widget={widget} hash={}", &bundle.hash[..bundle.hash.len().min(12)]));
    ok(json!({
        "ok": true,
        "garden": gid,
        "placement": placement,
        "widget": widget,
        "hash": bundle.hash,
        "nonce": nonce,
        "scheme": scheme.as_str(),
        "srcdoc": srcdoc,
        "libs": libs,
        "hello": hello,
    }))
}

// ── libraries ───────────────────────────────────────────────────────

/// `GET /cli/zen/remote/lib?id=&version=&file=` (or `?integrity=` for a
/// CDN file) and `POST /cli/zen/remote/lib/fetch {id, version} | {url,
/// integrity}` (GP-A6): the daemon's checked download/CDN cache, the same
/// as `/cli/zen/lib/*`. A bundled library is the phone's own copy: 404
/// `bundled_on_phone`.
fn lib(p: &Person, path: &str, params: &HashMap<String, String>, body: &[u8]) -> CliResponse {
    let named = if path == LIB {
        param(params, "id").zip(param(params, "version")).map(|(i, v)| (i.to_string(), v.to_string()))
    } else {
        serde_json::from_slice::<J>(body).ok().and_then(|b| {
            Some((b["id"].as_str()?.to_string(), b["version"].as_str()?.to_string()))
        })
    };
    if let Some((id, version)) = &named {
        let r = LibRef::Named { id: id.clone(), version: version.clone() };
        if stdlib::find(stdlib::zen_lib_manifest(), &r).is_some_and(|e| e.source == LibSource::Bundled) {
            return refuse("404 Not Found", "bundled_on_phone", format!("{id}@{version} ships with K2 Companion; use the phone's own pinned copy"));
        }
    }
    let inner = if path == LIB { "/cli/zen/lib/file" } else { "/cli/zen/lib/fetch" };
    audit("zen.remote.lib", p, format!("{} {}", if path == LIB { "file" } else { "fetch" }, named.map(|(i, v)| format!("{i}@{v}")).unwrap_or_else(|| "cdn".into())));
    let env = crate::zen_lib_routes::Env::live();
    envelope(crate::zen_lib_routes::route(&env, inner, params, body))
}

// ── POST call ───────────────────────────────────────────────────────

/// Garden data refusal codes that aren't in the catalog's closed error
/// list, and the catalog code a frame gets for each (GP-A3).
fn catalog_code(code: &str) -> &str {
    if catalog().errors.iter().any(|e| e.code == code) {
        return code;
    }
    match code {
        "widget_not_found" | "unknown_garden" | "zen_not_set_up" | "route_not_found" => "not_found",
        "bad_request" => "bad_args",
        "body_too_large" => "too_large",
        "owner_only" | "zen_local_only" | "origin_refused" => "not_granted",
        _ => "failed",
    }
}

/// A refusal from the runner, with a catalog code (the runner's own code
/// kept as `cause` when it differs).
fn call_envelope(r: CliResponse) -> CliResponse {
    let r = envelope(r);
    if r.status.starts_with('2') {
        return r;
    }
    let mut v: J = serde_json::from_str(&r.body).unwrap_or_else(|_| json!({ "ok": false, "error": { "code": "failed" } }));
    let code = v["error"]["code"].as_str().unwrap_or("failed").to_string();
    let mapped = catalog_code(&code).to_string();
    if mapped != code {
        v["error"]["cause"] = json!(code);
        v["error"]["code"] = json!(mapped);
    }
    CliResponse { status: r.status, content_type: "application/json", body: v.to_string() }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoteCall {
    garden: String,
    placement: String,
    hash: String,
    verb: String,
    #[serde(default)]
    args: Vec<J>,
}

/// `POST /cli/zen/remote/call {garden, placement, hash, verb, args}` (§3.2,
/// §4.3): the catalog's `phone` column first (`door` runs; `later` →
/// `verb_unavailable`; `host`/`never`/unknown → `not_exposed` /
/// `unknown_verb`), then the same runner as `/cli/zen/call` (placement,
/// live hash → `stale_widget`, caps → `cap_not_granted`, the signer gate,
/// pause, budgets). Answers `{ok: true, result}` or the envelope.
fn call(p: &Person, body: &[u8]) -> CliResponse {
    let b: RemoteCall = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return refuse("400 Bad Request", "bad_args", format!("remote/call takes {{garden, placement, hash, verb, args}}: {e}")),
    };
    let c = catalog();
    let refusal = match (c.verb(&b.verb), phone::phone_of(c, &b.verb)) {
        (None, _) => Some(refuse("400 Bad Request", "unknown_verb", format!("no verb {}", b.verb))),
        (Some(_), None) | (Some(_), Some(Phone::Never)) => {
            Some(refuse("403 Forbidden", "not_exposed", format!("{} isn't available to a widget on a phone", b.verb)))
        }
        (Some(_), Some(Phone::Host)) => {
            Some(refuse("400 Bad Request", "not_exposed", format!("{} is answered by the phone itself, not the server", b.verb)))
        }
        (Some(row), Some(Phone::Later)) => {
            let mut r = refuse("501 Not Implemented", "verb_unavailable", format!("{} isn't on phones yet", b.verb));
            let mut v: J = serde_json::from_str(&r.body).unwrap_or_default();
            v["error"]["feature"] = json!(row.feature);
            r.body = v.to_string();
            Some(r)
        }
        (Some(_), Some(Phone::Door)) if phone::BYTE_VERBS.contains(&b.verb.as_str()) => Some(refuse(
            "400 Bad Request",
            "bad_args",
            format!("{} moves bytes: use /cli/zen/remote/blob/{}", b.verb, if b.verb == "blobs.put" { "put" } else { "get" }),
        )),
        (Some(_), Some(Phone::Door)) => None,
    };
    if let Some(r) = refusal {
        audit("zen.remote.call", p, format!("{}/{} verb={} refused", b.garden, b.placement, b.verb));
        return r;
    }
    audit("zen.remote.call", p, format!("{}/{} verb={}", b.garden, b.placement, b.verb));
    let cb = CallBody { garden: b.garden, placement: b.placement, hash: b.hash, verb: b.verb, args: b.args, confirm_token: None };
    call_envelope(crate::garden_data::call::run_call(&cb))
}

// ── blobs ───────────────────────────────────────────────────────────

/// `POST /cli/zen/remote/blob/put` and `GET /cli/zen/remote/blob/get`: the
/// widget door's blob routes after this door's gate (same bodies, same
/// checks).
fn blob(p: &Person, path: &str, params: &HashMap<String, String>, body: &[u8]) -> CliResponse {
    use crate::garden_data::blob_routes::{widget_routed, WIDGET_GET, WIDGET_PUT};
    let inner = if path == BLOB_PUT { WIDGET_PUT } else { WIDGET_GET };
    let (garden, placement) = if path == BLOB_PUT {
        let b: J = serde_json::from_slice(body).unwrap_or_default();
        (b["garden"].as_str().unwrap_or("-").to_string(), b["placement"].as_str().unwrap_or("-").to_string())
    } else {
        (param(params, "garden").unwrap_or("-").to_string(), param(params, "placement").unwrap_or("-").to_string())
    };
    audit("zen.remote.blob", p, format!("{garden}/{placement} {}", if path == BLOB_PUT { "put" } else { "get" }));
    match widget_routed(inner, body, params) {
        Ok(v) => ok(v),
        Err(e) => call_envelope(crate::garden_data::response(&e)),
    }
}

// ── report, pause, resume, agents order ─────────────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReportBody {
    garden: String,
    placement: String,
    hash: String,
    event: String,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    stack: Option<String>,
    #[serde(default)]
    verb: Option<String>,
    #[serde(default)]
    code: Option<String>,
}

/// `POST /cli/zen/remote/report {garden, placement, hash, event: ready |
/// error | refusal, message?, stack?, verb?, code?}`: the frame's run
/// report, logged against the live version like desktop's `widget/run`
/// (`k2 zen widget errors` shows phone problems too).
fn report(p: &Person, body: &[u8]) -> CliResponse {
    let b: ReportBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return refuse("400 Bad Request", "usage", format!("remote/report takes {{garden, placement, hash, event, message?, stack?, verb?, code?}}: {e}")),
    };
    let w = match crate::garden_data::blob_routes::placed(&b.garden, &b.placement, &b.hash, "report") {
        Ok(w) => w,
        Err(e) => return call_envelope(crate::garden_data::response(&e)),
    };
    let run = match b.event.as_str() {
        "ready" => json!({ "widget": w.widget, "hash": w.hash, "event": "ready" }),
        "error" => json!({ "widget": w.widget, "hash": w.hash, "event": "error", "message": b.message, "stack": b.stack }),
        "refusal" => json!({
            "widget": w.widget,
            "hash": w.hash,
            "event": "error",
            "kind": "refusal",
            "code": b.code,
            "message": b.verb.as_deref().map(|v| format!("{v} refused on the phone")).or(b.message.clone()),
        }),
        other => return refuse("400 Bad Request", "usage", format!("event is ready, error or refusal, not {other}")),
    };
    audit("zen.remote.report", p, format!("{}/{} event={}", w.garden, w.placement, b.event));
    let r = crate::zen_routes::handle("/cli/zen/widget/run", true, &crate::sidecar_routes::Caller::Owner, "remote-garden", &HashMap::new(), run.to_string().as_bytes());
    envelope(r)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PlacementBody {
    garden: String,
    placement: String,
    #[serde(default)]
    reason: Option<String>,
}

/// `POST /cli/zen/remote/pause {garden, placement, reason: "runaway"}`:
/// taking power away, open to anyone the door accepts (as `widget/pause`).
fn pause(p: &Person, body: &[u8]) -> CliResponse {
    let b: PlacementBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return refuse("400 Bad Request", "usage", format!("remote/pause takes {{garden, placement, reason}}: {e}")),
    };
    audit("zen.remote.pause", p, format!("{}/{}", b.garden, b.placement));
    let inner = json!({ "garden": b.garden, "placement": b.placement, "reason": b.reason.unwrap_or_else(|| "runaway".into()) });
    let r = crate::zen_routes::handle("/cli/zen/widget/pause", true, &crate::sidecar_routes::Caller::Owner, "remote-garden", &HashMap::new(), inner.to_string().as_bytes());
    envelope(r)
}

/// `POST /cli/zen/remote/resume {garden, placement}`: the person's tap on
/// "Paused: too many posts. Resume", through `widget/resume`'s own
/// Garden-owner check as the phone's login.
fn resume(p: &Person, body: &[u8]) -> CliResponse {
    let b: PlacementBody = match serde_json::from_slice::<PlacementBody>(body) {
        Ok(b) if b.reason.is_none() => b,
        Ok(_) => return refuse("400 Bad Request", "usage", "remote/resume takes {garden, placement}"),
        Err(e) => return refuse("400 Bad Request", "usage", format!("remote/resume takes {{garden, placement}}: {e}")),
    };
    audit("zen.remote.resume", p, format!("{}/{}", b.garden, b.placement));
    let (owner, caller) = if p.client == "owner-token" {
        (true, crate::sidecar_routes::Caller::Owner)
    } else {
        (false, crate::sidecar_routes::Caller::Login { username: p.login.clone() })
    };
    let inner = json!({ "garden": b.garden, "placement": b.placement });
    let r = crate::zen_routes::handle("/cli/zen/widget/resume", owner, &caller, "remote-garden", &HashMap::new(), inner.to_string().as_bytes());
    envelope(r)
}

/// `GET /cli/zen/remote/agents-order?garden=`: the person's order and pins
/// for the phone's native Agents rows, read-only (GP-A17).
fn agents_order(p: &Person, params: &HashMap<String, String>) -> CliResponse {
    let garden = match need(params, "garden") {
        Ok(g) => g,
        Err(r) => return r,
    };
    audit("zen.remote.agents_order", p, format!("garden={garden}"));
    match files().agents_order_json(Some(garden)) {
        Ok(v) => ok(v),
        Err(e) => zen_err(e),
    }
}

#[cfg(test)]
#[path = "zen_remote_routes_tests.rs"]
mod tests;
