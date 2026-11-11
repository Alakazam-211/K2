//! Recording in Apps (Sew 0.9.0; K2 PRD prd-k2-bobbin-in-apps-v1 K-d): an
//! App server reads its recordings, on its own pass.
//!
//! ```text
//! GET /cli/sew/app/recordings            ?space=&after=&limit=   recordings.list
//! GET /cli/sew/app/recording             ?id=                    recordings.get
//! GET /cli/sew/app/recording/transcript  ?id=&after=&limit=      recordings.transcript
//! WS  /cli/sew/app/recordings/events                             recordings.events (frame recording_changed)
//! ```
//!
//! - **Only the governing App, only with `server_read`** (Q7, Q2): K2 checks
//!   Sew's flag (`mgmt_recording_reads`), the App's `server_read` (on only
//!   through the owner's `apps.recording` card), and that the App governs
//!   the Space (it is bound to it; a participant App gets `409
//!   app_not_governing`) before it calls Sew; Sew checks again. The feed and
//!   the summaries need `server_read` too (Rosson's answer 2).
//! - **The App's own pass** (never a guest's session pass, the owner token
//!   or a login), like `chat.member`.
//! - Every read sends `Sew-Actor: app:<service id>` ([`MgmtClient::get_as`]),
//!   which Sew audits as `recording.transcript_read`; K2 writes a
//!   `deploy_audit` row `recording_transcript_read` (App, recording, turns)
//!   for every transcript read.
//! - **No audio, ever.** The answers are K2's own re-serialization of Sew's
//!   (`sew_wire::RecordingSummary`, `TranscriptTurn`): a field K2 doesn't
//!   name (an audio link) never crosses.
//! - **The feed** ([`subscribe`]): one follower task per App with at least
//!   one open socket and `server_read` on. It long-polls Sew's
//!   `apps/{app}/recordings/changes` (`wait=30`, timeout `wait + 10 s`),
//!   keeps the cursor in skin.db (`tool_recording_feed`), and sends each
//!   change as `{kind: "recording_changed", seq, change, recordingId,
//!   spaceId, channelId, at}`, never content. Sew's feed never answers 410
//!   (its `k2_mgmt_0.9.2.json` golden): a cursor past its head gets an empty
//!   page. A socket that falls behind K2's own buffer gets `{kind:
//!   "recordings_refetch"}` (the App calls `recordings.list` again). No
//!   App-server stop in 0.9.0 (Rosson's answer 4).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use k2_core::skin::SkinPass;
use k2_core::tool_hook::recording::ReadRefusal;
use k2_core::tool_hook::sew_wire::{self, recording as wire};
use k2_core::tool_hook::{self as hook, features};
use serde_json::{json, Value as J};
use tokio::sync::broadcast;

use crate::cli_response::CliResponse;
use crate::guest_rooms_routes::{app_of, error, pass_invalid, OnLink};
use crate::sew_mgmt::{long_poll_timeout, MgmtClient, MgmtResponse};

pub const LIST: &str = "/cli/sew/app/recordings";
pub const ONE: &str = "/cli/sew/app/recording";
pub const TRANSCRIPT: &str = "/cli/sew/app/recording/transcript";
pub const EVENTS: &str = "/cli/sew/app/recordings/events";
/// The `deploy_audit` event of a transcript read.
pub const AUDIT_READ: &str = wire::AUDIT_TRANSCRIPT_READ;
/// The frames.
pub const FRAME_CHANGED: &str = "recording_changed";
pub const FRAME_REFETCH: &str = "recordings_refetch";
/// The feed's long-poll (Sew's `wait`, at most 30 s).
pub const FEED_WAIT_S: u32 = 30;
/// Default and largest page an App asks for.
pub const LIMIT_DEFAULT: u32 = 50;

pub fn is_route(p: &str) -> bool {
    p == LIST || p == ONE || p == TRANSCRIPT
}

pub fn app_pass_required() -> CliResponse {
    error(
        403,
        "app_pass_required",
        "recording reads take this App's own pass (K2_PASS_FILE inside the wall, or its remote pass); never a guest's pass, the owner token or a login",
        None,
    )
}

