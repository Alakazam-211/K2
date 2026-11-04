//! The two line counts on a grid (docs/POSITIONS.md). The verbs that mirror a real tool, and
//! `--lines`, `--at`, `insert` and `sed` addresses, count `\n` (K-PAR-7). `LN`, `row list`,
//! blame, conflict reports and claims' reported lines count the line map, where a lone CR also
//! ends a line. These tests hold the seams between the two on the files where they differ:
//! a lone CR inside a quoted value, and a lone CR ending the records.
//!
//! In `quoted`: L1 is the header, L2 r1, L3 and L4 r2, L5 r3, while `cat -n` numbers four lines
//! (the third holds r2, the fourth r3). In `all_cr` the map has one line per record and
//! `cat -n` one line in all.
//!
//! Planted faults (`MPC_LINE_MUTANT`, the setting echoed): `ln-keeps-line-map` is caught by the
//! `ln_` refusal tests, `ids-count-nl-lines` by the two `ids_` tests, and
//! `touched-count-nl-lines` by the `for_edit`, view, notes and `claim_lines` tests, and
//! `refused-read-is-a-read` by the `a_refused_ln_read_` tests and `tests/refused_read.rs`,
//! `row-list-keeps-lines` and `conflict-keeps-lines` by the `no_line_numbers_` tests.

#[path = "grid_pack/quote_lite.rs"]
mod quote_lite;

use k2_mpc_base::elem::Key;
use k2_mpc_base::session::{
    ActivityKind, ClaimSpan, ConnId, EditOptions, ForEdit, GridWrite, GridWriteOptions, LineRange,
    NoteKind, Session, SessionError, UndoOutcome,
};
use k2_mpc_base::table::ROWS;
use k2_mpc_base::tools::{ToolError, ToolRunError};
use quote_lite::{argv, fx, run, Fx};

const QUOTED: &str = "id,note\nk1,Oslo\nk2,\"a\rb\"\nk3,Lima\n";
const QUOTED_CRLF: &str = "id,note\r\nk1,Oslo\r\nk2,\"a\rb\"\r\nk3,Lima\r\n";
const ALL_CR: &str = "id,note\rk1,Oslo\rk2,Rome\rk3,Lima\r";
const ALL_CR_NO_FINAL: &str = "id,note\rk1,Oslo\rk2,Rome\rk3,Lima";
const CR_QUOTED_LF: &str = "id,note\rk1,Oslo\rk2,\"two\nlines\"\rk3,Lima\r";

/// `MPC_LINE_MUTANT` runs `f` with a line fault planted on this thread
/// (`k2_mpc_base::differential::LineMutant`). Unset, `f` runs as is. Any other value panics.
fn under_line_mutant<R>(f: impl FnOnce() -> R) -> R {
    use k2_mpc_base::differential::{with_line_mutant, LineMutant};
    let m = std::env::var("MPC_LINE_MUTANT");
    println!("MPC_LINE_MUTANT={}", m.as_deref().unwrap_or("(unset)"));
    match m.as_deref() {
        Ok("ln-keeps-line-map") => with_line_mutant(LineMutant::LnKeepsLineMap, f),
        Ok("ids-count-nl-lines") => with_line_mutant(LineMutant::IdsCountNlLines, f),
        Ok("touched-count-nl-lines") => with_line_mutant(LineMutant::TouchedCountNlLines, f),
        Ok("refused-read-is-a-read") => with_line_mutant(LineMutant::RefusedReadIsARead, f),
        // the file always renders here, so these change nothing: tests/resync_failure.rs's
        Ok("ln-goes-on-unrendered") => with_line_mutant(LineMutant::LnGoesOnUnrendered, f),
        Ok("claim-lines-go-on-unrendered") => {
            with_line_mutant(LineMutant::ClaimLinesGoOnUnrendered, f)
        }
        Ok("conflict-lines-fall-back") => with_line_mutant(LineMutant::ConflictLinesFallBack, f),
        Ok("row-list-keeps-lines") => with_line_mutant(LineMutant::RowListKeepsLines, f),
        Ok("conflict-keeps-lines") => with_line_mutant(LineMutant::ConflictKeepsLines, f),
        Ok(other) => panic!("MPC_LINE_MUTANT={other}: not a mutant this suite plants"),
        Err(_) => f(),
    }
}

fn k(s: &str) -> Key {
    Key::new(s).unwrap()
}

fn rows(a: &str, b: &str) -> ClaimSpan {
    ClaimSpan::Rows {
        first: k(a),
        last: k(b),
    }
}

fn ok(f: &Fx, a: &str, stdin: &str) -> String {
    let (exit, out) = run(f, a, stdin);
    assert_eq!(exit, 0, "{a}: {out}");
    out
}

/// What `--for-edit` took, as `(span, lines)`.
fn for_edit(f: &Fx, a: &str) -> (ClaimSpan, LineRange) {
    let o = f.s.run_tool(f.p, ROWS, &argv(a), b"").unwrap();
    match o.for_edit.as_deref() {
        Some(ForEdit::Claimed(c)) => (c.span.clone(), c.lines.clone()),
        other => panic!("{a}: {other:?}"),
    }
}

fn lines(first: usize, last: usize) -> LineRange {
    LineRange {
        root: ROWS.into(),
        first,
        last,
    }
}

// ---------- reading with cat -n, then editing by --lines ----------

/// The number `cat -n` printed for the line holding `what`.
fn numbered(f: &Fx, what: &str) -> usize {
    let cat = ok(f, "cat -n", "");
    let line = cat
        .split('\n')
        .find(|l| l.contains(what))
        .unwrap_or_else(|| panic!("{what:?} not in {cat:?}"));
    line.split('\t').next().unwrap().trim().parse().unwrap()
}

