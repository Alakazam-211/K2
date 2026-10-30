//! The gate in front of a published site (`k2 publish` visibility, PRD
//! `prd-publish-visibility-and-dev-mode-v1` §3–§5, 0.46.7).
//!
//! The E2E listener (`tunnel_tls_listener::serve_one`) asks [`classify`] about
//! each nested connection:
//!
//! - **Open**: K2's reserved labels and App runtime Apps (their own logins).
//!   Today's path, untouched. A `k2 publish run --cmd` folder App is *not* one
//!   of these: it has no logins, so the table speaks for it.
//! - **Public** (not dev): today's raw splice, untouched, byte for byte. Only
//!   [`until_not_public`] watches it, so a move to private cuts the connections
//!   already open.
//! - **Gated** (private, PIN, or any dev site): [`serve`]. The connection's
//!   first request is read and checked **before** the site's port is dialed,
//!   so a visitor who fails never causes a connection to the site. Then every
//!   request on the connection passes the same check, keep-alive and WebSocket
//!   upgrades included, with K2's cookies taken out. A socket that went raw
//!   after a `101` is re-checked on every change and every minute, and cut when
//!   its session no longer holds.
//!
//! Anything that can't be decided counts as a refusal: a label with no row is
//! private, and a database error reads as private.
//!
//! K2's own pages live under `/_k2/site/`, answered by the gate ([`pages`]).
//! There is no password form here: a private site is reached by a one-time
//! link a signed-in K2 client asked for.

pub mod dev;
pub mod pages;

use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use k2_core::auth_audit;
use k2_core::site_visibility::{self as sv, Mode, SessionKind};
use rusqlite::{params, Connection, OptionalExtension};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::client_ip::{self, GateHooks, GateRequest, GateVerdict};
use dev::DevWake;

/// K2's cookies: read by the gate, never forwarded to the site.
pub const COOKIES: &[&str] = &[sv::COOKIE_SITE, sv::COOKIE_PIN];
/// A PIN form is small. More than this is refused unread.
const MAX_PIN_BODY: u64 = 4096;
const BODY_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a passed check holds on one connection, and the longest a raw socket
/// goes unchecked: a role change ends a session within this (PV-5). A change made
/// through `site_visibility` (mode, PIN, revoke) applies at once.
#[cfg(not(test))]
const RECHECK: Duration = Duration::from_secs(60);
#[cfg(test)]
const RECHECK: Duration = Duration::from_millis(200);
/// How often a dev connection's traffic counts as the site being in use.
#[cfg(not(test))]
const DEV_TOUCH: Duration = Duration::from_secs(30);
#[cfg(test)]
const DEV_TOUCH: Duration = Duration::from_millis(50);
/// `X-K2-Mode` etc. on every response of a dev site (DM-4).
const DEV_RESPONSE_LINES: &[u8] = b"X-K2-Mode: dev\r\nX-Robots-Tag: noindex\r\n";

/// Argon2 runs at most this many at once: a burst of tries can't starve the box.
fn pin_slots() -> &'static tokio::sync::Semaphore {
    static S: std::sync::OnceLock<tokio::sync::Semaphore> = std::sync::OnceLock::new();
    S.get_or_init(|| tokio::sync::Semaphore::new(2))
}

// ── classifying a connection ──────────────────────────────────────────

/// What a nested connection is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Site {
    /// Not this table's: today's path.
    Open,
    /// Public, not dev: today's path, watched by [`until_not_public`].
    Public { label: String },
    Gated(Gated),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gated {
    pub label: String,
    pub dev: bool,
    /// What a visitor who passes is dialed to.
    pub upstream: String,
}

/// The App behind `label` or behind the port `target` names, as `(front port,
/// a publish folder App)`.
fn app_behind(conn: &Connection, label: &str, target: &str) -> rusqlite::Result<Option<(Option<i64>, bool)>> {
    let port = target.rsplit(':').next().and_then(|p| p.parse::<i64>().ok()).unwrap_or(-1);
    conn.query_row(
        "SELECT front_port, source = 'folder' AND path = 'open' FROM apps WHERE app_id = ?1 OR front_port = ?2
         ORDER BY app_id = ?1 DESC LIMIT 1",
        params![label, port],
        |r| Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, i64>(1)? != 0)),
    )
    .optional()
}

