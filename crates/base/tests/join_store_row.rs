//! The store's `clients` table and joins: a client ID gets its owner row with its first change,
//! so a participant that never writes through one adds no row for it. Each case counts the doc's
//! rows with `DocStore::clients_of` around N joins and leaves; the control shows the same count
//! moves when a participant edits. An own replica with edit rights is the exception: it keeps
//! its owner and kind rows from the moment it has those rights, change or no change. Public API
//! only.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use k2_mpc_base::session::{
    AuditEvent, Config, Host, Need, Participant, ParticipantKind, Rights, Session, SessionError,
    SessionManager, Stay, TextFilePack,
};
use k2_mpc_base::store::{DocKey, DocStore};

const ROOT: &str = TextFilePack::ROOT;
const DOC: &str = "one\ntwo\nthree\n";
const N: usize = 200;

struct TestHost {
    rights: Mutex<HashMap<String, Rights>>,
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
        1_000_000
    }
    fn on_event(&self, _ev: &AuditEvent) {}
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
    store: Arc<DocStore>,
    // keeps the session's manager for the test's length
    _mgr: SessionManager,
    s: Arc<Session>,
    id: u64,
}

fn fx(tag: &str, cfg: Config) -> Fx {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-join-row-{tag}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let host = Arc::new(TestHost {
        rights: Mutex::new(HashMap::new()),
    });
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mgr = SessionManager::new(store.clone(), host.clone(), cfg);
    let key = DocKey {
        space: "ws".into(),
        path: "notes.md".into(),
    };
    let s = mgr
        .open(
            Arc::new(TextFilePack::new("markdown", 4 << 20)),
            &key,
            Some(DOC.as_bytes()),
        )
        .unwrap();
    let id = store.find(&key).unwrap().unwrap();
    Fx {
        dir: TmpDir(dir),
        host,
        store,
        _mgr: mgr,
        s,
        id,
    }
}

impl Fx {
    fn grant(&self, pid: &str, r: Rights) {
        self.host.rights.lock().unwrap().insert(pid.into(), r);
    }
    fn rows(&self) -> usize {
        self.store.clients_of(self.id).unwrap().len()
    }
    fn file_bytes(&self) -> u64 {
        std::fs::metadata(self.dir.0.join("docs.redb"))
            .unwrap()
            .len()
    }
    /// N joins and leaves of `p`, each refused an edit with `refused`. Prints the counts, and
    /// returns the rows before and after with the distinct client IDs the joins got.
    fn cycle(
        &self,
        case: &str,
        p: &Participant,
        refused: SessionError,
    ) -> (usize, usize, HashSet<u64>) {
        let before = self.rows();
        let bytes = self.file_bytes();
        let started = Instant::now();
        let mut clients = HashSet::new();
        for _ in 0..N {
            let j = self.s.join(p.clone(), Stay::Held).unwrap();
            assert_ne!(j.client, 0, "a live doc's join gets a client ID");
            clients.insert(j.client);
            let t = self.s.read(j.conn).unwrap().text_by_root.remove(0).1;
            let e = self
                .s
                .replace_text(j.conn, ROOT, &format!("{t}x\n"))
                .expect_err("this participant can't edit");
            assert_eq!(e, refused, "{case}: the edit is refused as expected");
            assert!(self.s.leave(j.conn).unwrap(), "{case}: the leave closes it");
        }
        let after = self.rows();
        println!(
            "{case}: N={N} clients_of rows {before} -> {after}, distinct client IDs {}, \
             file bytes {bytes} -> {}, {} us per cycle",
            clients.len(),
            self.file_bytes(),
            started.elapsed().as_micros() / N as u128,
        );
        (before, after, clients)
    }
}

fn person(pid: &str) -> Participant {
    Participant::new(pid, pid, "#123456", ParticipantKind::Person, "tab")
}

fn agent(pid: &str) -> Participant {
    Participant::new(pid, pid, "#123456", ParticipantKind::Agent, "cli")
}

fn cannot_edit() -> SessionError {
    SessionError::Forbidden { need: Need::Edit }
}

#[test]
fn read_only_person_joins_and_leaves_add_no_client_rows() {
    let f = fx("reader", Config::default());
    f.grant("reader", Rights::read_only());
    let (before, after, _) = f.cycle("read-only person", &person("reader"), cannot_edit());
    assert_eq!(
        after, before,
        "read-only person: rows after {N} joins and leaves"
    );
}

#[test]
fn read_only_agent_joins_and_leaves_add_no_client_rows() {
    let f = fx("agent", Config::default());
    f.grant("bot", Rights::read_only());
    let (before, after, _) = f.cycle("read-only agent", &agent("bot"), cannot_edit());
    assert_eq!(
        after, before,
        "read-only agent: rows after {N} joins and leaves"
    );
}

