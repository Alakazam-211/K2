//! The pack's verbs inside the kit: the registry takes them, each read verb runs on a blob doc
//! through `BlobSession::run_tool` with its typed data, the pack's refusals come back by their
//! `DXF-E…` ids, and a hold reaches as far as its node kind says.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::errors::Value;
use k2_mpc_base::parity::{Registry, VerbKind};
use k2_mpc_base::session::{
    AuditEvent, BlobPack, BlobSession, Config, ConnId, HoldReach, Host, Participant,
    ParticipantKind, Rights, SessionManager, Stay,
};
use k2_mpc_base::store::{DocKey, DocStore};
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
        "k2cad-verbs-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
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

impl Fx {
    /// A doc of `bytes`, its first version indexed.
    fn open(&self, path: &str, bytes: &[u8]) -> Arc<BlobSession> {
        let key = DocKey {
            space: "ws".into(),
            path: path.into(),
        };
        let s = self
            .mgr
            .open_blob(
                Arc::new(DxfPack) as Arc<dyn BlobPack>,
                &key,
                Some(&mut &bytes[..]),
            )
            .unwrap();
        if let Some(j) = s.index_job(None).unwrap() {
            j.run().expect("the index job records");
        }
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

/// A call with its result as data.
fn call(s: &BlobSession, c: ConnId, a: &[&str]) -> ToolOutput {
    let argv: Vec<String> = a.iter().map(|x| (*x).to_owned()).collect();
    s.run_tool(c, &argv, &ToolOptions::new().result_json(true))
        .expect("with the result asked for, every outcome is a result")
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

/// The read verbs, as the pack names them (two words, or one).
const READS: [&str; 12] = [
    "drawing info",
    "layer list",
    "block list",
    "entity find",
    "text find",
    "attrib find",
    "block show",
    "entity props",
    "header get",
    "check",
    "draw header",
    "draw page",
];

#[test]
pub fn registry_takes_the_pack() {
    let r = match Registry::for_blob_pack(&DxfPack) {
        Ok(r) => r,
        Err(e) => panic!("registry_takes_the_pack: the registry refused the pack: {e:?}"),
    };
    let mut listed = 0;
    for name in READS {
        let v = r
            .get(name)
            .unwrap_or_else(|| panic!("registry_takes_the_pack: {name} isn't listed"));
        assert_eq!(v.kind, VerbKind::Read, "registry_takes_the_pack: {name}");
        assert!(
            v.runs_through_run_tool(),
            "registry_takes_the_pack: {name} runs through run_tool"
        );
        listed += 1;
    }
    for (name, word) in [
        ("attrib set", "set"),
        ("entity set", "changed"),
        ("entity move", "moved"),
        ("entity add", "added"),
        ("entity delete", "deleted"),
    ] {
        let v = r
            .get(name)
            .unwrap_or_else(|| panic!("registry_takes_the_pack: {name} isn't listed"));
        assert_eq!(v.kind, VerbKind::Edit, "registry_takes_the_pack: {name}");
        let b = v.blob.expect("registry_takes_the_pack: blob facts");
        assert_eq!(
            (b.merge, b.input),
            (Some("pack"), "whole"),
            "registry_takes_the_pack: {name}: the pack's merge covers it, read whole"
        );
        assert_eq!(
            k2_mpc_dxf::edit::word(name),
            word,
            "registry_takes_the_pack: {name}'s word"
        );
        listed += 1;
    }
    let ids: Vec<&str> = r.errors().iter().map(|e| e.id).collect();
    for id in ["DXF-E001", "DXF-E002", "DXF-E003", "DXF-E010", "DXF-E011"] {
        assert!(
            ids.contains(&id),
            "registry_takes_the_pack: {id} is one of the pack's refusals ({ids:?})"
        );
    }
    assert!(
        ids.iter().all(|i| i.starts_with("DXF-E")),
        "registry_takes_the_pack: every refusal under the pack's own name ({ids:?})"
    );
    println!(
        "registry_takes_the_pack: {listed} verbs, {} refusals",
        ids.len()
    );
}

#[test]
pub fn reads_through_the_kit() {
    let f = fx("reads");
    let b = testgen::sampler(Ver::R2018).build();
    let s = f.open("plan.dxf", &b.bytes);
    let ann = f.join(&s, "ann", ParticipantKind::Person);
    let o = call(&s, ann, &["drawing", "info"]);
    assert_eq!(
        o.exit,
        0,
        "reads_through_the_kit: drawing info: {}",
        res(&o).to_json()
    );
    let d = get(res(&o), &["data"]);
    assert_eq!(
        get(d, &["schema"]).as_str(),
        Some("cad.drawing/1"),
        "reads_through_the_kit: the data names its shape: {}",
        d.to_json()
    );
    assert_eq!(get(d, &["acadver"]).as_str(), Some("AC1032"));
    let o = call(&s, ann, &["layer", "list"]);
    assert_eq!(o.exit, 0, "reads_through_the_kit: layer list");
    assert_eq!(
        get(res(&o), &["data", "schema"]).as_str(),
        Some("cad.layer-list/1")
    );
    let v = s.status().unwrap().version;
    // a drawing read with no --version reads the version the kit gave the verb
    let o = call(&s, ann, &["draw", "header"]);
    assert_eq!(
        o.exit,
        0,
        "reads_through_the_kit: draw header: {}",
        res(&o).to_json()
    );
    assert_eq!(
        get(res(&o), &["data", "version"]).as_u64(),
        Some(v),
        "reads_through_the_kit: draw header carries the version read"
    );
    let other = (v + 7).to_string();
    let o = call(&s, ann, &["draw", "header", "--version", &other]);
    assert_eq!(
        o.exit, 2,
        "reads_through_the_kit: another version than the one read is refused"
    );
    let door = format!("@h:{:X}", b.handle("door1"));
    let wall = format!("@h:{:X}", b.handle("wall"));
    let mut ran = 0;
    for name in READS {
        let mut argv: Vec<&str> = name.split(' ').collect();
        match name {
            "header get" => argv.push("$ACADVER"),
            "text find" => argv.extend(["--contains", "ROOM"]),
            "attrib find" => argv.extend(["--tag", "NO"]),
            "entity find" => argv.extend(["--layer", "A-WALL"]),
            "block show" => argv.push(&door),
            "entity props" => argv.push(&wall),
            "draw page" => argv.extend(["--space", "model"]),
            _ => {}
        }
        let o = call(&s, ann, &argv);
        assert_eq!(
            o.exit,
            0,
            "reads_through_the_kit: {name} runs: {}",
            res(&o).to_json()
        );
        ran += 1;
    }
    println!("reads_through_the_kit: {ran} read verbs through run_tool at v{v}");
}

#[test]
pub fn refusals_by_pack_id() {
    let f = fx("refusals");
    let b = testgen::sampler(Ver::R2018).build();
    let s = f.open("plan.dxf", &b.bytes);
    let ann = f.join(&s, "ann", ParticipantKind::Person);
    let o = call(&s, ann, &["entity", "props", "@h:FFFFFF"]);
    assert_eq!(
        (err_id(&o).as_deref(), o.exit),
        (Some("DXF-E010"), 4),
        "refusals_by_pack_id: an address that names nothing: {}",
        res(&o).to_json()
    );
    let o = call(&s, ann, &["layer", "list", "--bogus"]);
    assert_eq!(
        o.exit,
        2,
        "refusals_by_pack_id: a usage refusal exits 2: {}",
        res(&o).to_json()
    );
    println!("refusals_by_pack_id: 2 refusals");
}

#[test]
pub fn hold_reach_by_kind() {
    let p = DxfPack;
    assert_eq!(p.hold_reach("layer"), HoldReach::Members("layer_key"));
    for k in ["block", "model", "layout"] {
        assert_eq!(
            p.hold_reach(k),
            HoldReach::Subtree,
            "hold_reach_by_kind: {k}"
        );
    }
    for k in ["line", "insert", "text", "drawing"] {
        assert_eq!(p.hold_reach(k), HoldReach::Node, "hold_reach_by_kind: {k}");
    }
    println!("hold_reach_by_kind: 8 kinds");
}
