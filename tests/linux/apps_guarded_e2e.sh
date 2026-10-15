#!/usr/bin/env bash
# The guarded App engine end to end (prd-k2-app-runtime-v1 S11/S12) — runs
# AS ROOT INSIDE a throwaway Debian 13 VM (tests/linux/run_apps_guarded_vm.sh
# boots it with qemu inside a container on the build box), never on a real
# box. The VM has the network (box prep installs the distro packages; the
# daemon fetches the pinned base image with skopeo).
#
#   /it/bin/k2-daemon            this checkout's daemon (debug)
#   /it/bin/k2-root-helper-cur   test-keys helper (protocol 6)
#   /it/bin/k2-app-engine        this checkout's engine
#   /it/bin/testsign             the helper's TEST signer (engine-sig)
#   /it/cli/k2, /it/scripts/install-root-helper.sh
#   /it/tests/guarded-fixture/server.mjs, /it/tests/engine_client.py
#
# Phase A: the real daemon (k2-daemon.service, user k2, K2_APPS=1): box
# prep, engine install (signed with the TEST key, checked by the test-keys
# helper), two guarded Apps, the escape subset from inside the wall, the
# gateway link and its per-App nft rule, a zero-downtime switch under load,
# logs and their rate limit, crash restart, youki when the distro has it,
# uninstall --purge.
# Phase B: a fake daemon as k2-daemon.service's main process (like
# tests/linux/root_helper_in_container.sh): the engine admits its main
# process and refuses its child (E18), the App pass and granted secrets
# arrive as Podman secrets (never in the unit or env file), and
# `pg hba-app` against a real Postgres.
set -Eeuo pipefail

IT=/it
pass=0
fail() {
	echo "FAIL: $*" >&2
	journalctl -u k2-daemon.service --no-pager 2>/dev/null | tail -n 25 >&2 || true
	journalctl --no-pager -n 40 _UID="$(id -u k2app 2>/dev/null || echo 0)" 2>/dev/null | tail -n 25 >&2 || true
	exit 1
}
ok() { echo "  ok $*"; pass=$((pass + 1)); }
info() { echo "  info $*"; }
# Name the line a `set -e` exit happens on (not inside `set +e` blocks).
trap 'rc=$?; [[ $- == *e* ]] && fail "line $LINENO: $BASH_COMMAND (exit $rc)"' ERR
jq_() { python3 -c "import json,sys; v=json.load(open(sys.argv[1])); $1" "$2"; }

ARCH="$(uname -m)"
VER="$(grep -a -o 'K2_APP_ENGINE_BUILD v=[0-9.]*' "$IT/bin/k2-app-engine" | head -n1 | sed 's/.*v=//')"
[ -n "$VER" ] || fail "engine build marker"
echo "== facts =="
. /etc/os-release
echo "  os: $PRETTY_NAME; kernel $(uname -r); $(systemctl --version | head -n1); engine v$VER"
HOSTIP="$(ip -4 route get 1.1.1.1 | sed -n 's/.* src \([0-9.]*\).*/\1/p')"
echo "  vm address: $HOSTIP"

useradd -m -s /bin/bash k2
K2HOME=/home/k2
WS=$K2HOME/ws
K2() { runuser -u k2 -- env HOME="$K2HOME" PATH=/usr/local/bin:/usr/bin:/bin K2_PROJECT_PATH="$WS" bash -c 'cd "$0" 2>/dev/null || cd ~; exec /it/cli/k2 "$@"' "$WS" "$@"; }
asapp() { runuser -u k2app -- env XDG_RUNTIME_DIR="/run/user/$(id -u k2app)" HOME=/var/lib/k2app bash -c 'cd ~ && "$@"' _ "$@"; }

echo "== phase A: helper + the real daemon =="
K2_ALLOW_TEST_KEYS_BUILD=1 bash "$IT/scripts/install-root-helper.sh" --file "$IT/bin/k2-root-helper-cur" >/tmp/irh.log 2>&1 \
	|| { cat /tmp/irh.log; fail "install-root-helper.sh"; }
install -m 0755 "$IT/bin/k2-daemon" /usr/local/bin/k2-daemon
cat >/etc/systemd/system/k2-daemon.service <<UNIT
[Unit]
Description=K2 daemon (guarded engine e2e)
After=network-online.target
[Service]
Type=simple
User=k2
Environment=HOME=$K2HOME K2_APPS=1 K2_APPS_WATCH_SECS=20 K2_APPS_DRAIN_SECS=3 K2_APPS_STOP_GRACE_SECS=15 K2_APPS_DEPLOY_TIMEOUT_SECS=240 K2_APPS_STALL_SECS=1200
ExecStart=/usr/local/bin/k2-daemon
UNIT
systemctl daemon-reload
systemctl start k2-daemon.service
wait_daemon() {
	for _ in $(seq 1 300); do
		[ -s "$K2HOME/.k2/heartbeat.port" ] && curl -sf "http://127.0.0.1:$(cat "$K2HOME/.k2/heartbeat.port")/boot-status" 2>/dev/null | grep -q '"ready"' && return 0
		sleep 0.2
	done
	fail "daemon not ready"
}
wait_daemon
DPORT="$(cat "$K2HOME/.k2/heartbeat.port")"
OWNER_TOKEN="$(cat "$K2HOME/.k2/heartbeat.token")"
ok "k2-daemon up as k2-daemon.service on $DPORT"

