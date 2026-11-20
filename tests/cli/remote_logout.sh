#!/usr/bin/env bash
# k2 remote logout --host H ends this computer's session on another K2
# server: POST /cli/auth/logout with the token in the Authorization header
# only (never the URL, the body, a child's argv or environment, or k2's own
# argv). The token is the saved one (_remote_token_for_host order:
# $K2_REMOTE_TOKEN, connect-tokens.json, the keychain) or one from
# --token-stdin. After a 2xx the saved copies of THAT token go
# (connect-tokens.json and the keychain items); a saved token that differs
# stays. A refusal or no answer keeps everything and exits non-zero. No
# --host is a usage error, and -h prints help and sends nothing.
#
# The real CLI runs against a stub server on TCP that records each request.
# Every python3, curl and env the CLI starts is wrapped so its argv and
# environment are logged; `security` (macOS) and `secret-tool` (Linux) are
# stubs on PATH backed by files, so the real keychain is never touched.
#
# No daemon; a throwaway HOME; never touches the real ~/.k2. Runs with the
# bash 3.2 of macOS as well (cli/k2 runs under /bin/bash).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"

W="$(mktemp -d "${TMPDIR:-/tmp}/k2rlo-XXXXXX")"
ARGLOG="$W/child-argv.log"
ENVLOG="$W/child-env.log"
REQLOG="$W/requests.log"
: >"$ARGLOG"; : >"$ENVLOG"; : >"$REQLOG"

cleanup() {
    if [ -n "${TCP_PID:-}" ]; then
        kill "$TCP_PID" 2>/dev/null || true
        wait "$TCP_PID" 2>/dev/null || true
    fi
    rm -rf "$W"
}
trap cleanup EXIT

REAL_PY="$(command -v python3)" || { echo "FAIL: python3 not found" >&2; exit 1; }
REAL_CURL="$(command -v curl)" || { echo "FAIL: curl not found" >&2; exit 1; }
REAL_ENV="$(command -v env)" || { echo "FAIL: env not found" >&2; exit 1; }

mkdir -p "$W/bin"
for prog in python3 curl env; do
    case "$prog" in
        python3) real="$REAL_PY" ;;
        curl) real="$REAL_CURL" ;;
        env) real="$REAL_ENV" ;;
    esac
    cat >"$W/bin/$prog" <<WRAP
#!/usr/bin/env bash
printf '$prog %s\n' "\$*" >>"$ARGLOG"
{ printf '== $prog\n'; "$REAL_ENV"; } >>"$ENVLOG"
exec "$real" "\$@"
WRAP
    chmod +x "$W/bin/$prog"
done

# Keychain stubs: one file per (service, account) holds the secret.
mkdir -p "$W/kc-security" "$W/kc-secret-tool"
cat >"$W/bin/security" <<STUB
#!/bin/bash
printf 'security %s\n' "\$*" >>"$ARGLOG"
{ printf '== security\n'; "$REAL_ENV"; } >>"$ENVLOG"
cmd="\$1"; shift
svc=""; acct=""
while [ \$# -gt 0 ]; do
    case "\$1" in
        -s) svc="\$2"; shift 2 ;;
        -a) acct="\$2"; shift 2 ;;
        *) shift ;;
    esac
done
f="$W/kc-security/\$svc@@\$acct"
case "\$cmd" in
    find-generic-password) [ -f "\$f" ] || exit 44; cat "\$f"; echo ;;
    delete-generic-password) [ -f "\$f" ] || exit 44; rm -f "\$f"; echo "security \$svc \$acct" >>"$W/kc-deletes.log" ;;
    *) exit 1 ;;
esac
STUB
cat >"$W/bin/secret-tool" <<STUB
#!/bin/bash
printf 'secret-tool %s\n' "\$*" >>"$ARGLOG"
{ printf '== secret-tool\n'; "$REAL_ENV"; } >>"$ENVLOG"
cmd="\$1"; shift
svc=""; acct=""
while [ \$# -gt 0 ]; do
    case "\$1" in
        service) svc="\$2"; shift 2 ;;
        account) acct="\$2"; shift 2 ;;
        *) shift ;;
    esac
done
f="$W/kc-secret-tool/\$svc@@\$acct"
case "\$cmd" in
    lookup) [ -f "\$f" ] || exit 1; cat "\$f" ;;
    clear) rm -f "\$f"; echo "secret-tool \$svc \$acct" >>"$W/kc-deletes.log" ;;
    *) exit 1 ;;
esac
STUB
chmod +x "$W/bin/security" "$W/bin/secret-tool"
export PATH="$W/bin:$PATH"

