#!/usr/bin/env bash
# A locked tool stays locked when flags come before its name. The CLI picks
# the credential from the tool a command names (_cli_apply_auth_for_verb):
# an `id` tool in an agent's session sends the session's passport, and with
# no passport is refused (exit 3) before any request. `k2 setup --json
# chief-of-staff` names the locked agent-harness tool the same as
# `k2 setup chief-of-staff`, and must be refused the same way.
#
# The table is read from the CLI's own `_cli_tool_id_for_verb`, so every
# entry whose tool is an `id` tool (each locked one among them) is tried,
# with --json before its sub-verb (a family keyed on one) or after its verb:
#   1. pure: in a session with a passport the credential is the passport;
#      with none, exit 3 and the same teaching error as without the flag;
#   2. end to end against a stub daemon (session marker set, no passport,
#      the owner token readable on disk): the same exit code and stderr as
#      the form without the flag, and no request at all;
#   3. k2 setup --json chief-of-staff with a passport sends the passport;
#   4. unlocked forms are unchanged (Open verbs, help, a flag's value that
#      happens to spell a locked sub-verb);
#   5. the session-passport check (_cli_check_session_passport) reads the
#      same tool: every form, with and without the flag, is refused exactly
#      as a first-word id verb is when the passport is stale (socket gone,
#      the daemon rejects it: only the whoami probe is sent) and when it
#      runs under a shared Codex app-server (no request at all). Open forms
#      get neither check.
#
# Never touches the real ~/.k2 (HOME is sandboxed, env -i).
# bash-only: evals the CLI's tool policy block and reads its table from the bash source.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
PY="$(command -v python3)" || { echo "FAIL: python3 is required for the stub daemon" >&2; exit 1; }

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
assert_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got=$(printf %q "$2") want=$(printf %q "$3"))"; fi; }

WORK="$(mktemp -d -t k2-locked-flags-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK"
}
trap cleanup EXIT

OWNER="owner-tok-5d1e"
PASSPORT="sess7.passport3c9a"

# ── The table: "<verb> <sub or -> <tool id> <locked 0|1>" per form ─────
# Read from the bash source (CLI_BASH_SOURCE): the same rows whichever
# program runs. A sub-verb pattern of `*` is tried with `list`; the empty
# and help sub-verbs are not tools.
"$PY" - "$CLI_BASH_SOURCE" >"$WORK/table" <<'PY'
import re, sys
src = open(sys.argv[1]).read()
m = re.search(r'^_cli_tool_id_for_verb\(\) \{\n(.*?)^\}\n', src, re.S | re.M)
if not m:
    sys.exit("no _cli_tool_id_for_verb in the CLI")
lk = re.search(r'^_cli_tool_is_locked\(\) \{\n    case "\$\{1:-\}" in\n        ([^)]*)\) return 0', src, re.M)
md = re.search(r'^_cli_tool_default_mode\(\) \{\n    case "\$\{1:-\}" in\n(.*?)\n    esac', src, re.S | re.M)
if not lk or not md:
    sys.exit("no locked or mode table in the CLI")
locked = set(lk.group(1).split("|"))
id_tools = set()
for line in md.group(1).split("\n"):
    t = re.match(r'^ +([^)]*)\) echo "id"', line)
    if t:
        id_tools.update(t.group(1).split("|"))
top, rows = None, []
for line in m.group(1).split("\n"):
    if re.match(r"^ *#", line):
        continue
    t = re.match(r"^        ([^ #][^)]*)\)(.*)$", line)
    pairs = re.findall(r'([^ ;()]+)\) echo "([^"]*)"', line)
    if t:
        top = t.group(1)
        if pairs:
            rows.append((top, None, pairs[0][1]))
        continue
    for pat, tid in pairs:
        rows.append((top, pat, tid))
n = 0
for verbs, subs, tid in rows:
    if tid not in id_tools:
        continue
    for verb in verbs.split("|"):
        if verb in ("*", ""):
            continue
        if subs is None:
            print(verb, "-", tid, int(tid in locked)); n += 1
            continue
        for sub in subs.split("|"):
            if sub in ('""', "help", "--help", "-h"):
                continue
            print(verb, "list" if sub == "*" else sub, tid, int(tid in locked)); n += 1
if n < 40:
    sys.exit("only %d id-tool forms in the table" % n)
missing = locked - {r[2] for r in rows}
if missing:
    sys.exit("locked tools no verb names: %s" % sorted(missing))
