//! The structure index's read side (slice A2) and claims by node (slice A5): the typed reads a
//! host's views call, `BlobSession::run_tool`'s verbs over them, the address grammar on nodes,
//! the read budget and its cursors, the registry a pack's blob docs get, the parity of the
//! verbs with the typed calls, and claims that never block a write. The file type is a
//! stand-in ([`index_pack`]); the kit knows no format.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

mod index_pack;
use index_pack::{file, IxPack, VERBS};

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::differential::under_env_index_mutant;
use k2_mpc_base::index::{
    IndexStatus, ListQuery, NodeChangeKind, NodeKey, READ_BUDGET, READ_BUDGET_MIN,
};
use k2_mpc_base::parity::{self, ParityRow, PersonAction, Registry, Tier};
use k2_mpc_base::session::{
    AnyFile, AuditEvent, BlobSession, ClaimEnd, ClaimSpan, Config, ConnId, Host, Participant,
    ParticipantKind, Rights, SessionError, SessionManager, Stay,
};
use k2_mpc_base::store::{DocKey, DocStore, VersionId};
use k2_mpc_base::tools::{PositionalMode, ToolOptions, ToolOutput, ToolRunError};

/// Run `f` with the mutant `MPC_INDEX_MUTANT` names planted on this thread (unset: as is).
fn m<R>(f: impl FnOnce() -> R) -> R {
    under_env_index_mutant(f)
}

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
        "k2mb-read-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
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
    let mgr = SessionManager::new(store, host.clone(), c).with_blob_store(blobs);
    Fx { dir, host, mgr }
}

fn key(p: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: p.into(),
    }
}

impl Fx {
    fn open(&self, path: &str, bytes: &[u8]) -> Arc<BlobSession> {
        self.mgr
            .open_blob(
                Arc::new(IxPack::default()),
                &key(path),
                Some(&mut &bytes[..]),
            )
            .unwrap()
    }
    fn grant(&self, pid: &str, r: Rights) {
        self.host.rights.lock().unwrap().insert(pid.into(), r);
    }
    fn join_as(&self, s: &BlobSession, pid: &str, r: Rights, kind: ParticipantKind) -> ConnId {
        self.grant(pid, r);
        s.join(
            Participant::new(pid, pid, "#123456", kind, "tab"),
            Stay::Held,
        )
        .unwrap()
        .conn
    }
    fn join(&self, s: &BlobSession, pid: &str, r: Rights) -> ConnId {
        self.join_as(s, pid, r, ParticipantKind::Person)
    }
    fn advance(&self, ms: u64) {
        self.host.now.fetch_add(ms, Ordering::SeqCst);
    }
    fn events(&self) -> Vec<AuditEvent> {
        self.host.events.lock().unwrap().clone()
    }
}

/// Run the jobs of every version that has no index yet.
fn index_all(s: &BlobSession, c: ConnId) {
    for (v, _) in s.versions(c).unwrap() {
        if let Some(j) = s.index_job(Some(v)).unwrap() {
            j.run().unwrap();
        }
    }
}

fn write(s: &BlobSession, c: ConnId, bytes: &[u8]) -> VersionId {
    let base = s.status().unwrap().version;
    s.write_bytes(c, bytes, base).unwrap().version
}

fn argv(a: &[&str]) -> Vec<String> {
    a.iter().map(|x| (*x).to_owned()).collect()
}

fn run(s: &BlobSession, c: ConnId, a: &[&str]) -> Result<ToolOutput, ToolRunError> {
    s.run_tool(c, &argv(a), &ToolOptions::default())
}

fn out(o: &ToolOutput) -> String {
    String::from_utf8(o.stdout.clone()).unwrap()
}

/// The refusal's id, for a session refusal.
fn id_of(e: &ToolRunError) -> &'static str {
    e.report().id()
}

/// `@n:1` Layers > (Title, Photo hidden, Inner > Logo), then Back.
const TREE: &[&str] = &[
    "@n:1|group|Layers|-|1|-|-",
    "@n:2|text|Title|0|1|Hello|-",
    "@n:3|raster|Photo|0|0|-|pixels|-",
    "@n:4|group|Inner|0|1|-|-",
    "@n:5|raster|Logo|3|1|-|logo|-",
    "@n:6|raster|Back|-|1|-|bg",
];

/// TREE, then: Title renamed, Photo shown, Logo removed, Back moved into Inner with new
/// content, New added.
const TREE2: &[&str] = &[
    "@n:1|group|Layers|-|1|-|-",
    "@n:2|text|Heading|0|1|Hello|-",
    "@n:3|raster|Photo|0|1|-|pixels|-",
    "@n:4|group|Inner|0|1|-|-",
    "@n:6|raster|Back|3|1|-|bg2",
    "@n:7|raster|New|-|1|-|new",
];

fn keys_of(o: &k2_mpc_base::index::Outline) -> Vec<&str> {
    o.nodes.iter().map(|n| n.key.as_str()).collect()
}

#[test]
fn the_outline_is_depth_first_with_positions_and_filters() {
    m(|| {
        let f = fx("outline");
        let s = f.open("a.idx", &file(TREE));
        let c = f.join(&s, "ann", Rights::full());
        index_all(&s, c);
        let o = s.index_list(c, &ListQuery::new()).unwrap();
        assert_eq!(
            keys_of(&o),
            ["@n:1", "@n:2", "@n:3", "@n:4", "@n:5", "@n:6"],
            "the outline is depth first, children in sibling order"
        );
        let pos: Vec<u32> = o.nodes.iter().map(|n| n.position).collect();
        assert_eq!(pos, [1, 2, 3, 4, 5, 6]);
        assert_eq!(o.total, 6);
        assert!(!o.nodes[2].visible_in_tree, "Photo is hidden");
        let roots = s.index_list(c, &ListQuery::new().depth(0)).unwrap();
        assert_eq!(keys_of(&roots), ["@n:1", "@n:6"]);
        let rasters = s.index_list(c, &ListQuery::new().kind("raster")).unwrap();
        assert_eq!(keys_of(&rasters), ["@n:3", "@n:5", "@n:6"]);
        assert_eq!(
            rasters.nodes.iter().map(|n| n.position).collect::<Vec<_>>(),
            [3, 5, 6],
            "a filter never renumbers"
        );
        // the verb prints the same, one line a node
        let t = run(&s, c, &["part", "list", "--tree"]).unwrap();
        assert_eq!(t.exit, 0);
        assert_eq!(
            out(&t),
            "#1\t@n:1\tgroup\t\"Layers\"\n  #2\t@n:2\ttext\t\"Title\"\n  #3\t@n:3\traster\t\"Photo\"\thidden\n  \
             #4\t@n:4\tgroup\t\"Inner\"\n    #5\t@n:5\traster\t\"Logo\"\n#6\t@n:6\traster\t\"Back\"\n"
        );
        assert_eq!(t.stderr_summary, "a.idx v1");
        assert!(
            t.fence.is_some(),
            "names come from the file: the output is fenced"
        );
        let j = run(&s, c, &["outline", "--json"]).unwrap();
        assert_eq!(out(&j), o.to_json() + "\n", "the verb is the typed call");
        // the kit names are the pack's verbs
        let j2 = run(&s, c, &["part", "list"]).unwrap();
        let j3 = run(&s, c, &["outline"]).unwrap();
        assert_eq!(j2.stdout, j3.stdout);
    });
}

