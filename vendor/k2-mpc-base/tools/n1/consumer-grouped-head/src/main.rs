//! N−1 of grouped commits (tools/n1/n1.sh `layout-grouped`), this build's side: built against
//! `<dir>/head`, this checkout's kit, with commits grouped at 1,024 changes and 10 s (the
//! host's clock stands still, so a group waits for its size or a flush).
//!
//! - `produce <dir>`: it writes `<dir>/clean.redb`: `a.md` made and edited three times, then
//!   every handle dropped (the clean close). Read through redb itself after the close: the
//!   open mark (`grouped_open`) is gone, `a.md`'s floor row (`seq_floor/…`) is kept, no lost
//!   range, and the features word lacks the seq floor bit.
//! - `unclean <dir>`: it makes `<dir>/stamped.redb` (`s.md`), edits it until a change waits for
//!   its group, and leaves the process with every handle still held (`std::process::exit`, no
//!   drop): the open mark stays set.
//! - `recover <dir>`: it opens `stamped.redb` (the recovery: the mark set), and finds the
//!   features word stamped with [`FEATURE_SEQ_FLOOR`]; after its clean close a lost range is
//!   on disk (`seq_lost/…`) and the mark is gone.
//! - `after <dir>`: after the previous release's `read` and `produce`
//!   (`tools/n1/consumer-grouped`): `clean.redb` opens (grouped), `a.md` reads with that
//!   release's edit, takes another, and reads it back after a reopen, its floor row still
//!   there; `prev.redb`, the previous release's own store, opens grouped, `b.md` reads as that
//!   release edited it, takes an edit and reads it back after a reopen, a floor row now
//!   written for it and no bit; and `stamped.redb`, refused there, opens here and reads.
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::session::{
    AuditEvent, Config, Host, Participant, ParticipantKind, Rights, SessionManager, Stay,
    TextFilePack,
};
use k2_mpc_base::store::{
    CommitDurability, DocId, DocKey, DocStore, StoreSettings, FEATURE_SEQ_FLOOR,
};
use redb::{Database, ReadableTable, TableDefinition};

const MD: TextFilePack = TextFilePack::new("markdown", 1 << 20);
const META: TableDefinition<&str, u64> = TableDefinition::new("meta");

/// `a.md`: made, then three changes; the last is what the previous release reads
/// (`consumer-grouped`, `CLEAN_TEXT`).
const CLEAN_STEPS: [&str; 4] = [
    "# Grouped\n",
    "# Grouped\n\nwritten with grouped commits\n",
    "# Grouped\n\nwritten with grouped commits\nsecond change\n",
    "# Grouped\n\nwritten with grouped commits\nthird change\n",
];
/// `a.md` after the previous release's edit (`consumer-grouped`, `PREV_EDIT`), then this
/// build's.
const PREV_EDIT: &str = "# Grouped\n\nwritten with grouped commits\nthird change\nedited by the previous release\n";
const HEAD_AGAIN: &str = "# Grouped\n\nwritten with grouped commits\nthird change\nedited by the previous release\nand by this build again\n";
/// `b.md` as the previous release left it (`consumer-grouped`, `PREV_EDITED`), then this
/// build's edit.
const PREV_EDITED: &str = "# Previous\n\nmade by the previous release\nand edited there\n";
const PREV_HEAD: &str = "# Previous\n\nmade by the previous release\nand edited there\nthen by this build\n";
/// `s.md`'s first line (every prefix a recovery keeps starts with it).
const STAMPED_START: &str = "# Stamped\n";

struct H {
    now: AtomicU64,
}

impl Host for H {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full().with_owner(true)
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

fn grouped() -> StoreSettings {
    StoreSettings::DEFAULT.with_commit_durability(CommitDurability::grouped(1024, 10_000).unwrap())
}

fn key(path: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: path.into(),
    }
}

fn mgr(s: &Arc<DocStore>) -> SessionManager {
    let host = Arc::new(H {
        now: AtomicU64::new(1_900_000_000_000),
    });
    SessionManager::new(s.clone(), host, Config::default())
}

fn who() -> Participant {
    Participant::new("h", "H", "#876543", ParticipantKind::Person, "cli")
}

