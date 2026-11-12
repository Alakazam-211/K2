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
k2 explain [<id>] [--json]

What an App refusal id means and how to fix it. Every App route refuses
with a stable id: the "error" field of the JSON line a refused k2 app
command prints (with a "fix:" line after it). Look the id up here.

  k2 explain                 every id, by area
  k2 explain <id>            what happened, the fix, who fixes it
  k2 explain <id> --json     the same, one JSON object:
                             {id, area, http, exit, what, fix, who,
                              askable, status, aliases}

Ids are lowercase with underscores (apps_manage_off). In a row: "exit" is
the code the refused command exits with; "who" is who can fix it (you,
the owner, an Admin, the person signed in, or K2 itself); "askable" means
k2 permission request can ask for it (the refusal ends with an Ask with:
line to run). "retired" ids are ones only older K2 servers send; they
still explain. Works with the daemon down. All of them on one page:
k2 study app-errors.
This command's own exit codes: 0 found, 4 unknown id, 2 bad usage.
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
    # The Markdown kit's ids (MPC-Ennn, MPCE-Ennn) are the kit's registry:
    # `k2 doc explain`, once Markdown tier 0 is on this K2 (kit-HOST-ERRORS).
    case "$(printf '%s' "$id" | tr '[:lower:]' '[:upper:]')" in
        MPC-*|MPCE-*)
            if declare -F cmd_doc >/dev/null 2>&1; then
                if [ "$json_out" = 1 ]; then cmd_doc explain "$id" --json; else cmd_doc explain "$id"; fi
                return $?
            fi
            echo "k2 explain: $id is a Markdown kit id; this K2 has no k2 doc yet (it arrives with Markdown files)" >&2
            exit 4
            ;;
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

# `k2 capabilities [--json] [--node <n>]` (TC-17).
_capabilities_usage() {
    cat <<'EOF'
k2 capabilities [--json] [--node <name>]

What this K2 server can do for Apps, before you try: one row per
capability, "ok" or "no". A "no" row names the reason, the fix and the
refusal id you would hit (k2 explain <id>).

  this box   Apps on, engines (walled, open, static), protocols, public
             ports, custom domains, phone simulators, Postgres versions,
             mail, push keys per App (set or not, never the keys), the
             update feed, limits (Seed, archive, upload part, disk guard)
  compute    each node your workspace may use: OS, CPU type, free slots,
             Xcode, Android SDK, tools

  --node <name>  only that compute node's row (k2 compute nodes)
  --json         {schema: 1, box: [...], compute: [...], doctor: {...}};
                 a row is {cap, ok, detail, missing?: {reason, fix, code}}

Rows to look for: protocols.other (HTTP/2 to the App, gRPC, UDP),
ports.public, engine.static, updates, push.<app> (one per App you see:
its APNs and FCM keys set or not), node.<name>.
Reasons: not_built (K2 doesn't do it yet), owner_disabled (the owner
turned it off), level (your access doesn't reach it: ask the owner),
not_prepared (the box isn't set up for it yet), hardware (this machine
can't).
k2 app doctor stays the box's setup check; its result is in "doctor".
Exit codes: 0, 3 refused, 2 bad usage.
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

