#!/usr/bin/env bash
# Tests for scripts/garden/cli-name-flip.py: its classes on a small made-up
# part, then the real rewrite on a COPY of cli/src and cli/text, which must
# still build, parse and compile. The real tree is never rewritten.
# Run: bash tests/garden/cli-name-flip.sh
set -euo pipefail
HERE="$(cd "$(dirname "$0")/../.." && pwd)"
FLIP="$HERE/scripts/garden/cli-name-flip.py"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
pass=0
ok() { pass=$((pass + 1)); }
die() { echo "FAIL: $1" >&2; exit 1; }

# ── a made-up tree ──────────────────────────────────────────────────────
S="$T/small"
mkdir -p "$S/cli/src/schema"
cat > "$S/cli/src/router.sh" <<'PART'
case "$1" in
    msg|send) cmd_msg "$@" ;;
    vault) cmd_vault "$@" ;;
esac
PART
cat > "$S/cli/src/a.sh" <<'PART'
# k2 msg sends a message (a comment)
cmd_a() {
    echo "Usage: k2 msg <addr> [text]" >&2
    echo "Run k2 --help or \`k2\` alone." >&2
    echo "[k2 sidecar] You are a sidecar"
    echo "--identity is k2 or standalone"
    MODE="k2"
    bin="$(command -v k2 || true)"
    out="$(k2 vault get "$name")"
    tok="$(mint "see k2 vault list" "$id")"; echo "after: k2 msg works"
    cat <<'HELP'
k2 vault get NAME     read a secret
Example: eval "$(k2 vault env)"
K2 is not k2.dev, k2so, k2skn_, $K2_PORT, ~/.k2, cli/k2 or dev.k2.daemon
HELP
    python3 - <<'PY'
# k2 msg in a python comment
k2 = load()
print("try: k2 msg <addr>", k2)
who = {"k2": "K2"}
PY
    python3 -c 'import sys; k2 = 1; print("use k2 vault list")'
    [ -n "$node" ] || { echo "usage: k2 vault get <name> [--node <node>]" >&2; exit 2; }
    local node="" pol_usage='usage: k2 msg <addr>'
    python3 -c "
# k2 msg in a comment inside python -c
x = 1  # k2 vault list, after code
print(f\"it stays off until: k2 msg {x}\")
k2 = [x]
print(k2)
"
    node -e 'const a = 1; console.log(k2, "use k2 msg <addr>")'
}
PART
printf '    {"name": "msg", "usage": "k2 msg <addr>", "note": "the k2 object"},\n' > "$S/cli/src/schema/talk.entries"

show() { python3 "$FLIP" --root "$S" --show "$1"; }
has() { case "$(show "$1")" in *"$2"*) ok ;; *) die "class $1 lacks: $2 :: $(show "$1")" ;; esac; }
hasnt() { case "$(show "$1")" in *"$2"*) die "class $1 has: $2" ;; *) ok ;; esac; }

