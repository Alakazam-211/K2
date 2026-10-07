//! Transcript and screen evidence for the activity store
//! (prd-daemon-activity-and-thread-working-v1 S3: DA27, DA28 / A13, A18,
//! A19).
//!
//! One follower per live agent session whose harness writes a transcript
//! (Claude, Codex, Grok, Gemini), and a 1 s grid scan for the titleless
//! CLIs (Hermes, cursor-agent). Both feed the store as evidence
//! ([`Evidence::Transcript`], [`Evidence::Screen`]); the row decides what
//! it means (`k2_core::activity::ends`).
//!
//! **Finding the file (A18): resolve once, never per tick.** The
//! conversation id comes from, in order: a hook claim or `SessionStart`
//! ([`note_conversation`]), the spawn argv (`--session-id` premint,
//! `--resume`, Codex `resume <id>`), or, for a fresh tab with neither,
//! adoption: the earliest transcript for the session's cwd that began
//! after the spawn and that no other live session has adopted
//! (`chat_continue::adoptable_transcripts`), stamped into
//! `workspace_tab_sessions.session_id`. A miss retries with backoff, 5 s
//! doubling to 60 s. Every resolve is one counted scan ([`resolve_scans`]).
//!
//! **Reading it: only while it can matter (A19, per-session cost).** All
//! I/O runs on one dedicated thread, never a tokio worker. A session is
//! read every 200 ms while its lead is mid-turn (or a cancel or an
//! `end_turn` cross-check waits on a record), every 1 s while it is
//! monitoring, and every 1 s for two minutes after it goes idle when the
//! transcript is its only turn-start signal (no hooks: Codex, Grok).
//! After that it is parked, with no I/O at all, until a title change or a
//! client keystroke wakes it ([`wake`]), or a hook makes the row busy. A
//! Claude row with working hooks is parked whenever it is idle: the hook
//! says when a turn starts.
//!
//! A file born after the session spawned is read from its first record
//! (`CatchUp`: all of it is this session's); an older file (a resumed
//! conversation) only from its end.
//!
//! Seam for S6: [`subscribe`] carries every transcript signal with its
//! session (the `[thread:<addr>]` binding, A22).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use parking_lot::Mutex;
use tokio::sync::broadcast;

use k2_core::activity::screen::{self, ScreenScan};
use k2_core::activity::{Evidence, TranscriptReader, TranscriptSignal};
use k2_core::log_debug;
use k2_core::session::SessionId;
use k2_core::transcript_follow::{FollowStart, TranscriptFollow};

/// The follower thread's tick.
pub const TICK: Duration = Duration::from_millis(200);

/// Read cadence while the lead is mid-turn (DA27).
pub const HOT_POLL_MS: i64 = 200;

/// Read cadence while idle-but-followed or monitoring (DA27).
pub const WARM_POLL_MS: i64 = 1_000;

/// After going idle, a session with no hooks is read this long before it
/// parks (its transcript is its only turn-start signal).
pub const IDLE_FOLLOW_MS: i64 = 120_000;

/// A18: a missed resolve retries after this, doubling…
pub const RESOLVE_BACKOFF_MIN_MS: i64 = 5_000;
/// …up to this.
pub const RESOLVE_BACKOFF_MAX_MS: i64 = 60_000;

/// A transcript that began this much before the spawn still counts as
/// born after it (clock and file-time granularity).
pub const ADOPT_SLACK_MS: i64 = 2_000;

const BUS_CAP: usize = 1024;

/// Where a follower's conversation id came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversationSource {
    /// A hook claim or `SessionStart` from the pane's owner.
    Hook,
    /// The spawn argv.
    Argv,
    /// Adopted: the transcript that began after the spawn (A18).
    Adopted,
}

/// What the follower needs about a session (from the store's register).
#[derive(Debug, Clone)]
pub struct TrackFacts {
    pub session_id: String,
    pub agent_name: String,
    pub harness: String,
    pub cwd: Option<String>,
    pub project_id: Option<String>,
}

/// One transcript signal, for in-process subscribers (S6's Thread turns).
// Read by S6 (Thread turn binding); tests read it today.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct TranscriptEvent {
    pub session_id: String,
    pub signal: TranscriptSignal,
    pub at: i64,
}

struct Follower {
    facts: TrackFacts,
    registered_at: i64,
    conversation: Option<(String, ConversationSource)>,
    reader: Option<TranscriptReader>,
    follow: Option<TranscriptFollow>,
    next_resolve_at: i64,
    backoff_ms: i64,
    next_poll_at: i64,
    idle_follow_until: i64,
    markers: &'static [&'static str],
    screen: ScreenScan,
    next_screen_at: i64,
}

