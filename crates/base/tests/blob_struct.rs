//! The structure index (slice A1): the job a host runs, the row and overlay in the store, the
//! caps, keyed hashes, the worker's wire, and everything a purge must reach. The file-type side
//! is a stand-in ([`index_pack`]); the kit knows no format.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

mod index_pack;
use index_pack::{file, IxPack, VERBS};

use k2_mpc_base::blob::{BlobConfig, BlobError, BlobStore};
use k2_mpc_base::differential::under_env_index_mutant;
use k2_mpc_base::errors::Refusal;
use k2_mpc_base::index::{
    index_to_wire, Digest, IndexDraft, IndexJobError, IndexLimits, IndexOutcome, IndexRefused,
    IndexSpec, IndexStatus, IndexView, KeyClass, NotIndexedWhy, INDEX_DEPTH_MAX, INDEX_TEXT_MAX,
};
use k2_mpc_base::session::{
    AgentWrites, AnyFile, AuditEvent, BlobPack, BlobSession, Config, ConnId, Host, NotReached,
    Participant, ParticipantKind, Rights, SessionError, SessionManager, Stay,
};
use k2_mpc_base::store::{DocKey, DocStore};

/// Run `f`, with the mutant `MPC_INDEX_MUTANT` names planted on this thread (unset: as is).
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
    store: Arc<DocStore>,
    blobs: Arc<BlobStore>,
    mgr: SessionManager,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn fx_with(tag: &str, tweak: impl FnOnce(&mut Config)) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-struct-{tag}-{}-{}",
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
    let mut bc = BlobConfig::default();
    bc.gc_grace_ms = 0;
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
    let mgr = SessionManager::new(store.clone(), host.clone(), c).with_blob_store(blobs.clone());
    Fx {
        dir,
        host,
        store,
        blobs,
        mgr,
    }
}

fn fx(tag: &str) -> Fx {
    fx_with(tag, |_| {})
}

fn key(space: &str, p: &str) -> DocKey {
    DocKey {
        space: space.into(),
        path: p.into(),
    }
}

fn owner(pid: &str) -> Participant {
    Participant::new(pid, pid, "#123456", ParticipantKind::Person, "tab")
}

impl Fx {
    fn open_with(&self, pack: IxPack, space: &str, path: &str, bytes: &[u8]) -> Arc<BlobSession> {
        self.mgr
            .open_blob(Arc::new(pack), &key(space, path), Some(&mut &bytes[..]))
            .unwrap()
    }
    fn open(&self, path: &str, bytes: &[u8]) -> Arc<BlobSession> {
        self.open_with(IxPack::default(), "ws", path, bytes)
    }
    fn join(&self, s: &BlobSession, pid: &str, r: Rights) -> ConnId {
        self.host.rights.lock().unwrap().insert(pid.into(), r);
        s.join(owner(pid), Stay::Held).unwrap().conn
    }
    fn events(&self) -> Vec<AuditEvent> {
        self.host.events.lock().unwrap().clone()
    }
    /// Does the store file hold any of `needles`? Scanned while the store is quiet.
    fn holds(&self, needles: &[&[u8]]) -> bool {
        !self.store.scan_for(needles).unwrap().is_empty()
    }
}

fn full_owner() -> Rights {
    Rights::full().with_owner(true)
}

fn wanted(ev: &[AuditEvent]) -> usize {
    ev.iter()
        .filter(|e| matches!(e, AuditEvent::IndexWanted { .. }))
        .count()
}

/// Run the job for the current version to completion.
fn index(s: &BlobSession) -> IndexStatus {
    s.index_job(None)
        .unwrap()
        .expect("a job is due")
        .run()
        .expect("the job records")
}

const TREE: &[&str] = &[
    "@n:1|group|Layers|-|1|-|-",
    "@n:2|text|Title|0|1|Hello marker-TEXT|-|marker-NOTE",
    "@n:3|raster|Photo|0|0|-|pixels-of-photo|-",
];

