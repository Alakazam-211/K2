//! The write locks' order under a flood (`fair.rs`): with 31 agents writing beside one person,
//! on the person's doc (the doc's session lock) or on other docs in the same store (the store's
//! writer turn), a person's call is passed over by at most one agent call, and every agent
//! call by at most the bound ([`crate::fair::OVERTAKES_MAX`]).
//!
//! Plants (`MPC_BETA_PLANT`): `session-lock-barging` (the session lock's order skipped: whoever
//! is first, as before) fails `overtakes_a_person_behind_31_agents_at_most_once_each` at its
//! overtake assert; `store-gate-barging` (the store's writer turn skipped: the store library's
//! own writer lock decides) fails `overtakes_a_person_behind_31_agents_on_31_other_docs` at its
//! store assert; `fair-lane-skipped` (a person's call in the agents' lane) fails
//! `overtakes_a_person_behind_31_agents_at_most_once_each` at its person assert.
//!
//! The typist rows print what each order costs (`COST` lines: the agents' edits a second, the
//! typist's and the tick's waits), flat out and at the kit's default rate limit, on one doc and
//! on 31; `MPC_FAIR_MODE` (`barging`, `fifo`) and `MPC_FAIR_OVERTAKES` choose the order to
//! measure. Only a loose bound on the typist's worst wait is judged.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::*;
use crate::fair::probe::{self, Lock};
use crate::store::{CommitDurability, DocKey, DocStore, StoreSettings};

/// The other writers beside the person: the harness's 31 agents.
const AGENTS: usize = 31;
/// Lines per agent's part of the doc.
const PART: usize = 4;

/// Full rights, the wall clock (the tick's and the store's group timing read it).
struct H;