fn refusal(r: &ReadRefusal) -> CliResponse {
    match r {
        ReadRefusal::FeatureMissing => error(
            409,
            "sew_feature_missing",
            "this server's chat can't hand recordings to Apps yet (it needs Sew 0.9.0: mgmt_recording_reads)",
            Some(json!({ "feature": features::MGMT_RECORDING_READS })),
        ),
        ReadRefusal::ServerReadOff => error(
            403,
            sew_wire::reasons::RECORDING_SERVER_READ_OFF,
            "this App's server may not read recordings: the server's owner turns server_read on, on a card (k2 permission request apps.recording app=<app> server-read=on --why \"…\")",
            None,
        ),
        ReadRefusal::NotGoverning => error(
            409,
            sew_wire::reasons::APP_NOT_GOVERNING,
            "this App doesn't govern that Space (it isn't bound to it here), so it reads none of its recordings",
            None,
        ),
    }
}

/// An id Sew hands out (`[A-Za-z0-9_-]`, 1–64), never a path.
fn id_ok(s: &str) -> bool {
    sew_wire::is_sew_id(s)
}

fn param<'a>(p: &'a HashMap<String, String>, k: &str) -> Option<&'a str> {
    p.get(k).map(|s| s.trim()).filter(|s| !s.is_empty())
}

fn limit_of(p: &HashMap<String, String>) -> Result<u32, CliResponse> {
    match param(p, "limit") {
        None => Ok(LIMIT_DEFAULT),
        Some(l) => l
            .parse::<u32>()
            .ok()
            .filter(|n| (1..=wire::LIMIT_MAX).contains(n))
            .ok_or_else(|| error(400, "usage", format!("limit is 1 to {}", wire::LIMIT_MAX), None)),
    }
}

fn after_of(p: &HashMap<String, String>) -> Result<Option<String>, CliResponse> {
    match param(p, "after") {
        None => Ok(None),
        Some(a) if id_ok(a) => Ok(Some(a.to_string())),
        Some(_) => Err(error(400, "usage", "after is the next cursor a page gave you", None)),
    }
}

/// What a read asks Sew.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Read {
    List { space: String, after: Option<String>, limit: u32 },
    One { id: String },
    Transcript { id: String, after: Option<String>, limit: u32 },
}

impl Read {
    pub fn parse(path: &str, p: &HashMap<String, String>) -> Result<Read, CliResponse> {
        let id = || -> Result<String, CliResponse> {
            param(p, "id").filter(|i| id_ok(i)).map(str::to_string).ok_or_else(|| error(400, "usage", "name the recording: id=<its id>", None))
        };
        Ok(match path {
            LIST => Read::List {
                space: param(p, "space")
                    .filter(|s| id_ok(s))
                    .map(str::to_string)
                    .ok_or_else(|| error(400, "usage", "name the Space: space=<its id> (chat.space lists this App's)", None))?,
                after: after_of(p)?,
                limit: limit_of(p)?,
            },
            ONE => Read::One { id: id()? },
            TRANSCRIPT => Read::Transcript { id: id()?, after: after_of(p)?, limit: limit_of(p)? },
            _ => return Err(error(404, "not_found", "no such route", None)),
        })
    }

    fn space(&self) -> Option<&str> {
        match self {
            Read::List { space, .. } => Some(space),
            _ => None,
        }
    }

    fn mgmt_path(&self, app: &str) -> String {
        match self {
            Read::List { space, after, limit } => sew_wire::space_recordings_path(space, app, after.as_deref(), *limit),
            Read::One { id } => sew_wire::recording_path(id, app),
            Read::Transcript { id, after, limit } => sew_wire::recording_transcript_path(id, app, after.as_deref(), *limit),
        }
    }
}

fn recording_json(r: &sew_wire::RecordingSummary) -> J {
    json!({
        "id": r.id,
        "spaceId": r.space_id,
        "channelId": r.channel_id,
        "state": r.state,
        "trigger": r.trigger,
        "startedBy": r.started_by,
        "startedAt": r.started_at,
        "stoppedAt": r.stopped_at,
        "stopReason": r.stop_reason,
        "transcriptReady": r.transcript_ready,
        "consents": r.consents.iter().map(|c| json!({ "externalId": c.external_id, "consent": c.consent })).collect::<Vec<_>>(),
    })
}

