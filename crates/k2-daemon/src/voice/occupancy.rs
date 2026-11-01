//! Who is in which voice room: the daemon's follower of Sew's SW-3 feed
//! (prd-k2-voice-rooms-v1 v1.9 §9.1, §6.6 2.2 SW-3; vs-live V14, V15).
//!
//! **Daemon-first, headless-safe.** One thread (`voice-occupancy`, started
//! after `set_ready()`) follows `GET /mgmt/v1/spaces/{id}/voice/changes`
//! for the K2 Admin Space while the voice rooms preview is on, Sew reports
//! `mgmt_voice_states` and the Space is ready. It runs with no client
//! attached; clients read `GET /cli/voice/rooms` and refetch on the
//! `voice_rooms_changed {rev}` event this module publishes on the session
//! events bus (a hosting change rides the same event: the list carries it). With the preview off (or no Sew) it holds nothing and
//! sends nothing.
//!
//! **The feed** ([`Feed::step`]):
//! - no cursor → `voice/states` (a snapshot and a cursor; the plain 10 s
//!   mgmt timeout);
//! - a cursor → `voice/changes?after=<cursor>&wait=30&limit=500` with a
//!   per-request timeout of `wait + 10 s` ([`crate::sew_mgmt::long_poll_timeout`]),
//!   never the 10 s mgmt default;
//! - `410 gone` `cursor_expired` (a Sew restart moves the epoch, or the
//!   ring dropped the cursor) → drop the cursor and re-read `voice/states`;
//! - any other failure → back off (1 s doubling to 30 s) and try again; the
//!   last snapshot stays shown, marked `retrying`.
//!
//! `channel_closing` / `channel_archived` changes also move the Admin
//! Space's room ledger (`tool_hook::admin_room_state`, lane B's hook).
//!
//! **Names** (V15): Sew names Connect members `k2:connect:<uuid>`; the
//! occupant's `person` is that login's name on this box (the owner is
//! `owner`), read from the Connect user store and cached.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

use k2_core::tool_hook::sew_wire::{
    self, voice_change, ExternalId, MgmtVoiceChange, MgmtVoiceState, VoiceChangesResponse, VoiceStatesResponse,
};

use crate::sew_mgmt::{long_poll_timeout, MgmtClient, MgmtResponse};

/// `wait` the follower asks Sew to hold an empty answer, seconds.
pub const FEED_WAIT_S: u32 = sew_wire::VOICE_CHANGES_WAIT_MAX_S;
/// Changes per read.
pub const FEED_LIMIT: u32 = sew_wire::VOICE_CHANGES_LIMIT_MAX;
/// How long the follower idles between looks while there is nothing to
/// follow (or until a kick).
const IDLE_EVERY: Duration = Duration::from_secs(30);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// How stale the login-name cache may be before a miss reloads it.
const NAMES_TTL: Duration = Duration::from_secs(10);

// ── the occupancy store ──────────────────────────────────────────────

/// One person in a voice room.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Occupant {
    /// Sew's member id (what moderation names).
    pub member_id: String,
    /// Sew's external id (`k2:owner`, `k2:connect:<uuid>`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_id: Option<String>,
    /// Who this is on this box: `owner`, a Connect login's name,
    /// `agent:<uuid>`, else the external id (or `member:<id>`).
    pub person: String,
    /// `connecting` | `connected`.
    pub connection: String,
    pub self_mute: bool,
    pub self_deaf: bool,
    pub server_mute: bool,
    pub server_deaf: bool,
    pub joined_at: String,
}

/// The follower's own health (`k2 voice status`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedStatus {
    /// `off` (nothing to follow) | `live` | `retrying`.
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub space_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_ok_ms: Option<i64>,
    /// Snapshots re-read after `410 cursor_expired`.
    pub resyncs: u64,
}

/// Who is in each voice channel, as the feed last saw it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Occupancy {
    /// member id → (channel id, occupant). One call per person.
    members: BTreeMap<String, (String, Occupant)>,
}

