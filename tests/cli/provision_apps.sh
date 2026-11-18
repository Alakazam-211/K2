#!/usr/bin/env bash
# Hermetic test for step 8b (Apps: box prep, App engine, the agents leaf
# and the doctor's verdict) of scripts/provision-k2-server.sh. The
# provisioner has no dry-run mode and must never run for real here, so this
# test:
#   1. checks the script's shape statically (8b sits after daemon readiness
#      and before the owner user; --allow-not-ready / K2_ALLOW_NOT_READY and
#      K2_CORES_MEM_MIN_MIB are documented and parsed),
#   2. runs the cores-floor sizing rule (k2_cores_mem_min_mib) over a table
#      of box sizes, and
#   3. cuts the 8b block out of the script and runs it alone with a fake
#      `k2` (k2_as_daemon_user), a fake daemon (k2_curl answering the
#      doctor), a fake k2_wait_ready and PATH shims for systemctl and awk
#      (a fake /proc/meminfo). No root, no network, no daemon.
# Run with: bash tests/cli/provision_apps.sh
# Literal $… in single quotes and A && B || fail are intended here.
# shellcheck disable=SC2015,SC2016
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
command -v python3 >/dev/null 2>&1 || { echo "FAIL: provision_apps.sh needs python3 (the provisioner does too)" >&2; exit 1; }
PROVISION="$ROOT/scripts/provision-k2-server.sh"
[ -f "$PROVISION" ] || { echo "FAIL: missing $PROVISION" >&2; exit 1; }

pass=0
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "  ok $*"; pass=$((pass + 1)); }

echo "== static shape =="
bash -n "$PROVISION" || fail "bash -n"
ok "bash -n"
line_of() { grep -n -- "$1" "$PROVISION" | head -n1 | cut -d: -f1; }
L_READY="$(line_of '# ── 8. wait for daemon readiness')"
L_APPS="$(line_of '# ── 8b. Apps')"
L_OWNER="$(line_of '# ── 9. first owner user')"
for v in L_READY L_APPS L_OWNER; do [ -n "${!v}" ] || fail "marker $v not found"; done
[ "$L_READY" -lt "$L_APPS" ] && [ "$L_APPS" -lt "$L_OWNER" ] || fail "8b must come after readiness (8) and before the owner user (9)"
ok "8b runs after 8 and before 9 (no guests or owner logins yet)"
grep -q '^#   K2_ALLOW_NOT_READY ' "$PROVISION" || fail "K2_ALLOW_NOT_READY must be documented in the header"
grep -q -- '--allow-not-ready) ALLOW_NOT_READY=1 ;;' "$PROVISION" || fail "--allow-not-ready must be parsed"
grep -q '^\[ "${K2_ALLOW_NOT_READY:-}" = "1" \] && ALLOW_NOT_READY=1$' "$PROVISION" || fail "K2_ALLOW_NOT_READY=1 must be the env alias"
grep -q '^#   ./provision-k2-server.sh --allow-not-ready$' "$PROVISION" || fail "the usage must show --allow-not-ready"
grep -q '^#   K2_CORES_MEM_MIN_MIB ' "$PROVISION" || fail "K2_CORES_MEM_MIN_MIB must be documented in the header"
ok "--allow-not-ready / K2_ALLOW_NOT_READY / K2_CORES_MEM_MIN_MIB documented and parsed"
BLOCK="$(sed -n '/^# ── 8b\. Apps/,/^# ── 9\. first owner/p' "$PROVISION" | sed '$d')"
[ -n "$BLOCK" ] || fail "8b block not found"
<<<"$BLOCK" grep -q 'k2_as_daemon_user app box prepare --install-podman --cores-mem-min "$APPS_CORES_MIB" --json' \
	|| fail "8b must prepare with --cores-mem-min"
<<<"$BLOCK" grep -q 'systemctl restart "$K2_UNIT_NAME"' || fail "8b must restart the daemon unit for the agents leaf"
<<<"$BLOCK" grep -q 'k2_wait_ready' || fail "8b must wait for the restarted daemon"
grep -q '^k2_wait_ready() {$' "$PROVISION" || fail "the readiness wait must be a function 8b can call again"
ok "8b prepares with --cores-mem-min, restarts the unit and waits for readiness again"