#[test]
fn an_edit_by_line_hits_the_line_cat_n_printed() {
    under_line_mutant(|| {
        for (seed, eol) in [(QUOTED, "\n"), (QUOTED_CRLF, "\r\n")] {
            let file = |ls: &[&str]| ls.iter().map(|l| format!("{l}{eol}")).collect::<String>();
            let (h, a, b, c) = ("id,note", "k1,Oslo", "k2,\"a\rb\"", "k3,Lima");

            let f = fx(seed);
            assert_eq!(
                numbered(&f, "k3"),
                4,
                "{seed:?}: a lone CR ends no cat -n line"
            );
            ok(&f, "delete --lines 4", "");
            assert_eq!(f.file(), file(&[h, a, b]), "delete");

            let f = fx(seed);
            assert_eq!(numbered(&f, "k2"), 3);
            ok(&f, "delete --lines 3", "");
            assert_eq!(
                f.file(),
                file(&[h, a, c]),
                "delete the row with the lone CR"
            );

            let f = fx(seed);
            assert_eq!(
                ok(&f, "sed -n 4p", ""),
                file(&[c]),
                "sed's address is the same line"
            );
            ok(&f, "sed -i 4d", "");
            assert_eq!(f.file(), file(&[h, a, b]), "sed -i");

            let f = fx(seed);
            ok(&f, "insert --at 4:1", "k");
            assert_eq!(f.file(), file(&[h, a, b, "kk3,Lima"]), "--at");

            if eol == "\n" {
                let f = fx(seed);
                ok(&f, "replace --lines 4,4", "k9,Bari\n");
                assert_eq!(f.file(), file(&[h, a, b, "k9,Bari"]), "replace");

                let f = fx(seed);
                ok(&f, "insert --line 4", "k8,Kyiv\n");
                assert_eq!(f.file(), file(&[h, a, b, "k8,Kyiv", c]), "insert");
            }
        }
    })
}

#[test]
fn on_an_all_cr_file_line_1_is_the_whole_file() {
    under_line_mutant(|| {
        for seed in [ALL_CR, ALL_CR_NO_FINAL] {
            let f = fx(seed);
            assert_eq!(
                ok(&f, "cat -n", ""),
                format!("     1\t{seed}"),
                "the mirror verbs print one line"
            );
            assert_eq!(ok(&f, "sed -n 1p", "").trim_end_matches('\n'), seed);
            assert_eq!(
                ok(&f, "view --lines 1,1", ""),
                seed,
                "view prints the line as it is"
            );
            let (exit, out) = run(&f, "delete --lines 2", "");
            assert_eq!(exit, 2, "there is no second line: {out}");
            assert_eq!(f.file(), seed, "nothing applied");
        }
    })
}

/// `delete --lines 1` on an all-CR file addresses the whole file, and its report says how many
/// rows that was.
#[test]
fn delete_lines_1_on_an_all_cr_file_reports_the_rows_it_touched() {
    under_line_mutant(|| {
        let f = fx(ALL_CR);
        let o =
            f.s.run_tool(f.p, ROWS, &argv("delete --lines 1"), b"")
                .unwrap();
        println!("summary: {}", o.stderr_summary);
        assert_eq!(o.exit, 0);
        assert!(
            o.stderr_summary.contains("rows −3"),
            "the report names the rows: {:?}",
            o.stderr_summary
        );
        assert_eq!(f.file(), "", "the whole file");
    })
}

/// A `\n`-framed block can't go into a file whose records end in a lone CR: the result mixes
/// terminators, the pack declines it, and nothing applies. It never lands somewhere else.
#[test]
fn an_insert_by_line_on_an_all_cr_file_refuses_and_applies_nothing() {
    under_line_mutant(|| {
        for a in [
            "insert --line 1",
            "insert --line 2",
            "insert --after 1",
            "append",
        ] {
            let f = fx(ALL_CR);
            let e =
                f.s.run_tool(f.p, ROWS, &argv(a), b"x,y\n")
                    .map(|o| format!("{} {:?}", o.exit, o.stderr_summary))
                    .unwrap_err();
            println!("{a}: exit {}: {e}", e.exit());
            assert_eq!(e.exit(), 2, "{a}");
            let m = e.to_string();
            assert!(
                m.contains("not a valid table") && m.contains("mixed_line_endings"),
                "{a}: {m}"
            );
            assert!(m.contains("nothing applied"), "{a}: {m}");
            assert_eq!(f.file(), ALL_CR, "{a}");
        }
    })
}

const CRLF: &str = "id,note\r\nk1,Oslo\r\nk2,Rome\r\n";
const PURE_INSERT: &str = "--- a\n+++ b\n@@ -0,0 +1 @@\n+x,y\n";
const INSERT_WAY: &str =
    "row insert works, or insert --at with text ending in the file's own line ending";
const APPEND_WAY: &str =
    "row insert --end works, or append with text ending in the file's own line ending";
const ROW_WAY: &str = "cell set, row insert and row delete work on this file";

/// A refusal for mixed line endings is never a dead end: whichever edit verb reaches it, the
/// reason says what works. It still refuses and applies nothing.
fn refused_as_mixed_with_a_way_forward(seed: &str, a: &str, stdin: &str, way: &str) {
    under_line_mutant(|| {
        let f = fx(seed);
        let e =
            f.s.run_tool(f.p, ROWS, &argv(a), stdin.as_bytes())
                .map(|o| format!("exit {} {:?}", o.exit, o.stderr_summary))
                .expect_err_named(a);
        assert_eq!(f.file(), seed, "{a}: nothing applied");
        assert_eq!(
            (e.exit(), e.report().id()),
            (2, "MPC-E122"),
            "{a} on {seed:?}: {e}"
        );
        let m = e.to_string();
        assert!(
            m.contains("mixed_line_endings"),
            "{a} on {seed:?}: this case should reach the mixed refusal: {m}"
        );
        assert!(
            m.ends_with(&format!("nothing applied; {way}")),
            "{a} on {seed:?}: {m}"
        );
    })
}

/// One test per verb and file, so each is seen on its own.
macro_rules! mixed_refusals {
    ($($name:ident: $seed:expr, $a:expr, $stdin:expr, $way:expr;)*) => {
        $(
            #[test]
            fn $name() {
                refused_as_mixed_with_a_way_forward($seed, $a, $stdin, $way);
            }
        )*
    };
}