/// [`classify`] on a connection. A database error is `Err`.
pub fn classify_with(conn: &Connection, label: &str, target: &str) -> Result<Site, String> {
    if !sv::is_covered(label) {
        return Ok(Site::Open);
    }
    let app = app_behind(conn, label, target).map_err(|e| e.to_string())?;
    let mut upstream = target.to_string();
    match app {
        // A real App: its own logins decide (PV-1, P-D3).
        Some((_, false)) => return Ok(Site::Open),
        // A `k2 publish run --cmd` folder App: its front port routes to
        // whichever instance is live, and answers when none is.
        Some((Some(front), true)) => upstream = format!("127.0.0.1:{front}"),
        Some((None, true)) | None => {}
    }
    let vis = sv::effective(conn, label)?;
    Ok(if vis.mode == Mode::Public && !vis.dev {
        Site::Public { label: label.to_string() }
    } else {
        Site::Gated(Gated { label: label.to_string(), dev: vis.dev, upstream })
    })
}

/// What this nested connection is, and a watch taken **before** the read, so a
/// change right after it isn't missed. Fail closed: a read that fails is gated.
pub async fn classify(label: &str, target: &str) -> (Site, tokio::sync::watch::Receiver<u64>) {
    let changes = sv::subscribe();
    (classify_read(label, target).await, changes)
}

async fn classify_read(label: &str, target: &str) -> Site {
    let (l, t) = (label.to_string(), target.to_string());
    let read = tokio::task::spawn_blocking(move || {
        let db = k2_core::db::shared();
        let conn = db.lock();
        classify_with(&conn, &l, &t)
    })
    .await;
    match read {
        Ok(Ok(site)) => site,
        Ok(Err(e)) => {
            k2_core::log_debug!("[site_gate] {label}: can't read its visibility ({e}): treating it as private");
            Site::Gated(Gated { label: label.to_string(), dev: false, upstream: target.to_string() })
        }
        Err(e) => {
            k2_core::log_debug!("[site_gate] {label}: visibility read failed ({e}): treating it as private");
            Site::Gated(Gated { label: label.to_string(), dev: false, upstream: target.to_string() })
        }
    }
}

/// Resolves when a public label stops being public (or can't be read). Reads
/// nothing until a change wakes it, so a busy public site pays nothing. `rx` is
/// the watch [`classify`] gave.
pub async fn until_not_public(label: String, mut rx: tokio::sync::watch::Receiver<u64>) {
    loop {
        if rx.changed().await.is_err() {
            return std::future::pending().await;
        }
        let l = label.clone();
        let mode = tokio::task::spawn_blocking(move || {
            let db = k2_core::db::shared();
            let conn = db.lock();
            sv::effective_mode(&conn, &l)
        })
        .await;
        if !matches!(mode, Ok(Ok(Mode::Public))) {
            return;
        }
    }
}

// ── the check ─────────────────────────────────────────────────────────

/// K2's cookie values on a request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Keys {
    site: Option<String>,
    pin: Option<String>,
}

impl Keys {
    fn of(req: &GateRequest) -> Self {
        let mut k = Keys::default();
        for (name, value) in &req.cookies {
            if name.eq_ignore_ascii_case(sv::COOKIE_SITE) {
                k.site = Some(value.clone());
            } else if name.eq_ignore_ascii_case(sv::COOKIE_PIN) {
                k.pin = Some(value.clone());
            }
        }
        k
    }
}

/// Where a request stands.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Access {
    /// The site is public now: nobody is asked.
    Public { dev: bool },
    In { expires_at: i64, dev: bool, notice_token: Option<String> },
    Out { mode: Mode, dev: bool },
}

/// May this login still open private sites? (PV-5: a role change or removal
/// ends the session.) The owner token always may.
fn user_may(user: &str) -> bool {
    user == sv::USER_OWNER_TOKEN || k2_core::connect_users::is_active_owner_or_admin(user)
}

