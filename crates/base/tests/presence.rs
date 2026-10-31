//! Agent presence (amendment 13): claims and their ends, waiters, the claim policy, `--for-edit`,
//! heads-up notes, the activity ring, richer roster rows, conflict reports, and the rule that
//! none of it ever reaches the store. Public API only.
//!
//! On a grid (A10, K-PRES-6, CSV contract §6), with the kit's test table pack
//! (`tests/grid_pack`): carets and claims by key follow moves and sorts; claims expand at
//! their ends and end `Deleted` when nothing they cover is live; the claim policy and the
//! notes judge cells, rows and columns; `write --base` conflicts name the claimants.

#[path = "grid_pack/mod.rs"]
mod grid_pack;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::parity::{PersonAction, Registry};
use k2_mpc_base::session::{
    ActivityKind, AuditEvent, ClaimEnd, ClaimId, ClaimPolicy, Config, ConnId, EditWithError,
    ForEdit, Host, LineRange, NoteKind, Notes, Participant, ParticipantKind, Rights, Session,
    SessionError, SessionManager, Stay, TextFilePack, WriteOutcome,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::tools::{self, exit, ToolOptions, ToolRunError};

const ROOT: &str = TextFilePack::ROOT;
const DOC: &str = "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\n";

// ---------- harness ----------

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
            .unwrap_or(Rights::editor())
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
    dir: TmpDir,
    host: Arc<TestHost>,
    mgr: SessionManager,
    s: Arc<Session>,
}

fn fx_with(tag: &str, config: Config) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-presence-{tag}-{}-{}",
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
    let mgr = SessionManager::new(store, host.clone(), config);
    let s = mgr
        .open(
            Arc::new(TextFilePack::new("markdown", 4 << 20)),
            &DocKey {
                space: "ws".into(),
                path: "notes.md".into(),
            },
            Some(DOC.as_bytes()),
        )
        .unwrap();
    Fx {
        dir: TmpDir(dir),
        host,
        mgr,
        s,
    }
}

fn fx(tag: &str) -> Fx {
    fx_with(tag, Config::default())
}

impl Fx {
    fn join(&self, pid: &str, kind: ParticipantKind) -> ConnId {
        let p = Participant::new(pid, pid.to_uppercase(), "#123456", kind, "cli");
        self.s.join(p, Stay::Held).unwrap().conn
    }
    fn agent(&self, pid: &str) -> ConnId {
        self.join(pid, ParticipantKind::Agent)
    }
    fn person(&self, pid: &str) -> ConnId {
        self.join(pid, ParticipantKind::Person)
    }
    fn advance(&self, ms: u64) {
        self.host.now.fetch_add(ms, Ordering::SeqCst);
    }
    fn text(&self, c: ConnId) -> String {
        self.s.read(c).unwrap().text_by_root.remove(0).1
    }
    fn claim(&self, c: ConnId, first: usize, last: usize) -> ClaimId {
        self.s
            .claim(c, lines(first, last), None, None, false)
            .unwrap()
            .id
    }
    fn tool(&self, c: ConnId, argv: &[&str]) -> tools::ToolOutput {
        self.tool_with(c, argv, ClaimPolicy::Ignore).unwrap()
    }
    fn tool_with(
        &self,
        c: ConnId,
        argv: &[&str],
        policy: ClaimPolicy,
    ) -> Result<tools::ToolOutput, ToolRunError> {
        let argv: Vec<String> = argv.iter().map(|a| (*a).to_owned()).collect();
        self.s
            .run_tool_with(c, ROOT, &argv, b"", &ToolOptions::new().policy(policy))
    }
    /// `ClaimEnded` events so far: (claim, how, waiters in order).
    fn ended(&self) -> Vec<(ClaimId, ClaimEnd, Vec<String>)> {
        self.host
            .events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                AuditEvent::ClaimEnded {
                    claim,
                    how,
                    waiters,
                    next_waiter,
                    ..
                } => {
                    assert_eq!(
                        next_waiter.as_ref(),
                        waiters.first(),
                        "next_waiter is the first waiter"
                    );
                    Some((*claim, *how, waiters.iter().map(|w| w.1.clone()).collect()))
                }
                _ => None,
            })
            .collect()
    }
    fn end_of(&self, id: ClaimId) -> Option<ClaimEnd> {
        self.ended()
            .into_iter()
            .find(|(c, _, _)| *c == id)
            .map(|e| e.1)
    }
}

fn lines(first: usize, last: usize) -> LineRange {
    LineRange {
        root: ROOT.into(),
        first,
        last,
    }
}

/// Replace line `n` (1-based) with `with`.
fn set_line(text: &str, n: usize, with: &str) -> String {
    text.split_inclusive('\n')
        .enumerate()
        .map(|(i, l)| {
            if i + 1 == n {
                format!("{with}\n")
            } else {
                l.to_owned()
            }
        })
        .collect()
}

fn edit_line(
    f: &Fx,
    c: ConnId,
    n: usize,
    with: &str,
    policy: ClaimPolicy,
) -> Result<(), SessionError> {
    f.s.edit_with_policy(c, ROOT, policy, |t| Ok::<_, ()>(set_line(t, n, with)))
        .map(|_| ())
        .map_err(|e| match e {
            EditWithError::Session(e) => e,
            EditWithError::Compute(()) => unreachable!(),
        })
}

// ---------- claims ----------

#[test]
fn claim_lifecycle_all_four_ends() {
    let f = fx("life");
    let a = f.agent("a");
    let obs = f.agent("obs");

    // 1. released
    let id = f.claim(a, 2, 3);
    assert_eq!(f.s.release_claim(a, Some(id)).unwrap(), vec![id]);
    assert_eq!(f.end_of(id), Some(ClaimEnd::Released));
    assert!(
        f.s.release_claim(a, Some(id)).unwrap().is_empty(),
        "twice: none"
    );

    // 2. edited: the owner's own edit touching it ends it (no hold) ...
    let id = f.claim(a, 2, 3);
    let other = f.claim(a, 8, 8);
    edit_line(&f, a, 3, "THREE", ClaimPolicy::Ignore).unwrap();
    assert_eq!(f.end_of(id), Some(ClaimEnd::Edited));
    assert_eq!(f.end_of(other), None, "an untouched claim stays");
    // ... but not someone else's edit
    let b = f.agent("b");
    edit_line(&f, b, 8, "EIGHT", ClaimPolicy::Ignore).unwrap();
    assert_eq!(f.end_of(other), None, "another's edit doesn't end it");
    f.s.release_claim(a, None).unwrap();

    // 3. expired: on tick (expire_idle) ...
    let id = f.claim(a, 5, 5);
    f.advance(29_999);
    f.s.expire_idle().unwrap();
    assert_eq!(f.end_of(id), None, "not yet");
    f.advance(1);
    f.s.expire_idle().unwrap();
    assert_eq!(f.end_of(id), Some(ClaimEnd::Expired));
    // ... and lazily on access
    let id = f.claim(a, 5, 5);
    f.advance(30_000);
    assert!(f.s.claims(obs).unwrap().is_empty());
    assert_eq!(f.end_of(id), Some(ClaimEnd::Expired));

    // 4. left: the connection that took it leaves (or is kicked)
    let id = f.claim(a, 6, 7);
    f.s.leave(a).unwrap();
    assert_eq!(f.end_of(id), Some(ClaimEnd::Left));
    let k = f.agent("k");
    let id = f.claim(k, 6, 7);
    f.s.kick("k").unwrap();
    assert_eq!(f.end_of(id), Some(ClaimEnd::Left));
    assert!(f.s.claims(obs).unwrap().is_empty());

    // every claim ever taken ended exactly once
    let mut ids: Vec<ClaimId> = f.ended().into_iter().map(|e| e.0).collect();
    let n = ids.len();
    ids.dedup();
    assert_eq!(ids.len(), n, "each claim ends once");
    let taken = f
        .host
        .events
        .lock()
        .unwrap()
        .iter()
        .filter(|e| matches!(e, AuditEvent::ClaimTaken { .. }))
        .count();
    assert_eq!(taken, n);
}

#[test]
fn hold_survives_own_edits_and_ends_on_release_ttl_or_leave() {
    let f = fx("hold");
    let a = f.agent("a");
    let id =
        f.s.claim(
            a,
            lines(2, 4),
            Some(60_000),
            Some("multi-edit".into()),
            true,
        )
        .unwrap()
        .id;
    edit_line(&f, a, 2, "TWO", ClaimPolicy::Ignore).unwrap();
    edit_line(&f, a, 3, "THREE", ClaimPolicy::Ignore).unwrap();
    assert_eq!(f.end_of(id), None, "held through its owner's edits");
    f.advance(60_000);
    f.s.expire_idle().unwrap();
    assert_eq!(f.end_of(id), Some(ClaimEnd::Expired));
}

#[test]
fn overlap_is_refused_at_acquire_with_who_lines_time_and_note() {
    let f = fx("overlap");
    let a = f.agent("a");
    let b = f.agent("b");
    let id =
        f.s.claim(a, lines(4, 6), Some(20_000), Some("intro".into()), false)
            .unwrap()
            .id;
    f.advance(5_000);
    let e = f.s.claim(b, lines(6, 9), None, None, false).unwrap_err();
    assert_eq!(
        e,
        SessionError::Claimed {
            id,
            by: "a".into(),
            name: "A".into(),
            lines: (4, 6),
            remaining_ms: 15_000,
            note: Some("intro".into()),
        }
    );
    assert_eq!(tools::session_exit(&e), exit::LEASED, "exit 5, as a lease");
    assert!(e.to_string().contains("lines 4–6 claimed by A (a)"), "{e}");
    // adjacent lines don't overlap; the owner may claim more of its own
    f.claim(b, 7, 9);
    f.claim(a, 5, 5);
    // a bad range
    assert!(matches!(
        f.s.claim(a, lines(20, 21), None, None, false),
        Err(SessionError::BadPosition(_))
    ));
    assert!(matches!(
        f.s.claim(a, lines(1, 1), Some(0), None, false),
        Err(SessionError::BadClaim(_))
    ));
    // a reader can't claim
    f.host
        .rights
        .lock()
        .unwrap()
        .insert("r".into(), Rights::read_only());
    let r = f.agent("r");
    assert!(matches!(
        f.s.claim(r, lines(1, 1), None, None, false),
        Err(SessionError::Forbidden { .. })
    ));
}

#[test]
fn deterministic_remaining_time_on_the_host_clock() {
    let mut c = Config::default();
    c.claim_max_ms = 90_000;
    let f = fx_with("clock", c);
    let a = f.agent("a");
    let obs = f.agent("obs");
    let info = f.s.claim(a, lines(1, 2), None, None, false).unwrap();
    assert_eq!(info.remaining_ms, 30_000, "the default TTL");
    assert_eq!(info.expires_ms, 1_030_000);
    f.advance(12_345);
    assert_eq!(f.s.claim_remaining_ms(obs, info.id).unwrap(), 17_655);
    assert_eq!(f.s.claims(obs).unwrap()[0].remaining_ms, 17_655);
    f.advance(17_655);
    assert_eq!(
        f.s.claim_remaining_ms(obs, info.id),
        Err(SessionError::NoClaim(Some(info.id)))
    );
    assert_eq!(
        tools::session_exit(&SessionError::NoClaim(Some(info.id))),
        4
    );
    // asks over the max are clamped
    let long =
        f.s.claim(a, lines(1, 2), Some(1 << 40), None, false)
            .unwrap();
    assert_eq!(long.remaining_ms, 90_000);
}

// ---------- the claim policy ----------

#[test]
fn respect_refuses_an_agent_ignore_never_does_and_a_person_is_never_refused() {
    let f = fx("policy");
    let a = f.agent("a");
    let b = f.agent("b");
    let p = f.person("p");
    let obs = f.agent("obs");
    let id = f.claim(a, 3, 4);

    // Respect: an agent's edit into a's claim is refused, nothing applied
    let before = f.text(obs);
    let e = edit_line(&f, b, 4, "B WAS HERE", ClaimPolicy::Respect).unwrap_err();
    assert!(
        matches!(e, SessionError::Claimed { id: x, .. } if x == id),
        "{e:?}"
    );
    assert_eq!(f.text(obs), before, "nothing applied");
    // ... through run_tool too: exit 5
    let e = f
        .tool_with(b, &["sed", "-i", "3s/three/3/"], ClaimPolicy::Respect)
        .unwrap_err();
    assert_eq!(e.exit(), exit::LEASED);
    assert_eq!(f.text(obs), before);
    // ... and write --base / write --base live
    let v = f.s.read(b).unwrap().version;
    let e =
        f.s.write_base_with(b, ROOT, v, &set_line(&before, 3, "X"), ClaimPolicy::Respect)
            .unwrap_err();
    assert!(matches!(e, SessionError::Claimed { .. }));
    let e =
        f.s.replace_text_with(b, ROOT, &set_line(&before, 3, "X"), ClaimPolicy::Respect)
            .unwrap_err();
    assert!(matches!(e, SessionError::Claimed { .. }));
    // an edit elsewhere goes through under Respect
    edit_line(&f, b, 8, "EIGHT", ClaimPolicy::Respect).unwrap();
    // typing at the claim's edge is inside it (expanding anchors)
    let edge = f.s.edit_with_policy(b, ROOT, ClaimPolicy::Respect, |t| {
        let at = t.find("three").unwrap();
        Ok::<_, ()>(format!("{}>{}", &t[..at], &t[at..]))
    });
    assert!(matches!(
        edge,
        Err(EditWithError::Session(SessionError::Claimed { .. }))
    ));

    // Ignore: the same edit applies (awareness only)
    edit_line(&f, b, 4, "B WAS HERE", ClaimPolicy::Ignore).unwrap();
    assert!(f.text(obs).contains("B WAS HERE"));
    assert_eq!(f.end_of(id), None, "another's edit doesn't end the claim");

    // Claims coordinate agents only: an app guest is never refused ...
    let g = f.join("g", ParticipantKind::App);
    edit_line(&f, g, 4, "APP GUEST", ClaimPolicy::Respect).unwrap();
    assert!(f.text(obs).contains("APP GUEST"));
    // ... and a person is never refused, whatever the policy
    edit_line(&f, p, 3, "PERSON", ClaimPolicy::Respect).unwrap();
    assert!(f.text(obs).contains("PERSON"));
    let out = f
        .tool_with(p, &["sed", "-i", "4s/.*/PERSON TOO/"], ClaimPolicy::Respect)
        .unwrap();
    assert_eq!(out.exit, 0);
    assert!(f.text(obs).contains("PERSON TOO"));
    let text = f.text(obs);
    f.s.replace_text_with(p, ROOT, &set_line(&text, 3, "P3"), ClaimPolicy::Respect)
        .unwrap();
    // the claim is still there (a person's edit doesn't end it either)
    assert_eq!(f.end_of(id), None);
    assert_eq!(f.s.claims(obs).unwrap().len(), 1);
}

