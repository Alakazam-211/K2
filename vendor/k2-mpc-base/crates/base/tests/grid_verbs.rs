//! The grid verbs (C1i, `table::verbs`; CSV contract §3.1) through `Session::run_tool` on a
//! table doc (the `grid-lite` test pack): every verb row of the contract, its addressing
//! (§1.7: keys, `#N`, `LN`, names, `key:`/`name:`, `--where`) and its exits (§3.3). Public API
//! only.

#[path = "grid_pack/mod.rs"]
mod grid_pack;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use grid_pack::GridLite;
use k2_mpc_base::session::{
    AuditEvent, Config, ConnId, Host, Participant, ParticipantKind, Rights, Session,
    SessionManager, Stay,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::table::ROWS;

struct H {
    rights: Mutex<HashMap<String, Rights>>,
}

impl Host for H {
    fn rights(&self, who: &Participant, _: &DocKey) -> Rights {
        self.rights
            .lock()
            .unwrap()
            .get(&who.pid)
            .copied()
            .unwrap_or(Rights::editor())
    }
    fn now_ms(&self) -> u64 {
        1_000_000
    }
    fn on_event(&self, _: &AuditEvent) {}
}

struct Fx {
    _dir: TmpDir,
    host: Arc<H>,
    s: Arc<Session>,
    p: ConnId,
}

struct TmpDir(std::path::PathBuf);
impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const SEED: &str = "id,city\nk1,Oslo\nk2,Rome\nk3,Lima\n";

fn fx(seed: &str) -> Fx {
    fx_pack(Arc::new(GridLite), seed)
}

fn fx_pack(pack: Arc<dyn k2_mpc_base::session::Pack>, seed: &str) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-grid-verbs-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let host = Arc::new(H {
        rights: Mutex::new(HashMap::new()),
    });
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mgr = SessionManager::new(store, host.clone(), Config::default());
    let s = mgr
        .open(
            pack,
            &DocKey {
                space: "ws".into(),
                path: "t.csv".into(),
            },
            Some(seed.as_bytes()),
        )
        .unwrap();
    let p = s
        .join(
            Participant::new("p", "P", "#1", ParticipantKind::Person, "tab"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    Fx {
        _dir: TmpDir(dir),
        host,
        s,
        p,
    }
}

/// A verb's exit and stdout (a refusal before it ran: its exit, its message).
fn run_as(f: &Fx, c: ConnId, argv: &str, stdin: &str) -> (u8, String) {
    let argv: Vec<String> = argv.split(' ').map(str::to_owned).collect();
    match f.s.run_tool(c, ROWS, &argv, stdin.as_bytes()) {
        Ok(o) => (o.exit, String::from_utf8_lossy(&o.stdout).into_owned()),
        Err(e) => (e.exit(), e.to_string()),
    }
}

fn run(f: &Fx, argv: &str) -> (u8, String) {
    run_as(f, f.p, argv, "")
}

fn ok(f: &Fx, argv: &str) -> String {
    let (exit, out) = run(f, argv);
    assert_eq!(exit, 0, "{argv}: {out}");
    out
}

/// The doc as its file would hold it.
fn file(f: &Fx) -> String {
    String::from_utf8(f.s.read(f.p).unwrap().bytes).unwrap()
}

// ---------- reads and addressing ----------

#[test]
fn cell_get_by_every_address() {
    let f = fx(SEED);
    for (argv, want) in [
        ("cell get --row r2 --col c2", "Rome\n"),
        ("cell get --row #2 --col #2", "Rome\n"),
        ("cell get --row L3 --col city", "Rome\n"),
        ("cell get --row r2 --col name:city", "Rome\n"),
        ("cell get --row r2 --col key:c2", "Rome\n"),
        ("cell get --where id=k2 --col city", "Rome\n"),
        ("cell get --where #1=k3 --col c2", "Lima\n"),
    ] {
        assert_eq!(ok(&f, argv), want, "{argv}");
    }
    let v = f.s.status().unwrap().version;
    assert_eq!(
        ok(&f, "cell get --row r1 --col c2 --json"),
        format!(
            "{{\"value\":\"Oslo\",\"row\":\"r1\",\"col\":\"c2\",\"version\":{v},\"author\":null,\"change\":null}}\n"
        ),
        "no history right: no author"
    );
}

/// With the history right, `cell get --json` carries the cell's last write (who, which
/// change) and a deleted row's refusal names who deleted it and in which change; without
/// it, neither (the refusal keeps the restore hint).
#[test]
fn json_and_deleted_refusals_carry_who_and_when() {
    let f = fx(SEED);
    f.host
        .rights
        .lock()
        .unwrap()
        .insert("p".into(), Rights::full());
    ok(&f, "cell set --row r2 --col city --value Milano");
    let set_in = f.s.status().unwrap().version;
    ok(&f, "row delete --row r3");
    let deleted_in = f.s.status().unwrap().version;
    assert_eq!(
        ok(&f, "cell get --row r2 --col city --json"),
        format!(
            "{{\"value\":\"Milano\",\"row\":\"r2\",\"col\":\"c2\",\"version\":{deleted_in},\"author\":\"p\",\"change\":{set_in}}}\n"
        )
    );
    for argv in [
        "cell get --row r3 --col city",
        "cell set --row r3 --col city --value x",
        "row move --row r3 --to #1",
    ] {
        let (exit, out) = run(&f, argv);
        assert_eq!(exit, 4, "{argv}: {out}");
        assert_eq!(
            out,
            format!("row r3 is deleted (by p, v{deleted_in}); row restore --row r3\n"),
            "{argv}"
        );
    }
    // a reader without history: the plain refusal
    let r =
        f.s.join(
            Participant::new("r", "R", "#2", ParticipantKind::Person, "tab"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    f.host
        .rights
        .lock()
        .unwrap()
        .insert("r".into(), Rights::read_only());
    assert_eq!(
        run_as(&f, r, "cell get --row r3 --col city", ""),
        (4, "row r3 is deleted; row restore --row r3\n".to_owned())
    );
}

#[test]
fn row_and_col_lists() {
    let f = fx(SEED);
    assert_eq!(
        ok(&f, "row list"),
        "r1\t#1\tL2\tk1\tOslo\nr2\t#2\tL3\tk2\tRome\nr3\t#3\tL4\tk3\tLima\n"
    );
    assert_eq!(
        ok(&f, "row list --cols city -n 2"),
        "r1\t#1\tL2\tOslo\nr2\t#2\tL3\tRome\n"
    );
    assert_eq!(
        ok(&f, "row list --where city=Lima"),
        "r3\t#3\tL4\tk3\tLima\n"
    );
    assert_eq!(run(&f, "row list --where city=Paris").0, 4);
    assert_eq!(ok(&f, "col list"), "c1\t#1\tid\nc2\t#2\tcity\n");
}

// ---------- cells ----------

#[test]
fn cell_set_by_value_and_stdin() {
    let f = fx(SEED);
    ok(&f, "cell set --row #1 --col city --value Bergen");
    assert_eq!(
        run_as(&f, f.p, "cell set --where id=k2 --col c2", "Milano\n").0,
        0
    );
    assert_eq!(file(&f), "id,city\nk1,Bergen\nk2,Milano\nk3,Lima\n");
}

#[test]
fn cells_set_pastes_a_block_and_adds_rows_past_the_end() {
    let f = fx(SEED);
    assert_eq!(
        run_as(&f, f.p, "cells set --from r3,c1", "k8,Accra\nk9,Bonn\n").0,
        0
    );
    assert_eq!(file(&f), "id,city\nk1,Oslo\nk2,Rome\nk8,Accra\nk9,Bonn\n");
    assert_eq!(
        run_as(&f, f.p, "cells set --from r1,c1", "").0,
        2,
        "no block"
    );
}

#[test]
fn cells_clear_a_rectangle() {
    let f = fx(SEED);
    ok(&f, "cells clear --from #1,#2 --to r2,city");
    assert_eq!(file(&f), "id,city\nk1,\nk2,\nk3,Lima\n");
}

// ---------- rows ----------

#[test]
fn row_insert_at_every_place() {
    let f = fx(SEED);
    let k = ok(&f, "row insert --at #2 --values k4,Kyiv");
    assert_eq!(k, "r4\n", "prints its key");
    ok(&f, "row insert --start --values k0,Accra");
    ok(&f, "row insert --end --values k9,Zug");
    ok(&f, "row insert --after r1 --values k5,Lyon");
    ok(&f, "row insert --before L2 --values kx,Bari");
    assert_eq!(
        run_as(&f, f.p, "row insert --end", "ks,Sofia\n").0,
        0,
        "values from stdin"
    );
    assert_eq!(
        file(&f),
        "id,city\nkx,Bari\nk0,Accra\nk1,Oslo\nk5,Lyon\nk4,Kyiv\nk2,Rome\nk3,Lima\nk9,Zug\nks,Sofia\n"
    );
}

#[test]
fn row_move_delete_restore() {
    let f = fx(SEED);
    ok(&f, "row move --row r3 --start");
    assert_eq!(file(&f), "id,city\nk3,Lima\nk1,Oslo\nk2,Rome\n");
    ok(&f, "row move --row r3 --to #3");
    assert_eq!(file(&f), "id,city\nk1,Oslo\nk2,Rome\nk3,Lima\n");
    ok(&f, "row move --where city=Oslo --after r2");
    assert_eq!(file(&f), "id,city\nk2,Rome\nk1,Oslo\nk3,Lima\n");
    ok(&f, "row delete --row r1");
    assert_eq!(file(&f), "id,city\nk2,Rome\nk3,Lima\n");
    ok(&f, "row restore --row r1");
    assert_eq!(file(&f), "id,city\nk2,Rome\nk1,Oslo\nk3,Lima\n");
    let (exit, out) = run(&f, "row restore --row r1");
    assert_eq!(
        (exit, out.as_str()),
        (0, "row r1 is not deleted: nothing to restore\n")
    );
}

// ---------- columns ----------

#[test]
fn col_insert_move_rename_delete_restore() {
    let f = fx(SEED);
    assert_eq!(ok(&f, "col insert --end --name pop"), "c3\n");
    ok(&f, "cell set --row r1 --col pop --value 5");
    ok(&f, "col move --col pop --to #1");
    assert_eq!(file(&f), "pop,id,city\n5,k1,Oslo\n,k2,Rome\n,k3,Lima\n");
    ok(&f, "col rename --col #3 --name town");
    ok(&f, "col delete --col id");
    assert_eq!(file(&f), "pop,town\n5,Oslo\n,Rome\n,Lima\n");
    ok(&f, "col restore --col c1");
    assert_eq!(
        ok(&f, "col list"),
        "c3\t#1\tpop\nc1\t#2\tid\nc2\t#3\ttown\n"
    );
    ok(&f, "col insert --at #1");
    assert_eq!(run(&f, "col insert --after nope").0, 4);
}

// ---------- sort and header ----------

#[test]
fn sort_by_name_desc_numeric_then() {
    let f = fx("n,city\n10,Oslo\n9,Rome\n10,Lima\n");
    ok(&f, "sort --by city");
    assert_eq!(file(&f), "n,city\n10,Lima\n10,Oslo\n9,Rome\n");
    ok(&f, "sort --by n --numeric --then city --desc");
    assert_eq!(file(&f), "n,city\n9,Rome\n10,Oslo\n10,Lima\n");
    // already sorted: no change, exit 0
    let v = f.s.status().unwrap().version;
    ok(&f, "sort --by n --numeric --then city --desc");
    assert_eq!(
        f.s.status().unwrap().version,
        v,
        "a sorted table is a no-op"
    );
    ok(&f, "sort --by #2 --desc");
    assert_eq!(file(&f), "n,city\n9,Rome\n10,Oslo\n10,Lima\n");
}

#[test]
fn header_off_and_on() {
    let f = fx(SEED);
    ok(&f, "header off");
    assert_eq!(
        file(&f),
        "id,city\nk1,Oslo\nk2,Rome\nk3,Lima\n",
        "same bytes"
    );
    assert_eq!(ok(&f, "col list"), "c1\t#1\t\nc2\t#2\t\n");
    assert_eq!(ok(&f, "header off"), "the header is already off\n");
    assert_eq!(run(&f, "col rename --col c1 --name x").0, 2, "header off");
    assert_eq!(run(&f, "cell get --row #1 --col city").0, 4, "no names");
    ok(&f, "header on");
    assert_eq!(ok(&f, "col list"), "c1\t#1\tid\nc2\t#2\tcity\n");
}

/// C1k (request 3): `header on` where the only row is blank is refused, exit 2, nothing
/// applied (the header line it would become isn't written: the file would change); `header
/// off` where no header line is written only changes the setting (no row added).
#[test]
fn header_switches_never_change_the_file() {
    // a blank header line (no names) and one row; the row deleted: no header line written
    let f = fx("\nb\n");
    ok(&f, "row delete --row r1");
    assert_eq!(file(&f), "", "nothing written");
    let v = f.s.status().unwrap().version;
    ok(&f, "header off");
    assert_eq!(file(&f), "", "same bytes");
    assert_eq!(ok(&f, "row list"), "", "no row added");
    assert_eq!(f.s.status().unwrap().version, v + 1, "the setting only");
    let f = fx("x\n\n");
    ok(&f, "header off");
    ok(&f, "row delete --row #1");
    assert_eq!(file(&f), "\n");
    let v2 = f.s.status().unwrap().version;
    let (exit, out) = run(&f, "header on");
    assert_eq!(exit, 2, "{out}");
    assert!(out.contains("only row is a blank line"), "{out}");
    assert_eq!(f.s.status().unwrap().version, v2, "nothing applied");
    assert_eq!(file(&f), "\n");
}

// ---------- exits (§3.3) ----------

#[test]
fn exits_are_the_contracts() {
    let f = fx("id,city,city\nk1,Oslo,x\nk2,Rome,y\nk2,Lima,z\n");
    // deleted row: 4, naming the restore
    ok(&f, "row delete --row r1");
    let (exit, out) = run(&f, "cell set --row r1 --col c1 --value x");
    assert_eq!(exit, 4, "{out}");
    assert!(
        out.contains("row r1 is deleted; row restore --row r1"),
        "{out}"
    );
    // two columns named city: 2, listing their keys
    let (exit, out) = run(&f, "cell get --row #1 --col city");
    assert_eq!(exit, 2, "{out}");
    assert!(out.contains("c2, c3"), "{out}");
    // --where matching 2 rows: 2, listing their keys
    let (exit, out) = run(&f, "cell get --where id=k2 --col c1");
    assert_eq!(exit, 2, "{out}");
    assert!(out.contains("r2, r3"), "{out}");
    // --where matching none: 4
    assert_eq!(run(&f, "cell get --where id=k9 --col c1").0, 4);
    // more values than columns: 2
    assert_eq!(run(&f, "row insert --end --values a,b,c,d").0, 2);
    // sort by an unknown column: 4
    assert_eq!(run(&f, "sort --by nope").0, 4);
    // unknown row, #N past the end, the header's line: 4
    assert_eq!(run(&f, "cell get --row r99 --col c1").0, 4);
    assert_eq!(run(&f, "cell get --row #9 --col c1").0, 4);
    assert_eq!(run(&f, "cell get --row L1 --col c1").0, 4);
    // a bad value (NUL; over 64 KiB): 2
    assert_eq!(run_as(&f, f.p, "cell set --row #1 --col c1", "a\0b").0, 2);
    let big = "x".repeat(65 * 1024);
    assert_eq!(run_as(&f, f.p, "cell set --row #1 --col c1", &big).0, 2);
    // flags: unknown, missing, both --row and --where: 2
    assert_eq!(run(&f, "cell get --row #1 --col c1 --bogus").0, 2);
    assert_eq!(run(&f, "cell get --col c1").0, 2);
    assert_eq!(run(&f, "cell get --row #1 --where id=k2 --col c1").0, 2);
    // --if-version stale: 1 (the kit's guard)
    let v = f.s.status().unwrap().version;
    let (exit, _) = run(
        &f,
        &format!(
            "cell set --row #1 --col c1 --value z --if-version {}",
            v - 1
        ),
    );
    assert_eq!(exit, 1);
    // read rights only: 3
    let r =
        f.s.join(
            Participant::new("r", "R", "#2", ParticipantKind::Person, "tab"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    f.host
        .rights
        .lock()
        .unwrap()
        .insert("r".into(), Rights::read_only());
    assert_eq!(
        run_as(&f, r, "cell set --row #1 --col c1 --value z", "").0,
        3
    );
    assert_eq!(
        run_as(&f, r, "cell get --row #1 --col c1", "").0,
        0,
        "reads are fine"
    );
}

#[test]
fn every_contract_verb_is_listed() {
    let names: Vec<&str> = k2_mpc_base::table::verbs::VERBS
        .iter()
        .map(|v| v.name())
        .collect();
    for v in [
        "cell get",
        "cell set",
        "cells set",
        "cells clear",
        "row insert",
        "row move",
        "row delete",
        "row restore",
        "row list",
        "col insert",
        "col move",
        "col delete",
        "col restore",
        "col rename",
        "col list",
        "sort",
        "header",
    ] {
        assert!(names.contains(&v), "{v}");
    }
}

// ---------- text edit verbs on a table (§3.2, through reconcile §2.9) ----------

/// A text edit verb runs on the rendering; its result applies as grid ops by key: rows keep
/// their keys, a changed line is a cell set, an added line a row insert, a removed line a
/// row delete (restorable).
#[test]
fn text_edit_verbs_apply_by_key() {
    let f = fx(SEED);
    let before = ok(&f, "row list");
    ok(&f, "sed -i s/Rome/Milano/");
    assert_eq!(file(&f), "id,city\nk1,Oslo\nk2,Milano\nk3,Lima\n");
    assert_eq!(
        ok(&f, "row list"),
        before.replace("Rome", "Milano"),
        "the same rows, by key"
    );
    assert_eq!(run_as(&f, f.p, "append", "k4,Kyiv\n").0, 0);
    assert_eq!(
        ok(&f, "cell get --row r4 --col city"),
        "Kyiv\n",
        "a new row, keyed"
    );
    ok(&f, "sed -i /Oslo/d");
    assert_eq!(file(&f), "id,city\nk2,Milano\nk3,Lima\nk4,Kyiv\n");
    ok(&f, "row restore --row r1");
    assert_eq!(file(&f), "id,city\nk1,Oslo\nk2,Milano\nk3,Lima\nk4,Kyiv\n");
}

/// The text verb's guards hold on a table: `--if-version` stale is 1; read rights only, 3.
#[test]
fn text_edit_verbs_on_a_table_keep_their_guards() {
    let f = fx(SEED);
    let v = f.s.status().unwrap().version;
    let (exit, _) = run(&f, &format!("sed -i s/Rome/X/ --if-version {}", v + 5));
    assert_eq!(exit, 1, "stale");
    let r =
        f.s.join(
            Participant::new("r", "R", "#2", ParticipantKind::Person, "tab"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    f.host
        .rights
        .lock()
        .unwrap()
        .insert("r".into(), Rights::read_only());
    assert_eq!(run_as(&f, r, "sed -i s/Rome/X/", "").0, 3);
    assert_eq!(file(&f), SEED, "nothing applied");
}

// ---------- write --base live on a grid ----------

/// `write --base live` replaces the grid, reconciled by key: rows matched by content keep
/// their keys; a changed cell is set, a missing row deleted (restorable), a new row added.
/// (The new row goes in another slot than the missing one: a row rewritten in its own slot
/// keeps its key by position, C1k.)
#[test]
fn write_base_live_replaces_the_grid_by_key() {
    let f = fx(SEED);
    let out =
        f.s.write_live_grid(
            f.p,
            f.s.read(f.p).unwrap().version,
            b"id,city\nk4,Kyiv\nk1,Oslo\nk2,Milano\n",
            &k2_mpc_base::session::GridWriteOptions::default(),
        )
        .unwrap();
    assert_eq!(out.exit(), 0);
    assert_eq!(file(&f), "id,city\nk4,Kyiv\nk1,Oslo\nk2,Milano\n");
    assert_eq!(
        ok(&f, "cell get --row r2 --col city"),
        "Milano\n",
        "r2 kept its key"
    );
    assert_eq!(run(&f, "cell get --row r3 --col city").0, 4, "r3 deleted");
    ok(&f, "row restore --row r3");
    assert_eq!(
        ok(&f, "cell get --row r3 --col city"),
        "Lima\n",
        "r3 back, with its cells"
    );
}

// ---------- a codec's own line (C1k, request 6) ----------

/// `grid-lite` with a `sep=,` line in front, which its line map gives to
/// `LineOwner::Preamble`.
struct SepLite;

impl k2_mpc_base::session::Pack for SepLite {
    fn schema(&self) -> k2_mpc_base::schema::Schema {
        GridLite.schema()
    }
    fn decode(
        &self,
        b: &[u8],
    ) -> Result<k2_mpc_base::session::PackContent, k2_mpc_base::session::Declined> {
        GridLite.decode(b.strip_prefix(b"sep=,\n").unwrap_or(b))
    }
    fn encode(
        &self,
        c: &[(String, k2_mpc_base::doc::Neutral)],
        codec: &[u8],
    ) -> Result<Vec<u8>, k2_mpc_base::session::PackError> {
        let mut out = b"sep=,\n".to_vec();
        out.extend(GridLite.encode(c, codec)?);
        Ok(out)
    }
    fn verbs(&self) -> &'static [k2_mpc_base::tools::PackVerb] {
        GridLite.verbs()
    }
    fn line_map(
        &self,
        content: &k2_mpc_base::session::View,
        _: &[u8],
    ) -> Option<k2_mpc_base::table::LineMap> {
        let mut m = content.table().ok()?.line_map();
        m.lines.insert(0, k2_mpc_base::table::LineOwner::Preamble);
        Some(m)
    }
}

#[test]
fn a_preamble_line_is_no_row_and_no_header() {
    let f = fx_pack(Arc::new(SepLite), "sep=,\nid,city\nk1,Oslo\n");
    assert_eq!(file(&f), "sep=,\nid,city\nk1,Oslo\n");
    assert_eq!(
        ok(&f, "cat --ids"),
        "preamble\tsep=,\nheader\tid,city\nr1\tk1,Oslo\n"
    );
    let (exit, out) = run(&f, "cell get --row L1 --col city");
    assert_eq!(exit, 4, "{out}");
    assert!(out.contains("preamble"), "{out}");
    assert_eq!(run(&f, "cell get --row L2 --col city").0, 4, "the header");
    assert_eq!(ok(&f, "cell get --row L3 --col city"), "Oslo\n");
}

// ---------- --ids ----------

/// `--ids` on `cat`, `head`, `tail`, `grep` and `view` prefixes each line with its row's key
/// (`header` for the header), from the pack's line map taken under the read's lock.
#[test]
fn ids_prefix_each_line_with_its_row_key() {
    let f = fx(SEED);
    assert_eq!(
        ok(&f, "cat --ids"),
        "header\tid,city\nr1\tk1,Oslo\nr2\tk2,Rome\nr3\tk3,Lima\n"
    );
    assert_eq!(ok(&f, "cat -n --ids"), {
        "header\t     1\tid,city\nr1\t     2\tk1,Oslo\nr2\t     3\tk2,Rome\nr3\t     4\tk3,Lima\n"
    });
    assert_eq!(ok(&f, "head -n 2 --ids"), "header\tid,city\nr1\tk1,Oslo\n");
    assert_eq!(ok(&f, "tail -n 1 --ids"), "r3\tk3,Lima\n");
    assert_eq!(
        ok(&f, "view --lines 3,4 --ids"),
        "r2\tk2,Rome\nr3\tk3,Lima\n"
    );
    assert_eq!(ok(&f, "grep --ids -n o"), "r1\t2:k1,Oslo\nr2\t3:k2,Rome\n");
    assert_eq!(ok(&f, "grep --ids -o R.."), "r2\tRom\n");
    assert_eq!(
        ok(&f, "grep --ids -A1 Oslo"),
        "r1\tk1,Oslo\nr2\tk2,Rome\n",
        "context lines too"
    );
    assert_eq!(
        ok(&f, "grep --ids -e id -e Lima -A1"),
        "header\tid,city\nr1\tk1,Oslo\n-\t--\nr3\tk3,Lima\n",
        "a separator is no doc line"
    );
    // keys follow the rows, not their places
    ok(&f, "sort --by city");
    assert_eq!(
        ok(&f, "cat --ids"),
        "header\tid,city\nr3\tk3,Lima\nr1\tk1,Oslo\nr2\tk2,Rome\n"
    );
    // (a multi-line cell puts its row's key on each of its lines: the pack's line map; the
    // test pack has no quoting, so the CSV pack's tests cover it)
    // empty output is fine; output that isn't doc lines is a usage error
    assert_eq!(run(&f, "grep --ids zzz"), (1, String::new()));
    for argv in ["head -c 3 --ids", "grep -c --ids a", "grep -l --ids a"] {
        assert_eq!(run(&f, argv).0, 2, "{argv}");
    }
    assert_eq!(run(&f, "wc --ids").0, 2, "wc takes no --ids");
}

// ---------- --for-edit ----------

/// `--for-edit` on `row list` / `cell get` takes a short claim on what they showed, by key
/// (rows first..last, or the one cell); on `col list` it shows no rows, so nothing is taken.
#[test]
fn for_edit_on_row_list_and_cell_get_claims_what_they_showed() {
    use k2_mpc_base::session::{ClaimSpan, ForEdit};
    let f = fx(SEED);
    let argv = |s: &str| -> Vec<String> { s.split(' ').map(str::to_owned).collect() };
    let o =
        f.s.run_tool(
            f.p,
            ROWS,
            &argv("row list --where city=Rome --for-edit"),
            b"",
        )
        .unwrap();
    assert_eq!(
        String::from_utf8(o.stdout).unwrap(),
        "r2\t#2\tL3\tk2\tRome\n"
    );
    match o.for_edit.as_deref() {
        Some(ForEdit::Claimed(c)) => assert_eq!(
            c.span,
            ClaimSpan::Rows {
                first: "r2".parse().unwrap(),
                last: "r2".parse().unwrap()
            }
        ),
        other => panic!("row list --for-edit: {other:?}"),
    }
    let o =
        f.s.run_tool(
            f.p,
            ROWS,
            &argv("cell get --row #3 --col city --for-edit --ttl 5s"),
            b"",
        )
        .unwrap();
    assert_eq!(String::from_utf8(o.stdout).unwrap(), "Lima\n");
    match o.for_edit.as_deref() {
        Some(ForEdit::Claimed(c)) => {
            assert_eq!(
                c.span,
                ClaimSpan::Cells {
                    rows: ("r3".parse().unwrap(), "r3".parse().unwrap()),
                    cols: ("c2".parse().unwrap(), "c2".parse().unwrap()),
                }
            );
            assert_eq!(c.expires_ms - c.taken_ms, 5_000, "--ttl");
        }
        other => panic!("cell get --for-edit: {other:?}"),
    }
    let o =
        f.s.run_tool(f.p, ROWS, &argv("col list --for-edit"), b"")
            .unwrap();
    assert!(
        matches!(o.for_edit.as_deref(), Some(ForEdit::NotTaken(_))),
        "{:?}",
        o.for_edit
    );
    // without the flag, no claim; --ttl alone is a usage error
    let o = f.s.run_tool(f.p, ROWS, &argv("row list"), b"").unwrap();
    assert!(o.for_edit.is_none());
    assert_eq!(run(&f, "row list --ttl 5s").0, 2);
    // another participant's claim covering the cell: the read succeeds, nothing is taken
    let q =
        f.s.join(
            Participant::new("q", "Q", "#3", ParticipantKind::Agent, "cli"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    let o =
        f.s.run_tool(q, ROWS, &argv("cell get --row r3 --col c2 --for-edit"), b"")
            .unwrap();
    assert_eq!(o.exit, 0);
    assert!(
        matches!(o.for_edit.as_deref(), Some(ForEdit::Conflict(c)) if c.pid == "p"),
        "{:?}",
        o.for_edit
    );
}
