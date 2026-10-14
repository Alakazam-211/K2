#!/usr/bin/env bash
# App runtime box prep (root helper `apps` namespace, protocol 6) —
# runs AS ROOT INSIDE a throwaway systemd container (Debian 13), never on
# a real box. Driven by tests/linux/run_apps_box_it.sh, which builds the
# test-keys helper and mounts:
#   /it/bin/k2-root-helper-cur   test-keys build
#   /it/scripts/                 install-root-helper.sh
#
# The container runs in its own user namespace (container root is not
# host root) and network namespace, so the verbs that touch the kernel
# (nft, ip_local_reserved_ports) change only this container. io.cost
# lives only at the host's cgroup root: here the verb must report
# "available": false, which is checked too.
#
# A fake daemon (bash) runs as user k2 in k2-daemon.service and calls the
# helper from its MAIN process ("daemon") or from a child ("agent"), like
# tests/linux/root_helper_in_container.sh. Every check fails loudly; the
# first failure stops the run. Lines starting "  info" are measurements
# that don't gate (nested rootless Podman inside a container).
set -euo pipefail

H=/usr/local/libexec/k2-root-helper
CUR=/it/bin/k2-root-helper-cur
pass=0
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "  ok $*"; pass=$((pass + 1)); }
info() { echo "  info $*"; }

echo "== facts =="
. /etc/os-release
. /it/tests/apps_box_netns.sh
echo "  os: $PRETTY_NAME; kernel $(uname -r); $(systemctl --version | head -n1)"
echo "  cgroup controllers: $(cat /sys/fs/cgroup/cgroup.controllers)"
echo "  uid_map: $(tr -s ' ' </proc/self/uid_map)"

# ── the fake daemon ──────────────────────────────────────────────────
id k2 >/dev/null 2>&1 || useradd -m -s /bin/bash k2
install -d -o k2 -g k2 -m 0700 /run/k2fd
mkfifo /run/k2fd/req && chown k2:k2 /run/k2fd/req
cat >/opt/fake-daemon.sh <<'SH'
#!/bin/bash
D=/run/k2fd
while true; do
	read -r id mode args <"$D/req" || continue
	# shellcheck disable=SC2086
	set -- $args
	if [ "$mode" = daemon ]; then
		sudo -n /usr/local/libexec/k2-root-helper "$@" </dev/null >"$D/$id.out" 2>"$D/$id.err"
		echo $? >"$D/$id.rc.tmp"
	else
		bash -c 'sudo -n /usr/local/libexec/k2-root-helper "$@"; rc=$?; exit $rc' agent "$@" \
			</dev/null >"$D/$id.out" 2>"$D/$id.err"
		echo $? >"$D/$id.rc.tmp"
	fi
	mv "$D/$id.rc.tmp" "$D/$id.rc"
done
SH
chmod 0755 /opt/fake-daemon.sh
k2it_netns_up
cat >/etc/systemd/system/k2-daemon.service <<UNIT
[Unit]
Description=fake K2 daemon (apps box prep integration test)
[Service]
Type=simple
User=k2
ExecStart=/opt/fake-daemon.sh
NetworkNamespacePath=$K2IT_NS
UNIT
systemctl daemon-reload
systemctl start k2-daemon.service
sleep 1
systemctl is-active --quiet k2-daemon.service || fail "fake daemon did not start"

N=0
req() {
	local mode="$1"
	shift
	N=$((N + 1))
	local id="$N"
	echo "$id $mode $*" >/run/k2fd/req
	# apt-get can take minutes.
	for _ in $(seq 1 6000); do
		[ -f "/run/k2fd/$id.rc" ] && break
		sleep 0.1
	done
	[ -f "/run/k2fd/$id.rc" ] || fail "request $id ($mode $*) timed out"
	RC="$(cat "/run/k2fd/$id.rc")"
	OUT="/run/k2fd/$id.out"
	ERR="$(cat "/run/k2fd/$id.err")"
}
expect_rc() {
	[ "$RC" = "$1" ] || fail "$2: rc $RC (want $1): $ERR $(head -c 600 "$OUT")"
}
jq_() { python3 -c 'import json,sys; v=json.load(open(sys.argv[1])); exec(sys.argv[2])' "$OUT" "$1"; }

echo "== install the protocol-6 root helper =="
K2_ALLOW_TEST_KEYS_BUILD=1 bash /it/scripts/install-root-helper.sh --file "$CUR" >/tmp/irh.log 2>&1 \
	|| { cat /tmp/irh.log; fail "install-root-helper.sh"; }