has comment 'a.sh:1:'
has comment 'a.sh:17:'
has text 'a.sh:3: [printed text] echo "Usage: k2 msg'
has text 'a.sh:4:'
has kept 'a.sh:5: [a [k2 ...] prefix or stamp]'
has unsure 'a.sh:6: [k2 followed by a word that is not a command]'
# A string that is exactly k2, and k2 as a word in shell code, are kept:
# the brand gate holds each such line to a value row with its class.
has kept 'a.sh:7: [a string that is exactly k2 (a value)]'
has kept 'a.sh:8: [k2 as a word in shell code]'
has kept 'a.sh:9: [k2 as a word in shell code]'
for n in 7 8 9; do hasnt unsure "a.sh:$n:"; done
# A string inside $( ) inside a string is still read right, and so is the
# rest of the line after it.
has text 'a.sh:10:'
[ "$(show text | grep -c '^cli/src/a.sh:10:')" = 2 ] || die "line 10 should have two text uses: $(show text)"; ok
has text 'a.sh:12: [printed text] k2 vault get NAME'
# In a quoted heredoc nothing runs: $(k2 ...) there is an example, so text.
has text 'a.sh:13:'
# Names that only contain k2 are not uses of the word at all.
for c in text comment kept unsure; do hasnt "$c" 'a.sh:14:'; done
# Interpreter code: an identifier is never text; a sentence in a string is.
# A name on the left of `=` is kept; a bare use of it stays unsure.
has kept 'a.sh:18: [a name in interpreter code (k2 = ...)]'
has text 'a.sh:19: [string in interpreter code]'
[ "$(show unsure | grep -c '^cli/src/a.sh:19:')" = 1 ] || die "line 19: the identifier k2 should be unsure: $(show unsure)"; ok
has unsure 'a.sh:19: [interpreter code, not in a string]'
has kept 'a.sh:20: [a string that is exactly k2 (a value)]'
has kept 'a.sh:22: [a name in interpreter code (k2 = ...)]'
has text 'a.sh:22: [string in interpreter code]'
# A shell variable, a flag or an assignment named node is not the Node
# interpreter: the usage line after it is printed text.
has text 'a.sh:23: [printed text] [ -n "$node" ] || { echo "usage: k2 vault get'
has text 'a.sh:24: [printed text] local node=""'
# Inside python -c "...": a # comment is a comment, a print whose quotes
# the shell escapes (\") is a string, a name set with = is kept, and a bare
# use is still code.
has comment 'a.sh:26: [comment in interpreter code]'
has comment 'a.sh:27: [comment in interpreter code] x = 1  # k2 vault list'
has text 'a.sh:28: [string in interpreter code] print(f\"it stays off'
has kept 'a.sh:29: [a name in interpreter code (k2 = ...)]'
has unsure 'a.sh:30: [interpreter code, not in a string] print(k2)'
# A real node -e program: the bare k2 is code, the sentence is text.
has unsure 'a.sh:32: [interpreter code, not in a string] node -e'
has text 'a.sh:32: [string in interpreter code] node -e'
for c in text comment kept unsure; do hasnt "$c" 'a.sh:31:'; done
has text 'schema/talk.entries:1:'
has unsure 'schema/talk.entries:1: [k2 followed by a word that is not a command]'

out="$(python3 "$FLIP" --root "$S")"
case "$out" in *"will change: text 14, comment 4"*) ok ;; *) die "the dry run counts: $out" ;; esac
case "$out" in *"will not:    kept 8, unsure 5"*) ok ;; *) die "the dry run counts: $out" ;; esac

# The dry run and --show change nothing.
before="$(cat "$S/cli/src/a.sh" "$S/cli/src/schema/talk.entries" | cksum)"
python3 "$FLIP" --root "$S" >/dev/null; python3 "$FLIP" --root "$S" --unsure >/dev/null
[ "$(cat "$S/cli/src/a.sh" "$S/cli/src/schema/talk.entries" | cksum)" = "$before" ] || die "a dry run changed a file"; ok