/// A room state the feed saw (`closing` / `archived`), for lane B's ledger.
pub type RoomNote = (String, &'static str);

impl Occupancy {
    /// Replace everything with a `voice/states` snapshot.
    pub fn replace(&mut self, st: &VoiceStatesResponse, name: &dyn Fn(Option<&ExternalId>, &str) -> String) -> bool {
        let mut next = BTreeMap::new();
        for s in &st.states {
            next.insert(s.member_id.clone(), (s.channel_id.clone(), occupant(s, name)));
        }
        let changed = next != self.members;
        self.members = next;
        changed
    }

    /// Apply feed changes in order. Returns whether anything moved and the
    /// room states seen.
    pub fn apply(&mut self, changes: &[MgmtVoiceChange], name: &dyn Fn(Option<&ExternalId>, &str) -> String) -> (bool, Vec<RoomNote>) {
        let mut changed = false;
        let mut notes = Vec::new();
        for c in changes {
            match c.kind.as_str() {
                voice_change::JOINED | voice_change::UPDATED => {
                    let Some(st) = c.state.as_ref() else { continue };
                    let o = occupant(st, name);
                    let ch = if st.channel_id.is_empty() { c.channel_id.clone() } else { st.channel_id.clone() };
                    let prev = self.members.insert(st.member_id.clone(), (ch.clone(), o.clone()));
                    changed |= prev != Some((ch, o));
                }
                voice_change::LEFT => {
                    if let Some(m) = c.member_id.as_deref() {
                        changed |= self.members.remove(m).is_some();
                    }
                }
                voice_change::CHANNEL_CLOSING => {
                    notes.push((c.channel_id.clone(), k2_core::tool_hook::admin_space::ROOM_CLOSING));
                    changed = true;
                }
                voice_change::CHANNEL_ARCHIVED => {
                    notes.push((c.channel_id.clone(), k2_core::tool_hook::admin_space::ROOM_ARCHIVED));
                    // Archiving ends the room's calls (Sew also sends each
                    // `left`); drop them here so the view never lags.
                    self.members.retain(|_, (ch, _)| ch != &c.channel_id);
                    changed = true;
                }
                // A kind a newer Sew adds: nothing to do here.
                _ => {}
            }
        }
        (changed, notes)
    }

    /// Occupants by channel id, each list by join time then member id.
    pub fn by_channel(&self) -> BTreeMap<String, Vec<Occupant>> {
        let mut out: BTreeMap<String, Vec<Occupant>> = BTreeMap::new();
        for (ch, o) in self.members.values() {
            out.entry(ch.clone()).or_default().push(o.clone());
        }
        for v in out.values_mut() {
            v.sort_by(|a, b| a.joined_at.cmp(&b.joined_at).then_with(|| a.member_id.cmp(&b.member_id)));
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }
}

fn occupant(s: &MgmtVoiceState, name: &dyn Fn(Option<&ExternalId>, &str) -> String) -> Occupant {
    Occupant {
        member_id: s.member_id.clone(),
        external_id: s.external_id.as_ref().map(|e| e.to_string()),
        person: name(s.external_id.as_ref(), &s.member_id),
        connection: s.connection.clone(),
        self_mute: s.self_mute,
        self_deaf: s.self_deaf,
        server_mute: s.server_mute,
        server_deaf: s.server_deaf,
        joined_at: s.joined_at.clone(),
    }
}

/// The name an occupant shows under (V15), given a uuid → login lookup.
pub fn person_name(ext: Option<&ExternalId>, member_id: &str, login_of: &dyn Fn(&str) -> Option<String>) -> String {
    match ext {
        Some(ExternalId::K2Owner) => "owner".into(),
        Some(ExternalId::K2Connect(uuid)) => login_of(uuid).unwrap_or_else(|| format!("k2:connect:{uuid}")),
        Some(ExternalId::K2Workspace(uuid)) => format!("agent:{uuid}"),
        Some(other) => other.to_string(),
        None => format!("member:{member_id}"),
    }
}

// ── uuid → login (V15) ───────────────────────────────────────────────

fn names() -> &'static Mutex<Option<(Instant, HashMap<String, String>)>> {
    static N: Mutex<Option<(Instant, HashMap<String, String>)>> = Mutex::new(None);
    &N
}

/// A Connect login's name by its uuid (Sew's `k2:connect:<uuid>`). A miss
/// reloads the user store at most every [`NAMES_TTL`].
pub fn login_of_uuid(uuid: &str) -> Option<String> {
    let mut g = names().lock().unwrap_or_else(|p| p.into_inner());
    if let Some((_, m)) = g.as_ref() {
        if let Some(n) = m.get(uuid) {
            return Some(n.clone());
        }
    }
    if g.as_ref().is_some_and(|(at, _)| at.elapsed() < NAMES_TTL) {
        return None;
    }
    let map: HashMap<String, String> = k2_core::connect_users::admin_space_facts()
        .map(|rows| rows.into_iter().map(|(id, username, _, _)| (id, username)).collect())
        .unwrap_or_default();
    let hit = map.get(uuid).cloned();
    *g = Some((Instant::now(), map));
    hit
}

fn live_name(ext: Option<&ExternalId>, member_id: &str) -> String {
    person_name(ext, member_id, &login_of_uuid)
}

// ── the shared store ─────────────────────────────────────────────────

#[derive(Default)]
struct Shared {
    occ: Occupancy,
    status: FeedStatus,
}

fn shared() -> &'static Mutex<Shared> {
    static S: std::sync::OnceLock<Mutex<Shared>> = std::sync::OnceLock::new();
    S.get_or_init(|| Mutex::new(Shared::default()))
}

