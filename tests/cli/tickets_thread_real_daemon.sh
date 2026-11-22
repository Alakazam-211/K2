#!/usr/bin/env bash
# 0.46.17 (ticket-thread-v1): a ticket's messages are the Thread
# conversation `ticket:<id>`. The CLI's own verbs, against a REAL daemon,
# land there and read back:
#
#   1. `tickets ask` posts the ask (title, then body) as the first item of
#      `ticket:<id>`, from the asking agent; `tickets show --json` reads
#      the same message back.
#   2. `tickets comment` posts the next item, with its text and the agent
#      as author; `show --json` reads it back and the ticket stays waiting.
#   3. `tickets ask --wait`: an option pick posted on the Thread ends the
#      wait with that answer (exit 0, the answer in the JSON).
#   4. `tickets ask --wait`: free text posted on the Thread ends the wait
#      with exit 5 and the person's words on stdout.
#
# The daemon's own read route (`GET /cli/thread?addr=ticket:<id>`) is the
# check that a message is in the conversation. cli-parity runs this on the
# bash CLI and the binary. Sandbox daemon under a temp HOME (worktree
# build), air-gapped. Build first: cargo build -p k2-daemon
# (CARGO_TARGET_DIR honoured).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

for v in $(env | cut -d= -f1 | grep -E '^(K2|K2SO|K5)_' || true); do unset "$v"; done
export K2_AIRGAP=1

source "$SCRIPT_DIR/_sandbox_daemon.sh"
sandbox_daemon_start
WAIT_PID=""
trap '[ -n "$WAIT_PID" ] && kill "$WAIT_PID" 2>/dev/null; _sandbox_daemon_cleanup' EXIT

base="http://127.0.0.1:${K2SO_PORT}"
pass=0
fail() { echo "FAIL: $*" >&2; echo "--- daemon log tail" >&2; tail -30 "$SANDBOX_HOME/daemon.log" >&2; exit 1; }
ok() { echo "  PASS: $*"; pass=$((pass + 1)); }

WS="$SANDBOX_HOME/work/acme-tickets"
mkdir -p "$WS"
(cd "$WS" && git init -q && git commit --allow-empty -m init -q)
reg="$(curl -s -X POST -H "Content-Type: application/json" --connect-timeout 5 --max-time 15 \
    -d "$(python3 -c 'import json,sys; print(json.dumps({"path": sys.argv[1]}))' "$WS")" \
    "${base}/cli/projects/add-from-path?token=${K2SO_TOKEN}")"
grep -qF "\"path\":\"$WS\"" <<<"$reg" || fail "register the workspace: $reg"

k2() {
    env -i HOME="$SANDBOX_HOME" PATH="$PATH" K2_AIRGAP=1 K2_PORT="$K2SO_PORT" \
        K2_PROJECT_PATH="$WS" K2_AGENT_NAME=agent-a "$K2_CLI" "$@"
}

# The daemon's read of conversation ticket:<id>, as JSON.
thread_json() {
    curl -sf -G --connect-timeout 3 --max-time 10 "${base}/cli/thread" \
        --data-urlencode "token=${K2SO_TOKEN}" --data-urlencode "addr=ticket:$1"
}

# A person's post on ticket:<id> through the Thread (owner token).
# $1 = id, $2 = text, $3 = true for an option pick.
thread_post() {
    curl -sf -X POST -H "Content-Type: application/json" --connect-timeout 3 --max-time 10 \
        -d "$(python3 -c 'import json,sys; print(json.dumps({"addr": "ticket:" + sys.argv[1], "text": sys.argv[2], "optionPick": sys.argv[3] == "true"}))' "$1" "$2" "$3")" \
        "${base}/cli/thread/post?token=${K2SO_TOKEN}"
}

# The id of the one ticket in the workspace titled $1 (waits up to 20 s).
ticket_titled() {
    local i out id=""
    for i in $(seq 1 100); do
        out="$(curl -sf -G --connect-timeout 3 --max-time 10 "${base}/cli/feedback/list" \
            --data-urlencode "token=${K2SO_TOKEN}" --data-urlencode "project=${WS}")" || out=""
        if [ -n "$out" ]; then
            id="$(python3 -c '
import json, sys
items = [i for i in json.loads(sys.argv[1])["items"] if i["title"] == sys.argv[2]]
assert len(items) <= 1, items
print(items[0]["id"] if items else "")' "$out" "$1")" || fail "the daemon list: $out"
            [ -n "$id" ] && break
        fi
        sleep 0.2
    done
    [ -n "$id" ] || fail "no ticket titled '$1' reached the daemon"
    printf '%s' "$id"
}

echo "== 1. tickets ask lands as the first item of ticket:<id> =="
set +e
out="$(k2 tickets ask "Ship it?" --body "The build is green" --options "Yes; No" --assign owner --json 2>"$SANDBOX_HOME/err1")"; rc=$?
set -e
[ "$rc" = 0 ] || fail "ask exit $rc: $out $(cat "$SANDBOX_HOME/err1")"
ID="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["id"])' "$out")" || fail "ask --json: $out"
t="$(thread_json "$ID")" || fail "the thread read of ticket:$ID"
python3 - "$t" "$ID" <<'PY' || fail "the thread after ask: $t"
import json, sys
v, tid = json.loads(sys.argv[1]), sys.argv[2]
assert v["conversation_id"] == "ticket:" + tid, v["conversation_id"]
items = v["items"]
assert len(items) == 1, items
d = items[0]["doc"]
assert (items[0]["seq"], d["ticket"], d["from"], d["body"]) == (1, tid, "agent-a", "Ship it?\nThe build is green"), items[0]
PY
ok "the ask is item 1 of ticket:<id>, from agent-a, title then body"

