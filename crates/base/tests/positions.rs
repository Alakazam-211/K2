//! The position encoding (K-ADDR-4, `docs/POSITIONS.md`): `capabilities` states it, every verb
//! that takes or prints positions has a row in its one table, and each unit it states is the
//! one the verbs and the API use, on text with multi-byte and non-BMP characters (`é` is 2
//! bytes, `中` 3, `🎉` 4 bytes and 2 UTF-16 units; each is one column), and a file with a BOM
//! and CRLF line breaks. Public API only.
//!
//! - kit API offsets: UTF-8 bytes of the doc text (`set_cursor`, `set_cursor_at`, edit hunks);
//! - command-line verbs: `line:col`, 1-based, columns in Unicode scalar values
//!   (`insert --at`, `replace --range`, `delete --range`);
//! - byte tools: the file's bytes (`head -c`, `tail -c`, `wc -c`), `wc -m` its characters;
//!   line tools: the doc text;
//! - the Yjs wire: UTF-16 code units inside the update bytes, converted at the adapter, so a
//!   remote client's insert at a UTF-16 index lands where the CLI's `line:col` says.

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

mod grid_pack;
use grid_pack::GridLite;

use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{Doc as YDoc, GetString, OffsetKind, Options, ReadTxn, Text, Transact, Update};

use k2_mpc_base::errors::{capabilities, run_meta, Value};
use k2_mpc_base::parity::positions::{self, PositionForm, Unit, UNITS, VERB_POSITIONS};
use k2_mpc_base::parity::{PersonAction, Registry, Source, Tier};
use k2_mpc_base::session::position::{self as pos, LineCol};
use k2_mpc_base::session::{
    At, AuditEvent, Config, ConnId, CursorAtVersion, Declined, Host, Pack, PackContent, PackError,
    Participant, ParticipantKind, Rights, Session, SessionManager, Stay, TextFilePack,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::table::verbs::VERBS as GRID_VERBS;
use k2_mpc_base::text::diff::Hunk;
use k2_mpc_base::tools::{exit, PackEdit, PackVerb, ToolError, ToolOutput, ToolRunError};
use k2_mpc_base::{Doc, Edit, Kind, Schema};

const ROOT: &str = TextFilePack::ROOT;
const MD: TextFilePack = TextFilePack::new("markdown", 4 << 20);

/// `é` (2 bytes), `中` (3), `🎉` (4 bytes, 2 UTF-16 units), then `z`: columns 1..=4.
const LINE: &str = "é中🎉z";

// ---------------------------------------------------------------- capabilities

fn caps(regs: &[&Registry]) -> Value {
    capabilities(regs, &Config::default())
}

fn list<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    match v.get(k) {
        Some(Value::List(l)) => l,
        other => panic!("{k}: not a list: {other:?}"),
    }
}

fn str_of<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{k}: not a string in {}", v.to_json()))
}

/// The unit `capabilities` states for `verb`'s form `at` (the first such form for `tier`, or
/// with no tier).
fn unit_of(c: &Value, verb: &str, at: &str) -> String {
    let p = c.get("positions").expect("capabilities has no positions");
    let row = list(p, "verbs")
        .iter()
        .find(|r| str_of(r, "verb") == verb)
        .unwrap_or_else(|| panic!("no positions row for {verb}"));
    let form = list(row, "forms")
        .iter()
        .find(|f| str_of(f, "at") == at && matches!(f.get("tier"), Some(Value::Null) | None))
        .or_else(|| {
            list(row, "forms").iter().find(|f| {
                str_of(f, "at") == at && f.get("tier").and_then(Value::as_str) == Some("text")
            })
        })
        .unwrap_or_else(|| panic!("no form {at} for {verb}"));
    str_of(form, "unit").to_owned()
}