#[test]
fn a_committed_version_is_indexed_by_a_job_and_reads_back() {
    let f = fx("job");
    let bytes = file(TREE);
    let s = f.open("a.idx", &bytes);
    assert_eq!(wanted(&f.events()), 1, "the import wants an index");
    assert_eq!(
        s.index_status(None).unwrap(),
        IndexStatus::Pending {
            version: 1,
            running: false
        }
    );
    // a verb on an unindexed version doesn't block: it says pending
    assert!(matches!(
        s.index_view(None),
        Err(SessionError::IndexPending { version: 1, .. })
    ));
    let job = s.index_job(None).unwrap().expect("a job");
    assert_eq!(job.version(), 1);
    assert_eq!(job.size(), bytes.len() as u64);
    // only one job out for these bytes at a time
    assert!(s.index_job(None).unwrap().is_none(), "the slot is taken");
    assert_eq!(
        s.index_status(None).unwrap(),
        IndexStatus::Pending {
            version: 1,
            running: true
        }
    );
    drop(job);
    assert!(
        s.index_job(None).unwrap().is_some(),
        "a dropped job frees its slot"
    );
    let st = index(&s);
    assert_eq!(
        st,
        IndexStatus::Ready {
            version: 1,
            nodes: 3
        }
    );
    assert_eq!(s.index_status(None).unwrap(), st);
    assert!(s.index_job(None).unwrap().is_none(), "the row exists");
    assert!(f.events().iter().any(|e| matches!(
        e,
        AuditEvent::IndexReady {
            version: 1,
            nodes: 3,
            ..
        }
    )));

    let v: IndexView = s.index_view(None).unwrap();
    assert_eq!(v.version, 1);
    assert_eq!(v.source, "fmt-1");
    let by_key: Vec<&str> = v.nodes.iter().map(|n| n.key.as_str()).collect();
    assert_eq!(by_key, ["@n:1", "@n:2", "@n:3"]);
    assert!(v.nodes.iter().all(|n| n.key_class == KeyClass::Native));
    assert_eq!(v.nodes[0].parent, None);
    assert_eq!(v.nodes[0].depth, 0);
    assert_eq!(v.nodes[1].parent.as_ref().unwrap().as_str(), "@n:1");
    assert_eq!(v.nodes[1].depth, 1);
    assert_eq!(v.nodes[1].order, 0);
    assert_eq!(v.nodes[2].order, 1, "the second child of the group");
    assert_eq!(v.nodes[1].text.as_deref(), Some("Hello marker-TEXT"));
    assert!(v.nodes[1].visible_in_tree);
    assert!(!v.nodes[2].visible, "its own flag");
    assert!(!v.nodes[2].visible_in_tree, "and so through the tree");
    assert_eq!(v.nodes[1].fingerprint.weak["placed"], "marker-NOTE");
    assert_eq!(v.nodes[1].attrs["note"], "marker-NOTE");
    assert!(v.nodes[2].content_hash.is_some());
    assert!(v.nodes[1].content_hash.is_none());
}

#[test]
fn a_host_that_has_not_adopted_the_index_sees_nothing_change() {
    let f = fx("adopt");
    let bytes = file(TREE);
    let s = f
        .mgr
        .open_blob(
            Arc::new(AnyFile),
            &key("ws", "a.bin"),
            Some(&mut &bytes[..]),
        )
        .unwrap();
    let c = f.join(&s, "alice", full_owner());
    let v2 = s
        .write_bytes(c, b"other bytes", s.status().unwrap().version)
        .unwrap();
    s.write_bytes(c, b"third", v2.version).unwrap();
    s.purge(c, Some(1)).unwrap();
    assert_eq!(s.index_status(None).unwrap(), IndexStatus::NoIndexer);
    assert!(s.index_job(None).unwrap().is_none());
    assert_eq!(
        s.index_view(None).unwrap_err(),
        SessionError::IndexUnsupported
    );
    assert_eq!(wanted(&f.events()), 0, "no event without an indexer");
    // the additive tables are created lazily: a purge's rewrite didn't make them either
    assert!(
        !f.holds(&[b"blob_struct"]),
        "a store with no index keeps no such table"
    );
}

#[test]
fn a_version_past_the_size_cap_gets_no_job() {
    let f = fx_with("size", |c| c.blob_index_max_bytes = 5);
    let s = f.open("a.idx", &file(TREE));
    assert_eq!(wanted(&f.events()), 0, "no event past the cap");
    assert!(matches!(
        s.index_status(None).unwrap(),
        IndexStatus::NotIndexed {
            version: 1,
            why: NotIndexedWhy::TooLarge { .. }
        }
    ));
    assert!(s.index_job(None).unwrap().is_none());
    assert!(matches!(
        s.index_view(None).unwrap_err(),
        SessionError::NotIndexed {
            why: NotIndexedWhy::TooLarge { .. },
            ..
        }
    ));
}

