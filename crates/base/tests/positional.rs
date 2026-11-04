//! Positional addresses are bound to a version (K-ADDR-3, R-1, R-2), through
//! `Session::run_tool` / `run_tool_with`. Public API only.
//!
//! - **Grids (R-1):** under `--base B`, a grid edit's `#N` and `LN` count in version B, not in
//!   the live table: a row or column inserted (or moved) by someone else between the agent's
//!   read and its edit never changes the target (a property). A base the kit no longer holds
//!   refuses a positional edit with exit 8 (`MPC-E112`); a key needs no base view.
//! - **Text and pack verbs (R-2):** an agent's edit by line or range (`--lines`, `--range`,
//!   `--at`, a `sed -i` line address), or by a pack verb's declared positional address
//!   (`--nth`), with no `--if-version`: in [`PositionalMode::Warn`] (this release's default)
//!   it applies as before and `ToolOutput::warnings` (`tools::Warnings`) carries one `note:` naming
//!   `positional_needs_version`; in [`PositionalMode::Refuse`] it is `MPC-E171`, exit 2,
//!   nothing applied. With `--if-version`, or by a person, neither.

#[path = "grid_pack/mod.rs"]
mod grid_pack;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use grid_pack::GridLite;
use k2_mpc_base::parity::positions::{self, PositionForm, Unit};
use k2_mpc_base::parity::{PersonAction, Registry};
use k2_mpc_base::session::{
    AuditEvent, Config, ConnId, Host, Pack, Participant, ParticipantKind, Rights, Session,
    SessionManager, Stay, TextFilePack,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::table::ROWS;
use k2_mpc_base::tools::{
    positional_note, EditOutput, PackEdit, PackVerb, PositionalMode, ToolError, ToolOptions,
    ToolOutput, ToolRunError, POSITIONAL_DEFAULT,
};
use proptest::prelude::*;

const ROOT: &str = TextFilePack::ROOT;
const MD: TextFilePack = TextFilePack::new("markdown", 4 << 20);

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
    person: ConnId,
    agent: ConnId,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fx(pack: Arc<dyn Pack>, path: &str, seed: &str) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-positional-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mgr = SessionManager::new(store, Arc::new(H), Config::default());
    let key = DocKey {
        space: "ws".into(),
        path: path.into(),
    };
    let s = mgr.open(pack, &key, Some(seed.as_bytes())).unwrap();
    let join = |pid: &str, kind| {
        s.join(
            Participant::new(pid, pid, "#123456", kind, "cli"),
            Stay::Held,
        )
        .unwrap()
        .conn
    };
    let person = join("p", ParticipantKind::Person);
    let agent = join("a", ParticipantKind::Agent);
    Fx {
        dir,
        s,
        person,
        agent,
    }
}

fn argv(a: &str) -> Vec<String> {
    a.split(' ').map(str::to_owned).collect()
}

fn ok(f: &Fx, c: ConnId, root: &str, a: &str, stdin: &str) -> ToolOutput {
    let o =
        f.s.run_tool(c, root, &argv(a), stdin.as_bytes())
            .unwrap_or_else(|e| panic!("{a}: {e}"));
    assert_eq!(o.exit, 0, "{a}: {o:?}");
    o
}

fn text(f: &Fx) -> String {
    String::from_utf8(f.s.read(f.person).unwrap().bytes).unwrap()
}

fn version(f: &Fx) -> u64 {
    f.s.status().unwrap().version
}

// ---------- grids: R-1 ----------

fn grid(rows: usize) -> Fx {
    let mut seed = String::from("id,v\n");
    for i in 1..=rows {
        seed.push_str(&format!("k{i},x{i}\n"));
    }
    fx(Arc::new(GridLite), "t.csv", &seed)
}

/// The agent's read: the version its `--base` names.
fn agent_reads(f: &Fx) -> u64 {
    ok(f, f.agent, ROWS, "row list", "").version
}

fn cell(f: &Fx, row: &str, col: &str) -> String {
    let o = ok(
        f,
        f.person,
        ROWS,
        &format!("cell get --row {row} --col {col}"),
        "",
    );
    String::from_utf8(o.stdout).unwrap().trim_end().to_owned()
}

