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
crun 0 "brand-gate: ok (0 cli, 0 value, 0 glued lines in 0 rows type the command name; 0 value rows unsorted)."

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
rows "1 value crates/b/src/lib.rs debt: unsorted value"
crun 0 "0 glued lines in 1 rows type the command name; 1 value rows unsorted"
crun 0 "brand-gate: 1 value row(s) unsorted (1 lines)." --unsorted
crun 0 "1 value crates/b/src/lib.rs" --unsorted
rows "1 value crates/b/src/lib.rs keeper: stored; the config file's tool key"
crun 0 "brand-gate: 0 value row(s) unsorted (0 lines)." --unsorted
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

# Gate 3: the unsure uses of the CLI's parts, one ceiling per reason.
cat > "$C/cli/src/a.sh" <<'SH'
cmd_a() {
    MODE="k2"
    OTHER="k2"
    bin="$(command -v k2 || true)"
}
SH
crun 1 "2 unsure uses of the command name in the CLI's parts (a string that is exactly k2 (a value)), the ceiling is 0"
printf '# unsure ceilings\n\n2 a string that is exactly k2 (a value)\n1 k2 as a word in shell code\n' > "$C/u.txt"
crun 0 "unsure in the CLI's parts: 2 a string that is exactly k2 (a value), 1 k2 as a word in shell code)"
cat > "$C/cli/src/a.sh" <<'SH'
cmd_a() {
    MODE="k2"
    OTHER="k2"
    bin="$(command -v k2 || true)"
    THIRD="k2"
}
SH
crun 1 "3 unsure uses of the command name in the CLI's parts (a string that is exactly k2 (a value)), the ceiling is 2"
# --tighten never raises a ceiling, and leaves the file as it was.
cp "$C/u.txt" "$C/u.before"
crun 1 "its ceiling is 2; --tighten never raises one" --tighten
cmp -s "$C/u.txt" "$C/u.before" || { echo "FAIL: --tighten changed the ceilings on a failure" >&2; exit 1; }
pass=$((pass + 1))
cat > "$C/cli/src/a.sh" <<'SH'
cmd_a() {
    MODE="k2"
    bin="$(command -v k2 || true)"
}
SH
crun 0 "brand-gate: 1 unsure ceiling(s) are above today's count"
crun 0 "brand-gate: u.txt tightened: 2 unsure uses for 2 reasons." --tighten
want="$(printf '# unsure ceilings\n\n1 a string that is exactly k2 (a value)\n1 k2 as a word in shell code')"
[ "$(cat "$C/u.txt")" = "$want" ] || { echo "FAIL: tightened ceilings: $(cat "$C/u.txt")" >&2; exit 1; }
pass=$((pass + 1))
printf '# unsure ceilings\n\n1 a string that is exactly k2 (a value)\nx k2 as a word in shell code\n' > "$C/u.txt"
crun 1 "want '<count> <reason>'"
rm "$C/cli/src/a.sh"
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
crun 0 "brand-gate: ok (1 cli, 1 value, 1 glued lines in 3 rows type the command name; 0 value rows unsorted)."

# The real tree passes, through the script CI runs.
out="$(bash "$HERE/scripts/brand-gate.sh" 2>&1)" || { echo "FAIL: the real tree: $out" >&2; exit 1; }
case "$out" in *"brand-gate: ok"*) ;; *) echo "FAIL: no brand-gate ok line: $out" >&2; exit 1 ;; esac
pass=$((pass + 1))

echo "brand-gate tests: $pass passed"
