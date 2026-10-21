//! Live read-state frames (thread-read-v1, 0.46.1): `thread_read` /
//! `project_read` on `/cli/sessions/events`, person-scoped.
//!
//! Callers say WHAT changed, never compute frames themselves:
//! - [`conversation_changed`]: a post landed, a card was answered /
//!   voided / expired, a Thread moved. Every CONNECTED person gets a frame
//!   for that conversation (their own counts and totals).
//! - [`person_changed`]: one person read or muted on some device. That
//!   person's sockets (desktop and phone alike) get a frame.
//!
//! Work runs on one background worker that batches a burst (an agent
//! posting fast) into one pass, so a route never computes counts under
//! its own lock and never waits on it. Under `cfg(test)` the pass runs
//! inline so unit tests can watch the bus. A person with no live socket
//! gets nothing (the phone catches up with `GET /cli/thread/unread` and
//! the push `unreadTotal`).

use std::collections::{BTreeMap, BTreeSet};

use k2_core::notify_prefs::People;
use k2_core::read_state::{self, Person, Totals};

use crate::session_events::{self, ReadFrame, SessionEvent};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Job {
    Conversation { kind: String, id: String },
    Person { person: String, kind: String, id: String },
}

/// Someone posted in, answered a card in, or moved this conversation:
/// every connected person's counts for it may have changed.
pub fn conversation_changed(kind: &str, id: &str) {
    submit(Job::Conversation { kind: kind.to_string(), id: id.to_string() });
}

/// `person` read or muted this conversation: their sockets get a frame.
pub fn person_changed(person: &str, kind: &str, id: &str) {
    submit(Job::Person { person: person.to_string(), kind: kind.to_string(), id: id.to_string() });
}

// ── App guests (app-read-state-v1, 0.46.3) ───────────────────────────

/// An App reader's cursor or mute moved in one Thread (`key` is its
/// read-state key, `read_routes::PassReader`). Guests never ride
/// `/cli/sessions/events`: their overlay and activity sockets listen here
/// and each computes its own frame for its own pass (its rooms, R3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestMoved {
    pub key: String,
    pub conversation_id: String,
}

fn guest_bus() -> &'static tokio::sync::broadcast::Sender<GuestMoved> {
    static TX: std::sync::OnceLock<tokio::sync::broadcast::Sender<GuestMoved>> = std::sync::OnceLock::new();
    TX.get_or_init(|| tokio::sync::broadcast::channel(256).0)
}

/// An App guest (or App pass) read or muted `conversation_id` on some
/// surface: every socket of the same reader key hears it. Never blocks
/// (a broadcast send); nobody listening is fine.
pub fn guest_moved(key: &str, conversation_id: &str) {
    let _ = guest_bus().send(GuestMoved { key: key.to_string(), conversation_id: conversation_id.to_string() });
}

/// A guest socket's subscription. A lagged receiver skips (the App
/// re-reads `thread.unread` on reconnect; the frames carry whole state).
pub fn subscribe_guest() -> tokio::sync::broadcast::Receiver<GuestMoved> {
    guest_bus().subscribe()
}

/// Wait for the next move of `key` (skipping other readers' moves and
/// lag). Pending forever when `key` is `None` (an owner/Connect socket)
/// or the bus is gone (it never is: the sender is static).
pub async fn next_guest_move(rx: &mut tokio::sync::broadcast::Receiver<GuestMoved>, key: Option<&str>) -> GuestMoved {
    let Some(key) = key else { return std::future::pending().await };
    loop {
        match rx.recv().await {
            Ok(m) if m.key == key => return m,
            Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return std::future::pending().await,
        }
    }
}

#[cfg(test)]
fn submit(job: Job) {
    process(vec![job]);
}

#[cfg(not(test))]
fn submit(job: Job) {
    use std::sync::mpsc;
    use std::sync::{Mutex, OnceLock};
    static TX: OnceLock<Mutex<mpsc::Sender<Job>>> = OnceLock::new();
    let tx = TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Job>();
        let spawned = std::thread::Builder::new().name("read-live".into()).spawn(move || {
            while let Ok(first) = rx.recv() {
                // Let a burst land, then take it all in one pass.
                std::thread::sleep(std::time::Duration::from_millis(40));
                let mut jobs = vec![first];
                while let Ok(more) = rx.try_recv() {
                    jobs.push(more);
                }
                process(jobs);
            }
        });
        if let Err(e) = spawned {
            k2_core::log_debug!("[read_live] worker spawn failed: {e}");
        }
        Mutex::new(tx)
    });
    let sent = tx.lock().unwrap_or_else(|p| p.into_inner()).send(job);
    if let Err(e) = sent {
        k2_core::log_debug!("[read_live] worker gone: {e}");
    }
}

/// One pass: dedupe the jobs, resolve who is connected, compute each
/// (person, conversation) once and each person's totals once.
fn process(jobs: Vec<Job>) {
    let jobs: BTreeSet<Job> = jobs.into_iter().collect();
    let logins = crate::presence::live_logins();
    if logins.is_empty() {
        return;
    }
    let people = match People::load() {
        Ok(p) => p,
        Err(e) => {
            k2_core::log_debug!("[read_live] people load failed: {e}");
            return;
        }
    };
    let mut live: BTreeMap<String, Person> = BTreeMap::new();
    for l in &logins {
        let p = Person::of_login(&people, l);
        live.entry(p.key.clone()).or_insert(p);
    }
    let mut wanted: BTreeSet<(String, String, String)> = BTreeSet::new();
    for job in &jobs {
        match job {
            Job::Conversation { kind, id } => {
                for key in live.keys() {
                    wanted.insert((key.clone(), kind.clone(), id.clone()));
                }
            }
            Job::Person { person, kind, id } => {
                if live.contains_key(person) {
                    wanted.insert((person.clone(), kind.clone(), id.clone()));
                }
            }
        }
    }
    if wanted.is_empty() {
        return;
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    if !read_state::tables_exist(&conn) {
        return;
    }
    let now = read_state::now_secs();
    let mut totals: BTreeMap<String, Totals> = BTreeMap::new();
    let mut frames = Vec::new();
    for (key, kind, id) in wanted {
        let Some(person) = live.get(&key) else { continue };
        let item = match read_state::item(&conn, person, &kind, &id, now) {
            Ok(i) => i,
            // The conversation is gone (deleted group, moved key): nothing to say.
            Err(read_state::ReadError::NotFound(_)) => continue,
            Err(e) => {
                k2_core::log_debug!("[read_live] {kind} {id} for {key}: {e}");
                continue;
            }
        };
        let t = match totals.get(&key) {
            Some(t) => *t,
            None => match read_state::totals(&conn, person, now) {
                Ok(t) => {
                    totals.insert(key.clone(), t);
                    t
                }
                Err(e) => {
                    k2_core::log_debug!("[read_live] totals for {key}: {e}");
                    continue;
                }
            },
        };
        let frame = ReadFrame::of(&key, &item, t);
        frames.push(if kind == read_state::KIND_PROJECT {
            SessionEvent::ProjectRead(frame)
        } else {
            SessionEvent::ThreadRead(frame)
        });
    }
    drop(conn);
    for f in frames {
        let _ = session_events::emit(f);
    }
}