#[test]
fn capabilities_states_the_encoding() {
    let c = caps(&[]);
    let p = c.get("positions").expect("capabilities has no positions");
    // K-ADDR-4: UTF-8 bytes inside, line:col in Unicode scalar values at the edge
    assert_eq!(str_of(p, "internal"), "utf8_bytes");
    assert_eq!(
        str_of(p, "line_col"),
        "1-based lines; 1-based columns in Unicode scalar values"
    );
    assert_eq!(
        str_of(p, "wire"),
        "utf8_bytes",
        "the hunk channel carries the kit's bytes"
    );
    assert_eq!(str_of(p, "yjs_wire"), "utf16_code_units");
    // every unit is listed once, with its base
    let units: Vec<&str> = list(p, "units").iter().map(|u| str_of(u, "unit")).collect();
    assert_eq!(units, UNITS.iter().map(|u| u.word()).collect::<Vec<_>>());
    for u in list(p, "units") {
        let base = u.get("base").and_then(Value::as_u64);
        match str_of(u, "unit") {
            "utf8_bytes" | "utf16_code_units" => assert_eq!(base, Some(0)),
            "lines" | "line_col" | "rendered_lines" | "ordinal" | "pack_nth" => {
                assert_eq!(base, Some(1))
            }
            _ => assert_eq!(base, None, "{}", u.to_json()),
        }
    }
    // the text form says it too
    let cfg = Config::default();
    let text = run_meta(&["capabilities".to_owned()], false, &[], &cfg).unwrap();
    let text = String::from_utf8(text.stdout).unwrap();
    assert!(
        text.contains("\nPositions (docs/POSITIONS.md):\n"),
        "{text}"
    );
    assert!(text.contains("  inside the kit: utf8_bytes\n"));
    assert!(text.contains("    insert: --line lines, --after lines, --at line_col\n"));
}

/// Position-shaped words in a verb's usage line: a verb whose usage has one must have a row.
const POSITION_TOKENS: &[&str] = &[
    "L:C",
    "--lines",
    "--line ",
    "--after",
    "--before",
    "--range",
    "-c ",
    "[-c]",
    "#N",
    "LN",
    "-L ",
    "R,C",
    "R1",
    "C1",
    "--row ",
    "--col ",
    "--rows",
    "--cols",
    "--cells",
    "--cell",
    "--from",
    "--by",
    "-U N",
    "-A/-B/-C",
    "-n N",
    "'a,bp'",
    "[-n]",
    "-F N",
    "diff on stdin",
    "--rect",
    "X,Y",
];

/// Verbs whose usage has a token above but that take no position: `-n N` there is a count of
/// entries, `--by P` a participant.
const COUNTS_ONLY: &[&str] = &["activity", "log"];

#[test]
fn every_verb_with_positions_has_a_row() {
    let grid = Registry::for_pack(&GridLite).unwrap();
    let blob = Registry::for_tier(Tier::Blob);
    let mut listed = BTreeSet::new();
    for regs in [vec![], vec![&grid, &blob]] {
        let c = caps(&regs);
        let p = c.get("positions").unwrap();
        let rows: HashMap<&str, &Value> = list(p, "verbs")
            .iter()
            .map(|r| (str_of(r, "verb"), r))
            .collect();
        let verbs: HashMap<&str, &str> = list(&c, "verbs")
            .iter()
            .map(|v| (str_of(v, "name"), str_of(v, "usage")))
            .collect();
        for (name, usage) in &verbs {
            let positional = POSITION_TOKENS.iter().any(|t| usage.contains(t));
            if positional && !COUNTS_ONLY.contains(name) {
                assert!(
                    rows.contains_key(name),
                    "{name} takes positions ({usage:?}) but capabilities states no unit for it: \
                     add its row to parity::positions::VERB_POSITIONS"
                );
            }
        }
        for (name, row) in &rows {
            let usage = verbs.get(name).unwrap_or_else(|| {
                panic!("positions row {name} for a verb this host doesn't serve")
            });
            for f in list(row, "forms") {
                let at = str_of(f, "at");
                if at.starts_with('-') {
                    assert!(
                        positions::usage_names(usage, at),
                        "{name}: the row's {at} isn't in its usage {usage:?}"
                    );
                } else {
                    assert!(positions::NON_FLAG_PLACES.contains(&at), "{name}: {at}");
                }
            }
            listed.insert(name.to_string());
        }
    }
    // no row is dead: each names a verb some registry serves
    for r in VERB_POSITIONS {
        assert!(listed.contains(r.verb), "row {} names no verb", r.verb);
    }
}

// ---------------------------------------------------------------- pack verbs

fn no_parse(_: &[String]) -> Result<PackEdit, ToolError> {
    Err(ToolError::usage("not run here"))
}