/// Sew's refusal, in K2's shape (its reason when K2 knows it).
fn sew_refusal(r: &MgmtResponse) -> CliResponse {
    let reason = r.error().and_then(|e| e.error.detail_str("reason").map(str::to_string));
    match (r.status, reason.as_deref()) {
        (403, Some(sew_wire::reasons::RECORDING_SERVER_READ_OFF)) => refusal(&ReadRefusal::ServerReadOff),
        (409, Some(sew_wire::reasons::APP_NOT_GOVERNING)) => refusal(&ReadRefusal::NotGoverning),
        (404, _) => error(404, "not_found", "no such recording in this App's Spaces", None),
        (s, _) if s >= 500 => error(503, "chat_unavailable", "this server's chat didn't answer; try again", None),
        _ => error(502, "failed", format!("chat refused the read: {}", r.error_line()), None),
    }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, CliResponse> {
    tokio::task::spawn_blocking(f).await.map_err(|e| CliResponse::internal_error(format!("worker join: {e}")))
}

async fn chat_client() -> Result<MgmtClient, CliResponse> {
    let t = blocking(|| hook::get_target(hook::CHAT_KIND)).await?;
    match t {
        Ok(Some(t)) if t.enabled => Ok(MgmtClient::new(&t.socket, &t.token_file)),
        Ok(_) => Err(error(503, "chat_unavailable", "Chat / Voice Hosting isn't on for this server", None)),
        Err(e) => Err(error(503, "chat_unavailable", e, None)),
    }
}

/// One read for App `app` (its pass already checked): K2's own checks,
/// then Sew as `app:<app>`.
pub async fn read_for_app(app: &str, path: &str, params: &HashMap<String, String>) -> CliResponse {
    let read = match Read::parse(path, params) {
        Ok(r) => r,
        Err(r) => return r,
    };
    let gate = {
        let (a, s) = (app.to_string(), read.space().map(str::to_string));
        blocking(move || hook::recording_read_gate(&a, s.as_deref())).await
    };
    match gate {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(r))) => return refusal(&r),
        Ok(Err(e)) => return error(503, "chat_unavailable", e, None),
        Err(r) => return r,
    }
    let client = match chat_client().await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let resp = match client.get_as(&read.mgmt_path(app), &sew_wire::app_actor(app)).await {
        Ok(r) => r,
        Err(e) => return error(503, "chat_unavailable", format!("this server's chat didn't answer: {e}"), None),
    };
    if !resp.is_success() {
        return sew_refusal(&resp);
    }
    let bad = |e: serde_json::Error| error(502, "failed", format!("chat's answer wasn't a recording read ({e})"), None);
    let body = match &read {
        Read::List { space, .. } => match serde_json::from_slice::<sew_wire::RecordingsPage>(&resp.body) {
            Ok(p) => json!({ "ok": true, "app": app, "spaceId": space, "recordings": p.recordings.iter().map(recording_json).collect::<Vec<_>>(), "next": p.next }),
            Err(e) => return bad(e),
        },
        Read::One { .. } => match serde_json::from_slice::<sew_wire::RecordingSummary>(&resp.body) {
            Ok(r) => json!({ "ok": true, "app": app, "recording": recording_json(&r) }),
            Err(e) => return bad(e),
        },
        Read::Transcript { id, .. } => match serde_json::from_slice::<sew_wire::TranscriptPage>(&resp.body) {
            Ok(t) => {
                let turns: Vec<J> = t
                    .turns
                    .iter()
                    .map(|x| json!({ "id": x.id, "speakerExternalId": x.speaker_external_id, "startMs": x.start_ms, "endMs": x.end_ms, "text": x.text }))
                    .collect();
                audit_read(app, id, turns.len()).await;
                json!({ "ok": true, "app": app, "recordingId": id, "turns": turns, "next": t.next })
            }
            Err(e) => return bad(e),
        },
    };
    CliResponse::ok_json(body.to_string())
}

/// K2's `deploy_audit` row beside Sew's `recording.transcript_read`.
async fn audit_read(app: &str, recording: &str, turns: usize) {
    let (app, recording) = (app.to_string(), recording.to_string());
    let _ = blocking(move || {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let subject = k2_core::deploy::store::get_app_by_service(&conn, &app).ok().flatten().map(|a| a.app_id).unwrap_or_else(|| app.clone());
        let payload = json!({ "service": app, "recording": recording, "turns": turns });
        if let Err(e) = k2_core::deploy::audit::append(&conn, k2_core::deploy::audit::SUBJECT_APP, &subject, AUDIT_READ, &sew_wire::app_actor(&app), None, &payload) {
            k2_core::log_debug!("[recording] read audit {app}: {e}");
        }
    })
    .await;
}

/// One read with an already-resolved pass (`None` = a `k2skn_` that didn't
/// resolve).
pub async fn handle(path: &str, pass: Option<SkinPass>, link: Option<OnLink>, is_post: bool, params: &HashMap<String, String>) -> CliResponse {
    if is_post {
        return error(405, "usage", format!("{path} is a GET"), None);
    }
    let Some(pass) = pass else { return pass_invalid() };
    let Some(app) = app_of(&pass, link.as_ref()) else { return app_pass_required() };
    read_for_app(&app, path, params).await
}

