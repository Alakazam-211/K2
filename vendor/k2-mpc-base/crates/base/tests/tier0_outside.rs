//! The structure reads across an outside save (`docs/SESSIONS.md`, "Across an outside save"):
//! another program's save of an indexed file, taken in by `file_changed` after an on-disk
//! write. A diff row whose key the save may have given to another thing says so; an anchor
//! across it is judged the node before against the node after; blame labels the rows that end
//! on it; a pack that says the writer reissues keys refuses the diff, one that proves it gets
//! plain answers. The pack is the index test pack ([`index_pack`]). Public API only.
//!
//! Each row names the plant (`MPC_T0V_MUTANT`) it catches; the plants part runs each alone.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

mod index_pack;
use index_pack::IxPack;

use k2_mpc_base::anchor::{AnchorDoubt, AnchorState};
use k2_mpc_base::annot::{NewAnchor, NewComment, ThreadId};
use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::errors::Value;
use k2_mpc_base::index::{Identity, NodeChangeKind};
use k2_mpc_base::session::{
    AuditEvent, BlobSession, Config, ConnId, Host, Participant, ParticipantKind, Rights,
    SavePairing, SessionManager, Stay,
};
use k2_mpc_base::store::{BlobReason, DocKey, DocStore, VersionId};

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
        "k2mb-t0out-{tag}-{}-{}",
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
    let mgr = SessionManager::new(store, host.clone(), c).with_blob_store(blobs);
    Fx { dir, ws, host, mgr }
}

fn key(p: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: p.into(),
    }
}

/// Index the current version.
fn index(s: &BlobSession) {
    if let Some(j) = s.index_job(None).unwrap() {
        j.run().unwrap();
    }
}

/// A file of `IDX` lines.
fn idx(lines: &[&str]) -> String {
    let mut s = "IDX\n".to_owned();
    for l in lines {
        s.push_str(l);
        s.push('\n');
    }
    s
}

/// Three nodes, each with content.
const A: &[&str] = &[
    "@n:1|layer|Walls|-|1|-|walls|-",
    "@n:2|line|Door|0|1|-|door|-",
    "@n:3|line|Window|0|1|-|window|-",
];

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
    fn join(&self, s: &BlobSession, pid: &str) -> ConnId {
        self.host
            .rights
            .lock()
            .unwrap()
            .insert(pid.into(), Rights::full().with_owner(true));
        s.join(
            Participant::new(pid, pid, "#123456", ParticipantKind::Person, "tab"),
            Stay::Held,
        )
        .unwrap()
        .conn
    }
    /// Another program writes `name` with `bytes`; the host's watcher reports it; the version
    /// it made (if any) is indexed.
    fn outside(&self, s: &BlobSession, name: &str, bytes: &str) {
        self.host.now.fetch_add(1_000, Ordering::SeqCst);
        fs::write(self.ws.join(name), bytes).unwrap();
        s.file_changed().unwrap();
        index(s);
    }
}

fn version(s: &BlobSession) -> VersionId {
    s.status().unwrap().version
}

/// A comment on node `k`, made now; its thread id.
fn anchor(s: &BlobSession, c: ConnId, k: &str) -> ThreadId {
    s.comment(c, NewComment::new(NewAnchor::node(k, None), "look here"))
        .unwrap()
        .id
}

/// The anchor's state now.
fn state(s: &BlobSession, c: ConnId, t: ThreadId) -> Option<AnchorState> {
    s.thread(c, t, None).unwrap().resolution.map(|r| r.state)
}

/// The state the doc's records hold for thread `t`, as the host heard it: the last
/// `AnchorState` event's, else the state it was made with (`made`).
fn stored(f: &Fx, t: ThreadId, made: Option<AnchorState>) -> Option<AnchorState> {
    f.host
        .events
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find_map(|e| match e {
            AuditEvent::AnchorState { thread, now, .. } if *thread == t => Some(*now),
            _ => None,
        })
        .or(made)
}

