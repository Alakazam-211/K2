#!/usr/bin/env bash
# Stand-in service managers for the `k2 daemon` lifecycle tests. Sourced,
# after `set -euo pipefail` and _cli_under_test.sh (K2 is the CLI to run).
#
# No row can touch a real service. The CLI runs under `env -i` with PATH set
# to exactly two folders: first the stand-ins (systemctl, launchctl,
# loginctl, journalctl, sudo, curl and uname), then a folder of links to a
# short list of ordinary tools (bash, python3, coreutils), none of them a
# service manager, sudo or a download tool. ds_guard checks it before any
# row runs and fails the test if a real one is reachable. HOME and the K2
# home are scratch folders under WORK.
#
# The stand-ins log every call, one line each (name and arguments), to
# $DS_CALLS. The systemctl stand-in keeps one unit's state in files under
# $DS_SVC/state:
#   unit      "<user|system> <name>", or no file: no unit at all
#   active    active | inactive | failed
#   enabled   enabled | disabled
#   stop-fails, start-fails, disable-fails   that call fails, exit 1
#   stop-keeps-running                       a stop "works" but the unit
#                                            stays active
# uname -s prints $DS_SVC/uname-s (default Linux). launchctl prints
# $DS_SVC/launchctl-<sub> on stderr when that file is there. loginctl,
# journalctl and sudo fail loudly: a row that runs one fails its calls
# check. curl logs, then runs the real curl (the CLI's calls to the stub
# daemon); ds_calls leaves those lines out.
#
# A python stub daemon on a free loopback port records every request in
# $DS_REQS ("METHOD path body") and answers $DS_WORK/reply (default
# {"ok":true}).
#
# Use:
#   ds_setup                    once: folders, stand-ins, the stub, the guard
#   ds_reset                    before each row: no unit, Linux, fresh files
#   ds_unit user active enabled set the unit (scope, state, enabled)
#   ds_run <words...>           runs `k2 daemon <words...>`; OUT ERR RC
#   DS_STDIN=<text> ds_run ...  the same with that text on stdin
#   DS_NOCONN=1 ds_run ...      the same with no port or token given
#   ds_calls                    the service calls the row made, one a line
#   ok / bad / eq               the counters

DS_TOOLS_LIST="bash sh env python3 awk sed grep tr cat head tail cut sort uniq wc date mkdir chmod mv rm cp ln id ps readlink realpath dirname basename mktemp sleep stat touch od base64 sha256sum shasum kill find ls tee printf hostname whoami expr test true false tty stty getconf seq cmp diff"
DS_STAND_INS="systemctl launchctl loginctl journalctl sudo curl uname"
DS_NEVER_REAL="systemctl launchctl loginctl journalctl sudo doas pkexec su service initctl curl wget uname"

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
# eq <label> <got> <want>
eq() {
    if [ "$2" = "$3" ]; then
        ok "$1"
    else
        bad "$1"
        printf '      want: %s\n      got:  %s\n' "$(printf %q "$3")" "$(printf %q "$2")" >&2
    fi
}

DS_STUB_PID=""
ds_cleanup() {
    if [ -n "$DS_STUB_PID" ]; then
        kill "$DS_STUB_PID" 2>/dev/null || true
        wait "$DS_STUB_PID" 2>/dev/null || true
    fi
    [ -n "${DS_WORK:-}" ] && rm -rf "$DS_WORK"
}

_ds_exe() { cat >"$1"; chmod 755 "$1"; }

