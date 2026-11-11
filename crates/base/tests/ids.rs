//! The ID table (K-ID-13) and the ID-churn judge (K-COD-10).
//!
//! - `table_matches_docs`: `docs/IDS.md` is what `ids::kit_markdown()` generates from
//!   `ids::KIT`, and every verb a row names is a registered verb (`MPC_BLESS=1` rewrites
//!   the file).
//! - The judge over the kit's packs: the text pack (no native IDs: every step still runs, and
//!   each no-op save is byte-exact) and the grid test pack (row and column keys through a
//!   no-op save, verbs' cell set, row insert and row delete, and outside saves). A test pack
//!   with IDs in its file passes too.
//! - Planted renumberings fail it: a grid pack whose read-back re-keys one untouched row, and
//!   a file-ID pack whose save renumbers one untouched line's ID.

mod grid_pack;

use std::path::{Path, PathBuf};

use grid_pack::{render, GridLite};
use k2_mpc_base::addr::{self, Addr, Forms};
use k2_mpc_base::doc::Neutral;
use k2_mpc_base::elem::{Anchor, Key};
use k2_mpc_base::ids::churn::{judge, ChurnKind, ChurnReport, Saved, Unit};
use k2_mpc_base::ids::{self, IdClass, IdKind, KIT};
use k2_mpc_base::parity::{Registry, ANNOTATION_VERBS, KIT_VERBS};
use k2_mpc_base::schema::Schema;
use k2_mpc_base::session::{
    default_reconcile, Declined, Pack, PackContent, PackError, Reconciled, TextFilePack, View,
};
use k2_mpc_base::table::{self, verbs, Table};
use k2_mpc_base::text::diff;
use k2_mpc_base::tools::VerbCtx;
use k2_mpc_base::Edit;

fn bless() -> bool {
    std::env::var_os("MPC_BLESS").is_some()
}

fn manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Compare `got` with the file, or write it when blessing.
fn golden(path: &Path, got: &str) {
    if bless() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(path)
        .unwrap_or_else(|_| panic!("{} is missing (MPC_BLESS=1 writes it)", path.display()));
    assert!(
        want == got,
        "{} differs from what ids::KIT generates (MPC_BLESS=1 regenerates it)",
        path.display()
    );
}

/// A verb the kit registers: its own ([`KIT_VERBS`], [`ANNOTATION_VERBS`]) or a grid verb
/// ([`verbs::VERBS`]).
fn kit_verb(v: &str) -> bool {
    KIT_VERBS
        .iter()
        .chain(ANNOTATION_VERBS)
        .any(|s| s.name == v)
        || verbs::VERBS.iter().any(|p| p.name() == v)
}

// ------------------------------------------------------------- the table

/// K-ID-13: the docs are the data. `docs/IDS.md` is generated from `ids::KIT`, and every
/// verb a row says accepts the kind is one the kit registers.
#[test]
fn table_matches_docs() {
    ids::check(KIT, kit_verb).unwrap_or_else(|e| panic!("{e}"));
    let md = ids::kit_markdown();
    for k in KIT {
        assert!(
            md.contains(&format!("| `{}` |", k.kind)),
            "{} is in the docs",
            k.kind
        );
    }
    for c in IdClass::ALL {
        assert!(md.contains(&format!("| {} |", c.word())), "class {c}");
    }
    golden(&manifest().join("../../docs/IDS.md"), &md);
}

