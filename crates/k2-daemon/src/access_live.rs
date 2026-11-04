//! The hub: an open pass finds out at once that its access changed
//! (`prd-live-access-changes-v1` S2, §6).
//!
//! `k2_core::skin::access_live` says "an access table changed" and nothing more.
//! Here, each distinct `(link, raw pass)` held open by a socket is one entry. On a
//! wake it is re-resolved once and its answer fingerprinted. Fingerprint the same:
//! nobody hears anything, so a no-op write or someone else's change sends nothing.
//! Different, or the pass is gone: the entry's `watch` tells that pass's sockets,
//! which narrow or close against their own room and capability without another
//! read.
//!
//! The walk runs on its own thread, one resolve at a time. Each resolve takes the
//! skin mutex by itself, so other requests get in between. A `tokens` write
//! re-resolves only the entries it names. Otherwise: at most one walk per 250 ms,
//! measured from the end of the last one.
//!
//! Bounds:
//! - A write through this process: 100 ms, plus the walk.
//! - Whatever the hook can't see, and the other daemon's writes during a handoff:
//!   the sweep, every 30 s ([`sweep_every`]).
//! - A pass's expiry: the hub wakes at the earliest one.
//!
//! The hub is an accelerator for the sockets that also check on a tick and before
//! a frame (`socket_access`). With `K2_ACCESS_HUB=off` those checks are what's left:
//! slower, still safe. [`PassWatch::changed`] then never fires.
//!
//! `WS /cli/skin/me/events?scope=me` (`me.events`, S3) is the one socket that exists
//! only for this: it sends `{"kind":"access_changed"}`, no details, and the client
//! reads `me.get`. Closes and narrowing go out in the walk, whatever its size. Only
//! those frames are paced ([`pace`]), because each one causes a `me.get`. An App's
//! server can hold one socket for all its guests instead (`scope=guests`, S7): its
//! frames add that guest's `principalId`, and only ever name that App's own guests.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, Weak};
use std::thread::Thread;
use std::time::{Duration, Instant};

use k2_core::skin::SkinPass;
use parking_lot::Mutex;
use tokio::net::TcpStream;
use tokio::sync::{watch, Notify};
use tokio_tungstenite::tungstenite::Message;

use crate::apps::gateway::LinkCtx;
use crate::cli_response::CliResponse;
use crate::socket_access::RoomAccess;

/// The daemon environment switch; `off` (or `0`/`false`/`no`) = no hub, no
/// `me.events`, no `live-access-v1` feature key.
pub const SWITCH_ENV: &str = "K2_ACCESS_HUB";

/// Is the hub on (default yes)?
pub fn enabled() -> bool {
    !matches!(std::env::var(SWITCH_ENV).ok().map(|v| v.trim().to_ascii_lowercase()).as_deref(), Some("off" | "0" | "false" | "no"))
}

/// A burst of writes (several `with_conn` calls in a row) becomes one walk.
const DEBOUNCE: Duration = Duration::from_millis(100);
/// At least this long between walks, from the end of one to the start of the next.
const WALK_FLOOR: Duration = Duration::from_millis(250);

/// How often every open pass is compared whatever happened. A test sets it short.
fn sweep_every() -> Duration {
    let ms = std::env::var("K2_ACCESS_SWEEP_MS").ok().and_then(|v| v.parse::<u64>().ok()).filter(|m| *m > 0).unwrap_or(30_000);
    Duration::from_millis(ms)
}

/// A pass as the hub last read it.
#[derive(Debug, Clone)]
pub enum Answer {
    /// `generation`: read before the resolve, so a socket taking this answer
    /// doesn't resolve again before its next frame.
    Live { pass: Arc<SkinPass>, generation: u64 },
    Gone,
}

impl Answer {
    /// What a socket on `room` may do under this answer, with the generation to
    /// settle its frame gate at. No database read.
    pub(crate) fn room_access(&self, room: &str, caps: &[&str]) -> (RoomAccess, u64) {
        match self {
            Answer::Gone => (RoomAccess::PassGone, 0),
            Answer::Live { pass, generation } if pass.has_any_cap_in_room(room, caps) => (RoomAccess::Still((**pass).clone()), *generation),
            Answer::Live { generation, .. } => (RoomAccess::Lost, *generation),
        }
    }
}

struct Entry {
    link: Option<Arc<LinkCtx>>,
    raw: String,
    /// Where this pass lives in `tokens`, so a write there can name it.
    rowid: Option<i64>,
    /// Of the answer sockets last heard. [`UNREAD`]: not read yet, so the next
    /// walk only records.
    fingerprint: AtomicU64,
    /// 0: never.
    expires_at: AtomicI64,
    tx: watch::Sender<Answer>,
    holders: AtomicU64,
    /// This pass's open `me.events` sockets.
    me: Mutex<Vec<Weak<MeSlot>>>,
}

/// One `me.events` socket's place in the pacer's queue.
struct MeSlot {
    owed: Notify,
    /// In a queue already: a second change while it waits is the same frame.
    queued: AtomicBool,
}

/// `(link id or "", raw pass)`.
type Key = (String, String);

/// Nothing hashes to this but by a one-in-2^64 accident, which costs one frame.
const UNREAD: u64 = 0;

static ENTRIES: Mutex<Option<HashMap<Key, Arc<Entry>>>> = Mutex::new(None);
/// The core's listener: something moved.
static WAKE: AtomicBool = AtomicBool::new(false);
/// A new entry may have missed a walk: compare everything, whatever the core says.
static FORCE_FULL: AtomicBool = AtomicBool::new(false);
static THREAD: OnceLock<Thread> = OnceLock::new();

