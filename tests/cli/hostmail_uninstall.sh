#!/usr/bin/env bash
# hostmail uninstall --purge --confirm-hostname is owner-only at the CLI.
# --purge without --confirm-hostname exits 2 and does not POST.
# The route stays off is_mail_manage_surface (asserted in mail_routes).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"

fail() { echo "FAIL: $*" >&2; exit 1; }

out="$(mktemp -t k2-hostmail-uninstall-XXXXXX)"
trap 'rm -f "$out"' EXIT

set +e
K2SO_PORT=1 K2_PORT=1 K2_HOOK_TOKEN=x "$K2" hostmail uninstall --purge --json >"$out" 2>&1
code=$?
set -e
if [ "$code" -ne 2 ]; then
    fail "uninstall --purge without --confirm-hostname must exit 2, got $code: $(cat "$out")"
fi
if grep -q daemon_unreachable "$out"; then
    fail "uninstall --purge without --confirm-hostname must not POST: $(cat "$out")"
fi
if ! grep -q confirm-hostname "$out"; then
    fail "usage error must mention confirm-hostname: $(cat "$out")"
fi

set +e
K2SO_PORT=1 K2_PORT=1 K2_HOOK_TOKEN=x "$K2" hostmail uninstall --json >"$out" 2>&1
code=$?
set -e
if [ "$code" -ne 2 ]; then
    fail "uninstall without --purge must exit 2, got $code: $(cat "$out")"
fi
if grep -q daemon_unreachable "$out"; then
    fail "uninstall without flags must not POST: $(cat "$out")"
fi

if ! grep -q 'body={"purgeData": True, "confirmHostname": hostname}' "$K2"; then
    fail "k2 hostmail uninstall must POST purgeData and confirmHostname"
fi
if ! grep -q '/cli/mail/server/uninstall' "$K2"; then
    fail "k2 hostmail uninstall must target /cli/mail/server/uninstall"
fi

unknown="$(mktemp -t k2-hostmail-unknown-XXXXXX)"
set +e
K2SO_PORT=1 K2_PORT=1 K2_HOOK_TOKEN=x "$K2" hostmail nosuch >"$unknown" 2>&1
ucode=$?
set -e
if [ "$ucode" -ne 2 ]; then
    fail "unknown hostmail command must exit 2, got $ucode: $(cat "$unknown")"
fi
if ! grep -q 'uninstall' "$unknown"; then
    fail "unknown-command usage must list uninstall: $(cat "$unknown")"
fi
rm -f "$unknown"

if ! "$K2" hostmail --help | grep -q 'uninstall --purge --confirm-hostname'; then
    fail "k2 hostmail --help must list uninstall --purge --confirm-hostname"
fi
if ! "$K2" hostmail uninstall --help | grep -q 'does not POST'; then
    fail "k2 hostmail uninstall --help must say missing confirm does not POST"
fi

surface="$(awk '/pub fn is_mail_manage_surface/,/^}/' "$PROJECT_ROOT/crates/k2-daemon/src/mail_routes.rs")"
if printf '%s\n' "$surface" | grep -q '/cli/mail/server/uninstall'; then
    fail "uninstall must stay off is_mail_manage_surface"
fi

echo "PASS: hostmail uninstall --purge --confirm-hostname"