static REV: AtomicU64 = AtomicU64::new(0);

/// The rooms view's revision (bumped on every change clients should see).
pub fn rev() -> u64 {
    REV.load(Ordering::Relaxed)
}

/// Occupants by channel id, and the revision they belong to.
pub fn snapshot() -> (u64, BTreeMap<String, Vec<Occupant>>) {
    let g = shared().lock().unwrap_or_else(|p| p.into_inner());
    (rev(), g.occ.by_channel())
}

/// The follower's health.
pub fn feed_status() -> FeedStatus {
    let mut s = shared().lock().unwrap_or_else(|p| p.into_inner()).status.clone();
    if s.state.is_empty() {
        s.state = "off".into();
    }
    s
}

fn set_status(f: impl FnOnce(&mut FeedStatus)) {
    let mut g = shared().lock().unwrap_or_else(|p| p.into_inner());
    f(&mut g.status);
}

/// Tell clients the rooms view moved: `voice_rooms_changed {rev}`.
/// Nothing goes out while the preview is off.
pub fn emit_rooms_changed() {
    if !k2_core::voice_rooms::enabled() {
        return;
    }
    let rev = REV.fetch_add(1, Ordering::Relaxed) + 1;
    let _ = crate::session_events::emit(crate::session_events::SessionEvent::VoiceRoomsChanged { rev });
}

/// The lane moved (lane A's [`super::lane::publish`]): rooms' availability
/// changed, so clients refetch, and the follower looks again.
pub fn lane_moved() {
    emit_rooms_changed();
    kick();
}

// ── the feed ─────────────────────────────────────────────────────────

/// What one [`Feed::step`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Read `voice/states` (first look, a new Space, or after a 410).
    Snapshot { changed: bool },
    /// Read `voice/changes`.
    Changes { count: usize, changed: bool },
    /// `410 cursor_expired`: the next step re-reads `voice/states`.
    Expired,
}

impl Step {
    pub fn changed(&self) -> bool {
        match self {
            Step::Snapshot { changed } | Step::Changes { changed, .. } => *changed,
            Step::Expired => false,
        }
    }
}

