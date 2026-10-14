#!/usr/bin/env bash
# k2-root-helper integration test — runs AS ROOT INSIDE a throwaway
# systemd container (Ubuntu 24.04), never on a real box. Driven by
# tests/linux/run_root_helper_it.sh, which builds the binaries and mounts:
#   /it/bin/k2-root-helper-cur    test-keys build, version = the daemon's
#   /it/bin/k2-root-helper-next   test-keys build, version 0.45.99
#   /it/bin/testsign              k2-root-helper-testsign (TEST key signer)
#   /it/scripts/                  install-root-helper.sh, k2-pg-helper
#
# A fake daemon (bash) runs as user k2 in k2-daemon.service and runs
# `sudo -n /usr/local/libexec/k2-root-helper …` for us from its MAIN
# process ("daemon") or from a child shell ("agent"). Every check fails
# loudly; the first failure stops the run.
set -euo pipefail

H=/usr/local/libexec/k2-root-helper
CUR=/it/bin/k2-root-helper-cur
NEXT=/it/bin/k2-root-helper-next
TS=/it/bin/testsign
CUR_V="$(grep -a -o 'K2_ROOT_HELPER_BUILD v=[0-9.]*' "$CUR" | sed 's/.*v=//')"
NEXT_V="$(grep -a -o 'K2_ROOT_HELPER_BUILD v=[0-9.]*' "$NEXT" | sed 's/.*v=//')"
pass=0
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "  ok $*"; pass=$((pass + 1)); }
echo "helper versions: cur=$CUR_V next=$NEXT_V"
[ -n "$CUR_V" ] && [ -n "$NEXT_V" ] || fail "binaries carry no marker"

# ── the fake daemon ──────────────────────────────────────────────────
install -d -o k2 -g k2 -m 0700 /run/k2fd
mkfifo /run/k2fd/req && chown k2:k2 /run/k2fd/req
cat >/opt/fake-daemon.sh <<'SH'
#!/bin/bash
# Main process: runs sudo itself (caller = this pid = MainPID) for
# "daemon" requests; "agent" requests run under a child shell.
D=/run/k2fd
while true; do
	read -r id mode args <"$D/req" || continue
	in=/dev/null
	[ -f "$D/$id.in" ] && in="$D/$id.in"
	# shellcheck disable=SC2086
	set -- $args
	if [ "$mode" = daemon ]; then
		sudo -n /usr/local/libexec/k2-root-helper "$@" <"$in" >"$D/$id.out" 2>"$D/$id.err"
		echo $? >"$D/$id.rc.tmp"
	elif [ "$mode" = tab ]; then
		# A K2 shell tab: a DIRECT child of the daemon in its own session
		# (setsid, like the PTY spawn) that `exec sudo …` (review #1).
		( exec setsid sudo -n /usr/local/libexec/k2-root-helper "$@" ) <"$in" >"$D/$id.out" 2>"$D/$id.err"
		echo $? >"$D/$id.rc.tmp"
	else
		bash -c 'sudo -n /usr/local/libexec/k2-root-helper "$@"; rc=$?; exit $rc' agent "$@" \
			<"$in" >"$D/$id.out" 2>"$D/$id.err"
		echo $? >"$D/$id.rc.tmp"
	fi
	mv "$D/$id.rc.tmp" "$D/$id.rc"
done
SH
chmod 0755 /opt/fake-daemon.sh
cat >/etc/systemd/system/k2-daemon.service <<'UNIT'
[Unit]
Description=fake K2 daemon (root-helper integration test)
[Service]
Type=simple
User=k2
ExecStart=/opt/fake-daemon.sh
UNIT
systemctl daemon-reload
systemctl start k2-daemon.service
sleep 1
systemctl is-active --quiet k2-daemon.service || fail "fake daemon did not start"

N=0
# req <daemon|agent> [@stdin-file] args... → RC OUT(file) ERR
req() {
	local mode="$1"
	shift
	N=$((N + 1))
	local id="$N"
	if [ "${1:0:1}" = "@" ]; then
		cp "${1:1}" "/run/k2fd/$id.in"
		chown k2:k2 "/run/k2fd/$id.in"
		shift
	fi
	echo "$id $mode $*" >/run/k2fd/req
	for _ in $(seq 1 600); do
		[ -f "/run/k2fd/$id.rc" ] && break
		sleep 0.1
	done
	[ -f "/run/k2fd/$id.rc" ] || fail "request $id ($mode $*) timed out"
	RC="$(cat "/run/k2fd/$id.rc")"
	OUT="/run/k2fd/$id.out"
	ERR="$(cat "/run/k2fd/$id.err")"
}
expect_rc() {
	# expect_rc <rc> <label>
	[ "$RC" = "$1" ] || fail "$2: rc $RC (want $1): $ERR $(head -c 400 "$OUT")"
}