#[test]
fn every_read_asks_for_its_rights() {
    m(|| {
        let f = fx("rights");
        let s = f.open("a.idx", &file(TREE));
        let w = f.join(&s, "wes", Rights::full());
        write(&s, w, &file(TREE2));
        index_all(&s, w);
        let r = f.join(&s, "rae", Rights::read_only());
        // the current version needs read only
        assert!(s.index_list(r, &ListQuery::new()).is_ok());
        assert!(s.index_nodes(r, &["#1".into()], None, 0).is_ok());
        // another version, a diff and a blame need history
        let hist = |e: SessionError| {
            matches!(
                e,
                SessionError::Forbidden {
                    need: k2_mpc_base::session::Need::History
                }
            )
        };
        assert!(
            s.index_list(r, &ListQuery::new().version(1))
                .is_err_and(hist),
            "an old version's index needs history"
        );
        assert!(
            s.index_nodes(r, &["#1".into()], Some(1), 0)
                .is_err_and(hist),
            "an old version's nodes need history"
        );
        assert!(
            s.index_diff(r, 1, 2).is_err_and(hist),
            "a diff needs history"
        );
        assert!(
            s.index_blame(r, None).is_err_and(hist),
            "a blame needs history"
        );
        let e = run(&s, r, &["part", "list", "--version", "1"]).unwrap_err();
        assert_eq!(e.exit(), 3, "the verb is refused as the call is");
        // read taken away: refused, and removed, on the next read of any kind
        f.grant("rae", Rights::none());
        assert!(
            matches!(
                s.index_list(r, &ListQuery::new()),
                Err(SessionError::Forbidden { .. })
            ),
            "rights are asked on every read"
        );
        assert!(matches!(
            s.index_list(r, &ListQuery::new()),
            Err(SessionError::NotJoined { .. })
        ));
    });
}

#[test]
fn an_address_that_names_nothing_is_reported_and_never_guessed() {
    m(|| {
        let f = fx("missing");
        let s = f.open("a.idx", &file(TREE));
        let c = f.join(&s, "ann", Rights::full());
        index_all(&s, c);
        let addrs: Vec<String> = argv(&["@n:2", "name:Titl", "#99", "@k:nope", "Logo"]);
        let r = s.index_nodes(c, &addrs, None, 0).unwrap();
        assert_eq!(r.nodes.len(), 5, "one entry per address, in order");
        assert_eq!(
            r.nodes[0].as_ref().map(|n| n.node.key.as_str()),
            Some("@n:2")
        );
        assert!(r.nodes[1].is_none(), "a missing name is never guessed");
        assert!(
            r.nodes[2].is_none(),
            "a position past the end names nothing"
        );
        assert!(r.nodes[3].is_none());
        assert_eq!(
            r.nodes[4].as_ref().map(|n| n.position),
            Some(5),
            "a bare word is a name"
        );
        assert_eq!(
            r.missing
                .iter()
                .map(|m| m.address.as_str())
                .collect::<Vec<_>>(),
            ["name:Titl", "#99", "@k:nope"]
        );
        // what exists is listed, by key and place, for the caller to pick
        let near: Vec<(&str, Option<&str>)> = r.missing[0]
            .candidates
            .iter()
            .map(|c| (c.address.as_str(), c.position.as_deref()))
            .collect();
        assert_eq!(near, [("@n:2", Some("#2"))]);
        // the verb: every address listed, null for the missing, exit 4
        let o = run(&s, c, &["part", "show", "@n:2", "name:Titl", "--json"]).unwrap();
        assert_eq!(o.exit, 4, "a partial answer exits 4");
        let rep = o.refusal.as_ref().expect("the refusal");
        assert_eq!(rep.id(), "MPC-E172");
        assert!(rep.detail("missing").is_some());
        let two = argv(&["@n:2", "name:Titl"]);
        let typed = s.index_nodes(c, &two, None, 0).unwrap();
        assert_eq!(out(&o), typed.to_json(&two) + "\n");
        assert!(out(&o).contains("\"node\":null"));
        // a number is never a position
        let n = s.index_nodes(c, &["2".into()], None, 0).unwrap();
        assert!(n.nodes[0].is_none(), "a bare number is a name, not #2");
        // all found: exit 0
        assert_eq!(run(&s, c, &["node", "#1", "--depth", "2"]).unwrap().exit, 0);
    });
}

#[test]
fn a_name_or_id_two_nodes_share_is_refused_with_every_holder() {
    m(|| {
        let f = fx("ambiguous");
        let tree = [
            "@n:1|group|Copy|-|1|-|-",
            "@n:2|raster|Copy|-|1|-|a",
            "@n:5|raster|Twin|-|1|-|b",
            "@n:5|raster|Twin2|-|1|-|c",
        ];
        let s = f.open("a.idx", &file(&tree));
        let c = f.join(&s, "ann", Rights::full());
        index_all(&s, c);
        let e = match s.index_nodes(c, &["Copy".into()], None, 0) {
            Err(e) => e,
            Ok(r) => panic!("an ambiguous name is refused, never read: {r:?}"),
        };
        let SessionError::IndexRead(rep) = &e else {
            panic!("an ambiguous name is refused with its id: {e:?}")
        };
        assert_eq!(rep.id(), "MPC-E170");
        let cands = format!("{:?}", rep.detail("candidates"));
        assert!(cands.contains("@n:1") && cands.contains("@n:2"), "{cands}");
        // a duplicated native ID: both holders got minted keys, and the ID names neither
        let o = s.index_list(c, &ListQuery::new()).unwrap();
        let twins: Vec<&str> = o.nodes[2..].iter().map(|n| n.key.as_str()).collect();
        assert!(twins.iter().all(|k| k.starts_with("@k:")), "{twins:?}");
        let e = run(&s, c, &["part", "show", "@n:5"]).unwrap_err();
        assert_eq!(id_of(&e), "MPC-E170", "an ID two nodes share is refused");
        assert_eq!(e.exit(), 2);
        // each by its own key
        let by_key = s.index_nodes(c, &[twins[0].to_owned()], None, 0).unwrap();
        assert_eq!(by_key.nodes[0].as_ref().unwrap().node.name, "Twin");
    });
}