impl Host for H {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

const MD: TextFilePack = TextFilePack::new("markdown", 1 << 20);

/// How fast the agents write.
#[derive(Clone, Copy)]
enum Pace {
    /// Back to back, with the edit rate limit lifted (the flood the default limit prevents).
    Flat,
    /// At the kit's default rate limit (10 edits a second each), as K2 runs it.
    Default,
}

/// The kit's defaults (the rate limit lifted when flat out), room for 32 writers, and no idle
/// leave or unload during the run.
fn config(pace: Pace) -> Config {
    let d = Config::default();
    let (rate, burst) = match pace {
        Pace::Flat => (1_000_000, 1_000_000),
        Pace::Default => (d.edit_rate_per_s, d.edit_burst),
    };
    Config {
        edit_rate_per_s: rate,
        edit_burst: burst,
        participants_max: 64,
        idle_leave_ms: u64::MAX / 4,
        idle_unload_ms: u64::MAX / 4,
        ..d
    }
}

struct Room {
    dir: std::path::PathBuf,
    mgr: SessionManager,
    /// The person's doc.
    s: Arc<Session>,
    person: ConnId,
    /// Each agent's doc and connection.
    agents: Vec<(Arc<Session>, ConnId)>,
}

/// Where the agents write.
#[derive(Clone, Copy)]
enum Docs {
    /// On the person's doc.
    Same,
    /// All on one other doc in the same store.
    OneOther,
    /// Each on its own doc in the same store.
    EachOwn,
}

/// Docs of one part per agent (each agent edits only its own lines) in one store with grouped
/// saves, as the harness ran it; the person joined on the first, each agent on its doc.
fn room(tag: &str, docs: Docs, pace: Pace) -> Room {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-fair-lock-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let settings =
        StoreSettings::DEFAULT.with_commit_durability(CommitDurability::grouped(32, 100).unwrap());
    let store = Arc::new(DocStore::open_with(dir.join("docs.redb"), settings).unwrap());
    let mgr = SessionManager::new(store, Arc::new(H), config(pace));
    let seed: String = (0..AGENTS * PART)
        .map(|l| format!("a{:02} line{l:03}\n", l / PART))
        .chain(std::iter::once("typed: ".to_owned()))
        .collect();
    let open = |n: usize| {
        let k = DocKey {
            space: "ws".into(),
            path: format!("d{n:02}.md"),
        };
        mgr.open(Arc::new(MD), &k, Some(seed.as_bytes())).unwrap()
    };
    let join = |s: &Session, pid: String, kind: ParticipantKind, via: &str| {
        s.join(
            Participant::new(&pid, &pid, "#123456", kind, via),
            Stay::Held,
        )
        .unwrap()
        .conn
    };
    let s = open(0);
    let person = join(&s, "ann".into(), ParticipantKind::Person, "tab");
    let other = matches!(docs, Docs::OneOther).then(|| open(1));
    let agents = (0..AGENTS)
        .map(|i| {
            let d = match docs {
                Docs::Same => s.clone(),
                Docs::OneOther => other.clone().expect("opened"),
                Docs::EachOwn => open(i + 1),
            };
            let c = join(&d, format!("agent{i:02}"), ParticipantKind::Agent, "cli");
            (d, c)
        })
        .collect();
    Room {
        dir,
        mgr,
        s,
        person,
        agents,
    }
}

/// Agent `i`'s `round`th edit: one line of its own part rewritten.
fn agent_edit(s: &Session, c: ConnId, i: usize, round: u64) {
    try_agent_edit(s, c, i, round)
        .unwrap()
        .expect("the agent's edit changes its line");
}

/// [`agent_edit`], refusals returned (the rate limit, at the default pace).
fn try_agent_edit(
    s: &Session,
    c: ConnId,
    i: usize,
    round: u64,
) -> std::result::Result<Option<EditResult>, EditWithError<()>> {
    let line = i * PART + (round as usize % PART);
    s.edit_with(c, TextFilePack::ROOT, |t| {
        let mut out = String::with_capacity(t.len() + 16);
        for (l, text) in t.split_inclusive('\n').enumerate() {
            if l == line {
                out.push_str(&format!("a{i:02} round{round:07}\n"));
            } else {
                out.push_str(text);
            }
        }
        Ok::<_, ()>(out)
    })
}

/// The person's `k`th keystroke, at the doc's end.
fn keystroke(s: &Session, c: ConnId, k: u64) {
    let ch = (b'a' + (k % 26) as u8) as char;
    s.edit_with(c, TextFilePack::ROOT, |t| Ok::<_, ()>(format!("{t}{ch}")))
        .unwrap()
        .expect("the keystroke changes the text");
}

/// What one flood saw, each as (session lock, store's writer turn): per person edit, the times
/// it was overtaken; the agents' worst; the times every call was overtaken in all; and the
/// agents' edits.
struct Overtaken {
    person: Vec<(u64, u64)>,
    agents: (u64, u64),
    /// The manager's tick's worst (it ticks every 20 ms through the flood).
    tick: (u64, u64),
    total: (u64, u64),
    agent_edits: u64,
}

/// Timing-free: the agents flat out, the manager's tick every 20 ms, and the person making
/// `person_edits` edits back to back.
/// Each call to a lock is numbered on arrival (`fair::probe`); a call let in while earlier
/// arrivals still wait overtakes each of them. Each agent stops when the person is done, or
/// after 400 edits (so a run where the person is starved still ends).
fn flood(r: &Room, label: &str, person_edits: u64) -> Overtaken {
    const AGENT_CAP: u64 = 400;
    let done = AtomicBool::new(false);
    let started = std::sync::Barrier::new(AGENTS + 1);
    // this thread's (worst, total) at each lock since the last take
    let take = || (probe::take(Lock::Session), probe::take(Lock::Store));
    let o = std::thread::scope(|sc| {
        let handles: Vec<_> = r
            .agents
            .iter()
            .enumerate()
            .map(|(i, (d, c))| {
                let (done, started) = (&done, &started);
                sc.spawn(move || {
                    started.wait();
                    take();
                    let mut round = 0u64;
                    while !done.load(Ordering::SeqCst) && round < AGENT_CAP {
                        agent_edit(d, *c, i, round);
                        round += 1;
                    }
                    (take(), round)
                })
            })
            .collect();
        let ticker = {
            let (mgr, done) = (&r.mgr, &done);
            sc.spawn(move || {
                let mut worst = (0u64, 0u64);
                while !done.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(20));
                    take();
                    mgr.tick().expect("the tick");
                    let ((sw, _), (gw, _)) = take();
                    worst = (worst.0.max(sw), worst.1.max(gw));
                }
                worst
            })
        };
        started.wait();
        let mut person = Vec::with_capacity(person_edits as usize);
        let mut total = (0u64, 0u64);
        for k in 0..person_edits {
            take();
            keystroke(&r.s, r.person, k);
            let ((sw, st), (gw, gt)) = take();
            person.push((sw, gw));
            total = (total.0 + st, total.1 + gt);
        }
        done.store(true, Ordering::SeqCst);
        let (mut agents, mut agent_edits) = ((0u64, 0u64), 0u64);
        for h in handles {
            let (((sw, st), (gw, gt)), n) = h.join().expect("agent thread");
            agents = (agents.0.max(sw), agents.1.max(gw));
            total = (total.0 + st, total.1 + gt);
            agent_edits += n;
        }
        Overtaken {
            person,
            agents,
            tick: ticker.join().expect("tick thread"),
            total,
            agent_edits,
        }
    });
    let max = |f: fn(&(u64, u64)) -> u64| o.person.iter().map(f).max().unwrap_or(0);
    eprintln!(
        "OVERTAKES {label}: person per edit (session, store) {:?}; person worst session {} store \
         {}; agents worst session {} store {}; tick worst session {} store {}; in all session {} \
         store {}; agent edits {}",
        o.person,
        max(|p| p.0),
        max(|p| p.1),
        o.agents.0,
        o.agents.1,
        o.tick.0,
        o.tick.1,
        o.total.0,
        o.total.1,
        o.agent_edits
    );
    assert!(
        o.agent_edits >= AGENTS as u64,
        "control: the agents wrote while the person typed ({} agent edits)",
        o.agent_edits
    );
    o
}

