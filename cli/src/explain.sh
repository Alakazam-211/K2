# ── App refusal ids, `k2 explain`, `k2 capabilities`, `k2 app status` ──
# (prd-tauri-core-0467-v1 TC-15, TC-17, TC-18, TC-19, 0.46.7). The rows
# live in k2-core (`refusals`); contract-gen writes them into this file as
# `_k2_refusals_json` (and `_study_page_app_errors`), so `k2 explain` and
# the refusal printer work with no daemon.

# TC-19: set by `cmd_app` and `cmd_capabilities` so `_compute_fail` adds
# the registry's `next` and `explain` to an App refusal (never to another
# family's: the generic ids' fixes are written for Apps).
_K2_REFUSAL_AREA=""

# `k2 explain [<id>] [--json]`.
_explain_usage() {
    cat <<'EOF'
%include cli/text/help/explain.txt
EOF
}

cmd_explain() {
    local id="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            -h|--help) _explain_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; echo "usage: k2 explain [<id>] [--json]" >&2; exit 2 ;;
            *) [ -z "$id" ] || { echo "usage: k2 explain [<id>] [--json]" >&2; exit 2; }; id="$1"; shift ;;
        esac
    done
    # The Markdown kit's ids (MPC-Ennn, MPCE-Ennn) are the kit's registry,
    # which the daemon holds: the kit's own explain (_explain_kit), never
    # a doc path.
    case "$(printf '%s' "$id" | tr '[:lower:]' '[:upper:]')" in
        MPC-*|MPCE-*) _explain_kit "$id" "$json_out"; return $? ;;
    esac
    _k2_refusals_json | K2_EX_ID="$id" K2_EX_JSON="$json_out" python3 -c '
import json, os, sys, textwrap
reg = json.load(sys.stdin)
want = os.environ.get("K2_EX_ID", "").strip().lower()
as_json = os.environ.get("K2_EX_JSON") == "1"
rows = reg["rows"]
if not want:
    if as_json:
        print(json.dumps({"schema": reg["schema"], "areas": reg["areas"], "rows": rows}))
        sys.exit(0)
    print("Refusal ids by area. k2 explain <id> prints what happened and the fix.")
    for a in reg["areas"]:
        ids = [r["id"] + (" (retired)" if r["status"] == "retired" else "") for r in rows if r["area"] == a["area"]]
        if not ids:
            continue
        print()
        print("%s (%s):" % (a["area"], a["about"]))
        print(textwrap.fill(", ".join(ids), width=76, initial_indent="  ", subsequent_indent="  ", break_on_hyphens=False))
    sys.exit(0)
row = next((r for r in rows if r["id"] == want), None) or next((r for r in rows if want in r.get("aliases", [])), None)
if row is None:
    if as_json:
        print(json.dumps({"error": "unknown_id", "id": want, "hint": "no App refusal by that id: k2 explain lists them"}))
    else:
        print("k2 explain: no App refusal id %r (k2 explain lists them; k2 study app-errors has all of them)" % want, file=sys.stderr)
    sys.exit(4)
if as_json:
    print(json.dumps(row))
    sys.exit(0)
who = {"agent": "you (the caller)", "owner": "the server'"'"'s owner", "admin": "the owner or an Admin",
       "person": "the person signed in", "k2": "K2 (wait, retry or update)"}.get(row["who"], row["who"])
print("%s  (%s · HTTP %s · exit %s)" % (row["id"], row["area"], row["http"], row["exit"]))
if row["status"] == "retired":
    print("retired: this K2 no longer sends it; older K2 servers may.")
print(textwrap.fill(row["what"], width=76))
print(textwrap.fill("fix: " + row["fix"], width=76, subsequent_indent="     "))
print("who fixes it: " + who)
if row.get("askable"):
    print("can be asked for: yes (the error ends with Ask with: k2 permission request …; run it as printed)")
if row.get("aliases"):
    print("also sent as: " + ", ".join(row["aliases"]))
'
}

