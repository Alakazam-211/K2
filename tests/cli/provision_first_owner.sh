#!/usr/bin/env bash
# Hermetic test for step 9 (the first owner and the website-management
# login) of scripts/provision-k2-server.sh, 0.46.18.
#
# Since 0.46.15 users/set-role owner with the owner token answers 403
# vault_presence_required until this computer's own user check (a person
# present) is passed, so the provisioner's `set-role <u> owner` stopped
# every provision. Now the provisioner makes no login at all: the k2.dev
# dashboard's backend makes the buyer's owner login and the
# website-management login (k2cloud) in one signed call,
# POST /cli/users/claim-first-owner. So:
#   - K2_OWNER_USER or K2_OPS_USER stops the run before any work, with a
#     line naming the k2.dev claim; --bake ignores them;
#   - step 9 sends no request (no users/add, no set-role, no vault socket);
#   - the callback keeps its shape with ownerUser, opsUser and opsPassword
#     empty and firstOwner "claim".
#
# How: the early check and the step 9 block are cut out of the script and
# run alone, with k2_curl, curl and runuser stand-ins that record any call
# (none may come). No root, no network, no daemon.
# K2_TEST_PROVISION names another copy of the script (the bite).
# Run with: bash tests/cli/provision_first_owner.sh
# shellcheck disable=SC2016
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PROVISION="${K2_TEST_PROVISION:-$ROOT/scripts/provision-k2-server.sh}"
[ -f "$PROVISION" ] || { echo "FAIL: missing $PROVISION" >&2; exit 1; }

pass=0
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "  ok $*"; pass=$((pass + 1)); }

echo "== static shape =="
bash -n "$PROVISION" || fail "bash -n"
BLOCK="$(sed -n '/^# ── 9\. first owner user/,/^# ── 9c\. Sew/p' "$PROVISION" | sed '$d')"
[ -n "$BLOCK" ] || fail "the step 9 block is not found"
EARLY="$(sed -n '/^# Step 9: this run makes no owner/,/^fi$/p' "$PROVISION")"
[ -n "$EARLY" ] || fail "the early K2_OWNER_USER / K2_OPS_USER check is not found"
L_EARLY="$(grep -n '^# Step 9: this run makes no owner' "$PROVISION" | cut -d: -f1)"
L_PKGS="$(grep -n '^# ── 1\. packages' "$PROVISION" | cut -d: -f1)"
[ "$L_EARLY" -lt "$L_PKGS" ] || fail "the check must run before step 1 (no work first)"
ok "the K2_OWNER_USER / K2_OPS_USER check runs before any work"
for never in 'users/set-role' 'users/add' 'vault.sock' 'confirm/finish' 'X-K2-Tty' 'K2_FIRST_OWNER'; do
	if grep -q -- "$never" "$PROVISION"; then fail "the provisioner must not use $never (no login is made by a run)"; fi
done
ok "no users/add, no set-role, no vault socket, no tty claim, no bridge switch anywhere in the script"
CB="$(sed -n '/^# ── 10\. control-plane callback/,/^# ── 11\./p' "$PROVISION")"
for f in '"ownerUser":"",' '"firstOwner":"claim",' '"opsUser":"",' '"opsPassword":"",'; do
	<<<"$CB" grep -qF "$f" || fail "the callback must carry $f"
done
ok "the callback keeps its shape: ownerUser, opsUser, opsPassword empty; firstOwner claim"

echo "== K2_OWNER_USER / K2_OPS_USER stop the run =="
run_early() { # env assignments... → prints the die line or "go"
	env -i PATH="$PATH" "$@" bash -c 'set -euo pipefail; die() { echo "DIE: $*"; exit 1; }; BAKE_ONLY="${BAKE_ONLY:-0}"; '"$EARLY"'; echo go' 2>&1 || true
}
out="$(run_early K2_OWNER_USER=alice K2_OWNER_PASSWORD=x)"
case "$out" in "DIE: K2_OWNER_USER is set, but this script no longer makes an owner login: a server's first owner is claimed from the k2.dev dashboard"*) ;; *) fail "K2_OWNER_USER must stop: $out" ;; esac
<<<"$out" grep -q 'k2 users add <you> --role owner from your own SSH login' || fail "the stop names the self-hosted way: $out"
ok "K2_OWNER_USER stops, naming the k2.dev claim and the self-hosted way"
out="$(run_early K2_OPS_USER=k2cloud)"
case "$out" in "DIE: K2_OPS_USER is set, but this script no longer makes the website-management login: the k2.dev claim makes it"*) ;; *) fail "K2_OPS_USER must stop: $out" ;; esac
ok "K2_OPS_USER stops, naming the claim's opsUsername"
[ "$(run_early K2_OWNER_USER=alice K2_FIRST_OWNER=terminal)" != go ] || fail "no switch brings back an owner-making run"
[ "$(run_early)" = go ] || fail "neither set must go on"
[ "$(run_early K2_OWNER_USER=alice K2_OPS_USER=k2cloud BAKE_ONLY=1)" = go ] || fail "--bake ignores both"
ok "no switch gets past it; neither set, and --bake, go on"

echo "== step 9 sends nothing =="
CALLS="$(mktemp)"
trap 'rm -f "$CALLS"' EXIT
out="$(env -i PATH="$PATH" CALLS="$CALLS" BASE=http://127.0.0.1:9 TOKEN=owner-tok K2_RUN_USER=k2 bash -c 'set -euo pipefail
log() { echo "LOG: $*"; }
die() { echo "DIE: $*"; exit 1; }
k2_curl() { echo "k2_curl $*" >>"$CALLS"; }
curl() { echo "curl $*" >>"$CALLS"; }
runuser() { echo "runuser $*" >>"$CALLS"; }
'"$BLOCK"'
echo DONE' 2>&1)" || fail "step 9 must succeed: $out"
[ ! -s "$CALLS" ] || fail "step 9 must send nothing, got: $(cat "$CALLS")"
<<<"$out" grep -q 'LOG: first owner: none made here; the k2.dev dashboard makes the owner and website-management logins (claim-first-owner' || fail "the owner line: $out"
<<<"$out" grep -q '^DONE$' || fail "step 9 ran to its end: $out"
ok "no request; one line naming the k2.dev claim"

echo "ALL PASS ($pass)"
