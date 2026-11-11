//! K2's `JoinProvider`: this person's saved login → `POST /cli/voice/join` on
//! the hosting daemon → Sew's join answer, for the engine (prd-k2-voice-rooms-v1
//! §14.4; vs-live V30). The person's answer to a recording notice goes to
//! `POST /cli/voice/recording/consent`, their Record and Stop (Sew 0.9.0) to
//! `POST /cli/voice/recording/start|stop`, the same way.
//!
//! No product logic: no room state, no logging in (a refused login answers
//! `signin_required`, and the renderer's login coordinator signs in again), no
//! session routes (since sew-57 the engine does leave, mute, stats and renewal
//! itself with the call credential). The daemon owns the rooms and every check.
//!
//! **Secrets.** The login token and the join answer stay in Rust memory. Nothing
//! here logs them, and no error or refusal carries them: errors are fixed text
//! and a refusal's detail is what the server said, dropped if it quotes a token.

use std::time::Duration;

use serde_json::{json, Value};
use tauri_plugin_sew_voice::sew_call_client::Recording;
use tauri_plugin_sew_voice::{
    BoxFuture, ConsentAnswer, ConsentRequired, JoinAnswer, JoinProvider, JoinRequest, ProviderError, RecordingControl, RecordingRequest,
};

/// The longest a daemon can take to answer a join (`voice/bff.rs`: a
/// reconnecting lane, a level sync, the member gate and Sew's answer add up to
/// 51 s). Kept above that, so an answer never lands after K2 gave up.
pub const JOIN_TIMEOUT: Duration = Duration::from_secs(55);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// The daemon lane the engine reaches Sew through (§14.11).
pub const API_BASE_PATH: &str = "/_k2/voice/sew/";

/// The `target` that means this computer's own daemon, whose only credential
/// is the owner token. The server refuses that token a seat anyway
/// (`owner_token_cannot_join`); K2 doesn't send it.
pub const LOCAL_TARGET: &str = "local";

/// A saved login: where the server is and the token to use there.
#[derive(Clone)]
pub struct Login {
    /// `https://host` or `http://host:port`: no trailing slash.
    pub base_url: String,
    pub token: String,
}

/// Where logins come from: the keychain in the app, a table in tests.
pub trait LoginSource: Send + Sync + 'static {
    /// `target` is the saved host's id. Blocking: it may read the keychain.
    fn login(&self, target: &str) -> Result<Login, ProviderError>;
}

pub struct K2Provider<L: LoginSource> {
    logins: std::sync::Arc<L>,
    http: reqwest::Client,
}

impl<L: LoginSource> K2Provider<L> {
    pub fn new(logins: L) -> Result<Self, reqwest::Error> {
        let http = reqwest::Client::builder().connect_timeout(CONNECT_TIMEOUT).timeout(JOIN_TIMEOUT).build()?;
        Ok(Self { logins: std::sync::Arc::new(logins), http })
    }