echo "== S0 facts =="
echo "  sudo: $(readlink -f /usr/bin/sudo) ($(sudo -V | head -n1))"
echo "  yama ptrace_scope: $(cat /proc/sys/kernel/yama/ptrace_scope 2>/dev/null || echo n/a)"
echo "  use_pty default: $(sudo -V 2>/dev/null | grep -i 'use_pty\|pseudo-terminal' | head -n1 || true)"

echo "== phase A: a box on the legacy helpers (protocol-2 mail helper, pg script) =="
LEGACY_V=0.45.3
curl -fsSL "https://github.com/Alakazam-211/K2/releases/download/v$LEGACY_V/install-mail-helper.sh" -o /tmp/old-imh.sh
bash /tmp/old-imh.sh --version "$LEGACY_V" >/tmp/old-imh.log 2>&1 || { cat /tmp/old-imh.log; fail "old install-mail-helper.sh"; }
install -m 0755 /it/scripts/k2-pg-helper /usr/local/libexec/k2-pg-helper
printf 'k2 ALL=(root) NOPASSWD: /usr/local/libexec/k2-pg-helper\n' >/etc/sudoers.d/k2-pg-helper
chmod 0440 /etc/sudoers.d/k2-pg-helper
OLD="$(runuser -u k2 -- sudo -n /usr/local/libexec/k2-mail-helper version)"
echo "$OLD" | grep -q '"protocol":2' || fail "legacy mail helper protocol 2: $OLD"
# The older hand-installed mechanism on akzm/lztek, byte for byte: the
# broad sudoers line plus the /usr/local/bin PATH shims that lean on it.
printf 'k2 ALL=(root) NOPASSWD: /usr/bin/systemctl, /usr/bin/chown, /usr/sbin/useradd\n' >/etc/sudoers.d/k2-mail-sidecar
chmod 0440 /etc/sudoers.d/k2-mail-sidecar
visudo -c >/dev/null || fail "k2-mail-sidecar fixture"
printf '#!/bin/sh\nREAL="$1"; shift\nif [ "$(id -un)" = k2 ]; then\n  exec sudo -n "$REAL" "$@"\nfi\nexec "$REAL" "$@"\n' >/usr/local/bin/k2-if-k2-sudo
printf '#!/bin/sh\nexec /usr/local/bin/k2-if-k2-sudo /usr/bin/systemctl "$@"\n' >/usr/local/bin/systemctl
printf '#!/bin/sh\nexec /usr/local/bin/k2-if-k2-sudo /usr/bin/chown "$@"\n' >/usr/local/bin/chown
printf '#!/bin/sh\nexec /usr/local/bin/k2-if-k2-sudo /usr/sbin/useradd "$@"\n' >/usr/local/bin/useradd
chmod 0755 /usr/local/bin/k2-if-k2-sudo /usr/local/bin/systemctl /usr/local/bin/chown /usr/local/bin/useradd
[ "$(wc -c </usr/local/bin/k2-if-k2-sudo)" = 106 ] || fail "k2-if-k2-sudo fixture is not the 106-byte fleet file"
K2PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
[ "$(runuser -u k2 -- env PATH="$K2PATH" sh -c 'command -v systemctl')" = /usr/local/bin/systemctl ] || fail "shim first in PATH"
runuser -u k2 -- env PATH="$K2PATH" systemctl is-active k2-daemon.service >/dev/null || fail "bare systemctl as k2 through the shim + line"
# The direct-wipe hole the legacy helper leaves open: any process of the
# daemon user (an agent) can stop Stalwart and wipe/restore the store.
runuser -u k2 -- sudo -n /usr/local/libexec/k2-mail-helper usage >/dev/null 2>&1 \
	|| fail "legacy mail helper should answer before the root helper is installed"
ok "legacy protocol-2 mail helper + pg helper + k2-mail-sidecar (+ PATH shims) installed the old way"