struct Slot {
    registered_at: i64,
    /// The latest wake (ms), set without the follower lock.
    woken_at: AtomicI64,
    /// A conversation id from a hook claim, applied on the next tick.
    note: Mutex<Option<String>>,
    /// The pane was just claimed: an unresolved transcript is retried on
    /// the next tick, not at the end of its backoff (A18).
    retry: std::sync::atomic::AtomicBool,
    follower: Mutex<Follower>,
}

fn slots() -> &'static Mutex<HashMap<String, Arc<Slot>>> {
    static S: OnceLock<Mutex<HashMap<String, Arc<Slot>>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

/// (harness, conversation id) owned by a live follower: adoption never
/// hands one file to two sessions.
fn claimed() -> &'static Mutex<HashSet<(String, String)>> {
    static C: OnceLock<Mutex<HashSet<(String, String)>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashSet::new()))
}

fn bus() -> &'static broadcast::Sender<TranscriptEvent> {
    static B: OnceLock<broadcast::Sender<TranscriptEvent>> = OnceLock::new();
    B.get_or_init(|| broadcast::channel(BUS_CAP).0)
}

static RESOLVE_SCANS: AtomicU64 = AtomicU64::new(0);

/// Every transcript signal any follower reads.
#[allow(dead_code)] // S6's Thread turn tracker; integration tests via the lib
pub fn subscribe() -> broadcast::Receiver<TranscriptEvent> {
    bus().subscribe()
}

