//! What a grid edit, check and read cost on a table at the interim limit (25,000 cells), as
//! C1c left them (C1d part 2's measurement). Through the public session API, with the store
//! (redb, each change durable):
//!
//! - a person's cell set (`edit_ops`: the size-limit check, the engine edit, the store);
//! - an agent's cell set with `--base` (adds the per-facet base check, one index lookup);
//! - an agent's row delete with `--base` (the base check also reads the cells the row hides,
//!   from the index by the row's key prefix: O(its cells));
//! - `edit_ops_with` (adds the keyed state handed to the closure: O(doc));
//! - a read (the content, the encode, and the read base's keyed state: O(doc));
//! - the doc's keyed state alone, and the cells map alone.
//!
//! Ignored; minutes in release. Run it on a compute node:
//!
//! ```sh
//! cargo test -p k2-mpc-base --release --test grid_cost -- --ignored --nocapture
//! ```

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use k2_mpc_base::doc::{Doc, Edit, Neutral};
use k2_mpc_base::elem::{Key, MapOp, Value};
use k2_mpc_base::schema::Schema;
use k2_mpc_base::session::{
    AuditEvent, Config, Declined, EditOptions, Host, Pack, PackContent, PackError, Participant,
    ParticipantKind, Rights, SessionManager, Stay, View,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::table::{self, Field, Record, Shape, Table, CELLS};

const ROWS_N: usize = 2_500;
const COLS_N: usize = 10;

struct Lite;
const LITE: Schema = Schema {
    name: "csv-lite",
    roots: table::ROOTS,
};
impl Pack for Lite {
    fn schema(&self) -> Schema {
        LITE
    }
    fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
        let recs: Vec<Record> = std::str::from_utf8(b)
            .unwrap()
            .lines()
            .map(|l| l.split(',').map(Field::new).collect())
            .collect();
        let (_, edits) = Table::import(&recs, false, Shape::detect(&recs)).unwrap();
        let mut d = Doc::new(LITE).unwrap();
        d.edit_as(2, &edits).unwrap();
        Ok(PackContent {
            roots: d.export_neutral().unwrap(),
            codec: vec![1],
        })
    }
    fn encode(&self, c: &[(String, Neutral)], _: &[u8]) -> Result<Vec<u8>, PackError> {
        let t = View::of_content(c)
            .table()
            .map_err(|e| PackError(e.to_string()))?;
        let mut out = String::new();
        for rec in t.render() {
            let f: Vec<&str> = rec.iter().map(|f| f.value.as_str()).collect();
            out.push_str(&f.join(","));
            out.push('\n');
        }
        Ok(out.into_bytes())
    }
}

