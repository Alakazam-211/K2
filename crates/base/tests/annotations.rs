//! Comment threads on blob docs (slices A3 and A4): anchors' states across versions, rights,
//! attribution and history, events, caps, App handles, and everything purge and erasure must
//! reach (the byte scan). The file-type side is the structure index's stand-in pack
//! ([`index_pack`]). Each test runs under the mutant `MPC_ANNOT_MUTANT` names, when set.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

mod index_pack;
use index_pack::{file, IxPack};

use k2_mpc_base::anchor::{AnchorDoubt, AnchorState, AnchorTarget, CandidateWhy};
use k2_mpc_base::annot::{
    AnnotOp, AnnotOutput, AnnotPurge, Attachment, NewAnchor, NewComment, Reanchored, Thread,
    ThreadId, ThreadQuery, ThreadStatus,
};
use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::differential::under_env_annot_mutant;
use k2_mpc_base::parity::{check_annotations, render, Registry, Tier, ANNOTATION_VERBS};
use k2_mpc_base::session::{
    AnyFile, AuditEvent, BlobPack, BlobSession, Config, ConnId, Host, Need, NotReached,
    Participant, ParticipantKind, PurgedScope, Rights, SessionError, SessionManager, Stay,
};
use k2_mpc_base::store::{DocKey, DocStore};

fn m<R>(f: impl FnOnce() -> R) -> R {
    under_env_annot_mutant(f)
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
        "k2mb-annot-{tag}-{}-{}",
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
    tweak(&mut c);
    let mgr = SessionManager::new(store.clone(), host.clone(), c).with_blob_store(blobs);
    Fx {
        dir,
        host,
        store,
        mgr,
    }
}

fn fx(tag: &str) -> Fx {
    fx_with(tag, |_| {})
}

fn key(p: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: p.into(),
    }
}

fn who(pid: &str, kind: ParticipantKind) -> Participant {
    Participant::new(pid, format!("Name of {pid}"), "#123456", kind, "tab")
}

impl Fx {
    fn open_pack(&self, pack: Arc<dyn BlobPack>, path: &str, bytes: &[u8]) -> Arc<BlobSession> {
        self.mgr
            .open_blob(pack, &key(path), Some(&mut &bytes[..]))
            .unwrap()
    }
    /// An indexed doc of the stand-in pack.
    fn open(&self, path: &str, bytes: &[u8]) -> Arc<BlobSession> {
        let s = self.open_pack(Arc::new(IxPack::default()), path, bytes);
        index(&s);
        s
    }
    fn join_as(&self, s: &BlobSession, pid: &str, kind: ParticipantKind, r: Rights) -> ConnId {
        self.host.rights.lock().unwrap().insert(pid.into(), r);
        s.join(who(pid, kind), Stay::Held).unwrap().conn
    }
    fn join(&self, s: &BlobSession, pid: &str, r: Rights) -> ConnId {
        self.join_as(s, pid, ParticipantKind::Person, r)
    }
    fn events(&self) -> Vec<AuditEvent> {
        self.host.events.lock().unwrap().clone()
    }
    fn holds(&self, needles: &[&[u8]]) -> bool {
        !self.store.scan_for(needles).unwrap().is_empty()
    }
}

fn index(s: &BlobSession) {
    if let Some(j) = s.index_job(None).unwrap() {
        j.run().unwrap();
    }
}

fn owner() -> Rights {
    Rights::full().with_owner(true)
}

/// A random-looking marker, unique per tag: no other byte of the store holds it, and no two
/// markers share an 8-byte window (a purge by text matches by window).
fn secret(tag: &str) -> String {
    let fnv = |s: &str| {
        s.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
        })
    };
    format!("z{:016x}q{:016x}", fnv(tag), fnv(&format!("{tag}!")))
}

const TREE: &[&str] = &[
    "@n:1|group|Layers|-|1|-|-",
    "@n:2|raster|Logo|0|1|-|logo-pixels|-",
    "@n:3|text|Title|0|0|Hello|-|-",
];

fn on_node(k: &str, body: &str) -> NewComment {
    NewComment::new(NewAnchor::node(k, None), body)
}

fn state(t: &Thread) -> Option<AnchorState> {
    t.resolution.as_ref().map(|r| r.state)
}

#[test]
fn a_comment_on_a_node_reads_attached_and_makes_no_version() {
    m(|| {
        let f = fx("node");
        let s = f.open("a.idx", &file(TREE));
        let c = f.join(&s, "alice", Rights::editor());
        let v = s.status().unwrap().version;
        let t = s
            .comment(
                c,
                on_node("@n:2", "Too dark")
                    .with_mentions(["bob", "bob", "carol"])
                    .with_attachment(Attachment::proposal("out/logo-v2.idx")),
            )
            .unwrap();
        assert_eq!(t.id, ThreadId::parse("ann-1").unwrap());
        assert_eq!(t.id.to_string(), "ann-1");
        assert_eq!(t.anchor.key().map(|k| k.as_str()), Some("@n:2"));
        assert_eq!(t.anchor.created_version, v);
        assert_eq!(state(&t), Some(AnchorState::Attached));
        assert!(t.resolution.as_ref().unwrap().current);
        assert_eq!(t.status, ThreadStatus::Open);
        assert_eq!(t.created.pid, "alice");
        assert_eq!(t.comments.len(), 1);
        let c0 = &t.comments[0];
        assert_eq!(c0.body.as_deref(), Some("Too dark"));
        assert_eq!(
            c0.mentions,
            ["bob", "carol"],
            "mentions once each, in order"
        );
        assert_eq!(c0.attachments[0].target, "out/logo-v2.idx");
        // no version, no write
        assert_eq!(s.status().unwrap().version, v);
        assert_eq!(s.versions(f.join(&s, "o", owner())).unwrap().len(), 1);
        // the event carries the mentions and the node, never the body
        let ev = f.events();
        let a = ev
            .iter()
            .find_map(|e| match e {
                AuditEvent::Annotation {
                    op: AnnotOp::Created,
                    mentions,
                    anchor_key,
                    pid,
                    seq,
                    ..
                } => Some((mentions.clone(), anchor_key.clone(), pid.clone(), *seq)),
                _ => None,
            })
            .expect("an annotation event");
        assert_eq!(a.0, ["bob", "carol"]);
        assert_eq!(a.1.map(|k| k.to_string()).as_deref(), Some("@n:2"));
        assert_eq!((a.2.as_str(), a.3), ("alice", 1));
        assert!(
            !format!("{ev:?}").contains("Too dark"),
            "no body in an event"
        );
    });
}