/// A node cap, and a pack that ignores it: the kit counts, and records the refusal.
#[test]
fn past_the_node_cap_is_recorded_as_not_indexed() {
    m(|| {
        let f = fx_with("nodes", |c| c.blob_index_max_nodes = 2);
        let s = f.open("a.idx", &file(TREE));
        let st = index(&s);
        assert!(
            matches!(
                st,
                IndexStatus::NotIndexed {
                    version: 1,
                    why: NotIndexedWhy::TooManyNodes { count: 3, limit: 2 }
                }
            ),
            "the node cap holds: {st:?}"
        );
        assert!(
            s.index_job(None).unwrap().is_none(),
            "the refusal is stored"
        );
        assert!(f
            .events()
            .iter()
            .any(|e| matches!(e, AuditEvent::IndexNotIndexed { .. })));
        // tier 0 is untouched
        let c = f.join(&s, "alice", Rights::editor());
        let mut out = Vec::new();
        std::io::Read::read_to_end(&mut s.read(c).unwrap().reader, &mut out).unwrap();
        assert_eq!(out, file(TREE));
    });
}

/// A chain deeper than the cap: refused, whatever the pack thought.
#[test]
fn past_the_depth_cap_is_recorded_as_not_indexed() {
    m(|| {
        let f = fx("deep");
        let mut lines = vec!["@n:0|group|g|-|1|-|-".to_owned()];
        for i in 1..(usize::from(INDEX_DEPTH_MAX) + 5) {
            lines.push(format!("@n:{i}|group|g{i}|{}|1|-|-", i - 1));
        }
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let s = f.open("a.idx", &file(&refs));
        let st = index(&s);
        assert!(
            matches!(
                st,
                IndexStatus::NotIndexed {
                    why: NotIndexedWhy::TooDeep { .. },
                    ..
                }
            ),
            "the depth cap holds: {st:?}"
        );
    });
}

/// A row over the stored cap is not kept: only a refusal row.
#[test]
fn a_row_past_the_stored_cap_is_recorded_as_not_indexed() {
    m(|| {
        let f = fx_with("stored", |c| c.blob_index_max_stored_bytes = 200);
        let s = f.open("a.idx", &file(TREE));
        let st = index(&s);
        assert!(
            matches!(
                st,
                IndexStatus::NotIndexed {
                    why: NotIndexedWhy::IndexTooBig { limit: 200, .. },
                    ..
                }
            ),
            "the stored cap holds: {st:?}"
        );
        assert!(
            !f.holds(&[b"marker-NOTE"]),
            "a row past the cap stored no node"
        );
    });
}

/// The hashes stored are keyed: unequal across spaces, equal within one, and no raw digest in
/// the store file.
#[test]
fn hashes_are_keyed_by_space_and_no_raw_digest_is_stored() {
    m(|| {
        let f = fx("keyed");
        let bytes = file(TREE);
        let a = f.open_with(IxPack::default(), "space-a", "x.idx", &bytes);
        let b = f.open_with(IxPack::default(), "space-b", "x.idx", &bytes);
        index(&a);
        index(&b);
        let va = a.index_view(None).unwrap();
        let vb = b.index_view(None).unwrap();
        // the fixture digests the line; compare a stored hash with the raw SHA-256 of it
        let raw = Digest::of(TREE[0].as_bytes());
        assert_ne!(
            va.nodes[0].record_hash.as_bytes(),
            raw.as_bytes(),
            "a stored hash is not the raw digest"
        );
        assert_ne!(
            va.nodes[0].record_hash, vb.nodes[0].record_hash,
            "equal content in two spaces is not comparable"
        );
        // the same bytes in the same space give one row
        let c = f.open_with(IxPack::default(), "space-a", "y.idx", &bytes);
        assert!(c.index_job(None).unwrap().is_none(), "the row is shared");
        assert_eq!(
            c.index_view(None).unwrap().nodes[0].record_hash,
            va.nodes[0].record_hash
        );
        let raw2 = Digest::of(b"pixels-of-photo");
        assert!(
            !f.holds(&[raw.as_bytes().as_slice(), raw2.as_bytes().as_slice()]),
            "no raw digest is stored"
        );
    });
}

/// A worker's path: the stream and limits out, the result back, the same index as in-process.
#[test]
fn a_worker_result_commits_like_an_in_process_run() {
    let f = fx("wire");
    let bytes = file(TREE);
    let s = f.open("a.idx", &bytes);
    let mut job = s.index_job(None).unwrap().unwrap();
    let (mut input, limits) = job.open().unwrap();
    assert_eq!(limits, job.limits());
    let wire = index_to_wire(&IxPack::default(), &mut input, bytes.len() as u64, &limits).unwrap();
    assert_eq!(
        job.commit_wire(input, &wire).unwrap(),
        IndexStatus::Ready {
            version: 1,
            nodes: 3
        }
    );
    assert_eq!(s.index_view(None).unwrap().nodes.len(), 3);
}

