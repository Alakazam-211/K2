#!/usr/bin/env bash
# `k2 tickets ask --html` / `show --html` / `template` — the ticket HTML
# brief CLI (prd-ticket-html-brief-v1 T7/T8, H28, H31, H32, H41, H42).
#
# Runs the REAL cli/k2 against a stub python daemon that records each
# POST /cli/feedback/create payload and serves canned `show` replies:
#   1. --html <file> sends briefHtml equal to the file bytes (small + 300 KiB:
#      the 300 KiB case proves the brief never rides an env var, which Linux
#      caps at 128 KiB per string).
#   2. --html - reads stdin.
#   3. --body-file 300 KiB goes by path too.
#   4. brief_required → exit 2, the hint on stderr as JSON.
#   5. warnings[] print as {"warning":…} on stderr, exit 0.
#   6. a reply without hasBrief → brief_not_stored warning (old daemon).
#   7. non-UTF-8 --html → brief_invalid, exit 2, nothing sent.
#   8. template prints k2-need with NO daemon (no port, no token, empty HOME).
#   9. show prints the body, the brief line, and the text extract; asks brief=1.
#  10. show --html prints exactly brief.html.
#  11. show --html on a brief-less ticket → exit 1, no_brief, empty stdout.
#  12. show on an unknown id → exit 4 (with and without --html).
#  13. assignee policy (0.43.2): --assign sends assignees; assignee_required /
#      assignee_unknown warnings print on stderr with exit 0; a Require-policy
#      assignee_* refusal exits 2; `assign` prints its warnings; template's
#      next-step line (with --assign) goes to stderr only; help, study, and
#      --schema teach assigning.
#
# No daemon build. Never touches ~/.k2.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"

