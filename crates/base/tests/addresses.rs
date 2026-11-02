//! The kit's address grammar (K-ADDR-1, K-ADDR-2) on the grid verbs, through
//! `Session::run_tool` on a table doc (the grid test pack): the `@k:` key form, a bare number
//! that is never a position, and an ambiguous name or `--where` refused with exit 2,
//! `MPC-E170 ambiguous_address` and every candidate in `details.candidates` (the golden
//! below), never resolved to the first match. Public API only.

#[path = "grid_pack/mod.rs"]
mod grid_pack;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use grid_pack::GridLite;
use k2_mpc_base::session::{
    AuditEvent, Config, ConnId, Host, Participant, ParticipantKind, Rights, Session,
    SessionManager, Stay,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::table::ROWS;
use k2_mpc_base::tools::{ToolError, ToolRunError};

struct H;

impl Host for H {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::editor()
    }
    fn now_ms(&self) -> u64 {
        1_000_000
    }
    fn on_event(&self, _: &AuditEvent) {}
}

struct Fx {
    dir: std::path::PathBuf,
    s: Arc<Session>,
    p: ConnId,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fx(seed: &str) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-addresses-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mgr = SessionManager::new(store, Arc::new(H), Config::default());
    let key = DocKey {
        space: "ws".into(),
        path: "t.csv".into(),
    };
    let s = mgr
        .open(Arc::new(GridLite), &key, Some(seed.as_bytes()))
        .unwrap();
    let p = s
        .join(
            Participant::new("p", "P", "#1", ParticipantKind::Person, "tab"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    Fx { dir, s, p }
}

fn argv(a: &str) -> Vec<String> {
    a.split(' ').map(str::to_owned).collect()
}

/// A verb's exit and stdout, or its refusal's exit and message.
fn run(f: &Fx, a: &str) -> (u8, String) {
    match f.s.run_tool(f.p, ROWS, &argv(a), b"") {
        Ok(o) => (o.exit, String::from_utf8_lossy(&o.stdout).into_owned()),
        Err(e) => (e.exit(), e.to_string()),
    }
}

/// The refusal of a call that must be refused as an ambiguous address.
fn ambiguous(f: &Fx, a: &str) -> k2_mpc_base::errors::ErrorReport {
    let v = f.s.status().unwrap().version;
    let e = f.s.run_tool(f.p, ROWS, &argv(a), b"").unwrap_err();
    assert!(
        matches!(e, ToolRunError::Tool(ToolError::Address(_))),
        "{a}: {e:?}"
    );
    assert_eq!(f.s.status().unwrap().version, v, "{a}: nothing applied");
    let r = e.report();
    assert_eq!((r.id(), r.exit(), e.exit()), ("MPC-E170", 2, 2), "{a}");
    assert_eq!(r.message(), e.to_string(), "{a}: the verb's own line");
    r
}

#[test]
fn keys_take_the_grammar_s_key_form() {
    let f = fx("id,city\nk1,Oslo\nk2,Rome\n");
    for a in [
        "cell get --row @k:r2 --col @k:c2",
        "cell get --row key:r2 --col key:c2",
        "cell get --row r2 --col c2",
        "cell get --row #2 --col #2",
        "cell get --row L3 --col city",
    ] {
        assert_eq!(run(&f, a), (0, "Rome\n".to_owned()), "{a}");
    }
    // a malformed key is a usage error, whatever its form
    for a in [
        "cell get --row @k:R2 --col c2",
        "cell get --row r2 --col @k:",
    ] {
        assert_eq!(run(&f, a).0, 2, "{a}");
    }
}

/// P12: a bare number is a key or a name, never a position (`#N` is).
#[test]
fn a_bare_number_is_never_a_position() {
    let f = fx("a,b\nx,y\n");
    // no column named "2", no key "2": not found, never column #2
    let (exit, out) = run(&f, "cell get --row #1 --col 2");
    assert_eq!(exit, 4, "{out}");
    assert!(out.contains("no column named \"2\""), "{out}");
    // a column named "2" is found by its name
    let f = fx("1,2\nx,y\n");
    assert_eq!(run(&f, "cell get --row #1 --col 2"), (0, "y\n".to_owned()));
    // a row isn't a number either: "1" is the key 1, which no row has
    assert_eq!(run(&f, "cell get --row 1 --col #1").0, 4);
}

/// A grid has no native ids: a header named `@timestamp` (or `L3`) is a column name.
#[test]
fn names_that_look_like_other_forms_still_work() {
    let f = fx("@timestamp,L3\n1,2\n");
    assert_eq!(
        run(&f, "cell get --row #1 --col @timestamp"),
        (0, "1\n".to_owned())
    );
    assert_eq!(run(&f, "cell get --row #1 --col L3"), (0, "2\n".to_owned()));
    assert_eq!(
        run(&f, "cell get --row #1 --col name:L3"),
        (0, "2\n".to_owned())
    );
}

/// K-ADDR-2, golden: an ambiguous column name is refused with every candidate (never the
/// first), on reads and edits alike.
#[test]
fn an_ambiguous_name_is_refused_with_its_candidates() {
    let f = fx("id,city,city\nk1,Oslo,x\n");
    for a in [
        "cell get --row #1 --col city",
        "cell set --row #1 --col city --value z",
        "col delete --col city",
        "sort --by city",
    ] {
        let r = ambiguous(&f, a);
        assert_eq!(
            r.message(),
            "2 columns are named \"city\": c2, c3 (name one by key)"
        );
        assert_eq!(
            r.to_value().get("details").unwrap().to_json(),
            r##"{"what":"column","given":"city","candidates":[{"address":"@k:c2","position":"#2"},{"address":"@k:c3","position":"#3"}]}"##,
            "{a}"
        );
    }
    // each candidate's address names it alone
    assert_eq!(
        run(&f, "cell get --row #1 --col @k:c3"),
        (0, "x\n".to_owned())
    );
}

/// K-ADDR-2, golden: a `--where` that matches two rows is refused with both.
#[test]
fn an_ambiguous_where_is_refused_with_its_candidates() {
    let f = fx("id,city\nk1,Oslo\nk2,Rome\nk2,Lima\n");
    for a in [
        "cell get --where id=k2 --col city",
        "row delete --where id=k2",
    ] {
        let r = ambiguous(&f, a);
        assert_eq!(
            r.message(),
            "2 rows where c1 is \"k2\": r2, r3 (name one by key)"
        );
        assert_eq!(
            r.to_value().get("details").unwrap().to_json(),
            r##"{"what":"row","given":"c1=k2","candidates":[{"address":"@k:r2","position":"#2"},{"address":"@k:r3","position":"#3"}]}"##,
            "{a}"
        );
    }
}