echo "== phase B: install the root helper over them =="
K2_ALLOW_TEST_KEYS_BUILD=1 bash /it/scripts/install-root-helper.sh --file "$CUR" >/tmp/irh.log 2>&1 \
	|| { cat /tmp/irh.log; fail "install-root-helper.sh"; }
sed 's/^/    | /' /tmp/irh.log
[ "$(stat -c '%U:%G %a' "$H")" = "root:root 755" ] || fail "helper owner/mode"
[ "$(stat -c '%U:%G %a' /etc/sudoers.d/k2-root-helper)" = "root:root 440" ] || fail "sudoers mode"
[ "$(stat -c '%U:%G %a' /etc/k2-root-helper/daemon.toml)" = "root:root 644" ] || fail "toml mode"
visudo -c >/dev/null || fail "visudo -c"
[ ! -e /usr/local/libexec/k2-pg-helper ] && [ ! -e /etc/sudoers.d/k2-pg-helper ] || fail "pg legacy not retired"
[ ! -e /usr/local/libexec/k2-mail-helper ] && [ ! -e /etc/sudoers.d/k2-mail-helper ] || fail "mail legacy not retired at install"
[ ! -e /etc/sudoers.d/k2-mail-sidecar ] || fail "a K2-shaped k2-mail-sidecar must be retired"
BK="$(ls -d /var/backups/k2-root-helper/*/ | head -n1)"
for f in k2-pg-helper k2-mail-helper k2-mail-sidecar k2-if-k2-sudo systemctl chown useradd; do [ -f "$BK/$f" ] || fail "backup lacks $f"; done
for f in k2-if-k2-sudo systemctl chown useradd; do [ ! -e "/usr/local/bin/$f" ] || fail "PATH shim /usr/local/bin/$f not retired"; done
grep -q "path-shims  : retired: /usr/local/bin/k2-if-k2-sudo /usr/local/bin/systemctl /usr/local/bin/chown /usr/local/bin/useradd" /tmp/irh.log || fail "summary: path-shims"
# Without the line AND the shims, a bare read-only systemctl as k2 still
# works (no sudo in the way) — the failure the shims alone would cause.
[ "$(runuser -u k2 -- env PATH="$K2PATH" sh -c 'command -v systemctl')" = /usr/bin/systemctl ] || fail "systemctl resolves to /usr/bin after retirement"
runuser -u k2 -- env PATH="$K2PATH" systemctl is-active k2-daemon.service >/dev/null || fail "bare systemctl is-active as k2 after retirement"
[ "$(stat -c '%U %a' "$BK")" = "root 700" ] || fail "backup dir mode"
grep -q "legacy mail : retired:" /tmp/irh.log || fail "summary: legacy mail"
grep -q "mail-sidecar: retired:" /tmp/irh.log || fail "summary: mail-sidecar"
# The hole is closed: the daemon user (and so any agent) can no longer
# reach the old helper's direct verbs.
if runuser -u k2 -- sudo -n /usr/local/libexec/k2-mail-helper restore-data </dev/null >/dev/null 2>&1; then
	fail "the old helper's restore-data still runs as root"
fi
if runuser -u k2 -- sudo -n -l /usr/local/libexec/k2-mail-helper >/dev/null 2>&1; then
	fail "sudoers still lets k2 run the old mail helper"
fi
visudo -c >/dev/null || fail "visudo -c after retirement"
ok "installed; pg + mail legacy, k2-mail-sidecar and its PATH shims retired (backed up 0700); the direct-wipe hole is closed"
runuser -u k2 -- sudo -n -l "$H" >/dev/null || fail "sudo -n -l as k2"
V="$(runuser -u k2 -- sudo -n "$H" helper version)"
echo "$V" | grep -q "\"version\":\"$CUR_V\"" && echo "$V" | grep -q '"protocol":6' || fail "helper version: $V"
ok "k2 may sudo -n it; helper version = $CUR_V protocol 6"