/// A kit write of `bytes` over the current version, indexed.
fn kit_write(s: &BlobSession, c: ConnId, bytes: &str) -> VersionId {
    let v = s
        .write_bytes(c, bytes.as_bytes(), version(s))
        .unwrap()
        .version;
    index(s);
    v
}

/// outside_no_change_save: the same bytes written again from outside: no version, so the diff
/// is empty, blame is unmoved and the anchor stays attached.
#[test]
fn outside_no_change_save() {
    let f = fx("same");
    let s = f.open("a.idx", &idx(A), IxPack::default());
    let c = f.join(&s, "ann");
    let v1 = version(&s);
    let t = anchor(&s, c, "@n:2");
    f.outside(&s, "a.idx", &idx(A));
    let now = version(&s);
    let d = s.index_diff(c, v1, now).unwrap();
    let bl = s.index_blame(c, None).unwrap();
    let moved = bl
        .rows
        .iter()
        .filter(|r| r.version != v1 || r.outside_save)
        .count();
    assert_eq!(
        (d.changes.len(), moved, state(&s, c, t)),
        (0, 0, Some(AnchorState::Attached)),
        "outside_no_change_save: the same bytes again change nothing (diff, blame, anchor)"
    );
}

/// outside_churn_save: every key reissued: the diff is all removed and added, `keys_kept` 0 of
/// 3, and the anchor is orphaned or candidate-only, never attached. Plant:
/// `keys-kept-uncounted`.
#[test]
fn outside_churn_save() {
    let f = fx("churn");
    let s = f.open("a.idx", &idx(A), IxPack::default());
    let c = f.join(&s, "ann");
    let v1 = version(&s);
    let t = anchor(&s, c, "@n:2");
    let churned: Vec<String> = A.iter().map(|l| l.replacen("@n:", "@n:1", 1)).collect();
    let churned: Vec<&str> = churned.iter().map(String::as_str).collect();
    f.outside(&s, "a.idx", &idx(&churned));
    let v2 = version(&s);
    let d = s.index_diff(c, v1, v2).unwrap();
    let all_out_in = d.changes.len() == 6
        && d.changes.iter().all(|ch| {
            ch.changes.contains(&NodeChangeKind::Added)
                || ch.changes.contains(&NodeChangeKind::Removed)
        });
    let kept: Vec<Option<(u32, u32)>> = d.saves.iter().map(|x| x.keys_kept).collect();
    let st = state(&s, c, t);
    let not_attached = matches!(
        st,
        Some(AnchorState::Orphaned | AnchorState::Suspect(AnchorDoubt::CandidateOnly))
    );
    assert_eq!(
        (all_out_in, kept, not_attached),
        (true, vec![Some((0, 3))], true),
        "outside_churn_save: every key reissued: all removed and added, keys_kept 0 of 3, the \
         anchor never attached ({st:?})"
    );
}

/// outside_reuse_save: two nodes swap keys (each renamed, with the other's content): both diff
/// rows carry the doubt, and the anchor on one is suspect. Plant: `reuse-reads-attached`.
#[test]
fn outside_reuse_save() {
    let f = fx("reuse");
    let s = f.open("a.idx", &idx(A), IxPack::default());
    let c = f.join(&s, "ann");
    let v1 = version(&s);
    let t = anchor(&s, c, "@n:2");
    let swapped = [
        "@n:1|layer|Walls|-|1|-|walls|-",
        "@n:2|line|Window|0|1|-|window|-",
        "@n:3|line|Door|0|1|-|door|-",
    ];
    f.outside(&s, "a.idx", &idx(&swapped));
    let v2 = version(&s);
    let d = s.index_diff(c, v1, v2).unwrap();
    let marks: Vec<(String, Option<Identity>)> = d
        .changes
        .iter()
        .map(|ch| (ch.key.as_str().to_owned(), ch.identity))
        .collect();
    let doubt = Some(Identity::Doubt {
        save: v2,
        judged: "identity_doubt",
    });
    assert_eq!(
        (marks, state(&s, c, t)),
        (
            vec![("@n:2".to_owned(), doubt), ("@n:3".to_owned(), doubt)],
            Some(AnchorState::Suspect(AnchorDoubt::IdentityDoubt))
        ),
        "outside_reuse_save: a reused key reads in doubt (both diff rows, the anchor)"
    );
}