// ---------- waiters and anti-starvation ----------

impl Fx {
    /// Reservation events so far: ("reserved", pid) or (how, pid).
    fn handoffs(&self) -> Vec<(String, String)> {
        self.host
            .events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                AuditEvent::Reserved { pid, .. } => Some(("reserved".to_owned(), pid.clone())),
                AuditEvent::ReservationEnded { pid, how, .. } => {
                    Some((how.as_str().to_owned(), pid.clone()))
                }
                _ => None,
            })
            .collect()
    }
}

fn ev(how: &str, pid: &str) -> (String, String) {
    (how.to_owned(), pid.to_owned())
}

/// When a claim with waiters ends, its range is reserved for the first waiter for
/// `claim_handoff_ms`: the owner can't grab it back, other agents' Respect edits are refused,
/// the waiter's edit lands and passes it on, a lapse passes it on, and a person is never
/// affected.
#[test]
fn handoff_reservation_goes_to_the_waiters_in_order() {
    let f = fx("handoff");
    let a = f.agent("a");
    let b = f.agent("b");
    let c = f.agent("c");
    let x = f.agent("x");
    let p = f.person("p");
    let obs = f.agent("obs");
    let id = f.claim(a, 2, 4);
    f.s.register_waiter(b, id).unwrap();
    f.s.register_waiter(c, id).unwrap();
    f.s.release_claim(a, Some(id)).unwrap();
    assert_eq!(f.handoffs(), vec![ev("reserved", "b")]);
    let r = f.s.reservations(obs).unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!((r[0].claim, r[0].lines.first, r[0].lines.last), (id, 2, 4));
    assert_eq!(
        (r[0].remaining_ms, r[0].waiters.clone()),
        (2_000, vec!["c".to_owned()])
    );

    // the owner's re-claim is refused, with the reservation's time left
    f.advance(500);
    let e = f.s.claim(a, lines(3, 3), None, None, false).unwrap_err();
    assert_eq!(
        e,
        SessionError::Claimed {
            id,
            by: "b".into(),
            name: "B".into(),
            lines: (2, 4),
            remaining_ms: 1_500,
            note: None,
        }
    );
    // another agent's Respect edit too; its Ignore edit and its claim elsewhere are fine
    let e = edit_line(&f, x, 3, "X", ClaimPolicy::Respect).unwrap_err();
    assert!(matches!(e, SessionError::Claimed { by, .. } if by == "b"));
    f.claim(x, 8, 8);
    // a person is never affected: edits under Respect, claims there
    edit_line(&f, p, 4, "PERSON", ClaimPolicy::Respect).unwrap();
    let pc = f.claim(p, 4, 4);
    f.s.release_claim(p, Some(pc)).unwrap();
    assert_eq!(
        f.s.reservations(obs).unwrap().len(),
        1,
        "untouched by the person"
    );

    // the waiter's edit lands (Respect) and passes the reservation to c
    edit_line(&f, b, 3, "B", ClaimPolicy::Respect).unwrap();
    assert!(f.text(obs).contains("B\n"));
    assert_eq!(
        f.handoffs(),
        vec![ev("reserved", "b"), ev("edited", "b"), ev("reserved", "c")]
    );
    assert_eq!(f.s.reservations(obs).unwrap()[0].remaining_ms, 2_000);
    // c lets it lapse: nobody is left, the range is free
    f.advance(1_999);
    assert!(f.s.claim(a, lines(2, 2), None, None, false).is_err());
    f.advance(1);
    f.s.expire_idle().unwrap();
    assert_eq!(f.handoffs().last(), Some(&ev("lapsed", "c")));
    assert!(f.s.reservations(obs).unwrap().is_empty());
    f.claim(a, 2, 2);
}

#[test]
fn a_reserved_waiter_claiming_takes_the_queue_with_it_and_cancel_or_leave_pass_it_on() {
    let f = fx("handoff2");
    let a = f.agent("a");
    let b = f.agent("b");
    let c = f.agent("c");
    let d = f.agent("d");
    let id = f.claim(a, 5, 6);
    for w in [b, c, d] {
        f.s.register_waiter(w, id).unwrap();
    }
    f.s.leave(a).unwrap(); // any end hands it on
    assert_eq!(f.handoffs(), vec![ev("reserved", "b")]);
    // b cancels: it passes to c
    assert!(f.s.cancel_waiter(b, id).unwrap());
    // c leaves: it passes to d
    f.s.leave(c).unwrap();
    assert_eq!(
        f.handoffs(),
        vec![
            ev("reserved", "b"),
            ev("cancelled", "b"),
            ev("reserved", "c"),
            ev("left", "c"),
            ev("reserved", "d"),
        ]
    );
    // d claims the range: the reservation is used, and nobody is left behind
    let e = f.agent("e");
    let mine = f.claim(d, 5, 5);
    assert_eq!(f.handoffs().last(), Some(&ev("claimed", "d")));
    assert!(f.s.reservations(e).unwrap().is_empty());
    assert_eq!(f.s.claims(e).unwrap()[0].id, mine);

    // the queue moves onto the waiter's new claim
    let g = f.agent("g");
    let h = f.agent("h");
    let id2 = f.claim(g, 9, 9);
    f.s.register_waiter(d, id2).unwrap();
    f.s.register_waiter(h, id2).unwrap();
    f.s.release_claim(g, None).unwrap();
    let c2 = f.s.claim(d, lines(9, 9), None, None, false).unwrap();
    assert_eq!(c2.waiters, vec!["h"], "h now waits for d's claim");
}

#[test]
fn handoff_off_with_zero() {
    let mut cfg = Config::default();
    cfg.claim_handoff_ms = 0;
    let f = fx_with("handoff0", cfg);
    let a = f.agent("a");
    let b = f.agent("b");
    let id = f.claim(a, 1, 2);
    f.s.register_waiter(b, id).unwrap();
    f.s.release_claim(a, None).unwrap();
    assert!(f.handoffs().is_empty());
    f.claim(a, 1, 2);
}

#[test]
fn fifo_waiters_and_claim_ended_events() {
    let f = fx("fifo");
    let a = f.agent("a");
    let b = f.agent("b");
    let c = f.agent("c");
    let d = f.agent("d");
    let id = f.claim(a, 2, 5);
    assert_eq!(f.s.register_waiter(b, id).unwrap().position, 1);
    assert_eq!(f.s.register_waiter(c, id).unwrap().position, 2);
    assert_eq!(f.s.register_waiter(d, id).unwrap().position, 3);
    assert_eq!(
        f.s.register_waiter(b, id).unwrap().position,
        1,
        "again: no-op"
    );
    assert!(matches!(
        f.s.register_waiter(a, id),
        Err(SessionError::BadClaim(_))
    ));
    assert!(f.s.cancel_waiter(c, id).unwrap());
    assert!(!f.s.cancel_waiter(c, id).unwrap());
    assert_eq!(f.s.claims(b).unwrap()[0].waiters, vec!["b", "d"]);
    // a waiter that leaves leaves the queue
    let e = f.agent("e");
    f.s.register_waiter(e, id).unwrap();
    f.s.leave(e).unwrap();
    assert_eq!(f.s.claims(b).unwrap()[0].waiters, vec!["b", "d"]);
    f.s.release_claim(a, None).unwrap();
    let ended = f.ended();
    assert_eq!(ended.len(), 1);
    assert_eq!(
        ended[0],
        (id, ClaimEnd::Released, vec!["b".to_owned(), "d".to_owned()])
    );
    let next = f.host.events.lock().unwrap().iter().find_map(|e| match e {
        AuditEvent::ClaimEnded { next_waiter, .. } => next_waiter.clone(),
        _ => None,
    });
    assert_eq!(next, Some((b, "b".to_owned())));
    assert_eq!(
        f.s.register_waiter(b, id),
        Err(SessionError::NoClaim(Some(id)))
    );
}

#[test]
fn no_renewal_or_hold_while_others_wait() {
    let f = fx("starve");
    let a = f.agent("a");
    let b = f.agent("b");
    let id = f.claim(a, 2, 3);
    // without waiters: renew and hold freely
    let r = f.s.renew_claim(a, None, Some(40_000), Some(true)).unwrap();
    assert!(r.hold);
    assert_eq!(r.remaining_ms, 40_000);
    f.s.register_waiter(b, id).unwrap();
    f.advance(10_000);
    // extending, or holding, is refused while b waits; nothing changes
    let e =
        f.s.renew_claim(a, Some(id), Some(40_000), None)
            .unwrap_err();
    assert_eq!(
        e,
        SessionError::ClaimContended {
            id,
            waiters: 1,
            remaining_ms: 30_000
        }
    );
    assert_eq!(tools::session_exit(&e), 2);
    f.s.renew_claim(a, Some(id), None, Some(false)).unwrap(); // dropping hold: fine
    assert!(matches!(
        f.s.renew_claim(a, Some(id), Some(60_000), Some(true)),
        Err(SessionError::ClaimContended { .. })
    ));
    // a fresh overlapping claim by the owner doesn't get around the queue either
    assert!(matches!(
        f.s.claim(a, lines(3, 4), Some(60_000), None, true),
        Err(SessionError::ClaimContended { .. })
    ));
    // a shorter TTL is allowed
    let r = f.s.renew_claim(a, Some(id), Some(5_000), None).unwrap();
    assert_eq!(r.remaining_ms, 5_000);
    // it still ends normally, telling b
    f.advance(5_000);
    f.s.expire_idle().unwrap();
    assert_eq!(
        f.ended(),
        vec![(id, ClaimEnd::Expired, vec!["b".to_owned()])]
    );
    // other errors
    assert_eq!(
        f.s.renew_claim(a, None, None, None),
        Err(SessionError::NoClaim(None))
    );
    let id2 = f.claim(a, 9, 9);
    assert!(matches!(
        f.s.renew_claim(b, Some(id2), None, None),
        Err(SessionError::BadClaim(_))
    ));
}

#[test]
fn a_connection_with_a_claim_or_waiting_does_not_idle_out() {
    let f = fx("idle");
    let p = |pid: &str| Participant::new(pid, pid, "#1", ParticipantKind::Agent, "cli");
    let a = f.s.join_shared(p("a"), Stay::UntilIdle).unwrap().conn;
    let b = f.s.join_shared(p("b"), Stay::UntilIdle).unwrap().conn;
    let id =
        f.s.claim(a, lines(1, 1), Some(120_000), None, false)
            .unwrap()
            .id;
    f.s.register_waiter(b, id).unwrap();
    f.advance(90_000); // past idle_leave_ms (60 s)
    assert!(f.s.expire_idle().unwrap().is_empty());
    f.advance(30_000);
    let gone = f.s.expire_idle().unwrap();
    assert_eq!(f.end_of(id), Some(ClaimEnd::Expired));
    assert_eq!(gone, vec![(a, "a".to_owned())], "the owner idles out");
    // b holds the hand-off reservation for 2 s, then idles out too
    let obs = f.agent("obs"); // (a call through b would keep b active)
    assert_eq!(f.s.reservations(obs).unwrap()[0].pid, "b");
    f.advance(2_000);
    assert_eq!(f.s.expire_idle().unwrap(), vec![(b, "b".to_owned())]);
}

// ---------- anchors ----------

#[test]
fn claims_follow_concurrent_edits() {
    let f = fx("follow");
    let a = f.agent("a");
    let b = f.agent("b");
    let id = f.claim(a, 5, 6);
    f.s.edit_with(b, ROOT, |t| Ok::<_, ()>(format!("x\ny\nz\n{t}")))
        .unwrap();
    let c = f.s.claims(a).unwrap().remove(0);
    assert_eq!((c.id, c.lines.first, c.lines.last), (id, 8, 9));
    let t = f.text(a);
    assert_eq!(&t[c.start..c.end], "five\nsix");
    // deleting above moves it back; deleting inside shrinks it
    f.s.edit_with(b, ROOT, |t| Ok::<_, ()>(t.replacen("x\ny\n", "", 1)))
        .unwrap();
    f.s.edit_with(b, ROOT, |t| Ok::<_, ()>(t.replacen("five\n", "", 1)))
        .unwrap();
    let c = f.s.claims(a).unwrap().remove(0);
    assert_eq!(c.lines.last, 6);
    assert!(c.start <= c.end);
    assert!(f.text(a)[c.start..c.end].ends_with("six"));
    // the roster reports it the same way
    let row =
        f.s.roster(b)
            .unwrap()
            .into_iter()
            .find(|r| r.participant.pid == "a")
            .unwrap();
    assert_eq!(row.claims, vec![c]);
}

