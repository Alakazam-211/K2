//! The chat delivery hub (S31 K22 core, K23 injector): Sew's agent
//! deliveries typed into the agent's terminal.
//!
//! **One poller per agent workspace with chat access** (any
//! `sew_chat_access` row). The supervisor ([`spawn`], started after
//! `set_ready()`) looks again every 30 s and on every kick
//! (`k2_core::sew_chat::hub::kick`: a S27 chat grant change, an access
//! grant or revoke). No Sew backend (Sew off, or a Mac) = no pollers.
//!
//! **Each step** ([`Poller::step`]): one long-poll of
//! `GET /mgmt/v1/agents/{member}/deliveries` → the deliveries a `wait`
//! (no address) is blocked on go to it → the rest are held until their
//! due time (prefs: batch window, quiet hours; a follow's own `--batch`)
//! → **the cursor is saved, in one statement, BEFORE anything is
//! injected** → what is due is injected in one go. A crash after the save
//! loses at most the held deliveries, never repeats one; the next start
//! says so in one status line ([`Notice::Restart`]). A Sew reset (a
//! restore's new `sync_epoch`, a 410 `behind_floor` / `ahead_of_head`)
//! jumps to the head and says so in one status line, never re-injecting.
//! No chat text is written to disk: held deliveries live in memory only.
//!
//! **The injector** ([`LiveInjector`]) is `k2 msg`'s throat,
//! `deliver_live_with_via(.., via = "chat")`, behind [`INJECT_SLOTS`]
//! box-wide slots. `via = chat` records no Chatter line (that would put
//! chat text on disk). Outcomes: delivered; a passing failure (busy,
//! stalled, a gate) is retried [`INJECT_TRIES`] times; an agent that is
//! asleep (only non-waking deliveries) or can't take messages counts them
//! as missed, and the next inject leads with one status line.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};

use k2_core::sew_chat::hub::{self as core, Cursor, Delivery, Notice, Step};

use super::sew::{ActAs, SewChat};

/// The `via` the hub stamps (and the Chatter skip keys on).
pub const VIA: &str = "chat";
/// The `[from …]` name on an injected delivery.
pub const FROM: &str = "sew-chat";
/// Concurrent injects box-wide (a wake can take seconds; a burst of chat
/// must not spawn every agent at once).
pub const INJECT_SLOTS: usize = 2;
/// How long a waking inject waits for the agent to be ready.
const WAKE_TIMEOUT: Duration = Duration::from_secs(45);
/// The supervisor looks again at least this often.
const SUPERVISE_EVERY: Duration = Duration::from_secs(30);

fn now() -> i64 {
    k2_core::sew_chat::now_secs()
}

// ── the injector (K23) ───────────────────────────────────────────────

/// What one inject did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub delivered: bool,
    /// `deliver_live`'s reason on a failure.
    pub reason: Option<String>,
}

/// Types text into an agent's terminal.
pub trait Injector: Send + Sync {
    fn inject(&self, project_path: &str, text: &str, wake: bool) -> Outcome;
}

static SLOTS: Mutex<usize> = Mutex::new(0);
static SLOT_FREE: Condvar = Condvar::new();

/// One of [`INJECT_SLOTS`]; waits for a free one.
pub struct InjectSlot(());

impl InjectSlot {
    pub fn take() -> Self {
        let mut g = SLOTS.lock().unwrap_or_else(|p| p.into_inner());
        while *g >= INJECT_SLOTS {
            g = SLOT_FREE.wait(g).unwrap_or_else(|p| p.into_inner());
        }
        *g += 1;
        InjectSlot(())
    }

    /// Slots in use now.
    pub fn in_use() -> usize {
        *SLOTS.lock().unwrap_or_else(|p| p.into_inner())
    }
}

impl Drop for InjectSlot {
    fn drop(&mut self) {
        let mut g = SLOTS.lock().unwrap_or_else(|p| p.into_inner());
        *g = g.saturating_sub(1);
        SLOT_FREE.notify_one();
    }
}

