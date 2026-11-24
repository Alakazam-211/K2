//! N−1 of the person-writer row's retired bit (`tools/n1/n1.sh node-retired`, its consumer
//! `tools/n1/consumer-retired`, built against the previous release).
//!
//! - `n1_retired_producer` (feature `native`): this build writes two stores under
//!   `$K2MB_N1_OUT`, each with one text doc (`keep.md`, on Yrs, so the previous release opens
//!   the store) whose person `ada` wrote on device `laptop` and released the writer: `free.redb`
//!   closed cleanly (the writer free, so the previous release lets it again: the control), and
//!   `retired.redb` closed uncleanly, then opened again (its writer incarnation rotates and the
//!   writer is retired: its row marked so). Each writer's id goes in `<store>.writer`.
//! - `n1_retired_after` (feature `native`): after the previous release opened both, joined
//!   `ada` on `laptop` and edited, this build reads `retired.redb` again: the doc as the
//!   previous release left it, the retired row still retired, and its own pick for `ada` on
//!   `laptop` never the retired writer.
//! - `n1_retired_prev_rows`: the other way: the previous release wrote `prev.redb` (a free and
//!   a parked writer of `ada`) and listed its rows in `prev.rows`; this build reads each row
//!   field for field. Without the native engine they are unchanged, retired bit clear, and
//!   `ada`'s free writer is let again; with it, this build's first open of a store without a
//!   writer incarnation rotates, so the same rows come back retired, every other field equal.
//!
//! Plant (`MPC_BETA_PLANT`): `retired-bit-dropped` (the bit not written): the previous release
//! lets the retired writer again, caught at the consumer's assert.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use super::*;
use crate::doc::ClientId;
use crate::store::{DocKey, DocStore, PersonWriterRow};

const MD: TextFilePack = TextFilePack::new("markdown", 1 << 20);

const NATIVE: bool = false;

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

fn out() -> PathBuf {
    PathBuf::from(std::env::var("K2MB_N1_OUT").expect("K2MB_N1_OUT"))
}

fn key() -> DocKey {
    DocKey {
        space: "ws".into(),
        path: "keep.md".into(),
    }
}

fn manager(store: &Arc<DocStore>) -> SessionManager {
    let host = Arc::new(H {
        now: AtomicU64::new(1_900_000_000_000),
    });
    let c = Config {
        reuse_person_writers: true,
        ..Config::default()
    };
    SessionManager::new(store.clone(), host, c)
}

fn ada() -> Participant {
    Participant::new("ada", "Ada", "#123456", ParticipantKind::Person, "tab")
}

fn laptop() -> JoinOptions {
    JoinOptions::new().device(b"laptop")
}

fn rows(store: &DocStore) -> Vec<(ClientId, String, PersonWriterRow)> {
    let id = store.find(&key()).unwrap().expect("keep.md is stored");
    store.person_writers(id).unwrap()
}

/// The other way (module docs).
#[test]
#[ignore = "N-1: tools/n1/n1.sh node-retired-prev"]
fn n1_retired_prev_rows() {
    let dir = out();
    // the previous release's rows: `client leased device token ended_token ended_seq ended_ms`
    let want: Vec<(ClientId, bool, String, u64, u64, u64, u64)> =
        std::fs::read_to_string(dir.join("prev.rows"))
            .unwrap()
            .lines()
            .map(|l| {
                let f: Vec<&str> = l.split(' ').collect();
                (
                    f[0].parse().unwrap(),
                    f[1] == "1",
                    f[2].to_owned(),
                    f[3].parse().unwrap(),
                    f[4].parse().unwrap(),
                    f[5].parse().unwrap(),
                    f[6].parse().unwrap(),
                )
            })
            .collect();
    assert!(
        want.len() >= 2,
        "control: the previous release wrote two rows: {want:?}"
    );
    let free: ClientId = std::fs::read_to_string(dir.join("prev.writer"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    // on a copy, so each build reads the rows as the previous release left them
    let native = NATIVE;
    let copy = dir.join(format!(
        "prev-{}.redb",
        if native { "native" } else { "default" }
    ));
    std::fs::copy(dir.join("prev.redb"), &copy).unwrap();
    let store = Arc::new(DocStore::open(&copy).expect("row: this build opens it"));
    let mut got: Vec<(ClientId, bool, String, u64, u64, u64, u64)> = Vec::new();
    for (c, owner, r) in rows(&store) {
        assert_eq!(owner, "ada", "row: the owner as written");
        assert_eq!(
            r.retired, native,
            "row: writer {c}: retired only by this build's own rotation (native: {native})"
        );
        let dev: String = r.device.iter().map(|b| format!("{b:02x}")).collect();
        got.push((
            c,
            r.leased,
            dev,
            r.token,
            r.ended_token,
            r.ended_seq,
            r.ended_ms,
        ));
    }
    got.sort();
    let mut want = want;
    want.sort();
    assert_eq!(
        got, want,
        "row: every row read field for field as the previous release wrote it"
    );
    let m = manager(&store);
    let s = m.open(Arc::new(MD), &key(), None).unwrap();
    let j = s.join_with(ada(), Stay::Held, &laptop()).unwrap();
    if native {
        assert_ne!(
            j.client, free,
            "row: native: the first open rotated, a fresh writer"
        );
    } else {
        assert_eq!(
            j.client, free,
            "row: ada's free writer is let again, as before"
        );
    }
    drop(s);
    m.release().unwrap();
    println!(
        "N1-RETIRED this build (native engine: {native}) read prev.redb's {} rows field for \
         field, retired bit {}, and its pick for ada on laptop {} ({})",
        got.len(),
        if native {
            "set by its own rotation"
        } else {
            "clear"
        },
        j.client,
        if j.client == free {
            "the free writer, let again"
        } else {
            "a fresh one"
        }
    );
}
