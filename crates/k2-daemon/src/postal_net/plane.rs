//! The Postal plane over HTTPS (K2 Web's `/api/postal/v1`, plane contract
//! `docs/postal-plane-v1.md`): the transport postal-sdk's sans-IO
//! `plane` requests go through, the box's session, and the owner's
//! decide token.
//!
//! **Credentials, and who may use which:**
//! - **The box** (agent-grade): `POST /session` with the box's K2 Connect
//!   credential (`tunnel.token`, from the vault) answers a 1-hour `pst_`
//!   for the box's own label (P-2). Every box call (`/me`, pairs, respond,
//!   reveal, hold, events) uses it; a 401 mints a new one once.
//! - **The owner** (people-grade): the `k2c_pd_` postal-decide token the
//!   owner approved for this label (the plane's device grant, P-7), kept in
//!   the vault and used only for `confirm` and `revoke`, and only after a
//!   person's answer. A `pst_` there is `owner_required`.
//!
//! Neither token, nor any body, is ever logged.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use k2_core::postal::sdk::plane::{self, Method, PlaneError, Request};

/// The plane's base URL (no path): `K2_POSTAL_PLANE_URL`, else K2 Web's
/// production host. The `/api/postal/v1` suffix is dropped if given.
pub fn base_url() -> String {
    let raw = std::env::var("K2_POSTAL_PLANE_URL").ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let raw = raw.unwrap_or_else(|| k2_core::hosts::WEB_API_BASE.to_string());
    raw.trim_end_matches('/').trim_end_matches(plane::API).trim_end_matches('/').to_string()
}

/// What a plane call came back with.
pub type Answer = (u16, Vec<u8>);

/// HTTP for the plane. The daemon's is [`Https`]; tests bring their own.
pub trait Transport: Send + Sync {
    /// One v1 call with `bearer`. `Err` is a transport failure (no answer).
    fn call(&self, bearer: &str, req: &Request) -> Result<Answer, String>;
    /// An unversioned JSON POST with no bearer (the device grant).
    fn post_open(&self, path: &str, body: &serde_json::Value) -> Result<Answer, String>;
}

/// reqwest, blocking (the Postal loop is a plain thread).
pub struct Https {
    base: String,
    client: reqwest::blocking::Client,
}

impl Https {
    pub fn new(base: String) -> Result<Self, String> {
        let mut builder = reqwest::blocking::Client::builder()
            // Long enough for a 25 s events long poll.
            .timeout(Duration::from_secs(45))
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("k2-postal/", env!("CARGO_PKG_VERSION")));
        // Loopback (127.0.0.1, ::1, localhost) and plain http never use a
        // proxy; the plane's remote https host may use this box's proxy
        // settings (k2_core::proxy_rule). Each call is still sent once.
        if k2_core::proxy_rule::direct(&base) {
            builder = builder.no_proxy();
        }
        let client = builder.build().map_err(|e| format!("postal http client: {e}"))?;
        Ok(Self { base, client })
    }
}

impl Transport for Https {
    fn call(&self, bearer: &str, req: &Request) -> Result<Answer, String> {
        let url = format!("{}{}", self.base, req.path);
        let rb = match req.method {
            Method::Get => self.client.get(&url),
            Method::Post => self.client.post(&url),
            Method::Put => self.client.put(&url),
        };
        let rb = rb.bearer_auth(bearer).header("Content-Type", "application/json");
        let rb = match &req.body {
            Some(b) => rb.body(b.clone()),
            None => rb,
        };
        // The path only, never the query or the body: a cursor or an id
        // is harmless, but keep the log line short and fixed.
        let path = req.path.split('?').next().unwrap_or("").to_string();
        let resp = rb.send().map_err(|e| format!("postal plane {path}: {}", short(&e.to_string())))?;
        let status = resp.status().as_u16();
        let api = resp.headers().get("Postal-Api").and_then(|v| v.to_str().ok()).map(str::to_string);
        let body = resp.bytes().map_err(|e| format!("postal plane {path}: {}", short(&e.to_string())))?.to_vec();
        if api.as_deref() != Some("1") {
            return Err(format!("postal plane {path}: answered {status} without Postal-Api: 1 (not a Postal plane v1)"));
        }
        Ok((status, body))
    }

