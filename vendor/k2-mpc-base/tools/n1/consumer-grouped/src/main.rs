//! N−1 of grouped commits (tools/n1/n1.sh `layout-grouped`), built against the previous
//! release: a kit that doesn't know grouped commits, their `meta` rows (`grouped_open`,
//! `seq_floor/…`, `seq_lost/…`) or the features bit a recovery stamps (bit 2, the seq floor).
//!
//! - `read <dir>`: `<dir>` holds two stores this repo's `tools/n1/consumer-grouped-head` wrote
//!   with grouped commits:
//!   - `clean.redb`, closed cleanly (its floor rows kept, no open mark, no lost range, the
//!     features word without the bit): it opens here, `a.md` reads as written, takes an edit
//!     that lands (its version past the one read), and reads it back after a reopen;
//!   - `stamped.redb`, which a recovery stamped with the bit (its grouped opener stopped
//!     without a clean close, after a change waited for its group): it is refused at open,
//!     typed `StoreError::UnknownFeatures { found: 8, unknown: bit 2 }`, `MPC-E502`
//!     `store_newer`, exit 6, one line, and the file's bytes are unchanged.
//! - `produce <dir>`: it writes `<dir>/prev.redb`, a store of its own whose doc (`b.md`) it
//!   made and edited, for this repo's `consumer-grouped-head after` to open, read and edit.
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::errors::Refusal;
use k2_mpc_base::session::{
    AuditEvent, Config, Host, Participant, ParticipantKind, Rights, SessionManager, Stay,
    TextFilePack,
};
use k2_mpc_base::store::{DocKey, DocStore, StoreError};
use k2_mpc_base::tools::exit;

const MD: TextFilePack = TextFilePack::new("markdown", 1 << 20);
/// The features bit a recovery stamps (this repo's `FEATURE_SEQ_FLOOR`), unknown here.
const SEQ_FLOOR_BIT: u64 = 1 << 2;
/// `a.md` as this repo's producer left `clean.redb` (`consumer-grouped-head`, `CLEAN_TEXT`).
const CLEAN_TEXT: &str = "# Grouped\n\nwritten with grouped commits\nthird change\n";
/// `a.md` after the edit here (`consumer-grouped-head after` reads it back).
const PREV_EDIT: &str = "# Grouped\n\nwritten with grouped commits\nthird change\nedited by the previous release\n";
/// `b.md` as made here, then as edited (`consumer-grouped-head after` reads the edit).
const PREV_MADE: &str = "# Previous\n\nmade by the previous release\n";
const PREV_EDITED: &str = "# Previous\n\nmade by the previous release\nand edited there\n";
/// The refusal's one line for `stamped.redb`.
const STAMPED_LINE: &str = "store is format 8 and requires feature bit 2, which this build \
                            doesn't know: use the newer build";

struct H {
    now: AtomicU64,
}

impl Host for H {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full().with_owner(true)
    }
    fn now_ms(&self) -> u64 {
        self.now.fetch_add(1, Ordering::SeqCst)
    }
    fn on_event(&self, _: &AuditEvent) {}
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
    Participant::new("p", "P", "#345678", ParticipantKind::Person, "cli")
}

/// The text and version of the doc at `path` in the store at `p` (opened and closed here).
fn read_doc(p: &Path, path: &str) -> (String, u64) {
    let s = Arc::new(DocStore::open(p).unwrap());
    let m = mgr(&s);
    let sess = m.open(Arc::new(MD), &key(path), None).unwrap();
    let c = sess.join(who(), Stay::Held).unwrap().conn;
    let r = sess.read(c).unwrap();
    let out = (r.text_by_root[0].1.clone(), r.version);
    drop(sess);
    m.release().unwrap();
    out
}