echo "== caller is the daemon (§6) =="
req daemon keep kek init
expect_rc 0 "daemon kek init"
req daemon keep kek get
expect_rc 0 "daemon kek get"
[ "$(wc -c <"$OUT")" = 32 ] || fail "kek is 32 bytes"
KEK1="$(od -An -tx1 "$OUT" | tr -d ' \n')"
req agent keep kek get
expect_rc 77 "agent (daemon child) kek get"
echo "$ERR" | grep -q caller_not_daemon || fail "77 says caller_not_daemon: $ERR"
set +e
runuser -u k2 -- sudo -n "$H" keep kek get >/tmp/x 2>/tmp/e
rc=$?
set -e
[ "$rc" = 77 ] && [ ! -s /tmp/x ] || fail "a k2 shell outside the unit got the kek (rc $rc)"
req tab keep kek get
expect_rc 77 "a setsid'd direct child of the daemon that exec's sudo (shell tab)"
[ ! -s "$OUT" ] || fail "the shell tab got KEK bytes"
req agent keep status
expect_rc 0 "agent keep status (open)"
grep -q '"kek":"present"' "$OUT" || fail "status: $(cat "$OUT")"
ok "daemon gets the KEK; a daemon child, a setsid shell tab that exec sudo, and a k2 shell get 77; open status works for agents"
for pty in use_pty '!use_pty'; do
	printf 'Defaults %s\n' "$pty" >/etc/sudoers.d/zz-pty && chmod 0440 /etc/sudoers.d/zz-pty
	visudo -c >/dev/null || fail "visudo $pty"
	req daemon keep gen bump
	expect_rc 0 "daemon verb with Defaults $pty"
	req agent keep gen bump
	expect_rc 77 "agent verb with Defaults $pty"
done
rm -f /etc/sudoers.d/zz-pty
ok "both sudo exec shapes (use_pty on/off) pass the daemon and refuse the agent"
req daemon keep kek stage
expect_rc 0 "stage"
req daemon keep kek commit
expect_rc 0 "commit"
req daemon keep kek get
[ "$(od -An -tx1 "$OUT" | tr -d ' \n')" != "$KEK1" ] || fail "rotation changed nothing"
[ "$(stat -c '%U %a' /var/lib/k2-root-helper/keep/k2/kek)" = "root 600" ] || fail "kek mode"
[ "$(stat -c '%U %a' /var/lib/k2-root-helper/keep/k2)" = "root 700" ] || fail "keep dir mode"
ok "two-phase KEK rotation; store root 0700 / 0600"

echo "== vault keeper contract (k2-vault keystore::keeper parses these replies) =="
req daemon keep status
expect_rc 0 "keep status"
python3 - "$OUT" <<'PY' || fail "keep status shape: $(cat "$OUT")"
import json, sys
v = json.load(open(sys.argv[1]))
assert isinstance(v["protocol"], int) and v["protocol"] >= 4, v
assert v["kek"] == "present", v            # KeepStatus.kek_present()
assert isinstance(v["gen"], int), v        # KeepStatus.gen: u64
for k in ("next", "sealed"):
    assert k not in v or v[k] is None or isinstance(v[k], str), v
PY
req daemon keep kek get
expect_rc 0 "kek get"
[ "$(stat -c %s "$OUT")" = 32 ] || fail "kek get must be exactly 32 raw bytes (keeper key_from), got $(stat -c %s "$OUT")"
req daemon keep gen get
expect_rc 0 "gen get"
G0="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["gen"])' "$OUT")" || fail "gen get reply is not {\"gen\":N}: $(cat "$OUT")"
req daemon keep gen bump
expect_rc 0 "gen bump"
G1="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["gen"])' "$OUT")" || fail "gen bump reply is not {\"gen\":N}: $(cat "$OUT")"
[ "$G1" -gt "$G0" ] || fail "gen bump must advance ($G0 -> $G1)"
ok "keep status JSON, 32-byte KEK, {\"gen\":N} get/bump that advances — the shapes the vault keeper reads"