    fn post_open(&self, path: &str, body: &serde_json::Value) -> Result<Answer, String> {
        let url = format!("{}{path}", self.base);
        let resp = self.client.post(&url).json(body).send().map_err(|e| format!("postal plane {path}: {}", short(&e.to_string())))?;
        let status = resp.status().as_u16();
        let body = resp.bytes().map_err(|e| format!("postal plane {path}: {}", short(&e.to_string())))?.to_vec();
        Ok((status, body))
    }
}

fn short(s: &str) -> String {
    s.chars().take(160).collect()
}

/// Why a call couldn't be made or failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallError {
    /// No answer (DNS, TLS, timeout, not a Postal plane).
    Transport(String),
    /// A v1 error answer.
    Plane(PlaneError),
    /// This box has no K2 Connect credential (not paired with K2 Connect).
    NoConnect(String),
    /// The owner hasn't linked a decide token for this label yet.
    NoOwnerLink(String),
}

impl CallError {
    pub fn code(&self) -> String {
        match self {
            CallError::Transport(_) => "postal_unreachable".into(),
            CallError::Plane(e) => match e.code() {
                Some(plane::Code::Other(c)) => c.clone(),
                Some(c) => format!("{c:?}").chars().fold(String::new(), |mut s, ch| {
                    if ch.is_uppercase() && !s.is_empty() {
                        s.push('_');
                    }
                    s.push(ch.to_ascii_lowercase());
                    s
                }),
                None => "postal_plane_answer".into(),
            },
            CallError::NoConnect(_) => "postal_no_connect".into(),
            CallError::NoOwnerLink(_) => "postal_owner_link_missing".into(),
        }
    }

    pub fn hint(&self) -> String {
        match self {
            CallError::Transport(e) => format!("the Postal network didn't answer: {e}"),
            CallError::Plane(e) => e.to_string(),
            CallError::NoConnect(e) => e.clone(),
            CallError::NoOwnerLink(label) => format!(
                "this server's owner hasn't linked Postal approvals for {label} yet: the owner runs {} postal owner-link once (it opens a page to approve)",
                k2_core::cli_name!()
            ),
        }
    }

    /// Worth trying again later.
    pub fn is_retryable(&self) -> bool {
        match self {
            CallError::Transport(_) => true,
            CallError::Plane(e) => e.is_retryable(),
            CallError::NoConnect(_) | CallError::NoOwnerLink(_) => false,
        }
    }
}

/// The box's Connect credential and label (`tunnel.json` + the vault).
pub fn connect_credential() -> Result<(String, String), CallError> {
    let cfg = k2_core::tunnel::config::load().map_err(|e| CallError::NoConnect(format!("this server's {} Connect settings can't be read: {e}", k2_core::product_name!())))?;
    let token = cfg.token.trim().to_string();
    let label = cfg.subdomain.trim().to_ascii_lowercase();
    if token.is_empty() || label.is_empty() {
        return Err(CallError::NoConnect(format!(
            "Postal uses this server's {p} Connect account, and this server isn't signed in to {p} Connect",
            p = k2_core::product_name!()
        )));
    }
    Ok((token, label))
}

/// What a decide-token mint came back with. No `Debug`: the token never
/// reaches a log line.
pub enum Mint {
    /// A new token for `label` (shown once: store it, never log it).
    Minted { token: String, label: String },
    /// The plane holds an active one already (`409 already_minted`).
    AlreadyMinted,
}

struct Session {
    token: String,
    label: String,
    until: Instant,
}

/// One box's plane access.
pub struct Plane {
    pub transport: Box<dyn Transport>,
    /// Where the Connect credential comes from (tests give their own).
    pub credential: Box<dyn Fn() -> Result<(String, String), CallError> + Send + Sync>,
    session: Mutex<Option<Session>>,
}