/// K2 Engine's case: a claim on a near-empty range, typed into at its edges. Expanding
/// anchors (start sticks before, end after) keep it valid and take the text in; the opposite
/// convention would invert it.
#[test]
fn expanding_anchors_on_a_near_empty_range() {
    let f = fx("anchor");
    let a = f.agent("a");
    let p = f.person("p");
    f.s.edit_with(a, ROOT, |_| Ok::<_, ()>("a\n\nb\n".to_owned()))
        .unwrap();
    let c = f.s.claim(a, lines(2, 2), None, None, false).unwrap();
    assert_eq!((c.start, c.end), (2, 2), "an empty line: an empty span");
    // the person types at the empty range (its start and end)
    f.s.edit_with(p, ROOT, |t| {
        Ok::<_, ()>(format!("{}xy{}", &t[..2], &t[2..]))
    })
    .unwrap();
    let c = f.s.claims(a).unwrap().remove(0);
    assert!(c.start <= c.end, "never inverted");
    assert_eq!((c.start, c.end), (2, 4), "the typed text lands inside");
    assert_eq!(&f.text(a)[c.start..c.end], "xy");
    // typing at the end edge too
    f.s.edit_with(p, ROOT, |t| Ok::<_, ()>(format!("{}!{}", &t[..4], &t[4..])))
        .unwrap();
    let c = f.s.claims(a).unwrap().remove(0);
    assert_eq!(&f.text(a)[c.start..c.end], "xy!");
    assert_eq!((c.lines.first, c.lines.last), (2, 2));
}

// ---------- --for-edit ----------

#[test]
fn for_edit_claims_the_lines_shown_and_the_next_edit_releases_it() {
    let f = fx("foredit");
    let a = f.agent("a");
    let out = f.tool(a, &["view", "--lines", "3,5", "--for-edit"]);
    assert_eq!(out.stdout, b"three\nfour\nfive\n");
    let Some(ForEdit::Claimed(c)) = out.for_edit.as_deref().cloned() else {
        panic!("{:?}", out.for_edit)
    };
    assert_eq!((c.lines.first, c.lines.last), (3, 5));
    assert!(c.for_edit);
    assert_eq!(c.remaining_ms, 30_000, "for_edit_ms");
    assert!(out.for_edit.unwrap().to_string().starts_with("note: "));
    // the next edit touching it releases it
    let e = f.tool(a, &["sed", "-i", "4s/four/FOUR/"]);
    assert_eq!(e.exit, 0);
    assert_eq!(f.end_of(c.id), Some(ClaimEnd::Edited));

    // --ttl, and grep's selected span
    let out = f.tool(a, &["grep", "-n", "--for-edit", "--ttl", "5s", "^s"]);
    assert_eq!(out.stdout, b"6:six\n7:seven\n");
    let Some(ForEdit::Claimed(c)) = out.for_edit.as_deref() else {
        panic!()
    };
    assert_eq!((c.lines.first, c.lines.last, c.remaining_ms), (6, 7, 5_000));
    // cat takes the whole doc; head and tail their lines
    let out = f.tool(a, &["head", "-n", "2", "--for-edit"]);
    assert!(matches!(out.for_edit.as_deref(), Some(ForEdit::Claimed(c)) if c.lines.last == 2));
    // a byte count shows no lines: no claim, the read still works
    let out = f.tool(a, &["head", "-c", "3", "--for-edit"]);
    assert_eq!(out.stdout, b"one");
    assert!(matches!(
        out.for_edit.as_deref(),
        Some(ForEdit::NotTaken(_))
    ));
    // --ttl without --for-edit, --for-edit on an edit or on wc: refused
    for argv in [
        &["view", "--lines", "1,2", "--ttl", "5"][..],
        &["sed", "-i", "--for-edit", "s/a/b/"][..],
        &["wc", "--for-edit"][..],
        &["view", "--lines", "1,2", "--for-edit", "--ttl", "0"][..],
    ] {
        let argv: Vec<String> = argv.iter().map(|a| (*a).to_owned()).collect();
        let e = f.s.run_tool(a, ROOT, &argv, b"").unwrap_err();
        assert_eq!(e.exit(), exit::USAGE, "{argv:?}");
    }
    // without --for-edit, nothing is claimed
    assert!(f.tool(a, &["view", "--lines", "9,9"]).for_edit.is_none());
}

#[test]
fn for_edit_into_another_claim_reads_and_reports_the_conflict() {
    let f = fx("foreditx");
    let a = f.agent("a");
    let b = f.agent("b");
    let theirs =
        f.s.claim(b, lines(4, 4), None, Some("mine".into()), false)
            .unwrap();
    let plain = f.tool(a, &["view", "--lines", "3,5"]);
    f.advance(6_000); // past the note rate limit
    let out = f.tool(a, &["view", "--lines", "3,5", "--for-edit"]);
    assert_eq!(out.exit, 0);
    assert_eq!(out.stdout, plain.stdout, "the read succeeds unchanged");
    let Some(ForEdit::Conflict(c)) = out.for_edit.as_deref() else {
        panic!("{:?}", out.for_edit)
    };
    assert_eq!(c.id, theirs.id);
    let line = out.for_edit.as_ref().unwrap().to_string();
    assert!(
        line.starts_with("note: ") && line.contains("claimed by B (b)"),
        "{line}"
    );
    assert_eq!(f.s.claims(a).unwrap().len(), 1, "nothing taken");
    // a reader's --for-edit takes nothing
    f.host
        .rights
        .lock()
        .unwrap()
        .insert("r".into(), Rights::read_only());
    let r = f.agent("r");
    let out = f.tool(r, &["view", "--lines", "8,9", "--for-edit"]);
    assert!(matches!(
        out.for_edit.as_deref(),
        Some(ForEdit::NotTaken(SessionError::Forbidden { .. }))
    ));
}

// ---------- notes ----------

#[test]
fn notes_say_who_is_on_the_lines_touched() {
    let f = fx("notes");
    let a = f.agent("a");
    let b = f.agent("b");
    let c = f.agent("c");
    let p = f.person("p");
    let far = f.agent("far");
    f.tool(b, &["view", "--lines", "2,4"]); // viewing 2-4
    f.s.claim(c, lines(3, 3), None, Some("fixing".into()), false)
        .unwrap();
    f.s.set_cursor(
        p,
        ROOT,
        k2_mpc_base::session::At::Line(k2_mpc_base::session::LineCol { line: 4, col: 2 }),
        k2_mpc_base::session::At::Line(k2_mpc_base::session::LineCol { line: 4, col: 2 }),
    )
    .unwrap();
    f.tool(far, &["view", "--lines", "9,10"]); // elsewhere: no note
    f.advance(8_000);
    let out = f.tool(a, &["view", "--lines", "3,4"]);
    let kinds: Vec<(String, NoteKind)> = out
        .notes
        .notes
        .iter()
        .map(|n| (n.pid.clone(), n.kind))
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("c".to_owned(), NoteKind::Claimed),
            ("p".to_owned(), NoteKind::Caret),
            ("b".to_owned(), NoteKind::Viewing),
        ]
    );
    let text = out.notes.to_string();
    assert!(text.lines().all(|l| l.starts_with("note: ")), "{text}");
    assert!(
        text.contains("note: C (c) claimed line 3 \"fixing\" (8 s ago, 22 s left)"),
        "{text}"
    );
    assert!(
        text.contains("note: B (b) viewing lines 2–4 (8 s ago)"),
        "{text}"
    );
    // an edit gets notes too, and b's edit shows as "edited" to others
    f.advance(6_000);
    f.tool(b, &["sed", "-i", "7s/seven/SEVEN/"]);
    f.advance(1_000);
    let out = f.tool(a, &["view", "--lines", "7,7"]);
    assert_eq!(out.notes.notes.len(), 1, "{}", out.notes);
    assert_eq!(out.notes.notes[0].kind, NoteKind::Edited);
    assert_eq!(out.notes.notes[0].age_ms, 1_000);
    f.advance(6_000);
    let e = f.tool(a, &["sed", "-i", "9s/nine/9/"]);
    let n: Vec<(&str, NoteKind)> = e
        .notes
        .notes
        .iter()
        .map(|n| (n.pid.as_str(), n.kind))
        .collect();
    assert_eq!(n, vec![("far", NoteKind::Viewing)], "{}", e.notes);
    // a note already given isn't repeated within note_repeat_ms
    f.advance(6_000);
    assert!(f.tool(a, &["view", "--lines", "3,4"]).notes.is_empty());
    // the caller's own presence never makes a note
    f.advance(6_000);
    f.s.claim(a, lines(1, 1), None, None, false).unwrap();
    f.tool(a, &["view", "--lines", "1,1"]);
    assert!(f.tool(a, &["view", "--lines", "1,1"]).notes.is_empty());
}

#[test]
fn notes_are_deduped_and_rate_limited() {
    let mut cfg = Config::default();
    cfg.note_batch_max = 2;
    let f = fx_with("dedupe", cfg);
    let a = f.agent("a");
    for (pid, l) in [("b", "1,3"), ("c", "2,5"), ("d", "3,3")] {
        let c = f.agent(pid);
        f.tool(c, &["view", "--lines", l]);
    }
    // a 20-read loop, a second apart: bounded notes
    let mut lines_printed = 0;
    let mut batches: Vec<Notes> = Vec::new();
    for _ in 0..20 {
        f.advance(1_000);
        let out = f.tool(a, &["cat"]);
        lines_printed += out.notes.lines().len();
        batches.push(out.notes);
    }
    // first batch: 2 shown, "+1 more"; the third comes once the 5 s interval passes; then
    // nothing repeats within the 60 s window
    assert_eq!(batches[0].notes.len(), 2);
    assert_eq!(batches[0].more, 1);
    assert_eq!(batches[0].lines().last().unwrap(), "note: +1 more");
    assert!(
        batches[1].is_empty() && batches[1].held == 1,
        "{:?}",
        batches[1]
    );
    assert_eq!(batches[5].notes.len(), 1, "after the interval");
    assert!(batches[6..].iter().all(|b| b.is_empty() && b.held == 0));
    assert_eq!(lines_printed, 4, "3 notes + one '+N more' in 20 reads");
    // after the repeat window they may come again
    f.advance(60_000);
    let out = f.tool(a, &["cat"]);
    assert_eq!(out.notes.notes.len(), 2);
}

#[test]
fn notes_never_touch_stdout() {
    let f = fx("stdout");
    let a = f.agent("a");
    let quiet = f.tool(a, &["cat", "-n"]).stdout;
    let quiet_grep = f.tool(a, &["grep", "-n", "e"]).stdout;
    let b = f.agent("b");
    f.s.claim(b, lines(1, 10), Some(60_000), Some("all".into()), false)
        .unwrap();
    for pid in ["c", "d"] {
        let c = f.agent(pid);
        f.tool(c, &["view", "--lines", "1,10"]);
    }
    f.advance(6_000);
    let busy = f.tool(a, &["cat", "-n"]);
    assert!(!busy.notes.is_empty(), "there was something to say");
    assert_eq!(busy.stdout, quiet, "stdout is byte-identical");
    assert!(!String::from_utf8_lossy(&busy.stdout).contains("note:"));
    f.advance(6_000);
    let busy = f.tool(a, &["grep", "-n", "e"]);
    assert_eq!(busy.stdout, quiet_grep);
    // an edit's stdout (empty for sed -i) doesn't carry them either
    f.advance(6_000);
    let e = f.tool(a, &["sed", "-i", "s/two/2/"]);
    assert!(e.stdout.is_empty());
}

// ---------- activity, roster, conflicts ----------

