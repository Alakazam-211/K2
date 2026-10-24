#!/usr/bin/env bash
# Node admin end-to-end, as root inside a THROWAWAY Linux container
# (prd-k2-node-admin-v1 §16 "Real root"). Never on a customer box or a
# person's machine: it creates users and writes /etc, /run, /usr/local.
#
#   K2_DISPOSABLE_BOX=1 bash node_admin_linux.sh <dir-with-builds>
#
# <dir> holds test-keys builds made by node_admin_linux_container.sh:
#   k2-node-0.0.1 (installed first), k2-node-0.0.2 (good update),
#   k2-node-0.0.3 (K2_NODE_TEST_BROKEN: exits at once), k2-node-testsign.
#
# Proves: signed command → executed; unsigned / wrong key / expired /
# replayed / garbage / privileged-without-owner → refused (and audited);
# a stranger can't reach the socket; update with a good build → applied
# and healthy; bad signature → refused; manual rollback; a build that
# never comes up → automatic rollback; the root audit chain verifies.
set -euo pipefail
cd /
[ "${K2_DISPOSABLE_BOX:-}" = "1" ] || { echo "refusing: set K2_DISPOSABLE_BOX=1 (throwaway container only)" >&2; exit 2; }
[ "$(id -u)" = 0 ] || { echo "run as root in the container" >&2; exit 2; }
B="${1:?dir with builds}"

BIN=/usr/local/libexec/k2-node
HOME_DIR=/var/lib/k2node
CFG=/etc/k2-node
STATE=/var/lib/k2-node-admin
SOCK=/run/k2-node-admin.sock
TS="$B/k2-node-testsign"
W=/var/tmp/e2e
ARCH="$(uname -m)"
pass=0
fail() { echo "FAIL: $*" >&2; echo "--- executor log"; tail -n 40 /root/executor.log || true; echo "--- node log"; tail -n 20 /root/node.log || true; exit 1; }
ok() { pass=$((pass + 1)); echo "ok $pass - $*"; }
asnode() { runuser -u k2node -- "$@"; }
field() { python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('$1') if d.get('$1') is not None else '')"; }
wait_for() { # <secs> <cmd...>
    local n="$1"; shift
    for _ in $(seq 1 "$((n * 5))"); do "$@" && return 0; sleep 0.2; done
    return 1
}

install -d -m 0755 "$W"
# ── a node, installed the way setup does it (users, dirs, files) ─────
useradd --system --user-group --home-dir "$HOME_DIR" --no-create-home --shell /usr/sbin/nologin k2node
install -d -m 0750 -o k2node -g k2node "$HOME_DIR"
install -d -m 0755 /usr/local/libexec
install -m 0755 "$B/k2-node-0.0.1" "$BIN"
install -d -m 2775 -o root -g root "$CFG"
"$BIN" install-files --os linux --binary "$BIN" --home "$HOME_DIR" --config-dir "$CFG" --user k2node --group k2node --out "$W/files" >/dev/null
install -m 0664 "$W/files/policy.toml" "$CFG/policy.toml"
install -m 0664 "$W/files/control.toml" "$CFG/control.toml"
cat > /root/hook.sh <<EOF
#!/bin/sh
# Stands in for systemctl in this container (test-keys builds only).
case "\$1" in
  restart-node)
    pkill -f "$BIN run" 2>/dev/null; sleep 0.3
    setsid runuser -u k2node -- "$BIN" run --home "$HOME_DIR" --config-dir "$CFG" >>/root/node.log 2>&1 < /dev/null &
    ;;
  restart-unit) echo "restart-unit \$2" >> /root/units.log ;;
