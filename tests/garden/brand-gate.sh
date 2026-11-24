#!/usr/bin/env bash
# Tests for scripts/garden/brand-gate.py, against a small throwaway tree,
# then the real tree (through scripts/brand-gate.sh).
# Run: bash tests/garden/brand-gate.sh
set -euo pipefail
HERE="$(cd "$(dirname "$0")/../.." && pwd)"
GATE="$HERE/scripts/garden/brand-gate.py"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
pass=0

# run <want exit> <want text> <args...>: the exit code and the text must both match.
run() {
    local want="$1" text="$2" out code; shift 2
    set +e; out="$(python3 "$GATE" --root "$T" --allowlist "$T/a.txt" --cli-allowlist "$T/c.txt" --unsure-ceilings "$T/u.txt" "$@" 2>&1)"; code=$?; set -e
    if [ "$code" != "$want" ]; then echo "FAIL: exit $code, wanted $want: $out" >&2; exit 1; fi
    case "$out" in *"$text"*) ;; *) echo "FAIL: output lacks '$text': $out" >&2; exit 1 ;; esac
    pass=$((pass + 1))
}
# lacks <text> <args...>: the output must not have the text.
lacks() {
    local text="$1" out; shift
    out="$(python3 "$GATE" --root "$T" --allowlist "$T/a.txt" --cli-allowlist "$T/c.txt" --unsure-ceilings "$T/u.txt" "$@" 2>&1 || true)"
    case "$out" in *"$text"*) echo "FAIL: output has '$text': $out" >&2; exit 1 ;; esac
    pass=$((pass + 1))
}

mkdir -p "$T/crates/a/src" "$T/crates/a/tests" "$T/crates/k2-client/src" "$T/src-tauri/src" "$T/src/renderer/lib"
printf '# a comment\n\n2 K2 crates/a/src/old.rs debt: copy\n1 K5 crates/a/src/old.rs keeper: the K5 Space\n' > "$T/a.txt"
printf '# the command name\n' > "$T/c.txt"
printf '# unsure ceilings\n' > "$T/u.txt"
cat > "$T/crates/a/src/old.rs" <<'RS'
// K2 in a comment is not counted
/// nor in a doc comment: K2 Connect
fn a() -> &'static str { "K2 restarts" }
fn b() -> String { format!("{n} on K2") } // a note about K2
fn c() -> &'static str { "the K5 Space" }
fn d() -> &'static str { let _ = (Who::K2, K2::new()); "K2_PORT K2Error X-K2-Client k2 k2.dev K2SO" }
#[cfg(all(test, unix))]
mod tests {
    fn t() { assert_eq!(x, "K2 and K5 and Alakazam-211/K2"); }
}
RS
printf 'pub const PRODUCT_NAME: &str = "K2";\n' > "$T/crates/k2-client/src/brand.rs"
printf "export const PRODUCT_NAME = 'K2'\n" > "$T/src/renderer/lib/brand.ts"
printf 'fn t() { let h = "K2"; }\n' > "$T/crates/a/tests/it.rs"
printf 'fn t() { let h = "K2"; }\n' > "$T/crates/a/src/old_tests.rs"
printf 'fn t() { let h = "K2"; }\n' > "$T/crates/a/src/tests_more.rs"
printf 'fn clean() {}\n' > "$T/src-tauri/src/main.rs"

# Two K2 lines and one K5 line at their rows pass. Comments, the test module,
# test files, look-alike names and the brand modules are not counted.
run 0 "brand-gate: ok (2 K2, 1 K5, 0 repo lines in 2 rows still spell a name out)."
run 0 "K2 crates/a/src/old.rs:3: fn a()" --list
run 0 "K2 crates/a/src/old.rs:4: fn b()" --list
run 0 "K5 crates/a/src/old.rs:5: fn c()" --list K5
lacks "old.rs:1" --list
lacks "old.rs:2" --list
lacks "old.rs:6" --list
lacks "old.rs:9" --list
lacks "brand.rs" --list
lacks "brand.ts" --list
lacks "it.rs" --list
lacks "old_tests.rs" --list
lacks "tests_more.rs" --list
# A `#[cfg(test)] mod name;` only declares a test file: the code after it
# still counts, for the product's names and the command's. A later inline
# test module still ends the count.
printf '#[cfg(test)]\nmod decl_tests;\nfn d() -> &str { "Restart K2, then k2 msg <addr>" }\n#[cfg(test)]\nmod tests {\n    fn t() { let h = "K2"; }\n}\n' > "$T/crates/a/src/decl.rs"
run 0 "K2 crates/a/src/decl.rs:3: fn d()" --list
lacks "decl.rs:6" --list
rm "$T/crates/a/src/decl.rs"