fn evaluate(conn: &Connection, label: &str, keys: &Keys) -> Result<Access, String> {
    let vis = sv::effective(conn, label)?;
    if vis.mode == Mode::Public {
        return Ok(Access::Public { dev: vis.dev });
    }
    if let Some(t) = &keys.site {
        if let Some(s) = sv::live_session(conn, label, t)? {
            if s.kind == SessionKind::Open && user_may(&s.user) {
                // The owner in a K2 tab never gets the dev notice (DM-4).
                return Ok(Access::In { expires_at: s.expires_at, dev: vis.dev, notice_token: None });
            }
        }
    }
    if vis.mode == Mode::Pin {
        if let Some(t) = &keys.pin {
            if let Some(s) = sv::live_session(conn, label, t)? {
                if s.kind == SessionKind::Pin {
                    let notice_token = (vis.dev && !s.notice_seen).then(|| t.clone());
                    return Ok(Access::In { expires_at: s.expires_at, dev: vis.dev, notice_token });
                }
            }
        }
    }
    Ok(Access::Out { mode: vis.mode, dev: vis.dev })
}

fn now_unix() -> i64 {
    chrono::Utc::now().timestamp()
}

/// `?name=value`'s value, percent-decoded.
fn query_value(path: &str, name: &str) -> Option<String> {
    k2_core::agent_hooks::parse_query_params(path).remove(name)
}

fn audit(event: &str, label: &str, outcome: &str, ip: Option<IpAddr>) {
    let ip = ip.map_or("-".to_string(), |i| i.to_string());
    let ev = auth_audit::AuditEvent::new(event, "-", outcome, "tunnel", ip, "web");
    let detail = serde_json::json!({ "kind": "site", "label": label });
    if let Err(e) = auth_audit::try_record_detail(&ev, detail) {
        k2_core::log_debug!("[site_gate] audit {event} for {label}: {e}");
    }
}

/// Run `f` on a blocking thread with the database, off the connection's task.
async fn with_db<T: Send + 'static>(f: impl FnOnce(&Connection) -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tokio::task::spawn_blocking(move || {
        let db = k2_core::db::shared();
        let conn = db.lock();
        f(&conn)
    })
    .await
    .map_err(|e| format!("site_gate worker: {e}"))?
}

#[derive(Debug)]
struct Cached {
    keys: Keys,
    version: u64,
    at: Instant,
    access: Access,
}

/// One gated connection.
struct Conn {
    label: String,
    host: String,
    dev: bool,
    cached: Mutex<Option<Cached>>,
    changes: tokio::sync::watch::Receiver<u64>,
}

impl Conn {
    fn new(g: &Gated, host: &str) -> Arc<Self> {
        Arc::new(Conn {
            label: g.label.clone(),
            host: host.trim().to_ascii_lowercase(),
            dev: g.dev,
            cached: Mutex::new(None),
            changes: sv::subscribe(),
        })
    }

    /// Check `keys`, from this connection's last answer when it still holds
    /// (`fresh` always reads).
    async fn access(&self, keys: &Keys, fresh: bool) -> Access {
        let version = *self.changes.borrow();
        if !fresh {
            let g = self.cached.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(c) = g.as_ref() {
                let live = match &c.access {
                    Access::In { expires_at, .. } => *expires_at > now_unix(),
                    Access::Public { .. } => true,
                    Access::Out { .. } => false,
                };
                if live && c.version == version && c.keys == *keys && c.at.elapsed() < RECHECK {
                    return c.access.clone();
                }
            }
        }
        let label = self.label.clone();
        let k = keys.clone();
        let access = with_db(move |conn| evaluate(conn, &label, &k)).await.unwrap_or_else(|e| {
            k2_core::log_debug!("[site_gate] {}: can't check a session ({e}): refusing", self.label);
            Access::Out { mode: Mode::Private, dev: self.dev }
        });
        *self.cached.lock().unwrap_or_else(|p| p.into_inner()) =
            Some(Cached { keys: keys.clone(), version, at: Instant::now(), access: access.clone() });
        access
    }

    fn refused(&self, mode: Mode, dev: bool, req: &GateRequest) -> Vec<u8> {
        let head = req.method.eq_ignore_ascii_case("HEAD");
        match mode {
            Mode::Pin => pages::pin_page(&next_of(&req.path), None, dev, head),
            _ => pages::private_page(&self.label, dev, head),
        }
    }