ds_setup() {
    DS_WORK="$(mktemp -d -t k2-daemon-stand-ins-XXXXXX)"
    trap ds_cleanup EXIT
    DS_SVC="$DS_WORK/svc"
    DS_SYS="$DS_WORK/sys"
    DS_CALLS="$DS_WORK/calls.log"
    DS_REQS="$DS_WORK/requests.log"
    DS_HOME="$DS_WORK/home"
    DS_K2HOME="$DS_HOME/.k2"
    mkdir -p "$DS_SVC/state" "$DS_SYS" "$DS_K2HOME" "$DS_WORK/tmp"
    : >"$DS_CALLS"
    : >"$DS_REQS"

    local S="$DS_SVC" L="$DS_CALLS" log
    log="{ printf '%s' \"\${0##*/}\"; for a in \"\$@\"; do printf ' %s' \"\$a\"; done; printf '\\n'; } >> '$L'"

    _ds_exe "$S/systemctl" <<EOF
#!/bin/sh
$log
st='$S/state'
scope=system
if [ "\$1" = --user ]; then scope=user; shift; fi
if [ ! -f "\$st/unit" ]; then echo "No files found for \$2." >&2; exit 1; fi
read s u < "\$st/unit"
if [ "\$s" != "\$scope" ] || [ "\$2" != "\$u" ]; then echo "No files found for \$2." >&2; exit 1; fi
case "\$1" in
  cat) echo "[Service]"; echo "ExecStart=/opt/k2/k2-daemon" ;;
  is-active) a=\$(cat "\$st/active"); echo "\$a"; [ "\$a" = active ] || exit 3 ;;
  is-enabled) e=\$(cat "\$st/enabled"); echo "\$e"; [ "\$e" = enabled ] || exit 1 ;;
  start|stop|disable)
    if [ -f "\$st/\$1-fails" ]; then echo "Failed to \$1 \$2: Access denied" >&2; exit 1; fi
    case "\$1" in
      start) echo active > "\$st/active" ;;
      stop) [ -f "\$st/stop-keeps-running" ] || echo inactive > "\$st/active" ;;
      disable) echo disabled > "\$st/enabled" ;;
    esac ;;
  *) echo "stand-in systemctl: no \$1 here" >&2; exit 1 ;;
esac
EOF
    _ds_exe "$S/launchctl" <<EOF
#!/bin/sh
$log
f='$S/launchctl-'"\$1"
if [ -f "\$f" ]; then cat "\$f" >&2; fi
exit 0
EOF
    local n
    for n in loginctl journalctl sudo; do
        _ds_exe "$S/$n" <<EOF
#!/bin/sh
$log
echo "stand-in $n: never run" >&2
exit 1
EOF
    done
    local real_curl
    real_curl="$(command -v curl)" || { echo "FAIL: no curl for the stub daemon's calls" >&2; exit 1; }
    _ds_exe "$S/curl" <<EOF
#!/bin/sh
$log
exec '$real_curl' "\$@"
EOF
    _ds_exe "$S/uname" <<EOF
#!/bin/sh
case "\$1" in
  -m) echo x86_64 ;;
  *) if [ -f '$S/uname-s' ]; then cat '$S/uname-s'; else echo Linux; fi ;;
