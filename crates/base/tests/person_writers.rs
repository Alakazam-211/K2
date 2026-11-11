//! Person-writer reuse (`Config::reuse_person_writers`, v0.14.0;
//! `docs/design/person-writer-reuse-r0.md` §5): one writer per person per device.
//!
//! The rows, each on Yrs through sessions; the text rows' writer schedules are also replayed on
//! the native engine (feature `native`), and every text row's store log is replayed through the
//! client admission with a check that no entry reuses a clock of its writer:
//! - `r1_two_tabs` (R1), `r2_reconnect_half_open` (R2), `r3_crash_with_pending_edits` (R3),
//!   `r4_offline_device_after_reload` (R4), `r5_stale_lease_after_expiry` and
//!   `r5_returning_tab_after_expiry_lands_its_hunks_once` (R5), `r6_*` (R6), `r7_*` (the
//!   in-crate N−1 view; the cross-kit run is `tools/n1`), `r8_long_history` (R8),
//!   `r10_one_store_one_session` (R10), `r12_tag_keys_across_hand_over` (R12).
//!
//! Planted faults (`MPC_STRUCT_MUTANT`, the setting echoed), each caught by name:
//! - `reuse-person-held` (R9, `ClientMutant::ReusePersonHeld`): `r1_two_tabs` fails at "two
//!   tabs of one device hold distinct writers";
//! - `reuse-parked` (R11, `ClientMutant::ReuseParked`): `r3_crash_with_pending_edits` fails at
//!   "a parked writer never goes to another connection";
//! - `handover-without-close` (R13, `ClientMutant::HandoverWithoutClose`):
//!   `r2_reconnect_half_open` fails at "the old connection is closed by the hand-over".

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::session::{
    AuditEvent, Config, ConnId, EditOptions, Host, JoinOptions, Joined, Pack, Participant,
    ParticipantKind, Rights, Session, SessionManager, Stay, TextFilePack, WriterLease,
};
use k2_mpc_base::store::{DocId, DocKey, DocStore, EntryBody};
use k2_mpc_base::text::diff::Hunk;
use k2_mpc_base::{ClientId, Doc, Edit};

fn under_mutant<R>(f: impl FnOnce() -> R) -> R {
    use k2_mpc_base::differential::{with_client_mutant, ClientMutant};
    let m = std::env::var("MPC_STRUCT_MUTANT");
    println!("MPC_STRUCT_MUTANT={}", m.as_deref().unwrap_or("(unset)"));
    match m.as_deref() {
        Ok("reuse-person-held") => with_client_mutant(ClientMutant::ReusePersonHeld, f),
        Ok("reuse-parked") => with_client_mutant(ClientMutant::ReuseParked, f),
        Ok("handover-without-close") => with_client_mutant(ClientMutant::HandoverWithoutClose, f),
        Ok(other) => panic!("MPC_STRUCT_MUTANT={other}: not a mutant this suite plants"),
        Err(_) => f(),
    }
}

const MD: TextFilePack = TextFilePack::new("markdown", 1 << 20);
const ROOT: &str = TextFilePack::ROOT;
const START: &str = "# Title\n\nbody\n";