/// The `"cursor":"…"` of a JSON page, if any.
fn cursor_of(json: &str) -> Option<String> {
    let at = json.find("\"cursor\":\"")? + 10;
    let end = json[at..].find('"')?;
    Some(json[at..at + end].to_owned())
}

fn many(n: usize) -> Vec<String> {
    (0..n)
        .map(|i| format!("@n:{i}|raster|node {i:04} {}|-|1|-|c{i}", "x".repeat(40)))
        .collect()
}

#[test]
fn pages_keep_to_the_budget_and_join_to_the_whole() {
    m(|| {
        let f = fx("pages");
        let lines = many(300);
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let s = f.open("a.idx", &file(&refs));
        let c = f.join(&s, "ann", Rights::full());
        index_all(&s, c);
        let budget = READ_BUDGET_MIN;
        let opts = ToolOptions::default().read_budget(budget);
        let whole = s.index_list(c, &ListQuery::new()).unwrap();
        let (mut pages, mut cursor, mut keys) = (0, None::<String>, Vec::new());
        loop {
            let mut a = argv(&["part", "list", "--json"]);
            if let Some(cur) = &cursor {
                a.extend(argv(&["--cursor", cur]));
            }
            let o = s.run_tool(c, &a, &opts).unwrap();
            let j = out(&o);
            assert!(
                j.trim_end().len() <= budget,
                "every page is within the budget ({} bytes)",
                j.len()
            );
            // the text page is the same page, with the same cursor
            let mut ta = argv(&["part", "list"]);
            if let Some(cur) = &cursor {
                ta.extend(argv(&["--cursor", cur]));
            }
            let t = out(&s.run_tool(c, &ta, &opts).unwrap());
            let next = cursor_of(&j);
            match &next {
                Some(n) => assert!(t.ends_with(&format!("--cursor {n}\n")), "{t}"),
                None => assert!(!t.contains("truncated")),
            }
            keys.extend(
                t.lines()
                    .filter(|l| l.starts_with('#'))
                    .map(|l| l.split('\t').nth(1).unwrap().to_owned()),
            );
            pages += 1;
            cursor = next;
            if cursor.is_none() {
                break;
            }
            assert!(pages < 100, "paging ends");
        }
        assert!(
            pages > 3,
            "a page is a budget's worth, not everything ({pages} pages)"
        );
        let all: Vec<String> = whole.nodes.iter().map(|n| n.key.to_string()).collect();
        assert_eq!(keys, all, "pages joined are the whole outline");
        // the default budget, and the floor a host can't go under
        assert_eq!(ToolOptions::default().read_budget, READ_BUDGET);
        assert_eq!(
            ToolOptions::default().read_budget(1).read_budget,
            READ_BUDGET_MIN
        );
        // everything or nothing
        let e = match s.run_tool(c, &argv(&["part", "list", "--no-truncate"]), &opts) {
            Err(e) => e,
            Ok(o) => panic!(
                "--no-truncate refuses past the budget, never pages ({} bytes)",
                o.stdout.len()
            ),
        };
        assert_eq!(
            id_of(&e),
            "MPC-E175",
            "--no-truncate refuses with over_budget"
        );
        assert_eq!(e.exit(), 2);
        let small = ToolOptions::default().read_budget(1 << 20);
        let o = s
            .run_tool(c, &argv(&["part", "list", "--no-truncate"]), &small)
            .unwrap();
        assert_eq!(o.exit, 0);
    });
}

#[test]
fn a_cursor_is_bound_to_its_version_doc_and_arguments() {
    m(|| {
        let f = fx("cursor");
        let lines = many(200);
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let s = f.open("a.idx", &file(&refs));
        let s2 = f.open("b.idx", &file(&refs));
        let c = f.join(&s, "ann", Rights::full());
        let c2 = f.join(&s2, "ann", Rights::full());
        index_all(&s, c);
        index_all(&s2, c2);
        let opts = ToolOptions::default().read_budget(READ_BUDGET_MIN);
        let page = |s: &BlobSession, c: ConnId, extra: &[&str]| {
            let mut a = argv(&["part", "list", "--json"]);
            a.extend(argv(extra));
            s.run_tool(c, &a, &opts)
        };
        let cur = cursor_of(&out(&page(&s, c, &[]).unwrap())).expect("more than one page");
        assert!(
            !cur.contains("a.idx") && !cur.contains("ws"),
            "a cursor carries no path"
        );
        assert!(
            page(&s, c, &["--cursor", &cur]).is_ok(),
            "the cursor continues its read"
        );
        let e = match page(&s2, c2, &["--cursor", &cur]) {
            Err(e) => e,
            Ok(_) => panic!("a cursor on another doc is stale, never read"),
        };
        assert_eq!(id_of(&e), "MPC-E173", "a cursor on another doc is stale");
        let e = page(&s, c, &["--cursor", &cur, "--kind", "raster"]).unwrap_err();
        assert_eq!(
            id_of(&e),
            "MPC-E173",
            "a cursor with other arguments is stale"
        );
        assert_eq!(e.exit(), 1);
        let mut bad = cur.clone();
        bad.truncate(bad.len() - 2);
        let e = page(&s, c, &["--cursor", &bad]).unwrap_err();
        assert_eq!(id_of(&e), "MPC-E174", "a mangled cursor is refused");
        assert_eq!(e.exit(), 2);
        let mut flipped = cur.clone().into_bytes();
        let last = flipped.len() - 1;
        flipped[last] = if flipped[last] == b'0' { b'1' } else { b'0' };
        let e = page(&s, c, &["--cursor", &String::from_utf8(flipped).unwrap()]).unwrap_err();
        assert_eq!(id_of(&e), "MPC-E173", "a changed cursor is refused");
        // an explicit version's cursor outlives an edit; the current one's doesn't
        let v1 = cursor_of(&out(&page(&s, c, &["--version", "1"]).unwrap())).unwrap();
        let mut moved = lines.clone();
        moved.push("@n:9999|raster|late|-|1|-|late".into());
        let refs2: Vec<&str> = moved.iter().map(String::as_str).collect();
        write(&s, c, &file(&refs2));
        index_all(&s, c);
        let e = page(&s, c, &["--cursor", &cur]).unwrap_err();
        assert_eq!(id_of(&e), "MPC-E173", "a cursor after an edit is stale");
        assert!(
            page(&s, c, &["--version", "1", "--cursor", &v1]).is_ok(),
            "a cursor for a named version stays good"
        );
        // rights come before the cursor
        let r = f.join(&s, "rae", Rights::read_only());
        let e = page(&s, r, &["--version", "1", "--cursor", "junk"]).unwrap_err();
        assert_eq!(e.exit(), 3, "rights are checked before the cursor");
    });
}

