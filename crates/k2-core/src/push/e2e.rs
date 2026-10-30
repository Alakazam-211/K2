//! End-to-end encrypted pushes: who gets what (prd-e2e-push-v1 rev 2 §4,
//! §4a, §5) and the sealed plaintexts.
//!
//! **Additive by rule (Rosson 2026-10-20):** today's push path is kept,
//! byte for byte, for every device without a registered key and for every
//! device when the switch is `off`. The sealed path runs BESIDE it, only for
//! a device that has an active key, and only on a relay that advertised the
//! `s1` capability (`X-K2-Push-Caps: s1`).
//!
//! | Effective mode | key + relay `s1` | otherwise |
//! |---|---|---|
//! | `off` | today's path | today's path |
//! | `sealed` (override) / `sealed-when-supported` | sealed | today's path |
//! | `sealed-only` | sealed | legacy-generic |

use super::{BadgePush, PushData, PushEvent, PushStamp, PUSH_SOUND_DEFAULT};
use crate::push::seal::{self, SealError, SealKind};
use crate::push_device_keys::DeviceOverride;
use unicode_segmentation::UnicodeSegmentation as _;

/// The owner's server switch, `pushE2E`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushE2eMode {
    /// Today's pushes for every phone (instant rollback).
    Off,
    /// Sealed for keyed phones; today's pushes for the rest (the default).
    SealedWhenSupported,
    /// Sealed for keyed phones; generic "K2 / New message" for the rest.
    SealedOnly,
}

/// Wire values, in order.
pub const PUSH_E2E_MODES: &[&str] = &["off", "sealed-when-supported", "sealed-only"];

impl PushE2eMode {
    pub const DEFAULT: Self = Self::SealedWhenSupported;

    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::SealedWhenSupported => "sealed-when-supported",
            Self::SealedOnly => "sealed-only",
        }
    }

    pub fn from_wire(s: &str) -> Option<Self> {
        match s.trim() {
            "off" => Some(Self::Off),
            "sealed-when-supported" => Some(Self::SealedWhenSupported),
            "sealed-only" => Some(Self::SealedOnly),
            _ => None,
        }
    }
}

#[cfg(test)]
thread_local! {
    static TEST_MODE: std::cell::Cell<Option<PushE2eMode>> = const { std::cell::Cell::new(None) };
}

/// Test-only: this thread's server mode (`None` = the default). Tests never
/// read the real settings file.
#[cfg(test)]
pub fn set_test_mode(mode: Option<PushE2eMode>) {
    TEST_MODE.with(|c| c.set(mode));
}

/// The server's `pushE2E` now.
pub fn server_mode() -> PushE2eMode {
    #[cfg(test)]
    {
        TEST_MODE.with(|c| c.get()).unwrap_or(PushE2eMode::DEFAULT)
    }
    #[cfg(not(test))]
    {
        PushE2eMode::from_wire(&crate::app_settings::load().push_e2e).unwrap_or(PushE2eMode::DEFAULT)
    }
}

/// The mode that applies to one device (§4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effective {
    Off,
    Sealed,
    SealedOnly,
}

/// The device override if set, else the server's mode.
pub fn effective(server: PushE2eMode, ov: Option<DeviceOverride>) -> Effective {
    match ov {
        Some(DeviceOverride::Off) => Effective::Off,
        Some(DeviceOverride::Sealed) => Effective::Sealed,
        None => match server {
            PushE2eMode::Off => Effective::Off,
            PushE2eMode::SealedWhenSupported => Effective::Sealed,
            PushE2eMode::SealedOnly => Effective::SealedOnly,
        },
    }
}

/// `e2e.enforce` for a device: `sealed-only` with no `off` override.
pub fn enforce(server: PushE2eMode, ov: Option<DeviceOverride>) -> bool {
    server == PushE2eMode::SealedOnly && ov != Some(DeviceOverride::Off)
}

/// Which path one device's push takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    /// Today's path, unchanged.
    Legacy,
    /// Sealed for this device.
    Sealed,
    /// `sealed-only`: "K2 / New message", no names or ids.
    Generic,
}

