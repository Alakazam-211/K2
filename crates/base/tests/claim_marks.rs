//! A claim ends `Edited` when its owner's change touching it lands, and only then: a change that
//! lands nothing, or is refused, leaves no mark for the next change by anyone to end it with. The
//! same holds for a hand-off reservation. Public API; the two exits no public call can drive
//! without damaging a store (a later step of a commit, the store's own commit) use
//! `k2_mpc_base::differential::with_apply_fault`.
//!
//! `MPC_STRUCT_MUTANT` plants a fault for the whole suite (`k2_mpc_base::differential`, the
//! `differential` feature, on for these tests):
//! - `grid-mark-before-landing` is `MarkMutant::GridMarkBeforeLanding`, caught by the `grid` tests
//!   of a change that lands nothing or is refused;
//! - `text-mark-first` is `MarkMutant::TextMarkFirst`, caught by the `text` tests of a change that
//!   reaches its apply and lands nothing there;
//! - `recover-keeps-claims` is `MarkMutant::RecoverKeepsClaims`, caught by the tests of a later
//!   step and of the store's commit.
//!
//! Unset, the tests run as is. Any other value stops the suite.

#[path = "grid_pack/mod.rs"]
mod grid_pack;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::differential::{with_apply_fault, with_mark_mutant, ApplyFault, MarkMutant};
use k2_mpc_base::doc::{DocError, Edit, Neutral};
use k2_mpc_base::elem::{Anchor, Key, ListOp, MapOp, OpRefusal};
use k2_mpc_base::schema::{Kind, Schema};
use k2_mpc_base::session::{
    AuditEvent, ClaimEnd, ClaimId, ClaimPolicy, ClaimSpan, Config, ConnId, Declined, EditOptions,
    EditWithError, Host, LineRange, Pack, PackContent, PackError, Participant, ParticipantKind,
    Rights, Session, SessionError, SessionManager, Stay, TextFilePack, UndoOutcome,
};
use k2_mpc_base::store::{DocKey, DocStore, StoreError};
use k2_mpc_base::table::{self, Table, TableError, CELLS, ROWS};
use k2_mpc_base::text::diff::Hunk;

fn under_struct_mutant<R>(f: impl FnOnce() -> R) -> R {
    let m = std::env::var("MPC_STRUCT_MUTANT");
    println!("MPC_STRUCT_MUTANT={}", m.as_deref().unwrap_or("(unset)"));
    match m.as_deref() {
        Ok("grid-mark-before-landing") => with_mark_mutant(MarkMutant::GridMarkBeforeLanding, f),
        Ok("text-mark-first") => with_mark_mutant(MarkMutant::TextMarkFirst, f),
        Ok("recover-keeps-claims") => with_mark_mutant(MarkMutant::RecoverKeepsClaims, f),
        Ok(other) => panic!("MPC_STRUCT_MUTANT={other}: not a mutant this suite plants"),
        Err(_) => f(),
    }
}

// ---------- harness ----------

struct TestHost {
    now: AtomicU64,
    rights: Mutex<HashMap<String, Rights>>,
    events: Mutex<Vec<AuditEvent>>,
}

impl Host for TestHost {
    fn rights(&self, who: &Participant, _: &DocKey) -> Rights {
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

struct TmpDir(std::path::PathBuf);
impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Fx {
    _dir: TmpDir,
    host: Arc<TestHost>,
    _mgr: SessionManager,
    s: Arc<Session>,
}

fn fx(tag: &str, pack: Arc<dyn Pack>, body: &str) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-marks-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_000_000),
        rights: Mutex::new(HashMap::new()),
        events: Mutex::new(Vec::new()),
    });
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mut c = Config::default();
    c.edit_burst = 100_000;
    c.edit_rate_per_s = 100_000;
    let mgr = SessionManager::new(store, host.clone(), c);
    let key = DocKey {
        space: "ws".into(),
        path: "t".into(),
    };
    let s = mgr.open(pack, &key, Some(body.as_bytes())).unwrap();
    Fx {
        _dir: TmpDir(dir),
        host,
        _mgr: mgr,
        s,
    }
}