/// The grid rows' examples are what the kit's grid parsers read as that kind (the table
/// can't drift from `table/verbs.rs`'s addressing).
#[test]
fn grid_examples_parse_as_their_kind() {
    use verbs::{parse_col, parse_row, parse_where, ColAt, RowAt};
    let ex = |kind: &str| -> Vec<&'static str> {
        let k = KIT
            .iter()
            .find(|k| k.kind == kind)
            .unwrap_or_else(|| panic!("{kind} is in ids::KIT"));
        k.example.split(" | ").collect()
    };
    assert_eq!(
        parse_row(ex("row-key")[0]).unwrap(),
        RowAt::Key(Key::new("r17").unwrap())
    );
    assert_eq!(parse_row(ex("row-nth")[0]).unwrap(), RowAt::Nth(3));
    assert_eq!(parse_row(ex("rendered-line")[0]).unwrap(), RowAt::Line(5));
    assert_eq!(parse_col(ex("col-nth")[0]).unwrap(), ColAt::Nth(2));
    let [canonical, word, forced] = ex("col-key")[..] else {
        panic!("col-key: three forms")
    };
    assert_eq!(
        parse_col(canonical).unwrap(),
        ColAt::Key(Key::new("c3").unwrap())
    );
    assert_eq!(parse_col(word).unwrap(), ColAt::Word("c3".into()));
    assert_eq!(
        parse_col(forced).unwrap(),
        ColAt::Key(Key::new("c3").unwrap())
    );
    let [word, forced] = ex("col-name")[..] else {
        panic!("col-name: two forms")
    };
    assert_eq!(parse_col(word).unwrap(), ColAt::Word("email".into()));
    assert_eq!(parse_col(forced).unwrap(), ColAt::Name("email".into()));
    let w = ex("where")[0].strip_prefix("--where ").unwrap();
    assert_eq!(
        parse_where(w).unwrap(),
        RowAt::Where(ColAt::Word("sku".into()), "A7".into())
    );
    // the durable kinds are the keys; the forms that move are positional
    let class = |kind: &str| KIT.iter().find(|k| k.kind == kind).unwrap().class;
    assert_eq!(class("row-key"), IdClass::Durable);
    assert_eq!(class("col-key"), IdClass::Durable);
    for k in ["row-nth", "col-nth", "rendered-line", "line", "line-col"] {
        assert_eq!(class(k), IdClass::Positional, "{k}");
    }
    for k in ["col-name", "where", "exact-text"] {
        assert_eq!(class(k), IdClass::UniqueName, "{k}");
    }
}

/// `row-key` and `col-key` list the spellings the parser reads, the canonical one first, and
/// each names the same element of a real grid. `@k:` and `key:` parse to [`Addr::Key`]; the
/// bare word parses to [`Addr::Word`], which a grid reads as the key.
#[test]
fn key_spellings_parse_and_name_the_same_element() {
    let content = GridLite.decode(b"name,qty\napple,1\npear,2\n").unwrap();
    let view = View::of_content(&content.roots);
    let t = view.table().unwrap();
    let (rows, cols) = (t.live_rows(), t.live_cols());
    let cell_get = |row: &str, col: &str| {
        let verb = verbs::VERBS
            .iter()
            .find(|v| v.name() == "cell get")
            .unwrap();
        let args = ["--row", row, "--col", col].map(str::to_owned);
        let run = verb
            .parse_structured(&args)
            .unwrap_or_else(|e| panic!("{row},{col}: {e:?}"));
        let ctx = VerbCtx::new(&view, b"", &GridLite, &content.codec);
        let out = run(&ctx).unwrap_or_else(|e| panic!("{row},{col}: {e:?}"));
        assert_eq!(out.exit, 0, "{row},{col}");
        out.stdout
    };
    let spellings = |kind: &str| -> Vec<&'static str> {
        let k = KIT.iter().find(|k| k.kind == kind).unwrap();
        assert_eq!(k.class, IdClass::Durable, "{kind}");
        k.example.split(" | ").collect()
    };
    for (kind, key) in [("row-key", "r17"), ("col-key", "c3")] {
        let listed = spellings(kind);
        // the canonical spelling comes first: it is what the parser prints back
        let first = addr::parse(listed[0], Forms::none()).unwrap();
        assert_eq!(first.to_string(), listed[0], "{kind}");
        assert_eq!(
            listed,
            [format!("@k:{key}"), key.to_owned(), format!("key:{key}")],
            "{kind}"
        );
        assert_eq!(
            addr::parse(listed[0], Forms::none()).unwrap(),
            Addr::Key(key.into())
        );
        assert_eq!(
            addr::parse(listed[1], Forms::none()).unwrap(),
            Addr::Word(key.into())
        );
        assert_eq!(
            addr::parse(listed[2], Forms::none()).unwrap(),
            Addr::Key(key.into())
        );
    }
    // on a real grid: each spelling of a row and a column names the same cell
    let (row, col) = (rows[1].as_str(), cols[1].as_str());
    let want = cell_get(row, col);
    assert_eq!(want, b"2\n");
    for r in spellings("row-key") {
        for c in spellings("col-key") {
            let (r, c) = (r.replace("r17", row), c.replace("c3", col));
            assert_eq!(cell_get(&r, &c), want, "{r},{c}");
        }
    }
    // the control: another row's key names another cell, so the agreement above means something
    assert_ne!(cell_get(&format!("@k:{}", rows[0].as_str()), col), want);
}