fn with_entries<T>(f: impl FnOnce(&mut HashMap<Key, Arc<Entry>>) -> T) -> T {
    let mut guard = ENTRIES.lock();
    f(guard.get_or_insert_with(HashMap::new))
}

/// Called with the core's locks free, but maybe under the main database lock. So:
/// an atomic and an unpark.
fn on_write() {
    WAKE.store(true, Ordering::SeqCst);
    if let Some(t) = THREAD.get() {
        t.unpark();
    }
}

/// Start the hub's thread once, on the first socket. A thread, not a task: a test
/// runtime ends with its test, and the walk blocks anyway.
fn start_once() {
    THREAD.get_or_init(|| {
        k2_core::skin::access_live::set_listener(on_write);
        std::thread::Builder::new().name("k2-access-hub".into()).spawn(run).expect("spawn the access hub").thread().clone()
    });
}

fn run() {
    let mut last_end = Instant::now();
    let mut last_sweep = Instant::now();
    loop {
        let sweep = sweep_every();
        let now = Instant::now();
        let next_sweep = last_sweep + sweep;
        let wait = next_sweep.saturating_duration_since(now).min(until_earliest_expiry());
        if !WAKE.load(Ordering::SeqCst) && !wait.is_zero() {
            std::thread::park_timeout(wait);
        }
        if WAKE.swap(false, Ordering::SeqCst) {
            std::thread::sleep(DEBOUNCE);
            let floor = last_end + WALK_FLOOR;
            let now = Instant::now();
            if now < floor {
                std::thread::sleep(floor - now);
            }
            let wake = k2_core::skin::access_live::take_wake();
            let force = FORCE_FULL.swap(false, Ordering::SeqCst);
            match wake {
                _ if force => walk_all(),
                k2_core::skin::access_live::Wake::Full => walk_all(),
                k2_core::skin::access_live::Wake::Tokens(rows) => {
                    walk(Which::Rows(rows.into_iter().collect()));
                    let mut owed = HashMap::new();
                    feeds_hear_ended_sessions(&mut owed);
                    pace(owed);
                }
                k2_core::skin::access_live::Wake::Nothing => {}
            }
            last_end = Instant::now();
        }
        let now = Instant::now();
        if now >= last_sweep + sweep {
            walk_all();
            last_end = Instant::now();
            last_sweep = last_end;
        } else if any_expired() {
            walk(Which::Expired);
            last_end = Instant::now();
        }
    }
}

fn any_expired() -> bool {
    let now = chrono::Utc::now().timestamp();
    with_entries(|m| m.values().any(|e| (1..now).contains(&e.expires_at.load(Ordering::SeqCst))))
}

/// How long until the first open pass has expired. At least a second, so an entry
/// that can't be read just now can't spin this.
fn until_earliest_expiry() -> Duration {
    let now = chrono::Utc::now().timestamp();
    let earliest = with_entries(|m| m.values().map(|e| e.expires_at.load(Ordering::SeqCst)).filter(|t| *t > 0).min());
    match earliest {
        Some(at) => Duration::from_secs(at.saturating_sub(now).max(0) as u64 + 1).min(Duration::from_secs(3600)),
        None => Duration::from_secs(3600),
    }
}

/// Every open pass, then every App feed.
fn walk_all() {
    walk(Which::All);
    // One pacing, so a guest switched off (their answer changed and their sessions
    // ended) is named once.
    let mut owed = HashMap::new();
    walk_feeds(&mut owed);
    feeds_hear_ended_sessions(&mut owed);
    pace(owed);
}

enum Which {
    All,
    Rows(HashSet<i64>),
    Expired,
}

/// Walks so far and how long they took, for the night run (`live_access_fanout`).
#[derive(Debug, Clone, Copy, Default)]
pub struct WalkStats {
    pub walks: u64,
    pub longest_ms: u64,
    pub last_ms: u64,
    /// How many passes the last walk compared.
    pub last_size: u64,
}

static STATS: Mutex<WalkStats> = Mutex::new(WalkStats { walks: 0, longest_ms: 0, last_ms: 0, last_size: 0 });

pub fn walk_stats() -> WalkStats {
    *STATS.lock()
}

/// Compare the entries `which` names, one resolve at a time. Sockets narrow or close
/// as the walk goes. The frames it owes are paced once it's done.
fn walk(which: Which) {
    let t0 = Instant::now();
    let now = chrono::Utc::now().timestamp();
    let entries: Vec<Arc<Entry>> = with_entries(|m| {
        m.values()
            .filter(|e| match &which {
                Which::All => true,
                Which::Rows(rows) => e.rowid.is_none_or(|r| rows.contains(&r)),
                Which::Expired => {
                    let at = e.expires_at.load(Ordering::SeqCst);
                    at > 0 && at < now
                }
            })
            .cloned()
            .collect()
    });
    if entries.is_empty() {
        return;
    }
    let world = match World::load() {
        Ok(w) => w,
        Err(e) => {
            // No answer about anyone: the next write or the sweep asks again.
            k2_core::log_debug!("[access_live] the rooms and projects couldn't be read: {e}");
            return;
        }
    };
    let size = entries.len() as u64;
    let mut owed: HashMap<DoorKey, Vec<Owed>> = HashMap::new();
    for e in entries {
        if compare(&e, &world) {
            let door = door_of(e.link.as_deref());
            let mut me = e.me.lock();
            me.retain(|w| w.strong_count() > 0);
            owed.entry(door).or_default().extend(me.iter().filter_map(Weak::upgrade).map(Owed::Me));
        }
    }
    pace(owed);
    let ms = t0.elapsed().as_millis() as u64;
    let mut stats = STATS.lock();
    stats.walks += 1;
    stats.last_ms = ms;
    stats.longest_ms = stats.longest_ms.max(ms);
    stats.last_size = size;
}