/// How many resolve scans (transcript locate or adoption) have run.
#[allow(dead_code)] // integration tests via the lib (T-S3g)
pub fn resolve_scans() -> u64 {
    RESOLVE_SCANS.load(Ordering::SeqCst)
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Start following a newly registered session, if its harness has a
/// transcript or a screen marker. Called from the store's register.
pub fn track(facts: TrackFacts, now: i64) {
    let reader = TranscriptReader::new(&facts.harness);
    let markers = screen::markers_for(&facts.harness);
    if reader.is_none() && markers.is_empty() {
        return;
    }
    let sid = facts.session_id.clone();
    let follower = Follower {
        facts,
        registered_at: now,
        conversation: None,
        reader,
        follow: None,
        next_resolve_at: now,
        backoff_ms: 0,
        next_poll_at: now,
        idle_follow_until: now + IDLE_FOLLOW_MS,
        markers,
        screen: ScreenScan::default(),
        next_screen_at: now,
    };
    let slot = Arc::new(Slot {
        registered_at: now,
        woken_at: AtomicI64::new(0),
        note: Mutex::new(None),
        retry: std::sync::atomic::AtomicBool::new(false),
        follower: Mutex::new(follower),
    });
    if let Some(old) = slots().lock().insert(sid, slot) {
        release_claim(&old.follower.lock());
    }
}

/// The session went away: stop following it.
pub fn untrack(session_id: &str) {
    let removed = slots().lock().remove(session_id);
    if let Some(slot) = removed {
        release_claim(&slot.follower.lock());
    }
}

/// The pane's owner revealed (or moved to) this conversation (DA14 claim,
/// `SessionStart` clear/resume/fork): re-resolve against it.
pub fn note_conversation(session_id: &str, conversation: &str) {
    let slot = slots().lock().get(session_id).cloned();
    if let Some(slot) = slot {
        *slot.note.lock() = Some(conversation.to_string());
    }
}

/// The pane's owner just claimed it (DA14): if its transcript hasn't
/// resolved yet, try again now (A18: resolve on claim).
pub fn note_claim(session_id: &str) {
    let slot = slots().lock().get(session_id).cloned();
    if let Some(slot) = slot {
        slot.retry.store(true, Ordering::SeqCst);
    }
}

/// Something happened in the session (a title change, a client
/// keystroke): read its transcript again even if it was parked.
pub fn wake(session_id: &str) {
    let slot = slots().lock().get(session_id).cloned();
    if let Some(slot) = slot {
        slot.woken_at.store(now_ms(), Ordering::SeqCst);
    }
}

/// The follower's conversation id and where it came from (status, tests).
#[allow(dead_code)] // integration tests via the lib
pub fn conversation_of(session_id: &str) -> Option<(String, ConversationSource)> {
    let slot = slots().lock().get(session_id).cloned()?;
    let f = slot.follower.lock();
    f.conversation.clone()
}

fn release_claim(f: &Follower) {
    if let Some((conv, _)) = f.conversation.as_ref() {
        claimed().lock().remove(&(f.facts.harness.clone(), conv.clone()));
    }
}

fn set_conversation(f: &mut Follower, conv: String, source: ConversationSource) {
    release_claim(f);
    claimed().lock().insert((f.facts.harness.clone(), conv.clone()));
    f.conversation = Some((conv, source));
}

/// One pass over every follower at `now`: resolve what is due, read what
/// is due, scan the grids that are due. The follower thread calls this
/// every [`TICK`]; tests call it directly.
pub fn poll_all_once(now: i64) {
    let mut all: Vec<(String, Arc<Slot>)> =
        slots().lock().iter().map(|(k, v)| (k.clone(), Arc::clone(v))).collect();
    // Earlier spawns adopt first (two fresh tabs in one cwd, A18).
    all.sort_by(|a, b| a.1.registered_at.cmp(&b.1.registered_at).then_with(|| a.0.cmp(&b.0)));
    for (sid, slot) in all {
        poll_one(&sid, &slot, now);
    }
}

fn poll_one(sid: &str, slot: &Slot, now: i64) {
    let Some(view) = crate::activity_store::follow_view(sid) else { return };
    let mut f = slot.follower.lock();
    let woken = slot.woken_at.swap(0, Ordering::SeqCst);
    if woken > 0 {
        f.idle_follow_until = f.idle_follow_until.max(woken + IDLE_FOLLOW_MS);
        f.next_poll_at = f.next_poll_at.min(now);
    }
    if view.hot || !view.display_idle {
        f.idle_follow_until = now + IDLE_FOLLOW_MS;
    }
    if slot.retry.swap(false, Ordering::SeqCst) && f.follow.is_none() {
        f.next_resolve_at = now;
    }
    if let Some(conv) = slot.note.lock().take() {
        let same = f.conversation.as_ref().is_some_and(|(c, _)| *c == conv);
        if !same {
            set_conversation(&mut f, conv, ConversationSource::Hook);
            detach(&mut f, sid);
            f.next_resolve_at = now;
            f.backoff_ms = 0;
        }
    }
    if f.reader.is_some() {
        if f.follow.is_none() && now >= f.next_resolve_at {
            resolve(&mut f, sid, now);
        }
        read_due(&mut f, sid, &view, now);
    }
    if !f.markers.is_empty() && !view.had_hook && !view.had_transcript && now >= f.next_screen_at {
        f.next_screen_at = now + screen::SCAN_EVERY_MS;
        if let Some(rows) = grid_rows(sid) {
            let present = screen::rows_show_marker(f.markers, &rows);
            if let Some(working) = f.screen.observe(present, now) {
                crate::activity_store::apply_at(sid, Evidence::Screen(working), now);
            }
        }
    }
}

/// The cadence this session is read at right now, or `None` (parked).
fn cadence(f: &Follower, view: &crate::activity_store::FollowView, now: i64) -> Option<i64> {
    if view.hot {
        Some(HOT_POLL_MS)
    } else if !view.display_idle {
        Some(WARM_POLL_MS)
    } else if !view.had_hook && now < f.idle_follow_until {
        Some(WARM_POLL_MS)
    } else {
        None
    }
}

fn read_due(f: &mut Follower, sid: &str, view: &crate::activity_store::FollowView, now: i64) {
    let Some(interval) = cadence(f, view, now) else { return };
    if f.follow.is_none() || now < f.next_poll_at {
        return;
    }
    f.next_poll_at = now + interval;
    let Follower { follow, reader, .. } = f;
    let (Some(follow), Some(reader)) = (follow.as_mut(), reader.as_mut()) else { return };
    let poll = follow.poll();
    if poll.missing {
        log_debug!("[activity/transcript] {sid}: transcript gone; re-resolving");
        detach(f, sid);
        f.next_resolve_at = now + RESOLVE_BACKOFF_MIN_MS;
        return;
    }
    if poll.reset {
        reader.reset();
    }
    let mut signals = Vec::new();
    for line in &poll.lines {
        signals.extend(reader.push_line(line));
    }
    for signal in signals {
        crate::activity_store::apply_at(sid, Evidence::Transcript(&signal), now);
        let _ = bus().send(TranscriptEvent { session_id: sid.to_string(), signal, at: now });
    }
}

fn detach(f: &mut Follower, sid: &str) {
    if f.follow.take().is_some() {
        crate::activity_store::set_transcript_resolvable(sid, false);
    }
    if let Some(reader) = f.reader.as_mut() {
        reader.reset();
    }
}

/// A18: find the transcript once. Counted, so a test can prove nothing
/// re-locates per tick.
fn resolve(f: &mut Follower, sid: &str, now: i64) {
    RESOLVE_SCANS.fetch_add(1, Ordering::SeqCst);
    let harness = f.facts.harness.clone();
    let cwd = f.facts.cwd.clone().unwrap_or_default();
    if f.conversation.is_none() {
        if let Some(id) = argv_conversation(sid) {
            set_conversation(f, id, ConversationSource::Argv);
        }
    }
    let found = match f.conversation.as_ref() {
        Some((conv, _)) => {
            k2_core::chat_continue::locate_continue_transcript(&harness, conv, &cwd).map(|p| {
                let born = file_birth_ms(&p);
                let start = if born >= f.registered_at - ADOPT_SLACK_MS {
                    FollowStart::CatchUp
                } else {
                    FollowStart::FromEnd
                };
                (p, start)
            })
        }
        None => adopt(f).map(|p| (p, FollowStart::CatchUp)),
    };
    match found {
        Some((path, start)) => {
            log_debug!("[activity/transcript] {sid}: following {} ({start:?})", path.display());
            f.follow = Some(TranscriptFollow::new(path, start));
            if let Some(reader) = f.reader.as_mut() {
                reader.reset();
            }
            f.backoff_ms = 0;
            f.next_poll_at = now;
            crate::activity_store::set_transcript_resolvable(sid, true);
        }
        None => {
            f.backoff_ms = (f.backoff_ms * 2).clamp(RESOLVE_BACKOFF_MIN_MS, RESOLVE_BACKOFF_MAX_MS);
            f.next_resolve_at = now + f.backoff_ms;
        }
    }
}

/// The earliest transcript for this cwd that began after the spawn and
/// that no other live session owns. Stamped into the tab's row.
fn adopt(f: &mut Follower) -> Option<std::path::PathBuf> {
    let cwd = f.facts.cwd.clone()?;
    let since = f.registered_at - ADOPT_SLACK_MS;
    let candidates = k2_core::chat_continue::adoptable_transcripts(&f.facts.harness, &cwd, since);
    let pick = {
        let owned = claimed().lock();
        candidates
            .into_iter()
            .find(|c| !owned.contains(&(f.facts.harness.clone(), c.id.clone())))?
    };
    set_conversation(f, pick.id.clone(), ConversationSource::Adopted);
    stamp_tab_session(&f.facts, &pick.id);
    Some(pick.path)
}

/// A18: a fresh tab's adopted conversation becomes its resume id. The
/// pinned chat keeps its own SSOT (`workspace_sessions`, stamped by the
/// spawn path's own adoption), so it is left alone.
fn stamp_tab_session(facts: &TrackFacts, conversation: &str) {
    let Some(project_id) = facts.project_id.as_deref() else { return };
    if facts.agent_name == project_id {
        return;
    }
    let pane_group_id = crate::session_events::pane_group_id_from_agent(&facts.agent_name)
        .unwrap_or_else(|| facts.agent_name.clone());
    let db = k2_core::db::shared();
    let conn = db.lock();
    // A `k2 sidecar` tab still waiting for its conversation (`adopt_since`
    // set, migration 0132) is adopted by the sidecar's own watch: it
    // matches the brief marker, moves the pane-keyed Chats name onto the
    // id and clears `adopt_since`. Stamping here first would end that
    // watch early and strand the name on the pane id, so the follower
    // keeps its file for activity and leaves the row alone.
    if k2_core::sidecar::meta(&conn, project_id, &pane_group_id)
        .and_then(|m| m.adopt_since)
        .is_some()
    {
        log_debug!("[activity/transcript] {pane_group_id}: sidecar adoption pending; not stamping");
        return;
    }
    if let Err(e) = k2_core::db::schema::WorkspaceTabSession::stamp_session_id(&conn, project_id, &pane_group_id, conversation) {
        log_debug!("[activity/transcript] stamp {pane_group_id} failed: {e}");
    }
}

/// The conversation id the spawn argv names (`--session-id`, `--resume`,
/// Codex `resume <id>`), if the session is live and has one.
fn argv_conversation(sid: &str) -> Option<String> {
    let id = SessionId::parse(sid)?;
    let session = crate::v2_session_map::lookup_by_session_id(&id)?;
    k2_core::workspace::provider_resume::session_id_from_spawn_argv(
        session.program.as_deref().unwrap_or(""),
        &session.args,
    )
}

fn grid_rows(sid: &str) -> Option<Vec<String>> {
    let id = SessionId::parse(sid)?;
    let session = crate::v2_session_map::lookup_by_session_id(&id)?;
    Some(session.visible_text_rows())
}

fn file_birth_ms(path: &std::path::Path) -> i64 {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.created().or_else(|_| m.modified()).ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Drop every follower (integration tests share the process).
#[allow(dead_code)] // called via the LIB target by integration tests
pub fn clear_for_tests() {
    slots().lock().clear();
    claimed().lock().clear();
}

/// Start the follower thread, once per process (from the store's spawn).
pub fn spawn() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        let thread = std::thread::Builder::new().name("activity-transcript".into()).spawn(|| loop {
            poll_all_once(now_ms());
            std::thread::sleep(TICK);
        });
        if let Err(e) = thread {
            log_debug!("[activity/transcript] could not start the follower thread: {e}");
        }
    });
}