#[test]
fn a_node_bigger_than_a_page_comes_alone_and_its_text_is_read_whole() {
    m(|| {
        let f = fx("big");
        let text = "word ".repeat(9_000);
        let tree = [
            "@n:1|group|Root|-|1|-|-".to_owned(),
            format!("@n:2|text|Long|0|1|{text}|-"),
        ];
        let refs: Vec<&str> = tree.iter().map(String::as_str).collect();
        let s = f.open("a.idx", &file(&refs));
        let c = f.join(&s, "ann", Rights::full());
        index_all(&s, c);
        let o = run(&s, c, &["part", "show", "@n:1", "@n:2", "--json"]).unwrap();
        let j = out(&o);
        assert_eq!(o.exit, 0);
        assert!(j.contains("\"text_omitted\":true"), "the text is left out");
        assert!(j.contains(&format!("\"text_bytes\":{}", text.len())));
        assert!(
            j.contains("\"text_read\":\"part show @n:2 --version 1 --text\""),
            "{j}"
        );
        let t = run(&s, c, &["part", "show", "@n:2", "--version", "1", "--text"]).unwrap();
        assert_eq!(out(&t), text, "the read the page names returns every byte");
        let none = run(&s, c, &["part", "show", "@n:1", "--text"]).unwrap();
        assert_eq!(none.exit, 1, "no text: the tool's no");
    });
}

#[test]
fn a_diff_names_each_change_and_a_shift_is_not_a_move() {
    m(|| {
        let f = fx("diff");
        let s = f.open("a.idx", &file(TREE));
        let c = f.join(&s, "ann", Rights::full());
        write(&s, c, &file(TREE2));
        index_all(&s, c);
        let d = s.index_diff(c, 1, 2).unwrap();
        let got: Vec<(&str, Vec<NodeChangeKind>, Vec<String>)> = d
            .changes
            .iter()
            .map(|x| {
                (
                    x.key.as_str(),
                    x.changes.iter().copied().collect(),
                    x.props.clone(),
                )
            })
            .collect();
        use NodeChangeKind::*;
        assert_eq!(
            got,
            [
                ("@n:2", vec![Renamed], vec![]),
                ("@n:3", vec![Props], vec!["visible".to_owned()]),
                ("@n:6", vec![Moved, Content], vec![]),
                ("@n:7", vec![Added], vec![]),
                ("@n:5", vec![Removed], vec![]),
            ]
        );
        assert_eq!(d.changes[0].name_then.as_deref(), Some("Title"));
        let v = run(&s, c, &["part", "diff", "1", "2", "--json"]).unwrap();
        assert_eq!(out(&v), s.index_diff(c, 1, 2).unwrap().to_json() + "\n");
        let short = run(&s, c, &["part", "diff", "1"]).unwrap();
        assert!(out(&short).starts_with("@n:2\trenamed"), "{}", out(&short));
    });
}

#[test]
fn siblings_that_only_shifted_are_not_moved() {
    m(|| {
        let f = fx("shift");
        let a = [
            "@n:1|group|G|-|1|-|-",
            "@n:2|raster|A|0|1|-|a",
            "@n:3|raster|B|0|1|-|b",
        ];
        let s = f.open("a.idx", &file(&a));
        let c = f.join(&s, "ann", Rights::full());
        // one put first: the others shift, and only it is new
        let b = [a[0], "@n:4|raster|Top|0|1|-|t", a[1], a[2]];
        write(&s, c, &file(&b));
        // then two swap: the longest run that kept its order stays, the other moved
        let swapped = [a[0], b[1], a[2], a[1]];
        write(&s, c, &file(&swapped));
        index_all(&s, c);
        let d = s.index_diff(c, 1, 2).unwrap();
        let keys: Vec<&str> = d.changes.iter().map(|x| x.key.as_str()).collect();
        assert_eq!(keys, ["@n:4"], "siblings that only shifted are not moved");
        let d = s.index_diff(c, 2, 3).unwrap();
        let got: Vec<(&str, Vec<NodeChangeKind>)> = d
            .changes
            .iter()
            .map(|x| (x.key.as_str(), x.changes.iter().copied().collect()))
            .collect();
        assert_eq!(got, [("@n:2", vec![NodeChangeKind::Moved])]);
    });
}

#[test]
fn blame_walks_back_while_a_node_is_unchanged() {
    m(|| {
        let f = fx("blame");
        let s = f.open("a.idx", &file(TREE));
        let a = f.join(&s, "ann", Rights::full());
        let b = f.join(&s, "bob", Rights::full());
        f.advance(10);
        write(&s, b, &file(TREE2));
        index_all(&s, a);
        let bl = s.index_blame(a, None).unwrap();
        let row = |k: &str| bl.rows.iter().find(|r| r.key.as_str() == k).unwrap();
        assert_eq!(row("@n:1").version, 1, "unchanged since the import");
        assert_eq!(row("@n:1").author, "kit:import");
        assert_eq!(row("@n:2").version, 2, "renamed by bob");
        assert_eq!(row("@n:2").author, "bob");
        assert!(bl.rows.iter().all(|r| r.settled) && bl.stopped_at.is_none());
        // a version with no index stops the walk; what is still open says so
        let c = f.join(&s, "cy", Rights::full());
        let mut t3 = TREE2.to_vec();
        t3.push("@n:9|raster|Extra|-|1|-|x");
        write(&s, c, &file(&t3));
        write(
            &s,
            c,
            &file(&{
                let mut t4 = t3.clone();
                t4.push("@n:10|raster|More|-|1|-|y");
                t4
            }),
        );
        s.index_job(None).unwrap().unwrap().run().unwrap();
        let bl = s.index_blame(a, None).unwrap();
        assert_eq!(bl.stopped_at, Some(3));
        assert_eq!(bl.missing, [3]);
        let r1 = bl.rows.iter().find(|r| r.key.as_str() == "@n:1").unwrap();
        assert!(!r1.settled && r1.version == 4, "{r1:?}");
        let r10 = bl.rows.iter().find(|r| r.key.as_str() == "@n:10").unwrap();
        assert!(
            !r10.settled,
            "a stopped walk settles nothing it didn't reach"
        );
        let v = run(&s, a, &["part", "blame", "--json"]).unwrap();
        assert_eq!(out(&v), bl.to_json() + "\n");
    });
}