#[test]
fn edit_implies_comment() {
    m(|| {
        let f = fx("edit");
        let s = f.open("a.idx", &file(TREE));
        let c = f.join(&s, "ed", Rights::editor());
        assert!(
            s.comment(c, NewComment::new(NewAnchor::Doc, "ok")).is_ok(),
            "edit implies comment"
        );
    });
}

#[test]
fn a_reader_without_the_comment_right_is_refused() {
    m(|| {
        let f = fx("rights");
        let s = f.open("a.idx", &file(TREE));
        let o = f.join(&s, "owner", owner());
        let t = s
            .comment(o, NewComment::new(NewAnchor::Doc, "first"))
            .unwrap()
            .id;
        let r = f.join(&s, "reader", Rights::read_only());
        let got = s.comment(r, NewComment::new(NewAnchor::Doc, "hi"));
        assert_eq!(
            got.as_ref().map(|_| ()),
            Err(&SessionError::Forbidden {
                need: Need::Comment
            }),
            "a reader without the comment right is refused"
        );
        let e = got.unwrap_err();
        assert_eq!(e.report().id(), "MPC-E116");
        assert_eq!(e.report().exit(), 3);
        for refused in [
            s.reply(r, t, "x".into(), vec![], vec![]).map(|_| ()),
            s.resolve(r, t).map(|_| ()),
            s.reopen(r, t).map(|_| ()),
            s.delete_comment(r, t, 0).map(|_| ()),
            s.reanchor(r, t, NewAnchor::Doc).map(|_| ()),
        ] {
            assert_eq!(
                refused,
                Err(SessionError::Forbidden {
                    need: Need::Comment
                })
            );
        }
        // reading is read's
        assert_eq!(s.comments(r, &ThreadQuery::new()).unwrap().threads.len(), 1);
        assert_eq!(s.thread(r, t, None).unwrap().comments.len(), 1);
        // the log is history's
        assert_eq!(
            s.annotation_log(r, None, 0, 10),
            Err(SessionError::Forbidden {
                need: Need::History
            })
        );
        // a read-only share the host grants the right comments
        let g = f.join(&s, "guest", Rights::read_only().with_comment(true));
        assert!(s
            .comment(g, NewComment::new(NewAnchor::Doc, "granted"))
            .is_ok());
        // the refusal was audited
        assert!(f.events().iter().any(|e| matches!(
            e,
            AuditEvent::Refused {
                need: Need::Comment,
                ..
            }
        )));
    });
}

#[test]
fn only_the_author_the_person_it_was_for_or_an_owner_deletes() {
    m(|| {
        let f = fx("delete");
        let s = f.open("a.idx", &file(TREE));
        let a = f.join(&s, "alice", Rights::editor());
        let b = f.join(&s, "bob", Rights::editor());
        let t = s
            .comment(a, NewComment::new(NewAnchor::Doc, "mine"))
            .unwrap()
            .id;
        let got = s.delete_comment(b, t, 0);
        assert!(
            matches!(got, Err(SessionError::NotCommentAuthor { .. })),
            "someone else's comment is refused: {got:?}"
        );
        let e = got.unwrap_err();
        assert_eq!(e.report().id(), "MPC-E117");
        assert_eq!(e.report().exit(), 3);
        // an agent's comment on alice's behalf: alice may delete it
        f.host
            .rights
            .lock()
            .unwrap()
            .insert("bot".into(), Rights::editor());
        let bot = s
            .join(
                who("bot", ParticipantKind::Agent).on_behalf_of("alice"),
                Stay::Held,
            )
            .unwrap()
            .conn;
        s.reply(bot, t, "for alice".into(), vec![], vec![]).unwrap();
        let after = s.delete_comment(a, t, 1).unwrap();
        assert!(after.comments[1].body.is_none());
        assert_eq!(after.comments[1].deleted.as_ref().unwrap().pid, "alice");
        // an owner may delete anyone's
        let o = f.join(&s, "owner", owner());
        let after = s.delete_comment(o, t, 0).unwrap();
        assert!(after.comments[0].body.is_none());
        // again: unchanged, no new history row
        let rows = s.annotation_log(o, Some(t), 0, 100).unwrap().len();
        s.delete_comment(o, t, 0).unwrap();
        assert_eq!(s.annotation_log(o, Some(t), 0, 100).unwrap().len(), rows);
        // no such comment, no such thread
        assert_eq!(
            s.delete_comment(o, t, 9).unwrap_err().report().id(),
            "MPC-E320"
        );
        assert_eq!(
            s.delete_comment(o, ThreadId::parse("ann-9").unwrap(), 0)
                .unwrap_err()
                .report()
                .exit(),
            4
        );
    });
}

#[test]
fn a_deleted_body_leaves_no_byte() {
    m(|| {
        let f = fx("delete-bytes");
        let s = f.open("a.idx", &file(TREE));
        let a = f.join(&s, "alice", owner());
        let body = secret("deleted");
        let t = s
            .comment(a, on_node("@n:2", &format!("look: {body}")))
            .unwrap()
            .id;
        assert!(f.holds(&[body.as_bytes()]), "stored before");
        let after = s.delete_comment(a, t, 0).unwrap();
        // gone from the store file at its next rewrite: no copy was left anywhere
        f.store.rewrite_file().unwrap();
        assert!(
            !f.holds(&[body.as_bytes()]),
            "a deleted body leaves no byte"
        );
        // and from every read at once
        assert_eq!(after.comments[0].body, None);
        assert!(after.comments[0].mentions.is_empty());
        let page = s.comments(a, &ThreadQuery::new()).unwrap();
        let log = s.annotation_log(a, None, 0, 100).unwrap();
        assert!(!format!("{page:?}{log:?}").contains(&body));
        assert_eq!(log.last().unwrap().op, AnnotOp::Deleted);
    });
}

