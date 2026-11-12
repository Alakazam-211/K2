//! A structured verb sees the doc's sync point (`VerbCtx::synced`): the version the file last
//! held. At the open it is the imported version; an edit not yet written leaves it where it
//! was; a write-back moves it to the doc's version; a read verb and an edit verb see the same
//! one; a doc with no file has none. What a pack's verb reads to tell written changes from
//! unwritten ones (a spreadsheet pack refuses mixed row inserts and deletes between two syncs).
//! Public API only.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::doc::Neutral;
use k2_mpc_base::elem::MapOp;
use k2_mpc_base::parity::PersonAction;
use k2_mpc_base::schema::{Kind, Schema};
use k2_mpc_base::session::{
    AuditEvent, Config, ConnId, Declined, Host, Pack, PackContent, PackError, Participant,
    ParticipantKind, Rights, Session, SessionManager, Stay,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::tools::{PackVerb, StructEdit, StructOutput, ToolError, VerbCtx};
use k2_mpc_base::{Edit, Key, Value};

const ITEMS: &str = "items";
const FIELDS: &str = "fields";

/// A keyed doc: one item a line (`a`, `b`), each with a `n` field from 0. The file is the
/// items, each `key=n` on its line.
struct Items;

const SCHEMA: Schema = Schema {
    name: "items-sync",
    roots: &[(ITEMS, Kind::MovableList), (FIELDS, Kind::Map)],
};

impl Pack for Items {
    fn schema(&self) -> Schema {
        SCHEMA
    }
    fn composite(&self) -> &'static [(&'static str, &'static str)] {
        &[(FIELDS, ITEMS)]
    }
    fn verbs(&self) -> &'static [PackVerb] {
        VERBS
    }
    fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
        let (mut items, mut fields) = (Vec::new(), BTreeMap::new());
        for line in std::str::from_utf8(b).unwrap().lines() {
            let (k, n) = line.split_once('=').unwrap_or((line, "0"));
            let k = Key::new(k.trim()).unwrap();
            fields.insert(format!("{k}\u{1f}n"), Value::Str(n.trim().into()));
            items.push((k, Value::Null));
        }
        Ok(PackContent {
            roots: vec![
                (ITEMS.into(), Neutral::List(items)),
                (FIELDS.into(), Neutral::Map(fields)),
            ],
            codec: Vec::new(),
        })
    }
    fn encode(&self, content: &[(String, Neutral)], _: &[u8]) -> Result<Vec<u8>, PackError> {
        let root = |n: &str| content.iter().find(|(r, _)| r == n).map(|(_, x)| x);
        let (Some(Neutral::List(items)), Some(Neutral::Map(fields))) = (root(ITEMS), root(FIELDS))
        else {
            return Err(PackError("no items".into()));
        };
        let mut out = String::new();
        for (k, _) in items {
            let n = match fields.get(&format!("{k}\u{1f}n")) {
                Some(Value::Str(s)) => s.clone(),
                _ => "0".into(),
            };
            out.push_str(&format!("{k}={n}\n"));
        }
        Ok(out.into_bytes())
    }
}

const S: &[PersonAction] = &[PersonAction::Format];

const VERBS: &[PackVerb] = &[
    PackVerb::structured_read("sync show", "sync show", S, &[0], sync_show).positions(&[]),
    PackVerb::structured_edit("item bump", "item bump <item>", S, &[0, 2], item_bump)
        .positions(&[]),
];

fn synced_word(ctx: &VerbCtx<'_>) -> String {
    ctx.synced().map_or("none".to_owned(), |s| s.to_string())
}

/// `sync show`: `version V synced S` (`none` with no file).
fn sync_show(_: &[String]) -> Result<StructEdit, ToolError> {
    Ok(Box::new(|ctx: &VerbCtx<'_>| {
        let out = format!(
            "version {} synced {}\n",
            ctx.version().map_or("none".to_owned(), |v| v.to_string()),
            synced_word(ctx)
        );
        Ok(StructOutput::new(Vec::new(), out.into_bytes(), 0))
    }))
}