/// The `meta` rows of the store at `p` that grouped commits write, read through redb itself
/// (no opener holds the file): the open mark, the floor of doc `id`, and the count of lost
/// ranges.
struct Rows {
    open_mark: Option<u64>,
    floor: Option<u64>,
    lost: usize,
}

fn rows(p: &Path, id: DocId) -> Rows {
    let db = Database::open(p).expect("no opener holds the store: its clean close ran");
    let r = db.begin_read().unwrap();
    let meta = r.open_table(META).unwrap();
    let floor_key = format!("seq_floor/{id:020}");
    Rows {
        open_mark: meta.get("grouped_open").unwrap().map(|v| v.value()),
        floor: meta.get(floor_key.as_str()).unwrap().map(|v| v.value()),
        lost: meta.range("seq_lost/".."seq_lost0").unwrap().count(),
    }
}

/// The text of the doc at `path` in the store at `p` opened with `settings`, its id and the
/// store's features word (opened and closed here).
fn read_doc(p: &Path, settings: StoreSettings, path: &str) -> (String, DocId, u64) {
    let s = Arc::new(DocStore::open_with(p, settings).unwrap());
    let word = s.required_features().unwrap();
    let m = mgr(&s);
    let sess = m.open(Arc::new(MD), &key(path), None).unwrap();
    let c = sess.join(who(), Stay::Held).unwrap().conn;
    let text = sess.read(c).unwrap().text_by_root[0].1.clone();
    drop(sess);
    m.release().unwrap();
    let id = s.find(&key(path)).unwrap().expect("the doc");
    (text, id, word)
}

/// Open the doc at `path` in the store at `p` (grouped), check its text is `want`, replace it
/// with `next`, and close everything cleanly. Its id.
fn edit_doc(p: &Path, path: &str, want: &str, next: &str) -> DocId {
    let s = Arc::new(DocStore::open_with(p, grouped()).unwrap());
    let m = mgr(&s);
    let sess = m.open(Arc::new(MD), &key(path), None).unwrap();
    let c = sess.join(who(), Stay::Held).unwrap().conn;
    assert_eq!(sess.read(c).unwrap().text_by_root[0].1, want, "{path}: as read");
    sess.replace_text(c, TextFilePack::ROOT, next).unwrap();
    drop(sess);
    m.release().unwrap();
    s.find(&key(path)).unwrap().expect("the doc")
}

fn produce(dir: &Path) {
    let p = dir.join("clean.redb");
    let id = {
        let s = Arc::new(DocStore::open_with(&p, grouped()).unwrap());
        let m = mgr(&s);
        let sess = m
            .open(Arc::new(MD), &key("a.md"), Some(CLEAN_STEPS[0].as_bytes()))
            .unwrap();
        let c = sess.join(who(), Stay::Held).unwrap().conn;
        let mut waited = 0;
        for t in &CLEAN_STEPS[1..] {
            let e = sess.replace_text(c, TextFilePack::ROOT, t).unwrap();
            waited += usize::from(!e.durable);
        }
        assert!(waited > 0, "control: a change waited for its group");
        drop(sess);
        m.release().unwrap();
        s.find(&key("a.md")).unwrap().expect("a.md")
    };
    let r = rows(&p, id);
    assert_eq!(r.open_mark, None, "row: the clean close removed the open mark");
    let floor = r.floor.expect("row: a.md's floor row kept after the clean close");
    assert_eq!(r.lost, 0, "row: no lost range");
    let (text, _, word) = read_doc(&p, StoreSettings::DEFAULT, "a.md");
    assert_eq!(text, CLEAN_STEPS[3]);
    assert_eq!(word & FEATURE_SEQ_FLOOR, 0, "row: no bit on a clean store");
    println!(
        "N1-GROUPED clean produced: a.md made and edited grouped, closed cleanly: no open mark, \
         floor row {floor} kept, no lost range, features word {word:#x}"
    );
}