fn live_rows(f: &Fx) -> Vec<String> {
    let o = ok(f, f.person, ROWS, "row list", "");
    String::from_utf8(o.stdout)
        .unwrap()
        .lines()
        .map(|l| l.split('\t').next().unwrap().to_owned())
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// R-1: rows inserted by someone else between the agent's read and its edit never move
    /// the target: `#N` and `LN` name what they named in the base version.
    #[test]
    fn an_insert_between_read_and_write_never_moves_the_target(
        rows in 2usize..6,
        target in 1usize..6,
        inserts in proptest::collection::vec(0usize..7, 1..3),
        by_line in any::<bool>(),
    ) {
        let target = target.min(rows);
        let f = grid(rows);
        let base = agent_reads(&f);
        for at in inserts {
            let n = live_rows(&f).len();
            let place = if at == 0 { "--start".to_owned() } else if at > n { "--end".to_owned() } else { format!("--at #{at}") };
            ok(&f, f.person, ROWS, &format!("row insert {place} --values new,new"), "");
        }
        // the header is line 1, so row #N is line N+1 in the base version
        let addr = if by_line { format!("L{}", target + 1) } else { format!("#{target}") };
        let o = ok(&f, f.agent, ROWS, &format!("cell set --row {addr} --col c2 --value HIT --base {base}"), "");
        prop_assert!(o.warnings.is_empty(), "{:?}", o.warnings);
        // the row that was #target at the base: r{target} (keys follow the seed's order)
        prop_assert_eq!(cell(&f, &format!("r{target}"), "c2"), "HIT");
        for i in (1..=rows).filter(|i| *i != target) {
            prop_assert_eq!(cell(&f, &format!("r{i}"), "c2"), format!("x{i}"));
        }
    }
}

/// R-1 on columns, moves, `--at #N` and deletes: each counts in the base version.
#[test]
fn every_positional_form_counts_in_the_base() {
    // a column inserted first: --col #2 is still c2 (the base's second column)
    let f = grid(2);
    let base = agent_reads(&f);
    ok(&f, f.person, ROWS, "col insert --start --name new", "");
    ok(
        &f,
        f.agent,
        ROWS,
        &format!("cell set --row r1 --col #2 --value HIT --base {base}"),
        "",
    );
    assert_eq!(cell(&f, "r1", "c2"), "HIT");
    // rows moved: --row #1 is still r1
    let f = grid(3);
    let base = agent_reads(&f);
    ok(&f, f.person, ROWS, "row move --row r3 --start", "");
    ok(
        &f,
        f.agent,
        ROWS,
        &format!("row delete --row #1 --base {base}"),
        "",
    );
    assert_eq!(live_rows(&f), ["r3", "r2"]);
    // --to #1 moves to be the base's first (before r1), not before what is #1 now
    let f = grid(3);
    let base = agent_reads(&f);
    ok(&f, f.person, ROWS, "row insert --start --values n,n", "");
    ok(
        &f,
        f.agent,
        ROWS,
        &format!("row move --row r3 --to #1 --base {base}"),
        "",
    );
    let order = live_rows(&f);
    assert_eq!(order[1..], ["r3", "r1", "r2"], "{order:?}");
    // without a base (a person), positions are the live table's: #1 is the new first row
    ok(
        &f,
        f.person,
        ROWS,
        "cell set --row #1 --col c2 --value P",
        "",
    );
    assert_eq!(cell(&f, &order[0], "c2"), "P");
    // --at #2 inserts before the base's second (r2), not before what is #2 now (r1)
    let f = grid(3);
    let base = agent_reads(&f);
    ok(&f, f.person, ROWS, "row insert --start --values n,n", "");
    ok(
        &f,
        f.agent,
        ROWS,
        &format!("row insert --at #2 --values a,a --base {base}"),
        "",
    );
    assert_eq!(live_rows(&f), ["r4", "r1", "r5", "r2", "r3"]);
}

// ---------- grids: the key counters aren't content (K-BR-12) ----------

fn live_cols(f: &Fx) -> Vec<String> {
    let o = ok(f, f.person, ROWS, "col list", "");
    String::from_utf8(o.stdout)
        .unwrap()
        .lines()
        .map(|l| l.split('\t').next().unwrap().to_owned())
        .collect()
}

/// An agent's insert with `--base` after someone else's insert: the only thing the two share
/// is the table's key counter, which is bookkeeping, so it applies. `--at #2` counts in the
/// base (R-1), so the new row goes before r2, the base's second.
#[test]
fn an_agents_row_insert_after_someone_elses_insert_applies() {
    let f = grid(3);
    let base = agent_reads(&f);
    ok(&f, f.person, ROWS, "row insert --start --values n,n", "");
    ok(
        &f,
        f.agent,
        ROWS,
        &format!("row insert --at #2 --values a,a --base {base}"),
        "",
    );
    assert_eq!(live_rows(&f), ["r4", "r1", "r5", "r2", "r3"]);
    assert_eq!(cell(&f, "r4", "c1"), "n");
    assert_eq!(cell(&f, "r5", "c1"), "a");
}