# _explain_kit ID JSON: the docs kit's `explain` for one of its ids, on
# GET /cli/kit/explain?id=<id>[&json=1]: documentation, so it answers with
# Board files on or off, and it reads no file (no path is sent). Before
# 0.46.17 a kit id went to `k2 doc explain <id>`, which took the id as a
# doc's path: a file of that name here was opened as the doc. Prints the
# kit's page (or its JSON with JSON=1) and exits with the kit's exit; a
# refusal by its row (exit the row's); a 404 or 405 (an older daemon with
# no such route) one line saying to update it, exit 1; any other error as
# k2 board prints it.
_explain_kit() {
    local id="$1" json_out="$2" query
    query="id=$(urlencode "$id")"
    [ "$json_out" = 1 ] && query="${query}&json=1"
    _compute_split "$(_compute_send GET /cli/kit/explain "$query")"
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
status, as_json = sys.argv[1], sys.argv[2] == "1"
raw = sys.stdin.read()
try:
    v = json.loads(raw) if raw.strip() else {}
except ValueError:
    v = {}
if not isinstance(v, dict):
    v = {}
if status == "200" and isinstance(v.get("stdout"), str):
    sys.stdout.write(v["stdout"])
    sys.exit(v.get("exit") or 0)
if status in ("404", "405"):
    # An older daemon has no /cli/kit/explain (K2 ruling: no fallback).
    print("This K2 server is older and can'"'"'t explain kit codes yet; update it (k2 update).", file=sys.stderr)
    sys.exit(1)
if "id" in v and "exit" in v:
    # A row of the kit (an unknown id is one): its id, name and exit.
    if as_json:
        print(raw.strip())
    else:
        print("Error [%s] %s: %s" % (v.get("code"), v.get("id"), v.get("message")), file=sys.stderr)
        if v.get("next"):
            print("Next: %s" % v["next"], file=sys.stderr)
    sys.exit(v["exit"] if isinstance(v.get("exit"), int) else 1)
err = v.get("error") if isinstance(v.get("error"), dict) else {}
code = err.get("code") or ("daemon_unreachable" if status == "000" else "error")
hint = err.get("hint") or ("no answer from the K2 daemon" if status == "000" else "the daemon refused (HTTP %s)" % status)
if as_json:
    print(raw.strip() or json.dumps({"error": {"code": code, "hint": hint}}))
else:
    print("Error [%s]: %s" % (code, hint), file=sys.stderr)
sys.exit(3 if code in ("not_enabled", "forbidden", "not_connected", "sandboxed_agent", "core_off") else 1)
' "$_CP_STATUS" "$json_out"
}

# `k2 capabilities [--json] [--node <n>]` (TC-17).
_capabilities_usage() {
    cat <<'EOF'
%include cli/text/help/capabilities.txt
EOF
}

cmd_capabilities() {
    local json_out=0 node=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            --node) [ $# -ge 2 ] || { echo "usage: k2 capabilities [--json] [--node <name>]" >&2; exit 2; }; node="$2"; shift 2 ;;
            -h|--help) _capabilities_usage; return 0 ;;
            *) echo "usage: k2 capabilities [--json] [--node <name>]" >&2; exit 2 ;;
        esac
    done
    _K2_REFUSAL_AREA=app
    local q=""
    [ -n "$node" ] && q="node=$(urlencode "$node")"
    _compute_call GET /cli/capabilities "$q"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
def row(r):
    mark = "ok" if r.get("ok") else "no"
    print("  %-2s  %-22s %s" % (mark, r.get("cap", ""), r.get("detail") or ""))
    m = r.get("missing")
    if m:
        code = (" (k2 explain %s)" % m["code"]) if m.get("code") else ""
        print("        %s: %s%s" % (m.get("reason", ""), m.get("fix", ""), code))
print("This box")
for r in v.get("box") or []:
    row(r)
comp = v.get("compute") or []
print()
print("Compute nodes your workspace may use" + ("" if comp else ": none (k2 study compute)"))
for r in comp:
    row(r)
d = v.get("doctor") or {}
if d:
    print()
    bad = ", ".join(d.get("failing") or [])
    print("Box setup (k2 app doctor): %s%s" % ("ready" if d.get("ready") else "not ready", (" — failing: " + bad) if bad else ""))
'
}

