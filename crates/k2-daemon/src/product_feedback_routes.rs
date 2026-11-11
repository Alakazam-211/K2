//! `/cli/product-feedback/*` — Send feedback to K2
//! (prd-k2-product-feedback-v1 rev 3 PF17, 0.46.11). Not the tickets
//! (`/cli/feedback/*`, whose POST prefix arm would catch a route under it).
//!
//! Routes (every mutating one POST-only: a GET is 405):
//! - `POST diagnostics` `{client?, page?, k2Version?, cliVersion?,
//!   includeClientFaults?}` → `{draftId, sections: [{id, title, text,
//!   bytes}], bytes, expiresAt}`: collects and scrubs the K2 details for
//!   the preview (PF26), kept an hour. An agent only while the owner
//!   turned on `agentDiagnostics` (PF10a).
//! - `POST send` `{category?, text, contactEmail?, client?, k2Version?,
//!   locale?, diagnostics?: {draftId, drop?, edited?}, screenshot?:
//!   {draftId, sha256}}` → `{id, state}`
//! - `POST screenshot` `{image, source, capture?}` → `{draftId, sha256,
//!   bytes, width, height, source, capture, expiresAt}`: a person's
//!   flattened PNG (base64, or a `data:image/png;base64,` URL; at most
//!   2 MiB, a 3 MiB body), checked, chunks cut, kept an hour as a draft
//!   (prd-feedback-screenshot-v1 §5.2). `GET screenshot?draftId=` → the
//!   stored bytes as `{dataUrl, sha256, bytes, width, height, source,
//!   capture}` (the picture the person confirms); `?id=pf_…` → a row's
//!   picture while it is still on the box. `POST screenshot/forget
//!   {draftId}` (Remove). People only: every passport gets 403
//!   `screenshot_people_only`, on these routes and on a `send` that names
//!   a screenshot. `send` refuses a hash that isn't the draft's (409
//!   `screenshot_changed`): what is sent is what the person saw.
//! - `GET  list?state=&limit=` → own rows; the owner and admins see all
//! - `GET  show?id=` → one row (own; the owner and admins any)
//! - `POST retry {id}` (`failed` → `queued`), `POST cancel {id}`
//!   (`queued` → `cancelled`)
//! - `GET  settings` → `{enabled, agents, agentDiagnostics,
//!   rememberDiagnostics, canEdit, canSend, why?, message?, endpoint,
//!   contactAddress, pending, service}`; `POST settings` `{enabled?,
//!   agents?, agentDiagnostics?}` (the owner or an admin, a person) and
//!   `{rememberDiagnostics?}` (any person, for themselves)
//!
//! Who: the owner token, a Connect login (Member floor in `route_policy`),
//! a K2 terminal's passport (a person typing) and an agent's passport. App
//! passes and API keys: 403 `product_feedback_wrong_credential`. An agent
//! sends only while `agents` is on, within the box limits (1 a minute and
//! 3 a day per workspace, 10 a day for all agents), with no contact email.
//! Only `senderKind` leaves the box; the sender's name stays here.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{json, Value as J};

use k2_core::connect_users::Role;
use k2_core::product_feedback::store::{self, Row, State};
use k2_core::product_feedback::{self as pf, now_ms, settings};

use crate::cli_response::CliResponse;
use crate::compute_routes::Who;
use crate::product_feedback_drain as drain;
use crate::session_events::{emit, SessionEvent};

/// A send carries the edited diagnostics (≤ 256 KB of text, escaped).
pub const MAX_BODY: usize = 1024 * 1024;
/// The screenshot upload: a 2 MiB PNG as base64 (about 2.67 MiB) in JSON.
pub const MAX_SCREENSHOT_BODY: usize = 3 * 1024 * 1024;

pub const DIAGNOSTICS: &str = "/cli/product-feedback/diagnostics";
pub const SEND: &str = "/cli/product-feedback/send";
pub const LIST: &str = "/cli/product-feedback/list";
pub const SHOW: &str = "/cli/product-feedback/show";
pub const RETRY: &str = "/cli/product-feedback/retry";
pub const CANCEL: &str = "/cli/product-feedback/cancel";
pub const SETTINGS: &str = "/cli/product-feedback/settings";
pub const SCREENSHOT: &str = "/cli/product-feedback/screenshot";
pub const SCREENSHOT_FORGET: &str = "/cli/product-feedback/screenshot/forget";

pub const GET_ROUTES: &[&str] = &[LIST, SHOW, SETTINGS, SCREENSHOT];
pub const POST_ROUTES: &[&str] = &[DIAGNOSTICS, SEND, RETRY, CANCEL, SETTINGS, SCREENSHOT, SCREENSHOT_FORGET];

pub fn is_route(path: &str) -> bool {
    GET_ROUTES.contains(&path) || POST_ROUTES.contains(&path)
}

/// The body cap for `path`: [`MAX_SCREENSHOT_BODY`] for the screenshot
/// upload, [`MAX_BODY`] for every other route here. The dispatcher and the
/// cell server both read it.
pub fn max_body(path: &str) -> usize {
    if path == SCREENSHOT {
        MAX_SCREENSHOT_BODY
    } else {
        MAX_BODY
    }
}

