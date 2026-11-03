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

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::Thread;
use std::time::{Duration, Instant};

use k2_core::skin::SkinPass;
use parking_lot::Mutex;
use tokio::sync::watch;

use crate::apps::gateway::LinkCtx;
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
                k2_core::skin::access_live::Wake::Tokens(rows) => walk(Which::Rows(rows.into_iter().collect())),
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

/// Every open pass.
fn walk_all() {
    walk(Which::All);
}

enum Which {
    All,
    Rows(HashSet<i64>),
    Expired,
}

/// Compare the entries `which` names, one resolve at a time.
fn walk(which: Which) {
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
    for e in entries {
        compare(&e, &world);
    }
}

fn compare(e: &Entry, world: &World) {
    let generation = k2_core::skin::write_generation();
    match crate::socket_access::live_pass(e.link.as_deref(), &e.raw) {
        // The database couldn't say, which is no answer about the pass.
        Err(err) => k2_core::log_debug!("[access_live] a pass couldn't be read: {err}"),
        Ok(None) => {
            e.expires_at.store(0, Ordering::SeqCst);
            e.fingerprint.store(GONE, Ordering::SeqCst);
            e.tx.send_replace(Answer::Gone);
        }
        Ok(Some(pass)) => {
            let fp = fingerprint(&pass, world);
            e.expires_at.store(pass.expires_at.unwrap_or(0), Ordering::SeqCst);
            let old = e.fingerprint.swap(fp, Ordering::SeqCst);
            if old != fp && old != UNREAD {
                e.tx.send_replace(Answer::Live { pass: Arc::new(pass), generation });
            }
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

/// One open socket's view of its pass. Off when the hub is off, or when the pass
/// couldn't be read at the moment it opened: [`Self::changed`] then never fires,
/// and the socket's tick and frame check are what's left.
pub struct PassWatch {
    rx: Option<watch::Receiver<Answer>>,
    entry: Option<(Key, Arc<Entry>)>,
}

impl PassWatch {
    pub fn off() -> Self {
        Self { rx: None, entry: None }
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
            return Self { rx: Some(found.tx.subscribe()), entry: Some((key, found)) };
        }
        // Before the read: a write that lands after it wakes a compare below.
        let generation = k2_core::skin::write_generation();
        let pass = match crate::socket_access::live_pass(link.as_deref(), raw) {
            Ok(Some(p)) => p,
            Ok(None) => return Self::off(),
            Err(e) => {
                k2_core::log_debug!("[access_live] a pass couldn't be read: {e}");
                return Self::off();
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
        Self { rx: Some(held.tx.subscribe()), entry: Some((key, held)) }
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
