//! The structure indexer (kit slice A1) and the blob-doc change events.
//!
//! **Queue only under the lock.** The kit emits `AuditEvent::IndexWanted`
//! with the session locked when a version of a file whose pack indexes it
//! (PSD/PSB files) commits. [`want`] only pushes `(doc, version)` onto a
//! bounded queue and wakes one thread; it never calls the kit. That thread
//! takes `BlobSession::index_job` off the lock and runs it:
//!
//! - **in the `k2-export` worker** (the `index` op): the file is hostile,
//!   so its parser runs in the sandboxed worker with the export limits.
//!   `IndexJob::open` streams the verified bytes to it, and
//!   `IndexJob::commit_wire` checks the worker's result as untrusted;
//! - **in this process** (`IndexJob::run`) only in a debug build with no
//!   worker (tests and dev); a release daemon without its worker indexes
//!   nothing (the verbs keep answering `MPC-E713`, the log says why).
//!
//! **Outcomes.** A recorded index (or a recorded refusal past a cap) is
//! done. `MPC-E714` (a refused result, or a worker that crashed or hit a
//! limit) isn't stored and the kit keeps no count of failures, so K2 does:
//! it backs off (5 s, then doubling) and gives up after
//! [`MAX_ATTEMPTS`] until the daemon restarts. `MPC-E717` (the version was
//! purged under the job) is dropped: nothing to index. A verb that meets
//! `MPC-E713` asks for the job again and waits briefly ([`wait_ready`]).
//!
//! **Events for open tabs** ([`changed`]): `doc_structure_changed` on the
//! session bus, metadata only (workspace id, path, what changed, version,
//! change seq, thread): an index recorded (refresh the layer panel), a
//! comment op (catch up with `comments --since <head>`), an anchor's new
//! state, a node claim taken or ended.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::AtomicBool;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use k2_core::log_debug;
use k2_mpc_base::index::{IndexJobError, IndexStatus};
use k2_mpc_base::session::{BlobSession, IndexJob};
use k2_mpc_base::store::{DocKey, VersionId};

use super::blob_packs;
use super::host::key_label;

/// Most `(doc, version)` pairs waiting. Past it a want is dropped (logged):
/// the next verb on that version asks again.
pub const MAX_QUEUED: usize = 256;
/// Attempts per version after `MPC-E714` before K2 stops trying.
pub const MAX_ATTEMPTS: u32 = 4;
/// The first back-off after a failed job; it doubles per attempt.
pub const FIRST_BACKOFF: Duration = Duration::from_secs(5);
/// How long a verb that met `MPC-E713` waits for the index before it
/// answers `index_pending`.
pub const VERB_WAIT: Duration = Duration::from_secs(8);

/// What changed on a blob doc, for [`changed`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    IndexReady,
    IndexNotIndexed,
    Annotation,
    AnchorState,
    ClaimTaken,
    ClaimEnded,
}

impl Change {
    pub const fn word(self) -> &'static str {
        match self {
            Change::IndexReady => "index_ready",
            Change::IndexNotIndexed => "index_not_indexed",
            Change::Annotation => "annotation",
            Change::AnchorState => "anchor_state",
            Change::ClaimTaken => "claim_taken",
            Change::ClaimEnded => "claim_ended",
        }
    }
}

/// Tell open tabs (`doc_structure_changed`). Called with the session
/// locked: a broadcast send, nothing else.
pub fn changed(key: &DocKey, change: Change, version: Option<VersionId>, seq: Option<u64>, thread: Option<String>) {
    let _ = crate::session_events::emit(crate::session_events::SessionEvent::DocStructureChanged {
        project_id: key.space.clone(),
        rel: key.path.clone(),
        change: change.word().to_string(),
        version,
        seq,
        thread,
    });
}

type Item = (DocKey, VersionId);

#[derive(Default)]
struct Queue {
    items: VecDeque<Item>,
    queued: HashSet<Item>,
    /// Failed attempts and when the next one may run.
    failures: HashMap<Item, (u32, Instant)>,
    running: bool,
}

fn queue() -> &'static (Mutex<Queue>, Condvar) {
    static Q: OnceLock<(Mutex<Queue>, Condvar)> = OnceLock::new();
    Q.get_or_init(|| (Mutex::new(Queue::default()), Condvar::new()))
}

/// Why [`push`] took nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pushed {
    Queued,
    Already,
    Full,
    GaveUp,
}