    async fn join_inner(&self, req: JoinRequest) -> Result<JoinAnswer, ProviderError> {
        if req.target == LOCAL_TARGET {
            return Err(refused("owner_token_cannot_join", "this computer's K2 has no login of yours to join with"));
        }
        let logins = std::sync::Arc::clone(&self.logins);
        let target = req.target.clone();
        let login = tokio::task::spawn_blocking(move || logins.login(&target))
            .await
            .map_err(|_| ProviderError::Other("reading the saved login failed".into()))??;
        let body = join_body(&req);
        let url = format!("{}/cli/voice/join", login.base_url);
        let answer = self.http.post(&url).bearer_auth(&login.token).json(&body).send().await;
        let res = match answer {
            Ok(r) => r,
            Err(e) if e.is_timeout() => return Err(refused("join_timeout", "the server didn't answer in time")),
            // Not `e`'s text: nothing from a failed request goes up the chain.
            Err(_) => return Err(ProviderError::Unavailable { retry_after_ms: None }),
        };
        let status = res.status().as_u16();
        let retry_after = res.headers().get(reqwest::header::RETRY_AFTER).and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<u64>().ok());
        let bytes = match res.bytes().await {
            Ok(b) => b,
            Err(e) if e.is_timeout() => return Err(refused("join_timeout", "the server didn't answer in time")),
            Err(_) => return Err(ProviderError::Unavailable { retry_after_ms: None }),
        };
        answer_from(status, retry_after, &bytes, &login)
    }

    /// `POST <base>/cli/voice/<route>` with this person's login, for the
    /// recording routes: `(status, Retry-After secs, body, login)`.
    async fn post_person(&self, target: &str, route: &str, body: Value) -> Result<(u16, Option<u64>, Vec<u8>, Login), ProviderError> {
        if target == LOCAL_TARGET {
            return Err(refused("owner_token_cannot_join", "this computer's K2 has no login of yours to answer with"));
        }
        let logins = std::sync::Arc::clone(&self.logins);
        let t = target.to_string();
        let login = tokio::task::spawn_blocking(move || logins.login(&t))
            .await
            .map_err(|_| ProviderError::Other("reading the saved login failed".into()))??;
        let url = format!("{}/cli/voice/{route}", login.base_url);
        let res = match self.http.post(&url).bearer_auth(&login.token).json(&body).send().await {
            Ok(r) => r,
            Err(e) if e.is_timeout() => return Err(refused("join_timeout", "the server didn't answer in time")),
            Err(_) => return Err(ProviderError::Unavailable { retry_after_ms: None }),
        };
        let status = res.status().as_u16();
        let retry_after = res.headers().get(reqwest::header::RETRY_AFTER).and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<u64>().ok());
        let bytes = res.bytes().await.map_err(|_| ProviderError::Unavailable { retry_after_ms: None })?;
        Ok((status, retry_after, bytes.to_vec(), login))
    }

    async fn consent_inner(&self, answer: ConsentAnswer) -> Result<Recording, ProviderError> {
        let (status, retry, body, login) = self.post_person(&answer.target, "recording/consent", consent_body(&answer)).await?;
        recording_from(status, retry, &body, &login)
    }

    async fn recording_inner(&self, req: RecordingRequest) -> Result<Recording, ProviderError> {
        let (route, body) = recording_body(&req)?;
        let (status, retry, bytes, login) = self.post_person(&req.target, route, body).await?;
        recording_from(status, retry, &bytes, &login)
    }
}

impl<L: LoginSource> JoinProvider for K2Provider<L> {
    fn join(&self, req: JoinRequest) -> BoxFuture<'_, Result<JoinAnswer, ProviderError>> {
        Box::pin(self.join_inner(req))
    }

    fn consent(&self, answer: ConsentAnswer) -> BoxFuture<'_, Result<Recording, ProviderError>> {
        Box::pin(self.consent_inner(answer))
    }

    fn recording(&self, req: RecordingRequest) -> BoxFuture<'_, Result<Recording, ProviderError>> {
        Box::pin(self.recording_inner(req))
    }
}

/// `POST /cli/voice/recording/consent`'s body (`voice/bff.rs` `consent_body`).
pub fn consent_body(a: &ConsentAnswer) -> Value {
    let mut b = json!({ "recording_id": a.recording_id.to_string(), "consent": a.consent });
    if let Some(v) = a.notice_version {
        b["notice_version"] = json!(v);
    }
    b
}

/// Record → `recording/start {room}`; Stop → `recording/stop {recordingId}`.
/// The daemon maps the room (a slug, or Sew's channel id after a move) to
/// the channel itself.
pub fn recording_body(req: &RecordingRequest) -> Result<(&'static str, Value), ProviderError> {
    match (req.action, req.recording_id) {
        (RecordingControl::Start, _) => Ok(("recording/start", json!({ "room": req.room }))),
        (RecordingControl::Stop, Some(id)) => Ok(("recording/stop", json!({ "recordingId": id.to_string() }))),
        (RecordingControl::Stop, None) => Err(refused("no_recording", "there is no recording to stop")),
        _ => Err(refused("recording_unsupported", "this K2 doesn't know that recording action")),
    }
}