V="$(runuser -u k2 -- sudo -n "$H" helper version)"
echo "$V" | grep -q '"protocol":6' || fail "helper version: $V"
echo "$V" | grep -q '"apps":true' || fail "namespaces.apps: $V"
ok "helper answers protocol 6 with the apps namespace"

echo "== argv and caller refusals (nothing changes) =="
for a in "apps process start ../k2-daemon" "apps process start Jobs" "apps nft apply 1-65535 38900-38949 5432" \
	"apps runtime runc" "apps packages install netcat" "apps cores-slice 768M" "apps linger on k2" "apps ensure-user root"; do
	req daemon $a
	expect_rc 2 "refused argv: $a"
done
for a in "apps unprep" "apps process start jobs" "apps runtime youki" "apps cores-slice 768" \
	"apps budget 4096 4096" "apps nft remove" "apps linger off" "apps reserve-ports off" "apps io-cost off"; do
	req agent $a
	expect_rc 77 "agent: $a"
done
id k2app >/dev/null 2>&1 && fail "a refusal created k2app"
ok "bad argv exits 2; daemon-only verbs from an agent exit 77; nothing created"

echo "== packages: the distro's Podman >= 5 with pasta and crun =="
req daemon apps packages install
expect_rc 0 "apps packages install"
jq_ 'assert v["podmanOk"] and v["pasta"] and v["crun"], v; print("    podman", v["podman"], "crun", v["crun"], "conmon", v["conmon"])'
command -v podman >/dev/null && [ -x /usr/bin/pasta ] || fail "podman/pasta not installed"
ok "apt installed podman $(podman --version | awk '{print $3}'), crun, passt, uidmap"

echo "== the k2app user =="
req agent apps ensure-user
expect_rc 0 "ensure-user (open: an agent may converge it)"
jq_ 'assert v["created"] is True, v'
U="$(id -u k2app)"
[ "$U" -lt 1000 ] || fail "k2app is not a system user ($U)"
[ "$(getent passwd k2app | cut -d: -f6,7)" = "/var/lib/k2app:/usr/sbin/nologin" ] || fail "home/shell: $(getent passwd k2app)"
[ "$(stat -c '%U %a' /var/lib/k2app)" = "k2app 750" ] || fail "home: $(stat -c '%U %a' /var/lib/k2app)"
case "$(getent shadow k2app | cut -d: -f2)" in '!'*|'*'*) ;; *) fail "k2app has a usable password field" ;; esac
id -nG k2app | grep -qwE 'sudo|adm|docker|kvm' && fail "k2app in a privileged group: $(id -nG k2app)"
req daemon apps ensure-user
expect_rc 0 "ensure-user again"
jq_ 'assert v["created"] is False, v'
ok "k2app uid $U: system, nologin, locked, home 0750; re-run changes nothing"

echo "== subordinate ids =="
req daemon apps subid
expect_rc 0 "subid"
grep -q "^k2app:1048576:4194304$" /etc/subuid || fail "subuid: $(cat /etc/subuid)"
grep -q "^k2app:1048576:4194304$" /etc/subgid || fail "subgid: $(cat /etc/subgid)"
before="$(md5sum /etc/subuid /etc/subgid)"
req daemon apps subid
expect_rc 0 "subid again"
[ "$before" = "$(md5sum /etc/subuid /etc/subgid)" ] || fail "subid re-run changed the files"
ok "k2app:1048576:4194304 in subuid and subgid (above $(grep -v k2app /etc/subuid | tr '\n' ' ')); re-run is a no-op"

echo "== delegation, then linger =="
req daemon apps delegate on
expect_rc 0 "delegate on"
[ "$(cat "/etc/systemd/system/user@$U.service.d/k2-delegate.conf" | grep '^Delegate=')" = "Delegate=cpu cpuset io memory pids" ] \
	|| fail "delegate drop-in"
req daemon apps linger on
expect_rc 0 "linger on"
[ -e /var/lib/systemd/linger/k2app ] || fail "linger flag"
# logind starts user@ for a lingering user on a real box; a container's
# logind may not, so start it the way logind would.
systemctl start "user@$U.service" || true
for _ in $(seq 1 50); do systemctl is-active --quiet "user@$U.service" && break; sleep 0.2; done
if systemctl is-active --quiet "user@$U.service"; then
	ctl="$(cat "/sys/fs/cgroup/user.slice/user-$U.slice/user@$U.service/cgroup.controllers" 2>/dev/null || echo '?')"
	info "user@$U active; delegated controllers: $ctl"
	for c in cpu memory pids; do echo " $ctl " | grep -q " $c " || fail "controller $c not delegated: $ctl"; done