/// A text pack with the verbs a test gives it.
struct WithVerbs(&'static [PackVerb]);

impl Pack for WithVerbs {
    fn schema(&self) -> Schema {
        MD.schema()
    }
    fn decode(&self, bytes: &[u8]) -> Result<PackContent, Declined> {
        MD.decode(bytes)
    }
    fn encode(
        &self,
        content: &[(String, k2_mpc_base::Neutral)],
        codec: &[u8],
    ) -> Result<Vec<u8>, PackError> {
        MD.encode(content, codec)
    }
    fn verbs(&self) -> &'static [PackVerb] {
        self.0
    }
}

const fn stitch() -> PackVerb {
    PackVerb::edit(
        "stitch",
        "stitch --heading H [--nth K]",
        &[PersonAction::Paste],
        &[0],
        no_parse,
    )
}

/// `capabilities`' row for `verb`, and its line in the text form.
fn row_and_line(reg: &Registry, verb: &str) -> (String, String) {
    let cfg = Config::default();
    let c = capabilities(&[reg], &cfg);
    let p = c.get("positions").unwrap();
    let row = list(p, "verbs")
        .iter()
        .find(|r| str_of(r, "verb") == verb)
        .unwrap_or_else(|| panic!("no positions row for {verb}"))
        .to_json();
    let text = run_meta(&["capabilities".to_owned()], false, &[reg], &cfg).unwrap();
    let text = String::from_utf8(text.stdout).unwrap();
    let start = format!("    {verb}: ");
    let line = text
        .lines()
        .find(|l| l.starts_with(&start))
        .unwrap_or_else(|| panic!("no line for {verb} in {text}"))
        .to_owned();
    (row, line)
}

/// A verb that declares its units shows them, in the same vocabulary as the kit's table.
#[test]
fn a_declared_pack_verb_shows_its_units() {
    static VERBS: &[PackVerb] = &[stitch().positions(&[PositionForm::new("--nth", Unit::PackNth)])];
    let reg = Registry::for_pack(&WithVerbs(VERBS)).unwrap();
    positions::check(&reg).unwrap();
    assert_eq!(unit_of(&caps(&[&reg]), "stitch", "--nth"), "pack_nth");
    assert_eq!(row_and_line(&reg, "stitch").1, "    stitch: --nth pack_nth");
}