# --apply rewrites text and comments, and nothing else.
python3 "$FLIP" --apply --root "$S" | grep -q "rewrote 18 uses of k2 to k5 in 2 files" || die "--apply said: $(python3 "$FLIP" --apply --root "$S")"; ok
line() { sed -n "${1}p" "$S/cli/src/a.sh"; }
[ "$(line 1)" = '# k5 msg sends a message (a comment)' ] || die "line 1: $(line 1)"; ok
[ "$(line 3)" = '    echo "Usage: k5 msg <addr> [text]" >&2' ] || die "line 3: $(line 3)"; ok
[ "$(line 4)" = '    echo "Run k5 --help or \`k5\` alone." >&2' ] || die "line 4: $(line 4)"; ok
[ "$(line 5)" = '    echo "[k2 sidecar] You are a sidecar"' ] || die "line 5: $(line 5)"; ok
[ "$(line 6)" = '    echo "--identity is k2 or standalone"' ] || die "line 6: $(line 6)"; ok
[ "$(line 7)" = '    MODE="k2"' ] || die "line 7: $(line 7)"; ok
[ "$(line 8)" = '    bin="$(command -v k2 || true)"' ] || die "line 8: $(line 8)"; ok
[ "$(line 9)" = '    out="$(k2 vault get "$name")"' ] || die "line 9: $(line 9)"; ok
[ "$(line 13)" = 'Example: eval "$(k5 vault env)"' ] || die "line 13: $(line 13)"; ok
[ "$(line 14)" = 'K2 is not k2.dev, k2so, k2skn_, $K2_PORT, ~/.k2, cli/k2 or dev.k2.daemon' ] || die "line 14: $(line 14)"; ok
[ "$(line 18)" = 'k2 = load()' ] || die "line 18: $(line 18)"; ok
[ "$(line 19)" = 'print("try: k5 msg <addr>", k2)' ] || die "line 19: $(line 19)"; ok
[ "$(line 22)" = "    python3 -c 'import sys; k2 = 1; print(\"use k5 vault list\")'" ] || die "line 22: $(line 22)"; ok
[ "$(line 23)" = '    [ -n "$node" ] || { echo "usage: k5 vault get <name> [--node <node>]" >&2; exit 2; }' ] || die "line 23: $(line 23)"; ok
[ "$(line 24)" = "    local node=\"\" pol_usage='usage: k5 msg <addr>'" ] || die "line 24: $(line 24)"; ok
[ "$(line 26)" = '# k5 msg in a comment inside python -c' ] || die "line 26: $(line 26)"; ok
[ "$(line 27)" = 'x = 1  # k5 vault list, after code' ] || die "line 27: $(line 27)"; ok
[ "$(line 28)" = 'print(f\"it stays off until: k5 msg {x}\")' ] || die "line 28: $(line 28)"; ok
[ "$(line 29)" = 'k2 = [x]' ] || die "line 29: $(line 29)"; ok
[ "$(line 30)" = 'print(k2)' ] || die "line 30: $(line 30)"; ok
[ "$(line 32)" = "    node -e 'const a = 1; console.log(k2, \"use k5 msg <addr>\")'" ] || die "line 32: $(line 32)"; ok
bash -n "$S/cli/src/a.sh" || die "the rewritten part does not parse"; ok
# A second run finds no text left to change.
python3 "$FLIP" --root "$S" | grep -q "will change: text 0, comment 0" || die "a second run still finds text: $(python3 "$FLIP" --root "$S")"; ok

# ── cli/text: printed pages, read where the folder exists ───────────────
mkdir -p "$S/cli/text/help"
printf 'Send one: k2 msg <addr> "text"\nThe mode is "k2" or off.\nk2.dev and ~/.k2 are not the word.\n' > "$S/cli/text/help/talk.txt"
has text 'help/talk.txt:1: [printed text]'
has kept 'help/talk.txt:2: [a string that is exactly k2 (a value)]'
for c in text comment kept unsure; do hasnt "$c" 'help/talk.txt:3:'; done
out="$(python3 "$FLIP" --root "$S")"
# (a.sh is already rewritten above: the page's line is the only text left.)
case "$out" in *"in 3 files of cli/src and cli/text"*"will change: text 1, comment 0"*) ok ;; *) die "the dry run with cli/text: $out" ;; esac
python3 "$FLIP" --apply --root "$S" | grep -q "rewrote 1 uses of k2 to k5 in 1 files" || die "--apply on cli/text: $(python3 "$FLIP" --root "$S")"; ok
[ "$(sed -n 1p "$S/cli/text/help/talk.txt")" = 'Send one: k5 msg <addr> "text"' ] || die "cli/text line 1: $(sed -n 1p "$S/cli/text/help/talk.txt")"; ok
[ "$(sed -n 2p "$S/cli/text/help/talk.txt")" = 'The mode is "k2" or off.' ] || die "cli/text line 2: $(sed -n 2p "$S/cli/text/help/talk.txt")"; ok
rm -r "$S/cli/text"

