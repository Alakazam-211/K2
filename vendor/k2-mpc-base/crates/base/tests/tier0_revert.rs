//! Reverting one version of a blob doc, and a participant's own undo and redo
//! (`docs/SESSIONS.md`, "Revert, undo and redo on a blob doc"): `BlobSession::run_revert`.
//! The file type's own merge takes version K's change away from head (base K, ours head,
//! theirs K-1); a node changed since K in a way that overlaps refuses, naming who changed it.
//! On the index test pack ([`index_pack`]) with its line merge. Public API only.
//!
//! Each row names the plant (`MPC_T0V_MUTANT`) it catches; the plants part runs each alone.

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

mod index_pack;
use index_pack::{IxPack, BATCH_SET};

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::errors::Value;
use k2_mpc_base::index::NodeKey;
use k2_mpc_base::session::{
    AuditEvent, BatchCall, BlobSession, ClaimPolicy, Config, ConnId, Host, Participant,
    ParticipantKind, RevertWhat, Rights, SessionManager, Stay,
};
use k2_mpc_base::store::{CommitDurability, DocKey, DocStore, StoreSettings, VersionId};
use k2_mpc_base::tools::{ToolOptions, ToolOutput};

struct TestHost {
    now: AtomicU64,
    rights: Mutex<HashMap<String, Rights>>,
}

impl Host for TestHost {
    fn rights(&self, who: &Participant, _key: &DocKey) -> Rights {
        self.rights
            .lock()
            .unwrap()
            .get(&who.pid)
            .copied()
            .unwrap_or(Rights::none())
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, _ev: &AuditEvent) {}
}

struct Fx {
    dir: PathBuf,
    ws: PathBuf,
    host: Arc<TestHost>,
    mgr: SessionManager,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn fx(tag: &str) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-t0revert-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("ws")).unwrap();
    let dir = fs::canonicalize(dir).unwrap();
    let ws = dir.join("ws");
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_000_000),
        rights: Mutex::new(HashMap::new()),
    });
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let h = host.clone();
    let blobs = Arc::new(
        BlobStore::open(
            store.clone(),
            dir.join("blobs"),
            BlobConfig::default(),
            Arc::new(move || h.now_ms()),
        )
        .unwrap(),
    );
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    let mgr = SessionManager::new(store, host.clone(), c).with_blob_store(blobs);
    Fx { dir, ws, host, mgr }
}

fn key(p: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: p.into(),
    }
}

/// The batch verbs and the pack's line merge.
fn pack() -> IxPack {
    IxPack {
        verbs: BATCH_SET,
        merge: true,
        line_merge: true,
        ..IxPack::default()
    }
}

/// A layer (`@n:1`) and three lines.
const DOC: &str = "IDX\n@n:1|layer|Walls|-|1|-|-|-\n@n:2|line|Door|-|1|-|door|-\n\
                   @n:3|line|Window|-|1|-|window|-\n@n:4|line|Wall|-|1|-|wall|-\n";

const NOTE: usize = 7;

fn index(s: &BlobSession) {
    if let Some(j) = s.index_job(None).unwrap() {
        j.run().unwrap();
    }
}

fn head(s: &BlobSession) -> VersionId {
    s.status().unwrap().version
}

impl Fx {
    fn open_with(&self, name: &str, bytes: &str, pack: IxPack) -> Arc<BlobSession> {
        fs::write(self.ws.join(name), bytes).unwrap();
        let s = self
            .mgr
            .open_blob_file(Arc::new(pack), &key(name), &self.ws.join(name), &self.ws)
            .unwrap();
        index(&s);
        s
    }
    fn open(&self, name: &str) -> Arc<BlobSession> {
        self.open_with(name, DOC, pack())
    }
    /// A participant with edit rights; `owner` adds the owner's.
    fn join_as(&self, s: &BlobSession, pid: &str, kind: ParticipantKind, owner: bool) -> ConnId {
        self.host
            .rights
            .lock()
            .unwrap()
            .insert(pid.into(), Rights::full().with_owner(owner));
        s.join(
            Participant::new(pid, pid, "#123456", kind, "cli"),
            Stay::Held,
        )
        .unwrap()
        .conn
    }
    fn agent(&self, s: &BlobSession, pid: &str) -> ConnId {
        self.join_as(s, pid, ParticipantKind::Agent, false)
    }
    /// Another program writes `name`; the host's watcher reports it; the version is indexed.
    fn outside(&self, s: &BlobSession, name: &str, bytes: &str) {
        self.host.now.fetch_add(1_000, Ordering::SeqCst);
        fs::write(self.ws.join(name), bytes).unwrap();
        s.file_changed().unwrap();
        index(s);
    }
}