/// `ids::check` refuses a table that doesn't fit its registry (the negative control for
/// `table_matches_docs`'s data check), and a pack's own rows are reached through
/// `Pack::ids` (none by default).
#[test]
fn check_refuses_bad_rows() {
    const BAD: &[IdKind] = &[
        IdKind {
            kind: "Para Id",
            example: "x",
            class: IdClass::FileLocal,
            scope: "a part",
            accepted_by: &["no-such-verb"],
            note: "n",
        },
        IdKind {
            kind: "tag",
            example: "x",
            class: IdClass::Stamp,
            scope: "a part",
            accepted_by: &["cat", "cat"],
            note: " ",
        },
        IdKind {
            kind: "tag",
            example: "x",
            class: IdClass::Durable,
            scope: "a part",
            accepted_by: &[],
            note: "n",
        },
    ];
    let e = ids::check(BAD, kit_verb).unwrap_err().0;
    for want in [
        "kind \"Para Id\": lowercase words joined by '-'",
        "kind \"Para Id\": verb \"no-such-verb\" is not in the registry",
        "kind \"tag\": verb \"cat\" is listed twice",
        "kind \"tag\": no note",
        "kind \"tag\": a stamp addresses nothing, so no verb accepts it",
        "kind \"tag\" is listed twice",
    ] {
        assert!(e.iter().any(|x| x == want), "{want:?} in {e:#?}");
    }
    assert_eq!(e.len(), 6, "{e:#?}");
    // a mutant table, one verb renamed: refused
    let mut renamed = KIT.to_vec();
    renamed[0].accepted_by = &["vieww"];
    assert!(ids::check(&renamed, kit_verb).is_err());
    // the kit's packs read no IDs of their own; a pack's rows check against its registry
    assert!(TextFilePack::new("text", 64).ids().is_empty());
    assert!(GridLite.ids().is_empty());
    let tagged = Tagged { renumber: false };
    assert_eq!(tagged.ids().len(), 1);
    let reg = Registry::for_pack(&tagged).unwrap();
    ids::check(tagged.ids(), |v| reg.get(v).is_some()).unwrap();
    assert!(ids::markdown(tagged.ids()).contains("| `line-tag` | `7:text` | file-local |"));
}

// ------------------------------------------------------------- the judge: text

const TEXT: TextFilePack = TextFilePack::new("text", 1 << 20);
const ROOT: &str = TextFilePack::ROOT;

fn text_corpus() -> Vec<(&'static str, &'static [u8])> {
    vec![
        ("lf", b"one\ntwo\n".as_slice()),
        ("crlf-bom", b"\xEF\xBB\xBFone\r\ntwo\r\n".as_slice()),
        ("empty", b"".as_slice()),
        ("no-final-newline", b"last line".as_slice()),
    ]
}

fn text_edit(s: &Saved<'_>, new: &str) -> Vec<Edit> {
    let old = s.view.text(ROOT).expect("a text root");
    vec![Edit::Text {
        root: ROOT.to_owned(),
        hunks: diff::hunks(old, new),
    }]
}

/// The text pack has no native IDs: the judge still runs every step (each no-op save
/// byte-exact) and compares nothing.
#[test]
fn churn_text_pack() {
    let units = |_: &str, s: &Saved<'_>| {
        let text = s.view.text(ROOT).unwrap().to_owned();
        vec![
            Unit::doc(
                "append (verb)",
                &[],
                text_edit(s, &format!("{text}three\n")),
            ),
            Unit::file(
                "append outside",
                &[],
                [s.bytes, b"more".as_slice()].concat(),
            ),
        ]
    };
    let j = judge(&TEXT, &text_corpus(), |_| Vec::new(), units).unwrap_or_else(|r| panic!("{r}"));
    assert_eq!((j.files, j.steps, j.ids_checked), (4, 12, 0));
}