# ── Rust: a made-up crate ───────────────────────────────────────────────
mkdir -p "$S/crates/a/src"
cat > "$S/crates/a/src/lib.rs" <<'RS'
//! Run `k2 msg` to send (a doc comment).
/* k2 vault in a block comment /* nested k2 msg */ still comment */
pub const HINT: &str = "Ask with: k2 msg <addr>";
pub const TAG: &str = "k2";
pub const RAW: &str = r#"say "k2 vault list" here"#;
fn f<'a>(k2: &'a str) -> String { let q = '"'; format!("{k2} is not k2 msg {q}") }
fn g() { log("[k2 sidecar] up"); let s = "the k2 object"; let b = b"k2 msg"; let u = "k2.dev k2so K2_PORT"; }
RS
rshow() { python3 "$FLIP" --area rust --root "$S" --show "$1"; }
rhas() { case "$(rshow "$1")" in *"$2"*) ok ;; *) die "rust class $1 lacks: $2 :: $(rshow "$1")" ;; esac; }
rhas comment 'lib.rs:1:'
[ "$(rshow comment | grep -c 'lib.rs:2:')" = 2 ] || die "both uses in the nested block comment are comments: $(rshow comment)"; ok
rhas text 'lib.rs:3: [string literal]'
rhas unsure 'lib.rs:4: [a string that is exactly k2 (a value)]'
rhas text 'lib.rs:5: [string literal]'
# A lifetime and a char literal holding a quote do not confuse the reader:
# {k2} in the format string is a value, the sentence after it is text.
rhas unsure 'lib.rs:6: [k2 followed by a word that is not a command]'
rhas text 'lib.rs:6: [string literal]'
rhas kept 'lib.rs:7: [a [k2 ...] prefix or stamp]'
rhas unsure 'lib.rs:7: [k2 followed by a word that is not a command]'
rhas text 'lib.rs:7: [string literal]'
out="$(python3 "$FLIP" --area rust --root "$S")"
case "$out" in *"will change: text 4, comment 3"*) ok ;; *) die "the rust dry run counts: $out" ;; esac
case "$out" in *"will not:    kept 1, unsure 3"*) ok ;; *) die "the rust dry run counts: $out" ;; esac
python3 "$FLIP" --area rust --apply --root "$S" | grep -q "rewrote 7 uses of k2 to k5 in 1 files" || die "rust --apply said: $(python3 "$FLIP" --area rust --root "$S")"; ok
rline() { sed -n "${1}p" "$S/crates/a/src/lib.rs"; }
[ "$(rline 3)" = 'pub const HINT: &str = "Ask with: k5 msg <addr>";' ] || die "rust line 3: $(rline 3)"; ok
[ "$(rline 4)" = 'pub const TAG: &str = "k2";' ] || die "rust line 4: $(rline 4)"; ok
[ "$(rline 5)" = 'pub const RAW: &str = r#"say "k5 vault list" here"#;' ] || die "rust line 5: $(rline 5)"; ok
[ "$(rline 6)" = "fn f<'a>(k2: &'a str) -> String { let q = '\"'; format!(\"{k2} is not k5 msg {q}\") }" ] || die "rust line 6: $(rline 6)"; ok
[ "$(rline 7)" = 'fn g() { log("[k2 sidecar] up"); let s = "the k2 object"; let b = b"k5 msg"; let u = "k2.dev k2so K2_PORT"; }' ] || die "rust line 7: $(rline 7)"; ok

# ── refusals ────────────────────────────────────────────────────────────
set +e
out="$(python3 "$FLIP" --apply 2>&1)"; code=$?
set -e
[ "$code" = 1 ] && case "$out" in *"--apply needs --root DIR"*) ok ;; *) die "--apply with no root: exit $code: $out" ;; esac
set +e
out="$(env -u CLI_NAME_FLIP_THIS_TREE python3 "$FLIP" --apply --root "$HERE" 2>&1)"; code=$?
set -e
[ "$code" = 1 ] && case "$out" in *"on this tree itself is refused"*) ok ;; *) die "--apply on the real tree: exit $code: $out" ;; esac
set +e
out="$(python3 "$FLIP" --nope 2>&1)"; code=$?
set -e
[ "$code" = 1 ] && case "$out" in *"usage: cli-name-flip.py"*) ok ;; *) die "bad argument: exit $code: $out" ;; esac