show="$(k2 tickets show "${ID:0:8}" --json)" || fail "show --json: $show"
python3 - "$show" "$t" <<'PY' || fail "show after ask: $show"
import json, sys
s, item = json.loads(sys.argv[1]), json.loads(sys.argv[2])["items"][0]
c = s["comments"]
assert len(c) == 1, c
assert (c[0]["id"], c[0]["author"], c[0]["body"]) == (item["id"], "agent-a", "Ship it?\nThe build is green"), c
assert s["status"] == "waiting", s
PY
ok "tickets show reads the ask back: the same message id, author and text"

echo "== 2. tickets comment lands as the next item =="
out="$(k2 tickets comment "${ID:0:8}" "Looking at the logs now" 2>&1)" || fail "comment: $out"
t="$(thread_json "$ID")" || fail "the thread read of ticket:$ID"
python3 - "$t" "$ID" <<'PY' || fail "the thread after comment: $t"
import json, sys
v, tid = json.loads(sys.argv[1]), sys.argv[2]
items = v["items"]
assert len(items) == 2, items
d = items[1]["doc"]
assert (items[1]["seq"], d["ticket"], d["from"], d["body"]) == (2, tid, "agent-a", "Looking at the logs now"), items[1]
PY
ok "the comment is item 2 of ticket:<id>, from agent-a, with its text"

show="$(k2 tickets show "${ID:0:8}" --json)" || fail "show --json: $show"
python3 - "$show" "$t" <<'PY' || fail "show after comment: $show"
import json, sys
s, item = json.loads(sys.argv[1]), json.loads(sys.argv[2])["items"][1]
c = s["comments"]
assert len(c) == 2, c
assert (c[1]["id"], c[1]["author"], c[1]["body"]) == (item["id"], "agent-a", "Looking at the logs now"), c
assert (s["status"], s["commentCount"]) == ("waiting", 2), s
PY
ok "tickets show reads the comment back; an agent's comment leaves the ticket waiting"

echo "== 3. ask --wait ends on an option pick posted on the Thread =="
k2 tickets ask "Which DB?" --options "SQLite; Postgres" --assign owner --wait --timeout 30 --json \
    >"$SANDBOX_HOME/out3" 2>"$SANDBOX_HOME/err3" &
WAIT_PID=$!
ID3="$(ticket_titled "Which DB?")"
kill -0 "$WAIT_PID" 2>/dev/null || fail "ask --wait ended before the answer: $(cat "$SANDBOX_HOME/out3" "$SANDBOX_HOME/err3")"
p="$(thread_post "$ID3" "SQLite" true)" || fail "the option pick on ticket:$ID3"
python3 - "$p" <<'PY' || fail "the option pick's answer: $p"
import json, sys
p = json.loads(sys.argv[1])
assert (p["ticket"]["status"], p["ticket"]["answered"], p["author"]["kind"]) == ("answered", True, "owner"), p
PY
set +e; wait "$WAIT_PID"; rc=$?; set -e; WAIT_PID=""
[ "$rc" = 0 ] || fail "ask --wait exit $rc: $(cat "$SANDBOX_HOME/out3" "$SANDBOX_HOME/err3")"
python3 - "$(cat "$SANDBOX_HOME/out3")" "$ID3" <<'PY' || fail "ask --wait's answer: $(cat "$SANDBOX_HOME/out3")"
import json, sys
d = json.loads(sys.argv[1])
assert d == {"ok": True, "id": sys.argv[2], "status": "answered", "answer": "SQLite", "attachments": []}, d
PY
ok "ask --wait exits 0 with the answer picked on the Thread"

echo "== 4. ask --wait ends on free text posted on the Thread =="
k2 tickets ask "Which region?" --options "East; West" --assign owner --wait --timeout 30 \
    >"$SANDBOX_HOME/out4" 2>"$SANDBOX_HOME/err4" &
WAIT_PID=$!
ID4="$(ticket_titled "Which region?")"
kill -0 "$WAIT_PID" 2>/dev/null || fail "ask --wait ended before the message: $(cat "$SANDBOX_HOME/out4" "$SANDBOX_HOME/err4")"
p="$(thread_post "$ID4" "Neither, ask the team" false)" || fail "the free text on ticket:$ID4"
set +e; wait "$WAIT_PID"; rc=$?; set -e; WAIT_PID=""
[ "$rc" = 5 ] || fail "ask --wait exit $rc, want 5: $(cat "$SANDBOX_HOME/out4" "$SANDBOX_HOME/err4")"
[ "$(cat "$SANDBOX_HOME/out4")" = "Neither, ask the team" ] || fail "stdout: $(cat "$SANDBOX_HOME/out4")"
grep -Fq '"status": "needs_discussion"' "$SANDBOX_HOME/err4" || fail "stderr: $(cat "$SANDBOX_HOME/err4")"
t="$(thread_json "$ID4")" || fail "the thread read of ticket:$ID4"
python3 - "$t" <<'PY' || fail "the thread after free text: $t"
import json, sys
items = json.loads(sys.argv[1])["items"]
assert [i["doc"]["body"] for i in items] == ["Which region?", "Neither, ask the team"], items
assert items[1]["doc"]["from"] != "agent-a", items[1]
PY
ok "ask --wait exits 5 with the person's words from the Thread"

echo ""
echo "tickets_thread_real_daemon: $pass passed, 0 failed"