echo "== box prep (owner) =="
K2 app box prepare 2>&1 | tail -n 4 | sed 's/^/    /'
set +e
K2 app doctor --json >/tmp/d1.json; rc=$?
set -e
[ "$rc" = 0 ] || { cat /tmp/d1.json; fail "doctor after prep: exit $rc"; }
jq_ 'st={c["id"]:c for c in v["checks"]}; assert st["engine"]["status"]=="warn", st["engine"]; assert st["skopeo"]["status"]=="ok", st["skopeo"]' /tmp/d1.json
ok "box prepared (doctor ready); engine not installed yet; skopeo present"

echo "== engine install (S11) =="
ED=/opt/k2-engine-it
install -d -m 0755 "$ED"
install -m 0644 "$IT/bin/k2-app-engine" "$ED/k2-app-engine-linux-$ARCH"
# The protocol the engine build speaks (2 in 0.46.0), from the binary itself.
EPROTO="$("$IT/bin/k2-app-engine" version | python3 -c 'import json,sys; print(json.load(sys.stdin)["protocol"])')"
"$IT/bin/testsign" engine-sig "$ED/k2-app-engine-linux-$ARCH" "$VER" "$EPROTO" "$ARCH" >"$ED/k2-app-engine-linux-$ARCH.sig"
# A tampered copy is refused before any root call.
TD=/opt/k2-engine-bad
install -d -m 0755 "$TD"
cp "$ED"/* "$TD/"
printf 'x' >>"$TD/k2-app-engine-linux-$ARCH"
set +e
K2 app engine install --from "$TD" >/tmp/bad.txt 2>&1; rc=$?
set -e
[ "$rc" = 3 ] && grep -q engine_refused /tmp/bad.txt || fail "tampered engine: rc $rc $(cat /tmp/bad.txt)"
[ ! -e /usr/local/libexec/k2-app-engine ] || fail "a tampered engine reached the box"
ok "a tampered engine is refused before the root call"
K2 app engine install --from "$ED" | tee /tmp/ei.txt | sed 's/^/    /'
grep -q "installed and answering" /tmp/ei.txt || fail "engine install"
[ "$(stat -c '%U %a' /usr/local/libexec/k2-app-engine)" = "root 755" ] || fail "engine binary owner/mode"
[ "$(stat -c '%U %G %a' /run/k2app)" = "k2app k2app 750" ] || fail "socket dir: $(stat -c '%U %G %a' /run/k2app)"
asapp systemctl --user is-active --quiet k2-app-engine.service || fail "engine unit not active"
set +e
K2 app doctor --json >/tmp/d2.json
set -e
jq_ 'st={c["id"]:c for c in v["checks"]}; assert st["engine"]["status"]=="ok", st["engine"]' /tmp/d2.json
ok "engine installed by the helper, running as k2app, doctor ok"

echo "== E18: only the daemon's main process =="
set +e
runuser -u k2 -- python3 "$IT/tests/engine_client.py" '{"verb":"version"}' >/tmp/e18.txt 2>&1; rc=$?
set -e
[ "$rc" = 1 ] && grep -q caller_not_daemon /tmp/e18.txt || fail "a k2-uid process that isn't the daemon: rc $rc $(cat /tmp/e18.txt)"
ok "an agent-like process (daemon uid, not its main pid): caller_not_daemon"
set +e
runuser -u nobody -- python3 "$IT/tests/engine_client.py" '{"verb":"version"}' >/tmp/e18n.txt 2>&1; rc=$?
set -e
[ "$rc" = 2 ] || fail "another user reached the socket: rc $rc $(cat /tmp/e18n.txt)"
ok "another user can't reach the socket at all (dir ACL)"
set +e
python3 "$IT/tests/engine_client.py" '{"verb":"version"}' >/tmp/e18r.txt 2>&1; rc=$?
set -e
[ "$rc" = 1 ] && grep -q caller_not_daemon /tmp/e18r.txt || fail "root: rc $rc $(cat /tmp/e18r.txt)"
ok "even root is not the daemon"

# S14: the daemon itself listens on each App's gateway port (its link);
# the fixture calls it at K2_GATEWAY_URL (127.0.0.1:8730 in the wall) with
# the App pass from K2_PASS_FILE.

echo "== workspace + fixture Apps =="
runuser -u k2 -- bash -c "mkdir -p '$WS' && git -C '$WS' -c init.defaultBranch=main init -q && git -C '$WS' -c user.name=t -c user.email=t@t -c commit.gpgsign=false commit -q --allow-empty -m one"
curl -sf -X POST -H 'Content-Type: application/json' -d "{\"path\":\"$WS\"}" "http://127.0.0.1:$DPORT/cli/projects/add-from-path?token=$OWNER_TOKEN" >/dev/null || fail "workspace"
mkapp() { # dir id tag
	runuser -u k2 -- mkdir -p "$1"
	printf '{"schema":1,"kind":"app","id":"%s","name":"Esc","version":"1.0.%s","host":{"mode":"cmd","start":["node","server.mjs"],"health":{"path":"/healthz"},"drainSecs":3}}\n' "$2" "$RANDOM" >"$1/manifest.json"
	printf '%s' "$3" >"$1/tag.txt"
	printf '{"daemonPort": %s, "hostIp": "%s"}\n' "$DPORT" "$HOSTIP" >"$1/facts.json"
	printf 'GREETING=hello-%s\n' "$3" >"$1/.env.k2"
	cp "$IT/tests/guarded-fixture/server.mjs" "$1/server.mjs"
	chown -R k2:k2 "$1"
}
mkapp "$WS/a1" esc-a a1
mkapp "$WS/a2" esc-a a2
mkapp "$WS/b1" esc-b b1

wait_dep() { # app → final result
	local out r
	for _ in $(seq 1 600); do
		out="$(K2 app deployment "$1" --json 2>/dev/null || true)"
		r="$(printf '%s' "$out" | python3 -c 'import json,sys
try:
    v=json.load(sys.stdin); print(v["deployment"]["result"])
except Exception: print("?")')"
		case "$r" in pending | starting | "?") sleep 1 ;; *) echo "$r"; return 0 ;; esac
	done
	echo timeout
}
front() { K2 app list --json | python3 -c "import json,sys; print([a for a in json.load(sys.stdin)['apps'] if a['appId']=='$1'][0]['frontPort'])"; }

echo "== guarded deploy (S12) =="
K2 app install esc-a --memory 256M
t0=$(date +%s)
K2 app deploy esc-a "$WS/a1" --engine guarded --no-wait >/dev/null || true
r="$(wait_dep esc-a)"
[ "$r" = live ] || { K2 app deployment esc-a --json 2>&1 | tail -c 3000; K2 app logs esc-a 2>&1 | tail -n 30; fail "esc-a first guarded deploy: $r"; }
FA="$(front esc-a)"
[ "$(curl -s "http://127.0.0.1:$FA/")" = a1 ] || fail "front esc-a"
ok "esc-a live behind the guarded engine in $(($(date +%s) - t0)) s (incl. the base image fetch + first id mapping)"
K2 app list | grep -q "Guarded (container)" || fail "label"
ok "label: Guarded (container)"
K2 app logs esc-a >/tmp/la.txt 2>&1 || true
grep -q "id mapping warmed in" /tmp/la.txt || fail "prepare warmed the mapping before the switch: $(tail -n 5 /tmp/la.txt)"
grep -q "esc a1 up" /tmp/la.txt || { head -c 6000 /tmp/la.txt; fail "the App's own output via k2 app logs (journal)"; }
ok "first id mapping paid at prepare (before the switch); App output reaches k2 app logs from the unit journal"

SVC="$(K2 app list --json | python3 -c "import json,sys; print([a for a in json.load(sys.stdin)['apps'] if a['appId']=='esc-a'][0]['serviceId'])")"
curl -s "http://127.0.0.1:$FA/probe" >/tmp/pa.json
python3 - /tmp/pa.json "$SVC" <<'PY'
import json, sys
v = json.load(open(sys.argv[1])); svc = sys.argv[2]
def no(k): assert v[k] not in ("open",), (k, v[k])
assert v["tag"] == "a1"
assert v["uid"] == 0, v["uid"]
assert v["daemonHome"] is False and v["ownerToken"] is False and v["k2appHome"] is False, v
assert v["writeApp"]["ok"] is False and v["writeRoot"]["ok"] is False, (v["writeApp"], v["writeRoot"])
assert v["writeData"]["ok"] is True and v["writeData"]["out"] == "a1", v["writeData"]
assert v["capEff"] == "0000000000000000" and v["capBnd"] == "0000000000000000", (v["capEff"], v["capBnd"])
assert v["noNewPrivs"] == "1" and v["seccomp"] == "2", (v["noNewPrivs"], v["seccomp"])
assert v["unshareU"]["ok"] is False and v["unshareR"]["ok"] is False, (v["unshareU"], v["unshareR"])
for k in ("daemonPort", "hostSsh", "hostIpSsh", "hostContainersInternal", "internet"):
    no(k)
assert v["dns"] != "" and not v["dns"][0].isdigit(), v["dns"]
assert v["gateway"]["status"] == 200 and v["gateway"]["link"] == "gw:38600" and v["gateway"]["url"] == "http://127.0.0.1:8730", v["gateway"]
assert v["otherGateway"]["status"] == 0, v["otherGateway"]
assert v["k2AppId"] == svc and v["k2AppName"] == "esc-a", (v["k2AppId"], v["k2AppName"])
assert "--max-old-space-size=128" in v["nodeOptions"], v["nodeOptions"]
for bad in ("K2_HOOK_TOKEN", "K2_HOOK_SOCK", "K2_CELL", "K2_OWNER_TOKEN", "K2_APPS"):
    assert bad not in v["env"], bad
print("    probe:", {k: v[k] for k in ("uid", "capEff", "seccomp", "daemonPort", "internet", "dns", "gateway", "procPids", "greeting")})
PY
ok "inside the wall: no daemon home/token, ro root + Seed, data rw, CapEff 0, NoNewPrivs, seccomp, no unshare -U, no host loopback/address/internet/DNS, own gateway yes, other gateway no, identity env (AR37)"
[ "$(jq_ 'print(v["greeting"])' /tmp/pa.json)" = "hello-a1" ] && ok ".env.k2 literal reached the App" || info ".env.k2 literal not delivered (vault unavailable headless): $(jq_ 'print(v["greeting"])' /tmp/pa.json)"

CTR="$(asapp podman ps --format '{{.Names}}' | grep '^k2app-esc-a--' | head -n1)"
[ -n "$CTR" ] || fail "no esc-a container"
asapp podman inspect "$CTR" >/tmp/ia.json
python3 - /tmp/ia.json <<'PY'
import json, sys
c = json.load(open(sys.argv[1]))[0]
hc = c["HostConfig"]
assert hc["ReadonlyRootfs"] is True, "ro root"
# The engine's OCI shim (named crun) runs crun (lane D: the MAC profile).
assert c["OCIRuntime"].rsplit("/", 1)[-1] == "crun", c["OCIRuntime"]
assert hc["Memory"] == 256 * 1024 * 1024, hc["Memory"]
assert hc.get("MemorySwap") == 256 * 1024 * 1024, hc.get("MemorySwap")
assert hc["PidsLimit"] == 512, hc["PidsLimit"]
so = " ".join(hc.get("SecurityOpt") or [])
assert "no-new-privileges" in so, so
assert hc.get("Privileged") is False
assert c["Config"]["Labels"]["k2.app"] == "esc-a"
ids = hc.get("IDMappings") or {}
print("    container:", c["OCIRuntime"], "mem", hc["Memory"], "pids", hc["PidsLimit"], "secopt", so[:80], "idmap", ids)
PY
ok "podman inspect: crun, read-only root, 256 MiB no swap, pids 512, no-new-privileges"
PASTA="$(pgrep -u k2app -x pasta.avx2 || pgrep -u k2app -x pasta || true)"
for p in $PASTA; do grep -q 'k2apps-esc_a.slice' "/proc/$p/cgroup" && PCG=1; done
[ "${PCG:-0}" = 1 ] || fail "pasta for esc-a is not in the App's slice: $(for p in $PASTA; do cat "/proc/$p/cgroup"; done)"
nft list chain inet k2app k2app_apps | grep -q 'k2apps-esc_a.slice" oifname "lo" tcp dport { 38600, 38900 } accept' || fail "per-App rule: $(nft list chain inet k2app k2app_apps)"
ok "pasta runs in the App's slice; the per-App nft rule admits only its gateway and proxy ports"

echo "== a second App: its own slot, port and gateway =="
K2 app install esc-b --memory 256M
K2 app deploy esc-b "$WS/b1" --engine guarded --no-wait >/dev/null || true
r="$(wait_dep esc-b)"
[ "$r" = live ] || fail "esc-b: $r"
FB="$(front esc-b)"
curl -s "http://127.0.0.1:$FB/probe" >/tmp/pb.json
jq_ 'assert v["gateway"]["status"]==200 and v["gateway"]["link"]=="gw:38601", v["gateway"]; assert v["otherGateway"]["status"]==0, v["otherGateway"]; assert v["writeData"]["out"]=="b1"' /tmp/pb.json
[ "$(runuser -u k2app -- cat /var/lib/k2app/engine/slots.json | python3 -c 'import json,sys; print(json.load(sys.stdin)["apps"])')" = "{'esc-a': 0, 'esc-b': 1}" ] || fail "slots"
DA="$(stat -c %u /var/lib/k2app/apps/esc-a/data)"; DB="$(stat -c %u /var/lib/k2app/apps/esc-b/data)"
[ "$DA" != "$DB" ] && [ "$DA" -ge 1048576 ] && [ "$DB" -ge 1048576 ] || fail "data owners: $DA $DB"
ok "esc-b: slot 1, gateway 38601, its own ids for its data ($DB vs esc-a $DA)"

echo "== egress (S15): only through the App's own proxy, only listed hosts =="
nft list chain inet k2app k2app_apps | grep -q 'k2apps-esc_b.slice" oifname "lo" tcp dport { 38601, 38901 } accept' || fail "esc-b per-App rule: $(nft list chain inet k2app k2app_apps)"
ss -ltnH 'sport = :38900' | grep -q 127.0.0.1 || fail "the daemon's proxy for esc-a isn't listening on 38900"
curl -s -m 30 "http://127.0.0.1:$FA/egress?target=example.com:443" >/tmp/ea.json
python3 - /tmp/ea.json <<'PY'
import json, sys
v = json.load(open(sys.argv[1]))
assert v["proxyEnv"] == {"https": "http://127.0.0.1:38900", "no": "127.0.0.1,localhost,::1", "node": "1"}, v["proxyEnv"]
assert v["connect"] == {"status": 403, "code": "egress_denied"}, ("not listed", v["connect"])
assert v["metadata"] == {"status": 403, "code": "egress_denied"}, v["metadata"]
assert v["loopback"] == {"status": 403, "code": "egress_denied"}, v["loopback"]
assert v["otherProxy"] != "open", ("another App's proxy", v["otherProxy"])
assert v["asK2"]["status"] == 400, ("the proxy never speaks to K2", v["asK2"])
assert v["fetch"]["status"] in (0, 403), ("fetch goes through the proxy and is refused", v["fetch"])
print("    egress:", v)
PY
ok "esc-a: proxy env set; unlisted host, metadata and loopback refused (403 egress_denied); esc-b's proxy unreachable; the proxy refuses K2-style requests"
K2 app egress esc-a --json >/tmp/eg.json
jq_ 'h=[x for x in v["hosts"] if x["host"]=="example.com"][0]; assert h["status"]=="seen" and h["refused"]>=1, h; assert v["enforced"] is True and v["proxy"]["listening"] is True, v' /tmp/eg.json
K2 app egress log esc-a --json | python3 -c 'import json,sys; L=json.load(sys.stdin)["lines"]; assert any(l.get("code")=="egress_denied" and l.get("host")=="example.com" for l in L), L'
ok "the refused host is recorded (seen, counted) and logged"
set +e
K2_NO_APPROVE=1 K2 app egress add esc-a example.com --reason e2e >/tmp/eadd.txt 2>&1; rc=$?
set -e
[ "$rc" = 2 ] && grep -q "held for the owner" /tmp/eadd.txt || fail "egress add: rc $rc $(cat /tmp/eadd.txt)"
REQ="$(K2 app approvals --json | python3 -c 'import json,sys; print([r for r in json.load(sys.stdin)["requests"] if r["appId"]=="esc-a"][0]["id"])')"
set +e
K2 app approve "$REQ" </dev/null >/tmp/eappr.txt 2>&1; rc=$?
set -e
[ "$rc" != 0 ] || fail "approved without an owner confirm: $(cat /tmp/eappr.txt)"
curl -s -m 30 "http://127.0.0.1:$FA/egress?target=example.com:443" | python3 -c 'import json,sys; v=json.load(sys.stdin); assert v["connect"]["status"]==403, v'
ok "egress add is held for the owner (exit 2); without a confirm nothing changes"
# The confirm (Touch ID / password) is covered by the unit tests; here the
# approval is applied as the daemon would, then a narrowing-free reload.
DBF="$K2HOME/.k2/k2.db"; [ -f "$DBF" ] || DBF="$K2HOME/.k2/k2so.db"
python3 - "$DBF" "$REQ" <<'PY'
import sqlite3, sys
c = sqlite3.connect(sys.argv[1])
c.execute("UPDATE app_egress_hosts SET status='allowed', request_id=NULL WHERE app_id='esc-a' AND request_id=?", (sys.argv[2],))
c.execute("UPDATE app_widenings SET status='approved', decided_by='e2e' WHERE id=?", (sys.argv[2],))
c.commit()
PY
K2 app egress mode esc-a allowlist >/dev/null
curl -s -m 30 "http://127.0.0.1:$FA/egress?target=example.com:443" >/tmp/ea2.json
jq_ 'assert v["connect"]=={"status":200}, v["connect"]; assert v["fetch"]["status"]==200, ("fetch through NODE_USE_ENV_PROXY", v["fetch"]); assert v["metadata"]["status"]==403' /tmp/ea2.json
K2 app egress rm esc-a example.com >/dev/null
curl -s -m 30 "http://127.0.0.1:$FA/egress?target=example.com:443" | python3 -c 'import json,sys; v=json.load(sys.stdin); assert v["connect"]["status"]==403 and v["fetch"]["status"]!=200, v'
ok "allowed: CONNECT 200 and Node fetch() reaches https://example.com through the proxy; rm applies at once"

echo "== zero-downtime switch under load =="
LOG=/tmp/load.txt
: >"$LOG"
(for _ in $(seq 1 600); do curl -s -o /dev/null -w '%{http_code} ' -m 5 "http://127.0.0.1:$FA/slow?ms=200" >>"$LOG"; sleep 0.05; done) &
LOADER=$!
sleep 2
K2 app deploy esc-a "$WS/a2" --no-wait >/dev/null || true
r="$(wait_dep esc-a)"
[ "$r" = live ] || fail "esc-a v2: $r"
wait "$LOADER"
tot="$(wc -w <"$LOG")"; bad="$(tr ' ' '\n' <"$LOG" | grep -c -v '^200$' || true)"
[ "$(curl -s "http://127.0.0.1:$FA/")" = a2 ] || fail "front esc-a v2"
[ "$bad" = 0 ] || fail "$bad of $tot requests failed across the switch: $(tr ' ' '\n' <"$LOG" | sort | uniq -c | tr '\n' ' ')"
ok "a1 → a2 under load: $tot requests, 0 failed (the new container was created and health-gated before the switch)"
for _ in $(seq 1 60); do
	n="$(asapp podman ps --format '{{.Names}}' | grep -c '^k2app-esc-a--' || true)"
	[ "$n" = 1 ] && break
	sleep 1
done
[ "$n" = 1 ] || fail "after the watch window esc-a still runs $n containers"
ok "after the watch window the warm previous container stopped"

echo "== logs: rate limited per unit =="
curl -s "http://127.0.0.1:$FA/flood" >/dev/null
sleep 3
UNIT="$(asapp podman ps --format '{{.Names}}' | grep '^k2app-esc-a--' | head -n1).service"
n="$(journalctl --no-pager -o cat _SYSTEMD_USER_UNIT="$UNIT" 2>/dev/null | grep -c 'flood a2' || true)"
[ "$n" -lt 20000 ] || fail "flood lines kept: $n of 20000 (no rate limit)"
# journald reports the suppressed count when the 30 s window closes.
SUP=0
for _ in $(seq 1 45); do
	journalctl --no-pager --since "-3min" | grep -q "Suppressed [0-9]* messages from user@$(id -u k2app).service" && { SUP=1; break; }
	sleep 1
done
[ "$SUP" = 1 ] || fail "no journald suppression for user@k2app"
ok "20000 log lines in a burst: the journal kept $n; journald suppressed the rest (LogRateLimit on user@k2app, all Apps together)"

echo "== crash: K2 restarts it (backoff) =="
asapp podman kill "${UNIT%.service}" >/dev/null 2>&1 || true
up=0
for _ in $(seq 1 120); do
	[ "$(curl -s -m 2 "http://127.0.0.1:$FA/")" = a2 ] && { up=1; break; }
	sleep 1
done
sleep 2
[ "$up" = 1 ] && [ "$(curl -s -m 2 "http://127.0.0.1:$FA/")" = a2 ] || fail "esc-a did not come back after a crash"
ok "a killed container is restarted by the supervisor"

echo "== lane D: MAC, io.max, exec, shell =="
UNIT="$(asapp podman ps --format '{{.Names}}' | grep '^k2app-esc-a--' | head -n1).service"
CPID="$(asapp podman inspect "${UNIT%.service}" --format '{{.State.Pid}}')"
if [ "$(cat /sys/module/apparmor/parameters/enabled 2>/dev/null)" = Y ]; then
	grep -q '^k2-app (enforce)' /sys/kernel/security/apparmor/profiles || fail "k2-app profile not loaded: $(grep k2 /sys/kernel/security/apparmor/profiles)"
	apparmor_parser -Q -K /etc/apparmor.d/k2-app || fail "the canned profile doesn't parse"
	[ "$(cat "/proc/$CPID/attr/current")" = "k2-app (enforce)" ] || fail "container init not under k2-app: $(cat "/proc/$CPID/attr/current")"
	ok "AppArmor: k2-app loaded (enforce); the container runs under it (pid $CPID)"
	K2 app doctor --json | python3 -c 'import json,sys; c=[x for x in json.load(sys.stdin)["checks"] if x["id"]=="mac"][0]; assert c["status"]=="ok", c'
	ok "doctor: mac ok"
else
	info "no AppArmor on $PRETTY_NAME: MAC checks skipped"
fi
CG="$(asapp systemctl --user show -p ControlGroup --value "$UNIT")"
grep -q 'wbps=41943040' "/sys/fs/cgroup$CG/io.max" || fail "io.max on $CG: $(cat "/sys/fs/cgroup$CG/io.max" 2>&1)"
grep -q 'wiops=2000' "/sys/fs/cgroup$CG/io.max" || fail "write IOPS ceiling: $(cat "/sys/fs/cgroup$CG/io.max")"
ok "io.max on the App's unit: $(tr '\n' ' ' <"/sys/fs/cgroup$CG/io.max")"
K2 app config esc-a --io-write 20M >/dev/null
K2 app restart esc-a >/dev/null
r="$(wait_dep esc-a)"; [ "$r" = live ] || fail "esc-a restart after io change: $r"
UNIT="$(asapp podman ps --format '{{.Names}}' | grep '^k2app-esc-a--' | head -n1).service"
CG="$(asapp systemctl --user show -p ControlGroup --value "$UNIT")"
grep -q 'wbps=20971520' "/sys/fs/cgroup$CG/io.max" || fail "io.max after config: $(cat "/sys/fs/cgroup$CG/io.max")"
ok "k2 app config --io-write 20M reaches io.max at the next restart"
out="$(K2 app exec esc-a -- id -u)"; rc=$?
[ "$rc" = 0 ] && printf '%s' "$out" | grep -qx 0 || fail "exec id -u: rc $rc $out"
printf '%s' "$out" | grep -q '<untrusted-app-output app="esc-a" kind="exec">' || fail "exec output is framed: $out"
st="$(K2 app exec esc-a -- cat /proc/self/status)"
printf '%s' "$st" | grep -q '^CapEff:\s*0000000000000000' || fail "exec CapEff: $st"
printf '%s' "$st" | grep -q '^NoNewPrivs:\s*1' || fail "exec NoNewPrivs: $st"
printf '%s' "$st" | grep -q '^Seccomp:\s*2' || fail "exec seccomp: $st"
if [ "$(cat /sys/module/apparmor/parameters/enabled 2>/dev/null)" = Y ]; then
	K2 app exec esc-a -- cat /proc/self/attr/current | grep -q 'k2-app (enforce)' || fail "exec process not under k2-app"
fi
set +e
K2 app exec esc-a -- sh -c 'exit 7' >/dev/null 2>&1; rc=$?
K2 app exec esc-a --timeout 2 -- sleep 30 >/dev/null 2>&1; rt=$?
set -e
[ "$rc" = 7 ] || fail "exec passes the command's exit code (got $rc)"
[ "$rt" = 124 ] || fail "exec timeout exits 124 (got $rt)"
ok "k2 app exec: as the App (uid 0 in its namespace, CapEff 0, NoNewPrivs, seccomp, MAC), framed, exit codes, timeout"
sh_out="$(printf 'echo shell-$((40+2))\nexit 5\n' | K2 app shell esc-a 2>&1)" && rc=0 || rc=$?
printf '%s' "$sh_out" | grep -q 'shell-42' || fail "shell output: $sh_out"
printf '%s' "$sh_out" | grep -q 'start of k2 app shell' || fail "shell banner: $sh_out"
[ "$rc" = 5 ] || fail "shell exit code (got $rc)"
sh_out="$(printf 'printf "\\033]52;c;ZXZpbA==\\007x\\033]0;t\\007y\\n"\nexit\n' | K2 app shell esc-a 2>&1 | od -c)"
printf '%s' "$sh_out" | grep -q ']   5   2' && fail "OSC 52 reached the client: $sh_out"
ok "k2 app shell: piped session, banners, exit code, OSC 52/title stripped"
K2 app history esc-a --json | python3 -c 'import json,sys; ev=[a["event"] for a in json.load(sys.stdin)["audit"]]; assert "exec" in ev and "shell_start" in ev and "shell_end" in ev, ev'
grep -q '"event":"app-exec"' "$K2HOME/.k2/auth-audit.jsonl" || fail "Access Audit has no app-exec"
ok "exec and shell are in the deploy audit and the Access Audit"

echo "== youki (opt-in) =="
set +e
K2 app box prepare --runtime youki >/tmp/py.txt 2>&1; rc=$?
set -e
if [ "$rc" = 0 ]; then
	K2 app restart esc-b >/dev/null
	r="$(wait_dep esc-b)"
	[ "$r" = live ] || fail "esc-b on youki: $r"
	CB="$(asapp podman ps --format '{{.Names}}' | grep '^k2app-esc-b--' | head -n1)"
	case "$(asapp podman inspect "$CB" --format '{{.OCIRuntime}}')" in youki | */youki) ;; *) fail "esc-b runtime" ;; esac
	curl -s "http://127.0.0.1:$FB/probe" >/tmp/py.json
	jq_ 'assert v["capEff"]=="0000000000000000" and v["seccomp"]=="2" and v["unshareU"]["ok"] is False and v["gateway"]["link"]=="gw:38601", v' /tmp/py.json
	ok "youki: esc-b restarted on youki, same floor from inside (CapEff 0, seccomp, no unshare -U, gateway)"
	K2 app box prepare --runtime crun >/dev/null
