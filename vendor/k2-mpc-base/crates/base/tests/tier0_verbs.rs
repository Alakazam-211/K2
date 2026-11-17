//! The tier-0 verb path (`docs/SESSIONS.md`, "Verbs on a blob doc"): a file type's own verb,
//! run by the kit on a blob doc, through `BlobSession::run_tool`. The pack is the index test
//! pack ([`index_pack`]) with its verbs: the kit applies their plans, checks the output with the
//! pack's own index, records the verb, and keeps the dry-run contract. Public API only.
//!
//! Each row names the plant (`MPC_T0V_MUTANT`, `k2_mpc_base::differential::Tier0VerbMutant`)
//! it catches; the plants part runs each one alone over this file.

use std::cell::Cell;
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

mod index_pack;
use index_pack::{set_hook, IxPack, PLANS, VERB_SET};

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::errors::{capabilities, Value};
use k2_mpc_base::index::{Digest, IndexSpec, NodeChangeKind, NodeKey};
use k2_mpc_base::parity::{PersonAction, Registry, Source, VerbKind};
use k2_mpc_base::session::{
    AgentWrites, AuditEvent, BlobPack, BlobPlan, BlobSession, BlobVerb, BlobVerbCtx, BlobVerbOut,
    BlobVerbRefused, ClaimPolicy, Config, ConnId, Host, Participant, ParticipantKind, Rights,
    SessionManager, Stay, VerbInput, VerbMerge,
};
use k2_mpc_base::store::{DocKey, DocStore, VersionId};
use k2_mpc_base::tools::{ToolOptions, ToolOutput};

// ---------- the fixture ----------

struct TestHost {
    now: AtomicU64,
    rights: Mutex<HashMap<String, Rights>>,
    events: Mutex<Vec<AuditEvent>>,
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
    fn on_event(&self, ev: &AuditEvent) {
        self.events.lock().unwrap().push(ev.clone());
    }
}

struct Fx {
    dir: PathBuf,
    ws: PathBuf,
    host: Arc<TestHost>,
    store: Arc<DocStore>,
    blobs: Arc<BlobStore>,
    config: Config,
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
        "k2mb-t0v-{tag}-{}-{}",
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
        events: Mutex::new(Vec::new()),
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
    c.blob_verb_max_spans = 4;
    tweak(&mut c);
    let mgr =
        SessionManager::new(store.clone(), host.clone(), c.clone()).with_blob_store(blobs.clone());
    Fx {
        dir,
        ws,
        host,
        store,
        blobs,
        config: c,
        mgr,
    }
}

fn key(p: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: p.into(),
    }
}

/// The test pack with its verbs, and a merge.
fn pack() -> IxPack {
    IxPack {
        verbs: VERB_SET,
        merge: true,
        ..IxPack::default()
    }
}

/// Three nodes; each node's line follows a doc attribute's line, and one ends the file.
const DOC: &str = "IDX\n#attr a=0123456789\n@n:1|layer|Walls|-|1|-|-|-\n#attr b=0123456789\n\
                   @n:2|line|L2|0|1|-|-|-\n#attr c=0123456789\n@n:3|line|L3|0|1|-|-|-\n\
                   #attr d=0123456789\n";
const LINE2: &str = "@n:2|line|L2|0|1|-|-|-";

/// Index the current version.
fn index(s: &BlobSession) {
    if let Some(j) = s.index_job(None).unwrap() {
        j.run().unwrap();
    }
}

impl Fx {
    /// A doc with no file, indexed.
    fn open(&self, name: &str, bytes: &str, pack: IxPack) -> Arc<BlobSession> {
        let s = self
            .mgr
            .open_blob(Arc::new(pack), &key(name), Some(&mut bytes.as_bytes()))
            .unwrap();
        index(&s);
        s
    }
    /// A doc of a file on disk, indexed.
    fn open_file(&self, name: &str, bytes: &str, pack: IxPack) -> Arc<BlobSession> {
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
    fn events(&self) -> usize {
        self.host.events.lock().unwrap().len()
    }
}

fn argv(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| (*s).to_owned()).collect()
}

/// A call with its result as data (every outcome is one object).
fn call_with(s: &BlobSession, c: ConnId, a: &[&str], opts: ToolOptions) -> ToolOutput {
    s.run_tool(c, &argv(a), &opts.result_json(true))
        .expect("with the result asked for, every outcome is a result")
}

fn call(s: &BlobSession, c: ConnId, a: &[&str]) -> ToolOutput {
    call_with(s, c, a, ToolOptions::new())
}

fn res(o: &ToolOutput) -> &Value {
    o.result().expect("a result")
}

fn at<'a>(v: &'a Value, path: &[&str]) -> &'a Value {
    path.iter().fold(v, |v, k| {
        v.get(k)
            .unwrap_or_else(|| panic!("no {k} in {}", v.to_json()))
    })
}

