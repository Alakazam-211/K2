#!/usr/bin/env bash
# Tests for scripts/garden/k2-literal-gate.py: a small throwaway git tree,
# then a copy of the real crates with one literal planted and one removed,
# then the real tree.
# Run: bash tests/garden/k2-literal-gate.sh
set -euo pipefail
HERE="$(cd "$(dirname "$0")/../.." && pwd)"
GATE="$HERE/scripts/garden/k2-literal-gate.py"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
export PYTHONDONTWRITEBYTECODE=1
pass=0
R="$T/tree"

# run <want exit> <want text> <args...>: the exit code and the text must both match.
run() {
    local want="$1" text="$2" out code; shift 2
    set +e; out="$(python3 "$GATE" --root "$R" --ceilings "$R/c.txt" "$@" 2>&1)"; code=$?; set -e
    if [ "$code" != "$want" ]; then echo "FAIL: exit $code, wanted $want: $out" >&2; exit 1; fi
    case "$out" in *"$text"*) ;; *) echo "FAIL: output lacks '$text': $out" >&2; exit 1 ;; esac
    pass=$((pass + 1))
}
# lacks <text> <args...>: the output must not have the text.
lacks() {
    local text="$1" out; shift
    out="$(python3 "$GATE" --root "$R" --ceilings "$R/c.txt" "$@" 2>&1 || true)"
    case "$out" in *"$text"*) echo "FAIL: output has '$text': $out" >&2; exit 1 ;; esac
    pass=$((pass + 1))
}
# ceilings <rows...>: the ceilings file, after a header line.
ceilings() { { printf '# ceilings\n\n'; for r in "$@"; do printf '%s\n' "$r"; done; } > "$R/c.txt"; }
g() { git -C "$R" -c user.name=t -c user.email=t@example.invalid -c commit.gpgsign=false "$@" >/dev/null; }

mkdir -p "$R/crates/k2-daemon/src/apps" "$R/crates/k2-daemon/src/tests" "$R/crates/k2-core/src/skills" "$R/crates/k2-client/src"
cat > "$R/crates/k2-daemon/src/apps/deploy.rs" <<'RS'
#[cfg(test)]
mod deploy_tests;
// k2 app deploy in a comment
/// k2 app deploy in a doc comment
fn a() -> &'static str { "run k2 app deploy <app>" }
fn b() -> u8 { 1 } // k2 app logs in a tail comment
fn c() -> &'static str { "first line \
    then k2 app list on the next" }
fn d() { log_warn!("k2 app crashed"); }
fn e() {
    eprintln!(
        "the k2 app runtime stopped"
    );
}
fn f() -> &'static str { "k2.dev xk2 app k2-node app K2 Connect" }
#[cfg(all(test, unix))]
mod tests {
    fn t() { assert_eq!(x, "k2 app deploy"); }
}
RS
printf 'fn t() { let _ = "k2 app deploy"; }\n' > "$R/crates/k2-daemon/src/apps/deploy_tests.rs"
printf 'fn t() { let _ = "k2 app deploy"; }\n' > "$R/crates/k2-daemon/src/tests/it.rs"
printf 'pub const S: &str = "k2 msg <addr>";\n' > "$R/crates/k2-core/src/skills/content.rs"
printf 'fn clean() {}\n' > "$R/crates/k2-client/src/lib.rs"
ceilings "skip crates/k2-core/src/skills/content.rs skill files written for agents" "" "2 apps"
g init -q; g add -A; g commit -q -m seed

# Two lines count: a string on one line, and a string's second line. Not
# comments, a tail comment, log lines, look-alikes, test modules and files,
# or a skipped file. A `mod name;` declaration does not end the count.
run 0 "k2-literal-gate: ok (2 lines in 1 families still type 'k2 <word>' in text for a person; 1 paths skipped, 0 lines kept)."
run 0 "apps: crates/k2-daemon/src/apps/deploy.rs:5: fn a()" --list
run 0 "apps: crates/k2-daemon/src/apps/deploy.rs:8: then k2 app list" --list apps
for n in 3 4 6 9 12 15 18; do lacks "deploy.rs:$n:" --list; done
lacks "deploy_tests.rs" --list
lacks "it.rs" --list
lacks "content.rs" --list