/// `true`: the answer changed, and the pass is still alive.
fn compare(e: &Entry, world: &World) -> bool {
    let generation = k2_core::skin::write_generation();
    match crate::socket_access::live_pass(e.link.as_deref(), &e.raw) {
        // The database couldn't say, which is no answer about the pass.
        Err(err) => {
            k2_core::log_debug!("[access_live] a pass couldn't be read: {err}");
            false
        }
        Ok(None) => {
            e.expires_at.store(0, Ordering::SeqCst);
            e.fingerprint.store(GONE, Ordering::SeqCst);
            e.tx.send_replace(Answer::Gone);
            false
        }
        Ok(Some(pass)) => {
            let fp = fingerprint(&pass, world);
            e.expires_at.store(pass.expires_at.unwrap_or(0), Ordering::SeqCst);
            let old = e.fingerprint.swap(fp, Ordering::SeqCst);
            if old == fp || old == UNREAD {
                return false;
            }
            e.tx.send_replace(Answer::Live { pass: Arc::new(pass), generation });
            true
        }
    }
}

/// See [`UNREAD`]: an answer can't hash to this either.
const GONE: u64 = 1;

/// What every pass's answer rests on outside skin.db. Read once per walk.
pub(crate) struct World {
    /// Rooms whose agent still exists: `me.get` lists no other.
    rooms: HashSet<String>,
    /// Each project and its member rooms: a guest reads a project only by holding
    /// `thread:read` in every one of them (`apps::project_chat`).
    projects: Vec<(String, Vec<String>)>,
}

impl World {
    pub(crate) fn load() -> Result<Self, String> {
        let rooms = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let mut st = conn.prepare("SELECT id FROM projects").map_err(|e| format!("projects: {e}"))?;
            let rows = st.query_map([], |r| r.get::<_, String>(0)).map_err(|e| format!("projects: {e}"))?;
            rows.collect::<Result<HashSet<_>, _>>().map_err(|e| format!("projects: {e}"))?
        };
        let projects = crate::apps::project_chat::all_projects()?;
        Ok(Self { rooms, projects })
    }
}

/// Everything about a pass that `me.get` and the room sockets answer from. Each
/// list is ordered first, so a rewrite in another order isn't a change.
pub(crate) fn fingerprint(pass: &SkinPass, world: &World) -> u64 {
    let mut h = DefaultHasher::new();
    let mut caps: Vec<&String> = pass.caps.iter().collect();
    caps.sort();
    caps.hash(&mut h);
    let mut rooms: Vec<&String> = pass.rooms.iter().collect();
    rooms.sort();
    rooms.hash(&mut h);
    for (room, caps) in &pass.room_policy {
        room.hash(&mut h);
        let mut caps: Vec<&String> = caps.iter().collect();
        caps.sort();
        caps.hash(&mut h);
    }
    // Debug of a BTreeMap of ordered lists: stable, and only compared to itself.
    format!("{:?}", pass.room_paths).hash(&mut h);
    pass.app.hash(&mut h);
    pass.chat.hash(&mut h);
    // The room a pass holds whose agent is gone.
    let held = if pass.session { pass.room_policy.keys().collect::<Vec<_>>() } else { rooms };
    for r in held {
        world.rooms.contains(r).hash(&mut h);
    }
    // The projects it may read.
    for (id, rooms) in &world.projects {
        if crate::apps::project_chat::holds_project(pass, rooms) {
            id.hash(&mut h);
        }
    }
    match h.finish() {
        UNREAD | GONE => 2,
        v => v,
    }
}

// ── pacing ───────────────────────────────────────────────────────────

/// Where a frame's `me.get` will land: a link has its own request budget.
#[derive(Clone, PartialEq, Eq, Hash)]
enum DoorKey {
    /// The main port and the App helper.
    Main,
    /// `(link id, App id)`.
    Link(String, String),
}

fn door_of(link: Option<&LinkCtx>) -> DoorKey {
    match link {
        Some(l) => DoorKey::Link(l.link_id.clone(), l.app_id.clone()),
        None => DoorKey::Main,
    }
}

/// One frame the pacer owes.
enum Owed {
    Me(Arc<MeSlot>),
    /// `scope=guests`: this socket, naming this guest.
    Guest(Arc<FeedSocket>, String),
}

/// A bucket for one door.
struct Door {
    queue: VecDeque<Owed>,
    tokens: f64,
    last: Instant,
}

/// How often the pacer spends a door's budget, give or take [`PACE_JITTER_MS`].
const PACE_TICK_MS: u64 = 100;
const PACE_JITTER_MS: u64 = 20;
/// Frames a second through the main port and the helper. Chosen so 5,000 open
/// sessions hear within 30 s; `S5` (`live_access_fanout`) measures it.
const MAIN_PACE: f64 = 200.0;