else
	grep -q youki_unavailable /tmp/py.txt || fail "youki prepare: $(cat /tmp/py.txt)"
	info "no youki package on $PRETTY_NAME (phase B tries the upstream release)"
fi

echo "== uninstall --purge (owner) =="
K2 app uninstall esc-b --purge >/dev/null
[ -z "$(asapp podman ps -a --format '{{.Names}}' | grep '^k2app-esc-b--' || true)" ] || fail "esc-b container left"
[ ! -e /var/lib/k2app/apps/esc-b ] || fail "esc-b data left"
runuser -u k2app -- cat /var/lib/k2app/engine/slots.json | grep -q esc-b && fail "esc-b slot not freed"
nft list chain inet k2app k2app_apps | grep -q esc_b && fail "esc-b nft rule left"
ok "purged: container, data, id slot and gateway rule gone"

echo "== phase B: a fake daemon as k2-daemon.service's main process =="
SEED="$(K2 app inspect esc-a --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["app"]["liveSeed"])')"
IMAGE="$(asapp podman images --format '{{.Repository}}:{{.Tag}}' | grep '^localhost/k2-base/' | head -n1)"
[ -n "$SEED" ] && [ -n "$IMAGE" ] || fail "seed/image for phase B"
K2 app stop esc-a >/dev/null
systemctl stop k2-daemon.service
install -d -o k2 -g k2 -m 0700 /run/k2fd
mkfifo /run/k2fd/req && chown k2:k2 /run/k2fd/req
cat >/opt/fake-daemon.py <<'PY'
import json, os, socket, subprocess
D = "/run/k2fd"
def engine(line):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.connect("/run/k2app/engine.sock")
    s.sendall(line.encode() + b"\n"); s.shutdown(socket.SHUT_WR)
    b = b""
    while True:
        x = s.recv(65536)
        if not x: break
        b += x
    return b.decode()
