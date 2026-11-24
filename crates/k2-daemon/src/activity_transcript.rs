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
//! After that it is parked, with no I/O at all, until a title change, a
//! client keystroke, screen output or a line K2 typed in wakes it
//! ([`wake`], [`note_delivery`]), or a hook makes the row busy. A Claude
//! row with working hooks is parked whenever it is idle: the hook says
//! when a turn starts. Except while a line K2 typed in is watched
//! ([`note_delivery`]): then any session is read every second, so the
//! Thread take check has a witness even when the hooks went quiet.
//!
//! **A new conversation in the same pane.** A hook-less harness (Codex)
//! tells K2 nothing on `/new`. While a delivered line is being watched
//! ([`note_delivery`]), its follower looks every [`ROLLOUT_CHECK_MS`] for
//! a newer transcript for its cwd that began after its current file went
//! quiet and that no other session owns, and switches to it.
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

/// A delivered line keeps the follower looking for a newer transcript
/// this long after the last [`note_delivery`]…
pub const DELIVERY_WATCH_MS: i64 = 30_000;
/// …every this often.
pub const ROLLOUT_CHECK_MS: i64 = 3_000;

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
    /// The last time the attached file was read (ms; 0 = never).
    last_read_at: i64,
    /// When the followed file began (ms), for the newer-file check.
    followed_born: i64,
    /// A delivered line is watched until then ([`note_delivery`]): read
    /// the transcript (any harness) and, hook-less, look for a newer one.
    rollout_watch_until: i64,
    next_rollout_check_at: i64,
}

