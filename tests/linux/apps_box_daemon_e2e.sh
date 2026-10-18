#!/usr/bin/env bash
# App runtime box prep end to end — runs AS ROOT INSIDE a throwaway systemd
# container (Debian 13, own user and network namespaces), never on a real
# box. Driven by tests/linux/run_apps_box_it.sh, which mounts:
#   /it/bin/k2-root-helper-cur   test-keys helper (protocol 6)
#   /it/bin/k2-daemon            this checkout's daemon (debug)
#   /it/cli/k2                   this checkout's CLI
#   /it/scripts/                 install-root-helper.sh
#
# The real daemon runs headless as k2-daemon.service (user k2, K2_APPS=1,
# air-gapped). The owner (the daemon token in the daemon user's HOME) runs
# `k2 app doctor` and `k2 app box prepare`; the daemon drives the helper
# through sudo from its own main process.
set -euo pipefail
. /it/tests/apps_box_netns.sh

pass=0
fail() { echo "FAIL: $*" >&2; journalctl -u k2-daemon.service --no-pager 2>/dev/null | tail -n 20 >&2 || true; exit 1; }
ok() { echo "  ok $*"; pass=$((pass + 1)); }
info() { echo "  info $*"; }

useradd -m -s /bin/bash k2
K2HOME=/home/k2
K2() { inns runuser -u k2 -- env HOME="$K2HOME" PATH=/usr/bin:/bin /it/cli/k2 "$@"; }

echo "== helper + the real daemon =="
K2_ALLOW_TEST_KEYS_BUILD=1 bash /it/scripts/install-root-helper.sh --file /it/bin/k2-root-helper-cur >/tmp/irh.log 2>&1 \
	|| { cat /tmp/irh.log; fail "install-root-helper.sh"; }
install -m 0755 /it/bin/k2-daemon /usr/local/bin/k2-daemon
k2it_netns_up
cat >/etc/systemd/system/k2-daemon.service <<UNIT
[Unit]
Description=K2 daemon (apps box prep e2e)
[Service]
Type=simple
User=k2
Environment=HOME=$K2HOME K2_APPS=1 K2_AIRGAP=1
ExecStart=/usr/local/bin/k2-daemon
NetworkNamespacePath=$K2IT_NS
UNIT
systemctl daemon-reload
systemctl start k2-daemon.service
wait_daemon() {
	for _ in $(seq 1 300); do
		[ -s "$K2HOME/.k2/heartbeat.port" ] && inns curl -sf "http://127.0.0.1:$(cat "$K2HOME/.k2/heartbeat.port")/boot-status" 2>/dev/null | grep -q '"ready"' && return 0
		sleep 0.2
	done
	fail "daemon not ready"
}
wait_daemon
ok "k2-daemon up as k2-daemon.service (port $(cat "$K2HOME/.k2/heartbeat.port"))"

echo "== doctor before prep =="
set +e
K2 app doctor --json >/tmp/d0.json; rc=$?
set -e
[ "$rc" = 1 ] || fail "doctor before prep must exit 1 (got $rc): $(cat /tmp/d0.json)"
python3 - /tmp/d0.json <<'PY'
import json, sys
v = json.load(open(sys.argv[1]))
st = {c["id"]: c["status"] for c in v["checks"]}
assert v["helperProtocol"] == 6 and st["helper"] == "ok", (v["helperProtocol"], st)
assert st["platform"] == "ok" and st["cgroupV2"] == "ok", st
# The image bakes the distro packages (offline container), so Podman is
# already there; the box itself is not prepared.
for k in ("appUser", "subid", "linger", "reservedPorts", "nft", "processUnit"):
    assert st[k] == "fail", (k, st)
assert v["ready"] is False, v
print("    before:", {k: st[k] for k in ("helper", "podman", "appUser", "nft", "ioCost")})
PY
human="$(K2 app doctor 2>&1 || true)"
printf '%s\n' "$human" | head -n 1 | grep -q "App box: NOT READY" || fail "human doctor: $human"
ok "doctor: helper protocol 6 ok, box not prepared (exit 1)"

echo "== k2 app box prepare (owner) =="
K2 app box prepare 2>&1 | tee /tmp/prep.txt | sed 's/^/    /'
grep -q "App box: ready" /tmp/prep.txt || fail "prepare did not end ready"
set +e
K2 app doctor --json >/tmp/d1.json; rc=$?
set -e
[ "$rc" = 0 ] || fail "doctor after prep must exit 0 (got $rc)"
python3 - /tmp/d1.json <<'PY'
import json, sys
v = json.load(open(sys.argv[1]))
st = {c["id"]: c["status"] for c in v["checks"]}
for k in ("helper", "podman", "pasta", "ociRuntime", "conmon", "appUser", "subid", "linger", "reservedPorts", "nft", "processUnit"):
    assert st[k] == "ok", (k, st)
assert v["runtime"] == "crun" and v["runtimeRecorded"] is True, v["runtime"]
print("    after:", st)
PY
ok "prepare ran every verb through the daemon; doctor ready (exit 0), runtime crun recorded"

echo "== prepare again: converges =="
K2 app box prepare --json >/tmp/p2.json
python3 - /tmp/p2.json <<'PY'
import json, sys
v = json.load(open(sys.argv[1]))
assert v["ok"], v
changed = [s["verb"] for s in v["steps"] if isinstance(s.get("result"), dict) and (s["result"].get("changed") or s["result"].get("created"))]
# nft apply always re-renders (no "changed" field); everything else is a no-op.
assert changed == [], changed
PY
ok "a second prepare changes nothing"

echo "== boot re-apply: the nft table comes back after a daemon restart =="
inns nft delete table inet k2app
systemctl restart k2-daemon.service
wait_daemon
for _ in $(seq 1 100); do inns nft list table inet k2app >/dev/null 2>&1 && break; sleep 0.2; done
grep -q "chain k2app_v4" <<<"$(inns nft list table inet k2app)" || fail "the daemon did not re-apply the nft table at boot"
ok "daemon boot re-applied inet k2app"

echo "== youki (opt-in) =="
set +e
K2 app box prepare --runtime youki >/tmp/py.txt 2>&1; rc=$?
set -e
sed 's/^/    /' /tmp/py.txt | tail -n 6
if [ "$rc" = 0 ]; then
	K2 app doctor --json | python3 -c 'import json,sys; v=json.load(sys.stdin); assert v["runtime"]=="youki", v["runtime"]'
	ok "youki installed from the distro and recorded as the active runtime"
	K2 app box prepare --runtime crun >/dev/null
	K2 app doctor --json | python3 -c 'import json,sys; v=json.load(sys.stdin); assert v["runtime"]=="crun", v["runtime"]'
	ok "back to crun"
else
	[ "$rc" = 3 ] || fail "youki prepare exit $rc"
	grep -q "youki_unavailable" /tmp/py.txt || fail "youki refusal names youki_unavailable"
	K2 app doctor --json | python3 -c 'import json,sys; v=json.load(sys.stdin); assert v["runtime"]=="crun", v["runtime"]'
	info "no youki package on this distro: prepare stopped at install-youki; crun stays active"
fi

echo "== refusals =="
set +e
K2 app box prepare --runtime runc >/dev/null 2>&1; rc=$?
set -e
[ "$rc" = 2 ] || fail "unknown runtime exit $rc"
# A K2 terminal's passport is not the owner: the agent verb list lets it
# read the doctor, never prepare. (Unit-tested: may_prepare.)
ok "unknown runtime refused (exit 2)"

echo
echo "apps_box_daemon_e2e: $pass checks passed"