impl Fx {
    fn person(&self, pid: &str) -> ConnId {
        self.host
            .rights
            .lock()
            .unwrap()
            .insert(pid.into(), Rights::full());
        self.s
            .join(
                Participant::new(
                    pid,
                    pid.to_uppercase(),
                    "#123456",
                    ParticipantKind::Person,
                    "t",
                ),
                Stay::Held,
            )
            .unwrap()
            .conn
    }
    fn version(&self) -> u64 {
        self.s.status().unwrap().version
    }
    fn ended(&self, id: ClaimId) -> Vec<ClaimEnd> {
        self.host
            .events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                AuditEvent::ClaimEnded { claim, how, .. } if *claim == id => Some(*how),
                _ => None,
            })
            .collect()
    }
    /// Who holds the doc's one reservation.
    fn holder(&self, c: ConnId) -> String {
        self.s.reservations(c).unwrap()[0].pid.clone()
    }
    fn bytes(&self, c: ConnId) -> String {
        String::from_utf8(self.s.read(c).unwrap().bytes).unwrap()
    }
}

fn is_planted_store_failure(e: &SessionError) -> bool {
    matches!(e, SessionError::Store(StoreError::Db(m)) if m == "a planted failure")
}

fn is_planted_refusal(e: &SessionError) -> bool {
    matches!(e, SessionError::BadEdit(m) if m == "a planted refusal")
}

// ---------- grids ----------

mod grid {
    use super::*;

    /// header + r1..r4
    const GRID: &str = "id,city\nk1,Oslo\nk2,Rome\nk3,Lima\nk4,Kyiv\n";

    /// The cell c2 of r1, by key.
    fn r1_c2() -> Vec<Edit> {
        vec![Edit::Map {
            root: CELLS.into(),
            ops: vec![MapOp::Delete {
                key: table::cell_key(&k("r1"), &k("c2")),
            }],
        }]
    }

    /// The grid with c2 of r1 deleted, so deleting it again does nothing.
    pub(super) fn gx(tag: &str) -> Fx {
        let f = fx(tag, Arc::new(grid_pack::GridLite), GRID);
        let c = f.person("setup");
        let v = f.version();
        raw(&f, c, r1_c2()).unwrap();
        assert!(f.version() > v, "the first delete lands");
        f.s.leave(c).unwrap();
        f
    }

    pub(super) fn k(s: &str) -> Key {
        Key::new(s).unwrap()
    }

    fn rows(a: &str, b: &str) -> ClaimSpan {
        ClaimSpan::Rows {
            first: k(a),
            last: k(b),
        }
    }

    pub(super) fn op(
        f: &Fx,
        c: ConnId,
        g: impl FnOnce(&mut Table) -> Result<Vec<Edit>, TableError>,
    ) -> Result<(), SessionError> {
        match f.s.edit_ops_with(c, &EditOptions::new(), |v| {
            let mut t = v.table().map_err(|e| e.to_string())?;
            g(&mut t).map_err(|e| e.to_string())
        }) {
            Ok(_) => Ok(()),
            Err(EditWithError::Session(e)) => Err(e),
            Err(EditWithError::Compute(e)) => panic!("table op: {e}"),
        }
    }

    fn raw(f: &Fx, c: ConnId, edits: Vec<Edit>) -> Result<(), SessionError> {
        f.s.edit_ops(c, edits, &EditOptions::new()).map(|_| ())
    }

    fn list(op: ListOp) -> Vec<Edit> {
        vec![Edit::List {
            root: ROWS.into(),
            ops: vec![op],
        }]
    }

    /// Restoring a live row, and deleting a cell that isn't there: each changes nothing.
    fn changes_that_land_nothing() -> Vec<Vec<Edit>> {
        vec![list(ListOp::Restore { key: k("r1") }), r1_c2()]
    }

    /// The doc refuses a move next to itself, past every check before the apply.
    fn refused() -> Vec<Edit> {
        list(ListOp::Move {
            key: k("r1"),
            to: Anchor::After(k("r1")),
        })
    }

    fn assert_refused_by_the_doc(r: Result<(), SessionError>) {
        let e = r.expect_err("the doc refuses the change");
        assert!(
            matches!(
                &e,
                SessionError::Doc(DocError::Op {
                    refusal: OpRefusal::SelfAnchor(_),
                    ..
                })
            ),
            "refused at the apply: {e:?}"
        );
    }

