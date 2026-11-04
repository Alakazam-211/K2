//! The grid line tools' output, pinned against the kit as it was before the one-definition-of-
//! "line" slice. `tests/golden/grid_lines.golden` was blessed from this test with the kit's
//! source at 3263a3e0 (the job is named in the commit that added it), so it holds what those
//! tools printed then, not what today's code computes.
//!
//! - Files with no lone CR: every line tool, grid tool and edit verb, byte for byte.
//! - Files with a lone CR (in a value, or ending the records): the verbs that mirror a real
//!   tool, the `\n` edit verbs, and the grid tools that keep the line map. `--ids`, `LN`,
//!   `--for-edit` and views change on those files, so `tests/grid_lines.rs` covers them.
//!
//! `MPC_BLESS=1` rewrites the golden; only the kit's own file changes, and a diff to a case
//! here is a change of behaviour on valid files.

#[path = "grid_pack/mod.rs"]
mod grid_pack;
#[path = "grid_pack/quote_lite.rs"]
mod quote_lite;

use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use k2_mpc_base::session::{LineRange, Pack};
use k2_mpc_base::table::ROWS;
use quote_lite::{argv, fx_pack, Fx, QuoteLite};

const PATCH: &str = "--- a\n+++ b\n@@ -2 +2 @@\n-k1,Oslo\n+k1,Bergen\n";

/// Read verbs, each on a fresh doc: `(argv, stdin)`.
const READS: &[(&str, &str)] = &[
    ("cat", ""),
    ("cat -n", ""),
    ("cat -b", ""),
    ("head -n 2", ""),
    ("head -n -1", ""),
    ("head -c 5", ""),
    ("head -c -2", ""),
    ("tail -n 2", ""),
    ("tail -n +2", ""),
    ("tail -c 4", ""),
    ("wc", ""),
    ("wc -l", ""),
    ("wc -w", ""),
    ("wc -c", ""),
    ("wc -m", ""),
    ("grep -n o", ""),
    ("grep -c o", ""),
    ("grep -v o", ""),
    ("grep -o o", ""),
    ("grep -n -A1 k1", ""),
    ("grep -n -B1 k3", ""),
    ("grep -l o", ""),
    ("grep -q o", ""),
    ("grep -n ^k", ""),
    ("grep -n a$", ""),
    ("sed -n 2,3p", ""),
    ("sed -n 2p", ""),
    ("sed -n $p", ""),
    ("sed -n =", ""),
    ("sed s/o/0/", ""),
    ("sed 2d", ""),
    ("diff", "id,note\nk0,x\n"),
    ("diff -U 1", "id,note\nk1,Oslo\n"),
    ("view --lines 2,3", ""),
    ("view --lines 1,$", ""),
    ("view --lines 9,9", ""),
];

/// Edit verbs that count `\n`, each on a fresh doc.
const EDITS: &[(&str, &str)] = &[
    ("insert --line 2", "x,y\n"),
    ("insert --line 1", "x,y\n"),
    ("insert --after 1", "x,y\n"),
    ("insert --after $", "x,y\n"),
    ("insert --at 2:1", "z"),
    ("replace --lines 2,2", "x,y\n"),
    ("replace --lines 1,$", "x,y\n"),
    ("replace --range 2:1-2:3", "zz"),
    ("replace --old Oslo --new Bergen", ""),
    ("delete --lines 2,2", ""),
    ("delete --lines 2,3", ""),
    ("delete --lines 1", ""),
    ("delete --lines 1,$", ""),
    ("delete --range 2:1-2:2", ""),
    ("sed -i s/k/K/", ""),
    ("sed -i s/o/0/g", ""),
    ("sed -i 2d", ""),
    ("sed -i $d", ""),
    ("sed -i 1d", ""),
    ("patch", PATCH),
    ("patch --dry-run", PATCH),
    ("append", "x,y\n"),
    ("append --ensure-newline", "x,y\n"),
];

/// Grid tools that keep the line map on every file, and addresses by key and `#N`.
const GRID: &[(&str, &str)] = &[
    ("row list", ""),
    ("col list", ""),
    ("@blame", ""),
    ("cell get --row r2 --col c2", ""),
    ("cell get --row #2 --col c2", ""),
    ("cell set --row #1 --col c2 --value v", ""),
    ("cells clear --from #1,c1 --to #2,c2", ""),
    ("row insert --before #2 --values a,b", ""),
    ("row insert --after r1 --values a,b", ""),
    ("row move --row #1 --end", ""),
    ("row delete --row r1", ""),
    ("sort --by c2", ""),
    ("@set_view 2 3", ""),
];