fn read(dir: &Path) {
    // a store this build closed cleanly: it opens, reads and takes an edit
    let p = dir.join("clean.redb");
    let (version, after) = {
        let s = Arc::new(DocStore::open(&p).expect("row: a cleanly closed grouped store opens"));
        let format = s.format().unwrap();
        let word = s.required_features().unwrap();
        assert_eq!(word & SEQ_FLOOR_BIT, 0, "clean.redb: no lost version, no bit");
        let m = mgr(&s);
        let sess = m.open(Arc::new(MD), &key("a.md"), None).unwrap();
        let c = sess.join(who(), Stay::Held).unwrap().conn;
        let r = sess.read(c).unwrap();
        assert_eq!(r.text_by_root[0].1, CLEAN_TEXT, "row: a.md as this build wrote it");
        let e = sess
            .replace_text(c, TextFilePack::ROOT, PREV_EDIT)
            .expect("row: the previous release's edit");
        assert!(e.version > r.version, "row: the edit made a version past {}", r.version);
        assert_eq!(sess.read(c).unwrap().text_by_root[0].1, PREV_EDIT, "the edit applied");
        drop(sess);
        m.release().unwrap();
        println!("N1-GROUPED clean store: format {format}, features word {word:#x}");
        (r.version, e.version)
    };
    let (text, kept) = read_doc(&p, "a.md");
    assert_eq!(text, PREV_EDIT, "row: the edit reads back after a reopen");
    assert_eq!(kept, after, "row: the edit's version kept");
    println!(
        "N1-GROUPED clean-open ok: the previous release opened a grouped store this build \
         closed cleanly, read a.md as written (version {version}), and its edit landed \
         (version {after}) and reads back after a reopen"
    );

    // a store a recovery stamped: refused at open, typed, its bytes unchanged
    let p = dir.join("stamped.redb");
    let before = fs::read(&p).unwrap();
    let e = match DocStore::open(&p) {
        Ok(_) => {
            eprintln!("row: the previous release opened a store a recovery stamped");
            std::process::exit(1);
        }
        Err(e) => e,
    };
    assert_eq!(
        e,
        StoreError::UnknownFeatures {
            found: 8,
            unknown: SEQ_FLOOR_BIT
        },
        "row: the refusal names the unknown bit"
    );
    let r = e.report();
    assert_eq!(
        (r.id(), r.entry().name, r.exit()),
        ("MPC-E502", "store_newer", exit::STORE),
        "row: typed store_newer"
    );
    let line = e.to_string();
    assert_eq!(line, STAMPED_LINE);
    assert!(!line.contains('\n'), "one line");
    assert!(fs::read(&p).unwrap() == before, "row: the refused open changed the file");
    println!(
        "N1-GROUPED stamped refused {} {} exit {}: {line}; bytes unchanged",
        r.id(),
        r.entry().name,
        r.exit()
    );
}

fn produce(dir: &Path) {
    let p = dir.join("prev.redb");
    let s = Arc::new(DocStore::open(&p).unwrap());
    let m = mgr(&s);
    let sess = m.open(Arc::new(MD), &key("b.md"), Some(PREV_MADE.as_bytes())).unwrap();
    let c = sess.join(who(), Stay::Held).unwrap().conn;
    let e = sess.replace_text(c, TextFilePack::ROOT, PREV_EDITED).unwrap();
    drop(sess);
    m.release().unwrap();
    drop(m);
    drop(s);
    let (text, version) = read_doc(&p, "b.md");
    assert_eq!(text, PREV_EDITED);
    assert_eq!(version, e.version);
    println!("N1-GROUPED prev produced prev.redb: b.md made and edited (version {version})");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (Some(mode), Some(dir)) = (args.get(1), args.get(2)) else {
        eprintln!("usage: read|produce <dir>");
        std::process::exit(2);
    };
    let dir = Path::new(dir);
    match mode.as_str() {
        "read" => read(dir),
        "produce" => produce(dir),
        _ => {
            eprintln!("usage: read|produce <dir>");
            std::process::exit(2);
        }
    }
}