else
	info "user@$U did not start in this container: $(systemctl status "user@$U.service" --no-pager 2>&1 | tail -n 3 | tr '\n' ' ')"
fi
ok "Delegate drop-in canned; linger on"

echo "== reserved ports (this container's netns) =="
inns cat /proc/sys/net/ipv4/ip_local_reserved_ports >/tmp/rp.before
req daemon apps reserve-ports on
expect_rc 0 "reserve-ports on"
inns grep -q '38600-38999' /proc/sys/net/ipv4/ip_local_reserved_ports || fail "runtime: $(cat /proc/sys/net/ipv4/ip_local_reserved_ports)"
grep -q '^net.ipv4.ip_local_reserved_ports = 38600-38999$' /etc/sysctl.d/90-k2-apps.conf || fail "sysctl file"
req daemon apps reserve-ports off
expect_rc 0 "reserve-ports off"
[ "$(inns cat /proc/sys/net/ipv4/ip_local_reserved_ports)" = "$(cat /tmp/rp.before)" ] || fail "off did not restore"
[ ! -e /etc/sysctl.d/90-k2-apps.conf ] || fail "off left the file"
req agent apps reserve-ports on
expect_rc 0 "reserve-ports on (agent: tightening)"
ok "38600-38999 reserved and released exactly"

echo "== io.cost =="
req daemon apps io-cost on
expect_rc 0 "io-cost on"
if [ -e /sys/fs/cgroup/io.cost.qos ]; then
	jq_ 'assert v["available"] and v["enabled"], v'
	ok "io.cost enabled on $(python3 -c 'import json,sys; print([d["dev"] for d in json.load(open(sys.argv[1]))["devices"]])' "$OUT")"
else
	jq_ 'assert v["available"] is False and v["changed"] is False, v'
	ok "no io.cost.qos at this cgroup root (a container): reported available=false, nothing written"
fi

echo "== nested rootless Podman as k2app (before nft; informational) =="
set +e
pinfo="$(cd / && timeout 120 runuser -u k2app -- env XDG_RUNTIME_DIR="/run/user/$U" podman info --format '{{.Host.OCIRuntime.Name}} rootless={{.Host.Security.Rootless}} net={{.Host.NetworkBackend}} pasta={{.Host.Pasta.Executable}}' 2>&1)"
prc=$?
set -e
info "podman info as k2app (rc $prc): $(echo "$pinfo" | tail -n 2 | tr '\n' ' ')"