/// POST-only rows (a GET on them is 405).
pub fn is_post_only(path: &str) -> bool {
    POST_ROUTES.contains(&path) && !GET_ROUTES.contains(&path)
}

pub fn who_from_tcp(query: &str, bearer: Option<&str>, owner_token: &str) -> Who {
    crate::compute_routes::who_from_tcp(query, bearer, owner_token)
}

fn resp(status: &'static str, body: J) -> CliResponse {
    CliResponse { status, content_type: "application/json", body: body.to_string() }
}

fn ok(mut body: J) -> CliResponse {
    if let Some(o) = body.as_object_mut() {
        o.insert("ok".into(), json!(true));
    }
    resp("200 OK", body)
}

fn refuse(status: &'static str, code: &str, message: impl Into<String>) -> CliResponse {
    resp(status, json!({ "ok": false, "error": code, "message": message.into() }))
}

/// The refusal for a body over [`max_body`].
pub fn too_large(path: &str) -> CliResponse {
    if path == SCREENSHOT {
        return refuse(
            "413 Payload Too Large",
            "screenshot_too_large",
            "A screenshot is at most 2 MB. Attach a smaller image.",
        );
    }
    refuse("413 Payload Too Large", "body_too_large", format!("Bodies here are at most {} KiB.", MAX_BODY / 1024))
}

fn bad(code: &str, message: impl Into<String>) -> CliResponse {
    refuse("400 Bad Request", code, message)
}

fn db_err(e: impl std::fmt::Display) -> CliResponse {
    CliResponse::internal_error(format!("product feedback store: {e}"))
}

/// The caller, as this module sees it.
#[derive(Debug, Clone)]
pub(crate) struct Caller {
    /// `person` or `agent` (what leaves the box).
    kind: &'static str,
    /// Local only: `owner`, `login:<user>`, `agent:<pid>`, `shell:<pid>`.
    sender: String,
    /// A person's own key for the remembered choice (owner token: `owner`).
    username: Option<String>,
    /// Owner token or an Owner/Admin login.
    admin: bool,
    /// A passport (agent or K2 terminal): always `client: cli`.
    passport: bool,
}

impl Caller {
    fn is_agent(&self) -> bool {
        self.kind == "agent"
    }
}

pub(crate) fn caller(who: &Who) -> Result<Caller, CliResponse> {
    Ok(match who {
        Who::Owner => Caller { kind: "person", sender: "owner".into(), username: Some("owner".into()), admin: true, passport: false },
        Who::Login { username, role } => Caller {
            kind: "person",
            sender: format!("login:{username}"),
            username: Some(username.clone()),
            admin: *role >= Role::Admin,
            passport: false,
        },
        // A K2 terminal tab: a person typing (PF9: only an agent passport
        // is an agent).
        Who::Shell { project_id, .. } => {
            Caller { kind: "person", sender: format!("shell:{project_id}"), username: None, admin: false, passport: true }
        }
        Who::Agent { project_id, .. } => {
            Caller { kind: "agent", sender: format!("agent:{project_id}"), username: None, admin: false, passport: true }
        }
        Who::Wrong => {
            return Err(refuse(
                "403 Forbidden",
                "product_feedback_wrong_credential",
                "App passes and API keys can't send feedback to K2. People use the button next to Help; agents use k2 send-feedback.",
            ))
        }
        Who::Invalid => return Err(refuse("401 Unauthorized", "unauthorized", "Sign in to this server first.")),
    })
}

/// Whether `c` may see `row`.
fn visible(c: &Caller, row: &Row) -> bool {
    c.admin || row.sender == c.sender
}

fn project_name(id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT name FROM projects WHERE id = ?1", [id], |r| r.get::<_, String>(0)).ok()
}

/// How the Sent tab names the sender (local only).
fn sender_label(sender: &str) -> String {
    if sender == "owner" {
        return "the owner".into();
    }
    if let Some(u) = sender.strip_prefix("login:") {
        return u.to_string();
    }
    if let Some(p) = sender.strip_prefix("agent:") {
        return format!("agent in {}", project_name(p).unwrap_or_else(|| "a removed workspace".into()));
    }
    if let Some(p) = sender.strip_prefix("shell:") {
        return format!("K2 terminal in {}", project_name(p).unwrap_or_else(|| "a removed workspace".into()));
    }
    sender.to_string()
}

/// `code: detail` → (code, detail).
fn split_error(e: &str) -> (String, String) {
    match e.split_once(": ") {
        Some((c, d)) if c.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_') => (c.to_string(), d.to_string()),
        _ => (String::new(), e.to_string()),
    }
}

/// The plain line the Sent tab and the CLI show for a row's reason.
pub fn reason_line(code: &str, detail: &str) -> String {
    match code {
        k2_core::product_feedback::wire::E_UNREACHABLE | k2_core::product_feedback::wire::E_RELAY_NOT_READY => {
            "K2's feedback service isn't reachable yet. Your feedback is saved on this server and sends when it is.".into()
        }
        "" => detail.to_string(),
        _ => detail.to_string(),
    }
}