/// The real injector: `k2 msg`'s throat with `via = chat`.
pub struct LiveInjector;

impl Injector for LiveInjector {
    fn inject(&self, project_path: &str, text: &str, wake: bool) -> Outcome {
        let _slot = InjectSlot::take();
        let r = crate::workspace_msg::deliver_live_with_via(project_path, text, FROM, "", wake, WAKE_TIMEOUT, VIA);
        Outcome { delivered: r.success, reason: r.reason }
    }
}

/// How a failed inject is handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// Busy, stalled, a held gate, a spawn hiccup: try again shortly.
    Retry,
    /// Asleep and nothing asked to wake it.
    Asleep,
    /// No agent, unknown workspace, an untrusted folder: count as missed.
    Gone,
}

pub fn failure(reason: Option<&str>) -> Failure {
    match reason {
        Some("dormant_no_wake") => Failure::Asleep,
        Some("workspace_not_found" | "no_agent_mode" | "folder_trust_pending" | "folder_trust_denied") => Failure::Gone,
        _ => Failure::Retry,
    }
}

// ── status (what admins and `prefs` see) ─────────────────────────────

/// A poller's live state. Counts and codes only; no chat text.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub running: bool,
    pub member: Option<String>,
    pub after: u64,
    pub held: usize,
    pub delivered: u64,
    pub missed: u64,
    pub notices: u64,
    pub last_outcome: Option<String>,
    pub last_at: Option<i64>,
    pub last_error: Option<String>,
    pub started_at: Option<i64>,
}

static STATUS: Mutex<Option<HashMap<String, Status>>> = Mutex::new(None);

fn with_status<T>(pid: &str, f: impl FnOnce(&mut Status) -> T) -> T {
    let mut g = STATUS.lock().unwrap_or_else(|p| p.into_inner());
    f(g.get_or_insert_with(HashMap::new).entry(pid.to_string()).or_default())
}

pub fn status_of(pid: &str) -> Option<Status> {
    STATUS.lock().unwrap_or_else(|p| p.into_inner()).as_ref().and_then(|m| m.get(pid).cloned())
}

pub fn is_running(pid: &str) -> bool {
    status_of(pid).is_some_and(|s| s.running)
}

// ── waiters: `k2 chat wait` without an address ───────────────────────

struct Waiter {
    id: u64,
    mention: bool,
    tx: mpsc::Sender<Vec<Delivery>>,
}

static WAITERS: Mutex<Option<HashMap<String, Vec<Waiter>>>> = Mutex::new(None);
static WAITER_ID: AtomicU64 = AtomicU64::new(1);

/// A registered `wait`; unregisters on drop.
pub struct WaitTicket {
    pid: String,
    id: u64,
    pub rx: mpsc::Receiver<Vec<Delivery>>,
}

impl Drop for WaitTicket {
    fn drop(&mut self) {
        let mut g = WAITERS.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(v) = g.as_mut().and_then(|m| m.get_mut(&self.pid)) {
            v.retain(|w| w.id != self.id);
        }
    }
}

/// Wait for the agent's next delivery (`mention`: only mentions and DMs).
/// What a waiter takes is not injected: the agent asked for it.
pub fn register_waiter(pid: &str, mention: bool) -> WaitTicket {
    let (tx, rx) = mpsc::channel();
    let id = WAITER_ID.fetch_add(1, Ordering::SeqCst);
    let mut g = WAITERS.lock().unwrap_or_else(|p| p.into_inner());
    g.get_or_insert_with(HashMap::new).entry(pid.to_string()).or_default().push(Waiter { id, mention, tx });
    WaitTicket { pid: pid.to_string(), id, rx }
}

