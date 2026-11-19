//! A tier-0 batch (`docs/SESSIONS.md`, "A batch of verbs"): an agent's calls of a file type's
//! verbs as one change, `BlobSession::run_batch`. Each call plans on what the calls before it
//! staged; the output is checked once against every call's declared nodes; one version, one
//! `blob_verbs` row (`batch`, its calls), all or nothing. On the index test pack
//! ([`index_pack`], its [`index_pack::BATCH_SET`] verbs). Public API only.
//!
//! Each row names the plant (`MPC_T0V_MUTANT`) it catches; the plants part runs each alone.

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier, Mutex};

mod index_pack;
use index_pack::{IxPack, BATCH_SET};

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::errors::Value;
use k2_mpc_base::index::NodeKey;
use k2_mpc_base::session::{
    AuditEvent, BatchCall, BlobSession, ClaimPolicy, Config, ConnId, HoldReach, Host, Participant,
    ParticipantKind, Rights, SessionManager, Stay,
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

/// A row's folder, removed when the row ends.
struct Dir(PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Fx {
    ws: PathBuf,
    host: Arc<TestHost>,
    store: Arc<DocStore>,
    mgr: SessionManager,
    dir: Dir,
}

fn fx(tag: &str, tweak: impl FnOnce(&mut Config)) -> Fx {
    fx_with(tag, StoreSettings::DEFAULT, tweak)
}

/// A fixture whose store is opened with `settings`.
fn fx_with(tag: &str, settings: StoreSettings, tweak: impl FnOnce(&mut Config)) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-t0batch-{tag}-{}-{}",
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
    let store = Arc::new(DocStore::open_with(dir.join("docs.redb"), settings).unwrap());
    let mut c = config();
    tweak(&mut c);
    let mgr = manager_on(&store, &dir, &host, c);
    Fx {
        ws,
        host,
        store,
        mgr,
        dir: Dir(dir),
    }
}

/// The rows' config: edits never rate-limited.
fn config() -> Config {
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    c
}

/// A session manager over `store`, its blobs in `dir`.
fn manager_on(
    store: &Arc<DocStore>,
    dir: &Path,
    host: &Arc<TestHost>,
    c: Config,
) -> SessionManager {
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
    SessionManager::new(store.clone(), host.clone(), c).with_blob_store(blobs)
}

fn key(p: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: p.into(),
    }
}

/// The batch verbs, the pack's merge on; a layer's hold reaches the nodes tagged with it.
fn pack() -> IxPack {
    IxPack {
        verbs: BATCH_SET,
        merge: true,
        reach: &[("layer", HoldReach::Members("tag"))],
        ..IxPack::default()
    }
}

/// A layer (`@n:1`) and three lines; `@n:2` is on the layer.
const DOC: &str = "IDX\n@n:1|layer|Walls|-|1|-|-|-\n@n:2|line|Door|-|1|-|door|-|@n:1\n\
                   @n:3|line|Window|-|1|-|window|-\n@n:4|line|Wall|-|1|-|wall|-\n";

fn index(s: &BlobSession) {
    if let Some(j) = s.index_job(None).unwrap() {
        j.run().unwrap();
    }
}

fn head(s: &BlobSession) -> VersionId {
    s.status().unwrap().version
}

impl Fx {
    fn open(&self, name: &str, bytes: &str) -> Arc<BlobSession> {
        fs::write(self.ws.join(name), bytes).unwrap();
        let s = self
            .mgr
            .open_blob_file(Arc::new(pack()), &key(name), &self.ws.join(name), &self.ws)
            .unwrap();
        index(&s);
        s
    }
    /// A doc with no file (nothing written back), indexed.
    fn open_stored(&self, name: &str, bytes: &str) -> Arc<BlobSession> {
        let s = self
            .mgr
            .open_blob(Arc::new(pack()), &key(name), Some(&mut bytes.as_bytes()))
            .unwrap();
        index(&s);
        s
    }
    fn join(&self, s: &BlobSession, pid: &str, kind: ParticipantKind) -> ConnId {
        join_on(&self.host, s, pid, kind)
    }
}