/// `full` rows (show) say whether the picture is still on the box.
fn row_json(c: &Caller, r: &Row, full: bool, shot_on_box: bool) -> J {
    let (code, detail) = r.last_error.as_deref().map(split_error).unwrap_or_default();
    let mut v = json!({
        "id": r.id,
        "createdAt": r.created_at,
        "updatedAt": r.updated_at,
        "category": r.category,
        "text": r.body,
        "senderKind": r.sender_kind,
        "client": r.client,
        "sender": sender_label(&r.sender),
        "mine": r.sender == c.sender,
        "state": r.state.as_str(),
        "attempts": r.attempts,
        "nextAttemptAt": if r.state == State::Queued { json!(r.next_attempt_at) } else { J::Null },
        "error": if code.is_empty() { J::Null } else { json!(code) },
        "errorMessage": r.last_error.as_ref().map(|_| reason_line(&code, &detail)),
        "remoteId": r.remote_id,
        "sentAt": r.sent_at,
        "contactEmail": r.contact_email,
        "hasDiagnostics": r.diagnostics_json.is_some(),
        "hasScreenshot": r.screenshot_meta.is_some(),
    });
    if full {
        v["screenshot"] = match r.screenshot_meta.as_deref().and_then(pf::screenshot::Meta::parse) {
            Some(m) => json!({
                "bytes": m.bytes,
                "width": m.width,
                "height": m.height,
                "source": m.source,
                "capture": m.capture,
                "onBox": shot_on_box,
            }),
            None => J::Null,
        };
        let sections: J = r
            .diagnostics_json
            .as_deref()
            .and_then(|d| serde_json::from_str(d).ok())
            .unwrap_or(J::Null);
        v["diagnostics"] = sections;
        v["meta"] = serde_json::from_str(&r.meta_json).unwrap_or(J::Null);
    }
    v
}

fn emit_row(id: &str, st: State, error: Option<&str>) {
    let _ = emit(SessionEvent::ProductFeedbackStatus {
        id: id.to_string(),
        state: st.as_str().to_string(),
        error: error.map(str::to_string),
    });
}

fn service_json() -> J {
    match drain::service_seen() {
        None => json!({ "state": "unknown" }),
        Some(s) => {
            let message = match s.state {
                "ok" => "K2's feedback service is reachable.".to_string(),
                "unreachable" => reason_line(k2_core::product_feedback::wire::E_UNREACHABLE, ""),
                "paused" => "Alakazam paused the feedback service for now. Your feedback is saved and sends when it opens.".into(),
                "limited" => "The feedback service asked this server to wait. Your feedback is saved and sends later.".into(),
                _ => s.detail.clone().unwrap_or_default(),
            };
            json!({ "state": s.state, "code": s.code, "message": message, "at": s.at_ms })
        }
    }
}

pub fn handle(path: &str, is_post: bool, who: Who, body: &[u8], params: &HashMap<String, String>) -> CliResponse {
    let c = match caller(&who) {
        Ok(c) => c,
        Err(r) => return r,
    };
    match (path, is_post) {
        (DIAGNOSTICS, true) => diagnostics(&c, body),
        (SEND, true) => send(&c, body),
        (LIST, false) => list(&c, params),
        (SHOW, false) => show(&c, params),
        (RETRY, true) => transition(&c, body, true),
        (CANCEL, true) => transition(&c, body, false),
        (SETTINGS, false) => settings_get(&c),
        (SETTINGS, true) => settings_post(&c, body),
        (SCREENSHOT, true) => screenshot_put(&c, body),
        (SCREENSHOT, false) => screenshot_get(&c, params),
        (SCREENSHOT_FORGET, true) => screenshot_forget(&c, body),
        _ => CliResponse::method_not_allowed(),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendBody {
    category: Option<String>,
    text: Option<String>,
    contact_email: Option<String>,
    client: Option<String>,
    k2_version: Option<String>,
    locale: Option<String>,
    diagnostics: Option<DiagChoice>,
    screenshot: Option<ShotChoice>,
}

/// The screenshot the person confirmed: the draft, and the hash of the
/// bytes they saw.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShotChoice {
    draft_id: Option<String>,
    sha256: Option<String>,
}

/// Which preview to send and what the person left out (PF26).
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiagChoice {
    draft_id: Option<String>,
    #[serde(default)]
    drop: Vec<String>,
    #[serde(default)]
    edited: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiagBody {
    client: Option<String>,
    page: Option<String>,
    k2_version: Option<String>,
    cli_version: Option<String>,
    #[serde(default)]
    include_client_faults: bool,
}

fn need_owner() -> CliResponse {
    refuse(
        "403 Forbidden",
        "diagnostics_need_owner",
        "An agent can't consent for a person. Ask your owner to allow agents to include K2 details: k2 send-feedback settings --agent-diagnostics on",
    )
}

/// `client` for this caller: a passport is always `cli`.
fn client_of<'a>(c: &Caller, asked: Option<&'a str>) -> Result<&'a str, CliResponse> {
    if c.passport {
        return Ok("cli");
    }
    match asked.map(str::trim).filter(|s| !s.is_empty()).unwrap_or("cli") {
        x if pf::CLIENTS.contains(&x) => Ok(x),
        _ => Err(bad("bad_client", "client is one of desktop, web, cli.")),
    }
}