#[test]
fn activity_is_a_bounded_ring_dropped_on_unload() {
    let mut cfg = Config::default();
    cfg.activity_max = 8;
    let f = fx_with("activity", cfg);
    let a = f.agent("a");
    let id = f.claim(a, 2, 2);
    f.tool(a, &["grep", "two"]);
    f.tool(a, &["sed", "-i", "2s/two/2/"]);
    let rows = f.s.activity(a, 10).unwrap();
    let kinds: Vec<(ActivityKind, &str)> = rows.iter().map(|r| (r.kind, r.verb.as_str())).collect();
    assert_eq!(
        kinds,
        vec![
            (ActivityKind::Claim, "claim"),
            (ActivityKind::Read, "grep"),
            (ActivityKind::ClaimEnded(ClaimEnd::Edited), "claim edited"),
            (ActivityKind::Edit, "sed -i"),
        ]
    );
    assert_eq!(rows[0].claim, Some(id));
    assert_eq!(
        rows[3].lines.as_ref().map(|l| (l.first, l.last)),
        Some((2, 2))
    );
    for _ in 0..20 {
        f.tool(a, &["view", "--lines", "1,1"]);
    }
    let rows = f.s.activity(a, 100).unwrap();
    assert_eq!(rows.len(), 8, "bounded");
    assert!(rows.iter().all(|r| r.verb == "view"));
    assert_eq!(f.s.activity(a, 3).unwrap().len(), 3, "the last n");
    // dropped when the doc unloads
    f.s.leave(a).unwrap();
    let key = f.s.key().unwrap();
    let Fx {
        dir: _dir,
        host,
        mgr,
        s,
    } = f;
    drop(s);
    host.now.fetch_add(11 * 60_000, Ordering::SeqCst);
    assert_eq!(mgr.unload_idle().unwrap(), 1);
    let s = mgr
        .open(Arc::new(TextFilePack::new("markdown", 4 << 20)), &key, None)
        .unwrap();
    let a = s
        .join(
            Participant::new("a", "A", "#1", ParticipantKind::Agent, "cli"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    assert!(s.activity(a, 100).unwrap().is_empty());
}

#[test]
fn roster_rows_carry_claims_views_last_edits_and_agent_carets() {
    let f = fx("roster");
    let a = f.agent("a");
    let obs = f.agent("obs");
    f.tool(a, &["view", "--lines", "6,8"]);
    f.tool(a, &["sed", "-i", "2s/two/TWO/"]);
    f.s.claim(a, lines(9, 10), None, Some("tail".into()), false)
        .unwrap();
    let row =
        f.s.roster(obs)
            .unwrap()
            .into_iter()
            .find(|r| r.participant.pid == "a")
            .unwrap();
    assert_eq!(row.view, Some(lines(6, 8)));
    assert_eq!(row.last_edit, Some(lines(2, 2)));
    assert_eq!(row.claims.len(), 1);
    assert_eq!(row.claims[0].lines, lines(9, 10));
    assert_eq!(row.claims[0].note.as_deref(), Some("tail"));
    let caret = row.cursor.expect("an agent's caret is at its last edit");
    assert_eq!(caret.head.line, 2);
}

#[test]
fn a_write_base_conflict_names_claimants_and_recent_editors() {
    let f = fx("conflict");
    let a = f.agent("a");
    let b = f.agent("b");
    let c = f.agent("c");
    let base = f.s.read(a).unwrap();
    let base_text = base.text_by_root[0].1.clone();
    // b edits line 4 since a's read; c claims lines 3-5
    edit_line(&f, b, 4, "B's four", ClaimPolicy::Ignore).unwrap();
    f.s.claim(c, lines(3, 5), None, Some("restructure".into()), false)
        .unwrap();
    f.advance(12_000);
    let rep =
        f.s.write_base_with(
            a,
            ROOT,
            base.version,
            &set_line(&base_text, 4, "A's four"),
            ClaimPolicy::Ignore,
        )
        .unwrap();
    assert!(matches!(rep.outcome, WriteOutcome::Conflicts(ref c) if c.len() == 1));
    let who: Vec<(usize, String, NoteKind)> = rep
        .conflict_notes
        .iter()
        .map(|(k, n)| (*k, n.pid.clone(), n.kind))
        .collect();
    assert_eq!(
        who,
        vec![
            (0, "c".to_owned(), NoteKind::Claimed),
            (0, "b".to_owned(), NoteKind::Edited),
        ]
    );
    let line = rep.conflict_notes[0].1.to_string();
    assert!(
        line.contains("claimed lines 3–5 \"restructure\" (12 s ago"),
        "{line}"
    );
    assert_eq!(rep.conflict_notes[1].1.age_ms, 12_000);
    // write_base (the old call) is unchanged
    assert!(matches!(
        f.s.write_base(a, ROOT, base.version, &set_line(&base_text, 4, "A's four"))
            .unwrap(),
        WriteOutcome::Conflicts(_)
    ));
}

// ---------- never in the store ----------

/// Views, claims, waiters, notes and activity are memory only: a session full of them leaves
/// the store file byte for byte as it was.
#[test]
fn presence_never_reaches_the_store() {
    let f = fx("store");
    // joins record client IDs (that is a store write): do them first
    let a = f.agent("a");
    let b = f.agent("b");
    let p = f.person("p");
    let db = f.dir.0.join("docs.redb");
    let before = std::fs::read(&db).unwrap();
    let id =
        f.s.claim(a, lines(2, 4), Some(60_000), Some("x".into()), true)
            .unwrap()
            .id;
    f.s.register_waiter(b, id).unwrap();
    for i in 0..30 {
        f.advance(700);
        f.tool(b, &["cat", "-n"]);
        f.tool(p, &["grep", "-n", "e"]);
        f.tool(
            b,
            &[
                "view",
                "--lines",
                &format!("{},{}", 1 + i % 5, 6 + i % 4),
                "--for-edit",
            ],
        );
        f.s.renew_claim(a, Some(id), Some(30_000), None).ok();
        f.s.roster(p).unwrap();
        f.s.activity(p, 50).unwrap();
        f.s.claims(b).unwrap();
        f.s.expire_idle().unwrap();
    }
    f.s.cancel_waiter(b, id).unwrap();
    f.s.renew_claim(a, Some(id), Some(60_000), None).unwrap();
    f.s.release_claim(a, None).unwrap();
    f.s.release_claim(b, None).unwrap();
    f.advance(120_000);
    f.s.expire_idle().unwrap();
    assert!(!f.s.activity(p, 300).unwrap().is_empty());
    assert_eq!(
        std::fs::read(&db).unwrap(),
        before,
        "the store file changed: presence must stay in memory"
    );
}

// ---------- registry ----------

#[test]
fn registry_rows_for_the_new_verbs() {
    let r = Registry::kit();
    for (verb, serves, exits) in [
        ("claim", PersonAction::Caret, &[0u8][..]),
        ("claim --renew", PersonAction::Caret, &[0]),
        ("claim --release", PersonAction::Caret, &[0, 1]),
        ("activity", PersonAction::Who, &[0]),
    ] {
        let v = r.get(verb).unwrap_or_else(|| panic!("{verb} registered"));
        assert_eq!(v.serves, &[serves], "{verb}");
        assert_eq!(v.exits, exits, "{verb}");
    }
    for verb in ["cat", "head", "tail", "view", "grep", "sed"] {
        assert!(
            r.get(verb).unwrap().usage.contains("--for-edit"),
            "{verb}'s usage names --for-edit"
        );
    }
    let form = |a: &[&str]| {
        k2_mpc_base::parity::verb_form(&a.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>())
    };
    assert_eq!(form(&["claim", "--lines", "1,2"]), "claim");
    assert_eq!(form(&["claim", "--renew"]), "claim --renew");
    assert_eq!(
        form(&["claim", "--release", "--id", "claim-3"]),
        "claim --release"
    );
    assert_eq!(form(&["view", "--lines", "1,2", "--for-edit"]), "view");
}

// ---------- last-edit ranges don't expand (finding F2) ----------

fn last_edit(f: &Fx, obs: ConnId, pid: &str) -> Option<LineRange> {
    f.s.roster(obs)
        .unwrap()
        .into_iter()
        .find(|r| r.participant.pid == pid)
        .and_then(|r| r.last_edit)
}

fn edited_notes_from<'a>(n: &'a Notes, pid: &str) -> Vec<&'a LineRange> {
    n.notes
        .iter()
        .filter(|x| x.pid == pid && x.kind == NoteKind::Edited)
        .map(|x| &x.lines)
        .collect()
}

/// A's line is undone, then B types at the same spot: nothing says A edited B's line.
#[test]
fn last_edit_undone_then_someone_types_there_is_not_theirs() {
    let f = fx("f2-undo");
    let a = f.agent("a");
    let b = f.agent("b");
    let obs = f.person("obs");
    let out = f.s.run_tool(
        a,
        ROOT,
        &["insert".into(), "--line".into(), "6".into()],
        b"A new\n",
    );
    assert!(out.unwrap().changed.is_some());
    assert_eq!(last_edit(&f, obs, "a"), Some(lines(6, 6)));
    assert!(matches!(
        f.s.undo(a, 1).unwrap(),
        k2_mpc_base::session::UndoOutcome::Applied { .. }
    ));
    let out =
        f.s.run_tool(
            b,
            ROOT,
            &["insert".into(), "--line".into(), "6".into()],
            b"B new\n",
        )
        .unwrap();
    assert!(
        edited_notes_from(&out.notes, "a").is_empty(),
        "{:?}",
        out.notes
    );
    // a's range: gone with its text, or at least not on b's line
    let le = last_edit(&f, obs, "a");
    assert!(
        le.as_ref().map_or(true, |l| l.first > 6 || l.last < 6),
        "{le:?}"
    );
    assert_eq!(last_edit(&f, obs, "b"), Some(lines(6, 6)));
}

/// Text typed at the edges of A's edited line stays out of A's range; a claim there still
/// takes it in (claims keep expanding anchors). A range whose text is all deleted is gone,
/// and none ever names a line past the end.
#[test]
fn last_edit_does_not_take_in_text_typed_at_its_edges() {
    let f = fx("f2-edges");
    let a = f.agent("a");
    let b = f.agent("b");
    let obs = f.person("obs");
    f.tool(a, &["sed", "-i", "2s/two/TWO/"]);
    f.claim(a, 2, 2);
    assert_eq!(last_edit(&f, obs, "a"), Some(lines(2, 2)));
    let ins = |argv: &[&str], stdin: &str| {
        let argv: Vec<String> = argv.iter().map(|x| (*x).to_owned()).collect();
        f.s.run_tool(b, ROOT, &argv, stdin.as_bytes()).unwrap()
    };
    // a line typed right before a's text (at its first byte)
    ins(&["insert", "--line", "2"], "B1\n");
    assert_eq!(last_edit(&f, obs, "a"), Some(lines(3, 3)));
    // the claim took it in
    let claims = f.s.claims(obs).unwrap();
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].lines, lines(2, 3));
    // text typed right after a's text, before its line break
    let out = ins(&["insert", "--at", "3:4"], "\nB2");
    assert_eq!(f.text(obs).lines().nth(2), Some("TWO"));
    assert_eq!(last_edit(&f, obs, "a"), Some(lines(3, 3)));
    assert!(
        edited_notes_from(&out.notes, "a")
            .iter()
            .all(|l| l.first == 3 && l.last == 3),
        "{:?}",
        out.notes
    );
    // a's text deleted: a's range is gone
    ins(&["delete", "--lines", "3,3"], "");
    assert_eq!(last_edit(&f, obs, "a"), None);
    // a appends the last line; b deletes everything after line 1: never past the end
    f.s.run_tool(a, ROOT, &["append".into()], b"last\n")
        .unwrap();
    let n = f.text(obs).lines().count();
    assert_eq!(last_edit(&f, obs, "a"), Some(lines(n, n)));
    let r = format!("2,{n}");
    ins(&["delete", "--lines", &r], "");
    assert_eq!(last_edit(&f, obs, "a"), None);
    let le = last_edit(&f, obs, "b");
    assert!(le.as_ref().map_or(true, |l| l.last <= 1), "{le:?}");
}

// ---------- adjacent claims and their shared edge (soak finding, v0.9.2) ----------

/// Insert `x` at byte `at` as `c` (an engine edit: no claim policy).
fn type_at(f: &Fx, c: ConnId, at: usize, x: &str) {
    f.s.edit(
        c,
        vec![k2_mpc_base::Edit::Text {
            root: ROOT.into(),
            hunks: vec![k2_mpc_base::text::diff::Hunk {
                start: at,
                end: at,
                insert: x.into(),
            }],
        }],
        None,
    )
    .unwrap();
}

/// Delete bytes `s..e` as `c`.
fn delete_at(f: &Fx, c: ConnId, s: usize, e: usize) {
    f.s.edit(
        c,
        vec![k2_mpc_base::Edit::Text {
            root: ROOT.into(),
            hunks: vec![k2_mpc_base::text::diff::Hunk {
                start: s,
                end: e,
                insert: String::new(),
            }],
        }],
        None,
    )
    .unwrap();
}

/// A held claim (it survives its owner's edits) on lines `first..=last`.
fn claim_held(f: &Fx, c: ConnId, first: usize, last: usize) -> ClaimId {
    f.s.claim(c, lines(first, last), None, None, true)
        .unwrap()
        .id
}

/// Every pair of two participants' live claims is apart (half-open spans).
fn assert_apart(f: &Fx, c: ConnId) {
    let cl = f.s.claims(c).unwrap();
    for (k, x) in cl.iter().enumerate() {
        for y in &cl[k + 1..] {
            assert!(
                x.pid == y.pid || x.end <= y.start || y.end <= x.start,
                "{}'s {}..{} and {}'s {}..{} overlap",
                x.pid,
                x.start,
                x.end,
                y.pid,
                y.start,
                y.end
            );
        }
    }
}

/// The span of claim `id` now.
fn span(f: &Fx, c: ConnId, id: ClaimId) -> (usize, usize) {
    let cl = f.s.claims(c).unwrap();
    let x = cl.iter().find(|x| x.id == id).expect("claim live");
    (x.start, x.end)
}

/// Agents `a` (line 1) and `b` (line 2) hold claims, in that order (held, so their owners'
/// own typing doesn't end them); person `c` deletes the line break between them, so they
/// meet at one point. Returns the point.
fn shared_edge(
    f: &Fx,
    a: ConnId,
    b: ConnId,
    c: ConnId,
    a_first: bool,
) -> (ClaimId, ClaimId, usize) {
    let (ia, ib) = if a_first {
        let ia = claim_held(f, a, 1, 1);
        (ia, claim_held(f, b, 2, 2))
    } else {
        let ib = claim_held(f, b, 2, 2);
        (claim_held(f, a, 1, 1), ib)
    };
    let nl = f.text(c).find('\n').unwrap();
    delete_at(f, c, nl, nl + 1);
    assert_eq!(span(f, c, ia).1, nl);
    assert_eq!(span(f, c, ib).0, nl);
    (ia, ib, nl)
}

/// The rule: two participants' claims never overlap. Text typed where they meet joins the
/// typist's own claim if they hold one of the two, otherwise the claim acquired first.
/// (The soak found both anchors expanding there; this was its repro.)
#[test]
fn adjacent_claims_never_overlap_after_typing_at_their_shared_edge() {
    let f = fx("shared-edge");
    let a = f.agent("a");
    let b = f.agent("b");
    let c = f.person("c");
    let (ia, ib, nl) = shared_edge(&f, a, b, c, true);
    // a person with no claim types there: it joins a's (acquired first)
    type_at(&f, c, nl, "X");
    assert_apart(&f, c);
    assert_eq!(span(&f, c, ia), (0, nl + 1), "X joins a's claim");
    assert_eq!(span(&f, c, ib).0, nl + 1, "b's starts after X");
    assert_eq!(&f.text(c)[nl..nl + 1], "X");
}