// ── the feed ─────────────────────────────────────────────────────────

fn feeds() -> &'static Mutex<HashMap<String, broadcast::Sender<String>>> {
    static F: OnceLock<Mutex<HashMap<String, broadcast::Sender<String>>>> = OnceLock::new();
    F.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A change as the App's sockets get it: no content.
pub fn frame_of(c: &sew_wire::RecordingChange) -> String {
    json!({
        "kind": FRAME_CHANGED,
        "seq": c.seq,
        "change": c.kind,
        "recordingId": c.recording_id,
        "spaceId": c.space_id,
        "channelId": c.channel_id,
        "at": c.at,
    })
    .to_string()
}

/// What one long-poll did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Polled {
    /// Changes sent (how many).
    Sent(usize),
    /// The App may not read any more (server_read off, not governing,
    /// the flag gone): the feed ends.
    Ended(String),
    /// Sew didn't answer; try again later.
    Retry(String),
}

/// One long-poll of App `app`'s feed into `tx` (`wait_s`: Sew's `wait`).
pub async fn poll_once(app: &str, tx: &broadcast::Sender<String>, wait_s: u32) -> Polled {
    let gate = {
        let a = app.to_string();
        blocking(move || hook::recording_read_gate(&a, None)).await
    };
    match gate {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(r))) => return Polled::Ended(format!("{r:?}")),
        Ok(Err(e)) => return Polled::Retry(e),
        Err(_) => return Polled::Retry("worker join".into()),
    }
    let client = match chat_client().await {
        Ok(c) => c,
        Err(_) => return Polled::Retry("chat off".into()),
    };
    let cursor = {
        let a = app.to_string();
        match blocking(move || hook::recording_feed_cursor(&a)).await {
            Ok(Ok(c)) => c,
            _ => return Polled::Retry("cursor read".into()),
        }
    };
    let path = sew_wire::app_recording_changes_path(app, cursor.as_deref(), wait_s, wire::LIMIT_MAX);
    let timeout = long_poll_timeout(Duration::from_secs(u64::from(wait_s)));
    let r = match client.get_as_with_timeout(&path, &sew_wire::app_actor(app), timeout).await {
        Ok(r) => r,
        Err(e) => return Polled::Retry(e.0),
    };
    let reason = r.error().and_then(|e| e.error.detail_str("reason").map(str::to_string));
    if r.status == 403 || reason.as_deref() == Some(sew_wire::reasons::APP_NOT_GOVERNING) {
        return Polled::Ended(reason.unwrap_or_else(|| r.error_line()));
    }
    if !r.is_success() {
        return Polled::Retry(r.error_line());
    }
    let page: sew_wire::RecordingChangesPage = match serde_json::from_slice(&r.body) {
        Ok(p) => p,
        Err(e) => return Polled::Retry(format!("feed answer: {e}")),
    };
    for c in &page.items {
        let _ = tx.send(frame_of(c));
    }
    if !page.next.is_empty() {
        let (a, n) = (app.to_string(), page.next.clone());
        let _ = blocking(move || hook::recording_set_feed_cursor(&a, Some(&n))).await;
    }
    Polled::Sent(page.items.len())
}

/// Listen to App `app`'s feed; the first listener starts its follower,
/// which ends when the last one goes or the App may no longer read.
pub fn subscribe(app: &str) -> broadcast::Receiver<String> {
    let mut g = feeds().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(tx) = g.get(app) {
        return tx.subscribe();
    }
    let (tx, rx) = broadcast::channel(256);
    g.insert(app.to_string(), tx.clone());
    let app = app.to_string();
    tokio::spawn(async move {
        let mut backoff = Duration::from_secs(1);
        loop {
            if tx.receiver_count() == 0 {
                break;
            }
            match poll_once(&app, &tx, FEED_WAIT_S).await {
                Polled::Sent(_) => backoff = Duration::from_secs(1),
                Polled::Ended(why) => {
                    k2_core::log_debug!("[recording] feed {app} ended: {why}");
                    break;
                }
                Polled::Retry(why) => {
                    k2_core::log_debug!("[recording] feed {app}: {why}; again in {}s", backoff.as_secs());
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(60));
                }
            }
        }
        feeds().lock().unwrap_or_else(|e| e.into_inner()).remove(&app);
    });
    rx
}

