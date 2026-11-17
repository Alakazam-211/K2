//! Per-node `--base V` on a file type's own verbs (`docs/SESSIONS.md`, "Verbs on a blob doc"):
//! a call that read version V lands on head when nothing it changes was changed since V in a
//! way that overlaps; it refuses `MPC-E723` when something was, `MPC-E719` across another
//! program's save the pack doesn't prove, and `MPC-E724` on keys the kit minted. Then two
//! agents on one file, and a call in the gap before a verb's version is indexed. The pack is
//! the index test pack ([`index_pack`], its [`index_pack::BASE_SET`] verbs). Public API only.
//!
//! Each row names the plant (`MPC_T0V_MUTANT`) it catches; the plants part runs each alone.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::time::Duration;

mod index_pack;
use index_pack::{IxPack, BASE_SET};

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::errors::Value;
use k2_mpc_base::index::{IndexStatus, NodeKey};
use k2_mpc_base::session::{
    AuditEvent, BlobSession, ClaimPolicy, Config, ConnId, HoldReach, Host, Participant,
    ParticipantKind, Rights, SavePairing, SessionManager, Stay,
};
use k2_mpc_base::store::{DocKey, DocStore, VersionId};
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
        "k2mb-t0base-{tag}-{}-{}",
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

/// The pack with the `--base` verbs, its merge on (so `merge: Pack` verbs are allowed); a
/// hold on a `layer` reaches the nodes whose `tag` is its key, one on a `block` its subtree.
fn pack() -> IxPack {
    IxPack {
        verbs: BASE_SET,
        merge: true,
        reach: &[
            ("layer", HoldReach::Members("tag")),
            ("block", HoldReach::Subtree),
        ],
        ..IxPack::default()
    }
}

/// Four keyed nodes.
const DOC: &str = "IDX\n@n:1|layer|Walls|-|1|-|-|-\n@n:2|line|Door|0|1|-|door|-\n\
                   @n:3|line|Window|0|1|-|window|-\n@n:4|line|Wall|0|1|-|wall|-\n";

/// The same nodes with no keys of their own (the kit mints them).
const KEYLESS: &str = "IDX\n-|layer|Walls|-|1|-|-|-\n-|line|Door|0|1|-|door|-\n\
                       -|line|Window|0|1|-|window|-\n";

fn index(s: &BlobSession) {
    if let Some(j) = s.index_job(None).unwrap() {
        j.run().unwrap();
    }
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

/// A call with its result as data.
fn call(s: &BlobSession, c: ConnId, a: &[&str]) -> ToolOutput {
    s.run_tool(c, &argv(a), &ToolOptions::new().result_json(true))
        .expect("with the result asked for, every outcome is a result")
}

/// A call under [`ClaimPolicy::Respect`] (an agent's change to a node someone holds refuses).
fn call_respect(s: &BlobSession, c: ConnId, a: &[&str]) -> ToolOutput {
    s.run_tool(
        c,
        &argv(a),
        &ToolOptions::new()
            .policy(ClaimPolicy::Respect)
            .result_json(true),
    )
    .expect("with the result asked for, every outcome is a result")
}

/// Hold the nodes `keys` as `c`.
fn hold(s: &BlobSession, c: ConnId, keys: &[&str]) {
    let keys: Vec<NodeKey> = keys.iter().map(|k| NodeKey::parse(k).unwrap()).collect();
    s.claim_nodes(c, &keys, None, None).unwrap();
}

/// A refusal's id and its holder (`details.by`).
fn refused_by(o: &ToolOutput) -> (Option<String>, Option<String>) {
    (
        err_id(o),
        detail(o, "by").and_then(Value::as_str).map(str::to_owned),
    )
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

/// The refusal's detail `k`.
fn detail<'a>(o: &'a ToolOutput, k: &str) -> Option<&'a Value> {
    res(o).get("error")?.get("details")?.get(k)
}

fn head(s: &BlobSession) -> VersionId {
    s.status().unwrap().version
}

/// The current bytes' line for `key` (its `|` fields), or for the line named `name` when
/// `key` is `-`.
fn line(s: &BlobSession, c: ConnId, key: &str) -> Vec<String> {
    let mut r = s.read_version(c, head(s)).unwrap();
    let mut b = Vec::new();
    r.reader.read_to_end(&mut b).unwrap();
    String::from_utf8(b)
        .unwrap()
        .lines()
        .find(|l| l.split('|').next() == Some(key) || l.split('|').nth(2) == Some(key))
        .map(|l| l.split('|').map(str::to_owned).collect())
        .unwrap_or_default()
}

