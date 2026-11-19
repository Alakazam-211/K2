//! A hold binds an agent's whole-file write (`docs/SESSIONS.md`, "Claims on nodes"): under
//! `ClaimPolicy::Respect` (`BlobWriteOptions::policy`), while others hold nodes, an agent's
//! `write` and `expect_save` are read with the file type's index and refused when they change a
//! node inside a hold's reach (`MPC-E301`), or when that index can't read them within its
//! limits (`MPC-E306`). Without holds, under `Ignore`, and for a person, writes are as before.
//! On the index test pack ([`index_pack`]). Public API only.
//!
//! Each row names the plant (`MPC_T0V_MUTANT`) it catches; the plants part runs each alone.

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

mod index_pack;
use index_pack::{index_runs, IxPack};

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::index::NodeKey;
use k2_mpc_base::session::{
    AuditEvent, BlobSession, BlobWriteOptions, ClaimPolicy, Config, ConnId, HoldReach, Host,
    Participant, ParticipantKind, Rights, SessionError, SessionManager, Stay,
};
use k2_mpc_base::store::{DocKey, DocStore, VersionId};

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

fn fx(tag: &str, tweak: impl FnOnce(&mut Config)) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-t0hw-{tag}-{}-{}",
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
    tweak(&mut c);
    let mgr = SessionManager::new(store, host.clone(), c).with_blob_store(blobs);
    Fx { dir, ws, host, mgr }
}

fn key(p: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: p.into(),
    }
}

/// The pack: a hold on a `layer` reaches the nodes whose `tag` is its key, one on a `block`
/// its subtree, any other the node itself.
fn pack() -> IxPack {
    IxPack {
        reach: &[
            ("layer", HoldReach::Members("tag")),
            ("block", HoldReach::Subtree),
        ],
        ..IxPack::default()
    }
}

/// A layer with one member (`@n:2`), a line on no layer (`@n:3`), and a block (`@n:5`) holding
/// a step (`@n:6`).
const DOC: &str = "IDX\n@n:1|layer|Walls|-|1|-|-|-\n@n:2|line|Door|-|1|-|door|-|@n:1\n\
                   @n:3|line|Window|-|1|-|window|-\n@n:5|block|Stair|-|1|-|stair|-\n\
                   @n:6|line|Step|3|1|-|step|-\n";

/// `DOC` with node `k`'s content field set to `content`.
fn changed(k: &str, content: &str) -> String {
    let mut out = String::new();
    for line in DOC.lines() {
        let mut f: Vec<&str> = line.split('|').collect();
        if f.first() == Some(&k) {
            f[6] = content;
        }
        out.push_str(&f.join("|"));
        out.push('\n');
    }
    out
}

/// `DOC` with a new line `@n:7` on the layer.
fn with_member() -> String {
    format!("{DOC}@n:7|line|Late|-|1|-|late|-|@n:1\n")
}

/// `DOC` with `n` more lines, each its own node.
fn with_lines(n: usize) -> String {
    let mut out = DOC.to_owned();
    for i in 0..n {
        out.push_str(&format!("@n:{}|line|Extra {i}|-|1|-|x{i}|-\n", 100 + i));
    }
    out
}

fn index(s: &BlobSession) {
    if let Some(j) = s.index_job(None).unwrap() {
        j.run().unwrap();
    }
}

fn head(s: &BlobSession) -> VersionId {
    s.status().unwrap().version
}

impl Fx {
    /// `name` on disk holding `bytes`, opened and indexed.
    fn open(&self, name: &str, bytes: &str, pack: IxPack) -> Arc<BlobSession> {
        fs::write(self.ws.join(name), bytes).unwrap();
        let s = self
            .mgr
            .open_blob_file(Arc::new(pack), &key(name), &self.ws.join(name), &self.ws)
            .unwrap();
        index(&s);
        s
    }
    fn join(&self, s: &BlobSession, pid: &str, kind: ParticipantKind) -> ConnId {
        self.host
            .rights
            .lock()
            .unwrap()
            .insert(pid.into(), Rights::full().with_owner(true));
        s.join(
            Participant::new(pid, pid, "#123456", kind, "cli"),
            Stay::Held,
        )
        .unwrap()
        .conn
    }
}

/// Hold the nodes `keys` as `c`.
fn hold(s: &BlobSession, c: ConnId, keys: &[&str]) {
    let keys: Vec<NodeKey> = keys.iter().map(|k| NodeKey::parse(k).unwrap()).collect();
    s.claim_nodes(c, &keys, None, None).unwrap();
}

fn respect() -> BlobWriteOptions {
    BlobWriteOptions::new().policy(ClaimPolicy::Respect)
}

/// How a whole-file call ended: `Ok("landed")`, or the holder an `MPC-E301` names, or the
/// error's id.
fn outcome<T>(r: Result<T, SessionError>) -> String {
    match r {
        Ok(_) => "landed".to_owned(),
        Err(SessionError::NodesClaimed(c)) => format!("held by {}", c.by),
        Err(e) => e.report().id().to_owned(),
    }
}