/// The worker's refusal is recorded; its failure to read is not.
#[test]
fn a_worker_refusal_is_recorded_and_a_stream_failure_is_not() {
    let f = fx("refuse");
    let s = f.open("a.idx", b"not an IDX file");
    let mut job = s.index_job(None).unwrap().unwrap();
    let (mut input, limits) = job.open().unwrap();
    let wire = index_to_wire(&IxPack::default(), &mut input, 15, &limits).unwrap();
    let st = job.commit_wire(input, &wire).unwrap();
    assert!(matches!(
        st,
        IndexStatus::NotIndexed {
            why: NotIndexedWhy::Unreadable { .. },
            ..
        }
    ));

    // to_wire of a stream failure is an error on the worker's side, never a row
    let out = IndexOutcome::Refused(IndexRefused::Read(std::io::Error::other("broken pipe")))
        .to_wire(&IndexLimits::new(10, 64, 1 << 20, INDEX_TEXT_MAX));
    assert!(matches!(out, Err(IndexJobError::BadResult(_))));
}

/// Bad results from the worker are refused and record nothing; the job can run again.
#[test]
fn a_malformed_worker_result_is_refused_and_records_nothing() {
    let f = fx("bad");
    let bytes = file(TREE);
    let s = f.open("a.idx", &bytes);
    let good = {
        let mut job = s.index_job(None).unwrap().unwrap();
        let (mut input, limits) = job.open().unwrap();
        index_to_wire(&IxPack::default(), &mut input, bytes.len() as u64, &limits).unwrap()
    };
    let mut bad: Vec<Vec<u8>> = vec![
        Vec::new(),
        b"garbage".to_vec(),
        good[..good.len() - 1].to_vec(),
    ];
    let mut with_extra = good.clone();
    with_extra.push(0);
    bad.push(with_extra);
    let mut wrong_magic = good.clone();
    wrong_magic[0] = b'X';
    bad.push(wrong_magic);
    let mut wrong_version = good.clone();
    wrong_version[4] = 99;
    bad.push(wrong_version);
    for wire in bad {
        let mut job = s.index_job(None).unwrap().unwrap();
        let (input, _) = job.open().unwrap();
        let e = job.commit_wire(input, &wire).unwrap_err();
        assert!(matches!(e, IndexJobError::BadResult(_)), "{e:?}");
        assert_eq!(
            e.report().id(),
            "MPC-E714",
            "the malformed result is refused with its own id"
        );
        assert!(
            matches!(s.index_status(None).unwrap(), IndexStatus::Pending { .. }),
            "nothing recorded"
        );
    }
    assert!(
        s.index_job(None).unwrap().is_some(),
        "the job can run again"
    );
}

/// A result over the wire's cap is refused unread.
#[test]
fn a_result_over_the_wire_cap_is_refused() {
    let f = fx_with("wiremax", |c| c.blob_index_max_stored_bytes = 10);
    let s = f.open("a.idx", &file(TREE));
    let mut job = s.index_job(None).unwrap().unwrap();
    let (input, limits) = job.open().unwrap();
    let huge = vec![0u8; limits.wire_max() + 1];
    let e = job.commit_wire(input, &huge).unwrap_err();
    assert!(matches!(e, IndexJobError::BadResult(_)), "{e:?}");
}

/// A node whose parent is not an earlier node, in-process: refused, nothing recorded.
#[test]
fn a_draft_with_a_forward_parent_is_refused() {
    struct Bad;
    impl BlobPack for Bad {
        fn index_spec(&self) -> Option<IndexSpec> {
            Some(IndexSpec::new("bad", 1, &VERBS))
        }
        fn index(
            &self,
            _src: &mut dyn std::io::Read,
            _size: u64,
            _limits: &IndexLimits,
        ) -> Result<IndexDraft, IndexRefused> {
            let mut d = IndexDraft::new("f");
            let mut n = k2_mpc_base::index::DraftNode::new("kind", "x", Digest::of(b"x"));
            n.parent = Some(5);
            d.nodes.push(n);
            Ok(d)
        }
    }
    let f = fx("forward");
    let s = f
        .mgr
        .open_blob(Arc::new(Bad), &key("ws", "a.idx"), Some(&mut &b"x"[..]))
        .unwrap();
    let e = s.index_job(None).unwrap().unwrap().run().unwrap_err();
    assert!(matches!(e, IndexJobError::BadResult(_)), "{e:?}");
    assert!(matches!(
        s.index_status(None).unwrap(),
        IndexStatus::Pending { .. }
    ));
}