echo "== mail verbs at parity =="
# The real tarballs from the fork mirror: the pin (0.16.25) and both
# upgrade sources (0.16.20, 0.16.10). The helper checks each by sha256.
STALWART_URL=https://github.com/Alakazam-211/stalwart/releases/download
STALWART_TGZ=/tmp/stalwart-0.16.25.tgz
curl -fsSL "$STALWART_URL/v0.16.25/stalwart-x86_64-unknown-linux-gnu.tar.gz" -o "$STALWART_TGZ"
curl -fsSL "$STALWART_URL/v0.16.20/stalwart-x86_64-unknown-linux-gnu.tar.gz" -o /tmp/stalwart-0.16.20.tgz
curl -fsSL "$STALWART_URL/v0.16.10/stalwart-x86_64-unknown-linux-gnu.tar.gz" -o /tmp/stalwart-0.16.10.tgz
req agent helper version
expect_rc 0 "helper version"
python3 - "$OUT" <<'PY' || fail "version json: pin 0.16.25, upgrade sources 0.16.20 + 0.16.10"
import json, sys
v = json.load(open(sys.argv[1]))
assert v["protocol"] >= 4, v["protocol"]
assert v["stalwartPin"] == "0.16.25", v["stalwartPin"]
assert v["stalwartPinSha256"] == "8ba9cc0ea2795121df5c1264af41e82353c5a3d6860161c4ab70817906d912d3"
assert v["stalwartPrevious"] == "0.16.20", v["stalwartPrevious"]
src = [(e["version"], e["sha256"]) for e in v["stalwartUpgradeFrom"]]
assert src == [
    ("0.16.20", "55184f166f89a0918c6523bb70be300370b63934357731b199dc26a12ca15abf"),
    ("0.16.10", "3ec4ab7eff49f61280f2fe2e4f9645ce5308d1840286ef0c0437524f92ac6a33"),
], src
PY
ok "version: pin 0.16.25, stalwartUpgradeFrom 0.16.20 + 0.16.10"
for v in "mail ensure-user" "mail mkdir config" "mail mkdir data" "mail mkdir log" "mail mkdir dropin-dir" \
	"mail chown config" "mail chown data" "mail chown log" "mail render-dropin" "mail systemctl daemon-reload"; do
	# shellcheck disable=SC2086
	req agent $v
	expect_rc 0 "agent $v (open)"
done
req agent @"$STALWART_TGZ" mail install-bin
expect_rc 0 "install-bin pinned tarball"
[ "$(stat -c '%U:%G %a' /usr/local/bin/stalwart)" = "root:root 755" ] || fail "stalwart binary"
printf 'not the pin' >/tmp/bad.tgz
req agent @/tmp/bad.tgz mail install-bin
expect_rc 2 "install-bin refuses a non-pinned tarball"
PW="$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')"
printf '%s' "$PW" >/tmp/pw
req agent @/tmp/pw mail render-unit
expect_rc 77 "agent render-unit (sets a credential: daemon-only)"
req daemon @/tmp/pw mail render-unit
expect_rc 0 "daemon render-unit"
grep -q "STALWART_RECOVERY_ADMIN=admin:$PW" /etc/systemd/system/stalwart.service || fail "unit password"
[ "$(stat -c '%a' /etc/systemd/system/stalwart.service)" = 600 ] || fail "unit mode"
grep -q '^ProtectSystem=strict$' /etc/systemd/system/stalwart.service.d/k2-hardening.conf || fail "drop-in"
id -u stalwart >/dev/null || fail "stalwart user"
[ "$(stat -c '%U' /var/lib/stalwart)" = stalwart ] || fail "chown data"
ok "ensure-user/mkdir/chown/render/install-bin; render-unit is daemon-only"

echo "== Stalwart upgrade path through the root helper (real tarballs) =="
stalwart_v() { /usr/local/bin/stalwart --version 2>/dev/null | grep -o '0\.16\.[0-9]*' | head -n1; }
for from in 0.16.20 0.16.10; do
	# The box runs an upgrade source …
	req agent @/tmp/stalwart-$from.tgz mail install-bin
	expect_rc 0 "install-bin upgrade source $from"
	grep -q "\"installed\": *\"$from\"" "$OUT" || fail "install-bin $from reported $(cat "$OUT")"
	[ "$(stalwart_v)" = "$from" ] || fail "binary is not $from ($(stalwart_v))"
	# … the upgrade snapshots (daemon-only), installs the pin …
	req daemon mail systemctl stop
	expect_rc 0 "stop before snapshot ($from)"
	req daemon mail snapshot-data
	expect_rc 0 "snapshot before upgrade from $from"
	req agent @"$STALWART_TGZ" mail install-bin
	expect_rc 0 "install-bin pin over $from"
	grep -q '"installed": *"0.16.25"' "$OUT" || fail "pin install reported $(cat "$OUT")"
	[ "$(stalwart_v)" = 0.16.25 ] || fail "binary is not 0.16.25 ($(stalwart_v))"
	# … and a failed health check rolls back to exactly what it had.
	req agent @/tmp/stalwart-$from.tgz mail install-bin
	expect_rc 0 "rollback install-bin $from"
	[ "$(stalwart_v)" = "$from" ] || fail "rollback binary is not $from ($(stalwart_v))"
	req daemon mail restore-data
	expect_rc 0 "restore after rollback to $from"