struct Slot {
    registered_at: i64,
    /// The latest wake (ms), set without the follower lock.
    woken_at: AtomicI64,
    /// A line was typed in since the last pass ([`note_delivery`]).
    delivered: std::sync::atomic::AtomicBool,
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
        last_read_at: 0,
        followed_born: 0,
        rollout_watch_until: 0,
        next_rollout_check_at: 0,
    };
    let slot = Arc::new(Slot {
        registered_at: now,
        woken_at: AtomicI64::new(0),
        delivered: std::sync::atomic::AtomicBool::new(false),
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

/// [`note_delivery`] for a delivery's answer: only a successful delivery
/// to a known session wakes the reader.
pub fn note_delivered(success: bool, session_id: Option<&str>) {
    if let Some(sid) = session_id.filter(|_| success) {
        note_delivery(sid);
    }
}

/// K2 typed a line into the session (a Thread delivery), or is watching
/// one it typed: read the transcript again even if it was parked, and,
/// for a hook-less harness, look for a newer transcript for a while.
pub fn note_delivery(session_id: &str) {
    let slot = slots().lock().get(session_id).cloned();
    if let Some(slot) = slot {
        slot.woken_at.store(now_ms(), Ordering::SeqCst);
        slot.delivered.store(true, Ordering::SeqCst);
    }
}

/// When the session's transcript was last read, or `None` when no file
/// is attached (the Thread take check: a missing stamp means nothing
/// unless the file was read since the line went in).
pub fn transcript_read_at(session_id: &str) -> Option<i64> {
    let slot = slots().lock().get(session_id).cloned()?;
    let f = slot.follower.lock();
    f.follow.as_ref().map(|_| f.last_read_at)
}

/// The follower's conversation id and where it came from (status, tests).
#[allow(dead_code)] // integration tests via the lib
pub fn conversation_of(session_id: &str) -> Option<(String, ConversationSource)> {
    let slot = slots().lock().get(session_id).cloned()?;
    let f = slot.follower.lock();
    f.conversation.clone()
}

/// The per-tab passport check's one re-check (`session_token::conversation`):
/// a hook-less tab (Codex) tells K2 nothing on `/new`, `/clear` or fork, so
/// a call from its new conversation finds K2's record stale. Switch the
/// tab to `wanted` only when K2's own record shows that log is this tab's
/// next one: a log with that id for the tab's folder that began after the
/// tab's current log (or after the spawn, with none) and after the current
/// log went quiet, and that no live follower has claimed. True when the
/// tab is on `wanted` afterwards. The caller has already refused an id
/// another live tab holds.
pub fn recheck_for(session_id: &str, wanted: &str) -> bool {
    let Some(slot) = slots().lock().get(session_id).cloned() else { return false };
    let mut f = slot.follower.lock();
    if f.reader.is_none() {
        return false;
    }
    if f.conversation.is_none() {
        if let Some(id) = argv_conversation(session_id) {
            set_conversation(&mut f, id, ConversationSource::Argv);
        }
    }
    let current = f.conversation.as_ref().map(|(c, _)| c.clone());
    if current.as_deref() == Some(wanted) {
        return true;
    }
    let Some(cwd) = f.facts.cwd.clone() else { return false };
    let harness = f.facts.harness.clone();
    // The current log: the followed file, else the one its id names.
    let current_file = match f.follow.as_ref() {
        Some(follow) => Some(follow.path().to_path_buf()),
        None => current
            .as_deref()
            .and_then(|c| k2_core::chat_continue::locate_continue_transcript(&harness, c, &cwd)),
    };
    let (since, quiet_at) = match current_file.as_deref() {
        Some(p) => {
            let born = if f.follow.is_some() && f.followed_born > 0 { f.followed_born } else { file_birth_ms(p) };
            let quiet = std::fs::metadata(p)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_millis() as i64);
            (born, quiet)
        }
        None => (f.registered_at - ADOPT_SLACK_MS, 0),
    };
    let candidates = k2_core::chat_continue::adoptable_transcripts(&harness, &cwd, since);
    let pick = {
        let owned = claimed().lock();
        candidates.into_iter().find(|c| {
            c.id == wanted
                && Some(&c.id) != current.as_ref()
                && c.created_ms + ADOPT_SLACK_MS >= quiet_at
                && !owned.contains(&(harness.clone(), c.id.clone()))
        })
    };
    let Some(pick) = pick else { return false };
    log_debug!("[activity/transcript] {session_id}: passport re-check moved the tab to {}", pick.path.display());
    set_conversation(&mut f, pick.id.clone(), ConversationSource::Adopted);
    stamp_tab_session(&f.facts, &pick.id);
    let was_following = f.follow.is_some();
    f.followed_born = pick.created_ms;
    f.follow = Some(TranscriptFollow::new(pick.path, FollowStart::CatchUp));
    if let Some(reader) = f.reader.as_mut() {
        reader.reset();
    }
    f.backoff_ms = 0;
    f.next_poll_at = now_ms();
    if !was_following {
        crate::activity_store::set_transcript_resolvable(session_id, true);
    }
    true
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
    // A wake opens the idle window from this pass (the pass runs every
    // 200 ms, so that is the wake's own time).
    if slot.woken_at.swap(0, Ordering::SeqCst) > 0 {
        f.idle_follow_until = f.idle_follow_until.max(now + IDLE_FOLLOW_MS);
        f.next_poll_at = f.next_poll_at.min(now);
    }
    if slot.delivered.swap(false, Ordering::SeqCst) {
        f.rollout_watch_until = now + DELIVERY_WATCH_MS;
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
        // Claude rows keep their hook-driven re-resolve (`SessionStart`).
        let hookless = !view.had_hook && f.facts.harness != "claude";
        if hookless && now < f.rollout_watch_until && now >= f.next_rollout_check_at {
            f.next_rollout_check_at = now + ROLLOUT_CHECK_MS;
            switch_to_newer(&mut f, sid, now);
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
///
/// 0.46.18: while a delivered Thread line is watched ([`note_delivery`]),
/// every session is read, hooks or not. An idle Claude row with hooks
/// was parked, so when its hooks stopped reaching K2 the take check saw
/// neither the prompt nor the queued command that read the line in.
fn cadence(f: &Follower, view: &crate::activity_store::FollowView, now: i64) -> Option<i64> {
    if view.hot {
        Some(HOT_POLL_MS)
    } else if !view.display_idle || (!view.had_hook && now < f.idle_follow_until) || now < f.rollout_watch_until {
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
    f.last_read_at = now;
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
    // Out of usage (0.46.17): a limit record in the new lines.
    crate::llm_out_notice::on_transcript_lines(sid, &f.facts.harness, f.facts.project_id.as_deref(), &poll.lines);
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
            f.followed_born = file_birth_ms(&path);
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

/// A hook-less harness started a new conversation in this pane (Codex
/// `/new`): the newest transcript for the cwd that began after the
/// followed one went quiet, and that no other live session owns, becomes
/// the followed file, read from its start. Not counted in
/// [`resolve_scans`]: it runs only while a delivered line is watched.
fn switch_to_newer(f: &mut Follower, sid: &str, now: i64) {
    let (Some(cwd), Some(follow)) = (f.facts.cwd.clone(), f.follow.as_ref()) else { return };
    let current_quiet_at = std::fs::metadata(follow.path())
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64);
    let current = f.conversation.as_ref().map(|(c, _)| c.clone());
    // From the followed file's own birth, not a tick later: file times
    // come from a coarse clock, so a file begun moments after it can share
    // its stamp. The followed file itself is left out by id below.
    let candidates = k2_core::chat_continue::adoptable_transcripts(&f.facts.harness, &cwd, f.followed_born);
    let pick = {
        let owned = claimed().lock();
        candidates.into_iter().rev().find(|c| {
            Some(&c.id) != current.as_ref()
                && c.created_ms + ADOPT_SLACK_MS >= current_quiet_at
                && !owned.contains(&(f.facts.harness.clone(), c.id.clone()))
        })
    };
    let Some(pick) = pick else { return };
    log_debug!("[activity/transcript] {sid}: newer transcript {}; following it", pick.path.display());
    set_conversation(f, pick.id.clone(), ConversationSource::Adopted);
    stamp_tab_session(&f.facts, &pick.id);
    f.followed_born = pick.created_ms;
    f.follow = Some(TranscriptFollow::new(pick.path, FollowStart::CatchUp));
    if let Some(reader) = f.reader.as_mut() {
        reader.reset();
    }
    f.next_poll_at = now;
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

/// The session's visible screen rows (also the Thread take check's
/// "is the text still on screen", `delivery_take`).
pub(crate) fn grid_rows(sid: &str) -> Option<Vec<String>> {
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