/// §4's table. `sealed_ok` = the relay advertised `s1` and the per-daemon
/// path is up.
pub fn lane(eff: Effective, active_key: bool, sealed_ok: bool) -> Lane {
    match eff {
        Effective::Off => Lane::Legacy,
        Effective::Sealed if active_key && sealed_ok => Lane::Sealed,
        Effective::Sealed => Lane::Legacy,
        Effective::SealedOnly if active_key && sealed_ok => Lane::Sealed,
        Effective::SealedOnly => Lane::Generic,
    }
}

// ── Relay capability (`X-K2-Push-Caps: s1`) ──────────────────────────────

/// The relay's capability header: a comma-separated token list. The S0
/// marker (`X-K2-Push-Gateway: s0`) never changes, because daemons up to
/// 0.46.2 compare it to exactly `s0`.
pub const CAPS_HEADER: &str = "X-K2-Push-Caps";
/// The token that says the relay accepts the sealed kinds.
pub const CAPS_S1: &str = "s1";

/// The token that says the relay also accepts format-2 blobs (the bigger
/// buckets, push-e2e-v2, 0.46.7): `data.v: "2"`, an alert `e2e` of 3951
/// characters and a nudge of 1476.
pub const CAPS_S2: &str = "s2";

fn caps_has(value: &str, token: &str) -> bool {
    value.split(',').any(|t| t.trim().eq_ignore_ascii_case(token))
}

/// True when a `X-K2-Push-Caps` value lists `s1`.
pub fn caps_has_s1(value: &str) -> bool {
    caps_has(value, CAPS_S1)
}

/// True when a `X-K2-Push-Caps` value lists `s2`.
pub fn caps_has_s2(value: &str) -> bool {
    caps_has(value, CAPS_S2)
}

/// What a relay last said it accepts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RelayCaps {
    /// Sealed kinds, format 1.
    pub s1: bool,
    /// Format 2 too (only meaningful with `s1`).
    pub s2: bool,
}

type CapsMap = std::collections::HashMap<String, RelayCaps>;

static RELAY_CAPS: std::sync::Mutex<Option<CapsMap>> = std::sync::Mutex::new(None);

fn relay_map() -> std::sync::MutexGuard<'static, Option<CapsMap>> {
    RELAY_CAPS.lock().unwrap_or_else(|p| p.into_inner())
}

/// Cache the relay's capability from a per-daemon-path response (any
/// status). A response without `s1` (a rollback, or a failover to an
/// older relay) sends the next dispatch back to today's path; one without
/// `s2` sends format-2 phones back to format 1. A change is persisted
/// (`push_relay_caps`, migrations 0211 + 0251) so the first push after a
/// restart is sealed too.
pub fn note_relay_caps(gateway_url: &str, s1: bool, s2: bool) {
    let caps = RelayCaps { s1, s2 };
    let changed = {
        let mut g = relay_map();
        let map = g.get_or_insert_with(Default::default);
        map.insert(gateway_url.to_string(), caps) != Some(caps)
    };
    if changed {
        if let Err(e) = store_relay_caps(gateway_url, caps) {
            crate::log_debug!("[push] relay caps store failed: {e}");
        }
    }
}

/// The last seen capability for this gateway: memory first, then the
/// stored value from before a restart; nothing when never seen (a fresh
/// install: the first push takes today's path).
pub fn relay_caps(gateway_url: &str) -> RelayCaps {
    if let Some(v) = relay_map().as_ref().and_then(|m| m.get(gateway_url)).copied() {
        return v;
    }
    let stored = match load_relay_caps_full(gateway_url) {
        Ok(v) => v.unwrap_or_default(),
        Err(e) => {
            crate::log_debug!("[push] relay caps read failed: {e}");
            RelayCaps::default()
        }
    };
    relay_map().get_or_insert_with(Default::default).insert(gateway_url.to_string(), stored);
    stored
}

/// The relay accepts the sealed kinds (`s1`).
pub fn relay_s1(gateway_url: &str) -> bool {
    relay_caps(gateway_url).s1
}