[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

WORK="$(mktemp -d -t k2-fb-html-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

# The exact cleaned HTML the stub serves for ticket "brief1".
BRIEF_HTML='<h2>Problem</h2><p>DNS &amp; TLS are both down.</p><section class="k2-need"><p>Pick one.</p></section>'
printf '%s' "$BRIEF_HTML" > "$WORK/served-brief.html"

# ── Stub daemon ──────────────────────────────────────────────────────────────
python3 - "$WORK" <<'PYEOF' &
import json, os, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlsplit, parse_qs

work = sys.argv[1]
counter = {"n": 0}
served = open(os.path.join(work, "served-brief.html")).read()
text_lines = ["Problem", "DNS & TLS are both down."] + ["line %d" % i for i in range(1, 51)]

def item(fid, has_brief):
    return {"ok": True, "id": fid, "title": "Deploy blocked", "kind": "approval",
            "priority": 2, "status": "waiting", "agentName": "scout",
            "createdAt": 0, "assignees": [], "options": None, "answer": None,
            "body": "Short summary.\nSecond line.",
            "hasBrief": has_brief,
            "briefBytes": len(served.encode()) if has_brief else None,
            "comments": [{"author": "scout", "body": "Deploy blocked", "at": 0}]}

class H(BaseHTTPRequestHandler):
    def reply(self, status, obj):
        data = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        if urlsplit(self.path).path == "/cli/feedback/assign":
            req = json.loads(body)
            with open(os.path.join(work, "assign-req.json"), "wb") as f:
                f.write(body)
            warnings = []
            if "ghost" in req.get("usernames", []):
                warnings = [{"code": "assignee_unknown", "hint": "`ghost` is not a user on this server"}]
            return self.reply(200, {"ok": True, "id": "fb-assigned-0001", "assignees": req.get("usernames", []), "warnings": warnings})
        if urlsplit(self.path).path != "/cli/feedback/create":
            return self.reply(404, {"error": {"code": "not_found", "hint": "stub: unknown route"}})
        counter["n"] += 1
        with open(os.path.join(work, "req-%d.json" % counter["n"]), "wb") as f:
            f.write(body)
        req = json.loads(body)
        title = req.get("title")
        if title == "require":
            return self.reply(400, {"ok": False, "error": {"code": "brief_required",
                "hint": "brief_required: agents must attach an HTML brief. Run `k2 tickets template > brief.html`"}})
        if title == "assign-require":
            return self.reply(400, {"ok": False, "error": {"code": "assignee_required",
                "hint": "A ticket must be assigned to a user on this server (`--assign <user>`)."}})
        resp = {"ok": True, "id": "fb-stub-%08d" % counter["n"], "title": title,
                "kind": "question", "priority": 3, "status": "waiting",
                "hasBrief": "briefHtml" in req, "warnings": []}
        if title == "warn":
            resp["hasBrief"] = False
            resp["warnings"] = [{"code": "brief_missing", "hint": "agents should attach a brief"},
                                {"code": "brief_no_need", "hint": "no k2-need section"}]
        if title == "old":
            del resp["hasBrief"]
            del resp["warnings"]
        if title == "assign-warn":
            resp["assignees"] = req.get("assignees", [])
            if not resp["assignees"]:
                resp["warnings"] = [{"code": "assignee_required",
                    "hint": "A ticket must be assigned to a user on this server (`--assign <user>`). This will be required in a future update."}]
        self.reply(200, resp)

    def do_GET(self):
        u = urlsplit(self.path)
        q = parse_qs(u.query)
        with open(os.path.join(work, "show-queries.txt"), "a") as f:
            f.write(u.query + "\n")
        if u.path != "/cli/feedback/show":
            return self.reply(404, {"error": {"code": "not_found", "hint": "stub: unknown route"}})
        fid = (q.get("id") or [""])[0]
        want_brief = (q.get("brief") or [""])[0] == "1"
        if fid == "brief1":
            d = item("brief1-full-id", True)
            if want_brief:
                d["brief"] = {"html": served, "text": "\n".join(text_lines),
                              "bytes": len(served.encode()), "sha256": "x" * 64,
                              "sanitizer": "k2-brief-v1", "createdAt": 0}
            return self.reply(200, d)
        if fid == "plain1":
            d = item("plain1-full-id", False)
            if want_brief:
                d["brief"] = None
            return self.reply(200, d)
        self.reply(404, {"ok": False, "error": {"code": "not_found", "hint": "no feedback item matches '%s'" % fid}})

    def log_message(self, *a):
        pass

srv = HTTPServer(("127.0.0.1", 0), H)
with open(os.path.join(work, "stub.port"), "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PYEOF
STUB_PID=$!
disown "$STUB_PID"

for _ in $(seq 1 50); do
    [ -f "$WORK/stub.port" ] && break
    sleep 0.1
done
[ -f "$WORK/stub.port" ] || { echo "FAIL: stub daemon never wrote its port" >&2; exit 1; }
STUB_PORT="$(cat "$WORK/stub.port")"

PASS=0
FAIL=0
ok()  { echo "PASS: $1"; PASS=$((PASS + 1)); }
bad() { echo "FAIL: $1" >&2; FAIL=$((FAIL + 1)); }

# k2 <args…> with the stub as the daemon; stdout → $WORK/out, stderr → $WORK/err.
RC=0
k2() {
    set +e
    (cd "$WORK" && env -u K2_HOOK_SOCK -u K2SO_HOOK_SOCK \
        K2_PORT="$STUB_PORT" K2_HOOK_TOKEN="stub-token" K2_PROJECT_PATH="$WORK" \
        "$K2_CLI" "$@") >"$WORK/out" 2>"$WORK/err"
    RC=$?
    set -e
}

N=0
# Sets $req (no $(…): a subshell would lose the counter).
next_req() { N=$((N + 1)); req="$WORK/req-$N.json"; }

# field <req.json> <key> → writes the payload's value for key to stdout (raw).
field_to() {
    python3 -c 'import json,sys; v=json.load(open(sys.argv[1])).get(sys.argv[2]); sys.stdout.write("" if v is None else v)' "$1" "$2" > "$3"
}

# 1. --html <file>: small file, then 300 KiB.
printf '<h2>Problem</h2>\n<p>caf\xc3\xa9 \xe2\x80\x94 quotes "x" and \\ backslash</p>\n' > "$WORK/small.html"
python3 -c '
import sys
chunk = "<p>" + "x" * 1000 + "</p>\n"
open(sys.argv[1], "w").write("<h2>Big</h2>\n" + chunk * 307)
' "$WORK/big.html"
big_size=$(wc -c < "$WORK/big.html" | tr -d ' ')
[ "$big_size" -gt 300000 ] || { echo "FAIL: big.html is only $big_size bytes" >&2; exit 1; }

for f in small.html big.html; do
    k2 tickets ask "with brief" --html "$WORK/$f" --json
    next_req
    if [ "$RC" -ne 0 ]; then bad "--html $f exit $RC: $(cat "$WORK/err")"; continue; fi
    [ -f "$req" ] || { bad "--html $f: stub recorded no payload"; continue; }
    field_to "$req" briefHtml "$WORK/sent.html"
    if cmp -s "$WORK/sent.html" "$WORK/$f"; then
        ok "--html $f sends briefHtml equal to the file bytes ($(wc -c < "$WORK/$f" | tr -d ' ') B)"
    else
        bad "--html $f: briefHtml differs from the file"
    fi
    if [ -s "$WORK/err" ]; then bad "--html $f: unexpected stderr: $(cat "$WORK/err")"; else ok "--html $f: stderr empty when stored"; fi
done

# 2. --html - reads stdin.
set +e
(cd "$WORK" && env -u K2_HOOK_SOCK -u K2SO_HOOK_SOCK \
    K2_PORT="$STUB_PORT" K2_HOOK_TOKEN="stub-token" K2_PROJECT_PATH="$WORK" \
    "$K2_CLI" tickets ask "stdin brief" --html - --json) <"$WORK/big.html" >"$WORK/out" 2>"$WORK/err"
RC=$?
set -e
next_req
if [ "$RC" -eq 0 ] && [ -f "$req" ]; then
    field_to "$req" briefHtml "$WORK/sent.html"
    if cmp -s "$WORK/sent.html" "$WORK/big.html"; then ok "--html - reads stdin"; else bad "--html -: briefHtml differs from stdin"; fi
else
    bad "--html - exit $RC: $(cat "$WORK/err")"
fi

# 3. --body-file 300 KiB goes by path.
python3 -c 'import sys; open(sys.argv[1],"w").write("summary " * 40000 + "\n\n")' "$WORK/body.md"
k2 tickets ask "big body" --body-file "$WORK/body.md" --json
next_req
if [ "$RC" -eq 0 ] && [ -f "$req" ]; then
    field_to "$req" body "$WORK/sent.body"
    expected_len=$(python3 -c 'import sys; print(len(open(sys.argv[1]).read().rstrip("\n")))' "$WORK/body.md")
    got_len=$(wc -c < "$WORK/sent.body" | tr -d ' ')
    if [ "$got_len" = "$expected_len" ]; then ok "--body-file 300 KiB sent by path ($got_len B)"; else bad "--body-file: got $got_len B, want $expected_len"; fi
    if python3 -c 'import json,sys; sys.exit(0 if "briefHtml" not in json.load(open(sys.argv[1])) else 1)' "$req"; then
        ok "no --html → no briefHtml key"
    else
        bad "no --html but briefHtml was sent"
    fi
else
    bad "--body-file exit $RC: $(cat "$WORK/err")"
fi

# 4. brief_required → exit 2 + hint on stderr.
k2 tickets ask "require"
next_req
if [ "$RC" -eq 2 ]; then ok "brief_required exits 2"; else bad "brief_required exit $RC (want 2)"; fi
if python3 -c 'import json,sys; e=json.loads(open(sys.argv[1]).read().strip().splitlines()[-1])["error"]; assert e["code"]=="brief_required" and "k2 tickets template" in e["hint"], e' "$WORK/err"; then
    ok "brief_required hint is JSON on stderr"
else
    bad "brief_required stderr: $(cat "$WORK/err")"
fi
if [ -s "$WORK/out" ]; then bad "brief_required wrote stdout: $(cat "$WORK/out")"; else ok "brief_required stdout empty"; fi

# 5. Warnings print on stderr as {"warning":…}; exit stays 0.
k2 tickets ask "warn"
next_req
if [ "$RC" -eq 0 ]; then ok "warnings keep exit 0"; else bad "warn exit $RC: $(cat "$WORK/err")"; fi
if python3 - "$WORK/err" <<'PY'
import json, sys
lines = [json.loads(l) for l in open(sys.argv[1]).read().splitlines() if l.strip()]
codes = [l["warning"]["code"] for l in lines]
assert codes == ["brief_missing", "brief_no_need"], codes
assert lines[0]["warning"]["hint"] == "agents should attach a brief", lines[0]
PY
then ok "each warnings[] entry prints as {\"warning\":{code,hint}}"; else bad "warnings stderr: $(cat "$WORK/err")"; fi
if grep -q '^Filed feedback ' "$WORK/out"; then ok "warn still files (human output)"; else bad "warn stdout: $(cat "$WORK/out")"; fi

# 6. Old daemon: reply has no hasBrief → brief_not_stored.
k2 tickets ask "old" --html "$WORK/small.html"
next_req
if [ "$RC" -eq 0 ] && python3 -c 'import json,sys; w=json.loads(open(sys.argv[1]).read().strip().splitlines()[-1])["warning"]; assert w["code"]=="brief_not_stored" and "older than 0.43.2" in w["hint"], w' "$WORK/err"; then
    ok "missing hasBrief → brief_not_stored warning, exit 0"
else
    bad "brief_not_stored: rc=$RC err=$(cat "$WORK/err")"
fi

# 7. Non-UTF-8 brief → brief_invalid, exit 2, nothing sent.
printf '<p>\xff\xfe bad</p>' > "$WORK/bad.html"
before=$(ls "$WORK" | grep -c '^req-' || true)
k2 tickets ask "bad bytes" --html "$WORK/bad.html"
after=$(ls "$WORK" | grep -c '^req-' || true)
if [ "$RC" -eq 2 ] && grep -q '"brief_invalid"' "$WORK/err" && [ "$before" = "$after" ]; then
    ok "non-UTF-8 --html → brief_invalid, exit 2, no request"
else
    bad "non-UTF-8: rc=$RC sent=$((after - before)) err=$(cat "$WORK/err")"
fi

# Missing --html file → usage exit 2.
k2 tickets ask "missing" --html "$WORK/nope.html"
if [ "$RC" -eq 2 ] && grep -q '"usage"' "$WORK/err"; then ok "missing --html file → usage exit 2"; else bad "missing file: rc=$RC err=$(cat "$WORK/err")"; fi

# 8. template with NO daemon at all.
EMPTY_HOME="$WORK/empty-home"
mkdir -p "$EMPTY_HOME"
set +e
tpl="$(env -i PATH="$PATH" HOME="$EMPTY_HOME" "$K2_CLI" tickets template 2>"$WORK/err")"
tpl_rc=$?
set -e
if [ "$tpl_rc" -eq 0 ] && printf '%s' "$tpl" | grep -q 'class="k2-need"' && printf '%s' "$tpl" | grep -q 'class="k2-options"'; then
    ok "template prints k2-need + k2-options with no daemon"
else
    bad "template: rc=$tpl_rc err=$(cat "$WORK/err")"
fi
if printf '%s' "$tpl" | grep -q '<!--'; then bad "template has an HTML comment (would warn brief_sanitized)"; else ok "template has no HTML comments"; fi
for h in "Problem" "What I tried" "What I need from you" "Options" "Context"; do
    if printf '%s' "$tpl" | grep -q "<h2>$h</h2>"; then ok "template section: $h"; else bad "template missing section: $h"; fi
done
set +e
env -i PATH="$PATH" HOME="$EMPTY_HOME" "$K2_CLI" feedback template >/dev/null 2>&1
alias_rc=$?
set -e
if [ "$alias_rc" -eq 0 ]; then ok "feedback template alias works"; else bad "feedback template exit $alias_rc"; fi

# 9. show prints body + brief line + text extract (40 lines), asks brief=1.
: > "$WORK/show-queries.txt"
k2 tickets show brief1
if [ "$RC" -eq 0 ]; then ok "show exit 0"; else bad "show exit $RC: $(cat "$WORK/err")"; fi
grep -q '^body     : Short summary\.$' "$WORK/out" && ok "show prints the body" || bad "show body missing: $(cat "$WORK/out")"
grep -q '^           Second line\.$' "$WORK/out" && ok "show prints multi-line body" || bad "show body line 2 missing"
grep -q '^brief    : HTML, ' "$WORK/out" && ok "show prints the brief line" || bad "show brief line missing"
grep -q '^  DNS & TLS are both down\.$' "$WORK/out" && ok "show prints the text extract" || bad "show text extract missing"
grep -q '^  line 38$' "$WORK/out" && ok "show prints line 40 of the extract" || bad "extract line 40 missing"
if grep -q '^  line 39$' "$WORK/out"; then bad "show printed past 40 lines"; else ok "extract capped at 40 lines"; fi
grep -q '12 more lines' "$WORK/out" && ok "show says how many lines remain" || bad "remaining-lines note missing"
if grep -q '<h2>' "$WORK/out"; then bad "show printed raw HTML"; else ok "show never prints the HTML"; fi
grep -q 'brief=1' "$WORK/show-queries.txt" && ok "show asks for brief=1" || bad "show did not ask brief=1: $(cat "$WORK/show-queries.txt")"

k2 tickets show brief1 --json
if [ "$RC" -eq 0 ] && python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); assert "brief" not in d and d["hasBrief"] is True, d.keys()' "$WORK/out"; then
    ok "show --json has hasBrief but no brief HTML"
else
    bad "show --json: rc=$RC out=$(head -c 300 "$WORK/out")"
fi

# 10. show --html prints exactly brief.html.
k2 tickets show brief1 --html
if [ "$RC" -eq 0 ] && cmp -s "$WORK/out" "$WORK/served-brief.html"; then
    ok "show --html prints exactly the cleaned HTML"
else
    bad "show --html: rc=$RC out=$(head -c 300 "$WORK/out")"
fi
k2 tickets show brief1 --html --json
if [ "$RC" -eq 0 ] && python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); assert d["brief"]["html"]==open(sys.argv[2]).read()' "$WORK/out" "$WORK/served-brief.html"; then
    ok "show --html --json includes the brief"
