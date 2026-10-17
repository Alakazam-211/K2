#!/usr/bin/env bash
# K2 installs Podman 5 (0.46.1 lane P) end to end. Runs AS ROOT INSIDE a
# throwaway Ubuntu 24.04 or Debian 12 VM (tests/linux/run_apps_podman5_lima.sh
# makes one with Lima on a dev Mac), never on a real box. The VM has the
# network (box prep installs distro packages; the daemon fetches the pinned
# base image with skopeo).
#
#   $IT/bin/k2-daemon            this checkout's daemon (debug)
#   $IT/bin/k2-root-helper-cur   test-keys helper (protocol 7)
#   $IT/bin/k2-app-engine        this checkout's engine
#   $IT/bin/testsign             the helper's TEST signer (engine-sig)
#   $IT/cli/k2, $IT/scripts/install-root-helper.sh
#   $IT/tests/guarded-fixture/server.mjs
#   $IT/podman/podman-linux-<arch>.tar.gz   the pinned podman-static tarball
#
# What it proves: the distro's Podman 4.x stays installed and untouched;
# prepare refuses without an opt-in and changes nothing; --dry-run changes
# nothing; a tampered tarball never reaches the helper; --install-podman
# installs the pinned bundle and the box is ready; a second prepare keeps
# it; a guarded hello App goes live behind K2's Podman (its Quadlet unit
# calls /opt/k2/podman/bin/podman) with the wall intact; other users keep
# the distro's Quadlet; uninstall waits for Apps, then removes only K2's.
set -Eeuo pipefail

IT="${1:-/it}"
pass=0
fail() {
	echo "FAIL: $*" >&2
	journalctl -u k2-daemon.service --no-pager 2>/dev/null | tail -n 25 >&2 || true
	journalctl --no-pager -n 40 _UID="$(id -u k2app 2>/dev/null || echo 0)" 2>/dev/null | tail -n 25 >&2 || true
	exit 1
}
ok() { echo "  ok $*"; pass=$((pass + 1)); }
trap '_erc=$?; [[ $- == *e* ]] && fail "line $LINENO: $BASH_COMMAND (exit $_erc)"' ERR
jq_() { python3 -c "import json,sys; v=json.load(open(sys.argv[1])); $1" "$2"; }
# has <needle> <text>: never `producer | grep -q` (an early grep exit
# SIGPIPEs the producer, and pipefail turns that into a false failure).
has() { grep -q -- "$1" <<<"$2"; }

ARCH="$(uname -m)"
case "$ARCH" in x86_64) PARCH=amd64 ;; aarch64) PARCH=arm64 ;; *) fail "arch $ARCH" ;; esac
TARBALL="$IT/podman/podman-linux-$PARCH.tar.gz"
[ -s "$TARBALL" ] || fail "no $TARBALL"
VER="$(grep -a -o 'K2_APP_ENGINE_BUILD v=[0-9.]*' "$IT/bin/k2-app-engine" | head -n1 | sed 's/.*v=//')"
[ -n "$VER" ] || fail "engine build marker"
. /etc/os-release
case "$ID $VERSION_ID" in "ubuntu 24.04" | "debian 12") ;; *) fail "this test is for Ubuntu 24.04 / Debian 12, not $ID $VERSION_ID" ;; esac
echo "== facts: $PRETTY_NAME; kernel $(uname -r); $(systemctl --version | head -n1); engine v$VER =="
HOSTIP="$(ip -4 route get 1.1.1.1 | sed -n 's/.* src \([0-9.]*\).*/\1/p')"

echo "== the distro's Podman first (it must survive) =="
export DEBIAN_FRONTEND=noninteractive
apt-get update -q >/dev/null
apt-get install -y -q --no-install-recommends podman >/dev/null
DISTRO_VER="$(podman --version)"
case "$DISTRO_VER" in "podman version 4."*) ;; *) fail "expected a distro Podman 4.x, got $DISTRO_VER" ;; esac
ok "distro: $DISTRO_VER"

useradd -m -s /bin/bash k2
K2HOME=/home/k2
WS=$K2HOME/ws
K2() { runuser -u k2 -- env HOME="$K2HOME" PATH=/usr/local/bin:/usr/bin:/bin K2_PROJECT_PATH="$WS" bash -c 'cd "$0" 2>/dev/null || cd ~; exec '"$IT"'/cli/k2 "$@"' "$WS" "$@"; }
asapp() { runuser -u k2app -- env XDG_RUNTIME_DIR="/run/user/$(id -u k2app)" HOME=/var/lib/k2app bash -c 'cd ~ && "$@"' _ "$@"; }