/// The relay accepts format 2 too (`s1` and `s2`).
pub fn relay_s2(gateway_url: &str) -> bool {
    let c = relay_caps(gateway_url);
    c.s1 && c.s2
}

fn store_relay_caps(gateway_url: &str, caps: RelayCaps) -> Result<(), String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let db = crate::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO push_relay_caps (gateway_url, s1, s2, seen_at) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(gateway_url) DO UPDATE SET s1 = excluded.s1, s2 = excluded.s2, seen_at = excluded.seen_at",
        rusqlite::params![gateway_url, i64::from(caps.s1), i64::from(caps.s2), now],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

/// The stored `s1` (`None` = never seen).
pub fn load_relay_caps(gateway_url: &str) -> Result<Option<bool>, String> {
    load_relay_caps_full(gateway_url).map(|c| c.map(|c| c.s1))
}

/// The stored capability (`None` = never seen).
pub fn load_relay_caps_full(gateway_url: &str) -> Result<Option<RelayCaps>, String> {
    use rusqlite::OptionalExtension as _;
    let db = crate::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT s1, s2 FROM push_relay_caps WHERE gateway_url = ?1",
        rusqlite::params![gateway_url],
        |r| Ok(RelayCaps { s1: r.get::<_, i64>(0)? != 0, s2: r.get::<_, i64>(1)? != 0 }),
    )
    .optional()
    .map_err(|e| e.to_string())
}

/// Test-only: forget the in-memory capability for one gateway, as a daemon
/// restart does (the stored value stays).
#[cfg(test)]
pub fn forget_relay_caps_memory(gateway_url: &str) {
    if let Some(m) = relay_map().as_mut() {
        m.remove(gateway_url);
    }
}

// ── Sealed plaintexts (§4, §5) ───────────────────────────────────────────

/// What every sealed plaintext carries besides the content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealCtx {
    /// The key's id (hex of the first 8 bytes of SHA-256(public key)).
    pub kid: String,
    /// The replay counter ([`crate::push_device_keys::take_ctr`]).
    pub ctr: u64,
    /// Unix seconds.
    pub ts: i64,
    /// The server stamp (`srv.sub`, `srv.iid`).
    pub stamp: PushStamp,
    /// The person's `badgeKey` (`bk`).
    pub badge_key: String,
}

fn head(kind: &str, ctx: &SealCtx) -> serde_json::Map<String, serde_json::Value> {
    let mut m = serde_json::Map::new();
    m.insert("v".into(), 1.into());
    m.insert("kid".into(), ctx.kid.clone().into());
    m.insert("ctr".into(), ctx.ctr.into());
    m.insert("ts".into(), ctx.ts.into());
    m.insert("kind".into(), kind.into());
    m.insert(
        "srv".into(),
        serde_json::json!({ "sub": ctx.stamp.subdomain, "iid": ctx.stamp.instance_id }),
    );
    m.insert("bk".into(), ctx.badge_key.clone().into());
    m
}

/// The sealed `kind` of an alert event: `thread`, `ticket`, `project` or
/// `chat_approval`.
/// `None` for the kinds that are never sealed (badge → nudge; the
/// desktop-era kinds).
pub fn alert_kind(event: &PushEvent) -> Option<&'static str> {
    match event {
        PushEvent::ThreadPost(_) => Some(super::PUSH_KIND_THREAD),
        PushEvent::FeedbackCreated { .. } | PushEvent::FeedbackCommented { .. } => Some(super::PUSH_KIND_TICKET),
        PushEvent::ProjectMessageCreated { .. } | PushEvent::ProjectPost(_) => Some(super::PUSH_KIND_PROJECT),
        // Sew S30: an agent's chat post waits for approval. Sealed like the
        // other alerts (it names the agent; it never carries the body).
        PushEvent::ChatApprovalRequested { .. } => Some(super::PUSH_KIND_CHAT_APPROVAL),
        PushEvent::Badge(_) | PushEvent::AgentNeedsAttention { .. } | PushEvent::HeartbeatFailed { .. } => None,
    }
}