mixed_refusals! {
    mixed_crlf_insert_line_1: CRLF, "insert --line 1", "x,y\n", INSERT_WAY;
    mixed_crlf_insert_line_2: CRLF, "insert --line 2", "x,y\n", INSERT_WAY;
    mixed_crlf_insert_after_1: CRLF, "insert --after 1", "x,y\n", INSERT_WAY;
    mixed_crlf_insert_after_end: CRLF, "insert --after $", "x,y\n", INSERT_WAY;
    mixed_crlf_insert_at: CRLF, "insert --at 2:1", "x,y\n", INSERT_WAY;
    mixed_crlf_append: CRLF, "append", "x,y\n", APPEND_WAY;
    mixed_crlf_append_ensure: CRLF, "append --ensure-newline", "x,y\n", APPEND_WAY;
    mixed_crlf_replace_lines: CRLF, "replace --lines 2,2", "x,y\n", ROW_WAY;
    mixed_crlf_replace_range: CRLF, "replace --range 2:1-2:8", "x,y\n", ROW_WAY;
    mixed_crlf_replace_old: CRLF, "replace --old Oslo --new a\nb", "", ROW_WAY;
    mixed_crlf_delete_range: CRLF, "delete --range 1:9-2:1", "", ROW_WAY;
    mixed_crlf_sed_insert: CRLF, "sed -i 1ix,y", "", ROW_WAY;
    mixed_crlf_patch: CRLF, "patch", PURE_INSERT, ROW_WAY;
    mixed_all_cr_insert_line_1: ALL_CR, "insert --line 1", "x,y\n", INSERT_WAY;
    mixed_all_cr_insert_line_2: ALL_CR, "insert --line 2", "x,y\n", INSERT_WAY;
    mixed_all_cr_insert_after_1: ALL_CR, "insert --after 1", "x,y\n", INSERT_WAY;
    mixed_all_cr_insert_after_end: ALL_CR, "insert --after $", "x,y\n", INSERT_WAY;
    mixed_all_cr_insert_at: ALL_CR, "insert --at 1:1", "x,y\n", INSERT_WAY;
    mixed_all_cr_append: ALL_CR, "append", "x,y\n", APPEND_WAY;
    mixed_all_cr_append_ensure: ALL_CR, "append --ensure-newline", "x,y\n", APPEND_WAY;
    mixed_all_cr_replace_lines: ALL_CR, "replace --lines 1", "x,y\nz,w\r", ROW_WAY;
    mixed_all_cr_replace_range: ALL_CR, "replace --range 1:1-1:2", "x\n", ROW_WAY;
    mixed_all_cr_replace_old: ALL_CR, "replace --old Oslo --new a\nb", "", ROW_WAY;
    mixed_all_cr_sed_insert: ALL_CR, "sed -i 1ix,y", "", ROW_WAY;
    mixed_all_cr_patch: ALL_CR, "patch", PURE_INSERT, ROW_WAY;
    mixed_cr_quoted_lf_replace_lines: CR_QUOTED_LF, "replace --lines 1", "x\n", ROW_WAY;
    // deleting whole lines reaches it when the deleted line ends inside a quoted value
    mixed_line_ending_in_quotes_delete_lines:
        "id,note\nk1,\"a\nb\rc\"\n", "delete --lines 2", "", ROW_WAY;
}

/// What those reasons name really applies on these files, keeping every other terminator.
#[test]
fn the_verbs_a_mixed_refusal_names_apply() {
    under_line_mutant(|| {
        for (seed, eol) in [(CRLF, "\r\n"), (ALL_CR, "\r")] {
            let rows_of = |seed: &str| seed.split(eol).count() - 1;
            let n = rows_of(seed);

            let f = fx(seed);
            ok(&f, "row insert --end --values x,y", "");
            assert_eq!(f.file(), format!("{seed}x,y{eol}"), "row insert --end");

            let f = fx(seed);
            ok(&f, "row insert --before r1 --values x,y", "");
            assert_eq!(
                f.file(),
                seed.replacen("k1,Oslo", &format!("x,y{eol}k1,Oslo"), 1),
                "row insert"
            );

            let f = fx(seed);
            ok(&f, "insert --at 1:1", &format!("x,y{eol}"));
            assert_eq!(f.file(), format!("x,y{eol}{seed}"), "insert --at");

            let f = fx(seed);
            ok(&f, "append", &format!("x,y{eol}"));
            assert_eq!(f.file(), format!("{seed}x,y{eol}"), "append");

            let f = fx(seed);
            ok(&f, "cell set --row r1 --col c2 --value Bergen", "");
            assert_eq!(f.file(), seed.replacen("Oslo", "Bergen", 1), "cell set");

            let f = fx(seed);
            ok(&f, "row delete --row r1", "");
            assert_eq!(
                f.file(),
                seed.replacen(&format!("k1,Oslo{eol}"), "", 1),
                "row delete"
            );
            assert_eq!(rows_of(&f.file()), n - 1);
        }
    })
}

/// Another decline at the same place keeps its text, and an edit that doesn't mix still applies.
#[test]
fn other_results_of_those_verbs_are_unchanged() {
    under_line_mutant(|| {
        for seed in [CRLF, ALL_CR] {
            let f = fx(seed);
            let e =
                f.s.run_tool(f.p, ROWS, &argv("insert --line 1"), b"\"x\n")
                    .map(|o| format!("exit {} {:?}", o.exit, o.stderr_summary))
                    .expect_err_named("an unclosed quote");
            let m = e.to_string();
            assert!(m.contains("not a valid table"), "{seed:?}: {m}");
            assert!(!m.contains("mixed_line_endings"), "{seed:?}: {m}");
            assert!(m.ends_with("nothing applied"), "{seed:?}: {m}");
            assert_eq!(f.file(), seed);
        }
        // delete --lines on these files removes whole lines of one ending: nothing to mix
        let f = fx(CRLF);
        ok(&f, "delete --lines 2,2", "");
        assert_eq!(f.file(), "id,note\r\nk2,Rome\r\n");
        // deleting inside an all-CR file's one line can't mix either
        let f = fx(ALL_CR);
        ok(&f, "delete --range 1:1-1:2", "");
        assert_eq!(f.file(), "d,note\rk1,Oslo\rk2,Rome\rk3,Lima\r");
        // a patch whose context has no CR doesn't match, so it never reaches the pack
        let f = fx(CRLF);
        let o =
            f.s.run_tool(
                f.p,
                ROWS,
                &argv("patch"),
                b"--- a\n+++ b\n@@ -2 +2 @@\n-k1,Oslo\n+k1,Bergen\n",
            )
            .unwrap();
        assert_eq!(o.exit, 1, "{:?}", o.stderr_summary);
        assert!(
            o.stderr_summary.contains("MPC-E123"),
            "{:?}",
            o.stderr_summary
        );
        assert_eq!(f.file(), CRLF);
        // a sed substitution keeps every terminator
        let f = fx(CRLF);
        ok(&f, "sed -i s/Oslo/Bergen/", "");
        assert_eq!(f.file(), CRLF.replacen("Oslo", "Bergen", 1));
    })
}