# A `[k2 ...]` stamp is a tag, not the command; a file a `#[cfg(test)] mod
# name;` declares is test code whatever its name; a keep row holds back the
# lines of its file that hold its text, and fails when it holds back none.
mkdir -p "$R/crates/k2-daemon/src/sql/live"
printf '#[cfg(test)]\nmod live;\nfn s() -> String { format!("[k2 apps] {a} crashed; [k2 vault ask 1] asks") }\n' > "$R/crates/k2-daemon/src/sql/mod.rs"
printf 'fn t() -> &str { "the k2 schema is ours" }\n' > "$R/crates/k2-daemon/src/sql/live.rs"
printf 'fn t() -> &str { "and k2 db list" }\n' > "$R/crates/k2-daemon/src/sql/live/more.rs"
printf 'fn m() -> &str { "k2 row cap" }\nfn n() -> &str { "run k2 db list" }\n' > "$R/crates/k2-daemon/src/sql/query.rs"
ceilings "skip crates/k2-core/src/skills/content.rs skill files written for agents" "" "2 apps" "1 db"
run 1 "k2-literal-gate: FAIL db has 2 lines with a literal 'k2 <word>', its ceiling is 1"
ceilings "skip crates/k2-core/src/skills/content.rs skill files written for agents" 'keep crates/k2-daemon/src/sql/query.rs "k2 row cap" a marker found again in Postgres errors' "" "2 apps" "1 db"
run 0 "1 paths skipped, 1 lines kept)."
lacks "query.rs:1:" --list
lacks "mod.rs:3:" --list
lacks "live.rs" --list
lacks "more.rs" --list
run 0 "db: crates/k2-daemon/src/sql/query.rs:2: fn n()" --list db
ceilings "skip crates/k2-core/src/skills/content.rs skill files written for agents" 'keep crates/k2-daemon/src/sql/query.rs "no such text" a stale row' "" "2 apps" "1 db"
run 1 'keep crates/k2-daemon/src/sql/query.rs "no such text" holds back no line; drop the row'
ceilings "skip crates/k2-core/src/skills/content.rs skill files written for agents" 'keep crates/k2-daemon/src/sql/gone.rs "k2 x" gone' "" "2 apps" "1 db"
run 1 "keep crates/k2-daemon/src/sql/gone.rs: no such file; drop the row"
ceilings "skip crates/k2-core/src/skills/content.rs skill files written for agents" 'keep crates/k2-daemon/src/sql/query.rs k2 row cap' "" "2 apps"
run 1 "want 'keep <path> \"<text>\" <reason>'"
rm -r "$R/crates/k2-daemon/src/sql"
ceilings "skip crates/k2-core/src/skills/content.rs skill files written for agents" "" "2 apps"

# A new literal 'k2 msg' fails, names the family and prints the new line:
# against HEAD while it is not committed, against HEAD~1 once it is.
cp "$R/crates/k2-daemon/src/apps/deploy.rs" "$T/keep.rs"
{ printf 'fn n() -> &'"'"'static str { "ask with k2 msg k2" }\n'; cat "$T/keep.rs"; } > "$R/crates/k2-daemon/src/apps/deploy.rs"
run 1 "k2-literal-gate: FAIL apps has 3 lines with a literal 'k2 <word>', its ceiling is 2"
run 1 "new since HEAD: crates/k2-daemon/src/apps/deploy.rs:1: fn n()"
lacks "deploy.rs:5:"
g commit -q -am planted
run 1 "new since HEAD~1: crates/k2-daemon/src/apps/deploy.rs:1: fn n()"
run 1 "new since HEAD~1: crates/k2-daemon/src/apps/deploy.rs:1" --base HEAD~1
run 1 "no line is new since HEAD" --base HEAD
# --tighten never raises a ceiling, and leaves the file as it was.
cp "$R/c.txt" "$T/c.before"
run 1 "--tighten never raises a ceiling" --tighten
cmp -s "$R/c.txt" "$T/c.before" || { echo "FAIL: --tighten changed the ceilings on a failure" >&2; exit 1; }
pass=$((pass + 1))
cp "$T/keep.rs" "$R/crates/k2-daemon/src/apps/deploy.rs"

# A family with no row may have none: a k2-client line fails.
printf 'fn a() -> &'"'"'static str { "k2 msg" }\n' > "$R/crates/k2-client/src/lib.rs"
run 1 "FAIL k2-client has 1 lines with a literal 'k2 <word>', its ceiling is 0"
printf 'fn clean() {}\n' > "$R/crates/k2-client/src/lib.rs"

# Removing one passes with a note; --tighten lowers the ceiling to today's
# count, drops a row at zero, and keeps the skip rows.
printf 'fn a() -> &'"'"'static str { "run {cli} app deploy <app>" }\n' > "$T/a.rs"
python3 - "$R/crates/k2-daemon/src/apps/deploy.rs" "$T/a.rs" <<'PY'
import sys
lines = open(sys.argv[1]).read().split("\n")
lines[4] = open(sys.argv[2]).read().rstrip("\n")
open(sys.argv[1], "w").write("\n".join(lines))
PY
ceilings "skip crates/k2-core/src/skills/content.rs skill files written for agents" "" "2 apps" "4 mail"
run 0 "2 families are below the ceiling (apps, mail); run scripts/garden/k2-literal-gate.py --tighten"
run 0 "k2-literal-gate: apps 2 -> 1" --tighten
want="$(printf '# ceilings\n\nskip crates/k2-core/src/skills/content.rs skill files written for agents\n\n1 apps')"
[ "$(cat "$R/c.txt")" = "$want" ] || { echo "FAIL: tightened ceilings: $(cat "$R/c.txt")" >&2; exit 1; }
pass=$((pass + 1))
run 0 "k2-literal-gate: ok (1 lines in 1 families"
lacks "below the ceiling"