fn diagnostics(c: &Caller, body: &[u8]) -> CliResponse {
    let b: DiagBody = match parse(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    if c.is_agent() && !settings::load().agent_diagnostics {
        return need_owner();
    }
    let client = match client_of(c, b.client.as_deref()) {
        Ok(x) => x,
        Err(r) => return r,
    };
    let ctx = crate::product_feedback_diag::Context {
        client: client.to_string(),
        page: b.page,
        desktop_version: b.k2_version.as_deref().and_then(|v| pf::short_field(v, 32)),
        cli_version: b.cli_version.as_deref().and_then(|v| pf::short_field(v, 32)),
        include_client_faults: b.include_client_faults && !c.passport,
    };
    let now = now_ms();
    let (facts, logs, book) = crate::product_feedback_diag::gather(&ctx);
    let sections = pf::diagnostics::build(&facts, &logs, &book, now);
    let draft = pf::diagnostics::put_draft(&c.sender, sections, now);
    let list: Vec<J> = draft
        .sections
        .iter()
        .map(|s| json!({ "id": s.id, "title": s.title, "text": s.text, "bytes": s.text.len() }))
        .collect();
    ok(json!({
        "draftId": draft.id,
        "sections": list,
        "bytes": pf::diagnostics::bytes_of(&draft.sections),
        "expiresAt": draft.created_at + pf::diagnostics::DRAFT_TTL_MS,
    }))
}

fn parse<T: for<'de> Deserialize<'de> + Default>(body: &[u8]) -> Result<T, CliResponse> {
    if body.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(T::default());
    }
    serde_json::from_slice(body).map_err(|e| bad("bad_json", format!("The body isn't the JSON this route takes: {e}")))
}

/// Why `c` may not send now (PF7, PF8, PF14, PF19), as the refusal.
fn send_gate(c: &Caller) -> Option<CliResponse> {
    if let Some(h) = drain::hold() {
        let status = if h == drain::Hold::NeedsConnect { "409 Conflict" } else { "403 Forbidden" };
        return Some(refuse(status, h.code(), h.message()));
    }
    if c.is_agent() && !settings::load().agents {
        return Some(refuse(
            "403 Forbidden",
            "agents_off",
            "The owner turned off feedback from agents on this server. Ask your owner to turn it on (k2 send-feedback settings --agents on), or ask them to send it.",
        ));
    }
    None
}

fn meta(c: &Caller, client: &str, b: &SendBody) -> J {
    let daemon = env!("CARGO_PKG_VERSION");
    let k2 = if client == "desktop" && !c.passport {
        b.k2_version.as_deref().and_then(|v| pf::short_field(v, 32)).unwrap_or_else(|| daemon.to_string())
    } else {
        daemon.to_string()
    };
    let locale = b
        .locale
        .as_deref()
        .and_then(|l| pf::short_field(l, 35))
        .or_else(|| {
            std::env::var("LANG").ok().and_then(|l| pf::short_field(l.split('.').next().unwrap_or(""), 35))
        })
        .unwrap_or_default();
    json!({
        "k2Version": k2,
        "daemonVersion": daemon,
        "installKind": crate::boot_status::install_kind(),
        "os": std::env::consts::OS,
        "osVersion": pf::os_version(),
        "arch": std::env::consts::ARCH,
        "locale": locale,
    })
}