#[test]
fn an_edit_on_a_cr_file_keeps_every_other_terminator() {
    under_line_mutant(|| {
        let f = fx(ALL_CR);
        ok(&f, "insert --at 1:1", "x,y\r");
        assert_eq!(
            f.file(),
            format!("x,y\r{ALL_CR}"),
            "--at with the file's own terminator"
        );

        let f = fx(ALL_CR);
        ok(&f, "row insert --end --values x,y", "");
        assert_eq!(f.file(), format!("{ALL_CR}x,y\r"), "row insert");

        let f = fx(ALL_CR);
        ok(&f, "sed -i s/Oslo/Bergen/", "");
        assert_eq!(f.file(), "id,note\rk1,Bergen\rk2,Rome\rk3,Lima\r", "sed -i");

        let f = fx(ALL_CR_NO_FINAL);
        ok(&f, "sed -i s/Lima/Quito/", "");
        assert_eq!(
            f.file(),
            "id,note\rk1,Oslo\rk2,Rome\rk3,Quito",
            "no final terminator"
        );

        let f = fx(QUOTED_CRLF);
        ok(&f, "sed -i s/Lima/Quito/", "");
        assert_eq!(
            f.file(),
            "id,note\r\nk1,Oslo\r\nk2,\"a\rb\"\r\nk3,Quito\r\n",
            "CRLF records and a lone CR in a value"
        );
    })
}

// ---------- LN ----------

/// Every verb that takes an `LN`, on a doc that has rows r1 to r3 and columns c1 and c2.
const LN_VERBS: &[(&str, &str)] = &[
    ("cell get --row L2 --col c1", ""),
    ("cell set --row L2 --col c1 --value v", ""),
    ("cells set --from L2,c1", "p,q\n"),
    ("cells clear --from L2,c1 --to #2,c2", ""),
    ("cells clear --from #1,c1 --to L2,c2", ""),
    ("row insert --before L2 --values a,b", ""),
    ("row insert --after L2 --values a,b", ""),
    ("row move --row L2 --end", ""),
    ("row move --row r1 --before L3", ""),
    ("row move --row r1 --after L3", ""),
    ("row delete --row L2", ""),
    ("row restore --row L2", ""),
];

#[test]
fn ln_refuses_on_a_grid_holding_a_lone_cr() {
    under_line_mutant(|| {
        for seed in [QUOTED, QUOTED_CRLF, ALL_CR, ALL_CR_NO_FINAL, CR_QUOTED_LF] {
            for (a, stdin) in LN_VERBS {
                let f = fx(seed);
                let v = f.s.status().unwrap().version;
                let e = f
                    .s
                    .run_tool(f.p, ROWS, &argv(a), stdin.as_bytes())
                    .map(|o| format!("exit {} {:?}", o.exit, String::from_utf8_lossy(&o.stdout)))
                    .expect_err_named(a);
                assert!(
                    matches!(e, ToolRunError::Tool(ToolError::Address(_))),
                    "{a} on {seed:?}: {e:?}"
                );
                let r = e.report();
                assert_eq!(
                    (r.id(), r.exit(), e.exit()),
                    ("MPC-E176", 2, 2),
                    "{a} on {seed:?}"
                );
                let m = e.to_string();
                assert!(
                    m.contains("key") && m.contains("#N") && m.contains("row list"),
                    "says what to do instead: {m}"
                );
                let step = r.entry().step.as_ref().expect("a next step");
                assert_eq!(step.verb, "row list");
                assert_eq!(f.s.status().unwrap().version, v, "{a}: nothing applied");
                assert_eq!(f.file(), *seed, "{a}: nothing applied");
            }
        }
    })
}

trait ExpectErrNamed<E> {
    fn expect_err_named(self, what: &str) -> E;
}

impl<E> ExpectErrNamed<E> for Result<String, E> {
    fn expect_err_named(self, what: &str) -> E {
        match self {
            Ok(got) => panic!("{what}: not refused, it ran: {got}"),
            Err(e) => e,
        }
    }
}

#[test]
fn keys_and_ordinals_still_work_on_those_files() {
    under_line_mutant(|| {
        for seed in [QUOTED, QUOTED_CRLF, ALL_CR, ALL_CR_NO_FINAL, CR_QUOTED_LF] {
            let f = fx(seed);
            assert_eq!(ok(&f, "cell get --row #3 --col c1", ""), "k3\n", "{seed:?}");
            assert_eq!(
                ok(&f, "cell get --row r1 --col c2", ""),
                "Oslo\n",
                "{seed:?}"
            );
            assert!(ok(&f, "row list", "").contains("r3\t#3\t"), "{seed:?}");
        }
    })
}

#[test]
fn ln_still_works_with_no_lone_cr() {
    under_line_mutant(|| {
        for (seed, l) in [
            (
                "id,note\nk1,Oslo\nk2,\"a\nb\"\nk3,Lima\n",
                ["L2", "L3", "L4", "L5"],
            ),
            (
                "id,note\r\nk1,Oslo\r\nk2,\"a\r\nb\"\r\nk3,Lima\r\n",
                ["L2", "L3", "L4", "L5"],
            ),
        ] {
            let f = fx(seed);
            let at = |i: usize| ok(&f, &format!("cell get --row {} --col c1", l[i]), "");
            assert_eq!(
                (at(0), at(1), at(2), at(3)),
                ("k1\n".into(), "k2\n".into(), "k2\n".into(), "k3\n".into()),
                "{seed:?}"
            );
            let (exit, out) = run(&f, "cell get --row L1 --col c1", "");
            assert_eq!(exit, 4, "the header: {out}");
        }
    })
}

/// The check reads the doc as it is now: a value that gains a lone CR turns `LN` off, and it
/// comes back when the value goes.
#[test]
fn ln_follows_the_doc_as_it_changes() {
    under_line_mutant(|| {
        let f = fx("id,note\nk1,Oslo\nk2,Rome\n");
        assert_eq!(ok(&f, "cell get --row L2 --col c1", ""), "k1\n");
        ok(&f, "cell set --row r1 --col c2 --value a\rb", "");
        assert_eq!(f.file(), "id,note\nk1,\"a\rb\"\nk2,Rome\n");
        let e =
            f.s.run_tool(f.p, ROWS, &argv("cell get --row L2 --col c1"), b"")
                .map(|_| String::new())
                .expect_err_named("LN after the value gained a lone CR");
        assert_eq!(e.report().id(), "MPC-E176");
        ok(&f, "cell set --row r1 --col c2 --value Oslo", "");
        assert_eq!(ok(&f, "cell get --row L2 --col c1", ""), "k1\n");
    })
}