#[test]
fn read_only_own_replica_joins_and_leaves_add_no_client_rows() {
    let f = fx("replica", Config::default());
    f.grant("tab", Rights::read_only());
    let p = person("tab").own_replica();
    let (before, after, _) = f.cycle("read-only own replica", &p, cannot_edit());
    assert_eq!(
        after, before,
        "read-only own replica: rows after {N} joins and leaves"
    );
}

#[test]
fn watcher_joins_past_the_limit_add_no_client_rows() {
    let mut cfg = Config::default();
    cfg.participants_max = 1;
    let f = fx("watcher", cfg);
    f.grant("ed", Rights::editor());
    f.grant("late", Rights::editor());
    let ed = f.s.join(person("ed"), Stay::Held).unwrap();
    let w = f.s.join(person("late"), Stay::Held).unwrap();
    assert_eq!(f.s.watching(ed.conn).unwrap(), 1, "the late pid watches");
    f.s.leave(w.conn).unwrap();
    assert_eq!(f.s.watching(ed.conn).unwrap(), 0);
    let (before, after, _) = f.cycle(
        "watcher past the limit",
        &person("late"),
        SessionError::Watching { max: 1 },
    );
    assert_eq!(
        after, before,
        "watcher past the limit: rows after {N} joins and leaves"
    );
}

/// The count moves: each join that edits leaves one row, naming its owner.
#[test]
fn control_a_writer_who_edits_adds_a_client_row() {
    let f = fx("writer", Config::default());
    f.grant("writer", Rights::editor());
    let before = f.rows();
    let mut clients = HashSet::new();
    for k in 0..3 {
        let j = f.s.join(person("writer"), Stay::Held).unwrap();
        clients.insert(j.client);
        let t = f.s.read(j.conn).unwrap().text_by_root.remove(0).1;
        f.s.replace_text(j.conn, ROOT, &format!("{t}w{k}\n"))
            .unwrap();
        assert_eq!(
            f.store.client_owner(f.id, j.client).unwrap().as_deref(),
            Some("writer"),
            "the edit's client ID names its owner once it has edited"
        );
        f.s.leave(j.conn).unwrap();
    }
    let after = f.rows();
    println!("control writer: 3 edits, clients_of rows {before} -> {after}");
    assert_eq!(clients.len(), 3, "a person's join mints a new client ID");
    assert_eq!(after, before + 3, "one row per client ID that edited");
}

/// Edit rights alone add nothing: the row waits for a change.
#[test]
fn person_with_edit_rights_who_never_edits_adds_no_client_row() {
    let f = fx("idle-editor", Config::default());
    f.grant("ed", Rights::editor());
    let before = f.rows();
    for _ in 0..N {
        let j = f.s.join(person("ed"), Stay::Held).unwrap();
        f.s.read(j.conn).unwrap();
        f.s.leave(j.conn).unwrap();
    }
    assert_eq!(
        f.rows(),
        before,
        "a person with edit rights: rows after {N} joins and leaves with no edit"
    );
}

/// `client_kinds` as the file holds it, read once the fixture has closed the store.
fn kind_rows(f: Fx) -> Vec<(u64, u64)> {
    use redb::{Database, ReadableTable, TableDefinition};
    const KINDS: TableDefinition<(u64, u64), &[u8]> = TableDefinition::new("client_kinds");
    let Fx {
        dir,
        store,
        _mgr,
        s,
        ..
    } = f;
    drop(s);
    drop(_mgr);
    drop(store);
    let d = Database::open(dir.0.join("docs.redb")).unwrap();
    let r = d.begin_read().unwrap();
    let t = r.open_table(KINDS).unwrap();
    let rows = t.iter().unwrap().map(|kv| kv.unwrap().0.value()).collect();
    rows
}

/// A host may check the store for a replica's owner when it comes back from offline, before it
/// has landed anything: a replica that can write has both rows from its join, with no edit.
#[test]
fn writer_own_replica_has_its_rows_with_no_edits() {
    let f = fx("writer-replica", Config::default());
    f.grant("tab", Rights::editor());
    let before = f.rows();
    let j = f.s.join(person("tab").own_replica(), Stay::Held).unwrap();
    assert_eq!(
        f.store.client_owner(f.id, j.client).unwrap().as_deref(),
        Some("tab"),
        "a writer own replica's owner row, with no edit"
    );
    assert_eq!(f.rows(), before + 1);
    assert!(
        !f.store.reusable_clients(f.id).unwrap().contains(&j.client),
        "an own replica's ID is never reusable"
    );
    let id = f.id;
    assert_eq!(
        kind_rows(f),
        [(id, j.client)],
        "a writer own replica's kind row, with no edit"
    );
}