# One more K2 line in a listed file fails and names the file and both counts.
cp "$T/crates/a/src/old.rs" "$T/keep.rs"
{ printf 'fn e() -> &str { "Update K2" }\n'; cat "$T/keep.rs"; } > "$T/crates/a/src/old.rs"
run 1 "crates/a/src/old.rs has 3 K2 lines, its row allows 2: use k2_core::brand or @/lib/brand for the new one."
# --tighten never raises a row, and leaves the file as it was.
cp "$T/a.txt" "$T/a.before"
run 1 "--tighten never raises one" --tighten
cmp -s "$T/a.txt" "$T/a.before" || { echo "FAIL: --tighten changed the allowlist on a failure" >&2; exit 1; }
pass=$((pass + 1))
cp "$T/keep.rs" "$T/crates/a/src/old.rs"

# A kind with no row fails and names the line: the repo, and a renderer file.
printf 'fn r() -> &str { "https://github.com/Alakazam-211/K2/releases" }\n' > "$T/src-tauri/src/main.rs"
run 1 "src-tauri/src/main.rs:1 spells out repo and has no repo row"
printf 'fn clean() {}\n' > "$T/src-tauri/src/main.rs"
printf "export const t = 'Restart K2'\n// K2 in a comment\n" > "$T/src/renderer/lib/a.ts"
run 1 "src/renderer/lib/a.ts:1 spells out K2 and has no K2 row"
rm "$T/src/renderer/lib/a.ts"
# A new K5 is the future name used too early: it fails like any other.
printf 'fn k() -> &str { "Welcome to K5" }\n' > "$T/crates/a/src/k5.rs"
run 1 "crates/a/src/k5.rs:1 spells out K5 and has no K5 row"
rm "$T/crates/a/src/k5.rs"
run 0 "brand-gate: ok"

# A row needs a reason that says keeper or debt.
printf '2 K2 crates/a/src/old.rs because\n1 K5 crates/a/src/old.rs keeper: the K5 Space\n' > "$T/a.txt"
run 1 "the reason must start 'keeper: ' or 'debt: '"
printf '2 K9 crates/a/src/old.rs debt: x\n' > "$T/a.txt"
run 1 "want '<lines> <K2|K5|repo> <path> <keeper|debt>: <reason>'"

# --tighten lowers a row to today's count, drops one at zero, keeps reasons.
printf '# head\n\n5 K2 crates/a/src/old.rs debt: copy\n1 K5 crates/a/src/old.rs keeper: the K5 Space\n3 repo crates/a/src/gone.rs debt: gone\n' > "$T/a.txt"
run 0 "row(s) are above today's count"
run 0 "brand-gate: a.txt tightened: 3 lines in 2 rows." --tighten
want="$(printf '# head\n\n2 K2 crates/a/src/old.rs debt: copy\n1 K5 crates/a/src/old.rs keeper: the K5 Space')"
[ "$(cat "$T/a.txt")" = "$want" ] || { echo "FAIL: tightened allowlist: $(cat "$T/a.txt")" >&2; exit 1; }
pass=$((pass + 1))

# ── The command's name: the kinds cli, value and glued, and the unsure
# ceilings of the bash CLI's parts, on a second small tree. ──────────────
C="$T/cmd"
# crun / clacks: as run / lacks, on this tree and its three files.
crun() {
    local want="$1" text="$2" out code; shift 2
    set +e; out="$(python3 "$GATE" --root "$C" --allowlist "$C/a.txt" --cli-allowlist "$C/c.txt" --unsure-ceilings "$C/u.txt" "$@" 2>&1)"; code=$?; set -e
    if [ "$code" != "$want" ]; then echo "FAIL: exit $code, wanted $want: $out" >&2; exit 1; fi
    case "$out" in *"$text"*) ;; *) echo "FAIL: output lacks '$text': $out" >&2; exit 1 ;; esac
    pass=$((pass + 1))
}
clacks() {
    local text="$1" out; shift
    out="$(python3 "$GATE" --root "$C" --allowlist "$C/a.txt" --cli-allowlist "$C/c.txt" --unsure-ceilings "$C/u.txt" "$@" 2>&1 || true)"
    case "$out" in *"$text"*) echo "FAIL: output has '$text': $out" >&2; exit 1 ;; esac
    pass=$((pass + 1))
}
# rows <c.txt rows...>: the cli allowlist, after its header.
rows() { { printf '# the command name\n\n'; for r in "$@"; do printf '%s\n' "$r"; done; } > "$C/c.txt"; }
mkdir -p "$C/cli/src" "$C/crates/b/src" "$C/crates/k2-brand/src" "$C/src/renderer/lib"
printf '# product names\n' > "$C/a.txt"
printf '# unsure ceilings\n' > "$C/u.txt"
rows
# The CLI's verbs come from its router, as the flip reads them.
cat > "$C/cli/src/router.sh" <<'SH'
case "$1" in
    msg|send) cmd_msg "$@" ;;
    inbox|mail|study|app|sidecar) cmd_other "$@" ;;