    #[test]
    fn a_change_that_lands_nothing_leaves_the_claim_alone() {
        under_struct_mutant(|| {
            for (n, edits) in changes_that_land_nothing().into_iter().enumerate() {
                let f = gx("g-nothing");
                let p = f.person("p");
                let other = f.person("other");
                let id =
                    f.s.claim_span(p, rows("r1", "r1"), None, None, false)
                        .unwrap()
                        .id;
                let v = f.version();
                raw(&f, p, edits).unwrap();
                assert_eq!(f.version(), v, "change {n} landed nothing");
                assert_eq!(f.ended(id), [], "ended by its own change {n}");
                op(&f, other, |t| t.delete_row(&k("r3"))).unwrap();
                assert_eq!(
                    f.ended(id),
                    [],
                    "change {n}: someone else's change ended it"
                );
            }
        });
    }

    #[test]
    fn a_refused_change_leaves_the_claim_alone() {
        under_struct_mutant(|| {
            let f = gx("g-refused");
            let p = f.person("p");
            let other = f.person("other");
            let id =
                f.s.claim_span(p, rows("r1", "r1"), None, None, false)
                    .unwrap()
                    .id;
            let v = f.version();
            assert_refused_by_the_doc(raw(&f, p, refused()));
            assert_eq!(f.version(), v, "nothing applied");
            op(&f, other, |t| t.delete_row(&k("r3"))).unwrap();
            assert_eq!(f.ended(id), [], "someone else's change ended it");
        });
    }

    /// `p` holds a reservation `w` should keep; `next` waits behind.
    fn reserved_for_w(tag: &str) -> (Fx, ConnId, ConnId, ConnId) {
        let f = gx(tag);
        let owner = f.person("owner");
        let w = f.person("w");
        let next = f.person("next");
        let other = f.person("other");
        let id =
            f.s.claim_span(owner, rows("r1", "r1"), None, None, false)
                .unwrap()
                .id;
        f.s.register_waiter(w, id).unwrap();
        f.s.register_waiter(next, id).unwrap();
        f.s.release_claim(owner, Some(id)).unwrap();
        assert_eq!(f.holder(other), "w");
        (f, w, next, other)
    }

    #[test]
    fn a_change_that_lands_nothing_leaves_the_reservation_alone() {
        under_struct_mutant(|| {
            for (n, edits) in changes_that_land_nothing().into_iter().enumerate() {
                let (f, w, _next, other) = reserved_for_w("g-res-nothing");
                let v = f.version();
                raw(&f, w, edits).unwrap();
                assert_eq!(f.version(), v, "change {n} landed nothing");
                assert_eq!(f.holder(other), "w", "change {n}: used by its own");
                op(&f, other, |t| t.delete_row(&k("r3"))).unwrap();
                assert_eq!(f.holder(other), "w", "change {n}: someone else's used it");
            }
        });
    }

    #[test]
    fn a_refused_change_leaves_the_reservation_alone() {
        under_struct_mutant(|| {
            let (f, w, _next, other) = reserved_for_w("g-res-refused");
            assert_refused_by_the_doc(raw(&f, w, refused()));
            op(&f, other, |t| t.delete_row(&k("r3"))).unwrap();
            assert_eq!(f.holder(other), "w", "someone else's change used it");
        });
    }

    /// A store commit that fails reloads the doc: the claim ends `Reset`, once, whatever the change
    /// touched, and the next change can't end it `Edited`.
    #[test]
    fn a_store_commit_that_fails_resets_the_claim() {
        under_struct_mutant(|| {
            let f = gx("g-store");
            let p = f.person("p");
            let other = f.person("other");
            let id =
                f.s.claim_span(p, rows("r1", "r1"), None, None, false)
                    .unwrap()
                    .id;
            let before = f.bytes(p);
            let e = with_apply_fault(ApplyFault::StoreCommitFails, || {
                op(&f, p, |t| t.set_cell(&k("r1"), &k("c2"), "X"))
            })
            .expect_err("the commit fails");
            assert!(is_planted_store_failure(&e), "{e:?}");
            assert_eq!(f.ended(id), [ClaimEnd::Reset], "ended by the reload");
            assert_eq!(f.bytes(p), before, "the doc is as the store has it");
            op(&f, other, |t| t.delete_row(&k("r3"))).unwrap();
            assert_eq!(f.ended(id), [ClaimEnd::Reset], "ended once");
        });
    }