fn send(c: &Caller, body: &[u8]) -> CliResponse {
    let b: SendBody = match parse(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    if b.screenshot.is_some() && c.passport {
        return people_only();
    }
    if let Some(r) = send_gate(c) {
        return r;
    }
    let category = b.category.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or("other");
    if !pf::CATEGORIES.contains(&category) {
        return bad("bad_category", "category is one of bug, idea, praise, other.");
    }
    let client = match client_of(c, b.client.as_deref()) {
        Ok(x) => x,
        Err(r) => return r,
    };
    let text = match pf::clean_text(b.text.as_deref().unwrap_or("")) {
        Ok(t) => t,
        Err((code, msg)) => return bad(code, msg),
    };
    let contact = match b.contact_email.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        None => None,
        Some(_) if c.is_agent() => {
            return bad("contact_people_only", "Only a person can ask Alakazam to email them; leave contactEmail out.")
        }
        Some(e) if pf::valid_email(e) => Some(e.to_string()),
        Some(_) => return bad("bad_email", "contactEmail is one email address."),
    };
    // The preview's own bytes, minus what the person left out (PF26): the
    // daemon never collects again here.
    let mut draft_id: Option<String> = None;
    let diagnostics_json = match &b.diagnostics {
        None => None,
        Some(choice) => {
            if c.is_agent() && !settings::load().agent_diagnostics {
                return need_owner();
            }
            let id = choice.draft_id.as_deref().unwrap_or("").trim();
            let Some(draft) = pf::diagnostics::get_draft(id, &c.sender, now_ms()) else {
                return refuse(
                    "410 Gone",
                    "draft_expired",
                    "That preview of the K2 details is gone (they are kept an hour). Open the preview again.",
                );
            };
            match pf::diagnostics::choose(&draft.sections, &choice.drop, &choice.edited) {
                Ok(sections) if sections.is_empty() => None,
                Ok(sections) => {
                    draft_id = Some(draft.id.clone());
                    Some(serde_json::to_string(&sections).expect("sections serialize"))
                }
                Err(e) => return bad("diagnostics_edit", e),
            }
        }
    };
    // The picture the person saw (§3.1 D): the draft's own bytes, only
    // when the hash they confirmed is the draft's.
    let shot = match &b.screenshot {
        None => None,
        Some(choice) => {
            let id = choice.draft_id.as_deref().unwrap_or("").trim();
            let Some(draft) = pf::screenshot::get_draft(id, &c.sender, now_ms()) else {
                return refuse(
                    "410 Gone",
                    "screenshot_expired",
                    "That screenshot is gone from the server (it's kept an hour). Add it again.",
                );
            };
            let seen = choice.sha256.as_deref().unwrap_or("").trim().to_ascii_lowercase();
            if seen != draft.sha256 {
                return refuse(
                    "409 Conflict",
                    "screenshot_changed",
                    "The picture changed after you saw it. Look at it again.",
                );
            }
            Some(draft)
        }
    };
    if let Some(d) = &shot {
        // The whole submission must fit the intake's cap with the picture.
        let est = d.png.len().div_ceil(3) * 4
            + diagnostics_json.as_deref().map_or(0, str::len)
            + serde_json::to_string(&text).map_or(0, |t| t.len())
            + 4096;
        if est > k2_core::product_feedback::wire::MAX_BODY_WITH_SCREENSHOT {
            return refuse(
                "413 Payload Too Large",
                "submission_too_large",
                "With the screenshot and the K2 details together this is over 3 MB. Leave out some details or attach a smaller picture.",
            );
        }
    }
    let now = now_ms();
    let db = k2_core::db::shared();
    let conn = db.lock();
    if c.is_agent() {
        match store::agent_limit(&conn, &c.sender, now) {
            Ok(None) => {}
            Ok(Some(l)) => {
                return resp(
                    "429 Too Many Requests",
                    json!({ "ok": false, "error": "agent_limit", "message": l.message(), "retryAfterSecs": l.retry_after_secs() }),
                )
            }
            Err(e) => return db_err(e),
        }
    }
    let n = store::NewRow {
        id: pf::new_client_id(),
        sender_kind: c.kind.to_string(),
        sender: c.sender.clone(),
        client: client.to_string(),
        category: category.to_string(),
        body: text,
        contact_email: contact,
        meta_json: meta(c, client, &b).to_string(),
        diagnostics_json,
        screenshot: shot.as_ref().map(|d| store::NewShot {
            png: (*d.png).clone(),
            sha256: d.sha256.clone(),
            meta_json: serde_json::to_string(&d.meta()).expect("meta serializes"),
        }),
    };
    let row = match store::insert(&conn, &n, now) {
        Ok(r) => r,
        Err(e) => return db_err(e),
    };
    drop(conn);
    if let Some(d) = draft_id {
        pf::diagnostics::drop_draft(&d);
    }
    if let Some(d) = &shot {
        pf::screenshot::drop_draft(&d.id);
    }
    emit_row(&row.id, State::Queued, None);
    drain::kick();
    ok(json!({
        "id": row.id,
        "state": row.state.as_str(),
        "service": service_json(),
        "message": "Saved on this server; it sends to Alakazam Labs as soon as it can.",
    }))
}

fn list(c: &Caller, params: &HashMap<String, String>) -> CliResponse {
    let state = match params.get("state").map(String::as_str).filter(|s| !s.is_empty()) {
        None => None,
        Some(s) => match State::parse(s) {
            Some(st) => Some(st),
            None => return bad("bad_state", "state is one of queued, sending, sent, failed, refused, cancelled."),
        },
    };
    let limit = match params.get("limit").filter(|s| !s.is_empty()) {
        None => 50,
        Some(l) => match l.parse::<i64>() {
            Ok(n) if (1..=200).contains(&n) => n,
            _ => return bad("bad_limit", "limit is 1 to 200."),
        },
    };
    let db = k2_core::db::shared();
    let rows = {
        let conn = db.lock();
        let sender = if c.admin { None } else { Some(c.sender.as_str()) };
        match store::list(&conn, sender, state, limit) {
            Ok(r) => r,
            Err(e) => return db_err(e),
        }
    };
    let items: Vec<J> = rows.iter().map(|r| row_json(c, r, false, false)).collect();
    ok(json!({ "items": items, "all": c.admin }))
}

fn find(c: &Caller, id: &str) -> Result<Row, CliResponse> {
    let not_found = || refuse("404 Not Found", "not_found", format!("No feedback {id} you can see on this server."));
    if id.is_empty() {
        return Err(bad("id_required", "Name the feedback id (pf_…)."));
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    match store::get(&conn, id) {
        Ok(Some(r)) if visible(c, &r) => Ok(r),
        Ok(_) => Err(not_found()),
        Err(e) => Err(db_err(e)),
    }
}

fn show(c: &Caller, params: &HashMap<String, String>) -> CliResponse {
    let id = params.get("id").map(String::as_str).unwrap_or("").trim();
    let r = match find(c, id) {
        Ok(r) => r,
        Err(r) => return r,
    };
    let on_box = if r.screenshot_meta.is_some() {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match store::has_screenshot(&conn, &r.id) {
            Ok(b) => b,
            Err(e) => return db_err(e),
        }
    } else {
        false
    };
    ok(json!({ "item": row_json(c, &r, true, on_box) }))
}

// ── the screenshot (prd-feedback-screenshot-v1 §5.2) ─────────────────

fn people_only() -> CliResponse {
    refuse(
        "403 Forbidden",
        "screenshot_people_only",
        "Only a person can attach a screenshot, in the Send feedback dialog. Agents and k2 send-feedback send text only.",
    )
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShotBody {
    image: Option<String>,
    source: Option<String>,
    capture: Option<String>,
}

fn shot_refusal(e: &pf::screenshot::ShotError) -> CliResponse {
    let status = if e.code() == "screenshot_too_large" { "413 Payload Too Large" } else { "400 Bad Request" };
    refuse(status, e.code(), e.message())
}

fn data_url(png: &[u8]) -> String {
    use base64::Engine;
    format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png))
}