/// `LN` under `--base` counts in the base version (K-ADDR-3), so the file that matters is the
/// base's.
#[test]
fn ln_under_base_asks_the_base_version() {
    under_line_mutant(|| {
        // no lone CR at the base, one now: it counts in the base, and works
        let f = fx("id,note\nk1,Oslo\nk2,Rome\n");
        let a = f.agent();
        let base = f.s.read(a).unwrap().version;
        ok(&f, "cell set --row r2 --col c2 --value a\rb", "");
        f.s.run_tool(
            a,
            ROWS,
            &argv(&format!(
                "cell set --row L2 --col c2 --value v --base {base}"
            )),
            b"",
        )
        .unwrap();
        assert_eq!(f.file(), "id,note\nk1,v\nk2,\"a\rb\"\n");

        // a lone CR at the base, none now: it counts in the base, and refuses
        let f = fx(QUOTED);
        let a = f.agent();
        let base = f.s.read(a).unwrap().version;
        ok(&f, "cell set --row r2 --col c2 --value plain", "");
        let e =
            f.s.run_tool(
                a,
                ROWS,
                &argv(&format!(
                    "cell set --row L2 --col c2 --value v --base {base}"
                )),
                b"",
            )
            .map(|_| String::new())
            .expect_err_named("LN at a base that held a lone CR");
        assert_eq!(e.report().id(), "MPC-E176");
    })
}

// ---------- --ids ----------

#[test]
fn ids_name_the_row_of_each_printed_line() {
    under_line_mutant(|| {
        let f = fx(QUOTED);
        assert_eq!(
            ok(&f, "cat --ids", ""),
            "header\tid,note\nr1\tk1,Oslo\nr2\tk2,\"a\rb\"\nr3\tk3,Lima\n"
        );
        assert_eq!(
        ok(&f, "cat -n --ids", ""),
        "header\t     1\tid,note\nr1\t     2\tk1,Oslo\nr2\t     3\tk2,\"a\rb\"\nr3\t     4\tk3,Lima\n"
    );
        assert_eq!(ok(&f, "view --lines 4,4 --ids", ""), "r3\tk3,Lima\n");
        assert_eq!(ok(&f, "tail -n 1 --ids", ""), "r3\tk3,Lima\n");
        assert_eq!(ok(&f, "grep --ids -n Lima", ""), "r3\t4:k3,Lima\n");
        assert_eq!(ok(&f, "grep --ids -o Lima", ""), "r3\tLima\n");
        assert_eq!(
            ok(&f, "grep --ids -B1 Lima", ""),
            "r2\tk2,\"a\rb\"\nr3\tk3,Lima\n"
        );

        let f = fx(QUOTED_CRLF);
        assert_eq!(
            ok(&f, "cat --ids", ""),
            "header\tid,note\r\nr1\tk1,Oslo\r\nr2\tk2,\"a\rb\"\r\nr3\tk3,Lima\r\n"
        );
    })
}

/// One `\n` line can hold several rows: the prefix names every one, in order.
#[test]
fn ids_name_every_row_a_line_holds() {
    under_line_mutant(|| {
        let f = fx(ALL_CR);
        assert_eq!(
            ok(&f, "cat --ids", ""),
            format!("header,r1,r2,r3\t{ALL_CR}")
        );
        assert_eq!(
            ok(&f, "view --lines 1,1 --ids", ""),
            format!("header,r1,r2,r3\t{ALL_CR}")
        );
        assert_eq!(
            ok(&f, "grep --ids -o Rome", ""),
            "header,r1,r2,r3\tRome\n",
            "the line's rows, not only the row the match is in"
        );

        let f = fx(ALL_CR_NO_FINAL);
        assert_eq!(
            ok(&f, "cat --ids", ""),
            format!("header,r1,r2,r3\t{ALL_CR_NO_FINAL}")
        );

        // the first line ends inside r2's quoted value, the second starts there
        let f = fx(CR_QUOTED_LF);
        assert_eq!(
            ok(&f, "cat --ids", ""),
            "header,r1,r2\tid,note\rk1,Oslo\rk2,\"two\nr2,r3\tlines\"\rk3,Lima\r"
        );
    })
}

// ---------- the lines a read looked at ----------

#[test]
fn for_edit_claims_the_rows_on_the_lines_it_read() {
    under_line_mutant(|| {
        let cases: &[(&str, &str, ClaimSpan, LineRange)] = &[
            (
                QUOTED,
                "view --lines 4,4 --for-edit",
                rows("r3", "r3"),
                lines(5, 5),
            ),
            (
                QUOTED,
                "view --lines 3,3 --for-edit",
                rows("r2", "r2"),
                lines(3, 4),
            ),
            (
                QUOTED,
                "view --lines 2,2 --for-edit",
                rows("r1", "r1"),
                lines(2, 2),
            ),
            (
                QUOTED,
                "tail -n 1 --for-edit",
                rows("r3", "r3"),
                lines(5, 5),
            ),
            (
                QUOTED,
                "grep -n Lima --for-edit",
                rows("r3", "r3"),
                lines(5, 5),
            ),
            (
                QUOTED,
                "sed -n 4p --for-edit",
                rows("r3", "r3"),
                lines(5, 5),
            ),
            (QUOTED, "cat --for-edit", rows("r1", "r3"), lines(2, 5)),
            (
                QUOTED_CRLF,
                "view --lines 4,4 --for-edit",
                rows("r3", "r3"),
                lines(5, 5),
            ),
            (
                ALL_CR,
                "view --lines 1,1 --for-edit",
                rows("r1", "r3"),
                lines(2, 4),
            ),
            (ALL_CR, "cat --for-edit", rows("r1", "r3"), lines(2, 4)),
            (
                ALL_CR_NO_FINAL,
                "cat --for-edit",
                rows("r1", "r3"),
                lines(2, 4),
            ),
            (
                CR_QUOTED_LF,
                "view --lines 1,1 --for-edit",
                rows("r1", "r2"),
                lines(2, 4),
            ),
            (
                CR_QUOTED_LF,
                "view --lines 2,2 --for-edit",
                rows("r2", "r3"),
                lines(3, 5),
            ),
        ];
        for (seed, a, span, l) in cases {
            let f = fx(seed);
            assert_eq!(
                for_edit(&f, a),
                (span.clone(), l.clone()),
                "{a} on {seed:?}"
            );
        }
    })
}

#[test]
fn a_view_shows_the_rows_on_the_lines_it_printed() {
    under_line_mutant(|| {
        let view_of = |f: &Fx| {
            f.s.roster(f.p)
                .unwrap()
                .into_iter()
                .find(|e| e.participant.pid == "p")
                .and_then(|e| e.view)
        };
        let f = fx(QUOTED);
        ok(&f, "view --lines 4,4", "");
        assert_eq!(view_of(&f), Some(lines(5, 5)), "r3's rendered line");
        let f = fx(ALL_CR);
        ok(&f, "view --lines 1,1", "");
        assert_eq!(view_of(&f), Some(lines(2, 4)), "every row");
    })
}