fn unclean(dir: &Path) {
    let p = dir.join("stamped.redb");
    let s = Arc::new(DocStore::open_with(&p, grouped()).unwrap());
    let m = mgr(&s);
    let sess = m
        .open(Arc::new(MD), &key("s.md"), Some(STAMPED_START.as_bytes()))
        .unwrap();
    let c = sess.join(who(), Stay::Held).unwrap().conn;
    s.flush().unwrap();
    let mut text = STAMPED_START.to_owned();
    let mut waited = 0;
    for i in 0..4 {
        text.push_str(&format!("line {i}\n"));
        let e = sess.replace_text(c, TextFilePack::ROOT, &text).unwrap();
        println!(
            "N1-GROUPED unclean change {i}: version {} durable {}",
            e.version, e.durable
        );
        waited += usize::from(!e.durable);
    }
    assert!(waited > 0, "control: a change waited for its group");
    println!("N1-GROUPED unclean: the process leaves with the store held (no clean close)");
    // every handle held: no drop runs, so the open mark stays set and the group waits
    std::process::exit(0);
}

fn recover(dir: &Path) {
    let p = dir.join("stamped.redb");
    let (text, id, word) = read_doc(&p, grouped(), "s.md");
    assert_eq!(
        word & FEATURE_SEQ_FLOOR,
        FEATURE_SEQ_FLOOR,
        "row: the recovery stamped the seq floor bit (word {word:#x})"
    );
    assert!(text.starts_with(STAMPED_START), "s.md kept a prefix: {text:?}");
    let r = rows(&p, id);
    assert_eq!(r.open_mark, None, "the clean close after the recovery removed the mark");
    assert!(r.lost > 0, "row: the recovery recorded a lost range");
    println!(
        "N1-GROUPED stamped by recovery: features word {word:#x} (seq floor bit set), {} lost \
         range(s), s.md read",
        r.lost
    );
}

fn after(dir: &Path) {
    // this build's clean store, after the previous release's edit
    let p = dir.join("clean.redb");
    let id = edit_doc(&p, "a.md", PREV_EDIT, HEAD_AGAIN);
    let (text, _, word) = read_doc(&p, StoreSettings::DEFAULT, "a.md");
    assert_eq!(text, HEAD_AGAIN, "row: this build's edit reads back after a reopen");
    assert_eq!(word & FEATURE_SEQ_FLOOR, 0);
    let r = rows(&p, id);
    assert!(r.floor.is_some(), "row: a.md's floor row kept through the previous release");
    assert_eq!((r.open_mark, r.lost), (None, 0));
    println!(
        "N1-GROUPED clean-after ok: this build opened its store after the previous release, \
         read that release's edit, edited it again and read that back after a reopen"
    );

    // the previous release's own store
    let p = dir.join("prev.redb");
    let id = edit_doc(&p, "b.md", PREV_EDITED, PREV_HEAD);
    let r = rows(&p, id);
    assert_eq!(r.open_mark, None, "row: the clean close removed the open mark");
    assert!(r.floor.is_some(), "row: b.md's floor row written");
    assert_eq!(r.lost, 0);
    let (text, _, word) = read_doc(&p, StoreSettings::DEFAULT, "b.md");
    assert_eq!(text, PREV_HEAD, "row: this build's edit reads back after a reopen");
    assert_eq!(word & FEATURE_SEQ_FLOOR, 0, "row: no bit on a clean store");
    println!(
        "N1-GROUPED prev-store ok: this build opened the previous release's store grouped, read \
         b.md as that release edited it, edited it and read that back after a reopen"
    );

    // the stamped store, refused by the previous release, opens here
    let (text, _, word) = read_doc(&dir.join("stamped.redb"), grouped(), "s.md");
    assert!(text.starts_with(STAMPED_START));
    assert_eq!(word & FEATURE_SEQ_FLOOR, FEATURE_SEQ_FLOOR, "the bit kept");
    println!("N1-GROUPED stamped-after ok: this build opens the stamped store and reads s.md");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (Some(mode), Some(dir)) = (args.get(1), args.get(2)) else {
        eprintln!("usage: produce|unclean|recover|after <dir>");
        std::process::exit(2);
    };
    let dir = Path::new(dir);
    match mode.as_str() {
        "produce" => produce(dir),
        "unclean" => unclean(dir),
        "recover" => recover(dir),
        "after" => after(dir),
        _ => {
            eprintln!("usage: produce|unclean|recover|after <dir>");
            std::process::exit(2);
        }
    }
}