/// Frames a second through a door. On a link, half the App's request rate, because
/// each frame costs it a `me.get` and it has its own work to do.
fn rate_of(door: &DoorKey) -> f64 {
    match door {
        DoorKey::Main => std::env::var("K2_ACCESS_PACE").ok().and_then(|v| v.parse::<f64>().ok()).filter(|r| *r >= 1.0).unwrap_or(MAIN_PACE),
        DoorKey::Link(_, app) => (crate::apps::quotas::quotas_for(app).rps as f64 / 2.0).max(1.0),
    }
}

/// What one tick may spend: a change that fits goes out at once.
fn burst_of(rate: f64) -> f64 {
    (rate * PACE_TICK_MS as f64 / 1000.0).ceil().max(1.0)
}

static DOORS: Mutex<Option<HashMap<DoorKey, Door>>> = Mutex::new(None);
static PACER: OnceLock<Thread> = OnceLock::new();

/// A little randomness, for the order and the tick. Nothing depends on its quality.
fn rough_random(below: usize) -> usize {
    (uuid::Uuid::new_v4().as_u128() % below.max(1) as u128) as usize
}

/// Hand a walk's frames to the pacer, in a shuffled order so nobody's always first
/// and a whole box doesn't re-read in the order it signed in.
fn pace(owed: HashMap<DoorKey, Vec<Owed>>) {
    if owed.is_empty() {
        return;
    }
    {
        let mut guard = DOORS.lock();
        let doors = guard.get_or_insert_with(HashMap::new);
        for (key, mut items) in owed {
            for i in (1..items.len()).rev() {
                items.swap(i, rough_random(i + 1));
            }
            let burst = burst_of(rate_of(&key));
            let door = doors.entry(key).or_insert_with(|| Door { queue: VecDeque::new(), tokens: burst, last: Instant::now() });
            for item in items {
                let fresh = match &item {
                    Owed::Me(s) => !s.queued.swap(true, Ordering::SeqCst),
                    Owed::Guest(s, who) => s.queued.lock().insert(who.clone()),
                };
                if fresh {
                    door.queue.push_back(item);
                }
            }
        }
    }
    PACER
        .get_or_init(|| std::thread::Builder::new().name("k2-access-pacer".into()).spawn(run_pacer).expect("spawn the access pacer").thread().clone())
        .unpark();
}

fn run_pacer() {
    loop {
        if spend() {
            let wait = PACE_TICK_MS - PACE_JITTER_MS + rough_random(2 * PACE_JITTER_MS as usize + 1) as u64;
            std::thread::sleep(Duration::from_millis(wait));
        } else {
            std::thread::park();
        }
    }
}

/// Send what each door can afford now. `true`: frames are still waiting.
fn spend() -> bool {
    let mut guard = DOORS.lock();
    let doors = guard.get_or_insert_with(HashMap::new);
    let now = Instant::now();
    let mut waiting = false;
    for (key, door) in doors.iter_mut() {
        let rate = rate_of(key);
        door.tokens = (door.tokens + now.duration_since(door.last).as_secs_f64() * rate).min(burst_of(rate));
        door.last = now;
        while door.tokens >= 1.0 {
            let Some(item) = door.queue.pop_front() else { break };
            match item {
                Owed::Me(slot) => {
                    slot.queued.store(false, Ordering::SeqCst);
                    slot.owed.notify_one();
                }
                Owed::Guest(sock, who) => {
                    sock.queued.lock().remove(&who);
                    sock.ready.lock().push_back(who);
                    sock.owed.notify_one();
                }
            }
            door.tokens -= 1.0;
        }
        waiting |= !door.queue.is_empty();
    }
    waiting
}

// ── scope=guests: one socket per App ─────────────────────────────────

/// One App's feed: every guest's answer, by person, for the sockets its own pass
/// holds open. K2 doesn't hold those guests' passes, so these fingerprints come from
/// [`k2_core::skin::app_guests_now`]. Keyed by the App: nothing about anyone else's
/// guest can reach it, because nothing about anyone else's guest is read for it.
struct Feed {
    service_id: String,
    prints: Mutex<HashMap<String, u64>>,
    /// `(since when, sessions already named at that time)`: revoke times are whole
    /// seconds, so the next look starts at the same second and skips these.
    ended: Mutex<(i64, HashSet<String>)>,
    sockets: Mutex<Vec<Weak<FeedSocket>>>,
}

struct FeedSocket {
    door: DoorKey,
    owed: Notify,
    /// Named and waiting in the pacer's queue: a second change is the same frame.
    queued: Mutex<HashSet<String>>,
    /// Paid for by the pacer, ready to send.
    ready: Mutex<VecDeque<String>>,
}

static FEEDS: Mutex<Option<HashMap<String, Arc<Feed>>>> = Mutex::new(None);

fn with_feeds<T>(f: impl FnOnce(&mut HashMap<String, Arc<Feed>>) -> T) -> T {
    let mut guard = FEEDS.lock();
    f(guard.get_or_insert_with(HashMap::new))
}

fn guest_prints(service_id: &str, world: &World) -> Result<HashMap<String, u64>, String> {
    Ok(k2_core::skin::app_guests_now(service_id)?
        .into_iter()
        .filter_map(|p| {
            let print = fingerprint(&p, world);
            p.principal_id.map(|id| (id, print))
        })
        .collect())
}

/// Tell a feed's sockets about these guests, through the pacer.
fn owe_feed(feed: &Feed, who: Vec<String>, owed: &mut HashMap<DoorKey, Vec<Owed>>) {
    let mut sockets = feed.sockets.lock();
    sockets.retain(|w| w.strong_count() > 0);
    for sock in sockets.iter().filter_map(Weak::upgrade) {
        for id in &who {
            owed.entry(sock.door.clone()).or_default().push(Owed::Guest(sock.clone(), id.clone()));
        }
    }
}

