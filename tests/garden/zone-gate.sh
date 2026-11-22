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
# A `#[cfg(test)] mod name;` only declares a test file: the code after it
# still counts. A later inline test module still ends the count.
printf '#[cfg(test)]\nmod decl_tests;\nfn d() -> &str { "acme.k2.dev" }\n#[cfg(test)]\nmod tests {\n    fn t() { let h = "b.k2.dev"; }\n}\n' > "$T/crates/a/src/decl.rs"
run 0 "crates/a/src/decl.rs:3: fn d()" --list
lacks "decl.rs:6" --list
rm "$T/crates/a/src/decl.rs"

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

# ── the renderer area ───────────────────────────────────────────────────
mkdir -p "$T/src/renderer/lib" "$T/src/renderer/components" "$T/src/renderer/__tests__"
printf 'export const ZONE = "k2.dev"\n' > "$T/src/renderer/lib/hosts.ts"
cat > "$T/src/renderer/components/A.tsx" <<'TS'
// k2.dev in a comment is not counted
/** nor in a doc comment: acme.k2.dev */
const url = `https://${sub}.k2.dev`
export function A() {
  return (
    <p>
      {/* k2.dev in a JSX comment is not counted */}
      Your k2.dev account
    </p>
  )
}
TS
printf 'it("x", () => expect(h).toBe("a.k2.dev"))\n' > "$T/src/renderer/components/A.test.tsx"
printf 'export const FAR = "far@other.k2.dev"\n' > "$T/src/renderer/components/a-fixture.ts"
printf 'const h = "a.k2.dev"\n' > "$T/src/renderer/__tests__/b.ts"
printf '# c\n\n2 src/renderer/components/A.tsx\n' > "$T/r.txt"
rrun() {
    local want="$1" text="$2" out code; shift 2
    set +e; out="$(python3 "$GATE" --area renderer --root "$T" --ceilings "$T/r.txt" "$@" 2>&1)"; code=$?; set -e
    if [ "$code" != "$want" ]; then echo "FAIL: renderer exit $code, wanted $want: $out" >&2; exit 1; fi
    case "$out" in *"$text"*) ;; *) echo "FAIL: renderer output lacks '$text': $out" >&2; exit 1 ;; esac
    pass=$((pass + 1))
}
# Code and copy are counted; comments, test files, fixtures, test folders
# and the renderer's hosts module are not. The Rust tree is not looked at.
rrun 0 "zone-gate: ok (2 lines in 1 files still spell out k2.dev)."
rrun 0 'src/renderer/components/A.tsx:3: const url' --list
rrun 0 'src/renderer/components/A.tsx:8: Your k2.dev account' --list
out="$(python3 "$GATE" --area renderer --root "$T" --ceilings "$T/r.txt" --list)"
case "$out" in *A.tsx:1:*|*A.tsx:2:*|*A.tsx:7:*|*A.test.tsx*|*a-fixture*|*__tests__*|*lib/hosts.ts*|*crates/*) echo "FAIL: the renderer list has a line it should not: $out" >&2; exit 1 ;; esac
pass=$((pass + 1))
# A new line in an unlisted renderer file fails and names the module to use.
printf 'export const h = `${x}.k2.dev`\n' > "$T/src/renderer/components/B.ts"
rrun 1 "src/renderer/components/B.ts:1 spells out k2.dev and the file is not listed: use @/lib/hosts."
rm "$T/src/renderer/components/B.ts"
# One more line in a listed file fails with both counts.
printf 'const more = "relay.k2.dev"\n' >> "$T/src/renderer/components/A.tsx"
rrun 1 "src/renderer/components/A.tsx has 3 lines with k2.dev, its ceiling is 2: use @/lib/hosts for the new one."
# An unknown area is refused.
set +e; out="$(python3 "$GATE" --area nope 2>&1)"; code=$?; set -e
[ "$code" = 1 ] && case "$out" in *"unknown argument: --area"*) pass=$((pass + 1)) ;; *) echo "FAIL: --area nope: $out" >&2; exit 1 ;; esac

# The real tree passes, and the hosts module really holds the zone.
out="$(python3 "$GATE")" || { echo "FAIL: the real tree: $out" >&2; exit 1; }
case "$out" in *"zone-gate: ok ("*) ;; *) echo "FAIL: the real tree: $out" >&2; exit 1 ;; esac
pass=$((pass + 1))
# ZONE is the primary zone under its older name, so the value sits on PRIMARY_ZONE.
grep -q '^pub const PRIMARY_ZONE: &str = "k2.dev";$' "$HERE/crates/k2-client/src/hosts.rs" || { echo "FAIL: crates/k2-client/src/hosts.rs has no PRIMARY_ZONE = k2.dev" >&2; exit 1; }
pass=$((pass + 1))
grep -q '^pub const ZONE: &str = PRIMARY_ZONE;$' "$HERE/crates/k2-client/src/hosts.rs" || { echo "FAIL: crates/k2-client/src/hosts.rs: ZONE is not PRIMARY_ZONE" >&2; exit 1; }
pass=$((pass + 1))
out="$(python3 "$GATE" --area renderer)" || { echo "FAIL: the real renderer tree: $out" >&2; exit 1; }
case "$out" in *"zone-gate: ok ("*) ;; *) echo "FAIL: the real renderer tree: $out" >&2; exit 1 ;; esac
pass=$((pass + 1))
grep -q "^export const ZONE = 'k2.dev'$" "$HERE/src/renderer/lib/hosts.ts" || { echo "FAIL: src/renderer/lib/hosts.ts has no ZONE" >&2; exit 1; }
pass=$((pass + 1))

echo "zone-gate tests: $pass passed"