/// A session is used for 50 of its 60 minutes.
const SESSION_USE: Duration = Duration::from_secs(50 * 60);

impl Plane {
    pub fn new(transport: Box<dyn Transport>, credential: Box<dyn Fn() -> Result<(String, String), CallError> + Send + Sync>) -> Self {
        Self { transport, credential, session: Mutex::new(None) }
    }

    /// The daemon's: HTTPS to [`base_url`], the Connect credential from
    /// this server's K2 Connect settings.
    pub fn daemon() -> Result<Self, String> {
        Ok(Self::new(Box::new(Https::new(base_url())?), Box::new(connect_credential)))
    }

    fn mint(&self) -> Result<(String, String), CallError> {
        let (connect, _) = (self.credential)()?;
        let req = Request { method: Method::Post, path: format!("{}/session", plane::API), body: Some("{}".into()) };
        let (status, body) = self.transport.call(&connect, &req).map_err(CallError::Transport)?;
        #[derive(serde::Deserialize)]
        struct S {
            token: String,
            label: String,
        }
        let s: S = plane::parse(status, &body).map_err(CallError::Plane)?;
        if !s.token.starts_with("pst_") {
            return Err(CallError::Plane(PlaneError::BadBody("session token is not a pst_".into())));
        }
        let mut g = self.session.lock().unwrap_or_else(|p| p.into_inner());
        *g = Some(Session { token: s.token.clone(), label: s.label.to_ascii_lowercase(), until: Instant::now() + SESSION_USE });
        Ok((s.token, s.label.to_ascii_lowercase()))
    }

    /// The live session (`pst_`, label), minted when there is none.
    pub fn session(&self) -> Result<(String, String), CallError> {
        {
            let g = self.session.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(s) = g.as_ref().filter(|s| s.until > Instant::now()) {
                return Ok((s.token.clone(), s.label.clone()));
            }
        }
        self.mint()
    }

    /// This box's label on the Postal network (its K2 Connect label).
    pub fn label(&self) -> Result<String, CallError> {
        self.session().map(|(_, l)| l)
    }

    fn forget(&self) {
        *self.session.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }

    /// A box call; a 401 mints a fresh session and tries once more.
    pub fn box_call(&self, req: &Request) -> Result<Answer, CallError> {
        let (token, _) = self.session()?;
        let a = self.transport.call(&token, req).map_err(CallError::Transport)?;
        if a.0 != 401 {
            return Ok(a);
        }
        self.forget();
        let (token, _) = self.mint()?;
        self.transport.call(&token, req).map_err(CallError::Transport)
    }

    /// A box call parsed as `T`.
    pub fn box_json<T: for<'de> serde::Deserialize<'de>>(&self, req: &Request) -> Result<T, CallError> {
        let (s, b) = self.box_call(req)?;
        plane::parse(s, &b).map_err(CallError::Plane)
    }

    /// An owner call (confirm, revoke) with the decide token of `label`.
    pub fn owner_call(&self, label: &str, req: &Request) -> Result<Answer, CallError> {
        let token = k2_core::postal::keys::decide_token(label)
            .map_err(CallError::NoConnect)?
            .ok_or_else(|| CallError::NoOwnerLink(label.to_string()))?;
        self.transport.call(&token, req).map_err(CallError::Transport)
    }

    /// The Connect label this box's credential names (no plane call).
    pub fn connect_label(&self) -> Result<String, CallError> {
        (self.credential)().map(|(_, l)| l.trim().to_ascii_lowercase())
    }

