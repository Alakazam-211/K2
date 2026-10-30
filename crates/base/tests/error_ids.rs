//! The error surface (stable ids): the registry is append-only against a committed
//! snapshot; `explain` is golden per id; `docs/ERRORS.md` and `capabilities` are generated;
//! and every id the registry says a verb reaches is produced here through the public API,
//! with the exit it always had.
//!
//! Regenerate the goldens, the snapshot and `docs/ERRORS.md` with `MPC_BLESS=1` (the snapshot
//! only grows: blessing refuses to drop or change a line).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::errors::{
    self, capabilities, errors_markdown, explain, explain_json, run_meta, ErrorReport, Reach,
    Status, Value, KIT,
};
use k2_mpc_base::parity::{PersonAction, Registry, Tier};
use k2_mpc_base::session::{
    AnyFile, ApprovalState, AuditEvent, Config, ConnId, DiskCheck, Host, LineRange, LogQuery, Pack,
    Participant, ParticipantKind, Rights, Session, SessionError, SessionManager, Stay,
    TextFilePack, UndoMode, UndoRequest,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::text::diff::Hunk;
use k2_mpc_base::tools::{exit, session_exit, EditOutput, PackEdit, PackVerb, ToolError};
use k2_mpc_base::Edit;

const ROOT: &str = TextFilePack::ROOT;
const MD: TextFilePack = TextFilePack::new("markdown", 4 << 20);

fn bless() -> bool {
    std::env::var_os("MPC_BLESS").is_some()
}

fn manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn golden_dir() -> PathBuf {
    manifest().join("tests/golden/errors")
}

/// Compare `got` with the file, or write it when blessing.
fn golden(path: &Path, got: &str) {
    if bless() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(path)
        .unwrap_or_else(|_| panic!("{} is missing (MPC_BLESS=1 writes it)", path.display()));
    assert!(
        want == got,
        "{} differs from what the registry generates (MPC_BLESS=1 regenerates it)",
        path.display()
    );
}

// ------------------------------------------------------------- the registry itself

fn snapshot_line(e: &errors::ErrorEntry) -> String {
    let status = match e.status {
        Status::Live => "live".to_owned(),
        Status::Reserved(_) => "reserved".to_owned(),
        Status::Retired(v) => format!("retired:{v}"),
    };
    format!("{} {} exit={} {status}", e.id, e.name, e.exit)
}

/// Condition 1: ids are stable forever. Every line of the committed snapshot still holds:
/// same id, same name, same exit; a status only moves forward (reserved → live → retired).
/// New ids are appended (bless). Blessing never drops or changes a line.
#[test]
fn registry_is_append_only_against_the_snapshot() {
    let path = golden_dir().join("registry.snapshot");
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    let now: BTreeMap<&str, &errors::ErrorEntry> = errors::entries().map(|e| (e.id, e)).collect();
    let rank = |s: &str| {
        if s == "reserved" {
            0
        } else if s == "live" {
            1
        } else {
            2
        }
    };
    let mut kept = BTreeSet::new();
    for line in old.lines().filter(|l| !l.starts_with('#') && !l.is_empty()) {
        let mut f = line.split(' ');
        let (id, name, exit, status) = (
            f.next().unwrap(),
            f.next().unwrap(),
            f.next().unwrap(),
            f.next().unwrap(),
        );
        let e = now
            .get(id)
            .unwrap_or_else(|| panic!("{id} ({name}) was removed: ids are never removed"));
        assert_eq!(e.name, name, "{id} was renamed");
        assert_eq!(
            format!("exit={}", e.exit),
            exit,
            "{id} changed its exit code"
        );
        let cur = snapshot_line(e);
        let cur_status = cur.rsplit(' ').next().unwrap();
        assert!(
            rank(cur_status) >= rank(status),
            "{id}: status went back from {status} to {cur_status}"
        );
        if status.starts_with("retired") {
            assert_eq!(cur_status, status, "{id}: a retired id stays retired");
        }
        kept.insert(id);
    }
    let added: Vec<&str> = now
        .keys()
        .filter(|id| !kept.contains(*id))
        .copied()
        .collect();
    if bless() {
        let mut out = String::from(
            "# The error registry, append only (tests/error_ids.rs). One line per id: id name \
             exit status.\n# Never edit a line by hand; MPC_BLESS=1 appends new ids and moves \
             statuses forward.\n",
        );
        for e in errors::entries() {
            out.push_str(&snapshot_line(e));
            out.push('\n');
        }
        std::fs::write(&path, out).unwrap();
        return;
    }
    assert!(
        added.is_empty(),
        "ids not in the snapshot: {added:?} (MPC_BLESS=1 appends them)"
    );
}

/// Condition 3: a golden per id. `explain <id>` (text and JSON) is what the registry says.
#[test]
fn explain_is_golden_per_id() {
    let dir = golden_dir().join("explain");
    let mut names = BTreeSet::new();
    for e in errors::entries() {
        let got = format!("{}--- json\n{}\n", explain(e), explain_json(e));
        golden(&dir.join(format!("{}.txt", e.id)), &got);
        names.insert(format!("{}.txt", e.id));
        // and the verb prints exactly that
        let cfg = Config::default();
        let out = run_meta(&[s("explain"), s(e.id)], false, &[], &cfg).unwrap();
        assert_eq!(String::from_utf8(out.stdout).unwrap(), explain(e));
    }
    if !bless() {
        for f in std::fs::read_dir(&dir).unwrap() {
            let n = f.unwrap().file_name().into_string().unwrap();
            assert!(names.contains(&n), "golden {n} names no id");
        }
    }
}

/// `docs/ERRORS.md` is generated from the registry.
#[test]
fn errors_md_is_generated() {
    golden(&manifest().join("../../docs/ERRORS.md"), &errors_markdown());
}

/// `capabilities` (text and JSON) is generated from the registries; its golden changes with
/// the verbs, the limits, the ids and the version, and only then.
#[test]
fn capabilities_is_generated() {
    let cfg = Config::default();
    let text = run_meta(&[s("capabilities")], false, &[], &cfg).unwrap();
    let json = run_meta(&[s("capabilities")], true, &[], &cfg).unwrap();
    golden(
        &golden_dir().join("capabilities.txt"),
        &String::from_utf8(text.stdout).unwrap(),
    );
    golden(
        &golden_dir().join("capabilities.json"),
        &String::from_utf8(json.stdout).unwrap(),
    );
    let v = capabilities(&[], &cfg);
    // every verb of both tiers, every person action, every Config field
    let verbs = match v.get("verbs") {
        Some(Value::List(l)) => l.len(),
        _ => 0,
    };
    let names: BTreeSet<&str> = [
        Registry::for_tier(Tier::Text),
        Registry::for_tier(Tier::Blob),
    ]
    .iter()
    .flat_map(|r| r.verbs().iter().map(|v| v.name).collect::<Vec<_>>())
    .collect();
    assert_eq!(verbs, names.len());
    match v.get("parity") {
        Some(Value::List(l)) => assert_eq!(l.len(), PersonAction::ALL.len()),
        _ => panic!("no parity"),
    }
    match v.get("limits") {
        Some(Value::Obj(m)) => assert_eq!(m.len(), cfg.limits().len() + 1),
        _ => panic!("no limits"),
    }
    // a pack's own verbs appear when the host passes its registry
    let reg = Registry::for_pack(&VerbPack).unwrap();
    let v = capabilities(&[&reg], &cfg);
    let Some(Value::List(vs)) = v.get("verbs") else {
        panic!()
    };
    let row = vs
        .iter()
        .find(|r| r.get("name").and_then(Value::as_str) == Some("replace-word"))
        .expect("the pack verb");
    let ids: Vec<&str> = match row.get("errors") {
        Some(Value::List(l)) => l.iter().filter_map(Value::as_str).collect(),
        _ => vec![],
    };
    assert!(
        ids.contains(&"MPC-E127") && ids.contains(&"MPC-E102"),
        "{ids:?}"
    );
}

fn s(x: &str) -> String {
    x.to_owned()
}

// ------------------------------------------------------------- a host for the walk

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
    host: Arc<TestHost>,
    mgr: SessionManager,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fx(tag: &str, tweak: impl FnOnce(&mut Config)) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-errids-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("ws")).unwrap();
    let dir = std::fs::canonicalize(dir).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_000_000),
        rights: Mutex::new(HashMap::new()),
    });
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let h = host.clone();
    let mut bc = BlobConfig::default();
    bc.max_blob_bytes = 64;
    let blobs = Arc::new(
        BlobStore::open(
            store.clone(),
            dir.join("blobs"),
            bc,
            Arc::new(move || h.now_ms()),
        )
        .unwrap(),
    );
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    tweak(&mut c);
    let mgr = SessionManager::new(store, host.clone(), c).with_blob_store(blobs);
    Fx { dir, host, mgr }
}