/// `pid` joins `s` with every right.
fn join_on(host: &TestHost, s: &BlobSession, pid: &str, kind: ParticipantKind) -> ConnId {
    host.rights
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

fn calls(cs: &[&[&str]]) -> Vec<BatchCall> {
    cs.iter()
        .map(|c| BatchCall::new(c.iter().map(|s| (*s).to_owned()).collect()))
        .collect()
}

/// A batch with its result as data; `flags` are the batch's own (`--if-version`, `--base`,
/// `--dry-run`).
fn batch_with(
    s: &BlobSession,
    c: ConnId,
    flags: &[&str],
    cs: &[&[&str]],
    opts: ToolOptions,
) -> ToolOutput {
    let flags: Vec<String> = flags.iter().map(|s| (*s).to_owned()).collect();
    s.run_batch(c, &flags, &calls(cs), &opts.result_json(true))
        .expect("with the result asked for, every outcome is a result")
}

fn batch(s: &BlobSession, c: ConnId, flags: &[&str], cs: &[&[&str]]) -> ToolOutput {
    batch_with(s, c, flags, cs, ToolOptions::new())
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

fn failed_at(o: &ToolOutput) -> Option<u64> {
    res(o)
        .get("batch")
        .and_then(|b| b.get("failed_at"))
        .and_then(Value::as_u64)
}

fn word(o: &ToolOutput) -> Option<String> {
    res(o)
        .get("changed")
        .and_then(|c| c.get("word"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn file(f: &Fx, name: &str) -> String {
    fs::read_to_string(f.ws.join(name)).unwrap()
}

fn bytes_at(s: &BlobSession, c: ConnId, v: VersionId) -> Vec<u8> {
    let mut r = s.read_version(c, v).unwrap();
    let mut out = Vec::new();
    r.reader.read_to_end(&mut out).unwrap();
    out
}

/// A line of the doc by its key: its fields.
fn fields(s: &BlobSession, c: ConnId, k: &str) -> Option<Vec<String>> {
    let b = bytes_at(s, c, head(s));
    String::from_utf8(b)
        .unwrap()
        .lines()
        .find(|l| l.split('|').next() == Some(k))
        .map(|l| l.split('|').map(str::to_owned).collect())
}

/// batch_one_version: three edit calls make one version, one `blob_verbs` row (`batch`, its
/// calls in order), the word `batch`, and each call's word in the result.
/// Plant: `batch-commits-each`.
#[test]
fn batch_one_version() {
    let f = fx("one", |_| {});
    let s = f.open("a.idx", DOC);
    let bot = f.join(&s, "bot", ParticipantKind::Agent);
    let v = head(&s);
    let o = batch(
        &s,
        bot,
        &[],
        &[
            &["part", "set", "@n:2", "note", "a"],
            &["part", "set", "@n:3", "note", "b"],
            &["part", "add", "@n:9", "line", "Sill"],
        ],
    );
    let log = s.log(bot).unwrap();
    let e = log.iter().find(|e| e.id == head(&s));
    let rec = e.map(|e| (e.verb.clone(), e.calls.clone()));
    let words: Vec<String> = match res(&o).get("batch").and_then(|b| b.get("calls")) {
        Some(Value::List(l)) => l
            .iter()
            .filter_map(|c| c.get("word").and_then(Value::as_str).map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    };
    assert_eq!(
        (err_id(&o), head(&s), rec, word(&o), words),
        (
            None,
            v + 1,
            Some((
                Some("batch".to_owned()),
                vec![
                    "part set".to_owned(),
                    "part set".to_owned(),
                    "part add".to_owned()
                ]
            )),
            Some("batch".to_owned()),
            vec!["set".to_owned(), "set".to_owned(), "added".to_owned()]
        ),
        "batch_one_version: a batch of three calls is one version, recorded as a batch of \
         those calls"
    );
}

/// batch_all_or_nothing: a refusal at call 2 of 3 (a pack's refusal; an unknown verb; a
/// call's own guard flag) applies nothing: head and the file unchanged, `failed_at = 2`.
/// Plant: `batch-commits-each`.
#[test]
fn batch_all_or_nothing() {
    let f = fx("all", |_| {});
    let s = f.open("a.idx", DOC);
    let bot = f.join(&s, "bot", ParticipantKind::Agent);
    let v = head(&s);
    let one: &[&str] = &["part", "set", "@n:2", "note", "a"];
    let three: &[&str] = &["part", "set", "@n:3", "note", "c"];
    let refused = batch(
        &s,
        bot,
        &[],
        &[one, &["part", "bad", "refuse", "@n:3"], three],
    );
    let unknown = batch(&s, bot, &[], &[one, &["part", "fly", "@n:3"], three]);
    let guarded = batch(
        &s,
        bot,
        &[],
        &[
            one,
            &["part", "set", "@n:3", "note", "b", "--if-version", "1"],
            three,
        ],
    );
    let untouched = head(&s) == v && file(&f, "a.idx") == DOC;
    assert_eq!(
        (
            failed_at(&refused),
            failed_at(&unknown),
            failed_at(&guarded),
            err_id(&refused).is_some() && err_id(&unknown).is_some() && err_id(&guarded).is_some(),
            untouched
        ),
        (Some(2), Some(2), Some(2), true, true),
        "batch_all_or_nothing: a refusal at a call applies nothing and names the call"
    );
}

/// batch_sees_staged: call 2 plans on what call 1 staged: two `part new` calls make two
/// nodes, and `$1` names the first one's key in call 3.
/// Plant: `batch-plans-on-head`.
#[test]
fn batch_sees_staged() {
    let f = fx("staged", |_| {});
    let s = f.open("a.idx", DOC);
    let bot = f.join(&s, "bot", ParticipantKind::Agent);
    let o = batch(
        &s,
        bot,
        &[],
        &[
            &["part", "new", "line", "Sill"],
            &["part", "new", "line", "Lintel"],
            &["part", "set", "$1", "note", "first"],
        ],
    );
    let five = fields(&s, bot, "@n:5").map(|l| (l[2].clone(), l[7].clone()));
    let six = fields(&s, bot, "@n:6").map(|l| l[2].clone());
    assert_eq!(
        (err_id(&o), five, six),
        (
            None,
            Some(("Sill".to_owned(), "first".to_owned())),
            Some("Lintel".to_owned())
        ),
        "batch_sees_staged: each call plans on the calls before it, and $1 names call 1's node"
    );
}

/// batch_exact: every byte outside the batch's composed ranges is head's: the stored output
/// equals head with the calls' edits made one after another by hand.
/// Plant: `batch-compose-wrong`.
#[test]
fn batch_exact() {
    let f = fx("exact", |_| {});
    let s = f.open("a.idx", DOC);
    let bot = f.join(&s, "bot", ParticipantKind::Agent);
    let o = batch(
        &s,
        bot,
        &[],
        &[
            &["part", "set", "@n:4", "note", "z"],
            &["part", "set", "@n:2", "note", "longer note"],
            &["part", "set", "@n:2", "name", "D"],
            &["part", "add", "@n:9", "line", "Sill"],
        ],
    );
    let want = DOC
        .replace("@n:4|line|Wall|-|1|-|wall|-", "@n:4|line|Wall|-|1|-|wall|z")
        .replace(
            "@n:2|line|Door|-|1|-|door|-|@n:1",
            "@n:2|line|D|-|1|-|door|longer note|@n:1",
        )
        + "@n:9|line|Sill|-|1|-|-|-\n";
    let got = String::from_utf8(bytes_at(&s, bot, head(&s))).unwrap();
    assert_eq!(
        (err_id(&o), got),
        (None, want),
        "batch_exact: the batch's output is head with each call's edit, every other byte head's"
    );
}

/// batch_strayed_union: a call changing a node no call declared (`part bad stray` declares
/// `@n:3` and changes `@n:4`) refuses the batch (`MPC-E726`), nothing written.
#[test]
fn batch_strayed_union() {
    let f = fx("stray", |_| {});
    let s = f.open("a.idx", DOC);
    let bot = f.join(&s, "bot", ParticipantKind::Agent);
    let v = head(&s);
    let o = batch(
        &s,
        bot,
        &[],
        &[
            &["part", "set", "@n:2", "note", "a"],
            &["part", "bad", "stray", "@n:3", "@n:4"],
        ],
    );
    assert_eq!(
        (err_id(&o), head(&s)),
        (Some("MPC-E726".to_owned()), v),
        "batch_strayed_union: a node no call declared refuses the batch"
    );
}

/// batch_guards_whole: `--if-version` is the batch's: a stale one refuses (`MPC-E110`) and a
/// current one lands; a call carrying its own guard is usage naming the call.
#[test]
fn batch_guards_whole() {
    let f = fx("guards", |_| {});
    let s = f.open("a.idx", DOC);
    let bot = f.join(&s, "bot", ParticipantKind::Agent);
    let v = head(&s);
    let one: &[&str] = &["part", "set", "@n:2", "note", "a"];
    let stale = batch(&s, bot, &["--if-version", &(v + 7).to_string()], &[one]);
    let current = batch(&s, bot, &["--if-version", &v.to_string()], &[one]);
    let own = batch(
        &s,
        bot,
        &[],
        &[&["part", "set", "@n:3", "note", "b", "--base", &v.to_string()]],
    );
    assert_eq!(
        (
            err_id(&stale),
            err_id(&current),
            head(&s),
            failed_at(&own),
            own.exit
        ),
        (Some("MPC-E110".to_owned()), None, v + 1, Some(1), 2),
        "batch_guards_whole: the guards are the batch's; a call's own refuses as usage"
    );
}

/// batch_base_per_node: `--base V` with another write since: a batch whose calls touch
/// other nodes lands; one whose call 2 sets the field that write moved is refused
/// (`MPC-E723`, `failed_at = 2`), and so is one whose call 1 does, with a last call that
/// conflicts with nothing (`failed_at = 1`).
/// Plant: `batch-base-last-call-only`.
#[test]
fn batch_base_per_node() {
    let f = fx("base", |_| {});
    let s = f.open("a.idx", DOC);
    let ann = f.join(&s, "ann", ParticipantKind::Person);
    let bot = f.join(&s, "bot", ParticipantKind::Agent);
    let v = head(&s).to_string();
    let r = s
        .run_tool(
            ann,
            &["part", "set", "@n:3", "note", "ann"].map(str::to_owned),
            &ToolOptions::new().result_json(true),
        )
        .unwrap();
    assert!(err_id(&r).is_none(), "the person's write lands");
    index(&s);
    let apart = batch(
        &s,
        bot,
        &["--base", &v],
        &[
            &["part", "set", "@n:2", "note", "a"],
            &["part", "set", "@n:4", "note", "b"],
        ],
    );
    let moved = batch(
        &s,
        bot,
        &["--base", &v],
        &[
            &["part", "set", "@n:1", "name", "W"],
            &["part", "set", "@n:3", "note", "bot"],
        ],
    );
    let early = batch(
        &s,
        bot,
        &["--base", &v],
        &[
            &["part", "set", "@n:3", "note", "bot"],
            &["part", "set", "@n:1", "note", "l"],
        ],
    );
    assert_eq!(
        (
            err_id(&apart),
            err_id(&moved),
            failed_at(&moved),
            err_id(&early),
            failed_at(&early)
        ),
        (
            None,
            Some("MPC-E723".to_owned()),
            Some(2),
            Some("MPC-E723".to_owned()),
            Some(1)
        ),
        "batch_base_per_node: --base is checked per node over every call"
    );
}

/// batch_claims_union: under Respect, a hold on a node only call 2 reaches refuses the batch
/// (`MPC-E301`, `failed_at = 2`); a person's batch is never refused.
/// Plant: `batch-claims-first-call`.
#[test]
fn batch_claims_union() {
    let f = fx("claims", |_| {});
    let s = f.open("a.idx", DOC);
    let ann = f.join(&s, "ann", ParticipantKind::Person);
    let cy = f.join(&s, "cy", ParticipantKind::Person);
    let bot = f.join(&s, "bot", ParticipantKind::Agent);
    s.claim_nodes(ann, &[NodeKey::parse("@n:1").unwrap()], None, None)
        .unwrap();
    let cs: &[&[&str]] = &[
        &["part", "set", "@n:3", "note", "a"],
        &["part", "set", "@n:2", "note", "b"],
    ];
    let respect = || ToolOptions::new().policy(ClaimPolicy::Respect);
    let agent = batch_with(&s, bot, &[], cs, respect());
    let person = batch_with(&s, cy, &[], cs, respect());
    assert_eq!(
        (err_id(&agent), failed_at(&agent), err_id(&person)),
        (Some("MPC-E301".to_owned()), Some(2), None),
        "batch_claims_union: a hold any call reaches refuses an agent's batch, naming the call"
    );
}

/// batch_lease_rules: a batch with a `merge: Lease` call needs the caller's own lease
/// (`MPC-E718`, `failed_at` names it); two agents' batches on other nodes, released together,
/// both land (no `MPC-E701` for overlap in time).
/// Plant: `batch-lease-whole-run`.
#[test]
fn batch_lease_rules() {
    let f = fx("lease", |_| {});
    let s = f.open("a.idx", DOC);
    let a = f.join(&s, "a", ParticipantKind::Agent);
    let b = f.join(&s, "b", ParticipantKind::Agent);
    let guarded = batch(
        &s,
        a,
        &[],
        &[
            &["part", "set", "@n:2", "note", "a"],
            &["part", "guard", "@n:3", "note", "b"],
        ],
    );
    let go = Arc::new(Barrier::new(2));
    let run = |c: ConnId, k: &'static str| {
        let (s, go) = (s.clone(), go.clone());
        std::thread::spawn(move || {
            go.wait();
            for _ in 0..2_000 {
                let o = batch(&s, c, &[], &[&["part", "set", k, "note", "x"]]);
                if err_id(&o).as_deref() != Some("MPC-E713") {
                    return err_id(&o);
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            Some("index never landed".to_owned())
        })
    };
    let (ta, tb) = (run(a, "@n:2"), run(b, "@n:4"));
    let both = (ta.join().unwrap(), tb.join().unwrap());
    assert_eq!(
        (err_id(&guarded), failed_at(&guarded), both),
        (Some("MPC-E718".to_owned()), Some(2), (None, None)),
        "batch_lease_rules: a lease-only call needs the caller's lease; agents' batches at \
         once both land"
    );
}

/// A batch (`Some`: its calls) or a single verb (`None`: `argv`), retried while head's index
/// is pending (`MPC-E713`), as a host's wait-and-retry does (a bounded wait: then the last
/// answer, for the row to read).
fn until_indexed(
    s: &BlobSession,
    c: ConnId,
    flags: &[String],
    cs: Option<&[Vec<String>]>,
    argv: &[String],
) -> ToolOutput {
    let mut last = None;
    for _ in 0..2_000 {
        let o = match cs {
            Some(cs) => {
                let calls: Vec<BatchCall> = cs.iter().cloned().map(BatchCall::new).collect();
                s.run_batch(c, flags, &calls, &ToolOptions::new().result_json(true))
                    .expect("with the result asked for, every outcome is a result")
            }
            None => s
                .run_tool(c, argv, &ToolOptions::new().result_json(true))
                .expect("with the result asked for, every outcome is a result"),
        };
        if err_id(&o).as_deref() != Some("MPC-E713") {
            return o;
        }
        last = Some(o);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    last.expect("at least one try")
}

/// One racer: its connection, a batch's calls (or none), a single verb's arguments.
type Job = (ConnId, Option<Vec<Vec<String>>>, Vec<String>);

/// batch_race: seven agents released together on one doc, each with `--base V`: three batches
/// on three different nodes, three batches setting the same field of a fourth node, and one
/// single verb on a field nobody else sets. Each lands or is refused for a real conflict
/// (`MPC-E723`); none is refused for another merely running (no `MPC-E701`). The three apart
/// and the single verb land; of the three on one field exactly one does. Nothing is lost or
/// applied twice: one version per landed call, each node holding its caller's values.
/// Plant: `batch-lease-whole-run`.
#[test]
fn batch_race() {
    let f = fx("race", |_| {});
    let s = f.open("a.idx", DOC);
    let agents: Vec<ConnId> = (0..7)
        .map(|n| f.join(&s, &format!("agent-{n}"), ParticipantKind::Agent))
        .collect();
    let v = head(&s);
    let flags = vec!["--base".to_owned(), v.to_string()];
    let argv = |x: &[&str]| x.iter().map(|s| (*s).to_owned()).collect::<Vec<String>>();
    let mut jobs: Vec<Job> = Vec::new();
    for (n, key) in ["@n:1", "@n:3", "@n:4"].iter().enumerate() {
        let (note, tag) = (format!("d{n}"), format!("e{n}"));
        jobs.push((
            agents[n],
            Some(vec![
                argv(&["part", "set", key, "note", &note]),
                argv(&["part", "set", key, "tag", &tag]),
            ]),
            Vec::new(),
        ));
    }
    for (n, a) in agents.iter().enumerate().take(6).skip(3) {
        let tag = format!("s{n}");
        jobs.push((
            *a,
            Some(vec![argv(&["part", "set", "@n:2", "tag", &tag])]),
            Vec::new(),
        ));
    }
    let mut single = argv(&["part", "set", "@n:1", "text", "t"]);
    single.extend(flags.iter().cloned());
    jobs.push((agents[6], None, single));
    let go = Arc::new(Barrier::new(jobs.len()));
    let threads: Vec<_> = jobs
        .into_iter()
        .map(|(c, cs, argv)| {
            let (s, go, flags) = (s.clone(), go.clone(), flags.clone());
            std::thread::spawn(move || {
                go.wait();
                until_indexed(&s, c, &flags, cs.as_deref(), &argv)
            })
        })
        .collect();
    let out: Vec<ToolOutput> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    let ids: Vec<Option<String>> = out.iter().map(err_id).collect();
    let leased = ids
        .iter()
        .filter(|i| i.as_deref() == Some("MPC-E701"))
        .count();
    let apart_landed = ids[..3].iter().all(Option::is_none) && ids[6].is_none();
    let same_landed = ids[3..6].iter().filter(|i| i.is_none()).count();
    let same_refused = ids[3..6]
        .iter()
        .filter(|i| i.as_deref() == Some("MPC-E723"))
        .count();
    let winner = (3..6).find(|n| ids[*n].is_none()).map(|n| format!("s{n}"));
    let at = |k: &str, col: usize| fields(&s, agents[0], k).and_then(|l| l.get(col).cloned());
    let held = (
        [at("@n:1", 7), at("@n:3", 7), at("@n:4", 7)],
        [at("@n:1", 8), at("@n:3", 8), at("@n:4", 8)],
        at("@n:1", 5),
    );
    let some = |x: &str| Some(x.to_owned());
    assert_eq!(
        (
            leased,
            apart_landed,
            same_landed,
            same_refused,
            held,
            at("@n:2", 8) == winner && winner.is_some(),
            head(&s) - v
        ),
        (
            0,
            true,
            1,
            2,
            (
                [some("d0"), some("d1"), some("d2")],
                [some("e0"), some("e1"), some("e2")],
                some("t")
            ),
            true,
            5
        ),
        "batch_race: batches and a verb at once on one doc: no refusal for overlap in time, \
         every disjoint batch lands, one of a field's batches lands, the others E723, nothing \
         lost or applied twice ({ids:?})"
    );
}

/// batch_dry_run_matches_real: a dry run's result is the real call's but for `dry_run` and
/// the version: nothing written, and the real call then lands.
#[test]
fn batch_dry_run_matches_real() {
    let f = fx("dry", |_| {});
    let s = f.open("a.idx", DOC);
    let bot = f.join(&s, "bot", ParticipantKind::Agent);
    let v = head(&s);
    let cs: &[&[&str]] = &[
        &["part", "new", "line", "Sill"],
        &["part", "set", "$1", "note", "n"],
    ];
    let dry = batch(&s, bot, &["--dry-run"], cs);
    let after_dry = head(&s);
    let real = batch(&s, bot, &[], cs);
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
        (None, v, true, v + 1),
        "batch_dry_run_matches_real: a dry run shows the real call's change and writes nothing"
    );
}

/// batch_caps: over `blob_batch_max_calls` the batch is refused before any call runs, naming
/// the limit; a batch at the limit lands.
#[test]
fn batch_caps() {
    let f = fx("caps", |c| c.blob_batch_max_calls = 2);
    let s = f.open("a.idx", DOC);
    let bot = f.join(&s, "bot", ParticipantKind::Agent);
    let v = head(&s);
    let over = batch(
        &s,
        bot,
        &[],
        &[
            &["part", "set", "@n:2", "note", "a"],
            &["part", "set", "@n:3", "note", "b"],
            &["part", "set", "@n:4", "note", "c"],
        ],
    );
    let refused_whole = head(&s) == v && failed_at(&over).is_none() && err_id(&over).is_some();
    let at = batch(
        &s,
        bot,
        &[],
        &[
            &["part", "set", "@n:2", "note", "a"],
            &["part", "set", "@n:3", "note", "b"],
        ],
    );
    assert_eq!(
        (refused_whole, err_id(&at), head(&s)),
        (true, None, v + 1),
        "batch_caps: a batch over the call cap is refused whole; one at it lands"
    );
}

/// batch_cap_setting_checked: a call cap outside 1 to `BATCH_CALLS_CEILING` refuses every
/// batch, naming the setting; nothing runs.
#[test]
fn batch_cap_setting_checked() {
    let refused = |cap: u64| {
        let f = fx("capset", |c| c.blob_batch_max_calls = cap);
        let s = f.open("a.idx", DOC);
        let bot = f.join(&s, "bot", ParticipantKind::Agent);
        let v = head(&s);
        let o = batch(&s, bot, &[], &[&["part", "set", "@n:2", "note", "a"]]);
        let named = res(&o)
            .get("error")
            .map(Value::to_json)
            .is_some_and(|e| e.contains("blob_batch_max_calls"));
        (head(&s) == v, failed_at(&o), o.exit, named)
    };
    assert_eq!(
        (
            refused(0),
            refused(k2_mpc_base::session::BATCH_CALLS_CEILING + 1)
        ),
        ((true, None, 2, true), (true, None, 2, true)),
        "batch_cap_setting_checked: a cap out of range refuses every batch, naming the setting"
    );
}

/// batch_seeds_index: after a batch, the next verb needs no index job.
#[test]
fn batch_seeds_index() {
    let f = fx("seeds", |_| {});
    let s = f.open("a.idx", DOC);
    let bot = f.join(&s, "bot", ParticipantKind::Agent);
    let o = batch(
        &s,
        bot,
        &[],
        &[
            &["part", "set", "@n:2", "note", "a"],
            &["part", "set", "@n:3", "note", "b"],
        ],
    );
    let job = s.index_job(None).unwrap().is_some();
    assert_eq!(
        (err_id(&o), job),
        (None, false),
        "batch_seeds_index: a batch's version is indexed by its own check"
    );
}

/// a_batch_commit_takes_the_group_durability: a batch commits as any other change does, at
/// the store's commit durability. Grouped at 1,024 changes and 10 s (neither bound fires
/// here), on a doc with no file whose first version since the open was made (it raised the
/// doc's blob floor, durably): the batch's commits (its output's blob row, its version and
/// its index row) are acknowledged and wait for the disk; a flush makes them durable, and a reopen of the store
/// holds the batch's version. On a doc of a file, its write-back (a barrier) makes the batch
/// durable before the file is written. At `Immediate` (the default) the batch is durable when
/// it returns.
/// Plant: `batch-commits-each`.
#[test]
fn a_batch_commit_takes_the_group_durability() {
    let cs: &[&[&str]] = &[
        &["part", "set", "@n:2", "note", "a"],
        &["part", "set", "@n:3", "note", "b"],
        &["part", "add", "@n:9", "line", "Sill"],
    ];
    let grouped = StoreSettings::DEFAULT
        .with_commit_durability(CommitDurability::grouped(1024, 10_000).unwrap());
    // grouped, a doc with no file: nothing outside the store depends on the batch
    let f = fx_with("grouped", grouped, |_| {});
    let s = f.open_stored("a.idx", DOC);
    let bot = f.join(&s, "bot", ParticipantKind::Agent);
    // the doc's first version since the open raises its blob floor, so it is made durable
    // (`store/seq_floor.rs`); the batch after it waits
    let first: &[&[&str]] = &[&["part", "set", "@n:4", "note", "w"]];
    assert_eq!(
        err_id(&batch(&s, bot, &[], first)),
        None,
        "control: a_batch_commit_takes_the_group_durability: the first change lands"
    );
    f.store.flush().unwrap();
    let (v, before) = (head(&s), f.store.commits());
    let o = batch(&s, bot, &[], cs);
    let made = (head(&s) - v, f.store.commits() - before);
    assert_eq!(
        (err_id(&o), made),
        (None, (1, 3)),
        "control: a_batch_commit_takes_the_group_durability: the batch is one version, three \
         commits (its output's blob row, the version, its index row)"
    );
    assert_eq!(
        f.store.durable_through(),
        before,
        "row: a_batch_commit_takes_the_group_durability: grouped, the batch's commits are \
         acknowledged and wait for the disk"
    );
    f.mgr.flush().unwrap();
    assert_eq!(
        f.store.durable_through(),
        f.store.commits(),
        "row: a_batch_commit_takes_the_group_durability: a flush makes the batch durable"
    );
    let vid = head(&s);
    let want = bytes_at(&s, bot, vid);
    drop(s);
    f.mgr.release().unwrap();
    let Fx {
        host,
        store,
        mgr,
        dir,
        ..
    } = f;
    drop((mgr, store));
    let store = Arc::new(DocStore::open_with(dir.0.join("docs.redb"), grouped).unwrap());
    let mgr = manager_on(&store, &dir.0, &host, config());
    let s = mgr
        .open_blob(Arc::new(pack()), &key("a.idx"), None)
        .unwrap();
    let own = join_on(&host, &s, "own", ParticipantKind::Person);
    let back = (head(&s), bytes_at(&s, own, head(&s)));
    assert_eq!(
        back,
        (vid, want),
        "row: a_batch_commit_takes_the_group_durability: a reopen of the store holds the \
         batch's version"
    );
    drop(s);
    mgr.release().unwrap();

    // grouped, a doc of a file: the write-back is a barrier
    let f = fx_with("grouped-file", grouped, |_| {});
    let s = f.open("a.idx", DOC);
    let bot = f.join(&s, "bot", ParticipantKind::Agent);
    f.store.flush().unwrap();
    let before = f.store.commits();
    let o = batch(&s, bot, &[], cs);
    let written = file(&f, "a.idx").into_bytes() == bytes_at(&s, bot, head(&s));
    assert_eq!(
        (err_id(&o), f.store.durable_through() > before, written),
        (None, true, true),
        "row: a_batch_commit_takes_the_group_durability: a doc's write-back makes the batch \
         durable before its file is written"
    );

    // Immediate, the default: durable when the batch returns
    let f = fx("immediate", |_| {});
    let s = f.open_stored("a.idx", DOC);
    let bot = f.join(&s, "bot", ParticipantKind::Agent);
    let before = f.store.commits();
    let o = batch(&s, bot, &[], cs);
    let durable = f.store.durable_through() == f.store.commits();
    assert_eq!(
        (err_id(&o), f.store.commits() - before, durable),
        (None, 3, true),
        "row: a_batch_commit_takes_the_group_durability: at Immediate the batch is durable \
         when it returns"
    );
}