PY
echo "== the table: $(wc -l <"$WORK/table" | tr -d ' ') id-tool forms, $(awk '$4 == 1' "$WORK/table" | wc -l | tr -d ' ') of them locked =="
locked_ids="$(awk '$4 == 1 {print $3}' "$WORK/table" | sort -u | tr '\n' ' ')"
echo "  locked tools: $locked_ids"
nested="$(awk '$2 != "-" {print $1" "$2}' "$WORK/table" | tr '\n' ',')"
echo "  keyed on a sub-verb: $nested"
case ",$nested" in *",setup chief-of-staff,"*) ok "the table has setup chief-of-staff" ;; *) bad "setup chief-of-staff is not in the table" ;; esac

# ── 1. Pure: the policy block of the CLI under test ───────────────────
echo "== pure: the credential for each form with a leading flag =="
# shellcheck disable=SC1090
eval "$(sed -n '/^# BEGIN_CLI_TOOL_POLICY/,/^# END_CLI_TOOL_POLICY/p' "$K2_CLI")"
declare -F _cli_apply_auth_for_verb >/dev/null || { echo "FAIL: no _cli_apply_auth_for_verb in $K2_CLI" >&2; exit 1; }

DISK_OWNER_TOKEN="$OWNER"
K2_HOOK_SOCK="$WORK/cell.sock"
# auth_token <args...>: TOKEN after the selection, in a session with a passport.
auth_token() {
    TOKEN="$OWNER"; SCOPED_TOKEN="$PASSPORT"
    _cli_apply_auth_for_verb "$@"
    printf '%s' "$TOKEN"
}
# refusal <args...>: "<rc>|<stderr>" in a session with no passport.
refusal() {
    local e rc
    set +e
    e="$(TOKEN="$OWNER"; SCOPED_TOKEN=""; _cli_apply_auth_for_verb "$@" 2>&1 >/dev/null)"
    rc=$?
    set -e
    printf '%s|%s' "$rc" "$e"
}
while read -r verb sub tid lk; do
    if [ "$sub" = "-" ]; then plain=("$verb"); flagged=("$verb" --json); else plain=("$verb" "$sub"); flagged=("$verb" --json "$sub"); fi
    if [ "$lk" = 1 ]; then label="${flagged[*]} ($tid, locked)"; else label="${flagged[*]} ($tid)"; fi
    assert_eq "$label: the passport" "$(auth_token "${flagged[@]}")" "$PASSPORT"
    want="$(refusal "${plain[@]}")"
    assert_eq "$label: no passport, refused as without the flag" "$(refusal "${flagged[@]}")" "$want"
    case "$want" in 3\|*) ok "$label: exit 3 without a passport" ;; *) bad "$label: not exit 3 without a passport: $want" ;; esac
done <"$WORK/table"

echo "== pure: k2 setup's flags and their values =="
assert_eq "setup --json --json chief-of-staff: the passport" "$(auth_token setup --json --json chief-of-staff)" "$PASSPORT"
assert_eq "setup --name Ada chief-of-staff: the passport" "$(auth_token setup --name Ada chief-of-staff)" "$PASSPORT"
assert_eq "setup --about x --tool claude chief-of-staff: the passport" "$(auth_token setup --about x --tool claude chief-of-staff)" "$PASSPORT"
assert_eq "setup -- chief-of-staff: the passport" "$(auth_token setup -- chief-of-staff)" "$PASSPORT"

echo "== pure: unlocked forms unchanged (Open: the owner token) =="
for form in "setup" "setup --json" "setup --json hide" "setup skip chief-of-staff" "setup --name chief-of-staff" \
            "setup --help chief-of-staff" "agent --help team" "agent hire" "agent --json hire" "workspace --json list" \
            "desktop --json" "desktop status" "organization --help" "activity --json" "whats-new"; do
    # shellcheck disable=SC2086
    assert_eq "$form: the owner token" "$(auth_token $form)" "$OWNER"
done
# An id verb whose first argument already names its tool keeps it.
assert_eq "organization --json: the passport" "$(auth_token organization --json)" "$PASSPORT"
assert_eq "agent team --json: the passport" "$(auth_token agent team --json)" "$PASSPORT"

# ── 2. End to end: refused the same, no request ──────────────────────
cat >"$WORK/stub.py" <<'PY'
import json, os
from http.server import BaseHTTPRequestHandler, HTTPServer
LOG_FILE = os.environ["LOG_FILE"]
class H(BaseHTTPRequestHandler):
    def _answer(self, method):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode() if n else ""
        with open(LOG_FILE, "a") as f:
            f.write(json.dumps({"method": method, "path": self.path, "auth": self.headers.get("Authorization"), "body": body}) + "\n")
        # With a `reject` file, whoami answers with the daemon's auth
        # refusal: the passport names no live session.
        if self.path.startswith("/cli/whoami") and os.path.exists(os.path.join(os.path.dirname(LOG_FILE), "reject")):
            data = json.dumps({"error": "Invalid or missing auth token"}).encode()
            self.send_response(401)
        else:
            data = b"Made your Chief of Staff: Ada (ada) at /home/me/k2-agents/ada\n"
            self.send_response(200)
        self.send_header("Content-Type", "text/plain; charset=utf-8")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    def do_GET(self):
        self._answer("GET")
    def do_POST(self):
        self._answer("POST")
    def log_message(self, *_a):
        pass