fn key(p: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: p.into(),
    }
}

fn who(pid: &str, kind: ParticipantKind) -> Participant {
    Participant::new(pid, pid.to_uppercase(), "#123456", kind, "cli")
}

impl Fx {
    fn open(&self, path: &str, bytes: &[u8]) -> Arc<Session> {
        self.mgr
            .open(Arc::new(MD), &key(path), Some(bytes))
            .unwrap()
    }
    fn grant(&self, pid: &str, r: Rights) {
        self.host.rights.lock().unwrap().insert(pid.into(), r);
    }
    fn join_as(&self, s: &Session, p: Participant, r: Rights) -> ConnId {
        self.grant(&p.pid, r);
        s.join(p, Stay::Held).unwrap().conn
    }
    fn person(&self, s: &Session, pid: &str, r: Rights) -> ConnId {
        self.join_as(s, who(pid, ParticipantKind::Person), r)
    }
    fn agent(&self, s: &Session, pid: &str, r: Rights) -> ConnId {
        self.join_as(s, who(pid, ParticipantKind::Agent), r)
    }
    fn ws(&self) -> PathBuf {
        self.dir.join("ws")
    }
}

fn argv(a: &[&str]) -> Vec<String> {
    a.iter().map(|x| (*x).to_owned()).collect()
}