fn argv(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| (*s).to_owned()).collect()
}

/// A verb with its result as data; the version it made indexed (its own seed does that).
fn call(s: &BlobSession, c: ConnId, a: &[&str]) -> ToolOutput {
    s.run_tool(c, &argv(a), &ToolOptions::new().result_json(true))
        .expect("with the result asked for, every outcome is a result")
}

fn revert_with(
    s: &BlobSession,
    c: ConnId,
    what: RevertWhat,
    flags: &[&str],
    opts: ToolOptions,
) -> ToolOutput {
    s.run_revert(c, what, &argv(flags), &opts.result_json(true))
        .expect("with the result asked for, every outcome is a result")
}

fn revert(s: &BlobSession, c: ConnId, what: RevertWhat) -> ToolOutput {
    revert_with(s, c, what, &[], ToolOptions::new())
}

fn res(o: &ToolOutput) -> &Value {
    o.result().expect("a result")
}

fn err_id(o: &ToolOutput) -> Option<String> {
    res(o)
        .get("error")
        .and_then(|e| e.get("id"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn detail<'a>(o: &'a ToolOutput, k: &str) -> Option<&'a Value> {
    res(o)
        .get("error")
        .and_then(|e| e.get("details"))
        .and_then(|d| d.get(k))
}

fn bytes_at(s: &BlobSession, c: ConnId, v: VersionId) -> Vec<u8> {
    let mut r = s.read_version(c, v).unwrap();
    let mut out = Vec::new();
    r.reader.read_to_end(&mut out).unwrap();
    out
}

/// One field of a line at head, by its key.
fn field(s: &BlobSession, c: ConnId, k: &str, col: usize) -> Option<String> {
    let b = bytes_at(s, c, head(s));
    String::from_utf8(b)
        .unwrap()
        .lines()
        .find(|l| l.split('|').next() == Some(k))
        .and_then(|l| l.split('|').nth(col).map(str::to_owned))
}

fn note(s: &BlobSession, c: ConnId, k: &str) -> Option<String> {
    field(s, c, k, NOTE)
}

/// Head's log entry: its verb and the version it reverts.
fn head_entry(s: &BlobSession, c: ConnId) -> (Option<String>, Option<VersionId>) {
    let h = head(s);
    s.log(c)
        .unwrap()
        .into_iter()
        .find(|e| e.id == h)
        .map(|e| (e.verb, e.reverts))
        .unwrap_or((None, None))
}

/// revert_one_version: version K changed two nodes (a batch), a later version changed a third.
/// Reverting K puts the two back and keeps the third; the version is the reverter's, with the
/// verb `revert` and `reverts: K`, and its result's data names K.
/// Plant: `revert-takes-whole-version`.
#[test]
fn revert_one_version() {
    let f = fx("one");
    let s = f.open("a.idx");
    let bot = f.agent(&s, "bot");
    let ann = f.join_as(&s, "ann", ParticipantKind::Person, false);
    let calls = vec![
        BatchCall::new(argv(&["part", "set", "@n:2", "note", "a"])),
        BatchCall::new(argv(&["part", "set", "@n:3", "note", "b"])),
    ];
    let b = s
        .run_batch(bot, &[], &calls, &ToolOptions::new().result_json(true))
        .unwrap();
    assert!(
        err_id(&b).is_none(),
        "the batch lands: {}",
        res(&b).to_json()
    );
    let k = head(&s);
    let later = call(&s, ann, &["part", "set", "@n:4", "note", "c"]);
    assert!(err_id(&later).is_none(), "the later change lands");
    let o = revert(&s, bot, RevertWhat::Version(k));
    let data = res(&o)
        .get("data")
        .and_then(|d| d.get("reverts"))
        .and_then(Value::as_u64);
    assert_eq!(
        (
            err_id(&o),
            [
                note(&s, bot, "@n:2"),
                note(&s, bot, "@n:3"),
                note(&s, bot, "@n:4")
            ],
            head_entry(&s, bot),
            data
        ),
        (
            None,
            [
                Some("-".to_owned()),
                Some("-".to_owned()),
                Some("c".to_owned())
            ],
            (Some("revert".to_owned()), Some(k)),
            Some(k)
        ),
        "revert_one_version: K's change is taken away, a later change kept ({})",
        res(&o).to_json()
    );
}

/// revert_conflict_names_who: a later version changed the same field K changed: refused
/// (`MPC-E731`), naming who changed it and in which version; nothing written.
/// Plant: `revert-overwrites`.
#[test]
fn revert_conflict_names_who() {
    let f = fx("conflict");
    let s = f.open("a.idx");
    let bot = f.agent(&s, "bot");
    let ann = f.join_as(&s, "ann", ParticipantKind::Person, false);
    call(&s, bot, &["part", "set", "@n:2", "note", "a"]);
    let k = head(&s);
    call(&s, ann, &["part", "set", "@n:2", "note", "z"]);
    let h = head(&s);
    let o = revert(&s, bot, RevertWhat::Version(k));
    let first = match res(&o).get("conflicts") {
        Some(Value::List(l)) => l.first().map(|c| {
            (
                c.get("key").and_then(Value::as_str).map(str::to_owned),
                c.get("by").and_then(Value::as_str).map(str::to_owned),
                c.get("change").and_then(Value::as_u64),
            )
        }),
        _ => None,
    };
    assert_eq!(
        (err_id(&o), first, head(&s), note(&s, bot, "@n:2")),
        (
            Some("MPC-E731".to_owned()),
            Some((Some("@n:2".to_owned()), Some("ann".to_owned()), Some(h))),
            h,
            Some("z".to_owned())
        ),
        "revert_conflict_names_who: a field changed since K refuses, naming who ({})",
        res(&o).to_json()
    );
}

/// revert_needs_author_or_owner: an agent may revert its own version; another participant
/// without the owner's rights may not (`author_or_owner`, nothing written); an owner may.
/// Plant: `revert-ignores-author`.
#[test]
fn revert_needs_author_or_owner() {
    let f = fx("rights");
    let s = f.open("a.idx");
    let bot = f.agent(&s, "bot");
    let eve = f.agent(&s, "eve");
    let own = f.join_as(&s, "own", ParticipantKind::Person, true);
    call(&s, bot, &["part", "set", "@n:2", "note", "a"]);
    let k = head(&s);
    let other = revert(&s, eve, RevertWhat::Version(k));
    let after_other = head(&s);
    let owner = revert(&s, own, RevertWhat::Version(k));
    assert_eq!(
        (
            err_id(&other).is_some(),
            detail(&other, "need")
                .and_then(Value::as_str)
                .map(str::to_owned),
            after_other,
            err_id(&owner),
            note(&s, bot, "@n:2")
        ),
        (
            true,
            Some("author_or_owner".to_owned()),
            k,
            None,
            Some("-".to_owned())
        ),
        "revert_needs_author_or_owner: only the version's author or an owner reverts it ({})",
        res(&other).to_json()
    );
}

/// undo_own_only: two agents interleave; one's undo reverts its own latest change, never the
/// other's.
/// Plant: `undo-latest-any`.
#[test]
fn undo_own_only() {
    let f = fx("own");
    let s = f.open("a.idx");
    let bot = f.agent(&s, "bot");
    let cy = f.agent(&s, "cy");
    call(&s, bot, &["part", "set", "@n:2", "note", "a"]);
    let mine = head(&s);
    call(&s, cy, &["part", "set", "@n:3", "note", "b"]);
    let o = revert(&s, bot, RevertWhat::Undo);
    assert_eq!(
        (
            err_id(&o),
            note(&s, bot, "@n:2"),
            note(&s, bot, "@n:3"),
            head_entry(&s, bot)
        ),
        (
            None,
            Some("-".to_owned()),
            Some("b".to_owned()),
            (Some("undo".to_owned()), Some(mine))
        ),
        "undo_own_only: an undo reverts the caller's own latest change ({})",
        res(&o).to_json()
    );
}

/// undo_redo_roundtrip: a change, its undo, then redo: the bytes are the change's again, in
/// two more versions (`undo`, then `redo`).
#[test]
fn undo_redo_roundtrip() {
    let f = fx("roundtrip");
    let s = f.open("a.idx");
    let bot = f.agent(&s, "bot");
    call(&s, bot, &["part", "set", "@n:2", "note", "a"]);
    let v = head(&s);
    let changed = bytes_at(&s, bot, v);
    let u = revert(&s, bot, RevertWhat::Undo);
    let undone = (err_id(&u), note(&s, bot, "@n:2"));
    let r = revert(&s, bot, RevertWhat::Redo);
    assert_eq!(
        (
            undone,
            err_id(&r),
            bytes_at(&s, bot, head(&s)) == changed,
            head(&s),
            head_entry(&s, bot).0
        ),
        (
            (None, Some("-".to_owned())),
            None,
            true,
            v + 2,
            Some("redo".to_owned())
        ),
        "undo_redo_roundtrip: undo then redo gives the change's bytes back ({})",
        res(&r).to_json()
    );
}

/// redo_cleared_by_new_edit: a change, its undo, a new change: there is nothing to redo (exit
/// 1, nothing written), and the next undo reverts the new change.
/// Plant: `redo-not-cleared`.
#[test]
fn redo_cleared_by_new_edit() {
    let f = fx("cleared");
    let s = f.open("a.idx");
    let bot = f.agent(&s, "bot");
    call(&s, bot, &["part", "set", "@n:2", "note", "a"]);
    revert(&s, bot, RevertWhat::Undo);
    call(&s, bot, &["part", "set", "@n:3", "note", "b"]);
    let h = head(&s);
    let r = revert(&s, bot, RevertWhat::Redo);
    let after_redo = head(&s);
    let u = revert(&s, bot, RevertWhat::Undo);
    assert_eq!(
        (
            r.exit,
            err_id(&r),
            after_redo,
            err_id(&u),
            note(&s, bot, "@n:2"),
            note(&s, bot, "@n:3")
        ),
        (1, None, h, None, Some("-".to_owned()), Some("-".to_owned())),
        "redo_cleared_by_new_edit: a new change clears redo; undo takes the new change ({})",
        res(&r).to_json()
    );
}

/// undo_of_batch: a batch's undo reverts the whole batch, as one version.
#[test]
fn undo_of_batch() {
    let f = fx("batch");
    let s = f.open("a.idx");
    let bot = f.agent(&s, "bot");
    let calls = vec![
        BatchCall::new(argv(&["part", "set", "@n:2", "note", "a"])),
        BatchCall::new(argv(&["part", "set", "@n:3", "note", "b"])),
    ];
    s.run_batch(bot, &[], &calls, &ToolOptions::new().result_json(true))
        .unwrap();
    let v = head(&s);
    let o = revert(&s, bot, RevertWhat::Undo);
    assert_eq!(
        (
            err_id(&o),
            note(&s, bot, "@n:2"),
            note(&s, bot, "@n:3"),
            head(&s)
        ),
        (None, Some("-".to_owned()), Some("-".to_owned()), v + 1),
        "undo_of_batch: one undo reverts a batch's version whole ({})",
        res(&o).to_json()
    );
}

/// revert_unproven_refused: another program saved the file since K and the pack doesn't prove
/// it keeps keys: `MPC-E719`, its `base` K-1, nothing written.
#[test]
fn revert_unproven_refused() {
    let f = fx("unproven");
    let s = f.open("a.idx");
    let bot = f.agent(&s, "bot");
    call(&s, bot, &["part", "set", "@n:2", "note", "a"]);
    let k = head(&s);
    let now = String::from_utf8(bytes_at(&s, bot, k)).unwrap();
    f.outside(&s, "a.idx", &now.replace("window|-", "window|saved"));
    let h = head(&s);
    let o = revert(&s, bot, RevertWhat::Version(k));
    assert_eq!(
        (
            err_id(&o),
            detail(&o, "base").and_then(Value::as_u64),
            head(&s)
        ),
        (Some("MPC-E719".to_owned()), Some(k - 1), h),
        "revert_unproven_refused: a revert across another program's unproven save refuses ({})",
        res(&o).to_json()
    );
}

/// revert_dry_run_matches_real: a dry run's change is the real run's, and it writes nothing.
#[test]
fn revert_dry_run_matches_real() {
    let f = fx("dry");
    let s = f.open("a.idx");
    let bot = f.agent(&s, "bot");
    call(&s, bot, &["part", "set", "@n:2", "note", "a"]);
    let k = head(&s);
    let dry = revert_with(
        &s,
        bot,
        RevertWhat::Version(k),
        &["--dry-run"],
        ToolOptions::new(),
    );
    let after_dry = head(&s);
    let real = revert(&s, bot, RevertWhat::Version(k));
    let units = |o: &ToolOutput| {
        res(o)
            .get("changed")
            .and_then(|c| c.get("units"))
            .map(Value::to_json)
    };
    assert_eq!(
        (
            err_id(&dry),
            after_dry,
            units(&dry) == units(&real) && units(&dry).is_some(),
            head(&s)
        ),
        (None, k, true, k + 1),
        "revert_dry_run_matches_real: a dry run shows the real revert and writes nothing"
    );
}

/// revert_held_refused: under Respect, an agent's revert reaching a node another participant
/// holds is refused (`MPC-E301`); a person's is not.
/// Plant: `revert-skips-claims`.
#[test]
fn revert_held_refused() {
    let f = fx("held");
    let s = f.open("a.idx");
    let bot = f.agent(&s, "bot");
    let ann = f.join_as(&s, "ann", ParticipantKind::Person, false);
    let own = f.join_as(&s, "own", ParticipantKind::Person, true);
    call(&s, bot, &["part", "set", "@n:2", "note", "a"]);
    let k = head(&s);
    s.claim_nodes(ann, &[NodeKey::parse("@n:2").unwrap()], None, None)
        .unwrap();
    let respect = || ToolOptions::new().policy(ClaimPolicy::Respect);
    let agent = revert_with(&s, bot, RevertWhat::Version(k), &[], respect());
    let person = revert_with(&s, own, RevertWhat::Version(k), &[], respect());
    assert_eq!(
        (err_id(&agent), err_id(&person), note(&s, bot, "@n:2")),
        (Some("MPC-E301".to_owned()), None, Some("-".to_owned())),
        "revert_held_refused: an agent's revert into a held node is refused; a person's lands"
    );
}

/// revert_unsupported: a file type that can't merge can't revert one version (`MPC-E730`,
/// its next step `restore`); nothing written.
#[test]
fn revert_unsupported() {
    let f = fx("unsupported");
    let s = f.open_with(
        "a.idx",
        DOC,
        IxPack {
            merge: false,
            line_merge: false,
            verbs: &[],
            ..pack()
        },
    );
    let bot = f.join_as(&s, "bot", ParticipantKind::Person, true);
    let w = s.write_bytes(bot, DOC.replace("door|-", "door|a").as_bytes(), head(&s));
    assert!(w.is_ok(), "the write lands");
    let k = head(&s);
    let o = revert(&s, bot, RevertWhat::Version(k));
    let next = res(&o)
        .get("error")
        .and_then(|e| e.get("next_step"))
        .map(Value::to_json)
        .unwrap_or_default();
    assert_eq!(
        (err_id(&o), next.contains("restore"), head(&s)),
        (Some("MPC-E730".to_owned()), true, k),
        "revert_unsupported: no merge, no revert of one version; restore is the next step ({})",
        res(&o).to_json()
    );
}

/// revert_stray_refused: a merge that changes a node K didn't change is refused (`MPC-E726`),
/// nothing written.
#[test]
fn revert_stray_refused() {
    let f = fx("stray");
    let s = f.open_with(
        "a.idx",
        DOC,
        IxPack {
            merge_strays: true,
            ..pack()
        },
    );
    let bot = f.agent(&s, "bot");
    call(&s, bot, &["part", "set", "@n:2", "note", "a"]);
    let k = head(&s);
    let o = revert(&s, bot, RevertWhat::Version(k));
    assert_eq!(
        (err_id(&o), head(&s)),
        (Some("MPC-E726".to_owned()), k),
        "revert_stray_refused: a revert may change only what K changed ({})",
        res(&o).to_json()
    );
}

/// revert_flags: `--base` is refused on a revert, and so is the first version (nothing is
/// before it): both usage (exit 2), nothing written.
#[test]
fn revert_flags() {
    let f = fx("flags");
    let s = f.open("a.idx");
    let bot = f.agent(&s, "bot");
    call(&s, bot, &["part", "set", "@n:2", "note", "a"]);
    let k = head(&s);
    let based = revert_with(
        &s,
        bot,
        RevertWhat::Version(k),
        &["--base", &k.to_string()],
        ToolOptions::new(),
    );
    let first = revert(&s, bot, RevertWhat::Version(1));
    let says = |o: &ToolOutput, what: &str| {
        res(o)
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(Value::as_str)
            .is_some_and(|m| m.contains(what))
    };
    assert_eq!(
        (
            based.exit,
            says(&based, "--base"),
            first.exit,
            says(&first, "version 1"),
            head(&s)
        ),
        (2, true, 2, true, k),
        "revert_flags: --base and the first version are usage errors, each saying why"
    );
}

/// This build's store in `K2MB_N1_OUT` (docs.redb, blobs/), as the N−1 halves use it.
fn n1_store() -> (Arc<DocStore>, SessionManager, Arc<TestHost>) {
    let dir = PathBuf::from(std::env::var("K2MB_N1_OUT").expect("K2MB_N1_OUT"));
    fs::create_dir_all(&dir).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_793_000_000_000),
        rights: Mutex::new(HashMap::new()),
    });
    host.rights
        .lock()
        .unwrap()
        .insert("ann".into(), Rights::full().with_owner(true));
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let h = host.clone();
    let blobs = Arc::new(
        BlobStore::open(
            store.clone(),
            dir.join("blobs"),
            BlobConfig::default(),
            Arc::new(move || h.now_ms()),
        )
        .unwrap(),
    );
    let mgr =
        SessionManager::new(store.clone(), host.clone(), Config::default()).with_blob_store(blobs);
    (store, mgr, host)
}