#[test]
fn heads_up_notes_are_about_the_rows_on_the_lines_read() {
    under_line_mutant(|| {
        let f = fx(QUOTED);
        let a = f.agent();
        f.s.claim_span(a, rows("r3", "r3"), None, None, false)
            .unwrap();
        let notes = |cmd: &str| {
            f.s.run_tool(f.p, ROWS, &argv(cmd), b"")
                .unwrap()
                .notes
                .lines()
        };
        assert_eq!(notes("view --lines 2,2"), Vec::<String>::new(), "r1");
        let n = notes("view --lines 4,4");
        assert!(
            n.iter().any(|l| l.contains('A') || l.contains("claim")),
            "r3 is claimed: {n:?}"
        );
    })
}

// ---------- claim --lines ----------

/// `claim --lines` names the lines `cat -n` printed; the claim reports its rendered lines.
#[test]
fn claim_lines_claims_the_line_cat_n_printed() {
    under_line_mutant(|| {
        let f = fx(QUOTED);
        let c = f.s.claim(f.p, lines(4, 4), None, None, false).unwrap();
        assert_eq!((c.span, c.lines), (rows("r3", "r3"), lines(5, 5)));

        let f = fx(QUOTED);
        let c = f.s.claim(f.p, lines(3, 3), None, None, false).unwrap();
        assert_eq!((c.span, c.lines), (rows("r2", "r2"), lines(3, 4)));

        let f = fx(ALL_CR);
        let c = f.s.claim(f.p, lines(1, 1), None, None, false).unwrap();
        assert_eq!((c.span, c.lines), (rows("r1", "r3"), lines(2, 4)));

        let f = fx(ALL_CR);
        let e = f.s.claim(f.p, lines(2, 2), None, None, false).unwrap_err();
        assert!(
            e.to_string().contains("hold no row"),
            "past the only line: {e}"
        );
    })
}

// ---------- line numbers an agent could give to --lines ----------

#[test]
fn no_line_numbers_from_row_list_on_a_lone_cr_file() {
    under_line_mutant(|| {
        for seed in [QUOTED, QUOTED_CRLF, ALL_CR, ALL_CR_NO_FINAL, CR_QUOTED_LF] {
            let f = fx(seed);
            let listing = ok(&f, "row list", "");
            for (i, line) in listing.lines().enumerate() {
                let cols: Vec<&str> = line.split('\t').collect();
                assert_eq!(
                    cols[..3],
                    [format!("r{}", i + 1), format!("#{}", i + 1), "-".to_owned()],
                    "{seed:?}: {line:?}"
                );
            }
            assert_eq!(listing.lines().count(), 3, "{seed:?}: {listing:?}");
        }
        // with no lone CR, a value's line breaks still count
        let f = fx("id,note\nk1,Oslo\nk2,\"a\nb\"\nk3,Lima\n");
        let listing = ok(&f, "row list", "");
        assert!(listing.contains("r2\t#2\tL3-4\t"), "{listing:?}");
        assert!(listing.contains("r3\t#3\tL5\t"), "{listing:?}");
    })
}

/// A write that conflicts at r3 of `seed`, with `body` changing the same cell: the report.
fn a_conflict_at_r3(seed: &str, body: &str) -> k2_mpc_base::session::GridConflict {
    let f = fx(seed);
    let a = f.agent();
    let base = f.s.read(a).unwrap().version;
    ok(&f, "cell set --row r3 --col c2 --value Quito", "");
    let r =
        f.s.write_base_grid(a, base, body.as_bytes(), &GridWriteOptions::default())
            .unwrap();
    let GridWrite::Conflicts(cs) = &r.outcome else {
        panic!("{r:?}")
    };
    assert_eq!(cs.len(), 1, "{cs:?}");
    cs[0].clone()
}

#[test]
fn no_line_numbers_from_a_conflict_on_a_lone_cr_file() {
    under_line_mutant(|| {
        let c = a_conflict_at_r3(QUOTED, "id,note\nk1,Oslo\nk2,\"a\rb\"\nk3,Bari\n");
        assert_eq!(c.lines, None, "no rendered line");
        let shown = c.to_string();
        assert!(shown.contains("row r3"), "still names the row: {shown}");
        assert!(!shown.contains("line "), "{shown}");

        // with no lone CR, it names the line
        let c = a_conflict_at_r3(
            "id,note\nk1,Oslo\nk2,\"a\nb\"\nk3,Lima\n",
            "id,note\nk1,Oslo\nk2,\"a\nb\"\nk3,Bari\n",
        );
        assert_eq!(c.lines, Some(5..6), "the conflict's rendered lines");
    })
}

/// A revert that conflicts reports through `ConflictRegion`, with `(0, 0)` for no line.
#[test]
fn no_line_numbers_from_a_conflict_region_on_a_lone_cr_file() {
    under_line_mutant(|| {
        let region = |seed: &str| {
            let f = fx(seed);
            let q = f.agent();
            let change =
                f.s.edit_ops_with(f.p, &EditOptions::new(), |v| {
                    let mut t = v.table().map_err(|e| e.to_string())?;
                    t.set_cell(&k("r3"), &k("c2"), "Quito")
                        .map_err(|e| e.to_string())
                })
                .unwrap()
                .unwrap()
                .version;
            let base = f.s.read(q).unwrap().version;
            let r = f.s.edit_ops_with(q, &EditOptions::new().base(base), |v| {
                let mut t = v.table().map_err(|e| e.to_string())?;
                t.set_cell(&k("r3"), &k("c2"), "Bari")
                    .map_err(|e| e.to_string())
            });
            assert!(r.is_ok(), "{r:?}");
            let o = f.s.revert(f.p, change, false).unwrap();
            let UndoOutcome::Conflicts { conflicts, .. } = &o else {
                panic!("{o:?}")
            };
            assert_eq!(conflicts.len(), 1, "{conflicts:?}");
            conflicts[0].lines
        };
        assert_eq!(region(QUOTED), (0, 0), "no rendered line");
        assert_eq!(region("id,note\nk1,Oslo\nk2,x\nk3,Lima\n"), (4, 4));
    })
}