/// The daemon's answer to a consent, Record or Stop: `{ok, recording}` →
/// Sew's recording; a changed notice → [`ProviderError::ConsentRequired`];
/// a refusal → its code (Sew's reason kept in `details`).
pub fn recording_from(status: u16, retry_after_secs: Option<u64>, body: &[u8], login: &Login) -> Result<Recording, ProviderError> {
    match status {
        200 => {
            let v: Value = serde_json::from_slice(body).map_err(|_| ProviderError::Other("the server's recording answer did not parse".into()))?;
            let rec = v.get("recording").filter(|r| r.is_object()).ok_or_else(|| ProviderError::Other("the server's answer carried no recording".into()))?;
            let bytes = serde_json::to_vec(rec).map_err(|_| ProviderError::Other("the server's recording answer did not parse".into()))?;
            ConsentAnswer::recording_from_json(&bytes)
        }
        401 => Err(ProviderError::SigninRequired),
        _ => {
            let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
            if let Some(c) = consent_required(&v) {
                return Err(ProviderError::ConsentRequired(Box::new(c)));
            }
            match v.pointer("/error/code").or_else(|| v.get("code")).and_then(Value::as_str) {
                Some(code) => {
                    let hint = v.pointer("/error/hint").or_else(|| v.get("hint")).cloned().unwrap_or(Value::Null);
                    let details = v.pointer("/error/details").or_else(|| v.get("details")).cloned().unwrap_or(Value::Null);
                    Err(ProviderError::Refused { code: code.to_string(), detail: clean_detail(json!({ "hint": hint, "details": details }), &login.token) })
                }
                None if matches!(status, 429 | 502 | 503 | 504) => {
                    Err(ProviderError::Unavailable { retry_after_ms: retry_after_secs.map(|s| s.saturating_mul(1000)) })
                }
                None => Err(ProviderError::Other(format!("the server answered {status}"))),
            }
        }
    }
}

fn refused(code: &str, hint: &str) -> ProviderError {
    ProviderError::Refused { code: code.into(), detail: json!({ "hint": hint, "details": null }) }
}

/// `POST /cli/voice/join`'s body: Sew's join with the call credential and no
/// `e2ee`, the same for a join and every re-join. The person's answer to a
/// recording notice (Sew's `recording_notice`) rides in the join when the
/// engine has one, so a re-join after a yes isn't refused again; the daemon
/// passes `join` to Sew as is.
pub fn join_body(req: &JoinRequest) -> Value {
    let mut join = json!({ "call_credential": true, "self_mute": req.muted || req.deafened, "self_deaf": req.deafened });
    if let Some(notice) = &req.recording_notice {
        join["recording_notice"] = json!(notice);
    }
    json!({ "room": req.room, "join": join })
}

/// The door the engine uses for this login's server.
pub fn api_base(base_url: &str) -> Result<url::Url, ProviderError> {
    url::Url::parse(&format!("{base_url}{API_BASE_PATH}")).map_err(|_| ProviderError::Other("the saved server address isn't a URL".into()))
}

/// What the daemon answered, as the plugin's types.
pub fn answer_from(status: u16, retry_after_secs: Option<u64>, body: &[u8], login: &Login) -> Result<JoinAnswer, ProviderError> {
    match status {
        200 => {
            let bad = || ProviderError::Other("the server's join answer did not parse".into());
            let v: Value = serde_json::from_slice(body).map_err(|_| bad())?;
            let join = v.get("join").filter(|j| j.is_object()).ok_or_else(bad)?;
            let bytes = serde_json::to_vec(join).map_err(|_| bad())?;
            let answer = JoinAnswer::from_json(&bytes, api_base(&login.base_url)?).map_err(|_| bad())?;
            if answer.call_credential().is_none() {
                return Err(refused("voice_server_too_old", "this server's Sew doesn't give native calls a call credential yet"));
            }
            Ok(answer)
        }
        401 => Err(ProviderError::SigninRequired),
        _ => {
            let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
            if let Some(c) = consent_required(&v) {
                return Err(ProviderError::ConsentRequired(Box::new(c)));
            }
            let code = v.pointer("/error/code").or_else(|| v.get("code")).and_then(Value::as_str);
            match code {
                Some(code) => {
                    let hint = v.pointer("/error/hint").or_else(|| v.get("hint")).cloned().unwrap_or(Value::Null);
                    let details = v.pointer("/error/details").or_else(|| v.get("details")).cloned().unwrap_or(Value::Null);
                    let detail = clean_detail(json!({ "hint": hint, "details": details }), &login.token);
                    Err(ProviderError::Refused { code: code.to_string(), detail })
                }
                None if matches!(status, 429 | 502 | 503 | 504) => {
                    Err(ProviderError::Unavailable { retry_after_ms: retry_after_secs.map(|s| s.saturating_mul(1000)) })
                }
                None => Err(ProviderError::Other(format!("the server answered {status}"))),
            }
        }
    }
}