/// `.positions(&[])` says the verb has none: it passes, with a line that says so.
#[test]
fn a_pack_verb_can_declare_it_has_no_positions() {
    static VERBS: &[PackVerb] = &[stitch().positions(&[])];
    let reg = Registry::for_pack(&WithVerbs(VERBS)).unwrap();
    positions::check(&reg).unwrap();
    let (row, line) = row_and_line(&reg, "stitch");
    assert_eq!(row, r#"{"verb":"stitch","forms":[]}"#);
    assert_eq!(line, "    stitch: takes and prints no positions");
}

/// The planted gap: a verb that declares nothing is not dropped from `capabilities`, it says
/// so, and the check every pack runs fails on it.
#[test]
fn an_undeclared_pack_verb_shows_and_fails_the_check() {
    static VERBS: &[PackVerb] = &[stitch()];
    let reg = Registry::for_pack(&WithVerbs(VERBS)).unwrap();
    let (row, line) = row_and_line(&reg, "stitch");
    assert_eq!(row, r#"{"verb":"stitch","undeclared":true,"forms":[]}"#);
    assert_eq!(
        line,
        "    stitch: undeclared (the pack states no position units)"
    );
    let e = positions::check(&reg).unwrap_err().0;
    assert_eq!(
        e,
        [
            "pack verb \"stitch\" declares no position units: add .positions(&[...]), or \
          .positions(&[]) if it takes and prints none"
        ]
    );
}

/// A declared flag the verb's usage doesn't have is refused; so is a place that isn't one.
#[test]
fn a_declared_form_must_be_in_the_usage() {
    static VERBS: &[PackVerb] = &[stitch().positions(&[
        PositionForm::new("--line", Unit::Lines),
        PositionForm::new("margin", Unit::Lines),
        PositionForm::new("--nth", Unit::PackNth),
    ])];
    let reg = Registry::for_pack(&WithVerbs(VERBS)).unwrap();
    let e = positions::check(&reg).unwrap_err().0;
    assert_eq!(e.len(), 2, "{e:#?}");
    assert!(e[0].contains("\"--line\""), "{e:#?}");
    assert!(e[1].contains("\"margin\""), "{e:#?}");
}

/// Every grid verb, as the kit's reference grid pack serves it: it states its positions, by
/// exactly one of the kit's table and its own declaration, and `capabilities` has its row.
#[test]
fn every_kit_grid_verb_states_its_positions() {
    let reg = Registry::for_pack(&GridLite).unwrap();
    positions::check(&reg).unwrap_or_else(|e| panic!("{e}"));
    let c = caps(&[&reg]);
    let p = c.get("positions").unwrap();
    for v in GRID_VERBS {
        let in_table = positions::of(v.name()).is_some();
        assert!(
            in_table != v.declared_positions().is_some(),
            "{}: in the kit's table: {in_table}; declared: {:?}",
            v.name(),
            v.declared_positions()
        );
        assert_eq!(reg.get(v.name()).unwrap().source, Source::Pack);
        assert!(
            list(p, "verbs")
                .iter()
                .any(|r| str_of(r, "verb") == v.name() && r.get("undeclared").is_none()),
            "{} has no declared row",
            v.name()
        );
    }
}

#[test]
fn the_units_capabilities_states_for_the_verbs_tested_here() {
    let c = caps(&[]);
    for (verb, at, unit) in [
        ("insert", "--at", "line_col"),
        ("replace", "--range", "line_col"),
        ("delete", "--range", "line_col"),
        ("replace", "--lines", "lines"),
        ("cursor", "--at", "line_col"),
        ("head", "-c", "file_bytes"),
        ("tail", "-c", "file_bytes"),
        ("wc", "-c", "file_bytes"),
        ("wc", "-m", "file_chars"),
        ("head", "-n", "lines"),
        ("view", "--lines", "lines"),
    ] {
        assert_eq!(unit_of(&c, verb, at), unit, "{verb} {at}");
    }
}

// ---------------------------------------------------------------- sessions

struct TestHost {
    now: AtomicU64,
}

impl Host for TestHost {
    fn rights(&self, _who: &Participant, _key: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        self.now.fetch_add(1, Ordering::SeqCst)
    }
    fn on_event(&self, _ev: &AuditEvent) {}
}

struct Fx {
    dir: std::path::PathBuf,
    mgr: SessionManager,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fx(tag: &str) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-positions-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_000_000),
    });
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    let mgr = SessionManager::new(store, host, c);
    Fx { dir, mgr }
}

impl Fx {
    fn open(&self, path: &str, bytes: &[u8]) -> (Arc<Session>, ConnId) {
        let key = DocKey {
            space: "ws".into(),
            path: path.into(),
        };
        let s = self.mgr.open(Arc::new(MD), &key, Some(bytes)).unwrap();
        let p = Participant::new("agent", "Agent", "#123456", ParticipantKind::Agent, "cli");
        let c = s.join(p, Stay::UntilIdle).unwrap().conn;
        (s, c)
    }
}

fn run(s: &Session, c: ConnId, a: &[&str], stdin: &str) -> Result<ToolOutput, ToolRunError> {
    let argv: Vec<String> = a.iter().map(|x| x.to_string()).collect();
    s.run_tool(c, ROOT, &argv, stdin.as_bytes())
}

fn ok(s: &Session, c: ConnId, a: &[&str], stdin: &str) -> ToolOutput {
    let o = run(s, c, a, stdin).unwrap_or_else(|e| panic!("{a:?}: {e:?}"));
    assert_eq!(
        o.exit,
        exit::OK,
        "{a:?}: {:?}",
        String::from_utf8_lossy(&o.stdout)
    );
    o
}

fn text(s: &Session, c: ConnId) -> String {
    s.read(c).unwrap().text_by_root.remove(0).1
}

fn file(s: &Session, c: ConnId) -> Vec<u8> {
    ok(s, c, &["cat"], "").stdout
}