/// Field `at` of `key`'s line (`-` or missing: `None`).
fn field(s: &BlobSession, c: ConnId, key: &str, at: usize) -> Option<String> {
    line(s, c, key).get(at).filter(|f| *f != "-").cloned()
}

const NOTE: usize = 7;
const TAG: usize = 8;
const TEXT: usize = 5;

/// A call, the refusal's id (none: it landed) and the version it made.
fn outcome(o: &ToolOutput) -> (Option<String>, Option<u64>) {
    let after = res(o)
        .get("version")
        .and_then(|v| v.get("after"))
        .and_then(Value::as_u64);
    (err_id(o), after)
}

/// base_lands_untouched: another node changed since the base; the call lands on head, both
/// changes kept. Plant: `base-whole-doc`.
#[test]
fn base_lands_untouched() {
    let f = fx("untouched");
    let s = f.open("a.idx", DOC, pack());
    let ann = f.join(&s, "ann", ParticipantKind::Person);
    let bot = f.join(&s, "bot", ParticipantKind::Agent);
    let v = head(&s).to_string();
    call(&s, ann, &["part", "set", "@n:3", "note", "x"]);
    let o = call(&s, bot, &["part", "set", "@n:2", "note", "y", "--base", &v]);
    let got = (
        outcome(&o).0,
        field(&s, bot, "@n:2", NOTE),
        field(&s, bot, "@n:3", NOTE),
    );
    assert_eq!(
        got,
        (None, Some("y".to_owned()), Some("x".to_owned())),
        "base_lands_untouched: --base per node: a change elsewhere since the base lands, both \
         kept ({})",
        res(&o).to_json()
    );
}

/// base_refuses_moved: the same node's same field changed since the base: `MPC-E723`, exit 1,
/// naming it with the field, who changed it and in which version; nothing written. Plants:
/// `base-ignored`, `plan-on-stale-head`.
#[test]
fn base_refuses_moved() {
    let f = fx("moved");
    let s = f.open("a.idx", DOC, pack());
    let ann = f.join(&s, "ann", ParticipantKind::Person);
    let bot = f.join(&s, "bot", ParticipantKind::Agent);
    let v = head(&s).to_string();
    call(&s, ann, &["part", "set", "@n:2", "note", "x"]);
    let h = head(&s);
    let o = call(&s, bot, &["part", "set", "@n:2", "note", "y", "--base", &v]);
    let c0 = detail(&o, "conflicts")
        .and_then(|c| match c {
            Value::List(l) => l.first().cloned(),
            _ => None,
        })
        .map(|c| c.to_json());
    let want = format!(
        r#"{{"address":"@n:2","root":"index","key":"@n:2","field":"attrs.note","by":"ann","change":{h}}}"#
    );
    assert_eq!(
        (
            err_id(&o),
            o.exit,
            c0,
            head(&s),
            field(&s, bot, "@n:2", NOTE)
        ),
        (
            Some("MPC-E723".to_owned()),
            1,
            Some(want),
            h,
            Some("x".to_owned())
        ),
        "base_refuses_moved: --base per node: an overlapping change since the base refuses \
         E723, naming the node, field, author and version; nothing written"
    );
}

/// base_field_level: since the base, one node's note changed and another's text; the call
/// sets the first's tag and the second's note: both land (attributes are fields; text is a
/// field of its own). Plant: `base-node-level-only`.
#[test]
fn base_field_level() {
    let f = fx("field");
    let s = f.open("a.idx", DOC, pack());
    let ann = f.join(&s, "ann", ParticipantKind::Person);
    let bot = f.join(&s, "bot", ParticipantKind::Agent);
    let v = head(&s).to_string();
    call(&s, ann, &["part", "set", "@n:2", "note", "x"]);
    call(&s, ann, &["part", "set", "@n:3", "text", "t"]);
    let a = call(&s, bot, &["part", "set", "@n:2", "tag", "y", "--base", &v]);
    let b = call(&s, bot, &["part", "set", "@n:3", "note", "z", "--base", &v]);
    let got = (
        err_id(&a),
        err_id(&b),
        field(&s, bot, "@n:2", NOTE),
        field(&s, bot, "@n:2", TAG),
        field(&s, bot, "@n:3", TEXT),
        field(&s, bot, "@n:3", NOTE),
    );
    let some = |x: &str| Some(x.to_owned());
    assert_eq!(
        got,
        (None, None, some("x"), some("y"), some("t"), some("z")),
        "base_field_level: --base per node: another attribute, and text against another field, \
         land"
    );
}