    /// `POST /decide-token` with the box's Connect credential: the owner's
    /// `k2c_pd_` decide token for this box's label. The plane mints one at
    /// a time: `409 already_minted` while one is active.
    pub fn decide_token_mint(&self) -> Result<Mint, CallError> {
        let (connect, label) = (self.credential)()?;
        let req = Request { method: Method::Post, path: format!("{}/decide-token", plane::API), body: Some("{}".into()) };
        let (status, body) = self.transport.call(&connect, &req).map_err(CallError::Transport)?;
        #[derive(serde::Deserialize)]
        struct M {
            token: String,
            #[serde(default)]
            label: String,
        }
        let m: M = match plane::parse(status, &body) {
            Ok(m) => m,
            Err(e) => {
                let e = CallError::Plane(e);
                return if e.code() == "already_minted" { Ok(Mint::AlreadyMinted) } else { Err(e) };
            }
        };
        if !m.token.starts_with("k2c_pd_") {
            return Err(CallError::Plane(PlaneError::BadBody("decide token is not a k2c_pd_".into())));
        }
        let label = if m.label.trim().is_empty() { label } else { m.label };
        Ok(Mint::Minted { token: m.token, label: label.trim().to_ascii_lowercase() })
    }

    /// `POST /decide-token/revoke` with the box's Connect credential: how
    /// many tokens it revoked (none is fine: it is idempotent).
    pub fn decide_token_revoke(&self) -> Result<u64, CallError> {
        let (connect, _) = (self.credential)()?;
        let req = Request { method: Method::Post, path: format!("{}/decide-token/revoke", plane::API), body: Some("{}".into()) };
        let (status, body) = self.transport.call(&connect, &req).map_err(CallError::Transport)?;
        #[derive(serde::Deserialize)]
        struct R {
            revoked: u64,
        }
        plane::parse::<R>(status, &body).map(|r| r.revoked).map_err(CallError::Plane)
    }

    /// Start the owner's postal-decide device grant for `label` (RFC 8628):
    /// `(device_code, user_code, verification_uri_complete, interval)`.
    pub fn owner_link_start(&self, label: &str) -> Result<(String, String, String, u64), CallError> {
        let (s, b) = self
            .transport
            .post_open("/api/postal/cli/device", &serde_json::json!({ "client": "k2", "scope": "postal-decide", "label": label }))
            .map_err(CallError::Transport)?;
        let v: serde_json::Value = serde_json::from_slice(&b).unwrap_or(serde_json::Value::Null);
        if s != 200 {
            let code = v["error"].as_str().unwrap_or("device_grant_refused").to_string();
            return Err(CallError::Plane(PlaneError::Api { status: s, code: plane::Code::Other(code), message: String::new(), retry_after_s: None }));
        }
        let field = |k: &str| v[k].as_str().map(str::to_string);
        match (field("device_code"), field("user_code"), field("verification_uri_complete").or_else(|| field("verification_uri"))) {
            (Some(d), Some(u), Some(url)) => Ok((d, u, url, v["interval"].as_u64().unwrap_or(5).clamp(1, 60))),
            _ => Err(CallError::Plane(PlaneError::BadBody("device grant answer".into()))),
        }
    }

    /// One poll of the device grant: `Ok(Some(token))` once approved,
    /// `Ok(None)` while pending, `Err` when it ended (denied, expired).
    pub fn owner_link_poll(&self, device_code: &str) -> Result<Option<String>, String> {
        let (s, b) = self.transport.post_open(
            "/api/postal/cli/device/token",
            &serde_json::json!({ "device_code": device_code, "grant_type": "urn:ietf:params:oauth:grant-type:device_code" }),
        )?;
        let v: serde_json::Value = serde_json::from_slice(&b).unwrap_or(serde_json::Value::Null);
        if s == 200 {
            return match (v["token"].as_str(), v["scope"].as_str()) {
                (Some(t), Some("postal-decide")) if t.starts_with("k2c_pd_") => Ok(Some(t.to_string())),
                _ => Err("the plane answered a token that isn't a postal-decide token".into()),
            };
        }
        match v["error"].as_str() {
            Some("authorization_pending") | Some("slow_down") => Ok(None),
            Some(e) => Err(e.to_string()),
            None => Err(format!("device grant poll answered {s}")),
        }
    }
}
