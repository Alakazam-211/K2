#!/usr/bin/env bash
# hostmail enable must require --hostname and POST {"hostname":...}.
# Empty daemon body stays 400 (covered in mail_routes). k2 db enable stays {}.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"

fail() { echo "FAIL: $*" >&2; exit 1; }

# Usage without --hostname (no daemon needed).
out="$(mktemp -t k2-hostmail-enable-XXXXXX)"
trap 'rm -f "$out"' EXIT
set +e
K2SO_PORT=1 K2_PORT=1 K2_HOOK_TOKEN=x "$K2" hostmail enable --json >"$out" 2>&1
code=$?
set -e
if [ "$code" -ne 2 ]; then
    fail "hostmail enable without --hostname must exit 2, got $code: $(cat "$out")"
fi
if ! grep -q hostname "$out"; then
    fail "usage error must mention hostname: $(cat "$out")"
fi

# CLI POSTs hostname; never empty {}.
if ! grep -q 'body={"hostname": hostname}' "$K2"; then
    fail "k2 hostmail enable must POST {\"hostname\": hostname}"
fi
if grep -A6 'verb == "server_enable"' "$K2" | grep -q 'body={}'; then
    fail "k2 hostmail enable must not POST empty {}"
fi
if ! grep -q '/cli/db/server/enable", body={}' "$K2"; then
    fail "k2 db enable must still POST {}"
fi

# Help / schema mention --hostname.
if ! "$K2" hostmail --help | grep -q -- '--hostname'; then
    fail "k2 hostmail --help must mention --hostname"
fi
if ! "$K2" hostmail --help | grep -q 'Host-wide inbound down'; then
    fail "k2 hostmail --help must say disable is host-wide inbound down"
fi
if ! "$K2" hostmail --help | grep -q 'systemctl is-active stalwart'; then
    fail "k2 hostmail --help must name systemctl is-active stalwart as Linux ground truth"
fi
if ! "$K2" hostmail status --help | grep -q 'systemctl is-active stalwart'; then
    fail "k2 hostmail status --help must name systemctl is-active stalwart"
fi

domain_help="$("$K2" hostmail domain --help)"
if ! printf '%s' "$domain_help" | grep -q 'k2 hostmail domain add'; then
    fail "k2 hostmail domain --help must be domain help, got: $domain_help"
fi
if printf '%s' "$domain_help" | grep -q 'k2 mail domain add'; then
    fail "k2 hostmail domain --help must not teach leftover k2 mail domain: $domain_help"
fi
if printf '%s' "$domain_help" | grep -q 'k2 hostmail <command>'; then
    fail "k2 hostmail domain --help must not be group help: $domain_help"
fi

set +e
mail_domain_out="$("$K2" mail domain add example.test 2>&1)"
mail_domain_rc=$?
set -e
if [ "$mail_domain_rc" -eq 0 ]; then
    fail "k2 mail domain add must exit 2 moved-to-hostmail, got 0: $mail_domain_out"
fi
if ! printf '%s' "$mail_domain_out" | grep -q "moved to 'k2 hostmail domain'"; then
    fail "k2 mail domain add must say moved to hostmail: $mail_domain_out"
fi

echo "PASS: hostmail enable --hostname"
