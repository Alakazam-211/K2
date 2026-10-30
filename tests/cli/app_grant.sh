#!/usr/bin/env bash
# k2 app grant list|add|set|rm|hooks — owner CLI for skin.db grants
# (incl. Sew S27 chat grants and the grant-hook outbox).
# Stub daemon only. Never touch real ~/.k2.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"

[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }

pass=0
fail=0
assert_contains() {
    local label="$1" hay="$2" needle="$3"
    case "$hay" in
        *"$needle"*)
            echo "  PASS: $label"
            pass=$((pass + 1))
            ;;
        *)
            echo "  FAIL: $label (missing $(printf %q "$needle"))" >&2
            fail=$((fail + 1))
            ;;
    esac
}
assert_not_contains() {
    local label="$1" hay="$2" needle="$3"
    case "$hay" in
        *"$needle"*)
            echo "  FAIL: $label (found $(printf %q "$needle"))" >&2
            fail=$((fail + 1))
            ;;
        *)
            echo "  PASS: $label"
            pass=$((pass + 1))
            ;;
    esac
}
assert_eq() {
    local label="$1" got="$2" want="$3"
    if [ "$got" = "$want" ]; then
        echo "  PASS: $label"
        pass=$((pass + 1))
    else
        echo "  FAIL: $label (got=$(printf %q "$got") want=$(printf %q "$want"))" >&2
        fail=$((fail + 1))
    fi
}

SANDBOX="$(mktemp -d -t k2-app-grant-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "${STUB_PID:-}" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

export HOME="$SANDBOX"
mkdir -p "$HOME/.k2"
echo "1" >"$HOME/.k2/heartbeat.port"
echo "fake-token" >"$HOME/.k2/heartbeat.token"
chmod 600 "$HOME/.k2/heartbeat.token"
export K2_HOST=127.0.0.1
export K2_PORT=1
export K2_HOOK_TOKEN="fake-token"
unset K2_HOOK_SOCK K2SO_HOOK_SOCK K2SO_PORT K2SO_HOOK_TOKEN || true

run_k2() {
    env -u K2_HOOK_SOCK -u K2SO_HOOK_SOCK \
        HOME="$HOME" \
        K2_HOST="${K2_HOST}" \
        K2_PORT="${K2_PORT}" \
        K2_HOOK_TOKEN="${K2_HOOK_TOKEN}" \
        "$K2_CLI" "$@"
}

echo "== bash -n =="
if bash -n "$K2_CLI"; then
    echo "  PASS: bash -n cli/k2"
    pass=$((pass + 1))
else
    echo "  FAIL: bash -n cli/k2" >&2
    fail=$((fail + 1))
fi

echo "== k2 app grant --help is the app family, not k2 people =="
set +e
help_out="$(run_k2 app grant --help 2>&1)"
help_rc=$?
set -e
assert_eq "k2 app grant --help exit 0" "$help_rc" "0"
assert_contains "grant help list" "$help_out" "k2 app grant list"
assert_contains "grant help add" "$help_out" "k2 app grant add"
assert_contains "grant help rm" "$help_out" "k2 app grant rm"
assert_contains "grant help subject" "$help_out" "--subject"
assert_contains "grant help kind" "$help_out" "--kind"
assert_contains "grant help target" "$help_out" "--target"
assert_contains "grant help role" "$help_out" "--role"
assert_contains "grant help scope" "$help_out" "--scope"
assert_not_contains "grant help is not people" "$help_out" "k2 people"
assert_not_contains "grant help is not /cli/people" "$help_out" "/cli/people"
schema="$(cat "$K2_CLI")"
assert_contains "schema grant add" "$schema" '"name": "app grant add"'
assert_contains "grant route" "$schema" "/cli/skin/grants"

echo "== missing flags fail before the daemon =="
set +e
missing="$(run_k2 app grant add --kind app 2>&1)"
missing_rc=$?
set -e
assert_eq "add missing flags exit 1" "$missing_rc" "1"
assert_contains "add usage" "$missing" "k2 app grant add"

echo "== list add rm against a stub daemon =="
STUB_PY="$SANDBOX/stub.py"
cat >"$STUB_PY" <<'PY'
import json
from http.server import BaseHTTPRequestHandler, HTTPServer
import os

seen = os.environ["SEEN"]