while True:
    with open(f"{D}/req") as f:
        for raw in f:
            rid, mode, body = raw.rstrip("\n").split(" ", 2)
            if mode == "daemon":
                out = engine(body)
            elif mode == "agent":
                out = subprocess.run(["python3", "/it/tests/engine_client.py", body], capture_output=True, text=True).stdout
            elif mode == "helper":
                p = subprocess.run(["sudo", "-n", "/usr/local/libexec/k2-root-helper"] + body.split(), capture_output=True, text=True)
                out = json.dumps({"rc": p.returncode, "out": p.stdout, "err": p.stderr})
            else:
                # An agent: a terminal the daemon spawned (its own session,
                # like the daemon's PTY spawn), not the daemon's own sudo.
                p = subprocess.run(["bash", "-c", "sudo -n /usr/local/libexec/k2-root-helper \"$@\"; exit $?", "agent"] + body.split(), capture_output=True, text=True, start_new_session=True)
                out = json.dumps({"rc": p.returncode, "out": p.stdout, "err": p.stderr})
            with open(f"{D}/{rid}.tmp", "w") as o: o.write(out)
            os.rename(f"{D}/{rid}.tmp", f"{D}/{rid}.out")
PY
mkdir -p /etc/systemd/system/k2-daemon.service.d
printf '[Service]\nExecStart=\nExecStart=/usr/bin/python3 /opt/fake-daemon.py\n' >/etc/systemd/system/k2-daemon.service.d/zz-fake.conf
systemctl daemon-reload
systemctl start k2-daemon.service
sleep 1
N=0
req() { # mode body → answer (a fresh id per call: callers pipe it, so no shared counter)
	local id="r$RANDOM$RANDOM$RANDOM"
	echo "$id $1 $2" >/run/k2fd/req
	for _ in $(seq 1 1200); do [ -f "/run/k2fd/$id.out" ] && break; sleep 0.25; done
	cat "/run/k2fd/$id.out"
}
req daemon '{"verb":"version"}' | grep -q '"ok":true' || fail "the daemon's main process was refused"
req agent '{"verb":"version"}' | grep -q caller_not_daemon || fail "the daemon's child was admitted"
ok "E18: the engine admits the daemon unit's main process and refuses its child"