/// What changes on a file with a lone CR, pinned here on the files with none.
const NO_CR_ONLY: &[(&str, &str)] = &[
    ("cat --ids", ""),
    ("cat -n --ids", ""),
    ("cat -b --ids", ""),
    ("head -n 2 --ids", ""),
    ("tail -n 1 --ids", ""),
    ("view --lines 2,3 --ids", ""),
    ("grep --ids -n o", ""),
    ("grep --ids -o o", ""),
    ("grep --ids -v o", ""),
    ("grep --ids -A1 k1", ""),
    ("head -c 2 --ids", ""),
    ("cat --for-edit", ""),
    ("head -n 2 --for-edit", ""),
    ("tail -n 1 --for-edit", ""),
    ("view --lines 2,2 --for-edit", ""),
    ("view --lines 3,4 --for-edit", ""),
    ("view --lines 1,1 --for-edit", ""),
    ("grep -n k1 --for-edit", ""),
    ("sed -n 2p --for-edit", ""),
    ("cell get --row L2 --col c2", ""),
    ("cell get --row L3 --col c1", ""),
    ("cell get --row L4 --col c1", ""),
    ("cell get --row L1 --col c1", ""),
    ("cell get --row L99 --col c1", ""),
    ("cell set --row L2 --col c2 --value v", ""),
    ("cells set --from L2,c1", "p,q\n"),
    ("cells clear --from L2,c1 --to L3,c2", ""),
    ("row insert --before L2 --values a,b", ""),
    ("row insert --after L2 --values a,b", ""),
    ("row insert --at L2 --values a,b", ""),
    ("row move --row L2 --end", ""),
    ("row move --row #1 --before L3", ""),
    ("row delete --row L2", ""),
    ("row restore --row L2", ""),
    ("@claim 2 2", ""),
    ("@claim 3 4", ""),
    ("@claim 1 1", ""),
    ("@view 2 3", ""),
    ("@view 3 4", ""),
    ("@agent cell set --row L2 --col c2 --value v", ""),
    ("@agent cell set --row #1 --col c2 --value v", ""),
    ("@agent delete --lines 2,2", ""),
    ("@agent cell set --row L2 --col c2 --value v --base 1", ""),
    ("@agent row delete --row L3 --base 1", ""),
    ("@agent cell get --row L2 --col c2", ""),
];

fn quote() -> Arc<dyn Pack> {
    Arc::new(QuoteLite)
}

fn grid_lite() -> Arc<dyn Pack> {
    Arc::new(grid_pack::GridLite)
}

/// One case's whole output, one field a line (strings as Rust debug strings, so no raw CR).
fn case(out: &mut String, name: &str, f: &Fx, a: &str, stdin: &str) {
    let _ = writeln!(out, "=== {name} | {a} | {stdin:?}");
    let conn = match a.strip_prefix("@agent ") {
        Some(_) => f.agent(),
        None => f.p,
    };
    let a = a.strip_prefix("@agent ").unwrap_or(a);
    let num = |s: &str| s.parse::<usize>().unwrap();
    let range = |x: &str, y: &str| LineRange {
        root: ROWS.into(),
        first: num(x),
        last: num(y),
    };
    match a.split(' ').collect::<Vec<_>>().as_slice() {
        ["@blame"] => {
            let _ = writeln!(out, "blame {:?}", f.s.blame(f.p, ROWS, None));
        }
        ["@claim", x, y] => {
            let c = f.s.claim(f.p, range(x, y), None, None, false);
            let _ = writeln!(
                out,
                "claim {:?}",
                c.map(|c| (c.span, c.lines)).map_err(|e| e.to_string())
            );
        }
        ["@set_view", x, y] => {
            let v = f.s.set_view(f.p, range(x, y));
            let _ = writeln!(out, "set_view {:?}", v.map_err(|e| e.to_string()));
            view_of(out, f);
        }
        ["@view", x, y] => {
            run(out, f, conn, &format!("view --lines {x},{y}"), "");
            view_of(out, f);
        }
        _ => run(out, f, conn, a, stdin),
    }
    let _ = writeln!(out, "file {:?}", f.file());
}