class H(BaseHTTPRequestHandler):
    def _send(self, code, body):
        raw = body.encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def _read(self):
        n = int(self.headers.get("Content-Length") or "0")
        raw = self.rfile.read(n)
        return json.loads(raw.decode() or "{}")

    def do_GET(self):
        path = self.path.split("?", 1)[0]
        if path == "/cli/skin/grants":
            self._send(200, json.dumps({"grants": [{
                "id": "g1",
                "subjectKind": "principal",
                "subjectId": "p-ada",
                "kind": "app",
                "targetId": "booking",
                "roleId": "r-guest",
                "scope": None,
            }, {
                "id": "g-chat",
                "subjectKind": "principal",
                "subjectId": "p-ann",
                "kind": "chat",
                "targetId": "sew",
                "roleId": None,
                "scope": "member",
                "hookState": "blocked",
                "hookError": "validation_failed: bad",
            }]}))
            return
        if path == "/cli/skin/grants/own":
            # An agent's list (app.login-grant): the logins on its own Apps.
            self._send(200, json.dumps({"grants": [{
                "id": "g9", "username": "ada", "app": "booking", "role": "guest", "enabled": True,
            }], "scope": "your workspace's Apps"}))
            return
        ZERO = {"provisioned": 0, "updated": 0, "deprovisioned": 0, "skippedBusy": 0, "skippedHeld": 0,
                "ignoredLocal": 0, "ignoredOther": 0, "orphansRemoved": 0}
        if path == "/cli/skin/grant-hooks":
            self._send(200, json.dumps({
                "targets": [{"kind": "chat", "enabled": True, "healthy": False,
                             "lastError": "mgmt token rejected", "socket": "/run/sew/mgmt.sock",
                             "lastReconcileAt": 1760000600, "lastReconcile": dict(ZERO),
                             "lastChangeAt": 1760000000, "lastChange": dict(ZERO, provisioned=2, orphansRemoved=1),
                             "reconcileHistory": [dict(ZERO, provisioned=2, orphansRemoved=1, at=1760000000),
                                                  dict(ZERO, skippedBusy=3, at=1760000300),
                                                  dict(ZERO, at=1760000600)]}],
                "outbox": {"pending": 2, "blocked": 1, "held": 1, "oldestPendingAgeSecs": 40},
                "blocked": [{"id": "ob-1", "op": "provision", "subjectKey": "k2:principal:x", "lastError": "validation_failed: bad"}],
                "held": [{"id": "ob-2", "op": "provision", "subjectKey": "k2:principal:y", "lastError": "skipped by Sew"}],
            }))
            return
        if path == "/cli/skin/grant-hooks/log":
            with open(seen, "a") as f:
                f.write("GET " + self.path + "\n")
            self._send(200, json.dumps({"rows": [{"id": "ob-1", "state": "done", "op": "provision",
                "subjectKey": "k2:owner", "actor": "owner-token", "cause": "target_enable", "attempts": 1}]}))
            return
        self._send(404, json.dumps({"error": "not found"}))

    def do_POST(self):
        path = self.path.split("?", 1)[0]
        body = self._read()
        with open(seen, "a") as f:
            f.write(path + "\n" + json.dumps(body) + "\n")
        if path == "/cli/skin/grants":
            self._send(200, json.dumps({
                "id": "g-new",
                "subjectKind": body.get("subjectKind"),
                "subjectId": body.get("subjectId"),
                "kind": body.get("kind"),
                "targetId": body.get("targetId"),
                "roleId": body.get("roleId"),
                "scope": body.get("scope"),
            }))
            return
        if path == "/cli/skin/grants/delete":
            cascaded = []
            if body.get("id") == "g-chat":
                cascaded = [{"id": "g-asst", "kind": "chat", "subjectKind": "workspace",
                             "subjectId": "w-1", "scope": "assistant:p-ann"}]
            self._send(200, json.dumps({"success": True, "cascaded": cascaded}))
            return
        if path == "/cli/app/test-guest/add" and body.get("username") == "zz-held":
            # 0.46.5 item 15: a held test guest; the CLI sent a password.
            out = {"status": "held", "request": {"id": "w_held1", "summary": "make the test guest zz-held of rocore"},
                   "hint": "held for the owner",
                   "afterApproval": "Held for the owner. After approval, set its password once: k2 app user password zz-held --test --password-stdin."}
            if body.get("password"):
                out["passwordNotUsed"] = True
                out["passwordNote"] = "the password you sent was not used or kept (a held request never stores one): set it once after the owner approves"
            self._send(202, json.dumps(out))
            return
        if path == "/cli/app/test-guest/add":
            self._send(200, json.dumps({"username": body.get("username"), "app": body.get("app"),
                                        "role": body.get("role") or None, "rooms": [], "expiresAt": 2000000000,
                                        "password": "pw-once", "note": ""}))
            return
        if path == "/cli/skin/grants/role":
            same = body.get("role") == "desk"
            self._send(200, json.dumps({"id": body.get("id"), "kind": "app", "subjectKind": "principal",
                                        "subjectId": "p-ada", "targetId": "svc-1", "roleId": "r-" + body.get("role", ""),
                                        "changed": not same}))
            return
        if path == "/cli/skin/grants/update":
            self._send(200, json.dumps({"id": body.get("id"), "kind": "chat", "scope": body.get("scope")}))
            return
        if path in ("/cli/skin/grant-hooks/retry", "/cli/skin/grant-hooks/skip"):
            self._send(200, json.dumps({"id": body.get("id"), "state": "pending" if path.endswith("retry") else "skipped"}))
            return
        if path == "/cli/skin/grant-hooks/reconcile":
            self._send(200, json.dumps({"started": True}))
            return
        if path == "/cli/skin/grant-hooks/target":
            self._send(200, json.dumps({"kind": body.get("kind"), "enabled": body.get("enabled"),
                                        "socket": body.get("socket") or "/run/sew/mgmt.sock"}))
            return
        if path in ("/cli/skin/grants/copy-host", "/cli/app/grant/copy-host"):
            self._send(200, json.dumps({
                "ok": True,
                "app": "svc-1",
                "dryRun": bool(body.get("dryRun")),
                "added": ["ada", "bob"],
                "keptExisting": [{"username": "dora", "enabled": False}],
                "skippedOtherAppRole": ["olga"],
            }))
            return
        if path == "/cli/skin/users":
            self._send(200, json.dumps({"username": body.get("username"), "id": "p-new"}))
            return
        self._send(404, json.dumps({"error": "not found"}))

    def log_message(self, *_args):
        pass