/// `insert --at L:C` counts columns in Unicode scalar values: column 4 is after the emoji (in
/// UTF-16 units it would split it; in bytes it would land inside `中`).
#[test]
fn insert_at_counts_columns_in_scalar_values() {
    let f = fx("insert-at");
    let doc = format!("{LINE}\nsecond\n");
    for (i, (at, want)) in [
        ("1:1", "Xé中🎉z\n"),
        ("1:2", "éX中🎉z\n"),
        ("1:3", "é中X🎉z\n"),
        ("1:4", "é中🎉Xz\n"),
        ("1:5", "é中🎉zX\n"), // chars + 1: the line's end
    ]
    .into_iter()
    .enumerate()
    {
        let (s, c) = f.open(&format!("d{i}.md"), doc.as_bytes());
        ok(&s, c, &["insert", "--at", at], "X");
        assert_eq!(text(&s, c), format!("{want}second\n"), "insert --at {at}");
    }
    let (s, c) = f.open("e.md", doc.as_bytes());
    // past the line's end (4 characters: 1:6), even though the line is 10 bytes, 5 UTF-16 units
    let e = run(&s, c, &["insert", "--at", "1:6"], "X").unwrap_err();
    assert_eq!(e.exit(), exit::USAGE, "{e:?}");
    assert_eq!(text(&s, c), doc, "a refused insert applies nothing");
    // line 2 starts after the first line's `\n`
    ok(&s, c, &["insert", "--at", "2:7"], "!");
    assert_eq!(text(&s, c), format!("{LINE}\nsecond!\n"));
}

/// `replace --range` and `delete --range` count the same way; the range is half-open.
#[test]
fn ranges_count_columns_in_scalar_values() {
    let f = fx("range");
    let (s, c) = f.open("r.md", format!("{LINE}\n").as_bytes());
    ok(&s, c, &["replace", "--range", "1:3-1:4"], "!");
    assert_eq!(text(&s, c), "é中!z\n", "the emoji is one column");
    ok(&s, c, &["delete", "--range", "1:1-1:3"], "");
    assert_eq!(text(&s, c), "!z\n", "é and 中 are one column each");
    ok(&s, c, &["replace", "--range", "1:1-2:1"], "a🎉\n");
    assert_eq!(text(&s, c), "a🎉\n", "a range across a line break");
}

/// Byte tools count the file's bytes (BOM, CRLF), `wc -m` its characters; line tools and the
/// edit verbs' `line:col` see the doc text (no BOM, `\n`).
#[test]
fn byte_tools_count_the_file_and_line_tools_the_text() {
    let f = fx("bom-crlf");
    // BOM (3 bytes), é (2), 🎉 (4), CRLF (2), b, CRLF: 14 bytes, 8 characters, 2 lines
    let bytes = b"\xEF\xBB\xBF\xC3\xA9\xF0\x9F\x8E\x89\r\nb\r\n";
    let (s, c) = f.open("p.md", bytes);
    assert_eq!(text(&s, c), "é🎉\nb\n");
    assert_eq!(ok(&s, c, &["wc", "-c"], "").stdout, b"14 p.md\n");
    assert_eq!(ok(&s, c, &["wc", "-m"], "").stdout, b"8 p.md\n");
    assert_eq!(ok(&s, c, &["wc", "-l"], "").stdout, b"2 p.md\n");
    assert_eq!(
        ok(&s, c, &["head", "-c", "3"], "").stdout,
        b"\xEF\xBB\xBF",
        "the BOM"
    );
    assert_eq!(
        ok(&s, c, &["head", "-c", "5"], "").stdout,
        "\u{feff}é".as_bytes()
    );
    assert_eq!(
        ok(&s, c, &["tail", "-c", "3"], "").stdout,
        b"b\r\n",
        "CRLF counted"
    );
    assert_eq!(
        ok(&s, c, &["head", "-n", "1"], "").stdout,
        "é🎉\n".as_bytes()
    );
    assert_eq!(ok(&s, c, &["view", "--lines", "2,2"], "").stdout, b"b\n");
    // column 3 is after the emoji: the BOM is no column
    ok(&s, c, &["insert", "--at", "1:3"], "X");
    assert_eq!(text(&s, c), "é🎉X\nb\n");
    assert_eq!(
        file(&s, c),
        b"\xEF\xBB\xBF\xC3\xA9\xF0\x9F\x8E\x89X\r\nb\r\n"
    );
    assert_eq!(ok(&s, c, &["wc", "-c"], "").stdout, b"15 p.md\n");
}