/// `stale commit refused`: a job out while its version is purged can't put the row back.
#[test]
fn a_job_commit_after_a_purge_is_refused_and_records_no_row() {
    m(|| {
        let f = fx("stale");
        let bytes = file(TREE);
        let s = f.open("a.idx", &bytes);
        let c = f.join(&s, "alice", full_owner());
        // a job for version 1 has its stream and result while the version is replaced and
        // purged (the stream is open, so it reads to its end)
        let mut job = s.index_job(None).unwrap().unwrap();
        let (mut input, limits) = job.open().unwrap();
        let wire =
            index_to_wire(&IxPack::default(), &mut input, bytes.len() as u64, &limits).unwrap();
        let v = s.status().unwrap().version;
        s.write_bytes(c, &file(&["@n:9|group|Other|-|1|-|-"]), v)
            .unwrap();
        s.purge(c, Some(v)).unwrap();
        let out = job.commit_wire(input, &wire);
        assert!(
            matches!(out, Err(IndexJobError::Stale { version: 1, .. })),
            "a stale job commit is refused: {out:?}"
        );
        assert_eq!(out.unwrap_err().report().id(), "MPC-E717");
        assert!(
            !f.holds(&[b"marker-NOTE"]),
            "the refused job left no index bytes"
        );
    });
}

/// A blob read past the job: the commit also re-checks the blob.
#[test]
fn a_job_for_a_collected_blob_is_refused() {
    m(|| {
        let f = fx("collected");
        let bytes = file(TREE);
        let s = f.open("a.idx", &bytes);
        let mut job = s.index_job(None).unwrap().unwrap();
        let (mut input, limits) = job.open().unwrap();
        let wire =
            index_to_wire(&IxPack::default(), &mut input, bytes.len() as u64, &limits).unwrap();
        f.mgr.delete_doc(&key("ws", "a.idx")).unwrap();
        f.blobs.gc().unwrap();
        let e = job.commit_wire(input, &wire).unwrap_err();
        assert!(
            matches!(e, IndexJobError::Stale { .. }),
            "a job for a deleted doc records nothing: {e:?}"
        );
    });
}

/// The byte scan: after the purge no node text, name, attribute or hash is in the store file.
#[test]
fn a_purged_version_leaves_no_index_byte_in_the_store_file() {
    m(|| {
        let f = fx("purge");
        let bytes = file(TREE);
        let s = f.open("a.idx", &bytes);
        let c = f.join(&s, "alice", full_owner());
        index(&s);
        assert!(f.holds(&[b"marker-TEXT"]), "the row is stored");
        let before = s.index_view(None).unwrap();
        let v = s.status().unwrap().version;
        s.write_bytes(c, &file(&["@n:9|group|Other|-|1|-|-"]), v)
            .unwrap();
        index(&s);
        let out = s.purge(c, Some(v)).unwrap();
        assert!(out.rewrite.is_some(), "the store was rewritten");
        assert!(matches!(
            s.index_status(Some(v)).unwrap(),
            IndexStatus::NotIndexed {
                why: NotIndexedWhy::Purged,
                ..
            }
        ));
        let record = before.nodes[0].record_hash;
        let found = f
            .store
            .scan_for(&[
                b"marker-TEXT".as_slice(),
                b"marker-NOTE".as_slice(),
                b"Hello marker".as_slice(),
                b"Title".as_slice(),
                record.as_bytes().as_slice(),
            ])
            .unwrap();
        assert!(
            found.is_empty(),
            "after a purge no index byte survives: {found:?}"
        );
        // the current version's row is kept by the same rewrite
        assert!(s.index_view(None).unwrap().nodes.len() == 1);
    });
}

/// The blob is removed by GC: its row goes in the same transaction.
#[test]
fn collecting_a_blob_removes_its_row() {
    m(|| {
        let f = fx("gc");
        let bytes = file(TREE);
        let s = f.open("a.idx", &bytes);
        index(&s);
        drop(s);
        f.mgr.delete_doc(&key("ws", "a.idx")).unwrap();
        // the blob is unreferenced; gc (grace 0) removes its index row, and with it the rows
        f.host.now.fetch_add(1, Ordering::SeqCst);
        f.blobs.gc().unwrap();
        // a new doc with the same bytes finds no row
        let t = f.open("b.idx", &bytes);
        assert!(
            matches!(t.index_status(None).unwrap(), IndexStatus::Pending { .. }),
            "the row went with its blob"
        );
    });
}