echo "== helper (protocol 7) + daemon =="
K2_ALLOW_TEST_KEYS_BUILD=1 bash "$IT/scripts/install-root-helper.sh" --file "$IT/bin/k2-root-helper-cur" >/tmp/irh.log 2>&1 \
	|| { cat /tmp/irh.log; fail "install-root-helper.sh"; }
/usr/local/libexec/k2-root-helper helper version | grep -q '"protocol":7' || fail "helper protocol 7"
install -m 0755 "$IT/bin/k2-daemon" /usr/local/bin/k2-daemon
cat >/etc/systemd/system/k2-daemon.service <<UNIT
[Unit]
Description=K2 daemon (Podman 5 e2e)
After=network-online.target
[Service]
Type=simple
User=k2
Environment=HOME=$K2HOME K2_APPS=1 K2_APPS_WATCH_SECS=20 K2_APPS_DRAIN_SECS=3 K2_APPS_STOP_GRACE_SECS=15 K2_APPS_DEPLOY_TIMEOUT_SECS=300 K2_APPS_STALL_SECS=1200
ExecStart=/usr/local/bin/k2-daemon
UNIT
systemctl daemon-reload
systemctl start k2-daemon.service
for _ in $(seq 1 300); do
	[ -s "$K2HOME/.k2/heartbeat.port" ] && curl -sf "http://127.0.0.1:$(cat "$K2HOME/.k2/heartbeat.port")/boot-status" 2>/dev/null | grep -q '"ready"' && break
	sleep 0.2
done
DPORT="$(cat "$K2HOME/.k2/heartbeat.port")"
OWNER_TOKEN="$(cat "$K2HOME/.k2/heartbeat.token")"
ok "helper protocol 7 + k2-daemon on $DPORT"

nothing_of_ours() {
	[ ! -e /opt/k2/podman ] || fail "$1: /opt/k2/podman exists"
	[ ! -e /etc/systemd/user-generators/podman-user-generator ] || fail "$1: generator exists"
	[ ! -e /etc/apparmor.d/k2-podman ] || fail "$1: AppArmor profile exists"
	[ "$(podman --version)" = "$DISTRO_VER" ] || fail "$1: distro Podman changed"
}

echo "== the doctor points at the install =="
set +e
K2 app doctor --json >/tmp/d0.json; rc=$?
set -e
[ "$rc" = 1 ] || fail "doctor before prep: exit $rc"
jq_ 'st={c["id"]:c for c in v["checks"]}; p=st["podman"]; assert p["status"]=="fail", p; assert "--install-podman" in p["fix"], p' /tmp/d0.json
ok "doctor: podman fail, fix = --install-podman"

echo "== no opt-in: refused before any step =="
set +e
K2 app box prepare </dev/null >/tmp/p0.txt 2>&1; rc=$?
set -e
[ "$rc" = 3 ] && grep -q podman_install_needed /tmp/p0.txt || fail "prepare without opt-in: rc $rc $(cat /tmp/p0.txt)"
id k2app >/dev/null 2>&1 && fail "prepare without opt-in made k2app (a step ran)"
nothing_of_ours "no opt-in"
ok "prepare without --install-podman: podman_install_needed (exit 3), no step ran"

echo "== --dry-run changes nothing =="
K2 app box prepare --dry-run --json >/tmp/dr.json
jq_ 'assert v["dryRun"] is True and v["changed"] is False, v; p=v["podman"]; assert p["action"]=="offer", p; assert p["bundle"]["version"]=="5.8.8", p; assert "apps podman install" in v["steps"], v["steps"]' /tmp/dr.json
K2 app box prepare --dry-run --install-podman --json >/tmp/dr2.json
jq_ 'assert v["podman"]["action"]=="install", v["podman"]' /tmp/dr2.json
has "Dry run: nothing changed." "$(K2 app box prepare --dry-run)" || fail "dry run text"
id k2app >/dev/null 2>&1 && fail "dry run made k2app"
nothing_of_ours "dry run"
ok "dry run: offer/install reported, every write listed, nothing changed"