#[test]
fn purge_removes_comment_bodies_and_rewrites_the_store() {
    m(|| {
        let f = fx("purge");
        let s = f.open("a.idx", &file(TREE));
        let o = f.join(&s, "owner", owner());
        let e = f.join(&s, "ed", Rights::editor());
        let (one, two, three) = (secret("one"), secret("two"), secret("three"));
        let t1 = s
            .comment(e, NewComment::new(NewAnchor::Doc, format!("a {one} b")))
            .unwrap()
            .id;
        s.reply(e, t1, "harmless".into(), vec![], vec![]).unwrap();
        let t2 = s
            .comment(
                e,
                on_node("@n:2", &format!("c {two}")).with_attachment(Attachment::proposal("p")),
            )
            .unwrap()
            .id;
        let t3 = s
            .comment(e, NewComment::new(NewAnchor::Doc, three.clone()))
            .unwrap()
            .id;
        // only an owner purges
        assert_eq!(
            s.purge_annotations(e, AnnotPurge::All).unwrap_err(),
            SessionError::Forbidden { need: Need::Owner }
        );
        // by text: the body holding it, whole
        let out = s
            .purge_annotations(o, AnnotPurge::Text(one.clone()))
            .unwrap();
        assert!(out.rewrite.is_some(), "the store was rewritten");
        assert!(!f.holds(&[one.as_bytes()]), "purge removes comments");
        assert!(out.not_reached.contains(&NotReached::SentNotifications));
        assert_eq!(out.counts.comments, 1);
        let th = s.thread(o, t1, None).unwrap();
        assert_eq!(th.comments[0].body, None);
        assert_eq!(th.comments[1].body.as_deref(), Some("harmless"));
        // one thread: its bodies and its anchor's copy of the node
        let out = s.purge_annotations(o, AnnotPurge::Thread(t2)).unwrap();
        assert!(!f.holds(&[two.as_bytes()]), "purge removes comments");
        assert_eq!((out.counts.comments, out.counts.anchors), (1, 1));
        let th = s.thread(o, t2, None).unwrap();
        assert!(th.anchor.fingerprint.is_none() && th.anchor.last_seen.is_none());
        assert_eq!(
            state(&th),
            Some(AnchorState::Suspect(AnchorDoubt::HintsPurged))
        );
        // one comment
        s.purge_annotations(o, AnnotPurge::Comment(t3, 0)).unwrap();
        assert!(!f.holds(&[three.as_bytes()]), "purge removes comments");
        // the threads stay, who and when stay, and the history says purged
        assert_eq!(s.comments(o, &ThreadQuery::new()).unwrap().threads.len(), 3);
        let purged = s
            .annotation_log(o, None, 0, 100)
            .unwrap()
            .into_iter()
            .filter(|e| e.op == AnnotOp::Purged)
            .count();
        assert_eq!(purged, 3);
        assert!(f.events().iter().any(|ev| matches!(
            ev,
            AuditEvent::Purged {
                scope: PurgedScope::AnnotationText { .. },
                ..
            }
        )));
        // a text shorter than a window is refused
        assert_eq!(
            s.purge_annotations(o, AnnotPurge::Text("short".into()))
                .unwrap_err()
                .report()
                .id(),
            "MPC-E321"
        );
        // all
        let t4 = s
            .comment(e, NewComment::new(NewAnchor::Doc, secret("four")))
            .unwrap()
            .id;
        s.purge_annotations(o, AnnotPurge::All).unwrap();
        assert!(s.thread(o, t4, None).unwrap().comments[0].body.is_none());
        assert!(
            !f.holds(&[secret("four").as_bytes()]),
            "purge removes comments"
        );
    });
}

#[test]
fn participant_erasure_removes_their_bodies() {
    m(|| {
        let f = fx("erase");
        let s = f.open("a.idx", &file(TREE));
        let o = f.join(&s, "owner", owner());
        let pid = "pid-7f3a9c2e";
        let a = f.join(&s, pid, Rights::editor());
        let b = f.join(&s, "bob", Rights::editor());
        let theirs = secret("theirs");
        let t = s
            .comment(a, NewComment::new(NewAnchor::Doc, theirs.clone()))
            .unwrap()
            .id;
        s.reply(b, t, "bob's reply".into(), vec![pid.into()], vec![])
            .unwrap();
        s.resolve(a, t).unwrap();
        let v = s.status().unwrap().version;
        s.write_bytes(a, &file(&["@n:9|group|Other|-|1|-|-"]), v)
            .unwrap();
        // only an owner erases
        assert_eq!(
            s.erase_participant(b, pid, "erased-1").unwrap_err(),
            SessionError::Forbidden { need: Need::Owner }
        );
        let out = s.erase_participant(o, pid, "erased-1").unwrap();
        assert!(out.rewrite.is_some());
        assert!(
            !f.holds(&[theirs.as_bytes(), pid.as_bytes()]),
            "the erased participant's bodies are gone"
        );
        assert_eq!(out.counts.comments, 1);
        let th = s.thread(o, t, None).unwrap();
        assert_eq!(
            th.comments[0].body, None,
            "the erased participant's bodies are gone"
        );
        assert_eq!(th.comments[0].by.pid, "erased-1");
        assert_eq!(th.comments[0].by.name, "erased-1");
        assert_eq!(th.comments[1].body.as_deref(), Some("bob's reply"));
        assert_eq!(th.comments[1].mentions, ["erased-1"]);
        assert_eq!(th.created.pid, "erased-1");
        assert!(matches!(&th.status, ThreadStatus::Resolved { by } if by.pid == "erased-1"));
        let log = s.annotation_log(o, None, 0, 100).unwrap();
        assert!(log.iter().all(|e| e.by.pid != pid));
        let versions = s.versions(o).unwrap();
        assert!(versions.iter().all(|(_, v)| v.author != pid));
        // their connections are gone
        assert!(matches!(s.touch(a), Err(SessionError::NotJoined { .. })));
    });
}