async fn write_http(stream: &mut tokio::net::TcpStream, r: &CliResponse) {
    let resp = format!(
        "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{frame}Connection: close\r\n\r\n{}",
        r.status,
        r.body.len(),
        r.body,
        frame = k2_core::frame_policy::CONNECT_HEADER_LINES,
    );
    crate::routes::http::refuse_upgrade(stream, &resp).await;
}

/// `WS /cli/sew/app/recordings/events`, the App's own pass: refusals are
/// answered before the upgrade; then each change, and a re-check every
/// 30 s (server_read turned off closes the socket).
pub async fn serve_events(stream: &mut tokio::net::TcpStream, pass: Option<SkinPass>, link: Option<OnLink>, params: &HashMap<String, String>) {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let Some(pass) = pass else { return write_http(stream, &pass_invalid()).await };
    let Some(app) = app_of(&pass, link.as_ref()) else { return write_http(stream, &app_pass_required()).await };
    // The socket's `app` (the SDK sends the App's own service id): another
    // App's is the same 404 as nothing.
    if param(params, "app").is_some_and(|a| a != app) {
        return write_http(stream, &error(404, "not_found", "this socket is the App's own whose pass this is: app=<its service id>", None)).await;
    }
    let gate = {
        let a = app.clone();
        blocking(move || hook::recording_read_gate(&a, None)).await
    };
    match gate {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(r))) => return write_http(stream, &refusal(&r)).await,
        Ok(Err(e)) => return write_http(stream, &error(503, "chat_unavailable", e, None)).await,
        Err(r) => return write_http(stream, &r).await,
    }
    let ws = match tokio_tungstenite::accept_async(&mut *stream).await {
        Ok(ws) => ws,
        Err(e) => {
            k2_core::log_debug!("[recording] events handshake: {e}");
            return;
        }
    };
    let (mut write, mut read) = ws.split();
    let mut rx = subscribe(&app);
    let mut recheck = tokio::time::interval(Duration::from_secs(30));
    recheck.tick().await;
    loop {
        tokio::select! {
            m = rx.recv() => match m {
                Ok(text) => {
                    if write.send(Message::Text(text)).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let _ = write.send(Message::Text(json!({ "kind": FRAME_REFETCH }).to_string())).await;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    let _ = write.send(Message::Close(None)).await;
                    break;
                }
            },
            m = read.next() => match m {
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            },
            _ = recheck.tick() => {
                let a = app.clone();
                let still = matches!(blocking(move || hook::recording_read_gate(&a, None)).await, Ok(Ok(Ok(()))));
                if !still {
                    let _ = write.send(Message::Close(None)).await;
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(kv: &[(&str, &str)]) -> HashMap<String, String> {
        kv.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn reads_parse_their_own_params_only() {
        assert_eq!(
            Read::parse(LIST, &q(&[("space", "5101"), ("after", "71"), ("limit", "10")])).unwrap_or_else(|r| panic!("{}", r.body)),
            Read::List { space: "5101".into(), after: Some("71".into()), limit: 10 }
        );
        assert!(Read::parse(LIST, &q(&[])).is_err(), "the Space is named");
        assert!(Read::parse(LIST, &q(&[("space", "../x")])).is_err());
        assert!(Read::parse(ONE, &q(&[("id", "a/b")])).is_err());
        assert!(Read::parse(TRANSCRIPT, &q(&[("id", "71"), ("limit", "0")])).is_err());
        assert!(Read::parse(TRANSCRIPT, &q(&[("id", "71"), ("limit", "201")])).is_err());
        let t = Read::parse(TRANSCRIPT, &q(&[("id", "71")])).unwrap_or_else(|r| panic!("{}", r.body));
        assert_eq!(t.mgmt_path("svc"), "/mgmt/v1/recordings/71/transcript?app_id=svc&limit=50");
    }

    #[test]
    fn a_feed_frame_carries_no_content() {
        let c = sew_wire::RecordingChange {
            seq: 3,
            kind: "transcript_ready".into(),
            recording_id: "71".into(),
            space_id: "5101".into(),
            channel_id: "9".into(),
            at: "t".into(),
        };
        let v: J = serde_json::from_str(&frame_of(&c)).unwrap();
        assert_eq!(
            v,
            json!({"kind": "recording_changed", "seq": 3, "change": "transcript_ready", "recordingId": "71", "spaceId": "5101", "channelId": "9", "at": "t"})
        );
    }
}