#[test]
fn shared_edge_a_third_party_agent_typing_joins_the_claim_acquired_first() {
    for a_first in [true, false] {
        let f = fx("shared-edge-third");
        let a = f.agent("a");
        let b = f.agent("b");
        let c = f.person("c");
        let d = f.agent("d");
        let (ia, ib, nl) = shared_edge(&f, a, b, c, a_first);
        type_at(&f, d, nl, "YY");
        assert_apart(&f, c);
        if a_first {
            assert_eq!(span(&f, c, ia).1, nl + 2, "a's (first) takes it");
            assert_eq!(span(&f, c, ib).0, nl + 2);
        } else {
            assert_eq!(span(&f, c, ia).1, nl, "a's (second) stays before it");
            assert_eq!(span(&f, c, ib).0, nl, "b's (first) takes it");
            assert_eq!(span(&f, c, ib).1, f.text(c).find("two").unwrap() + 3);
        }
    }
}

#[test]
fn shared_edge_a_claimant_typing_joins_their_own_claim() {
    // b acquired second, but types there: its own claim takes the text
    let f = fx("shared-edge-claimant");
    let a = f.agent("a");
    let b = f.agent("b");
    let c = f.person("c");
    let (ia, ib, nl) = shared_edge(&f, a, b, c, true);
    type_at(&f, b, nl, "Z");
    assert_apart(&f, c);
    assert_eq!(span(&f, c, ia), (0, nl), "a's stays before Z");
    assert_eq!(span(&f, c, ib).0, nl, "b's takes Z");
    // and a, acquired first, typing there takes it into its own
    type_at(&f, a, nl, "W");
    assert_apart(&f, c);
    assert_eq!(span(&f, c, ia), (0, nl + 1), "a's takes W");
    assert_eq!(span(&f, c, ib).0, nl + 1);
    assert_eq!(&f.text(c)[nl..nl + 2], "WZ");
    // a person holding a claim is a claimant too
    let g = fx("shared-edge-person-claimant");
    let p = g.person("p");
    let q = g.agent("q");
    let r = g.person("r");
    let (ip, iq, nl) = shared_edge(&g, p, q, r, true);
    type_at(&g, q, nl, "V");
    assert_apart(&g, r);
    assert_eq!(span(&g, r, ip).1, nl);
    assert_eq!(span(&g, r, iq).0, nl);
}

#[test]
fn shared_edge_an_emptied_claim_moves_away_from_the_text() {
    // b's line is emptied with its line break: b's claim is a point on a's end
    let f = fx("shared-edge-empty");
    let a = f.agent("a");
    let b = f.agent("b");
    let c = f.person("c");
    let ia = claim_held(&f, a, 1, 1);
    let ib = claim_held(&f, b, 2, 2);
    let nl = f.text(c).find('\n').unwrap();
    delete_at(&f, c, nl, nl + 4); // "\ntwo"
    assert_eq!(span(&f, c, ib), (nl, nl));
    type_at(&f, c, nl, "X");
    assert_apart(&f, c);
    assert_eq!(span(&f, c, ia), (0, nl + 1), "a's (first) takes X");
    assert_eq!(span(&f, c, ib), (nl + 1, nl + 1), "b's point moves past X");
    // b typing at its own point takes its text, a's stays
    type_at(&f, b, nl + 1, "Y");
    assert_apart(&f, c);
    assert_eq!(span(&f, c, ia), (0, nl + 1));
    assert_eq!(span(&f, c, ib), (nl + 1, nl + 2));
}

#[test]
fn shared_edge_the_policy_and_edited_ends_follow_the_claim_that_takes_the_text() {
    use k2_mpc_base::session::EditOptions;
    let f = fx("shared-edge-policy");
    let a = f.agent("a");
    let b = f.agent("b");
    let c = f.person("c");
    let d = f.agent("d");
    let ia = f.claim(a, 1, 1);
    let ib = f.claim(b, 2, 2);
    let nl = f.text(c).find('\n').unwrap();
    delete_at(&f, c, nl, nl + 1);
    let respect = EditOptions::new().policy(ClaimPolicy::Respect);
    let ins = |t: &str, x: &str| format!("{}{x}{}", &t[..nl], &t[nl..]);
    // a third agent under Respect: the text would join a's claim, so a's refuses it
    match f
        .s
        .edit_with_options(d, ROOT, &respect, |t| Ok::<_, ()>(ins(t, "D")))
    {
        Err(EditWithError::Session(SessionError::Claimed { id, by, .. })) => {
            assert_eq!((id, by.as_str()), (ia, "a"))
        }
        other => panic!("{other:?}"),
    }
    // b under Respect: it joins b's own claim, so a's doesn't stand in the way; b's own
    // edit touching its (unheld) claim ends it; a's lives on, before the text
    f.s.edit_with_options(b, ROOT, &respect, |t| Ok::<_, ()>(ins(t, "B")))
        .unwrap()
        .expect("applied");
    let ended: Vec<(ClaimId, ClaimEnd)> = f.ended().into_iter().map(|e| (e.0, e.1)).collect();
    assert_eq!(ended, vec![(ib, ClaimEnd::Edited)]);
    assert_eq!(span(&f, c, ia), (0, nl));
}

// ---------- grids (A10) ----------

mod grid {
    use super::*;
    use k2_mpc_base::elem::{Anchor, Key};
    use k2_mpc_base::session::{ClaimSpan, EditOptions, GridWrite, GridWriteOptions, View};
    use k2_mpc_base::table::{Table, TableError, CELLS, ROWS};
    use k2_mpc_base::Edit;

    /// header + r1..r4
    const GRID: &str = "id,city\nk1,Oslo\nk2,Rome\nk3,Lima\nk4,Kyiv\n";

    pub(super) struct Gx {
        _dir: TmpDir,
        host: Arc<TestHost>,
        _mgr: SessionManager,
        pub s: Arc<Session>,
    }

    pub(super) fn gx(tag: &str) -> Gx {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "k2mb-gpresence-{tag}-{}-{}",
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
        let mut config = Config::default();
        config.edit_burst = 100_000;
        config.edit_rate_per_s = 100_000;
        let mgr = SessionManager::new(store, host.clone(), config);
        let s = mgr
            .open(
                Arc::new(grid_pack::GridLite),
                &DocKey {
                    space: "ws".into(),
                    path: "t.csv".into(),
                },
                Some(GRID.as_bytes()),
            )
            .unwrap();
        Gx {
            _dir: TmpDir(dir),
            host,
            _mgr: mgr,
            s,
        }
    }

    pub(super) fn k(s: &str) -> Key {
        Key::new(s).unwrap()
    }

    pub(super) fn rows(a: &str, b: &str) -> ClaimSpan {
        ClaimSpan::Rows {
            first: k(a),
            last: k(b),
        }
    }

    impl Gx {
        pub fn join(&self, pid: &str, kind: ParticipantKind) -> ConnId {
            let p = Participant::new(pid, pid.to_uppercase(), "#123456", kind, "cli");
            self.s.join(p, Stay::Held).unwrap().conn
        }
        /// A table op as `c` (an agent names the live version).
        pub fn op(
            &self,
            c: ConnId,
            f: impl FnOnce(&mut Table) -> Result<Vec<Edit>, TableError>,
        ) -> Result<u64, SessionError> {
            self.op_with(c, ClaimPolicy::Ignore, f)
        }
        pub fn op_with(
            &self,
            c: ConnId,
            policy: ClaimPolicy,
            f: impl FnOnce(&mut Table) -> Result<Vec<Edit>, TableError>,
        ) -> Result<u64, SessionError> {
            let v = self.s.status().unwrap().version;
            let opts = EditOptions::new().policy(policy).if_version(v);
            match self.s.edit_ops_with(c, &opts, |view: &View| {
                let mut t = view.table().map_err(|e| e.to_string())?;
                f(&mut t).map_err(|e| e.to_string())
            }) {
                Ok(r) => Ok(r.map_or(0, |r| r.version)),
                Err(EditWithError::Session(e)) => Err(e),
                Err(EditWithError::Compute(e)) => panic!("table op: {e}"),
            }
        }
        pub fn tool(
            &self,
            c: ConnId,
            argv: &[&str],
            policy: ClaimPolicy,
        ) -> Result<tools::ToolOutput, ToolRunError> {
            let v = self.s.status().unwrap().version.to_string();
            let mut argv: Vec<String> = argv.iter().map(|a| (*a).to_owned()).collect();
            if argv.len() > 1
                && !["head", "view", "cat", "tail", "grep"].contains(&argv[0].as_str())
            {
                argv.extend(["--if-version".to_owned(), v]);
            }
            self.s
                .run_tool_with(c, ROWS, &argv, b"", &ToolOptions::new().policy(policy))
        }
        pub fn ended(&self) -> Vec<(ClaimId, ClaimEnd)> {
            self.host
                .events
                .lock()
                .unwrap()
                .iter()
                .filter_map(|e| match e {
                    AuditEvent::ClaimEnded { claim, how, .. } => Some((*claim, *how)),
                    _ => None,
                })
                .collect()
        }
        pub fn spans(&self, c: ConnId) -> Vec<String> {
            self.s
                .claims(c)
                .unwrap()
                .iter()
                .map(|i| format!("{} {}", i.pid, i.span))
                .collect()
        }
        pub fn cell_of(&self, c: ConnId, pid: &str) -> Option<k2_mpc_base::session::CellCursor> {
            self.s
                .roster(c)
                .unwrap()
                .into_iter()
                .find(|e| e.participant.pid == pid)
                .and_then(|e| e.cell)
        }
    }

    impl Gx {
        /// Reservation events so far: ("reserved", pid) or (how, pid).
        pub fn handoffs(&self) -> Vec<(String, String)> {
            self.host
                .events
                .lock()
                .unwrap()
                .iter()
                .filter_map(|e| match e {
                    AuditEvent::Reserved { pid, .. } => Some(("reserved".to_owned(), pid.clone())),
                    AuditEvent::ReservationEnded { pid, how, .. } => {
                        Some((how.as_str().to_owned(), pid.clone()))
                    }
                    _ => None,
                })
                .collect()
        }
        pub fn advance(&self, ms: u64) {
            self.host.now.fetch_add(ms, Ordering::SeqCst);
        }
    }

    fn ev(how: &str, pid: &str) -> (String, String) {
        (how.to_owned(), pid.to_owned())
    }

    /// Hand-off on a grid (C1 must-have (a), as text claims have it): when a grid claim with
    /// waiters is released, its rows (cells, columns) are reserved for the first waiter; an
    /// agent other than that waiter can neither claim nor (under `Respect`) edit there; the
    /// waiter's claim there takes it over (the rest of the queue waits for that claim).
    #[test]
    fn grid_claim_hands_off_to_its_waiter() {
        let g = gx("handoff");
        let owner = g.join("p2", ParticipantKind::Agent);
        let bot = g.join("bot", ParticipantKind::Agent);
        let aide = g.join("aide", ParticipantKind::Agent);
        let p = g.join("p1", ParticipantKind::Person);
        let id =
            g.s.claim_span(owner, rows("r2", "r3"), None, None, false)
                .unwrap()
                .id;
        assert_eq!(g.s.register_waiter(bot, id).unwrap().position, 1);
        assert_eq!(g.s.register_waiter(aide, id).unwrap().position, 2);
        g.s.release_claim(owner, None).unwrap();
        assert_eq!(g.handoffs(), [ev("reserved", "bot")]);
        let res = g.s.reservations(p).unwrap();
        assert_eq!(res.len(), 1);
        assert_eq!(
            (res[0].pid.as_str(), &res[0].span),
            ("bot", &rows("r2", "r3"))
        );
        assert_eq!((res[0].lines.first, res[0].lines.last), (3, 4));
        assert_eq!(res[0].waiters, ["aide"]);
        // another agent can't take it, nor edit there under Respect; a person can edit
        let e =
            g.s.claim_span(aide, rows("r3", "r3"), None, None, false)
                .unwrap_err();
        assert!(
            matches!(&e, SessionError::Claimed { by, .. } if by == "bot"),
            "{e:?}"
        );
        let e = g
            .op_with(aide, ClaimPolicy::Respect, |t| {
                t.set_cell(&k("r2"), &k("c1"), "x")
            })
            .unwrap_err();
        assert!(
            matches!(&e, SessionError::Claimed { by, .. } if by == "bot"),
            "{e:?}"
        );
        g.op_with(aide, ClaimPolicy::Respect, |t| {
            t.set_cell(&k("r4"), &k("c1"), "ok")
        })
        .unwrap();
        g.op_with(p, ClaimPolicy::Respect, |t| {
            t.set_cell(&k("r2"), &k("c1"), "person")
        })
        .unwrap();
        // the waiter's claim takes it over; the rest of the queue now waits for that claim
        let c =
            g.s.claim_span(bot, rows("r2", "r2"), None, None, false)
                .unwrap();
        assert_eq!(g.handoffs().last(), Some(&ev("claimed", "bot")));
        assert!(g.s.reservations(p).unwrap().is_empty());
        assert_eq!(c.waiters, ["aide"]);
        // and when that claim ends, aide gets it
        g.s.release_claim(bot, None).unwrap();
        assert_eq!(g.handoffs().last(), Some(&ev("reserved", "aide")));
        assert_eq!(g.s.reservations(p).unwrap()[0].span, rows("r2", "r2"));
    }