/// `item bump <item>`: its `n` goes one up; prints the sync point it saw.
fn item_bump(args: &[String]) -> Result<StructEdit, ToolError> {
    let [k] = args else {
        return Err(ToolError::usage("name one item"));
    };
    let at = format!("{k}\u{1f}n");
    Ok(Box::new(move |ctx: &VerbCtx<'_>| {
        let n = match ctx.view().map(FIELDS).and_then(|m| m.get(&at)) {
            Some(Value::Str(s)) => s.parse::<u64>().unwrap_or(0),
            _ => 0,
        };
        Ok(StructOutput::new(
            vec![Edit::Map {
                root: FIELDS.into(),
                ops: vec![MapOp::Set {
                    key: at.clone(),
                    value: Value::Str((n + 1).to_string()),
                }],
            }],
            format!("synced {}\n", synced_word(ctx)).into_bytes(),
            0,
        ))
    }))
}

struct TestHost {
    now: AtomicU64,
}

impl Host for TestHost {
    fn rights(&self, _who: &Participant, _key: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, _ev: &AuditEvent) {}
}

struct Fx {
    dir: PathBuf,
    ws: PathBuf,
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
        "k2mb-sync-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let ws = dir.join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let ws = std::fs::canonicalize(ws).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(100_000),
    });
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    let mgr = SessionManager::new(store, host, c);
    Fx { dir, ws, mgr }
}

fn key(p: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: p.into(),
    }
}

fn agent(s: &Session) -> ConnId {
    s.join(
        Participant::new("atlas", "Atlas", "#6d5bd0", ParticipantKind::Agent, "cli"),
        Stay::Held,
    )
    .unwrap()
    .conn
}

fn argv(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

fn show(s: &Session, c: ConnId) -> String {
    let o = s.run_tool(c, ITEMS, &argv(&["sync", "show"]), b"").unwrap();
    assert_eq!(o.exit, 0);
    String::from_utf8(o.stdout).unwrap()
}

fn bump(s: &Session, c: ConnId, item: &str) -> (u64, String) {
    let v = s.status().unwrap().version;
    let a = argv(&["item", "bump", item, "--base", &v.to_string()]);
    let o = s.run_tool(c, ITEMS, &a, b"").unwrap();
    assert_eq!(o.exit, 0, "{}", o.stderr_summary);
    assert!(o.changed.is_some(), "item bump changed nothing");
    (o.version, String::from_utf8(o.stdout).unwrap())
}

#[test]
fn a_verb_sees_the_sync_point_move_only_with_the_file() {
    let f = fx("file");
    let p = f.ws.join("list.items");
    std::fs::write(&p, "a=0\nb=0\n").unwrap();
    let s = f
        .mgr
        .open_file(Arc::new(Items), &key("list.items"), &p, &f.ws)
        .unwrap();
    let c = agent(&s);
    let v0 = s.status().unwrap().version;
    assert_eq!(
        show(&s, c),
        format!("version {v0} synced {v0}\n"),
        "at the open, the file holds the doc"
    );
    let (v1, saw) = bump(&s, c, "a");
    assert_eq!(saw, format!("synced {v0}\n"), "the edit verb sees it too");
    let (v2, saw) = bump(&s, c, "b");
    assert!(v2 > v1 && v1 > v0);
    assert_eq!(
        saw,
        format!("synced {v0}\n"),
        "an unwritten edit leaves the sync point where it was"
    );
    assert_eq!(show(&s, c), format!("version {v2} synced {v0}\n"));
    let out = s.flush(c).unwrap();
    assert!(out.written, "the flush wrote the file");
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "a=1\nb=1\n");
    assert_eq!(
        show(&s, c),
        format!("version {v2} synced {v2}\n"),
        "the write-back moved the sync point to the doc's version"
    );
    let (v3, saw) = bump(&s, c, "a");
    assert_eq!(saw, format!("synced {v2}\n"));
    assert_eq!(show(&s, c), format!("version {v3} synced {v2}\n"));
}

#[test]
fn a_doc_with_no_file_has_no_sync_point() {
    let f = fx("nofile");
    let s = f
        .mgr
        .open(Arc::new(Items), &key("mem.items"), Some(b"a=0\n"))
        .unwrap();
    let c = agent(&s);
    let v0 = s.status().unwrap().version;
    assert_eq!(show(&s, c), format!("version {v0} synced none\n"));
    let (_, saw) = bump(&s, c, "a");
    assert_eq!(saw, "synced none\n");
}
