#!/usr/bin/env bash
# Tests for scripts/k2-home-dot-gate.py (the .k2 half of k2-home-gate.sh),
# against a small throwaway tree, then the real tree.
# Run: bash tests/garden/k2-home-dot-gate.sh
set -euo pipefail
HERE="$(cd "$(dirname "$0")/../.." && pwd)"
GATE="$HERE/scripts/k2-home-dot-gate.py"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
pass=0

# run <want exit> <want text> <args...>: the exit code and the text must both match.
run() {
    local want="$1" text="$2" out code; shift 2
    set +e; out="$(python3 "$GATE" --root "$T" --allowlist "$T/allow.txt" "$@" 2>&1)"; code=$?; set -e
    if [ "$code" != "$want" ]; then echo "FAIL: exit $code, wanted $want: $out" >&2; exit 1; fi
    case "$out" in *"$text"*) ;; *) echo "FAIL: output lacks '$text': $out" >&2; exit 1 ;; esac
    pass=$((pass + 1))
}
# lacks <text> <args...>: the output must not have the text.
lacks() {
    local text="$1" out; shift
    out="$(python3 "$GATE" --root "$T" --allowlist "$T/allow.txt" "$@" 2>&1 || true)"
    case "$out" in *"$text"*) echo "FAIL: output has '$text': $out" >&2; exit 1 ;; esac
    pass=$((pass + 1))
}

mkdir -p "$T/crates/a/src" "$T/crates/a/tests" "$T/crates/k2-client/src" "$T/src-tauri/src" "$T/crates/a/target"
cat > "$T/crates/a/src/old.rs" <<'RS'
// home.join(".k2") in a comment is not counted
/// nor in a doc comment: root.join(".k2")
fn a(home: &Path) -> PathBuf { home.join(".k2").join("x") }
fn b() -> bool { name == ".k2" } // a note about join(".k2")
fn c(root: &Path) -> PathBuf { root.join(".k2/heartbeats") }
fn d() -> &'static str { "see ~/.k2/wiki" }
fn e() -> &'static str { ".k2so" }
#[cfg(test)]
fn helper(h: &Path) -> PathBuf {
    h.join(".k2")
}
#[cfg(test)]
mod tests {
    fn t() {
        let j = r#"{"a": "}"#;
        let p = home.join(".k2");
    }
}
fn f(home: &Path) -> PathBuf { home.join(".k2") }
RS
printf 'pub const HOME_DIR_NAME: &str = ".k2";\nfn x(h: &Path) -> PathBuf { h.join(".k2") }\n' > "$T/crates/k2-client/src/paths.rs"
printf 'fn y(r: &Path) -> PathBuf { r.join(".k2") }\n' > "$T/crates/k2-client/src/workspace_dir.rs"
printf 'fn t() { let p = home.join(".k2"); }\n' > "$T/crates/a/tests/it.rs"
printf 'fn t() { let p = home.join(".k2"); }\n' > "$T/crates/a/src/old_tests.rs"
printf 'fn t() { let p = home.join(".k2"); }\n' > "$T/crates/a/target/built.rs"
printf 'fn clean() {}\n' > "$T/src-tauri/src/main.rs"

# What is counted: joins and the exact literal in code; not comments, other
# strings, test-only items, test files, build output or the two helpers.
run 0 "crates/a/src/old.rs:3: fn a(" --list
run 0 "crates/a/src/old.rs:4: fn b()" --list
run 0 "crates/a/src/old.rs:5: fn c(" --list
run 0 "crates/a/src/old.rs:19: fn f(" --list
for n in 1 2 6 7 9 10 11 15 16; do lacks "old.rs:$n:" --list; done
lacks "paths.rs" --list
lacks "workspace_dir.rs" --list
lacks "it.rs" --list
lacks "old_tests.rs" --list
lacks "built.rs" --list

# Nothing listed: every counted line fails, naming the file and line.
: > "$T/allow.txt"
run 1 "k2-home-gate VIOLATION: crates/a/src/old.rs:3: fn a("
run 1 "Use k2_core::paths::k2_home()"

# Each line listed with a reason: clean, and the count of entries is shown.
cat > "$T/allow.txt" <<'TXT'
# a comment

crates/a/src/old.rs :: fn a( :: a reason
crates/a/src/old.rs :: name == ".k2" :: a reason
crates/a/src/old.rs :: root.join(".k2/heartbeats") :: a reason
crates/a/src/old.rs :: fn f( :: a reason
TXT
run 0 "k2-home-gate: .k2 clean (4 allowlist entries)"

# A new line in a listed file fails; the entry must match its text.
printf 'fn g(home: &Path) -> PathBuf { home.join(".k2") }\n' >> "$T/crates/a/src/old.rs"
run 1 "VIOLATION: crates/a/src/old.rs:20: fn g("
lacks "VIOLATION: crates/a/src/old.rs:3:"
printf 'crates/a/src/old.rs :: fn g( :: a reason\n' >> "$T/allow.txt"
run 0 "clean (5 allowlist entries)"

# An entry that matches nothing fails: the list only shrinks.
printf 'crates/a/src/gone.rs :: join(".k2") :: was here\n' >> "$T/allow.txt"
run 1 "STALE ENTRY: $T/allow.txt line 8: crates/a/src/gone.rs :: join(\".k2\") matches nothing; remove it"
sed -i.bak '$d' "$T/allow.txt"
run 0 "clean (5 allowlist entries)"

# An entry without a reason is refused.
printf 'crates/a/src/old.rs :: fn a(\n' >> "$T/allow.txt"
run 1 "want '<path> :: <text> :: <reason>'"
sed -i.bak '$d' "$T/allow.txt"
printf 'crates/a/src/old.rs :: fn a( :: \n' >> "$T/allow.txt"
run 1 "want '<path> :: <text> :: <reason>'"
sed -i.bak '$d' "$T/allow.txt"

# The real tree passes against the committed list.
out="$(python3 "$GATE" 2>&1)" || { echo "FAIL: the real tree: $out" >&2; exit 1; }
pass=$((pass + 1))

echo "k2-home-dot-gate: $pass passed"