/// The rule: an anchor whose key is gone is never moved to a look-alike; it reads suspect with
/// the candidate, keeps where it was last seen, and only a re-anchor moves it.
#[test]
fn an_anchor_never_moves_silently() {
    m(|| {
        let f = fx("silent");
        let s = f.open("a.idx", &file(TREE));
        let a = f.join(&s, "alice", owner());
        let t = s.comment(a, on_node("@n:2", "Logo note")).unwrap().id;
        let v1 = s.status().unwrap().version;
        // the next version: @n:2 is gone, @n:7 looks like it
        let next = file(&[
            "@n:1|group|Layers|-|1|-|-",
            "@n:7|raster|Logo|0|1|-|other-pixels|-",
        ]);
        let v2 = s.write_bytes(a, &next, v1).unwrap().version;
        index(&s);
        let th = s.thread(a, t, None).unwrap();
        assert_eq!(
            th.anchor.target,
            AnchorTarget::Node {
                key: k2_mpc_base::index::NodeKey::parse("@n:2").unwrap()
            },
            "an anchor never moves silently"
        );
        let r = th.resolution.as_ref().unwrap();
        assert_eq!(
            r.state,
            AnchorState::Suspect(AnchorDoubt::CandidateOnly),
            "an anchor never moves silently"
        );
        assert_eq!(r.version, v2);
        assert_eq!(r.candidates[0].key.as_str(), "@n:7");
        assert_eq!(r.candidates[0].why, CandidateWhy::SameKindAndName);
        assert_eq!(th.anchor.last_seen.as_ref().unwrap().version, v1);
        // at the old version it was attached (history)
        let old = s.thread(a, t, Some(v1)).unwrap();
        assert_eq!(state(&old), Some(AnchorState::Attached));
        // the host heard about the change, with no content
        let changes: Vec<(Option<AnchorState>, AnchorState)> = f
            .events()
            .iter()
            .filter_map(|e| match e {
                AuditEvent::AnchorState {
                    was, now, version, ..
                } if *version == v2 => Some((*was, *now)),
                _ => None,
            })
            .collect();
        assert_eq!(
            changes,
            [(
                Some(AnchorState::Attached),
                AnchorState::Suspect(AnchorDoubt::CandidateOnly)
            )]
        );
        // a person re-anchors: now it moves, and the history says who
        let done = s.reanchor(a, t, NewAnchor::node("@n:7", None)).unwrap();
        let Reanchored::Applied(th) = done else {
            panic!("a person's re-anchor applies")
        };
        assert_eq!(state(&th), Some(AnchorState::Attached));
        let last = s.annotation_log(a, Some(t), 0, 100).unwrap().pop().unwrap();
        assert_eq!(
            (last.op, last.by.pid.as_str()),
            (AnnotOp::Reanchored, "alice")
        );
    });
}

#[test]
fn an_agents_reanchor_is_a_proposal_a_person_confirms() {
    m(|| {
        let f = fx("propose");
        let s = f.open("a.idx", &file(TREE));
        let a = f.join(&s, "alice", Rights::editor());
        let bot = f.join_as(&s, "bot", ParticipantKind::Agent, Rights::editor());
        let t = s
            .comment(a, NewComment::new(NewAnchor::Doc, "where?"))
            .unwrap()
            .id;
        let r = s.reanchor(bot, t, NewAnchor::node("@n:2", None)).unwrap();
        let Reanchored::Proposed(th) = r else {
            panic!("an agent's re-anchor is a proposal: {r:?}")
        };
        assert_eq!(
            th.anchor.target,
            AnchorTarget::Doc,
            "an agent's re-anchor is a proposal"
        );
        let p = th.pending_anchor.as_ref().expect("a pending proposal");
        assert_eq!(p.by.pid, "bot");
        // the person confirms it
        let Reanchored::Applied(th) = s.reanchor(a, t, NewAnchor::node("@n:2", None)).unwrap()
        else {
            panic!("a person's applies")
        };
        assert!(th.pending_anchor.is_none());
        assert_eq!(th.anchor.key().map(|k| k.as_str()), Some("@n:2"));
        let ops: Vec<AnnotOp> = s
            .annotation_log(f.join(&s, "o", owner()), Some(t), 0, 100)
            .unwrap()
            .into_iter()
            .map(|e| e.op)
            .collect();
        assert_eq!(
            ops,
            [
                AnnotOp::Created,
                AnnotOp::ReanchorProposed,
                AnnotOp::Reanchored
            ]
        );
    });
}

/// A version purge takes the anchors' copies of that version's node names (content).
#[test]
fn a_version_purge_takes_the_anchors_copies() {
    m(|| {
        let f = fx("vpurge");
        let name = secret("layername");
        let line = format!("@n:1|raster|{name}|-|1|-|-");
        let first = file(&[line.as_str()]);
        let s = f.open("a.idx", &first);
        let o = f.join(&s, "owner", owner());
        let t = s
            .comment(
                o,
                NewComment::new(NewAnchor::node("@n:1", None), "x").with_hint_for_test(),
            )
            .unwrap()
            .id;
        let v1 = s.status().unwrap().version;
        // the next version renames the node: the old name lives only in v1
        s.write_bytes(o, &file(&["@n:1|raster|Renamed|-|1|-|-"]), v1)
            .unwrap();
        index(&s);
        assert!(f.holds(&[name.as_bytes()]));
        let out = s.purge(o, Some(v1)).unwrap();
        assert!(out.rewrite.is_some());
        assert!(
            !f.holds(&[name.as_bytes()]),
            "a version purge takes the anchors' copies"
        );
        assert_eq!(out.counts.anchors, 1);
        let th = s.thread(o, t, None).unwrap();
        assert!(th.anchor.fingerprint.is_none() && th.anchor.hint.is_none());
        assert_eq!(
            state(&th),
            Some(AnchorState::Suspect(AnchorDoubt::HintsPurged))
        );
    });
}

trait HintForTest {
    fn with_hint_for_test(self) -> Self;
}

impl HintForTest for NewComment {
    fn with_hint_for_test(mut self) -> Self {
        self.anchor = self.anchor.with_hint("0.5,0.5");
        self
    }
}