/// The follower's cursor on one Space.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Feed {
    pub space_id: Option<String>,
    pub cursor: Option<String>,
}

fn sew_error_reason(r: &MgmtResponse) -> String {
    let v: serde_json::Value = serde_json::from_slice(&r.body).unwrap_or_default();
    v.pointer("/error/details/reason")
        .and_then(|x| x.as_str())
        .or_else(|| v.pointer("/error/code").and_then(|x| x.as_str()))
        .unwrap_or("")
        .to_string()
}

impl Feed {
    /// One read of the feed for `space_id` (module doc). Applies what it
    /// read to the shared store and returns the room notes.
    pub async fn step(&mut self, c: &MgmtClient, space_id: &str, wait_s: u32) -> Result<(Step, Vec<RoomNote>), String> {
        if self.space_id.as_deref() != Some(space_id) {
            self.space_id = Some(space_id.to_string());
            self.cursor = None;
        }
        let Some(cursor) = self.cursor.clone() else {
            let r = c.get(&sew_wire::voice_states_path(space_id)).await.map_err(|e| format!("voice/states: {e}"))?;
            if !r.is_success() {
                return Err(format!("voice/states: {}", r.error_line()));
            }
            let st: VoiceStatesResponse =
                serde_json::from_slice(&r.body).map_err(|e| format!("voice/states: unexpected answer ({e})"))?;
            let changed = {
                let mut g = shared().lock().unwrap_or_else(|p| p.into_inner());
                g.occ.replace(&st, &live_name)
            };
            self.cursor = Some(st.cursor);
            return Ok((Step::Snapshot { changed }, Vec::new()));
        };
        let path = sew_wire::voice_changes_path(space_id, &cursor, wait_s, FEED_LIMIT);
        let timeout = long_poll_timeout(Duration::from_secs(u64::from(wait_s)));
        let r = c.get_with_timeout(&path, timeout).await.map_err(|e| format!("voice/changes: {e}"))?;
        if r.status == 410 && sew_error_reason(&r) == sew_wire::reasons::CURSOR_EXPIRED {
            self.cursor = None;
            set_status(|s| s.resyncs += 1);
            return Ok((Step::Expired, Vec::new()));
        }
        if !r.is_success() {
            return Err(format!("voice/changes: {}", r.error_line()));
        }
        let ch: VoiceChangesResponse =
            serde_json::from_slice(&r.body).map_err(|e| format!("voice/changes: unexpected answer ({e})"))?;
        let (changed, notes) = {
            let mut g = shared().lock().unwrap_or_else(|p| p.into_inner());
            g.occ.apply(&ch.changes, &live_name)
        };
        self.cursor = Some(ch.next);
        Ok((Step::Changes { count: ch.changes.len(), changed }, notes))
    }
}

/// Clear the store (nothing to follow). Returns whether it held anything.
pub fn clear() -> bool {
    let mut g = shared().lock().unwrap_or_else(|p| p.into_inner());
    let had = !g.occ.is_empty();
    g.occ = Occupancy::default();
    g.status = FeedStatus { state: "off".into(), resyncs: g.status.resyncs, ..Default::default() };
    had
}

// ── the follower thread ──────────────────────────────────────────────

struct Kick {
    n: Mutex<u64>,
    cv: Condvar,
}

fn kicker() -> &'static Kick {
    static K: Kick = Kick { n: Mutex::new(0), cv: Condvar::new() };
    &K
}

/// Wake the follower (a preview flip, a lane change, a room change).
pub fn kick() {
    let k = kicker();
    *k.n.lock().unwrap_or_else(|p| p.into_inner()) += 1;
    k.cv.notify_all();
}

fn wait_kick(seen: u64, d: Duration) -> u64 {
    let k = kicker();
    let g = k.n.lock().unwrap_or_else(|p| p.into_inner());
    let (g, _) = k.cv.wait_timeout_while(g, d, |n| *n == seen).unwrap_or_else(|p| p.into_inner());
    *g
}