# A cli/text file is the body of a printed, quoted heredoc: all text, and
# --apply rewrites it like a heredoc in a part.
X="$T/text"
mkdir -p "$X/cli/src" "$X/cli/text/study"
printf 'case "$1" in\n    msg) cmd_msg "$@" ;;\nesac\n' > "$X/cli/src/router.sh"
printf 'Run k2 msg <addr> to send.\nExample: eval "$(k2 msg --json)"\n[k2 sidecar] a stamp\n' > "$X/cli/text/study/a.txt"
xshow() { python3 "$FLIP" --root "$X" --show "$1"; }
[ "$(xshow text | grep -c '^cli/text/study/a.txt:[12]: \[printed text\]')" = 2 ] || die "the text file's text uses: $(xshow text)"; ok
xshow kept | grep -q '^cli/text/study/a.txt:3: ' || die "the text file's stamp: $(xshow kept)"; ok
python3 "$FLIP" --apply --root "$X" | grep -q "rewrote 2 uses of k2 to k5 in 1 files" || die "--apply on the text file"; ok
[ "$(cat "$X/cli/text/study/a.txt")" = "$(printf 'Run k5 msg <addr> to send.\nExample: eval "$(k5 msg --json)"\n[k2 sidecar] a stamp')" ] || die "the rewritten text file: $(cat "$X/cli/text/study/a.txt")"; ok

# ── the real parts, rewritten in a copy ─────────────────────────────────
C="$T/copy"
mkdir -p "$C/cli" "$C/scripts" "$C/tests"
cp -R "$HERE/cli/src" "$C/cli/src"
cp -R "$HERE/cli/text" "$C/cli/text"
cp "$HERE/scripts/build-cli.sh" "$C/scripts/"
cp "$HERE/tests/check-embedded-python.sh" "$C/tests/"
out="$(python3 "$FLIP" --root "$C")"
case "$out" in *"the reader lost its place"*) die "the reader lost its place in a real part: $out" ;; esac; ok
# (file, line, reason) of every unsure use; the line's text may change when
# a text use shares the line.
unsure_list() { python3 "$FLIP" --root "$C" --unsure | sed 's/\] .*/]/' | cksum; }
unsure_before="$(unsure_list)"
python3 "$FLIP" --apply --root "$C" | grep -qE "rewrote [0-9]{4,} uses of k2 to k5 in [0-9]+ files" || die "--apply on the copy rewrote too little"; ok
# The uses it would not touch are still there, each still k2.
[ "$(unsure_list)" = "$unsure_before" ] || die "--apply changed a use it called unsure"; ok
python3 "$FLIP" --root "$C" | grep -q "will change: text 0, comment 0" || die "text is left after --apply: $(python3 "$FLIP" --root "$C")"; ok
# The rewritten parts still join, parse as bash, and every embedded Python
# block still compiles.
bash "$C/scripts/build-cli.sh" --root "$C" >/dev/null || die "the rewritten parts do not build"; ok
bash -n "$C/cli/k2" || die "the rewritten cli/k2 does not parse"; ok
out="$(bash "$C/tests/check-embedded-python.sh" 2>&1)" || die "embedded Python in the rewritten CLI: $(printf '%s' "$out" | tail -5)"
case "$out" in *"OK: every embedded Python block compiles."*) ok ;; *) die "embedded Python check: $(printf '%s' "$out" | tail -3)" ;; esac
# The real Rust sources: the dry run reads all of them and finds text.
out="$(python3 "$FLIP" --area rust)"
case "$out" in *"files of crates and src-tauri/src"*) ok ;; *) die "the real rust dry run: $out" ;; esac
n="$(printf '%s\n' "$out" | sed -n 's/.*will change: text \([0-9]*\),.*/\1/p')"
[ "$n" -gt 1000 ] || die "the real rust dry run found only $n text uses"; ok
# The real tree was not touched by any of this.
bash "$HERE/scripts/build-cli.sh" --check >/dev/null || die "the real cli/k2 is no longer the join of its parts"; ok

echo "cli-name-flip tests: $pass passed"