/// outside_kit_write_between: an outside save, a kit write, another outside save: blame
/// credits the kit write for the node it made, and labels the rows that end on an outside save.
/// Plant: `blame-label-missing`.
#[test]
fn outside_kit_write_between() {
    let f = fx("between");
    let s = f.open("a.idx", &idx(A), IxPack::default());
    let c = f.join(&s, "ann");
    let first = [A[0], "@n:2|line|Door|0|1|-|door-2|-", A[2]];
    f.outside(&s, "a.idx", &idx(&first));
    let v2 = version(&s);
    let mine = [first[0], first[1], "@n:3|line|Window|0|1|-|window-3|-"];
    let v3 = kit_write(&s, c, &idx(&mine));
    let last = ["@n:1|layer|Walls|-|1|-|walls-4|-", mine[1], mine[2]];
    f.outside(&s, "a.idx", &idx(&last));
    let v4 = version(&s);
    let bl = s.index_blame(c, None).unwrap();
    let rows: Vec<(String, VersionId, bool, String)> = bl
        .rows
        .iter()
        .map(|r| {
            (
                r.key.as_str().to_owned(),
                r.version,
                r.outside_save,
                r.author.clone(),
            )
        })
        .collect();
    let reasons: Vec<BlobReason> = s
        .versions(c)
        .unwrap()
        .into_iter()
        .map(|(_, v)| v.reason)
        .collect();
    assert_eq!(
        reasons[1..].to_vec(),
        vec![
            BlobReason::External,
            BlobReason::Write,
            BlobReason::External
        ],
        "outside_kit_write_between: two outside saves around a kit write"
    );
    let row =
        |k: &str, v: VersionId, label: bool, who: &str| (k.to_owned(), v, label, who.to_owned());
    assert_eq!(
        rows,
        vec![
            row("@n:1", v4, true, "kit:external"),
            row("@n:2", v2, true, "kit:external"),
            row("@n:3", v3, false, "ann"),
        ],
        "outside_kit_write_between: blame credits the kit write, and labels the rows that end \
         on an outside save"
    );
}

/// outside_two_honest_edits: a rename by one kit write, then a content edit by another, then
/// an outside save that leaves the nodes as they are: no doubt anywhere, because the save is
/// judged the node just before it against the node in it, never the fingerprint the anchor
/// was made with. Plant: `judge-against-fingerprint`.
#[test]
fn outside_two_honest_edits() {
    let f = fx("honest");
    let s = f.open("a.idx", &idx(A), IxPack::default());
    let c = f.join(&s, "ann");
    let v1 = version(&s);
    let t = anchor(&s, c, "@n:2");
    let renamed = [A[0], "@n:2|line|Front door|0|1|-|door|-", A[2]];
    kit_write(&s, c, &idx(&renamed));
    let edited = [A[0], "@n:2|line|Front door|0|1|-|door-oak|-", A[2]];
    let mut saved = idx(&edited);
    kit_write(&s, c, &saved);
    // the app saves again, its own stamp added: the nodes are as they were
    saved.push_str("#attr saved_by=app\n");
    f.outside(&s, "a.idx", &saved);
    let v4 = version(&s);
    let d = s.index_diff(c, v1, v4).unwrap();
    let marks: Vec<Option<Identity>> = d.changes.iter().map(|ch| ch.identity).collect();
    assert_eq!(
        (d.saves.len(), marks, state(&s, c, t)),
        (1, vec![None], Some(AnchorState::Attached)),
        "outside_two_honest_edits: two honest kit edits and a save that changes no node read no \
         doubt (the diff row, the anchor)"
    );
}