#[test]
fn an_index_that_isnt_there_says_why() {
    m(|| {
        let f = fx("absent");
        let s = f.open("a.idx", &file(TREE));
        let c = f.join(&s, "ann", Rights::full());
        let e = run(&s, c, &["part", "list"]).unwrap_err();
        assert_eq!(id_of(&e), "MPC-E713", "not indexed yet: pending, retry");
        assert_eq!(e.exit(), 1);
        assert!(matches!(
            s.status().unwrap().index,
            Some(IndexStatus::Pending { version: 1, .. })
        ));
        index_all(&s, c);
        assert!(matches!(
            s.status().unwrap().index,
            Some(IndexStatus::Ready {
                version: 1,
                nodes: 6
            })
        ));
        // a type with no index
        let any = f
            .mgr
            .open_blob(Arc::new(AnyFile), &key("b.bin"), Some(&mut &b"bytes"[..]))
            .unwrap();
        let a = f.join(&any, "ann", Rights::full());
        assert!(any.status().unwrap().index.is_none());
        for verb in [
            &["outline"][..],
            &["node", "#1"],
            &["claim", "--node", "#1"],
        ] {
            let e = run(&any, a, verb).unwrap_err();
            assert_eq!(id_of(&e), "MPC-E711", "{verb:?}");
        }
        // run_tool runs these verbs only
        let e = run(&s, c, &["cat"]).unwrap_err();
        assert_eq!(e.exit(), 2);
        let e = run(&s, c, &["part", "list", "--bogus"]).unwrap_err();
        assert_eq!(e.exit(), 2);
    });
}

#[test]
fn node_labels_name_the_nodes_someone_is_on() {
    m(|| {
        let f = fx("labels");
        let s = f.open("a.idx", &file(TREE));
        let c = f.join(&s, "ann", Rights::full());
        index_all(&s, c);
        s.set_region(c, Some("@n:3".into())).unwrap();
        let region = s.roster(c).unwrap()[0].region.clone().unwrap();
        let keys = [
            NodeKey::parse(&region).unwrap(),
            NodeKey::parse("@n:77").unwrap(),
        ];
        let l = s.node_labels(c, &keys).unwrap();
        let first = l[0].as_ref().unwrap();
        assert_eq!(
            (first.name.as_str(), first.kind.as_str(), first.visible),
            ("Photo", "raster", false)
        );
        assert!(l[1].is_none());
    });
}

/// The claims `pid` holds now, as the roster shows them.
fn roster_claims(s: &BlobSession, c: ConnId, pid: &str) -> Vec<String> {
    s.roster(c)
        .unwrap()
        .into_iter()
        .find(|e| e.participant.pid == pid)
        .map(|e| e.claims.iter().map(|x| x.span.to_string()).collect())
        .unwrap_or_default()
}

fn ended(f: &Fx) -> Vec<ClaimEnd> {
    f.events()
        .iter()
        .filter_map(|e| match e {
            AuditEvent::ClaimEnded { how, .. } => Some(*how),
            _ => None,
        })
        .collect()
}

#[test]
fn a_node_claim_is_advisory_and_never_blocks_a_save() {
    m(|| {
        let f = fx("claims");
        let s = f.open("a.idx", &file(TREE));
        let ann = f.join_as(&s, "ann", Rights::full(), ParticipantKind::Agent);
        let bob = f.join(&s, "bob", Rights::full());
        index_all(&s, ann);
        let k2 = NodeKey::parse("@n:2").unwrap();
        let one = std::slice::from_ref(&k2);
        let c = s
            .claim_nodes(ann, one, None, Some("retitling".into()))
            .unwrap();
        assert!(matches!(&c.span, ClaimSpan::Nodes { keys } if keys.as_slice() == one));
        assert_eq!(
            roster_claims(&s, bob, "ann"),
            ["node @n:2"],
            "the roster shows it"
        );
        // the same node is someone else's: refused, exit 5
        let e = s.claim_nodes(
            bob,
            &[k2.clone(), NodeKey::parse("@n:3").unwrap()],
            None,
            None,
        );
        assert!(
            matches!(&e, Err(SessionError::NodesClaimed(x)) if x.by == "ann" && x.keys == [k2.clone()]),
            "another's claim on the node is refused: {e:?}"
        );
        assert_eq!(k2_mpc_base::tools::session_exit(&e.unwrap_err()), 5);
        // other nodes are free
        assert!(s
            .claim_nodes(bob, &[NodeKey::parse("@n:3").unwrap()], None, None)
            .is_ok());
        // a person with the lease saves over a claimed node: a claim never blocks a save
        s.acquire_lease(bob, None).unwrap();
        let v = s.status().unwrap().version;
        let w = s.write_bytes(bob, &file(TREE2), v);
        assert!(w.is_ok(), "a claim never blocks a save: {w:?}");
        s.release_lease(bob).unwrap();
        // nor the claimant's own write, nor a restore
        assert!(s
            .write_bytes(ann, &file(TREE), s.status().unwrap().version)
            .is_ok());
        assert!(s.restore(bob, 2).is_ok());
        // a claim is carried by key across versions
        index_all(&s, ann);
        assert_eq!(s.claims(bob).unwrap().len(), 2);
        // release
        let gone = s.release_claim(bob, None).unwrap();
        assert_eq!(gone.len(), 1);
        assert_eq!(
            run(&s, bob, &["claim", "--release"]).unwrap().exit,
            1,
            "none left: 1"
        );
        // time
        let t = s
            .claim_nodes(bob, &[NodeKey::parse("@n:4").unwrap()], Some(50), None)
            .unwrap();
        f.advance(51);
        assert!(
            s.claims(bob).unwrap().iter().all(|x| x.id != t.id),
            "it ran out"
        );
        // leaving
        let cy = f.join(&s, "cy", Rights::full());
        s.claim_nodes(cy, &[NodeKey::parse("@n:1").unwrap()], None, None)
            .unwrap();
        s.leave(cy).unwrap();
        assert!(roster_claims(&s, bob, "cy").is_empty());
        assert_eq!(
            ended(&f),
            [ClaimEnd::Released, ClaimEnd::Expired, ClaimEnd::Left]
        );
        // a version without its node: once indexed, the claim ends, deleted
        let v = s.status().unwrap().version;
        // TREE2 without @n:2 (the parents renumbered: a parent is a line index)
        let without = [
            "@n:1|group|Layers|-|1|-|-",
            "@n:3|raster|Photo|0|1|-|pixels|-",
            "@n:4|group|Inner|0|1|-|-",
            "@n:6|raster|Back|2|1|-|bg2",
            "@n:7|raster|New|-|1|-|new",
        ];
        s.write_bytes(bob, &file(&without), v).unwrap();
        assert_eq!(
            s.claims(bob).unwrap().len(),
            1,
            "not indexed yet: unchanged"
        );
        index_all(&s, bob);
        assert!(
            s.claims(bob).unwrap().is_empty(),
            "a claim whose nodes are gone ends"
        );
        assert_eq!(ended(&f).last(), Some(&ClaimEnd::Deleted));
    });
}