#[test]
fn states_follow_the_versions_and_a_pending_index_says_so() {
    m(|| {
        let f = fx("states");
        let s = f.open("a.idx", &file(TREE));
        let a = f.join(&s, "alice", owner());
        let logo = s.comment(a, on_node("@n:2", "logo")).unwrap().id;
        let title = s.comment(a, on_node("@n:3", "title")).unwrap().id;
        assert_eq!(
            state(&s.thread(a, title, None).unwrap()),
            Some(AnchorState::Hidden),
            "an invisible node is hidden"
        );
        let v1 = s.status().unwrap().version;
        // the next version: the logo is a text node now, not yet indexed
        let next = file(&[
            "@n:1|group|Layers|-|1|-|-",
            "@n:2|text|Logo|0|1|-|-|-",
            "@n:3|text|Title|0|1|Hello|-|-",
        ]);
        let v2 = s.write_bytes(a, &next, v1).unwrap().version;
        let th = s.thread(a, logo, None).unwrap();
        let r = th.resolution.as_ref().unwrap();
        assert!(!r.current, "resolved against an older index");
        assert_eq!((r.version, r.state), (v1, AnchorState::Attached));
        index(&s);
        let th = s.thread(a, logo, None).unwrap();
        let r = th.resolution.as_ref().unwrap();
        assert!(r.current);
        assert_eq!(
            (r.version, r.state),
            (v2, AnchorState::Suspect(AnchorDoubt::KindChanged))
        );
        assert_eq!(
            state(&s.thread(a, title, None).unwrap()),
            Some(AnchorState::Attached)
        );
        // an older version needs history
        let r = f.join(&s, "reader", Rights::read_only());
        assert_eq!(
            s.thread(r, logo, Some(v1)).unwrap_err(),
            SessionError::Forbidden {
                need: Need::History
            }
        );
        assert_eq!(
            s.comments(r, &ThreadQuery::new().at_version(v1))
                .unwrap_err(),
            SessionError::Forbidden {
                need: Need::History
            }
        );
        let page = s.comments(a, &ThreadQuery::new().at_version(v1)).unwrap();
        assert_eq!(state(&page.threads[0]), Some(AnchorState::Attached));
    });
}

#[test]
fn bad_input_and_caps_are_refused_with_their_ids() {
    m(|| {
        let f = fx_with("caps", |c| {
            c.annot_threads_max = 2;
            c.annot_comments_max = 2;
            c.annot_body_max_bytes = 10;
        });
        let s = f.open("a.idx", &file(TREE));
        let a = f.join(&s, "alice", Rights::editor());
        let id = |r: Result<Thread, SessionError>| r.unwrap_err().report().id();
        assert_eq!(
            id(s.comment(a, NewComment::new(NewAnchor::Doc, "  "))),
            "MPC-E321"
        );
        assert_eq!(
            id(s.comment(a, NewComment::new(NewAnchor::Doc, "x".repeat(11)))),
            "MPC-E907"
        );
        assert_eq!(
            id(s.comment(a, on_node("Logo", "x"))),
            "MPC-E321",
            "a name is not a key"
        );
        assert_eq!(
            id(s.comment(a, on_node("@n:99", "x"))),
            "MPC-E321",
            "no such node"
        );
        assert_eq!(
            id(s.comment(a, NewComment::new(NewAnchor::Doc, "x").with_mentions([""]))),
            "MPC-E321"
        );
        assert_eq!(
            id(s.comment(
                a,
                NewComment::new(
                    NewAnchor::node("@n:2", None).with_hint("h".repeat(300)),
                    "x"
                )
            )),
            "MPC-E321"
        );
        let t = s
            .comment(a, NewComment::new(NewAnchor::Doc, "one"))
            .unwrap()
            .id;
        s.comment(a, NewComment::new(NewAnchor::Doc, "two"))
            .unwrap();
        let e = s
            .comment(a, NewComment::new(NewAnchor::Doc, "three"))
            .unwrap_err();
        assert_eq!(
            e,
            SessionError::AnnotationCap {
                what: "threads",
                limit: 2
            }
        );
        assert_eq!(e.report().exit(), 2);
        s.reply(a, t, "r".into(), vec![], vec![]).unwrap();
        assert_eq!(
            id(s.reply(a, t, "r".into(), vec![], vec![])),
            "MPC-E907",
            "two comments a thread"
        );
        // no index: only the whole doc
        let plain = f.open_pack(Arc::new(AnyFile), "x.bin", b"bytes");
        let p = f.join(&plain, "alice", Rights::editor());
        assert_eq!(id(plain.comment(p, on_node("@n:1", "x"))), "MPC-E711");
        let th = plain
            .comment(p, NewComment::new(NewAnchor::Doc, "fine"))
            .unwrap();
        assert_eq!(state(&th), Some(AnchorState::Attached));
        // a pending index
        // (other bytes: the same bytes' index is shared, and recorded already)
        let pend = f.open_pack(
            Arc::new(IxPack::default()),
            "p.idx",
            &file(&["@n:2|raster|Pending|-|1|-|-"]),
        );
        let q = f.join(&pend, "alice", Rights::editor());
        let e = pend.comment(q, on_node("@n:2", "x")).unwrap_err();
        assert_eq!(e.report().id(), "MPC-E713");
    });
}

#[test]
fn pages_and_since_and_filters() {
    m(|| {
        let f = fx("pages");
        let s = f.open("a.idx", &file(TREE));
        let a = f.join(&s, "alice", Rights::editor());
        let t1 = s.comment(a, on_node("@n:2", "1")).unwrap().id;
        let t2 = s
            .comment(a, NewComment::new(NewAnchor::Doc, "2"))
            .unwrap()
            .id;
        let t3 = s.comment(a, on_node("@n:2", "3")).unwrap().id;
        s.resolve(a, t2).unwrap();
        let p = s.comments(a, &ThreadQuery::new().limit(2)).unwrap();
        assert_eq!(p.threads.iter().map(|t| t.id).collect::<Vec<_>>(), [t1, t2]);
        assert_eq!(p.next, Some(t2));
        let p2 = s
            .comments(a, &ThreadQuery::new().after(t2).limit(2))
            .unwrap();
        assert_eq!(p2.threads.iter().map(|t| t.id).collect::<Vec<_>>(), [t3]);
        assert_eq!(p2.next, None);
        let open = s.comments(a, &ThreadQuery::new().open(true)).unwrap();
        assert_eq!(open.threads.len(), 2);
        let on = s.comments(a, &ThreadQuery::new().on("@n:2")).unwrap();
        assert_eq!(
            on.threads.iter().map(|t| t.id).collect::<Vec<_>>(),
            [t1, t3]
        );
        let head = p.head;
        s.reply(a, t3, "later".into(), vec![], vec![]).unwrap();
        let since = s.comments(a, &ThreadQuery::new().since(head)).unwrap();
        assert_eq!(since.threads.iter().map(|t| t.id).collect::<Vec<_>>(), [t3]);
        assert!(since.head > head);
    });
}