/// Replace the first `old` with `new` as `c`; the change's log position.
fn replace(s: &Session, c: ConnId, old: &str, new: &str) -> u64 {
    s.edit_with(c, ROOT, |t| Ok::<_, ()>(t.replacen(old, new, 1)))
        .unwrap()
        .expect("a change")
        .version
}

/// A pack with one verb, `replace-word OLD NEW`, that applies nothing (exit 1) when OLD
/// isn't there: a pack verb's "no".
struct VerbPack;

fn replace_word(args: &[String]) -> Result<PackEdit, ToolError> {
    let [old, new] = args else {
        return Err(ToolError::usage("replace-word OLD NEW"));
    };
    let (old, new) = (old.clone(), new.clone());
    Ok(Box::new(move |_doc: &str, live: &str, _stdin: &[u8]| {
        Ok(if live.contains(&old) {
            EditOutput::new(Some(live.replacen(&old, &new, 1)), Vec::new(), 0)
        } else {
            EditOutput::new(None, format!("replace-word: no {old:?}\n").into_bytes(), 1)
        })
    }))
}

static VERBS: &[PackVerb] = &[PackVerb::edit(
    "replace-word",
    "replace-word OLD NEW",
    &[PersonAction::FindReplace],
    &[0, 1],
    replace_word,
)];

impl Pack for VerbPack {
    fn schema(&self) -> k2_mpc_base::Schema {
        MD.schema()
    }
    fn decode(
        &self,
        bytes: &[u8],
    ) -> Result<k2_mpc_base::session::PackContent, k2_mpc_base::session::Declined> {
        MD.decode(bytes)
    }
    fn encode(
        &self,
        content: &[(String, k2_mpc_base::Neutral)],
        codec: &[u8],
    ) -> Result<Vec<u8>, k2_mpc_base::session::PackError> {
        MD.encode(content, codec)
    }
    fn verbs(&self) -> &'static [PackVerb] {
        VERBS
    }
}

/// What the walk saw: each report with the exit the call itself gave.
#[derive(Default)]
struct Seen(Vec<(ErrorReport, u8)>);

impl Seen {
    fn err(&mut self, e: SessionError) {
        let code = session_exit(&e);
        self.0.push((e.report(), code));
    }
    #[track_caller]
    fn export<T>(&mut self, r: Result<T, k2_mpc_base::export::ExportError>) {
        match r {
            Err(e) => {
                let code = e.exit();
                self.0
                    .push((k2_mpc_base::errors::Refusal::report(&e), code));
            }
            Ok(_) => panic!("expected an export refusal"),
        }
    }
    #[track_caller]
    fn res<T>(&mut self, r: Result<T, SessionError>) {
        match r {
            Err(e) => self.err(e),
            Ok(_) => panic!("expected a refusal"),
        }
    }
    #[track_caller]
    fn tool(
        &mut self,
        r: Result<k2_mpc_base::tools::ToolOutput, k2_mpc_base::tools::ToolRunError>,
    ) {
        match r {
            Err(e) => {
                let code = e.exit();
                self.0
                    .push((k2_mpc_base::errors::Refusal::report(&e), code));
            }
            Ok(o) => {
                let r = o
                    .refusal
                    .clone()
                    .unwrap_or_else(|| panic!("expected a refusal: {o:?}"));
                assert!(
                    o.stderr_summary.ends_with(&format!("({})", r.id())),
                    "{}",
                    o.stderr_summary
                );
                self.0.push((r, o.exit));
            }
        }
    }
    fn report(&mut self, r: ErrorReport, code: u8) {
        self.0.push((r, code));
    }
}

