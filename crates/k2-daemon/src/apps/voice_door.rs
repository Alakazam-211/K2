//! The native call door on an App's own addresses (0.46.9): `/_k2/voice/sew/`
//! on `<app>.<label>.k2.dev` and the App's live custom names, for a native
//! app (desktop or phone) whose App server mints Sew's short-lived `sewcc_`
//! call credential for a signed-in person. The app holds no K2 pass; it
//! talks to its App's host for the call's key traffic and socket, and to
//! TURN directly for media.
//!
//! The relay is the box label's door, unchanged (`voice::sew_lane`): Sew's
//! exact allowlist (anything else 404), a `sewcc_` bearer only (no `k2skn_`,
//! `sews_` or cookie ever counts or crosses), no Origin needed, the gateway
//! WebSocket spliced to Sew's loopback listener, the same per-credential
//! rate limit and relay cap, and K2's own answers carry the frame headers.
//!
//! **Sew's web kit** (`voice::kit_door`): on the same gate, a kit route with
//! one of THIS App's login sessions (`k2skn_`) as the bearer runs as that
//! person, with a Sew session from their own chat pass (the App's `app`
//! claim and caps). App sessions get the narrow route set; any other bearer
//! is refused there.
//!
//! **Media signalling** (`voice::signal_lane`): on the same gate plus the
//! App's approved `sew-signal` box service ([`signal_open_of`]), a native
//! engine's `GET /rtc/v1` WebSocket with a non-K2 `Authorization: Bearer`
//! is spliced to Sew's signal listener on loopback, with K2's own
//! `X-Forwarded-For`. Browsers' `/rtc*` requests (cookie or query token) and
//! everything on a shut gate reach the App exactly as before.
//!
//! **The gate** ([`open_of`]). The door opens on an App's addresses only when
//! all of these hold:
//! - voice is installed on this box (Chat / Voice Hosting with `k2 hostchat
//!   media install`);
//! - the App may reach Sew at all: the owner approved its `sew-api` box
//!   service (Sew's client API, the only way its server can mint a
//!   credential);
//! - the App isn't suspended;
//! - its chat caps allow calls: an App whose manifest asks for chat caps
//!   needs `voice:join` approved (`grants::chat_ceiling`, the same ceiling
//!   K2 puts on its people's chat passes); one that asks for none (Hearth and
//!   Meet) leaves the ceiling to its people's roles and Sew, as chat passes do.
//!
//! It is not tied to the voice rooms preview, which is K2's own numbered rooms.
//! The `sew` core tick (`k2 app access <ws> --core sew=on`) is not the gate
//! either: it only pre-approves an App's chat caps at deploy, and a cap the
//! owner approved by hand counts the same.
//!
//! **Where it is read.** On a box with voice installed, the App path
//! (`apps::router`) reads each connection's first request head before it
//! dials the App, as it does for the pairing link files. A head on the door
//! is K2's (answered or relayed, then the connection closes); anything else
//! goes to the App with the bytes replayed. A box without voice reads
//! nothing and changes nothing. A door request later on an App's keep-alive
//! connection reaches the App: the engine's own connections only ever carry
//! door requests, and K2 closes each one after its answer.

use std::net::IpAddr;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

use crate::chat_pass::identity::AppChatCeiling;
use crate::client_ip::{LocalAnswer, Replay};
use crate::voice::kit_door::{Door, KitCtx};
use crate::voice::{sew_lane, signal_lane};

use super::router::FirstRequest;

/// The cap that lets an App's people call.
pub const VOICE_CAP: &str = "voice:join";
/// The box service that is Sew's client API.
pub const SEW_API: &str = "sew-api";
/// The box service that is Sew's media signalling listener.
pub const SEW_SIGNAL: &str = "sew-signal";
/// How long "voice is installed here" is trusted between reads.
const VOICE_TTL: Duration = Duration::from_secs(10);

/// What the gate looks at, for one App.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    /// Voice is installed on this box.
    pub voice: bool,
    /// The App's `sew-api` box service is approved.
    pub sew_api: bool,
    pub suspended: bool,
    /// Its chat ceiling (`grants::chat_ceiling`).
    pub ceiling: AppChatCeiling,
}

/// The gate (pure).
pub fn open_of(f: &Facts) -> bool {
    if !f.voice || !f.sew_api || f.suspended {
        return false;
    }
    match &f.ceiling {
        AppChatCeiling::NotManaged => false,
        AppChatCeiling::NotDeclared => true,
        AppChatCeiling::Declared(caps) => caps.iter().any(|c| c.trim() == VOICE_CAP),
    }
}