/// Compare every feed's guests. A guest who changed, arrived or is no longer on the
/// App's list is named: each was the App's own guest a moment ago.
fn walk_feeds(owed: &mut HashMap<DoorKey, Vec<Owed>>) {
    drop_empty_feeds();
    let feeds: Vec<Arc<Feed>> = with_feeds(|m| m.values().cloned().collect());
    if feeds.is_empty() {
        return;
    }
    let world = match World::load() {
        Ok(w) => w,
        Err(e) => {
            k2_core::log_debug!("[access_live] the rooms and projects couldn't be read: {e}");
            return;
        }
    };
    for feed in feeds {
        let now = match guest_prints(&feed.service_id, &world) {
            Ok(p) => p,
            Err(e) => {
                k2_core::log_debug!("[access_live] an App's guests couldn't be read: {e}");
                continue;
            }
        };
        let mut prints = feed.prints.lock();
        let mut who: Vec<String> = now.iter().filter(|(id, p)| prints.get(*id) != Some(*p)).map(|(id, _)| id.clone()).collect();
        who.extend(prints.keys().filter(|id| !now.contains_key(*id)).cloned());
        *prints = now;
        drop(prints);
        owe_feed(&feed, who, owed);
    }
}

/// A session ending doesn't change a person's access on paper, but the App holding
/// that pass needs to hear: its next read says the pass is dead. Asked of the rows, by
/// time, not from what a write named: a revoke can land in the same walk as any other
/// write. A new session is not news: nobody's access moved.
fn feeds_hear_ended_sessions(owed: &mut HashMap<DoorKey, Vec<Owed>>) {
    let feeds: Vec<Arc<Feed>> = with_feeds(|m| m.values().cloned().collect());
    for feed in feeds {
        let now = chrono::Utc::now().timestamp();
        let since = feed.ended.lock().0;
        let rows = match k2_core::skin::access_live::sessions_ended_since(&feed.service_id, since) {
            Ok(rows) => rows,
            Err(e) => {
                k2_core::log_debug!("[access_live] an App's sessions couldn't be read: {e}");
                continue;
            }
        };
        let mut ended = feed.ended.lock();
        let mut who: Vec<String> = rows.iter().filter(|(id, _)| !ended.1.contains(id)).map(|(_, person)| person.clone()).collect();
        who.sort();
        who.dedup();
        *ended = (now, rows.into_iter().map(|(id, _)| id).collect());
        drop(ended);
        if !who.is_empty() {
            owe_feed(&feed, who, owed);
        }
    }
}

/// Open `service_id`'s feed for one more socket. Reads skin.db. The socket joins
/// under the map's lock, so a feed can't be dropped as a socket joins it.
fn join_feed(service_id: &str, door: DoorKey) -> Result<(Arc<Feed>, Arc<FeedSocket>), String> {
    let sock = Arc::new(FeedSocket { door, owed: Notify::new(), queued: Mutex::new(HashSet::new()), ready: Mutex::new(VecDeque::new()) });
    let join = |f: &Arc<Feed>| f.sockets.lock().push(Arc::downgrade(&sock));
    let found = with_feeds(|m| {
        m.get(service_id).map(|f| {
            join(f);
            f.clone()
        })
    });
    if let Some(feed) = found {
        return Ok((feed, sock));
    }
    let generation = k2_core::skin::write_generation();
    let prints = guest_prints(service_id, &World::load()?)?;
    // What ended before this feed opened isn't news either.
    let now = chrono::Utc::now().timestamp();
    let already = k2_core::skin::access_live::sessions_ended_since(service_id, now)?.into_iter().map(|(id, _)| id).collect();
    let fresh = Arc::new(Feed { service_id: service_id.to_string(), prints: Mutex::new(prints), ended: Mutex::new((now, already)), sockets: Mutex::new(Vec::new()) });
    let feed = with_feeds(|m| {
        let f = m.entry(service_id.to_string()).or_insert(fresh);
        join(f);
        f.clone()
    });
    if k2_core::skin::write_generation() != generation {
        FORCE_FULL.store(true, Ordering::SeqCst);
        on_write();
    }
    Ok((feed, sock))
}

/// Feeds whose sockets have all gone: nobody's reading those guests any more.
fn drop_empty_feeds() {
    with_feeds(|m| {
        m.retain(|_, f| {
            let mut sockets = f.sockets.lock();
            sockets.retain(|w| w.strong_count() > 0);
            !sockets.is_empty()
        })
    });
}

// ── me.events ────────────────────────────────────────────────────────

/// `me.events`' socket.
pub const ME_EVENTS_PATH: &str = "/cli/skin/me/events";
/// The whole frame, ever: read `me.get` again.
const ACCESS_CHANGED: &str = r#"{"kind":"access_changed"}"#;
/// On the main port nothing else bounds these (V20): each takes a task and a slot.
const MAX_ME_SOCKETS_PER_PASS: usize = 16;
/// The same bound for `scope=guests`, per App: its remote pass is on the main port,
/// where no link quota counts sockets. One is what an App needs.
const MAX_FEED_SOCKETS_PER_APP: usize = 16;

fn refusal(status: &'static str, code: &str, hint: &str) -> CliResponse {
    CliResponse { status, content_type: "application/json", body: serde_json::json!({ "ok": false, "error": { "code": code, "hint": hint } }).to_string() }
}