HTTPServer(("127.0.0.1", int(os.environ["PORT"])), H).serve_forever()
PY
STUB_PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
export K2_PORT="$STUB_PORT"
echo "$STUB_PORT" >"$HOME/.k2/heartbeat.port"
SEEN="$SANDBOX/seen.txt"
: >"$SEEN"
PORT="$STUB_PORT" SEEN="$SEEN" python3 "$STUB_PY" &
STUB_PID=$!
for _i in 1 2 3 4 5 6 7 8 9 10; do
    if curl -sf --connect-timeout 1 "http://127.0.0.1:${STUB_PORT}/cli/skin/grants" >/dev/null 2>&1; then
        break
    fi
    sleep 0.1
done

set +e
# The owner's list: a terminal outside K2 (no passport; the owner token on
# disk). With a passport the CLI asks for the agent's own Apps' logins.
list_out="$(env -u K2_HOOK_TOKEN -u K2_HOOK_SOCK -u K2SO_HOOK_SOCK HOME="$HOME" K2_HOST="${K2_HOST}" K2_PORT="${K2_PORT}" "$K2_CLI" app grant list 2>/dev/null)"
list_rc=$?
own_out="$(run_k2 app grant list 2>/dev/null)"
own_rc=$?
# A K2 shell tab carries a passport and the cell marker (K2_HOOK_SOCK). `app`
# is an id tool, so the CLI keeps the passport there (never the disk owner
# token); /cli/skin/grants is owner-only and refuses every passport (it did
# before app.login-grant too: owner_only), so a shell tab, like the agent,
# lists its own workspace's Apps' logins. The socket path doesn't exist, so
# the call goes over TCP to the stub.
shell_out="$(env HOME="$HOME" K2_HOST="${K2_HOST}" K2_PORT="${K2_PORT}" K2_HOOK_TOKEN="${K2_HOOK_TOKEN}" \
    K2_HOOK_SOCK="$SANDBOX/no-cell.sock" "$K2_CLI" app grant list 2>/dev/null)"