/// `purge_space_key` removes the space's rows and rewrites the file.
#[test]
fn purging_a_space_removes_its_rows_and_rewrites_the_store() {
    m(|| {
        let f = fx("space");
        let bytes = file(TREE);
        let s = f.open("a.idx", &bytes);
        index(&s);
        assert!(f.holds(&[b"marker-TEXT"]));
        let rep = f.blobs.purge_space_key("ws").unwrap();
        assert_eq!(rep.index_rows_removed, 1, "the space's rows were removed");
        assert!(rep.store_rewritten, "and the store file rewritten");
        assert!(
            !f.holds(&[b"marker-TEXT"]),
            "no index byte survives a space purge"
        );
    });
}

/// A purged version whose blob is still shared keeps the row: the bytes are still held.
#[test]
fn a_purged_version_of_a_shared_blob_keeps_its_row() {
    m(|| {
        let f = fx("shared");
        let bytes = file(TREE);
        let s = f.open("a.idx", &bytes);
        let other = f.open("b.idx", &bytes);
        let c = f.join(&s, "alice", full_owner());
        index(&s);
        let v = s.status().unwrap().version;
        s.write_bytes(c, &file(&["@n:9|group|Other|-|1|-|-"]), v)
            .unwrap();
        let out = s.purge(c, Some(v)).unwrap();
        assert!(out.not_reached.contains(&NotReached::SharedBlobs(1)));
        // the other doc still holds these bytes, and its index
        let st = other.index_status(None).unwrap();
        assert!(
            matches!(st, IndexStatus::Ready { nodes: 3, .. }),
            "a shared blob keeps its row: {st:?}"
        );
        assert_eq!(other.index_view(None).unwrap().nodes.len(), 3);
    });
}

/// A damaged blob stores no row, even for a pack that stopped before the damage.
#[test]
fn a_damaged_blob_records_no_row() {
    m(|| {
        let f = fx("damaged");
        let bytes = file(TREE);
        let pack = IxPack {
            stop_after: Some(8),
            ..IxPack::default()
        };
        let s = f.open_with(pack, "ws", "a.idx", &bytes);
        // a damaged tail, past where the pack stops reading
        let path = blob_file(&f);
        let mut b = fs::read(&path).unwrap();
        let last = b.len() - 1;
        b[last] ^= 1;
        fs::write(&path, &b).unwrap();
        let out = s.index_job(None).unwrap().unwrap().run();
        assert!(
            matches!(out, Err(IndexJobError::Blob(BlobError::Damaged(_)))),
            "a damaged stream records nothing: {out:?}"
        );
        assert_eq!(out.unwrap_err().report().id(), "MPC-E705");
        assert!(
            matches!(s.index_status(None).unwrap(), IndexStatus::Pending { .. }),
            "no row was stored"
        );
    });
}

