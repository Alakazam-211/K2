#!/usr/bin/env bash
# Tests for scripts/garden/dispatcher-arms.py, against small throwaway
# files, then the real dispatcher.
# Run: bash tests/garden/dispatcher-arms.sh
set -euo pipefail
HERE="$(cd "$(dirname "$0")/../.." && pwd)"
TOOL="$HERE/scripts/garden/dispatcher-arms.py"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
pass=0

# run <want exit> <want text> <args...>: the exit code and the text must both match.
run() {
    local want="$1" text="$2" out code; shift 2
    set +e; out="$(python3 "$TOOL" "$@" 2>&1)"; code=$?; set -e
    if [ "$code" != "$want" ]; then echo "FAIL: exit $code, wanted $want ($*): $out" >&2; exit 1; fi
    case "$out" in *"$text"*) ;; *) echo "FAIL: output lacks '$text' ($*): $out" >&2; exit 1 ;; esac
    pass=$((pass + 1))
}
# is <want> <args...>: the whole output must be exactly this.
is() {
    local want="$1" out; shift
    out="$(python3 "$TOOL" "$@")" || { echo "FAIL: $* failed" >&2; exit 1; }
    if [ "$out" != "$want" ]; then echo "FAIL ($*): got" >&2; printf '%s\n' "$out" >&2; echo "wanted" >&2; printf '%s\n' "$want" >&2; exit 1; fi
    pass=$((pass + 1))
}

# ── A small match with everything that can fool a bracket counter.
cat > "$T/old.rs" <<'RS'
// match path.as_str() { in a comment is not the match
const NOT_IT: &str = "match path.as_str() {";
async fn handle(path: String) -> DispatchOutcome {
    let is_post = true;
    match path.as_str() {
        // a comment before the first arm, with => and { in it
        crate::pair::PAGE => {
            send("a brace } in a string, and => too").await;
        }
        "/ping" => {
            let c = '{';
            let q = '\'';
            let life: &'static str = r#"raw " } => "#;
            if c == '}' {
                return DispatchOutcome::Done;
            }
            send(life).await;
        }
        "/" | "/account" if !is_post => {
            send("root").await;
        }
        p if is_post
            // a comment inside a head
            && (p.starts_with("/cli/a/")
                || p == "/cli/b") =>
        {
            if p.len() > 3 {
                return DispatchOutcome::KeepAlive;
            }
            let v = vec![1, 2, 3];
            send(&format!("{v:?}")).await;
        }
        p if crate::x::is_route(p) => crate::x::serve(p, &[1, 2], (3, 4)).await,
        "/tail" => {
            send("tail").await;
        },
        _ => {
            send("404").await;
        }
    }
    DispatchOutcome::KeepAlive
}
fn after() { match 1 { _ => {} } }
RS

is 'crate::pair::PAGE
"/ping"
"/" | "/account" if !is_post
p if is_post && (p.starts_with("/cli/a/") || p == "/cli/b")
p if crate::x::is_route(p)
"/tail"
_' heads "$T/old.rs"

is '1	7	3	crate::pair::PAGE
2	10	9	"/ping"
3	19	3	"/" | "/account" if !is_post
4	22	11	p if is_post && (p.starts_with("/cli/a/") || p == "/cli/b")
5	33	1	p if crate::x::is_route(p)
6	34	3	"/tail"
7	37	3	_' list "$T/old.rs"

is 'let c = '"'"'{'"'"';
let q = '"'"'\'"'"''"'"';
let life: &'"'"'static str = r#"raw " } => "#;
if c == '"'"'}'"'"' {
    return DispatchOutcome::Done;
}
send(life).await;' body 2 "$T/old.rs"
is 'crate::x::serve(p, &[1, 2], (3, 4)).await' body 5 "$T/old.rs"
run 1 "no arm 8: the match has 7" body 8 "$T/old.rs"
run 1 "no arm 0: the match has 7" body 0 "$T/old.rs"