HTTPServer(("127.0.0.1", int(os.environ["PORT"])), H).serve_forever()
PY
PORT="$("$PY" -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
LOG_FILE="$WORK/requests"
: >"$LOG_FILE"
LOG_FILE="$LOG_FILE" PORT="$PORT" "$PY" "$WORK/stub.py" &
STUB_PID=$!
up=0
for _i in $(seq 1 50); do
    if curl -s --connect-timeout 1 -o /dev/null "http://127.0.0.1:${PORT}/up"; then up=1; break; fi
    sleep 0.1
done
[ "$up" = 1 ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }

mkdir -p "$WORK/home/.k2"
printf '%s' "$OWNER" >"$WORK/home/.k2/heartbeat.token"
chmod 600 "$WORK/home/.k2/heartbeat.token"
printf '%s' "$PORT" >"$WORK/home/.k2/heartbeat.port"

# VIA: words to run the CLI under (empty: run it directly).
VIA=()
run() { # env words..., then --, then the k2 args
    local envs=()
    while [ "$1" != "--" ]; do envs+=("$1"); shift; done
    shift
    : >"$LOG_FILE"
    set +e
    out="$(cd "$WORK" && env -i PATH="/usr/bin:/bin" HOME="$WORK/home" ${envs[@]+"${envs[@]}"} ${VIA[@]+"${VIA[@]}"} "$K2_CLI" "$@" 2>"$WORK/err" </dev/null)"
    rc=$?
    err="$(cat "$WORK/err")"
    set -e
}
requests() { wc -l <"$LOG_FILE" | tr -d ' '; }

echo "== end to end: a session with no passport, the owner token on disk =="
NOPASS=(K2_PORT="$PORT" K2_HOOK_SOCK="$WORK/cell.sock")
while read -r verb sub tid lk; do
    if [ "$sub" = "-" ]; then plain=("$verb"); flagged=("$verb" --json); else plain=("$verb" "$sub"); flagged=("$verb" --json "$sub"); fi
    run "${NOPASS[@]}" -- "${plain[@]}"
    want_rc="$rc"; want_err="$err"
    run "${NOPASS[@]}" -- "${flagged[@]}"
    label="k2 ${flagged[*]} ($tid)"
    assert_eq "$label: exit as without the flag" "$rc" "$want_rc"
    assert_eq "$label: exit 3" "$rc" "3"
    assert_eq "$label: stderr as without the flag" "$err" "$want_err"
    assert_eq "$label: no request" "$(requests)" "0"
done <"$WORK/table"

echo "== end to end: k2 setup --json chief-of-staff with a passport =="
# The session socket is missing, so the passport check probes whoami first
# (the daemon accepts the passport), then the command's own request.
run K2_PORT="$PORT" K2_HOOK_SOCK="$WORK/cell.sock" K2_HOOK_TOKEN="$PASSPORT" -- setup --json chief-of-staff --name Ada --about ops --tool claude
assert_eq "with a passport: exit 0" "$rc" "0"
assert_eq "with a passport: the whoami probe, then one request" "$(requests)" "2"
case "$(head -1 "$LOG_FILE")" in
    *'"path": "/cli/whoami"'*"\"auth\": \"Bearer $PASSPORT\""*) ok "with a passport: the probe is whoami with the passport" ;;
    *) bad "with a passport: the first request is not the whoami probe with the passport: $(head -1 "$LOG_FILE")" ;;
esac
case "$(sed -n 2p "$LOG_FILE")" in
    *'"path": "/cli/first-run/'*"\"auth\": \"Bearer $PASSPORT\""*) ok "with a passport: the passport is sent" ;;
    *) bad "with a passport: not the passport: $(cat "$LOG_FILE")" ;;
esac
if grep -Fq -- "$OWNER" "$LOG_FILE"; then bad "with a passport: the owner token was sent"; else ok "with a passport: the owner token never sent"; fi

echo "== end to end: the owner's own terminal is unchanged =="
run -- setup --json chief-of-staff --name Ada --about ops --tool claude
assert_eq "owner: exit 0" "$rc" "0"
case "$(cat "$LOG_FILE")" in
    *"\"auth\": \"Bearer $OWNER\""*) ok "owner: the owner token, as before" ;;
    *) bad "owner: not the owner token: $(cat "$LOG_FILE")" ;;
esac