async fn refuse(stream: &mut TcpStream, r: CliResponse) {
    let resp = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n{}Connection: close\r\n\r\n{}",
        r.status,
        r.content_type,
        r.body.len(),
        k2_core::frame_policy::CONNECT_HEADER_LINES,
        r.body
    );
    crate::routes::http::refuse_upgrade(stream, &resp).await;
}

/// A credential that isn't an App pass: never an owner or login socket.
pub fn me_events_forbidden() -> CliResponse {
    refusal("403 Forbidden", "skin_token", "me.events is for a live app or guest pass (k2skn_)")
}

/// With the hub off this socket isn't there, like the feature key.
pub fn me_events_off() -> CliResponse {
    refusal("404 Not Found", "not_found", "this server has no me.events: read me.get on open, and on each request's refusal")
}

/// `WS /cli/skin/me/events?scope=me`, for a live pass, or `?scope=guests`, for an
/// App's own pass. The hub holds the pass **before** the upgrade, so a read after
/// the socket opens can't miss a change: a commit before that read is in the read,
/// and a commit after it moves the fingerprint taken here.
pub async fn serve_me_events(stream: &mut TcpStream, params: HashMap<String, String>, pass: SkinPass, raw: String) {
    match params.get("scope").map(String::as_str) {
        Some("me") => {}
        Some("guests") => {
            serve_guest_feed(stream, pass, raw).await;
            return;
        }
        _ => {
            refuse(stream, refusal("400 Bad Request", "usage", "name the scope: scope=me, or scope=guests with the App's own pass")).await;
            return;
        }
    }
    let link = crate::apps::gateway::current();
    let mut access = PassWatch::open(link, raw).await;
    let Some(entry) = access.entry.as_ref().map(|(_, e)| e.clone()) else {
        let r = if access.unreadable { CliResponse::internal_error("the pass couldn't be read; try again".to_string()) } else { crate::skin_routes::revoked_skin_response() };
        refuse(stream, r).await;
        return;
    };
    let slot = Arc::new(MeSlot { owed: Notify::new(), queued: AtomicBool::new(false) });
    let room_for_it = {
        let mut me = entry.me.lock();
        me.retain(|w| w.strong_count() > 0);
        let room = me.len() < MAX_ME_SOCKETS_PER_PASS;
        if room {
            me.push(Arc::downgrade(&slot));
        }
        room
    };
    if !room_for_it {
        refuse(stream, refusal("429 Too Many Requests", "rate_limited", "this pass holds too many me.events sockets; one per page is plenty")).await;
        return;
    }
    let ws = match tokio_tungstenite::accept_async(&mut *stream).await {
        Ok(ws) => ws,
        Err(e) => {
            k2_core::log_debug!("[access_live] handshake failed: {e}");
            return;
        }
    };
    use futures_util::{SinkExt, StreamExt};
    let (mut write, mut read) = ws.split();
    // Idle by design, so a dead peer is only noticed by a failed ping (V19).
    let mut keepalive = tokio::time::interval(crate::overlay_ws::ping_interval());
    keepalive.tick().await;
    loop {
        tokio::select! {
            biased;
            _ = keepalive.tick() => {
                if write.send(Message::Ping(Vec::new())).await.is_err() {
                    break;
                }
            }
            _ = access.changed() => {
                // Closes don't wait for the pacer. A live answer's frame is paced.
                if matches!(access.answer(), Answer::Gone) {
                    let _ = write.send(crate::presence::session_revoked_close_message()).await;
                    break;
                }
            }
            _ = slot.owed.notified() => {
                if write.send(Message::Text(ACCESS_CHANGED.to_string())).await.is_err() {
                    break;
                }
            }
            incoming = read.next() => match incoming {
                Some(Ok(Message::Ping(p))) => {
                    if write.send(Message::Pong(p)).await.is_err() {
                        break;
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            },
        }
    }
}

/// `scope=guests`: one socket for all of this App's guests. Only the App's own pass
/// opens it (AR50), and it only ever names that App's guests.
async fn serve_guest_feed(stream: &mut TcpStream, pass: SkinPass, raw: String) {
    let on_link = crate::guest_rooms_routes::OnLink::current();
    let Some(service_id) = crate::guest_rooms_routes::app_of(&pass, on_link.as_ref()) else {
        refuse(stream, refusal("403 Forbidden", "app_pass_required", "scope=guests is for this App's own pass; a guest's pass uses scope=me")).await;
        return;
    };
    let link = crate::apps::gateway::current();
    let door = door_of(link.as_deref());
    let mut access = PassWatch::open(link, raw).await;
    if access.entry.is_none() {
        let r = if access.unreadable { CliResponse::internal_error("the pass couldn't be read; try again".to_string()) } else { crate::skin_routes::revoked_skin_response() };
        refuse(stream, r).await;
        return;
    }
    let joined = {
        let (service, door) = (service_id.clone(), door.clone());
        tokio::task::spawn_blocking(move || join_feed(&service, door)).await
    };
    let (feed, sock) = match joined {
        Ok(Ok(j)) => j,
        Ok(Err(e)) => {
            refuse(stream, CliResponse::internal_error(e)).await;
            return;
        }
        Err(e) => {
            refuse(stream, CliResponse::internal_error(e.to_string())).await;
            return;
        }
    };
    let over = {
        let mut sockets = feed.sockets.lock();
        sockets.retain(|w| w.strong_count() > 0);
        sockets.len() > MAX_FEED_SOCKETS_PER_APP
    };
    if over {
        drop(sock);
        refuse(stream, refusal("429 Too Many Requests", "rate_limited", "this App holds too many scope=guests sockets; one is plenty")).await;
        return;
    }
    let ws = match tokio_tungstenite::accept_async(&mut *stream).await {
        Ok(ws) => ws,
        Err(e) => {
            k2_core::log_debug!("[access_live] handshake failed: {e}");
            drop(sock);
            drop_empty_feeds();
            return;
        }
    };
    use futures_util::{SinkExt, StreamExt};
    let (mut write, mut read) = ws.split();
    let mut keepalive = tokio::time::interval(crate::overlay_ws::ping_interval());
    keepalive.tick().await;
    'socket: loop {
        tokio::select! {
            biased;
            _ = keepalive.tick() => {
                if write.send(Message::Ping(Vec::new())).await.is_err() {
                    break;
                }
            }
            _ = access.changed() => {
                // The App's own pass: it rotates, or the App was suspended.
                if matches!(access.answer(), Answer::Gone) {
                    let _ = write.send(crate::presence::session_revoked_close_message()).await;
                    break;
                }
            }
            _ = sock.owed.notified() => {
                let ready: Vec<String> = sock.ready.lock().drain(..).collect();
                for who in ready {
                    let frame = serde_json::json!({ "kind": "access_changed", "principalId": who }).to_string();
                    if write.send(Message::Text(frame)).await.is_err() {
                        break 'socket;
                    }
                }
            }
            incoming = read.next() => match incoming {
                Some(Ok(Message::Ping(p))) => {
                    if write.send(Message::Pong(p)).await.is_err() {
                        break;
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            },
        }
    }
    drop(sock);
    drop_empty_feeds();
}

/// One open socket's view of its pass. Off when the hub is off, or when the pass
/// couldn't be read at the moment it opened: [`Self::changed`] then never fires,
/// and the socket's tick and frame check are what's left.
pub struct PassWatch {
    rx: Option<watch::Receiver<Answer>>,
    entry: Option<(Key, Arc<Entry>)>,
    /// The database couldn't say, which is no answer about the pass.
    unreadable: bool,
}

impl PassWatch {
    pub fn off() -> Self {
        Self { rx: None, entry: None, unreadable: false }
    }

    /// Hold `raw` open on `link`, and hear when its answer changes. Reads skin.db:
    /// a blocking thread ([`Self::open`]).
    pub(crate) fn open_blocking(link: Option<Arc<LinkCtx>>, raw: &str) -> Self {
        if !enabled() {
            return Self::off();
        }
        start_once();
        let key: Key = (link.as_ref().map(|l| l.link_id.clone()).unwrap_or_default(), raw.to_string());
        // Holders change under the map's lock, so an entry can't leave as a socket joins.
        let found = with_entries(|m| {
            m.get(&key).map(|e| {
                e.holders.fetch_add(1, Ordering::SeqCst);
                e.clone()
            })
        });
        if let Some(found) = found {
            return Self { rx: Some(found.tx.subscribe()), entry: Some((key, found)), unreadable: false };
        }
        // Before the read: a write that lands after it wakes a compare below.
        let generation = k2_core::skin::write_generation();
        let pass = match crate::socket_access::live_pass(link.as_deref(), raw) {
            Ok(Some(p)) => p,
            Ok(None) => return Self::off(),
            Err(e) => {
                k2_core::log_debug!("[access_live] a pass couldn't be read: {e}");
                return Self { rx: None, entry: None, unreadable: true };
            }
        };
        let fp = match World::load() {
            Ok(world) => fingerprint(&pass, &world),
            Err(e) => {
                k2_core::log_debug!("[access_live] the rooms and projects couldn't be read: {e}");
                UNREAD
            }
        };
        let rowid = k2_core::skin::access_live::token_rowid(raw).unwrap_or_else(|e| {
            k2_core::log_debug!("[access_live] token row: {e}");
            None
        });
        let (tx, _) = watch::channel(Answer::Live { pass: Arc::new(pass.clone()), generation });
        let fresh = Arc::new(Entry {
            link,
            raw: raw.to_string(),
            rowid,
            fingerprint: AtomicU64::new(fp),
            expires_at: AtomicI64::new(pass.expires_at.unwrap_or(0)),
            tx,
            holders: AtomicU64::new(0),
            me: Mutex::new(Vec::new()),
        });
        let held = with_entries(|m| {
            let e = m.entry(key.clone()).or_insert(fresh);
            e.holders.fetch_add(1, Ordering::SeqCst);
            e.clone()
        });
        // The walk that followed a write in between found nothing to compare.
        if k2_core::skin::write_generation() != generation {
            FORCE_FULL.store(true, Ordering::SeqCst);
            on_write();
        }
        if let Some(t) = THREAD.get() {
            // It may need to wake at this pass's expiry.
            t.unpark();
        }
        Self { rx: Some(held.tx.subscribe()), entry: Some((key, held)), unreadable: false }
    }

    /// [`Self::open_blocking`] off the async worker.
    pub(crate) async fn open(link: Option<Arc<LinkCtx>>, raw: String) -> Self {
        tokio::task::spawn_blocking(move || Self::open_blocking(link, &raw)).await.unwrap_or_else(|e| {
            k2_core::log_debug!("[access_live] a watch failed to open: {e}");
            Self::off()
        })
    }

    /// Resolves when this pass's answer has changed. Never, on a watch that's off.
    pub async fn changed(&mut self) {
        match self.rx.as_mut() {
            Some(rx) => {
                if rx.changed().await.is_err() {
                    std::future::pending::<()>().await;
                }
            }
            None => std::future::pending::<()>().await,
        }
    }

    /// The latest answer. Only after [`Self::changed`] resolved.
    pub fn answer(&self) -> Answer {
        match self.rx.as_ref() {
            Some(rx) => rx.borrow().clone(),
            // `changed` never resolves on a watch that's off.
            None => Answer::Gone,
        }
    }
}

impl Drop for PassWatch {
    fn drop(&mut self) {
        let Some((key, entry)) = self.entry.take() else { return };
        with_entries(|m| {
            if entry.holders.fetch_sub(1, Ordering::SeqCst) == 1 {
                m.remove(&key);
            }
        });
    }
}

/// How many distinct passes the hub holds open.
pub fn open_passes() -> usize {
    with_entries(|m| m.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pass(rooms: &[(&str, &[&str])]) -> SkinPass {
        let room_policy: k2_core::skin::RoomPolicy = rooms.iter().map(|(r, c)| (r.to_string(), c.iter().map(|c| c.to_string()).collect())).collect();
        SkinPass {
            id: "t".into(),
            principal_id: Some("p".into()),
            username: "dana".into(),
            caps: room_policy.values().flatten().cloned().collect(),
            rooms: room_policy.keys().cloned().collect(),
            session: true,
            room_policy,
            app: "host".into(),
            room_paths: Default::default(),
            expires_at: None,
            chat: false,
        }
    }

    fn world(rooms: &[&str], projects: &[(&str, &[&str])]) -> World {
        World {
            rooms: rooms.iter().map(|r| r.to_string()).collect(),
            projects: projects.iter().map(|(id, m)| (id.to_string(), m.iter().map(|r| r.to_string()).collect())).collect(),
        }
    }

    #[test]
    fn the_fingerprint_moves_when_the_answer_does_and_only_then() {
        let w = world(&["a", "b"], &[]);
        let base = pass(&[("a", &["thread:read", "files:read"])]);
        let same = fingerprint(&base, &w);
        assert_eq!(fingerprint(&base.clone(), &w), same, "stable");

        let mut reordered = base.clone();
        reordered.room_policy.insert("a".into(), vec!["files:read".into(), "thread:read".into()]);
        reordered.caps.reverse();
        assert_eq!(fingerprint(&reordered, &w), same, "the same caps in another order are no change");

        let mut other_expiry = base.clone();
        other_expiry.expires_at = Some(123);
        assert_eq!(fingerprint(&other_expiry, &w), same, "a refreshed session is no change");

        let mut renamed = base.clone();
        renamed.username = "dee".into();
        assert_eq!(fingerprint(&renamed, &w), same, "a rename isn't access");

        assert_ne!(fingerprint(&pass(&[("a", &["thread:read"])]), &w), same, "a cap");
        assert_ne!(fingerprint(&pass(&[("a", &["thread:read", "files:read"]), ("b", &["thread:read"])]), &w), same, "a room");
        let mut chat = base.clone();
        chat.chat = true;
        assert_ne!(fingerprint(&chat, &w), same, "chat");
        let mut folders = base.clone();
        let mut lines = k2_core::skin::RoomPaths::new();
        lines.insert("a".into(), k2_core::skin::RoomPathScope { read: vec!["docs".into()], ..Default::default() });
        folders.room_paths = k2_core::skin::paths::PassPaths::Lines(lines);
        assert_ne!(fingerprint(&folders, &w), same, "folders");
        let mut app = base.clone();
        app.app = "svc".into();
        assert_ne!(fingerprint(&app, &w), same, "the App");
    }

    #[test]
    fn a_small_change_goes_at_once_and_a_big_one_is_paced() {
        assert_eq!(burst_of(200.0), 20.0, "one tick's budget on the main port");
        assert_eq!(burst_of(25.0), 3.0, "a link at the default 50 rps");
        assert_eq!(burst_of(0.5), 1.0, "never nothing");
    }

    #[test]
    fn a_platform_pass_has_a_fingerprint_too() {
        // V12: a platform pass's room_policy is empty and its rooms are its own.
        let w = world(&["a", "b"], &[]);
        let mut p = pass(&[]);
        p.session = false;
        p.principal_id = None;
        p.app = String::new();
        p.caps = vec!["thread:read".into()];
        p.rooms = vec!["a".into()];
        let same = fingerprint(&p, &w);
        let mut moved = p.clone();
        moved.rooms = vec!["a".into(), "b".into()];
        assert_ne!(fingerprint(&moved, &w), same, "set_token_rooms moves it");
        let mut caps = p.clone();
        caps.caps = vec!["thread:read".into(), "files:read".into()];
        assert_ne!(fingerprint(&caps, &w), same);
    }

    #[test]
    fn what_outside_skin_db_decides_moves_it_too() {
        let p = pass(&[("a", &["thread:read"]), ("b", &["thread:read"])]);
        let before = world(&["a", "b"], &[("proj", &["a"])]);
        let same = fingerprint(&p, &before);
        assert_ne!(fingerprint(&p, &world(&["a"], &[("proj", &["a"])])), same, "a room's agent is gone");
        assert_ne!(fingerprint(&p, &world(&["a", "b"], &[("proj", &["a", "z"])])), same, "a project grew past this person");
        assert_ne!(fingerprint(&p, &world(&["a", "b"], &[])), same, "a project went");
        assert_eq!(fingerprint(&p, &world(&["a", "b", "other"], &[("proj", &["a"]), ("theirs", &["other"])])), same, "another room or another person's project is no change");
    }
}