/// The signalling door's gate (pure): the call door's gate, and the App's
/// `sew-signal` box service approved at host port `signal_port`. `Some`: the
/// loopback address to splice to.
pub fn signal_open_of(f: &Facts, signal_port: Option<u16>) -> Option<String> {
    let port = signal_port?;
    open_of(f).then(|| format!("127.0.0.1:{port}"))
}

/// The host port of `app_id`'s approved `sew-signal` box service, if any.
pub fn signal_port(app_id: &str) -> Option<u16> {
    let h = k2_core::db::shared();
    let conn = h.lock();
    let rows = k2_core::deploy::box_service::list(&conn, app_id).ok()?;
    rows.iter()
        .find(|s| s.status == k2_core::deploy::box_service::STATUS_ALLOWED && (s.name == SEW_SIGNAL || s.preset.as_deref() == Some(SEW_SIGNAL)))
        .map(|s| s.host_port)
}

/// Where `app_id`'s native signalling socket goes now, when its gate is open.
pub fn signal_addr(app_id: &str) -> Option<String> {
    let port = signal_port(app_id)?;
    signal_open_of(&facts(app_id), Some(port))
}

/// Is voice installed on this box (read at most every [`VOICE_TTL`])?
pub fn voice_installed() -> bool {
    static C: OnceLock<Mutex<Option<(Instant, bool)>>> = OnceLock::new();
    let now = Instant::now();
    let mut g = C.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, v)) = *g {
        if now.duration_since(at) < VOICE_TTL {
            return v;
        }
    }
    let v = crate::sew::voice::current_tier(&crate::sew::supervisor::live()).is_some();
    *g = Some((now, v));
    v
}

/// The gate's facts for the runtime App `app_id` now. Anything K2 can't read
/// counts as shut.
pub fn facts(app_id: &str) -> Facts {
    let voice = voice_installed();
    let looked = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        match k2_core::deploy::store::get_app(&conn, app_id) {
            Ok(Some(app)) => {
                let sew_api = k2_core::deploy::box_service::list(&conn, app_id)
                    .map(|v| {
                        v.iter().any(|s| {
                            s.status == k2_core::deploy::box_service::STATUS_ALLOWED
                                && (s.name == SEW_API || s.preset.as_deref() == Some(SEW_API))
                        })
                    })
                    .unwrap_or(false);
                let suspended = super::grants::is_suspended(&conn, app_id).unwrap_or(true);
                Some((app.service_id, sew_api, suspended))
            }
            _ => None,
        }
    };
    let Some((service_id, sew_api, suspended)) = looked else {
        return Facts { voice, sew_api: false, suspended: false, ceiling: AppChatCeiling::NotManaged };
    };
    let ceiling = super::grants::chat_ceiling(&service_id).unwrap_or(AppChatCeiling::NotManaged);
    Facts { voice, sew_api, suspended, ceiling }
}

/// Does the App path read `app_id`'s first request heads for the door?
pub fn watching(app_id: &str) -> bool {
    #[cfg(test)]
    {
        if test_door(app_id).is_some() {
            return true;
        }
    }
    let _ = app_id;
    voice_installed()
}

/// The head at the start of `bytes` when it is complete and its target is on
/// the door: `(head text, head length in bytes with its blank line)`.
pub fn lane_head(bytes: &[u8]) -> Option<(String, usize)> {
    let len = bytes.windows(4).position(|w| w == b"\r\n\r\n")? + 4;
    let head = String::from_utf8_lossy(&bytes[..len]).into_owned();
    let target = head.lines().next()?.split_whitespace().nth(1)?;
    sew_lane::is_lane(target).then_some((head, len))
}

/// Read a connection's first request for `app_id` (the App not dialed yet):
/// the door's, K2 relays it or refuses it; one of `local`'s (the pairing link
/// files, the update feed), K2 answers it; anything else, the bytes to replay
/// to the App. `visitor`: the client address K2 derived for this connection
/// (`client_ip::visitor_of`), sent to the signal listener.
pub async fn first_request<S>(client: &mut S, app_id: &str, local: Option<&LocalAnswer>, visitor: Option<IpAddr>) -> FirstRequest
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    #[cfg(test)]
    {
        if let Some(t) = test_door(app_id) {
            let signal = t.signal.clone().filter(|_| t.open);
            return first_request_door(client, local, || t.open, &t.sew_addr, None, visitor, move || signal).await;
        }
    }
    let kit = KitCtx::live(Door::App { app_id: app_id.to_string() });
    first_request_door(
        client,
        local,
        || open_of(&facts(app_id)),
        &crate::sew::ops::client_addr(),
        Some(&kit),
        visitor,
        || signal_addr(app_id),
    )
    .await
}