PASS_HEX="$(printf 'k2skn_e2e_pass' | od -An -tx1 | tr -d ' \n')"
SK_HEX="$(printf 'sk_e2e_value' | od -An -tx1 | tr -d ' \n')"
DBJ_HEX="$(printf '{"db":1}' | od -An -tx1 | tr -d ' \n')"
DEP=d-0000000000f1
START="{\"verb\":\"start\",\"app\":\"esc-a\",\"deployment\":\"$DEP\",\"seed\":\"$SEED\",\"image\":\"$IMAGE\",\"port\":41999,\"argv\":[\"node\",\"server.mjs\"],\"env\":{\"NODE_ENV\":\"production\",\"K2_PORT\":\"38600\"},\"memoryMib\":256,\"drainSecs\":3,\"secrets\":[{\"env\":\"STRIPE_KEY\",\"file\":null,\"valueHex\":\"$SK_HEX\"},{\"env\":null,\"file\":\"db.json\",\"valueHex\":\"$DBJ_HEX\"}],\"passHex\":\"$PASS_HEX\"}"
req daemon '{"verb":"prepare","app":"esc-a","image":"'"$IMAGE"'","seed":"'"$SEED"'"}' | grep -q '"ok":true' || fail "prepare"
req daemon "$START" >/tmp/st.json
grep -q '"ok":true' /tmp/st.json || fail "start with secrets: $(cat /tmp/st.json)"
for _ in $(seq 1 100); do curl -sf "http://127.0.0.1:41999/healthz" >/dev/null && break; sleep 0.3; done
curl -s "http://127.0.0.1:41999/probe" >/tmp/ps.json
jq_ 'assert v["passFile"]["ok"] and v["passFile"]["out"]=="k2skn_e2e_pass", v["passFile"]; assert "db.json" in v["secretsDir"]["out"], v["secretsDir"]; assert "STRIPE_KEY" in v["env"], v["env"]' /tmp/ps.json
Q="/var/lib/k2app/.config/containers/systemd/k2app-esc-a--$DEP.container"
for f in "$Q" "/var/lib/k2app/engine/run/esc-a/$DEP.env"; do
	grep -q -e k2skn_e2e_pass -e sk_e2e_value "$f" && fail "a secret value is in $f"
