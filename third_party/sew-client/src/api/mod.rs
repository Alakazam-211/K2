//! The client and its request pipeline (S25 §6): build → auth headers → idempotency key → send →
//! meta → error mapping → retry decision → loop.

use std::fmt;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures_util::future::BoxFuture;
use http::Method;
use serde::Serialize;
use serde::de::DeserializeOwned;
use sew_proto::clock::Clock;
use sew_proto::http::IdempotencyKey;
use sew_proto::id::Id;

use crate::config::{ClientBuilder, RequestOptions};
use crate::error::{ClientError, from_response, snippet};
use crate::meta::{RawResponse, Response, ResponseMeta};
use crate::retry::{Decision, Outcome, RetryPolicy, decide, new_idempotency_key};
use crate::secret::SecretString;
use crate::session::TokenCell;
use crate::transport::{Auth, Transport, Wire, WireResp, tcp};

pub(crate) mod auth;

/// Public routes that take a pass or password: never sent a Bearer.
const LOGIN_ROUTES: [&str; 3] = [
    sew_proto::routes::AUTH_K2,
    sew_proto::routes::AUTH_LOGIN,
    sew_proto::routes::AUTH_PASSWORD_SET,
];

/// `/api/v1/auth/*`: no idempotency key; retried only when the request never left (§8.1).
const AUTH_PREFIX: &str = "/api/v1/auth/";

pub(crate) struct Inner {
    pub(crate) transport: Transport,
    pub(crate) retry: RetryPolicy,
    pub(crate) request_timeout: Duration,
    pub(crate) user_agent: String,
    pub(crate) clock: Arc<dyn Clock>,
}

impl fmt::Debug for Inner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Inner")
            .field("transport", &self.transport.describe())
            .field("retry", &self.retry)
            .field("request_timeout", &self.request_timeout)
            .finish_non_exhaustive()
    }
}

/// The Sew client. Cheap to clone (an `Arc` inside); clones share the session token cell.
///
/// Every typed method is a thin wrapper around [`Client::call`]. Mutating methods take
/// [`RequestOptions`] last (S8 and S10 methods keep their first-cut signatures and use the
/// defaults). Every `POST` except `/api/v1/auth/*` carries an `Idempotency-Key` that is reused
/// on every retry of the call, with byte-identical bodies.
#[derive(Clone)]
pub struct Client {
    pub(crate) inner: Arc<Inner>,
    pub(crate) auth: Auth,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut d = f.debug_struct("Client");
        d.field("base", &self.inner.transport.describe());
        match &self.auth {
            Auth::Session(c) => {
                d.field("token", &c.token().map(|_| "***"));
                d.field("member_id", &c.member_id());
            }
            Auth::Service { act_as, .. } => {
                d.field("service_token", &"***");
                d.field(
                    "act_as",
                    &act_as.as_ref().map(|(t, v)| format!("{t} via {v}")),
                );
            }
        }
        d.finish()
    }
}

/// `true` for a 2xx status.
fn is_success(status: u16) -> bool {
    (200..300).contains(&status)
}

impl Client {
    /// A builder for `base_url` (`http(s)://host[:port][/prefix]`).
    pub fn builder(base_url: &str) -> ClientBuilder {
        ClientBuilder::new(base_url)
    }

    /// A client for `base_url` with no token (S8 name kept).
    pub fn new(base_url: &str) -> Result<Client, ClientError> {
        ClientBuilder::new(base_url).build()
    }

    pub(crate) fn from_builder(b: ClientBuilder) -> Result<Client, ClientError> {
        let base = normalize_base(&b.base)?;
        let http = tcp::build(b.connect_timeout).map_err(ClientError::Config)?;
        Ok(Client {
            inner: Arc::new(Inner {
                transport: Transport::Tcp { base, http },
                retry: b.retry,
                request_timeout: b.request_timeout,
                user_agent: b.user_agent,
                clock: b.clock,
            }),
            auth: Auth::Session(Arc::new(TokenCell::new(b.token, b.pass_source))),
        })
    }