#[test]
fn an_app_viewer_sees_handles() {
    m(|| {
        let f = fx("app");
        let s = f.open("a.idx", &file(TREE));
        let a = f.join(&s, "alice", Rights::editor());
        s.comment(
            a,
            NewComment::new(NewAnchor::Doc, "hi").with_mentions(["bob"]),
        )
        .unwrap();
        let app = f.join_as(&s, "guest-77", ParticipantKind::App, Rights::editor());
        let page = s.comments(app, &ThreadQuery::new()).unwrap();
        let t = &page.threads[0];
        assert_eq!(t.created.pid, "p1", "a handle, not the pid");
        assert_eq!(t.comments[0].by.pid, "p1");
        assert_eq!(t.comments[0].mentions, ["p2"]);
        let mine = s
            .comment(app, NewComment::new(NewAnchor::Doc, "me"))
            .unwrap();
        assert_eq!(mine.created.pid, "p3", "not even its own pid");
        // a mention it passes is one of its handles
        let back = s
            .comment(
                app,
                NewComment::new(NewAnchor::Doc, "you").with_mentions(["p1"]),
            )
            .unwrap();
        assert_eq!(back.comments[0].mentions, ["p1"]);
        let o = f.join(&s, "o", owner());
        let raw = s.thread(o, back.id, None).unwrap();
        assert_eq!(raw.comments[0].mentions, ["alice"], "stored as the pid");
    });
}

#[test]
fn verbs_are_the_typed_calls() {
    m(|| {
        let f = fx("verbs");
        let s = f.open("a.idx", &file(TREE));
        let bot = f.join_as(&s, "bot", ParticipantKind::Agent, Rights::editor());
        let argv = |v: &[&str]| v.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
        let out = s
            .run_annotation(
                bot,
                &argv(&["comment", "--node", "@n:2", "--body", "agent note"]),
            )
            .unwrap();
        let AnnotOutput::Thread(t) = out else {
            panic!()
        };
        assert_eq!(t.created.pid, "bot");
        let e = s
            .run_annotation(bot, &argv(&["comment", "--node", "@n:2"]))
            .unwrap_err();
        assert_eq!(e.exit(), 2);
        assert_eq!(e.report().id(), "MPC-E122");
        let e = s
            .run_annotation(bot, &argv(&["reply", "ann-5", "--body", "x"]))
            .unwrap_err();
        assert_eq!((e.exit(), e.report().id()), (4, "MPC-E320"));
        // the registry: no tier of their own, so a host that hasn't adopted them sees none
        let blob = Registry::for_tier(Tier::Blob);
        assert!(ANNOTATION_VERBS.iter().all(|v| blob.get(v.name).is_none()));
        let with = Registry::for_tier(Tier::Blob).with_annotations();
        assert_eq!(
            with.verbs().len(),
            blob.verbs().len() + ANNOTATION_VERBS.len()
        );
    });
}

/// Parity with nodes: every person action has its verb, with the same effect.
#[test]
fn annotation_parity_holds_on_an_indexed_type() {
    m(|| {
        let n = check_annotations(Arc::new(IxPack::default()), &file(TREE), Some("@n:2"))
            .unwrap_or_else(|f| panic!("{}", render(&f)));
        assert_eq!(n, 13);
    });
}

#[test]
fn a_doc_delete_takes_its_threads() {
    m(|| {
        let f = fx("docdel");
        let s = f.open("a.idx", &file(TREE));
        let a = f.join(&s, "alice", Rights::editor());
        let body = secret("docdel");
        s.comment(a, NewComment::new(NewAnchor::Doc, body.clone()))
            .unwrap();
        drop(s);
        f.mgr.delete_doc(&key("a.idx")).unwrap();
        f.store.rewrite_file().unwrap();
        assert!(!f.holds(&[body.as_bytes()]));
        let s = f.open("a.idx", &file(TREE));
        let a = f.join(&s, "alice", Rights::editor());
        assert!(s
            .comments(a, &ThreadQuery::new())
            .unwrap()
            .threads
            .is_empty());
    });
}

/// A participant with these fields.
fn long(pid: &str, name: &str, kind: ParticipantKind) -> Participant {
    Participant::new(pid, name, "#123456", kind, "tab")
}

/// Ids are identities: one too long to store is refused at the write (the host's to fix).
/// Display fields are cut there. So every row written reads back, and purges.
#[test]
fn long_attribution_fields_are_bounded_at_the_write() {
    m(|| {
        let f = fx("long-attr");
        let s = f.open("a.idx", &file(TREE));
        f.host
            .rights
            .lock()
            .unwrap()
            .insert("pid-long-name".into(), Rights::editor());
        let c = s
            .join(
                long(
                    "pid-long-name",
                    &"N".repeat(10_000),
                    ParticipantKind::Person,
                ),
                Stay::Held,
            )
            .unwrap()
            .conn;
        let t = s
            .comment(c, NewComment::new(NewAnchor::Doc, "named at length"))
            .unwrap();
        assert!(
            t.created.name.len() <= 256 && t.comments[0].by.name.len() <= 256,
            "a long name is cut at the write"
        );
        // every call on the thread still works, purge included
        let o = f.join(&s, "owner", owner());
        assert_eq!(s.comments(o, &ThreadQuery::new()).unwrap().threads.len(), 1);
        s.purge_annotations(o, AnnotPurge::All).unwrap();
        // an id too long to store is refused, nothing written
        let pid = "p".repeat(300);
        f.host
            .rights
            .lock()
            .unwrap()
            .insert(pid.clone(), Rights::editor());
        let c = s
            .join(long(&pid, "Long", ParticipantKind::Person), Stay::Held)
            .unwrap()
            .conn;
        let e = s
            .comment(c, NewComment::new(NewAnchor::Doc, "x"))
            .unwrap_err();
        assert!(
            matches!(e, SessionError::BadParticipant(_)),
            "a long id is refused at the write: {e:?}"
        );
        assert_eq!(e.report().id(), "MPC-E115");
        assert_eq!(s.comments(o, &ThreadQuery::new()).unwrap().threads.len(), 1);
        // and an erasure's pseudonym is bounded alike
        let e = s
            .erase_participant(o, "pid-long-name", &"q".repeat(300))
            .unwrap_err();
        assert!(matches!(e, SessionError::BadParticipant(_)));
    });
}

