#!/usr/bin/env bash
# `k2 hooks status` and GET /cli/hooks/status, headless: a real k2-daemon
# under a temp HOME, air-gapped, through the real CLI.
#
#   cargo build -p k2-daemon && bash tests/cli/hooks_status.sh
#
# Checks:
#   1. GET /cli/hooks/status: 200, `port` is the daemon's own port, the
#      `notify_script` path, `injections` for claude/cursor/gemini (path,
#      exists, injected) plus notify_script, `recent_events` and its cap;
#   2. after `k2 hooks install` with ~/.claude present, an unreadable
#      ~/.gemini/settings.json and no Cursor config, `injections` reports
#      each one as it is on disk;
#   3. `k2 hooks status` prints "K2 hook server port: <port>", the notify
#      script line, and the "Config injections:" block with one row per CLI
#      ([OK ] claude, [-- ] cursor, [!! ] gemini);
#   4. `k2 hooks status --json` is valid JSON with the same keys;
#   5. the legacy GET /hook/complete refuses a wrong token with 403.
# Every check fails loudly (exit 1). Never ~/.k2, never the real daemon.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not executable" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

for v in $(env | cut -d= -f1 | grep -E '^(K2|K2SO)_' || true); do unset "$v"; done
export K2_AIRGAP=1

source "$SCRIPT_DIR/_sandbox_daemon.sh"
sandbox_daemon_start

k2() {
    env -i HOME="$SANDBOX_HOME" PATH="$PATH" K2_AIRGAP=1 \
        K2_PORT="$K2SO_PORT" "$K2_CLI" "$@"
}
BASE="http://127.0.0.1:${K2SO_PORT}"

pass=0
fail() { echo "FAIL: $*" >&2; echo "--- daemon log tail" >&2; tail -40 "$SANDBOX_HOME/daemon.log" >&2; exit 1; }
ok() { echo "  PASS: $*"; pass=$((pass + 1)); }
has() { grep -Fq -- "$2" <<<"$1" || fail "missing '$2' in: $1"; }

# status → the body of GET /cli/hooks/status; fails on any status but 200.
status() {
    local code
    code="$(curl -s -o "$SANDBOX_HOME/status.json" -w '%{http_code}' \
        "$BASE/cli/hooks/status?token=$K2SO_TOKEN&limit=50")"
    [ "$code" = "200" ] || fail "GET /cli/hooks/status: $code $(cat "$SANDBOX_HOME/status.json")"
    cat "$SANDBOX_HOME/status.json"
}

# check_shape JSON PORT HOME → exits non-zero with the reason on stderr.
check_shape() {
    python3 - "$1" "$2" "$3" <<'PY'
import json, sys
d = json.loads(sys.argv[1])
port, home = int(sys.argv[2]), sys.argv[3]
for key in ("port", "notify_script", "injections", "recent_events", "recent_events_cap"):
    assert key in d, f"{key} missing: {d}"
assert d["port"] == port, f"port {d['port']} is not the daemon's {port}"
assert d["notify_script"] == f"{home}/.k2/hooks/notify.sh", d["notify_script"]
assert isinstance(d["recent_events"], list), d["recent_events"]
assert d["recent_events_cap"] == 50, d["recent_events_cap"]
inj = d["injections"]
for key in ("claude", "cursor", "gemini", "notify_script"):
    assert key in inj, f"injections.{key} missing: {inj}"
for cli in ("claude", "cursor", "gemini"):
    entry = inj[cli]
    for key in ("path", "exists", "injected"):
        assert key in entry, f"injections.{cli}.{key} missing: {entry}"
PY
}

# The boot install pass runs once after ready; wait for it so it can't
# race the config files this test writes.
for i in $(seq 1 100); do
    last="$(status | python3 -c 'import json,sys; print(json.load(sys.stdin)["lastInstall"] is not None)')"
    [ "$last" = "True" ] && break
    sleep 0.2
done
[ "$last" = "True" ] || fail "the boot install pass never ran: $(status)"

echo "== GET /cli/hooks/status =="
body="$(status)"
check_shape "$body" "$K2SO_PORT" "$SANDBOX_HOME" || fail "status shape: $body"
ok "status: port is the daemon's ($K2SO_PORT), injections for claude/cursor/gemini, recent_events"

echo "== injections follow the configs on disk =="
mkdir -p "$SANDBOX_HOME/.claude" "$SANDBOX_HOME/.gemini"
out="$(k2 hooks install)" || fail "k2 hooks install: $out"
has "$out" "Hook install: notify.sh"
printf '{ not json' >"$SANDBOX_HOME/.gemini/settings.json"
body="$(status)"
python3 - "$body" "$SANDBOX_HOME" <<'PY' || fail "injections after install: $body"
import json, sys
inj, home = json.loads(sys.argv[1])["injections"], sys.argv[2]
c, g, u = inj["claude"], inj["gemini"], inj["cursor"]
assert c["path"] == f"{home}/.claude/settings.json" and c["exists"] and c["injected"], c
assert "Stop" in c["events"], c
assert g["exists"] and not g["injected"] and g["configUnreadable"], g
assert u["path"] == f"{home}/.cursor/hooks.json" and not u["exists"] and not u["injected"], u
s = inj["notify_script"]
assert s["path"] == f"{home}/.k2/hooks/notify.sh" and s["exists"] and s["version"], s
PY
ok "claude injected, gemini unreadable, cursor absent, notify.sh stamped"

echo "== k2 hooks status =="
out="$(k2 hooks status)" || fail "k2 hooks status: $out"
has "$out" "K2 hook server port: $K2SO_PORT"
has "$out" "Notify script: $SANDBOX_HOME/.k2/hooks/notify.sh  exists=True"
has "$out" "Config injections:"
has "$out" "[OK ] claude   $SANDBOX_HOME/.claude/settings.json"
has "$out" "[-- ] cursor   $SANDBOX_HOME/.cursor/hooks.json"
has "$out" "[!! ] gemini   $SANDBOX_HOME/.gemini/settings.json  config_unreadable: left untouched"
has "$out" "Recent events"
ok "header, notify script line and the Config injections block, one row per CLI"

out="$(k2 hooks status --json)" || fail "k2 hooks status --json: $out"
check_shape "$out" "$K2SO_PORT" "$SANDBOX_HOME" || fail "--json shape: $out"
ok "--json is valid JSON with the status keys"

echo "== legacy /hook/complete =="
code="$(curl -s -o /dev/null -w '%{http_code}' \
    "$BASE/hook/complete?paneId=x&tabId=y&eventType=Stop&token=not-the-token")"
[ "$code" = "403" ] || fail "a wrong token on /hook/complete: $code (want 403)"
ok "a wrong token on /hook/complete is 403"

echo "hooks_status: $pass checks passed"