done
asapp podman inspect "k2app-esc-a--$DEP" | grep -q -e k2skn_e2e_pass -e sk_e2e_value && fail "a secret value is in podman inspect"
ok "App pass at /run/k2/pass (K2_PASS_FILE), file and env secrets arrive as Podman secrets; none in the unit, env file or inspect"
req daemon '{"verb":"stop","app":"esc-a","deployment":"'"$DEP"'"}' | grep -q '"ok":true' || fail "stop"
[ -z "$(asapp podman secret ls --format '{{.Name}}' | grep -- "--$DEP--" || true)" ] || fail "secrets left after stop"
ok "stop removes the deployment's Podman secrets"

echo "== youki (opt-in) from the upstream release when the distro has none =="
if [ ! -x /usr/bin/youki ]; then
	curl -fsSL -o /tmp/youki.tgz "https://github.com/youki-dev/youki/releases/download/v0.7.0/youki-0.7.0-$ARCH-gnu.tar.gz"
	tar -xzf /tmp/youki.tgz -C /tmp youki
	install -m 0755 /tmp/youki /usr/bin/youki
fi
info "youki $(/usr/bin/youki --version | head -n1)"
req helper "apps runtime youki" >/tmp/yk.json
jq_ 'assert v["rc"]==0, v' /tmp/yk.json
DEP2=d-0000000000f2
req daemon "$(printf '%s' "$START" | sed "s/$DEP/$DEP2/; s/41999/41998/")" >/tmp/st2.json
if grep -q '"ok":true' /tmp/st2.json; then
	for _ in $(seq 1 100); do curl -sf "http://127.0.0.1:41998/healthz" >/dev/null && break; sleep 0.3; done
	[ "$(asapp podman inspect "k2app-esc-a--$DEP2" --format '{{.OCIRuntime}}')" = youki ] || fail "not on youki"
	curl -s "http://127.0.0.1:41998/probe" >/tmp/py.json
	jq_ 'assert v["capEff"]=="0000000000000000" and v["noNewPrivs"]=="1" and v["seccomp"]=="2", v; assert v["unshareU"]["ok"] is False, v["unshareU"]; assert v["writeRoot"]["ok"] is False and v["writeData"]["ok"] is True; assert v["passFile"]["out"]=="k2skn_e2e_pass"' /tmp/py.json
	req daemon '{"verb":"stop","app":"esc-a","deployment":"'"$DEP2"'"}' | grep -q '"ok":true' || fail "stop youki"
	ok "youki: the same container floor from inside (CapEff 0, NoNewPrivs, seccomp, no unshare -U, ro root, data rw, pass secret)"