    /// The same client with a new token cell holding `token` (S8 name kept).
    pub fn with_token(&self, token: impl Into<SecretString>) -> Client {
        Client {
            inner: self.inner.clone(),
            auth: Auth::Session(Arc::new(TokenCell::new(Some(token.into()), None))),
        }
    }

    /// The same client with a new, empty token cell.
    pub fn without_token(&self) -> Client {
        Client {
            inner: self.inner.clone(),
            auth: Auth::Session(Arc::new(TokenCell::new(None, None))),
        }
    }

    /// The base URL (`http(s)://…`, no trailing `/`), or `unix:<socket>` under act-as.
    pub fn base_url(&self) -> String {
        self.inner.transport.describe()
    }

    /// The session token, if any.
    pub fn token(&self) -> Option<SecretString> {
        match &self.auth {
            Auth::Session(c) => c.token(),
            Auth::Service { .. } => None,
        }
    }

    /// The member this client acts for: set after a login, a renew, or (under act-as) the
    /// first successful `me()`.
    pub fn member_id(&self) -> Option<Id> {
        match &self.auth {
            Auth::Session(c) => c.member_id(),
            Auth::Service { member, .. } => *member.lock().unwrap_or_else(|e| e.into_inner()),
        }
    }

    /// Whether this client acts as an agent over the mgmt socket.
    pub fn is_act_as(&self) -> bool {
        matches!(
            &self.auth,
            Auth::Service {
                act_as: Some(_),
                ..
            }
        )
    }

    pub(crate) fn clock(&self) -> Arc<dyn Clock> {
        self.inner.clock.clone()
    }

    pub(crate) fn session_cell(&self) -> Result<&Arc<TokenCell>, ClientError> {
        match &self.auth {
            Auth::Session(c) => Ok(c),
            Auth::Service { .. } => Err(ClientError::Unsupported(
                "sessions are not available when acting as an agent (C9)",
            )),
        }
    }

    pub(crate) fn set_member_id(&self, id: Id) {
        match &self.auth {
            Auth::Session(c) => c.set_member_id(id),
            Auth::Service { member, .. } => {
                *member.lock().unwrap_or_else(|e| e.into_inner()) = Some(id);
            }
        }
    }

    /// Refuses session-only calls (auth, gateway) under act-as, before anything is sent.
    pub(crate) fn refuse_act_as(&self, what: &'static str) -> Result<(), ClientError> {
        if self.is_act_as() {
            return Err(ClientError::Unsupported(what));
        }
        Ok(())
    }

    // ------------------------------------------------------------------ the pipeline

    /// Generic typed call. `method` is `GET` or `POST` (C5). `query` is encoded with
    /// `serde_urlencoded` rules; `body` is serialized once and reused on every retry; a
    /// bodiless `POST` sends `{}`.
    ///
    /// Cancel-safe: dropping the future sends no further retry.
    pub async fn call<Q, B, R>(
        &self,
        method: Method,
        path: &str,
        query: Option<&Q>,
        body: Option<&B>,
        opts: RequestOptions,
    ) -> Result<Response<R>, ClientError>
    where
        Q: Serialize + ?Sized,
        B: Serialize + ?Sized,
        R: DeserializeOwned,
    {
        let path = with_query(path, query)?;
        let body = match body {
            Some(b) => Some(Bytes::from(serde_json::to_vec(b).map_err(|e| {
                ClientError::invalid("body", format!("does not serialize: {e}"))
            })?)),
            None => None,
        };
        let (resp, meta) = self.execute(method, path, body, opts).await?;
        let value = decode(resp.status, &resp.body)?;
        Ok(Response { value, meta })
    }