esac
SH
printf 'fn clean() {}\n' > "$C/crates/b/src/lib.rs"
crun 0 "brand-gate: ok (0 cli, 0 value, 0 glued lines in 0 rows type the command name; 0 value lines are the command)."

# Gate 1, cli. A new command line in a Rust string with no row fails.
printf 'fn a() -> &'"'"'static str { "run k2 msg <addr>" }\n' > "$C/crates/b/src/lib.rs"
crun 1 "crates/b/src/lib.rs:1 spells out cli and has no cli row: use CLI_NAME or cli_name!()"
# At its row it passes; one more line fails; one line fewer passes.
rows "1 cli crates/b/src/lib.rs debt: the command name typed out; use CLI_NAME or cli_name!()"
crun 0 "brand-gate: ok (1 cli, 0 value, 0 glued lines in 1 rows"
printf 'fn b() -> String { format!("then k2 msg {}", 1) }\n' >> "$C/crates/b/src/lib.rs"
crun 1 "crates/b/src/lib.rs has 2 cli lines, its row allows 1: use CLI_NAME or cli_name!()"
printf 'fn b() -> String { format!("then {}", 1) }\n' > "$C/crates/b/src/lib.rs"
crun 0 "row(s) are above today's count in c.txt"
# A line with two uses is one line.
printf 'fn a() -> &'"'"'static str { "k2 msg a, or k2 inbox read" }\n' > "$C/crates/b/src/lib.rs"
crun 0 "brand-gate: ok (1 cli,"
# A comment, a test module, a test file, the brand crate: not counted.
cat > "$C/crates/b/src/lib.rs" <<'RS'
// run k2 msg <addr> in a comment
fn a() -> u8 { 1 } // run k2 msg <addr> after code
/// run k2 msg <addr> in a doc comment
/* run k2 msg <addr> in a block comment */
fn b() -> [&'static str; 5] { ["open k2.dev", "K2_PORT msg", "~/.k2 msg", "k2-daemon msg", "[k2 sidecar] up"] }
#[cfg(test)]
mod tests {
    fn t() -> &'static str { "run k2 msg <addr>" }
}
RS
printf 'fn t() -> &'"'"'static str { "run k2 msg <addr>" }\n' > "$C/crates/b/src/foo_tests.rs"
printf 'pub const L: &str = "run k2 msg <addr>";\n' > "$C/crates/k2-brand/src/lib.rs"
rows
crun 0 "brand-gate: ok (0 cli, 0 value, 0 glued lines in 0 rows"
clacks "lib.rs" --list
clacks "foo_tests.rs" --list
# The same words in a Rust string are counted: the [k2 ...] stamp only is kept.
printf 'fn c() -> &'"'"'static str { "[k2 sidecar] up; k2 sidecar list" }\n' > "$C/crates/b/src/lib.rs"
crun 0 "cli crates/b/src/lib.rs:1: fn c()" --list cli
# k5 is the command's name too.
printf 'fn k() -> &'"'"'static str { "ask with: k5 msg <addr>" }\n' > "$C/crates/b/src/lib.rs"
crun 1 "crates/b/src/lib.rs:1 spells out cli and has no cli row"
printf 'fn clean() {}\n' > "$C/crates/b/src/lib.rs"
# The renderer: any code line, not a test file.
printf 'export const open = (id: string) => `Open: k2 inbox read ${id}`\n' > "$C/src/renderer/lib/x.ts"
crun 1 "src/renderer/lib/x.ts:1 spells out cli and has no cli row"
mv "$C/src/renderer/lib/x.ts" "$C/src/renderer/lib/x.test.tsx"
crun 0 "brand-gate: ok (0 cli,"
rm "$C/src/renderer/lib/x.test.tsx"

# Gate 2, value: a Rust string that is exactly the name.
printf 'fn v() -> &'"'"'static str { "k2" }\n' > "$C/crates/b/src/lib.rs"
crun 1 "crates/b/src/lib.rs:1 spells out value and has no value row: use CLI_NAME, or add a keeper: row"
printf 'fn v() -> [&'"'"'static str; 2] { ["k2 ", "k2so"] }\n' > "$C/crates/b/src/lib.rs"
crun 0 "brand-gate: ok (0 cli, 0 value,"
printf 'fn v() -> &'"'"'static str { "k5" }\n' > "$C/crates/b/src/lib.rs"
# B1's seed, `debt: unsorted value`, is refused now: a new exact value
# needs its class the day it is written. --unsorted still lists such rows
# (a tree that has some), and prints nothing when there are none.
rows "1 value crates/b/src/lib.rs debt: unsorted value"
crun 1 "the value row for crates/b/src/lib.rs: 'debt: unsorted value' is not a reason any more: read who reads each line and give it a class"
crun 0 "brand-gate: 1 value row(s) unsorted (1 lines)." --unsorted
crun 0 "1 value crates/b/src/lib.rs" --unsorted
rows "1 value crates/b/src/lib.rs keeper: stored; the config file's tool key"
crun 0 "0 glued lines in 1 rows type the command name; 0 value lines are the command"
out="$(python3 "$GATE" --root "$C" --allowlist "$C/a.txt" --cli-allowlist "$C/c.txt" --unsure-ceilings "$C/u.txt" --list value --unsorted 2>&1)"
[ -z "$out" ] || { echo "FAIL: --unsorted with none left printed: $out" >&2; exit 1; }
pass=$((pass + 1))
mv "$C/crates/b/src/lib.rs" "$C/crates/b/src/lib_test.rs"
rows
crun 0 "brand-gate: ok (0 cli, 0 value,"
mv "$C/crates/b/src/lib_test.rs" "$C/crates/b/src/lib.rs"
rows "1 value crates/b/src/lib.rs because"
crun 1 "the reason must start 'keeper: ' or 'debt: '"
rows "1 K2 crates/b/src/lib.rs debt: x"
crun 1 "want '<lines> <cli|value|glued> <path> <keeper|debt>: <reason>'"

# A value row's reason has a class: keeper: <class>[+<class>]; <who reads
# it>, or debt: command (<n> lines); <step>[; keeps: <class>; <who>].
printf 'fn v() -> [&'"'"'static str; 3] { ["k2", "k2", "k2"] }\nfn w() -> &'"'"'static str { "k2" }\nfn x() -> &'"'"'static str { "k2" }\n' > "$C/crates/b/src/lib.rs"
for good in "keeper: stored; the audit log's actor column" \
            "keeper: stored+wire; the firewall config, and the app reads it" \
            "keeper: outside; Postgres, the schema's name" \
            "keeper: inert; bytes written to probe a folder" \
            "debt: command (3 lines); B9" \
            "debt: command (2 lines); B3; keeps: outside; the package's lib folder" \
            "debt: command (1 line); B9; keeps: stored+wire; the agent mode"; do
    rows "3 value crates/b/src/lib.rs $good"
    crun 0 "3 value, 0 glued lines in 1 rows"
done
bad() { rows "3 value crates/b/src/lib.rs $1"; crun 1 "the value row for crates/b/src/lib.rs: $2"; }
bad "keeper: the config file's tool key" "want 'keeper: <class>[+<class>]; <who reads it>' or 'debt: command (<n> lines); <step>"
bad "keeper: stored" "want 'keeper: <class>[+<class>]; <who reads it>'"
bad "keeper: kept; the audit log" "unknown class 'kept'; a kept value is stored, wire, outside, inert"
bad "keeper: stored+shown; the audit log" "unknown class 'shown'"
bad "keeper: command; the bin folder" "unknown class 'command'"
bad "keeper: ask; who reads the label?" "class 'ask': the gardener has not answered yet"
bad "debt: command (2 lines); B3; keeps: ask; the label" "class 'ask'"
bad "debt: command (4 lines); B9" "'command (4 lines)' must be 1 to the row's 3 lines"
bad "debt: command (0 lines); B9; keeps: stored; x" "'command (0 lines)' must be 1 to the row's 3 lines"
bad "debt: command (3 lines)" "a command debt names the step that moves it"
bad "debt: command (3 lines); later" "want 'keeper: <class>[+<class>]; <who reads it>'"
bad "debt: command (2 lines); B9" "1 of the row's 3 lines are not the command: add '; keeps: <class>; <who reads it>'"
bad "debt: command (3 lines); B9; keeps: stored; x" "all 3 lines are the command, so the row keeps none"
bad "debt: should move" "want 'keeper: <class>[+<class>]; <who reads it>'"

# --list value --debt: the command's lines, row by row, and their total.
rows "3 value crates/b/src/lib.rs debt: command (2 lines); B3; keeps: inert; a probe" \
     "1 value src/renderer/lib/v.ts debt: command (1 line); B9"
printf "export const tool = 'k2'\n" > "$C/src/renderer/lib/v.ts"
crun 0 "2 crates/b/src/lib.rs B3" --list value --debt
crun 0 "1 src/renderer/lib/v.ts B9" --list value --debt
crun 0 "brand-gate: 3 value lines are the command (B3 2, B9 1)." --list value --debt
crun 1 "--debt goes with --list value" --list cli --debt
crun 0 "0 glued lines in 2 rows type the command name; 3 value lines are the command)."
rm "$C/src/renderer/lib/v.ts"
rows
crun 0 "brand-gate: 0 value lines are the command." --list value --debt

# The renderer: a code line with a string that is exactly the name, in any
# of the three quote forms. Not in a test file, not in a comment, not a
# longer string.
printf 'fn clean() {}\n' > "$C/crates/b/src/lib.rs"
printf "export const who = 'k2'\n" > "$C/src/renderer/lib/w.ts"
crun 1 "src/renderer/lib/w.ts:1 spells out value and has no value row"
for line in 'export const who = "k2"' 'export const who = `k2`' "export type Who = 'k5' | 'local'" "if (mode === 'k2') go() // the mode"; do
    printf '%s\n' "$line" > "$C/src/renderer/lib/w.ts"
    crun 0 "value src/renderer/lib/w.ts:1:" --list value
done
cp "$C/src/renderer/lib/w.ts" "$C/src/renderer/lib/w.test.ts"
rows "1 value src/renderer/lib/w.ts keeper: inert; the wiki page's own mode"
crun 0 "0 cli, 1 value, 0 glued lines in 1 rows"
rm "$C/src/renderer/lib/w.test.ts"
cat > "$C/src/renderer/lib/w.ts" <<'TS'
// const who = 'k2'
/* const who = 'k2' */
 * const who = 'k2'
export const a = 'k2 ', b = 'k2so', c = "~/.k2"
export const e = go() // was 'k2'
export const f = 'see // k2', g = `${'k2-node'}`
TS
rows
crun 0 "0 cli, 0 value, 0 glued lines in 0 rows"
rm "$C/src/renderer/lib/w.ts"

# Gate 4, glued: a brand name built into a token fails with no row and
# passes with a keeper row.
glued() {
    local file="$1" body="$2"
    printf '%s\n' "$body" > "$C/$file"
    crun 1 "$file:1 spells out glued and has no glued row"
    rows "1 glued $file keeper: the other side's header parser"
    crun 0 "0 value, 1 glued lines in 1 rows"
    rm "$C/$file"; rows
}
glued crates/b/src/g.rs 'fn g() -> String { format!("X-{PRODUCT_NAME}-Sig") }'
glued crates/b/src/g.rs 'fn g() -> String { format!("{CLI_NAME}_session") }'
glued crates/b/src/g.rs 'fn g() -> String { format!("{BRAND_ID}.json") }'
glued crates/b/src/g.rs 'fn g() -> String { format!("{CLI_NAME}:{id}") }'
glued crates/b/src/g.rs 'const P: &str = concat!(cli_name!(), "skn_");'
glued crates/b/src/g.rs 'const P: &str = concat!("https://x/", product_name!());'
glued src/renderer/lib/g.ts 'export const bin = `${CLI_NAME}-cli`'
# The name passed as a value is glued the same way (C4): a format hole by
# position, index or name, a `+` beside a literal, concat and join.
glued crates/b/src/g.rs 'fn g() -> String { format!("X-{}-Sig", PRODUCT_NAME) }'
glued crates/b/src/g.rs 'fn g(id: &str) -> String { format!("{1}_{0}", id, k2_core::brand::CLI_NAME) }'
glued crates/b/src/g.rs 'fn g() -> String { format!("{n}skn_", n = crate::brand::BRAND_ID) }'
glued crates/b/src/g.rs 'fn g(w: &mut String) { let _ = write!(w, "{}.sock", &BRAND_ID); }'
glued crates/b/src/g.rs 'fn g() -> String { format!("/run/{}app", cli_name!()) }'
glued crates/b/src/g.rs 'fn g() -> String { CLI_NAME.to_string() + "_session" }'
glued crates/b/src/g.rs 'fn g() -> String { "X-".to_string() + PRODUCT_NAME }'
glued crates/b/src/g.rs 'fn g() -> String { String::from("__Host-") + &BRAND_ID }'
glued crates/b/src/g.rs 'fn g() -> String { [CLI_NAME, "-cli.fish"].concat() }'
glued crates/b/src/g.rs 'fn g() -> String { ["x", BRAND_ID, "v1"].join("-") }'
glued src/renderer/lib/g.ts "export const k = 'X-' + PRODUCT_NAME"
glued src/renderer/lib/g.ts "export const k = brand.CLI_NAME + '_session'"
glued src/renderer/lib/g.ts "export const p = [CLI_NAME, 'cli'].join('-')"
# Prose beside the name is not glued, nor is a name passed as a value with
# a space, a stop or an escape beside it.
cat > "$C/crates/b/src/v.rs" <<'RS'
fn v(id: &str) -> Vec<String> { vec![format!("run {} app list", CLI_NAME), format!("Use {}.", k2_core::brand::CLI_NAME), format!("{0}: {1}", CLI_NAME, id), format!("in {p}, ok", p = PRODUCT_NAME), format!("\n{}\t", CLI_NAME)] }
fn w() -> String { CLI_NAME.to_string() + " study" }
fn x() -> String { [CLI_NAME, "app", "list"].join(" ") }
fn y() -> String { format!("{}-{}", id, other) }
RS
cat > "$C/src/renderer/lib/v.ts" <<'TS'
export const a = 'Run ' + CLI_NAME + ' study.'
export const b = [CLI_NAME, 'app', 'list'].join(' ')
TS
crun 0 "0 value, 0 glued lines in 0 rows"
rm "$C/crates/b/src/v.rs" "$C/src/renderer/lib/v.ts"
# Prose beside the name is not glued.
cat > "$C/crates/b/src/g.rs" <<'RS'
fn p() -> Vec<String> { vec![format!("run {CLI_NAME} app list"), format!("Use {CLI_NAME}."), format!("{CLI_NAME}: no such command"), format!("in {PRODUCT_NAME}, ok")] }
const S: &str = concat!("see `", cli_name!(), " study`");
const T: &str = concat!("Nobody saves a drawing whole in ", product_name!(), ": a .dxf changes");
const U: &str = concat!("run ", cli_name!(), "\n");
RS
printf 'export const t = `Run ${CLI_NAME} study, in ${PRODUCT_NAME}.`\n' > "$C/src/renderer/lib/g.ts"
crun 0 "0 value, 0 glued lines in 0 rows"
rm "$C/crates/b/src/g.rs" "$C/src/renderer/lib/g.ts"
rows "1 glued crates/b/src/lib.rs debt: should move"
crun 1 "a glued row is always 'keeper: <who reads this name>'"
rows

# The slot rule (B7 trap 1): a {cli} or {product} slot in a format-family
# macro's format string is a capture, and fails with no row to allow it,
# in code and tests alike. {{cli}} is the slot's form there; a plain
# literal, a json! value, a .replace pattern and a value compared by
# assert_eq! are not format strings. The k5 CLI and the brand crate are not
# read.
slot() {
    local file="$1" body="$2" hole="$3"
    mkdir -p "$(dirname "$C/$file")"
    printf '%s\n' "$body" > "$C/$file"
    crun 1 "$file:1 has the slot $hole in a format string, where Rust reads it as a capture: in a format string write the slot {{cli}} or {{product}}"
    rm "$C/$file"
}
slot crates/b/src/s.rs 'fn s(cli: &str) -> String { format!("run {cli} app list") }' '{cli}'
slot crates/b/src/s.rs 'fn s() -> String { format!("in {product}, run x", product = PRODUCT_NAME) }' '{product}'
slot crates/b/src/s.rs 'fn s(w: &mut String, cli: &str) { let _ = writeln!(w, "{cli:>8} x"); }' '{cli:>8}'
slot crates/b/src/s.rs 'fn s(cli: &str) { log_debug!("open cli={cli}"); }' '{cli}'
slot crates/b/src/s.rs 'fn s(cli: &str) -> Result<(), E> { bail!("no {cli} here") }' '{cli}'
slot crates/b/src/s.rs 'fn s(x: bool, cli: &str) { assert!(x, "{cli}"); }' '{cli}'
slot crates/b/src/s.rs 'fn s(cli: &str) { tracing::warn!(target: "t", n = 1, "a {cli}"); }' '{cli}'
slot crates/b/tests/it.rs 'fn s(cli: &str) -> String { format!("K2CLI={cli}") }' '{cli}'
slot src-tauri/src/s.rs 'fn s(cli: &str) -> String { std::format!("{cli} x") }' '{cli}'
cat > "$C/crates/b/src/s.rs" <<'RS'
fn s(cli: &str, t: &str) -> Vec<String> {
    let a = json!({"hintTemplate": "run {cli} app list from outside {product}"});
    let b = "run {cli} x".replace("{cli}", CLI_NAME);
    assert_eq!(t, "run {cli} x", "{t}");
    vec![format!("run {{cli}} app list in {{product}}, here {CLI_NAME} {PRODUCT_NAME} {}", cli), format!("{t}"), b]
}
const T: &str = "run {cli} app list";
RS
mkdir -p "$C/crates/k5-cli/src" "$C/crates/k2-brand/src"
printf 'fn s(cli: &str) -> String { format!("{cli}: no such command") }\n' > "$C/crates/k5-cli/src/s.rs"
printf 'fn s(product: &str) -> String { format!("{product} Server") }\n' > "$C/crates/k2-brand/src/s.rs"
crun 0 "brand-gate: ok (no {cli} or {product} slot in a format string)."
rm -r "$C/crates/b/src/s.rs" "$C/crates/k5-cli" "$C/crates/k2-brand/src/s.rs"

# Gate 2, value, in the bash CLI's parts (cli/src, and cli/text where it
# exists): a line where the flip finds a string that is exactly the name, or
# the name as a word in shell code. The flip calls both kept; the row holds
# them. A line with two uses is one line; text, stamps and look-alikes are
# not counted.
cat > "$C/cli/src/v.sh" <<'SH'
cmd_v() {
    MODE="k2"; OTHER="k5"
    case "$m" in off|k2|k5) ;; esac
    echo "run k2 msg <addr>"
    echo "[k2 sidecar] up, k2.dev, ~/.k2, k2so"
}
SH
crun 1 "cli/src/v.sh:2 spells out value and has no value row: use CLI_NAME, or add a keeper: row"
crun 1 "cli/src/v.sh:3 spells out value and has no value row"
clacks "cli/src/v.sh:4"
clacks "cli/src/v.sh:5"
rows "2 value cli/src/v.sh keeper: stored; the mode"
crun 0 "0 cli, 2 value, 0 glued lines in 1 rows"
crun 0 'value cli/src/v.sh:2: MODE="k2"; OTHER="k5"' --list value
crun 0 'value cli/src/v.sh:3: case "$m" in off|k2|k5) ;; esac' --list value
printf 'cmd_w() { M2="k2"; }\n' >> "$C/cli/src/v.sh"
crun 1 "cli/src/v.sh has 3 value lines, its row allows 2: use CLI_NAME, or add a keeper: row"
rows "3 value cli/src/v.sh debt: command (1 line); B4; keeps: stored; the mode"
crun 0 "1 cli/src/v.sh B4" --list value --debt
crun 0 "0 glued lines in 1 rows type the command name; 1 value lines are the command)."
# A cli/text page is read where the folder exists.
mkdir -p "$C/cli/text/help"
printf 'The mode is "k2" or off; run k2 msg <addr>.\n' > "$C/cli/text/help/page.txt"
crun 1 "cli/text/help/page.txt:1 spells out value and has no value row"
rm -r "$C/cli/text" "$C/cli/src/v.sh"
rows