    /// The decision for one request, except a PIN `POST` (the caller reads its
    /// body first).
    async fn decide(&self, req: GateRequest) -> GateVerdict {
        let keys = Keys::of(&req);
        let access = self.access(&keys, false).await;
        let own = own_page(&req.path);
        match (&access, own) {
            // A public site has no K2 pages: `/_k2/site/` is the site's.
            (Access::Public { .. }, _) => {}
            (_, Some(page)) => return self.own_page(page, &req, &access).await,
            (Access::Out { mode, dev }, None) => return GateVerdict::Answer(self.refused(*mode, *dev, &req)),
            (Access::In { .. }, None) => {}
        }
        if let Access::In { notice_token: Some(_), dev: true, .. } = &access {
            // The first page view of this PIN session on a dev site (DM-4).
            if req.is_document && (req.method == "GET" || req.method == "HEAD") {
                return GateVerdict::Answer(pages::dev_notice_page(&self.label, &next_of(&req.path), req.method == "HEAD"));
            }
        }
        // Only a visitor who passed counts as use of a dev site (DM-6).
        let dev = matches!(access, Access::Public { dev: true } | Access::In { dev: true, .. });
        if dev {
            dev::touch(&self.label);
        }
        GateVerdict::Allow
    }

    /// K2's own pages under `/_k2/site/`.
    async fn own_page(&self, page: &str, req: &GateRequest, access: &Access) -> GateVerdict {
        let head = req.method.eq_ignore_ascii_case("HEAD");
        let get = req.method.eq_ignore_ascii_case("GET") || head;
        let dev = match access {
            Access::Public { dev } | Access::In { dev, .. } | Access::Out { dev, .. } => *dev,
        };
        GateVerdict::Answer(match (page, get) {
            ("open", true) if !head => self.open_landing(req, dev).await,
            ("pin", true) => match access {
                Access::Out { mode: Mode::Pin, .. } => pages::pin_page(&safe_next(req), None, dev, head),
                Access::Out { mode, .. } => self.refused(*mode, dev, req),
                _ => pages::redirect(&safe_next(req), None, dev),
            },
            ("continue", true) if !head => {
                if let Access::In { notice_token: Some(t), .. } = access {
                    let (label, t) = (self.label.clone(), t.clone());
                    if let Err(e) = with_db(move |conn| sv::mark_notice_seen(conn, &label, &t)).await {
                        k2_core::log_debug!("[site_gate] {}: notice: {e}", self.label);
                    }
                }
                match access {
                    Access::Out { mode, .. } => self.refused(*mode, dev, req),
                    _ => pages::redirect(&safe_next(req), None, dev),
                }
            }
            // A `POST` here came on a connection that already passed: its
            // body isn't read there. The browser sends it again on a new one.
            ("pin", false) if req.method.eq_ignore_ascii_case("POST") => pages::reconnect("/_k2/site/pin", dev),
            _ => pages::plain("404 Not Found", "Not found.", dev, head),
        })
    }

    /// `GET /_k2/site/open?code=…`: spend the code, set the cookie, go.
    async fn open_landing(&self, req: &GateRequest, dev: bool) -> Vec<u8> {
        let code = query_value(&req.path, "code").unwrap_or_default();
        let label = self.label.clone();
        let spent = with_db(move |conn| {
            let Some(r) = sv::redeem_open_code(conn, &label, &code)? else {
                return Ok(None);
            };
            if !user_may(&r.user) {
                return Ok(None);
            }
            let s = sv::create_session(conn, &label, SessionKind::Open, &r.user)?;
            Ok(Some((r, s)))
        })
        .await;
        match spent {
            Ok(Some((r, s))) => {
                audit("site-open", &self.label, "ok", req.client_ip);
                pages::redirect(&r.path, Some((sv::COOKIE_SITE, &s.token, s.max_age)), dev)
            }
            Ok(None) => {
                audit("site-open", &self.label, "bad_code", req.client_ip);
                pages::link_dead_page(&self.label, dev, false)
            }
            Err(e) => {
                k2_core::log_debug!("[site_gate] {}: one-time link: {e}", self.label);
                pages::link_dead_page(&self.label, dev, false)
            }
        }
    }