fn push(q: &mut Queue, item: Item) -> Pushed {
    if q.queued.contains(&item) {
        return Pushed::Already;
    }
    if q.failures.get(&item).is_some_and(|(n, _)| *n >= MAX_ATTEMPTS) {
        return Pushed::GaveUp;
    }
    if q.items.len() >= MAX_QUEUED {
        return Pushed::Full;
    }
    q.queued.insert(item.clone());
    q.items.push_back(item);
    Pushed::Queued
}

/// Tests: every `(doc, version)` [`want`] was asked for, on any thread.
#[cfg(test)]
pub(crate) static SEEN: Mutex<Vec<Item>> = Mutex::new(Vec::new());

/// `IndexWanted`: queue `(key, version)` and wake the indexer. Quick, and
/// never calls the kit (the session is locked).
pub fn want(key: &DocKey, version: VersionId) {
    #[cfg(test)]
    SEEN.lock().unwrap_or_else(|p| p.into_inner()).push((key.clone(), version));
    let (m, cv) = queue();
    let mut q = m.lock().unwrap_or_else(|p| p.into_inner());
    match push(&mut q, (key.clone(), version)) {
        Pushed::Queued => {}
        Pushed::Already => return,
        Pushed::Full => {
            log_debug!("[docs] WARN: index queue full ({MAX_QUEUED}); {} version {version} waits for its next read", key_label(key));
            return;
        }
        Pushed::GaveUp => return,
    }
    if !q.running {
        q.running = true;
        if std::thread::Builder::new().name("k2-docs-index".into()).spawn(worker).is_err() {
            q.running = false;
            log_debug!("[docs] WARN: the indexer thread couldn't start");
        }
    }
    cv.notify_all();
}

/// After a fresh open: a version with no index and no job out is queued
/// (versions stored before this K2 read PSD/PSB files emitted no
/// `IndexWanted`).
pub fn on_open(s: &BlobSession, key: &DocKey) {
    if let Ok(IndexStatus::Pending { version, running: false }) = s.index_status(None) {
        want(key, version);
    }
}

