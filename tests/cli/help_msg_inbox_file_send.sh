#!/usr/bin/env bash
# Help copy: agents send files with `k2 msg --inbox-wake`, not inbox/mail.
# No daemon: help + --schema fire before HTTP.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"

[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }

pass=0
fail=0
assert_contains() {
    local label="$1" hay="$2" needle="$3"
    if printf '%s' "$hay" | grep -Fq -- "$needle"; then
        echo "  PASS: $label"
        pass=$((pass + 1))
    else
        echo "  FAIL: $label (missing $(printf %q "$needle"))" >&2
        fail=$((fail + 1))
    fi
}
assert_absent() {
    local label="$1" hay="$2" needle="$3"
    if printf '%s' "$hay" | grep -Fq -- "$needle"; then
        echo "  FAIL: $label (unexpected $(printf %q "$needle"))" >&2
        fail=$((fail + 1))
    else
        echo "  PASS: $label"
        pass=$((pass + 1))
    fi
}

K2_FAKE=(env K2_PORT=9 K2_HOOK_TOKEN=fake)

echo "== k2 help msg =="
msg_help="$("${K2_FAKE[@]}" "$K2_CLI" help msg 2>&1 || true)"
assert_contains "THREE TOOLS" "$msg_help" "THREE TOOLS"
assert_contains "msg sends files" "$msg_help" "--inbox-wake"
assert_contains "inbox cannot send" "$msg_help" "Cannot send a file"
assert_contains "mail is SMTP" "$msg_help" "real email"
assert_absent "no inbox send verb" "$msg_help" "inbox send"
assert_absent "inbox is not mail" "$msg_help" "inbox is mail"

echo "== k2 help inbox =="
inbox_help="$("${K2_FAKE[@]}" "$K2_CLI" help inbox 2>&1 || true)"
assert_contains "YOUR tray" "$inbox_help" "YOUR tray"
assert_contains "not email" "$inbox_help" "Not email"
assert_contains "others send with msg --inbox-wake" "$inbox_help" "k2 msg <your-handle> --inbox-wake"
assert_contains "cannot send with inbox" "$inbox_help" "You cannot send a file with \`k2 inbox\`"
assert_absent "inbox help is not email-like" "$inbox_help" "email-like"

echo "== k2 help read =="
read_help="$("${K2_FAKE[@]}" "$K2_CLI" help read 2>&1 || true)"
assert_contains "msg sends tray package" "$read_help" "sends a tray package"
assert_contains "inbox is YOUR tray" "$read_help" "is YOUR tray"
assert_absent "read help does not call inbox mail" "$read_help" "inbox is mail"

echo "== k2 help (daily) =="
daily="$("${K2_FAKE[@]}" "$K2_CLI" help 2>&1 || true)"
assert_contains "daily --inbox-wake is send file + knock" "$daily" "Send file + knock"
assert_contains "daily inbox is receive" "$daily" "YOUR tray — receive/triage"
assert_absent "daily inbox is not email-like" "$daily" "email-like"

echo "== k2 --schema msg (source) =="
# Grep the CLI source: `k2 --schema` JSON currently contains a pre-existing
# unescaped newline elsewhere, so dumping it is a brittle way to pin copy.
assert_contains "schema files use --inbox-wake" "$(grep -F 'For files use --inbox-wake' "$K2_CLI" || true)" "For files use --inbox-wake"
assert_contains "schema live text is SHORT" "$(grep -F 'Live text is SHORT' "$K2_CLI" || true)" "Live text is SHORT"
assert_contains "schema not k2 mail" "$(grep -F 'Not k2 mail' "$K2_CLI" || true)" "Not k2 mail"

echo ""
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