    /// `POST /_k2/site/pin`: Origin, limits, argon2, session.
    async fn pin_post(&self, req: &GateRequest, body: &[u8]) -> Vec<u8> {
        let expected = format!("https://{}", self.host);
        if !req.origin.as_deref().is_some_and(|o| o.eq_ignore_ascii_case(&expected)) {
            audit("site-pin", &self.label, "bad_origin", req.client_ip);
            return pages::plain("403 Forbidden", "This form must be sent from the site's own page.", self.dev, false);
        }
        let form = crate::routes::http::parse_form_body(body);
        let next = form.get("next").and_then(|n| sv::safe_path(n)).unwrap_or_else(|| "/".to_string());
        let pin = form.get("pin").cloned().unwrap_or_default();

        let label = self.label.clone();
        let ip = req.client_ip.map(|i| i.to_string());
        let now = now_unix();
        let begun = with_db(move |conn| {
            let vis = sv::effective(conn, &label)?;
            if vis.mode != Mode::Pin || !vis.pin_set {
                return Ok((vis, None));
            }
            let a = sv::pin_attempt_begin(conn, &label, ip.as_deref(), now)?;
            Ok((vis, Some(a)))
        })
        .await;
        let (vis, attempt) = match begun {
            Ok(v) => v,
            Err(e) => {
                k2_core::log_debug!("[site_gate] {}: PIN: {e}", self.label);
                return pages::plain("503 Service Unavailable", "Please try again.", self.dev, false);
            }
        };
        let Some(attempt) = attempt else {
            // No PIN mode (any more). Nothing to try.
            return match vis.mode {
                Mode::Public => pages::redirect(&next, None, vis.dev),
                _ => pages::private_page(&self.label, vis.dev, false),
            };
        };
        let tripped = match attempt {
            sv::PinAttempt::Wait { retry_after, .. } => {
                audit("site-pin", &self.label, "locked", req.client_ip);
                return pages::pin_locked_page(retry_after, vis.dev);
            }
            sv::PinAttempt::Go { site_tripped } => site_tripped,
        };
        let right = {
            let Ok(_slot) = pin_slots().acquire().await else {
                return pages::plain("503 Service Unavailable", "Please try again.", vis.dev, false);
            };
            let (v, p) = (vis.clone(), pin);
            tokio::task::spawn_blocking(move || v.verify_pin(&p)).await.unwrap_or(false)
        };
        if !right {
            audit("site-pin", &self.label, "wrong", req.client_ip);
            if tripped {
                crate::apps::runtime_notes::owner_notice(
                    "A site's PIN is locked",
                    &format!(
                        "{}: 30 wrong PINs in an hour. New tries wait an hour; people already in keep working. To end it now, set a new PIN (this also signs everyone with the old PIN out): k2 publish visibility {} pin",
                        self.label, self.label
                    ),
                );
            }
            return pages::pin_page(&next, Some("That PIN isn't right."), vis.dev, false);
        }
        let label = self.label.clone();
        let ip = req.client_ip.map(|i| i.to_string());
        let session = with_db(move |conn| {
            sv::pin_attempt_right(conn, &label, ip.as_deref())?;
            sv::create_session(conn, &label, SessionKind::Pin, sv::USER_PIN)
        })
        .await;
        match session {
            Ok(s) => {
                audit("site-pin", &self.label, "ok", req.client_ip);
                pages::redirect(&next, Some((sv::COOKIE_PIN, &s.token, s.max_age)), vis.dev)
            }
            Err(e) => {
                k2_core::log_debug!("[site_gate] {}: PIN session: {e}", self.label);
                pages::plain("503 Service Unavailable", "Please try again.", vis.dev, false)
            }
        }
    }

    /// Runs beside a forwarded connection; returns when it must be cut.
    async fn watchdog(&self, activity: &AtomicU64) {
        let mut rx = self.changes.clone();
        let mut touch = tokio::time::interval(DEV_TOUCH);
        let mut seen = activity.load(Ordering::Relaxed);
        let mut checked = Instant::now();
        loop {
            let check = tokio::select! {
                r = rx.changed() => {
                    if r.is_err() {
                        return std::future::pending().await;
                    }
                    true
                }
                _ = touch.tick() => {
                    let now = activity.load(Ordering::Relaxed);
                    if self.dev && now != seen {
                        seen = now;
                        dev::touch(&self.label);
                    }
                    // The tick that would run past the limit checks early.
                    checked.elapsed() + DEV_TOUCH >= RECHECK
                }
            };
            if check {
                checked = Instant::now();
                let keys = {
                    let g = self.cached.lock().unwrap_or_else(|p| p.into_inner());
                    g.as_ref().map(|c| c.keys.clone()).unwrap_or_default()
                };
                if matches!(self.access(&keys, true).await, Access::Out { .. }) {
                    return;
                }
            }
        }
    }
}

