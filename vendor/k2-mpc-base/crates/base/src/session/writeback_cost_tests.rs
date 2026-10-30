//! Where a file write-back's time goes as the doc's history grows (v0.9.7 work: the soak's
//! write-back was 65 ms at a 1.26 MB engine state). Ignored: a measurement, release.
//!
//! The history is a churn like the soak's: lines typed at random places, random lines
//! deleted and words replaced inside lines, by 16 clients, the text staying about 300 lines.
//! The sizes are `WB_CHANGES` (changes, comma-separated) or `WB_STATE_KB` (the engine state
//! to reach, in KiB; the soak's write-backs were at 740 and 1,290 KiB). At each size it times the four
//! whole-state steps of today's write-back (encode the doc; load the disk replica from that;
//! store it as the write-back in flight; promote it), and the incremental alternative: the
//! changes since the last write-back applied to the replica the last write-back left.
//!
//! ```sh
//! cargo test -p k2-mpc-base --release --lib writeback_cost -- --ignored --nocapture
//! WB_STATE_KB=740,1290 cargo test -p k2-mpc-base --release --lib writeback_cost -- --ignored --nocapture
//! ```
//!
//! Each size prints a `RUNREC` line (`tools/evidence/`): µs per step, and the two totals:
//! today's (encode, load, store, promote) and the incremental one (encode, apply, store,
//! promote).

use std::sync::Arc;
use std::time::Instant;

use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};

use crate::doc::{Doc, Edit};
use crate::schema::{Kind, Schema};
use crate::store::{DocKey, DocStore, SyncState};
use crate::text::diff::Hunk;

const S: Schema = Schema {
    name: "t",
    roots: &[("source", Kind::Text)],
};

/// One churn edit by `client`: a word replaced inside a random line (a third of the time), a
/// line typed at a random line start, or a random line deleted (when over 300 lines). Its
/// update.
fn churn(doc: &mut Doc, rng: &mut SmallRng, client: u64, n: u64) -> Vec<u8> {
    let t = doc.text("source").unwrap();
    let starts: Vec<usize> = std::iter::once(0)
        .chain(t.match_indices('\n').map(|(k, _)| k + 1))
        .filter(|&k| k < t.len())
        .collect();
    let word = if starts.len() > 50 && rng.gen_range(0..3) == 0 {
        // a word (between two spaces) inside a random line
        let k = rng.gen_range(0..starts.len());
        let end = starts.get(k + 1).copied().unwrap_or(t.len());
        let line = &t[starts[k]..end];
        let spaces: Vec<usize> = line.match_indices(' ').map(|(j, _)| j).collect();
        (spaces.len() >= 2).then(|| {
            let j = rng.gen_range(0..spaces.len() - 1);
            Hunk {
                start: starts[k] + spaces[j] + 1,
                end: starts[k] + spaces[j + 1],
                insert: format!("w{n}"),
            }
        })
    } else {
        None
    };
    let hunk = if let Some(h) = word {
        h
    } else if starts.len() > 300 || (starts.len() > 50 && rng.gen_bool(0.5)) {
        let k = rng.gen_range(0..starts.len());
        let end = starts.get(k + 1).copied().unwrap_or(t.len());
        Hunk {
            start: starts[k],
            end,
            insert: String::new(),
        }
    } else {
        let at = if starts.is_empty() {
            0
        } else {
            starts[rng.gen_range(0..starts.len())]
        };
        Hunk {
            start: at,
            end: at,
            insert: format!("line {n} by {client}, some words to type\n"),
        }
    };
    doc.edit_as(
        client,
        &[Edit::Text {
            root: "source".into(),
            hunks: vec![hunk],
        }],
    )
    .unwrap()
    .unwrap()
}

