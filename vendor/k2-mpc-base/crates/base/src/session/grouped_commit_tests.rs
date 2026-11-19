//! Grouped commits (`CommitDurability::Grouped`): a change is acknowledged before it is on
//! disk, its group made durable later. What must hold across an abrupt stop and a power loss: the
//! store reopens as a prefix of the commits, every durable change kept, none torn; and the file
//! on disk is never ahead of the store. Red first: these rows fail until grouped commits are
//! built (each at its own `row:` assert that grouping happened), then hold its promise.
//!
//! - `grouped_commits_survive_a_kill_as_a_prefix`: a child process (this test binary run again)
//!   makes changes until the parent kills it (`SIGKILL`) at a seeded point;
//! - `grouped_commits_survive_a_power_loss_as_a_prefix`: the store on a test storage layer
//!   (`store/power_loss.rs`) loses power at a seeded point, writes in flight kept, lost or
//!   torn;
//! - `a_write_back_is_never_ahead_of_the_store`: a write-back, then a power loss that keeps no
//!   write in flight: the reopened store holds everything the file holds.
//!
//! Plants (`MPC_BETA_PLANT`), one per run: `durable-claimed-early` (every commit claimed
//! durable) caught at the kill row's durable-kept assert; `write-back-skips-barrier` (the
//! write-back doesn't make the group durable first) at the write-back row's outside-edit assert.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use super::*;
use crate::doc::Edit;
use crate::store::{CommitDurability, DocKey, DocStore, StoreSettings};
use crate::text::diff::Hunk;

const MD: TextFilePack = TextFilePack::new("markdown", 1 << 20);
const ROOT: &str = TextFilePack::ROOT;

struct H {
    now: AtomicU64,
    events: std::sync::Mutex<Vec<AuditEvent>>,
}

fn host(now: u64) -> Arc<H> {
    Arc::new(H {
        now: AtomicU64::new(now),
        events: Default::default(),
    })
}

impl Host for H {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full().with_owner(true)
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, e: &AuditEvent) {
        self.events.lock().unwrap().push(e.clone());
    }
}

fn config() -> Config {
    Config {
        edit_rate_per_s: 1_000_000,
        edit_burst: 1_000_000,
        idle_leave_ms: u64::MAX / 4,
        native_beta: true,
        ..Config::default()
    }
}

fn grouped() -> StoreSettings {
    StoreSettings::DEFAULT.with_commit_durability(CommitDurability::grouped(32, 100).unwrap())
}

fn manager(store: &Arc<DocStore>, host: &Arc<H>) -> SessionManager {
    manager_with(store, host, config())
}

fn manager_with(store: &Arc<DocStore>, host: &Arc<H>, c: Config) -> SessionManager {
    with_test_keys(SessionManager::new(store.clone(), host.clone(), c))
}

/// A build without the native engine has no key service to give.
fn with_test_keys(m: SessionManager) -> SessionManager {
    m
}

/// A build without the native engine: every barrier's doc opens as the default.
fn barrier_opts(_barrier: &str) -> OpenOptions {
    OpenOptions::new()
}

fn key(path: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: path.into(),
    }
}