/// The bound on an agent's call: overtaken by at most this many agent calls.
fn agent_bound() -> u64 {
    u64::from(crate::fair::overtakes_max())
}

/// Every person edit and every tick passed over by at most one agent call, and every agent
/// call by at most the bound, at the lock `which` picks (0: the session lock, 1: the store's
/// writer turn).
fn assert_bounds(o: &Overtaken, which: usize, at: &str) {
    let pick = |p: &(u64, u64)| if which == 0 { p.0 } else { p.1 };
    for (k, p) in o.person.iter().enumerate() {
        let n = pick(p);
        assert!(
            n <= 1,
            "row: person edit {k} was overtaken {n} times at the {at} by agent calls that \
             arrived after it (bound 1)"
        );
    }
    let n = pick(&o.tick);
    assert!(
        n <= 1,
        "row: a tick was overtaken {n} times at the {at} by agent calls that arrived after it \
         (bound 1)"
    );
    let n = pick(&o.agents);
    let max = agent_bound();
    assert!(
        n <= max,
        "row: an agent call was overtaken {n} times at the {at} by agent calls that arrived \
         after it (bound {max})"
    );
}

/// 31 agents flat out on the person's doc, the person making 100 edits: at the doc's session
/// lock, every person edit is overtaken by at most one agent call and every agent call by at
/// most the bound.
#[test]
fn overtakes_a_person_behind_31_agents_at_most_once_each() {
    let r = room("overtakes", Docs::Same, Pace::Flat);
    let o = flood(&r, "same doc", 100);
    assert_bounds(&o, 0, "session lock");
    r.mgr.release().unwrap();
    let _ = std::fs::remove_dir_all(&r.dir);
}

/// 31 agents flat out on one other doc in the same store, the person alone on hers: at the
/// store's writer turn (two writers there at a time: each doc's session lock lets one through).
#[test]
fn overtakes_a_person_on_one_doc_behind_31_agents_on_another() {
    let r = room("other-doc", Docs::OneOther, Pace::Flat);
    let o = flood(&r, "one other doc", 40);
    assert_bounds(&o, 1, "store's writer turn");
    r.mgr.release().unwrap();
    let _ = std::fs::remove_dir_all(&r.dir);
}