# ── 5. The session-passport check reads the same tool ──────────────────
# The reference is a first-word id verb (`k2 compute`): every form, with and
# without the flag, must be refused exactly as it is.
GONE="$WORK/gone.sock"
STALE=(K2_PORT="$PORT" K2_HOOK_SOCK="$GONE" K2_HOOK_TOKEN="$PASSPORT")
# Names a shell like Codex's shared app-server and runs k2 as its child
# (the trailing exit keeps bash from exec'ing k2 in its place).
cat >"$WORK/codex-shared.sh" <<'SH'
exec -a "codex app-server --listen unix://" /bin/bash -c '"$0" "$@"; exit $?' "$@"
SH
CODEX_VIA=(/bin/bash "$WORK/codex-shared.sh")
both_forms() { # <verb> <sub or -> sets plain and flagged
    if [ "$2" = "-" ]; then plain=("$1"); flagged=("$1" --json); else plain=("$1" "$2"); flagged=("$1" --json "$2"); fi
}

echo "== passport check: a stale passport (socket gone, the daemon rejects it) =="
touch "$WORK/reject"
VIA=()
run "${STALE[@]}" -- compute
ref_rc="$rc"; ref_err="$err"
assert_eq "k2 compute: exit 3" "$ref_rc" "3"
case "$ref_err" in
    *"credentials are stale"*"$GONE"*) ok "k2 compute: the stale-passport error naming the socket" ;;
    *) bad "k2 compute: not the stale-passport error: $ref_err" ;;
esac
while read -r verb sub tid lk; do
    both_forms "$verb" "$sub"
    for form in plain flagged; do
        if [ "$form" = plain ]; then args=("${plain[@]}"); else args=("${flagged[@]}"); fi
        run "${STALE[@]}" -- "${args[@]}"
        label="stale: k2 ${args[*]} ($tid)"
        assert_eq "$label: exit as k2 compute" "$rc" "$ref_rc"
        assert_eq "$label: stderr as k2 compute" "$err" "$ref_err"
        assert_eq "$label: only the whoami probe" "$(requests)" "1"
        case "$(cat "$LOG_FILE")" in
            *'"path": "/cli/whoami"'*"\"auth\": \"Bearer $PASSPORT\""*) ok "$label: the probe is whoami with the passport" ;;
            *) bad "$label: not the whoami probe: $(cat "$LOG_FILE")" ;;
        esac
        if grep -Fq -- "$OWNER" "$LOG_FILE"; then bad "$label: the owner token was sent"; else ok "$label: the owner token never sent"; fi
    done
done <"$WORK/table"

echo "== passport check: under a shared Codex app-server =="
rm -f "$WORK/reject"
SHARED=("${STALE[@]}" CODEX_THREAD_ID="thread-1")
VIA=("${CODEX_VIA[@]}")
run "${SHARED[@]}" -- compute
ref_rc="$rc"; ref_err="$err"
assert_eq "shared: k2 compute: exit 3" "$ref_rc" "3"
case "$ref_err" in
    *"shared Codex app-server started by another session"*) ok "shared: k2 compute: the shared-server error" ;;
    *) bad "shared: k2 compute: not the shared-server error: $ref_err" ;;
esac
while read -r verb sub tid lk; do
    both_forms "$verb" "$sub"
    for form in plain flagged; do
        if [ "$form" = plain ]; then args=("${plain[@]}"); else args=("${flagged[@]}"); fi
        run "${SHARED[@]}" -- "${args[@]}"
        label="shared: k2 ${args[*]} ($tid)"
        assert_eq "$label: exit as k2 compute" "$rc" "$ref_rc"
        assert_eq "$label: stderr as k2 compute" "$err" "$ref_err"
        assert_eq "$label: no request" "$(requests)" "0"
    done
done <"$WORK/table"

echo "== passport check: Open forms get neither check =="
# Open forms that send no whoami of their own (`k2 activity` asks whoami for
# the passport's workspace, so it is not one of them).
touch "$WORK/reject"
for form in "setup --json hide" "agent --help team" "organization --help" "help" "whats-new"; do
    # shellcheck disable=SC2206
    args=($form)
    VIA=()
    run "${STALE[@]}" -- "${args[@]}"
    case "$err" in *"credentials are stale"*) bad "stale: k2 $form: refused as stale" ;; *) ok "stale: k2 $form: no stale refusal" ;; esac
    if grep -q '"/cli/whoami' "$LOG_FILE"; then bad "stale: k2 $form: a whoami probe"; else ok "stale: k2 $form: no whoami probe"; fi
    VIA=("${CODEX_VIA[@]}")
    run "${STALE[@]}" CODEX_THREAD_ID="thread-1" -- "${args[@]}"
    case "$err" in *"shared Codex app-server"*) bad "shared: k2 $form: refused as shared" ;; *) ok "shared: k2 $form: no shared-server refusal" ;; esac
done
VIA=()
rm -f "$WORK/reject"

echo
echo "locked_tool_leading_flags: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