fn n1_join(s: &BlobSession) -> ConnId {
    s.join(
        Participant::new("ann", "ann", "#123456", ParticipantKind::Person, "cli"),
        Stay::Held,
    )
    .unwrap()
    .conn
}

/// tools/n1 `node-revert`: writes the store the previous release's consumer reads: one blob
/// doc (`revert.idx`) with the import, a verb's version (2) and its revert (3, a record
/// version 2).
#[test]
#[ignore = "tools/n1/n1.sh: writes the store for the previous release's consumer"]
fn n1_revert_producer() {
    let (_, mgr, _) = n1_store();
    let s = mgr
        .open_blob(
            Arc::new(pack()),
            &key("revert.idx"),
            Some(&mut &b"IDX\n@n:1|k|a|-|1|-|-|-\n"[..]),
        )
        .unwrap();
    index(&s);
    let c = n1_join(&s);
    let o = call(&s, c, &["part", "set", "@n:1", "note", "x"]);
    assert_eq!(o.exit, 0, "the verb lands: {}", res(&o).to_json());
    let r = revert(&s, c, RevertWhat::Version(2));
    assert_eq!(r.exit, 0, "the revert lands: {}", res(&r).to_json());
    mgr.release().unwrap();
    println!("N-1 REVERT (producer) OK");
}