/// Hand new deliveries to `pid`'s waiters (each takes what matches, then
/// is done). Returns the ones no waiter took.
fn offer_to_waiters(pid: &str, entries: Vec<Delivery>) -> Vec<Delivery> {
    let mut g = WAITERS.lock().unwrap_or_else(|p| p.into_inner());
    let Some(ws) = g.as_mut().and_then(|m| m.get_mut(pid)) else {
        return entries;
    };
    let mut taken: HashSet<u64> = HashSet::new();
    ws.retain(|w| {
        let hits: Vec<Delivery> = entries.iter().filter(|d| !w.mention || d.is_addressed()).cloned().collect();
        if hits.is_empty() {
            return true;
        }
        if w.tx.send(hits.clone()).is_ok() {
            taken.extend(hits.iter().map(|d| d.seq));
        }
        false
    });
    entries.into_iter().filter(|d| !taken.contains(&d.seq)).collect()
}

/// A delivery as `wait` prints it (the CLI wraps `body` in UNTRUSTED
/// markers, like `messages`).
pub fn delivery_json(d: &Delivery) -> Value {
    let mut v = json!({
        "id": format!("msg_{}", d.message_id.trim_start_matches("msg_")),
        "addr": d.addr,
        "author": {"id": d.author_id, "username": d.author_username, "name": d.author_display,
                   "kind": if d.author_is_agent { "agent" } else { "person" }},
        "body": d.body,
        "bodyTruncated": d.body_truncated,
        "reason": d.reason,
        "place": d.place().as_str(),
        "createdAt": d.created_at,
        "replyTo": d.thread_root_id.as_deref().map(|r| format!("msg_{r}")),
        "canReply": d.place() != core::Place::PersonDm,
    });
    if let Some(o) = &d.on_behalf_of {
        v["onBehalfOf"] = o.clone().into();
    }
    if let Some(n) = &d.on_behalf_of_display_name {
        v["onBehalfOfName"] = n.clone().into();
    }
    if let Some(k) = &d.message_kind {
        v["kind"] = k.clone().into();
    }
    if let Some(w) = d.wake {
        v["wake"] = w.into();
    }
    if let Some(r) = &d.response {
        v["response"] = r.clone();
    }
    v
}

// ── the poller (K22) ─────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct Held {
    d: Delivery,
    due: i64,
    tries: u32,
}

/// One agent workspace's delivery loop.
pub struct Poller {
    pub pid: String,
    path: String,
    pub(crate) sew: Arc<dyn SewChat>,
    inj: Arc<dyn Injector>,
    act: ActAs,
    cursor: Option<Cursor>,
    held: Vec<Held>,
    /// Status lines waiting to lead the next inject.
    notices: Vec<String>,
    /// The notices were tried alone once already (an asleep agent isn't
    /// woken for a status line; they ride the next delivery).
    notices_tried: bool,
    /// Deliveries missed since the last notice about them.
    missed: usize,
}

fn db_err(e: String) -> String {
    format!("chat hub store: {e}")
}