# A throwaway HOME, outside any K2 session (test process only).
export HOME="$W/home"
mkdir -p "$HOME/.k2"
for p in HOOK_SOCK HOOK_TOKEN CELL PORT HOST REMOTE_TOKEN HOME CONNECT_DB_URL; do
    unset "K5_$p" "K2_$p" "K2SO_$p" 2>/dev/null || true
done

# The remote server: records each request; answers per $W/mode.
printf '200' >"$W/mode"
"$REAL_PY" - "$W/tcp-port" "$REQLOG" "$W/mode" <<'PYEOF' &
import http.server, json, os, sys, threading
port_path, log_path, mode_path = sys.argv[1], sys.argv[2], sys.argv[3]
lock = threading.Lock()
class H(http.server.BaseHTTPRequestHandler):
    def _any(self):
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n).decode("utf-8", "replace") if n else ""
        with lock, open(log_path, "a") as f:
            f.write(json.dumps({"method": self.command, "target": self.path,
                                "headers": [[k, v] for k, v in self.headers.items()],
                                "body": raw}) + "\n")
        mode = open(mode_path).read().strip()
        if mode == "401":
            status, obj = 401, {"error": "unauthorized"}
        else:
            status, obj = 200, {"success": True}
        b = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)
    do_GET = do_POST = _any
    def log_message(self, *a):
        pass
srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
with open(port_path + ".tmp", "w") as f:
    f.write(str(srv.server_address[1]))
os.rename(port_path + ".tmp", port_path)
srv.serve_forever()
PYEOF
TCP_PID=$!
for _ in $(seq 1 50); do [ -s "$W/tcp-port" ] && break; sleep 0.1; done
[ -s "$W/tcp-port" ] || { echo "FAIL: the stub server never came up" >&2; exit 1; }
REMOTE="http://127.0.0.1:$(cat "$W/tcp-port")"
# A port nothing listens on: bind one, note it, close it.
DEAD="http://127.0.0.1:$("$REAL_PY" -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
cd "$W"

PASS=0; FAIL=0
ok() { echo "PASS: $1"; PASS=$((PASS + 1)); }
bad() { echo "FAIL: $1" >&2; FAIL=$((FAIL + 1)); }
expect_eq() {
    if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi
}
expect_has() {
    case "$2" in *"$3"*) ok "$1" ;; *) bad "$1 (no $(printf %q "$3") in $(printf %q "$2"))" ;; esac
}