/// An outside save that overwrites a live value tells its author, with the row's key.
#[test]
fn no_line_numbers_from_an_overwrite_note_on_a_lone_cr_file() {
    under_line_mutant(|| {
        let line_of = |seed: &str, saved: &str| {
            let (f, path) = quote_lite::fx_file(seed);
            ok(&f, "cell set --row r3 --col c2 --value LIVE", "");
            std::fs::write(&path, saved).unwrap();
            f.s.file_changed().unwrap();
            let n = f.s.take_notes(f.p).unwrap();
            let o: Vec<_> = n
                .notes
                .iter()
                .filter(|x| x.kind == NoteKind::Overwritten)
                .map(|x| x.overwrite.clone().expect("an Overwritten note says what"))
                .collect();
            assert_eq!(o.len(), 1, "{n:?}");
            assert_eq!(o[0].region, "r3/c2");
            o[0].line
        };
        assert_eq!(
            line_of(QUOTED, "id,note\nk1,Oslo\nk2,\"a\rb\"\nk3,FILE\n"),
            None,
            "no rendered line"
        );
        assert_eq!(
            line_of(
                "id,note\nk1,Oslo\nk2,x\nk3,Lima\n",
                "id,note\nk1,Oslo\nk2,x\nk3,FILE\n"
            ),
            Some(4)
        );
    })
}

/// Blame stays numbered, by the line map, on every file: its numbers are for reading.
#[test]
fn blame_keeps_the_line_map() {
    under_line_mutant(|| {
        let f = fx(QUOTED);
        let b = f.s.blame(f.p, ROWS, None).unwrap();
        assert_eq!(b.lines.len(), 5, "{b:?}");
        assert_eq!(b.lines[4].text, "k3,Lima");
        let f = fx(ALL_CR);
        assert_eq!(f.s.blame(f.p, ROWS, None).unwrap().lines.len(), 4);
    })
}

// ---------- a refusal leaves nothing ----------

/// A keyed write as of `old`, on a cell nobody has written since: a change that marked its rows
/// or columns refuses this one.
fn stale_write(s: &Session, bot: ConnId, old: u64) -> Result<(), SessionError> {
    let r = s.edit_ops_with(bot, &EditOptions::new().base(old), |v| {
        let mut t = v.table().map_err(|e| e.to_string())?;
        t.set_cell(&k("r1"), &k("c1"), "BOT")
            .map_err(|e| e.to_string())
    });
    match r {
        Ok(_) => Ok(()),
        Err(k2_mpc_base::session::EditWithError::Session(e)) => Err(e),
        Err(k2_mpc_base::session::EditWithError::Compute(e)) => panic!("the write: {e}"),
    }
}

/// A body whose columns map onto `CRLF`'s by content, not exactly. With `eol` as the last record's
/// ending: `\r\n` lands, and `\n` mixes endings.
fn remap_body(eol: &str) -> Vec<u8> {
    format!("note,id,N\r\nOslo,k1,z\r\nRome,k2,z{eol}").into_bytes()
}

/// With nothing refused, the next test's next changes land. A write that remaps the columns and
/// lands does mark them, so the next test would see a mark left behind.
#[test]
fn a_remap_that_lands_marks_the_columns() {
    under_line_mutant(|| {
        let f = fx(CRLF);
        let bot = f.agent();
        let old = f.s.read(bot).unwrap().version;
        let r =
            f.s.write_live_grid(bot, old, &remap_body("\r\n"), &GridWriteOptions::default())
                .unwrap();
        assert!(matches!(r.outcome, GridWrite::Applied(_)), "{r:?}");
        let r = stale_write(&f.s, bot, old);
        assert!(
            matches!(
                &r,
                Err(SessionError::FacetConflict(c))
                    if c.iter().any(|c| c.rewritten_row)
            ),
            "{r:?}"
        );
    })
}

/// A refused `LN` or a refused mixed-endings edit leaves nothing: no change, no log entry, no
/// mark or label on the next change, whoever makes it.
#[test]
fn a_refused_edit_leaves_nothing_for_the_next_change() {
    under_line_mutant(|| {
        let tools: &[(&str, &str, &str)] = &[
            (QUOTED, "cell set --row L2 --col c1 --value v", ""),
            (QUOTED, "row delete --row L2", ""),
            (ALL_CR, "cell set --row L2 --col c1 --value v", ""),
            (ALL_CR, "row insert --before L2 --values a,b", ""),
            (ALL_CR, "insert --line 1", "x,y\n"),
            (ALL_CR, "append", "x,y\n"),
            (CRLF, "insert --after 1", "x,y\n"),
            (CRLF, "replace --lines 2,2", "x,y\n"),
            (CRLF, "patch", PURE_INSERT),
        ];
        let check = |f: &quote_lite::Fx, seed: &str, what: &str, bot: ConnId, old: u64| {
            assert_eq!(f.s.status().unwrap().version, old, "{what}: no change");
            assert_eq!(f.file(), seed, "{what}: the file");
            assert_eq!(f.s.claims(f.p).unwrap(), Vec::new(), "{what}: no claim");
            let entries = f.s.status().unwrap().log_entries;
            // the next change has no label of its own, so a refused verb's would show
            let r = f.s.edit_ops_with(f.p, &EditOptions::new(), |v| {
                let mut t = v.table().map_err(|e| e.to_string())?;
                t.set_cell(&k("r2"), &k("c2"), "P")
                    .map_err(|e| e.to_string())
            });
            assert!(r.is_ok(), "{what}: {r:?}");
            assert_eq!(f.s.status().unwrap().log_entries, entries + 1, "{what}");
            let edits: Vec<String> =
                f.s.activity(f.p, 50)
                    .unwrap()
                    .into_iter()
                    .filter(|a| a.kind == ActivityKind::Edit)
                    .map(|a| a.verb)
                    .collect();
            assert_eq!(edits, ["edit"], "{what}: its label reached the next change");
            let r = stale_write(&f.s, bot, old);
            assert!(r.is_ok(), "{what}: the next change was marked: {r:?}");
        };
        for (seed, a, stdin) in tools {
            let f = fx(seed);
            let bot = f.agent();
            let old = f.s.read(bot).unwrap().version;
            let e = f.s.run_tool(f.p, ROWS, &argv(a), stdin.as_bytes());
            assert!(e.is_err(), "{a} on {seed:?}: not refused: {e:?}");
            check(&f, seed, a, bot, old);
        }
        // the same body that lands above, with its last record ending otherwise: declined before
        // it could mark anything
        let f = fx(CRLF);
        let bot = f.agent();
        let old = f.s.read(bot).unwrap().version;
        let r =
            f.s.write_live_grid(bot, old, &remap_body("\n"), &GridWriteOptions::default());
        let e = r.expect_err("the mixed body is refused");
        assert!(e.to_string().contains("mixed_line_endings"), "{e}");
        check(&f, CRLF, "write", bot, old);
    })
}