else
    bad "show --html --json: rc=$RC"
fi

# 11. show --html on a brief-less ticket.
k2 tickets show plain1 --html
if [ "$RC" -eq 1 ] && [ ! -s "$WORK/out" ] && grep -q '"no_brief"' "$WORK/err"; then
    ok "show --html with no brief → exit 1, no_brief, empty stdout"
else
    bad "no_brief: rc=$RC out=$(cat "$WORK/out") err=$(cat "$WORK/err")"
fi
k2 tickets show plain1
if [ "$RC" -eq 0 ] && ! grep -q '^brief ' "$WORK/out"; then ok "show without a brief prints no brief line"; else bad "plain show: rc=$RC"; fi

# 12. Unknown id → exit 4.
for flag in "" "--html"; do
    # shellcheck disable=SC2086 # empty flag on purpose
    k2 tickets show nope $flag
    if [ "$RC" -eq 4 ] && grep -q '"not_found"' "$WORK/err" && [ ! -s "$WORK/out" ]; then
        ok "show nope ${flag:-(plain)} → exit 4"
    else
        bad "show nope $flag: rc=$RC err=$(cat "$WORK/err")"
    fi
done

# 13. Assignee policy (0.43.2).
k2 tickets ask "assign-warn" --html "$WORK/small.html"
next_req
if [ "$RC" -eq 0 ]; then ok "assignee_required keeps exit 0"; else bad "assign-warn exit $RC: $(cat "$WORK/err")"; fi
if python3 - "$WORK/err" <<'PY'
import json, sys
lines = [json.loads(l) for l in open(sys.argv[1]).read().splitlines() if l.strip()]
assert [l["warning"]["code"] for l in lines] == ["assignee_required"], lines
assert "required in a future update" in lines[0]["warning"]["hint"], lines[0]
PY
then ok "assignee_required prints as {\"warning\":…} on stderr"; else bad "assignee_required stderr: $(cat "$WORK/err")"; fi
grep -q '^Filed feedback ' "$WORK/out" && ok "unassigned still files" || bad "unassigned stdout: $(cat "$WORK/out")"

