//! Agent presence (amendment 13): claims and their ends, waiters, the claim policy, `--for-edit`,
//! heads-up notes, the activity ring, richer roster rows, conflict reports, and the rule that
//! none of it ever reaches the store. Public API only.

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