/// The same on columns (the `next_col` counter).
#[test]
fn an_agents_col_insert_after_someone_elses_insert_applies() {
    let f = grid(2);
    let base = agent_reads(&f);
    ok(&f, f.person, ROWS, "col insert --start --name n", "");
    ok(
        &f,
        f.agent,
        ROWS,
        &format!("col insert --at #2 --name a --base {base}"),
        "",
    );
    assert_eq!(live_cols(&f), ["c3", "c1", "c4", "c2"]);
}

/// Two agents insert with `--base` from the same read: both apply, under distinct keys.
#[test]
fn two_agents_inserting_from_the_same_read_both_apply() {
    let f = grid(3);
    let other =
        f.s.join(
            Participant::new("b", "b", "#123456", ParticipantKind::Agent, "cli"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    let base = agent_reads(&f);
    assert_eq!(ok(&f, other, ROWS, "row list", "").version, base);
    let key = |o: ToolOutput| String::from_utf8(o.stdout).unwrap().trim_end().to_owned();
    let first = key(ok(
        &f,
        f.agent,
        ROWS,
        &format!("row insert --end --values a,a --base {base}"),
        "",
    ));
    let second = key(ok(
        &f,
        other,
        ROWS,
        &format!("row insert --end --values b,b --base {base}"),
        "",
    ));
    assert_ne!(first, second);
    assert_eq!(cell(&f, &first, "c1"), "a");
    assert_eq!(cell(&f, &second, "c1"), "b");
    assert_eq!(
        live_rows(&f),
        ["r1", "r2", "r3", first.as_str(), second.as_str()]
    );
}

/// A real conflict on an insert is still refused and applies nothing, with or without a
/// counter write beside it: the anchor moved after the base is `MPC-E803`; the anchor deleted
/// after the base is `MPC-E805`, as the verb resolves `--after` in the live table before any
/// facet is checked.
#[test]
fn a_real_conflict_on_an_insert_is_still_refused() {
    let insert = |base: u64| argv(&format!("row insert --after r2 --values a,a --base {base}"));
    // r2 deleted since the base
    let f = grid(3);
    let base = agent_reads(&f);
    ok(&f, f.person, ROWS, "row delete --row r2", "");
    let v = version(&f);
    let o = f.s.run_tool(f.agent, ROWS, &insert(base), b"").unwrap();
    assert_eq!(o.exit, 4, "{o:?}");
    assert_eq!(
        o.refusal.as_ref().map(|r| r.id()),
        Some("MPC-E805"),
        "{o:?}"
    );
    assert_eq!(version(&f), v, "nothing applied");
    assert_eq!(live_rows(&f), ["r1", "r3"]);
    // r2 moved since the base, alone and with someone's insert moving the counter too
    for insert_too in [false, true] {
        let f = grid(3);
        let base = agent_reads(&f);
        ok(&f, f.person, ROWS, "row move --row r2 --start", "");
        if insert_too {
            ok(&f, f.person, ROWS, "row insert --end --values n,n", "");
        }
        let (v, rows) = (version(&f), live_rows(&f));
        let e = f.s.run_tool(f.agent, ROWS, &insert(base), b"").unwrap_err();
        assert_eq!((e.exit(), e.report().id()), (1, "MPC-E803"), "{e}");
        assert!(e.to_string().contains("r2"), "{e}");
        assert_eq!(version(&f), v, "nothing applied");
        assert_eq!(live_rows(&f), rows, "nothing applied");
    }
}

/// An element `#N` named at the base that is gone at live is refused (exit 4), never
/// replaced by whatever is #N now.
#[test]
fn a_positional_target_deleted_since_the_base_is_refused() {
    let f = grid(3);
    let base = agent_reads(&f);
    ok(&f, f.person, ROWS, "row delete --row r2", "");
    let v = version(&f);
    let o =
        f.s.run_tool(
            f.agent,
            ROWS,
            &argv(&format!(
                "cell set --row #2 --col c2 --value X --base {base}"
            )),
            b"",
        )
        .unwrap();
    assert_eq!(o.exit, 4, "{o:?}");
    assert_eq!(version(&f), v, "nothing applied");
    assert_eq!(cell(&f, "r3", "c2"), "x3");
}

/// A positional edit whose base the kit doesn't hold (never read, never stored): exit 8,
/// `MPC-E112`, nothing applied; never resolved in the live table instead.
#[test]
fn a_positional_edit_on_a_base_not_held_is_too_old() {
    let f = grid(2);
    let v = version(&f);
    let e =
        f.s.run_tool(
            f.agent,
            ROWS,
            &argv("cell set --row #1 --col c2 --value X --base 999"),
            b"",
        )
        .unwrap_err();
    assert_eq!((e.exit(), e.report().id()), (8, "MPC-E112"), "{e}");
    assert_eq!(version(&f), v, "nothing applied");
}

// ---------- text and pack verbs: R-2 and the warn mode ----------

fn md() -> Fx {
    fx(Arc::new(MD), "n.md", "a\nb\nc\n")
}

fn refuse() -> ToolOptions {
    ToolOptions::new().positional(PositionalMode::Refuse)
}

fn warn() -> ToolOptions {
    ToolOptions::new().positional(PositionalMode::Warn)
}

/// This release warns; the next refuses (the constant the release flips).
#[test]
fn this_release_warns_by_default() {
    assert_eq!(POSITIONAL_DEFAULT, PositionalMode::Warn);
    assert_eq!(ToolOptions::default().positional, PositionalMode::Warn);
}

/// The note: one line, naming the id it becomes, and no host.
#[test]
fn the_note_names_the_refusal_and_no_host() {
    let n = positional_note("--lines 2,2");
    assert_eq!(
        n,
        "note: --lines 2,2 is a position, and this edit names no version: it applied to the \
         live text; a later release refuses it (positional_needs_version, MPC-E171). Pass \
         --if-version with the version you read, or name the target by key."
    );
    assert!(!n.contains('\n'));
    assert!(!n.contains("k2"), "{n}");
}

const POSITIONAL: &[(&str, &str, &str)] = &[
    ("replace --lines 2,2", "B\n", "--lines 2,2"),
    ("replace --range 2:1-2:2", "B", "--range 2:1-2:2"),
    ("delete --lines 2,2", "", "--lines 2,2"),
    ("insert --line 2", "x\n", "--line 2"),
    ("insert --after 1", "x\n", "--after 1"),
    ("insert --at 2:1", "x", "--at 2:1"),
    ("sed -i 2d", "", "sed -i line 2"),
];

/// Warn mode: an agent's positional edit with no version applies as before, with the note.
#[test]
fn warn_mode_applies_with_the_note() {
    for (a, stdin, addr) in POSITIONAL {
        for opts in [ToolOptions::default(), warn()] {
            let f = md();
            let o =
                f.s.run_tool_with(f.agent, ROOT, &argv(a), stdin.as_bytes(), &opts)
                    .unwrap_or_else(|e| panic!("{a}: {e}"));
            assert_eq!(o.exit, 0, "{a}: {o:?}");
            assert!(o.changed.is_some(), "{a}: applied");
            assert_eq!(o.warnings, vec![positional_note(addr)], "{a}");
            assert!(o.stdout.is_empty(), "{a}: stdout never carries it");
        }
    }
}

/// Refuse mode: `MPC-E171`, exit 2, nothing applied, the address in details.
#[test]
fn refuse_mode_refuses() {
    for (a, stdin, addr) in POSITIONAL {
        let f = md();
        let v = version(&f);
        let e =
            f.s.run_tool_with(f.agent, ROOT, &argv(a), stdin.as_bytes(), &refuse())
                .unwrap_err();
        assert!(
            matches!(e, ToolRunError::Tool(ToolError::Address(_))),
            "{a}: {e:?}"
        );
        let r = e.report();
        assert_eq!((r.id(), r.exit(), e.exit()), ("MPC-E171", 2, 2), "{a}");
        assert_eq!(
            r.detail("address").and_then(|v| v.as_str()),
            Some(*addr),
            "{a}"
        );
        assert_eq!(version(&f), v, "{a}: nothing applied");
        assert_eq!(text(&f), "a\nb\nc\n", "{a}");
    }
}

/// With `--if-version`, or by a person, or not positional: no note, never refused.
#[test]
fn a_version_a_person_or_no_position_needs_nothing() {
    for (a, stdin, _) in POSITIONAL {
        let f = md();
        let v = version(&f);
        let with = format!("{a} --if-version {v}");
        let o =
            f.s.run_tool_with(f.agent, ROOT, &argv(&with), stdin.as_bytes(), &refuse())
                .unwrap_or_else(|e| panic!("{with}: {e}"));
        assert!(
            o.warnings.is_empty() && o.changed.is_some(),
            "{with}: {o:?}"
        );
        let f = md();
        let o =
            f.s.run_tool_with(f.person, ROOT, &argv(a), stdin.as_bytes(), &refuse())
                .unwrap_or_else(|e| panic!("{a}: {e}"));
        assert!(o.warnings.is_empty() && o.changed.is_some(), "{a}: {o:?}");
    }
    for (a, stdin) in [
        ("append", "d\n"),
        ("insert --after $", "d\n"),
        ("insert --after 0", "z\n"),
        ("sed -i s/b/B/", ""),
        ("sed -i /b/d", ""),
        ("replace --old b --new B", ""),
    ] {
        let f = md();
        let o =
            f.s.run_tool_with(f.agent, ROOT, &argv(a), stdin.as_bytes(), &refuse())
                .unwrap_or_else(|e| panic!("{a}: {e}"));
        assert!(o.warnings.is_empty() && o.changed.is_some(), "{a}: {o:?}");
    }
}

/// A pack's text verb that declares its positional address (`--nth K`, R-2) gets the same
/// rule; without the declaration, nothing changes.
struct NthPack;

/// `pick --nth K` (stdin: the new text of the Kth line that is `x`); `pick` alone: the first.
fn pick(args: &[String]) -> Result<PackEdit, ToolError> {
    let k: usize = match args {
        [] => 1,
        [f, k] if f == "--nth" => k.parse().map_err(|_| ToolError::usage("--nth K"))?,
        _ => return Err(ToolError::usage("pick [--nth K]")),
    };
    Ok(Box::new(move |_doc: &str, live: &str, stdin: &[u8]| {
        let new = String::from_utf8_lossy(stdin).into_owned();
        let mut seen = 0;
        let out: Vec<String> = live
            .lines()
            .map(|l| {
                if l == "x" {
                    seen += 1;
                    if seen == k {
                        return new.clone();
                    }
                }
                l.to_owned()
            })
            .collect();
        Ok(EditOutput::new(Some(out.join("\n") + "\n"), Vec::new(), 0))
    }))
}

fn nth_of(args: &[String]) -> Option<String> {
    let i = args.iter().position(|a| a == "--nth")?;
    Some(format!("--nth {}", args.get(i + 1)?))
}

static NTH_VERBS: &[PackVerb] = &[PackVerb::edit(
    "pick",
    "pick [--nth K] (stdin: the new line)",
    &[PersonAction::FindReplace],
    &[0],
    pick,
)
.positional(nth_of)
.positions(&[PositionForm::new("--nth", Unit::PackNth)])];

impl Pack for NthPack {
    fn schema(&self) -> k2_mpc_base::Schema {
        MD.schema()
    }
    fn decode(
        &self,
        bytes: &[u8],
    ) -> Result<k2_mpc_base::session::PackContent, k2_mpc_base::session::Declined> {
        MD.decode(bytes)
    }
    fn encode(
        &self,
        content: &[(String, k2_mpc_base::Neutral)],
        codec: &[u8],
    ) -> Result<Vec<u8>, k2_mpc_base::session::PackError> {
        MD.encode(content, codec)
    }
    fn verbs(&self) -> &'static [PackVerb] {
        NTH_VERBS
    }
}

#[test]
fn the_nth_pack_declares_its_position_units() {
    positions::check(&Registry::for_pack(&NthPack).unwrap()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn a_pack_verb_s_declared_position_follows_the_rule() {
    let f = fx(Arc::new(NthPack), "x.md", "x\ny\nx\nx\n");
    let o =
        f.s.run_tool_with(f.agent, ROOT, &argv("pick --nth 2"), b"X", &warn())
            .unwrap();
    assert_eq!(o.warnings, vec![positional_note("--nth 2")]);
    assert_eq!(text(&f), "x\ny\nX\nx\n");
    let v = version(&f);
    let e =
        f.s.run_tool_with(f.agent, ROOT, &argv("pick --nth 1"), b"Z", &refuse())
            .unwrap_err();
    assert_eq!((e.report().id(), e.exit()), ("MPC-E171", 2));
    assert_eq!(version(&f), v, "nothing applied");
    // with --if-version (the kit takes it out first), or no --nth: neither
    let o =
        f.s.run_tool_with(
            f.agent,
            ROOT,
            &argv(&format!("pick --nth 1 --if-version {v}")),
            b"Z",
            &refuse(),
        )
        .unwrap();
    assert!(o.warnings.is_empty() && o.changed.is_some(), "{o:?}");
    let o =
        f.s.run_tool_with(f.agent, ROOT, &argv("pick"), b"W", &refuse())
            .unwrap();
    assert!(o.warnings.is_empty() && o.changed.is_some(), "{o:?}");
}