esac
exit 0
EOF
chmod 0755 /root/hook.sh
/root/hook.sh restart-node
wait_for 10 test -f "$HOME_DIR/state/node-key.pem" || fail "node never made its key"
NODE_FP="$(asnode "$BIN" status --home "$HOME_DIR" --json | field node_fp)"
[ ${#NODE_FP} = 64 ] || fail "node fp: $NODE_FP"

# ── controller + owner (test stand-ins) and the machine owner's consent ─
"$TS" keygen "$W/ctl.pem" >/dev/null
"$TS" keygen "$W/other.pem" >/dev/null
OWNER_SEED="$(printf '11%.0s' $(seq 1 32))"
STRANGER_SEED="$(printf '22%.0s' $(seq 1 32))"
OWNER_PUB="$("$TS" owner-pub "$OWNER_SEED")"
"$TS" pin-json "$W/ctl.pem" > "$W/pin.json"
install -m 0600 -o k2node -g k2node "$W/pin.json" "$HOME_DIR/state/controller.json"
"$BIN" admin init --config-dir "$CFG" --home "$HOME_DIR" --level full >/dev/null
# Approval key: written by root at the machine (the TOFU frame path is unit-tested).
python3 - "$CFG/admin.toml" "$OWNER_PUB" <<'PY'
import sys
p, k = sys.argv[1], sys.argv[2]
s = open(p).read().replace("approval_keys = []", f'approval_keys = ["{k}"]')
open(p, "w").write(s)
PY
grep -q "$OWNER_PUB" "$CFG/admin.toml" || fail "approval key not pinned"
# No controller to reach here: drop the pin so the node counts as
# unenrolled (healthy at start) — root kept its own copy in admin.toml.
rm -f "$HOME_DIR/state/controller.json"
/root/hook.sh restart-node
ok "node + admin.toml (level full, controller + approval key pinned by root)"

# ── the executor: socket, restart loop (re-exec after an update) ─────
install -d -m 0700 "$STATE"
(
    # The unit's UMask=0077 (0.46.3 staged 0700 builds under it; the update
    # below must still come up: prd-compute-update-continuity-v1 Bug 1).
    umask 077
    while true; do
        K2_NODE_ADMIN_TEST_HOOK=/root/hook.sh "$BIN" admin serve --config-dir "$CFG" --state-dir "$STATE" \
            --home "$HOME_DIR" --binary "$BIN" --socket "$SOCK" >>/root/executor.log 2>&1 || true
        sleep 0.2
    done
) &
wait_for 10 test -S "$SOCK" || fail "executor socket never appeared"
[ "$(stat -c '%a %U %G' "$SOCK")" = "660 root k2node" ] || fail "socket mode/owner: $(stat -c '%a %U %G' "$SOCK")"
ok "executor listening on $SOCK (0660 root:k2node)"

mk() { "$TS" cmd --key "$W/ctl.pem" --node-fp "$NODE_FP" "$@"; }
send() { asnode "$TS" send "$SOCK" "$@"; }
state_of() { python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('state',''), d.get('code') or '')"; }

# 1. signed operate command → executed
mk --verb control.pause > "$W/pause.json"
chmod 0644 "$W"/*.json
r="$(send command "$W/pause.json" | state_of)"
[ "$r" = "done " ] || fail "pause: $r"
grep -q 'state = "paused"' "$CFG/control.toml" || fail "control.toml not paused"
ok "signed control.pause executed (control.toml paused)"
# 2. replay
r="$(send command "$W/pause.json" | state_of)"
[ "$r" = "refused replayed" ] || fail "replay: $r"
ok "replayed command refused"
# 3. wrong controller key
"$TS" cmd --key "$W/other.pem" --node-fp "$NODE_FP" --verb control.resume > "$W/other.json"; chmod 0644 "$W/other.json"
r="$(send command "$W/other.json" | state_of)"
[ "$r" = "refused bad_controller_signature" ] || fail "wrong key: $r"
ok "wrong controller key refused"
# 4. unsigned (signature stripped)
python3 -c "import json;d=json.load(open('$W/pause.json'));d['sig_c']='AAAA';json.dump(d,open('$W/unsigned.json','w'))"; chmod 0644 "$W/unsigned.json"
r="$(send command "$W/unsigned.json" | state_of)"
[ "$r" = "refused bad_controller_signature" ] || fail "unsigned: $r"
ok "unsigned command refused"
# 5. expired
mk --verb control.resume --iat-offset -7200 --ttl 600 > "$W/old.json"; chmod 0644 "$W/old.json"
r="$(send command "$W/old.json" | state_of)"
[ "$r" = "refused approval_expired" ] || fail "expired: $r"
ok "expired command refused"
# 6. privileged without / with the owner's approval
mk --verb unit.restart --args '{"unit":"k2-node-packages"}' > "$W/priv.json"; chmod 0644 "$W/priv.json"
r="$(send command "$W/priv.json" | state_of)"
[ "$r" = "refused approval_required" ] || fail "privileged w/o owner: $r"
mk --verb unit.restart --args '{"unit":"k2-node-packages"}' --owner-seed "$STRANGER_SEED" > "$W/priv2.json"; chmod 0644 "$W/priv2.json"
r="$(send command "$W/priv2.json" | state_of)"
[ "$r" = "refused bad_approval_signature" ] || fail "privileged w/ stranger: $r"
[ ! -f /root/units.log ] || fail "a refused privileged command ran"
mk --verb unit.restart --args '{"unit":"k2-node-packages"}' --owner-seed "$OWNER_SEED" > "$W/priv3.json"; chmod 0644 "$W/priv3.json"
r="$(send command "$W/priv3.json" | state_of)"
[ "$r" = "done " ] || fail "privileged w/ owner: $r"
grep -q "restart-unit k2-node-packages" /root/units.log || fail "unit.restart didn't run"
ok "privileged: refused without approval and with a stranger's key; runs with the owner's"
# 7. garbage and a stranger
head -c 300 /dev/urandom > "$W/garbage"; chmod 0644 "$W/garbage"
r="$(asnode "$TS" send-raw "$SOCK" "$W/garbage")"
echo "$r" | grep -q '"code":"malformed"' || fail "garbage: $r"
if runuser -u nobody -- "$TS" send "$SOCK" status >/dev/null 2>&1; then fail "nobody reached the executor"; fi
ok "garbage refused; a stranger (nobody) can't reach the socket"
# 8. policy: narrowing runs, widening is held
mk --verb policy.set --args '{"max_parallel":1}' > "$W/narrow.json"; chmod 0644 "$W/narrow.json"
[ "$(send command "$W/narrow.json" | state_of)" = "done " ] || fail "narrow"
mk --verb policy.set --args '{"max_parallel":8}' > "$W/widen.json"; chmod 0644 "$W/widen.json"
[ "$(send command "$W/widen.json" | state_of)" = "refused approval_required" ] || fail "widen w/o owner"
grep -q "max_parallel = 1" "$CFG/policy.toml" || fail "policy.toml"
ok "policy.set: narrowing applied, widening needs the owner"

# ── self-update ──────────────────────────────────────────────────────
upd() { # <version> <file> <sig>
    mk --verb node.update --args "{\"version\":\"$1\",\"sha256\":\"$(sha256sum "$2" | cut -d' ' -f1)\",\"trigger\":\"explicit\"}" > "$W/u-$1.json"
    chmod 0644 "$W/u-$1.json"
    send update "$W/u-$1.json" "$2" "$3"
}
for v in 0.0.2 0.0.3; do
    "$TS" build-sig "$B/k2-node-$v" linux "$ARCH" "$v" 2 > "$W/k2-node-$v.sig"
done
cp "$B/k2-node-0.0.2" "$W/k2-node-0.0.2"; cp "$B/k2-node-0.0.3" "$W/k2-node-0.0.3"; chmod 0644 "$W"/k2-node-0.0.*
phase() { python3 -c "import json; print(json.load(open('$STATE/update.json')).get('phase',''))" 2>/dev/null; }
# 9. a bad signature (tampered build, controller signed its sha) → refused
cp "$W/k2-node-0.0.2" "$W/tampered"; printf 'X' | dd of="$W/tampered" bs=1 seek=4096 conv=notrunc 2>/dev/null; chmod 0644 "$W/tampered"
r="$(upd 0.0.2 "$W/tampered" "$W/k2-node-0.0.2.sig" | state_of)"
[ "$r" = "refused bad_build" ] || fail "tampered: $r"
cmp -s "$BIN" "$B/k2-node-0.0.1" || fail "binary changed after a refused update"
ok "update with a bad signature refused; binary untouched"
# 10. good update → swapped → healthy (executor re-execs)
r="$(upd 0.0.2 "$W/k2-node-0.0.2" "$W/k2-node-0.0.2.sig" | state_of)"
[ "$r" = "done " ] || fail "good update: $r"
wait_for 30 sh -c "[ \"\$(python3 -c \"import json; print(json.load(open('$STATE/update.json'))['phase'])\")\" = healthy ]" || fail "never healthy: $(cat $STATE/update.json)"
"$BIN" version | grep -q "k2-node 0.0.2" || fail "installed: $("$BIN" version)"
cmp -s "$BIN.prev" "$B/k2-node-0.0.1" || fail ".prev is not 0.0.1"
wait_for 15 sh -c "runuser -u k2node -- '$TS' send '$SOCK' status | grep -q '\"version\":\"0.0.2\"'" || fail "executor didn't re-exec on 0.0.2"
ok "good signed update applied, node healthy, executor re-exec'd on 0.0.2"
# 11. manual rollback → 0.0.1; 0.0.2 now refused
mk --verb node.rollback > "$W/rb.json"; chmod 0644 "$W/rb.json"
[ "$(send command "$W/rb.json" | state_of)" = "done " ] || fail "manual rollback"
"$BIN" version | grep -q "k2-node 0.0.1" || fail "after rollback: $("$BIN" version)"
wait_for 15 sh -c "runuser -u k2node -- '$TS' send '$SOCK' status | grep -q '\"version\":\"0.0.1\"'" || fail "executor didn't re-exec on 0.0.1"
r="$(upd 0.0.2 "$W/k2-node-0.0.2" "$W/k2-node-0.0.2.sig" | state_of)"
[ "$r" = "refused rolled_back_version" ] || fail "re-offer after rollback: $r"
ok "manual rollback to .prev; the rolled-back version is refused after"
# 12. a build that never comes up → automatic rollback (120 s gate)
r="$(upd 0.0.3 "$W/k2-node-0.0.3" "$W/k2-node-0.0.3.sig" | state_of)"
[ "$r" = "done " ] || fail "broken update: $r"
wait_for 200 sh -c "[ \"\$(python3 -c \"import json; print(json.load(open('$STATE/update.json'))['phase'])\")\" = rolled_back ]" || fail "never rolled back: $(cat $STATE/update.json)"
"$BIN" version | grep -q "k2-node 0.0.1" || fail "after auto rollback: $("$BIN" version)"
cmp -s "$BIN.bad" "$B/k2-node-0.0.3" || fail ".bad is not the broken build"
grep -q '"health_gate"' "$STATE/update.json" || fail "reason"
ok "unhealthy build rolled back automatically (health gate)"
# 13. the root audit chain
"$BIN" admin audit --state-dir "$STATE" --tail 200 > "$W/audit.txt" 2> "$W/audit.err" || fail "audit chain: $(cat "$W/audit.err")"
grep -q "audit chain ok" "$W/audit.err" || fail "audit"
grep -q "refused" "$W/audit.txt" && grep -q "update" "$W/audit.txt" || fail "audit content"
[ "$(stat -c %a "$STATE")" = 700 ] && [ "$(stat -c %a "$STATE/audit.log")" = 600 ] || fail "state perms"
ok "root audit chain verifies ($(wc -l < "$W/audit.txt") lines; state 0700, log 0600)"
echo "ALL $pass PASSED"