    /// The same when a later step of an undo fails, after an earlier one touched the claim.
    #[test]
    fn a_later_step_that_fails_resets_the_claim() {
        under_struct_mutant(|| {
            let f = gx("g-step");
            let p = f.person("p");
            let other = f.person("other");
            op(&f, p, |t| t.set_cell(&k("r1"), &k("c1"), "A")).unwrap();
            op(&f, p, |t| t.set_cell(&k("r1"), &k("c2"), "B")).unwrap();
            let id =
                f.s.claim_span(p, rows("r1", "r1"), None, None, false)
                    .unwrap()
                    .id;
            let before = f.bytes(p);
            let e = with_apply_fault(ApplyFault::LaterStepRefused, || f.s.undo(p, 2))
                .expect_err("the second step fails");
            assert!(is_planted_refusal(&e), "{e:?}");
            assert_eq!(f.ended(id), [ClaimEnd::Reset], "ended by the reload");
            assert_eq!(f.bytes(p), before, "the doc is as the store has it");
            op(&f, other, |t| t.delete_row(&k("r3"))).unwrap();
            assert_eq!(f.ended(id), [ClaimEnd::Reset], "ended once");
        });
    }

    /// An op on the claimed row with no effect, beside one that lands elsewhere: the change landed,
    /// and it didn't touch the claim.
    fn no_effect_beside_one_that_lands() -> Vec<Edit> {
        vec![Edit::List {
            root: ROWS.into(),
            ops: vec![
                ListOp::Restore { key: k("r1") },
                ListOp::Delete { key: k("r3") },
            ],
        }]
    }

    #[test]
    fn an_op_with_no_effect_beside_one_that_lands_leaves_the_claim_alone() {
        under_struct_mutant(|| {
            let f = gx("g-beside-op");
            let p = f.person("p");
            let other = f.person("other");
            let id =
                f.s.claim_span(p, rows("r1", "r1"), None, None, false)
                    .unwrap()
                    .id;
            let v = f.version();
            raw(&f, p, no_effect_beside_one_that_lands()).unwrap();
            assert!(f.version() > v, "it landed");
            assert_eq!(f.ended(id), [], "ended by its own op with no effect");
            op(&f, other, |t| t.delete_row(&k("r4"))).unwrap();
            assert_eq!(f.ended(id), [], "someone else's change ended it");
        });
    }

    #[test]
    fn an_op_with_no_effect_beside_one_that_lands_leaves_the_reservation_alone() {
        under_struct_mutant(|| {
            let (f, w, _next, other) = reserved_for_w("g-res-beside-op");
            let v = f.version();
            raw(&f, w, no_effect_beside_one_that_lands()).unwrap();
            assert!(f.version() > v, "it landed");
            assert_eq!(f.holder(other), "w", "used by its own op with no effect");
        });
    }

    /// The claim policy judges what a change names, before it applies: a change the claim would
    /// refuse is refused, even if its op there would have no effect.
    #[test]
    fn the_claim_policy_still_judges_what_a_change_names() {
        under_struct_mutant(|| {
            let f = gx("g-policy");
            let owner = f.person("owner");
            f.host
                .rights
                .lock()
                .unwrap()
                .insert("bot".into(), Rights::full());
            let bot =
                f.s.join(
                    Participant::new("bot", "BOT", "#123456", ParticipantKind::Agent, "t"),
                    Stay::Held,
                )
                .unwrap()
                .conn;
            f.s.claim_span(owner, rows("r1", "r1"), None, None, false)
                .unwrap();
            let opts = EditOptions::new()
                .policy(ClaimPolicy::Respect)
                .if_version(f.version());
            let e =
                f.s.edit_ops(bot, list(ListOp::Restore { key: k("r1") }), &opts)
                    .expect_err("the claim refuses it");
            assert!(matches!(e, SessionError::Claimed { .. }), "{e:?}");
        });
    }

    // ---- the controls: what a change that lands still does ----

    #[test]
    fn a_change_that_lands_ends_the_claim_as_edited() {
        under_struct_mutant(|| {
            let f = gx("g-lands");
            let p = f.person("p");
            let id =
                f.s.claim_span(p, rows("r1", "r1"), None, None, false)
                    .unwrap()
                    .id;
            op(&f, p, |t| t.set_cell(&k("r1"), &k("c2"), "X")).unwrap();
            assert_eq!(f.ended(id), [ClaimEnd::Edited]);
        });
    }