k2 tickets ask "assign-warn" --html "$WORK/small.html" --assign " owner, julie ,"
next_req
if [ "$RC" -eq 0 ] && [ ! -s "$WORK/err" ] && python3 -c 'import json,sys; assert json.load(open(sys.argv[1]))["assignees"]==["owner","julie"]' "$req"; then
    ok "--assign sends trimmed assignees, no warning"
else
    bad "--assign: rc=$RC err=$(cat "$WORK/err") req=$(cat "$req" 2>/dev/null)"
fi
grep -q -- '→ owner, julie' "$WORK/out" && ok "ask prints the assignees" || bad "ask assignees stdout: $(cat "$WORK/out")"

k2 tickets ask "assign-require" --html "$WORK/small.html"
next_req
if [ "$RC" -eq 2 ] && grep -q '"assignee_required"' "$WORK/err" && [ ! -s "$WORK/out" ]; then
    ok "assignee_required refusal (Require policy) exits 2"
else
    bad "assign-require: rc=$RC err=$(cat "$WORK/err")"
fi

k2 tickets assign fb-1 owner ghost
if [ "$RC" -eq 0 ] && grep -q '"assignee_unknown"' "$WORK/err" && grep -q '^Assigned fb-assig → owner, ghost\.$' "$WORK/out"; then
    ok "assign prints assignee_unknown on stderr, exit 0"