fn err_id(o: &ToolOutput) -> Option<String> {
    res(o)
        .get("error")
        .and_then(|e| e.get("id"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn after_of(o: &ToolOutput) -> Option<VersionId> {
    at(res(o), &["version", "after"]).as_u64()
}

fn bytes_at(s: &BlobSession, c: ConnId, v: VersionId) -> Vec<u8> {
    let mut r = s.read_version(c, v).unwrap();
    let mut out = Vec::new();
    r.reader.read_to_end(&mut out).unwrap();
    out
}

fn sha(b: &[u8]) -> String {
    let d = Digest::of(b);
    let mut s = "sha256:".to_owned();
    for x in d.as_bytes() {
        s.push_str(&format!("{x:02x}"));
    }
    s
}

/// Where `a` and `b` differ: the length of their common prefix, and of their common suffix
/// after it.
fn differing(a: &[u8], b: &[u8]) -> (usize, usize) {
    let p = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let q = a[p..]
        .iter()
        .rev()
        .zip(b[p..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    (p, q)
}

/// Assert the bytes `input` and `stored` differ only inside `input[line_at..line_end]`.
fn only_inside(input: &[u8], stored: &[u8], line_at: usize, line_end: usize, row: &str) {
    let (p, q) = differing(input, stored);
    assert!(
        p >= line_at && input.len() - q <= line_end,
        "{row}: the bytes outside the spans are the input's (they differ at {p}..{}, the span \
         is {line_at}..{line_end})",
        input.len() - q
    );
}

/// The changed units of a result, as (address, field).
fn units(v: &Value) -> BTreeSet<(String, Option<String>)> {
    let Some(Value::List(us)) = v.get("changed").and_then(|c| c.get("units")) else {
        return BTreeSet::new();
    };
    us.iter()
        .map(|u| {
            (
                u.get("address")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                u.get("field").and_then(Value::as_str).map(str::to_owned),
            )
        })
        .collect()
}

/// The units an index diff of two stored versions gives, as (address, field): computed here,
/// apart from the kit's own result code.
fn diff_units(
    s: &BlobSession,
    c: ConnId,
    from: VersionId,
    to: VersionId,
) -> BTreeSet<(String, Option<String>)> {
    let d = s.index_diff(c, from, to).unwrap();
    let mut out = BTreeSet::new();
    for ch in &d.changes {
        let k = ch.key.as_str().to_owned();
        let whole = [
            NodeChangeKind::Added,
            NodeChangeKind::Removed,
            NodeChangeKind::Moved,
            NodeChangeKind::Renamed,
            NodeChangeKind::Content,
            NodeChangeKind::Text,
        ];
        if ch.changes.iter().any(|x| whole.contains(x)) {
            out.insert((k.clone(), None));
        }
        for p in &ch.props {
            out.insert((k.clone(), Some(p.clone())));
        }
    }
    out
}

// ---------- the kit rows ----------

/// copy_exact: the bytes outside the plan's ranges are the input's, by an independent diff of
/// the input and the stored output. Plant: `applier-off-by-one`.
#[test]
fn copy_exact() {
    let f = fx("copy", |_| {});
    let s = f.open("a.idx", DOC, pack());
    let c = f.join(&s, "ann", ParticipantKind::Person);
    let h = s.status().unwrap().version;
    let o = call(&s, c, &["part", "set", "@n:2", "name", "Wall two"]);
    assert_eq!(
        o.exit,
        0,
        "copy_exact: the verb lands: {}",
        res(&o).to_json()
    );
    let after = after_of(&o).expect("copy_exact: a version");
    let input = bytes_at(&s, c, h);
    let stored = bytes_at(&s, c, after);
    // the independent diff: the input with the one line replaced, byte for byte
    let want = String::from_utf8_lossy(&input).replace(LINE2, "@n:2|line|Wall two|0|1|-|-|-");
    assert_eq!(
        String::from_utf8_lossy(&stored),
        want,
        "copy_exact: the bytes outside the spans are the input's"
    );
    println!("copy_exact: {} bytes compared, one span", stored.len());
}

/// plan_checked: unsorted, overlapping, out-of-file and over-cap plans each refuse E725, and
/// the version stays. Plant: `plan-unchecked`.
#[test]
fn plan_checked() {
    let f = fx("plan", |_| {});
    let s = f.open("a.idx", DOC, pack());
    let c = f.join(&s, "ann", ParticipantKind::Person);
    let h = s.status().unwrap().version;
    let mut got = Vec::new();
    for a in [
        &["part", "bad", "unsorted", "@n:2", "@n:3"][..],
        &["part", "bad", "overlap", "@n:2"],
        &["part", "bad", "outside"],
        &["part", "bad", "many"],
    ] {
        let o = call(&s, c, a);
        got.push((err_id(&o), o.exit, s.status().unwrap().version == h));
    }
    let want = vec![(Some("MPC-E725".to_owned()), 2, true); 4];
    assert_eq!(
        got, want,
        "plan_checked: each bad plan is refused as a bad plan (E725), exit 2, nothing written"
    );
    println!("plan_checked: {} bad plans refused", got.len());
}

/// strayed_refused: a verb changing a node it didn't declare refuses E726, naming it, and
/// nothing is written. Plant: `stray-unchecked`.
#[test]
fn strayed_refused() {
    let f = fx("stray", |_| {});
    let s = f.open("a.idx", DOC, pack());
    let c = f.join(&s, "ann", ParticipantKind::Person);
    let h = s.status().unwrap().version;
    let o = call(&s, c, &["part", "bad", "stray", "@n:2", "@n:3"]);
    let keys = res(&o)
        .get("error")
        .and_then(|e| e.get("details"))
        .and_then(|d| d.get("keys"))
        .map(Value::to_json);
    let got = (err_id(&o), keys, s.status().unwrap().version == h);
    let want = (
        Some("MPC-E726".to_owned()),
        Some(r#"["@n:3"]"#.to_owned()),
        true,
    );
    assert_eq!(
        got, want,
        "strayed_refused: a neighbour's change refuses E726, naming it; nothing written"
    );
    println!("strayed_refused: 1 strayed node");
}

/// output_checked: an output the pack's own index refuses gives E727; nothing is written.
/// Plant: `output-index-skipped`.
#[test]
fn output_checked() {
    let f = fx("output", |_| {});
    let s = f.open("a.idx", DOC, pack());
    let c = f.join(&s, "ann", ParticipantKind::Person);
    let h = s.status().unwrap().version;
    let o = call(&s, c, &["part", "bad", "garble"]);
    let got = (err_id(&o), s.status().unwrap().version == h);
    assert_eq!(
        got,
        (Some("MPC-E727".to_owned()), true),
        "output_checked: an output the index refuses gives E727; nothing written"
    );
    println!("output_checked: 1 output refused");
}

/// units_from_index: the result's changed units are the index diff of the two stored
/// versions, not the pack's `touched`. Plant: `units-from-plan`.
#[test]
fn units_from_index() {
    let f = fx("units", |_| {});
    let s = f.open("a.idx", DOC, pack());
    let c = f.join(&s, "ann", ParticipantKind::Person);
    let h = s.status().unwrap().version;
    let o = call(&s, c, &["part", "set", "@n:2", "note", "hello"]);
    assert_eq!(o.exit, 0, "units_from_index: lands: {}", res(&o).to_json());
    let after = after_of(&o).unwrap();
    index(&s);
    let want = diff_units(&s, c, h, after);
    let n = want.len();
    assert!(n > 0, "units_from_index: the diff has units");
    let u = at(res(&o), &["changed", "units"]).to_json();
    let values = u.contains(r#""before":null,"after":"hello""#);
    assert_eq!(
        (units(res(&o)), values),
        (want, true),
        "units_from_index: the changed units are the index diff's (by address and field, and \
         the attribute's values)"
    );
    println!("units_from_index: {n} units");
}

/// lease_only: a `merge: Lease` verb without the caller's own lease refuses E718; with it, it
/// lands and says so; under another's lease, E701 exit 5. Plant: `lease-only-ignored`.
#[test]
fn lease_only() {
    let f = fx("lease", |_| {});
    let s = f.open("a.idx", DOC, pack());
    let ann = f.join(&s, "ann", ParticipantKind::Person);
    let bob = f.join(&s, "bob", ParticipantKind::Person);
    let h = s.status().unwrap().version;
    let a = ["part", "guard", "@n:2", "note", "x"];
    let mut got = Vec::new();
    let o = call(&s, ann, &a);
    got.push((err_id(&o), o.exit, s.status().unwrap().version == h));
    s.acquire_lease(ann, None).unwrap();
    let o = call(&s, ann, &a);
    let noted = at(res(&o), &["notes"]).to_json().contains("lease_only");
    got.push((err_id(&o), o.exit, noted));
    s.release_lease(ann).unwrap();
    s.acquire_lease(bob, None).unwrap();
    let o = call(&s, ann, &a);
    got.push((err_id(&o), o.exit, true));
    let e = |id: &str| Some(id.to_owned());
    let want = vec![
        (e("MPC-E718"), 2, true),
        (None, 0, true),
        (e("MPC-E701"), 5, true),
    ];
    assert_eq!(
        got, want,
        "lease_only: without the caller's own lease, E718; under it, it lands and says \
         lease_only; under another's, E701 exit 5"
    );
    println!("lease_only: 3 cases");
}

/// A pack with no structure index and the given verbs.
struct NoIndex(&'static [BlobVerb]);
impl BlobPack for NoIndex {
    fn verbs(&self) -> &'static [BlobVerb] {
        self.0
    }
}

/// A pack with an index, the given verbs and refusals.
struct Verbs(
    &'static [BlobVerb],
    bool,
    &'static [k2_mpc_base::errors::ErrorEntry],
);
impl BlobPack for Verbs {
    fn index_spec(&self) -> Option<IndexSpec> {
        Some(IndexSpec::new("idx", 1, &index_pack::VERBS))
    }
    fn can_merge(&self) -> bool {
        self.1
    }
    fn verbs(&self) -> &'static [BlobVerb] {
        self.0
    }
    fn errors(&self) -> &'static [k2_mpc_base::errors::ErrorEntry] {
        self.2
    }
}

fn noop(
    _: &BlobVerbCtx<'_>,
    _: &mut dyn Read,
    _: &[String],
) -> Result<BlobVerbOut, BlobVerbRefused> {
    Ok(BlobVerbOut::edit(BlobPlan::new(), ""))
}

const E: &[PersonAction] = &[PersonAction::Rewrite];

macro_rules! verbs {
    ($($v:expr),* $(,)?) => {{
        static V: &[BlobVerb] = &[$($v),*];
        V
    }};
}

const fn ed(name: &'static str) -> BlobVerb {
    BlobVerb::edit(name, "", E, VerbMerge::Pack, VerbInput::Stream, noop).with_word("set")
}

/// verbs_only_admits_verbs: a pack whose agents change it only through its verbs
/// (`AgentWrites::VerbsOnly`): an agent's whole-file write is refused (`MPC-E715`) and nothing
/// is written; its verb lands, and a dry run of the verb answers as the real run. Plant:
/// `verbs-only-check-in-verb-path`.
#[test]
fn verbs_only_admits_verbs() {
    let f = fx("verbs-only", |_| {});
    let s = f.open(
        "a.idx",
        DOC,
        IxPack {
            agent_writes: Some(AgentWrites::VerbsOnly),
            ..pack()
        },
    );
    let bot = f.join(&s, "bot", ParticipantKind::Agent);
    let v = s.status().unwrap().version;
    let write = s
        .write_bytes(bot, format!("{DOC}#attr w=1\n").as_bytes(), v)
        .err()
        .map(|e| e.report().id().to_owned());
    let unwritten = s.status().unwrap().version == v;
    let a = ["part", "set", "@n:2", "note", "x"];
    let dry: Vec<&str> = a.iter().copied().chain(["--dry-run"]).collect();
    let p = call(&s, bot, &dry);
    let r = call(&s, bot, &a);
    let alike = ["exit", "changed"]
        .iter()
        .all(|k| res(&p).get(k) == res(&r).get(k));
    assert_eq!(
        (write, unwritten, err_id(&p), err_id(&r), alike),
        (Some("MPC-E715".to_owned()), true, None, None, true),
        "verbs_only_admits_verbs: an agent's whole-file write refuses E715, nothing written; its \
         verb lands, the dry run alike (a dry run commits nothing)"
    );
}

/// refused_and_allowed_unchanged: `AgentWrites::Refused` refuses an agent's write and its verb
/// (`MPC-E715`); `AgentWrites::Allowed` admits both.
#[test]
fn refused_and_allowed_unchanged() {
    let run = |w: AgentWrites| {
        let f = fx("agent-writes", |_| {});
        let s = f.open(
            "a.idx",
            DOC,
            IxPack {
                agent_writes: Some(w),
                ..pack()
            },
        );
        let bot = f.join(&s, "bot", ParticipantKind::Agent);
        let v = s.status().unwrap().version;
        let write = s
            .write_bytes(bot, format!("{DOC}#attr w=1\n").as_bytes(), v)
            .err()
            .map(|e| e.report().id().to_owned());
        // the written version's index, as the host's job records it
        index(&s);
        let verb = err_id(&call(&s, bot, &["part", "set", "@n:2", "note", "x"]));
        (write, verb)
    };
    let e715 = Some("MPC-E715".to_owned());
    assert_eq!(
        (run(AgentWrites::Refused), run(AgentWrites::Allowed)),
        ((e715.clone(), e715), (None, None)),
        "refused_and_allowed_unchanged: Refused refuses an agent's write and verb (E715); \
         Allowed admits both"
    );
}

/// verb_word: an edit verb's past-tense word is in `capabilities` and in its result
/// (`changed.word`), on a dry run and a real run alike; the registry refuses an edit verb with
/// no word, or one not lowercase. Plant: `word-dropped`.
#[test]
fn verb_word() {
    let r = Registry::for_blob_pack(&pack()).expect("verb_word: the test pack's verbs");
    let caps = capabilities(&[&r], &Config::default()).to_json();
    let listed = caps.contains(r#""input":"stream","word":"set""#);
    let f = fx("word", |_| {});
    let s = f.open("a.idx", DOC, pack());
    let c = f.join(&s, "ann", ParticipantKind::Person);
    let word = |o: &ToolOutput| {
        res(o)
            .get("changed")
            .and_then(|ch| ch.get("word"))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let dry = word(&call(
        &s,
        c,
        &["part", "set", "@n:2", "note", "x", "--dry-run"],
    ));
    let real = word(&call(&s, c, &["part", "set", "@n:2", "note", "x"]));
    let none = Registry::for_blob_pack(&Verbs(
        verbs![BlobVerb::edit(
            "part x",
            "",
            E,
            VerbMerge::Pack,
            VerbInput::Stream,
            noop
        )],
        true,
        &[],
    ))
    .is_err();
    let upper = Registry::for_blob_pack(&Verbs(verbs![ed("part x").with_word("Moved")], true, &[]))
        .is_err();
    assert_eq!(
        (listed, dry, real, none, upper),
        (
            true,
            Some("set".to_owned()),
            Some("set".to_owned()),
            true,
            true
        ),
        "verb_word: the word is in capabilities and in the result, a dry run's and the same \
         call's real run (a dry run commits nothing); the registry refuses each bad pack: an \
         edit verb with no word or one not lowercase"
    );
}

/// registry_rules: the registry lists a blob pack's verbs (kind, merge, input; in
/// `capabilities` too) and refuses: `merge: Pack` on a pack that can't merge, an edit verb
/// with no index, a name clash, twice, serving nothing, an exit outside the table, and a
/// refusal whose id isn't the pack's. A pack it refuses runs nothing. Plant:
/// `registry-unchecked`.
#[test]
fn registry_rules() {
    let r = Registry::for_blob_pack(&pack()).expect("registry_rules: the test pack's verbs");
    let set = r
        .get("part set")
        .expect("registry_rules: part set is listed");
    assert_eq!(set.kind, VerbKind::Edit);
    assert_eq!(set.source, Source::Pack);
    assert!(set.runs_through_run_tool());
    let b = set.blob.expect("blob facts");
    assert_eq!((b.merge, b.input), (Some("pack"), "stream"));
    assert_eq!(
        r.get("part guard").unwrap().blob.unwrap().merge,
        Some("lease")
    );
    assert_eq!(r.get("part add").unwrap().blob.unwrap().input, "whole");
    let peek = r.get("part peek").unwrap();
    assert_eq!(
        (peek.kind, peek.blob.unwrap().merge),
        (VerbKind::Read, None)
    );
    assert_eq!(r.errors().len(), 1, "registry_rules: the pack's refusal");
    let caps = capabilities(&[&r], &Config::default()).to_json();
    assert!(
        caps.contains(r#""name":"part set","kind":"edit""#)
            && caps.contains(r#""merge":"pack","input":"stream""#),
        "registry_rules: capabilities lists merge and input"
    );
    let mut refused = 0;
    let bad: Vec<(&str, Box<dyn BlobPack>)> = vec![
        (
            "merge: Pack, no merge",
            Box::new(Verbs(VERB_SET, false, &[])),
        ),
        (
            "an edit verb with no index",
            Box::new(NoIndex(verbs![ed("part set")])),
        ),
        (
            "a kit verb's word",
            Box::new(Verbs(verbs![ed("cat it")], true, &[])),
        ),
        (
            "an index verb's name",
            Box::new(Verbs(verbs![ed("part list")], true, &[])),
        ),
        ("batch", Box::new(Verbs(verbs![ed("batch it")], true, &[]))),
        (
            "an annotation verb's word",
            Box::new(Verbs(verbs![ed("comment it")], true, &[])),
        ),
        (
            "outline",
            Box::new(Verbs(verbs![ed("outline it")], true, &[])),
        ),
        (
            "twice",
            Box::new(Verbs(verbs![ed("part x"), ed("part x")], true, &[])),
        ),
        (
            "three words",
            Box::new(Verbs(verbs![ed("part x y")], true, &[])),
        ),
        (
            "serves nothing",
            Box::new(Verbs(
                verbs![BlobVerb::edit(
                    "part x",
                    "",
                    &[],
                    VerbMerge::Pack,
                    VerbInput::Stream,
                    noop
                )],
                true,
                &[],
            )),
        ),
        (
            "an exit outside the table",
            Box::new(Verbs(verbs![ed("part x").with_exits(&[0, 9])], true, &[])),
        ),
        (
            "another pack's id",
            Box::new(Verbs(verbs![ed("part x")], true, {
                static X: &[k2_mpc_base::errors::ErrorEntry] =
                    &[k2_mpc_base::errors::ErrorEntry::pack(
                        "XYZ-E001", "x", 2, "x", "x",
                    )];
                X
            })),
        ),
    ];
    let mut missed: Vec<String> = Vec::new();
    for (why, p) in &bad {
        if Registry::for_blob_pack(p.as_ref()).is_ok() {
            missed.push((*why).to_owned());
        }
        refused += 1;
    }
    // a pack the registry refuses runs nothing
    let f = fx("registry", |_| {});
    let s = f.open(
        "a.idx",
        DOC,
        IxPack {
            verbs: VERB_SET,
            merge: false,
            ..IxPack::default()
        },
    );
    let c = f.join(&s, "ann", ParticipantKind::Person);
    let h = s.status().unwrap().version;
    let o = call(&s, c, &["part", "set", "@n:2", "note", "x"]);
    if o.exit != 2 || s.status().unwrap().version != h {
        missed.push("a refused pack's verb ran".to_owned());
    }
    assert_eq!(
        missed,
        Vec::<String>::new(),
        "registry_rules: the registry refuses each bad pack, and a refused pack runs nothing"
    );
    println!("registry_rules: {refused} packs refused");
}

/// verb_recorded: the log, the versions and node blame show the verb; it is in the store
/// (another manager reads it); a content purge keeps it; erasure clears it. Plants:
/// `verb-not-recorded-blob`, `verb-kept-on-erase`.
#[test]
fn verb_recorded() {
    let f = fx("record", |_| {});
    let s = f.open("a.idx", DOC, pack());
    let own = f.join(&s, "own", ParticipantKind::Person);
    let bot = f.join(&s, "bot", ParticipantKind::Agent);
    let o = call(&s, bot, &["part", "set", "@n:2", "name", "Two"]);
    assert_eq!(o.exit, 0, "verb_recorded: lands: {}", res(&o).to_json());
    let v = after_of(&o).unwrap();
    let set = Some("part set");
    let mut checks: Vec<(&str, bool)> = Vec::new();
    let log = s.log(own).unwrap();
    let e = log.iter().find(|e| e.id == v).unwrap();
    checks.push(("the log shows the verb", e.verb.as_deref() == set));
    checks.push(("no calls but a batch's", e.calls.is_empty()));
    let vs = s.versions(own).unwrap();
    checks.push((
        "the version shows it",
        vs.last().unwrap().1.verb.as_deref() == set,
    ));
    checks.push(("the import has none", vs[0].1.verb.is_none()));
    index(&s);
    let bl = s.index_blame(own, None).unwrap();
    let row = |k: &str| bl.rows.iter().find(|r| r.key.as_str() == k).unwrap();
    let two = row("@n:2");
    checks.push((
        "node blame shows the verb",
        (two.version, two.verb.as_deref()) == (v, set),
    ));
    checks.push(("a node it didn't make has none", row("@n:1").verb.is_none()));
    // another manager over the same store reads it back
    drop(s);
    f.mgr.release().unwrap();
    let mgr = SessionManager::new(f.store.clone(), f.host.clone(), f.config.clone())
        .with_blob_store(f.blobs.clone());
    let s = mgr
        .open_blob(Arc::new(pack()), &key("a.idx"), None)
        .unwrap();
    let own = s
        .join(
            Participant::new("own", "own", "#1", ParticipantKind::Person, "cli"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    let entry = |s: &BlobSession| s.log(own).unwrap().into_iter().find(|e| e.id == v).unwrap();
    checks.push(("after a restart", entry(&s).verb.as_deref() == set));
    // a content purge keeps it
    let head = s.status().unwrap().version;
    s.write_bytes(own, DOC.as_bytes(), head).unwrap();
    s.purge(own, Some(v)).unwrap();
    let e = entry(&s);
    checks.push((
        "a content purge keeps it",
        e.redacted && e.verb.as_deref() == set,
    ));
    // erasure clears it
    s.erase_participant(own, "bot", "erased-1").unwrap();
    checks.push(("erasure clears it", entry(&s).verb.is_none()));
    let failed: Vec<&str> = checks
        .iter()
        .filter(|(_, ok)| !ok)
        .map(|(n, _)| *n)
        .collect();
    assert_eq!(
        failed,
        Vec::<&str>::new(),
        "verb_recorded: the verb's record holds (log, versions, blame, restart, purge, erasure)"
    );
    println!("verb_recorded: {} checks", checks.len());
}

/// head_moved_mid_plan: head moves between the plan and the commit (an app saves the file
/// while the verb plans): one more plan on the new head, then E110; the change applies at
/// most once. Plant: `commit-without-recheck`.
#[test]
fn head_moved_mid_plan() {
    let f = fx("moved", |_| {});
    let s = f.open_file("m.idx", DOC, pack());
    let c = f.join(&s, "ann", ParticipantKind::Person);
    let path = f.ws.join("m.idx");
    // the app saves on every plan (a longer name each time), and the save is indexed
    let saves = Rc::new(Cell::new(0usize));
    let hook = |limit: usize| {
        let (s2, p, n) = (s.clone(), path.clone(), saves.clone());
        Box::new(move || {
            if n.get() >= limit {
                return;
            }
            n.set(n.get() + 1);
            let text = DOC.replace("|L3|", &format!("|L3{}|", "x".repeat(n.get())));
            fs::write(&p, text).unwrap();
            s2.file_changed().unwrap();
            index(&s2);
        }) as Box<dyn FnMut()>
    };
    set_hook(Some(hook(usize::MAX)));
    let plans = PLANS.load(Ordering::SeqCst);
    let o = call(&s, c, &["part", "set", "@n:2", "name", "Two"]);
    set_hook(None);
    let planned = PLANS.load(Ordering::SeqCst) - plans;
    let now = bytes_at(&s, c, s.status().unwrap().version);
    let every = (
        err_id(&o),
        planned,
        String::from_utf8_lossy(&now).contains("|Two|"),
    );
    // head moves once: the second plan lands on the new head, once
    saves.set(0);
    set_hook(Some(hook(1)));
    let o = call(&s, c, &["part", "set", "@n:2", "name", "Two"]);
    set_hook(None);
    let now = after_of(&o)
        .map(|v| String::from_utf8_lossy(&bytes_at(&s, c, v)).into_owned())
        .unwrap_or_default();
    let once = (o.exit, now.matches("|Two|").count(), now.contains("|L3x|"));
    assert_eq!(
        (every, once),
        ((Some("MPC-E110".to_owned()), 2, false), (0, 1, true)),
        "head_moved_mid_plan: head moved on every plan: one more plan, then stale (E110), \
         nothing applied; moved once: it lands on the new head, applied once, the app's save kept"
    );
    println!("head_moved_mid_plan: 2 cases, {} saves", saves.get());
}

/// claims_respected: under `ClaimPolicy::Respect`, an agent's verb touching a node another
/// participant claims refuses E301 (exit 5); a node nobody claims lands; under `Ignore`, it
/// lands. Plant: `claim-unchecked`.
#[test]
fn claims_respected() {
    let f = fx("claims", |_| {});
    let s = f.open("a.idx", DOC, pack());
    let ann = f.join(&s, "ann", ParticipantKind::Person);
    let bot = f.join(&s, "bot", ParticipantKind::Agent);
    s.claim_nodes(ann, &[NodeKey::parse("@n:2").unwrap()], None, None)
        .unwrap();
    let respect = ToolOptions::new().policy(ClaimPolicy::Respect);
    let h = s.status().unwrap().version;
    let o = call_with(&s, bot, &["part", "set", "@n:2", "note", "x"], respect);
    let claimed = (err_id(&o), o.exit, s.status().unwrap().version == h);
    let o = call_with(&s, bot, &["part", "set", "@n:3", "note", "x"], respect);
    let unclaimed = o.exit;
    // a verb reads head's index (the verb's own version is seeded: this runs nothing)
    index(&s);
    let o = call(&s, bot, &["part", "set", "@n:2", "note", "x"]);
    assert_eq!(
        (claimed, unclaimed, o.exit),
        ((Some("MPC-E301".to_owned()), 5, true), 0, 0),
        "claims_respected: under Respect, another's claim on a touched node refuses E301 (exit \
         5, nothing written); an unclaimed node lands; under Ignore it lands"
    );
    println!("claims_respected: 3 cases");
}

/// merge_bound_shown: `status.merge` gives the bound; every verb result (a read, an edit) on a
/// doc over it carries `merge_over_bound`; under it, none; a pack that can't merge has none.
/// Plant: `merge-note-missing`.
#[test]
fn merge_bound_shown() {
    let f = fx("bound", |c| c.blob_merge_max_bytes = 64);
    let s = f.open("a.idx", DOC, pack());
    let c = f.join(&s, "ann", ParticipantKind::Person);
    let mut checks: Vec<(&str, bool)> = Vec::new();
    let m = s.status().unwrap().merge;
    checks.push((
        "status.merge",
        m.map(|m| (m.max_bytes, m.size, m.fits)) == Some((64, DOC.len() as u64, false)),
    ));
    let read = call(&s, c, &["part", "peek", "@n:2"]);
    let data = at(res(&read), &["data"]).to_json();
    checks.push((
        "the info read carries the bound",
        data.contains(r#""fits":false"#),
    ));
    let edit = call(&s, c, &["part", "set", "@n:2", "note", "x"]);
    for (what, o) in [("a read's note", &read), ("an edit's note", &edit)] {
        let notes = at(res(o), &["notes"]).to_json();
        checks.push((what, notes.contains("merge_over_bound")));
    }
    let o = call(&s, c, &["part", "peek", "@n:2", "--if-version", "1"]);
    checks.push(("a read refuses a guard", o.exit == 2));
    // under the bound: no note
    let g = fx("bound-under", |_| {});
    let s = g.open("a.idx", DOC, pack());
    let c = g.join(&s, "ann", ParticipantKind::Person);
    checks.push((
        "fits under it",
        s.status().unwrap().merge.is_some_and(|m| m.fits),
    ));
    let o = call(&s, c, &["part", "peek", "@n:2"]);
    let notes = at(res(&o), &["notes"]).to_json();
    checks.push(("no note under it", !notes.contains("merge_over_bound")));
    // a pack that can't merge: none
    let plain = IxPack {
        merge: false,
        ..IxPack::default()
    };
    let s = g.open("b.idx", DOC, plain);
    checks.push(("none without a merge", s.status().unwrap().merge.is_none()));
    let failed: Vec<&str> = checks
        .iter()
        .filter(|(_, ok)| !ok)
        .map(|(n, _)| *n)
        .collect();
    assert_eq!(
        failed,
        Vec::<&str>::new(),
        "merge_bound_shown: the bound in status and the info read, merge_over_bound on a verb \
         over the bound, none under it"
    );
    println!("merge_bound_shown: {} checks", checks.len());
}

// ---------- the dry-run contract on tier 0 ----------

/// dry_run_matches_real: a dry run's result is the real run's (with `--if-version H`) but for
/// `dry_run` and `version.after`; its change matches the real run's effect computed apart: the
/// bytes outside the ranges, the index diff of the stored versions, the stored digest. Plants:
/// `commits`, `units-from-plan`, `applier-off-by-one`.
#[test]
fn dry_run_matches_real() {
    let f = fx("dry-real", |_| {});
    let s = f.open("a.idx", DOC, pack());
    let c = f.join(&s, "ann", ParticipantKind::Person);
    let h = s.status().unwrap().version;
    let p = call(
        &s,
        c,
        &["part", "set", "@n:2", "note", "hello", "--dry-run"],
    );
    assert_eq!(
        p.exit,
        0,
        "dry_run_matches_real: the dry run runs: {}",
        res(&p).to_json()
    );
    let hs = h.to_string();
    let r = call(
        &s,
        c,
        &["part", "set", "@n:2", "note", "hello", "--if-version", &hs],
    );
    assert_eq!(
        r.exit,
        0,
        "dry_run_matches_real: the real run with --if-version H lands (a dry run commits \
         nothing): {}",
        res(&r).to_json()
    );
    let (pv, rv) = (res(&p), res(&r));
    assert_eq!(pv.get("dry_run"), Some(&Value::Bool(true)));
    assert_eq!(rv.get("dry_run"), Some(&Value::Bool(false)));
    assert_eq!(
        at(pv, &["version", "after"]),
        &Value::Null,
        "dry_run_matches_real: no version"
    );
    for k in [
        "exit",
        "data",
        "error",
        "conflicts",
        "notes",
        "warnings",
        "changed",
    ] {
        assert_eq!(
            pv.get(k),
            rv.get(k),
            "dry_run_matches_real: {k} is the real run's"
        );
    }
    let after = after_of(&r).unwrap();
    let input = bytes_at(&s, c, h);
    let stored = bytes_at(&s, c, after);
    let line_at = DOC.find(LINE2).unwrap();
    only_inside(
        &input,
        &stored,
        line_at,
        line_at + LINE2.len(),
        "dry_run_matches_real",
    );
    let b = at(pv, &["changed", "bytes"]);
    assert_eq!(
        b.get("after_digest").and_then(Value::as_str),
        Some(sha(&stored).as_str()),
        "dry_run_matches_real: the digest is the stored bytes'"
    );
    assert_eq!(
        b.get("size_after").and_then(Value::as_u64),
        Some(stored.len() as u64)
    );
    assert_eq!(
        b.get("size_before").and_then(Value::as_u64),
        Some(input.len() as u64)
    );
    index(&s);
    assert_eq!(
        units(pv),
        diff_units(&s, c, h, after),
        "dry_run_matches_real: the changed units are the index diff's"
    );
    println!(
        "dry_run_matches_real: {} units, {} bytes",
        units(pv).len(),
        stored.len()
    );
}

/// dry_run_leaves_no_trace: after a dry run the version, the log, the blob versions, the file
/// (bytes and stat), the lease, the rate bucket, presence, claims, events, the index and the
/// blob store's open puts are as they were. Plants: `commits`, `takes-lease`,
/// `charges-rate`, `moves-presence`, `stores-index`, `leaves-put`, `writes-back`.
#[test]
fn dry_run_leaves_no_trace() {
    // one edit token, never refilled within the test's millisecond
    let f = fx("dry-trace", |c| {
        c.edit_burst = 1;
        c.edit_rate_per_s = 1;
    });
    let s = f.open_file("t.idx", DOC, pack());
    let c = f.join(&s, "ann", ParticipantKind::Agent);
    let path = f.ws.join("t.idx");
    let stat = |p: &PathBuf| {
        let m = fs::metadata(p).unwrap();
        (m.len(), m.ino(), m.mtime(), m.mtime_nsec())
    };
    let me = |s: &BlobSession| {
        s.roster(c)
            .unwrap()
            .into_iter()
            .find(|e| e.participant.pid == "ann")
            .map(|e| (e.last_edit_ms, e.holds_lease))
            .unwrap()
    };
    let version = s.status().unwrap().version;
    let (log, vers) = (s.log(c).unwrap().len(), s.versions(c).unwrap().len());
    let (bytes, st) = (fs::read(&path).unwrap(), stat(&path));
    let (lease, presence, claims) = (s.lease().unwrap(), me(&s), s.claims(c).unwrap().len());
    let (events, index_now, puts) = (
        f.events(),
        s.index_status(None).unwrap(),
        f.blobs.waiting_puts(),
    );
    let p = call(
        &s,
        c,
        &["part", "set", "@n:2", "note", "hello", "--dry-run"],
    );
    let seen: Vec<AuditEvent> = f.host.events.lock().unwrap()[events..].to_vec();
    assert_eq!(
        p.exit,
        0,
        "dry_run_leaves_no_trace: the dry run runs: {}",
        res(&p).to_json()
    );
    assert_eq!(
        s.status().unwrap().version,
        version,
        "dry_run_leaves_no_trace: the version is unchanged (a dry run commits nothing)"
    );
    assert_eq!(
        p.version_made(),
        None,
        "dry_run_leaves_no_trace: it tells no version (a dry run commits nothing)"
    );
    assert_eq!(
        s.log(c).unwrap().len(),
        log,
        "dry_run_leaves_no_trace: the log (a dry run commits nothing)"
    );
    assert_eq!(
        s.versions(c).unwrap().len(),
        vers,
        "dry_run_leaves_no_trace: the versions (a dry run commits nothing)"
    );
    assert_eq!(
        s.lease().unwrap(),
        lease,
        "dry_run_leaves_no_trace: a dry run takes no lease"
    );
    assert_eq!(
        me(&s),
        presence,
        "dry_run_leaves_no_trace: a dry run moves no presence"
    );
    assert_eq!(
        s.claims(c).unwrap().len(),
        claims,
        "dry_run_leaves_no_trace: claims"
    );
    assert!(
        seen.is_empty(),
        "dry_run_leaves_no_trace: a dry run emits no event: {seen:?}"
    );
    assert_eq!(
        s.index_status(None).unwrap(),
        index_now,
        "dry_run_leaves_no_trace: the index"
    );
    assert_eq!(
        f.blobs.waiting_puts(),
        puts,
        "dry_run_leaves_no_trace: a dry run leaves no open put"
    );
    assert_eq!(
        (fs::read(&path).unwrap(), stat(&path)),
        (bytes, st),
        "dry_run_leaves_no_trace: a dry run leaves the file as it was"
    );
    // charged as a read: the one edit token is still there for the real run
    let r = call(&s, c, &["part", "set", "@n:2", "note", "hello"]);
    assert_eq!(
        r.exit,
        0,
        "dry_run_leaves_no_trace: a dry run is charged as a read (the real run lands): {}",
        res(&r).to_json()
    );
    println!(
        "dry_run_leaves_no_trace: 11 checks, {} events before",
        events
    );
}

/// What a refusal fixture does before its calls.
type Setup = Box<dyn Fn(&Fx, &BlobSession, ConnId)>;

/// A refusal fixture: its name, the pack, the config, the setup, the call and its options.
type Fixture = (
    &'static str,
    IxPack,
    fn(&mut Config),
    Setup,
    Vec<&'static str>,
    ToolOptions,
);

/// dry_run_refuses_alike: for each refusal fixture, the dry run's error is the real run's
/// (id, exit, `next_step`, `conflicts`). Plant: `skips-guards`.
#[test]
fn dry_run_refuses_alike() {
    let none = || -> Setup { Box::new(|_, _, _| {}) };
    let fixtures: Vec<Fixture> = vec![
        (
            "E701 another's lease",
            pack(),
            |_| {},
            Box::new(|f, s, _| {
                let b = f.join(s, "bob", ParticipantKind::Person);
                s.acquire_lease(b, None).unwrap();
            }),
            vec!["part", "set", "@n:2", "note", "x"],
            ToolOptions::new(),
        ),
        (
            "E110",
            pack(),
            |_| {},
            none(),
            vec!["part", "set", "@n:2", "note", "x", "--if-version", "99"],
            ToolOptions::new(),
        ),
        (
            "E718",
            pack(),
            |_| {},
            none(),
            vec!["part", "guard", "@n:2", "note", "x"],
            ToolOptions::new(),
        ),
        (
            "E301 under Respect",
            pack(),
            |_| {},
            Box::new(|f, s, _| {
                let b = f.join(s, "bob", ParticipantKind::Person);
                s.claim_nodes(b, &[NodeKey::parse("@n:2").unwrap()], None, None)
                    .unwrap();
            }),
            vec!["part", "set", "@n:2", "note", "x"],
            ToolOptions::new().policy(ClaimPolicy::Respect),
        ),
        (
            "E725",
            pack(),
            |_| {},
            none(),
            vec!["part", "bad", "outside"],
            ToolOptions::new(),
        ),
        (
            "E726",
            pack(),
            |_| {},
            none(),
            vec!["part", "bad", "stray", "@n:2", "@n:3"],
            ToolOptions::new(),
        ),
        (
            "E727",
            pack(),
            |_| {},
            none(),
            vec!["part", "bad", "garble"],
            ToolOptions::new(),
        ),
        (
            "E728",
            pack(),
            |c| c.blob_verb_max_bytes = 16,
            none(),
            vec!["part", "add", "@n:9", "line", "L9"],
            ToolOptions::new(),
        ),
        (
            "E729",
            pack(),
            |_| {},
            none(),
            vec!["part", "set", "@n:2", "note", "x", "--expect", "sha256:00"],
            ToolOptions::new(),
        ),
        (
            "E715",
            IxPack {
                refuse_agents: true,
                ..pack()
            },
            |_| {},
            none(),
            vec!["part", "set", "@n:2", "note", "x"],
            ToolOptions::new(),
        ),
        (
            "the pack's refusal",
            pack(),
            |_| {},
            none(),
            vec!["part", "bad", "refuse", "@n:2"],
            ToolOptions::new(),
        ),
    ];
    let mut refused = 0;
    let mut differ = Vec::new();
    for (name, pk, tweak, setup, a, opts) in fixtures {
        let f = fx("dry-alike", tweak);
        let s = f.open("a.idx", DOC, pk);
        let c = f.join(&s, "bot", ParticipantKind::Agent);
        setup(&f, &s, c);
        let mut dry = a.clone();
        dry.push("--dry-run");
        let p = call_with(&s, c, &dry, opts);
        let r = call_with(&s, c, &a, opts);
        for k in ["error", "exit", "conflicts"] {
            if res(&p).get(k) != res(&r).get(k) {
                differ.push(format!("{name} ({k})"));
            }
        }
        if err_id(&r).is_some() {
            refused += 1;
        }
    }
    assert!(
        refused >= 1,
        "dry_run_refuses_alike: no fixture refused: the row is vacuous"
    );
    assert_eq!(
        differ,
        Vec::<String>::new(),
        "dry_run_refuses_alike: the dry run refuses as the real run does"
    );
    println!("dry_run_refuses_alike: {refused} fixtures refused alike");
}

/// dry_run_pack_is_pure: two identical dry runs, then the real run, agree. Plants:
/// `pack-impure`, `commits`.
#[test]
fn dry_run_pack_is_pure() {
    let f = fx("dry-pure", |_| {});
    let s = f.open("a.idx", DOC, pack());
    let c = f.join(&s, "ann", ParticipantKind::Person);
    let h = s.status().unwrap().version.to_string();
    let a = ["part", "set", "@n:2", "text", "hello"];
    let dry: Vec<&str> = a.iter().copied().chain(["--dry-run"]).collect();
    let p1 = call(&s, c, &dry);
    let p2 = call(&s, c, &dry);
    assert_eq!(
        p1.exit,
        0,
        "dry_run_pack_is_pure: the dry run runs: {}",
        res(&p1).to_json()
    );
    let real: Vec<&str> = a.iter().copied().chain(["--if-version", &h]).collect();
    let r = call(&s, c, &real);
    let agree: Vec<bool> = ["exit", "data", "changed"]
        .iter()
        .map(|k| res(&p1).get(k) == res(&r).get(k))
        .collect();
    assert_eq!(
        (res(&p1) == res(&p2), agree),
        (true, vec![true; 3]),
        "dry_run_pack_is_pure: two identical dry runs, then the real run, agree (the pack is \
         pure; a dry run commits nothing)"
    );
    println!("dry_run_pack_is_pure: 3 runs agree");
}

/// `--expect DIGEST`: the real run with the digest its dry run showed lands, and stores those
/// bytes (the refusal on other bytes is a fixture of `dry_run_refuses_alike`). Plant:
/// `commits`.
#[test]
fn expect_takes_the_previewed_digest() {
    let f = fx("expect", |_| {});
    let s = f.open("a.idx", DOC, pack());
    let c = f.join(&s, "ann", ParticipantKind::Person);
    let p = call(&s, c, &["part", "add", "@n:4", "line", "L4", "--dry-run"]);
    let d = at(res(&p), &["changed", "bytes", "after_digest"])
        .as_str()
        .unwrap()
        .to_owned();
    let r = call(
        &s,
        c,
        &["part", "add", "@n:4", "line", "L4", "--expect", &d],
    );
    let stored = after_of(&r).map(|v| sha(&bytes_at(&s, c, v)));
    let added = units(res(&r)).contains(&("@n:4".to_owned(), None));
    assert_eq!(
        (r.exit, stored, added),
        (0, Some(d), true),
        "expect: the real run with the previewed digest lands, stores those bytes and adds the \
         node (a dry run commits nothing): {}",
        res(&r).to_json()
    );
    println!("expect: 1 digest");
}

/// The kinds of the events the host heard since `from` (each event's variant name).
fn kinds_since(f: &Fx, from: usize) -> Vec<String> {
    f.host.events.lock().unwrap()[from..]
        .iter()
        .map(|e| {
            format!("{e:?}")
                .split(|c: char| !c.is_ascii_alphanumeric())
                .next()
                .unwrap_or("")
                .to_owned()
        })
        .collect()
}

/// verb_version_event: a verb that makes a version tells it as a whole-file write does. A
/// tier-0 doc has no kit event of its own for a new version: a host tells its clients from the
/// call's answer (`BlobWriteResult::{changed, version}` for a write, `ToolOutput::version_made`
/// for a verb). Here a verb's `version_made` is the version a write of the same bytes reports
/// on a twin doc, and the kit's events for the two calls (the index's aside: a verb's version is
/// indexed from its output) are the same kinds, in order (a dry run tells none:
/// `dry_run_leaves_no_trace`). Plant: `version-made-hidden`.
#[test]
fn verb_version_event() {
    let f = fx("version", |_| {});
    let s = f.open("a.idx", DOC, pack());
    let twin = f.open("b.idx", DOC, pack());
    let c = f.join(&s, "ann", ParticipantKind::Person);
    let d = f.join(&twin, "ann", ParticipantKind::Person);
    let a = ["part", "set", "@n:2", "name", "Two"];
    let from = f.events();
    let o = s.run_tool(c, &argv(&a), &ToolOptions::new()).unwrap();
    // the index events differ by design: the verb's version is indexed from its output
    // (IndexReady), and a write of the same bytes then finds that row (no IndexWanted)
    let not_index = |mut k: Vec<String>| {
        k.retain(|k| !k.starts_with("Index"));
        k
    };
    let verb = (o.version_made(), not_index(kinds_since(&f, from)));
    let bytes = bytes_at(&s, c, s.status().unwrap().version);
    let from = f.events();
    let w = twin
        .write_bytes(d, &bytes, twin.status().unwrap().version)
        .unwrap();
    let write = (
        w.changed.then_some(w.version),
        not_index(kinds_since(&f, from)),
    );
    assert!(
        write.0.is_some(),
        "verb_version_event: the write made a version"
    );
    assert_eq!(
        verb, write,
        "verb_version_event: a verb's version is told as a write's (the same version, the same \
         events)"
    );
    println!("verb_version_event: {:?}", write.1);
}

/// Two layers, each with a line under it.
const DOC_VIEW: &str = "IDX\n#attr a=0123456789\n@n:1|layer|L1|-|1|-|-|-\n#attr b=0123456789\n\
                        @n:2|line|A|0|1|-|-|-\n#attr c=0123456789\n@n:4|layer|L2|-|1|-|-|-\n\
                        #attr d=0123456789\n@n:5|line|B|2|1|-|-|-\n#attr e=0123456789\n";

/// view_on_blob_doc: a verb's result says where its change is (typed reads' one shape): the
/// changed index nodes in the diff's order as units, their containers (here `layer` nodes) with
/// the last touched last, and the primary address, the container of the **last** changed node;
/// cells, lines and time null. A dry run's view is the real run's; a read has none. Plants:
/// `view-first-node`, `commits`.
#[test]
fn view_on_blob_doc() {
    let f = fx("view", |_| {});
    let s = f.open(
        "a.idx",
        DOC_VIEW,
        IxPack {
            view: true,
            ..pack()
        },
    );
    let c = f.join(&s, "bot", ParticipantKind::Agent);
    let h = s.status().unwrap().version.to_string();
    let view = |o: &ToolOutput| at(res(o), &["view"]).to_json();
    let read = call(&s, c, &["part", "peek", "@n:2"]);
    let a = ["part", "both", "@n:2", "@n:5", "x"];
    let dry: Vec<&str> = a.iter().copied().chain(["--dry-run"]).collect();
    let p = call(&s, c, &dry);
    let real: Vec<&str> = a.iter().copied().chain(["--if-version", &h]).collect();
    let r = call(&s, c, &real);
    let want = r#"{"address":"@n:4","containers":["@n:1","@n:4"],"units":[{"address":"@n:2","live":true},{"address":"@n:5","live":true}],"units_more":0,"cells":null,"lines":null,"time":null}"#;
    assert_eq!(
        (r.exit, view(&r), view(&p), view(&read)),
        (0, want.to_owned(), want.to_owned(), "null".to_owned()),
        "view_on_blob_doc: the view is the last changed node's place, with every changed node; \
         a dry run's is the real run's (a dry run commits nothing); a read has none"
    );
    println!("view_on_blob_doc: 2 units, 2 containers");
}

/// Three nodes and one with no key of its own (the kit mints one).
const DOC_MINTED: &str = "IDX\n#attr a=0123456789\n@n:1|layer|Walls|-|1|-|-|-\n\
                          #attr b=0123456789\n@n:2|line|L2|0|1|-|-|-\n-|line|Anon|0|1|-|-|-\n\
                          #attr c=0123456789\n";

/// seeded_index_is_the_jobs: the version a verb made is indexed at once, from the output its
/// check read, and the stored row and minted keys are byte for byte what the host's index job
/// records for that version (the job's reader on the stored bytes, its own row, keys and
/// limits). Plants: `seed-without-carry`, `seed-skipped`.
#[test]
fn seeded_index_is_the_jobs() {
    let f = fx("seed", |_| {});
    let s = f.open("a.idx", DOC_MINTED, pack());
    let c = f.join(&s, "ann", ParticipantKind::Person);
    let o = call(&s, c, &["part", "set", "@n:2", "name", "Two"]);
    assert_eq!(
        o.exit,
        0,
        "seeded_index_is_the_jobs: lands: {}",
        res(&o).to_json()
    );
    let v = after_of(&o).unwrap();
    let ((row, keys), (job_row, job_keys)) = s.index_rows_for_tests(v).unwrap();
    assert!(
        job_keys.is_some(),
        "seeded_index_is_the_jobs: the job mints keys here (the check isn't vacuous)"
    );
    assert_eq!(
        (row, keys),
        (Some(job_row), job_keys),
        "seeded_index_is_the_jobs: the seeded index is the job's (its row and minted keys)"
    );
    println!("seeded_index_is_the_jobs: version {v}");
}

/// next_verb_needs_no_job: an agent's second verb runs on the version its first made with no
/// index job run between them (its index was seeded), and the host's job finds nothing to do.
/// Plant: `seed-skipped`.
#[test]
fn next_verb_needs_no_job() {
    let f = fx("seed-next", |_| {});
    let s = f.open("a.idx", DOC, pack());
    let c = f.join(&s, "bot", ParticipantKind::Agent);
    let first = call(&s, c, &["part", "set", "@n:2", "note", "one"]);
    let second = call(&s, c, &["part", "set", "@n:3", "note", "two"]);
    let no_job = s.index_job(None).unwrap().is_none();
    assert_eq!(
        (first.exit, err_id(&second), second.exit, no_job),
        (0, None, 0, true),
        "next_verb_needs_no_job: the seeded index lets the next verb run at once: {}",
        res(&second).to_json()
    );
    println!("next_verb_needs_no_job: 2 verbs, no job");
}

/// The `blob_verbs` rows of the store at `path`, read raw: those of deleted docs, then those of
/// live ones.
/// Row keys of a per-doc table: `(doc, version)`.
type RowKeys = Vec<(u64, u64)>;

fn raw_blob_verbs(path: &std::path::Path) -> (RowKeys, RowKeys) {
    use redb::{Database, TableDefinition};
    const DOCS: TableDefinition<u64, &[u8]> = TableDefinition::new("docs");
    const VERBS: TableDefinition<(u64, u64), &[u8]> = TableDefinition::new("blob_verbs");
    let d = Database::open(path).unwrap();
    let r = d.begin_read().unwrap();
    let docs = r.open_table(DOCS).unwrap();
    let verbs = match r.open_table(VERBS) {
        Ok(t) => t,
        Err(redb::TableError::TableDoesNotExist(_)) => return (Vec::new(), Vec::new()),
        Err(e) => panic!("{e}"),
    };
    let (mut gone, mut live) = (Vec::new(), Vec::new());
    for kv in redb::ReadableTable::iter(&verbs).unwrap() {
        let (k, _) = kv.unwrap();
        if docs.get(k.value().0).unwrap().is_some() {
            live.push(k.value());
        } else {
            gone.push(k.value());
        }
    }
    (gone, live)
}

/// This build's store in `K2MB_N1_OUT` (docs.redb, blobs/), as the N−1 halves use it.
fn n1_store() -> (PathBuf, Arc<DocStore>, SessionManager) {
    let dir = PathBuf::from(std::env::var("K2MB_N1_OUT").expect("K2MB_N1_OUT"));
    fs::create_dir_all(&dir).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_793_000_000_000),
        rights: Mutex::new(HashMap::new()),
        events: Mutex::new(Vec::new()),
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
    let mgr = SessionManager::new(store.clone(), host, Config::default()).with_blob_store(blobs);
    (dir, store, mgr)
}

/// tools/n1 `node-verbs`: writes the store the previous release's consumer reads: two blob docs
/// (`keep.idx`, `gone.idx`), each with a version a verb made (a `blob_verbs` row).
#[test]
#[ignore = "tools/n1/n1.sh: writes the store for the previous release's consumer"]
fn n1_verbs_producer() {
    let (_, _, mgr) = n1_store();
    for name in ["keep.idx", "gone.idx"] {
        let s = mgr
            .open_blob(
                Arc::new(pack()),
                &key(name),
                Some(&mut &b"IDX\n@n:1|k|a|-|1|-|-\n"[..]),
            )
            .unwrap();
        index(&s);
        let c = s
            .join(
                Participant::new("ann", "ann", "#123456", ParticipantKind::Person, "cli"),
                Stay::Held,
            )
            .unwrap()
            .conn;
        let o = call(&s, c, &["part", "set", "@n:1", "note", "x"]);
        assert_eq!(o.exit, 0, "{name}: the verb lands: {}", res(&o).to_json());
    }
    mgr.release().unwrap();
    println!("N-1 VERBS (producer) OK");
}

/// tools/n1 `node-verbs`, after the previous release read, wrote, was refused a rewrite and
/// deleted `gone.idx`: this build's open sweeps the deleted doc's verb rows, `keep.idx`'s verb
/// reads back (the previous release's version beside it, with none), and the store rewrites.
#[test]
#[ignore = "tools/n1/n1.sh: reads the store the previous release's consumer left"]
fn n1_verbs_after() {
    let dir = PathBuf::from(std::env::var("K2MB_N1_OUT").expect("K2MB_N1_OUT"));
    let path = dir.join("docs.redb");
    let (gone_before, live_before) = raw_blob_verbs(&path);
    // this build's open sweeps what a delete by the previous release left
    let (_, store, mgr) = n1_store();
    mgr.release().unwrap();
    drop((store, mgr));
    let (gone_after, live_after) = raw_blob_verbs(&path);
    let (_, store, mgr) = n1_store();
    let s = mgr
        .open_blob(Arc::new(pack()), &key("keep.idx"), None)
        .unwrap();
    let c = s
        .join(
            Participant::new("ann", "ann", "#123456", ParticipantKind::Person, "cli"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    let verbs: Vec<Option<String>> = s
        .versions(c)
        .unwrap()
        .into_iter()
        .map(|(_, v)| v.verb)
        .collect();
    mgr.release().unwrap();
    drop((s, mgr));
    let rewrote = store.check_rewritable().is_ok() && store.rewrite_file().is_ok();
    assert_eq!(
        (
            gone_before.len(),
            live_before.len(),
            gone_after.len(),
            live_after.len(),
            verbs,
            rewrote
        ),
        (
            1,
            1,
            0,
            1,
            vec![None, Some("part set".to_owned()), None],
            true
        ),
        "n1_verbs_after: the deleted doc's verb row swept at the open, keep.idx's verb read \
         back, the store rewritten"
    );
    println!("N-1 VERBS (this build) OK");
}