/// An agent's write from memory, at head, with `opts`.
fn write_bytes(s: &BlobSession, c: ConnId, bytes: &str, opts: &BlobWriteOptions) -> String {
    outcome(s.write_bytes_with(c, bytes.as_bytes(), head(s), opts))
}

/// An agent's streamed write, at head, with `opts`.
fn write_stream(s: &BlobSession, c: ConnId, bytes: &str, opts: &BlobWriteOptions) -> String {
    outcome(s.write_with(c, &mut bytes.as_bytes(), head(s), opts))
}

/// A host's save ticket for an agent's bytes, at head, with `opts`.
fn ticket(s: &BlobSession, c: ConnId, bytes: &str, opts: &BlobWriteOptions) -> String {
    outcome(s.expect_save_with(c, head(s), bytes.as_bytes(), opts))
}

/// whole_write_respects_holds: a person holds `@n:2`; under Respect an agent's `write_bytes`,
/// streamed `write` and `expect_save` that change it are each refused naming the holder, and
/// nothing is written; the same three changing only `@n:3` land.
/// Plant: `whole-write-skips-holds`.
#[test]
fn whole_write_respects_holds() {
    let f = fx("respects", |_| {});
    let s = f.open("a.idx", DOC, pack());
    let ann = f.join(&s, "ann", ParticipantKind::Person);
    let bot = f.join(&s, "bot", ParticipantKind::Agent);
    hold(&s, ann, &["@n:2"]);
    let v = head(&s);
    let held = changed("@n:2", "red");
    let refused = [
        write_bytes(&s, bot, &held, &respect()),
        write_stream(&s, bot, &held, &respect()),
        ticket(&s, bot, &held, &respect()),
    ];
    let untouched = head(&s) == v && fs::read_to_string(f.ws.join("a.idx")).unwrap() == DOC;
    let landed = [
        write_bytes(&s, bot, &changed("@n:3", "one"), &respect()),
        write_stream(&s, bot, &changed("@n:3", "two"), &respect()),
        ticket(&s, bot, &changed("@n:3", "three"), &respect()),
    ];
    let by = "held by ann".to_owned();
    let ok = "landed".to_owned();
    assert_eq!(
        (refused, untouched, landed),
        (
            [by.clone(), by.clone(), by],
            true,
            [ok.clone(), ok.clone(), ok]
        ),
        "whole_write_respects_holds: a hold binds a whole-file write by an agent: write, a \
         streamed write and a save ticket changing a held node are refused naming the holder \
         and write nothing; changing a node nobody holds, they land"
    );
}

/// whole_write_hold_reach: a layer hold refuses a write changing a member, and one adding a
/// member; a block hold refuses a change under it; a node outside both lands.
/// Plant: `whole-write-skips-holds`.
#[test]
fn whole_write_hold_reach() {
    let f = fx("reach", |_| {});
    let s = f.open("a.idx", DOC, pack());
    let ann = f.join(&s, "ann", ParticipantKind::Person);
    let bot = f.join(&s, "bot", ParticipantKind::Agent);
    hold(&s, ann, &["@n:1", "@n:5"]);
    let member = write_bytes(&s, bot, &changed("@n:2", "red"), &respect());
    let added = write_bytes(&s, bot, &with_member(), &respect());
    let under = write_bytes(&s, bot, &changed("@n:6", "tread"), &respect());
    let outside = write_bytes(&s, bot, &changed("@n:3", "wide"), &respect());
    let by = "held by ann".to_owned();
    assert_eq!(
        [member, added, under, outside],
        [by.clone(), by.clone(), by, "landed".to_owned()],
        "whole_write_hold_reach: a hold's reach binds a whole-file write: a layer's member, a \
         member the write adds, a node under a held block are refused; a node outside lands"
    );
}

/// whole_write_person_never_refused: a person's whole-file write over a held node lands under
/// Respect (holds coordinate agents; they never block a person).
#[test]
fn whole_write_person_never_refused() {
    let f = fx("person", |_| {});
    let s = f.open("a.idx", DOC, pack());
    let ann = f.join(&s, "ann", ParticipantKind::Person);
    let cy = f.join(&s, "cy", ParticipantKind::Person);
    hold(&s, ann, &["@n:2"]);
    let got = write_bytes(&s, cy, &changed("@n:2", "red"), &respect());
    assert_eq!(
        got, "landed",
        "whole_write_person_never_refused: a person's write over a held node lands"
    );
}

/// whole_write_ignore_unchanged: under `ClaimPolicy::Ignore` (the default) an agent's write
/// over a held node lands, as before, with no index pass.
#[test]
fn whole_write_ignore_unchanged() {
    let f = fx("ignore", |_| {});
    let s = f.open("a.idx", DOC, pack());
    let ann = f.join(&s, "ann", ParticipantKind::Person);
    let bot = f.join(&s, "bot", ParticipantKind::Agent);
    hold(&s, ann, &["@n:2"]);
    let runs = index_runs();
    let got = write_bytes(&s, bot, &changed("@n:2", "red"), &BlobWriteOptions::new());
    assert_eq!(
        (got.as_str(), index_runs() - runs),
        ("landed", 0),
        "whole_write_ignore_unchanged: under Ignore an agent's write over a held node lands, \
         unread"
    );
}