else
	# youki 0.7.0 rootless under Podman 5.4.2 can't run the floor: the
	# read-only root, the tmpfs mounts and the App's id map each fail with
	# "failed to prepare rootfs" (probed one flag at a time).
	journalctl --no-pager -o cat _SYSTEMD_USER_UNIT="k2app-esc-a--$DEP2.service" | grep -q "prepare rootfs" \
		|| fail "youki start failed another way: $(cat /tmp/st2.json) $(journalctl --no-pager -o cat _SYSTEMD_USER_UNIT="k2app-esc-a--$DEP2.service" | tail -n 5)"
	[ -z "$(asapp podman ps -a --format '{{.Names}}' | grep -- "--$DEP2" || true)" ] || fail "a youki container was left"
	info "youki $(/usr/bin/youki --version | head -n1 | awk '{print $NF}') can't run the guarded floor rootless (read-only root, tmpfs, id map: 'failed to prepare rootfs')"
	ok "youki fails closed: the start errors, nothing is left, nothing falls back to crun"
fi
req helper "apps runtime crun" | grep -q '"rc": 0' || fail "back to crun"

echo "== pg hba-app against a real Postgres =="
HBA="$(ls /etc/postgresql/*/main/pg_hba.conf | head -n1)"
req helper "pg hba-app add esc-a" >/tmp/hba.json
jq_ 'assert v["rc"]==0, v' /tmp/hba.json
grep -q '^host app_esc_a app_esc_a 127.0.0.1/32 scram-sha-256$' "$HBA" || fail "hba line"
runuser -u postgres -- psql -tAc "select count(*) from pg_hba_file_rules where database[1]='app_esc_a'" | grep -qx 1 || fail "Postgres did not load the rule"
req agent-helper "pg hba-app remove esc-a" >/tmp/hba2.json
jq_ 'assert v["rc"]==77, v' /tmp/hba2.json
req helper "pg hba-app remove esc-a" >/tmp/hba3.json
jq_ 'assert v["rc"]==0, v' /tmp/hba3.json
grep -q app_esc_a "$HBA" && fail "hba line left"
ok "pg hba-app: the line lands above every rule, Postgres reloads it, the daemon's child is refused (77), remove restores the file"

echo
echo "apps_guarded_e2e: $pass checks passed"