/// base_selector_at_v: the call selects by name; since the base the node it selected was
/// renamed, so at head it selects nothing: the call run at the base still sees it, and
/// refuses `MPC-E723`. Plant: `base-head-only`.
#[test]
fn base_selector_at_v() {
    let f = fx("selector");
    let s = f.open("a.idx", DOC, pack());
    let ann = f.join(&s, "ann", ParticipantKind::Person);
    let bot = f.join(&s, "bot", ParticipantKind::Agent);
    let v = head(&s).to_string();
    call(&s, ann, &["part", "set", "@n:2", "name", "Gate"]);
    let h = head(&s);
    let o = call(
        &s,
        bot,
        &["part", "named", "Door", "note", "y", "--base", &v],
    );
    let addresses = detail(&o, "addresses").map(Value::to_json);
    assert_eq!(
        (err_id(&o), addresses, head(&s)),
        (
            Some("MPC-E723".to_owned()),
            Some(r#"["@n:2"]"#.to_owned()),
            h
        ),
        "base_selector_at_v: --base per node: a node that left the selection since the base \
         refuses E723 (the call is run at the base too)"
    );
}

/// base_created_keys_never_conflict: since the base someone added a node with the next key;
/// the call adds one too: at the base it would take that key, at head the one after. Keys a
/// verb creates never conflict: it lands. Plant: `created-keys-conflict`.
#[test]
fn base_created_keys_never_conflict() {
    let f = fx("created");
    let s = f.open("a.idx", DOC, pack());
    let ann = f.join(&s, "ann", ParticipantKind::Person);
    let bot = f.join(&s, "bot", ParticipantKind::Agent);
    let v = head(&s).to_string();
    call(&s, ann, &["part", "new", "line", "A"]);
    let o = call(&s, bot, &["part", "new", "line", "B", "--base", &v]);
    let names = (field(&s, bot, "@n:5", 2), field(&s, bot, "@n:6", 2));
    assert_eq!(
        (err_id(&o), names),
        (None, (Some("A".to_owned()), Some("B".to_owned()))),
        "base_created_keys_never_conflict: --base per node: two adds land, each with its own key \
         ({})",
        res(&o).to_json()
    );
}

/// base_unproven_writer: another program saved the file since the base and the pack doesn't
/// prove it keeps keys: `MPC-E719`, naming the save; a pack that proves it: the call lands.
/// Plant: `pairing-assumed`.
#[test]
fn base_unproven_writer() {
    let other = DOC.replace("window|-", "window|saved");
    let run = |pairing: Option<SavePairing>| {
        let f = fx("unproven");
        let s = f.open("a.idx", DOC, IxPack { pairing, ..pack() });
        let bot = f.join(&s, "bot", ParticipantKind::Agent);
        let v = head(&s).to_string();
        f.outside(&s, "a.idx", &other);
        let save = head(&s);
        let o = call(&s, bot, &["part", "set", "@n:2", "note", "y", "--base", &v]);
        let save_named = detail(&o, "save").and_then(Value::as_u64) == Some(save);
        (err_id(&o), save_named, field(&s, bot, "@n:2", NOTE))
    };
    let unknown = run(None);
    let proven = run(Some(SavePairing::Proven));
    assert_eq!(
        (unknown, proven),
        (
            (Some("MPC-E719".to_owned()), true, None),
            (None, false, Some("y".to_owned()))
        ),
        "base_unproven_writer: --base per node: across another program's save the pack doesn't \
         prove, E719 naming it; proven, it lands"
    );
}

/// weak_key_if_version: a pack whose nodes carry no keys of their own: `--base` on a node the
/// kit keyed refuses `MPC-E724`, exit 2; the same call with `--if-version` head lands. Plant:
/// `weak-key-guarded`.
#[test]
fn weak_key_if_version() {
    let f = fx("weak");
    let s = f.open("a.idx", KEYLESS, pack());
    let ann = f.join(&s, "ann", ParticipantKind::Person);
    let bot = f.join(&s, "bot", ParticipantKind::Agent);
    let v = head(&s).to_string();
    call(&s, ann, &["part", "named", "Window", "note", "x"]);
    let o = call(
        &s,
        bot,
        &["part", "named", "Door", "note", "y", "--base", &v],
    );
    let refused = (err_id(&o), o.exit, field(&s, bot, "Door", NOTE));
    let h = head(&s).to_string();
    let g = call(
        &s,
        bot,
        &["part", "named", "Door", "note", "y", "--if-version", &h],
    );
    assert_eq!(
        (refused, err_id(&g), field(&s, bot, "Door", NOTE)),
        (
            (Some("MPC-E724".to_owned()), 2, None),
            None,
            Some("y".to_owned())
        ),
        "weak_key_if_version: --base per node: a key the kit minted refuses E724; --if-version \
         lands"
    );
}

/// base_dry_run_refuses_alike: the dry run of each of the three refusals refuses as the real
/// run does (id, exit, details), and nothing is written.
#[test]
fn base_dry_run_refuses_alike() {
    type Setup = fn(&Fx, &BlobSession, ConnId);
    let moved: Setup = |_, s, c| {
        call(s, c, &["part", "set", "@n:2", "note", "x"]);
    };
    let saved: Setup = |f, s, _| f.outside(s, "a.idx", &DOC.replace("window|-", "window|saved"));
    let elsewhere: Setup = |_, s, c| {
        call(s, c, &["part", "named", "Window", "note", "x"]);
    };
    let fixtures: [(&str, &str, Setup, &[&str]); 3] = [
        (
            "MPC-E723",
            DOC,
            moved,
            &["part", "set", "@n:2", "note", "y"],
        ),
        (
            "MPC-E719",
            DOC,
            saved,
            &["part", "set", "@n:2", "note", "y"],
        ),
        (
            "MPC-E724",
            KEYLESS,
            elsewhere,
            &["part", "named", "Door", "note", "y"],
        ),
    ];
    let mut got = Vec::new();
    for (id, doc, setup, a) in fixtures {
        let f = fx("dry-base");
        let s = f.open("a.idx", doc, pack());
        let ann = f.join(&s, "ann", ParticipantKind::Person);
        let bot = f.join(&s, "bot", ParticipantKind::Agent);
        let v = head(&s).to_string();
        setup(&f, &s, ann);
        let h = head(&s);
        let mut real: Vec<&str> = a.to_vec();
        real.extend(["--base", v.as_str()]);
        let mut dry = real.clone();
        dry.push("--dry-run");
        let p = call(&s, bot, &dry);
        let r = call(&s, bot, &real);
        let alike = ["exit"].iter().all(|k| res(&p).get(k) == res(&r).get(k))
            && res(&p).get("error").map(Value::to_json) == res(&r).get("error").map(Value::to_json);
        got.push((id, err_id(&r).as_deref() == Some(id), alike, head(&s) == h));
    }
    assert_eq!(
        got,
        vec![
            ("MPC-E723", true, true, true),
            ("MPC-E719", true, true, true),
            ("MPC-E724", true, true, true)
        ],
        "base_dry_run_refuses_alike: --base per node: each refusal, dry run and real run alike, \
         nothing written"
    );
}

/// Run `a` as agent `c` until it isn't refused for the other agent's lease (each verb holds a
/// lease of its own while it runs) or for head's index not recorded yet: the result.
fn until_unleased(s: &BlobSession, c: ConnId, a: &[String]) -> ToolOutput {
    let refs: Vec<&str> = a.iter().map(String::as_str).collect();
    for _ in 0..2_000 {
        let o = call(s, c, &refs);
        if !matches!(err_id(&o).as_deref(), Some("MPC-E701" | "MPC-E713")) {
            return o;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!("the other agent's lease never ended")
}

/// Two agents' calls on one session, from two threads released together; each retries while
/// the other's verb holds the file. Their results, A's then B's.
fn race(
    s: &Arc<BlobSession>,
    a: (ConnId, Vec<String>),
    b: (ConnId, Vec<String>),
) -> [ToolOutput; 2] {
    let go = Arc::new(Barrier::new(2));
    let run = |(c, args): (ConnId, Vec<String>)| {
        let (s, go) = (s.clone(), go.clone());
        std::thread::spawn(move || {
            go.wait();
            until_unleased(&s, c, &args)
        })
    };
    let (ta, tb) = (run(a), run(b));
    [ta.join().unwrap(), tb.join().unwrap()]
}

/// two_agents_one_drawing (the CAD verbs design's race row, on the test pack): agents A and B
/// read version V and call with `--base V` at once, from two threads.
/// 1. different nodes: both land;
/// 2. the same node and field: one lands, the other refuses `MPC-E723` naming the winner; the
///    node changed once;
/// 3. the same node, different fields: both land;
/// 4. concurrent adds: both land, each with its own key.
///
/// 5. a hold: A holds a layer; B's change to a node on it refuses `MPC-E301` naming A.
///
/// Step 6 (a verbatim retry answered `replayed`, no new version) is a known gap: it fails if
/// it starts passing. Plants: `base-whole-doc`, `base-ignored`, `base-node-level-only`,
/// `base-head-only`, `created-keys-conflict`, `plan-on-stale-head`, `reach-ignored`.
#[test]
fn two_agents_one_drawing() {
    let f = fx("race");
    let s = f.open("a.idx", DOC, pack());
    let a = f.join(&s, "agent-a", ParticipantKind::Agent);
    let b = f.join(&s, "agent-b", ParticipantKind::Agent);
    let with = |v: VersionId, x: &[&str]| {
        let mut x = argv(x);
        x.extend(["--base".to_owned(), v.to_string()]);
        x
    };
    // 1. different nodes
    let v = head(&s);
    let step1_a = with(v, &["part", "set", "@n:2", "note", "a1"]);
    let [r1a, r1b] = race(
        &s,
        (a, step1_a.clone()),
        (b, with(v, &["part", "set", "@n:3", "note", "b1"])),
    );
    let step1 = (
        err_id(&r1a),
        err_id(&r1b),
        head(&s) - v,
        field(&s, a, "@n:2", NOTE),
        field(&s, a, "@n:3", NOTE),
    );
    // 2. the same node and field
    let v = head(&s);
    let [r2a, r2b] = race(
        &s,
        (a, with(v, &["part", "set", "@n:4", "note", "a2"])),
        (b, with(v, &["part", "set", "@n:4", "note", "b2"])),
    );
    let (won, lost, winner) = if err_id(&r2a).is_none() {
        (&r2a, &r2b, "agent-a")
    } else {
        (&r2b, &r2a, "agent-b")
    };
    let by = detail(lost, "conflicts").and_then(|c| match c {
        Value::List(l) => l
            .first()?
            .get("by")
            .and_then(Value::as_str)
            .map(str::to_owned),
        _ => None,
    });
    let step2 = (
        err_id(won),
        err_id(lost),
        by.as_deref() == Some(winner),
        head(&s) - v,
    );
    // 3. the same node, different fields
    let v = head(&s);
    let [r3a, r3b] = race(
        &s,
        (a, with(v, &["part", "set", "@n:1", "note", "a3"])),
        (b, with(v, &["part", "set", "@n:1", "tag", "b3"])),
    );
    let step3 = (
        err_id(&r3a),
        err_id(&r3b),
        field(&s, a, "@n:1", NOTE),
        field(&s, a, "@n:1", TAG),
    );
    // 4. concurrent adds
    let v = head(&s);
    let [r4a, r4b] = race(
        &s,
        (a, with(v, &["part", "new", "line", "A"])),
        (b, with(v, &["part", "new", "line", "B"])),
    );
    let mut added = vec![field(&s, a, "@n:5", 2), field(&s, a, "@n:6", 2)];
    added.sort();
    let step4 = (err_id(&r4a), err_id(&r4b), head(&s) - v, added);
    // 5. a hold: A puts @n:3 on layer @n:1 and holds the layer; B's change to it refuses
    call(&s, a, &["part", "set", "@n:3", "tag", "@n:1"]);
    hold(&s, a, &["@n:1"]);
    let v = head(&s).to_string();
    let step5 = refused_by(&call_respect(
        &s,
        b,
        &["part", "set", "@n:3", "note", "b5", "--base", &v],
    ));
    // 6. (known gap) A's step 1 call again, verbatim: not yet answered `replayed`
    let before = head(&s);
    let again = call(
        &s,
        a,
        &step1_a.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    let replayed = again.exit == 0 && head(&s) == before;
    let some = |x: &str| Some(x.to_owned());
    assert_eq!(
        (step1, step2, step3, step4, step5, replayed),
        (
            (None, None, 2, some("a1"), some("b1")),
            (None, Some("MPC-E723".to_owned()), true, 1),
            (None, None, some("a3"), some("b3")),
            (None, None, 2, vec![some("A"), some("B")]),
            (some("MPC-E301"), some("agent-a")),
            false
        ),
        "two_agents_one_drawing: --base per node: different nodes land; the same field: one \
         lands, the other E723 naming the winner; different fields land; two adds land; a hold's \
         reach covers a layer's members (a verbatim retry isn't answered replayed yet: a known \
         gap)"
    );
}

/// A call's outcome as the gap rows record it: landed, or the refusal's id.
fn landed_or(o: &ToolOutput) -> Result<(), String> {
    match err_id(o) {
        None => Ok(()),
        Some(id) => Err(id),
    }
}

/// seed_gap_race: agent B's verb runs in the gap after agent A's verb committed, before A's
/// version is indexed from its output: it lands, or refuses `MPC-E713` and lands on retry;
/// never a lost edit (both edits are in the file, counted by key). Plant: `seed-gap-loses`.
#[test]
fn seed_gap_race() {
    let f = fx("gap");
    let s = f.open("a.idx", DOC, pack());
    let a = f.join(&s, "agent-a", ParticipantKind::Agent);
    let b = f.join(&s, "agent-b", ParticipantKind::Agent);
    let in_gap: Rc<RefCell<Option<Result<(), String>>>> = Rc::new(RefCell::new(None));
    let (s2, gap) = (s.clone(), in_gap.clone());
    let ra = k2_mpc_base::differential::with_seed_gap_hook(
        move || {
            let o = call(&s2, b, &["part", "set", "@n:3", "note", "b"]);
            *gap.borrow_mut() = Some(landed_or(&o));
        },
        || call(&s, a, &["part", "set", "@n:2", "note", "a"]),
    );
    let first = in_gap.borrow().clone();
    // B's retry, once A's call returned
    let retried = match &first {
        Some(Err(id)) if id == "MPC-E713" => Some(landed_or(&call(
            &s,
            b,
            &["part", "set", "@n:3", "note", "b"],
        ))),
        _ => None,
    };
    let landed = matches!(first, Some(Ok(()))) || retried == Some(Ok(()));
    let applied = [
        field(&s, a, "@n:2", NOTE).as_deref() == Some("a"),
        field(&s, a, "@n:3", NOTE).as_deref() == Some("b"),
    ];
    assert_eq!(
        (landed_or(&ra), first.is_some(), landed, applied),
        (Ok(()), true, true, [true, true]),
        "seed_gap_race: a call in the gap lands, or E713 then lands on retry: never a lost edit \
         (first {first:?}, retry {retried:?})"
    );
}

/// wait_and_retry_hides_gap: a host's loop as K2's tool runner does it (on `MPC-E713`, wait
/// once for head's index, then call again), run in the gap after another agent's verb: it
/// always lands, and both edits are kept. Plant: `seed-gap-loses`.
#[test]
fn wait_and_retry_hides_gap() {
    let f = fx("wait");
    let s = f.open("a.idx", DOC, pack());
    let a = f.join(&s, "agent-a", ParticipantKind::Agent);
    let b = f.join(&s, "agent-b", ParticipantKind::Agent);
    let (tx, rx) = std::sync::mpsc::channel::<Option<String>>();
    let loop_thread: Rc<RefCell<Option<std::thread::JoinHandle<Option<String>>>>> =
        Rc::new(RefCell::new(None));
    let (s2, held) = (s.clone(), loop_thread.clone());
    let ra = k2_mpc_base::differential::with_seed_gap_hook(
        move || {
            let s3 = s2.clone();
            let t = std::thread::spawn(move || {
                let args = ["part", "set", "@n:3", "note", "b"];
                let o = call(&s3, b, &args);
                let first = err_id(&o);
                tx.send(first.clone()).unwrap();
                if first.as_deref() != Some("MPC-E713") {
                    return first;
                }
                // wait once for head's index, then call again
                for _ in 0..2_000 {
                    if matches!(s3.status().unwrap().index, Some(IndexStatus::Ready { .. })) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                err_id(&call(&s3, b, &args))
            });
            // the loop's first call is made in the gap
            rx.recv().unwrap();
            *held.borrow_mut() = Some(t);
        },
        || call(&s, a, &["part", "set", "@n:2", "note", "a"]),
    );
    let t = loop_thread.borrow_mut().take();
    let last = t.map(|t| t.join().unwrap());
    let applied = [
        field(&s, a, "@n:2", NOTE).as_deref() == Some("a"),
        field(&s, a, "@n:3", NOTE).as_deref() == Some("b"),
    ];
    assert_eq!(
        (err_id(&ra), last, applied),
        (None, Some(None), [true, true]),
        "wait_and_retry_hides_gap: a wait-once-then-retry loop in the gap always lands: never a \
         lost edit"
    );
}

/// Nodes on two layers (by `tag`), and a block with a node under it.
const HELD: &str = "IDX\n@n:1|layer|Walls|-|1|-|-|-\n@n:2|line|Door|-|1|-|door|-|@n:1\n\
                    @n:3|line|Window|-|1|-|window|-|@n:6\n@n:4|block|Frame|-|1|-|-|-\n\
                    @n:5|line|Jamb|3|1|-|jamb|-\n@n:6|layer|Glass|-|1|-|-|-\n";

/// hold_layer_covers_members: a person holds a layer; an agent's change to a node on it
/// refuses `MPC-E301` naming the holder, a node put on the layer after the hold too (the reach
/// is read at the version the verb runs on); a node on another layer lands; a person's change
/// is never refused. Plants: `reach-ignored`, `reach-at-hold-time`.
#[test]
fn hold_layer_covers_members() {
    let f = fx("hold-layer");
    let s = f.open("a.idx", HELD, pack());
    let ann = f.join(&s, "ann", ParticipantKind::Person);
    let cy = f.join(&s, "cy", ParticipantKind::Person);
    let bot = f.join(&s, "bot", ParticipantKind::Agent);
    hold(&s, ann, &["@n:1"]);
    let member = refused_by(&call_respect(
        &s,
        bot,
        &["part", "set", "@n:2", "note", "x"],
    ));
    // a node put on the layer after the hold
    call(&s, ann, &["part", "new", "line", "Late"]);
    call(&s, ann, &["part", "set", "@n:7", "tag", "@n:1"]);
    let late = refused_by(&call_respect(
        &s,
        bot,
        &["part", "set", "@n:7", "note", "y"],
    ));
    let neighbour = err_id(&call_respect(
        &s,
        bot,
        &["part", "set", "@n:3", "note", "z"],
    ));
    let person = err_id(&call_respect(&s, cy, &["part", "set", "@n:2", "note", "w"]));
    let some = |x: &str| Some(x.to_owned());
    assert_eq!(
        (member, late, neighbour, person),
        (
            (some("MPC-E301"), some("ann")),
            (some("MPC-E301"), some("ann")),
            None,
            None
        ),
        "hold_layer_covers_members: a hold's reach: a layer hold refuses an agent's change to \
         any node on it, one added after the hold too; a neighbour and a person land"
    );
}

/// hold_block_covers_subtree: a person holds a block; an agent's change to a node under it
/// refuses `MPC-E301` naming the holder; a node outside it lands. Plant: `reach-ignored`.
#[test]
fn hold_block_covers_subtree() {
    let f = fx("hold-block");
    let s = f.open("a.idx", HELD, pack());
    let ann = f.join(&s, "ann", ParticipantKind::Person);
    let bot = f.join(&s, "bot", ParticipantKind::Agent);
    hold(&s, ann, &["@n:4"]);
    let under = refused_by(&call_respect(
        &s,
        bot,
        &["part", "set", "@n:5", "note", "x"],
    ));
    let outside = err_id(&call_respect(
        &s,
        bot,
        &["part", "set", "@n:3", "note", "y"],
    ));
    assert_eq!(
        (under, outside),
        ((Some("MPC-E301".to_owned()), Some("ann".to_owned())), None),
        "hold_block_covers_subtree: a hold's reach: a block hold refuses an agent's change \
         under it; a node outside lands"
    );
}
