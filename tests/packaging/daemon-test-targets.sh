#!/usr/bin/env bash
# scripts/check-daemon-test-targets.sh passes on this tree and fails on each
# kind of broken test layout, with the reason. No cargo.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
CHECK="$ROOT/scripts/check-daemon-test-targets.sh"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
npass=0 nfail=0
pass() { echo "  ok: $1"; npass=$((npass + 1)); }
fail() { echo "  FAIL: $1" >&2; [ -z "${2:-}" ] || echo "        $2" >&2; nfail=$((nfail + 1)); }

T="crates/k2-daemon/tests"
fresh() {
    rm -rf "$tmp/t"
    mkdir -p "$tmp/t/crates/k2-daemon"
    cp -R "$ROOT/$T" "$tmp/t/$T"
}

echo "== this tree =="
if out="$(bash "$CHECK" 2>&1)"; then pass "the real layout passes"; else fail "the real layout" "$out"; fi

# broken <name> <wanted text> <shell edit run in the copy's tests dir>
broken() {
    local name="$1" want="$2" edit="$3" rc=0 out
    fresh
    ( cd "$tmp/t/$T" && eval "$edit" )
    out="$(bash "$CHECK" "$tmp/t" 2>&1)" || rc=$?
    if [ "$rc" != 1 ]; then fail "$name" "exit $rc, wanted 1: $out"; return; fi
    if ! grep -qF -- "$want" <<<"$out"; then fail "$name" "no '$want' in: $out"; return; fi
    pass "$name"
}

echo "== broken layouts =="
broken "a module file main.rs doesn't declare" \
    "tests/it_misc/stray_integration.rs is not declared in tests/it_misc/main.rs" \
    'printf "#[test]\nfn t() {}\n" > it_misc/stray_integration.rs'
broken "main.rs declares a module with no file" \
    "declares \`mod git_tools_integration;\` but tests/it_misc/git_tools_integration.rs doesn't exist" \
    'rm it_misc/git_tools_integration.rs'
broken "a group without main.rs" \
    "tests/it_misc/ has no main.rs" \
    'rm it_misc/main.rs'
broken "a group without the runner guard" \
    "tests/it_misc/main.rs has no runner_guard test" \
    'python3 -I -c "import re,sys; p=sys.argv[1]; s=open(p).read(); n=re.sub(r\"#\[test\]\nfn runner_guard\(\) \{\n.*?\n\}\n\", \"\", s, flags=re.S); assert n != s; open(p,\"w\").write(n)" it_misc/main.rs'
broken "a grouped module that still includes support with #[path]" \
    "tests/it_contract/ensure_cli_route.rs includes support with #[path]" \
    'printf "#[path = \"support/app_fixture.rs\"]\nmod app_fixture;\n" >> it_contract/ensure_cli_route.rs'
broken "a name both top level and in a group" \
    "git_tools_integration is both tests/git_tools_integration.rs and tests/it_misc/git_tools_integration.rs" \
    'cp it_misc/git_tools_integration.rs git_tools_integration.rs'
broken "a name in two groups" \
    "zen_core is both tests/it_misc/zen_core.rs and tests/it_zen/zen_core.rs" \
    'cp it_zen/zen_core.rs it_misc/zen_core.rs && echo "mod zen_core;" >> it_misc/main.rs'
broken "a nested directory in a group" \
    "tests/it_misc/nested/: a group holds flat module files only" \
    'mkdir it_misc/nested'

echo "daemon-test-targets: $npass passed, $nfail failed"
[ "$nfail" = 0 ]