/// [`first_request`] with the gate and Sew's address given, and no kit lane.
pub async fn first_request_with<S, F>(client: &mut S, local: Option<&LocalAnswer>, open: F, sew_addr: &str) -> FirstRequest
where
    S: AsyncRead + AsyncWrite + Unpin,
    F: FnOnce() -> bool,
{
    first_request_kit(client, local, open, sew_addr, None).await
}

/// [`first_request_with`] with the App's kit lane (`voice::kit_door`): its
/// people's App login sessions on Sew's web kit routes.
pub async fn first_request_kit<S, F>(client: &mut S, local: Option<&LocalAnswer>, open: F, sew_addr: &str, kit: Option<&KitCtx>) -> FirstRequest
where
    S: AsyncRead + AsyncWrite + Unpin,
    F: FnOnce() -> bool,
{
    first_request_door(client, local, open, sew_addr, kit, None, || None).await
}

/// [`first_request_kit`] with the media-signalling door: `signal` answers
/// where the App's native signalling socket goes when its gate is open
/// ([`signal_addr`]); it is only asked for a head that is the door's.
pub async fn first_request_door<S, F, G>(
    client: &mut S,
    local: Option<&LocalAnswer>,
    open: F,
    sew_addr: &str,
    kit: Option<&KitCtx>,
    visitor: Option<IpAddr>,
    signal: G,
) -> FirstRequest
where
    S: AsyncRead + AsyncWrite + Unpin,
    F: FnOnce() -> bool,
    G: FnOnce() -> Option<String>,
{
    let p = crate::client_ip::peek_head(client).await;
    // 0.46.9: the transfer door (`transfer_door`), App up or down.
    if let Some((head, len)) = crate::transfer_door::door_head(&p.bytes) {
        {
            let mut r = Replay::new(p.bytes, client);
            crate::transfer_door::serve(&mut r, &head, len).await;
        }
        let _ = client.shutdown().await;
        return FirstRequest::Answered;
    }
    if let Some((head, len)) = lane_head(&p.bytes) {
        let open = open();
        {
            let mut r = Replay::new(p.bytes, client);
            sew_lane::serve_app(&mut r, &head, len, open, sew_addr, kit).await;
        }
        let _ = client.shutdown().await;
        return FirstRequest::Answered;
    }
    // The native engine's media signalling. A shut gate: the App's, as before.
    if let Some((head, len)) = signal_lane::native_head(&p.bytes) {
        if let Some(addr) = signal() {
            {
                let mut r = Replay::new(p.bytes, client);
                signal_lane::serve(&mut r, &head, len, &addr, visitor).await;
            }
            let _ = client.shutdown().await;
            return FirstRequest::Answered;
        }
        return FirstRequest::Pass(p.bytes);
    }
    if let (Some(l), Some((method, path, host))) = (local, &p.plain) {
        if let Some(bytes) = l(method, path, host.as_deref()) {
            let _ = client.write_all(&bytes).await;
            let _ = client.shutdown().await;
            return FirstRequest::Answered;
        }
    }
    FirstRequest::Pass(p.bytes)
}

// ── tests: a door per App id, without a box ─────────────────────────────

#[cfg(test)]
#[derive(Debug, Clone)]
pub struct TestDoor {
    pub open: bool,
    pub sew_addr: String,
    /// The signal listener when the App's `sew-signal` is approved.
    pub signal: Option<String>,
}

#[cfg(test)]
fn test_doors() -> &'static Mutex<std::collections::HashMap<String, TestDoor>> {
    static T: OnceLock<Mutex<std::collections::HashMap<String, TestDoor>>> = OnceLock::new();
    T.get_or_init(Default::default)
}

#[cfg(test)]
fn test_door(app_id: &str) -> Option<TestDoor> {
    test_doors().lock().unwrap_or_else(|e| e.into_inner()).get(app_id).cloned()
}

/// Tests: `app_id`'s door, as if voice were installed here, `open` or shut,
/// relaying to `sew_addr`.
#[cfg(test)]
pub fn set_test_door(app_id: &str, open: bool, sew_addr: &str) {
    test_doors().lock().unwrap_or_else(|e| e.into_inner()).insert(app_id.to_string(), TestDoor { open, sew_addr: sew_addr.to_string(), signal: None });
}

/// Tests: `app_id`'s door as [`set_test_door`], with its `sew-signal`
/// approved at `signal_addr` (`None`: not approved).
#[cfg(test)]
pub fn set_test_signal(app_id: &str, open: bool, sew_addr: &str, signal_addr: Option<&str>) {
    let t = TestDoor { open, sew_addr: sew_addr.to_string(), signal: signal_addr.map(str::to_string) };
    test_doors().lock().unwrap_or_else(|e| e.into_inner()).insert(app_id.to_string(), t);
}

#[cfg(test)]
#[path = "voice_door_tests.rs"]
mod tests;