/// 31 agents flat out each on its own doc in the same store (31 writers at the store at once),
/// the person alone on hers, making 200 edits: at the store's writer turn.
#[test]
fn overtakes_a_person_behind_31_agents_on_31_other_docs() {
    let r = room("other-docs", Docs::EachOwn, Pace::Flat);
    let o = flood(&r, "31 other docs", 200);
    assert_bounds(&o, 1, "store's writer turn");
    r.mgr.release().unwrap();
    let _ = std::fs::remove_dir_all(&r.dir);
}

/// The order measured: `MPC_FAIR_MODE`, the bound, and the plant.
fn mode_label() -> String {
    let var = |k: &str| std::env::var(k).unwrap_or_else(|_| "none".into());
    format!(
        "mode={} n={} plant={}",
        std::env::var("MPC_FAIR_MODE").unwrap_or_else(|_| "bounded".into()),
        agent_bound(),
        var("MPC_BETA_PLANT")
    )
}

/// The harness's `w32-typist` shape, printed: 31 agents (`docs`, `pace`), the person at 8
/// keystrokes a second, the manager's tick every 250 ms. Prints the typist's p50 and worst
/// wait, the agents' worst and their edits a second, and the tick's p50 and worst; judges
/// only a loose bound on the typist's worst wait (2 s). The agents stop when the typist is
/// done, or after 20 s (so a run where the typist is starved still ends).
fn typist(tag: &str, docs: Docs, pace: Pace) {
    const KEYS: u64 = 24;
    const TYPING: Duration = Duration::from_millis(125);
    const TICK: Duration = Duration::from_millis(250);
    const AGENT_GAP: Duration = Duration::from_millis(100);
    const AGENTS_FOR_AT_MOST: Duration = Duration::from_secs(20);
    let r = room(tag, docs, pace);
    let done = AtomicBool::new(false);
    let ticks = Mutex::new(Vec::<Duration>::new());
    let started = std::sync::Barrier::new(AGENTS + 1);
    let t0 = Instant::now();
    let (typist, agents_worst, agent_edits, refused, agents_wall) = std::thread::scope(|sc| {
        let handles: Vec<_> = r
            .agents
            .iter()
            .enumerate()
            .map(|(i, (d, c))| {
                let (done, started) = (&done, &started);
                sc.spawn(move || {
                    started.wait();
                    let (mut worst, mut round, mut refused) = (Duration::ZERO, 0u64, 0u64);
                    let mut next = Instant::now();
                    while !done.load(Ordering::SeqCst) && t0.elapsed() < AGENTS_FOR_AT_MOST {
                        if let Pace::Default = pace {
                            let now = Instant::now();
                            if next > now {
                                std::thread::sleep(next - now);
                            }
                            next = next.max(Instant::now()) + AGENT_GAP;
                        }
                        let t = Instant::now();
                        match try_agent_edit(d, *c, i, round) {
                            Ok(_) => round += 1,
                            Err(EditWithError::Session(SessionError::RateLimited { .. })) => {
                                refused += 1
                            }
                            Err(e) => panic!("agent {i}: {e:?}"),
                        }
                        worst = worst.max(t.elapsed());
                    }
                    (worst, round, refused, t0.elapsed())
                })
            })
            .collect();
        {
            let (mgr, done, ticks) = (&r.mgr, &done, &ticks);
            sc.spawn(move || {
                while !done.load(Ordering::SeqCst) {
                    std::thread::sleep(TICK);
                    let t = Instant::now();
                    mgr.tick().expect("the tick");
                    ticks.lock().unwrap().push(t.elapsed());
                }
            });
        }
        started.wait();
        let mut waits = Vec::with_capacity(KEYS as usize);
        let mut next = Instant::now();
        for k in 0..KEYS {
            let now = Instant::now();
            if next > now {
                std::thread::sleep(next - now);
            }
            next = next.max(Instant::now()) + TYPING;
            let t = Instant::now();
            keystroke(&r.s, r.person, k);
            waits.push(t.elapsed());
        }
        done.store(true, Ordering::SeqCst);
        let (mut worst, mut edits, mut refused, mut wall) = (Duration::ZERO, 0, 0, Duration::ZERO);
        for h in handles {
            let (w, n, x, at) = h.join().expect("agent thread");
            worst = worst.max(w);
            edits += n;
            refused += x;
            wall = wall.max(at);
        }
        (waits, worst, edits, refused, wall)
    });
    let pct = |v: &[Duration], q: usize| {
        let mut v = v.to_vec();
        v.sort();
        match v.len() {
            0 => Duration::ZERO,
            n => v[(n * q / 100).min(n - 1)],
        }
    };
    let ticks = ticks.into_inner().unwrap();
    let ms = |d: Duration| d.as_secs_f64() * 1e3;
    let worst = pct(&typist, 100);
    eprintln!(
        "COST {tag} {}: agent edits/s {:.0} ({agent_edits} edits, {refused} refused, over \
         {:.2} s); typist p50 {:.1} ms worst {:.1} ms; agents worst {:.1} ms; tick p50 {:.1} \
         ms worst {:.1} ms over {} ticks",
        mode_label(),
        agent_edits as f64 / agents_wall.as_secs_f64().max(1e-9),
        agents_wall.as_secs_f64(),
        ms(pct(&typist, 50)),
        ms(worst),
        ms(agents_worst),
        ms(pct(&ticks, 50)),
        ms(pct(&ticks, 100)),
        ticks.len()
    );
    assert!(
        agent_edits >= AGENTS as u64,
        "control: the agents wrote while the typist typed ({agent_edits} agent edits)"
    );
    assert!(
        worst < Duration::from_secs(2),
        "row: the typist's worst keystroke waited {:.1} ms behind the agents (loose bound 2 s)",
        ms(worst)
    );
    r.mgr.release().unwrap();
    let _ = std::fs::remove_dir_all(&r.dir);
}