struct H {
    now: AtomicU64,
}
impl Host for H {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        self.now.fetch_add(1, Ordering::SeqCst)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

fn k(s: &str) -> Key {
    Key::new(s).unwrap()
}

/// p50, p99 and max of `us` (µs), printed and as a run record `grid_cost_<slug>`.
fn stats(slug: &str, name: &str, mut us: Vec<u128>) {
    us.sort_unstable();
    let at = |q: f64| us[((us.len() as f64 * q) as usize).min(us.len() - 1)];
    println!(
        "[grid cost] {name}: n {}, p50 {} µs, p99 {} µs, max {} µs",
        us.len(),
        at(0.5),
        at(0.99),
        us[us.len() - 1]
    );
    println!(
        "RUNREC {{\"job\":null,\"run\":\"grid_cost_{slug}\",\"tool\":\"grid_cost\",\"status\":\"MEASURED\",\"n\":{},\"p50_us\":{},\"p99_us\":{},\"max_us\":{}}}",
        us.len(),
        at(0.5),
        at(0.99),
        us[us.len() - 1]
    );
}

fn set(r: usize, c: usize, v: &str) -> Vec<Edit> {
    vec![Edit::Map {
        root: CELLS.into(),
        ops: vec![MapOp::Set {
            key: table::cell_key(&k(&format!("r{r}")), &k(&format!("c{c}"))),
            value: Value::str(v),
        }],
    }]
}

#[test]
#[ignore]
fn grid_costs_at_the_cell_limit() {
    let dir = std::env::temp_dir().join(format!("k2mb-gridcost-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut body = String::new();
    for r in 0..ROWS_N {
        let f: Vec<String> = (0..COLS_N).map(|c| format!("v{r}x{c}")).collect();
        body.push_str(&f.join(","));
        body.push('\n');
    }
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mut c = Config::default();
    c.edit_burst = 1_000_000;
    c.edit_rate_per_s = 1_000_000;
    let mgr = SessionManager::new(
        store,
        Arc::new(H {
            now: AtomicU64::new(1),
        }),
        c,
    );
    let t0 = Instant::now();
    let key = DocKey {
        space: "ws".into(),
        path: "big.csv".into(),
    };
    let s = mgr
        .open(Arc::new(Lite), &key, Some(body.as_bytes()))
        .unwrap();
    println!(
        "[grid cost] import of {} cells: {} ms",
        ROWS_N * COLS_N,
        t0.elapsed().as_millis()
    );
    let join = |pid: &str, kind| {
        s.join(
            Participant::new(pid, pid, "#111111", kind, "cli"),
            Stay::Held,
        )
        .unwrap()
        .conn
    };
    let p1 = join("p1", ParticipantKind::Person);
    let p2 = join("p2", ParticipantKind::Agent);
    let _warm = s.read(p1).unwrap();

    let mut us = Vec::new();
    for i in 0..300 {
        let e = set(i * 7 % ROWS_N + 1, i % COLS_N + 1, &format!("p{i}"));
        let t = Instant::now();
        s.edit_ops(p1, e, &EditOptions::new()).unwrap();
        us.push(t.elapsed().as_micros());
    }
    stats(
        "person_cell_set",
        "person cell set (limit check + edit + store)",
        us,
    );

    let mut us = Vec::new();
    for i in 0..300 {
        let base = s.read(p2).unwrap().version;
        let e = set(i * 11 % ROWS_N + 1, i % COLS_N + 1, &format!("a{i}"));
        let t = Instant::now();
        s.edit_ops(p2, e, &EditOptions::new().base(base)).unwrap();
        us.push(t.elapsed().as_micros());
    }
    stats(
        "agent_cell_set_base",
        "agent cell set with --base (+ base check)",
        us,
    );

    let mut us = Vec::new();
    for i in 0..50 {
        let base = s.read(p2).unwrap().version;
        let e = vec![Edit::List {
            root: table::ROWS.into(),
            ops: vec![k2_mpc_base::elem::ListOp::Delete {
                key: k(&format!("r{}", ROWS_N - i)),
            }],
        }];
        let t = Instant::now();
        s.edit_ops(p2, e, &EditOptions::new().base(base)).unwrap();
        us.push(t.elapsed().as_micros());
    }
    stats(
        "agent_row_delete_base",
        "agent row delete with --base (base check reads the row's cells from the index)",
        us,
    );

    let mut us = Vec::new();
    for i in 0..100 {
        let t = Instant::now();
        s.edit_ops_with(p1, &EditOptions::new(), |_v| {
            Ok::<_, String>(set(i * 13 % ROWS_N + 1, 1, &format!("w{i}")))
        })
        .unwrap();
        us.push(t.elapsed().as_micros());
    }
    stats(
        "edit_ops_with",
        "edit_ops_with (+ keyed state for the view)",
        us,
    );

    let mut us = Vec::new();
    for i in 0..100 {
        // a change first, so each read records a new read base (its keyed state)
        s.edit_ops(p1, set(i + 1, 2, &format!("r{i}")), &EditOptions::new())
            .unwrap();
        let t = Instant::now();
        s.read(p1).unwrap();
        us.push(t.elapsed().as_micros());
    }
    stats(
        "read",
        "read (content + encode + read base's keyed state)",
        us,
    );

    // the engine readers alone, on a doc holding the same table
    let content = s.read(p1).unwrap().content;
    let mut d = Doc::new(LITE).unwrap();
    let rows: Vec<Record> = View::of_content(&content).table().unwrap().render();
    let (_, edits) = Table::import(&rows, false, Shape::Rect).unwrap();
    d.edit_as(2, &edits).unwrap();
    let mut us = Vec::new();
    for _ in 0..30 {
        let t = Instant::now();
        let _ = d.keyed_state().unwrap();
        us.push(t.elapsed().as_micros());
    }
    stats("keyed_state", "Doc::keyed_state alone", us);
    let mut us = Vec::new();
    for _ in 0..30 {
        let t = Instant::now();
        let _ = d.map(CELLS).unwrap();
        us.push(t.elapsed().as_micros());
    }
    stats("map_cells", "Doc::map(cells) alone", us);
    drop((s, mgr));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The roster push's flush on a grid at the cell limit (v0.10.0, the deferred O(doc) item): a
/// subscriber watches while a person moves their cell caret (no change to the doc), and the
/// host flushes the roster after each move. Each flush reads the grid's axes and line map for
/// the carets' places; `roster_flush_caret` is that flush's cost. `roster_flush_edit` is a
/// flush right after a cell edit (the grid changed since the last flush). Ignored, release.
#[test]
#[ignore]
fn grid_roster_flush_cost() {
    let dir = std::env::temp_dir().join(format!("k2mb-gridroster-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut body = String::new();
    for r in 0..ROWS_N {
        let f: Vec<String> = (0..COLS_N).map(|c| format!("v{r}x{c}")).collect();
        body.push_str(&f.join(","));
        body.push('\n');
    }
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mut c = Config::default();
    c.edit_burst = 1_000_000;
    c.edit_rate_per_s = 1_000_000;
    let host = Arc::new(H {
        now: AtomicU64::new(1),
    });
    let mgr = SessionManager::new(store, host.clone(), c);
    let key = DocKey {
        space: "ws".into(),
        path: "big.csv".into(),
    };
    let s = mgr
        .open(Arc::new(Lite), &key, Some(body.as_bytes()))
        .unwrap();
    let join = |pid: &str, kind| {
        s.join(
            Participant::new(pid, pid, "#111111", kind, "cli"),
            Stay::Held,
        )
        .unwrap()
        .conn
    };
    let watcher = join("w", ParticipantKind::Person);
    let p1 = join("p1", ParticipantKind::Person);
    let rx = s.subscribe_roster(watcher).unwrap();
    let mut seen = 0usize;
    let mut us = Vec::new();
    for i in 0..300 {
        let r = k(&format!("r{}", i * 7 % ROWS_N + 1));
        let col = k(&format!("c{}", i % COLS_N + 1));
        s.set_cell_cursor(p1, (&r, &col), (&r, &col)).unwrap();
        host.now.fetch_add(1_000, Ordering::SeqCst);
        let t = Instant::now();
        s.flush_roster().unwrap();
        us.push(t.elapsed().as_micros());
        while rx.try_recv().is_ok() {
            seen += 1;
        }
    }
    assert!(seen >= 300, "a caret event per flush: {seen}");
    stats(
        "roster_flush_caret",
        "roster flush after a caret move (doc unchanged)",
        us,
    );
    let mut us = Vec::new();
    for i in 0..50 {
        let e = set(i * 13 % ROWS_N + 1, i % COLS_N + 1, &format!("e{i}"));
        s.edit_ops(p1, e, &EditOptions::new()).unwrap();
        host.now.fetch_add(1_000, Ordering::SeqCst);
        let t = Instant::now();
        s.flush_roster().unwrap();
        us.push(t.elapsed().as_micros());
        while rx.try_recv().is_ok() {}
    }
    stats(
        "roster_flush_edit",
        "roster flush after a cell edit (the grid changed)",
        us,
    );
    drop((rx, s, mgr));
    let _ = std::fs::remove_dir_all(&dir);
}