done
req agent @"$STALWART_TGZ" mail install-bin
expect_rc 0 "install-bin pin (final)"
[ "$(stalwart_v)" = 0.16.25 ] || fail "final binary is not 0.16.25"
ok "upgrade 0.16.20 → 0.16.25 and 0.16.10 → 0.16.25 with rollback to each, real tarballs"
req agent mail usage
expect_rc 0 "usage"
grep -q '"freeBytes"' "$OUT" || fail "usage json"
echo "rocksdb bytes" >/var/lib/stalwart/f.sst && chown stalwart: /var/lib/stalwart/f.sst
for v in "mail snapshot-data" "mail restore-data" "mail systemctl stop" "mail systemctl disable-now" "mail remove data"; do
	# shellcheck disable=SC2086
	req agent $v
	expect_rc 77 "agent $v"
done
[ -d /var/lib/stalwart ] || fail "an agent removed data"
req daemon mail snapshot-data
expect_rc 0 "daemon snapshot"
[ -f /var/lib/stalwart.k2-snap/K2-SNAPSHOT.json ] || fail "snapshot marker"
rm /var/lib/stalwart/f.sst
req daemon mail restore-data
expect_rc 0 "daemon restore"
[ -f /var/lib/stalwart/f.sst ] || fail "restore did not bring the file back"
req daemon mail systemctl stop
expect_rc 0 "daemon stop"
ok "snapshot/restore/stop/purge are daemon-only and work for the daemon"
ln -s /etc /var/log/stalwart.lnk
req agent mail remove /var/log/stalwart
expect_rc 2 "a path as a token"
rm /var/log/stalwart.lnk
for t in unit dropin-dir bin log config data; do
	req daemon mail remove "$t"
	expect_rc 0 "daemon remove $t"
done
[ ! -e /var/lib/stalwart ] && [ ! -e /usr/local/bin/stalwart ] && [ ! -e /etc/systemd/system/stalwart.service ] || fail "uninstall"
ok "uninstall/purge through named targets"

echo "== pg namespace =="
systemctl is-active --quiet postgresql || systemctl start postgresql
CONF="$(ls -d /etc/postgresql/*/main/conf.d | head -n1)"
mv "$CONF" "$CONF.real"
ln -s /etc/sudoers.d "$CONF"
req agent pg install-ram-fence
expect_rc 3 "ram fence refuses a symlinked conf.d"
[ ! -e /etc/sudoers.d/99-k2-ram.conf ] || fail "root wrote through the planted link"
[ ! -e /etc/systemd/system/postgresql.service.d/k2-ram.conf ] || fail "fence wrote something before refusing"
rm "$CONF" && mv "$CONF.real" "$CONF"
req agent pg install-ram-fence
expect_rc 0 "ram fence"
[ -f "$CONF/99-k2-ram.conf" ] && [ -f /etc/systemd/system/postgresql@.service.d/k2-ram.conf ] || fail "fence files"
visudo -c >/dev/null || fail "sudoers intact"
ok "install-ram-fence refuses a planted link (RH9), then writes the canned files"
req agent pg systemctl daemon-reload
expect_rc 0 "pg daemon-reload"
req agent pg systemctl restart postgresql
expect_rc 0 "pg restart"
sleep 2
printf 'SELECT 1;' >/tmp/q.sql
req agent @/tmp/q.sql pg psql -d postgres -tA
expect_rc 77 "agent psql"
req daemon @/tmp/q.sql pg psql -d postgres -tA
expect_rc 0 "daemon psql"
[ "$(tr -d ' \n' <"$OUT")" = 1 ] || fail "psql output: $(cat "$OUT")"
printf "COPY (SELECT 1) TO PROGRAM 'id';" >/tmp/q2.sql
req daemon @/tmp/q2.sql pg psql -d postgres -c x
expect_rc 2 "psql -c refused"
ok "pg systemctl + psql (daemon-only, runuser, argv array)"