fn view_of(out: &mut String, f: &Fx) {
    let v: Vec<_> =
        f.s.roster(f.p)
            .unwrap()
            .into_iter()
            .map(|e| (e.participant.pid, e.view))
            .collect();
    let _ = writeln!(out, "roster {v:?}");
}

fn run(out: &mut String, f: &Fx, conn: k2_mpc_base::session::ConnId, a: &str, stdin: &str) {
    match f.s.run_tool(conn, ROWS, &argv(a), stdin.as_bytes()) {
        Ok(o) => {
            let _ = writeln!(out, "exit {}", o.exit);
            let _ = writeln!(out, "stdout {:?}", String::from_utf8_lossy(&o.stdout));
            let _ = writeln!(out, "summary {:?}", o.stderr_summary);
            let _ = writeln!(out, "notes {:?}", o.notes.lines());
            let _ = writeln!(out, "warnings {:?}", o.warnings.lines());
            let claim = o.for_edit.as_deref().map(|e| match e {
                k2_mpc_base::session::ForEdit::Claimed(c) => {
                    format!("claimed {:?} {:?}", c.span, c.lines)
                }
                other => format!("{other:?}"),
            });
            let _ = writeln!(out, "for_edit {claim:?}");
            let _ = writeln!(out, "refusal {:?}", o.refusal.map(|r| r.id().to_owned()));
        }
        Err(e) => {
            let _ = writeln!(out, "error {} {:?}", e.exit(), e.to_string());
            let _ = writeln!(out, "id {:?}", e.report().id());
        }
    }
}

fn all_cases() -> String {
    type P = fn() -> Arc<dyn Pack>;
    let no_cr: &[(&str, P, &str)] = &[
        (
            "lf",
            quote,
            "id,note\nk1,Oslo\nk2,\"two\nlines\"\nk3,\"a,b\"\n",
        ),
        ("lf-no-final", quote, "id,note\nk1,Oslo\nk2,Rome"),
        ("lf-blank", quote, "id,note\nk1,Oslo\n\nk3,Lima\n"),
        (
            "crlf",
            quote,
            "id,note\r\nk1,Oslo\r\nk2,\"two\r\nlines\"\r\nk3,\"a,b\"\r\n",
        ),
        (
            "crlf-bare-lf",
            quote,
            "id,note\r\nk1,Oslo\r\nk2,\"two\nlines\"\r\n",
        ),
        (
            "grid-lite",
            grid_lite,
            "id,note\nk1,Oslo\nk2,Rome\nk3,Lima\n",
        ),
    ];
    let cr: &[(&str, P, &str)] = &[
        ("all-cr", quote, "id,note\rk1,Oslo\rk2,Rome\rk3,Lima\r"),
        ("all-cr-no-final", quote, "id,note\rk1,Oslo\rk2,Rome"),
        (
            "all-cr-quoted-lf",
            quote,
            "id,note\rk1,Oslo\rk2,\"two\nlines\"\rk3,Lima\r",
        ),
        (
            "lf-quoted-cr",
            quote,
            "id,note\nk1,Oslo\nk2,\"a\rb\"\nk3,Lima\n",
        ),
        (
            "crlf-quoted-cr",
            quote,
            "id,note\r\nk1,Oslo\r\nk2,\"a\rb\"\r\nk3,Lima\r\n",
        ),
    ];
    let mut out = String::new();
    let shared = || READS.iter().chain(EDITS).chain(GRID);
    for (name, pack, seed) in no_cr {
        for (a, stdin) in shared().chain(NO_CR_ONLY) {
            case(&mut out, name, &fx_pack(pack(), seed), a, stdin);
        }
    }
    for (name, pack, seed) in cr {
        for (a, stdin) in shared() {
            case(&mut out, name, &fx_pack(pack(), seed), a, stdin);
        }
    }
    out
}

#[test]
fn grid_line_tools_print_what_they_printed_before_the_slice() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/grid_lines.golden");
    let got = all_cases();
    if std::env::var_os("MPC_BLESS").is_some() {
        std::fs::write(&path, &got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("{} is missing (MPC_BLESS=1 writes it)", path.display()));
    if got == want {
        return;
    }
    let mut header = "";
    for (g, w) in got.lines().zip(want.lines()) {
        if w.starts_with("=== ") {
            header = w;
        }
        assert_eq!(g, w, "first difference, in case: {header}");
    }
    panic!(
        "the golden has {} lines, the run {}",
        want.lines().count(),
        got.lines().count()
    );
}