# ── same-heads
run 0 "same heads (7 arms, same order)." same-heads "$T/old.rs" "$T/old.rs"
# A body may change freely: the heads are still the same.
sed 's|send("root").await;|areas::root(cx).await;|' "$T/old.rs" > "$T/body-changed.rs"
run 0 "same heads (7 arms, same order)." same-heads "$T/old.rs" "$T/body-changed.rs"
# A comment inside or between heads does not count.
sed 's|// a comment inside a head|// another comment, reworded|' "$T/old.rs" > "$T/comment.rs"
run 0 "same heads" same-heads "$T/old.rs" "$T/comment.rs"
# A guard changed.
sed 's|"/account" if !is_post|"/account" if is_post|' "$T/old.rs" > "$T/guard.rs"
run 1 'FAIL arm 3 differs.' same-heads "$T/old.rs" "$T/guard.rs"
run 1 'new: "/" | "/account" if is_post' same-heads "$T/old.rs" "$T/guard.rs"
# A path in a pattern changed (a string is part of the head).
sed 's|"/ping" =>|"/pong" =>|' "$T/old.rs" > "$T/path.rs"
run 1 'FAIL arm 2 differs.' same-heads "$T/old.rs" "$T/path.rs"
# Two arms swapped.
python3 - "$T/old.rs" "$T/swapped.rs" <<'PY'
import sys
s = open(sys.argv[1]).read()
a = '        "/tail" => {\n            send("tail").await;\n        },\n'
b = '        p if crate::x::is_route(p) => crate::x::serve(p, &[1, 2], (3, 4)).await,\n'
assert a in s and b in s
open(sys.argv[2], "w").write(s.replace(b + a, a + b))
PY
run 1 'FAIL arm 5 differs.' same-heads "$T/old.rs" "$T/swapped.rs"
# An arm dropped from the end of the list of real arms.
python3 - "$T/old.rs" "$T/dropped.rs" <<'PY'
import sys
s = open(sys.argv[1]).read()
a = '        _ => {\n            send("404").await;\n        }\n'
assert a in s
open(sys.argv[2], "w").write(s.replace(a, ""))
PY
run 1 'FAIL the old match has 7 arms, the new one 6.' same-heads "$T/old.rs" "$T/dropped.rs"
# No match at all.
printf 'fn f() {}\n' > "$T/none.rs"
run 1 'no `match path.as_str() {` in the file' heads "$T/none.rs"

# ── same-body: arm 4 moved into a function.
cat > "$T/area.rs" <<'RS'
//! An area file. fn moved_a in a comment is not the function.
pub(in crate::routes) async fn moved_a(cx: &mut Cx<'_>, p: &str) -> Option<DispatchOutcome> {
    let Cx {
        stream,
        buf,
        ..
    } = cx;
    if p.len() > 3 {
        return Some(DispatchOutcome::KeepAlive);
    }
    let v = vec![1, 2, 3];
    send(&format!("{v:?}")).await;
    None
}

pub(in crate::routes) async fn ping(cx: &mut Cx<'_>) -> Option<DispatchOutcome> {
    let Cx { stream, .. } = cx;
    let c = '{';
    let q = '\'';
    let life: &'static str = r#"raw " } => "#;
    if c == '}' {
        return Some(DispatchOutcome::Done);
    }
    send(life).await;
    None
}