esac
EOF
    local t p
    for t in $DS_TOOLS_LIST; do
        p="$(command -v "$t" 2>/dev/null)" || continue
        case "$p" in /*) ln -s "$p" "$DS_SYS/$t" ;; esac
    done
    for t in bash cat tail id python3; do
        [ -e "$DS_SYS/$t" ] || { echo "FAIL: no $t for the rows" >&2; exit 1; }
    done
    DS_PATH="$DS_SVC:$DS_SYS"
    ds_guard

    python3 - "$DS_WORK" <<'PYEOF' &
import os, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer
work = sys.argv[1]
class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def _handle(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode() if n else ""
        path = urllib.parse.urlparse(self.path).path
        with open(work + "/requests.log", "a") as f:
            f.write("%s %s %s\n" % (self.command, path, body))
        try:
            out = open(work + "/reply", "rb").read()
        except FileNotFoundError:
            out = b'{"ok":true}'
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(out)))
        self.end_headers()
        self.wfile.write(out)
    do_GET = _handle
    do_POST = _handle
srv = HTTPServer(("127.0.0.1", 0), H)
with open(work + "/port.tmp", "w") as f:
    f.write(str(srv.server_address[1]))
os.rename(work + "/port.tmp", work + "/port")
srv.serve_forever()
PYEOF
    DS_STUB_PID=$!
    local _i
    for _i in $(seq 1 100); do
        [ -s "$DS_WORK/port" ] && break
        sleep 0.1
    done
    [ -s "$DS_WORK/port" ] || { echo "FAIL: the stub daemon never wrote its port" >&2; exit 1; }
    DS_PORT="$(cat "$DS_WORK/port")"
}

# The guard: the CLI's PATH is the stand-ins then the tools, every
# stand-in is there, and no service manager, sudo or download tool is
# reachable outside the stand-ins' folder. Fails the whole test, at once.
ds_guard() {
    local n got
    [ "$DS_PATH" = "$DS_SVC:$DS_SYS" ] || { echo "FAIL: guard: PATH is not the stand-ins then the tools: $DS_PATH" >&2; exit 1; }
    for n in $DS_STAND_INS; do
        [ -x "$DS_SVC/$n" ] || { echo "FAIL: guard: no stand-in $n" >&2; exit 1; }
    done
    for n in $DS_NEVER_REAL; do
        if [ -e "$DS_SYS/$n" ] || [ -L "$DS_SYS/$n" ]; then
            echo "FAIL: guard: $n is reachable outside the stand-ins: $DS_SYS/$n" >&2
            exit 1
        fi
        got="$(PATH="$DS_PATH"; command -v "$n" 2>/dev/null || true)"
        case "$got" in
            ""|"$DS_SVC/$n") ;;
            *) echo "FAIL: guard: $n resolves to $got, not a stand-in" >&2; exit 1 ;;
        esac
    done
    case "$DS_HOME" in "$DS_WORK"/*) ;; *) echo "FAIL: guard: HOME is not scratch: $DS_HOME" >&2; exit 1 ;; esac
}

DS_TOKEN="daemon-tok-5151"

# Before each row: no unit, Linux, no launchd files, the daemon's port and
# token files, no stop intent, no logged calls or requests.
ds_reset() {
    rm -rf "$DS_SVC/state" "$DS_HOME/Library" "$DS_K2HOME/run"
    mkdir -p "$DS_SVC/state"
    rm -f "$DS_SVC/uname-s" "$DS_SVC"/launchctl-* "$DS_WORK/reply"
    printf '%s\n' "$DS_PORT" >"$DS_K2HOME/daemon.port"
    printf '%s\n' "$DS_TOKEN" >"$DS_K2HOME/daemon.token"
    : >"$DS_CALLS"
    : >"$DS_REQS"
}

# ds_unit <user|system> <active|inactive|failed> <enabled|disabled> [name]
ds_unit() {
    printf '%s %s\n' "$1" "${4:-k2-daemon.service}" >"$DS_SVC/state/unit"
    echo "$2" >"$DS_SVC/state/active"
    echo "$3" >"$DS_SVC/state/enabled"
}

ds_fails() { : >"$DS_SVC/state/$1"; }

ds_mac() {
    echo Darwin >"$DS_SVC/uname-s"
    mkdir -p "$DS_HOME/Library/LaunchAgents"
    echo '<plist/>' >"$DS_HOME/Library/LaunchAgents/dev.k2.daemon.plist"
}

# ds_run <words...>: `k2 daemon <words...>` with the stand-ins; OUT, ERR, RC.
ds_run() {
    ds_guard
    printf '%s' "${DS_STDIN:-}" >"$DS_WORK/stdin"
    local conn=(K2_PORT="$DS_PORT" K2_HOOK_TOKEN="owner-tok-4242")
    # DS_NOCONN=1: no port or token in the environment (and, when the row
    # took the files away, none on disk): no connection to K2 at all.
    [ "${DS_NOCONN:-0}" = 1 ] && conn=()
    set +e
    # ${conn[@]+...}: an empty array under set -u on bash 3.2.
    OUT="$(env -i HOME="$DS_HOME" PATH="$DS_PATH" TMPDIR="$DS_WORK/tmp" LC_ALL=C ${conn[@]+"${conn[@]}"} \
        "$K2" daemon "$@" <"$DS_WORK/stdin" 2>"$DS_WORK/err")"
    RC=$?
    set -e
    ERR="$(cat "$DS_WORK/err")"
}

# The service calls the row made (the stand-ins' log without curl).
ds_calls() { grep -v '^curl' "$DS_CALLS" || true; }

# ds_files: which of the files a verb may take away are there.
ds_files() {
    local f out=""
    for f in daemon.port daemon.token run/stop-intent.json; do
        if [ -e "$DS_K2HOME/$f" ]; then out="$out $f"; fi
    done
    [ -e "$DS_HOME/Library/LaunchAgents/dev.k2.daemon.plist" ] && out="$out plist"
    printf '%s\n' "${out# }"
}

# ds_state: the unit's state after the row ("<active> <enabled>").
ds_state() { printf '%s %s\n' "$(cat "$DS_SVC/state/active")" "$(cat "$DS_SVC/state/enabled")"; }