#[test]
fn claim_verbs_resolve_addresses_and_bind_positions_to_a_version() {
    m(|| {
        let f = fx("claimverbs");
        let s = f.open("a.idx", &file(TREE));
        let agent = f.join_as(&s, "ann", Rights::full(), ParticipantKind::Agent);
        index_all(&s, agent);
        // an agent's #N with no version: a warning now, a refusal when the host says so
        let o = run(&s, agent, &["claim", "--node", "#2"]).unwrap();
        assert_eq!(o.exit, 0);
        assert!(o
            .warnings
            .lines()
            .iter()
            .any(|l| l.contains("positional_needs_version")));
        run(&s, agent, &["claim", "--release"]).unwrap();
        let refuse = ToolOptions::default().positional(PositionalMode::Refuse);
        let e = s
            .run_tool(agent, &argv(&["claim", "--node", "#2"]), &refuse)
            .unwrap_err();
        assert_eq!(id_of(&e), "MPC-E171");
        let ok = s
            .run_tool(
                agent,
                &argv(&["claim", "--node", "#2", "--version", "1"]),
                &refuse,
            )
            .unwrap();
        assert!(ok.warnings.is_empty());
        assert_eq!(s.claims(agent).unwrap()[0].span.to_string(), "node @n:2");
        // a missing address claims nothing
        let e = run(&s, agent, &["claim", "--node", "name:nobody"]).unwrap_err();
        assert_eq!(
            id_of(&e),
            "MPC-E172",
            "an address that names nothing is refused"
        );
        assert_eq!(e.exit(), 4);
        let r = run(&s, agent, &["claim", "--renew", "--ttl", "90000"]).unwrap();
        assert_eq!(r.exit, 0);
        assert_eq!(s.claims(agent).unwrap()[0].remaining_ms, 90_000);
        let e = run(&s, agent, &["claim"]).unwrap_err();
        assert_eq!(e.exit(), 2, "a claim on a blob doc names nodes");
        // a flag's value is never a flag: a note saying --release is a note
        run(&s, agent, &["claim", "--release"]).unwrap();
        let o = run(
            &s,
            agent,
            &["claim", "--node", "@n:3", "--note", "--release"],
        )
        .unwrap();
        assert_eq!(o.exit, 0);
        let mine = s.claims(agent).unwrap();
        assert_eq!(mine.len(), 1, "the call claimed, it didn't release");
        assert_eq!(mine[0].note.as_deref(), Some("--release"));
        // someone else's claim id: refused, naming no one
        let p = f.join(&s, "pat", Rights::full());
        let e = s.release_claim(p, Some(mine[0].id));
        assert_eq!(e, Ok(Vec::new()), "release takes only the caller's own");
        let e = s.renew_claim(p, Some(mine[0].id), None).unwrap_err();
        assert!(
            matches!(&e, SessionError::BadClaim(m) if !m.contains("ann")),
            "{e:?}"
        );
    });
}

/// A character JSON writes as six bytes, `n` times.
fn wide(n: usize) -> String {
    "\u{1}".repeat(n)
}

/// A root with a long name, a long text and a long attribute, and `children` children with
/// long names: every field at its worst for a page.
fn huge(children: usize) -> Vec<u8> {
    let mut lines = vec![format!(
        "@n:0|group|{}|-|1|{}|-|{}",
        wide(4096),
        wide(60_000),
        wide(4096)
    )];
    for i in 1..=children {
        lines.push(format!("@n:{i}|raster|{}|0|1|-|c{i}", wide(4096)));
    }
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    file(&refs)
}

/// Every page of `argv` (with `--cursor` from each to the next) under `opts`: their stdouts.
fn pages(s: &BlobSession, c: ConnId, argv0: &[&str], opts: &ToolOptions) -> Vec<String> {
    let (mut out, mut cursor) = (Vec::new(), None::<String>);
    loop {
        let mut a = argv(argv0);
        if let Some(cur) = &cursor {
            a.extend(argv(&["--cursor", cur]));
        }
        let o = s.run_tool(c, &a, opts).unwrap();
        let page = out_of(&o);
        let json = argv0.contains(&"--json");
        cursor = if json {
            cursor_of(&page)
        } else {
            page.lines()
                .last()
                .and_then(|l| l.strip_prefix("truncated: more with --cursor "))
                .map(str::to_owned)
        };
        out.push(page);
        if cursor.is_none() || out.len() > 1000 {
            return out;
        }
    }
}

fn out_of(o: &ToolOutput) -> String {
    String::from_utf8(o.stdout.clone()).unwrap()
}

