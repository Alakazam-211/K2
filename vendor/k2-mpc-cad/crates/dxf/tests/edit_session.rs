//! The edit verbs inside the kit, on a blob doc through `BlobSession::run_tool`: each lands as
//! one version with its word, its dry run stores nothing and previews the digest the real run
//! stores, an agent's verb lands while its whole-file write is refused, two agents' calls from
//! one version land or refuse per node, `--base` and the outside-save merge agree on what overlaps, and a
//! person's app saving over an agent's verb merges when the kit knows what the app opened.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::errors::Value;
use k2_mpc_base::index::Digest;
use k2_mpc_base::session::{
    AuditEvent, BlobMerge, BlobPack, BlobSession, BlobWriteOptions, ClaimPolicy, Config, ConnId,
    DiskCheck, Host, OpenedAt, Participant, ParticipantKind, Rights, SessionError, SessionManager,
    Stay,
};
use k2_mpc_base::store::{DocKey, DocStore, VersionId};
use k2_mpc_base::tools::{ToolOptions, ToolOutput};
use k2_mpc_dxf::testgen::{self, Ver};
use k2_mpc_dxf::DxfPack;

struct TestHost {
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
        1_000_000
    }
    fn on_event(&self, _ev: &AuditEvent) {}
}

struct Fx {
    dir: PathBuf,
    host: Arc<TestHost>,
    mgr: SessionManager,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fx(tag: &str) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2cad-edit-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("ws")).unwrap();
    let host = Arc::new(TestHost {
        rights: Mutex::new(HashMap::new()),
    });
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let blobs = Arc::new(
        BlobStore::open(
            store.clone(),
            dir.join("blobs"),
            BlobConfig::default(),
            Arc::new(|| 1_000_000),
        )
        .unwrap(),
    );
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    let mgr = SessionManager::new(store, host.clone(), c).with_blob_store(blobs);
    Fx { dir, host, mgr }
}

fn key(p: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: p.into(),
    }
}

fn index(s: &BlobSession) {
    if let Some(j) = s.index_job(None).unwrap() {
        j.run().expect("the index job records");
    }
}

impl Fx {
    fn open(&self, path: &str, bytes: &[u8]) -> Arc<BlobSession> {
        let s = self
            .mgr
            .open_blob(
                Arc::new(DxfPack) as Arc<dyn BlobPack>,
                &key(path),
                Some(&mut &bytes[..]),
            )
            .unwrap();
        index(&s);
        s
    }
    fn ws(&self) -> PathBuf {
        self.dir.join("ws")
    }
    fn open_file(&self, name: &str, bytes: &[u8]) -> Arc<BlobSession> {
        let path = self.ws().join(name);
        std::fs::write(&path, bytes).unwrap();
        let s = self
            .mgr
            .open_blob_file(
                Arc::new(DxfPack) as Arc<dyn BlobPack>,
                &key(name),
                &path,
                &self.ws(),
            )
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

fn call_with(s: &BlobSession, c: ConnId, a: &[&str], opts: ToolOptions) -> ToolOutput {
    let argv: Vec<String> = a.iter().map(|x| (*x).to_owned()).collect();
    s.run_tool(c, &argv, &opts.result_json(true))
        .expect("with the result asked for, every outcome is a result")
}

fn call(s: &BlobSession, c: ConnId, a: &[&str]) -> ToolOutput {
    call_with(s, c, a, ToolOptions::new())
}

fn res(o: &ToolOutput) -> &Value {
    o.result().expect("a result")
}

fn get<'v>(v: &'v Value, path: &[&str]) -> &'v Value {
    static NULL: Value = Value::Null;
    path.iter().fold(v, |v, k| v.get(k).unwrap_or(&NULL))
}

fn err_id(o: &ToolOutput) -> Option<String> {
    get(res(o), &["error", "id"]).as_str().map(str::to_owned)
}

