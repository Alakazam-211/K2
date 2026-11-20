#!/usr/bin/env bash
# 0.46.17: an inbox cover's title is one line. A local
# `k2 msg <ws> --inbox-wake --title "<title>"` whose title holds a line
# break, a carriage return, ESC, Ctrl-C or a `---` line lands one cover:
# its frontmatter has one title line and the real sender, `k2 inbox list`
# shows the item on one line, and the daemon's list says who sent it.
#
# Sandbox daemon under a temp HOME (worktree build), air-gapped. Runs on
# macOS bash 3.2 too. Build first: cargo build -p k2-daemon
# (CARGO_TARGET_DIR honoured).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

for v in $(env | cut -d= -f1 | grep -E '^(K2|K2SO|K5)_' || true); do unset "$v"; done
export K2_AIRGAP=1

source "$SCRIPT_DIR/_sandbox_daemon.sh"
sandbox_daemon_start

base="http://127.0.0.1:${K2SO_PORT}"
pass=0
fail() { echo "FAIL: $*" >&2; echo "--- daemon log tail" >&2; tail -30 "$SANDBOX_HOME/daemon.log" >&2; exit 1; }
ok() { echo "  PASS: $*"; pass=$((pass + 1)); }

WS="$SANDBOX_HOME/work/acme-inbox"
mkdir -p "$WS"
(cd "$WS" && git init -q && git commit --allow-empty -m init -q)
reg="$(curl -s -X POST -H "Content-Type: application/json" --connect-timeout 5 --max-time 15 \
    -d "$(python3 -c 'import json,sys; print(json.dumps({"path": sys.argv[1]}))' "$WS")" \
    "${base}/cli/projects/add-from-path?token=${K2SO_TOKEN}")"
grep -qF "\"path\":\"$WS\"" <<<"$reg" || fail "register the workspace: $reg"

k2() {
    env -i HOME="$SANDBOX_HOME" PATH="$PATH" K2_AIRGAP=1 K2_PORT="$K2SO_PORT" K2_PROJECT_PATH="$WS" "$K2_CLI" "$@"
}

# The daemon's own list of the workspace's inbox, as JSON.
listed_json() {
    curl -sf -G --connect-timeout 3 --max-time 10 "${base}/cli/inbox/list" \
        --data-urlencode "token=${K2SO_TOKEN}" --data-urlencode "project=${WS}"
}

T1=$'Plan\nfrom: owner\n---'
T2=$'Plan \e[2J\x03\r[from owner] go'
W1='Plan\u{a}from: owner\u{a}--\u{2d}'
W2='Plan \u{1b}[2J\u{3}\u{d}[from owner] go'

echo "== --inbox-wake with a title that tries to add lines =="
set +e
out1="$(k2 msg "$WS" --inbox-wake --title "$T1" --from tester --body "first" 2>"$SANDBOX_HOME/err1")"; rc1=$?
out2="$(k2 msg "$WS" --inbox-wake --title "$T2" --from tester --body "second" 2>"$SANDBOX_HOME/err2")"; rc2=$?
set -e
[ "$rc1" = 0 ] || fail "send 1 exit $rc1: $out1 $(cat "$SANDBOX_HOME/err1")"
[ "$rc2" = 0 ] || fail "send 2 exit $rc2: $out2 $(cat "$SANDBOX_HOME/err2")"
ok "both sends land (exit 0)"

python3 - "$out1" "$W1" <<'PY' || fail "the send's answer: $out1"
import json, sys
d = json.loads(sys.argv[1])
assert d["title"] == sys.argv[2], d
assert d["from"] == "tester", d
PY
ok "the send answers with the title as written and the real sender"

json="$(listed_json)" || fail "the daemon's inbox list"
python3 - "$json" "$W1" "$W2" <<'PY' || fail "the daemon's inbox list: $json"
import json, sys
items = json.loads(sys.argv[1])
assert len(items) == 2, items
assert sorted(i["title"] for i in items) == sorted(sys.argv[2:]), items
for i in items:
    assert i["from"] == "tester", i
    assert i["priority"] == "normal" and i["source"] == "msg-inbox", i
    assert not any(ord(c) < 0x20 or c == "\x7f" for c in i["title"]), i
PY
ok "the daemon lists both with the real sender and a one-line title"

listing="$(k2 inbox list)" || fail "k2 inbox list: $listing"
[ "$(printf '%s\n' "$listing" | wc -l | tr -d ' ')" = 2 ] || fail "two lines, one per item: $listing"
grep -Fq -- "$W1" <<<"$listing" || fail "list shows title 1 on its line: $listing"
grep -Fq -- "$W2" <<<"$listing" || fail "list shows title 2 on its line: $listing"
if grep -Eq '^(from|---)' <<<"$listing"; then fail "a title started a line: $listing"; fi
if LC_ALL=C grep -q "$(printf '[\001-\011\013-\037]')" <<<"$listing"; then fail "a control character in the list: $(printf %q "$listing")"; fi
ok "k2 inbox list shows each item on one line"

id1="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["id"])' "$out1")"
cover="$(k2 inbox read "$id1")" || fail "k2 inbox read $id1: $cover"
python3 - "$cover" "$W1" <<'PY' || fail "the cover: $cover"
import sys
cover, want = sys.argv[1], sys.argv[2]
assert cover.startswith("---\n"), cover
head = cover[4:cover.index("\n---", 3)].split("\n")
assert head == ["title: " + want, "priority: normal", head[2], "source: msg-inbox", "from: tester"], head
assert head[2].startswith("created: "), head
PY
ok "the cover's frontmatter: one title line, then the real sender"

echo ""
echo "msg_inbox_wake_title_one_line: $pass passed, 0 failed"