/// True for the events the sealed path handles (alerts and the badge nudge).
pub fn is_sealable(event: &PushEvent) -> bool {
    alert_kind(event).is_some() || matches!(event, PushEvent::Badge(_))
}

/// The phone's grouping key (`threadIdentifier` / Android `tag`), sealed.
pub fn alert_group(event: &PushEvent) -> String {
    match event {
        PushEvent::ThreadPost(t) => format!("thread:{}", t.conversation_id),
        PushEvent::FeedbackCreated { feedback_id, .. } | PushEvent::FeedbackCommented { feedback_id, .. } => {
            format!("ticket:{feedback_id}")
        }
        PushEvent::ProjectMessageCreated { group_id, .. } => format!("project:{group_id}"),
        PushEvent::ProjectPost(p) => format!("project:{}", p.group_id),
        PushEvent::ChatApprovalRequested { outbox_id, .. } => format!("chat_approval:{outbox_id}"),
        _ => String::new(),
    }
}

/// The sealed alert plaintext (§4) for `event` as THIS device would see it
/// today: pass [`PushEvent::without_preview`] for a device with previews
/// off. `title`/`body` are what today's path shows; `data` is today's
/// `PushData` for the kind (the server stamp rides in `srv`/`bk`).
pub fn alert_plaintext(event: &PushEvent, ctx: &SealCtx) -> Option<serde_json::Value> {
    let kind = alert_kind(event)?;
    let mut m = head(kind, ctx);
    m.insert("title".into(), event.title().into());
    m.insert("body".into(), event.body().into());
    m.insert("group".into(), alert_group(event).into());
    m.insert("data".into(), event.data().to_value());
    if let PushEvent::ThreadPost(t) = event {
        if let Some(msg) = &t.msg {
            m.insert("msg".into(), msg_value(msg));
        }
    }
    Some(serde_json::Value::Object(m))
}

/// The sealed `msg` object (prd-e2e-push-v1 follow-up): the latest post for
/// the phone to show before it fetches. Sealed alerts only.
pub fn msg_value(msg: &super::ThreadMsg) -> serde_json::Value {
    serde_json::json!({
        "id": msg.id,
        "seq": msg.seq,
        "conversationId": msg.conversation_id,
        "from": msg.from,
        "at": msg.at,
        "kind": msg.kind.as_wire(),
        "text": msg.text,
        "truncated": msg.truncated,
    })
}