/// 31 agents flat out on the typist's doc.
#[test]
fn a_typist_beside_31_agents_flat_out_waits_one_round() {
    typist("flat-1doc", Docs::Same, Pace::Flat);
}

/// 31 agents flat out each on its own doc in the typist's store.
#[test]
fn a_typist_beside_31_agents_flat_out_on_31_other_docs() {
    typist("flat-31docs", Docs::EachOwn, Pace::Flat);
}

/// 31 agents at the kit's default rate limit on the typist's doc.
#[test]
fn a_typist_beside_31_agents_at_the_default_rate() {
    typist("default-1doc", Docs::Same, Pace::Default);
}

/// 31 agents at the kit's default rate limit each on its own doc in the typist's store.
#[test]
fn a_typist_beside_31_agents_at_the_default_rate_on_31_other_docs() {
    typist("default-31docs", Docs::EachOwn, Pace::Default);
}

/// One seed of the session stress row: what it saw.
fn session_stress(seed: u64) -> String {
    use crate::fair_tests::{watch_progress, Rng};
    const SLOTS: usize = 12;
    const EDITS: u64 = 1_500;
    let mut rng = Rng::new(seed);
    let docs = [1usize, 4][(seed % 2) as usize];
    let dir = std::env::temp_dir().join(format!(
        "k2mb-fair-stress-{seed}-{}-{}",
        std::process::id(),
        rng.next()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let settings =
        StoreSettings::DEFAULT.with_commit_durability(CommitDurability::grouped(32, 100).unwrap());
    let store = Arc::new(DocStore::open_with(dir.join("docs.redb"), settings).unwrap());
    let mgr = SessionManager::new(store, Arc::new(H), config(Pace::Flat));
    let text: String = (0..SLOTS).map(|i| format!("slot{i:02} line\n")).collect();
    let sessions: Vec<Arc<Session>> = (0..docs)
        .map(|n| {
            let k = DocKey {
                space: "ws".into(),
                path: format!("s{n:02}.md"),
            };
            mgr.open(Arc::new(MD), &k, Some(text.as_bytes())).unwrap()
        })
        .collect();
    let claimed = AtomicU64::new(0);
    let done = AtomicU64::new(0);
    let finished = AtomicBool::new(false);
    let (joins, ticks) = (AtomicU64::new(0), AtomicU64::new(0));
    let bad = Mutex::new(Vec::<String>::new());
    let report = |what: String| {
        let mut b = bad.lock().unwrap();
        if b.len() < 20 {
            b.push(what);
        }
    };
    let check = |who: &str, prompt: bool| {
        let (sw, _) = probe::take(Lock::Session);
        let (gw, _) = probe::take(Lock::Store);
        let max = if prompt { 1 } else { agent_bound() };
        if sw > max || gw > max {
            report(format!(
                "{who} was overtaken by agent calls {sw} times at a session lock and {gw} at the \
                 store (bound {max})"
            ));
        }
    };
    std::thread::scope(|sc| {
        for slot in 0..SLOTS {
            let slot_seed = rng.next();
            let (sessions, claimed, done, joins, check) =
                (&sessions, &claimed, &done, &joins, &check);
            sc.spawn(move || {
                let mut r = Rng::new(slot_seed);
                let mut k = 0u64;
                while claimed.load(Ordering::SeqCst) < EDITS {
                    // a participant joins a doc, makes some edits, and leaves
                    let s = &sessions[r.below(sessions.len() as u64) as usize];
                    let person = r.below(4) == 0;
                    let kind = if person {
                        ParticipantKind::Person
                    } else {
                        ParticipantKind::Agent
                    };
                    let pid = format!("p{slot:02}-{k}");
                    k += 1;
                    let c = s
                        .join(
                            Participant::new(&pid, &pid, "#123456", kind, "cli"),
                            Stay::Held,
                        )
                        .unwrap()
                        .conn;
                    joins.fetch_add(1, Ordering::SeqCst);
                    for _ in 0..1 + r.below(30) {
                        if claimed.fetch_add(1, Ordering::SeqCst) >= EDITS {
                            break;
                        }
                        let spins = [0, 0, 50, 5_000][r.below(4) as usize];
                        let round = r.next() % 1_000_000;
                        probe::take(Lock::Session);
                        probe::take(Lock::Store);
                        s.edit_with(c, TextFilePack::ROOT, |t| {
                            (0..spins).for_each(|_| std::hint::spin_loop());
                            let mut out = String::with_capacity(t.len() + 16);
                            for (l, line) in t.split_inclusive('\n').enumerate() {
                                if l == slot {
                                    out.push_str(&format!("slot{slot:02} {round}\n"));
                                } else {
                                    out.push_str(line);
                                }
                            }
                            Ok::<_, ()>(out)
                        })
                        .unwrap();
                        check(
                            if person {
                                "a person's edit"
                            } else {
                                "an agent's edit"
                            },
                            person,
                        );
                        done.fetch_add(1, Ordering::SeqCst);
                    }
                    s.leave(c).unwrap();
                }
            });
        }
        {
            let (mgr, finished, ticks, check) = (&mgr, &finished, &ticks, &check);
            sc.spawn(move || {
                while !finished.load(Ordering::SeqCst) {
                    probe::take(Lock::Session);
                    probe::take(Lock::Store);
                    mgr.tick().expect("the tick");
                    check("a tick", true);
                    ticks.fetch_add(1, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(1));
                }
            });
        }
        watch_progress(&done, EDITS, || {
            let mut out: Vec<String> = sessions
                .iter()
                .map(|s| format!("session {:?}", s.lock_snapshot_for_tests()))
                .collect();
            out.push(format!("joins {}", joins.load(Ordering::SeqCst)));
            out.join("; ")
        });
        finished.store(true, Ordering::SeqCst);
    });
    let bad = bad.into_inner().unwrap();
    assert!(bad.is_empty(), "row: seed {seed} (docs {docs}): {bad:?}");
    for (i, s) in sessions.iter().enumerate() {
        assert_eq!(
            s.lock_snapshot_for_tests(),
            (false, 0, false, false, 0),
            "row: seed {seed}: doc {i}'s lock left with a holder, waiters or a hand-off"
        );
    }
    let line = format!(
        "seed {seed} docs {docs}: {} edits, {} participants joined and left, {} ticks",
        done.load(Ordering::SeqCst),
        joins.load(Ordering::SeqCst),
        ticks.load(Ordering::SeqCst)
    );
    drop(sessions);
    mgr.release().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    line
}

/// Random joins and leaves of people and agents on 1 or 4 docs in one store, random hold times
/// (zero included), the tick throughout: every edit completes (a watchdog stops the run when
/// none does for 30,000 polls of 1 ms), people and ticks are overtaken by at most one agent
/// call and agents by at most the bound, and each doc's lock ends clear. Seeds:
/// `MPC_FAIR_SESSION_SEEDS` (default 2), from `MPC_FAIR_STRESS_FROM` (default 0).
#[test]
fn sessions_stay_live_and_bounded_as_people_and_agents_come_and_go() {
    let var = |k: &str, d: u64| {
        std::env::var(k)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(d)
    };
    let (from, seeds) = (
        var("MPC_FAIR_STRESS_FROM", 0),
        var("MPC_FAIR_SESSION_SEEDS", 2),
    );
    for seed in from..from + seeds {
        eprintln!("SESSION-STRESS {}", session_stress(seed));
    }
    eprintln!("SESSION-STRESS done: {seeds} seeds from {from}");
}

/// An agent's edit whose computation unwinds while a person and two agents wait for the doc:
/// as before, the session lets the lock go before the unwind reaches the caller, so the
/// waiters' edits go through, the doc's lock ends clear (nobody left waiting, due or in the
/// lane), and the session stays usable.
#[test]
fn an_edit_that_unwinds_leaves_the_session_usable_and_its_lock_clear() {
    let r = room("unwind", Docs::Same, Pace::Flat);
    let (s, person) = (&r.s, r.person);
    std::thread::scope(|sc| {
        let (inside_tx, inside_rx) = std::sync::mpsc::channel();
        let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
        let a0 = r.agents[0].1;
        let unwinding = sc.spawn(move || {
            let _ = s.edit_with(
                a0,
                TextFilePack::ROOT,
                |_| -> std::result::Result<String, ()> {
                    inside_tx.send(()).unwrap();
                    go_rx.recv().unwrap();
                    panic!("the computation unwinds");
                },
            );
        });
        inside_rx.recv().unwrap();
        let waiters: Vec<_> = [(person, 0u64), (r.agents[1].1, 1), (r.agents[2].1, 2)]
            .into_iter()
            .map(|(c, i)| {
                sc.spawn(move || {
                    if i == 0 {
                        keystroke(s, c, 0);
                    } else {
                        agent_edit(s, c, i as usize, 0);
                    }
                })
            })
            .collect();
        let queued = AtomicU64::new(0);
        std::thread::scope(|w| {
            w.spawn(|| {
                while s.lock_snapshot_for_tests().1 < 3 {
                    std::thread::yield_now();
                }
                queued.store(3, Ordering::SeqCst);
            });
            crate::fair_tests::watch_progress(&queued, 3, || {
                format!("{:?}", s.lock_snapshot_for_tests())
            });
        });
        go_tx.send(()).unwrap();
        assert!(
            unwinding.join().is_err(),
            "control: the computation unwound"
        );
        for (i, w) in waiters.into_iter().enumerate() {
            assert!(w.join().is_ok(), "row: waiter {i}'s edit went through");
        }
    });
    assert_eq!(
        s.lock_snapshot_for_tests(),
        (false, 0, false, false, 0),
        "row: the doc's lock is clear after the unwind"
    );
    keystroke(s, person, 1);
    r.mgr.release().unwrap();
    let _ = std::fs::remove_dir_all(&r.dir);
}