// ------------------------------------------------------------- the judge: grid

fn grid_corpus() -> Vec<(&'static str, &'static [u8])> {
    vec![
        ("fruit", b"name,qty\napple,1\npear,2\nplum,3\n".as_slice()),
        ("wide", b"a,b,c\n1,2,3\n4,5,6\n7,8,9\n10,11,12\n".as_slice()),
        ("one-col", b"k\nalpha\nbeta\ngamma\n".as_slice()),
        ("header-only", b"x,y\n".as_slice()),
    ]
}

fn row_ref(t: &Table, r: &Key) -> String {
    let vals: Vec<&str> = t
        .live_cols()
        .iter()
        .map(|c| t.cell(r, c).unwrap_or(""))
        .collect();
    format!("row:{}", vals.join(","))
}

/// A grid's IDs: each live column's key by its header name, each live row's key by its
/// values (the corpus has no two alike).
fn grid_ids(s: &Saved<'_>) -> Vec<(String, String)> {
    let t = s.view.table().expect("a grid view");
    let mut out = Vec::new();
    for c in t.live_cols() {
        out.push((
            format!("col:{}", t.name(&c).unwrap_or("")),
            c.as_str().to_owned(),
        ));
    }
    for r in t.live_rows() {
        out.push((row_ref(&t, &r), r.as_str().to_owned()));
    }
    out
}

fn grid_units(_: &str, s: &Saved<'_>) -> Vec<Unit> {
    let t = s.view.table().expect("a grid view");
    let (rows, cols) = (t.live_rows(), t.live_cols());
    let vals: Vec<String> = (0..cols.len()).map(|i| format!("new{i}")).collect();
    let mut out = Vec::new();
    let at = rows
        .first()
        .map_or(Anchor::Start, |r| Anchor::After(r.clone()));
    let (_, ins) = t.clone().insert_row(at, &vals).unwrap();
    out.push(Unit::doc("row insert (verb)", &[], ins));
    if let (Some(r), Some(c)) = (rows.get(rows.len() / 2), cols.first()) {
        let touched = row_ref(&t, r);
        let set = t.clone().set_cell(r, c, "edited").unwrap();
        out.push(Unit::doc(
            format!("cell set {r},{c} (verb)"),
            &[touched.as_str()],
            set,
        ));
        let del = t.clone().delete_row(r).unwrap();
        out.push(Unit::doc(
            format!("row delete {r} (verb)"),
            &[touched.as_str()],
            del,
        ));
        let mut outside = t.clone();
        outside.set_cell(r, c, "outside").unwrap();
        out.push(Unit::file(
            format!("cell {r},{c} edited outside"),
            &[touched.as_str()],
            render(&outside),
        ));
    }
    let text = std::str::from_utf8(s.bytes).unwrap();
    let (head, rest) = text.split_once('\n').unwrap_or((text, ""));
    out.push(Unit::file(
        "row inserted outside",
        &[],
        format!("{head}\n{}\n{rest}", vals.join(",")),
    ));
    out
}

/// K-COD-10 on the grid test pack: zero churn over its corpus. Row and column keys hold
/// through a no-op save, a verb's cell set, row insert and row delete written back, and an
/// edited cell and an inserted row saved outside.
#[test]
fn churn_grid_pack() {
    let j =
        judge(&GridLite, &grid_corpus(), grid_ids, grid_units).unwrap_or_else(|r| panic!("{r}"));
    // three files with rows: no-op + 5 units; header-only: no-op + 2 units
    assert_eq!((j.files, j.steps), (4, 3 * 6 + 3));
    // fruit 5+5+4+4+4+5, wide 7+7+6+6+6+7, one-col 4+4+3+3+3+4, header-only 2+2+2
    assert_eq!(j.ids_checked, 27 + 39 + 21 + 6, "{j:?}");
}

