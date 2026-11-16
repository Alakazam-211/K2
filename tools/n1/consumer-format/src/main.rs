//! N−1 of store format 8, the required-features word (tools/n1/n1.sh `layout-format`), built
//! against the previous release. What it must do depends on its read range
//! (`STORE_FORMAT_READ`, at the previous release's tag): v0.14.0 to v0.18.0 read up to format
//! 7; v0.19.0 and later read format 8.
//!
//! - `read <dir>`: `<dir>` holds `chunked.redb`, `native.redb` and `both.redb`, stores this
//!   repo's `n1_format_producer` made and stamped format 8 by hand (the word naming chunked,
//!   native, and both), and `chunked.versions`, the chunked store's doc's versions as this
//!   repo reads them. A previous release that reads up to format 7 refuses each at open with
//!   one line, `StoreError::Newer { found: 8 }` ("store is format 8; this build reads up to
//!   7"). One that reads format 8 opens `chunked.redb` and reads its doc (`y.md`) as written:
//!   its versions equal `chunked.versions` and its text the producer's; and it refuses
//!   `native.redb` and `both.redb` at open with one line, as holding no engine store. A
//!   refused file's bytes are unchanged. An opened store's bytes are not (every kit's open
//!   commits a write transaction: its tables and sweep marker), so the chunked store is
//!   checked by content: opened again, it is still format 8 with the same versions, and this
//!   repo's `n1_format_after` then finds its word and versions as it wrote them. The text is
//!   read through a session on a copy.
//! - `produce <dir>`: it writes `<dir>/prev.redb`, a store of its own with a text doc (`a.md`),
//!   for this repo's `n1_format_after` to open as before.
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::session::{
    AuditEvent, Config, Host, Participant, ParticipantKind, Rights, SessionManager, Stay,
    TextFilePack,
};
use k2_mpc_base::store::{DocKey, DocStore, StoreError, STORE_FORMAT_READ};

const MD: TextFilePack = TextFilePack::new("markdown", 1 << 20);
/// The text doc's content (this repo's `n1_format_after` reads it back).
const TEXT: &str = "# Written by the previous release\n\nbody\n";
/// The text this repo's producer put in each format-8 store's doc (`engine_stamp.rs`, `TEXT`).
const PRODUCER_TEXT: &str = "# Title\n\nbody\n";
/// A format-7 reader's refusal of a format-8 store.
const NEWER_LINE: &str = "store is format 8; this build reads up to 7";
/// A format-8 reader's refusal of a store that sets the native feature with no engine store.
const NATIVE_LINE: &str =
    "store sets the native feature but holds no engine store: use the newer build";

struct H {
    now: AtomicU64,
    rights: Mutex<HashMap<String, Rights>>,
}

impl Host for H {
    fn rights(&self, who: &Participant, _key: &DocKey) -> Rights {
        self.rights
            .lock()
            .unwrap()
            .get(&who.pid)
            .copied()
            .unwrap_or(Rights::none())
    }
    fn now_ms(&self) -> u64 {
        self.now.fetch_add(1, Ordering::SeqCst)
    }
    fn on_event(&self, _ev: &AuditEvent) {}
}

fn read(dir: &Path) {
    let reads_8 = STORE_FORMAT_READ >= 8;
    println!(
        "N-1 FORMAT previous release reads up to format {STORE_FORMAT_READ}: {}",
        if reads_8 {
            "it opens and reads the chunked store, and refuses native and both"
        } else {
            "it refuses each format-8 store"
        }
    );
    for name in ["chunked", "native", "both"] {
        let p = dir.join(format!("{name}.redb"));
        let before = fs::read(&p).unwrap();
        let opens = reads_8 && name == "chunked";
        match DocStore::open(&p) {
            Ok(store) => {
                assert!(
                    opens,
                    "{name}: the previous release (reads up to {STORE_FORMAT_READ}) opened it"
                );
                assert_eq!(store.format().unwrap(), 8, "{name}");
                let id = store.find(&doc_key("y.md")).unwrap().expect("y.md");
                let versions = store.versions(id).unwrap();
                let want = fs::read_to_string(dir.join(format!("{name}.versions"))).unwrap();
                assert_eq!(format!("{versions:?}"), want, "{name}: the versions as written");
                drop(store);
                let again = DocStore::open(&p).unwrap();
                assert_eq!(again.format().unwrap(), 8, "{name}: the format kept");
                let id = again.find(&doc_key("y.md")).unwrap().expect("y.md");
                let kept = format!("{:?}", again.versions(id).unwrap());
                assert_eq!(kept, want, "{name}: the versions kept");
                drop(again);
                let copy = dir.join(format!("{name}-copy.redb"));
                fs::copy(&p, &copy).unwrap();
                assert_eq!(session_text(&copy, "y.md"), PRODUCER_TEXT, "{name}: the text");
                println!(
                    "N-1 FORMAT 8 READ {name}: opened at format 8, {} version(s) and the text as \
                     written; reopened: format and versions kept",
                    versions.len()
                );
            }
            Err(e) => {
                assert!(
                    !opens,
                    "{name}: the previous release (reads up to {STORE_FORMAT_READ}) refused: {e}"
                );
                let line = e.to_string();
                if reads_8 {
                    assert_eq!(line, NATIVE_LINE, "{name}");
                } else {
                    assert_eq!(e, StoreError::Newer { found: 8 }, "{name}");
                    assert_eq!(line, NEWER_LINE, "{name}");
                }
                assert!(!line.contains('\n'), "{name}: one line");
                assert!(
                    fs::read(&p).unwrap() == before,
                    "{name}: the refused open changed the file"
                );
                println!("N-1 FORMAT 8 REFUSED {name}: {line}; bytes unchanged");
            }
        }
    }
}

fn doc_key(path: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: path.into(),
    }
}

/// The text of the doc at `path` in the store at `p`, through a session (a reader's join).
fn session_text(p: &Path, path: &str) -> String {
    let host = Arc::new(H {
        now: AtomicU64::new(1_793_300_000_000),
        rights: Mutex::new(HashMap::new()),
    });
    host.rights.lock().unwrap().insert("r".into(), Rights::read_only());
    let store = Arc::new(DocStore::open(p).unwrap());
    let mgr = SessionManager::new(store, host, Config::default());
    let s = mgr.open(Arc::new(MD), &doc_key(path), None).unwrap();
    let who = Participant::new("r", "R", "#222222", ParticipantKind::Person, "cli");
    let c = s.join(who, Stay::Held).unwrap().conn;
    let text = s.read(c).unwrap().text_by_root.remove(0).1;
    drop(s);
    mgr.release().unwrap();
    text
}

fn produce(dir: &Path) {
    let host = Arc::new(H {
        now: AtomicU64::new(1_793_200_000_000),
        rights: Mutex::new(HashMap::new()),
    });
    let store = Arc::new(DocStore::open(dir.join("prev.redb")).unwrap());
    let mgr = SessionManager::new(store.clone(), host, Config::default());
    let key = DocKey {
        space: "ws".into(),
        path: "a.md".into(),
    };
    drop(mgr.open(Arc::new(MD), &key, Some(TEXT.as_bytes())).unwrap());
    mgr.release().unwrap();
    println!(
        "N-1 FORMAT produced prev.redb: format {}",
        store.format().unwrap()
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = Path::new(args.get(2).expect("usage: read|produce <dir>"));
    match args.get(1).map(String::as_str) {
        Some("read") => read(dir),
        Some("produce") => produce(dir),
        _ => panic!("usage: read|produce <dir>"),
    }
}