#[test]
fn every_page_fits_its_budget_however_large_an_item() {
    m(|| {
        let f = fx("huge");
        let s = f.open("a.idx", &file(&[]));
        let c = f.join(&s, "ann", Rights::full());
        let v = s.status().unwrap().version;
        s.write_bytes(c, &huge(300), v).unwrap();
        index_all(&s, c);
        let budget = READ_BUDGET_MIN;
        let opts = ToolOptions::default().read_budget(budget);
        for mode in [&["--json"][..], &[]] {
            let mut a = vec!["part", "show", "@n:0", "--depth", "64"];
            a.extend_from_slice(mode);
            let all = pages(&s, c, &a, &opts);
            for p in &all {
                assert!(
                    p.len() <= budget,
                    "every page fits the budget ({mode:?}: {} bytes)",
                    p.len()
                );
            }
            let p = &all[0];
            if mode.is_empty() {
                assert!(p.contains("text: left out (60000 bytes); read it with: part show @n:0 --version 2 --text"), "{}", &p[..p.len().min(600)]);
                assert!(p.contains("descendants: 0 of 300 shown; the rest: part list --version 2 --under @n:0 --depth 64"), "{p}");
            } else {
                assert!(p.contains("\"text_omitted\":true"));
                assert!(p.contains("\"descendants_total\":300"));
                assert!(p.contains(
                    "\"descendants_read\":\"part list --version 2 --under @n:0 --depth 64\""
                ));
                assert!(p.contains("\"read\":\"part show @n:0 --version 2 --whole\""));
            }
        }
        // each read a shortened item names returns what it left out
        let t = run(&s, c, &["part", "show", "@n:0", "--version", "2", "--text"]).unwrap();
        assert_eq!(out_of(&t), wide(60_000));
        let w = run(
            &s,
            c,
            &["part", "show", "@n:0", "--version", "2", "--whole"],
        )
        .unwrap();
        assert!(out_of(&w).len() > 4096 * 6 * 2, "the node whole");
        for mode in [&["--json"][..], &[]] {
            let mut a = vec![
                "part",
                "list",
                "--version",
                "2",
                "--under",
                "@n:0",
                "--depth",
                "64",
            ];
            a.extend_from_slice(mode);
            let all = pages(&s, c, &a, &opts);
            assert!(
                all.iter().all(|p| p.len() <= budget),
                "every list page fits the budget"
            );
            let n: usize = all
                .iter()
                .map(|p| {
                    if mode.is_empty() {
                        p.lines().filter(|l| l.starts_with('#')).count()
                    } else {
                        p.matches("\"key_class\"").count()
                    }
                })
                .sum();
            assert_eq!(n, 300, "the continuation lists every descendant");
        }
        // --no-truncate refuses an answer shortened to one page, not only one with more
        let e = match s.run_tool(c, &argv(&["part", "show", "@n:0", "--no-truncate"]), &opts) {
            Err(e) => e,
            Ok(_) => panic!("--no-truncate refuses a shortened item"),
        };
        assert_eq!(id_of(&e), "MPC-E175");
        // a continuation's address that names nothing
        let e = run(&s, c, &["part", "list", "--under", "name:nobody"]).unwrap_err();
        assert_eq!((id_of(&e), e.exit()), ("MPC-E172", 4));
    });
}

#[test]
fn candidates_are_capped_and_counted() {
    m(|| {
        let f = fx("cands");
        let lines: Vec<String> = (0..30)
            .map(|i| format!("@n:{i}|raster|Same|-|1|-|c{i}"))
            .collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let s = f.open("a.idx", &file(&refs));
        let c = f.join(&s, "ann", Rights::full());
        index_all(&s, c);
        let Err(SessionError::IndexRead(rep)) = s.index_nodes(c, &["Same".into()], None, 0) else {
            panic!("an ambiguous name is refused")
        };
        let n = match rep.detail("candidates") {
            Some(k2_mpc_base::errors::Value::List(l)) => l.len(),
            other => panic!("{other:?}"),
        };
        assert_eq!(n, 20, "an ambiguous address lists at most 20 candidates");
        assert_eq!(
            rep.detail("candidates_total").and_then(|v| v.as_u64()),
            Some(30)
        );
        let r = s.index_nodes(c, &["Sam".into()], None, 0).unwrap();
        assert_eq!(
            r.missing[0].candidates.len(),
            20,
            "a missing address lists at most 20"
        );
        assert_eq!(r.missing[0].candidates_total, 30);
    });
}

#[test]
fn the_misses_are_on_the_first_page_only_and_capped() {
    m(|| {
        let f = fx("misses");
        let lines: Vec<String> = (0..60)
            .map(|i| format!("@n:{i}|raster|node {i} {}|-|1|-|c{i}", "y".repeat(200)))
            .collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let s = f.open("a.idx", &file(&refs));
        let c = f.join(&s, "ann", Rights::full());
        index_all(&s, c);
        // 256 misses, each near every node
        let asked: Vec<String> = (0..256).map(|_| "name:yyyyyyyy".to_owned()).collect();
        let mut a = vec!["part", "show", "--json"];
        a.extend(asked.iter().map(String::as_str));
        let opts = ToolOptions::default().read_budget(READ_BUDGET_MIN);
        let all = pages(&s, c, &a, &opts);
        assert!(all.len() > 1, "more than a page of entries");
        assert!(
            all[0].len() <= READ_BUDGET_MIN,
            "the first page's misses fit the budget ({} bytes)",
            all[0].len()
        );
        assert!(all[0].contains("\"missing_total\":256"));
        for p in &all[1..] {
            assert!(
                !p.contains("\"missing\""),
                "the misses are on the first page only"
            );
            assert!(p.len() <= READ_BUDGET_MIN);
        }
    });
}

#[test]
fn no_index_is_decoded_under_the_session_lock() {
    m(|| {
        use k2_mpc_base::differential::index_loads;
        let f = fx("lockless");
        let s = f.open("a.idx", &file(TREE));
        let ann = f.join(&s, "ann", Rights::full());
        index_all(&s, ann);
        let before = index_loads();
        s.claim_nodes(ann, &[NodeKey::parse("@n:2").unwrap()], None, None)
            .unwrap();
        let v = s.status().unwrap().version;
        s.write_bytes(ann, &file(TREE2), v).unwrap();
        index_all(&s, ann);
        let mid = index_loads();
        for _ in 0..3 {
            s.roster(ann).unwrap();
        }
        s.claims(ann).unwrap();
        s.renew_claim(ann, None, None).unwrap();
        let after = index_loads();
        assert_eq!(
            after.1 - before.1,
            0,
            "no index is decoded under the session lock"
        );
        assert!(
            after.0 - mid.0 <= 1,
            "the claims' check decodes a version's index once, not per call ({})",
            after.0 - mid.0
        );
        assert_eq!(
            s.claims(ann).unwrap().len(),
            1,
            "carried to the new version"
        );
    });
}