# Gate 3: the unsure uses of the CLI's parts, one ceiling per reason.
cat > "$C/cli/src/a.sh" <<'SH'
cmd_a() {
    echo "--identity is k2 or standalone"
    echo "--variant is k2 or standalone"
    cat <<EOF
$(k2 msg a)
EOF
}
SH
crun 1 "2 unsure uses of the command name in the CLI's parts (k2 followed by a word that is not a command), the ceiling is 0"
printf '# unsure ceilings\n\n2 k2 followed by a word that is not a command\n1 $(k2 ...): runs the command\n' > "$C/u.txt"
crun 0 "unsure in the CLI's parts: 2 k2 followed by a word that is not a command, 1 \$(k2 ...): runs the command)"
cat > "$C/cli/src/a.sh" <<'SH'
cmd_a() {
    echo "--identity is k2 or standalone"
    echo "--variant is k2 or standalone"
    echo "--mode is k2 or off"
    cat <<EOF
$(k2 msg a)
EOF
}
SH
crun 1 "3 unsure uses of the command name in the CLI's parts (k2 followed by a word that is not a command), the ceiling is 2"
# --tighten never raises a ceiling, and leaves the file as it was.
cp "$C/u.txt" "$C/u.before"
crun 1 "its ceiling is 2; --tighten never raises one" --tighten
cmp -s "$C/u.txt" "$C/u.before" || { echo "FAIL: --tighten changed the ceilings on a failure" >&2; exit 1; }
pass=$((pass + 1))
cat > "$C/cli/src/a.sh" <<'SH'
cmd_a() {
    echo "--identity is k2 or standalone"
    cat <<EOF
$(k2 msg a)
EOF
}
SH
crun 0 "brand-gate: 1 unsure ceiling(s) are above today's count"
crun 0 "brand-gate: u.txt tightened: 2 unsure uses for 2 reasons." --tighten
want="$(printf '# unsure ceilings\n\n1 $(k2 ...): runs the command\n1 k2 followed by a word that is not a command')"
[ "$(cat "$C/u.txt")" = "$want" ] || { echo "FAIL: tightened ceilings: $(cat "$C/u.txt")" >&2; exit 1; }
pass=$((pass + 1))
printf '# unsure ceilings\n\n1 $(k2 ...): runs the command\nx k2 followed by a word that is not a command\n' > "$C/u.txt"
crun 1 "want '<count> <reason>'"
rm "$C/cli/src/a.sh"
printf '# unsure ceilings\n' > "$C/u.txt"