/// A verb met `MPC-E713` on `version`: ask for the job and wait up to
/// `max` for the version to leave `Pending`. True when it did (the verb
/// runs again).
pub fn wait_ready(s: &BlobSession, key: &DocKey, version: VersionId, max: Duration) -> bool {
    want(key, version);
    let until = Instant::now() + max;
    loop {
        match s.index_status(Some(version)) {
            Ok(IndexStatus::Pending { .. }) => {}
            Ok(_) => return true,
            Err(_) => return false,
        }
        if Instant::now() >= until {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The next item whose back-off has passed, else how long until one has.
fn take(q: &mut Queue, now: Instant) -> Result<Item, Option<Duration>> {
    let mut soonest: Option<Duration> = None;
    for i in 0..q.items.len() {
        let due = q.failures.get(&q.items[i]).map(|(_, at)| *at);
        match due {
            Some(at) if at > now => {
                let d = at - now;
                soonest = Some(soonest.map_or(d, |s| s.min(d)));
            }
            _ => {
                let item = q.items.remove(i).expect("index in range");
                q.queued.remove(&item);
                return Ok(item);
            }
        }
    }
    Err(soonest)
}

fn worker() {
    let (m, cv) = queue();
    loop {
        let item = {
            let mut q = m.lock().unwrap_or_else(|p| p.into_inner());
            loop {
                match take(&mut q, Instant::now()) {
                    Ok(item) => break item,
                    Err(wait) => {
                        let d = wait.unwrap_or(Duration::from_secs(30));
                        let (g, t) = cv.wait_timeout(q, d).unwrap_or_else(|p| p.into_inner());
                        q = g;
                        // Idle with nothing queued: the thread ends; the next want starts one.
                        if t.timed_out() && q.items.is_empty() {
                            q.running = false;
                            return;
                        }
                    }
                }
            }
        };
        let outcome = index_one(&item);
        let mut q = m.lock().unwrap_or_else(|p| p.into_inner());
        record(&mut q, item, outcome, Instant::now());
    }
}

/// What one job did.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Outcome {
    /// Recorded (an index or a refusal), or nothing to do.
    Done,
    /// `MPC-E714`, a worker failure or a store/blob error: back off.
    Failed(String),
    /// `MPC-E717`: the version moved under the job.
    Stale,
}

fn record(q: &mut Queue, item: Item, outcome: Outcome, now: Instant) {
    match outcome {
        Outcome::Done | Outcome::Stale => {
            q.failures.remove(&item);
        }
        Outcome::Failed(why) => {
            let n = q.failures.get(&item).map_or(0, |(n, _)| *n) + 1;
            let delay = FIRST_BACKOFF.saturating_mul(1 << (n - 1).min(10));
            if n >= MAX_ATTEMPTS {
                log_debug!(
                    "[docs] WARN: indexing {} version {} failed {n} times ({why}); not retried until the daemon restarts",
                    key_label(&item.0),
                    item.1
                );
                q.failures.insert(item, (n, now));
                return;
            }
            log_debug!(
                "[docs] WARN: indexing {} version {} failed ({why}); retry {n} in {}s",
                key_label(&item.0),
                item.1,
                delay.as_secs()
            );
            q.failures.insert(item.clone(), (n, now + delay));
            if !q.queued.contains(&item) {
                q.queued.insert(item.clone());
                q.items.push_back(item);
            }
        }
    }
}

fn index_one((key, version): &Item) -> Outcome {
    let Ok(rt) = super::runtime() else { return Outcome::Done };
    // Only a loaded doc: an unloaded one is queued again on its next open.
    let Some(s) = rt.get_blob(key) else { return Outcome::Done };
    let job = match s.index_job(Some(*version)) {
        Ok(Some(j)) => j,
        Ok(None) => return Outcome::Done,
        Err(e) => {
            log_debug!("[docs] {} version {version}: no index job ({e})", key_label(key));
            return Outcome::Done;
        }
    };
    let kind = blob_packs::kind_of(&key.path);
    let res = match (blob_packs::index_pack_name(kind), super::export_worker::binary()) {
        (Some(pack), Ok(bin)) => run_in_worker(job, pack, bin),
        (_, Err(why)) if cfg!(debug_assertions) => {
            log_debug!("[docs] {} version {version}: indexing in the daemon (debug build, no worker: {why})", key_label(key));
            job.run().map_err(Fail::Job)
        }
        (_, Err(why)) => Err(Fail::Worker(format!("the export worker isn't available ({why})"))),
        (None, Ok(_)) => return Outcome::Done,
    };
    outcome_of(res)
}

/// Why a job didn't record.
#[derive(Debug)]
enum Fail {
    Job(IndexJobError),
    Worker(String),
}

fn outcome_of(res: Result<IndexStatus, Fail>) -> Outcome {
    match res {
        Ok(_) => Outcome::Done,
        Err(Fail::Job(IndexJobError::Stale { .. })) => Outcome::Stale,
        Err(Fail::Job(e)) => Outcome::Failed(format!("{} ({e})", k2_mpc_base::errors::Refusal::report(&e).id())),
        Err(Fail::Worker(m)) => Outcome::Failed(m),
    }
}

/// The job in the `k2-export` worker: the verified stream goes to its
/// stdin, its wire result comes back as `D` frames and is committed as
/// untrusted (`commit_wire` reads the stream to its verified end first).
fn run_in_worker(mut job: IndexJob, pack: &str, bin: std::path::PathBuf) -> Result<IndexStatus, Fail> {
    let (mut input, il) = job.open().map_err(Fail::Job)?;
    let size = job.size();
    let mut limits = super::export_worker::limits();
    limits.max_input_bytes = size;
    limits.max_output_bytes = il.wire_max() as u64;
    let id = format!("index-{}", uuid::Uuid::new_v4().simple());
    let dir = super::export_worker::jobs_root().join(&id);
    std::fs::create_dir_all(&dir).map_err(|e| Fail::Worker(format!("job folder: {e}")))?;
    let header = k2_export::proto::Header::Index { job: id, pack: pack.to_string(), limits, index: il, size };
    let spawn = k2_export::host::Spawn { bin, cwd: dir.clone() };
    let never = AtomicBool::new(false);
    let mut wire: Vec<u8> = Vec::new();
    let res = k2_export::host::run_input(
        &spawn,
        &header,
        Some(k2_export::host::Input::Reader(&mut input, size)),
        super::export_worker::bounds(&limits),
        &never,
        &|_, _| {},
        &mut wire,
    );
    let _ = std::fs::remove_dir_all(&dir);
    match res {
        Ok(done) => {
            if !done.stderr.trim().is_empty() {
                log_debug!("[docs] index worker stderr: {}", done.stderr.trim());
            }
            job.commit_wire(input, &wire).map_err(Fail::Job)
        }
        Err(f) => {
            if !f.stderr.trim().is_empty() {
                log_debug!("[docs] index worker stderr: {}", f.stderr.trim());
            }
            Err(Fail::Worker(match f.error {
                k2_export::host::RunError::Kit(v) => format!("{} {}", v["id"].as_str().unwrap_or("?"), v["message"].as_str().unwrap_or("")),
                other => format!("{other:?}"),
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(p: &str) -> DocKey {
        DocKey { space: "ws".into(), path: p.into() }
    }

    #[test]
    fn a_want_queues_once_and_the_queue_is_bounded() {
        let mut q = Queue::default();
        assert_eq!(push(&mut q, (key("a.psd"), 1)), Pushed::Queued);
        assert_eq!(push(&mut q, (key("a.psd"), 1)), Pushed::Already, "the same version twice is one job");
        assert_eq!(push(&mut q, (key("a.psd"), 2)), Pushed::Queued);
        for v in 3..(MAX_QUEUED as u64 + 1) {
            assert_eq!(push(&mut q, (key("a.psd"), v)), Pushed::Queued, "{v}");
        }
        assert_eq!(q.items.len(), MAX_QUEUED);
        assert_eq!(push(&mut q, (key("b.psd"), 1)), Pushed::Full, "past the cap a want is dropped");
    }

    /// E714 isn't stored by the kit and it keeps no count: K2 backs off,
    /// doubling, and stops after MAX_ATTEMPTS; E717 and a recorded index
    /// clear the count; a version given up on isn't queued again.
    #[test]
    fn a_failed_job_backs_off_doubling_and_gives_up() {
        let mut q = Queue::default();
        let item = (key("a.psd"), 7);
        let t0 = Instant::now();
        record(&mut q, item.clone(), Outcome::Failed("MPC-E714".into()), t0);
        assert_eq!(q.failures[&item], (1, t0 + FIRST_BACKOFF));
        assert!(q.queued.contains(&item), "queued again for its retry");
        assert!(matches!(take(&mut q, t0), Err(Some(d)) if d == FIRST_BACKOFF), "not before the back-off");
        assert_eq!(take(&mut q, t0 + FIRST_BACKOFF), Ok(item.clone()));
        record(&mut q, item.clone(), Outcome::Failed("MPC-E714".into()), t0);
        assert_eq!(q.failures[&item].1, t0 + FIRST_BACKOFF * 2, "doubles");
        for _ in 2..MAX_ATTEMPTS {
            let _ = take(&mut q, t0 + Duration::from_secs(3600));
            record(&mut q, item.clone(), Outcome::Failed("MPC-E714".into()), t0);
        }
        assert_eq!(q.failures[&item].0, MAX_ATTEMPTS);
        let _ = take(&mut q, t0 + Duration::from_secs(3600));
        assert!(!q.queued.contains(&item), "given up: not queued");
        assert_eq!(push(&mut q, item.clone()), Pushed::GaveUp);

        let other = (key("b.psd"), 1);
        record(&mut q, other.clone(), Outcome::Failed("x".into()), t0);
        record(&mut q, other.clone(), Outcome::Stale, t0);
        assert!(!q.failures.contains_key(&other), "E717 clears the count");
    }

    #[test]
    fn e714_and_worker_failures_back_off_and_e717_does_not() {
        let bad = outcome_of(Err(Fail::Job(IndexJobError::BadResult("garbled".into()))));
        assert!(matches!(&bad, Outcome::Failed(m) if m.contains("MPC-E714")), "{bad:?}");
        assert_eq!(outcome_of(Err(Fail::Job(IndexJobError::Stale { version: 3, reason: "purged" }))), Outcome::Stale);
        assert!(matches!(outcome_of(Err(Fail::Worker("crashed".into()))), Outcome::Failed(_)));
    }

    #[test]
    fn change_words_are_stable() {
        let words: Vec<&str> = [
            Change::IndexReady,
            Change::IndexNotIndexed,
            Change::Annotation,
            Change::AnchorState,
            Change::ClaimTaken,
            Change::ClaimEnded,
        ]
        .iter()
        .map(|c| c.word())
        .collect();
        assert_eq!(words, ["index_ready", "index_not_indexed", "annotation", "anchor_state", "claim_taken", "claim_ended"]);
    }
}
