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
    set +e; out="$(python3 "$GATE" --root "$T" --allowlist "$T/a.txt" "$@" 2>&1)"; code=$?; set -e
    if [ "$code" != "$want" ]; then echo "FAIL: exit $code, wanted $want: $out" >&2; exit 1; fi
    case "$out" in *"$text"*) ;; *) echo "FAIL: output lacks '$text': $out" >&2; exit 1 ;; esac
    pass=$((pass + 1))
}
# lacks <text> <args...>: the output must not have the text.
lacks() {
    local text="$1" out; shift
    out="$(python3 "$GATE" --root "$T" --allowlist "$T/a.txt" "$@" 2>&1 || true)"
    case "$out" in *"$text"*) echo "FAIL: output has '$text': $out" >&2; exit 1 ;; esac
    pass=$((pass + 1))
}

mkdir -p "$T/crates/a/src" "$T/crates/a/tests" "$T/crates/k2-client/src" "$T/src-tauri/src" "$T/src/renderer/lib"
printf '# a comment\n\n2 K2 crates/a/src/old.rs debt: copy\n1 K5 crates/a/src/old.rs keeper: the K5 Space\n' > "$T/a.txt"
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
run 0 "brand-gate: allowlist tightened: 3 lines in 2 rows." --tighten
want="$(printf '# head\n\n2 K2 crates/a/src/old.rs debt: copy\n1 K5 crates/a/src/old.rs keeper: the K5 Space')"
[ "$(cat "$T/a.txt")" = "$want" ] || { echo "FAIL: tightened allowlist: $(cat "$T/a.txt")" >&2; exit 1; }
pass=$((pass + 1))

# The real tree passes, through the script CI runs.
out="$(bash "$HERE/scripts/brand-gate.sh" 2>&1)" || { echo "FAIL: the real tree: $out" >&2; exit 1; }
case "$out" in *"brand-gate: ok"*) ;; *) echo "FAIL: no brand-gate ok line: $out" >&2; exit 1 ;; esac
pass=$((pass + 1))

echo "brand-gate tests: $pass passed"