/// Fit by cutting `msg.text` (grapheme boundary, `truncated: true`): the
/// longest cut that fits. `None` when even an empty text doesn't fit.
fn fit_msg_text(v: &serde_json::Value, max: usize) -> Option<serde_json::Value> {
    let text = v.pointer("/msg/text").and_then(|t| t.as_str())?.to_string();
    let total = text.graphemes(true).count();
    let with = |k: usize| {
        let mut c = v.clone();
        c["msg"]["text"] = first_graphemes(&text, k).into();
        c["msg"]["truncated"] = true.into();
        c
    };
    if json_bytes(&with(0)).len() > max {
        return None;
    }
    let (mut lo, mut hi) = (0usize, total.saturating_sub(1));
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if json_bytes(&with(mid)).len() <= max {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    Some(with(lo))
}

/// The sealed nudge plaintext (§5): totals only. No conversation ids or
/// seqs in format 1, ever.
pub fn nudge_plaintext(b: &BadgePush, ctx: &SealCtx) -> serde_json::Value {
    nudge_plaintext_in(b, ctx, seal::Format::V1)
}

/// [`nudge_plaintext`] in `format`. Format 2 (push-content-available-v1,
/// 0.46.7) also carries the stale hint inside the blob: `data.stale` =
/// `"<conversationId>:<seq>,…"` (newest first) and `data.staleMore` =
/// `"1"` when pairs were left out. Ids and seqs only, never content.
pub fn nudge_plaintext_in(b: &BadgePush, ctx: &SealCtx, format: seal::Format) -> serde_json::Value {
    let mut m = head(super::seal::WIRE_KIND_NUDGE, ctx);
    let mut data = serde_json::json!({ "unreadTotal": b.unread_total, "waitingTotal": b.waiting_total });
    if format == seal::Format::V2 && !b.stale.is_empty() {
        data["stale"] = b.stale.wire().into();
        if b.stale.more {
            data["staleMore"] = "1".into();
        }
    }
    m.insert("data".into(), data);
    serde_json::Value::Object(m)
}

/// Longest display name kept when an alert must shrink (rule step 2).
pub const NAME_CAP_CHARS: usize = 64;
const ELLIPSIS: &str = "…";

fn json_bytes(v: &serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(v).expect("a JSON value always serializes")
}

fn first_graphemes(s: &str, n: usize) -> String {
    s.graphemes(true).take(n).collect()
}

/// Fit by cutting `body` at a character (grapheme) boundary plus "…": the
/// longest cut that fits, or `None` when even "…" doesn't.
fn fit_body(v: &serde_json::Value, max: usize) -> Option<Vec<u8>> {
    let bytes = json_bytes(v);
    if bytes.len() <= max {
        return Some(bytes);
    }
    let body = v.get("body").and_then(|b| b.as_str()).unwrap_or("").to_string();
    let total = body.graphemes(true).count();
    let with = |k: usize| {
        let mut c = v.clone();
        c["body"] = format!("{}{ELLIPSIS}", first_graphemes(&body, k)).into();
        json_bytes(&c)
    };
    if with(0).len() > max {
        return None;
    }
    // Largest k in [0, total) that fits (size grows with k).
    let (mut lo, mut hi) = (0usize, total.saturating_sub(1));
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if with(mid).len() <= max {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    Some(with(lo))
}

fn cap_name(v: &mut serde_json::Value, pointer: &str) {
    if let Some(slot) = v.pointer_mut(pointer) {
        if let Some(s) = slot.as_str() {
            if s.graphemes(true).count() > NAME_CAP_CHARS {
                *slot = format!("{}{ELLIPSIS}", first_graphemes(s, NAME_CAP_CHARS - 1)).into();
            }
        }
    }
}

/// The alert JSON bytes within the 1536-byte bucket (§3), never growing
/// the bucket. In order: cut `msg.text` (`truncated: true`; the app
/// fetches the rest); then cut `body` (+ "…"); then cap names at 64
/// characters; then drop `avatarRev`/`agentColor`.
pub fn fit_alert(v: &serde_json::Value) -> Result<Vec<u8>, SealError> {
    fit_alert_in(v, seal::Format::V1)
}

/// [`fit_alert`] within `format`'s alert bucket (2880 bytes in format 2).
pub fn fit_alert_in(v: &serde_json::Value, format: seal::Format) -> Result<Vec<u8>, SealError> {
    let max = format.max_json(SealKind::Alert);
    let bytes = json_bytes(v);
    if bytes.len() <= max {
        return Ok(bytes);
    }
    let mut v = v.clone();
    if v.get("msg").is_some() {
        match fit_msg_text(&v, max) {
            Some(fitted) => return Ok(json_bytes(&fitted)),
            None => {
                // Even an empty text doesn't fit: keep it empty (truncated)
                // and shrink the rest.
                v["msg"]["text"] = "".into();
                v["msg"]["truncated"] = true.into();
            }
        }
    }
    if let Some(b) = fit_body(&v, max) {
        return Ok(b);
    }
    cap_name(&mut v, "/title");
    cap_name(&mut v, "/data/agentName");
    if let Some(b) = fit_body(&v, max) {
        return Ok(b);
    }
    if let Some(d) = v.get_mut("data").and_then(|d| d.as_object_mut()) {
        d.remove("avatarRev");
        d.remove("agentColor");
    }
    fit_body(&v, max).ok_or_else(|| SealError::TooLarge { len: json_bytes(&v).len(), max })
}

/// The nudge JSON bytes within the 256-byte bucket.
pub fn fit_nudge(v: &serde_json::Value) -> Result<Vec<u8>, SealError> {
    fit_nudge_in(v, seal::Format::V1)
}

/// [`fit_nudge`] within `format`'s bucket. A stale hint that doesn't fit
/// loses pairs from its end (`staleMore: "1"`); the totals never shrink.
pub fn fit_nudge_in(v: &serde_json::Value, format: seal::Format) -> Result<Vec<u8>, SealError> {
    let max = format.max_json(SealKind::Nudge);
    let bytes = json_bytes(v);
    if bytes.len() <= max {
        return Ok(bytes);
    }
    let mut v = v.clone();
    loop {
        let Some(stale) = v.pointer("/data/stale").and_then(|s| s.as_str()).map(str::to_string) else { break };
        let mut pairs: Vec<&str> = stale.split(',').collect();
        pairs.pop();
        if pairs.is_empty() {
            if let Some(d) = v.get_mut("data").and_then(|d| d.as_object_mut()) {
                d.remove("stale");
            }
        } else {
            v["data"]["stale"] = pairs.join(",").into();
        }
        v["data"]["staleMore"] = "1".into();
        let bytes = json_bytes(&v);
        if bytes.len() <= max {
            return Ok(bytes);
        }
    }
    let bytes = json_bytes(&v);
    if bytes.len() > max {
        return Err(SealError::TooLarge { len: bytes.len(), max });
    }
    Ok(bytes)
}

// ── Wire bodies ───────────────────────────────────────────────────────────

/// One sealed device entry: `{platform, token, e2e}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedEntry {
    pub platform: String,
    pub token: String,
    pub e2e: String,
}

/// The relay dispatch body for sealed devices: `payload.data` is exactly
/// `{kind, v}`; no title, body, sound, threadId, collapseId or stamp.
pub fn sealed_dispatch_body(kind: SealKind, entries: &[SealedEntry]) -> serde_json::Value {
    sealed_dispatch_body_in(kind, seal::Format::V1, entries)
}

/// [`sealed_dispatch_body`] for `format`'s blobs (`v: "2"` for format 2:
/// every blob in one call has the same format).
pub fn sealed_dispatch_body_in(kind: SealKind, format: seal::Format, entries: &[SealedEntry]) -> serde_json::Value {
    serde_json::json!({
        "devices": entries
            .iter()
            .map(|e| serde_json::json!({ "platform": e.platform, "token": e.token, "e2e": e.e2e }))
            .collect::<Vec<_>>(),
        "payload": { "data": { "kind": kind.wire_kind(), "v": format.wire_version() } },
    })
}

/// The `sealed-only` payload for a device without a key (§4
/// legacy-generic): "K2" / "New message", `data = {kind, badgeKey,
/// subdomain, instanceId, unreadTotal, waitingTotal}`. No names, preview,
/// ids, `threadId` or `collapseId`. A badge push keeps today's payload (it
/// already carries no names or ids).
pub fn generic_payload(event: &PushEvent, stamp: &PushStamp, badge_key: &str) -> serde_json::Value {
    if let PushEvent::Badge(_) = event {
        return super::K2Cloud::payload(event, stamp, badge_key);
    }
    let full = event.data();
    let mut data = PushData::default();
    if let Some(k) = full.get("kind") {
        data.insert("kind", k);
    }
    if !badge_key.is_empty() {
        data.insert("badgeKey", badge_key);
    }
    if !stamp.subdomain.is_empty() {
        data.insert("subdomain", stamp.subdomain.clone());
    }
    if !stamp.instance_id.is_empty() {
        data.insert("instanceId", stamp.instance_id.clone());
    }
    for k in ["unreadTotal", "waitingTotal"] {
        if let Some(v) = full.get(k) {
            data.insert(k, v);
        }
    }
    serde_json::json!({
        "title": GENERIC_TITLE,
        "body": GENERIC_BODY,
        "data": data,
        "sound": PUSH_SOUND_DEFAULT,
    })
}

/// The constant visible text of a sealed or generic alert.
pub const GENERIC_TITLE: &str = "K2";
pub const GENERIC_BODY: &str = "New message";