echo "== self-update / rollback (§9.2, §9.3) =="
"$TS" payload "$NEXT" "$NEXT_V" 4 x86_64 >/tmp/next.payload
cp /tmp/next.payload /tmp/bad.payload
python3 - <<'PY'
p = bytearray(open("/tmp/bad.payload", "rb").read())
p[-200] ^= 0xFF
open("/tmp/bad.payload", "wb").write(p)
PY
SHA_BEFORE="$(sha256sum "$H" | cut -d' ' -f1)"
req agent @/tmp/bad.payload helper self-update
expect_rc 4 "tampered build"
[ "$(sha256sum "$H" | cut -d' ' -f1)" = "$SHA_BEFORE" ] || fail "tampered build changed the helper"
"$TS" payload "$NEXT" "0.45.98" 4 x86_64 >/tmp/lie.payload
req agent @/tmp/lie.payload helper self-update
expect_rc 4 "comment/marker mismatch"
"$TS" sig "$NEXT" "$NEXT_V" 4 x86_64 | sed 's/k2-root-helper-linux-x86_64/k2-daemon-linux-x86_64/' >/tmp/daemon.sig
"$TS" payload-raw "$NEXT" /tmp/daemon.sig >/tmp/daemon.payload
req agent @/tmp/daemon.payload helper self-update
expect_rc 4 "a comment naming the daemon asset"
ok "tampered / mismatched / wrong-file builds refused, helper unchanged"
req daemon @/tmp/next.payload helper self-update
expect_rc 0 "self-update"
grep -q "\"to\":\"$NEXT_V\"" "$OUT" || fail "self-update answer: $(cat "$OUT")"
runuser -u k2 -- sudo -n "$H" helper version | grep -q "\"version\":\"$NEXT_V\"" || fail "new version not running"
cmp -s "$CUR" "$H.prev" || fail ".prev is not the old build"
[ "$(stat -c '%U:%G %a' "$H")" = "root:root 755" ] || fail "updated helper mode"
ok "signed self-update to $NEXT_V; .prev kept byte for byte"
req agent helper self-rollback
expect_rc 77 "agent rollback"
req daemon helper self-rollback
expect_rc 0 "rollback"
runuser -u k2 -- sudo -n "$H" helper version | grep -q "\"version\":\"$CUR_V\"" || fail "rollback version"
grep -q "\"$NEXT_V\"" /var/lib/k2-root-helper/state.json || fail "refused list"
req daemon @/tmp/next.payload helper self-update
expect_rc 4 "reinstall of a rolled-back version"
ok "daemon-only rollback; the bad version is refused afterwards"

echo "== legacy retire + audit =="
req agent helper legacy-retire mail
expect_rc 77 "agent legacy-retire"
req daemon helper legacy-retire mail
expect_rc 0 "legacy-retire mail"
[ ! -e /usr/local/libexec/k2-mail-helper ] && [ ! -e /etc/sudoers.d/k2-mail-helper ] || fail "legacy mail files"
grep -q '"visudo":"ok"' "$OUT" || fail "visudo after retire: $(cat "$OUT")"
req agent helper audit-tail
expect_rc 0 "audit-tail"
grep -q 'caller_not_daemon' "$OUT" || fail "audit-tail lacks the 77 lines"
grep -q '"verb":"helper self-update"' "$OUT" || fail "audit-tail lacks self-update"
if grep -q "$PW" "$OUT" || journalctl -t k2-root-helper --no-pager | grep -q "$PW"; then fail "a password reached the journal"; fi
ok "legacy-retire (daemon-only, visudo ok); audit lines in the journal, no secrets"

echo "== real minisign -t round trip through the same check (S0 #5) =="
minisign -G -W -p /tmp/t.pub -s /tmp/t.key >/dev/null
minisign -S -s /tmp/t.key -m "$NEXT" -x /tmp/real.sig -t "file:k2-root-helper-linux-x86_64 version:$NEXT_V protocol:4" >/dev/null
"$TS" verify "$(sed -n 2p /tmp/t.pub)" "$NEXT" /tmp/real.sig x86_64 | grep -q "ok version=$NEXT_V protocol=4" \
	|| fail "real minisign signature not accepted"
sed -i 's/protocol:4/protocol:5/' /tmp/real.sig
if "$TS" verify "$(sed -n 2p /tmp/t.pub)" "$NEXT" /tmp/real.sig x86_64 >/dev/null 2>&1; then fail "tampered real comment accepted"; fi
ok "a real 'minisign -S -t' signature verifies; a tampered comment does not"