# fresh: empty logs and stores; the server answers 200.
fresh() {
    : >"$ARGLOG"; : >"$ENVLOG"; : >"$REQLOG"; : >"$W/kc-deletes.log"
    rm -f "$W"/kc-security/* "$W"/kc-secret-tool/* "$HOME/.k2/connect-tokens.json" "$HOME/.k2/connect-hosts.json"
    printf '200' >"$W/mode"
}

# save_json HOST TOKEN: connect-tokens.json maps HOST (and another host)
# to TOKEN; connect-hosts.json gives HOST the id host-1.
save_json() {
    "$REAL_PY" - "$HOME/.k2/connect-tokens.json" "$1" "$2" <<'PY'
import json, sys
json.dump({sys.argv[2]: sys.argv[3], "other.k2.dev": "k2rt_OTHER_HOST_TOKEN"}, open(sys.argv[1], "w"))
PY
    save_host "$1"
}
save_host() {
    "$REAL_PY" - "$HOME/.k2/connect-hosts.json" "$1" <<'PY'
import json, sys
json.dump([{"id": "host-1", "hostname": sys.argv[2], "label": "Test"}], open(sys.argv[1], "w"))
PY
}
# save_kc STORE SERVICE TOKEN: a keychain item for host-1.
save_kc() { printf '%s' "$3" >"$W/kc-$1/$2@@host-1"; }

# json_value HOST: the connect-tokens.json value for HOST, or <none>.
json_value() {
    "$REAL_PY" - "$HOME/.k2/connect-tokens.json" "$1" <<'PY'
import json, sys
print(json.load(open(sys.argv[1])).get(sys.argv[2], "<none>"))
PY
}
kc_value() {
    if [ -f "$W/kc-$1/$2@@host-1" ]; then cat "$W/kc-$1/$2@@host-1"; else printf '<none>'; fi
}

run_k2() {
    local input="$1"; shift
    printf 'k2 %s\n' "$*" >>"$ARGLOG"
    RC=0
    OUT="$(printf '%s' "$input" | "$K2_CLI" "$@" 2>"$W/stderr")" || RC=$?
    ERR="$(cat "$W/stderr")"
}

# assert_sent LABEL TOKEN: exactly one request, POST /cli/auth/logout,
# with `Authorization: Bearer TOKEN` and the token nowhere else in it.
assert_sent() {
    local label="$1" tok="$2" why=""
    why="$("$REAL_PY" - "$REQLOG" "$tok" <<'PY'
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
tok = sys.argv[2]
if len(rows) != 1:
    print("%d requests, want 1" % len(rows)); sys.exit()
r = rows[0]
if r["method"] != "POST" or r["target"] != "/cli/auth/logout":
    print("got %s %s" % (r["method"], r["target"])); sys.exit()
auth = [v for k, v in r["headers"] if k.lower() == "authorization"]
if auth != ["Bearer " + tok]:
    print("Authorization headers %r" % auth); sys.exit()
others = [k for k, v in r["headers"] if k.lower() != "authorization" and tok in v]
if others or tok in r["body"]:
    print("the token is also in %r or the body" % others)
PY
)"
    if [ -z "$why" ]; then ok "$label: the token went in the Authorization header only"; else bad "$label: $why"; fi
}
assert_no_request() {
    if [ -s "$REQLOG" ]; then bad "$1: a request was sent: $(cut -c1-200 "$REQLOG")"; else ok "$1: no request"; fi
}
# assert_private LABEL TOKEN [argv-only]: the token is in no child's argv
# (nor k2's own), and unless argv-only in no child's environment.
assert_private() {
    if grep -qF -- "$2" "$ARGLOG"; then bad "$1: the token is in an argv: $(grep -F -- "$2" "$ARGLOG" | cut -c1-120 | head -3)"; else ok "$1: the token is in no argv"; fi
    [ "${3:-}" = argv-only ] && return 0
    if grep -qF -- "$2" "$ENVLOG"; then bad "$1: the token is in a child's environment"; else ok "$1: the token is in no child's environment"; fi
}

SVC=dev.k2.connect.host-token
OLD_SVC=com.k2so.connect.host-token

echo "== the saved token (connect-tokens.json and the keychain) =="
fresh
T=k2rt_LOGOUT_SAVED_token_A
save_json "$REMOTE" "$T"; save_kc security "$SVC" "$T"; save_kc security "$OLD_SVC" "$T"
run_k2 "" remote logout --host "$REMOTE"
expect_eq "saved: exit 0" "$RC" "0"
expect_has "saved: one line on success" "$OUT" "signed out of $REMOTE"
expect_eq "saved: stdout is one line" "$(printf '%s\n' "$OUT" | wc -l | tr -d ' ')" "1"
expect_has "saved: says what it removed" "$OUT" "removed the saved token: connect-tokens.json, keychain"
assert_sent "saved" "$T"
assert_private "saved" "$T"
expect_eq "saved: the connect-tokens.json entry is gone" "$(json_value "$REMOTE")" "<none>"
expect_eq "saved: another host's entry stays" "$(json_value other.k2.dev)" "k2rt_OTHER_HOST_TOKEN"
expect_eq "saved: the keychain item is gone" "$(kc_value security "$SVC")" "<none>"
expect_eq "saved: the old keychain item is gone" "$(kc_value security "$OLD_SVC")" "<none>"
expect_eq "saved: connect-tokens.json stays 0600" "$("$REAL_PY" -c 'import os,sys; print(oct(os.stat(sys.argv[1]).st_mode & 0o777))' "$HOME/.k2/connect-tokens.json")" "0o600"
expect_has "saved: connect-hosts.json keeps the server" "$(cat "$HOME/.k2/connect-hosts.json")" "host-1"

echo "== --token-stdin with a different saved token =="
fresh
T=k2rt_LOGOUT_PASTED_token_B
KEEP=k2rt_LOGOUT_KEPT_token_B2
save_json "$REMOTE" "$KEEP"; save_kc security "$SVC" "$KEEP"
run_k2 "$T" remote logout --host "$REMOTE" --token-stdin
expect_eq "stdin: exit 0" "$RC" "0"
expect_has "stdin: nothing saved matched" "$OUT" "no saved copy of this token here"
assert_sent "stdin" "$T"
assert_private "stdin" "$T"
expect_eq "stdin: the different saved token stays (json)" "$(json_value "$REMOTE")" "$KEEP"
expect_eq "stdin: the different saved token stays (keychain)" "$(kc_value security "$SVC")" "$KEEP"
expect_eq "stdin: no keychain delete" "$(cat "$W/kc-deletes.log")" ""

echo "== --token-stdin with the saved token =="
fresh
T=k2rt_LOGOUT_PASTED_SAME_C
save_json "$REMOTE" "$T"; save_kc security "$SVC" "$T"
run_k2 "$T" remote logout --host "$REMOTE" --token-stdin
expect_eq "stdin same: exit 0" "$RC" "0"
assert_sent "stdin same" "$T"
assert_private "stdin same" "$T"
expect_eq "stdin same: the entry is gone" "$(json_value "$REMOTE")" "<none>"
expect_eq "stdin same: the keychain item is gone" "$(kc_value security "$SVC")" "<none>"

echo "== the Linux keyring (secret-tool) =="
fresh
T=k2rt_LOGOUT_KEYRING_token_D
save_host "$REMOTE"; save_kc secret-tool "$SVC" "$T"
run_k2 "" remote logout --host "$REMOTE"
expect_eq "keyring: exit 0" "$RC" "0"
assert_sent "keyring" "$T"
assert_private "keyring" "$T"
expect_eq "keyring: the item is gone" "$(kc_value secret-tool "$SVC")" "<none>"
expect_has "keyring: secret-tool clear ran for host-1" "$(cat "$W/kc-deletes.log")" "secret-tool $SVC host-1"

echo "== \$K2_REMOTE_TOKEN comes first; a different saved token stays =="
fresh
T=k2rt_LOGOUT_ENV_token_E
KEEP=k2rt_LOGOUT_ENV_KEPT_E2
save_json "$REMOTE" "$KEEP"
export K2_REMOTE_TOKEN="$T"
run_k2 "" remote logout --host "$REMOTE"
unset K2_REMOTE_TOKEN
expect_eq "env: exit 0" "$RC" "0"
assert_sent "env" "$T"
assert_private "env" "$T" argv-only
expect_eq "env: the different saved token stays" "$(json_value "$REMOTE")" "$KEEP"

echo "== a refusal keeps everything =="
fresh
T=k2rt_LOGOUT_REFUSED_token_F
save_json "$REMOTE" "$T"; save_kc security "$SVC" "$T"
printf '401' >"$W/mode"
run_k2 "" remote logout --host "$REMOTE"
expect_eq "401: exit 3" "$RC" "3"
expect_has "401: prints the refusal" "$ERR" "refused (HTTP 401)"
expect_has "401: prints the server's answer" "$ERR" "unauthorized"
expect_eq "401: nothing on stdout" "$OUT" ""
assert_sent "401" "$T"
expect_eq "401: the entry stays" "$(json_value "$REMOTE")" "$T"
expect_eq "401: the keychain item stays" "$(kc_value security "$SVC")" "$T"

echo "== no answer keeps everything =="
fresh
T=k2rt_LOGOUT_NOANSWER_token_G
save_json "$DEAD" "$T"
run_k2 "" remote logout --host "$DEAD"
expect_eq "no answer: exit 1" "$RC" "1"
expect_has "no answer: says so" "$ERR" "no answer from $DEAD"
expect_eq "no answer: the entry stays" "$(json_value "$DEAD")" "$T"

echo "== usage and help send nothing =="
fresh
run_k2 "" remote logout
expect_eq "no --host: exit 2" "$RC" "2"
expect_has "no --host: usage line" "$ERR" "usage: k2 remote logout --host HOST [--token-stdin]"
assert_no_request "no --host"
fresh
run_k2 "" remote logout --host
expect_eq "--host with no value: exit 2" "$RC" "2"
assert_no_request "--host with no value"
fresh
run_k2 "" remote logout --host "$REMOTE" --token k2rt_ON_ARGV
expect_eq "--token: exit 2" "$RC" "2"
expect_has "--token: points to --token-stdin" "$ERR" "use --token-stdin"
assert_no_request "--token"
fresh
run_k2 "" remote logout --host "$REMOTE"
expect_eq "no saved token: exit 2" "$RC" "2"
expect_has "no saved token: says --token-stdin" "$ERR" "pass it with --token-stdin"
assert_no_request "no saved token"
fresh
save_json example.invalid k2rt_LOGOUT_PLAIN_HTTP
run_k2 "" remote logout --host http://example.invalid
expect_eq "plain http to another computer: exit 2" "$RC" "2"
assert_no_request "plain http to another computer"
fresh
save_json "$REMOTE" k2rt_LOGOUT_HELP_token
run_k2 "" remote logout --host "$REMOTE" -h
expect_eq "-h: exit 0" "$RC" "0"
expect_has "-h: says an owner token can't be revoked" "$OUT" "It can't revoke an owner token"
expect_has "-h: says success proves no session" "$OUT" "Success can't prove a session existed"
assert_no_request "-h"
expect_eq "-h: the entry stays" "$(json_value "$REMOTE")" "k2rt_LOGOUT_HELP_token"
run_k2 "" remote --help
expect_eq "remote --help: exit 0" "$RC" "0"
assert_no_request "remote --help"
run_k2 "" remote
expect_eq "bare remote: exit 2" "$RC" "2"
run_k2 "" remote nope
expect_eq "unknown verb: exit 2" "$RC" "2"
assert_no_request "unknown verb"

echo
echo "remote_logout: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