echo "== the cores floor by RAM =="
SIZER="$(sed -n '/^k2_cores_mem_min_mib() {$/,/^}$/p' "$PROVISION")"
[ -n "$SIZER" ] || fail "k2_cores_mem_min_mib not found"
eval "$SIZER"
# MemTotal (KiB) as a real box reports it → the floor (MiB).
while read -r kib want what; do
	got="$(k2_cores_mem_min_mib "$kib")" || fail "k2_cores_mem_min_mib $kib ($what) failed"
	[ "$got" = "$want" ] || fail "$what (MemTotal $kib kB): want $want MiB, got $got"
	ok "$what → $want MiB"
done <<'TABLE'
3884000 948 4GB-board(quarter)
8000000 2048 8GB
16300000 2048 16GB
32786000 2048 32GB
33554432 2048 32GiB-exact
49300000 3072 48GB
65842476 4096 64GB
67108864 4096 64GiB-exact
98600000 6144 96GB
131900000 8192 128GB
134217728 8192 128GiB-exact
263000000 8192 256GB
TABLE
for bad in "" "abc" "0"; do
	if k2_cores_mem_min_mib "$bad" >/dev/null 2>&1; then fail "k2_cores_mem_min_mib '$bad' must fail"; fi
done
ok "an unreadable MemTotal fails (no guessed floor)"

echo "== 8b, run alone =="
WORK="$(mktemp -d -t k2-provision-apps-XXXXXX)"
trap 'rm -rf "$WORK"' EXIT
SHIMS="$WORK/shims"
mkdir -p "$SHIMS"
printf '%s\n' "$BLOCK" >"$WORK/block.sh"
# The block's own helper, as the provisioner defines it.
printf '%s\n' "$SIZER" >"$WORK/sizer.sh"
REAL_AWK="$(command -v awk)"
cat >"$SHIMS/awk" <<SH
#!/bin/sh
# The block runs: awk '<program>' /proc/meminfo. The test's box is \$FAKE_MEMINFO.
if [ "\$#" = 2 ] && [ "\$2" = /proc/meminfo ]; then exec '$REAL_AWK' "\$1" "\$FAKE_MEMINFO"; fi
exec '$REAL_AWK' "\$@"
SH
cat >"$SHIMS/systemctl" <<'SH'
#!/bin/sh
echo "systemctl $*" >>"$FAKE_STATE/calls.log"
[ "$1" = "restart" ] && : >"$FAKE_STATE/restarted"
exit 0
SH
chmod +x "$SHIMS/awk" "$SHIMS/systemctl"

cat >"$WORK/harness.sh" <<'SH'
set -euo pipefail
log() { printf '[provision] %s\n' "$*"; }
die() { printf '[provision] ERROR: %s\n' "$*" >&2; exit 1; }
K2_UNIT_NAME=k2-daemon
K2_RUN_USER=k2
BASE="http://127.0.0.1:1"
TOKEN=tok
ROOT_HELPER_MODE=root
k2_as_daemon_user() {
	echo "k2 $*" >>"$FAKE_STATE/calls.log"
	case "$*" in
		"app box prepare"*) echo '{"ok":true}' ;;
		"app engine install"*) : >"$FAKE_STATE/engine"; echo '{"ok":true}' ;;
		"app box auto-prepare on"*) [ -z "${FAKE_AUTO_FAIL:-}" ] || { echo "refused" >&2; return 3; }; echo '{"ok":true}' ;;
	esac
}
k2_wait_ready() {
	[ -f "$FAKE_STATE/restarted" ] || { echo "k2_wait_ready before the restart" >&2; exit 9; }
	echo "wait-ready" >>"$FAKE_STATE/calls.log"
}
# The fake daemon: GET /cli/app/box. The agents leaf reads ok once the unit
# restarted (or when FAKE_LEAF=ok); readiness is FAKE_READY.
k2_curl() {
	local url="$1"; shift 3
	case "$url" in *"/cli/app/box?"*) ;; *) echo "unexpected k2_curl $url" >&2; return 1 ;; esac
	local leaf="${FAKE_LEAF:-warn}"
	[ -f "$FAKE_STATE/restarted" ] && leaf=ok
	local engine=fail
	[ -f "$FAKE_STATE/engine" ] && engine=ok
	local doc
	doc=$(python3 -c '
import json, sys
leaf, engine, ready, blocking = sys.argv[1:5]
checks = [
  {"id": "kernel", "status": "ok", "detail": "6.8", "fix": None},
  {"id": "nft", "status": "ok" if ready == "true" else "fail", "detail": "table k2app missing", "fix": "k2 app box prepare (owner)"},
  {"id": "agentsLeaf", "status": leaf, "detail": "leaf", "fix": None},
  {"id": "engine", "status": engine, "detail": "engine", "fix": "k2 app engine install (owner)"},
]
d = {"ready": ready == "true", "checks": checks}
if blocking == "yes":
  d["blocking"] = [] if ready == "true" else ["nft"]
  d["autoPrepare"] = {"enabled": False, "setRoute": "/cli/app/box/auto-prepare"}
print(json.dumps(d))
' "$leaf" "$engine" "${FAKE_READY:-true}" "${FAKE_BLOCKING:-yes}")
	case " $* " in
		*" -w "*) printf '%s\n200' "$doc" ;;
		*) printf '%s\n' "$doc" ;;
	esac
}
SH