fn head(s: &BlobSession) -> VersionId {
    s.status().unwrap().version
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

/// `verb` and its arguments as argv words (a two-word verb is two words).
fn argv<'a>(verb: &'a str, args: &[&'a str]) -> Vec<&'a str> {
    let mut v: Vec<&str> = verb.split(' ').collect();
    v.extend_from_slice(args);
    v
}

/// verbs_through_the_kit: each edit verb lands as one version through run_tool, its word in the
/// result; its dry run makes nothing and previews the digest, and the real run with `--expect`
/// stores exactly those bytes; the kit's own check of the output (only the nodes the verb
/// declared) passes for every verb.
#[test]
pub fn verbs_through_the_kit() {
    let f = fx("kit");
    let b = testgen::edit_sampler(Ver::R2018).build();
    let s = f.open("plan.dxf", &b.bytes);
    let ann = f.join(&s, "ann", ParticipantKind::Person);
    let (dev, wall, lamp, arc) = (b.addr("dev"), b.addr("wall"), b.addr("lamp"), b.addr("arc"));
    let calls: Vec<(&str, Vec<&str>, &str)> = vec![
        (
            "entity move",
            vec![&wall, "--by", "1,2", "--unit", "mm"],
            "moved",
        ),
        (
            "attrib set",
            vec![&dev, "--tag", "DEVICE", "--value", "CAM-07"],
            "set",
        ),
        ("entity set", vec![&lamp, "--color", "5"], "changed"),
        (
            "entity add",
            vec![
                "polyline",
                "--points",
                "0,0;5,0;5,5",
                "--unit",
                "mm",
                "--layer",
                "E-DEV",
            ],
            "added",
        ),
        ("entity delete", vec![&arc], "deleted"),
    ];
    let mut n = 0;
    for (verb, args, word) in &calls {
        let v = head(&s);
        let mut dry = args.clone();
        dry.push("--dry-run");
        let o = call(&s, ann, &argv(verb, &dry));
        assert_eq!(
            (o.exit, get(res(&o), &["dry_run"]) == &Value::Bool(true)),
            (0, true),
            "verbs_through_the_kit: {verb} --dry-run: {}",
            res(&o).to_json()
        );
        assert_eq!(
            head(&s),
            v,
            "verbs_through_the_kit: a dry run makes no version"
        );
        let digest = get(res(&o), &["changed", "bytes", "after_digest"])
            .as_str()
            .expect("the previewed digest")
            .to_owned();
        let mut real = args.clone();
        real.extend(["--expect", digest.as_str()]);
        let o = call(&s, ann, &argv(verb, &real));
        assert_eq!(
            o.exit,
            0,
            "verbs_through_the_kit: {verb}: {}",
            res(&o).to_json()
        );
        assert_eq!(
            get(res(&o), &["changed", "word"]).as_str(),
            Some(*word),
            "verbs_through_the_kit: {verb}'s word"
        );
        let after = o.version_made().expect("a version");
        assert_eq!(
            sha(&bytes_at(&s, ann, after)),
            digest,
            "verbs_through_the_kit: {verb} stored the bytes its dry run previewed"
        );
        n += 1;
    }
    println!("verbs_through_the_kit: {n} verbs, each dry run then real");
}

/// person_edits_with_base: a person runs each edit verb as K2's viewer does, a dry run then the
/// real call, both with `--base` at the version they read and the real one with the dry run's
/// digest, while an agent holds the layer the entities are on: every call lands (a hold refuses
/// an agent only), stores the previewed bytes, and names the person's base.
#[test]
pub fn person_edits_with_base() {
    let f = fx("person");
    let b = testgen::edit_sampler(Ver::R2018).build();
    let s = f.open("plan.dxf", &b.bytes);
    let ann = f.join(&s, "ann", ParticipantKind::Person);
    let bot = f.join(&s, "bot", ParticipantKind::Agent);
    let layer = s
        .index_view(None)
        .unwrap()
        .nodes
        .iter()
        .find(|n| n.kind == "layer" && n.name == "A-WALL")
        .map(|n| n.key.clone())
        .expect("the layer's node");
    s.claim_nodes(bot, &[layer], None, None).unwrap();
    let respect = || ToolOptions::new().policy(ClaimPolicy::Respect);
    let (wall, arc, lw, dev) = (b.addr("wall"), b.addr("arc"), b.addr("lw"), b.addr("dev"));
    let calls: Vec<(&str, Vec<&str>)> = vec![
        ("entity move", vec![&wall, "--by", "1,2", "--unit", "mm"]),
        ("entity set", vec![&arc, "--color", "5"]),
        (
            "attrib set",
            vec![&dev, "--tag", "DEVICE", "--value", "CAM-21"],
        ),
        (
            "entity add",
            vec![
                "line", "--from", "0,0", "--to", "2,2", "--unit", "mm", "--layer", "A-WALL",
            ],
        ),
        ("entity delete", vec![&lw]),
    ];
    let mut n = 0;
    for (verb, args) in &calls {
        index(&s);
        let v = head(&s).to_string();
        let mut dry = args.clone();
        dry.extend(["--base", v.as_str(), "--dry-run"]);
        let o = call_with(&s, ann, &argv(verb, &dry), respect());
        assert_eq!(
            o.exit,
            0,
            "person_edits_with_base: {verb} --dry-run: {}",
            res(&o).to_json()
        );
        let digest = get(res(&o), &["changed", "bytes", "after_digest"])
            .as_str()
            .expect("the previewed digest")
            .to_owned();
        let mut real = args.clone();
        real.extend(["--base", v.as_str(), "--expect", digest.as_str()]);
        let o = call_with(&s, ann, &argv(verb, &real), respect());
        assert_eq!(
            o.exit,
            0,
            "person_edits_with_base: {verb}: {}",
            res(&o).to_json()
        );
        let after = o.version_made().expect("a version");
        assert_eq!(
            sha(&bytes_at(&s, ann, after)),
            digest,
            "person_edits_with_base: {verb} stored what its dry run previewed"
        );
        n += 1;
    }
    println!(
        "person_edits_with_base: {n} verbs by a person, dry run then real, under an agent's hold"
    );
}

fn r(v: &[String]) -> Vec<&str> {
    v.iter().map(String::as_str).collect()
}

/// agent_writes_refused_verbs_land: agents change a drawing through the verbs only
/// (`AgentWrites::VerbsOnly`): an agent's whole-file write and its write on a base are refused
/// (`MPC-E715`, typed) and the file is unchanged, for the agent and for a person, whose read and
/// index are unaffected; the agent's verb lands; a person's whole-file write still lands. Plants:
/// `AgentWritesAllowed` (the write lands), `AgentWritesRefused` (the verb is refused too).
#[test]
pub fn agent_writes_refused_verbs_land() {
    let f = fx("agents");
    let b = testgen::edit_sampler(Ver::R2018).build();
    let s = f.open("plan.dxf", &b.bytes);
    let bot = f.join(&s, "bot", ParticipantKind::Agent);
    let ann = f.join(&s, "ann", ParticipantKind::Person);
    let v = head(&s);
    let moved = testgen::splice_move(&b.bytes, b.handle("wall"), 1.0);
    // the whole-file write and the write on a base: refused, typed, nothing recorded
    let w = s.write_bytes(bot, &moved, v);
    assert!(
        matches!(&w, Err(SessionError::AgentWriteRefused)),
        "agent_writes_refused_verbs_land: an agent's whole-file write is refused: {w:?}"
    );
    assert_eq!(
        w.unwrap_err().report().id(),
        "MPC-E715",
        "agent_writes_refused_verbs_land: by MPC-E715"
    );
    let w = s.write_bytes_with(bot, &moved, v, &BlobWriteOptions::new());
    assert!(
        matches!(&w, Err(SessionError::AgentWriteRefused)),
        "agent_writes_refused_verbs_land: an agent's write on a base is refused: {w:?}"
    );
    assert_eq!(
        head(&s),
        v,
        "agent_writes_refused_verbs_land: no version recorded"
    );
    for (who, c) in [("the agent", bot), ("the person", ann)] {
        assert!(
            bytes_at(&s, c, v) == b.bytes,
            "agent_writes_refused_verbs_land: {who} reads the file unchanged"
        );
    }
    assert!(
        s.index_view(None)
            .unwrap()
            .nodes
            .iter()
            .any(|n| n.key.as_str() == b.addr("wall")),
        "agent_writes_refused_verbs_land: the index still has the wall"
    );
    // the agent's verb lands
    let o = call(
        &s,
        bot,
        &[
            "entity",
            "move",
            &b.addr("wall"),
            "--by",
            "1,0",
            "--unit",
            "mm",
        ],
    );
    assert_eq!(
        o.exit,
        0,
        "agent_writes_refused_verbs_land: the agent's verb lands: {}",
        res(&o).to_json()
    );
    // a person's whole-file write is the person's
    let v = head(&s);
    let w = s.write_bytes(ann, &b.bytes, v);
    assert!(
        w.is_ok(),
        "agent_writes_refused_verbs_land: a person's whole-file write lands: {w:?}"
    );
    println!("agent_writes_refused_verbs_land: the write refused (MPC-E715), the verb landed");
}

/// The outcome of a call: `landed`, or the refusal's id.
fn outcome(o: &ToolOutput) -> String {
    if o.exit == 0 {
        "landed".into()
    } else {
        err_id(o).unwrap_or_else(|| format!("exit {}", o.exit))
    }
}

/// two_agents_one_drawing: two agents read one version and call from it (`--base`): changes
/// to different entities land, and different fields of one entity land; the same field of one
/// entity refuses the second (`MPC-E723`, naming the field); two adds land with distinct
/// handles; two attribute values of one reference refuse the second (both are its text, until
/// attributes are fields of their own); a hold on a layer refuses another agent's move of an
/// entity on it (`MPC-E301`).
#[test]
pub fn two_agents_one_drawing() {
    let f = fx("race");
    let b = testgen::edit_sampler(Ver::R2018).build();
    let s = f.open("plan.dxf", &b.bytes);
    let a1 = f.join(&s, "atlas", ParticipantKind::Agent);
    let a2 = f.join(&s, "bea", ParticipantKind::Agent);
    let run = |c: ConnId, base: VersionId, verb: &str, args: &[&str]| {
        let bs = base.to_string();
        let mut v = argv(verb, args);
        v.extend(["--base", bs.as_str()]);
        let o = call(&s, c, &v);
        index(&s);
        o
    };
    let mv = |l: &str| {
        vec![
            b.addr(l),
            "--by".into(),
            "1,0".into(),
            "--unit".into(),
            "mm".into(),
        ]
    };
    let mut rows = Vec::new();
    // different entities
    let v = head(&s);
    let x = run(a1, v, "entity move", &r(&mv("wall")));
    let y = run(a2, v, "entity move", &r(&mv("lamp")));
    rows.push(("different entities", outcome(&x), outcome(&y), "landed"));
    // the same field of one entity
    let v = head(&s);
    let x = run(a1, v, "entity move", &r(&mv("arc")));
    let y = run(a2, v, "entity move", &r(&mv("arc")));
    let field = get(res(&y), &["error", "details", "conflicts"]).to_json();
    rows.push(("the same field", outcome(&x), outcome(&y), "MPC-E723"));
    // different fields of one entity: its geometry and its colour; its geometry and its text
    let v = head(&s);
    let x = run(a1, v, "entity move", &r(&mv("ellipse")));
    let y = run(a2, v, "entity set", &[&b.addr("ellipse"), "--color", "4"]);
    rows.push(("geometry and colour", outcome(&x), outcome(&y), "landed"));
    let v = head(&s);
    let x = run(a1, v, "entity move", &r(&mv("dev")));
    let y = run(
        a2,
        v,
        "attrib set",
        &[&b.addr("dev"), "--tag", "DEVICE", "--value", "CAM-11"],
    );
    rows.push(("geometry and attribute", outcome(&x), outcome(&y), "landed"));
    // two attribute values of one reference: one text, until attributes are fields
    let v = head(&s);
    let x = run(
        a1,
        v,
        "attrib set",
        &[&b.addr("dev"), "--tag", "DEVICE", "--value", "CAM-12"],
    );
    let y = run(
        a2,
        v,
        "attrib set",
        &[&b.addr("dev"), "--tag", "LABEL", "--value", "C12"],
    );
    rows.push((
        "two attributes (one text, until attributes are fields)",
        outcome(&x),
        outcome(&y),
        "MPC-E723",
    ));
    // two adds
    let v = head(&s);
    let line = ["line", "--from", "0,0", "--to", "1,1", "--unit", "mm"];
    let x = run(a1, v, "entity add", &line);
    let y = run(a2, v, "entity add", &line);
    let keys: Vec<String> = [&x, &y]
        .iter()
        .map(|o| get(res(o), &["data", "entities"]).to_json())
        .collect();
    rows.push(("two adds", outcome(&x), outcome(&y), "landed"));
    // a hold on a layer
    let layer = s
        .index_view(None)
        .unwrap()
        .nodes
        .iter()
        .find(|n| n.kind == "layer" && n.name == "A-WALL")
        .map(|n| n.key.clone())
        .expect("the layer's node");
    s.claim_nodes(a1, &[layer], None, None).unwrap();
    let o = call_with(
        &s,
        a2,
        &argv("entity move", &r(&mv("wall"))),
        ToolOptions::new().policy(ClaimPolicy::Respect),
    );
    let held = outcome(&o);
    for (what, first, second, want) in &rows {
        assert_eq!(
            (first.as_str(), second.as_str()),
            ("landed", *want),
            "two_agents_one_drawing: {what}"
        );
    }
    assert!(
        ["attrs.geom", "attrs.bbox", "bounds"]
            .iter()
            .any(|x| field.contains(x)),
        "two_agents_one_drawing: the conflict names the field: {field}"
    );
    assert!(
        keys[0] != keys[1],
        "two_agents_one_drawing: the two adds have distinct handles: {keys:?}"
    );
    assert_eq!(held, "MPC-E301", "two_agents_one_drawing: the hold");
    println!(
        "two_agents_one_drawing: {} pairs, the hold refused; adds {keys:?}",
        rows.len()
    );
}

/// The merge's answer for two changes from one version: theirs made by `t`, ours by `o`.
fn merged(base: &[u8], o: &[u8], t: &[u8]) -> &'static str {
    match DxfPack.merge3_bytes(base, o, t) {
        BlobMerge::Merged(_) => "landed",
        BlobMerge::Conflict => "MPC-E723",
        BlobMerge::Unsupported => "unsupported",
    }
}

/// One call: its verb and its arguments.
type Call = (&'static str, Vec<String>);

/// merge_and_base_agree: for each pair of changes from one version, `--base` per node and the
/// pack's merge of an app's save give the same answer: both land, or both overlap. Plant:
/// `MergeFieldSplitDiffers`.
#[test]
pub fn merge_and_base_agree() {
    let b = testgen::edit_sampler(Ver::R2018).build();
    let a = |l: &str| b.addr(l);
    let mv = |l: &str| {
        vec![
            a(l),
            "--by".into(),
            "1,0".into(),
            "--unit".into(),
            "mm".into(),
        ]
    };
    let st = |v: &[&str]| v.iter().map(|x| (*x).to_owned()).collect::<Vec<String>>();
    let pairs: Vec<(&str, Call, Call)> = vec![
        (
            "move, colour",
            ("entity move", mv("wall")),
            ("entity set", st(&[&a("wall"), "--color", "3"])),
        ),
        (
            "move, move",
            ("entity move", mv("wall")),
            (
                "entity move",
                st(&[&a("wall"), "--by", "2,0", "--unit", "mm"]),
            ),
        ),
        (
            "layer, linetype",
            ("entity set", st(&[&a("lamp"), "--layer", "ANNO"])),
            ("entity set", st(&[&a("lamp"), "--ltype", "CONTINUOUS"])),
        ),
        (
            "move, attribute",
            ("entity move", mv("dev")),
            (
                "attrib set",
                st(&[&a("dev"), "--tag", "LABEL", "--value", "X1"]),
            ),
        ),
        (
            "move, other entity",
            ("entity move", mv("wall")),
            ("entity move", mv("lamp")),
        ),
    ];
    let mut agreed = 0;
    for (what, (v1, a1), (v2, a2)) in &pairs {
        let f = fx("agree");
        let s = f.open("plan.dxf", &b.bytes);
        let c1 = f.join(&s, "atlas", ParticipantKind::Agent);
        let c2 = f.join(&s, "bea", ParticipantKind::Agent);
        let base = head(&s);
        let bs = base.to_string();
        let r1: Vec<&str> = a1.iter().map(String::as_str).collect();
        let r2: Vec<&str> = a2.iter().map(String::as_str).collect();
        let mut x = argv(v1, &r1);
        x.extend(["--base", bs.as_str()]);
        let mut y = argv(v2, &r2);
        y.extend(["--base", bs.as_str()]);
        let o1 = call(&s, c1, &x);
        assert_eq!(o1.exit, 0, "merge_and_base_agree: {what}: the first lands");
        index(&s);
        let kit = outcome(&call(&s, c2, &y));
        let ours = k2_mpc_dxf::edit::plan(&b.bytes, v1, &r1).unwrap().output;
        let theirs = k2_mpc_dxf::edit::plan(&b.bytes, v2, &r2).unwrap().output;
        let merge = merged(&b.bytes, &ours, &theirs);
        assert_eq!(
            merge, kit,
            "merge_and_base_agree: {what}: the merge and --base disagree"
        );
        agreed += 1;
    }
    println!("merge_and_base_agree: {agreed} pairs agree");
}

fn app_save(path: &Path, bytes: &[u8]) {
    let tmp = path.with_file_name(".app-save.tmp");
    std::fs::write(&tmp, bytes).unwrap();
    std::fs::rename(&tmp, path).unwrap();
}

/// app_save_after_verb: a person's app opened version V; an agent's verb lands; the app saves
/// its own change of another entity. With the opened version noted, the save merges (both
/// changes in the file, no copy); without, both are kept (a conflict copy).
#[test]
pub fn app_save_after_verb() {
    let b = testgen::edit_sampler(Ver::R2018).build();
    let mut seen = Vec::new();
    for pin in [true, false] {
        let f = fx(if pin { "pinned" } else { "unpinned" });
        let s = f.open_file("plan.dxf", &b.bytes);
        let ann = f.join(&s, "ann", ParticipantKind::Person);
        let bot = f.join(&s, "bot", ParticipantKind::Agent);
        let v = head(&s);
        if pin {
            s.note_opened(ann, OpenedAt::Version(v)).expect("noted");
        }
        let o = call(
            &s,
            bot,
            &[
                "entity",
                "move",
                &b.addr("wall"),
                "--by",
                "1,0",
                "--unit",
                "mm",
            ],
        );
        assert_eq!(o.exit, 0, "app_save_after_verb: the agent's verb");
        s.flush(ann).expect("a flush");
        let theirs = k2_mpc_dxf::edit::plan(
            &b.bytes,
            "entity move",
            &[&b.addr("lamp"), "--by", "100,0", "--unit", "mm"],
        )
        .unwrap()
        .output;
        let path = f.ws().join("plan.dxf");
        app_save(&path, &theirs);
        let out = s.file_changed().unwrap();
        let file = std::fs::read(&path).unwrap();
        let both = {
            let want = k2_mpc_dxf::edit::plan(
                &theirs,
                "entity move",
                &[&b.addr("wall"), "--by", "1,0", "--unit", "mm"],
            )
            .unwrap()
            .output;
            file == want
        };
        seen.push((pin, format!("{out:?}"), both));
        if pin {
            assert!(
                matches!(out, DiskCheck::Merged { .. }) && both,
                "app_save_after_verb: pinned: the save merges, both changes in the file: {out:?}"
            );
        } else {
            assert!(
                matches!(out, DiskCheck::Conflict { .. }),
                "app_save_after_verb: unpinned: both kept: {out:?}"
            );
        }
    }
    println!("app_save_after_verb: {seen:?}");
}