shell_rc=$?
add_out="$(run_k2 app grant add --subject-kind principal --subject p-ada --kind app --target booking --role guest 2>/dev/null)"
add_rc=$?
rm_out="$(run_k2 app grant rm --subject-kind workspace --subject ws-1 --kind mailbox --target inbox 2>/dev/null)"
rm_rc=$?
rooms_out="$(run_k2 app grant add --subject-kind principal --subject p-bob --kind app --target docs/portal --room sales --room docs 2>/dev/null)"
rooms_rc=$?
copy_out="$(run_k2 app grant copy-host docs/portal --dry-run 2>/dev/null)"
copy_rc=$?
add_app_out="$(run_k2 app user add eve --app docs/portal --role desk 2>/dev/null)"
add_app_rc=$?
add_role_only="$(run_k2 app user add eve --role desk 2>&1)"
add_role_only_rc=$?
chat_add_out="$(run_k2 app grant add --kind chat --subject-kind principal --subject ann 2>/dev/null)"
chat_add_rc=$?
chat_ws_out="$(run_k2 app grant add --kind chat --subject-kind workspace --subject sales --scope assistant:ann 2>/dev/null)"
chat_ws_rc=$?
chat_set_out="$(run_k2 app grant set --id g-chat --scope guest 2>/dev/null)"
chat_set_rc=$?
chat_rm_out="$(run_k2 app grant rm --id g-chat 2>/dev/null)"
chat_rm_rc=$?
chat_rm_key_out="$(run_k2 app grant rm --kind chat --subject-kind principal --subject ann 2>/dev/null)"
chat_rm_key_rc=$?
hooks_out="$(run_k2 app grant hooks 2>/dev/null)"
hooks_rc=$?
hooks_json_out="$(run_k2 app grant hooks --json 2>/dev/null)"
hooks_json_rc=$?
hooks_status_json_out="$(run_k2 app grant hooks status --json 2>/dev/null)"
hooks_status_json_rc=$?
hooks_log_out="$(run_k2 app grant hooks log --subject k2:owner --limit 5 2>/dev/null)"
hooks_log_rc=$?
hooks_retry_out="$(run_k2 app grant hooks retry ob-1 2>/dev/null)"
hooks_retry_rc=$?
hooks_skip_out="$(run_k2 app grant hooks skip ob-1 2>/dev/null)"
hooks_skip_rc=$?
hooks_rec_out="$(run_k2 app grant hooks reconcile 2>/dev/null)"
hooks_rec_rc=$?
hooks_tgt_out="$(run_k2 app grant hooks target chat --on --socket /run/sew/mgmt.sock 2>/dev/null)"
hooks_tgt_rc=$?
set_missing="$(run_k2 app grant set --id g-chat 2>&1)"
set_missing_rc=$?
role_out="$(run_k2 app grant role g-app --role reader 2>/dev/null)"
role_rc=$?
role_same_out="$(run_k2 app grant role --id g-app --role desk 2>/dev/null)"
role_same_rc=$?
role_missing="$(run_k2 app grant role g-app 2>&1)"
role_missing_rc=$?
tg_role_out="$(run_k2 app user add rocore zz-rtest --test --role rtest-files 2>&1)"
tg_role_rc=$?
tg_held_out="$(printf 'piped-pw-1\n' | run_k2 app user add rocore zz-held --test --room r1 --password-stdin 2>/dev/null)"
tg_held_rc=$?
tg_held_err="$(printf 'piped-pw-1\n' | run_k2 app user add rocore zz-held --test --room r1 --password-stdin 2>&1 >/dev/null)"
set -e
assert_eq "grant list exit 0" "$list_rc" "0"
assert_contains "list prints grant" "$list_out" "g1"
assert_contains "list prints subject" "$list_out" "principal p-ada"
assert_contains "list prints kind" "$list_out" "app booking"
assert_eq "agent grant list exit 0" "$own_rc" "0"
assert_contains "agent list is its own Apps' logins" "$own_out" "g9  ada on booking  role guest"
assert_eq "shell tab grant list exit 0" "$shell_rc" "0"
assert_contains "shell tab list is its workspace's Apps' logins" "$shell_out" "g9  ada on booking  role guest"
assert_eq "grant add exit 0" "$add_rc" "0"
assert_contains "add prints id" "$add_out" "g-new"
assert_eq "grant rm exit 0" "$rm_rc" "0"
assert_contains "rm removed" "$rm_out" "Grant removed."
assert_eq "grant add --room exit 0" "$rooms_rc" "0"
assert_contains "add --room prints id" "$rooms_out" "g-new"
assert_eq "copy-host exit 0" "$copy_rc" "0"
assert_contains "copy-host dry run verb" "$copy_out" "Would copy the box login onto App svc-1 for 2 user(s): ada, bob"
assert_contains "copy-host kept" "$copy_out" "kept dora"
assert_contains "copy-host skipped" "$copy_out" "skipped olga"
assert_eq "user add --app exit 0" "$add_app_rc" "0"
assert_contains "user add --app says no box login" "$add_app_out" "no box login"
assert_eq "user add --role without --app exit 2" "$add_role_only_rc" "2"
assert_contains "user add --role teaches --app" "$add_role_only" "needs --app"