/// An own replica joined read-only has no rows; once the host gives it edit rights it can write
/// offline, so both rows exist from the next call that sees those rights.
#[test]
fn own_replica_rising_to_edit_rights_has_its_rows_at_once() {
    let f = fx("rising-replica", Config::default());
    f.grant("tab", Rights::read_only());
    let before = f.rows();
    let j = f.s.join(person("tab").own_replica(), Stay::Held).unwrap();
    f.s.read(j.conn).unwrap();
    assert_eq!(
        f.rows(),
        before,
        "a read-only own replica has no owner row yet"
    );
    f.grant("tab", Rights::editor());
    // a read, not an edit: the rights are what count
    f.s.read(j.conn).unwrap();
    assert_eq!(
        f.store.client_owner(f.id, j.client).unwrap().as_deref(),
        Some("tab"),
        "the owner row, once the rights rose"
    );
    let rows = f.rows();
    // losing the rights and getting them back writes nothing more
    f.grant("tab", Rights::read_only());
    f.s.read(j.conn).unwrap();
    f.grant("tab", Rights::editor());
    f.s.read(j.conn).unwrap();
    assert_eq!(f.rows(), rows, "one row, however often the rights move");
    let id = f.id;
    assert_eq!(
        kind_rows(f),
        [(id, j.client)],
        "the kind row, once the rights rose"
    );
}

/// The same when the rights rise for a pid and the host's periodic re-check is what sees it.
#[test]
fn own_replica_rising_on_a_recheck_has_its_rows_at_once() {
    let f = fx("recheck-replica", Config::default());
    f.grant("tab", Rights::read_only());
    let before = f.rows();
    let j = f.s.join(person("tab").own_replica(), Stay::Held).unwrap();
    assert_eq!(f.rows(), before, "a read-only own replica has no row yet");
    f.grant("tab", Rights::editor());
    f.s.recheck_rights().unwrap();
    assert_eq!(
        f.store.client_owner(f.id, j.client).unwrap().as_deref(),
        Some("tab"),
        "the owner row, once a re-check saw the rights rise"
    );
}

/// D1 reads owners from a map built once per load: a change through a client ID the map doesn't
/// know yet still counts as its writer's own afterwards, so the writer's next edit inside that
/// text keeps the characters around it.
#[test]
fn a_first_change_counts_as_its_writers_own() {
    let f = fx("own-text", Config::default());
    f.grant("a", Rights::editor());
    f.grant("b", Rights::editor());
    // a's edit builds the session's owner map, before b has a client ID
    let a = f.s.join(person("a"), Stay::Held).unwrap();
    let t = f.s.read(a.conn).unwrap().text_by_root.remove(0).1;
    f.s.replace_text(a.conn, ROOT, &format!("{t}from a\n"))
        .unwrap();
    let b = f.s.join(person("b"), Stay::Held).unwrap();
    let t = f.s.read(b.conn).unwrap().text_by_root.remove(0).1;
    f.s.replace_text(b.conn, ROOT, &format!("{t}quip\n"))
        .unwrap();
    let id_of_q = |f: &Fx| {
        let c = f.s.char_blame(ROOT).unwrap();
        c.iter()
            .find(|x| x.ch == 'q')
            .map(|x| (x.client, x.clock))
            .unwrap()
    };
    let before = id_of_q(&f);
    assert_eq!(before.0, b.client, "b wrote the word");
    let t = f.s.read(b.conn).unwrap().text_by_root.remove(0).1;
    f.s.replace_text(b.conn, ROOT, &t.replace("quip", "qUip"))
        .unwrap();
    assert_eq!(
        id_of_q(&f),
        before,
        "b's own character stays in place when b edits next to it"
    );
}

/// K-QUOTA-3's `joins_1m`, **for read-only joins only**: 10^6 joins and leaves by a read-only
/// participant leave the client table as it was, so within its 8 MiB. Checked along the way, from
/// the first join on, as well as at the end.
///
/// **This doesn't meet K-QUOTA-3.** Writers who reconnect aren't covered: each connection that
/// writes adds a row, and the requirement stays open until a later change.
#[test]
fn joins_1m() {
    const JOINS: u64 = 1_000_000;
    const TABLE_MAX: u64 = 8 << 20;
    let f = fx("joins-1m", Config::default());
    f.grant("reader", Rights::read_only());
    let before = f.rows();
    let started = Instant::now();
    for k in 0..JOINS {
        let j = f.s.join(person("reader"), Stay::Held).unwrap();
        f.s.leave(j.conn).unwrap();
        if k == 0 || (k + 1) % 50_000 == 0 {
            assert_eq!(
                f.rows(),
                before,
                "client rows after {} read-only joins",
                k + 1
            );
        }
    }
    let table: u64 = f
        .store
        .clients_of(f.id)
        .unwrap()
        .iter()
        .map(|(_, owner)| 16 + owner.len() as u64)
        .sum();
    println!(
        "joins_1m: {JOINS} read-only joins, clients_of rows {before} -> {}, table {table} bytes, \
         {} us per join and leave",
        f.rows(),
        started.elapsed().as_micros() / u128::from(JOINS),
    );
    assert_eq!(
        f.rows(),
        before,
        "client rows after {JOINS} read-only joins"
    );
    assert!(table <= TABLE_MAX, "the table holds {table} bytes");
}