#[test]
fn a_blob_pack_s_registry_adds_its_index_verbs_only_when_it_has_one() {
    let any = Registry::for_blob_pack(&AnyFile).unwrap();
    assert_eq!(
        any,
        Registry::for_tier(Tier::Blob).with_annotations(),
        "no index: exactly tier 0's verbs and the annotation verbs"
    );
    let r = Registry::for_blob_pack(&IxPack::default()).unwrap();
    for v in [
        "part list",
        "part show",
        "part diff",
        "part blame",
        "outline",
        "node",
        "claim",
        "claim --renew",
        "claim --release",
    ] {
        let spec = r.get(v).unwrap_or_else(|| panic!("{v} is registered"));
        assert!(spec.is_index() && spec.runs_through_run_tool(), "{v}");
        assert!(spec.applies_to(Tier::Blob), "{v}");
    }
    assert_eq!(r.get("outline").unwrap().alias_of(), Some("part list"));
    assert_eq!(r.get("node").unwrap().alias_of(), Some("part show"));
    assert_eq!(r.form_of(&argv(&["part", "show", "#1"])), "part show");
    assert_eq!(r.form_of(&argv(&["claim", "--renew"])), "claim --renew");
    assert!(
        Registry::for_tier(Tier::Blob).get("part list").is_none(),
        "for_tier is unchanged"
    );
    // names the kit would shadow, or that aren't two words, are refused
    struct P(&'static k2_mpc_base::index::IndexVerbs);
    impl k2_mpc_base::session::BlobPack for P {
        fn index_spec(&self) -> Option<k2_mpc_base::index::IndexSpec> {
            Some(k2_mpc_base::index::IndexSpec::new("p", 1, self.0))
        }
    }
    for bad in [
        k2_mpc_base::index::IndexVerbs::new(
            "node list",
            "node show",
            "node diff",
            "node blame",
            None,
        ),
        k2_mpc_base::index::IndexVerbs::new("list", "x show", "x diff", "x blame", None),
        k2_mpc_base::index::IndexVerbs::new("cat list", "cat show", "cat diff", "cat blame", None),
        k2_mpc_base::index::IndexVerbs::new("x list", "x list", "x diff", "x blame", None),
    ] {
        let verbs: &'static k2_mpc_base::index::IndexVerbs = Box::leak(Box::new(bad));
        assert!(Registry::for_blob_pack(&P(verbs)).is_err(), "{bad:?}");
    }
}

fn index_row(a: PersonAction) -> Option<ParityRow> {
    Some(match a {
        PersonAction::ReadAll => {
            ParityRow::live(a, &["cat", "part list", "part show", "outline", "node"])
        }
        PersonAction::History => ParityRow::live(a, &["log", "show --version", "part diff"]),
        PersonAction::Blame => ParityRow::live(a, &["blame", "part blame"]),
        PersonAction::Caret => ParityRow::live(a, &["claim", "claim --renew", "claim --release"]),
        PersonAction::Annotate => ParityRow::live(
            a,
            &[
                "comment",
                "reply",
                "resolve",
                "resolve --reopen",
                "comment --delete",
                "comments",
                "comments --log",
                "reanchor",
            ],
        ),
        PersonAction::Mention => ParityRow::live(a, &["comment", "reply"]),
        _ => return None,
    })
}

/// The kit's tier-0 list (`tests/parity_blob.rs`), with the index's rows.
fn rows() -> Vec<ParityRow> {
    use PersonAction as P;
    PersonAction::ALL
        .iter()
        .filter_map(|&a| {
            if let Some(r) = index_row(a) {
                return Some(r);
            }
            if !a.in_tier(Tier::Blob) {
                return None;
            }
            Some(match a {
                P::Open => ParityRow::live(a, &["open"]),
                P::Close => ParityRow::live(a, &["leave"]),
                P::ViewRange => ParityRow::deferred(a, &["view"], "later", "a view-region verb"),
                P::Who => ParityRow::live(a, &["who"]),
                P::Rewrite => ParityRow::live(a, &["write"]),
                P::Saved => ParityRow::live(a, &["status", "flush"]),
                P::Watch => ParityRow::deferred(a, &["watch"], "later", "no blob subscription"),
                P::Restore => ParityRow::live(a, &["restore", "version --name"]),
                P::Rename => ParityRow::live(a, &["mv"]),
                P::Export => ParityRow::live(a, &["export", "export --version"]),
                P::WhyOff => ParityRow::live(a, &["status"]),
                P::Purge => ParityRow::deferred(a, &["purge"], "later", "no purge verb yet"),
                P::Approve => ParityRow::live(a, &["approve", "decline"]),
                P::Pending => ParityRow::live(a, &["log --pending", "approvals"]),
                P::Lease => ParityRow::live(a, &["lease", "lease --renew"]),
                P::ReleaseLease => ParityRow::live(a, &["lease --release"]),
                P::BreakLease => ParityRow::live(a, &["lease --break"]),
                other => panic!("{other:?}: no row"),
            })
        })
        .collect()
}

#[test]
fn every_index_verb_does_what_the_typed_call_does() {
    m(|| {
        let pack: Arc<dyn k2_mpc_base::session::BlobPack> = Arc::new(IxPack::default());
        let reg = Registry::for_blob_pack(pack.as_ref()).unwrap();
        let first: &'static [u8] = Box::leak(file(TREE).into_boxed_slice());
        let second: &'static [u8] = Box::leak(file(TREE2).into_boxed_slice());
        let extra = parity::kit_index_cases(&VERBS, first, second);
        match parity::check_blob(pack.clone(), &rows(), &reg, &extra) {
            Ok(report) => {
                assert!(report
                    .matched
                    .iter()
                    .any(|(a, v, _)| *a == PersonAction::Caret && *v == "claim"));
                assert!(report.matched.len() >= extra.len());
            }
            Err(f) => panic!("{}", parity::render(&f)),
        }
        // the list is checked both ways: an index verb left out of the list is a failure
        let short: Vec<ParityRow> = rows()
            .into_iter()
            .map(|r| match r.action {
                PersonAction::Blame => ParityRow::live(PersonAction::Blame, &["blame"]),
                _ => r,
            })
            .collect();
        assert!(
            !parity::check_blob_list(&short, &reg, &extra).is_empty(),
            "an index verb missing from the list fails"
        );
    });
}