/// The kit's API speaks UTF-8 byte offsets of the doc text: a caret set at `line:col` comes
/// back in bytes, a byte offset off a character boundary is refused, and a caret given in
/// bytes at a version (the hunk channel's `cursor` frame) is where `line:col` says.
#[test]
fn the_api_speaks_bytes() {
    let f = fx("api");
    let (s, c) = f.open("a.md", format!("{LINE}\n").as_bytes());
    let t = text(&s, c);
    let cur = s
        .set_cursor(
            c,
            ROOT,
            At::Line(LineCol { line: 1, col: 4 }),
            At::Line(LineCol { line: 1, col: 5 }),
        )
        .unwrap();
    assert_eq!((cur.anchor, cur.head), (9, 10), "2 + 3 + 4 bytes before z");
    assert_eq!(&t[cur.anchor..cur.head], "z");
    // inside the emoji (byte 6 of its 4 bytes 5..9): not a position
    assert!(s.set_cursor(c, ROOT, At::Byte(6), At::Byte(6)).is_err());
    // an edit's hunk is in bytes
    let v = s.status().unwrap().version;
    s.edit(
        c,
        vec![Edit::Text {
            root: ROOT.into(),
            hunks: vec![Hunk {
                start: 9,
                end: 9,
                insert: "Y".into(),
            }],
        }],
        None,
    )
    .unwrap();
    assert_eq!(text(&s, c), "é中🎉Yz\n");
    // a caret at byte 9 of the text at v (before the Y), carried through the change since
    match s.set_cursor_at(c, ROOT, v, 9, 9).unwrap() {
        CursorAtVersion::Set(k) => {
            assert_eq!(k.head, 10, "pushed past the Y (Bias::After)");
            assert_eq!(
                pos::line_col(&text(&s, c), k.head),
                Some(LineCol { line: 1, col: 5 })
            );
        }
        other => panic!("{other:?}"),
    }
}

/// `position::line_col` and `position::offset` are inverses on every character boundary, and
/// count columns in scalar values.
#[test]
fn byte_offsets_and_line_col_round_trip() {
    let t = format!("{LINE}\n\n🎉🎉\r\nlast");
    for (i, _) in t.char_indices().chain([(t.len(), ' ')]) {
        let lc = pos::line_col(&t, i).unwrap_or_else(|| panic!("byte {i}"));
        assert_eq!(pos::offset(&t, lc), Some(i), "byte {i} -> {lc:?}");
    }
    for i in 0..=t.len() {
        if !t.is_char_boundary(i) {
            assert_eq!(pos::line_col(&t, i), None, "byte {i} is inside a character");
        }
    }
    let z = t.find('z').unwrap();
    assert_eq!(z, 9);
    assert_eq!(pos::line_col(&t, z), Some(LineCol { line: 1, col: 4 }));
    // line 3: two emoji then a CR, which in doc text is a character of its line (only `\n`
    // ends a line)
    let l3 = pos::line_start(&t, 3).unwrap();
    assert_eq!(pos::offset(&t, LineCol { line: 3, col: 2 }), Some(l3 + 4));
    assert_eq!(
        pos::offset(&t, LineCol { line: 3, col: 3 }),
        Some(l3 + 8),
        "before the CR"
    );
    assert_eq!(
        pos::offset(&t, LineCol { line: 3, col: 4 }),
        Some(l3 + 9),
        "the line's end"
    );
    assert_eq!(pos::offset(&t, LineCol { line: 3, col: 5 }), None);
    assert_eq!(pos::line_count(&t), 4);
}

// ---------------------------------------------------------------- the Yjs wire's UTF-16

const S: Schema = Schema {
    name: "positions",
    roots: &[(ROOT, Kind::Text)],
};

fn client(id: u64) -> (YDoc, yrs::TextRef) {
    let doc = YDoc::with_options(Options {
        client_id: yrs::block::ClientID::new(id),
        offset_kind: OffsetKind::Utf16,
        ..Options::default()
    });
    let text = doc.get_or_insert_text(ROOT);
    (doc, text)
}

fn sync(c: &YDoc, s: &Doc) {
    let sv = c.transact().state_vector().encode_v1();
    let u = s.encode_diff(&sv).unwrap();
    c.transact_mut()
        .apply_update(Update::decode_v1(&u).unwrap())
        .unwrap();
}