/// outside_reissues_refuses: a pack that says the writer reissues keys: a diff across the save
/// refuses `MPC-E722`, naming the save and the widest diff that doesn't cross it, and an anchor
/// across it is not attached. Plant: `reissues-answered`.
#[test]
fn outside_reissues_refuses() {
    let f = fx("reissues");
    let pack = IxPack {
        pairing: Some(SavePairing::Reissues),
        ..IxPack::default()
    };
    let s = f.open("a.idx", &idx(A), pack);
    let c = f.join(&s, "ann");
    let v1 = version(&s);
    let t = anchor(&s, c, "@n:3");
    let changed = [A[0], "@n:2|line|Door|0|1|-|door-2|-", A[2]];
    f.outside(&s, "a.idx", &idx(&changed));
    let v2 = version(&s);
    let refusal = s.index_diff(c, v1, v2).err().map(|e| {
        let r = e.report();
        let d = |k: &str| r.detail(k).and_then(Value::as_u64);
        (r.id().to_owned(), d("save"), d("safe_from"), d("to"))
    });
    assert_eq!(
        (refusal, state(&s, c, t)),
        (
            Some(("MPC-E722".to_owned(), Some(v2), Some(v2), Some(v2))),
            Some(AnchorState::Suspect(AnchorDoubt::IdentityDoubt))
        ),
        "outside_reissues_refuses: a diff across a save that reissues keys refuses E722 (the \
         widest safe diff named), and the anchor isn't attached"
    );
}

/// outside_proven_plain: a pack that proves the writer keeps keys: the same swap as
/// `outside_reuse_save` reads plainly (no marks, the anchor attached by the pack's own judge).
#[test]
fn outside_proven_plain() {
    let f = fx("proven");
    let pack = IxPack {
        pairing: Some(SavePairing::Proven),
        ..IxPack::default()
    };
    let s = f.open("a.idx", &idx(A), pack);
    let c = f.join(&s, "ann");
    let v1 = version(&s);
    let t = anchor(&s, c, "@n:2");
    let swapped = [
        "@n:1|layer|Walls|-|1|-|walls|-",
        "@n:2|line|Window|0|1|-|window|-",
        "@n:3|line|Door|0|1|-|door|-",
    ];
    f.outside(&s, "a.idx", &idx(&swapped));
    let v2 = version(&s);
    let d = s.index_diff(c, v1, v2).unwrap();
    let marks: Vec<Option<Identity>> = d.changes.iter().map(|ch| ch.identity).collect();
    let pairing: Vec<&str> = d.saves.iter().map(|x| x.pairing).collect();
    assert_eq!(
        (marks, pairing, state(&s, c, t)),
        (
            vec![None, None],
            vec!["proven"],
            Some(AnchorState::Attached)
        ),
        "outside_proven_plain: a proven writer's save reads plainly"
    );
}

