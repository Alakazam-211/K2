#!/usr/bin/env bash
# Git Bash (Windows): curl is a native Windows program, and the config the
# CLI feeds it on stdin isn't path-converted the way argv is. A body file
# (`data-binary = "@/tmp/…"`) is written the Windows way there (cygpath -w),
# or curl can't open it (exit 26) and every k2 doc call with a body reads as
# "no answer from the K2 daemon". Everywhere else the line is unchanged.
#
# Runs on any OS: a stand-in cygpath plays Git Bash's. On Git Bash itself the
# last case also drives the real cygpath and the real curl.
# bash-only: reads cli/k2 as source; the parity gate skips it for the binary.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_BASH_SOURCE"
pass=0
die() { echo "FAIL: $*" >&2; exit 1; }

fns="$(sed -n '/^_k2_cq()/,/^}/p; /^_k2_cfg_add()/,/^}/p' "$K2_CLI")"
[ "$(printf '%s\n' "$fns" | grep -c '^_k2_cfg_add()')" = 1 ] || die "_k2_cfg_add not found once in cli/k2"
grep -q '^_K2_CYGPATH=""$' "$K2_CLI" || die "cli/k2 sets no _K2_CYGPATH default"
eval "$fns"

line() { # line <name> <value> → the config line _k2_cfg_add writes
    local _K2_CFG=""
    _k2_cfg_add "$1" "$2"
    printf '%s' "$_K2_CFG"
}
want() { # want <case> <got> <wanted>
    [ "$2" = "$3" ] || die "$1: got [$2], wanted [$3]"
    pass=$((pass + 1))
}

# Not Git Bash: every value as given.
_K2_CYGPATH=""
want "unix body file" "$(line data-binary @/tmp/k2/body.json)" 'data-binary = "@/tmp/k2/body.json"'
want "unix data" "$(line data a=b)" 'data = "a=b"'

# Git Bash, with a stand-in cygpath: only a body FILE becomes a Windows path.
_K2_CYGPATH=1
cygpath() { [ "$1" = -w ] || die "cygpath called without -w"; printf 'C:\\Users\\me\\AppData\\Local\\Temp%s' "$(printf '%s' "${2#/tmp}" | tr / '\\')"; }
want "Windows body file" "$(line data-binary @/tmp/k2/body.json)" 'data-binary = "@C:\\Users\\me\\AppData\\Local\\Temp\\k2\\body.json"'
want "a body that isn't a file" "$(line data-binary '{"a":1}')" 'data-binary = "{\"a\":1}"'
want "a relative @ name stays" "$(line data-binary @body.json)" 'data-binary = "@body.json"'
want "other options stay" "$(line url http://127.0.0.1:1/x)" 'url = "http://127.0.0.1:1/x"'
unset -f cygpath

# On Git Bash itself: the real cygpath, and curl reads the file it names.
case "${OSTYPE:-}" in
    msys*|cygwin*)
        tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
        printf '{"k2":"windows"}' > "$tmp/body.json"
        cfg="$(line data-binary "@$tmp/body.json")"
        rc=0; printf '%s' "$cfg" | curl -K - -s -o /dev/null --connect-timeout 2 -X POST http://127.0.0.1:9/ || rc=$?
        # 7: nothing listens on :9, so curl got as far as connecting; 26 would
        # be "can't read the body file".
        want "real curl reads the body file" "$rc" 7
        ;;
esac

echo "windows_curl_body_path: $pass passed"