/// A planted renumbering: a grid pack whose read-back re-keys its last live row (deleted and
/// inserted again with the same cells), so the file is the same and one untouched row's key
/// isn't. The judge catches it at every file's no-op save.
struct Rekeys;

impl Pack for Rekeys {
    fn schema(&self) -> Schema {
        GridLite.schema()
    }
    fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
        GridLite.decode(b)
    }
    fn encode(&self, c: &[(String, Neutral)], codec: &[u8]) -> Result<Vec<u8>, PackError> {
        GridLite.encode(c, codec)
    }
    fn reconcile(&self, synced: &View, bytes: &[u8], codec: &[u8]) -> Result<Reconciled, Declined> {
        let _ = codec;
        let mut r = default_reconcile(self, synced, bytes)?;
        let mut t = synced.table().unwrap();
        t.apply(&r.edits).unwrap();
        if let Some(last) = t.live_rows().last().cloned() {
            let vals: Vec<String> = t
                .live_cols()
                .iter()
                .map(|c| t.cell(&last, c).unwrap_or("").to_owned())
                .collect();
            let (_, ins) = t.insert_row(Anchor::After(last.clone()), &vals).unwrap();
            r.edits.extend(ins);
            r.edits.extend(t.delete_row(&last).unwrap());
        }
        Ok(r)
    }
}

fn renumbered(r: &ChurnReport, file: &str, step: &str, prefix: &str) -> bool {
    r.failures.iter().any(|c| {
        c.file == file
            && c.step == step
            && matches!(&c.kind, ChurnKind::Renumbered { object, .. } if object.starts_with(prefix))
    })
}

#[test]
fn churn_catches_a_planted_rekey() {
    let r = judge(&Rekeys, &grid_corpus(), grid_ids, grid_units).unwrap_err();
    for f in ["fruit", "wide", "one-col"] {
        assert!(renumbered(&r, f, "no-op save", "row:"), "{f}: {r}");
    }
    // the file itself is unchanged: only the key moved
    assert!(
        !r.failures.iter().any(|c| matches!(
            c.kind,
            ChurnKind::NotByteExact { .. } | ChurnKind::Failed(_)
        )),
        "{r}"
    );
    // header-only has no row to re-key
    assert!(!r.failures.iter().any(|c| c.file == "header-only"), "{r}");
    assert_eq!(r.judged.files, 4);
}

// ------------------------------------------------------------- the judge: IDs in the file

/// A text pack whose lines carry their own IDs (`7:text`), as a codec with IDs in its file
/// does (docx's `w14:paraId`). `renumber`: the planted bug, a save that renumbers the last
/// line's ID (as a producer "fixing" IDs on save would).
struct Tagged {
    renumber: bool,
}

const TAGGED: TextFilePack = TextFilePack::new("tagged", 1 << 20);

const TAGGED_IDS: &[IdKind] = &[IdKind {
    kind: "line-tag",
    example: "7:text",
    class: IdClass::FileLocal,
    scope: "one file's lines",
    accepted_by: &[],
    note: "the number before a line's first `:`; read, never addressed",
}];

impl Pack for Tagged {
    fn schema(&self) -> Schema {
        TAGGED.schema()
    }
    fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
        TAGGED.decode(b)
    }
    fn encode(&self, c: &[(String, Neutral)], codec: &[u8]) -> Result<Vec<u8>, PackError> {
        let b = TAGGED.encode(c, codec)?;
        Ok(if self.renumber { renumber_last(b) } else { b })
    }
    fn ids(&self) -> &'static [IdKind] {
        TAGGED_IDS
    }
}

fn renumber_last(b: Vec<u8>) -> Vec<u8> {
    let s = String::from_utf8(b).unwrap();
    let start = s
        .trim_end_matches(['\n', '\r'])
        .rfind('\n')
        .map_or(0, |i| i + 1);
    let Some(colon) = s[start..].find(':') else {
        return s.into_bytes();
    };
    let id: u64 = s[start..start + colon].parse().unwrap();
    format!("{}{}{}", &s[..start], id + 100, &s[start + colon..]).into_bytes()
}