/// The page name when `path` is K2's: `/_k2/site/<page>[?…]`.
fn own_page(path: &str) -> Option<&str> {
    let bare = path.split(['?', '#']).next()?;
    bare.strip_prefix(sv::RESERVED_PATH)
}

/// Where a refused request should come back to.
fn next_of(path: &str) -> String {
    sv::safe_path(path).unwrap_or_else(|| "/".to_string())
}

fn safe_next(req: &GateRequest) -> String {
    query_value(&req.path, "next").and_then(|n| sv::safe_path(&n)).unwrap_or_else(|| "/".to_string())
}

// ── a gated connection ────────────────────────────────────────────────

/// Counts the bytes a connection carries: a raw socket's use of a dev site.
struct Activity<'a, S> {
    inner: S,
    n: &'a AtomicU64,
}

impl<S: AsyncRead + Unpin> AsyncRead for Activity<'_, S> {
    fn poll_read(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>, buf: &mut tokio::io::ReadBuf<'_>) -> std::task::Poll<std::io::Result<()>> {
        let me = self.get_mut();
        let before = buf.filled().len();
        let r = std::pin::Pin::new(&mut me.inner).poll_read(cx, buf);
        me.n.fetch_add((buf.filled().len() - before) as u64, Ordering::Relaxed);
        r
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Activity<'_, S> {
    fn poll_write(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>, buf: &[u8]) -> std::task::Poll<std::io::Result<usize>> {
        let me = self.get_mut();
        let r = std::pin::Pin::new(&mut me.inner).poll_write(cx, buf);
        if let std::task::Poll::Ready(Ok(n)) = &r {
            me.n.fetch_add(*n as u64, Ordering::Relaxed);
        }
        r
    }
    fn poll_flush(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }
    fn poll_shutdown(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

/// Answer with K2's bytes and end the connection.
async fn answer<S: AsyncWrite + Unpin>(client: &mut S, bytes: &[u8]) {
    let _ = client.write_all(bytes).await;
    let _ = client.flush().await;
    let _ = client.shutdown().await;
}

/// Read a PIN form's body: what came with the head, then the rest.
/// `Err` = K2's refusal.
async fn pin_body<S: AsyncRead + Unpin>(client: &mut S, head_len: usize, bytes: &[u8], req: &GateRequest, dev: bool) -> Result<Vec<u8>, Vec<u8>> {
    if req.chunked {
        return Err(pages::plain("400 Bad Request", "This form can't be sent that way.", dev, false));
    }
    let want = req.content_length.unwrap_or(0);
    if want > MAX_PIN_BODY {
        return Err(pages::plain("413 Content Too Large", "That form is too long.", dev, false));
    }
    let want = want as usize;
    let mut body = bytes[head_len..].to_vec();
    let read = async {
        let mut tmp = [0u8; 1024];
        while body.len() < want {
            match client.read(&mut tmp).await {
                Ok(0) | Err(_) => return false,
                Ok(n) => body.extend_from_slice(&tmp[..n]),
            }
        }
        true
    };
    match tokio::time::timeout(BODY_TIMEOUT, read).await {
        Ok(true) => {}
        _ => return Err(pages::plain("408 Request Timeout", "The form didn't arrive.", dev, false)),
    }
    body.truncate(want);
    Ok(body)
}

/// Serve one gated connection on a terminated TLS stream. `host` is the SNI.
pub async fn serve<S>(client: &mut S, host: &str, g: &Gated, ip: Option<IpAddr>, dial_timeout: Duration) -> Result<(), String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let conn = Conn::new(g, host);
    let peeked = client_ip::peek_gate_request(client, COOKIES, ip).await;
    let Some(req) = peeked.req.clone() else {
        // A silent peer just closes. Anything else that isn't a whole HTTP/1.x
        // request is refused, and nothing was dialed.
        if !peeked.bytes.is_empty() {
            answer(client, &pages::plain("400 Bad Request", "K2 could not read this request.", g.dev, false)).await;
        }
        return Ok(());
    };
    if own_page(&req.path) == Some("pin") && req.method.eq_ignore_ascii_case("POST") {
        let keys = Keys::of(&req);
        // A public site's `/_k2/site/` is the site's own: it goes on.
        if !matches!(conn.access(&keys, false).await, Access::Public { .. }) {
            let bytes = match pin_body(client, peeked.head_len, &peeked.bytes, &req, g.dev).await {
                Ok(body) => conn.pin_post(&req, &body).await,
                Err(refusal) => refusal,
            };
            answer(client, &bytes).await;
            return Ok(());
        }
    }
    if let GateVerdict::Answer(bytes) = conn.decide(req.clone()).await {
        answer(client, &bytes).await;
        return Ok(());
    }

    // Passed. Only now may anything reach the site.
    let head = req.method.eq_ignore_ascii_case("HEAD");
    if g.dev {
        match dev::wake(&g.label) {
            DevWake::Up | DevWake::NotDev => {}
            DevWake::Starting => {
                answer(client, &pages::starting_page(head)).await;
                return Ok(());
            }
            DevWake::Failed(why) => {
                k2_core::log_debug!("[site_gate] {}: the dev server didn't start: {why}", g.label);
                answer(client, &pages::dev_failed_page(head)).await;
                return Ok(());
            }
        }
    }
    let mut upstream = match crate::upstream_dial::dial_target(&g.upstream, dial_timeout).await {
        Ok(s) => s,
        Err(fail) => {
            k2_core::log_debug!("[site_gate] {}: upstream {fail}: answering 502", g.label);
            answer(client, &crate::tunnel_tls_listener::unreachable_502_bytes()).await;
            return Ok(());
        }
    };
    if let Err(why) = crate::splice_guard::refuse_self_dial(&upstream) {
        k2_core::log_debug!("[site_gate] {}: refusing target {}: {why}: answering 502", g.label, g.upstream);
        drop(upstream);
        answer(client, &crate::tunnel_tls_listener::unreachable_502_bytes()).await;
        return Ok(());
    }
    let _splice_mark = crate::splice_guard::mark(&upstream);
    let _hop = client_ip::register_hop(&upstream, ip);

    let gate_conn = conn.clone();
    let hooks = GateHooks {
        gate: Arc::new(move |req: GateRequest| {
            let c = gate_conn.clone();
            Box::pin(async move { c.decide(req).await })
        }),
        strip_cookies: COOKIES,
        response_lines: if g.dev { DEV_RESPONSE_LINES.to_vec() } else { Vec::new() },
    };
    let activity = AtomicU64::new(0);
    let replay = client_ip::Replay::new(peeked.bytes, &mut *client);
    let counted = Activity { inner: replay, n: &activity };
    let res = tokio::select! {
        r = client_ip::forward_gated(counted, &mut upstream, client_ip::Mode::Tunnel(ip), hooks) => {
            r.map_err(|e| format!("splice TLS↔site {}: {e}", g.upstream))
        }
        _ = conn.watchdog(&activity) => {
            k2_core::log_debug!("[site_gate] {}: a session ended: cutting its connection", g.label);
            Ok(())
        }
    };
    let _ = upstream.shutdown().await;
    res
}

// ── doors (PV-4) ──────────────────────────────────────────────────────

/// Why `label` can't be made private or PIN right now, or `None`. The gate
/// sits in the E2E listener's nested branch: E2E off (no such branch), and a
/// custom name on a `k2 publish run --cmd` folder App (the App TLS door serves
/// it directly), are doors it doesn't cover. `<code>: <plain words>`.
pub fn door_refusal(label: &str) -> Option<String> {
    if let Some(why) = sv::door_refusal() {
        return Some(why);
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    let names = match custom_names(&conn, label) {
        Ok(n) => n,
        Err(e) => return Some(format!("{}: K2 can't read this site's custom names ({e}), so it can't promise its gate covers them.", sv::DOOR_UNSUPPORTED)),
    };
    names.first().map(|n| {
        format!(
            "{}: '{n}' serves this site directly, past K2's gate, so it can't be private or have a PIN yet. Remove that name first: k2 app domain rm {label} {n}",
            sv::DOOR_UNSUPPORTED
        )
    })
}

/// The custom names of `label`'s folder App (a real App's aren't this table's).
fn custom_names(conn: &Connection, label: &str) -> Result<Vec<String>, String> {
    let folder = matches!(app_behind(conn, label, "").map_err(|e| e.to_string())?, Some((_, true)));
    if !folder {
        return Ok(Vec::new());
    }
    let all = k2_core::domains::app_domains_for(conn, label).map_err(|e| e.to_string())?;
    Ok(all.into_iter().map(|d| d.hostname).collect())
}

/// `k2 app domain add <host>` on a folder App: that name would serve the site
/// past the gate, so its site must already be public, on purpose. `None` = go
/// ahead (a real App, or a public site).
pub fn custom_name_refusal(label: &str, host: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let refuse = |why: String| Some(format!("{}: {why}", sv::DOOR_UNSUPPORTED));
    match app_behind(&conn, label, "") {
        Ok(Some((_, true))) => {}
        Ok(_) => return None,
        Err(e) => return refuse(format!("K2 can't read this site's App ({e}).")),
    }
    match sv::effective_mode(&conn, label) {
        Ok(Mode::Public) => None,
        Ok(m) => refuse(format!(
            "'{host}' would serve the {} site {label} past K2's gate, so it can't be added now. Keep {label} as it is and skip this name, or make {label} public first (anyone can then open it): k2 publish visibility {label} public --reason <why>",
            m.as_str()
        )),
        Err(e) => refuse(format!("K2 can't read this site's visibility ({e}).")),
    }
}

// ── boot ──────────────────────────────────────────────────────────────

/// The upgrade pass ran to its end (or was never owed): no more database reads
/// on the map path.
static SETTLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A routing map is about to route. While the upgrade's pass is owed, its old
/// labels become `public` first (vs-live PA-13): nothing public today breaks.
fn on_map(map: &k2_core::tunnel::subdomains::SubdomainMap, fetched: bool) {
    if SETTLED.load(Ordering::Relaxed) {
        return;
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    let labels: Vec<&str> = map.targets.keys().map(String::as_str).collect();
    // A locked database gets a few tries: on failure these sites read as
    // private until the next map, which tries again.
    let mut last = None;
    for attempt in 0..5u64 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(200 * attempt));
        }
        match sv::grandfather_map(&conn, &map.primary, labels.iter().copied(), fetched) {
            Ok(n) => {
                if n > 0 {
                    k2_core::log_debug!("[site_gate] kept {n} existing site(s) public (upgrade to 0.46.7)");
                }
                last = None;
                break;
            }
            Err(e) => last = Some(e),
        }
    }
    if let Some(e) = last {
        k2_core::log_debug!("ERROR [site_gate] the upgrade's pass over the routing map failed ({e}): these sites read as private until the next map");
        return;
    }
    if matches!(sv::grandfather_pending(&conn), Ok(false)) {
        SETTLED.store(true, Ordering::Relaxed);
    }
}