# run <name> [VAR=value...]: 8b alone; stdout+stderr in $WORK/<name>.out,
# the calls in $WORK/<name>/calls.log; prints the exit code.
run() {
	local name="$1"; shift
	mkdir -p "$WORK/$name"
	: >"$WORK/$name/calls.log"
	set +e
	env -i PATH="$SHIMS:/usr/bin:/bin:/usr/sbin:/sbin" HOME="$WORK" \
		FAKE_STATE="$WORK/$name" FAKE_MEMINFO="$WORK/meminfo-64g" ALLOW_NOT_READY=0 "$@" \
		bash -c 'source "$1"; source "$2"; source "$3"; echo "APPS_LINE=$APPS_LINE"' _ "$WORK/harness.sh" "$WORK/sizer.sh" "$WORK/block.sh" \
		>"$WORK/$name.out" 2>&1
	local rc=$?
	set -e
	echo "$rc"
}
printf 'MemTotal:       65842476 kB\nMemFree:        1000 kB\n' >"$WORK/meminfo-64g"
printf 'MemTotal:       131900000 kB\n' >"$WORK/meminfo-128g"

rc="$(run fresh)"
[ "$rc" = 0 ] || { cat "$WORK/fresh.out" >&2; fail "fresh box: exit $rc"; }
grep -qx 'k2 app box prepare --install-podman --cores-mem-min 4096 --json' "$WORK/fresh/calls.log" \
	|| { cat "$WORK/fresh/calls.log" >&2; fail "a 64 GB box prepares with --cores-mem-min 4096"; }
ok "a 64 GB box prepares with --install-podman --cores-mem-min 4096"
grep -qx 'k2 app engine install --json' "$WORK/fresh/calls.log" || fail "the engine is installed"
order="$(grep -E '^(k2 app box prepare|k2 app engine install|systemctl restart|wait-ready)' "$WORK/fresh/calls.log" | cut -d' ' -f1-3 | tr '\n' '|')"
[ "$order" = "k2 app box|k2 app engine|systemctl restart k2-daemon|wait-ready|" ] \
	|| fail "prepare, engine, then the restart and a fresh readiness wait (got: $order)"
[ "$(grep -c '^systemctl restart' "$WORK/fresh/calls.log")" = 1 ] || fail "exactly one restart"
ok "the agents leaf: one k2-daemon restart in the setup flow, after prepare and the engine, then readiness again"
grep -q 'APPS_LINE=ready (box prepared, App engine installed, cores floor 4096 MiB, automatic prep on)' "$WORK/fresh.out" || { cat "$WORK/fresh.out" >&2; fail "the summary line says ready"; }
grep -q 'k2 app doctor: READY' "$WORK/fresh.out" || fail "READY is logged"
ok "READY at the end; the summary line says so"
grep -qx 'k2 app box auto-prepare on --json' "$WORK/fresh/calls.log" || fail "READY turns on automatic prep"
[ "$(tail -n1 "$WORK/fresh/calls.log")" = 'k2 app box auto-prepare on --json' ] || fail "automatic prep goes on last, after READY"
ok "READY turns on automatic box prep (k2 app box auto-prepare on), last"
rc="$(run autofail FAKE_AUTO_FAIL=1)"
[ "$rc" != 0 ] && grep -q 'turning on automatic box prep failed' "$WORK/autofail.out" || fail "a refused auto-prepare stops provisioning"
ok "a refused k2 app box auto-prepare on stops provisioning"