    #[test]
    fn a_change_that_lands_leaves_a_held_claim_alone() {
        under_struct_mutant(|| {
            let f = gx("g-held");
            let p = f.person("p");
            let id =
                f.s.claim_span(p, rows("r1", "r1"), None, None, true)
                    .unwrap()
                    .id;
            op(&f, p, |t| t.set_cell(&k("r1"), &k("c2"), "X")).unwrap();
            assert_eq!(f.ended(id), []);
            assert_eq!(f.s.claims(p).unwrap().len(), 1);
        });
    }

    #[test]
    fn a_change_that_lands_uses_the_reservation() {
        under_struct_mutant(|| {
            let (f, w, _next, other) = reserved_for_w("g-res-lands");
            op(&f, w, |t| t.set_cell(&k("r1"), &k("c2"), "X")).unwrap();
            assert_eq!(f.holder(other), "next", "passed on");
        });
    }

    /// Someone else's change beside the claim never ends it, so the tests above can see a mark.
    #[test]
    fn someone_elses_change_beside_the_claim_leaves_it_alone() {
        under_struct_mutant(|| {
            let f = gx("g-beside");
            let p = f.person("p");
            let other = f.person("other");
            let id =
                f.s.claim_span(p, rows("r1", "r1"), None, None, false)
                    .unwrap()
                    .id;
            op(&f, other, |t| t.delete_row(&k("r3"))).unwrap();
            assert_eq!(f.ended(id), []);
        });
    }
}

// ---------- text ----------

mod text {
    use super::*;

    const TEXT: TextFilePack = TextFilePack::new("marks", 1 << 20);
    const ROOT: &str = TextFilePack::ROOT;
    const DOC: &str = "one\ntwo\nthree\nfour\nfive\n";

    fn tx(tag: &str) -> Fx {
        fx(tag, Arc::new(TEXT), DOC)
    }

    fn lines(root: &str, first: usize, last: usize) -> LineRange {
        LineRange {
            root: root.into(),
            first,
            last,
        }
    }

    fn edit(root: &str, start: usize, end: usize, insert: &str) -> Edit {
        Edit::Text {
            root: root.into(),
            hunks: vec![Hunk {
                start,
                end,
                insert: insert.into(),
            }],
        }
    }

    /// Runs past the end of the text, over the claimed lines at its start.
    fn bad_hunk(root: &str) -> Edit {
        edit(root, 0, 1_000_000, "x")
    }

    /// Someone else's change on line 5 of `DOC`: a change that ends a mark left behind.
    fn later_change(f: &Fx, c: ConnId) {
        f.s.edit(
            c,
            vec![edit(ROOT, DOC.len() - 5, DOC.len(), "FIVE\n")],
            None,
        )
        .unwrap();
    }

    fn assert_refused_hunk(r: Result<u64, SessionError>) {
        let e = r.expect_err("the hunk is refused");
        assert!(
            matches!(&e, SessionError::Doc(DocError::Hunk(_))),
            "refused for its hunk: {e:?}"
        );
    }

    #[test]
    fn a_refused_hunk_leaves_the_claim_alone() {
        under_struct_mutant(|| {
            let f = tx("t-refused");
            let p = f.person("p");
            let other = f.person("other");
            let id =
                f.s.claim(p, lines(ROOT, 1, 2), None, None, false)
                    .unwrap()
                    .id;
            let v = f.version();
            assert_refused_hunk(f.s.edit(p, vec![bad_hunk(ROOT)], None).map(|r| r.version));
            assert_eq!(f.version(), v, "nothing applied");
            later_change(&f, other);
            assert_eq!(f.ended(id), [], "someone else's change ended it");
        });
    }

    /// No hunks reaches the commit and its apply, which return nothing.
    #[test]
    fn an_edit_with_no_hunks_leaves_the_claim_alone() {
        under_struct_mutant(|| {
            let f = tx("t-empty");
            let p = f.person("p");
            let other = f.person("other");
            let id =
                f.s.claim(p, lines(ROOT, 1, 2), None, None, false)
                    .unwrap()
                    .id;
            let v = f.version();
            f.s.edit(
                p,
                vec![Edit::Text {
                    root: ROOT.into(),
                    hunks: Vec::new(),
                }],
                None,
            )
            .unwrap();
            assert_eq!(f.version(), v, "it landed nothing");
            later_change(&f, other);
            assert_eq!(f.ended(id), [], "someone else's change ended it");
        });
    }