impl Poller {
    /// A poller for `pid` (its folder `path`). A saved cursor with held
    /// deliveries means the daemon stopped while holding them: one
    /// status line says so.
    pub fn new(pid: &str, path: &str, sew: Arc<dyn SewChat>, inj: Arc<dyn Injector>) -> Result<Self, String> {
        let cursor = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            core::load_cursor(&conn, pid).map_err(db_err)?
        };
        let mut p = Poller {
            pid: pid.to_string(),
            path: path.to_string(),
            sew,
            inj,
            act: ActAs {
                workspace_uuid: pid.to_ascii_lowercase(),
                k2_session: core::HUB_SESSION.into(),
                on_behalf_of: None,
                actor: None,
            },
            cursor,
            held: Vec::new(),
            notices: Vec::new(),
            notices_tried: false,
            missed: 0,
        };
        if let Some(c) = p.cursor.as_mut() {
            if c.held > 0 {
                p.notices.push(core::status_line(Notice::Restart(c.held)));
                c.held = 0;
            }
        }
        with_status(pid, |s| {
            s.running = true;
            s.started_at = Some(now());
            s.member = p.cursor.as_ref().map(|c| c.member.clone());
            s.after = p.cursor.as_ref().map(|c| c.after).unwrap_or(0);
        });
        Ok(p)
    }

    fn save(&mut self, at: i64) -> Result<(), String> {
        let Some(c) = self.cursor.as_mut() else { return Ok(()) };
        c.held = self.held.len();
        let db = k2_core::db::shared();
        let conn = db.lock();
        core::save_cursor(&conn, &self.pid, c, at).map_err(db_err)?;
        let (after, held, member) = (c.after, c.held, c.member.clone());
        with_status(&self.pid, |s| {
            s.after = after;
            s.held = held;
            s.member = Some(member);
        });
        Ok(())
    }

    /// When the next held delivery is due.
    pub fn next_due(&self) -> Option<i64> {
        self.held.iter().map(|h| h.due).min()
    }

    pub fn held_count(&self) -> usize {
        self.held.len()
    }

    fn notice(&mut self, n: Notice) {
        self.notices.push(core::status_line(n));
        self.notices_tried = false;
        with_status(&self.pid, |s| s.notices += 1);
    }

    /// First start (no cursor): begin at the feed's head. Old messages are
    /// never injected into an agent.
    fn start_at_head(&mut self, at: i64) -> Result<(), String> {
        let me = self.sew.me(&self.act).map_err(|e| format!("sew me: {} {}", e.status, e.code))?;
        let (head, epoch) = match self.sew.deliveries(&self.act, &me.member_id, 0, 0).map_err(|e| format!("sew deliveries: {} {}", e.status, e.code))? {
            Step::Page { head, sync_epoch, .. } => (head, sync_epoch),
            Step::Resync { head, sync_epoch, .. } => (head, sync_epoch),
        };
        self.cursor = Some(Cursor { member: me.member_id, after: head, epoch, held: 0 });
        self.save(at)
    }

    /// One long-poll (at most `max_wait_ms`, less when something held
    /// comes due sooner), then inject what is due.
    pub fn step(&mut self, at: i64, max_wait_ms: u64) -> Result<(), String> {
        if self.cursor.is_none() {
            self.start_at_head(at)?;
            return self.flush(at);
        }
        let wait_ms = match self.next_due() {
            Some(d) if d <= at => 0,
            Some(d) => (((d - at) as u64) * 1000).min(max_wait_ms),
            None => max_wait_ms,
        };
        let (member, after) = {
            let c = self.cursor.as_ref().expect("checked above");
            (c.member.clone(), c.after)
        };
        let step = self
            .sew
            .deliveries(&self.act, &member, after, wait_ms)
            .map_err(|e| format!("sew deliveries: {} {}", e.status, e.code))?;
        let stored_epoch = self.cursor.as_ref().and_then(|c| c.epoch.clone());
        match step {
            Step::Page { entries, next_after, head, sync_epoch, .. } => {
                let epoch_moved = matches!((&stored_epoch, &sync_epoch), (Some(a), Some(b)) if a != b);
                let c = self.cursor.as_mut().expect("checked above");
                if sync_epoch.is_some() {
                    c.epoch = sync_epoch;
                }
                if epoch_moved {
                    // A restore: the page belongs to another history.
                    c.after = head;
                    self.notice(Notice::Reset("epoch_changed"));
                } else {
                    c.after = c.after.max(next_after);
                    let left = offer_to_waiters(&self.pid, entries);
                    if !left.is_empty() {
                        let (prefs, follows) = {
                            let db = k2_core::db::shared();
                            let conn = db.lock();
                            (
                                core::load_prefs(&conn, &self.pid).map_err(db_err)?,
                                core::list_follows(&conn, &self.pid).map_err(db_err)?,
                            )
                        };
                        for d in left {
                            let batch = follows.iter().find(|f| f.channel_id == d.channel_id).and_then(|f| f.batch_secs);
                            let due = core::due_at(&prefs, batch, at);
                            self.held.push(Held { d, due, tries: 0 });
                        }
                    }
                }
            }
            Step::Resync { reason, head, sync_epoch } => {
                let c = self.cursor.as_mut().expect("checked above");
                c.after = head;
                if sync_epoch.is_some() {
                    c.epoch = sync_epoch;
                }
                self.notice(Notice::Reset(core::reset_reason(&reason)));
            }
        }
        // The cursor is on disk before anything reaches the terminal.
        self.save(at)?;
        self.flush(at)
    }

    /// Inject what is due (and any status lines).
    pub fn flush(&mut self, at: i64) -> Result<(), String> {
        let (due, later): (Vec<Held>, Vec<Held>) = self.held.drain(..).partition(|h| h.due <= at);
        self.held = later;
        if due.is_empty() {
            let Some(lead) = self.lead().filter(|_| !self.notices_tried) else {
                return Ok(());
            };
            // A status line alone never wakes a sleeping agent.
            let out = self.inj.inject(&self.path, &lead, false);
            self.record(&out, 0);
            if out.delivered {
                self.notices.clear();
                self.missed = 0;
            } else {
                self.notices_tried = true;
            }
            return self.save(at);
        }
        let items: Vec<Delivery> = due.iter().map(|h| h.d.clone()).collect();
        let wake = items.iter().any(Delivery::wakes);
        let lead = self.lead();
        let text = core::render(lead.as_deref(), &items);
        let out = self.inj.inject(&self.path, &text, wake);
        self.record(&out, items.len());
        if out.delivered {
            self.notices.clear();
            self.notices_tried = false;
            self.missed = 0;
        } else {
            let mut dropped = 0;
            match failure(out.reason.as_deref()) {
                Failure::Retry => {
                    for mut h in due {
                        h.tries += 1;
                        if h.tries < core::INJECT_TRIES {
                            h.due = at + core::RETRY_SECS;
                            self.held.push(h);
                        } else {
                            dropped += 1;
                        }
                    }
                }
                Failure::Asleep | Failure::Gone => dropped = due.len(),
            }
            if dropped > 0 {
                // One line about everything missed so far rides the next
                // delivery (an asleep agent isn't woken for it alone).
                self.missed += dropped;
                self.notices_tried = true;
                with_status(&self.pid, |s| s.missed += dropped as u64);
            }
        }
        self.save(at)
    }

    /// The status lines that lead the next inject: restart/reset notices,
    /// then one line for everything missed.
    fn lead(&self) -> Option<String> {
        let mut v = self.notices.clone();
        if self.missed > 0 {
            v.push(core::status_line(Notice::Missed(self.missed)));
        }
        (!v.is_empty()).then(|| v.join("\n"))
    }

    fn record(&self, out: &Outcome, n: usize) {
        with_status(&self.pid, |s| {
            if out.delivered {
                s.delivered += n as u64;
                s.last_outcome = Some("delivered".into());
            } else {
                s.last_outcome = Some(out.reason.clone().unwrap_or_else(|| "failed".into()));
            }
            s.last_at = Some(now());
        });
    }
}