echo "== nft: App sockets reach only the gateway/proxy/db ports =="
req agent apps nft apply 38600-38899 38900-38949 5432
expect_rc 0 "nft apply (open)"
inns nft list table inet k2app | grep -q "chain k2app_v1" || fail "chain"
inns nft list table inet k2app | grep -q "meta skuid $U jump k2app_v1" || fail "skuid rule: $(nft list table inet k2app)"
req daemon apps nft apply 38600-38899 38900-38949 5432
expect_rc 0 "nft apply again (atomic replace)"
[ "$(inns nft list tables | grep -c 'inet k2app$')" = 1 ] || fail "one table"
inns python3 -m http.server 38650 --bind 127.0.0.1 >/dev/null 2>&1 &
L1=$!
inns python3 -m http.server 9999 --bind 127.0.0.1 >/dev/null 2>&1 &
L2=$!
inns python3 -m http.server 9998 --bind 0.0.0.0 >/dev/null 2>&1 &
L3=$!
sleep 1
MYIP=203.0.113.1
as_app() { inns runuser -u k2app -- curl -s -o /dev/null -m 3 -w '%{http_code}' "$1" 2>/dev/null || true; }
as_k2() { inns runuser -u k2 -- curl -s -o /dev/null -m 5 -w '%{http_code}' "$1" 2>/dev/null || true; }
[ "$(as_app http://127.0.0.1:38650/)" = 200 ] || fail "k2app → gateway port"
[ "$(as_app http://127.0.0.1:9999/)" = 000 ] || fail "k2app → another loopback port was allowed"
[ "$(as_app "http://$MYIP:9998/")" = 000 ] || fail "k2app → this host's own address was allowed"
[ "$(as_k2 http://127.0.0.1:9999/)" = 200 ] || fail "the daemon user must be unaffected (loopback)"
[ "$(as_k2 "http://$MYIP:9998/")" = 200 ] || fail "the daemon user must be unaffected (own address)"
# Control: without the table k2app reaches both, so the 000s above are nft's.
inns nft delete table inet k2app
[ "$(as_app http://127.0.0.1:9999/)" = 200 ] || fail "control: k2app → loopback without the table"
[ "$(as_app "http://$MYIP:9998/")" = 200 ] || fail "control: k2app → own address without the table"
req daemon apps nft apply 38600-38899 38900-38949 5432
expect_rc 0 "nft re-apply"
[ "$(as_app http://127.0.0.1:9999/)" = 000 ] || fail "re-applied table does not drop"
kill "$L1" "$L2" "$L3"
ok "k2app: gateway port yes; other loopback and this host's address ($MYIP) no (control without the table: yes); the daemon user unaffected"

echo "== the process exception (system unit, User=k2app) =="
req daemon apps process-unit on
expect_rc 0 "process-unit on"
systemctl cat k2app-process@.service >/dev/null || fail "unit not loaded"
req daemon apps process start jobs
expect_rc 3 "start without an engine"
echo "$ERR" | grep -q engine_missing || fail "engine_missing: $ERR"
# A stand-in engine: report what the sandbox gives the App, then idle.
cat >/usr/local/libexec/k2-app-engine <<'SH'
#!/bin/bash
[ "$1" = process-run ] || exit 2
{
	echo "user=$(id -un)"
	if ls /home/k2 >/dev/null 2>&1; then echo home=visible; else echo home=hidden; fi
	if unshare -U true 2>/dev/null; then echo userns=ok; else echo userns=denied; fi
	grep CapBnd /proc/self/status | awk '{print "capbnd="$2}'
	echo "pwd=$(pwd)"
} >report.tmp && mv report.tmp report
exec sleep 300
SH
chmod 0755 /usr/local/libexec/k2-app-engine
req agent apps process start jobs
expect_rc 77 "an agent starting an App"
req daemon apps process start jobs
expect_rc 0 "process start jobs"
R=/var/lib/k2app/apps/jobs/report
for _ in $(seq 1 50); do [ -f "$R" ] && break; sleep 0.2; done
if [ -f "$R" ]; then
	sed 's/^/    /' "$R"
	grep -q '^user=k2app$' "$R" || fail "runs as k2app"
	grep -q '^home=hidden$' "$R" || fail "ProtectHome not applied"
	grep -q '^userns=denied$' "$R" || fail "RestrictNamespaces not applied"
	grep -q '^capbnd=0000000000000000$' "$R" || fail "capabilities not dropped"
	ok "k2app-process@jobs: user k2app, home hidden, userns denied, no capabilities"
else
	info "the unit did not start inside this container: $(systemctl status k2app-process@jobs.service --no-pager 2>&1 | tail -n 4 | tr '\n' ' ')"
	journalctl -u k2app-process@jobs.service --no-pager 2>/dev/null | tail -n 5 | sed 's/^/    /' || true
fi
req daemon apps status
expect_rc 0 "status"
jq_ 'print("    processApps", v["processApps"])'
if systemctl is-active --quiet k2app-process@jobs.service; then
	req daemon apps process-unit off
	expect_rc 3 "process-unit off while an App runs"
	[ -e /etc/systemd/system/k2app-process@.service ] || fail "a refused off removed the unit"
fi
req daemon apps process stop jobs
expect_rc 0 "process stop jobs"
systemctl is-active --quiet k2app-process@jobs.service && fail "still running after stop"
ok "start/stop are daemon-only; the unit is refused while it runs"

echo "== OCI runtime: crun by default, youki opt-in =="
req agent apps runtime crun
expect_rc 0 "runtime crun"
grep -q '^runtime = "crun"$' /etc/k2app/containers.conf || fail "containers.conf"
req daemon apps packages install-youki
if [ "$RC" = 0 ]; then
	req daemon apps runtime youki
	expect_rc 0 "runtime youki"
	grep -q '^runtime = "youki"$' /etc/k2app/containers.conf || fail "containers.conf youki"
	req agent apps status
	jq_ 'assert v["engine"]["runtime"] == "youki", v["engine"]; print("    youki", v["engine"]["youki"])'
	ok "youki $(youki --version 2>/dev/null | head -n1) installed from the distro and recorded as the active runtime"
	req daemon apps runtime crun
	expect_rc 0 "back to crun"
else
	expect_rc 3 "install-youki on a distro without youki"
	echo "$ERR" | grep -q youki_unavailable || fail "youki_unavailable: $ERR"
	info "this distro has no youki package: $ERR"
fi

echo "== cores slice and the Apps budget =="
req daemon apps cores-slice 768
expect_rc 0 "cores-slice 768"
grep -q '^MemoryMin=768M$' /etc/systemd/system/k2cores.slice || fail "slice"
grep -q '^Slice=k2cores.slice$' /etc/systemd/system/k2-daemon.service.d/k2-cores.conf || fail "daemon drop-in"
grep -q KillMode /etc/systemd/system/k2-daemon.service.d/k2-cores.conf && fail "KillMode in the cores drop-in (AR48)"
[ "$(systemctl show k2cores.slice -p MemoryMin --value)" = 805306368 ] || fail "systemd reads MemoryMin: $(systemctl show k2cores.slice -p MemoryMin --value)"
req daemon apps budget 4096 4096
expect_rc 0 "budget"
[ "$(systemctl show "user@$U.service" -p MemoryMax --value)" = 4294967296 ] || fail "budget MemoryMax: $(systemctl show "user@$U.service" -p MemoryMax --value)"
req daemon apps cores-slice off
expect_rc 0 "cores-slice off"
[ ! -e /etc/systemd/system/k2cores.slice ] || fail "slice left"
ok "k2cores.slice MemoryMin 768M (no KillMode), user@k2app MemoryMax 4G; off removes them"

echo "== status (agent) =="
req agent apps status
expect_rc 0 "status"
jq_ '
u=v["user"]; assert u["ok"] and u["uid"]=='"$U"', u
assert v["subid"]["ok"] and v["linger"] and v["delegate"]=="ok", v
assert v["reservedPorts"]["covered"] and v["nft"]["current"] and v["processUnit"]=="ok", v
assert v["engine"]["podmanOk"] and v["engine"]["runtime"]=="crun", v["engine"]
print("    status:", json.dumps({k: v[k] for k in ("linger","delegate","processUnit")}), "ioCost.available", v["ioCost"]["available"])'
ok "apps status reports the prepared box"

echo "== unprep: refused while k2app runs anything, then reverses config =="
runuser -u k2app -- sleep 120 &
SP=$!
sleep 0.5
req daemon apps unprep
expect_rc 3 "unprep while a k2app process runs"
[ -e /etc/systemd/system/k2app-process@.service ] || fail "refused unprep changed something"
kill "$SP" 2>/dev/null || true
wait "$SP" 2>/dev/null || true
systemctl stop "user@$U.service" 2>/dev/null || true
req agent apps unprep
expect_rc 77 "agent unprep"
req daemon apps unprep
expect_rc 0 "unprep"
for f in /etc/systemd/system/k2app-process@.service /etc/sysctl.d/90-k2-apps.conf /etc/k2app/containers.conf \
	"/etc/systemd/system/user@$U.service.d/k2-delegate.conf" "/etc/systemd/system/user@$U.service.d/k2-budget.conf" \
	/var/lib/systemd/linger/k2app; do
	[ ! -e "$f" ] || fail "unprep left $f"
done
inns nft list table inet k2app >/dev/null 2>&1 && fail "unprep left the nft table"
inns grep -q 38600-38999 /proc/sys/net/ipv4/ip_local_reserved_ports && fail "unprep left the reserved range"
id k2app >/dev/null || fail "unprep must keep the user"
grep -q '^k2app:' /etc/subuid || fail "unprep must keep the subids"
ok "unprep reversed every setting, kept k2app, its ids and /var/lib/k2app"

echo "== prepare again from scratch: every verb converges =="
for a in "apps ensure-user" "apps subid" "apps delegate on" "apps linger on" "apps reserve-ports on" \
	"apps io-cost on" "apps nft apply 38600-38899 38900-38949 5432" "apps process-unit on" "apps runtime crun"; do
	req daemon $a
	expect_rc 0 "$a"
done
for a in "apps ensure-user" "apps subid" "apps delegate on" "apps reserve-ports on" "apps process-unit on" "apps runtime crun"; do
	req daemon $a
	expect_rc 0 "$a (again)"
	python3 -c 'import json,sys; v=json.load(open(sys.argv[1])); assert not v.get("changed") and not v.get("created"), v' "$OUT" \
		|| fail "$a changed something on a re-run: $(cat "$OUT")"
done
ok "a second prep is a no-op"

echo "== audit =="
journalctl -t k2-root-helper --no-pager | grep -q '"ns":"apps"' || fail "no apps audit lines"
journalctl -t k2-root-helper --no-pager | grep '"ns":"apps"' | grep -q caller_not_daemon || fail "77s not audited"
ok "apps calls are audited (journal)"

echo
echo "apps_box_in_container: $pass checks passed"