fn dir(tag: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let d = std::env::temp_dir().join(format!(
        "k2mb-grouped-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("ws")).unwrap();
    std::fs::canonicalize(d).unwrap()
}

/// The docs the rows write: one on Yrs, and with the native engine one native.
fn docs() -> Vec<(DocKey, OpenOptions)> {
    #[allow(unused_mut)]
    let mut v = vec![(key("y.md"), OpenOptions::new())];
    v
}

/// The docs made, each holding `start\n`, and made durable.
fn make_docs(m: &SessionManager, store: &DocStore) -> Vec<(Arc<Session>, ConnId)> {
    let out = docs()
        .into_iter()
        .map(|(k, opts)| {
            let s = m
                .open_with(Arc::new(MD), &k, Some(b"start\n"), &opts)
                .unwrap();
            let c = s
                .join(
                    Participant::new("ann", "Ann", "#123456", ParticipantKind::Person, "tab"),
                    Stay::Held,
                )
                .unwrap()
                .conn;
            (s, c)
        })
        .collect();
    store.flush().unwrap();
    out
}

/// What a run of changes acknowledged: the changes made, the last one known durable, and
/// whether a change was ever acknowledged before it was durable (grouping happened).
#[derive(Debug, Default)]
struct Run {
    acked: u64,
    /// The changes begun (a child announces each before its commit): none past these can be
    /// in the store. 0 where every change begun was acknowledged (in-process runs).
    begun: u64,
    durable: Option<u64>,
    lagged: bool,
    positions: Vec<u64>,
}

impl Run {
    /// Change `i` acknowledged at commit position `at`; the store durable through `durable`.
    fn ack(&mut self, i: u64, at: u64, durable: u64) {
        self.positions.push(at);
        self.acked = i + 1;
        let known = self.positions.iter().rposition(|&p| p <= durable);
        self.durable = known.map(|j| j as u64);
        if known.map_or(true, |j| (j as u64) < i) {
            self.lagged = true;
        }
    }
}

/// Change `i`: the line `c{i}` appended to doc `i % docs`.
fn change(sessions: &[(Arc<Session>, ConnId)], i: u64) -> EditResult {
    change_r(sessions, i).unwrap()
}

/// [`change`], its result as it is.
fn change_r(
    sessions: &[(Arc<Session>, ConnId)],
    i: u64,
) -> std::result::Result<EditResult, SessionError> {
    let (s, c) = &sessions[(i % sessions.len() as u64) as usize];
    let at = s.read(*c)?.text_by_root[0].1.len();
    s.edit(
        *c,
        vec![Edit::Text {
            root: ROOT.into(),
            hunks: vec![Hunk {
                start: at,
                end: at,
                insert: format!("c{i}\n"),
            }],
        }],
        None,
    )
}

/// The reopened store holds a prefix of the changes (module docs): every doc's lines are its
/// own changes in order, together exactly changes `0..k` for one `k`, with `k` covering every
/// change known durable and none past the last acknowledged. Returns `k`.
fn check_prefix(store: &Arc<DocStore>, host: &Arc<H>, run: &Run, what: &str) -> u64 {
    let m = manager(store, host);
    let mut seen = Vec::new();
    let n_docs = docs().len() as u64;
    for (d, (k, _)) in docs().into_iter().enumerate() {
        let s = m
            .open(Arc::new(MD), &k, None)
            .unwrap_or_else(|e| panic!("row {what}: {k:?} reopens after the loss: {e}"));
        let c = s
            .join(
                Participant::new("bob", "Bob", "#654321", ParticipantKind::Person, "tab"),
                Stay::Held,
            )
            .unwrap()
            .conn;
        let text = s.read(c).unwrap().text_by_root[0].1.clone();
        let mut lines = text.lines();
        assert_eq!(
            lines.next(),
            Some("start"),
            "row {what}: {k:?} starts whole"
        );
        let mine: Vec<u64> = lines
            .map(|l| {
                l.strip_prefix('c')
                    .and_then(|n| n.parse().ok())
                    .unwrap_or_else(|| panic!("row {what}: {k:?} holds a torn line {l:?}"))
            })
            .collect();
        for (j, i) in mine.iter().enumerate() {
            assert_eq!(
                *i,
                d as u64 + j as u64 * n_docs,
                "row {what}: {k:?}'s changes in order, none missing between: {mine:?}"
            );
        }
        seen.extend(mine);
    }
    m.release().unwrap();
    seen.sort_unstable();
    let k = seen.len() as u64;
    assert!(
        seen.iter().copied().eq(0..k),
        "row {what}: the docs together hold exactly changes 0..{k}"
    );
    let upper = run.begun.max(run.acked);
    assert!(
        k <= upper,
        "control {what}: no change past the last one begun ({k} > {upper})"
    );
    if let Some(d) = run.durable {
        assert!(
            k > d,
            "row {what}: every change known durable is kept: change {d} lost ({k} kept)"
        );
    }
    k
}

/// The changes a child or a run makes before its loss, from `seed`: 50 to 600.
fn target(seed: u64) -> u64 {
    50 + (seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 33) % 551
}

fn env(k: &str, d: u64) -> u64 {
    std::env::var(k)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(d)
}

const CHILD: &str = "K2MB_GROUPED_CHILD";

/// The child's half of the kill row: changes, each acknowledged on stdout with the last change
/// known durable, until it is killed.
fn kill_row_child(d: &Path) {
    let host = host(1_000_000);
    let store = Arc::new(DocStore::open_with(d.join("docs.redb"), grouped()).unwrap());
    let m = manager(&store, &host);
    let sessions = make_docs(&m, &store);
    println!("GROUPED-CHILD READY");
    let mut run = Run::default();
    for i in 0..1_000_000u64 {
        // announced before its commit: the store may hold it once this line is out
        println!("GROUPED-CHILD BEGIN {i}");
        change(&sessions, i);
        run.ack(i, store.commits(), store.durable_through());
        let durable = run.durable.map_or(-1, |d| d as i64);
        println!("GROUPED-CHILD ACK {i} {durable}");
        if i % 10 == 9 {
            host.now.fetch_add(5, Ordering::SeqCst);
            m.tick().unwrap();
        }
    }
}

/// Kill at any point (module docs): `K2MB_GROUPED_KILLS` kills (default 10; 50 by name), each
/// at a seeded point, of a child making grouped changes on a Yrs doc and a native one; after
/// each, the store reopens as a prefix of the changes, every one the child knew durable kept.
/// Over the kills, some change was acknowledged before it was durable (grouping happened).
#[test]
fn grouped_commits_survive_a_kill_as_a_prefix() {
    if let Ok(d) = std::env::var(CHILD) {
        kill_row_child(Path::new(&d));
        return;
    }
    let kills = env("K2MB_GROUPED_KILLS", 10);
    let me = std::env::current_exe().unwrap();
    let mut lagged = false;
    for seed in 0..kills {
        let d = dir("kill");
        let mut child = std::process::Command::new(&me)
            .args([
                "session::grouped_commit_tests::grouped_commits_survive_a_kill_as_a_prefix",
                "--exact",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD, &d)
            .stdout(std::process::Stdio::piped())
            .stderr(std::fs::File::create(d.join("child.err")).unwrap())
            .spawn()
            .unwrap();
        let mut out = BufReader::new(child.stdout.take().unwrap());
        let want = target(seed);
        let mut run = Run::default();
        let mut ready = false;
        let mut other = Vec::new();
        // one whole line of the child's (`None` at its end; a line cut by the kill is dropped)
        let mut read = |run: &mut Run, ready: &mut bool, other: &mut Vec<String>| -> bool {
            let mut line = String::new();
            if out.read_line(&mut line).unwrap_or(0) == 0 || !line.ends_with('\n') {
                return false;
            }
            let line = line.trim_end();
            if !line.contains("GROUPED-CHILD") && other.len() < 20 {
                other.push(line.to_owned());
            }
            if line.ends_with("GROUPED-CHILD READY") {
                *ready = true;
            }
            if let Some(at) = line.find("GROUPED-CHILD BEGIN ") {
                let i: u64 = line[at + "GROUPED-CHILD BEGIN ".len()..].parse().unwrap();
                run.begun = run.begun.max(i + 1);
            }
            if let Some(at) = line.find("GROUPED-CHILD ACK ") {
                let mut f = line[at + "GROUPED-CHILD ACK ".len()..].split(' ');
                let i: u64 = f.next().unwrap().parse().unwrap();
                let durable: i64 = f.next().unwrap().parse().unwrap();
                run.acked = i + 1;
                run.durable = u64::try_from(durable).ok();
                if durable < i as i64 {
                    run.lagged = true;
                }
            }
            true
        };
        while run.acked < want && read(&mut run, &mut ready, &mut other) {}
        let early = child.try_wait().ok().flatten();
        child.kill().unwrap();
        // what the child wrote before the kill took it: it ran on after the last line read
        while read(&mut run, &mut ready, &mut other) {}
        let _ = child.wait();
        assert!(
            ready && run.acked > 0,
            "control {seed}: the child made changes before its kill (it ended first: {early:?}; \
             its stdout: {other:?}; its stderr: {})",
            std::fs::read_to_string(d.join("child.err")).unwrap_or_default()
        );
        let host = host(2_000_000);
        let store = Arc::new(DocStore::open(d.join("docs.redb")).unwrap());
        let k = check_prefix(&store, &host, &run, &format!("kill {seed}"));
        println!(
            "GROUPED-KILL seed={seed} acked={} durable={:?} kept={k}",
            run.acked, run.durable
        );
        lagged |= run.lagged;
        drop(store);
        let _ = std::fs::remove_dir_all(&d);
    }
    assert!(
        lagged,
        "row: grouped commits: some change was acknowledged before it was durable"
    );
}

/// A power loss at any point (module docs): `K2MB_GROUPED_LOSSES` losses (default 10) per
/// durability, each at a seeded point, writes in flight kept, lost or torn; the store reopens
/// from the image alone as a prefix of the changes, every durable one kept. At `Immediate`
/// every acknowledged change survives; grouped, some change was acknowledged before it was
/// durable.
#[test]
fn grouped_commits_survive_a_power_loss_as_a_prefix() {
    use crate::store::power_loss::PowerLoss;
    let losses = env("K2MB_GROUPED_LOSSES", 10);
    for (arm, settings) in [
        ("immediate", StoreSettings::DEFAULT),
        ("grouped", grouped()),
    ] {
        let mut lagged = false;
        for seed in 0..losses {
            let d = dir("power");
            let layer = PowerLoss::default();
            let host = host(1_000_000);
            let store = Arc::new(
                DocStore::open_on_backend_for_tests(layer.clone(), d.join("docs.redb"), settings)
                    .unwrap(),
            );
            let m = manager(&store, &host);
            let sessions = make_docs(&m, &store);
            let mut run = Run::default();
            for i in 0..target(seed) {
                change(&sessions, i);
                run.ack(i, store.commits(), store.durable_through());
                if i % 10 == 9 {
                    host.now.fetch_add(5, Ordering::SeqCst);
                    m.tick().unwrap();
                }
            }
            let image = layer.lose_power(seed, false);
            drop(sessions);
            drop(m);
            drop(store);
            let store = Arc::new(
                DocStore::open_on_backend_for_tests(
                    PowerLoss::from_image(image),
                    d.join("docs.redb"),
                    settings,
                )
                .unwrap(),
            );
            let k = check_prefix(&store, &host, &run, &format!("{arm} power {seed}"));
            println!(
                "GROUPED-POWER arm={arm} seed={seed} acked={} durable={:?} kept={k}",
                run.acked, run.durable
            );
            if arm == "immediate" {
                assert_eq!(
                    k, run.acked,
                    "row {arm} {seed}: at Immediate every acknowledged change survives"
                );
            }
            lagged |= run.lagged;
            let _ = std::fs::remove_dir_all(&d);
        }
        if arm == "grouped" {
            assert!(
                lagged,
                "row {arm}: grouped commits: some change was acknowledged before it was durable"
            );
        }
    }
}

/// The file on disk is never ahead of the store (module docs): a doc kept in sync with its
/// file, five grouped edits (not yet durable), the write-back after the quiet window, then a
/// power loss that keeps no write in flight; the reopened store holds everything the file holds:
/// the file's check at the next open is no outside edit, and the doc's text is the file's.
/// Grouped at 1,024 changes and 10 s, and the tick just past the quiet window, so the
/// write-back's barrier alone makes the edits durable.
#[test]
fn a_write_back_is_never_ahead_of_the_store() {
    use crate::store::power_loss::PowerLoss;
    let d = dir("writeback");
    let file = d.join("ws").join("a.md");
    std::fs::write(&file, "start\n").unwrap();
    let layer = PowerLoss::default();
    let host = host(1_000_000);
    let store = Arc::new(
        DocStore::open_on_backend_for_tests(
            layer.clone(),
            d.join("docs.redb"),
            grouped_as(1024, 10_000),
        )
        .unwrap(),
    );
    let m = manager(&store, &host);
    let k = key("a.md");
    let s = m
        .open_file_with(Arc::new(MD), &k, &file, &d.join("ws"), &OpenOptions::new())
        .unwrap();
    store.flush().unwrap();
    let c = s
        .join(
            Participant::new("ann", "Ann", "#123456", ParticipantKind::Person, "tab"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    let sessions = vec![(s.clone(), c)];
    for i in 0..5 {
        change(&sessions, i);
    }
    assert!(
        store.durable_through() < store.commits(),
        "row: grouped: the edits before the write-back aren't durable yet ({} of {})",
        store.durable_through(),
        store.commits()
    );
    host.now
        .fetch_add(config().write_quiet_ms + 1, Ordering::SeqCst);
    let rep = m.tick().unwrap();
    assert_eq!(
        rep.written,
        vec![k.clone()],
        "control: the tick wrote the file back"
    );
    let on_disk = std::fs::read_to_string(&file).unwrap();
    assert!(on_disk.contains("c4"), "control: the file holds the edits");
    let image = layer.lose_power(0, true);
    drop(sessions);
    drop(s);
    drop(m);
    drop(store);
    let store = Arc::new(
        DocStore::open_on_backend_for_tests(
            PowerLoss::from_image(image),
            d.join("docs.redb"),
            grouped_as(1024, 10_000),
        )
        .unwrap(),
    );
    host.events.lock().unwrap().clear();
    let m = manager(&store, &host);
    let s = m
        .open_file_with(Arc::new(MD), &k, &file, &d.join("ws"), &OpenOptions::new())
        .unwrap();
    let outside: Vec<AuditEvent> = host
        .events
        .lock()
        .unwrap()
        .iter()
        .filter(|e| matches!(e, AuditEvent::ExternalEdit { .. }))
        .cloned()
        .collect();
    assert!(
        outside.is_empty(),
        "row: the file wasn't ahead of the store: the open took it in as an outside edit: \
         {outside:?}"
    );
    let check = s.check_disk().unwrap();
    assert!(
        !matches!(
            check,
            DiskCheck::Ingested { .. } | DiskCheck::Conflict { .. }
        ),
        "row: the file wasn't ahead of the store: its check is {check:?}"
    );
    let c = s
        .join(
            Participant::new("bob", "Bob", "#654321", ParticipantKind::Person, "tab"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    assert_eq!(
        s.read(c).unwrap().text_by_root[0].1,
        on_disk,
        "row: the reopened doc holds what the file holds"
    );
    m.release().unwrap();
    let _ = std::fs::remove_dir_all(&d);
}

/// One doc `k` (with `opts`) holding `start\n`, joined by Ann, and made durable.
fn make_one(
    m: &SessionManager,
    store: &DocStore,
    k: &DocKey,
    opts: &OpenOptions,
) -> (Arc<Session>, ConnId) {
    let s = m
        .open_with(Arc::new(MD), k, Some(b"start\n"), opts)
        .unwrap();
    let c = s
        .join(
            Participant::new("ann", "Ann", "#123456", ParticipantKind::Person, "tab"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    store.flush().unwrap();
    (s, c)
}

fn grouped_as(max_changes: u32, max_ms: u64) -> StoreSettings {
    StoreSettings::DEFAULT
        .with_commit_durability(CommitDurability::grouped(max_changes, max_ms).unwrap())
}

/// The group's bounds (`store/group.rs`), grouped at 4 changes and 100 ms: no more than 3
/// commits ever wait for the disk, and a result says durable only when its change is; a group
/// that waited 100 ms is made durable at the tick, and at the next commit; at `Immediate` every
/// result says durable. Plants: `group-changes-ignored` caught at the count assert,
/// `group-ms-ignored` at the tick's time assert.
#[test]
fn the_group_bound_holds() {
    use std::sync::atomic::Ordering::SeqCst;
    let d = dir("bound");
    let h = host(1_000_000);
    let store = Arc::new(DocStore::open_with(d.join("docs.redb"), grouped_as(4, 100)).unwrap());
    let m = manager(&store, &h);
    let sessions = make_docs(&m, &store);
    let (mut lagged, mut reached) = (false, false);
    for i in 0..40 {
        let r = change(&sessions, i);
        let waiting = store.commits() - store.durable_through();
        assert!(
            waiting < 4,
            "row: the count bound: {waiting} commits wait (at most 3)"
        );
        assert!(
            !r.durable || waiting == 0,
            "row: a result says durable only when it is ({waiting} waiting)"
        );
        lagged |= !r.durable;
        reached |= waiting == 0;
    }
    assert!(
        lagged && reached,
        "control: changes waited, and groups were made durable"
    );
    m.release().unwrap();
    drop(sessions);
    drop(m);
    drop(store);
    // the time bounds, grouped at 1,024 changes and 100 ms: each doc's first change raises its
    // seq floor (made durable at once, `store/seq_floor.rs`), and no other change reaches it
    let d1 = dir("bound-time");
    let store = Arc::new(DocStore::open_with(d1.join("docs.redb"), grouped_as(1024, 100)).unwrap());
    let m = manager(&store, &h);
    let sessions = make_docs(&m, &store);
    let n = sessions.len() as u64;
    for i in 0..n {
        assert!(
            change(&sessions, i).durable,
            "control: a doc's first change raises its floor, durably"
        );
    }
    // the time bound at the tick
    let r = change(&sessions, n);
    assert!(!r.durable, "control: a change waits for its group");
    h.now.fetch_add(99, SeqCst);
    m.tick().unwrap();
    assert!(
        store.durable_through() < store.commits(),
        "control: a tick before 100 ms leaves the group waiting"
    );
    h.now.fetch_add(1, SeqCst);
    m.tick().unwrap();
    assert_eq!(
        store.durable_through(),
        store.commits(),
        "row: the time bound at the tick: the group is durable once it waited 100 ms"
    );
    // the time bound at the next commit
    let r = change(&sessions, n + 1);
    assert!(!r.durable, "control: a change waits for its group");
    h.now.fetch_add(100, SeqCst);
    let r = change(&sessions, n + 2);
    assert!(
        r.durable,
        "row: the time bound at a commit: the commit 100 ms after the group's first is durable"
    );
    m.release().unwrap();
    drop(sessions);
    drop(store);
    // Immediate: every result durable
    let d2 = dir("bound-immediate");
    let store = Arc::new(DocStore::open(d2.join("docs.redb")).unwrap());
    let m = manager(&store, &h);
    let sessions = make_docs(&m, &store);
    for i in 0..10 {
        let r = change(&sessions, i);
        assert!(r.durable, "row: at Immediate every result says durable");
        assert_eq!(
            store.durable_through(),
            store.commits(),
            "row: at Immediate every commit is durable"
        );
    }
    m.release().unwrap();
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&d2);
}

/// The barriers (`store/group.rs`): grouped at 1,024 changes and 10 s, so only a barrier
/// makes a group durable here; three edits wait, then each barrier in turn, on a fresh store,
/// leaves every commit durable. Plants: `barrier-skipped-{name}` (each barrier made an
/// ordinary commit) caught at this row's assert for that barrier.
#[test]
fn barriers_make_the_group_durable() {
    use crate::blob::{BlobConfig, BlobStore};
    use std::sync::atomic::Ordering::SeqCst;
    #[allow(unused_mut)]
    let mut barriers = vec![
        "flush", "close", "unload", "release", "delete", "purge", "rewrite", "demote", "promote",
    ];
    for barrier in barriers {
        let d = dir("barrier");
        let h = host(1_000_000);
        let store =
            Arc::new(DocStore::open_with(d.join("docs.redb"), grouped_as(1024, 10_000)).unwrap());
        let hc = h.clone();
        let blobs = Arc::new(
            BlobStore::open(
                store.clone(),
                d.join("blobs"),
                BlobConfig::default(),
                Arc::new(move || hc.now.load(SeqCst)),
            )
            .unwrap(),
        );
        // idle unloads after 1 s, well inside the group's 10 s
        let c = Config {
            idle_unload_ms: 1_000,
            ..config()
        };
        let m = manager_with(&store, &h, c).with_blob_store(blobs);
        let k = key("y.md");
        let opts = barrier_opts(barrier);
        if barrier == "promote" {
            let mut start: &[u8] = b"start\n";
            m.open_blob(Arc::new(AnyFile), &key("p.md"), Some(&mut start))
                .unwrap();
        }
        // the demotion's doc lives in a file (a doc made from bytes alone is demoted with them)
        let file = d.join("ws").join("y.md");
        let one = if barrier == "demote" {
            std::fs::write(&file, "start\n").unwrap();
            let s = m
                .open_file_with(Arc::new(MD), &k, &file, &d.join("ws"), &opts)
                .unwrap();
            let c = s
                .join(
                    Participant::new("ann", "Ann", "#123456", ParticipantKind::Person, "tab"),
                    Stay::Held,
                )
                .unwrap()
                .conn;
            store.flush().unwrap();
            (s, c)
        } else {
            make_one(&m, &store, &k, &opts)
        };
        let sessions = vec![one.clone()];
        for i in 0..3 {
            change(&sessions, i);
        }
        assert!(
            store.durable_through() < store.commits(),
            "control {barrier}: the edits wait for the disk"
        );
        // what was made before the barrier (the call may commit more after it)
        let before = store.commits();
        match barrier {
            "flush" => m.flush().unwrap(),
            "close" => assert!(m.close(&k, "test").unwrap(), "control: closed"),
            "unload" => {
                one.0.leave(one.1).unwrap();
                drop(sessions);
                drop(one);
                h.now.fetch_add(1_001, SeqCst);
                assert_eq!(m.unload_idle().unwrap(), 1, "control: unloaded");
            }
            "release" => m.release().unwrap(),
            "delete" => m.delete_doc(&k).unwrap(),
            "rewrite" => {
                store.rewrite_file().unwrap();
            }
            "purge" => {
                one.0
                    .purge(one.1, PurgeRequest::new(PurgeScope::History))
                    .unwrap();
            }
            "demote" => {
                m.demote_text_file(Arc::new(MD), Arc::new(AnyFile), &k, &file, &d.join("ws"))
                    .unwrap();
            }
            "promote" => {
                m.promote_blob(Arc::new(MD), &key("p.md")).unwrap();
            }
            _ => unreachable!(),
        }
        assert!(
            store.durable_through() >= before,
            "row: the barrier {barrier} makes every commit before it durable ({} of {before})",
            store.durable_through()
        );
        let _ = m.release();
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// A failed durable commit (`store/group.rs`; the design's in-process rollback): grouped edits
/// on each doc (a Yrs one and a native one) wait; the next commit, 100 ms on, waits for the
/// disk and fails there. The edit is refused with the store's error, and so is every call that
/// reaches the store until the manager's tick, which reopens the store at its durable state
/// and reloads every live doc from it: each doc's text is its durable changes only, with one
/// `RolledBack` event each and one `StoreRolledBack`; the next edit on each lands, and a reopen
/// of what the disk holds loads every doc cleanly with it. Plants: `rollback-keeps-live-docs`
/// and `rollback-skips-yrs`, each caught at a text assert.
#[test]
fn a_failed_durable_commit_reloads_every_live_doc() {
    use crate::store::power_loss::PowerLoss;
    use std::sync::atomic::Ordering::SeqCst;
    let d = dir("failed");
    let layer = PowerLoss::default();
    let h = host(1_000_000);
    let settings = grouped_as(1024, 100);
    let store = Arc::new(
        DocStore::open_on_backend_for_tests(layer.clone(), d.join("docs.redb"), settings).unwrap(),
    );
    let m = manager(&store, &h);
    let sessions = make_docs(&m, &store);
    let n = sessions.len() as u64;
    let before: Vec<crate::store::Seq> = sessions
        .iter()
        .map(|(s, c)| s.read(*c).unwrap().version)
        .collect();
    // each doc's first change since the open raises its seq floor, so it is made durable
    // (`store/seq_floor.rs`); its second waits
    for i in 0..2 * n {
        let r = change(&sessions, i);
        assert_eq!(
            r.durable,
            i < n,
            "control: change {i} durable only as its doc's first"
        );
    }
    assert!(
        store.durable_through() < store.commits(),
        "control: the edits wait for the disk"
    );
    h.now.fetch_add(100, SeqCst);
    layer.fail_next_sync();
    let failed = change_r(&sessions, 2 * n);
    assert!(
        failed
            .as_ref()
            .is_err_and(|e| e.to_string().contains("planted sync failure")),
        "row: the edit whose commit failed at the disk is refused with the store's error: \
         {failed:?}"
    );
    assert!(
        change_r(&sessions, 2 * n + 1).is_err(),
        "row: the store refuses every change until it is reopened"
    );
    // a read in between is refused as well, typed and retryable: the doc holds changes the
    // store lost until the tick reloads it
    for (j, (s, c)) in sessions.iter().enumerate() {
        let read = s.read(*c);
        assert!(
            read.as_ref().is_err_and(|e| e.report().id() == "MPC-E526"),
            "row: doc {j}: a read before the tick is refused MPC-E526: {:?}",
            read.map(|r| r.version)
        );
        assert_eq!(
            read.unwrap_err().report().exit(),
            1,
            "row: doc {j}: MPC-E526 exits 1 (retry)"
        );
    }
    h.events.lock().unwrap().clear();
    m.tick().unwrap();
    let events = std::mem::take(&mut *h.events.lock().unwrap());
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, AuditEvent::StoreRolledBack { .. }))
            .count(),
        1,
        "row: one StoreRolledBack: {events:?}"
    );
    // every connection was closed (its writer held lost changes), and joins again
    let sessions: Vec<(Arc<Session>, ConnId)> = sessions
        .iter()
        .enumerate()
        .map(|(j, (s, c))| {
            let left = events.iter().any(|e| {
                matches!(e, AuditEvent::Left { conn, reason: LeaveReason::RolledBack, .. }
                    if conn == c)
            });
            assert!(
                left,
                "row: doc {j}'s connection left RolledBack: {events:?}"
            );
            assert!(s.read(*c).is_err(), "row: doc {j}'s old connection is gone");
            let again = s
                .join(
                    Participant::new("ann", "Ann", "#123456", ParticipantKind::Person, "tab"),
                    Stay::Held,
                )
                .unwrap()
                .conn;
            (s.clone(), again)
        })
        .collect();
    for (j, (s, c)) in sessions.iter().enumerate() {
        let r = s.read(*c).unwrap();
        assert_eq!(
            r.text_by_root[0].1,
            format!("start\nc{j}\n"),
            "row: doc {j} holds its durable changes only"
        );
        assert_eq!(
            r.version,
            before[j] + 1,
            "row: doc {j} is at its durable version"
        );
        let key = s.key().unwrap();
        let mine: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, AuditEvent::RolledBack { key: k, .. } if *k == key))
            .collect();
        assert_eq!(
            mine,
            vec![&AuditEvent::RolledBack {
                key: key.clone(),
                lost_from: before[j] + 2,
                durable_seq: before[j] + 1,
            }],
            "row: one RolledBack for doc {j}"
        );
    }
    // the next edit on each lands, and is made durable
    for i in 0..n {
        change(&sessions, 100 + i);
    }
    m.flush().unwrap();
    m.release().unwrap();
    drop(sessions);
    drop(m);
    let image = layer.lose_power(0, true);
    drop(store);
    let store = Arc::new(
        DocStore::open_on_backend_for_tests(
            PowerLoss::from_image(image),
            d.join("docs.redb"),
            settings,
        )
        .unwrap(),
    );
    let m = manager(&store, &h);
    for (j, (k, _)) in docs().into_iter().enumerate() {
        let s = m
            .open(Arc::new(MD), &k, None)
            .unwrap_or_else(|e| panic!("row: doc {j} loads cleanly after the rollback: {e}"));
        let c = s
            .join(
                Participant::new("bob", "Bob", "#654321", ParticipantKind::Person, "tab"),
                Stay::Held,
            )
            .unwrap()
            .conn;
        assert_eq!(
            s.read(c).unwrap().text_by_root[0].1,
            format!("start\nc{j}\nc{}\n", 100 + j),
            "row: doc {j} holds its durable changes and the one after the rollback"
        );
    }
    m.release().unwrap();
    let _ = std::fs::remove_dir_all(&d);
}

/// An agent's join on `s`: its connection and its writer.
fn bot(s: &Session) -> (ConnId, crate::doc::ClientId) {
    let j = s
        .join(
            Participant::new("bot", "Bot", "#0a0a0a", ParticipantKind::Agent, "cli"),
            Stay::Held,
        )
        .unwrap();
    (j.conn, j.client)
}

/// The line `line` appended to `s`'s text by `c`.
fn append(s: &Session, c: ConnId, line: &str) -> EditResult {
    let at = s.read(c).unwrap().text_by_root[0].1.len();
    s.edit(
        c,
        vec![Edit::Text {
            root: ROOT.into(),
            hunks: vec![Hunk {
                start: at,
                end: at,
                insert: format!("{line}\n"),
            }],
        }],
        None,
    )
    .unwrap()
}

const LOST_CHILD: &str = "K2MB_LOST_CHILD";

/// The kill arm's child: doc `K2MB_LOST_DOC` of [`docs`], an agent's changes until it is
/// killed, each acknowledged on stdout with its version and whether it was durable.
fn lost_row_child(d: &Path) {
    let (k, opts) = docs().swap_remove(env("K2MB_LOST_DOC", 0) as usize);
    let h = host(1_000_000);
    let store =
        Arc::new(DocStore::open_with(d.join("docs.redb"), grouped_as(1024, 10_000)).unwrap());
    let m = manager(&store, &h);
    let s = m
        .open_with(Arc::new(MD), &k, Some(b"start\n"), &opts)
        .unwrap();
    let (c, client) = bot(&s);
    store.flush().unwrap();
    println!("LOST-CHILD READY {client}");
    for i in 0..1_000_000u64 {
        let r = append(&s, c, &format!("b{i}"));
        println!("LOST-CHILD ACK {} {}", r.version, u8::from(r.durable));
    }
}

/// A group lost, `arm`: a kill, a power loss, or a commit failing at the disk in the process.
/// The agent's versions acknowledged before the loss, its writer then, and the store and its
/// manager after it (reopened, or rolled back in place).
struct AfterLoss {
    acked: Vec<crate::store::Seq>,
    client: crate::doc::ClientId,
    store: Arc<DocStore>,
    m: SessionManager,
    s: Arc<Session>,
}

fn lose_a_group(arm: &str, d: &Path, doc: usize, h: &Arc<H>) -> AfterLoss {
    use crate::store::power_loss::PowerLoss;
    let (k, opts) = docs().swap_remove(doc);
    let settings = grouped_as(1024, 10_000);
    let path = d.join("docs.redb");
    if arm == "kill" {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "session::grouped_commit_tests::a_lost_version_is_refused_and_no_id_is_reissued",
                "--exact",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(LOST_CHILD, d)
            .env("K2MB_LOST_DOC", doc.to_string())
            .stdout(std::process::Stdio::piped())
            .stderr(std::fs::File::create(d.join("child.err")).unwrap())
            .spawn()
            .unwrap();
        let out = BufReader::new(child.stdout.take().unwrap());
        let (mut acked, mut client, mut lagging) = (Vec::new(), None, 0);
        for line in out.lines() {
            let line = line.unwrap();
            let mut f = match line.find("LOST-CHILD ") {
                Some(at) => line[at + "LOST-CHILD ".len()..].split(' '),
                None => continue,
            };
            match f.next() {
                Some("READY") => client = f.next().and_then(|c| c.parse().ok()),
                Some("ACK") => {
                    acked.push(f.next().unwrap().parse().unwrap());
                    if f.next() == Some("0") {
                        lagging += 1;
                    }
                }
                _ => {}
            }
            if lagging >= 5 {
                break;
            }
        }
        child.kill().unwrap();
        let _ = child.wait();
        assert!(
            lagging >= 5,
            "control kill: the child acknowledged changes not yet durable ({acked:?}; its \
             stderr: {})",
            std::fs::read_to_string(d.join("child.err")).unwrap_or_default()
        );
        let store = Arc::new(DocStore::open_with(&path, settings).unwrap());
        let m = manager(&store, h);
        let s = m.open(Arc::new(MD), &k, None).unwrap();
        return AfterLoss {
            acked,
            client: client.expect("the child's writer"),
            store,
            m,
            s,
        };
    }
    let layer = PowerLoss::default();
    let store = Arc::new(
        DocStore::open_on_backend_for_tests(layer.clone(), path.clone(), settings).unwrap(),
    );
    let m = manager(&store, h);
    let s = m
        .open_with(Arc::new(MD), &k, Some(b"start\n"), &opts)
        .unwrap();
    let (c, client) = bot(&s);
    store.flush().unwrap();
    let mut acked = Vec::new();
    for i in 0..6 {
        acked.push(append(&s, c, &format!("b{i}")).version);
    }
    assert!(
        store.durable_through() < store.commits(),
        "control {arm}: the agent's changes wait for the disk"
    );
    if arm == "power" {
        let image = layer.lose_power(0, true);
        drop(s);
        drop(m);
        drop(store);
        let store = Arc::new(
            DocStore::open_on_backend_for_tests(PowerLoss::from_image(image), path, settings)
                .unwrap(),
        );
        let m = manager(&store, h);
        let s = m.open(Arc::new(MD), &k, None).unwrap();
        return AfterLoss {
            acked,
            client,
            store,
            m,
            s,
        };
    }
    // in the process: the group's commit fails at the disk, and the tick rolls back
    layer.fail_next_sync();
    assert!(
        m.flush().is_err(),
        "control {arm}: the flush failed at the disk"
    );
    m.tick().unwrap();
    assert!(
        s.read(c).is_err(),
        "row {arm}: the connection whose writer made the lost changes was closed"
    );
    AfterLoss {
        acked,
        client,
        store,
        m,
        s,
    }
}

/// Versions and writers after a lost group (`store/seq_floor.rs`), on each doc (a Yrs one,
/// and with the native engine a native one), after a kill, a power loss and a commit failing
/// at the disk in the process: an agent makes changes grouped at 1,024 changes and 10 s (the
/// first raises the doc's seq floor, durably; the rest wait), and the group is lost. After it,
/// the agent's join takes a fresh writer, never the one the lost changes had; a person's next
/// change takes a version past every one the agent was given; and a write at a lost version,
/// or the changes since one, are refused `MPC-E112` `base_expired` (exit 8). Plants:
/// `no-rotation-after-recovery` caught at the fresh-writer assert (or, in the process, at the
/// closed-connection assert), `seq-floor-ignored` at the write's `base_expired` assert. The
/// native engine's part (after a rollback, it refuses state from before the reload) waits for
/// the engine's re-vendor (no raise site in this build).
#[test]
fn a_lost_version_is_refused_and_no_id_is_reissued() {
    if let Ok(d) = std::env::var(LOST_CHILD) {
        lost_row_child(Path::new(&d));
        return;
    }
    let mut rows = 0;
    for doc in 0..docs().len() {
        for arm in ["kill", "power", "failed"] {
            let d = dir("lost");
            let h = host(1_000_000);
            let what = format!("{arm} doc {doc}");
            let a = lose_a_group(arm, &d, doc, &h);
            let (c, client) = bot(&a.s);
            assert_ne!(
                client, a.client,
                "row {what}: the agent's writer after the loss is fresh"
            );
            let s = &a.s;
            let ann = s
                .join(
                    Participant::new("ann", "Ann", "#123456", ParticipantKind::Person, "tab"),
                    Stay::Held,
                )
                .unwrap()
                .conn;
            let kept = a.s.read(ann).unwrap().version;
            let newest = *a.acked.iter().max().unwrap();
            assert!(
                kept < newest,
                "control {what}: the loss dropped some of the agent's versions ({kept} kept, \
                 {newest} given)"
            );
            let lost = kept + 1;
            let next = append(&a.s, ann, "after").version;
            let w = a.s.write_base(c, ROOT, lost, "start\nmine\n").unwrap();
            assert!(
                matches!(w, WriteOutcome::BaseExpired),
                "row {what}: a write at lost version {lost} is refused base_expired: {w:?}"
            );
            assert_eq!(w.exit(), 8, "row {what}: base_expired exits 8");
            let since = a.s.events_since(c, lost);
            assert!(
                since.as_ref().is_err_and(|e| e.id() == "MPC-E112"),
                "row {what}: the changes since lost version {lost} are refused MPC-E112: \
                 {since:?}"
            );
            assert!(
                next > newest,
                "row {what}: the next version ({next}) is past every one given ({newest})"
            );
            assert!(
                a.store.recovered().is_some_and(|r| r.lost_ranges > 0),
                "row {what}: the recovery recorded the lost range"
            );
            a.m.release().unwrap();
            drop(a);
            let _ = std::fs::remove_dir_all(&d);
            rows += 1;
        }
    }
    assert_eq!(
        rows,
        3 * docs().len(),
        "control: every arm ran on every doc"
    );
}

/// The test pack the integration tests share (`tests/index_pack`): `IDX` files, one line per
/// node, with its `--base` verbs.
#[path = "../../tests/index_pack/mod.rs"]
mod index_pack;

/// The blob doc the tier-0 rows write: four keyed nodes of [`index_pack`].
const IDX: &str = "IDX\n@n:1|layer|Walls|-|1|-|-|-\n@n:2|line|Door|0|1|-|door|-\n\
                   @n:3|line|Window|0|1|-|window|-\n@n:4|line|Wall|0|1|-|wall|-\n";

fn ix_pack() -> Arc<index_pack::IxPack> {
    Arc::new(index_pack::IxPack {
        verbs: index_pack::BASE_SET,
        merge: true,
        ..index_pack::IxPack::default()
    })
}

/// [`manager`] with a blob store under `d`.
fn blob_manager(d: &Path, store: &Arc<DocStore>, h: &Arc<H>) -> SessionManager {
    use crate::blob::{BlobConfig, BlobStore};
    let hc = h.clone();
    let blobs = BlobStore::open(
        store.clone(),
        d.join("blobs"),
        BlobConfig::default(),
        Arc::new(move || hc.now.load(Ordering::SeqCst)),
    )
    .unwrap();
    manager(store, h).with_blob_store(Arc::new(blobs))
}

/// Head indexed (the verbs read its index).
fn indexed(s: &BlobSession) {
    if let Some(j) = s.index_job(None).unwrap() {
        j.run().unwrap();
    }
}

fn blob_join(s: &BlobSession, pid: &str, kind: ParticipantKind) -> ConnId {
    s.join(
        Participant::new(pid, pid, "#123456", kind, "cli"),
        Stay::Held,
    )
    .unwrap()
    .conn
}

/// A verb call: the refusal's id (none: it landed), its exit, and the version it made.
fn blob_call(s: &BlobSession, c: ConnId, a: &[&str]) -> (Option<String>, u8, Option<u64>) {
    use crate::errors::Value;
    let argv: Vec<String> = a.iter().map(|x| (*x).to_owned()).collect();
    let o = s
        .run_tool(
            c,
            &argv,
            &crate::tools::ToolOptions::new().result_json(true),
        )
        .expect("with the result asked for, every outcome is a result");
    let r = o.result().expect("a result");
    let err = r
        .get("error")
        .and_then(|e| e.get("id"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let after = r
        .get("version")
        .and_then(|v| v.get("after"))
        .and_then(Value::as_u64);
    (err, o.exit, after)
}

/// Tier-0 versions after a lost group (`store/seq_floor.rs`, the blob space), after a power
/// loss and after a commit failing at the disk in the process: a blob doc with no file, of the
/// shared test pack; an agent's verbs grouped at 1,024 changes and 10 s (the first raises the
/// doc's blob floor, durably; the rest wait), and the group is lost. After it, the agent's verb
/// with `--base` at a lost version is refused `MPC-E112` `base_expired` (exit 8), and a
/// person's next change takes a version past every one the agent was given. Plant:
/// `blob-floor-ignored` caught at the `base_expired` assert (the person's change takes the lost
/// version again, so the base names head and is taken).
#[test]
fn a_lost_blob_version_is_refused_and_no_blob_version_is_reissued() {
    use crate::store::power_loss::PowerLoss;
    let mut rows = 0;
    for arm in ["power", "failed"] {
        let d = dir("blob-lost");
        let h = host(1_000_000);
        let settings = grouped_as(1024, 10_000);
        let path = d.join("docs.redb");
        let k = key("a.idx");
        let layer = PowerLoss::default();
        let store = Arc::new(
            DocStore::open_on_backend_for_tests(layer.clone(), path.clone(), settings).unwrap(),
        );
        let m = blob_manager(&d, &store, &h);
        let mut bytes = IDX.as_bytes();
        let s = m.open_blob(ix_pack(), &k, Some(&mut bytes)).unwrap();
        let bot = blob_join(&s, "bot", ParticipantKind::Agent);
        store.flush().unwrap();
        let mut given = Vec::new();
        for i in 0..6 {
            indexed(&s);
            let note = format!("a{i}");
            let (err, _, after) = blob_call(&s, bot, &["part", "set", "@n:3", "note", &note]);
            assert_eq!(err, None, "control {arm}: the agent's change {i} lands");
            given.push(after.expect("control: the version the change made"));
        }
        assert!(
            store.durable_through() < store.commits(),
            "control {arm}: the agent's changes wait for the disk"
        );
        let (store, m, s) = if arm == "power" {
            let image = layer.lose_power(0, true);
            drop(s);
            drop(m);
            drop(store);
            let store = Arc::new(
                DocStore::open_on_backend_for_tests(PowerLoss::from_image(image), path, settings)
                    .unwrap(),
            );
            let m = blob_manager(&d, &store, &h);
            let s = m.open_blob(ix_pack(), &k, None).unwrap();
            (store, m, s)
        } else {
            // in the process: the group's commit fails at the disk, the tick rolls back and
            // closes the tier-0 session, and the next open reloads it
            layer.fail_next_sync();
            assert!(
                m.flush().is_err(),
                "control {arm}: the flush failed at the disk"
            );
            m.tick().unwrap();
            let again = m
                .open_blob(ix_pack(), &k, None)
                .unwrap_or_else(|e| panic!("row {arm}: the doc reopens after the rollback: {e}"));
            assert!(
                !Arc::ptr_eq(&s, &again),
                "control {arm}: the rollback closed the tier-0 session"
            );
            (store, m, again)
        };
        let bot = blob_join(&s, "bot", ParticipantKind::Agent);
        let ann = blob_join(&s, "ann", ParticipantKind::Person);
        let kept = s.status().unwrap().version;
        let newest = *given.iter().max().unwrap();
        assert!(
            kept < newest,
            "control {arm}: the loss dropped some of the agent's versions ({kept} kept, \
             {newest} given)"
        );
        let lost = kept + 1;
        let mine = IDX.replace("wall|-", "wall|p");
        let next = s.write_bytes(ann, mine.as_bytes(), kept).unwrap().version;
        indexed(&s);
        let base = lost.to_string();
        let (err, exit, _) = blob_call(
            &s,
            bot,
            &["part", "set", "@n:2", "note", "y", "--base", &base],
        );
        assert_eq!(
            err.as_deref(),
            Some("MPC-E112"),
            "row {arm}: a verb at lost blob version {lost} is refused base_expired (head {next})"
        );
        assert_eq!(exit, 8, "row {arm}: base_expired exits 8");
        assert!(
            next > newest,
            "row {arm}: the next blob version ({next}) is past every one given ({newest})"
        );
        assert!(
            store.recovered().is_some_and(|r| r.lost_ranges > 0),
            "row {arm}: the recovery recorded the lost range"
        );
        m.release().unwrap();
        drop(s);
        drop(m);
        drop(store);
        let _ = std::fs::remove_dir_all(&d);
        rows += 1;
    }
    assert_eq!(rows, 2, "control: both arms ran");
}