    /// A typed `GET` returning the value only.
    pub(crate) async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, ClientError> {
        let (resp, _) = self
            .execute(
                Method::GET,
                path.to_owned(),
                None,
                RequestOptions::default(),
            )
            .await?;
        decode(resp.status, &resp.body)
    }

    /// A typed `POST` with default options, returning the value only (the S8/S10 first-cut
    /// helper; later slices' methods may keep using it).
    pub(crate) async fn post<B: Serialize + ?Sized, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, ClientError> {
        self.post_with(path, body, RequestOptions::default()).await
    }

    /// A typed `POST` with options.
    pub(crate) async fn post_with<B: Serialize + ?Sized, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
        opts: RequestOptions,
    ) -> Result<T, ClientError> {
        let bytes = serde_json::to_vec(body)
            .map_err(|e| ClientError::invalid("body", format!("does not serialize: {e}")))?;
        let (resp, _) = self
            .execute(
                Method::POST,
                path.to_owned(),
                Some(Bytes::from(bytes)),
                opts,
            )
            .await?;
        decode(resp.status, &resp.body)
    }

    /// The pipeline: returns the 2xx response and its meta, or the typed error. Boxed with an
    /// explicit `Send` because it recurses (a renew or re-login runs through it).
    pub(crate) fn execute(
        &self,
        method: Method,
        path: String,
        body: Option<Bytes>,
        opts: RequestOptions,
    ) -> BoxFuture<'_, Result<(WireResp, ResponseMeta), ClientError>> {
        Box::pin(self.execute_inner(method, path, body, opts))
    }

    async fn execute_inner(
        &self,
        method: Method,
        path: String,
        body: Option<Bytes>,
        opts: RequestOptions,
    ) -> Result<(WireResp, ResponseMeta), ClientError> {
        if method != Method::GET && method != Method::POST {
            return Err(ClientError::invalid(
                "method",
                format!("only GET and POST are served (C5); use request() for {method}"),
            ));
        }
        let auth_route = path.starts_with(AUTH_PREFIX);
        if !auth_route {
            self.ensure_fresh().await?;
        }
        let key = if method == Method::POST && !auth_route {
            Some(
                opts.idempotency_key
                    .clone()
                    .unwrap_or_else(new_idempotency_key),
            )
        } else {
            None
        };
        let body = if method == Method::POST {
            Some(body.unwrap_or_else(|| Bytes::from_static(b"{}")))
        } else {
            None
        };
        let policy = if opts.no_retry {
            RetryPolicy::none()
        } else {
            self.inner.retry.clone()
        };
        let mut relogged = false;
        loop {
            let seen = match &self.auth {
                Auth::Session(c) => c.generation(),
                Auth::Service { .. } => 0,
            };
            let r = self
                .attempts(
                    &method,
                    &path,
                    body.as_ref(),
                    key.as_ref(),
                    &opts,
                    &policy,
                    auth_route,
                )
                .await;
            match r {
                Err(e) if e.is_auth() && !auth_route && !relogged && self.has_pass_source() => {
                    relogged = true;
                    tracing::debug!(error = %e, "sew-client: 401 with a pass source; logging in again");
                    self.relogin_since(seen).await?;
                }
                other => return other,
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn attempts(
        &self,
        method: &Method,
        path: &str,
        body: Option<&Bytes>,
        key: Option<&IdempotencyKey>,
        opts: &RequestOptions,
        policy: &RetryPolicy,
        auth_route: bool,
    ) -> Result<(WireResp, ResponseMeta), ClientError> {
        let safe = *method == Method::GET || key.is_some();
        let timeout = opts.timeout.unwrap_or(self.inner.request_timeout);
        let started = tokio::time::Instant::now();
        let mut attempt: u32 = 0;
        loop {
            attempt += 1;
            let wire = Wire {
                method: method.clone(),
                path: path.to_owned(),
                headers: self.headers(path, body.is_some(), key, opts.forwarded_for),
                body: body.cloned(),
                timeout,
            };
            let err = match self.inner.transport.send(&wire).await {
                Ok(r) if is_success(r.status) => {
                    let meta =
                        ResponseMeta::from_headers(r.status, &r.headers, key.cloned(), attempt);
                    return Ok((r, meta));
                }
                Ok(r) => from_response(r.status, &r.headers, &r.body),
                Err(t) => ClientError::Transport(t),
            };
            let outcome = match &err {
                ClientError::Api(a) => Outcome::Api(a),
                ClientError::Transport(t) => Outcome::Transport(t),
                _ => Outcome::Other,
            };
            let wait = match decide(policy, safe, auth_route, outcome) {
                Decision::Stop => return Err(err),
                Decision::After(d) => d,
                Decision::Backoff => {
                    let n = attempt - 1;
                    policy.backoff().sample(n, &mut rand::thread_rng())
                }
            };
            if attempt >= policy.max_attempts || started.elapsed() + wait > policy.max_elapsed {
                return Err(err);
            }
            tracing::debug!(
                %method, path, attempt, wait_ms = wait.as_millis() as u64, error = %err,
                "sew-client: retrying"
            );
            tokio::time::sleep(wait).await;
        }
    }

    /// Headers for one attempt: user agent, accept, content type, auth, act-as, key, XFF.
    fn headers(
        &self,
        path: &str,
        has_body: bool,
        key: Option<&IdempotencyKey>,
        forwarded_for: Option<IpAddr>,
    ) -> Vec<(String, String)> {
        let mut h = vec![
            ("user-agent".to_owned(), self.inner.user_agent.clone()),
            ("accept".to_owned(), "application/json".to_owned()),
        ];
        if has_body {
            h.push(("content-type".to_owned(), "application/json".to_owned()));
        }
        let route = path.split('?').next().unwrap_or(path);
        match &self.auth {
            Auth::Session(c) => {
                if !LOGIN_ROUTES.contains(&route)
                    && let Some(t) = c.token()
                {
                    h.push(("authorization".to_owned(), format!("Bearer {}", t.expose())));
                }
            }
            Auth::Service { token, act_as, .. } => {
                h.push((
                    "authorization".to_owned(),
                    format!("Bearer {}", token.expose()),
                ));
                if let Some((target, via)) = act_as {
                    h.push(("sew-act-as".to_owned(), target.to_string()));
                    h.push(("sew-via".to_owned(), via.to_string()));
                }
            }
        }
        if let Some(k) = key {
            h.push(("idempotency-key".to_owned(), k.as_str().to_owned()));
        }
        if let Some(ip) = forwarded_for {
            h.push(("x-forwarded-for".to_owned(), ip.to_string()));
        }
        h
    }

    /// Sends any request once and returns whatever came back: no status handling, no retry, no
    /// idempotency key (S8 convention tests). The client's credentials are sent unless
    /// `headers` has an `Authorization`.
    pub async fn request(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<Vec<u8>>,
    ) -> Result<RawResponse, ClientError> {
        let method = Method::from_bytes(method.as_bytes())
            .map_err(|e| ClientError::invalid("method", e.to_string()))?;
        let has_auth = headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("authorization"));
        let mut h: Vec<(String, String)> = Vec::new();
        match &self.auth {
            Auth::Session(c) => {
                if let (Some(t), false) = (c.token(), has_auth) {
                    h.push(("authorization".to_owned(), format!("Bearer {}", t.expose())));
                }
            }
            Auth::Service { token, act_as, .. } => {
                if !has_auth {
                    h.push((
                        "authorization".to_owned(),
                        format!("Bearer {}", token.expose()),
                    ));
                }
                if let Some((target, via)) = act_as {
                    h.push(("sew-act-as".to_owned(), target.to_string()));
                    h.push(("sew-via".to_owned(), via.to_string()));
                }
            }
        }
        for (k, v) in headers {
            h.push(((*k).to_owned(), (*v).to_owned()));
        }
        let wire = Wire {
            method,
            path: path.to_owned(),
            headers: h,
            body: body.map(Bytes::from),
            timeout: self.inner.request_timeout,
        };
        let r = self.inner.transport.send(&wire).await?;
        Ok(RawResponse {
            status: r.status,
            headers: r.headers,
            body: r.body.to_vec(),
        })
    }
}

/// Parses a 2xx body as `T`, else `Decode`.
pub(crate) fn decode<T: DeserializeOwned>(status: u16, body: &[u8]) -> Result<T, ClientError> {
    serde_json::from_slice(body).map_err(|e| ClientError::Decode {
        status,
        body_snippet: snippet(body),
        error: e.to_string(),
    })
}

/// Appends an encoded query (`serde_urlencoded` rules; `#` becomes `%23`).
pub(crate) fn with_query<Q: Serialize + ?Sized>(
    path: &str,
    query: Option<&Q>,
) -> Result<String, ClientError> {
    let Some(q) = query else {
        return Ok(path.to_owned());
    };
    let qs = serde_urlencoded::to_string(q)
        .map_err(|e| ClientError::invalid("query", format!("does not encode: {e}")))?;
    if qs.is_empty() {
        return Ok(path.to_owned());
    }
    let sep = if path.contains('?') { '&' } else { '?' };
    Ok(format!("{path}{sep}{qs}"))
}

/// Checks and trims a base URL: `http://` or `https://`, a path kept as a prefix, no trailing
/// `/`, no query or fragment.
pub(crate) fn normalize_base(base: &str) -> Result<String, ClientError> {
    let trimmed = base.trim_end_matches('/');
    let url = url::Url::parse(trimmed)
        .map_err(|e| ClientError::Config(format!("invalid base URL {trimmed:?}: {e}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(ClientError::Config(format!(
            "invalid base URL {trimmed:?}: expected http:// or https://"
        )));
    }
    if url.query().is_some() || url.fragment().is_some() || url.host_str().is_none() {
        return Err(ClientError::Config(format!(
            "invalid base URL {trimmed:?}: no query or fragment, and a host"
        )));
    }
    Ok(trimmed.to_owned())
}