/// Every refusal a verb can return, through the public API.
fn walk() -> Seen {
    let mut seen = Seen::default();
    let cfg = Config::default();

    // ---- exports (0xx), through ExportRegistry::export
    export_walk(&mut seen);

    // ---- rights, joining, edits (1xx)
    let f = fx("rights", |_| {});
    let s = f.open("a.md", b"one\ntwo\nthree\n");
    f.grant("nobody", Rights::none());
    seen.res(s.join(who("nobody", ParticipantKind::Agent), Stay::Held)); // E101
    let reader = f.agent(&s, "reader", Rights::read_only());
    seen.tool(s.run_tool(reader, ROOT, &argv(&["insert", "--line", "1"]), b"x\n")); // E102
    seen.res(s.log(reader, &LogQuery::default())); // E103
    let ed = f.person(&s, "ed", Rights::editor());
    let gone = f.agent(&s, "gone", Rights::editor());
    s.leave(gone).unwrap();
    seen.tool(s.run_tool(gone, ROOT, &argv(&["cat"]), b"")); // E107
    seen.res(s.join(who("kit:x", ParticipantKind::Agent), Stay::Held)); // E115
    let v = s.status().unwrap().version;
    replace(&s, ed, "one", "ONE");
    seen.tool(s.run_tool(
        ed,
        ROOT,
        &argv(&["insert", "--line", "1", "--if-version", &v.to_string()]),
        b"x\n",
    )); // E110
    seen.res(s.name_version(ed, "", None)); // E113
    seen.res(s.set_view(
        ed,
        LineRange {
            root: ROOT.into(),
            first: 99,
            last: 100,
        },
    )); // E114
    seen.res(s.edit(
        ed,
        vec![Edit::Text {
            root: "nope".into(),
            hunks: vec![Hunk {
                start: 0,
                end: 0,
                insert: "x".into(),
            }],
        }],
        None,
    )); // E148
    seen.res(s.edit(
        ed,
        vec![Edit::Text {
            root: ROOT.into(),
            hunks: vec![Hunk {
                start: 999,
                end: 1000,
                insert: "x".into(),
            }],
        }],
        None,
    )); // E151
        // write --base: a conflict, an expired base
    let r = s.read(ed).unwrap();
    let base = r.version;
    replace(&s, ed, "two", "TWO");
    let rep = s.write_base(ed, ROOT, base, "ONE\nTwo!\nthree\n").unwrap();
    seen.report(rep.refusal(base).expect("a conflict"), rep.exit()); // E111
    let rep = s.write_base(ed, ROOT, 9_999, "x\n").unwrap();
    seen.report(rep.refusal(9_999).expect("expired"), rep.exit()); // E112
                                                                   // the tools
    seen.tool(s.run_tool(ed, ROOT, &argv(&["frob"]), b"")); // E120
    seen.tool(s.run_tool(ed, ROOT, &argv(&["sed", "w out.txt"]), b"")); // E121
    seen.tool(s.run_tool(ed, ROOT, &argv(&["head", "-n", "x"]), b"")); // E122
    let bad = "--- a.md\n+++ a.md\n@@ -1,1 +1,1 @@\n-nothing like this\n+x\n";
    seen.tool(s.run_tool(ed, ROOT, &argv(&["patch"]), bad.as_bytes())); // E123
    let mixed = "--- a.md\n+++ a.md\n@@ -1,1 +1,1 @@\n-ONE\n+One\n@@ -3,1 +3,1 @@\n-nothing like this\n+x\n";
    seen.tool(s.run_tool(ed, ROOT, &argv(&["patch", "--partial"]), mixed.as_bytes())); // E124
    seen.tool(s.run_tool(
        ed,
        ROOT,
        &argv(&["replace", "--old", "zzz", "--new", "y"]),
        b"",
    )); // E125
    s.run_tool(ed, ROOT, &argv(&["append"]), b"three\n")
        .unwrap();
    seen.tool(s.run_tool(
        ed,
        ROOT,
        &argv(&["replace", "--old", "three", "--new", "y"]),
        b"",
    )); // E126
    seen.tool(s.run_tool(ed, ROOT, &argv(&["grep", "-E", "a{2000}"]), b"")); // E902
                                                                             // the meta verbs
    let out = run_meta(&argv(&["explain", "MPC-E000"]), false, &[], &cfg).unwrap();
    seen.report(out.refusal.unwrap(), out.exit); // E160
                                                 // a pack's own verb that applies nothing
    let p = f
        .mgr
        .open(Arc::new(VerbPack), &key("v.md"), Some(b"alpha\n"))
        .unwrap();
    let pe = f.agent(&p, "pe", Rights::editor());
    seen.tool(p.run_tool(pe, ROOT, &argv(&["replace-word", "zeta", "eta"]), b"")); // E127
                                                                                   // no doc; another pack's doc
    seen.res(f.mgr.open(Arc::new(MD), &key("none.md"), None)); // E108
    seen.res(f.mgr.open(
        Arc::new(TextFilePack::new("text", 1 << 20)),
        &key("a.md"),
        None,
    )); // E109
        // a path another doc has (mv)
    seen.res(f.mgr.rekey(&key("v.md"), key("a.md"))); // E504

    // ---- approvals: an assistant acting for a person (1xx)
    let f = fx("approvals", |c| c.pending_max_records = 1);
    let s = f.open("p.md", b"one\ntwo\n");
    let pat = f.person(&s, "pat", Rights::editor());
    let other = f.person(&s, "other", Rights::editor());
    let asst = f.join_as(
        &s,
        who("asst", ParticipantKind::Agent).on_behalf_of("pat"),
        Rights::editor(),
    );
    let pending = replace(&s, asst, "one", "ONE");
    seen.tool(s.run_tool(asst, ROOT, &argv(&["append"]), b"three\n")); // E130
    seen.res(s.set_approval(other, pending, ApprovalState::Approved)); // E104
    let mine = replace(&s, pat, "two", "TWO");
    seen.res(s.set_approval(pat, mine, ApprovalState::Approved)); // E131
    s.set_approval(pat, pending, ApprovalState::Approved)
        .unwrap();
    seen.res(s.set_approval(pat, pending, ApprovalState::Declined)); // E132
    seen.res(s.revert(other, mine, false)); // E105

    // ---- history and undo (2xx)
    let f = fx("undo", |_| {});
    let s = f.open("u.md", b"one\ntwo\nthree\nfour\nfive\n");
    let a = f.person(&s, "a", Rights::editor());
    let b = f.agent(&s, "b", Rights::editor());
    s.replace_text(a, ROOT, "ONE\ntwo\nthree\nfour\nFIVE\n")
        .unwrap();
    replace(&s, b, "FIVE", "FIxVE");
    let o = s.undo(a, 1).unwrap();
    seen.report(o.refusal().expect("a conflict"), o.exit()); // E201
    let o = s
        .undo_with(a, &UndoRequest::default().mode(UndoMode::Partial))
        .unwrap();
    seen.report(o.refusal().expect("skipped"), o.exit()); // E203
    replace(&s, a, "three", "THREE");
    replace(&s, b, "THREE", "THxREE");
    let o = s
        .undo_with(a, &UndoRequest::default().mode(UndoMode::Partial))
        .unwrap();
    seen.report(o.refusal().expect("blocked"), o.exit()); // E202
    seen.res(s.revert(a, 9_999, false)); // E205
    s.clear_undo_history().unwrap();
    let o = s.undo(a, 1).unwrap();
    seen.report(o.refusal().expect("cleared"), o.exit()); // E207
    let f = fx("lost", |c| c.idle_unload_ms = 1_000);
    {
        let s = f.open("l.md", b"hello\n");
        let a = f.person(&s, "a", Rights::editor());
        replace(&s, a, "hello", "HELLO");
        s.leave(a).unwrap();
    }
    f.host.now.fetch_add(2_000, Ordering::SeqCst);
    assert_eq!(f.mgr.unload_idle().unwrap(), 1);
    let s = f.mgr.open(Arc::new(MD), &key("l.md"), None).unwrap();
    let a = f.person(&s, "a", Rights::editor());
    let o = s.undo(a, 1).unwrap();
    seen.report(o.refusal().expect("lost"), o.exit()); // E206
    let f = fx("tooold", |c| {
        c.revert_keep_changes = 2;
        c.revert_keep_ms = 60_000;
    });
    let s = f.open("w.md", b"l 1\nl 2\n");
    let a = f.person(&s, "a", Rights::editor());
    let old = replace(&s, a, "l 1", "L 1");
    for i in 0..6 {
        s.edit_with(a, ROOT, |t| Ok::<_, ()>(format!("{t}tail {i}\n")))
            .unwrap();
    }
    f.host.now.fetch_add(2 * 60_000, Ordering::SeqCst);
    f.mgr.tick().unwrap();
    seen.res(s.revert(a, old, false)); // E204

    // ---- claims (3xx) and their caps (9xx)
    let f = fx("claims", |c| {
        c.claims_per_participant = 1;
        c.claim_waiters_max = 1;
    });
    let s = f.open("c.md", b"1\n2\n3\n4\n5\n6\n7\n8\n");
    let x = f.agent(&s, "x", Rights::editor());
    let y = f.agent(&s, "y", Rights::editor());
    let z = f.agent(&s, "z", Rights::editor());
    let lines = |a, b| LineRange {
        root: ROOT.into(),
        first: a,
        last: b,
    };
    let cx = s.claim(x, lines(1, 2), None, None, false).unwrap();
    seen.res(s.claim(y, lines(2, 3), None, None, false)); // E301
    seen.res(s.claim(x, lines(5, 6), None, None, false)); // E904
    seen.res(s.claim(y, lines(5, 6), Some(0), None, false)); // E304
    seen.res(s.renew_claim(y, None, None, None)); // E302
    s.register_waiter(y, cx.id).unwrap();
    seen.res(s.register_waiter(z, cx.id)); // E905
    seen.res(s.renew_claim(x, Some(cx.id), Some(120_000), None)); // E303
    let f = fx("claimcap", |c| c.claims_max = 1);
    let s = f.open("c.md", b"1\n2\n3\n4\n");
    let x = f.agent(&s, "x", Rights::editor());
    let y = f.agent(&s, "y", Rights::editor());
    s.claim(x, lines(1, 1), None, None, false).unwrap();
    seen.res(s.claim(y, lines(3, 3), None, None, false)); // E903
                                                          // a join past participants_max watches (K11): its edit is refused
    let f = fx("watching", |c| c.participants_max = 1);
    let s = f.open("w.md", b"1\n");
    f.agent(&s, "x", Rights::editor());
    let w = f.agent(&s, "w", Rights::editor());
    seen.res(s.replace_text(w, ROOT, "2\n")); // E305
                                              // the rate limit
    let f = fx("rate", |c| {
        c.edit_rate_per_s = 1;
        c.edit_burst = 1;
    });
    let s = f.open("r.md", b"a\n");
    let x = f.agent(&s, "x", Rights::editor());
    s.run_tool(x, ROOT, &argv(&["append"]), b"b\n").unwrap();
    seen.tool(s.run_tool(x, ROOT, &argv(&["append"]), b"c\n")); // E901

    // ---- disk: declined files, a bad path, live editing turned off (4xx)
    let f = fx("disk", |_| {});
    let small = TextFilePack::new("markdown", 8);
    for (name, bytes, pack) in [
        ("big.md", &b"0123456789abcdef\n"[..], small),
        ("bad.md", &b"\xff\xfe\n"[..], MD),
        ("bin.md", &b"a\0b\n"[..], MD),
        ("mixed.csv", &b"a,b\r\nc,d\n"[..], MD),
    ] {
        let p = f.ws().join(name);
        std::fs::write(&p, bytes).unwrap();
        seen.res(f.mgr.open_file(Arc::new(pack), &key(name), &p, &f.ws())); // E401–E404
    }
    seen.res(f.mgr.open_file(Arc::new(MD), &key("ws"), &f.ws(), &f.ws())); // E411
                                                                           // a file with two names (a hard link) isn't opened live (K9)
    let one = f.ws().join("linked.md");
    std::fs::write(&one, "x\n").unwrap();
    std::fs::hard_link(&one, f.ws().join("linked-too.md")).unwrap();
    seen.res(
        f.mgr
            .open_file(Arc::new(MD), &key("linked.md"), &one, &f.ws()),
    ); // E412
    let p = f.ws().join("off.md");
    std::fs::write(&p, b"text\n").unwrap();
    let s = f
        .mgr
        .open_file(Arc::new(MD), &key("off.md"), &p, &f.ws())
        .unwrap();
    let x = f.agent(&s, "x", Rights::editor());
    std::fs::write(&p, b"now\0binary\n").unwrap();
    assert!(matches!(s.check_disk().unwrap(), DiskCheck::Declined(_)));
    seen.tool(s.run_tool(x, ROOT, &argv(&["append"]), b"more\n")); // E405

    // ---- tier 0 (7xx), erasure (6xx), the store's lifecycle (5xx)
    let f = fx("blob", |_| {});
    f.grant("own", Rights::full().with_owner(true));
    f.grant("ed", Rights::editor().with_history(true));
    let b = f
        .mgr
        .open_blob(Arc::new(AnyFile), &key("r.pdf"), Some(&mut &b"v1"[..]))
        .unwrap();
    let own = b
        .join(who("own", ParticipantKind::Person), Stay::Held)
        .unwrap()
        .conn;
    let ed = b
        .join(who("ed", ParticipantKind::Agent), Stay::Held)
        .unwrap()
        .conn;
    let v1 = b.status().unwrap().version;
    seen.res(b.renew_lease(ed, None)); // E702
    b.acquire_lease(own, None).unwrap();
    seen.res(b.acquire_lease(ed, None)); // E701
    seen.res(b.break_lease(ed)); // E106
    b.release_lease(own).unwrap();
    seen.res(b.write_bytes(ed, &[b'x'; 100], v1)); // E703
    let v2 = b.write_bytes(ed, b"v2", v1).unwrap().version;
    seen.res(b.read_version(ed, 9_999)); // E511
    seen.res(b.purge(own, Some(v2))); // E601
    b.purge(own, Some(v1)).unwrap();
    seen.res(b.read_version(ed, v1)); // E602
    let f = fx("noblob", |_| {});
    let plain = SessionManager::new(
        Arc::new(DocStore::open(f.dir.join("other.redb")).unwrap()),
        f.host.clone(),
        Config::default(),
    );
    seen.res(plain.open_blob(Arc::new(AnyFile), &key("x.pdf"), Some(&mut &b"x"[..]))); // E708
    let f = fx("life", |_| {});
    let s = f.open("d.md", b"d\n");
    let x = f.agent(&s, "x", Rights::editor());
    f.mgr.delete_doc(&key("d.md")).unwrap();
    seen.res(s.read(x)); // E516

    // the host closes a doc's live session, members or not (v0.9.7.2)
    let s = f.open("c.md", b"c\n");
    let x = f.agent(&s, "x", Rights::editor());
    f.mgr.close(&key("c.md"), "core_off").unwrap();
    seen.res(s.read(x)); // E519
                         // a blob doc promoted to text: the old handle is unloaded
    let p = f.ws().join("n.md");
    std::fs::write(&p, b"notes\n").unwrap();
    let b = f
        .mgr
        .open_blob_file(Arc::new(AnyFile), &key("n.md"), &p, &f.ws())
        .unwrap();
    f.grant("x", Rights::editor());
    let x = b
        .join(who("x", ParticipantKind::Agent), Stay::Held)
        .unwrap()
        .conn;
    f.mgr
        .promote_blob_file(Arc::new(MD), &key("n.md"), &p, &f.ws())
        .unwrap();
    seen.res(b.read(x)); // E513
    f.mgr.release().unwrap();
    seen.res(f.mgr.open(Arc::new(MD), &key("d2.md"), Some(b"x\n"))); // E514
    seen
}