echo "== a tampered tarball never reaches the helper =="
TD=/opt/k2-podman-bad
install -d -m 0755 "$TD"
cp "$TARBALL" "$TD/"
printf 'x' >>"$TD/$(basename "$TARBALL")"
set +e
K2 app box prepare --install-podman --podman-from "$TD" >/tmp/bad.txt 2>&1; rc=$?
set -e
[ "$rc" = 3 ] && grep -q podman_download_failed /tmp/bad.txt || fail "tampered tarball: rc $rc $(cat /tmp/bad.txt)"
has '"verb":"apps podman install"' "$(journalctl -t k2-root-helper --no-pager)" && fail "the helper was called with a tampered tarball"
nothing_of_ours "tampered"
ok "a tampered tarball is refused before the root call"

echo "== --install-podman =="
PD=/opt/k2-podman-it
install -d -m 0755 "$PD"
cp "$TARBALL" "$PD/"
K2 app box prepare --install-podman --podman-from "$PD" >/tmp/p1.txt 2>&1 || { cat /tmp/p1.txt; fail "prepare --install-podman"; }
grep -q "apps podman install" /tmp/p1.txt || fail "the install step: $(cat /tmp/p1.txt)"
set +e
K2 app doctor --json >/tmp/d1.json; rc=$?
set -e
[ "$rc" = 0 ] || { cat /tmp/d1.json; fail "doctor after prep: exit $rc"; }
jq_ 'st={c["id"]:c for c in v["checks"]}; p=st["podman"]; assert p["status"]=="ok" and "5.8.8" in p["detail"] and "K2" in p["detail"], p; assert v["helperStatus"]["engine"]["podmanSource"]=="k2", v["helperStatus"]["engine"]; assert v["helperStatus"]["podmanBundle"]["current"] is True, v["helperStatus"]["podmanBundle"]' /tmp/d1.json
[ "$(/opt/k2/podman/bin/podman --version)" = "podman version 5.8.8" ] || fail "bundle version"
[ "$(podman --version)" = "$DISTRO_VER" ] || fail "the distro Podman changed: $(podman --version)"
dpkg-query -W -f '${Status}' podman | grep -q "install ok installed" || fail "the distro podman package"
[ "$(stat -c '%U %a' /opt/k2/podman/bin/podman)" = "root 755" ] || fail "bundle owner/mode"
[ -f "/etc/containers/containers.rootless.conf.d/$(id -u k2app)/90-k2-podman.conf" ] || fail "k2app's Podman config"
if [ "$(cat /proc/sys/kernel/apparmor_restrict_unprivileged_userns 2>/dev/null)" = 1 ]; then
	grep -q '^k2-podman ' /sys/kernel/security/apparmor/profiles || fail "k2-podman profile not loaded"
	ok "userns AppArmor profiles loaded (Ubuntu restriction on)"
fi
has '"podman":"5.8.8"' "$(journalctl -t k2-root-helper --no-pager | grep -F '"verb":"apps podman install"')" || fail "helper audit line"
grep -q '"event":"app-podman-install"' "$K2HOME/.k2/auth-audit.jsonl" || fail "auth audit row"
ok "K2 Podman 5.8.8 installed, box ready, distro Podman untouched, audited"

echo "== a second prepare keeps it =="
K2 app box prepare --json >/tmp/p2.json
jq_ 'p=v["podman"]; assert p["action"]=="keep" and p["source"]=="k2", p; assert not any(s["verb"]=="apps podman install" for s in v["steps"]), v["steps"]' /tmp/p2.json
ok "prepare again: Podman kept, no reinstall"

echo "== Quadlet: K2's for k2app, the distro's for everyone else =="
has "5.8.8" "$(asapp /etc/systemd/user-generators/podman-user-generator -version 2>&1)" || fail "k2app's Quadlet"
has "${DISTRO_VER#podman version }" "$(runuser -u nobody -- /etc/systemd/user-generators/podman-user-generator -version 2>&1)" || fail "nobody's Quadlet"
ok "k2app gets Quadlet 5.8.8; other users get the distro's"

echo "== engine install =="
ED=/opt/k2-engine-it
install -d -m 0755 "$ED"
install -m 0644 "$IT/bin/k2-app-engine" "$ED/k2-app-engine-linux-$ARCH"
EPROTO="$("$IT/bin/k2-app-engine" version | python3 -c 'import json,sys; print(json.load(sys.stdin)["protocol"])')"
"$IT/bin/testsign" engine-sig "$ED/k2-app-engine-linux-$ARCH" "$VER" "$EPROTO" "$ARCH" >"$ED/k2-app-engine-linux-$ARCH.sig"
K2 app engine install --from "$ED" | tee /tmp/ei.txt | sed 's/^/    /'
grep -q "installed and answering" /tmp/ei.txt || fail "engine install"
grep -q "5.8.8" /tmp/ei.txt || fail "the engine reports K2's Podman: $(cat /tmp/ei.txt)"
ok "engine installed; it runs K2's Podman 5.8.8"