#[test]
#[ignore = "a measurement: release, --nocapture"]
fn writeback_cost() {
    let dir = std::env::temp_dir().join(format!("k2mb-wbcost-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    // a doc in the store to hang the sync rows on
    let host: Arc<dyn crate::session::Host> = Arc::new(NoHost);
    let mgr =
        crate::session::SessionManager::new(store.clone(), host, crate::session::Config::default());
    let key = DocKey {
        space: "ws".into(),
        path: "a.md".into(),
    };
    mgr.open(
        Arc::new(crate::session::TextFilePack::new("markdown", 64 << 20)),
        &key,
        Some(b"x\n"),
    )
    .unwrap();
    let id = store.find(&key).unwrap().unwrap();
    mgr.release().unwrap();

    let mut rng = SmallRng::seed_from_u64(1);
    let mut doc = Doc::new(S).unwrap();
    let mut replica = Doc::load(S, &doc.encode_state().unwrap()).unwrap();
    let mut since: Vec<Vec<u8>> = Vec::new();
    let mut changes = 0u64;
    let list =
        |v: String| -> Vec<u64> { v.split(',').map(|v| v.trim().parse().unwrap()).collect() };
    // (changes, state bytes): reach both
    let sizes: Vec<(u64, usize)> = match std::env::var("WB_STATE_KB") {
        Ok(v) => list(v)
            .into_iter()
            .map(|kb| (0, kb as usize * 1024))
            .collect(),
        Err(_) => {
            list(std::env::var("WB_CHANGES").unwrap_or_else(|_| "5000,20000,50000,80000".into()))
                .into_iter()
                .map(|c| (c, 0))
                .collect()
        }
    };
    let built = Instant::now();
    let mut state_len = 0usize;
    for (target, target_bytes) in sizes {
        while changes < target || state_len < target_bytes {
            if target_bytes > 0 && changes % 2000 == 0 {
                state_len = doc.encode_state().unwrap().len();
                if state_len >= target_bytes {
                    break;
                }
            }
            let u = churn(&mut doc, &mut rng, 100 + changes % 16, changes);
            since.push(u);
            changes += 1;
            // a write-back every 50 changes, incrementally (the replica stays in step)
            if changes % 50 == 0 {
                for u in since.drain(..) {
                    replica.apply_trusted(&u).unwrap();
                }
            }
        }
        // the last 50 changes since a write-back, as the soak's write-backs see
        for u in since.drain(..) {
            replica.apply_trusted(&u).unwrap();
        }
        let mut pending = Vec::new();
        for k in 0..50 {
            pending.push(churn(&mut doc, &mut rng, 100 + k % 16, changes + k));
        }
        changes += 50;
        let reps = 5;
        let (mut enc, mut load, mut put, mut promote, mut inc, mut bytes) =
            (0.0, 0.0, 0.0, 0.0, 0.0, 0);
        for _ in 0..reps {
            let t = Instant::now();
            let state = doc.encode_state().unwrap();
            enc += t.elapsed().as_secs_f64();
            bytes = state.len();
            let t = Instant::now();
            let fresh = Doc::load(S, &state).unwrap();
            load += t.elapsed().as_secs_f64();
            let t = Instant::now();
            store
                .set_pending_sync(
                    id,
                    &SyncState {
                        hash: vec![7; 32],
                        seq: changes,
                        state: state.clone(),
                    },
                )
                .unwrap();
            put += t.elapsed().as_secs_f64();
            let t = Instant::now();
            let meta = store.meta(id).unwrap();
            store.promote_pending_sync(id, &meta).unwrap();
            promote += t.elapsed().as_secs_f64();
            let mut r = Doc::load(S, &replica.encode_state().unwrap()).unwrap();
            let t = Instant::now();
            for u in &pending {
                r.apply_trusted(u).unwrap();
            }
            inc += t.elapsed().as_secs_f64();
            assert_eq!(r.text("source").unwrap(), fresh.text("source").unwrap());
            assert_eq!(r.state_vector(), fresh.state_vector());
            assert!(
                r.same_content(&fresh),
                "a replica kept in step holds what one reloaded from the doc holds"
            );
        }
        for u in pending {
            replica.apply_trusted(&u).unwrap();
        }
        let ms = |x: f64| x / reps as f64 * 1e3;
        let us = |x: f64| (x / reps as f64 * 1e6).round() as u64;
        eprintln!(
            "[writeback] {changes} changes, state {bytes} B (history built in {:.0} s): encode {:.2} ms, load replica {:.2} ms, store in flight {:.2} ms, promote {:.2} ms; incremental (50 changes onto the replica) {:.3} ms; today's write-back {:.2} ms, incremental {:.2} ms",
            built.elapsed().as_secs_f64(),
            ms(enc),
            ms(load),
            ms(put),
            ms(promote),
            ms(inc),
            ms(enc + load + put + promote),
            ms(enc + inc + put + promote)
        );
        let job = crate::differential::runrec_job();
        eprintln!(
            "RUNREC {{\"job\":{job},\"run\":\"writeback_cost_{changes}\",\"tool\":\"writeback_cost\",\
             \"status\":\"COMPLETE\",\"changes\":{changes},\"state_bytes\":{bytes},\"reps\":{reps},\
             \"since_last\":50,\"encode_us\":{},\"load_us\":{},\"pending_us\":{},\"promote_us\":{},\
             \"incremental_us\":{},\"full_total_us\":{},\"incremental_total_us\":{}}}",
            us(enc),
            us(load),
            us(put),
            us(promote),
            us(inc),
            us(enc + load + put + promote),
            us(enc + inc + put + promote)
        );
        state_len = bytes;
    }
    let _ = std::fs::remove_dir_all(&dir);
}

struct NoHost;

impl crate::session::Host for NoHost {
    fn rights(&self, _: &crate::session::Participant, _: &DocKey) -> crate::session::Rights {
        crate::session::Rights::full()
    }
    fn now_ms(&self) -> u64 {
        1_000
    }
    fn on_event(&self, _: &crate::session::AuditEvent) {}
}