fn tagged_ids(s: &Saved<'_>) -> Vec<(String, String)> {
    std::str::from_utf8(s.bytes)
        .unwrap()
        .lines()
        .filter_map(|l| l.split_once(':'))
        .map(|(id, text)| (text.to_owned(), id.to_owned()))
        .collect()
}

fn tagged_units(_: &str, s: &Saved<'_>) -> Vec<Unit> {
    let text = s.view.text(ROOT).unwrap().to_owned();
    let first = text.lines().next().unwrap().to_owned();
    let (_, first_text) = first.split_once(':').unwrap();
    let outside = String::from_utf8(s.bytes.to_vec()).unwrap().replacen(
        first_text,
        &first_text.to_uppercase(),
        1,
    );
    vec![
        Unit::doc(
            "append (verb)",
            &[],
            text_edit(s, &format!("{text}99:added\n")),
        ),
        Unit::file("first line edited outside", &[first_text], outside),
    ]
}

fn tagged_corpus() -> Vec<(&'static str, &'static [u8])> {
    vec![
        ("tags", b"1:alpha\n2:beta\n3:gamma\n".as_slice()),
        ("crlf", b"10:x\r\n20:y\r\n".as_slice()),
    ]
}

/// IDs in the file: an honest codec keeps every untouched line's ID.
#[test]
fn churn_file_ids() {
    let j = judge(
        &Tagged { renumber: false },
        &tagged_corpus(),
        tagged_ids,
        tagged_units,
    )
    .unwrap_or_else(|r| panic!("{r}"));
    assert_eq!((j.files, j.steps), (2, 6));
    // tags: 3 per step, 2 untouched after the outside edit; crlf: 2, 2, 1
    assert_eq!(j.ids_checked, 3 + 3 + 2 + 2 + 2 + 1);
}

/// The planted file renumbering: the no-op save renumbers an untouched line's ID, and is
/// caught (the outside save is the outside app's bytes, never the pack's).
#[test]
fn churn_catches_a_renumbered_file_id() {
    let r = judge(
        &Tagged { renumber: true },
        &tagged_corpus(),
        tagged_ids,
        tagged_units,
    )
    .unwrap_err();
    assert!(renumbered(&r, "tags", "no-op save", "gamma"), "{r}");
    assert!(renumbered(&r, "crlf", "no-op save", "y"), "{r}");
    // the append's renumbering lands on the line it added, which is free
    assert!(!r.failures.iter().any(|c| c.step == "append (verb)"), "{r}");
    assert!(
        r.failures
            .iter()
            .any(|c| c.step == "no-op save" && matches!(c.kind, ChurnKind::NotByteExact { .. })),
        "{r}"
    );
    assert!(!r
        .failures
        .iter()
        .any(|c| c.step == "first line edited outside"));
}

/// The judge refuses a test it can't judge: two objects named alike, a unit touching an
/// object the file doesn't have, a file the pack declines.
#[test]
fn churn_refuses_unclear_tests() {
    let corpus = grid_corpus();
    let r = judge(
        &GridLite,
        &corpus[..1],
        |_| vec![("x".into(), "1".into()), ("x".into(), "2".into())],
        |_, _| vec![Unit::doc("nothing", &["ghost"], Vec::new())],
    )
    .unwrap_err();
    let kinds: Vec<&ChurnKind> = r.failures.iter().map(|c| &c.kind).collect();
    assert!(
        kinds.contains(&&ChurnKind::Ambiguous { object: "x".into() }),
        "{r}"
    );
    assert!(
        kinds.contains(&&ChurnKind::UnknownTouch {
            object: "ghost".into()
        }),
        "{r}"
    );
    let r = judge(
        &GridLite,
        &[("bad", b"\xff".as_slice())],
        grid_ids,
        grid_units,
    )
    .unwrap_err();
    assert!(
        matches!(&r.failures[..], [c] if c.step == "import" && matches!(c.kind, ChurnKind::Failed(_))),
        "{r}"
    );
    // the grid schema's roots are what the judge imports into
    assert_eq!(GridLite.schema().roots, table::ROOTS);
}