seen="$(cat "$SEEN")"
assert_contains "add posts grants" "$seen" "/cli/skin/grants"
assert_contains "add body subject" "$seen" '"subjectKind": "principal"'
assert_contains "add body subject id" "$seen" '"subjectId": "p-ada"'
assert_contains "add body kind" "$seen" '"kind": "app"'
assert_contains "add body target" "$seen" '"targetId": "booking"'
assert_contains "add body role" "$seen" '"roleId": "guest"'
assert_contains "rm posts delete" "$seen" "/cli/skin/grants/delete"
assert_contains "rm body workspace" "$seen" '"subjectKind": "workspace"'
assert_contains "rm body kind" "$seen" '"kind": "mailbox"'
assert_contains "rm body target" "$seen" '"targetId": "inbox"'
assert_contains "add --room body" "$seen" '"rooms": ["sales", "docs"]'
assert_contains "add --room target" "$seen" '"targetId": "docs/portal"'
assert_contains "copy-host posts (the App route since 0.46.4)" "$seen" "/cli/app/grant/copy-host"
assert_contains "copy-host body" "$seen" '{"app": "docs/portal", "dryRun": true}'
assert_contains "user add --app body" "$seen" '{"username": "eve", "app": "docs/portal", "role": "desk"}'

echo "== Sew S27 chat grants + hooks =="
assert_contains "list shows chat hook state" "$list_out" "[blocked: validation_failed: bad]"
assert_eq "chat add exit 0" "$chat_add_rc" "0"
assert_contains "chat add says queued" "$chat_add_out" "Queued for chat."
assert_eq "chat workspace add exit 0" "$chat_ws_rc" "0"
assert_eq "chat set exit 0" "$chat_set_rc" "0"
assert_contains "chat set prints scope" "$chat_set_out" "Grant g-chat scope guest"
assert_eq "chat rm exit 0" "$chat_rm_rc" "0"
assert_contains "chat rm prints cascade" "$chat_rm_out" "also removed chat grant g-asst (workspace w-1, scope assistant:p-ann)"
assert_eq "chat rm by key without --target exit 0" "$chat_rm_key_rc" "0"
assert_eq "hooks status exit 0" "$hooks_rc" "0"
assert_contains "hooks status target" "$hooks_out" "chat  on  unhealthy: mgmt token rejected"
assert_contains "hooks status counts" "$hooks_out" "outbox: 2 pending, 1 blocked, 1 held  (oldest pending 40s)"
assert_contains "hooks status blocked row" "$hooks_out" "BLOCKED ob-1"
assert_contains "hooks status held row" "$hooks_out" "HELD    ob-2"
assert_contains "hooks status attribution note" "$hooks_out" "not proof a person acted"
assert_contains "hooks status latest pass, plain" "$hooks_out" "last reconcile: 2025-10-09 09:03:20Z  no changes"
assert_contains "hooks status last non-empty pass" "$hooks_out" "last change   : 2025-10-09 08:53:20Z  2 provisioned, 1 orphans removed"
assert_contains "hooks status per-pass history" "$hooks_out" "    2025-10-09 08:58:20Z  3 skipped busy"
assert_not_contains "hooks status no raw JSON summary" "$hooks_out" '"provisioned": 0'
assert_contains "hooks --json keeps the history" "$hooks_json_out" '"reconcileHistory"'
assert_eq "bare hooks --json exit 0 (defaults to status)" "$hooks_json_rc" "0"
assert_eq "bare hooks --json = hooks status --json" "$hooks_json_out" "$hooks_status_json_out"
assert_eq "hooks status --json exit 0" "$hooks_status_json_rc" "0"
assert_contains "hooks --json is the raw body" "$hooks_json_out" '"targets"'
assert_eq "hooks log exit 0" "$hooks_log_rc" "0"
assert_contains "hooks log row" "$hooks_log_out" "ob-1  done"
assert_eq "hooks retry exit 0" "$hooks_retry_rc" "0"
assert_contains "hooks retry" "$hooks_retry_out" "Outbox row ob-1 is now pending."
assert_eq "hooks skip exit 0" "$hooks_skip_rc" "0"
assert_contains "hooks skip" "$hooks_skip_out" "Outbox row ob-1 is now skipped."
assert_eq "hooks reconcile exit 0" "$hooks_rec_rc" "0"
assert_contains "hooks reconcile" "$hooks_rec_out" "Reconcile started."
assert_eq "hooks target exit 0" "$hooks_tgt_rc" "0"
assert_contains "hooks target" "$hooks_tgt_out" "Target chat on (socket /run/sew/mgmt.sock)"
assert_eq "set without --scope exit 1" "$set_missing_rc" "1"
assert_eq "grant role exit 0" "$role_rc" "0"
assert_contains "grant role keeps logins" "$role_out" "Grant g-app  now role reader (p-ada on svc-1). Live logins kept"
assert_eq "grant role same exit 0" "$role_same_rc" "0"
assert_contains "grant role same role" "$role_same_out" "Grant g-app already has role desk. Nothing changed."
assert_eq "grant role without --role exit 1" "$role_missing_rc" "1"
assert_eq "test guest add --role with a positional app: exit 0 (0.46.5 item 13)" "$tg_role_rc" "0"
assert_not_contains "  no Connect --role refusal" "$tg_role_out" "Connect --role is not a skin command"
assert_contains "  says it" "$tg_role_out" "Test guest zz-rtest of App rocore (role rtest-files"
assert_eq "held test guest: exit 2 (0.46.5 item 15)" "$tg_held_rc" "2"
assert_contains "  says the one password set after approval" "$tg_held_out" "Held for the owner. After approval, set its password once: k2 app user password zz-held --test --password-stdin."
assert_contains "  the piped password is said to be unused" "$tg_held_err" "note: the password you sent was not used or kept"
assert_contains "grant role usage" "$role_missing" "Usage: k2 app grant role <grant-id> --role <role>"
seen="$(cat "$SEEN")"
assert_contains "chat add body kind" "$seen" '"kind": "chat"'
assert_contains "chat add body default target" "$seen" '"subjectId": "ann", "kind": "chat", "targetId": "sew"'
assert_contains "chat ws body scope" "$seen" '"scope": "assistant:ann"'
assert_contains "set posts update" "$seen" '/cli/skin/grants/update'
assert_contains "set body" "$seen" '{"id": "g-chat", "scope": "guest"}'
assert_contains "role posts /cli/skin/grants/role" "$seen" '/cli/skin/grants/role'
assert_contains "test guest add posts its role" "$seen" '"role": "rtest-files"'
assert_contains "role body (positional id)" "$seen" '{"id": "g-app", "role": "reader"}'
assert_contains "role body (--id)" "$seen" '{"id": "g-app", "role": "desk"}'
assert_contains "schema grant role" "$schema" '"name": "app grant role"'
assert_contains "help names grant role" "$help_out" "k2 app grant role <grant-id> --role <role>"
assert_contains "rm by key fills sew" "$seen" '"subjectId": "ann", "kind": "chat", "targetId": "sew"'
assert_contains "retry posts" "$seen" '/cli/skin/grant-hooks/retry'
assert_contains "skip posts" "$seen" '/cli/skin/grant-hooks/skip'
assert_contains "reconcile posts" "$seen" '/cli/skin/grant-hooks/reconcile'
assert_contains "target body" "$seen" '{"kind": "chat", "enabled": true, "socket": "/run/sew/mgmt.sock"}'
assert_contains "log query subject (curl lowercases %3a)" "$seen" "subject=k2%3aowner"
assert_contains "log query limit" "$seen" 'limit=5'
assert_contains "schema grant set" "$schema" '"name": "app grant set"'
assert_contains "schema grant hooks" "$schema" '"name": "app grant hooks"'
assert_contains "help names chat" "$help_out" "--kind chat"
assert_contains "help says Sew owns roles" "$help_out" "roles and channels are set in Sew"

echo
echo "$pass passed, $fail failed"
[ "$fail" -eq 0 ]