    /// Writing the text the doc already holds lands nothing, whichever call writes it; `write_base`
    /// reaches the apply with no hunks.
    #[test]
    fn writing_the_same_text_leaves_the_claim_alone() {
        under_struct_mutant(|| {
            let f = tx("t-same");
            let p = f.person("p");
            let other = f.person("other");
            let id =
                f.s.claim(p, lines(ROOT, 1, 2), None, None, false)
                    .unwrap()
                    .id;
            let v = f.version();
            f.s.edit_with(p, ROOT, |t| Ok::<_, ()>(t.to_owned()))
                .unwrap();
            assert_eq!(f.version(), v, "edit_with landed nothing");
            f.s.write_base(p, ROOT, v, DOC).unwrap();
            assert_eq!(f.version(), v, "write_base landed nothing");
            later_change(&f, other);
            assert_eq!(f.ended(id), [], "someone else's change ended it");
        });
    }

    #[test]
    fn a_refused_hunk_leaves_the_reservation_alone() {
        under_struct_mutant(|| {
            let f = tx("t-res");
            let owner = f.person("owner");
            let w = f.person("w");
            let next = f.person("next");
            let other = f.person("other");
            let id =
                f.s.claim(owner, lines(ROOT, 1, 2), None, None, false)
                    .unwrap()
                    .id;
            f.s.register_waiter(w, id).unwrap();
            f.s.register_waiter(next, id).unwrap();
            f.s.release_claim(owner, Some(id)).unwrap();
            assert_eq!(f.holder(other), "w");
            assert_refused_hunk(f.s.edit(w, vec![bad_hunk(ROOT)], None).map(|r| r.version));
            later_change(&f, other);
            assert_eq!(f.holder(other), "w", "someone else's change used it");
            // and its own edit there uses it
            f.s.edit(w, vec![edit(ROOT, 0, 3, "ONE")], None).unwrap();
            assert_eq!(f.holder(other), "next", "passed on");
        });
    }

    // ---- a doc with two text roots: one edit, one root at a time ----