async fn no_prelude() -> Option<DispatchOutcome> {
    send("root").await;
    None
}
RS
run 0 "same body (arm 4, 5 lines, fn moved_a)." same-body "$T/old.rs" 4 "$T/area.rs" moved_a
run 0 "same body (arm 2, 7 lines, fn ping)." same-body "$T/old.rs" 2 "$T/area.rs" ping
run 0 "same body (arm 3, 1 lines, fn no_prelude)." same-body "$T/old.rs" 3 "$T/area.rs" no_prelude
# The wrong arm.
run 1 "FAIL arm 3 and fn ping differ at body line 1." same-body "$T/old.rs" 3 "$T/area.rs" ping
# One character of the body changed.
sed 's|if p.len() > 3 {|if p.len() > 4 {|' "$T/area.rs" > "$T/area-changed.rs"
run 1 "FAIL arm 4 and fn moved_a differ at body line 1." same-body "$T/old.rs" 4 "$T/area-changed.rs" moved_a
run 1 "new: if p.len() > 4 {" same-body "$T/old.rs" 4 "$T/area-changed.rs" moved_a
# A line added.
sed 's|    let v = vec!\[1, 2, 3\];|    audit();\n    let v = vec![1, 2, 3];|' "$T/area.rs" > "$T/area-added.rs"
run 1 "differ at body line 4." same-body "$T/old.rs" 4 "$T/area-added.rs" moved_a
# A line dropped from the end.
sed 's|    send(&format!("{v:?}")).await;||' "$T/area.rs" > "$T/area-dropped.rs"
run 1 "differ at body line 5." same-body "$T/old.rs" 4 "$T/area-dropped.rs" moved_a
# A return that was not in the old body.
sed 's|return Some(DispatchOutcome::KeepAlive);|return Some(DispatchOutcome::Done);|' "$T/area.rs" > "$T/area-return.rs"
run 1 "new:     return DispatchOutcome::Done;" same-body "$T/old.rs" 4 "$T/area-return.rs" moved_a
# The rewrite is undone in code only: the same words in a string stay.
cat > "$T/old-str.rs" <<'RS'
async fn handle(path: String) -> DispatchOutcome {
    match path.as_str() {
        "/s" => {
            log("return Some(DispatchOutcome::Done);");
        }
        _ => {}
    }
}
RS
cat > "$T/area-str.rs" <<'RS'
async fn s() -> Option<DispatchOutcome> {
    log("return Some(DispatchOutcome::Done);");
    None
}
RS
run 0 "same body (arm 1, 1 lines, fn s)." same-body "$T/old-str.rs" 1 "$T/area-str.rs" s
# An expression arm has no block to move; a function that is missing or defined twice is refused.
run 1 "arm 5 has no block body to move" same-body "$T/old.rs" 5 "$T/area.rs" ping
run 1 '`fn nowhere` is defined 0 times in the new file, wanted 1' same-body "$T/old.rs" 4 "$T/area.rs" nowhere
cat "$T/area.rs" "$T/area.rs" > "$T/area-twice.rs"
run 1 '`fn ping` is defined 2 times in the new file, wanted 1' same-body "$T/old.rs" 2 "$T/area-twice.rs" ping
# A first statement that only looks like the context line.
sed 's|    } = cx;|    } = other;|' "$T/area.rs" > "$T/area-prelude.rs"
run 1 'the first statement starts `let Cx {` but does not end `} = cx;`' same-body "$T/old.rs" 4 "$T/area-prelude.rs" moved_a

# ── The real dispatcher.
REAL="$HERE/crates/k2-daemon/src/routes/dispatcher.rs"
heads="$(python3 "$TOOL" heads "$REAL")"
n="$(printf '%s\n' "$heads" | wc -l | tr -d ' ')"
[ "$n" -ge 200 ] || { echo "FAIL: the real match has only $n arms" >&2; exit 1; }; pass=$((pass + 1))
[ "$(printf '%s\n' "$heads" | tail -1)" = "_" ] || { echo "FAIL: the real match does not end with the catch-all" >&2; exit 1; }; pass=$((pass + 1))
if printf '%s\n' "$heads" | grep -qx ''; then echo "FAIL: an arm of the real match has an empty head" >&2; exit 1; fi; pass=$((pass + 1))
# Every arm starts after the one before it ends, and the last ends before the match closes.
python3 "$TOOL" list "$REAL" | awk -F'\t' '
    { if ($2 < prev_end) { print "FAIL: arm " $1 " starts on line " $2 ", before arm " $1 - 1 " ends (" prev_end ")"; bad = 1 }
      if ($3 < 1) { print "FAIL: arm " $1 " has " $3 " lines"; bad = 1 }
      prev_end = $2 + $3 - 1 }
    END { exit bad }' >&2 || exit 1
pass=$((pass + 1))
run 0 "same heads ($n arms, same order)." same-heads "$REAL" "$REAL"
# Every block arm's body can be read back and ends where the next arm starts.
# Arm 1 is D1's passport arm (an agent's scoped passport on agent routes,
# ahead of every other arm); the pair page comes right after it.
python3 "$TOOL" body 1 "$REAL" | grep -q "passport_routes::serve_tcp" || { echo "FAIL: arm 1 of the real match is not the passport arm" >&2; exit 1; }; pass=$((pass + 1))
python3 "$TOOL" body 2 "$REAL" | grep -q "send_pair_page" || { echo "FAIL: arm 2 of the real match is not the pair page" >&2; exit 1; }; pass=$((pass + 1))

echo "dispatcher-arms tests: $pass passed"