/// Blob erasure refuses what Session's erasure refuses, and a pseudonym that is already a
/// participant's.
#[test]
fn erasure_refuses_what_it_must() {
    m(|| {
        let f = fx("erase-checks");
        let s = f.open("a.idx", &file(TREE));
        let o = f.join(&s, "owner", owner());
        let a = f.join(&s, "alice", Rights::editor());
        let b = f.join(&s, "bob", Rights::editor());
        s.comment(a, NewComment::new(NewAnchor::Doc, "a")).unwrap();
        s.comment(b, NewComment::new(NewAnchor::Doc, "b")).unwrap();
        s.leave(b).unwrap();
        for (pid, ps, why) in [
            ("alice", "kit:x", "a reserved pseudonym"),
            ("kit:import", "erased-1", "a reserved pid"),
            ("owner", "erased-1", "the owner themselves"),
            ("alice", "owner", "a present participant's pid"),
            ("alice", "bob", "a pid in the doc's records"),
        ] {
            let got = s.erase_participant(o, pid, ps);
            assert!(
                matches!(got, Err(SessionError::BadParticipant(_))),
                "an erasure refuses {why}: {got:?}"
            );
        }
        // nothing was renamed
        let page = s.comments(o, &ThreadQuery::new()).unwrap();
        let by: Vec<&str> = page
            .threads
            .iter()
            .map(|t| t.created.pid.as_str())
            .collect();
        assert_eq!(by, ["alice", "bob"]);
        // a fresh pseudonym goes through
        s.erase_participant(o, "alice", "erased-1").unwrap();
    });
}