/// A clock the rows can move on (park expiry).
struct H(AtomicU64);
impl Host for H {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

fn key() -> DocKey {
    DocKey {
        space: "ws".into(),
        path: "a.md".into(),
    }
}

fn config(park_ms: u64) -> Config {
    let mut c = unlimited();
    c.reuse_person_writers = true;
    c.person_writer_park_ms = park_ms;
    c
}

/// The defaults, without the edit rate limit (the rows edit fast, on a clock of their own).
fn unlimited() -> Config {
    let mut c = Config::default();
    c.edit_rate_per_s = 1 << 30;
    c.edit_burst = 1 << 30;
    c
}

/// Every edit a row made, as the session applied it: the writer and the applied hunks (the
/// writer schedule the native column replays).
#[derive(Default)]
struct Trace(Vec<(ClientId, Vec<Hunk>)>);

struct Fx {
    dir: std::path::PathBuf,
    host: Arc<H>,
    config: Config,
    mgr: Option<SessionManager>,
    st: Option<Arc<DocStore>>,
    trace: Trace,
}

impl Drop for Fx {
    fn drop(&mut self) {
        if let Some(m) = self.mgr.take() {
            let _ = m.release();
        }
        self.st = None;
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fx(name: &str, config: Config) -> Fx {
    let dir = std::env::temp_dir().join(format!("k2mpc-pw-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let host = Arc::new(H(AtomicU64::new(1)));
    let mut f = Fx {
        dir,
        host,
        config,
        mgr: None,
        st: None,
        trace: Trace::default(),
    };
    f.start();
    f
}

impl Fx {
    fn start(&mut self) {
        let store = Arc::new(DocStore::open(self.dir.join("d.redb")).unwrap());
        self.st = Some(store.clone());
        self.mgr = Some(SessionManager::new(
            store,
            self.host.clone(),
            self.config.clone(),
        ));
    }

    fn mgr(&self) -> &SessionManager {
        self.mgr.as_ref().unwrap()
    }

    fn s(&self) -> Arc<Session> {
        self.mgr()
            .open(Arc::new(MD), &key(), Some(START.as_bytes()))
            .unwrap()
    }

    /// The host closes the doc's session (an unload): every lease ends.
    fn close(&self) {
        self.mgr().close(&key(), "test").unwrap();
    }

    /// A new process: the manager goes, the store file is opened again.
    fn restart(&mut self) {
        self.close();
        self.mgr.take().unwrap().release().unwrap();
        self.st = None;
        self.start();
    }

    fn store(&self) -> Arc<DocStore> {
        self.st.clone().expect("the store is open")
    }

    fn doc_id(&self) -> DocId {
        self.store()
            .find(&key())
            .unwrap()
            .expect("the doc is stored")
    }

    fn clock_skip(&self, ms: u64) {
        self.host.0.fetch_add(ms, Ordering::SeqCst);
    }

    /// Append `text` at the end of the root as `conn` (writer `client`), at the version it
    /// read: the trace records the hunks as applied.
    fn type_at_end(&mut self, s: &Session, conn: ConnId, client: ClientId, text: &str) -> u64 {
        let r = s.read(conn).unwrap();
        let now = &r.text_by_root.iter().find(|(x, _)| x == ROOT).unwrap().1;
        let at = now.len();
        self.edit_at(s, conn, client, r.version, at, text)
    }

    /// Insert `text` at byte `at` of the text at `base` as `conn` (writer `client`).
    fn edit_at(
        &mut self,
        s: &Session,
        conn: ConnId,
        client: ClientId,
        base: u64,
        at: usize,
        text: &str,
    ) -> u64 {
        let out = s
            .edit_at_base(
                conn,
                ROOT,
                base,
                vec![Hunk {
                    start: at,
                    end: at,
                    insert: text.to_owned(),
                }],
                &EditOptions::new(),
            )
            .unwrap_or_else(|e| panic!("edit as {client}: {e:?}"));
        if !out.hunks.is_empty() {
            self.trace.0.push((client, out.hunks.clone()));
        }
        out.result.version
    }
}

fn person(pid: &str) -> Participant {
    Participant::new(pid, pid, "#7c3aed", ParticipantKind::Person, "tab")
}

fn tab(s: &Session, pid: &str, dev: &str) -> Joined {
    s.join_with(person(pid), Stay::Held, &JoinOptions::new().device(dev))
        .unwrap()
}

fn reclaim(s: &Session, pid: &str, dev: &str, lease: WriterLease) -> Joined {
    s.join_with(
        person(pid),
        Stay::Held,
        &JoinOptions::new().device(dev).reclaim(lease),
    )
    .unwrap()
}

fn text(s: &Session, conn: ConnId) -> String {
    let r = s.read(conn).unwrap();
    r.text_by_root
        .into_iter()
        .find(|(x, _)| x == ROOT)
        .unwrap()
        .1
}

/// No two present connections hold one writer.
fn one_holder_each(s: &Session) -> bool {
    let m = s.member_clients_for_tests().unwrap();
    let set: BTreeSet<ClientId> = m.iter().map(|(_, c)| *c).collect();
    set.len() == m.len()
}

/// The writers the doc's log holds changes of (the import's included), by the store's log.
fn log_writers(store: &DocStore, id: DocId) -> BTreeSet<ClientId> {
    store
        .load(id)
        .unwrap()
        .updates
        .iter()
        .map(|(_, e)| e.client)
        .collect()
}

/// A Yrs v1 state vector: client → clock.
fn sv(doc: &Doc) -> BTreeMap<u64, u64> {
    let b = doc.state_vector();
    let mut i = 0;
    let mut var = || {
        let (mut v, mut shift) = (0u64, 0);
        loop {
            let x = b[i];
            i += 1;
            v |= u64::from(x & 0x7f) << shift;
            if x & 0x80 == 0 {
                return v;
            }
            shift += 7;
        }
    };
    let n = var();
    (0..n).map(|_| (var(), var())).collect()
}

/// The doc's whole store log replayed into a fresh Yrs doc through the client admission
/// (each entry as its own writer's): every entry is accepted, every entry that inserts moves
/// its own writer's clock forward and no other's (a clock reused under a writer would be
/// skipped as known: caught here), and the replica ends at the session's text.
fn yrs_log_replay(store: &DocStore, id: DocId, expect: &str) {
    let loaded = store.load(id).unwrap();
    let mut doc = match &loaded.snapshot {
        Some((_, state)) => Doc::load(MD.schema(), state).unwrap(),
        None => Doc::new(MD.schema()).unwrap(),
    };
    for (seq, e) in &loaded.updates {
        let EntryBody::Yrs(bytes) = &e.body else {
            panic!("entry {seq}: a Yrs doc's log holds Yrs entries");
        };
        let before = sv(&doc);
        let info = doc
            .apply_client_update(bytes, e.client)
            .unwrap_or_else(|err| panic!("entry {seq} under {}: refused: {err}", e.client));
        let after = sv(&doc);
        if info.inserts_from.contains(&e.client) {
            assert!(
                after.get(&e.client).copied().unwrap_or(0)
                    > before.get(&e.client).copied().unwrap_or(0),
                "entry {seq} under {}: its clocks were already known (a forked line)",
                e.client
            );
        }
        for (c, k) in &before {
            if *c != e.client {
                assert_eq!(
                    after.get(c),
                    Some(k),
                    "entry {seq} moved writer {c}'s clock"
                );
            }
        }
    }
    assert_eq!(doc.text(ROOT).unwrap(), expect, "the Yrs replay's text");
}

/// The row's writer schedule (the import, then each edit as its writer, with the hunks the
/// session applied) made on a fresh doc of `engine`, and each change delivered to a second one
/// through the client admission as its writer's (Yrs: no skip, no foreign content; native:
/// contiguity and E195): both end at the session's text.
fn schedule_replay(engine: &str, new: impl Fn() -> Doc, trace: &Trace, expect: &str) {
    let (mut a, mut b) = (new(), new());
    const IMPORT: ClientId = 3;
    let import = vec![Edit::Text {
        root: ROOT.into(),
        hunks: vec![Hunk {
            start: 0,
            end: 0,
            insert: START.into(),
        }],
    }];
    let u = a
        .edit_as(IMPORT, &import)
        .unwrap()
        .expect("the import changes");
    b.apply_client_update(&u, IMPORT).unwrap();
    for (n, (client, hunks)) in trace.0.iter().enumerate() {
        let e = vec![Edit::Text {
            root: ROOT.into(),
            hunks: hunks.clone(),
        }];
        let u = a
            .edit_as(*client, &e)
            .unwrap_or_else(|err| panic!("{engine}: edit {n} as {client}: {err}"))
            .expect("a change");
        b.apply_client_update(&u, *client)
            .unwrap_or_else(|err| panic!("{engine}: admission of edit {n} as {client}: {err}"));
    }
    assert_eq!(a.text(ROOT).unwrap(), expect, "{engine}: the doc's text");
    assert_eq!(
        b.text(ROOT).unwrap(),
        expect,
        "{engine}: the replica's text"
    );
}

/// Every engine column at the end of a row: the store log's Yrs replay, and the writer
/// schedule on Yrs and (feature `native`) on the native engine.
fn engines(f: &Fx, s: &Session, conn: ConnId) {
    let t = text(s, conn);
    yrs_log_replay(&f.store(), f.doc_id(), &t);
    schedule_replay("yrs", || Doc::new(MD.schema()).unwrap(), &f.trace, &t);
}

// ---------------------------------------------------------------------------------------------

/// R1: two tabs of one person on one device hold distinct writers; a released tab's writer
/// goes to the next tab of that device; every change is the person's.
#[test]
fn r1_two_tabs() {
    under_mutant(|| {
        let mut f = fx("r1", config(0));
        let s = f.s();
        let t1 = tab(&s, "ada", "laptop");
        let t2 = tab(&s, "ada", "laptop");
        assert_ne!(
            t1.client, t2.client,
            "two tabs of one device hold distinct writers"
        );
        assert!(
            one_holder_each(&s),
            "two tabs of one device hold distinct writers"
        );
        assert_ne!(
            s.writer_tag(t1.conn).unwrap(),
            s.writer_tag(t2.conn).unwrap(),
            "two tabs mint under distinct tags"
        );
        assert!(t1.lease.is_some() && t2.lease.is_some());
        f.type_at_end(&s, t1.conn, t1.client, "one ");
        f.type_at_end(&s, t2.conn, t2.client, "two ");
        assert!(s.release(t2.conn).unwrap());
        let t3 = tab(&s, "ada", "laptop");
        assert_eq!(
            t3.client, t2.client,
            "the released writer goes to the next tab"
        );
        assert!(!t3.kept, "a plain join keeps nothing");
        f.type_at_end(&s, t3.conn, t3.client, "three ");
        // another device of the same person, and another person, never get it
        let p1 = tab(&s, "ada", "phone");
        assert!(![t1.client, t3.client].contains(&p1.client));
        let b1 = tab(&s, "bea", "laptop");
        assert!(![t1.client, t3.client, p1.client].contains(&b1.client));
        let store = f.store();
        let owners: BTreeMap<ClientId, String> =
            store.clients_of(f.doc_id()).unwrap().into_iter().collect();
        assert_eq!(owners.get(&t1.client).map(String::as_str), Some("ada"));
        assert_eq!(owners.get(&t2.client).map(String::as_str), Some("ada"));
        let ws = log_writers(&store, f.doc_id());
        assert_eq!(ws.len(), 3, "the import's and ada's two writers: {ws:?}");
        engines(&f, &s, t1.conn);
    });
}

/// R2: a reconnect names its lease while its old connection is still present (a half-open
/// socket): the writer is handed over under the lock, the old connection is closed (nothing of
/// it lands after), the line goes on, and the roster shows one device for it.
#[test]
fn r2_reconnect_half_open() {
    under_mutant(|| {
        let mut f = fx("r2", config(0));
        let s = f.s();
        let t1 = tab(&s, "ada", "laptop");
        let v = f.type_at_end(&s, t1.conn, t1.client, "before ");
        let t1b = reclaim(&s, "ada", "laptop", t1.lease.unwrap());
        assert!(t1b.kept, "the reclaim keeps its writer");
        assert_eq!(t1b.client, t1.client);
        assert_eq!(t1b.replaced, Some(t1.conn));
        assert_ne!(
            t1b.lease.unwrap().token,
            t1.lease.unwrap().token,
            "a new token"
        );
        // the old connection's edit in flight, arriving after the hand-over
        let late = s.edit_at_base(
            t1.conn,
            ROOT,
            v,
            vec![Hunk {
                start: 0,
                end: 0,
                insert: "late ".into(),
            }],
            &EditOptions::new(),
        );
        assert!(
            late.is_err(),
            "the old connection is closed by the hand-over (its edit landed: {late:?})"
        );
        assert!(one_holder_each(&s));
        f.type_at_end(&s, t1b.conn, t1b.client, "after ");
        let roster = s.roster(t1b.conn).unwrap();
        let ada = roster
            .iter()
            .find(|e| e.participant.pid == "ada")
            .expect("ada is present");
        assert_eq!(
            ada.devices.iter().filter(|d| d.client == t1.client).count(),
            1,
            "one device for the writer"
        );
        assert_eq!(log_writers(&f.store(), f.doc_id()).len(), 2);
        engines(&f, &s, t1b.conn);
    });
}

/// R3: a tab's socket dies with an edit in flight and edits queued (an unclean end): its writer
/// is parked; another tab of the device gets another writer; the tab reclaims its own and its
/// queued edits land under it, once; the line is continuous on both engines.
#[test]
fn r3_crash_with_pending_edits() {
    under_mutant(|| {
        let mut f = fx("r3", config(0));
        let s = f.s();
        let t1 = tab(&s, "ada", "laptop");
        let v = f.type_at_end(&s, t1.conn, t1.client, "landed ");
        assert!(s.leave(t1.conn).unwrap()); // the socket went: no release
        let t2 = tab(&s, "ada", "laptop");
        assert_ne!(
            t2.client, t1.client,
            "a parked writer never goes to another connection"
        );
        f.type_at_end(&s, t2.conn, t2.client, "sibling ");
        let t1b = reclaim(&s, "ada", "laptop", t1.lease.unwrap());
        assert!(
            t1b.kept && t1b.client == t1.client,
            "the tab takes its writer back"
        );
        assert_eq!(t1b.replaced, None, "nothing to close: it had left");
        // its queued edit, made at the version it had: lands once, under its own writer
        let at = START.len();
        f.edit_at(&s, t1b.conn, t1b.client, v, at, "queued ");
        let now = text(&s, t1b.conn);
        assert_eq!(now.matches("queued ").count(), 1, "{now:?}");
        let rows = s.person_writers_for_tests().unwrap();
        assert!(rows.iter().all(|(_, o, _, _)| o == "ada"));
        engines(&f, &s, t1b.conn);
    });
}

/// R4: the device was offline while the doc's session closed and the process restarted: the
/// tab's lease is parked in the store; its reclaim takes the writer back; a reclaim with a
/// wrong token closes nothing and gets another writer.
#[test]
fn r4_offline_device_after_reload() {
    under_mutant(|| {
        let mut f = fx("r4", config(0));
        let s = f.s();
        let t1 = tab(&s, "ada", "laptop");
        let other = tab(&s, "ada", "laptop");
        let v = f.type_at_end(&s, t1.conn, t1.client, "before ");
        f.type_at_end(&s, other.conn, other.client, "other ");
        drop(s);
        f.restart();
        let s = f.s();
        let rows = s.person_writers_for_tests().unwrap();
        assert!(
            rows.iter().all(|(_, _, r, held)| r.leased && !held),
            "both leases are parked after the restart: {rows:?}"
        );
        let wrong = WriterLease {
            client: t1.client,
            token: t1.lease.unwrap().token ^ 1,
        };
        let x = reclaim(&s, "ada", "laptop", wrong);
        assert!(!x.kept, "a wrong token keeps nothing");
        assert_ne!(x.client, t1.client);
        assert_ne!(x.client, other.client, "a parked writer stays parked");
        assert_eq!(x.replaced, None);
        let t1b = reclaim(&s, "ada", "laptop", t1.lease.unwrap());
        assert!(t1b.kept && t1b.client == t1.client);
        // its hunk made offline, at its old base: lands, or asks for a reload
        match s.edit_at_base(
            t1b.conn,
            ROOT,
            v,
            vec![Hunk {
                start: 0,
                end: 0,
                insert: "offline ".into(),
            }],
            &EditOptions::new(),
        ) {
            Ok(out) => f.trace.0.push((t1b.client, out.hunks)),
            Err(e) => assert_eq!(e.id(), "MPC-E112", "only base_expired: {e:?}"),
        }
        engines(&f, &s, t1b.conn);
    });
}

/// R5: with a park expiry, a parked writer goes to the next tab of the device after it; the
/// returning tab's stale lease closes nothing, gets another writer, and is told the version
/// its lease ended at. A bare `prev` with a device key closes nothing.
#[test]
fn r5_stale_lease_after_expiry() {
    under_mutant(|| {
        let mut f = fx("r5", config(1_000));
        let s = f.s();
        let t1 = tab(&s, "ada", "laptop");
        f.type_at_end(&s, t1.conn, t1.client, "one ");
        let ended = s.read(t1.conn).unwrap().version;
        assert!(s.leave(t1.conn).unwrap());
        // before the expiry: parked
        let early = tab(&s, "ada", "laptop");
        assert_ne!(early.client, t1.client, "parked until the expiry");
        assert!(s.release(early.conn).unwrap());
        f.clock_skip(2_000);
        let t2 = tab(&s, "ada", "laptop");
        // the lowest free writer: the parked one is free after the expiry, the released one too
        let free: BTreeSet<ClientId> = [t1.client, early.client].into();
        assert!(free.contains(&t2.client));
        let t2b = if t2.client == t1.client {
            t2
        } else {
            let t3 = tab(&s, "ada", "laptop");
            assert_eq!(t3.client, t1.client, "the expired writer is free");
            t3
        };
        f.type_at_end(&s, t2b.conn, t2b.client, "sibling ");
        let back = reclaim(&s, "ada", "laptop", t1.lease.unwrap());
        assert!(!back.kept, "a stale lease keeps nothing");
        assert_ne!(back.client, t1.client);
        assert_eq!(back.replaced, None, "the sibling is untouched");
        assert_eq!(back.prev_ended, Some(ended), "told when its lease ended");
        assert!(s.read(t2b.conn).is_ok(), "the sibling is still present");
        let bare = s
            .join_with(
                person("ada"),
                Stay::Held,
                &JoinOptions::new().device("laptop").replacing(t1.client),
            )
            .unwrap();
        assert_eq!(bare.replaced, None, "a bare prev closes nothing");
        assert!(s.read(t2b.conn).is_ok(), "the sibling is still present");
        engines(&f, &s, t2b.conn);
    });
}

/// R5 (the ruling's row): a tab returning after its writer was let go gets a new writer, and
/// its pending hunks land exactly once.
#[test]
fn r5_returning_tab_after_expiry_lands_its_hunks_once() {
    under_mutant(|| {
        let mut f = fx("r5b", config(1_000));
        let s = f.s();
        let t1 = tab(&s, "ada", "laptop");
        let v = f.type_at_end(&s, t1.conn, t1.client, "one ");
        assert!(s.leave(t1.conn).unwrap());
        f.clock_skip(2_000);
        let t2 = tab(&s, "ada", "laptop");
        assert_eq!(t2.client, t1.client, "re-let after the expiry");
        f.type_at_end(&s, t2.conn, t2.client, "two ");
        let back = reclaim(&s, "ada", "laptop", t1.lease.unwrap());
        assert!(!back.kept);
        assert_ne!(back.client, t1.client, "a new writer");
        // its pending edit, made at the version it had, sent once on its new connection
        f.edit_at(&s, back.conn, back.client, v, START.len(), "pending ");
        let now = text(&s, back.conn);
        assert_eq!(now.matches("pending ").count(), 1, "{now:?}");
        let id = f.doc_id();
        let store = f.store();
        let with: Vec<_> = store
            .load(id)
            .unwrap()
            .updates
            .iter()
            .filter(|(_, e)| e.client == back.client)
            .map(|(s, _)| *s)
            .collect();
        assert_eq!(with.len(), 1, "one change of the returning tab: {with:?}");
        engines(&f, &s, back.conn);
    });
}

/// R6: without a device key, or with reuse off, a join is as before: a fresh writer per
/// connection, `prev` closes its connection, no lease, no kind-2 rows.
#[test]
fn r6_no_device_key_is_as_before() {
    under_mutant(|| {
        let mut f = fx("r6", config(0));
        let s = f.s();
        let a = s.join(person("ada"), Stay::Held).unwrap();
        assert!(a.lease.is_none() && !a.kept && a.prev_ended.is_none());
        f.type_at_end(&s, a.conn, a.client, "a ");
        assert!(
            s.release(a.conn).unwrap(),
            "release is a leave without a lease"
        );
        let b = s.join(person("ada"), Stay::Held).unwrap();
        assert_ne!(b.client, a.client, "fresh per connection");
        let c = s
            .join_replacing(person("ada"), Stay::Held, b.client)
            .unwrap();
        assert_eq!(c.replaced, Some(b.conn), "prev closes its connection");
        assert!(f.store().person_writers(f.doc_id()).unwrap().is_empty());
    });
}

#[test]
fn r6_reuse_off_ignores_the_device_key() {
    under_mutant(|| {
        let mut f = fx("r6off", unlimited());
        let s = f.s();
        let a = tab(&s, "ada", "laptop");
        assert!(a.lease.is_none());
        f.type_at_end(&s, a.conn, a.client, "a ");
        assert!(s.release(a.conn).unwrap());
        let b = tab(&s, "ada", "laptop");
        assert_ne!(b.client, a.client, "off: fresh per connection");
        drop(s);
        f.close();
        assert!(f.store().person_writers(f.doc_id()).unwrap().is_empty());
    });
}

/// Agents and own replicas keep their own rules with a device key: an agent's reuse
/// (`reuse_agent_clients`), an own replica never reused.
#[test]
fn r6_agents_and_own_replicas_keep_their_rules() {
    under_mutant(|| {
        let f = fx("r6agent", config(0));
        let s = f.s();
        let agent = Participant::new("bot", "bot", "#000000", ParticipantKind::Agent, "cli");
        let a = s
            .join_with(
                agent.clone(),
                Stay::Held,
                &JoinOptions::new().device("laptop"),
            )
            .unwrap();
        assert!(a.lease.is_none(), "an agent takes no person lease");
        let rep = person("ada").own_replica();
        let r1 = s
            .join_with(
                rep.clone(),
                Stay::Held,
                &JoinOptions::new().device("laptop"),
            )
            .unwrap();
        assert!(r1.lease.is_none(), "an own replica takes no person lease");
        assert!(s.release(r1.conn).unwrap());
        let r2 = s
            .join_with(rep, Stay::Held, &JoinOptions::new().device("laptop"))
            .unwrap();
        assert_ne!(
            r2.client, r1.client,
            "an own replica's writer is never reused"
        );
    });
}

/// R7 (the store's side; the cross-kit run is `tools/n1`): kind-2 rows are never in the agent
/// reuse list an older kit reads, their owner rows attribute them, and they survive a store
/// reopen; a row of an unknown layout is never reused.
#[test]
fn r7_rows_as_an_older_kit_reads_them() {
    under_mutant(|| {
        let mut f = fx("r7", config(0));
        let s = f.s();
        let t1 = tab(&s, "ada", "laptop");
        f.type_at_end(&s, t1.conn, t1.client, "one ");
        assert!(s.release(t1.conn).unwrap());
        drop(s);
        f.restart();
        let store = f.store();
        let id = f.doc_id();
        assert!(
            !store.reusable_clients(id).unwrap().contains(&t1.client),
            "never in the agent reuse list"
        );
        let pw = store.person_writers(id).unwrap();
        assert_eq!(pw.len(), 1);
        assert_eq!(pw[0].0, t1.client);
        assert_eq!(pw[0].1, "ada");
        assert!(!pw[0].2.leased, "the release is in the store");
        let s = f.s();
        let t2 = tab(&s, "ada", "laptop");
        assert_eq!(t2.client, t1.client, "reused across a restart");
    });
}

/// R8: a long simulated history (seeded): people with devices open tabs, reconnect, release,
/// and drop without a release; the session unloads now and then. Writers in the log, with and
/// without reuse; with reuse at most the most tabs each person-device had open at once, plus
/// the parked ones never taken back.
#[test]
fn r8_long_history() {
    under_mutant(|| {
        let sessions: u64 = std::env::var("MPC_PW_SESSIONS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(120);
        let (off, _) = simulate("r8off", unlimited(), sessions);
        let (on, bound) = simulate("r8on", config(0), sessions);
        println!(
            "MEASURE r8 sessions={sessions} writers_off={off} writers_on={on} bound_on={bound}"
        );
        assert!(
            on <= bound,
            "writers with reuse {on} over the bound {bound}"
        );
        assert!(
            on * 5 <= off,
            "reuse cuts writers at least five times here: {on} of {off}"
        );
    });
}

/// One seeded history; returns (writers in the log, the reuse bound).
fn simulate(name: &str, c: Config, sessions: u64) -> (usize, usize) {
    let mut f = fx(name, c);
    let mut rng = 0x7077_5f72_3830_3031u64;
    let mut next = move |n: u64| {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng % n
    };
    let people = ["ada", "bea", "cyd"];
    let devices = ["laptop", "phone"];
    let mut max_open: BTreeMap<(usize, usize), usize> = BTreeMap::new();
    let mut leaked = 0usize;
    for round in 0..sessions {
        let s = f.s();
        let p = next(3) as usize;
        let d = if next(10) < 7 { 0 } else { 1 };
        let tabs = if next(20) == 0 { 2 } else { 1 };
        let mut open: Vec<Joined> = (0..tabs).map(|_| tab(&s, people[p], devices[d])).collect();
        let e = max_open.entry((p, d)).or_default();
        *e = (*e).max(tabs);
        for j in &mut open {
            f.type_at_end(&s, j.conn, j.client, "x");
            // reconnects in the session, with an edit after each
            while next(3) == 0 {
                let back = match j.lease {
                    Some(l) => reclaim(&s, people[p], devices[d], l),
                    None => s
                        .join_replacing(person(people[p]), Stay::Held, j.client)
                        .unwrap(),
                };
                *j = back;
                f.type_at_end(&s, j.conn, j.client, "y");
            }
        }
        for j in open {
            if next(20) == 0 {
                s.leave(j.conn).unwrap(); // never comes back
                leaked += 1;
            } else {
                s.release(j.conn).unwrap();
            }
        }
        drop(s);
        if round % 25 == 24 {
            f.close();
        }
    }
    let n = log_writers(&f.store(), f.doc_id()).len() - 1; // the import's
    (n, max_open.values().sum::<usize>() + leaked)
}

/// R10: one store file is open once; a doc has one session in a manager (the held set is one
/// set).
#[test]
fn r10_one_store_one_session() {
    under_mutant(|| {
        let f = fx("r10", config(0));
        let s1 = f.s();
        let s2 = f.s();
        assert!(Arc::ptr_eq(&s1, &s2), "one session per doc");
        assert!(
            DocStore::open(f.dir.join("d.redb")).is_err(),
            "a second open of an open store is refused"
        );
    });
}

// ---- R12: tagged keys across a hand-over (a keyed pack, as `tests/mint.rs`'s)

/// A MovableList `items` with tagged keys (prefix `i`); the file is one key per line.
struct ItemsPack;

impl Pack for ItemsPack {
    fn schema(&self) -> k2_mpc_base::Schema {
        k2_mpc_base::Schema {
            name: "items",
            roots: &[("items", k2_mpc_base::Kind::MovableList)],
        }
    }
    fn decode(
        &self,
        b: &[u8],
    ) -> Result<k2_mpc_base::session::PackContent, k2_mpc_base::session::Declined> {
        let items = std::str::from_utf8(b)
            .unwrap()
            .lines()
            .map(|k| (k2_mpc_base::Key::new(k).unwrap(), k2_mpc_base::Value::Null))
            .collect();
        Ok(k2_mpc_base::session::PackContent {
            roots: vec![("items".into(), k2_mpc_base::Neutral::List(items))],
            codec: Vec::new(),
        })
    }
    fn encode(
        &self,
        c: &[(String, k2_mpc_base::Neutral)],
        _: &[u8],
    ) -> Result<Vec<u8>, k2_mpc_base::session::PackError> {
        let mut out = String::new();
        for (_, n) in c {
            if let k2_mpc_base::Neutral::List(es) = n {
                for (k, _) in es {
                    out.push_str(&format!("{k}\n"));
                }
            }
        }
        Ok(out.into_bytes())
    }
    fn tagged_keys(&self) -> &'static [(&'static str, &'static [&'static str])] {
        &[("items", &["i"])]
    }
}

fn insert(key: &k2_mpc_base::Key) -> Vec<Edit> {
    vec![Edit::List {
        root: "items".into(),
        ops: vec![k2_mpc_base::elem::ListOp::Insert {
            key: key.clone(),
            at: k2_mpc_base::elem::Anchor::End,
            value: k2_mpc_base::Value::Null,
        }],
    }]
}

fn items(s: &Session) -> Vec<String> {
    let st = s.keyed_state_for_tests().unwrap();
    match st.roots.iter().find(|(r, _)| r == "items") {
        Some((_, k2_mpc_base::elem::KeyedRoot::List { live, .. })) => {
            live.iter().map(|(k, _)| k.to_string()).collect()
        }
        other => panic!("items: {other:?}"),
    }
}

/// R12: a key a tab minted under its writer's tag and hadn't sent: (a) the tab reclaims its
/// writer, and the insert is its own; (b) after an expiry the writer went to a sibling, which
/// minted and inserted the same key; the returning tab's insert is refused, and the list holds
/// the key once.
#[test]
fn r12_tag_keys_across_hand_over() {
    under_mutant(|| {
        let f = fx("r12", config(1_000));
        let ikey = DocKey {
            space: "ws".into(),
            path: "a.items".into(),
        };
        let s = f.mgr().open(Arc::new(ItemsPack), &ikey, Some(b"")).unwrap();
        let ana = |o: JoinOptions| {
            let p = Participant::new("ana", "ana", "#000000", ParticipantKind::Person, "app");
            s.join_with(p, Stay::Held, &o).unwrap()
        };
        // (a) kept: the minted key is still its own
        let t1 = ana(JoinOptions::new().device("tablet"));
        let k1 = s.mint_key(t1.conn, "items", "i").unwrap();
        assert!(s.leave(t1.conn).unwrap());
        let t1b = ana(JoinOptions::new()
            .device("tablet")
            .reclaim(t1.lease.unwrap()));
        assert!(t1b.kept);
        s.edit_ops(t1b.conn, insert(&k1), &EditOptions::new())
            .expect("a kept writer's minted key is its own");
        // (b) let go after the expiry: a sibling mints the same next key and inserts it
        let k2 = s.mint_key(t1b.conn, "items", "i").unwrap();
        assert!(s.leave(t1b.conn).unwrap());
        f.clock_skip(2_000);
        let sib = ana(JoinOptions::new().device("tablet"));
        assert_eq!(
            sib.client, t1.client,
            "the expired writer goes to the sibling"
        );
        let k2s = s.mint_key(sib.conn, "items", "i").unwrap();
        assert_eq!(k2s, k2, "the same tag mints the same next key");
        s.edit_ops(sib.conn, insert(&k2s), &EditOptions::new())
            .unwrap();
        let back = ana(JoinOptions::new()
            .device("tablet")
            .reclaim(t1b.lease.unwrap()));
        assert!(!back.kept && back.client != t1.client);
        assert!(
            s.edit_ops(back.conn, insert(&k2), &EditOptions::new())
                .is_err(),
            "the returning tab's stale key is refused"
        );
        let live = items(&s);
        assert_eq!(
            live.iter().filter(|k| **k == k2.to_string()).count(),
            1,
            "{live:?}"
        );
    });
}