    /// A grid reservation ends as a text one does: the waiter's own edit there uses it (it
    /// passes on), it lapses after the hand-off time (passes on), and, on a grid, it ends
    /// `deleted` when every row it held is deleted (nothing passes on). An expired claim
    /// hands off too.
    #[test]
    fn grid_reservations_end_edited_lapsed_or_deleted() {
        let g = gx("handoff-ends");
        let owner = g.join("p2", ParticipantKind::Agent);
        let bot = g.join("bot", ParticipantKind::Agent);
        let aide = g.join("aide", ParticipantKind::Agent);
        let p = g.join("p1", ParticipantKind::Person);
        // edited: bot's own edit in the rows uses it; aide is next
        let id =
            g.s.claim_span(owner, rows("r1", "r1"), None, None, false)
                .unwrap()
                .id;
        g.s.register_waiter(bot, id).unwrap();
        g.s.register_waiter(aide, id).unwrap();
        g.s.release_claim(owner, None).unwrap();
        g.op(bot, |t| t.set_cell(&k("r1"), &k("c2"), "mine"))
            .unwrap();
        assert_eq!(
            g.handoffs(),
            [
                ev("reserved", "bot"),
                ev("edited", "bot"),
                ev("reserved", "aide")
            ]
        );
        // lapsed: aide doesn't act within the hand-off time; nobody is next
        g.advance(2_001);
        assert!(g.s.reservations(p).unwrap().is_empty());
        assert_eq!(g.handoffs().last(), Some(&ev("lapsed", "aide")));
        // deleted: the reserved rows go; nothing passes on
        let id =
            g.s.claim_span(owner, rows("r3", "r4"), None, None, false)
                .unwrap()
                .id;
        g.s.register_waiter(bot, id).unwrap();
        g.s.register_waiter(aide, id).unwrap();
        g.s.release_claim(owner, None).unwrap();
        assert_eq!(g.handoffs().last(), Some(&ev("reserved", "bot")));
        g.op(p, |t| t.delete_row(&k("r3"))).unwrap();
        assert_eq!(g.s.reservations(p).unwrap().len(), 1, "r4 is still live");
        g.op(p, |t| t.delete_row(&k("r4"))).unwrap();
        assert_eq!(g.handoffs().last(), Some(&ev("deleted", "bot")));
        assert!(g.s.reservations(p).unwrap().is_empty(), "nothing passes on");
        // an expired claim hands off
        let id =
            g.s.claim_span(owner, rows("r1", "r2"), Some(1_000), None, false)
                .unwrap()
                .id;
        g.s.register_waiter(bot, id).unwrap();
        g.advance(1_001);
        assert_eq!(g.s.reservations(p).unwrap()[0].pid, "bot");
        assert!(g.ended().contains(&(id, ClaimEnd::Expired)));
    }

    /// A hand-off reservation expands with its anchors, as the claim it comes from does: a
    /// row inserted right after its last row joins it (another agent's edit there is then
    /// refused under Respect); a row inserted away from it doesn't; it ends `deleted` only
    /// when nothing it holds, the joined row included, is live.
    #[test]
    fn grid_reservations_expand_with_their_anchors() {
        let g = gx("handoff-expand");
        let owner = g.join("p2", ParticipantKind::Agent);
        let bot = g.join("bot", ParticipantKind::Agent);
        let aide = g.join("aide", ParticipantKind::Agent);
        let p = g.join("p1", ParticipantKind::Person);
        let id =
            g.s.claim_span(owner, rows("r2", "r3"), None, None, false)
                .unwrap()
                .id;
        g.s.register_waiter(bot, id).unwrap();
        g.s.release_claim(owner, None).unwrap();
        // a person inserts right after r3: the reservation takes it
        g.op(p, |t| {
            t.insert_row(Anchor::After(k("r3")), &["n".into()])
                .map(|x| x.1)
        })
        .unwrap();
        let res = g.s.reservations(p).unwrap();
        assert_eq!(res[0].span, rows("r2", "r5"), "{res:?}");
        assert_eq!((res[0].lines.first, res[0].lines.last), (3, 5));
        // another agent's edit in the joined row is refused under Respect
        let e = g
            .op_with(aide, ClaimPolicy::Respect, |t| {
                t.set_cell(&k("r5"), &k("c1"), "x")
            })
            .unwrap_err();
        assert!(
            matches!(&e, SessionError::Claimed { by, .. } if by == "bot"),
            "{e:?}"
        );
        // a row inserted away from it doesn't join
        g.op(p, |t| {
            t.insert_row(Anchor::Start, &["top".into()]).map(|x| x.1)
        })
        .unwrap();
        assert_eq!(g.s.reservations(p).unwrap()[0].span, rows("r2", "r5"));
        // before its first row: joins at the start
        g.op(p, |t| {
            t.insert_row(Anchor::Before(k("r2")), &["b".into()])
                .map(|x| x.1)
        })
        .unwrap();
        assert_eq!(g.s.reservations(p).unwrap()[0].span, rows("r7", "r5"));
        // its rows all deleted, the joined ones too: it ends deleted
        for r in ["r7", "r2", "r3"] {
            g.op(p, |t| t.delete_row(&k(r))).unwrap();
        }
        assert_eq!(g.s.reservations(p).unwrap().len(), 1, "r5 is still live");
        g.op(p, |t| t.delete_row(&k("r5"))).unwrap();
        assert!(g.s.reservations(p).unwrap().is_empty());
        assert_eq!(g.handoffs().last(), Some(&ev("deleted", "bot")));
    }

    /// `ClaimPolicy::Respect` on `write_base_grid` (C1 must-have (b)): an agent's merge whose
    /// applying part touches another's claim is exit 5, nothing applied; elsewhere it
    /// applies; `Ignore` and a person are never refused.
    #[test]
    fn grid_write_base_respects_claims() {
        let g = gx("write-respect");
        let owner = g.join("p2", ParticipantKind::Agent);
        let bot = g.join("bot", ParticipantKind::Agent);
        let p = g.join("p1", ParticipantKind::Person);
        g.s.claim_span(owner, rows("r2", "r2"), None, Some("mine".into()), false)
            .unwrap();
        let respect = GridWriteOptions::default().policy(ClaimPolicy::Respect);
        let base = g.s.status().unwrap().version;
        let claimed = "id,city\nk1,Oslo\nk2,Roma\nk3,Lima\nk4,Kyiv\n";
        let e =
            g.s.write_base_grid(bot, base, claimed.as_bytes(), &respect)
                .unwrap_err();
        assert!(
            matches!(&e, SessionError::Claimed { by, .. } if by == "p2"),
            "{e:?}"
        );
        assert_eq!(tools::session_exit(&e), exit::LEASED);
        assert_eq!(g.s.status().unwrap().version, base, "nothing applied");
        // a merge that doesn't touch the claimed row applies
        let elsewhere = "id,city\nk1,Oslo\nk2,Rome\nk3,Lima\nk4,Lviv\n";
        let r =
            g.s.write_base_grid(bot, base, elsewhere.as_bytes(), &respect)
                .unwrap();
        assert!(matches!(r.outcome, GridWrite::Applied(_)), "{r:?}");
        // a row inserted right after the claimed row touches it (claims expand)
        let base = g.s.status().unwrap().version;
        let edge = "id,city\nk1,Oslo\nk2,Rome\nnew,Baku\nk3,Lima\nk4,Lviv\n";
        let e =
            g.s.write_base_grid(bot, base, edge.as_bytes(), &respect)
                .unwrap_err();
        assert!(matches!(e, SessionError::Claimed { .. }), "{e:?}");
        // Ignore lets the agent through; a person is never refused
        let base = g.s.status().unwrap().version;
        let r =
            g.s.write_base_grid(
                bot,
                base,
                b"id,city\nk1,Oslo\nk2,Roma\nk3,Lima\nk4,Lviv\n",
                &GridWriteOptions::default(),
            )
            .unwrap();
        assert!(matches!(r.outcome, GridWrite::Applied(_)), "{r:?}");
        let base = g.s.status().unwrap().version;
        let person = "id,city\nk1,Oslo\nk2,Milan\nk3,Lima\nk4,Lviv\n";
        let r =
            g.s.write_base_grid(p, base, person.as_bytes(), &respect)
                .unwrap();
        assert!(matches!(r.outcome, GridWrite::Applied(_)), "{r:?}");
        // a reservation handed to someone else binds the merge too
        let id = g.s.claims(p).unwrap()[0].id;
        let w = g.join("aide", ParticipantKind::Agent);
        g.s.register_waiter(w, id).unwrap();
        g.s.release_claim(owner, None).unwrap();
        let base = g.s.status().unwrap().version;
        let again = "id,city\nk1,Oslo\nk2,Turin\nk3,Lima\nk4,Lviv\n";
        let e =
            g.s.write_base_grid(bot, base, again.as_bytes(), &respect)
                .unwrap_err();
        assert!(
            matches!(&e, SessionError::Claimed { by, .. } if by == "aide"),
            "{e:?}"
        );
    }

    /// IT `presence::grid_caret_follows_move` (K-PRES-6): a cell caret is a key pair; it
    /// follows a move and a sort with no transform, and is dropped when its row is deleted.
    #[test]
    fn grid_caret_follows_move() {
        let g = gx("caret");
        let p = g.join("p1", ParticipantKind::Person);
        let other = g.join("p2", ParticipantKind::Person);
        let c =
            g.s.set_cell_cursor(p, (&k("r2"), &k("c2")), (&k("r2"), &k("c2")))
                .unwrap();
        assert_eq!(c.at, Some((2, 2)));
        assert_eq!(c.name.as_deref(), Some("city"));
        assert_eq!(c.to_string(), "cell r2/c2 (#2, \"city\")");
        // another participant moves the row to the end: the caret goes with it
        g.op(other, |t| t.move_row(&k("r2"), Anchor::End)).unwrap();
        let c = g.cell_of(other, "p1").expect("the caret is still there");
        assert_eq!((c.anchor.clone(), c.at), ((k("r2"), k("c2")), Some((4, 2))));
        assert_eq!(c.lines.map(|l| (l.first, l.last)), Some((5, 5)));
        // a sort: still the same cell
        g.op(other, |t| {
            t.sort(&[k2_mpc_base::table::SortKey::by(k("c2"))])
        })
        .unwrap();
        let c = g.cell_of(other, "p1").unwrap();
        assert_eq!(c.head, (k("r2"), k("c2")));
        assert_eq!(c.at, Some((4, 2)), "Rome sorts last");
        // the same through At::Elem (set_cursor with cells)
        g.s.set_cursor(
            p,
            CELLS,
            k2_mpc_base::session::At::cell(&k("r1"), &k("c1")),
            k2_mpc_base::session::At::cell(&k("r3"), &k("c2")),
        )
        .unwrap();
        let c = g.cell_of(other, "p1").unwrap();
        assert_eq!((c.anchor, c.head), ((k("r1"), k("c1")), (k("r3"), k("c2"))));
        // a dead cell is refused, and a text position too
        g.op(other, |t| t.delete_row(&k("r4"))).unwrap();
        assert!(matches!(
            g.s.set_cell_cursor(p, (&k("r4"), &k("c1")), (&k("r4"), &k("c1"))),
            Err(SessionError::BadPosition(_))
        ));
        // its row deleted: the caret is dropped
        g.op(other, |t| t.delete_row(&k("r3"))).unwrap();
        assert_eq!(g.cell_of(other, "p1"), None);
        // a column delete drops a caret too
        g.s.set_cell_cursor(p, (&k("r1"), &k("c2")), (&k("r1"), &k("c2")))
            .unwrap();
        g.op(other, |t| t.delete_col(&k("c2"))).unwrap();
        assert_eq!(g.cell_of(other, "p1"), None);
    }

    /// IT `presence::claim_ends_deleted` (K-PRES-6): a claim whose rows are all deleted ends
    /// with `Deleted`; one with a row left keeps covering it (a deleted end keeps its place).
    #[test]
    fn claim_ends_deleted() {
        let g = gx("deleted");
        let a = g.join("bot", ParticipantKind::Agent);
        let p = g.join("p1", ParticipantKind::Person);
        let id =
            g.s.claim_span(a, rows("r2", "r3"), None, Some("dedupe".into()), false)
                .unwrap()
                .id;
        g.op(p, |t| t.delete_row(&k("r2"))).unwrap();
        let info = &g.s.claims(p).unwrap()[0];
        assert_eq!(
            info.span,
            rows("r2", "r3"),
            "the deleted end keeps its place"
        );
        assert_eq!(
            (info.lines.first, info.lines.last),
            (3, 3),
            "only r3 is live"
        );
        assert!(g.ended().is_empty());
        g.op(p, |t| t.delete_row(&k("r3"))).unwrap();
        assert_eq!(g.ended(), [(id, ClaimEnd::Deleted)]);
        assert!(g.s.claims(p).unwrap().is_empty());
        let act = g.s.activity(p, 10).unwrap();
        assert!(act
            .iter()
            .any(|e| e.kind == ActivityKind::ClaimEnded(ClaimEnd::Deleted)));
        // a column claim ends the same way; a cell claim when its rows (or columns) go
        let c =
            g.s.claim_span(
                a,
                ClaimSpan::Cols {
                    first: k("c2"),
                    last: k("c2"),
                },
                None,
                None,
                false,
            )
            .unwrap()
            .id;
        let cells = g.s.claim_span(
            a,
            ClaimSpan::Cells {
                rows: (k("r1"), k("r1")),
                cols: (k("c1"), k("c1")),
            },
            None,
            None,
            false,
        );
        assert!(
            matches!(cells, Err(SessionError::BadClaim(_)) | Ok(_)),
            "{cells:?}"
        );
        g.op(p, |t| t.delete_col(&k("c2"))).unwrap();
        assert!(g.ended().contains(&(c, ClaimEnd::Deleted)));
        // an undo brings the column back, but an ended claim stays ended
        assert!(g.s.claims(p).unwrap().iter().all(|i| i.id != c));
    }

