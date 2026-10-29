#!/usr/bin/env bash
# k2 app updates (0.46.7): the CLI's own checks, offline (no daemon is
# needed or touched). --help reaches the family's own page (not the App
# family's), usage errors exit 2 before any call, the token is never taken
# as an argument, --artifact/--sig take <platform>=<file>, and push names
# missing files before anything is sent. The daemon side is covered by
# k2-daemon's apps::updates_routes tests (a stand-in download host).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
K2="$(cd "$SCRIPT_DIR/../.." && pwd)/cli/k2"
for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done
SANDBOX="$(mktemp -d /tmp/k2upd-XXXXXX)"
trap 'rm -rf "$SANDBOX"' EXIT
export HOME="$SANDBOX/home"
# Never reach a real daemon: connection files for a port nothing listens on.
mkdir -p "$HOME/.k2"
printf '1' >"$HOME/.k2/heartbeat.port"
printf 'offline-test-token' >"$HOME/.k2/heartbeat.token"

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
run() { set +e; OUT="$(bash "$K2" "$@" 2>&1)"; RC=$?; set -e; }
expect() {
    local label="$1" rc="$2" needle="$3"; shift 3
    run "$@"
    if [ "$RC" = "$rc" ] && printf '%s' "$OUT" | grep -Fq -- "$needle"; then ok "$label"; else bad "$label (exit $RC, want $rc; output: $(printf '%s' "$OUT" | head -3))"; fi
}

echo "== help =="
expect "app updates --help is its own page" 0 "k2 app updates — signed desktop updates" app updates --help
expect "app updates (no verb) prints the page" 0 "Exit codes:" app updates
expect "push --help prints the page" 0 "--drop-platform" app updates push --help
expect "the App help names the family" 0 "k2 app updates push|ls|promote|rollback|pull" app --help
expect "an unknown verb is usage" 2 "usage: k2 app updates" app updates frobnicate

echo "== usage errors (no call made) =="
expect "push without --version" 2 "usage: k2 app updates push" app updates push jobs --channel stable --artifact darwin-aarch64=x.app.tar.gz
expect "push --artifact without platform=" 2 "--artifact takes <platform>=<file>" app updates push jobs --channel stable --version 1.0.0 --artifact x.app.tar.gz
expect "push names a missing file" 2 "is not a file" app updates push jobs --channel stable --version 1.0.0 --artifact darwin-aarch64="$SANDBOX/nope.app.tar.gz"
touch "$SANDBOX/a.app.tar.gz"
expect "push --sig without platform=" 2 "--sig takes <platform>=<file.sig>" app updates push jobs --channel stable --version 1.0.0 --artifact darwin-aarch64="$SANDBOX/a.app.tar.gz" --sig x.sig
expect "promote needs --to and --from or --version" 2 "usage: k2 app updates promote" app updates promote jobs --to stable
expect "rollback needs --to" 2 "usage: k2 app updates rollback" app updates rollback jobs --channel stable
expect "pull needs --version" 2 "usage: k2 app updates pull" app updates pull jobs
expect "key --set-file needs a file" 2 "is not a file" app updates key jobs --set-file "$SANDBOX/none.pub"

echo "== the token is never an argument =="
expect "token --token is refused" 2 "never an argument" app updates token jobs --token k2dl_0123456789abcdef0123456789abcdef
expect "token as a second word is refused" 2 "never the token itself" app updates token jobs k2dl_0123456789abcdef0123456789abcdef
expect "token --file needs a file" 2 "is not a file" app updates token jobs --file "$SANDBOX/none.txt"

echo
echo "app_updates_cli: $pass passed, $fail failed"
[ "$fail" = 0 ]