/// Sew 0.7.3's recorded room: the reason, not the code, says so (Sew answers
/// `409 conflict` with `details.reason = recording_consent_required`). Only
/// [`ProviderError::ConsentRequired`] makes the engine hold the join for the
/// person's answer. The daemon passes Sew's details through as its own
/// (`error.details`); a daemon from before 0.46.9 nests Sew's error under
/// `error.details.sew`. A refusal without a prompt stays a plain refusal.
fn consent_required(v: &Value) -> Option<ConsentRequired> {
    ["/error/details", "/error/details/sew/details"].iter().find_map(|p| {
        let d = v.pointer(p)?;
        (d.get("reason").and_then(Value::as_str) == Some("recording_consent_required")).then(|| ConsentRequired::from_details(d)).flatten()
    })
}

/// A refusal's detail reaches the webview. Sew's refusals carry no token, so a
/// detail that quotes one is a bug somewhere: send none of it.
fn clean_detail(detail: Value, login_token: &str) -> Value {
    let text = detail.to_string();
    if text.contains("sewcc_") || (!login_token.is_empty() && text.contains(login_token)) {
        return json!({ "hint": null, "details": null });
    }
    detail
}

/// The app's logins: `~/.k2/connect-hosts.json` for the address, the keychain
/// for the token, `~/.k2/connect-tokens.json` by hostname when the keychain has
/// none (a Linux desktop with no Secret Service; every login writes both).
pub struct SavedLogins;

const KEYCHAIN_SERVICE: &str = "dev.k2.connect.host-token";
const LEGACY_KEYCHAIN_SERVICE: &str = "com.k2so.connect.host-token";

/// `hostBaseUrl` in `stores/connect-host.ts`: secure on 443 omits the port.
pub fn host_base_url(hostname: &str, port: u64, secure: bool) -> String {
    let scheme = if secure { "https" } else { "http" };
    if secure && port == 443 {
        format!("{scheme}://{hostname}")
    } else {
        format!("{scheme}://{hostname}:{port}")
    }
}

/// One saved host out of `connect-hosts.json`'s text.
pub fn find_host(hosts_json: &str, target: &str) -> Option<(String, String)> {
    let hosts: Vec<Value> = serde_json::from_str(hosts_json).ok()?;
    let h = hosts.iter().find(|h| h.get("id").and_then(Value::as_str) == Some(target))?;
    let hostname = h.get("hostname").and_then(Value::as_str)?.to_string();
    let port = h.get("port").and_then(Value::as_u64)?;
    // Absent reads as false, as `loadHosts` does.
    let secure = h.get("secure").and_then(Value::as_bool).unwrap_or(false);
    Some((host_base_url(&hostname, port, secure), hostname))
}

fn keychain_token(target: &str) -> Option<String> {
    for service in [KEYCHAIN_SERVICE, LEGACY_KEYCHAIN_SERVICE] {
        let Ok(entry) = keyring::Entry::new(service, target) else { continue };
        if let Ok(token) = entry.get_password() {
            if !token.is_empty() {
                return Some(token);
            }
        }
    }
    None
}

impl LoginSource for SavedLogins {
    fn login(&self, target: &str) -> Result<Login, ProviderError> {
        let hosts = crate::commands::connect_hosts::connect_hosts_read().map_err(|_| ProviderError::Other("the saved server list can't be read".into()))?;
        // A host that isn't saved has no login: sign in.
        let (base_url, hostname) = find_host(&hosts, target).ok_or(ProviderError::SigninRequired)?;
        let token = keychain_token(target)
            .or_else(|| crate::commands::connect_hosts::read_cli_token(&hostname))
            .ok_or(ProviderError::SigninRequired)?;
        Ok(Login { base_url, token })
    }
}
