#!/usr/bin/env bash
# Custom domains, Path A: the go / no-go checklist (prd-custom-domains-path-a-
# 0466-v1 §9, N1–N9) as far as a temp-HOME daemon can run it, plus the
# 0.46.6 firewall contract K2 Web's E0 collector builds on (§4.1).
#
# Runs against a sandbox daemon (temp HOME, never ~/.k2, never the running
# daemon). Needs target/{debug,release}/k2-daemon: build it first, on a
# compute node (never on z3mbp):
#   cargo build -p k2-daemon && bash tests/cli/firewall_path_a_go_no_go.sh
#
# What a sandbox can't show is printed as MANUAL with the exact command to
# run on the real box (DNS, a public IP, a real :443). Those are the box's
# checks on the day; everything else fails loudly here.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

for v in $(env | grep -oE '^K2(SO)?_[A-Z0-9_]+'); do unset "$v"; done

# shellcheck source=_sandbox_daemon.sh
source "$SCRIPT_DIR/_sandbox_daemon.sh"
sandbox_daemon_start

k2() {
    env -i HOME="$SANDBOX_HOME" PATH="$PATH" K2_AIRGAP=1 K2_PORT="$K2SO_PORT" "$K2_CLI" "$@"
}

pass=0
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "  PASS: $*"; pass=$((pass + 1)); }
manual() { echo "  MANUAL: $*"; }
jget() { python3 -c 'import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1], {"d": d}))' "$1"; }

echo "== N1: the daemon is 0.46.6 or later and serves the view =="
ver="$(k2 firewall ports --json | jget 'd["daemon"]')"
python3 -c 'import sys; v=tuple(int(x) for x in sys.argv[1].split(".")[:3]); sys.exit(0 if v >= (0,46,5) else 1)' "$ver" \
    || fail "daemon $ver is older than the view"
ok "view from daemon $ver (the release bump makes it 0.46.6)"
manual "on the box: k2 --version  →  0.46.6"

echo "== the contract (§4.1) =="
view="$(k2 firewall ports --json)"
for k in version generatedAt hash daemon box public knownNotPublic upstream; do
    printf '%s' "$view" | python3 -c 'import json,sys; d=json.load(sys.stdin); sys.exit(0 if sys.argv[1] in d else 1)' "$k" || fail "view has no $k: $view"
done
ok "view has every contract field"
[ "$(printf '%s' "$view" | jget 'd["version"]')" = "1" ] || fail "version is not 1"
[ "$(printf '%s' "$view" | jget 'd["public"][0]["source"]')" = "ssh" ] || fail "SSH is not first/always: $view"
ok "SSH is always in public (rule 2)"
h1="$(printf '%s' "$view" | jget 'd["hash"]')"
h2="$(k2 firewall ports --json | jget 'd["hash"]')"
[ "$h1" = "$h2" ] || fail "hash moved between two reads ($h1 / $h2)"
ok "hash stable across reads"

echo "== owner options =="
k2 firewall open 6000/udp --label "game server" --from 198.51.100.0/24 >/dev/null
h3="$(k2 firewall ports --json | jget 'd["hash"]')"
[ "$h3" != "$h1" ] || fail "open didn't move the hash"
k2 firewall ports --json | jget '[r for r in d["public"] if r["port"]==6000][0]["allowFrom"]' | grep -q "198.51.100.0/24" || fail "6000/udp not narrowed"
ok "open 6000/udp moves the hash, narrowed to its source"
k2 firewall close 6000/udp >/dev/null
[ "$(k2 firewall ports --json | jget 'd["hash"]')" = "$h1" ] || fail "close didn't restore the hash"
ok "close restores it"
if out="$(k2 firewall close 22/tcp 2>&1)"; then fail "close 22/tcp succeeded: $out"; fi
printf '%s' "$out" | grep -q "ssh_stays_open" || fail "close 22/tcp: $out"
ok "close 22/tcp refused (lockout guard)"
if out="$(k2 firewall ssh allow 2>&1)"; then fail "empty ssh allow succeeded: $out"; fi
ok "an empty ssh allow is refused"
rules="$(k2 firewall rules --format ufw)"
printf '%s' "$rules" | grep -m1 "sudo ufw allow" | grep -q "22/tcp" || fail "ufw rules don't start with SSH: $rules"
k2 firewall rules --format nft | grep -q "policy drop" || fail "nft rules aren't default-deny"
ok "rules: SSH first, default deny"