fn kick_count() -> u64 {
    *kicker().n.lock().unwrap_or_else(|p| p.into_inner())
}

static STARTED: AtomicBool = AtomicBool::new(false);
static HOSTS_ROOMS: AtomicBool = AtomicBool::new(false);

/// Whether this daemon hosts voice rooms now (the `voice-rooms-v1`
/// `/boot-status` feature): the preview on, Linux, the Admin Space ready,
/// Sew able to hold rooms, Pro, and at least one room.
pub fn hosts_rooms() -> bool {
    HOSTS_ROOMS.load(Ordering::Relaxed) && k2_core::voice_rooms::enabled()
}

/// What the follower follows now: a mgmt client and the Admin Space id.
fn follow_target() -> Option<(MgmtClient, String)> {
    if !k2_core::voice_rooms::enabled() {
        return None;
    }
    let flags = k2_core::tool_hook::sew_voice_features().ok()?;
    if !flags.mgmt_voice_states {
        return None;
    }
    let space = k2_core::tool_hook::admin_space_id().ok()??;
    let t = k2_core::tool_hook::get_target(k2_core::tool_hook::CHAT_KIND).ok()??;
    if !t.enabled {
        return None;
    }
    Some((MgmtClient::new(&t.socket, &t.token_file), space))
}

/// Start the follower once (after `set_ready()`). Idle while the preview
/// is off.
pub fn spawn() -> Option<std::thread::JoinHandle<()>> {
    if STARTED.swap(true, Ordering::SeqCst) {
        return None;
    }
    match std::thread::Builder::new().name("voice-occupancy".into()).spawn(run) {
        Ok(h) => Some(h),
        Err(e) => {
            k2_core::log_debug!("[voice-rooms] occupancy follower didn't start: {e}");
            None
        }
    }
}

fn run() {
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            k2_core::log_debug!("[voice-rooms] occupancy follower runtime: {e}");
            return;
        }
    };
    let mut feed = Feed::default();
    let mut backoff = Duration::from_secs(1);
    let mut last_hosting = String::new();
    loop {
        let seen = kick_count();
        let h = super::hosting_state();
        HOSTS_ROOMS.store(h.state == super::HOSTING_OK, Ordering::Relaxed);
        if h.state != last_hosting {
            // The rooms list carries `hosting`, so one event covers both.
            if !last_hosting.is_empty() {
                emit_rooms_changed();
            }
            last_hosting = h.state.clone();
        }
        let Some((client, space)) = follow_target() else {
            feed = Feed::default();
            if clear() {
                emit_rooms_changed();
            }
            wait_kick(seen, IDLE_EVERY);
            continue;
        };
        match rt.block_on(feed.step(&client, &space, FEED_WAIT_S)) {
            Ok((step, notes)) => {
                backoff = Duration::from_secs(1);
                set_status(|s| {
                    s.state = "live".into();
                    s.space_id = Some(space.clone());
                    s.last_error = None;
                    s.last_ok_ms = Some(chrono::Utc::now().timestamp_millis());
                });
                for (channel, state) in &notes {
                    if let Err(e) = k2_core::tool_hook::admin_room_state(channel, state) {
                        k2_core::log_debug!("[voice-rooms] room {channel} → {state}: {e}");
                    }
                }
                if step.changed() {
                    emit_rooms_changed();
                }
            }
            Err(e) => {
                k2_core::log_debug!("[voice-rooms] occupancy feed: {e} (again in {}s)", backoff.as_secs());
                set_status(|s| {
                    s.state = "retrying".into();
                    s.space_id = Some(space.clone());
                    s.last_error = Some(e);
                });
                wait_kick(seen, backoff);
                backoff = (backoff * 2).min(BACKOFF_MAX);
            }
        }
    }
}