    struct TwoRoots;
    const TWO: Schema = Schema {
        name: "two-roots",
        roots: &[("a", Kind::Text), ("b", Kind::Text)],
    };
    impl Pack for TwoRoots {
        fn schema(&self) -> Schema {
            TWO
        }
        fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
            let s = std::str::from_utf8(b).unwrap();
            let (a, b) = s.split_once('|').unwrap();
            Ok(PackContent {
                roots: vec![
                    ("a".into(), Neutral::Text(a.into())),
                    ("b".into(), Neutral::Text(b.into())),
                ],
                codec: Vec::new(),
            })
        }
        fn encode(&self, c: &[(String, Neutral)], _: &[u8]) -> Result<Vec<u8>, PackError> {
            let t = |i: usize| match &c[i].1 {
                Neutral::Text(t) => t.clone(),
                _ => unreachable!("text roots"),
            };
            Ok(format!("{}|{}", t(0), t(1)).into_bytes())
        }
    }

    const TWO_DOC: &str = "a1\na2\n|b1\nb2\n";

    #[test]
    fn a_later_root_refused_leaves_the_claim_on_the_first_alone() {
        under_struct_mutant(|| {
            let f = fx("t-roots", Arc::new(TwoRoots), TWO_DOC);
            let p = f.person("p");
            let other = f.person("other");
            let id =
                f.s.claim(p, lines("a", 1, 1), None, None, false)
                    .unwrap()
                    .id;
            let v = f.version();
            assert_refused_hunk(
                f.s.edit(p, vec![edit("a", 0, 2, "A1"), bad_hunk("b")], None)
                    .map(|r| r.version),
            );
            assert_eq!(f.version(), v, "nothing applied");
            assert_eq!(f.bytes(p), TWO_DOC, "not even the first root");
            f.s.edit(other, vec![edit("b", 0, 2, "B1")], None).unwrap();
            assert_eq!(f.ended(id), [], "someone else's change ended it");
        });
    }

    /// A root with no hunks, beside one that lands: its claim isn't touched.
    #[test]
    fn a_root_with_no_hunks_beside_one_that_lands_leaves_its_claim_alone() {
        under_struct_mutant(|| {
            let f = fx("t-noop-root", Arc::new(TwoRoots), TWO_DOC);
            let p = f.person("p");
            let id =
                f.s.claim(p, lines("a", 1, 1), None, None, false)
                    .unwrap()
                    .id;
            let v = f.version();
            f.s.edit(
                p,
                vec![
                    Edit::Text {
                        root: "a".into(),
                        hunks: Vec::new(),
                    },
                    edit("b", 0, 2, "B1"),
                ],
                None,
            )
            .unwrap();
            assert!(f.version() > v, "it landed");
            assert_eq!(f.ended(id), [], "its claim on the other root ended");
        });
    }

    // ---- the exits after the apply: every mark is gone with the reload ----

    #[test]
    fn a_store_commit_that_fails_resets_the_claim() {
        under_struct_mutant(|| {
            let f = tx("t-store");
            let p = f.person("p");
            let other = f.person("other");
            let id =
                f.s.claim(p, lines(ROOT, 1, 2), None, None, false)
                    .unwrap()
                    .id;
            let e = with_apply_fault(ApplyFault::StoreCommitFails, || {
                f.s.edit(p, vec![edit(ROOT, 0, 3, "ONE")], None)
            })
            .expect_err("the commit fails");
            assert!(is_planted_store_failure(&e), "{e:?}");
            assert_eq!(f.ended(id), [ClaimEnd::Reset], "ended by the reload");
            assert_eq!(f.bytes(p), DOC, "the doc is as the store has it");
            later_change(&f, other);
            assert_eq!(f.ended(id), [ClaimEnd::Reset], "ended once");
        });
    }

    #[test]
    fn a_later_step_that_fails_resets_the_claim() {
        under_struct_mutant(|| {
            let f = tx("t-step");
            let p = f.person("p");
            let other = f.person("other");
            f.s.edit(p, vec![edit(ROOT, 0, 3, "ONE")], None).unwrap();
            f.s.edit(p, vec![edit(ROOT, 4, 7, "TWO")], None).unwrap();
            let before = f.bytes(p);
            let id =
                f.s.claim(p, lines(ROOT, 1, 2), None, None, false)
                    .unwrap()
                    .id;
            let e = with_apply_fault(ApplyFault::LaterStepRefused, || f.s.undo(p, 2))
                .expect_err("the second step fails");
            assert!(is_planted_refusal(&e), "{e:?}");
            assert_eq!(f.ended(id), [ClaimEnd::Reset], "ended by the reload");
            assert_eq!(f.bytes(p), before, "the doc is as the store has it");
            f.s.edit(
                other,
                vec![edit(ROOT, before.len() - 5, before.len(), "FIVE\n")],
                None,
            )
            .unwrap();
            assert_eq!(f.ended(id), [ClaimEnd::Reset], "ended once");
        });
    }

    /// An open undo group outlives a reload (`recover` clears the stacks, not the group), and joins
    /// nothing: the next grouped edit is a step of its own.
    #[test]
    fn an_open_undo_group_joins_nothing_after_a_failed_commit() {
        under_struct_mutant(|| {
            let f = tx("t-group");
            let p = f.person("p");
            let opts = EditOptions::new().group("input-1");
            let ins = |at: usize, s: &'static str| {
                f.s.edit_with_options(p, ROOT, &opts, move |t| {
                    Ok::<_, ()>(format!("{}{s}{}", &t[..at], &t[at..]))
                })
            };
            ins(0, "a").unwrap();
            let e = with_apply_fault(ApplyFault::StoreCommitFails, || ins(1, "b"));
            let Err(EditWithError::Session(e)) = e else {
                panic!("the commit fails: {e:?}")
            };
            assert!(is_planted_store_failure(&e), "{e:?}");
            ins(1, "c").unwrap();
            assert_eq!(f.bytes(p), format!("ac{DOC}"));
            let u = f.s.undo(p, 1).unwrap();
            assert!(matches!(u, UndoOutcome::Applied { steps: 1, .. }), "{u:?}");
            assert_eq!(
                f.bytes(p),
                format!("a{DOC}"),
                "only the edit after the reload"
            );
        });
    }

    /// The same edits with no failed commit between them are one step, so the test above can tell.
    #[test]
    fn grouped_edits_with_no_failed_commit_are_one_undo_step() {
        under_struct_mutant(|| {
            let f = tx("t-group-control");
            let p = f.person("p");
            let opts = EditOptions::new().group("input-1");
            for (at, s) in [(0, "a"), (1, "c")] {
                f.s.edit_with_options(p, ROOT, &opts, move |t| {
                    Ok::<_, ()>(format!("{}{s}{}", &t[..at], &t[at..]))
                })
                .unwrap();
            }
            assert_eq!(f.bytes(p), format!("ac{DOC}"));
            let u = f.s.undo(p, 1).unwrap();
            assert!(matches!(u, UndoOutcome::Applied { steps: 1, .. }), "{u:?}");
            assert_eq!(f.bytes(p), DOC, "both edits in the one step");
        });
    }

    // ---- an edit that landed is never reported as failed ----

    /// A text pack whose `encode` can be made to fail.
    struct FailingEncode(Arc<AtomicBool>);
    impl Pack for FailingEncode {
        fn schema(&self) -> Schema {
            TEXT.schema()
        }
        fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
            TEXT.decode(b)
        }
        fn encode(&self, c: &[(String, Neutral)], codec: &[u8]) -> Result<Vec<u8>, PackError> {
            if self.0.load(Ordering::SeqCst) {
                return Err(PackError("a planted failure".into()));
            }
            TEXT.encode(c, codec)
        }
    }

    /// The doc's replica is re-synced for grids only, so nothing after a text edit's store commit
    /// renders the doc: an edit that landed returns `Ok`, and it can be undone.
    #[test]
    fn a_failing_encode_does_not_fail_an_edit_that_landed() {
        under_struct_mutant(|| {
            let root =
                std::env::temp_dir().join(format!("k2mb-marks-encode-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            let _dir = TmpDir(root.clone());
            let host = Arc::new(TestHost {
                now: AtomicU64::new(1_000_000),
                rights: Mutex::new(HashMap::new()),
                events: Mutex::new(Vec::new()),
            });
            host.rights
                .lock()
                .unwrap()
                .insert("p".into(), Rights::full());
            let store = Arc::new(DocStore::open(root.join("docs.redb")).unwrap());
            let mut c = Config::default();
            c.edit_burst = 100_000;
            c.edit_rate_per_s = 100_000;
            let mgr = SessionManager::new(store, host, c);
            let files = root.join("files");
            std::fs::create_dir_all(&files).unwrap();
            let file = files.join("t.md");
            std::fs::write(&file, DOC).unwrap();
            let fail = Arc::new(AtomicBool::new(false));
            let key = DocKey {
                space: "ws".into(),
                path: "t.md".into(),
            };
            let s = mgr
                .open_file(Arc::new(FailingEncode(fail.clone())), &key, &file, &files)
                .unwrap();
            let p = s
                .join(
                    Participant::new("p", "P", "#123456", ParticipantKind::Person, "t"),
                    Stay::Held,
                )
                .unwrap()
                .conn;
            let v = s.status().unwrap().version;
            fail.store(true, Ordering::SeqCst);
            let r = s.edit(p, vec![edit(ROOT, 0, 3, "ONE")], None);
            assert!(r.is_ok(), "told it failed: {r:?}");
            assert!(s.status().unwrap().version > v, "it landed");
            let u = s.undo(p, 1).unwrap();
            assert!(
                matches!(u, UndoOutcome::Applied { .. }),
                "on the stack: {u:?}"
            );
            fail.store(false, Ordering::SeqCst);
        });
    }

    // ---- the controls ----

    #[test]
    fn a_change_that_lands_ends_the_claim_as_edited() {
        under_struct_mutant(|| {
            let f = tx("t-lands");
            let p = f.person("p");
            let id =
                f.s.claim(p, lines(ROOT, 1, 2), None, None, false)
                    .unwrap()
                    .id;
            f.s.edit(p, vec![edit(ROOT, 0, 3, "ONE")], None).unwrap();
            assert_eq!(f.ended(id), [ClaimEnd::Edited]);
        });
    }

    #[test]
    fn a_change_that_lands_leaves_a_held_claim_alone() {
        under_struct_mutant(|| {
            let f = tx("t-held");
            let p = f.person("p");
            let id =
                f.s.claim(p, lines(ROOT, 1, 2), None, None, true)
                    .unwrap()
                    .id;
            f.s.edit(p, vec![edit(ROOT, 0, 3, "ONE")], None).unwrap();
            assert_eq!(f.ended(id), []);
        });
    }

    #[test]
    fn someone_elses_change_beside_the_claim_leaves_it_alone() {
        under_struct_mutant(|| {
            let f = tx("t-beside");
            let p = f.person("p");
            let other = f.person("other");
            let id =
                f.s.claim(p, lines(ROOT, 1, 2), None, None, false)
                    .unwrap()
                    .id;
            later_change(&f, other);
            assert_eq!(f.ended(id), []);
        });
    }
}