/// A Yjs-style client counts UTF-16 code units; the kit converts at the engine adapter, so a
/// remote insert at UTF-16 index 4 (é 1, 中 1, 🎉 2) lands at byte 9, which is `1:4`; and the
/// kit's insert at byte 9 is at UTF-16 index 4 in the client's text.
#[test]
fn a_yjs_client_counts_utf16_and_the_kit_converts() {
    let mut s = Doc::new(S).unwrap();
    s.edit_as(
        2,
        &[Edit::Text {
            root: ROOT.into(),
            hunks: vec![Hunk {
                start: 0,
                end: 0,
                insert: format!("{LINE}\n"),
            }],
        }],
    )
    .unwrap();
    let id = 101;
    let (c, ct) = client(id);
    sync(&c, &s);
    assert_eq!(ct.get_string(&c.transact()), format!("{LINE}\n"));
    let at16 = "é中🎉".encode_utf16().count() as u32;
    assert_eq!(at16, 4);
    let upd = {
        let mut t = c.transact_mut();
        ct.insert(&mut t, at16, "Y");
        t.encode_update_v1()
    };
    s.apply_client_update(&upd, id).unwrap();
    let now = s.text(ROOT).unwrap();
    assert_eq!(now, "é中🎉Yz\n");
    let y = now.find('Y').unwrap();
    assert_eq!(y, 9, "UTF-16 index 4 is byte 9");
    assert_eq!(pos::line_col(&now, y), Some(LineCol { line: 1, col: 4 }));
    assert_eq!(pos::offset(&now, LineCol { line: 1, col: 4 }), Some(y));
    // the other way: the kit's byte offset 10 (after the Y) is UTF-16 index 5 for the client
    s.edit_as(
        3,
        &[Edit::Text {
            root: ROOT.into(),
            hunks: vec![Hunk {
                start: 10,
                end: 10,
                insert: "W".into(),
            }],
        }],
    )
    .unwrap();
    sync(&c, &s);
    let theirs = ct.get_string(&c.transact());
    assert_eq!(theirs, "é中🎉YWz\n");
    let w16 = theirs.encode_utf16().position(|u| u == 'W' as u16).unwrap();
    assert_eq!(w16, 5, "byte 10 is UTF-16 index 5");
}

/// A doc's own coordinates are positions, though the kit doesn't interpret them; and on a grid,
/// `who` and `activity` report what the line map counts.
#[test]
fn presence_rows_state_what_they_take_and_print() {
    let grid = Registry::for_pack(&GridLite).unwrap();
    let c = caps(&[&grid]);
    let p = c.get("positions").unwrap();
    let forms = |verb: &str| -> Vec<(String, String, Option<String>)> {
        let rows = list(p, "verbs");
        let row = rows
            .iter()
            .find(|r| str_of(r, "verb") == verb)
            .unwrap_or_else(|| panic!("no row for {verb}"));
        list(row, "forms")
            .iter()
            .map(|f| {
                (
                    str_of(f, "at").to_owned(),
                    str_of(f, "unit").to_owned(),
                    f.get("tier").and_then(Value::as_str).map(str::to_owned),
                )
            })
            .collect()
    };
    let form = |at: &str, unit: &str, tier: Option<&str>| {
        (at.to_owned(), unit.to_owned(), tier.map(str::to_owned))
    };
    assert_eq!(forms("pointer"), [form("--at", "doc_coords", None)]);
    assert_eq!(forms("viewport"), [form("--rect", "doc_coords", None)]);
    assert!(
        !list(p, "verbs")
            .iter()
            .any(|r| str_of(r, "verb") == "select"),
        "select takes keys, which don't shift"
    );
    let who = forms("who");
    let (text, grid_word) = ("text", "grid");
    assert!(
        who.contains(&form("stdout", "lines", Some(text))),
        "{who:?}"
    );
    assert!(
        who.contains(&form("stdout", "rendered_lines", Some(grid_word))),
        "{who:?}"
    );
    assert!(
        who.contains(&form("stdout", "doc_coords", Some(grid_word))),
        "{who:?}"
    );
    assert_eq!(
        forms("activity"),
        [
            form("stdout", "lines", Some(text)),
            form("stdout", "rendered_lines", Some(grid_word)),
        ]
    );
}