/// "Every id is reachable or retired": every live id the registry marks `Reach::Verb` is
/// produced here through the API, each with its registered exit, which is the exit the call
/// gave (an id never changes an exit). Indirect ids are mapped from constructed errors in
/// the crate's unit tests; reserved ids from nothing.
#[test]
fn every_verb_reachable_id_is_reached_with_its_exit() {
    let seen = walk();
    let mut got = BTreeSet::new();
    for (r, code) in &seen.0 {
        assert_eq!(r.exit(), *code, "{} {}: exit", r.id(), r.message());
        assert_eq!(
            r.entry().status,
            Status::Live,
            "{} returned but not live",
            r.id()
        );
        // the plain form and the JSON shape
        assert_eq!(r.plain(), format!("{} ({})", r.message(), r.id()));
        let j = r.to_json();
        assert!(
            j.starts_with(&format!(
                "{{\"id\":\"{}\",\"exit\":{},\"message\":",
                r.id(),
                code
            )),
            "{j}"
        );
        got.insert(r.id());
    }
    let want: BTreeSet<&str> = KIT
        .iter()
        .filter(|e| e.status == Status::Live && e.reach == Reach::Verb)
        .map(|e| e.id)
        .collect();
    let missing: Vec<_> = want.difference(&got).collect();
    assert!(
        missing.is_empty(),
        "reachable ids never produced: {missing:?}"
    );
    let extra: Vec<_> = got
        .iter()
        .filter(|id| errors::lookup(id).is_some_and(|e| e.reach != Reach::Verb))
        .collect();
    assert!(
        extra.is_empty(),
        "these are reached through the API: mark them Reach::Verb: {extra:?}"
    );
}