    /// Claims expand with their anchors (as text claims do): a row inserted right after the
    /// last row or right before the first joins; rows moved in between are members; a row
    /// inserted elsewhere isn't.
    #[test]
    fn grid_claims_expand_with_their_anchors() {
        let g = gx("expand");
        let a = g.join("bot", ParticipantKind::Agent);
        let p = g.join("p1", ParticipantKind::Person);
        g.s.claim_span(a, rows("r2", "r3"), None, None, false)
            .unwrap();
        // inserted right after r3: joins
        g.op(p, |t| {
            t.insert_row(Anchor::After(k("r3")), &["k5".into()])
                .map(|x| x.1)
        })
        .unwrap();
        assert_eq!(g.spans(p), ["bot rows r2–r5"]);
        // inserted right before r2: joins
        g.op(p, |t| {
            t.insert_row(Anchor::Before(k("r2")), &["k6".into()])
                .map(|x| x.1)
        })
        .unwrap();
        assert_eq!(g.spans(p), ["bot rows r6–r5"]);
        let i = &g.s.claims(p).unwrap()[0];
        assert_eq!((i.lines.first, i.lines.last), (3, 6));
        // at the start, away from it: not a member
        g.op(p, |t| {
            t.insert_row(Anchor::Start, &["k7".into()]).map(|x| x.1)
        })
        .unwrap();
        let i = &g.s.claims(p).unwrap()[0];
        assert_eq!((i.lines.first, i.lines.last), (4, 7));
        // r4 moved into the middle: a member now (the span is the rows between its ends)
        g.op(p, |t| t.move_row(&k("r4"), Anchor::After(k("r2"))))
            .unwrap();
        let i = &g.s.claims(p).unwrap()[0];
        assert_eq!((i.lines.first, i.lines.last), (4, 8));
    }

    /// Where two participants' claims meet, an insert between them joins one: the inserter's
    /// own, else the one taken first. They never overlap.
    #[test]
    fn grid_claims_meeting_share_nothing() {
        let g = gx("meet");
        let a = g.join("bot", ParticipantKind::Agent);
        let b = g.join("aide", ParticipantKind::Agent);
        let p = g.join("p1", ParticipantKind::Person);
        // held: their owners' own inserts don't end them
        g.s.claim_span(a, rows("r1", "r2"), None, None, true)
            .unwrap();
        g.s.claim_span(b, rows("r3", "r4"), None, None, true)
            .unwrap();
        // p1 (no claim) inserts between r2 and r3: the first taken (bot's) takes it
        g.op(p, |t| {
            t.insert_row(Anchor::After(k("r2")), &["x".into()])
                .map(|x| x.1)
        })
        .unwrap();
        assert_eq!(g.spans(p), ["bot rows r1–r5", "aide rows r3–r4"]);
        // aide inserts between r5 and r3: aide's own claim takes it (policy Ignore)
        g.op(b, |t| {
            t.insert_row(Anchor::After(k("r5")), &["y".into()])
                .map(|x| x.1)
        })
        .unwrap();
        assert_eq!(g.spans(p), ["bot rows r1–r5", "aide rows r6–r4"]);
        // a third claim on bot's rows is refused (they overlap), naming bot
        let e =
            g.s.claim_span(b, rows("r5", "r5"), None, None, false)
                .unwrap_err();
        assert!(
            matches!(&e, SessionError::Claimed { by, .. } if by == "bot"),
            "{e:?}"
        );
        assert_eq!(tools::session_exit(&e), exit::LEASED);
    }

    /// The claim policy on grids (contract §6): under `Respect` an agent's change touching
    /// another's claim is exit 5, nothing applied; touching means a cell inside it, one of its
    /// rows (or columns) moved, deleted or renamed, or a row inserted between or at its ends.
    /// A person is never refused; `Ignore` refuses nobody.
    #[test]
    fn grid_respect_refuses_agents_touching_a_claim() {
        let g = gx("respect");
        let p2 = g.join("p2", ParticipantKind::Agent);
        let bot = g.join("bot", ParticipantKind::Agent);
        let p = g.join("p1", ParticipantKind::Person);
        g.s.claim_span(p2, rows("r2", "r2"), None, Some("mine".into()), false)
            .unwrap();
        let v0 = g.s.status().unwrap().version;
        let refused = |r: Result<u64, SessionError>| {
            assert!(
                matches!(&r, Err(SessionError::Claimed { by, .. }) if by == "p2"),
                "{r:?}"
            );
        };
        refused(g.op_with(bot, ClaimPolicy::Respect, |t| {
            t.set_cell(&k("r2"), &k("c1"), "x")
        }));
        refused(g.op_with(bot, ClaimPolicy::Respect, |t| {
            t.move_row(&k("r2"), Anchor::End)
        }));
        refused(g.op_with(bot, ClaimPolicy::Respect, |t| t.delete_row(&k("r2"))));
        refused(g.op_with(bot, ClaimPolicy::Respect, |t| {
            t.insert_row(Anchor::After(k("r2")), &["edge".into()])
                .map(|x| x.1)
        }));
        // through a structured verb: exit 5, nothing applied
        let e = g
            .tool(
                bot,
                &["cell", "set", "--row", "r2", "--col", "c2", "--value", "x"],
                ClaimPolicy::Respect,
            )
            .unwrap_err();
        assert_eq!(e.exit(), exit::LEASED, "{e}");
        assert_eq!(g.s.status().unwrap().version, v0, "nothing applied");
        // elsewhere is fine; a person is never refused; Ignore lets the agent through
        g.op_with(bot, ClaimPolicy::Respect, |t| {
            t.set_cell(&k("r3"), &k("c1"), "ok")
        })
        .unwrap();
        g.op_with(bot, ClaimPolicy::Respect, |t| {
            t.move_row(&k("r4"), Anchor::Start)
        })
        .unwrap();
        let p4 = g.join("p4", ParticipantKind::Person);
        g.op_with(p4, ClaimPolicy::Respect, |t| {
            t.set_cell(&k("r2"), &k("c1"), "person")
        })
        .unwrap();
        g.op_with(bot, ClaimPolicy::Ignore, |t| {
            t.set_cell(&k("r2"), &k("c2"), "agent")
        })
        .unwrap();
        // a column claim: renaming or moving the column is touching it; moving a row isn't
        let c = g.s.claim_span(
            p,
            ClaimSpan::Cols {
                first: k("c1"),
                last: k("c1"),
            },
            None,
            None,
            false,
        );
        assert!(
            matches!(c, Err(SessionError::Claimed { .. })),
            "c1 crosses p2's row: {c:?}"
        );
        g.s.release_claim(p2, None).unwrap();
        g.s.claim_span(
            p,
            ClaimSpan::Cols {
                first: k("c1"),
                last: k("c1"),
            },
            None,
            None,
            false,
        )
        .unwrap();
        refused_by(
            g.op_with(bot, ClaimPolicy::Respect, |t| t.rename_col(&k("c1"), "who")),
            "p1",
        );
        refused_by(
            g.op_with(bot, ClaimPolicy::Respect, |t| {
                t.move_col(&k("c1"), Anchor::End)
            }),
            "p1",
        );
        refused_by(
            g.op_with(bot, ClaimPolicy::Respect, |t| {
                t.set_cell(&k("r1"), &k("c1"), "z")
            }),
            "p1",
        );
        g.op_with(bot, ClaimPolicy::Respect, |t| {
            t.set_cell(&k("r1"), &k("c2"), "z")
        })
        .unwrap();
        g.op_with(bot, ClaimPolicy::Respect, |t| {
            t.move_row(&k("r1"), Anchor::End)
        })
        .unwrap();
    }

    fn refused_by(r: Result<u64, SessionError>, who: &str) {
        assert!(
            matches!(&r, Err(SessionError::Claimed { by, .. }) if by == who),
            "{r:?}"
        );
    }

    /// The claimant's own change touching its claim ends it (`Edited`) unless it is held.
    #[test]
    fn grid_own_edit_ends_claim_unless_held() {
        let g = gx("own");
        let a = g.join("bot", ParticipantKind::Agent);
        let id =
            g.s.claim_span(a, rows("r1", "r2"), None, None, false)
                .unwrap()
                .id;
        g.op(a, |t| t.set_cell(&k("r3"), &k("c1"), "elsewhere"))
            .unwrap();
        assert!(g.ended().is_empty(), "a change elsewhere leaves it");
        g.op(a, |t| t.set_cell(&k("r2"), &k("c1"), "mine")).unwrap();
        assert_eq!(g.ended(), [(id, ClaimEnd::Edited)]);
        let held =
            g.s.claim_span(a, rows("r1", "r2"), None, None, true)
                .unwrap()
                .id;
        g.op(a, |t| t.set_cell(&k("r1"), &k("c1"), "held")).unwrap();
        assert!(!g.ended().iter().any(|(c, _)| *c == held));
        assert_eq!(g.s.claims(a).unwrap().len(), 1);
    }

    /// `write --base` on a grid names who claims each conflicting region (`claimed_by`), not
    /// the writer itself, nor claims elsewhere.
    #[test]
    fn grid_conflict_names_the_claimants() {
        let g = gx("conflict");
        let p2 = g.join("p2", ParticipantKind::Person);
        let bot = g.join("bot", ParticipantKind::Agent);
        let aide = g.join("aide", ParticipantKind::Agent);
        let base = g.s.status().unwrap().version;
        g.op(p2, |t| t.set_cell(&k("r2"), &k("c2"), "Paris"))
            .unwrap();
        g.s.claim_span(p2, rows("r2", "r2"), None, None, false)
            .unwrap();
        g.s.claim_span(aide, rows("r4", "r4"), None, None, false)
            .unwrap();
        g.s.claim_span(bot, rows("r1", "r1"), None, None, false)
            .unwrap();
        let body = "id,city\nk1,Bergen\nk2,Roma\nk3,Lima\nk4,Kyiv\n";
        let r =
            g.s.write_base_grid(bot, base, body.as_bytes(), &GridWriteOptions::default())
                .unwrap();
        let GridWrite::Conflicts(c) = &r.outcome else {
            panic!("{r:?}")
        };
        assert_eq!(c.len(), 1, "{c:?}");
        assert_eq!(c[0].claimed_by, ["p2"]);
        assert_eq!(c[0].written_by.as_deref(), Some("p2"));
        // a cell in a column someone claims: named too
        let c2 = g.join("p3", ParticipantKind::Agent);
        for c in [p2, aide, bot] {
            g.s.release_claim(c, None).unwrap();
        }
        g.s.claim_span(
            c2,
            ClaimSpan::Cols {
                first: k("c2"),
                last: k("c2"),
            },
            None,
            None,
            false,
        )
        .unwrap();
        let r =
            g.s.write_base_grid(bot, base, body.as_bytes(), &GridWriteOptions::default())
                .unwrap();
        let GridWrite::Conflicts(c) = &r.outcome else {
            panic!("{r:?}")
        };
        assert_eq!(c[0].claimed_by, ["p3"]);
    }

    /// A row's conflict names a claim on a column only through a cell in that row one side
    /// changed since V. Here p3 claims c1, which neither side changed in r2 (`k2,Rome`: the text
    /// and p2 both changed c2): p3 is not named.
    #[test]
    fn grid_row_conflict_skips_a_claim_on_a_column_neither_side_changed() {
        let g = gx("conflict-unchanged-col");
        let p2 = g.join("p2", ParticipantKind::Person);
        let bot = g.join("bot", ParticipantKind::Agent);
        let p3 = g.join("p3", ParticipantKind::Agent);
        let base = g.s.status().unwrap().version;
        g.op(p2, |t| t.set_cell(&k("r2"), &k("c2"), "Paris"))
            .unwrap();
        g.s.claim_span(
            p3,
            ClaimSpan::Cols {
                first: k("c1"),
                last: k("c1"),
            },
            None,
            None,
            false,
        )
        .unwrap();
        let body = "id,city\nk1,Oslo\nk2,Roma\nk3,Lima\nk4,Kyiv\n";
        let r =
            g.s.write_base_grid(bot, base, body.as_bytes(), &GridWriteOptions::default())
                .unwrap();
        let GridWrite::Conflicts(c) = &r.outcome else {
            panic!("{r:?}")
        };
        assert_eq!(c.len(), 1, "{c:?}");
        assert_eq!(
            c[0].conflict.at,
            k2_mpc_base::table::Region::Row(k("r2")),
            "{c:?}"
        );
        assert!(c[0].claimed_by.is_empty(), "{c:?}");
    }

    /// A row's conflict names a claim on a column only the text changed in that row (r2,
    /// `k2,Rome`): p2 changed its c1, the text its c2, and p3 claims c2.
    #[test]
    fn grid_row_conflict_names_a_claim_on_a_column_only_the_text_changed() {
        let g = gx("conflict-text-col");
        let p2 = g.join("p2", ParticipantKind::Person);
        let bot = g.join("bot", ParticipantKind::Agent);
        let p3 = g.join("p3", ParticipantKind::Agent);
        let base = g.s.status().unwrap().version;
        g.op(p2, |t| t.set_cell(&k("r2"), &k("c1"), "k2x")).unwrap();
        g.s.claim_span(
            p3,
            ClaimSpan::Cols {
                first: k("c2"),
                last: k("c2"),
            },
            None,
            None,
            false,
        )
        .unwrap();
        let body = "id,city\nk1,Oslo\nk2,Roma\nk3,Lima\nk4,Kyiv\n";
        let r =
            g.s.write_base_grid(bot, base, body.as_bytes(), &GridWriteOptions::default())
                .unwrap();
        let GridWrite::Conflicts(c) = &r.outcome else {
            panic!("{r:?}")
        };
        assert_eq!(c.len(), 1, "{c:?}");
        assert_eq!(
            c[0].conflict.at,
            k2_mpc_base::table::Region::Row(k("r2")),
            "{c:?}"
        );
        assert_eq!(c[0].claimed_by, ["p3"], "{c:?}");
    }

