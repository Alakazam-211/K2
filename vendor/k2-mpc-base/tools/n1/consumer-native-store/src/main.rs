//! N−1 of a store holding a native doc (tools/n1/n1.sh `layout-native-store`), built against the
//! previous release, which reads up to store format 7.
//!
//! - `read <dir>`: `<dir>/native.redb` is a store this repo's `n1_native_store_producer` wrote
//!   through the native engine's store (a native doc with its engine records: format 8 with the
//!   native bit). The previous release refuses it at OPEN, before any doc is read: one line,
//!   `StoreError::Newer { found: 8 }` ("store is format 8; this build reads up to 7"), the
//!   store-level `MPC-E502` (`store_newer`), never a doc-level engine refusal; the file's bytes
//!   are unchanged.
use std::fs;
use std::path::Path;

use k2_mpc_base::store::{DocStore, StoreError};

fn read(dir: &Path) {
    let p = dir.join("native.redb");
    let before = fs::read(&p).unwrap();
    let e = match DocStore::open(&p) {
        Ok(_) => panic!("the previous release opened a store holding a native doc"),
        Err(e) => e,
    };
    assert_eq!(e, StoreError::Newer { found: 8 }, "the store-level refusal at open");
    let line = e.to_string();
    assert_eq!(line, "store is format 8; this build reads up to 7");
    assert!(!line.contains('\n'), "one line");
    assert!(fs::read(&p).unwrap() == before, "the refused open changed the file");
    println!("N-1 NATIVE STORE REFUSED AT OPEN (store-level, MPC-E502 store_newer): {line}; bytes unchanged");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = Path::new(args.get(2).expect("usage: read <dir>"));
    match args.get(1).map(String::as_str) {
        Some("read") => read(dir),
        _ => panic!("usage: read <dir>"),
    }
}