fn blob_file(f: &Fx) -> PathBuf {
    fs::read_dir(f.dir.join("blobs"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name() != "tmp")
        .flat_map(|e| fs::read_dir(e.path()).unwrap().flatten())
        .flat_map(|e| fs::read_dir(e.path()).unwrap().flatten())
        .next()
        .unwrap()
        .path()
}

/// Nodes the file doesn't key get a minted key; one stays while the node can be recognised.
#[test]
fn minted_keys_are_carried_by_alignment() {
    let f = fx("mint");
    let v1 = file(&[
        "-|group|Sky|-|1|-|-",
        "-|text|Note|0|1|-|-",
        "@n:7|raster|Kept|-|1|-|-",
    ]);
    let s = f.open("a.idx", &v1);
    let c = f.join(&s, "alice", full_owner());
    index(&s);
    let one = s.index_view(None).unwrap();
    let sky = one.nodes[0].key.clone();
    let note = one.nodes[1].key.clone();
    assert!(sky.as_str().starts_with("@k:r1."), "{sky}");
    assert_eq!(one.nodes[2].key.as_str(), "@n:7", "a native ID stays");
    assert_eq!(one.nodes[0].key_class, KeyClass::Minted { weak: false });

    // the same tree, one more node at the end: kept keys and a new one
    let v2 = file(&[
        "-|group|Sky|-|1|-|-",
        "-|text|Note|0|1|-|-",
        "@n:7|raster|Kept|-|1|-|-",
        "-|text|Added|-|1|-|-",
    ]);
    s.write_bytes(c, &v2, s.status().unwrap().version).unwrap();
    index(&s);
    let two = s.index_view(None).unwrap();
    assert_eq!(
        two.nodes[0].key, sky,
        "carried by kind + name + parent path"
    );
    assert_eq!(two.nodes[1].key, note);
    assert_eq!(two.nodes[0].key_class, KeyClass::Minted { weak: false });
    assert!(
        two.nodes[3].key.as_str().starts_with("@k:r2."),
        "{}",
        two.nodes[3].key
    );

    // a rename keeps its key only by position, and says so
    let v3 = file(&[
        "-|group|Sky|-|1|-|-",
        "-|text|Renamed|0|1|-|-",
        "@n:7|raster|Kept|-|1|-|-",
        "-|text|Added|-|1|-|-",
    ]);
    s.write_bytes(c, &v3, s.status().unwrap().version).unwrap();
    index(&s);
    let three = s.index_view(None).unwrap();
    assert_eq!(three.nodes[1].key, note, "carried by position");
    assert_eq!(three.nodes[1].key_class, KeyClass::Minted { weak: true });

    // a tie never pairs: two equal siblings, unkeyed on both sides, get fresh keys...
    let v4 = file(&["-|text|Twin|-|1|-|-", "-|text|Twin|-|1|-|-"]);
    s.write_bytes(c, &v4, s.status().unwrap().version).unwrap();
    index(&s);
    let tied = s.index_view(None).unwrap();
    assert_ne!(tied.nodes[0].key, tied.nodes[1].key);
    let v5 = file(&[
        "#source fmt-2",
        "-|text|Twin|-|1|-|-",
        "-|text|Twin|-|1|-|-",
    ]);
    s.write_bytes(c, &v5, s.status().unwrap().version).unwrap();
    index(&s);
    let tied2 = s.index_view(None).unwrap();
    // a tie on name: the position rule pairs them weakly, one to one
    assert_eq!(tied2.nodes[0].key, tied.nodes[0].key);
    assert_eq!(tied2.nodes[1].key, tied.nodes[1].key);
    assert_eq!(tied2.nodes[0].key_class, KeyClass::Minted { weak: true });
}

/// A file ID held by two nodes is ambiguous: both are minted, and the ID stays readable.
#[test]
fn a_duplicated_native_id_is_minted_for_every_holder() {
    let f = fx("dup");
    let s = f.open(
        "a.idx",
        &file(&["@n:5|text|One|-|1|-|-", "@n:5|text|Two|-|1|-|-"]),
    );
    index(&s);
    let v = s.index_view(None).unwrap();
    assert!(v.nodes[0].key.as_str().starts_with("@k:"));
    assert!(v.nodes[1].key.as_str().starts_with("@k:"));
    assert_ne!(v.nodes[0].key, v.nodes[1].key);
    assert_eq!(v.nodes[0].native.as_ref().unwrap().as_str(), "@n:5");
    assert_eq!(v.nodes[1].native.as_ref().unwrap().as_str(), "@n:5");
}

/// A restore reuses the row (no event), and the restored version's minted keys are the keys
/// the same content had.
#[test]
fn a_restore_reuses_its_row_and_keeps_its_keys() {
    let f = fx("restore");
    let v1 = file(&["-|text|Note|-|1|-|-"]);
    let s = f.open("a.idx", &v1);
    let c = f.join(&s, "alice", full_owner());
    index(&s);
    let key1 = s.index_view(None).unwrap().nodes[0].key.clone();
    let cur = s.status().unwrap().version;
    s.write_bytes(c, &file(&["-|text|Else|-|1|-|-"]), cur)
        .unwrap();
    index(&s);
    let n = wanted(&f.events());
    s.restore(c, cur).unwrap();
    assert_eq!(wanted(&f.events()), n, "a restore reuses the row: no event");
    assert_eq!(
        s.index_view(None).unwrap().nodes[0].key,
        key1,
        "the restored version carries its keys"
    );
}

/// The index survives a rewrite, and rows for the version that wasn't purged stay.
#[test]
fn a_rewrite_keeps_a_live_row() {
    let f = fx("rewrite");
    let s = f.open("a.idx", &file(TREE));
    index(&s);
    f.store.rewrite_file().unwrap();
    assert_eq!(s.index_view(None).unwrap().nodes.len(), 3);
}

/// An agent can't write a type whose pack refuses it; a person can, and a restore is fine.
#[test]
fn an_agent_cannot_write_a_type_that_refuses_it() {
    let f = fx("agent");
    let pack = IxPack {
        refuse_agents: true,
        ..IxPack::default()
    };
    let s = f.open_with(pack.clone(), "ws", "a.idx", &file(TREE));
    f.host
        .rights
        .lock()
        .unwrap()
        .insert("bot".into(), Rights::editor());
    let agent = s
        .join(
            Participant::new("bot", "bot", "#000000", ParticipantKind::Agent, "cli"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    let v = s.status().unwrap().version;
    let e = s.write_bytes(agent, b"agent bytes", v).unwrap_err();
    assert_eq!(e, SessionError::AgentWriteRefused);
    assert_eq!(e.report().id(), "MPC-E715");
    // a person writes as ever
    let c = f.join(&s, "alice", Rights::editor());
    s.write_bytes(c, b"person bytes", v).unwrap();
    assert_eq!(pack.agent_writes(), AgentWrites::Refused);
}

/// A pack format bump: the overlay minted for the old format is absent to the new one, never
/// laid over its rows. Version 3 holds version 1's bytes, so job for 1 at the new format finds
/// 3's old overlay; the reader's output changed, so its positions mean other nodes.
#[test]
fn a_format_bump_treats_an_older_overlay_as_absent() {
    m(|| {
        let f = fx("bump");
        let v1 = file(&["-|text|A|-|1|-|-"]);
        let s = f.open("a.idx", &v1);
        let c = f.join(&s, "alice", full_owner());
        index(&s);
        let cur = s.status().unwrap().version;
        s.write_bytes(c, &file(&["-|text|B|-|1|-|-"]), cur).unwrap();
        index(&s);
        s.restore(c, 1).unwrap();
        let old = s.index_view(None).unwrap();
        assert_eq!(old.nodes.len(), 1);
        f.mgr.close(&key("ws", "a.idx"), "format bump").unwrap();
        let bumped = IxPack {
            format: 2,
            extra_root: true,
            ..IxPack::default()
        };
        let s = f
            .mgr
            .open_blob(Arc::new(bumped), &key("ws", "a.idx"), None)
            .unwrap();
        let st = s
            .index_job(Some(1))
            .unwrap()
            .expect("a new format is due")
            .run();
        assert_eq!(
            st,
            Ok(IndexStatus::Ready {
                version: 1,
                nodes: 2
            })
        );
        let restored = s.status().unwrap().version;
        let view = s.index_view(Some(restored));
        assert!(
            view.is_ok(),
            "a format bump must not read an older overlay: {view:?}"
        );
        let view = view.unwrap();
        assert_eq!(view.nodes[0].name, "Extra");
        assert_eq!(view.nodes[1].name, "A");
        assert_ne!(view.nodes[0].key, view.nodes[1].key);
    });
}

/// Keys already written don't change: a job that outlives a session unload lands after
/// another job has minted and shown keys, with a neighbour that has since gone.
#[test]
fn keys_already_shown_do_not_change_when_a_late_job_lands() {
    m(|| {
        let f = fx("late");
        let s = f.open("a.idx", &file(&["-|text|A|-|1|-|-"]));
        let c = f.join(&s, "alice", full_owner());
        index(&s);
        let cur = s.status().unwrap().version;
        s.write_bytes(c, &file(&["-|text|B|-|1|-|-"]), cur).unwrap();
        // job A has its result for version 2, and the session unloads
        let mut job_a = s.index_job(None).unwrap().unwrap();
        let (mut input, limits) = job_a.open().unwrap();
        let wire = index_to_wire(&IxPack::default(), &mut input, 20, &limits).unwrap();
        f.mgr.close(&key("ws", "a.idx"), "unload").unwrap();
        let s = f
            .mgr
            .open_blob(Arc::new(IxPack::default()), &key("ws", "a.idx"), None)
            .unwrap();
        let c = f.join(&s, "alice", full_owner());
        // job B records it first, and its keys are shown
        index(&s);
        let shown = s.index_view(None).unwrap().nodes[0].key.clone();
        // the neighbour B aligned with goes; A now mints differently
        s.purge(c, Some(1)).unwrap();
        job_a.commit_wire(input, &wire).unwrap();
        assert_eq!(
            s.index_view(None).unwrap().nodes[0].key,
            shown,
            "keys already shown don't change"
        );
    });
}