/// whole_write_no_holds_no_pass: with nobody holding a node, an agent's write under Respect
/// lands with no index pass.
#[test]
fn whole_write_no_holds_no_pass() {
    let f = fx("no-holds", |_| {});
    let s = f.open("a.idx", DOC, pack());
    let bot = f.join(&s, "bot", ParticipantKind::Agent);
    let runs = index_runs();
    let got = write_bytes(&s, bot, &changed("@n:2", "red"), &respect());
    assert_eq!(
        (got.as_str(), index_runs() - runs),
        ("landed", 0),
        "whole_write_no_holds_no_pass: no holds: an agent's write under Respect lands, unread"
    );
}

/// whole_write_unindexable_with_holds: with a hold present, an agent's write the index can't
/// read within its limits is refused (`MPC-E306`, exit 5, the holders named, a next step);
/// once the hold is released the same write lands. (No plant: the refusal is the read's, made
/// before the commit's claim check that `whole-write-skips-holds` skips; a red run shows it.)
#[test]
fn whole_write_unindexable_with_holds() {
    let f = fx("unindexable", |c| c.blob_index_max_nodes = 8);
    let s = f.open("a.idx", DOC, pack());
    let ann = f.join(&s, "ann", ParticipantKind::Person);
    let bot = f.join(&s, "bot", ParticipantKind::Agent);
    hold(&s, ann, &["@n:2"]);
    let big = with_lines(10);
    let r = s.write_bytes_with(bot, big.as_bytes(), head(&s), &respect());
    let (id, exit, holders, step) = match &r {
        Err(e) => {
            let rep = e.report();
            let holders = rep.detail("holders").map(|h| h.to_json());
            (
                Some(rep.id()),
                rep.entry().exit,
                holders,
                rep.next_step().is_some(),
            )
        }
        Ok(_) => (None, 0, None, false),
    };
    s.release_claim(ann, None).unwrap();
    let after = write_bytes(&s, bot, &big, &respect());
    assert_eq!(
        (id, exit, holders, step, after.as_str()),
        (
            Some("MPC-E306"),
            5,
            Some(r#"["ann"]"#.to_owned()),
            true,
            "landed"
        ),
        "whole_write_unindexable_with_holds: a hold binds a whole-file write: with one present, \
         a write past the index's limits is refused naming the holders, with a next step; \
         released, it lands"
    );
}

/// A source that takes `ann`'s hold on `@n:2` the first time it is read (a hold arriving while
/// the write uploads).
struct HoldOnRead {
    bytes: Vec<u8>,
    at: usize,
    s: Arc<BlobSession>,
    ann: ConnId,
    taken: bool,
}

impl Read for HoldOnRead {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if !self.taken {
            self.taken = true;
            hold(&self.s, self.ann, &["@n:2"]);
        }
        let n = (self.bytes.len() - self.at).min(buf.len());
        buf[..n].copy_from_slice(&self.bytes[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

/// whole_write_hold_during_upload: nobody holds a node when an agent's write is admitted, and a
/// hold arrives while it uploads: the write can't tell what the hold covers, so it is refused
/// naming the holder (nothing written); the same write again is checked and lands, since it
/// changes a node outside the hold.
/// Plant: `hold-during-upload-missed`.
#[test]
fn whole_write_hold_during_upload() {
    let f = fx("upload", |_| {});
    let s = f.open("a.idx", DOC, pack());
    let ann = f.join(&s, "ann", ParticipantKind::Person);
    let bot = f.join(&s, "bot", ParticipantKind::Agent);
    let bytes = changed("@n:3", "wide");
    let v = head(&s);
    let mut src = HoldOnRead {
        bytes: bytes.clone().into_bytes(),
        at: 0,
        s: s.clone(),
        ann,
        taken: false,
    };
    let first = outcome(s.write_with(bot, &mut src, v, &respect()));
    let unchanged = head(&s) == v;
    let again = write_bytes(&s, bot, &bytes, &respect());
    assert_eq!(
        (first.as_str(), unchanged, again.as_str()),
        ("held by ann", true, "landed"),
        "whole_write_hold_during_upload: a hold taken while the write uploads refuses it \
         (nothing written); the write again is checked and lands"
    );
}

/// whole_write_seeds_index: an agent's checked write records the index of its version from
/// the check's own read, so no index job is out for it afterwards.
#[test]
fn whole_write_seeds_index() {
    let f = fx("seeds", |_| {});
    let s = f.open("a.idx", DOC, pack());
    let ann = f.join(&s, "ann", ParticipantKind::Person);
    let bot = f.join(&s, "bot", ParticipantKind::Agent);
    hold(&s, ann, &["@n:2"]);
    let got = write_bytes(&s, bot, &changed("@n:3", "wide"), &respect());
    let job = s.index_job(None).unwrap().is_some();
    assert_eq!(
        (got.as_str(), job),
        ("landed", false),
        "whole_write_seeds_index: a checked write's version is indexed by its check"
    );
}