    /// Notes on a grid (contract §6): a structured verb's change and a read of the rendered
    /// file say who claimed, selected, viewed or just edited the rows they touched, by key,
    /// on stderr only (`note: ` lines); stdout never changes.
    #[test]
    fn grid_notes_name_who_is_there_by_key() {
        let g = gx("notes");
        let p2 = g.join("p2", ParticipantKind::Person);
        let bot = g.join("bot", ParticipantKind::Agent);
        let me = g.join("me", ParticipantKind::Agent);
        g.s.set_cell_cursor(p2, (&k("r2"), &k("c2")), (&k("r2"), &k("c2")))
            .unwrap();
        g.s.claim_span(
            bot,
            rows("r2", "r3"),
            Some(300_000),
            Some("dedupe".into()),
            false,
        )
        .unwrap();
        let out = g
            .tool(
                me,
                &["cell", "set", "--row", "r2", "--col", "c2", "--value", "x"],
                ClaimPolicy::Ignore,
            )
            .unwrap();
        let lines = out.notes.lines();
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("note: BOT (bot) claimed rows r2–r3 \"dedupe\"")),
            "{lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l == "note: P2 (p2) has a caret at cell r2/c2 (0 s ago)"),
            "{lines:?}"
        );
        assert!(out.notes.notes.iter().all(|n| n.span.is_some()));
        assert!(out.stdout.is_empty());
        // a cell elsewhere: no note
        g.host.now.fetch_add(10_000, Ordering::SeqCst);
        let out = g
            .tool(
                me,
                &["cell", "set", "--row", "r4", "--col", "c1", "--value", "y"],
                ClaimPolicy::Ignore,
            )
            .unwrap();
        assert!(out.notes.is_empty(), "{:?}", out.notes.lines());
        // a read: the rows on the lines shown (line 3 is r2), once the same notes may repeat
        g.host.now.fetch_add(61_000, Ordering::SeqCst);
        let out = g
            .tool(me, &["view", "--lines", "3,3"], ClaimPolicy::Ignore)
            .unwrap();
        assert_eq!(out.stdout, b"k2,x\n");
        let kinds: Vec<NoteKind> = out.notes.notes.iter().map(|n| n.kind).collect();
        assert!(
            kinds.contains(&NoteKind::Claimed) && kinds.contains(&NoteKind::Caret),
            "{kinds:?}"
        );
        // the reader's view is the row, by key: it follows a move
        let p = g.join("p1", ParticipantKind::Person);
        g.op(p, |t| t.move_row(&k("r2"), Anchor::End)).unwrap();
        let me_row =
            g.s.roster(p)
                .unwrap()
                .into_iter()
                .find(|e| e.participant.pid == "me")
                .unwrap();
        assert_eq!(me_row.view.map(|v| (v.first, v.last)), Some((5, 5)));
    }

    /// `--for-edit` on a grid read takes a short claim on the rows shown; someone else's claim
    /// there is reported instead.
    #[test]
    fn grid_for_edit_claims_the_rows_shown() {
        let g = gx("foredit");
        let a = g.join("bot", ParticipantKind::Agent);
        let b = g.join("aide", ParticipantKind::Agent);
        let out = g
            .tool(a, &["head", "-n", "3", "--for-edit"], ClaimPolicy::Ignore)
            .unwrap();
        let Some(f) = out.for_edit.as_deref() else {
            panic!("no for-edit")
        };
        let ForEdit::Claimed(c) = f else {
            panic!("{f:?}")
        };
        assert_eq!(c.span, rows("r1", "r2"), "line 1 is the header: rows r1-r2");
        assert!(c.for_edit);
        let out = g
            .tool(
                b,
                &["view", "--lines", "3,4", "--for-edit"],
                ClaimPolicy::Ignore,
            )
            .unwrap();
        assert!(
            matches!(out.for_edit.as_deref(), Some(ForEdit::Conflict(c)) if c.pid == "bot"),
            "{:?}",
            out.for_edit
        );
        // an outside edit deleting the rows ends the claim with Deleted (no actor)
        g.s.flush(a).unwrap();
        let path = g.s.file_path().unwrap();
        if let Some(path) = path {
            std::fs::write(&path, "id,city\nk3,Lima\nk4,Kyiv\n").unwrap();
            g.s.file_changed().unwrap();
            assert!(
                g.ended().iter().any(|(_, how)| *how == ClaimEnd::Deleted),
                "{:?}",
                g.ended()
            );
        }
    }

    /// The roster push's grid side (K5 with C1): a cell caret is a
    /// `RosterEvent::CellCursor`, coalesced like a text caret (moves inside one interval are
    /// one event, the last place), told again when its position changes under it (a move by
    /// someone else), and `None` when its row is deleted; an App viewer sees it by handle. A
    /// grid claim's start and end are pushed too.
    #[test]
    fn the_roster_push_carries_cell_carets() {
        use k2_mpc_base::session::{RosterEvent, Who};
        let g = gx("roster-cell");
        let watcher = g.join("w", ParticipantKind::Person);
        let app = g.join("app", ParticipantKind::App);
        let p = g.join("p1", ParticipantKind::Person);
        let other = g.join("p2", ParticipantKind::Person);
        let rx = g.s.subscribe_roster(watcher).unwrap();
        let rx_app = g.s.subscribe_roster(app).unwrap();
        let flush = || {
            g.advance(1_000);
            g.s.flush_roster().unwrap();
        };
        let drain = |rx: &std::sync::mpsc::Receiver<RosterEvent>| -> Vec<RosterEvent> {
            let mut v = Vec::new();
            while let Ok(e) = rx.try_recv() {
                v.push(e);
            }
            v
        };
        let cells = |evs: &[RosterEvent]| {
            evs.iter()
                .filter_map(|e| match e {
                    RosterEvent::CellCursor { who, cell, .. } => Some((who.clone(), cell.clone())),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        flush();
        drain(&rx);
        drain(&rx_app);
        // three moves inside one interval: one event, the last place
        for (r, c) in [("r1", "c1"), ("r2", "c1"), ("r3", "c2")] {
            g.s.set_cell_cursor(p, (&k(r), &k(c)), (&k(r), &k(c)))
                .unwrap();
        }
        flush();
        let got = cells(&drain(&rx));
        assert_eq!(got.len(), 1, "{got:?}");
        let (who, cell) = &got[0];
        assert_eq!(who, &Who::Pid("p1".into()));
        let cell = cell.clone().expect("a caret");
        assert_eq!(cell.head, (k("r3"), k("c2")));
        assert_eq!(cell.at, Some((3, 2)));
        // the App viewer's push names it by handle, never the pid
        let app_got = cells(&drain(&rx_app));
        assert_eq!(app_got.len(), 1, "{app_got:?}");
        assert!(app_got[0].0.handle().is_some(), "{app_got:?}");
        assert_eq!(
            app_got[0].1.as_ref().map(|c| c.head.clone()),
            Some(cell.head.clone())
        );
        // nothing moved: nothing told
        flush();
        assert_eq!(cells(&drain(&rx)), vec![]);
        // someone else moves its row to the end: the same cell, at its new place
        g.op(other, |t| t.move_row(&k("r3"), Anchor::End)).unwrap();
        flush();
        let got = cells(&drain(&rx));
        assert_eq!(got.len(), 1, "{got:?}");
        let moved = got[0].1.clone().expect("still there");
        assert_eq!((moved.head, moved.at), ((k("r3"), k("c2")), Some((4, 2))));
        // its row deleted: the caret is gone
        g.op(other, |t| t.delete_row(&k("r3"))).unwrap();
        flush();
        assert_eq!(cells(&drain(&rx)), vec![(Who::Pid("p1".into()), None)]);
        // a grid claim: its start and its end are pushed
        let ci =
            g.s.claim_span(p, rows("r1", "r2"), None, None, false)
                .unwrap();
        g.s.release_claim(p, Some(ci.id)).unwrap();
        flush();
        let evs = drain(&rx);
        assert!(
            evs.iter()
                .any(|e| matches!(e, RosterEvent::ClaimStarted { claim, .. } if *claim == ci.id)),
            "{evs:?}"
        );
        assert!(
            evs.iter()
                .any(|e| matches!(e, RosterEvent::ClaimEnded { claim, .. } if *claim == ci.id)),
            "{evs:?}"
        );
    }

    /// `events_since` (K3) on a grid: its changes are ops by key, which the live window
    /// (text hunks) can't replay, so a base behind the doc is `MPC-E112 base_expired` (read
    /// again), never an empty list that would read as "nothing happened"; the doc's own
    /// version gives none.
    #[test]
    fn events_since_on_a_grid_asks_for_a_reload() {
        let g = gx("events-since");
        let p = g.join("p1", ParticipantKind::Person);
        let v0 = g.s.status().unwrap().version;
        g.op(p, |t| t.set_cell(&k("r1"), &k("c2"), "Bergen"))
            .unwrap();
        let v1 = g.s.status().unwrap().version;
        assert!(v1 > v0);
        let r = g.s.events_since(p, v0).unwrap_err();
        assert_eq!((r.id(), r.exit()), ("MPC-E112", 8), "{r:?}");
        assert_eq!(g.s.events_since(p, v1).unwrap(), vec![]);
    }

    /// K5 on a grid: an App viewer's cell blame, structure blame, `write --base` conflicts
    /// and roster name the others by handle, never by pid.
    #[test]
    fn an_app_viewer_of_a_grid_sees_no_pid() {
        let g = gx("app-grid");
        for pid in ["boxxx", "appxx"] {
            g.host
                .rights
                .lock()
                .unwrap()
                .insert(pid.into(), Rights::full());
        }
        let a = g.join("anaxx", ParticipantKind::Person);
        let b = g.join("boxxx", ParticipantKind::Person);
        let app = g.join("appxx", ParticipantKind::App);
        let base = g.s.status().unwrap().version;
        g.op(a, |t| t.set_cell(&k("r1"), &k("c2"), "Bergen"))
            .unwrap();
        g.op(b, |t| t.move_row(&k("r2"), Anchor::End)).unwrap();
        g.s.claim_span(a, rows("r3", "r3"), None, None, false)
            .unwrap();
        let no_pid = |what: &str, shown: String| {
            for pid in ["anaxx", "boxxx", "appxx"] {
                assert!(
                    !shown.contains(pid),
                    "{what}: {pid} reached an App viewer: {shown}"
                );
            }
        };
        let cell = g.s.blame_cell(app, &k("r1"), &k("c2")).unwrap().unwrap();
        assert!(cell.author.starts_with('p'), "{cell:?}");
        no_pid("blame_cell", format!("{cell:?}"));
        no_pid(
            "blame_structure",
            format!("{:?}", g.s.blame_structure(app).unwrap()),
        );
        no_pid("roster", format!("{:?}", g.s.roster(app).unwrap()));
        // a write at the old base over the cell someone else wrote since: a conflict
        let body = "id,city\nk1,Paris\nk2,Rome\nk3,Lima\nk4,Kyiv\n";
        let r =
            g.s.write_base_grid(app, base, body.as_bytes(), &GridWriteOptions::default())
                .unwrap();
        assert!(matches!(r.outcome, GridWrite::Conflicts(_)), "{r:?}");
        no_pid("write_base_grid", format!("{r:?}"));
        // the same calls by a person still name pids
        let cell = g.s.blame_cell(b, &k("r1"), &k("c2")).unwrap().unwrap();
        assert_eq!(cell.author, "anaxx");
    }

    /// The roster's grid reading is cached between changes (v0.10.0): after every kind of
    /// change (a move, a sort, a row insert and delete, a column insert, a header switch) and
    /// after caret moves with no change, the caret's place in the roster and in the push is
    /// the one the doc gives now (its row's and column's live index in a fresh read).
    #[test]
    fn the_cached_grid_reading_follows_every_change() {
        use k2_mpc_base::session::{RosterEvent, View};
        let g = gx("roster-cache");
        let watcher = g.join("w", ParticipantKind::Person);
        let p = g.join("p1", ParticipantKind::Person);
        let other = g.join("p2", ParticipantKind::Person);
        let rx = g.s.subscribe_roster(watcher).unwrap();
        g.s.set_cell_cursor(p, (&k("r3"), &k("c2")), (&k("r3"), &k("c2")))
            .unwrap();
        let expect = |what: &str| {
            let t = View::of_content(&g.s.read(watcher).unwrap().content)
                .table()
                .unwrap();
            let r = t.live_rows().iter().position(|x| *x == k("r3")).unwrap() + 1;
            let c = t.live_cols().iter().position(|x| *x == k("c2")).unwrap() + 1;
            let got = g.cell_of(watcher, "p1").expect("the caret").at;
            assert_eq!(got, Some((r, c)), "{what}: the roster");
            g.advance(1_000);
            g.s.flush_roster().unwrap();
            let mut last = None;
            while let Ok(e) = rx.try_recv() {
                if let RosterEvent::CellCursor { who, cell, .. } = e {
                    if who == k2_mpc_base::session::Who::Pid("p1".into()) {
                        last = cell.map(|c| c.at);
                    }
                }
            }
            if let Some(at) = last {
                assert_eq!(at, Some((r, c)), "{what}: the push");
            }
        };
        expect("start");
        g.op(other, |t| t.move_row(&k("r3"), Anchor::Start))
            .unwrap();
        expect("a move");
        g.op(other, |t| {
            t.sort(&[k2_mpc_base::table::SortKey::by(k("c2"))])
        })
        .unwrap();
        expect("a sort");
        g.op(other, |t| {
            Ok(t.insert_row(Anchor::Start, &["k0".into(), "Bern".into()])?
                .1)
        })
        .unwrap();
        expect("a row inserted before it");
        g.op(other, |t| t.delete_row(&k("r1"))).unwrap();
        expect("a row deleted");
        g.op(other, |t| Ok(t.insert_col(Anchor::Start, Some("n"))?.1))
            .unwrap();
        expect("a column inserted before it");
        g.op(other, |t| t.set_header(false)).unwrap();
        expect("the header off");
        // caret moves with no change: still right
        for (r, c) in [("r2", "c2"), ("r3", "c2")] {
            g.s.set_cell_cursor(p, (&k(r), &k(c)), (&k(r), &k(c)))
                .unwrap();
        }
        expect("caret moves");
    }
}