/// A refused `LN` read gave the caller nothing, so it isn't their read: the next read still says
/// who changed the doc since they last read it.
#[test]
fn a_refused_ln_read_keeps_who_changed_the_doc() {
    under_line_mutant(|| {
        let f = fx(QUOTED);
        let bot = f.agent();
        let read = |a: &str| f.s.run_tool(bot, ROWS, &argv(a), b"");
        read("cat").unwrap();
        ok(&f, "cell set --row r1 --col c2 --value P", "");
        read("cell get --row L2 --col c1").unwrap_err();
        let fence = read("cat").unwrap().fence.unwrap();
        assert_eq!(
            fence.contributors,
            ["p"],
            "who changed it since the last real read"
        );
    })
}

/// Nor is it activity others see.
#[test]
fn a_refused_ln_read_is_no_activity() {
    under_line_mutant(|| {
        let f = fx(QUOTED);
        let bot = f.agent();
        let before = f.s.activity(bot, 50).unwrap().len();
        f.s.run_tool(bot, ROWS, &argv("cell get --row L2 --col c1"), b"")
            .unwrap_err();
        assert_eq!(f.s.activity(bot, 50).unwrap().len(), before);
    })
}

/// A change that fails after the doc took it reloads the doc. That can't happen inside a read:
/// only commits, outside saves, write-backs and purges reload. So this does both around one
/// refused read: the reload doesn't bring back what the refused read didn't move, and the refused
/// read doesn't count what the reload dropped.
#[test]
fn a_refused_read_after_a_reload_moves_nothing() {
    under_line_mutant(|| {
        use k2_mpc_base::differential::{with_apply_fault, ApplyFault};
        use k2_mpc_base::session::{Participant, ParticipantKind, Stay};
        for refused in ["sed s//x/", "cell get --row L2 --col c1"] {
            let f = fx(QUOTED);
            let reader = f.agent();
            let other =
                f.s.join(
                    Participant::new("q", "Q", "#3", ParticipantKind::Person, "tab"),
                    Stay::Held,
                )
                .unwrap()
                .conn;
            let read = |a: &str| f.s.run_tool(reader, ROWS, &argv(a), b"");
            read("cat").unwrap();
            ok(&f, "cell set --row r1 --col c2 --value P", "");
            f.s.claim_span(f.p, rows("r3", "r3"), None, None, false)
                .unwrap();
            let landed = f.file();
            // q's change fails after the doc took it: the doc reloads, and every claim ends
            let e = with_apply_fault(ApplyFault::BookkeepingFailsAfter(0), || {
                f.s.edit_ops_with(other, &EditOptions::new(), |v| {
                    let mut t = v.table().map_err(|e| e.to_string())?;
                    t.set_cell(&k("r2"), &k("c2"), "Q")
                        .map_err(|e| e.to_string())
                })
            });
            assert!(e.is_err(), "{refused}: {e:?}");
            assert_eq!(f.file(), landed, "{refused}: as the store has it");
            assert_eq!(f.s.claims(f.p).unwrap(), Vec::new(), "{refused}: reloaded");

            let before = f.s.activity(reader, 50).unwrap().len();
            read(refused).unwrap_err();
            assert_eq!(
                f.s.activity(reader, 50).unwrap().len(),
                before,
                "{refused}: no activity"
            );
            let fence = read("cat").unwrap().fence.unwrap();
            assert_eq!(
                fence.contributors,
                ["p"],
                "{refused}: who landed a change, once, and not who failed to"
            );
            assert!(
                read("cat").unwrap().fence.unwrap().contributors.is_empty(),
                "{refused}: that read counted"
            );
        }
    })
}

/// **This pins a known limit, not a wanted result.** Under `--base`, whether a file holds a lone CR
/// is asked of the base's content rendered with the live codec, because a read base keeps no
/// codec. So a file that was all-CR at the base and whose terminators a later save changed reads
/// as holding none, and `LN` runs: though `cat -n` at that base printed one line, `L2` here names
/// r1. `docs/POSITIONS.md` states it. If this starts refusing, the limit is gone: delete this test
/// and say so there.
#[test]
fn known_limit_ln_under_base_asks_with_the_live_codec() {
    under_line_mutant(|| {
        let (f, path) = quote_lite::fx_file(ALL_CR);
        let a = f.agent();
        let base = f.s.read(a).unwrap().version;
        std::fs::write(&path, "id,note\nk1,Oslo\nk2,Rome\nk3,Quito\n").unwrap();
        f.s.file_changed().unwrap();
        assert!(f.s.status().unwrap().version > base, "a later version");
        assert_eq!(f.file(), "id,note\nk1,Oslo\nk2,Rome\nk3,Quito\n");
        let o =
            f.s.run_tool(
                a,
                ROWS,
                &argv(&format!(
                    "cell set --row L2 --col c2 --value v --base {base}"
                )),
                b"",
            )
            .unwrap();
        assert_eq!(o.exit, 0, "the limit: LN runs: {o:?}");
        assert_eq!(f.file(), "id,note\nk1,v\nk2,Rome\nk3,Quito\n");
    })
}

/// A refused edit marks no claim, so the next change, whoever makes it, doesn't end its owner's.
#[test]
fn a_refused_edit_leaves_its_owners_claim() {
    under_line_mutant(|| {
        for (seed, a, stdin) in [
            (QUOTED, "cell set --row L3 --col c1 --value v", ""),
            (CRLF, "replace --lines 2,2", "x,y\n"),
            (ALL_CR, "insert --line 1", "x,y\n"),
        ] {
            let f = fx(seed);
            f.s.claim_span(f.p, rows("r1", "r1"), None, None, false)
                .unwrap();
            let e = f.s.run_tool(f.p, ROWS, &argv(a), stdin.as_bytes());
            assert!(e.is_err(), "{a} on {seed:?}: not refused: {e:?}");
            // the next change has no label and names another row
            let r = f.s.edit_ops_with(f.p, &EditOptions::new(), |v| {
                let mut t = v.table().map_err(|e| e.to_string())?;
                t.set_cell(&k("r2"), &k("c2"), "P")
                    .map_err(|e| e.to_string())
            });
            assert!(r.is_ok(), "{a}: {r:?}");
            let claims = f.s.claims(f.p).unwrap();
            assert_eq!(claims.len(), 1, "{a} on {seed:?}: {claims:?}");
            assert_eq!(claims[0].span, rows("r1", "r1"));
        }
    })
}