/// Builds a service client (mgmt) over a transport.
#[cfg(all(feature = "mgmt", unix))]
pub(crate) fn service_client(
    socket: std::path::PathBuf,
    token: SecretString,
    act_as: Option<(sew_proto::mgmt::ActAsTarget, sew_proto::mgmt::SewVia)>,
    retry: RetryPolicy,
    request_timeout: Duration,
    clock: Arc<dyn Clock>,
) -> Client {
    Client {
        inner: Arc::new(Inner {
            transport: Transport::Uds { socket },
            retry,
            request_timeout,
            user_agent: crate::config::default_user_agent(),
            clock,
        }),
        auth: Auth::Service {
            token,
            act_as,
            member: Arc::new(std::sync::Mutex::new(None)),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_urls() {
        assert_eq!(
            normalize_base("http://127.0.0.1:8470/").ok().as_deref(),
            Some("http://127.0.0.1:8470")
        );
        assert_eq!(
            normalize_base("https://chat.example.com/sew/")
                .ok()
                .as_deref(),
            Some("https://chat.example.com/sew")
        );
        assert!(normalize_base("ftp://x").is_err());
        assert!(normalize_base("http://x/?a=1").is_err());
        assert!(normalize_base("not a url").is_err());
    }

    #[test]
    fn query_encoding() {
        #[derive(Serialize)]
        struct Q {
            addr: &'static str,
            limit: Option<u32>,
        }
        let q = Q {
            addr: "acme/#deploys",
            limit: None,
        };
        assert_eq!(
            with_query("/api/v1/resolve", Some(&q)).ok().as_deref(),
            Some("/api/v1/resolve?addr=acme%2F%23deploys")
        );
    }
}