/// outside_stored_state_matches_read: the state an index job records for an anchor (and tells
/// the host) is judged across an outside save exactly as a read judges it, so the two never
/// disagree: after a reuse save both say suspect; after a save of the same bytes, and after
/// two honest kit edits and a save that changes no node, both stay attached. Plant:
/// `stored-judge-by-fingerprint`.
#[test]
fn outside_stored_state_matches_read() {
    let swapped = [
        "@n:1|layer|Walls|-|1|-|walls|-",
        "@n:2|line|Window|0|1|-|window|-",
        "@n:3|line|Door|0|1|-|door|-",
    ];
    // a reuse save
    let f = fx("stored-reuse");
    let s = f.open("a.idx", &idx(A), IxPack::default());
    let c = f.join(&s, "ann");
    let t = anchor(&s, c, "@n:2");
    let made = state(&s, c, t);
    f.outside(&s, "a.idx", &idx(&swapped));
    let reuse = (stored(&f, t, made), state(&s, c, t));
    // the same bytes again
    let g = fx("stored-same");
    let s2 = g.open("a.idx", &idx(A), IxPack::default());
    let c2 = g.join(&s2, "ann");
    let t2 = anchor(&s2, c2, "@n:2");
    let made2 = state(&s2, c2, t2);
    g.outside(&s2, "a.idx", &idx(A));
    let same = (stored(&g, t2, made2), state(&s2, c2, t2));
    // two honest kit edits, then a save that changes no node
    let h = fx("stored-honest");
    let s3 = h.open("a.idx", &idx(A), IxPack::default());
    let c3 = h.join(&s3, "ann");
    let t3 = anchor(&s3, c3, "@n:2");
    let made3 = state(&s3, c3, t3);
    kit_write(
        &s3,
        c3,
        &idx(&[A[0], "@n:2|line|Front door|0|1|-|door|-", A[2]]),
    );
    let mut saved = idx(&[A[0], "@n:2|line|Front door|0|1|-|door-oak|-", A[2]]);
    kit_write(&s3, c3, &saved);
    saved.push_str("#attr saved_by=app\n");
    h.outside(&s3, "a.idx", &saved);
    let honest = (stored(&h, t3, made3), state(&s3, c3, t3));
    let suspect = Some(AnchorState::Suspect(AnchorDoubt::IdentityDoubt));
    let attached = Some(AnchorState::Attached);
    assert_eq!(
        (reuse, same, honest),
        (
            (suspect, suspect),
            (attached, attached),
            (attached, attached)
        ),
        "outside_stored_state_matches_read: the recorded state (and its event) is the read's \
         across an outside save (a reused key reads in doubt; the same bytes, and two honest \
         kit edits and a save that changes no node, read no doubt)"
    );
}

/// outside_minted_keys_stored_matches_read: a pack whose nodes carry no keys of their own (the
/// kit mints them), and an outside save after a version whose keys were never recorded (its
/// bytes were indexed for another doc, so no job ran for it and nothing read it): the state
/// the save's index job records mints that version's keys exactly as a read does, so the
/// recorded state, its event and a read agree. The pack doubts a node whose content changed
/// across a save; a save of the same bytes changes nothing. Plant: `stored-skips-unminted`.
#[test]
fn outside_minted_keys_stored_matches_read() {
    let pack = || IxPack {
        content_doubt: true,
        ..IxPack::default()
    };
    let x = idx(&["-|line|Door|-|1|-|door|-", "-|line|Window|-|1|-|window|-"]);
    let mut y = x.clone();
    y.push_str("#attr v=2\n");
    let mut z = idx(&["-|line|Door|-|1|-|door-x|-", "-|line|Window|-|1|-|window|-"]);
    z.push_str("#attr v=2\n");
    let door = |s: &BlobSession| {
        let view = s.index_view(None).unwrap();
        let n = view.nodes.iter().find(|n| n.name == "Door").unwrap();
        n.key.as_str().to_owned()
    };
    // run: `y` indexed for another doc first, then this doc's kit write of `y` (no job, no
    // keys), then the outside save `last`
    let run = |tag: &str, last: &str| {
        let f = fx(tag);
        f.open("b.idx", &y, pack());
        let s = f.open("a.idx", &x, pack());
        let c = f.join(&s, "ann");
        let t = anchor(&s, c, &door(&s));
        let made = state(&s, c, t);
        kit_write(&s, c, &y);
        f.outside(&s, "a.idx", last);
        (stored(&f, t, made), state(&s, c, t))
    };
    let changed = run("minted-changed", &z);
    let same = run("minted-same", &y);
    let suspect = Some(AnchorState::Suspect(AnchorDoubt::IdentityDoubt));
    let attached = Some(AnchorState::Attached);
    assert_eq!(
        (changed, same),
        ((suspect, suspect), (attached, attached)),
        "outside_minted_keys_stored_matches_read: across a version whose keys were never \
         recorded, the recorded state (and its event) is the read's (a reused key reads in \
         doubt: suspect; a save of the same bytes: attached)"
    );
}