/// Today's wording is kept: the id is appended, never in place of a message, and exit codes
/// are the table's (K-EXIT-1).
#[test]
fn plain_text_keeps_the_wording() {
    let e = SessionError::Held {
        reason: "approvals_backlog",
    };
    assert_eq!(e.report().plain(), "held: approvals_backlog (MPC-E130)");
    assert_eq!(e.report().exit(), exit::HELD);
}

/// Every export refusal through `ExportRegistry::export`: a test exporter whose target
/// `fail` fails as its `mode` option says.
fn export_walk(seen: &mut Seen) {
    use k2_mpc_base::export::{
        ExportCtl, ExportError, ExportLimits, ExportRegistry, ExportReport, ExportTarget, Exporter,
        Fidelity, NoResources, OptionKind, OptionSpec, Options, Sink, Source, SourceKind, VecSink,
    };
    use std::sync::atomic::AtomicBool;
    struct Failing;
    const MODES: &[&str] = &["decode", "io", "panic", "big"];
    const OPTS: &[OptionSpec] = &[OptionSpec::new("mode", OptionKind::Choice(MODES), None, "")];
    impl Exporter for Failing {
        fn id(&self) -> &'static str {
            "failing@0"
        }
        fn targets(&self, _: &SourceKind) -> Vec<ExportTarget> {
            vec![ExportTarget::new(
                "fail",
                "text/plain",
                "txt",
                Fidelity::Lossy,
                OPTS,
            )]
        }
        fn export(
            &self,
            _: Source<'_>,
            _: &str,
            o: &Options,
            out: &mut dyn Sink,
            _: &ExportCtl<'_>,
        ) -> Result<ExportReport, ExportError> {
            match o.get("mode") {
                Some("decode") => Err(ExportError::Decode("bad".into())),
                Some("io") => Err(std::io::Error::other("disk full").into()),
                Some("panic") => panic!("boom"),
                _ => {
                    out.write_all(&[0u8; 64])?;
                    Ok(ExportReport::default())
                }
            }
        }
    }
    let mut reg = ExportRegistry::with_as_is();
    reg.register(Arc::new(Failing)).unwrap();
    let kind = SourceKind::Pack("markdown".into());
    let roots = vec![("body".to_owned(), "x".to_owned())];
    let no = AtomicBool::new(false);
    let yes = AtomicBool::new(true);
    let progress = |_: u64, _: u64| {};
    let small = {
        let mut l = ExportLimits::default();
        l.max_output_bytes = 8;
        l
    };
    let ctl = |cancel: &'static str| -> (bool, ExportLimits) {
        match cancel {
            "cancel" => (true, ExportLimits::default()),
            "small" => (false, small),
            _ => (false, ExportLimits::default()),
        }
    };
    for (target, opts, how) in [
        ("png", vec![], "plain"),               // E001
        ("fail", vec!["nope=1"], "plain"),      // E002 (bad_option)
        ("fail", vec!["mode=big"], "small"),    // E003 (the output limit)
        ("as-is", vec![], "cancel"),            // E004
        ("fail", vec!["mode=decode"], "plain"), // E005
        ("fail", vec!["mode=io"], "plain"),     // E006
        ("fail", vec!["mode=panic"], "plain"),  // E007
    ] {
        let (c, limits) = ctl(how);
        let ctl = ExportCtl {
            cancel: if c { &yes } else { &no },
            progress: &progress,
            limits,
            resources: &NoResources,
        };
        let opts = Options::parse(opts.iter().copied()).unwrap();
        let mut out = VecSink::default();
        let src = Source::Text {
            pack: "markdown",
            roots: &roots,
        };
        seen.export(reg.export(&kind, src, target, &opts, &mut out, &ctl));
    }
}