# Bad rows are errors, not passes: a skip for a path that is gone, a skip
# outside the three crates, a row with no count, a family twice.
ceilings "skip crates/k2-core/src/gone.rs a reason" "1 apps"
run 1 "skip crates/k2-core/src/gone.rs: no such file; drop the row"
ceilings "skip crates/k2-node/src/ x" "1 apps"
run 1 "crates/k2-node/src/ is not under"
ceilings "apps"
run 1 "want '<count> <family>' or 'skip <path> <reason>'"
ceilings "1 apps" "2 apps"
run 1 "apps is listed twice"

# A copy of the real crates (no git): plant one literal 'k2 msg' and it
# fails; turn one counted line into a template and --tighten lowers its
# family by one.
C="$T/real"
mkdir -p "$C/crates/k2-daemon" "$C/crates/k2-core" "$C/crates/k2-client" "$C/scripts/garden"
for k in k2-daemon k2-core k2-client; do cp -R "$HERE/crates/$k/src" "$C/crates/$k/src"; done
cp "$HERE/scripts/garden/k2-literal-ceilings.txt" "$C/c.txt"
crun() {
    local want="$1" text="$2" out code; shift 2
    set +e; out="$(python3 "$GATE" --root "$C" --ceilings "$C/c.txt" "$@" 2>&1)"; code=$?; set -e
    if [ "$code" != "$want" ]; then echo "FAIL: real copy: exit $code, wanted $want: $out" >&2; exit 1; fi
    case "$out" in *"$text"*) ;; *) echo "FAIL: real copy: output lacks '$text': $out" >&2; exit 1 ;; esac
    pass=$((pass + 1))
}
crun 0 "k2-literal-gate: ok"
apps="$(awk '$2 == "apps" && NF == 2 { print $1 }' "$C/c.txt")"
[ -n "$apps" ] || { echo "FAIL: the real ceilings have no apps row" >&2; exit 1; }
f="$C/crates/k2-daemon/src/app_push/mod.rs"
{ printf 'pub const PLANTED: &str = "ask with k2 msg k2";\n'; cat "$HERE/crates/k2-daemon/src/app_push/mod.rs"; } > "$f"
crun 1 "FAIL apps has $((apps + 1)) lines with a literal 'k2 <word>', its ceiling is $apps"
crun 1 "git can't tell which lines are new; --list 'apps' prints them all"
cp "$HERE/crates/k2-daemon/src/app_push/mod.rs" "$f"
# The first counted apps line becomes a {cli} template.
python3 "$GATE" --root "$C" --ceilings "$C/c.txt" --list apps > "$T/apps.txt"
first="$(sed -n 1p "$T/apps.txt")"
loc="${first#apps: }"; loc="${loc%%: *}"; file="${loc%:*}"; line="${loc##*:}"
python3 - "$C/$file" "$line" <<'PY'
import re, sys
path, n = sys.argv[1], int(sys.argv[2]) - 1
lines = open(path, encoding="utf-8").read().split("\n")
new = re.sub(r"(^|[^A-Za-z0-9_./$-])k2 ([a-z])", r"\1{cli} \2", lines[n])
if new == lines[n]:
    sys.exit(f"no literal to turn into a template on {path}:{n + 1}")
lines[n] = new
open(path, "w", encoding="utf-8").write("\n".join(lines))
PY
crun 0 "family is below the ceiling (apps)"
crun 0 "k2-literal-gate: apps $apps -> $((apps - 1))" --tighten
[ "$(awk '$2 == "apps" && NF == 2 { print $1 }' "$C/c.txt")" = "$((apps - 1))" ] || { echo "FAIL: real copy: apps ceiling not lowered" >&2; exit 1; }
pass=$((pass + 1))
# Only that row moved.
moved="$(diff "$HERE/scripts/garden/k2-literal-ceilings.txt" "$C/c.txt" | grep '^[<>]' || true)"
[ "$moved" = "$(printf '< %s apps\n> %s apps' "$apps" "$((apps - 1))")" ] || { echo "FAIL: real copy: --tighten moved more than the apps row: $moved" >&2; exit 1; }
pass=$((pass + 1))

# The real tree is within its ceilings, through the script CI runs.
out="$(bash "$HERE/scripts/brand-gate.sh" 2>&1)" || { echo "FAIL: the real tree: $out" >&2; exit 1; }
case "$out" in *"k2-literal-gate: ok"*) ;; *) echo "FAIL: no k2-literal-gate ok line: $out" >&2; exit 1 ;; esac
pass=$((pass + 1))

echo "k2-literal-gate tests: $pass passed"