echo "== user-scope daemon (self-hosted install-daemon.sh shape) =="
loginctl enable-linger k2 || true
# logind starts user@<uid> for a lingering user on a real box; in a
# container start it directly (what logind would do).
systemctl start "user@$(id -u k2).service" || true
for _ in $(seq 1 50); do systemctl is-active --quiet user@"$(id -u k2)".service && break; sleep 0.2; done
systemctl is-active --quiet user@"$(id -u k2)".service || fail "user manager for k2 did not start (linger)"
K2_ALLOW_TEST_KEYS_BUILD=1 bash /it/scripts/install-root-helper.sh --file "$CUR" --scope user --unit k2-daemon.service >/tmp/irh2.log 2>&1 \
	|| { cat /tmp/irh2.log; fail "reinstall with --scope user"; }
grep -q '^scope = "user"$' /etc/k2-root-helper/daemon.toml || fail "scope user"
req daemon keep gen bump
expect_rc 77 "the SYSTEM unit is no longer the configured daemon"
systemctl stop k2-daemon.service
install -d -o k2 -g k2 /home/k2/.config/systemd/user
cp /etc/systemd/system/k2-daemon.service /home/k2/.config/systemd/user/k2-daemon.service
sed -i '/^User=k2$/d' /home/k2/.config/systemd/user/k2-daemon.service
chown k2:k2 /home/k2/.config/systemd/user/k2-daemon.service
# No dbus-user-session in this image (like a server box): drive the user
# manager over its private socket — the helper's fallback path.
usys() { runuser -u k2 -- env XDG_RUNTIME_DIR="/run/user/$(id -u k2)" systemctl --user "$@"; }
usys daemon-reload
usys start k2-daemon.service
sleep 1
MP="$(usys show -p MainPID --value k2-daemon.service)"
[ "${MP:-0}" -gt 0 ] || fail "user unit MainPID"
req daemon keep gen bump
expect_rc 0 "user-scope daemon verb"
req agent keep gen bump
expect_rc 77 "user-scope agent verb"
ok "user-scope MainPID (private-socket fallback, no user bus): the user-scope daemon passes, its child is refused"

echo "== a non-k2 daemon user (nsi: alakazamlabs) =="
usys stop k2-daemon.service || true
useradd -m -s /bin/bash alakazamlabs
chown -R alakazamlabs:alakazamlabs /run/k2fd
sed 's/^User=k2$/User=alakazamlabs/' /etc/systemd/system/k2-daemon.service >/etc/systemd/system/k2-daemon-nsi.service
grep -q '^User=alakazamlabs$' /etc/systemd/system/k2-daemon-nsi.service || fail "nsi unit"
systemctl daemon-reload
systemctl start k2-daemon-nsi.service
sleep 1
systemctl is-active --quiet k2-daemon-nsi.service || fail "alakazamlabs fake daemon did not start"
K2_ALLOW_TEST_KEYS_BUILD=1 bash /it/scripts/install-root-helper.sh --file "$CUR" --user alakazamlabs --unit k2-daemon-nsi.service >/tmp/irh3.log 2>&1 \
	|| { cat /tmp/irh3.log; fail "install with --user alakazamlabs"; }
grep -q '^user = "alakazamlabs"$' /etc/k2-root-helper/daemon.toml || fail "daemon.toml user"
grep -q '^unit = "k2-daemon-nsi.service"$' /etc/k2-root-helper/daemon.toml || fail "daemon.toml unit"
grep -q '^alakazamlabs ALL=(root) NOPASSWD: /usr/local/libexec/k2-root-helper$' /etc/sudoers.d/k2-root-helper || fail "sudoers user"
grep -q "check       : alakazamlabs may run it" /tmp/irh3.log || fail "door check as alakazamlabs: $(cat /tmp/irh3.log)"
if runuser -u k2 -- sudo -n -l "$H" >/dev/null 2>&1; then fail "k2 must no longer reach the helper"; fi
req daemon keep kek init
expect_rc 0 "alakazamlabs daemon kek init"
[ "$(stat -c '%U %a' /var/lib/k2-root-helper/keep/alakazamlabs/kek)" = "root 600" ] || fail "keeper path for alakazamlabs"
req daemon keep kek get
expect_rc 0 "alakazamlabs daemon kek get"
req agent keep kek get
expect_rc 77 "alakazamlabs agent kek get"
req agent helper version
expect_rc 0 "alakazamlabs helper version"
ok "--user alakazamlabs: caller check, unit, sudoers and keeper path follow the daemon user"

echo ""
echo "PASS: root helper integration ($pass checks)"