// ── the supervisor ───────────────────────────────────────────────────

struct Running {
    stop: Arc<AtomicBool>,
    handle: std::thread::JoinHandle<()>,
}

static STARTED: AtomicBool = AtomicBool::new(false);

/// Start the supervisor (once per process). Call after `set_ready()`.
pub fn spawn() -> Option<std::thread::JoinHandle<()>> {
    if STARTED.swap(true, Ordering::SeqCst) {
        return None;
    }
    match std::thread::Builder::new().name("sew-hub".into()).spawn(supervise) {
        Ok(h) => Some(h),
        Err(e) => {
            k2_core::log_debug!("[sew-hub] supervisor didn't start: {e}");
            None
        }
    }
}

fn supervise() {
    k2_core::log_debug!("[sew-hub] supervisor started");
    let mut running: HashMap<String, Running> = HashMap::new();
    let mut seen = core::kick_count();
    loop {
        reconcile(&mut running);
        seen = core::wait_kick(seen, SUPERVISE_EVERY);
    }
}

/// The workspaces that should have a poller now, with their folders.
/// Empty while Sew chat is off.
fn wanted() -> Vec<(String, String)> {
    if super::sew::backend().is_none() {
        return Vec::new();
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    let ids = match core::hub_workspaces(&conn) {
        Ok(v) => v,
        Err(e) => {
            k2_core::log_debug!("[sew-hub] can't list chat workspaces: {e}");
            return Vec::new();
        }
    };
    ids.into_iter()
        .filter_map(|id| {
            conn.query_row("SELECT path FROM projects WHERE id = ?1", rusqlite::params![id], |r| r.get::<_, String>(0))
                .ok()
                .map(|p| (id, p))
        })
        .collect()
}

/// Which pollers to start and stop, given what runs and what is wanted.
pub fn plan(running: &HashSet<String>, wanted: &HashSet<String>) -> (Vec<String>, Vec<String>) {
    let mut start: Vec<String> = wanted.difference(running).cloned().collect();
    let mut stop: Vec<String> = running.difference(wanted).cloned().collect();
    start.sort();
    stop.sort();
    (start, stop)
}

fn reconcile(running: &mut HashMap<String, Running>) {
    running.retain(|pid, r| {
        let alive = !r.handle.is_finished();
        if !alive {
            k2_core::log_debug!("[sew-hub] poller for {pid} ended");
        }
        alive
    });
    let sew_on = super::sew::backend().is_some();
    let want: HashMap<String, String> = wanted().into_iter().collect();
    let (start, stop) = plan(&running.keys().cloned().collect(), &want.keys().cloned().collect());
    for pid in stop {
        if let Some(r) = running.remove(&pid) {
            r.stop.store(true, Ordering::SeqCst);
        }
        if sew_on {
            // Access gone: a later grant starts at the head again, never
            // replaying what came in meanwhile.
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = core::clear_cursor(&conn, &pid);
        }
    }
    for pid in start {
        let path = want[&pid].clone();
        let stop = Arc::new(AtomicBool::new(false));
        let s2 = stop.clone();
        let p2 = pid.clone();
        match std::thread::Builder::new().name("sew-hub-poll".into()).spawn(move || run(&p2, &path, &s2)) {
            Ok(handle) => {
                running.insert(pid, Running { stop, handle });
            }
            Err(e) => k2_core::log_debug!("[sew-hub] poller for {pid} didn't start: {e}"),
        }
    }
}

fn run(pid: &str, path: &str, stop: &AtomicBool) {
    let inj: Arc<dyn Injector> = Arc::new(LiveInjector);
    let mut poller: Option<Poller> = None;
    let mut backoff = Duration::from_secs(2);
    while !stop.load(Ordering::SeqCst) {
        let Some(sew) = super::sew::backend() else { break };
        if poller.as_ref().is_none_or(|p| !Arc::ptr_eq(&p.sew, &sew)) {
            match Poller::new(pid, path, sew, inj.clone()) {
                Ok(p) => poller = Some(p),
                Err(e) => {
                    with_status(pid, |s| s.last_error = Some(e));
                    std::thread::sleep(backoff);
                    continue;
                }
            }
        }
        let p = poller.as_mut().expect("set above");
        match p.step(now(), core::POLL_MS) {
            Ok(()) => {
                backoff = Duration::from_secs(2);
                with_status(pid, |s| s.last_error = None);
            }
            Err(e) => {
                k2_core::log_debug!("[sew-hub] {pid}: {e}");
                with_status(pid, |s| s.last_error = Some(e));
                std::thread::sleep(backoff);
                backoff = (backoff * 2).min(Duration::from_secs(60));
            }
        }
    }
    with_status(pid, |s| s.running = false);
}

#[cfg(test)]
pub(crate) mod test_seams {
    use super::*;

    /// Mark `pid`'s poller as running (the route tests have no supervisor).
    pub(crate) fn set_running(pid: &str, on: bool) {
        with_status(pid, |s| s.running = on);
    }

    /// Hand deliveries to `pid`'s waiters as a poll would.
    pub(crate) fn offer(pid: &str, entries: Vec<Delivery>) -> Vec<Delivery> {
        offer_to_waiters(pid, entries)
    }
}

#[cfg(test)]
mod tests;