echo "== a walled hello App =="
runuser -u k2 -- bash -c "mkdir -p '$WS' && git -C '$WS' -c init.defaultBranch=main init -q && git -C '$WS' -c user.name=t -c user.email=t@t -c commit.gpgsign=false commit -q --allow-empty -m one"
curl -sf -X POST -H 'Content-Type: application/json' -d "{\"path\":\"$WS\"}" "http://127.0.0.1:$DPORT/cli/projects/add-from-path?token=$OWNER_TOKEN" >/dev/null || fail "workspace"
A="$WS/hello"
runuser -u k2 -- mkdir -p "$A"
printf '{"schema":1,"kind":"app","id":"hello","name":"Hello","version":"1.0.0","host":{"mode":"cmd","start":["node","server.mjs"],"health":{"path":"/healthz"},"drainSecs":3}}\n' >"$A/manifest.json"
printf 'h1' >"$A/tag.txt"
printf '{"daemonPort": %s, "hostIp": "%s"}\n' "$DPORT" "$HOSTIP" >"$A/facts.json"
printf 'GREETING=hello-h1\n' >"$A/.env.k2"
cp "$IT/tests/guarded-fixture/server.mjs" "$A/server.mjs"
chown -R k2:k2 "$A"
K2 app install hello --memory 256M >/dev/null
K2 app deploy hello "$A" --engine guarded --no-wait >/dev/null || true
r=""
for _ in $(seq 1 600); do
	r="$(K2 app deployment hello --json 2>/dev/null | python3 -c 'import json,sys
try:
    print(json.load(sys.stdin)["deployment"]["result"])
except Exception: print("?")')"
	case "$r" in pending | starting | "?") sleep 1 ;; *) break ;; esac
done
[ "$r" = live ] || { K2 app deployment hello --json 2>&1 | tail -c 3000; K2 app logs hello 2>&1 | tail -n 40; fail "hello guarded deploy: $r"; }
FA="$(K2 app list --json | python3 -c "import json,sys; print([a for a in json.load(sys.stdin)['apps'] if a['appId']=='hello'][0]['frontPort'])")"
[ "$(curl -s "http://127.0.0.1:$FA/")" = h1 ] || fail "front hello"
ok "hello is live behind the guarded engine"
UNIT="$(asapp systemctl --user list-units --type=service --no-legend 'k2app-hello*' | awk '{print $1}' | head -n1)"
[ -n "$UNIT" ] || UNIT="$(asapp systemctl --user list-units --type=service --no-legend | awk '/hello/ {print $1}' | head -n1)"
has '^ExecStart=/opt/k2/podman/bin/podman .* run ' "$(asapp systemctl --user cat "$UNIT")" || fail "the App's Quadlet unit doesn't call K2's podman ($UNIT)"
ok "its Quadlet unit ($UNIT) runs /opt/k2/podman/bin/podman"
curl -s "http://127.0.0.1:$FA/probe" >/tmp/pa.json
python3 - /tmp/pa.json <<'PY'
import json, sys
v = json.load(open(sys.argv[1]))
assert v["tag"] == "h1"
assert v["uid"] == 0, v["uid"]
assert v["daemonHome"] is False and v["ownerToken"] is False and v["k2appHome"] is False, v
assert v["writeApp"]["ok"] is False and v["writeRoot"]["ok"] is False, (v["writeApp"], v["writeRoot"])
assert v["writeData"]["ok"] is True, v["writeData"]
assert v["capEff"] == "0000000000000000" and v["capBnd"] == "0000000000000000", (v["capEff"], v["capBnd"])
assert v["noNewPrivs"] == "1" and v["seccomp"] == "2", (v["noNewPrivs"], v["seccomp"])
assert v["unshareU"]["ok"] is False and v["unshareR"]["ok"] is False, (v["unshareU"], v["unshareR"])
for k in ("daemonPort", "hostSsh", "hostIpSsh", "internet"):
    assert v[k] != "open", (k, v[k])
assert v["gateway"]["status"] == 200, v["gateway"]
print("    probe:", {k: v[k] for k in ("uid", "capEff", "seccomp", "noNewPrivs", "daemonPort", "internet", "gateway")})
PY
ok "the wall holds under K2's Podman (uid 0 in its own id range, no caps, seccomp, no new privs, read-only root, no host ports, no internet, gateway reachable)"
if [ "$(cat /sys/module/apparmor/parameters/enabled 2>/dev/null)" = Y ]; then
	APID="$(pgrep -u "$(id -u k2app)" -x conmon | head -n1)"
	LBL="$(for p in /proc/[0-9]*; do l="$(cat "$p/attr/current" 2>/dev/null)"; case "$l" in k2-app*) echo "$l"; break ;; esac; done)"
	case "$LBL" in "k2-app"*) ;; *) fail "no App process carries the k2-app profile (got '$LBL', conmon $APID)" ;; esac
	DEN="$(dmesg | grep 'apparmor="DENIED"' | grep -c -E 'profile="k2-(crun|podman|pasta)"' || true)"
	[ "$DEN" = 0 ] || fail "$DEN AppArmor denials from K2's Podman profiles: $(dmesg | grep 'apparmor="DENIED"' | grep -E 'profile="k2-(crun|podman|pasta)"' | tail -n 3)"
	ok "the App runs as $LBL; no denials from K2's Podman profiles"