echo "== N2: the :443 front and who holds :443 =="
front="$(k2 front status --json)"
printf '%s' "$front" | python3 -c 'import json,sys; d=json.load(sys.stdin); sys.exit(0 if "port443" in d and "passthrough" in d and "guard" in d else 1)' \
    || fail "front status lacks port443/passthrough/guard: $front"
holder="$(printf '%s' "$front" | jget '(d["port443"] or {}).get("holder")')"
ok "front status names :443's holder ($holder)"
manual "on the box: k2 front status  →  serving, an app route; if ':443 : foreign', the box is NO-GO until the server moves (k2 front --help)"

echo "== passthrough names (option 1 for a box with an existing web server on :443) =="
k2 front passthrough add shop.example.com --to 127.0.0.1:9443 --proxy-v2 >/dev/null
k2 front passthrough --json | jget '[p["host"] for p in d["passthrough"]]' | grep -q "shop.example.com" || fail "passthrough not saved"
if out="$(k2 front passthrough add x.example.com --to 127.0.0.1:8443 2>&1)"; then fail "a K2 port was accepted: $out"; fi
k2 front passthrough rm shop.example.com >/dev/null
ok "passthrough add / refuse K2's port / rm"

echo "== N3–N5, N7: names, DNS, certificates, uploads, old names =="
manual "N3: k2 domain list  →  the customer apex attached with DNS write"
manual "N4: k2 app domain check <app> <name>  →  issuing → live (A record DNS-only)"
manual "N5: long WebSockets and 100 MB / 20 MB uploads from outside (door tests: big_uploads_pass_whole, a_websocket_upgrade_carries_k2s_client_ip_and_the_apps_credentials, door_websocket_eleven_idle_minutes --ignored)"
manual "N7: k2 app domains <app>  →  k2.dev names kept, names bound before 0.46.6 external; K2_APP_ORIGINS after restart"

echo "== N6: the visitor's address (automated in cargo test) =="
manual "door tests: a_live_name_reaches_its_app_and_the_client_ip_headers_are_k2s, the_door_keeps_host_says_https_and_never_redirects; on the box: App logs show X-K2-Client-IP = the tester's IP"

echo "== N8: the guard is on with generous defaults, counters start at 0 =="
fw="$(k2 firewall --json)"
[ "$(printf '%s' "$fw" | jget 'd["guard"]["settings"]["enabled"]')" = "True" ] || fail "guard off by default"
[ "$(printf '%s' "$fw" | jget 'd["guard"]["settings"]["connsPerIp"]')" = "64" ] || fail "connsPerIp default"
[ "$(printf '%s' "$fw" | jget 'd["guard"]["settings"]["writesPerMin"]')" = "120" ] || fail "writesPerMin default"
[ "$(printf '%s' "$fw" | jget 'd["guard"]["counters"]["perIpRefused"] + d["guard"]["counters"]["guardLimited"]')" = "0" ] || fail "counters not 0"
ok "guard on: 64 connections, 120 writes a minute; counters 0"
k2 firewall guard --writes-per-min 60 >/dev/null
[ "$(k2 firewall --json | jget 'd["guard"]["settings"]["writesPerMin"]')" = "60" ] || fail "guard change not kept"
k2 firewall guard --reset >/dev/null
ok "guard limits change and reset"
manual "on the box during the customer's own test pass: k2 firewall  →  refused 0 · limited 0 (rollback: k2 firewall guard <app> off)"

echo "== N9: E0 =="
[ "$(k2 firewall ports --json | jget 'd["upstream"]["managedBy"]')" = "owner" ] || fail "managedBy before any ack"
k2 firewall upstream --hash "$h1" --provider hetzner --applied-at 1700000000 >/dev/null
up="$(k2 firewall ports --json)"
[ "$(printf '%s' "$up" | jget 'd["upstream"]["managedBy"]')" = "k2" ] || fail "ack didn't set managedBy k2: $up"
[ "$(printf '%s' "$up" | jget 'd["upstream"]["applied"]["current"]')" = "True" ] || fail "ack not current: $up"
ok "K2 Web's applied-ack recorded (managedBy k2, current)"
manual "K2 Web confirms E0 is not applied to the box yet, or applied with 443 in the list"

echo "PASS: $pass checks (MANUAL lines are the box's own, on the day)"
