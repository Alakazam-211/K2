#!/usr/bin/env bash
# Federated --inbox-wake: same plane as live msg (no Connect token;
# no UDS split after _apply_remote_host; staged files kept on failure).
# Loud: never skip-if-missing.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="${ROOT}/cli/k2"

if [ ! -f "$K2_CLI" ]; then
    echo "FAIL: cli/k2 not found at $K2_CLI" >&2
    exit 1
fi

eval "$(sed -n '/^# BEGIN_CLI_SEND_TCP_LOCK/,/^# END_CLI_SEND_TCP_LOCK/p' "$K2_CLI")"
eval "$(sed -n '/^_uds_eligible()/,/^}/p' "$K2_CLI")"
eval "$(sed -n '/^_split_remote_target()/,/^}/p' "$K2_CLI")"
eval "$(sed -n '/^# BEGIN_MSG_INBOX_TRAY_RESTORE/,/^# END_MSG_INBOX_TRAY_RESTORE/p' "$K2_CLI")"

fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "OK: $*"; }

# ── 4. Bare --inbox is still exit 2 teaching ──────────────────────────
set +e
bare_out="$(env K2_PORT=9 K2_HOOK_TOKEN=fake "$K2_CLI" msg peer --inbox ./x.md 2>&1)"
bare_rc=$?
set -e
[ "$bare_rc" = "2" ] || fail "bare --inbox exit want 2 got $bare_rc body=$bare_out"
printf '%s' "$bare_out" | grep -Fq "bare --inbox is no longer accepted" \
    || fail "bare --inbox teaching missing: $bare_out"
ok "bare --inbox exit 2 teaching"

# ── Bug A: after remote apply, UDS is skipped ─────────────────────────
_K2_FORCE_TCP=0
_K2_TRAY_REMOTE_APPLIED=0
if _cli_skip_uds; then fail "default must allow UDS"; fi
ok "default allows UDS (local cell)"

_uds_eligible "/cli/inbox/deliver" || fail "inbox/deliver must stay UDS-eligible for local"
if _uds_eligible "/cli/workspace/resolve"; then
    fail "workspace/resolve must stay TCP"
fi
ok "inbox/deliver UDS-eligible; resolve is TCP"

_K2_TRAY_REMOTE_APPLIED=1
_cli_skip_uds || fail "_K2_TRAY_REMOTE_APPLIED must skip UDS"
ok "remote-applied skips UDS (Bug A lock)"

_K2_TRAY_REMOTE_APPLIED=0
_K2_FORCE_TCP=1
_cli_skip_uds || fail "_K2_FORCE_TCP must skip UDS"
ok "_K2_FORCE_TCP skips UDS"

# Source lock: _apply_remote_host sets _K2_FORCE_TCP
grep -q '_K2_FORCE_TCP=1' "$K2_CLI" || fail "_apply_remote_host must set _K2_FORCE_TCP=1"
ok "_apply_remote_host forces TCP"

# ── Bug A/B: agent::host tray does NOT require Connect token ──────────
inner="$(awk '/^_cmd_msg_inbox_form_inner\(\)/,/^cmd_msg_signal_form\(\)/' "$K2_CLI")"
if printf '%s' "$inner" | grep -E '^[[:space:]]*_tray_token_missing_error '; then
    fail "_cmd_msg_inbox_form_inner must not call _tray_token_missing_error"
fi
if printf '%s' "$inner" | grep -E '^[[:space:]]*_apply_remote_host '; then
    fail "_cmd_msg_inbox_form_inner must not _apply_remote_host for agent::host"
fi
ok "tray agent::host does not take the Connect-token path"

_split_remote_target "ahama::nsi.k2.dev"
[ "$_BARE_TARGET" = "ahama" ] || fail "bare target: $_BARE_TARGET"
[ "$_REMOTE_HOST" = "nsi.k2.dev" ] || fail "host: $_REMOTE_HOST"
[ "${_IS_WIRE_FORM:-0}" = "0" ] || fail "user form must not be wire"
ok "agent::host still splits as user form"

# Keep original token (do not strip to bare) — grep the comment/lock.
printf '%s' "$inner" | grep -q 'Keep the original agent::host token' \
    || fail "inner must keep agent::host for local-daemon federation"
ok "CLI keeps agent::host token for local daemon hop"

# ── Staged delete only on success ─────────────────────────────────────
# The two deliver sites must map_response BEFORE delete, and delete only
# when rc==0.
python3 - "$K2_CLI" <<'PY' || fail "staged-delete-on-success order"
import pathlib, sys, re
text = pathlib.Path(sys.argv[1]).read_text()
# Both staged deliver tails: map_response then conditional delete.
pat = re.compile(
    r"_msg_inbox_map_response \"\$response\" \"\$wake_flag\"\s*\n"
    r"\s*local \w+_rc=\$\?\s*\n"
    r".*?if \[ \"\$\w+_rc\" -eq 0 \]; then\s*\n"
    r"\s*_msg_inbox_delete_staged",
    re.S,
)
hits = pat.findall(text)
if len(hits) < 2:
    sys.stderr.write(f"expected 2 success-only staged deletes, found {len(hits)}\n")
    sys.exit(1)
# Old bug: delete immediately after deliver, before map.
if re.search(
    r"_msg_inbox_post_deliver_single \"\$deliver_path\"\)\s*\n\s*_msg_inbox_delete_staged",
    text,
):
    sys.stderr.write("single-file still deletes staged before mapping response\n")
    sys.exit(1)
if re.search(
    r"_msg_inbox_post_deliver_bundle \"\$staged_path\"\)\s*\n\s*_msg_inbox_delete_staged",
    text,
):
    sys.stderr.write("bundle still deletes staged before mapping response\n")
    sys.exit(1)
print("OK: staged delete only on deliver success")
PY

# ── Schema / study copy: federation plane, not Connect-only ───────────
grep -F 'Federated live AND tray' "$K2_CLI" >/dev/null \
    || fail "schema must say live AND tray use federation"
grep -F 'Tray agent::host needs a K2 Connect host token' "$K2_CLI" \
    && fail "schema still says tray needs Connect token"
ok "schema copy uses federation for tray"

echo "OK: msg_inbox_federated_tray"