/// tools/n1 `node-revert`, after the previous release read the versions (the revert's as no
/// verb), wrote one and rewrote the store: this build reads the revert's record back whole,
/// beside the previous release's version with none.
#[test]
#[ignore = "tools/n1/n1.sh: reads the store the previous release's consumer left"]
fn n1_revert_after() {
    let (_, mgr, _) = n1_store();
    let s = mgr
        .open_blob(Arc::new(pack()), &key("revert.idx"), None)
        .unwrap();
    let c = n1_join(&s);
    let got: Vec<(Option<String>, Option<VersionId>)> = s
        .log(c)
        .unwrap()
        .into_iter()
        .map(|e| (e.verb, e.reverts))
        .collect();
    mgr.release().unwrap();
    assert_eq!(
        got,
        vec![
            (None, None),
            (Some("part set".to_owned()), None),
            (Some("revert".to_owned()), Some(2)),
            (None, None)
        ],
        "n1_revert_after: the revert's record read back after the previous release's rewrite"
    );
    println!("N-1 REVERT (this build) OK");
}

/// a_revert_commit_takes_the_group_durability: a revert's commits go through the store's
/// group like any other (grouped commits). On a store grouped at 1,024 changes and 10 s, a doc
/// with no file (nothing outside the store depends on it): the revert's commits are
/// acknowledged and wait for the disk, and the manager's flush makes them durable. At
/// `Immediate` (the default) the revert is durable when it returns.
#[test]
fn a_revert_commit_takes_the_group_durability() {
    let grouped = StoreSettings::DEFAULT
        .with_commit_durability(CommitDurability::grouped(1024, 10_000).unwrap());
    for (arm, settings) in [("grouped", grouped), ("immediate", StoreSettings::DEFAULT)] {
        let f = fx(&format!("group-{arm}"));
        let store = Arc::new(DocStore::open_with(f.dir.join("grouped.redb"), settings).unwrap());
        let h = f.host.clone();
        let blobs = Arc::new(
            BlobStore::open(
                store.clone(),
                f.dir.join("grouped-blobs"),
                BlobConfig::default(),
                Arc::new(move || h.now_ms()),
            )
            .unwrap(),
        );
        let mut c = Config::default();
        c.edit_rate_per_s = 1_000_000;
        c.edit_burst = 1_000_000;
        let mgr = SessionManager::new(store.clone(), f.host.clone(), c).with_blob_store(blobs);
        let s = mgr
            .open_blob(Arc::new(pack()), &key("g.idx"), Some(&mut DOC.as_bytes()))
            .unwrap();
        index(&s);
        let bot = f.agent(&s, "bot");
        let o = call(&s, bot, &["part", "set", "@n:2", "note", "a"]);
        assert!(err_id(&o).is_none(), "control {arm}: the change lands");
        let k = head(&s);
        mgr.flush().unwrap();
        let before = store.commits();
        let o = revert(&s, bot, RevertWhat::Version(k));
        assert_eq!(
            (err_id(&o), note(&s, bot, "@n:2")),
            (None, Some("-".to_owned())),
            "control {arm}: the revert lands ({})",
            res(&o).to_json()
        );
        assert!(
            store.commits() > before,
            "control {arm}: the revert committed"
        );
        if arm == "grouped" {
            assert_eq!(
                store.durable_through(),
                before,
                "row grouped: the revert's commits are acknowledged and wait for the disk"
            );
            mgr.flush().unwrap();
        }
        assert_eq!(
            store.durable_through(),
            store.commits(),
            "row {arm}: the revert's commits are durable (grouped: after the flush)"
        );
        mgr.release().unwrap();
    }
}