else
    bad "assign unknown: rc=$RC out=$(cat "$WORK/out") err=$(cat "$WORK/err")"
fi
k2 tickets assign fb-1 owner
if [ "$RC" -eq 0 ] && [ ! -s "$WORK/err" ]; then ok "assign to a user here: no warning"; else bad "assign owner: rc=$RC err=$(cat "$WORK/err")"; fi

set +e
env -i PATH="$PATH" HOME="$EMPTY_HOME" "$K2_CLI" tickets template >"$WORK/tpl.out" 2>"$WORK/tpl.err"
tpl_rc=$?
set -e
if [ "$tpl_rc" -eq 0 ] && grep -q -- '--assign <user>' "$WORK/tpl.err" && grep -q 'required in a future update' "$WORK/tpl.err" && ! grep -q -- '--assign' "$WORK/tpl.out"; then
    ok "template: --assign next step on stderr, brief on stdout stays clean"
else
    bad "template assign line: rc=$tpl_rc err=$(cat "$WORK/tpl.err")"
fi

k2 tickets ask --help
grep -q 'assignee_required' "$WORK/out" && grep -q 'k2 connections list --users' "$WORK/out" && ok "ask --help teaches --assign" || bad "ask --help missing assignee text"
k2 tickets template --help
grep -q -- '--assign <user>' "$WORK/out" && ok "template --help teaches --assign" || bad "template --help missing --assign"
set +e
study_tb="$(env -i PATH="$PATH" HOME="$EMPTY_HOME" "$K2_CLI" study ticket-brief 2>/dev/null)"
study_fl="$(env -i PATH="$PATH" HOME="$EMPTY_HOME" "$K2_CLI" study feedback-loop 2>/dev/null)"
set -e
if printf '%s' "$study_tb" | grep -q 'A ticket must be assigned to a user on this server (`--assign <user>`)' \
    && printf '%s' "$study_tb" | grep -q 'assignee_unknown' \
    && printf '%s' "$study_fl" | grep -q -- '--assign <user>'; then
    ok "study ticket-brief + feedback-loop teach assigning"