/// A label was made or removed on the control plane.
fn on_label(event: k2_core::tunnel::subdomains::LabelEvent<'_>) {
    use k2_core::tunnel::subdomains::LabelEvent;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let res = match event {
        LabelEvent::Created(l) => sv::note_new_label(&conn, l, "create"),
        LabelEvent::Deleted(l) => sv::forget_label(&conn, l),
    };
    if let Err(e) = res {
        k2_core::log_debug!("[site_gate] label change: {e}");
    }
}

/// The E2E listener's boot, before the first map routes anything.
pub fn start(subdomain: Option<&str>) {
    k2_core::tunnel::subdomains::set_visibility_hooks(on_map, on_label);
    // Nothing to read on the map path once the pass is settled.
    let db = k2_core::db::shared();
    if let Ok(false) = sv::grandfather_pending(&db.lock()) {
        SETTLED.store(true, Ordering::Relaxed);
    }
    drop(db);
    if let Some(sub) = subdomain {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if let Err(e) = sv::settle_grandfather_at_boot(&conn, sub) {
            k2_core::log_debug!("ERROR [site_gate] {e}");
        }
        if let Ok(false) = sv::grandfather_pending(&conn) {
            SETTLED.store(true, Ordering::Relaxed);
        }
    }
    // PA-6: expired sessions, codes and limits go hourly.
    tokio::spawn(async {
        let mut tick = tokio::time::interval(Duration::from_secs(3600));
        loop {
            tick.tick().await;
            if let Err(e) = with_db(sv::sweep_expired).await {
                k2_core::log_debug!("[site_gate] sweep: {e}");
            }
        }
    });
}

#[cfg(test)]
mod tests;