/// A comment admitted before an erasure can't commit after it with the erased pid: the
/// erasure waits for it.
#[test]
fn a_comment_admitted_before_an_erasure_does_not_outlive_it() {
    m(|| {
        use std::sync::mpsc::channel;
        let f = fx("erase-race");
        let s = f.open("a.idx", &file(TREE));
        let o = f.join(&s, "owner", owner());
        let pid = "pid-race";
        let a = f.join(&s, pid, Rights::editor());
        let (paused_tx, paused_rx) = channel();
        let (resume_tx, resume_rx) = channel();
        k2_mpc_base::differential::pause_next_annotation(paused_tx, resume_rx);
        // a mutant is planted per thread: each thread plants the one the run names
        let s1 = s.clone();
        let writer = std::thread::spawn(move || {
            m(|| s1.comment(a, NewComment::new(NewAnchor::Doc, "late")))
        });
        paused_rx.recv().unwrap();
        let s2 = s.clone();
        let eraser = std::thread::spawn(move || m(|| s2.erase_participant(o, pid, "erased-race")));
        // the erasure either finishes at once (it didn't wait) or waits for the writer
        for _ in 0..100 {
            if eraser.is_finished() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        resume_tx.send(()).unwrap();
        let _ = writer.join().unwrap();
        eraser.join().unwrap().unwrap();
        let page = s.comments(o, &ThreadQuery::new()).unwrap();
        let pids: Vec<&str> = page
            .threads
            .iter()
            .flat_map(|t| t.comments.iter().map(|c| c.by.pid.as_str()))
            .collect();
        assert!(
            !pids.contains(&pid),
            "a comment admitted before an erasure doesn't outlive it: {pids:?}"
        );
    });
}

/// Erasure clears what an agent wrote on the erased person's behalf, as delete treats it as
/// theirs.
#[test]
fn erasure_clears_what_was_written_on_their_behalf() {
    m(|| {
        let f = fx("erase-behalf");
        let s = f.open("a.idx", &file(TREE));
        let o = f.join(&s, "owner", owner());
        let a = f.join(&s, "alice", Rights::editor());
        f.host
            .rights
            .lock()
            .unwrap()
            .insert("bot".into(), Rights::editor());
        let bot = s
            .join(
                who("bot", ParticipantKind::Agent).on_behalf_of("alice"),
                Stay::Held,
            )
            .unwrap()
            .conn;
        let t = s
            .comment(a, NewComment::new(NewAnchor::Doc, "mine"))
            .unwrap()
            .id;
        let theirs = secret("behalf");
        s.reply(bot, t, theirs.clone(), vec![], vec![]).unwrap();
        s.erase_participant(o, "alice", "erased-2").unwrap();
        assert!(
            !f.holds(&[theirs.as_bytes()]),
            "erasure clears what was written on their behalf"
        );
        let th = s.thread(o, t, None).unwrap();
        assert_eq!(th.comments[1].by.pid, "bot");
        assert_eq!(th.comments[1].by.on_behalf_of.as_deref(), Some("erased-2"));
        assert!(th.comments[1].body.is_none());
    });
}

/// `comments --since head` sees every change, history row or not: an erasure's renames, a
/// version purge's clears, an index record's new anchor state.
#[test]
fn since_sees_kit_side_changes() {
    m(|| {
        let f = fx("since-kit");
        let s = f.open("a.idx", &file(TREE));
        let o = f.join(&s, "owner", owner());
        let a = f.join(&s, "alice", Rights::editor());
        let t = s.comment(a, on_node("@n:2", "logo")).unwrap().id;
        let changed = |head: u64| -> Vec<ThreadId> {
            s.comments(o, &ThreadQuery::new().since(head))
                .unwrap()
                .threads
                .iter()
                .map(|t| t.id)
                .collect()
        };
        // an index record that changes the anchor's state
        let head = s.comments(o, &ThreadQuery::new()).unwrap().head;
        let v1 = s.status().unwrap().version;
        let v2 = s
            .write_bytes(o, &file(&["@n:1|group|Layers|-|1|-|-"]), v1)
            .unwrap()
            .version;
        index(&s);
        assert_eq!(changed(head), [t], "a kit-side change shows to since");
        // a version purge's clears
        let head = s.comments(o, &ThreadQuery::new()).unwrap().head;
        s.purge(o, Some(v1)).unwrap();
        assert_eq!(changed(head), [t], "a kit-side change shows to since");
        // an erasure's renames
        let head = s.comments(o, &ThreadQuery::new()).unwrap().head;
        s.erase_participant(o, "alice", "erased-3").unwrap();
        assert_eq!(changed(head), [t], "a kit-side change shows to since");
        assert!(v2 > v1);
    });
}

/// A reader without history sees no copy of a node from an older version: not the
/// fingerprint or last-seen record taken there, nor a resolution against an older index.
#[test]
fn a_reader_without_history_sees_only_the_current_version() {
    m(|| {
        let f = fx("no-history");
        let s = f.open("a.idx", &file(TREE));
        let o = f.join(&s, "owner", owner());
        let t = s.comment(o, on_node("@n:2", "logo")).unwrap().id;
        let v1 = s.status().unwrap().version;
        // the node renamed in the next version, its index not recorded yet
        s.write_bytes(
            o,
            &file(&[
                "@n:1|group|Layers|-|1|-|-",
                "@n:2|raster|Renamed|0|1|-|logo-pixels|-",
            ]),
            v1,
        )
        .unwrap();
        let r = f.join(&s, "reader", Rights::read_only());
        let th = s.thread(r, t, None).unwrap();
        assert!(
            th.anchor.fingerprint.is_none()
                && th.anchor.last_seen.is_none()
                && th.resolution.is_none(),
            "a reader without history sees only the current version: {:?}",
            th.anchor
        );
        // someone with history sees them
        let full = s.thread(o, t, None).unwrap();
        assert_eq!(full.anchor.fingerprint.unwrap().name, "Logo");
        assert!(!full.resolution.unwrap().current);
        // once the current version is indexed, the reader sees its resolution and record
        index(&s);
        let th = s.thread(r, t, None).unwrap();
        assert_eq!(th.anchor.last_seen.unwrap().name, "Renamed");
        assert!(th.resolution.unwrap().current);
        assert!(th.anchor.fingerprint.is_none());
    });
}

/// A purge's clearing of an anchor's records sticks: the next index record doesn't put a
/// `last_seen` back; a re-anchor does.
#[test]
fn a_purges_clear_sticks_until_a_reanchor() {
    m(|| {
        let f = fx("sticky");
        let s = f.open("a.idx", &file(TREE));
        let o = f.join(&s, "owner", owner());
        let t = s.comment(o, on_node("@n:2", "logo")).unwrap().id;
        s.purge_annotations(o, AnnotPurge::Thread(t)).unwrap();
        let v1 = s.status().unwrap().version;
        s.write_bytes(
            o,
            &file(&["@n:2|raster|Logo again|-|1|-|other-pixels|-"]),
            v1,
        )
        .unwrap();
        index(&s);
        let th = s.thread(o, t, None).unwrap();
        assert!(
            th.anchor.last_seen.is_none() && th.anchor.fingerprint.is_none(),
            "a purge's clear sticks until a re-anchor"
        );
        let Reanchored::Applied(th) = s.reanchor(o, t, NewAnchor::node("@n:2", None)).unwrap()
        else {
            panic!("a person's applies")
        };
        assert_eq!(th.anchor.last_seen.unwrap().name, "Logo again");
    });
}

/// History is bounded per thread: past the cap, only replies, deletes and purges still fit.
#[test]
fn a_threads_history_is_capped() {
    m(|| {
        let f = fx_with("log-cap", |c| c.annot_thread_log_max = 4);
        let s = f.open("a.idx", &file(TREE));
        let a = f.join(&s, "alice", Rights::editor());
        let t = s
            .comment(a, NewComment::new(NewAnchor::Doc, "x"))
            .unwrap()
            .id;
        s.resolve(a, t).unwrap();
        s.reopen(a, t).unwrap();
        s.resolve(a, t).unwrap();
        let e = s.reopen(a, t).unwrap_err();
        assert_eq!(
            e,
            SessionError::AnnotationCap {
                what: "history rows",
                limit: 4
            }
        );
        assert_eq!(e.report().id(), "MPC-E907");
        s.reply(a, t, "still fits".into(), vec![], vec![]).unwrap();
        s.delete_comment(a, t, 1).unwrap();
        let o = f.join(&s, "o", owner());
        assert_eq!(s.annotation_log(o, Some(t), 0, 100).unwrap().len(), 6);
    });
}

/// Mentions are capped before any work, in the verb and in the call.
#[test]
fn mentions_are_capped_early() {
    m(|| {
        let f = fx("mentions");
        let s = f.open("a.idx", &file(TREE));
        let a = f.join(&s, "alice", Rights::editor());
        let many: Vec<String> = (0..33).map(|i| format!("p{i}")).collect();
        let e = s
            .comment(
                a,
                NewComment::new(NewAnchor::Doc, "x").with_mentions(many.clone()),
            )
            .unwrap_err();
        assert_eq!(e.report().id(), "MPC-E321");
        let mut argv: Vec<String> = ["comment", "--doc", "--body", "x"]
            .iter()
            .map(|x| (*x).to_owned())
            .collect();
        for m in &many {
            argv.push("--mention".into());
            argv.push(m.clone());
        }
        assert_eq!(s.run_annotation(a, &argv).unwrap_err().exit(), 2);
        // 32 with repeats, once each
        let some: Vec<String> = (0..32).map(|i| format!("p{}", i % 4)).collect();
        let t = s
            .comment(a, NewComment::new(NewAnchor::Doc, "y").with_mentions(some))
            .unwrap();
        assert_eq!(t.comments[0].mentions, ["p0", "p1", "p2", "p3"]);
    });
}

/// `run_tool` hands the annotation verbs to `run_annotation`.
#[test]
fn run_tool_runs_the_annotation_verbs() {
    m(|| {
        let f = fx("run-tool");
        let s = f.open("a.idx", &file(TREE));
        let a = f.join_as(&s, "bot", ParticipantKind::Agent, Rights::editor());
        let argv = |v: &[&str]| v.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
        let opts = k2_mpc_base::tools::ToolOptions::default();
        let out = s
            .run_tool(
                a,
                &argv(&["comment", "--doc", "--body", "from run_tool"]),
                &opts,
            )
            .unwrap();
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(text.starts_with("ann-1 doc attached open by bot"), "{text}");
        assert!(text.contains("from run_tool"));
        let out = s
            .run_tool(a, &argv(&["comments", "--json"]), &opts)
            .unwrap();
        let json = String::from_utf8(out.stdout).unwrap();
        assert!(json.contains("\"id\":\"ann-1\""), "{json}");
        // and every annotation verb is a run_tool verb in the blob pack's registry
        let reg = Registry::for_blob_pack(&IxPack::default()).unwrap();
        for v in ANNOTATION_VERBS {
            assert!(
                reg.get(v.name).unwrap().runs_through_run_tool(),
                "{}",
                v.name
            );
        }
    });
}