# The three closed reasons have no ceiling, ever. A use in embedded code
# that is not in a string fails with nothing listed; an exact value and a
# shell word fail with no value row; a ceiling row for any of them is
# refused.
cat > "$C/cli/src/a.sh" <<'SH'
cmd_a() {
    python3 -c 'print(k2)'
}
SH
crun 1 "1 unsure uses of the command name in the CLI's parts (interpreter code, not in a string), the ceiling is 0: write it as text, a comment, or a name set with ="
cat > "$C/cli/src/a.sh" <<'SH'
cmd_a() {
    python3 -c "k2 = 1  # see k2 msg"
}
SH
crun 0 "brand-gate: ok (unsure in the CLI's parts: none)."
printf 'cmd_a() { MODE="k2"; }\n' > "$C/cli/src/a.sh"
crun 1 "cli/src/a.sh:1 spells out value and has no value row"
printf 'cmd_a() { case "$m" in k2) ;; esac; }\n' > "$C/cli/src/a.sh"
crun 1 "cli/src/a.sh:1 spells out value and has no value row"
rm "$C/cli/src/a.sh"
for r in "a string that is exactly k2 (a value)" "k2 as a word in shell code" "interpreter code, not in a string"; do
    printf '# unsure ceilings\n\n1 %s\n' "$r" > "$C/u.txt"
    crun 1 "'$r' is closed and has no ceiling"
