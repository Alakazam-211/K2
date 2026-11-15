//! N−1 of store format 8, the required-features word (tools/n1/n1.sh `layout-format`), built
//! against the previous release, which reads up to format 7.
//!
//! - `read <dir>`: `<dir>` holds `chunked.redb`, `native.redb` and `both.redb`, stores this
//!   repo's `n1_format_producer` made and stamped format 8 by hand (the word naming chunked,
//!   native, and both). The previous release refuses each at open with one line,
//!   `StoreError::Newer { found: 8 }` ("store is format 8; this build reads up to 7"), and the
//!   file's bytes are unchanged.
//! - `produce <dir>`: it writes `<dir>/prev.redb`, a store of its own with a text doc (`a.md`),
//!   for this repo's `n1_format_after` to open as before.
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::session::{
    AuditEvent, Config, Host, Participant, Rights, SessionManager, TextFilePack,
};
use k2_mpc_base::store::{DocKey, DocStore, StoreError};

const MD: TextFilePack = TextFilePack::new("markdown", 1 << 20);
/// The text doc's content (this repo's `n1_format_after` reads it back).
const TEXT: &str = "# Written by the previous release\n\nbody\n";

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
    for name in ["chunked", "native", "both"] {
        let p = dir.join(format!("{name}.redb"));
        let before = fs::read(&p).unwrap();
        let e = match DocStore::open(&p) {
            Ok(_) => panic!("{name}: the previous release opened a format-8 store"),
            Err(e) => e,
        };
        assert_eq!(e, StoreError::Newer { found: 8 }, "{name}");
        let line = e.to_string();
        assert_eq!(line, "store is format 8; this build reads up to 7", "{name}");
        assert!(!line.contains('\n'), "{name}: one line");
        assert!(
            fs::read(&p).unwrap() == before,
            "{name}: the refused open changed the file"
        );
        println!("N-1 FORMAT 8 REFUSED {name}: {line}; bytes unchanged");
    }
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