rc="$(run big FAKE_MEMINFO="$WORK/meminfo-128g")"
[ "$rc" = 0 ] || { cat "$WORK/big.out" >&2; fail "128 GB box: exit $rc"; }
grep -qx 'k2 app box prepare --install-podman --cores-mem-min 8192 --json' "$WORK/big/calls.log" || fail "a 128 GB box gets 8192"
ok "a 128 GB box prepares with --cores-mem-min 8192"

rc="$(run override K2_CORES_MEM_MIN_MIB=3000)"
[ "$rc" = 0 ] || fail "override: exit $rc"
grep -qx 'k2 app box prepare --install-podman --cores-mem-min 3000 --json' "$WORK/override/calls.log" || fail "K2_CORES_MEM_MIN_MIB wins"
rc="$(run badoverride K2_CORES_MEM_MIN_MIB=4G)"
[ "$rc" != 0 ] && grep -q 'K2_CORES_MEM_MIN_MIB must be a number of MiB' "$WORK/badoverride.out" || fail "a non-number override stops"
! grep -q '^k2 app box prepare' "$WORK/badoverride/calls.log" || fail "a bad override changes nothing"
ok "K2_CORES_MEM_MIN_MIB overrides the size; a non-number stops before any change"

rc="$(run leafok FAKE_LEAF=ok)"
[ "$rc" = 0 ] || fail "leaf ok: exit $rc"
! grep -q '^systemctl' "$WORK/leafok/calls.log" || fail "no restart when agent sessions already have their leaf"
ok "no restart when the agents leaf is already on (a re-run)"

rc="$(run notready FAKE_READY=false)"
[ "$rc" != 0 ] || { cat "$WORK/notready.out" >&2; fail "NOT READY must stop provisioning"; }
grep -q 'NOT READY' "$WORK/notready.out" || fail "the stop says NOT READY"
grep -q 'nft (fail): table k2app missing \[fix: k2 app box prepare (owner)\]' "$WORK/notready.out" \
	|| { cat "$WORK/notready.out" >&2; fail "the stop names the failing check, its detail and its fix"; }
! grep -q 'kernel (ok)' "$WORK/notready.out" || fail "passing checks aren't named"
grep -q -- '--allow-not-ready' "$WORK/notready.out" || fail "the stop says how to allow a non-App box"
! grep -q '^APPS_LINE=' "$WORK/notready.out" || fail "NOT READY never reaches the summary"
ok "NOT READY is fatal and names the failing checks (detail + fix) and the override"

rc="$(run oldd FAKE_READY=false FAKE_BLOCKING=no)"
[ "$rc" != 0 ] && grep -q 'nft (fail)' "$WORK/oldd.out" || fail "without the daemon's blocking list, the non-ok rows are named"
ok "a daemon without the blocking list: the failing rows are still named"
rc="$(run oldready FAKE_BLOCKING=no)"
[ "$rc" = 0 ] || fail "an older daemon that is READY: exit $rc"
! grep -q 'auto-prepare' "$WORK/oldready/calls.log" || fail "an older daemon without the route isn't asked"
grep -q 'automatic prep unchanged (older daemon)' "$WORK/oldready.out" || fail "the summary says automatic prep is unchanged"
ok "an older daemon (no auto-prepare route): READY, automatic prep left as set"

rc="$(run allowed FAKE_READY=false ALLOW_NOT_READY=1)"
[ "$rc" = 0 ] || { cat "$WORK/allowed.out" >&2; fail "--allow-not-ready continues"; }
grep -q 'WARN: Apps: k2 app doctor: NOT READY (allowed: --allow-not-ready): nft (fail)' "$WORK/allowed.out" || fail "the warning names the checks"
grep -q '^APPS_LINE=NOT READY (allowed): nft (fail)' "$WORK/allowed.out" || fail "the summary says NOT READY (allowed)"
! grep -q 'auto-prepare' "$WORK/allowed/calls.log" || fail "a non-App box doesn't get automatic prep"
ok "--allow-not-ready: NOT READY is a warning, named in the summary"

rc="$(run skipped K2_SKIP_APPS=1)"
[ "$rc" = 0 ] && grep -q '^APPS_LINE=skipped$' "$WORK/skipped.out" || fail "K2_SKIP_APPS=1 skips 8b"
[ ! -s "$WORK/skipped/calls.log" ] || fail "K2_SKIP_APPS=1 calls nothing"
ok "K2_SKIP_APPS=1 skips 8b (no prep, no restart, no doctor gate)"

echo "provision_apps: $pass checks passed"