done
printf '# unsure ceilings\n' > "$C/u.txt"

# All kinds: --tighten lowers rows to today's counts, drops rows at zero and
# keeps reasons; --list KIND prints every counted line.
cat > "$C/crates/b/src/lib.rs" <<'RS'
fn a() -> &'static str { "run k2 msg <addr>" }
fn v() -> &'static str { "k2" }
fn g() -> String { format!("{CLI_NAME}_session") }
RS
rows "5 cli crates/b/src/lib.rs debt: the command name typed out; use CLI_NAME or cli_name!()" \
     "2 value crates/b/src/lib.rs keeper: stored; the session file's tool key" \
     "1 glued crates/b/src/lib.rs keeper: the session cookie's reader" \
     "3 cli crates/b/src/gone.rs debt: gone"
crun 0 "cli crates/b/src/lib.rs:1: fn a()" --list cli
crun 0 "value crates/b/src/lib.rs:2: fn v()" --list value
crun 0 "glued crates/b/src/lib.rs:3: fn g()" --list glued
clacks "value crates" --list cli
crun 0 "brand-gate: c.txt tightened: 3 lines in 3 rows." --tighten
want="$(printf '# the command name\n\n1 cli crates/b/src/lib.rs debt: the command name typed out; use CLI_NAME or cli_name!()\n1 value crates/b/src/lib.rs keeper: stored; the session file'"'"'s tool key\n1 glued crates/b/src/lib.rs keeper: the session cookie'"'"'s reader')"
[ "$(cat "$C/c.txt")" = "$want" ] || { echo "FAIL: tightened cli allowlist: $(cat "$C/c.txt")" >&2; exit 1; }
pass=$((pass + 1))
crun 0 "brand-gate: ok (1 cli, 1 value, 1 glued lines in 3 rows type the command name; 0 value lines are the command)."

# The real tree passes, through the script CI runs.
out="$(bash "$HERE/scripts/brand-gate.sh" 2>&1)" || { echo "FAIL: the real tree: $out" >&2; exit 1; }
case "$out" in *"brand-gate: ok"*) ;; *) echo "FAIL: no brand-gate ok line: $out" >&2; exit 1 ;; esac
pass=$((pass + 1))

echo "brand-gate tests: $pass passed"