fn screenshot_put(c: &Caller, body: &[u8]) -> CliResponse {
    if c.passport {
        return people_only();
    }
    let b: ShotBody = match parse(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let source = b.source.as_deref().map(str::trim).unwrap_or("");
    if !pf::screenshot::SOURCES.contains(&source) {
        return bad("bad_screenshot", "source is window (K2's window) or file (an image the person chose).");
    }
    let capture = b.capture.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let capture = match (source, capture) {
        ("window", Some(x)) if pf::screenshot::CAPTURES.contains(&x) => Some(x),
        ("window", _) => return bad("bad_screenshot", "A window picture says how it was taken: capture is page or webview."),
        (_, None) => None,
        (_, Some(_)) => return bad("bad_screenshot", "capture is only for a picture of K2's window; leave it out for a file."),
    };
    let image = b.image.as_deref().unwrap_or("");
    if image.trim().is_empty() {
        return bad("bad_screenshot", "image is the PNG, base64.");
    }
    let raw = match pf::screenshot::decode_base64(image) {
        Ok(r) => r,
        Err(e) => return shot_refusal(&e),
    };
    let n = match pf::screenshot::normalize(&raw) {
        Ok(n) => n,
        Err(e) => return shot_refusal(&e),
    };
    let d = pf::screenshot::put_draft(&c.sender, n, source, capture, now_ms());
    ok(json!({
        "draftId": d.id,
        "sha256": d.sha256,
        "bytes": d.png.len(),
        "width": d.width,
        "height": d.height,
        "source": d.source,
        "capture": d.capture,
        "expiresAt": d.created_at + pf::screenshot::DRAFT_TTL_MS,
    }))
}

fn screenshot_get(c: &Caller, params: &HashMap<String, String>) -> CliResponse {
    if c.passport {
        return people_only();
    }
    let draft_id = params.get("draftId").map(|s| s.trim()).filter(|s| !s.is_empty());
    let row_id = params.get("id").map(|s| s.trim()).filter(|s| !s.is_empty());
    match (draft_id, row_id) {
        (Some(id), None) => match pf::screenshot::get_draft(id, &c.sender, now_ms()) {
            Some(d) => ok(json!({
                "dataUrl": data_url(&d.png),
                "sha256": d.sha256,
                "bytes": d.png.len(),
                "width": d.width,
                "height": d.height,
                "source": d.source,
                "capture": d.capture,
            })),
            None => refuse(
                "410 Gone",
                "screenshot_expired",
                "That screenshot is gone from the server (it's kept an hour). Add it again.",
            ),
        },
        (None, Some(id)) => {
            let row = match find(c, id) {
                Ok(r) => r,
                Err(r) => return r,
            };
            let meta = row.screenshot_meta.as_deref().and_then(pf::screenshot::Meta::parse);
            let stored = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                match store::screenshot(&conn, id) {
                    Ok(s) => s,
                    Err(e) => return db_err(e),
                }
            };
            match (meta, stored) {
                (Some(m), Some(s)) => ok(json!({
                    "dataUrl": data_url(&s.png),
                    "sha256": s.sha256,
                    "bytes": s.png.len(),
                    "width": m.width,
                    "height": m.height,
                    "source": m.source,
                    "capture": m.capture,
                })),
                (Some(_), None) => refuse(
                    "404 Not Found",
                    "screenshot_not_on_box",
                    "This server no longer keeps that screenshot: it is deleted once the feedback is sent or cancelled.",
                ),
                (None, _) => refuse("404 Not Found", "no_screenshot", format!("{id} has no screenshot.")),
            }
        }
        _ => bad("draft_id_required", "Name one: draftId (a draft, pfs_…) or id (a feedback, pf_…)."),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ForgetBody {
    draft_id: Option<String>,
}

fn screenshot_forget(c: &Caller, body: &[u8]) -> CliResponse {
    if c.passport {
        return people_only();
    }
    let b: ForgetBody = match parse(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let id = b.draft_id.as_deref().unwrap_or("").trim();
    if id.is_empty() {
        return bad("draft_id_required", "Name the draft (draftId, pfs_…).");
    }
    // Idempotent: a draft already gone (or another person's) is not an error.
    let forgotten = pf::screenshot::forget_draft(id, &c.sender);
    ok(json!({ "draftId": id, "forgotten": forgotten }))
}

#[derive(Debug, Default, Deserialize)]
struct IdBody {
    id: Option<String>,
}

fn transition(c: &Caller, body: &[u8], is_retry: bool) -> CliResponse {
    let b: IdBody = match parse(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let id = b.id.as_deref().unwrap_or("").trim().to_string();
    let row = match find(c, &id) {
        Ok(r) => r,
        Err(r) => return r,
    };
    let now = now_ms();
    let changed = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let r = if is_retry { store::retry(&conn, &id, now) } else { store::cancel(&conn, &id, now) };
        match r {
            Ok(c) => c,
            Err(e) => return db_err(e),
        }
    };
    if !changed {
        let (want, verb) = if is_retry { ("failed", "retry") } else { ("queued", "cancel") };
        return refuse(
            "409 Conflict",
            "wrong_state",
            format!("{id} is {}; you can {verb} only a {want} one.", row.state.as_str()),
        );
    }
    let st = if is_retry { State::Queued } else { State::Cancelled };
    emit_row(&id, st, None);
    if is_retry {
        drain::kick();
    }
    ok(json!({ "id": id, "state": st.as_str() }))
}

fn settings_get(c: &Caller) -> CliResponse {
    let s = settings::load();
    let hold = drain::hold();
    let agent_off = c.is_agent() && !s.agents;
    let (can_send, why, message) = match (&hold, agent_off) {
        (Some(h), _) => (false, Some(h.code()), Some(h.message())),
        (None, true) => (false, Some("agents_off"), Some("The owner turned off feedback from agents on this server.".to_string())),
        (None, false) => (true, None, None),
    };
    let remember = match &c.username {
        Some(u) => {
            let db = k2_core::db::shared();
            let conn = db.lock();
            match store::remember_diagnostics(&conn, u) {
                Ok(v) => json!(v),
                Err(e) => return db_err(e),
            }
        }
        None => J::Null,
    };
    let pending = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::pending_count(&conn).unwrap_or(0)
    };
    ok(json!({
        "enabled": s.enabled && !k2_core::airgap::enabled(),
        "agents": s.agents,
        "agentDiagnostics": s.agent_diagnostics,
        "rememberDiagnostics": remember,
        "canEdit": c.admin && !c.passport,
        "canSend": can_send,
        "why": why,
        "message": message,
        "endpoint": pf::endpoint(),
        "contactAddress": pf::CONTACT_ADDRESS,
        "senderKind": c.kind,
        "pending": pending,
        "service": service_json(),
    }))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SettingsBody {
    enabled: Option<bool>,
    agents: Option<bool>,
    agent_diagnostics: Option<bool>,
    remember_diagnostics: Option<bool>,
}

fn settings_post(c: &Caller, body: &[u8]) -> CliResponse {
    let b: SettingsBody = match parse(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    if c.passport {
        return refuse(
            "403 Forbidden",
            "settings_people_only",
            "Only a person signed in to this server changes these settings (the owner or an admin in Settings → General → Send feedback).",
        );
    }
    let server_keys = b.enabled.is_some() || b.agents.is_some() || b.agent_diagnostics.is_some();
    if server_keys && !c.admin {
        return refuse("403 Forbidden", "admin_only", "Only the owner or an admin can change this server's feedback settings.");
    }
    if let Some(on) = b.remember_diagnostics {
        let Some(u) = c.username.as_deref() else {
            return refuse("403 Forbidden", "settings_people_only", "Only a person can remember this choice.");
        };
        let db = k2_core::db::shared();
        let conn = db.lock();
        if let Err(e) = store::set_remember_diagnostics(&conn, u, on) {
            return db_err(e);
        }
    }
    if server_keys {
        let mut s = settings::load();
        if let Some(v) = b.enabled {
            s.enabled = v;
        }
        if let Some(v) = b.agents {
            s.agents = v;
        }
        if let Some(v) = b.agent_diagnostics {
            s.agent_diagnostics = v;
        }
        if let Err(e) = settings::save(&s) {
            return CliResponse::internal_error(format!("save feedback settings: {e}"));
        }
        if s.enabled {
            // Rows held while it was off may go now.
            drain::kick();
        }
    }
    settings_get(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_sets() {
        for p in [DIAGNOSTICS, SEND, RETRY, CANCEL, SCREENSHOT_FORGET] {
            assert!(is_post_only(p), "{p}");
        }
        for p in [LIST, SHOW, SETTINGS, SCREENSHOT] {
            assert!(!is_post_only(p), "{p}");
        }
        assert!(!is_route("/cli/feedback/create"));
        assert!(GET_ROUTES.iter().chain(POST_ROUTES).all(|p| p.starts_with("/cli/product-feedback/")));
    }

    fn must(w: Who) -> Caller {
        match caller(&w) {
            Ok(c) => c,
            Err(r) => panic!("refused: {}", r.body),
        }
    }

    #[test]
    fn app_passes_and_api_keys_are_refused() {
        let r = caller(&Who::Wrong).unwrap_err();
        assert_eq!(r.status, "403 Forbidden");
        assert!(r.body.contains("product_feedback_wrong_credential"));
        let a = must(Who::Agent { project_id: "p".into(), session_id: "s".into() });
        assert_eq!((a.kind, a.sender.as_str(), a.passport, a.admin), ("agent", "agent:p", true, false));
        let s = must(Who::Shell { project_id: "p".into(), session_id: "s".into() });
        assert_eq!((s.kind, s.passport), ("person", true));
        let m = must(Who::Login { username: "al".into(), role: Role::Member });
        assert!(!m.admin);
        let a = must(Who::Login { username: "ad".into(), role: Role::Admin });
        assert!(a.admin);
    }

    #[test]
    fn only_the_screenshot_upload_gets_the_bigger_body() {
        assert_eq!(max_body(SCREENSHOT), 3 * 1024 * 1024);
        for p in GET_ROUTES.iter().chain(POST_ROUTES).filter(|p| **p != SCREENSHOT) {
            assert_eq!(max_body(p), MAX_BODY, "{p}");
        }
        assert!(too_large(SCREENSHOT).body.contains("screenshot_too_large"));
        assert!(too_large(SEND).body.contains("body_too_large"));
        // Both front doors read the per-path cap (and the refusal for it).
        for (file, src) in [
            ("routes/dispatcher.rs", include_str!("routes/dispatcher.rs")),
            ("cell_server.rs", include_str!("cell_server.rs")),
        ] {
            assert!(src.contains("product_feedback_routes::max_body(p)"), "{file} doesn't use max_body");
            assert!(src.contains("product_feedback_routes::too_large(p)"), "{file} doesn't use too_large(p)");
            assert!(!src.contains("product_feedback_routes::MAX_BODY"), "{file} still uses the fixed cap");
        }
    }

    #[test]
    fn every_passport_is_refused_on_the_screenshot_routes() {
        let agent = must(Who::Agent { project_id: "p".into(), session_id: "s".into() });
        let shell = must(Who::Shell { project_id: "p".into(), session_id: "s".into() });
        for c in [&agent, &shell] {
            for r in [
                screenshot_put(c, br#"{"image":"x","source":"file"}"#),
                screenshot_get(c, &HashMap::from([("draftId".to_string(), "pfs_x".to_string())])),
                screenshot_forget(c, br#"{"draftId":"pfs_x"}"#),
                send(c, br#"{"text":"x","screenshot":{"draftId":"pfs_x","sha256":"00"}}"#),
            ] {
                assert_eq!(r.status, "403 Forbidden", "{}", r.body);
                assert!(r.body.contains("screenshot_people_only"), "{}", r.body);
            }
        }
    }

    #[test]
    fn upload_checks_source_capture_and_bytes() {
        use base64::Engine;
        let me = must(Who::Login { username: "shot-route-test".into(), role: Role::Member });
        let png = k2_core::product_feedback::screenshot::fixture::png(12, 9);
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
        let put = |body: J| screenshot_put(&me, body.to_string().as_bytes());
        for (body, code) in [
            (json!({"image": b64, "source": "screen"}), "bad_screenshot"),
            (json!({"image": b64, "source": "window"}), "bad_screenshot"),
            (json!({"image": b64, "source": "window", "capture": "native"}), "bad_screenshot"),
            (json!({"image": b64, "source": "file", "capture": "page"}), "bad_screenshot"),
            (json!({"image": "%%% not base64", "source": "file"}), "bad_screenshot"),
            (json!({"image": base64::engine::general_purpose::STANDARD.encode(b"GIF89a not a png"), "source": "file"}), "bad_screenshot"),
        ] {
            let r = put(body.clone());
            assert_eq!(r.status, "400 Bad Request", "{body}: {}", r.body);
            assert!(r.body.contains(code), "{}", r.body);
        }
        let big = base64::engine::general_purpose::STANDARD.encode(vec![7u8; pf::screenshot::MAX_BYTES + 10]);
        let r = put(json!({"image": big, "source": "file"}));
        assert_eq!(r.status, "413 Payload Too Large", "{}", r.body);
        assert!(r.body.contains("screenshot_too_large"));

        let r = put(json!({"image": b64, "source": "window", "capture": "page"}));
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let v: J = serde_json::from_str(&r.body).unwrap();
        assert_eq!((v["width"].clone(), v["height"].clone(), v["capture"].clone()), (json!(12), json!(9), json!("page")));
        let id = v["draftId"].as_str().unwrap().to_string();
        let g = screenshot_get(&me, &HashMap::from([("draftId".to_string(), id.clone())]));
        let gv: J = serde_json::from_str(&g.body).unwrap();
        let url = gv["dataUrl"].as_str().unwrap();
        let bytes = base64::engine::general_purpose::STANDARD.decode(url.strip_prefix("data:image/png;base64,").unwrap()).unwrap();
        assert_eq!(pf::screenshot::sha256_hex(&bytes), v["sha256"].as_str().unwrap(), "the hash is of the bytes the person sees");
        // Someone else can't see it; forget is the owner's.
        let other = must(Who::Login { username: "shot-route-other".into(), role: Role::Member });
        let g = screenshot_get(&other, &HashMap::from([("draftId".to_string(), id.clone())]));
        assert_eq!(g.status, "410 Gone", "{}", g.body);
        let f: J = serde_json::from_str(&screenshot_forget(&other, json!({"draftId": id}).to_string().as_bytes()).body).unwrap();
        assert_eq!(f["forgotten"], json!(false));
        let f: J = serde_json::from_str(&screenshot_forget(&me, json!({"draftId": id}).to_string().as_bytes()).body).unwrap();
        assert_eq!(f["forgotten"], json!(true));
        let g = screenshot_get(&me, &HashMap::from([("draftId".to_string(), id)]));
        assert_eq!(g.status, "410 Gone", "Remove forgot it");
    }
}