fi
# Debian 13's passt (2025_05_03) logged "Flow N (TCP connection
# (spliced)): ERROR on epoll_ctl(): No such file or directory" for Hearth;
# the bundle's pasta must not.
SINCE="$(date '+%Y-%m-%d %H:%M:%S')"
for _ in $(seq 1 300); do curl -s -o /dev/null "http://127.0.0.1:$FA/"; done
sleep 2
EPOLL="$(journalctl --no-pager --since "$SINCE" _UID="$(id -u k2app)" 2>/dev/null | grep -c 'epoll_ctl' || true)"
[ "$EPOLL" = 0 ] || fail "$EPOLL pasta epoll_ctl errors after 300 requests"
ok "300 spliced requests: no pasta epoll_ctl errors ($(/opt/k2/podman/bin/pasta --version | head -n1))"
CG="$(asapp systemctl --user show -p ControlGroup --value "$UNIT")"
MM="$(find "/sys/fs/cgroup$CG" -name memory.max -path '*libpod-payload*' -exec cat {} \; | head -n1)"
[ "$MM" = "268435456" ] || fail "the App's 256M cap: memory.max=$MM under $CG"
ok "its memory cap holds (memory.max $MM in the payload cgroup)"

echo "== uninstall waits for Apps =="
set +e
K2 app box podman uninstall >/tmp/u0.txt 2>&1; rc=$?
set -e
[ "$rc" = 3 ] && grep -q apps_running /tmp/u0.txt || fail "uninstall while an App runs: rc $rc $(cat /tmp/u0.txt)"
[ -x /opt/k2/podman/bin/podman ] || fail "the refused uninstall removed something"
ok "uninstall refused while hello runs"
K2 app uninstall hello --purge >/dev/null
for _ in $(seq 1 60); do
	has hello "$(asapp systemctl --user list-units --type=service --no-legend)" || break
	sleep 1
done
K2 app box podman uninstall | tee /tmp/u1.txt | sed 's/^/    /'
grep -q "/opt/k2/podman" /tmp/u1.txt || fail "uninstall output"
[ ! -e /opt/k2/podman ] || fail "/opt/k2/podman still there"
[ ! -e /etc/systemd/user-generators/podman-user-generator ] || fail "generator still there"
[ ! -e "/etc/containers/containers.rootless.conf.d/$(id -u k2app)" ] || fail "k2app's config still there"
grep -q '^k2-podman ' /sys/kernel/security/apparmor/profiles 2>/dev/null && fail "k2-podman profile still loaded"
[ "$(podman --version)" = "$DISTRO_VER" ] || fail "the distro Podman changed"
grep -q '"event":"app-podman-uninstall"' "$K2HOME/.k2/auth-audit.jsonl" || fail "auth audit row (uninstall)"
set +e
K2 app doctor --json >/tmp/d3.json
set -e
jq_ 'st={c["id"]:c for c in v["checks"]}; p=st["podman"]; assert p["status"]=="fail" and "--install-podman" in p["fix"], p' /tmp/d3.json
ok "uninstall removed only K2's Podman; the distro's is as it was; the doctor offers the install again"

echo "PODMAN5-E2E PASS ($pass checks) on $PRETTY_NAME $ARCH"
