#!/usr/bin/env bash
# Tests for scripts/garden/zone-gate.py, against a small throwaway tree,
# then the real tree.
# Run: bash tests/garden/zone-gate.sh
set -euo pipefail
HERE="$(cd "$(dirname "$0")/../.." && pwd)"
GATE="$HERE/scripts/garden/zone-gate.py"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
pass=0

# run <want exit> <want text> <args...>: the exit code and the text must both match.
run() {
    local want="$1" text="$2" out code; shift 2
    set +e; out="$(python3 "$GATE" --root "$T" --ceilings "$T/c.txt" "$@" 2>&1)"; code=$?; set -e
    if [ "$code" != "$want" ]; then echo "FAIL: exit $code, wanted $want: $out" >&2; exit 1; fi
    case "$out" in *"$text"*) ;; *) echo "FAIL: output lacks '$text': $out" >&2; exit 1 ;; esac
    pass=$((pass + 1))
}
# lacks <text> <args...>: the output must not have the text.
lacks() {
    local text="$1" out; shift
    out="$(python3 "$GATE" --root "$T" --ceilings "$T/c.txt" "$@" 2>&1 || true)"
    case "$out" in *"$text"*) echo "FAIL: output has '$text': $out" >&2; exit 1 ;; esac
    pass=$((pass + 1))
}

mkdir -p "$T/crates/a/src" "$T/crates/a/tests" "$T/crates/k2-client/src" "$T/src-tauri/src" "$T/crates/a/target"
printf '# a comment\n\n2 crates/a/src/old.rs\n' > "$T/c.txt"
cat > "$T/crates/a/src/old.rs" <<'RS'
// k2.dev in a comment is not counted
/// nor in a doc comment: acme.k2.dev
fn a() -> bool { host.ends_with(".k2.dev") }
fn b() -> String { format!("{l}.k2.dev") } // a note about k2.dev
fn c() {}
#[cfg(test)]
mod tests {
    fn t() { assert_eq!(x, "acme.k2.dev"); }
}
RS
printf 'pub const ZONE: &str = "k2.dev";\n' > "$T/crates/k2-client/src/hosts.rs"
printf 'fn t() { let h = "a.k2.dev"; }\n' > "$T/crates/a/tests/it.rs"
printf 'fn t() { let h = "a.k2.dev"; }\n' > "$T/crates/a/src/old_tests.rs"
printf 'fn t() { let h = "a.k2.dev"; }\n' > "$T/crates/a/target/built.rs"
printf 'fn clean() {}\n' > "$T/src-tauri/src/main.rs"

# Two code lines at a ceiling of two pass. Comments, the test module, test
# files, build output and the hosts module are not counted.
run 0 "zone-gate: ok (2 lines in 1 files still spell out k2.dev)."
run 0 "crates/a/src/old.rs:3: fn a()" --list
run 0 "crates/a/src/old.rs:4: fn b()" --list
lacks "old.rs:1" --list
lacks "old.rs:2" --list
lacks "old.rs:8" --list
lacks "hosts.rs" --list
lacks "it.rs" --list
lacks "old_tests.rs" --list
lacks "built.rs" --list

# One more line in a listed file fails and names the file and both counts.
cat "$T/crates/a/src/old.rs" > "$T/keep.rs"
{ printf 'fn d() -> &str { "relay.k2.dev" }\n'; cat "$T/keep.rs"; } > "$T/crates/a/src/old.rs"
run 1 "crates/a/src/old.rs has 3 lines with k2.dev, its ceiling is 2: use k2_core::hosts for the new one."
# --tighten never raises a ceiling, and leaves the file as it was.
run 1 "--tighten never raises one" --tighten
[ "$(cat "$T/c.txt")" = "$(printf '# a comment\n\n2 crates/a/src/old.rs')" ] || { echo "FAIL: --tighten changed the ceilings on a failure" >&2; exit 1; }
pass=$((pass + 1))
cp "$T/keep.rs" "$T/crates/a/src/old.rs"

# A line in a file that is not listed fails and names the line. src-tauri is checked too.
printf 'fn clean() {}\nfn h() -> String { format!("{x}.k2.dev") }\n' > "$T/src-tauri/src/main.rs"
run 1 "src-tauri/src/main.rs:2 spells out k2.dev and the file is not listed: use k2_core::hosts."
printf 'fn clean() {}\n' > "$T/src-tauri/src/main.rs"

# Another zone or a look-alike is not this gate's business.
printf 'fn o() -> &str { "acme.example.com k2xdev k2.devs" }\n' > "$T/crates/a/src/other.rs"
run 1 "crates/a/src/other.rs:1"
printf 'fn o() -> &str { "acme.example.com k2xdev" }\n' > "$T/crates/a/src/other.rs"
run 0 "zone-gate: ok (2 lines"

# A file that got better: the check says so, --tighten lowers the row, and
# a file at zero loses its row.
sed -i.bak '3d' "$T/crates/a/src/old.rs" && rm "$T/crates/a/src/old.rs.bak"
run 0 "1 file(s) are below their ceiling"
run 0 "ceilings tightened: 1 lines in 1 files." --tighten
[ "$(cat "$T/c.txt")" = "$(printf '# a comment\n\n1 crates/a/src/old.rs')" ] || { echo "FAIL: --tighten wrote: $(cat "$T/c.txt")" >&2; exit 1; }
pass=$((pass + 1))
printf 'fn none() {}\n' > "$T/crates/a/src/old.rs"
run 0 "ceilings tightened: 0 lines in 0 files." --tighten
[ "$(cat "$T/c.txt")" = "$(printf '# a comment\n')" ] || { echo "FAIL: an empty list wrote: $(cat "$T/c.txt")" >&2; exit 1; }
pass=$((pass + 1))
run 0 "zone-gate: ok (0 lines in 0 files"

# A bad ceilings file stops the gate with the line.
printf '# c\nmany crates/a/src/old.rs\n' > "$T/c.txt"
run 1 "line 2: want '<lines> <path>'"
printf '# c\n1 crates/a/src/old.rs\n2 crates/a/src/old.rs\n' > "$T/c.txt"
run 1 "line 3: crates/a/src/old.rs is listed twice"
run 1 "unknown argument: --nope" --nope

# The real tree passes, and the hosts module really holds the zone.
out="$(python3 "$GATE")" || { echo "FAIL: the real tree: $out" >&2; exit 1; }
case "$out" in *"zone-gate: ok ("*) ;; *) echo "FAIL: the real tree: $out" >&2; exit 1 ;; esac
pass=$((pass + 1))
grep -q '^pub const ZONE: &str = "k2.dev";$' "$HERE/crates/k2-client/src/hosts.rs" || { echo "FAIL: crates/k2-client/src/hosts.rs has no ZONE" >&2; exit 1; }
pass=$((pass + 1))

echo "zone-gate tests: $pass passed"