else
    bad "study pages missing assignee text"
fi

# Help + schema mention the flags.
k2 tickets ask --help
grep -q -- '--html <file|->' "$WORK/out" && ok "ask --help documents --html" || bad "ask --help missing --html"
k2 tickets show --help
grep -q -- '--html' "$WORK/out" && ok "show --help documents --html" || bad "show --help missing --html"
set +e
schema="$(env -i PATH="$PATH" HOME="$EMPTY_HOME" "$K2_CLI" --schema 2>/dev/null)"
set -e
if python3 - "$schema" <<'PY'
import json, sys
d = json.loads(sys.argv[1])
cmds = {c["name"]: c for c in d["commands"]}
assert any(f["name"] == "--html" for f in cmds["feedback ask"]["flags"]), "ask --html"
assert any(f["name"] == "--html" for f in cmds["feedback show"]["flags"]), "show --html"
assert "feedback template" in cmds, "template"
ask = cmds["feedback ask"]
assert "--assign <user>" in ask["description"] and "future update" in ask["description"], "ask description"
assign_flag = [f for f in ask["flags"] if f["name"] == "--assign"][0]
assert "assignee_required" in assign_flag["description"], assign_flag
PY
then ok "--schema lists ask --html, show --html, template"; else bad "--schema missing brief entries"; fi

echo
echo "feedback_ask_html: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
