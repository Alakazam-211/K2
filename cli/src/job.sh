# ── k2 job (prd-vault-gaps-v1 item 5) ─────────────────────────────────
_job_usage() {
    cat <<'EOF'
k2 job — scheduled jobs the K2 daemon runs (no session, no LLM, no cron)

  k2 job add NAME (--every 15m | --cron '*/15 * * * *') [--timeout 10m]
             [--cwd <dir>] -- <cmd> [args…]
  k2 job edit NAME [--every …|--cron …] [--timeout …] [--cwd …] [-- <cmd> …]
  k2 job list [--all] [--json]          show NAME [--json]
  k2 job logs NAME [--run ID] [--json]  run NAME (same: run-now)
  k2 job enable|disable NAME            (same: resume|pause NAME)
  k2 job kill NAME                      stop the run in progress
  k2 job rm NAME                        remove it and its secret grants
  k2 job pause --all | resume --all     every job (owner or admin only)
  --workspace <name>                    people: which workspace (default:
                                        the one this folder is in)

Each run gets its own K2 identity (k2 vault fetch, k2 db dsn and k2 msg
work as in a K2 tab), only the secrets granted to job:NAME
(k2 vault grant KEY --to job:NAME --as env|file|agent|fetch), the workspace
folder as its working folder, and a PATH with k2 on it. No shell unless
you write one: -- sh -c '…'. Runs never overlap. The timeout (default
10m, max 6h) stops the whole process group. Logs are redacted
([k2v:NAME]) and capped at 1 MiB; the last 20 are kept.
Agents may add jobs in their own workspace; each add, edit and removal
posts a line on the workspace's Thread.
More: k2 study jobs
EOF
}

_job_exit_for() {
    case "$1" in
        200) echo 0 ;;
        400|413) echo 2 ;;
        401|403) echo 3 ;;
        404) echo 4 ;;
        *) echo 1 ;;
    esac
}

# _job_call GET|POST <route> [json|query] → _SC_BODY/_SC_STATUS; fails loud.
_job_call() {
    local method="$1" route="$2" payload="${3:-}"
    _sidecar_split "$(_sidecar_send "$method" "/cli/jobs/${route}" "$payload")"
    if [ "$_SC_STATUS" != "200" ]; then
        if [ "$_SC_STATUS" = "000" ]; then
            echo "daemon_unreachable: the K2 daemon did not answer" >&2
            exit 1
        fi
        printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
raw = sys.stdin.read()
try:
    e = json.loads(raw).get("error") or {}
    print("%s: %s" % (e.get("code", "error"), e.get("hint", "")) if isinstance(e, dict) else str(e), file=sys.stderr)
except Exception:
    print(raw[:300] or "the daemon did not answer", file=sys.stderr)
'
        exit "$(_job_exit_for "$_SC_STATUS")"
    fi
}

_job_render_list() {
    printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
jobs = v.get("jobs") or []
if v.get("paused"):
    print("ALL JOBS PAUSED by %s (k2 job resume --all)" % (v.get("pausedBy") or "?"))
if not jobs:
    print("no jobs (k2 job add NAME --every 15m -- <cmd>)")
    sys.exit(0)
print("%-24s %-22s %-8s %-28s %s" % ("NAME", "SCHEDULE", "STATE", "LAST RUN", "COMMAND"))
for j in jobs:
    state = "running" if j.get("running") else ("on" if j.get("enabled") else "off")
    last = j.get("lastRun") or {}
    lr = ("%s %s" % (last.get("outcome", ""), (last.get("startedAt") or "")[5:16])) if last else "-"
    name = j.get("name", "")
    if v.get("all") or len({x.get("workspace") for x in jobs}) > 1:
        name = "%s/%s" % (j.get("workspace"), name)
    cmd = j.get("command", "")
    if len(cmd) > 60:
        cmd = cmd[:57] + "..."
    print("%-24s %-22s %-8s %-28s %s" % (name, j.get("schedule", ""), state, lr, cmd))
'
}

_job_render_show() {
    printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
j = json.load(sys.stdin).get("job") or {}
state = "running (run %s)" % j["running"] if j.get("running") else ("on" if j.get("enabled") else "off")
print("job:        %s (workspace %s)" % (j.get("name"), j.get("workspace")))
print("schedule:   %s" % j.get("schedule"))
print("state:      %s" % state)
print("next:       %s" % (j.get("next") or "-"))
print("command:    %s" % j.get("command"))
print("folder:     %s" % (j.get("cwd") or "(the workspace folder)"))
print("timeout:    %ss" % j.get("timeoutSecs"))
print("made by:    %s at %s" % (j.get("createdBy"), j.get("createdAt")))
if j.get("updatedBy") and j.get("updatedAt") != j.get("createdAt"):
    print("changed by: %s at %s" % (j.get("updatedBy"), j.get("updatedAt")))
g = j.get("grants") or []
print("secrets:    %s" % (", ".join("%s (%s%s)" % (x.get("secret"), x.get("delivery"), (" as $" + x["envName"]) if x.get("envName") else "") for x in g) if g else "none granted (k2 vault grant KEY --to job:%s)" % j.get("name")))
runs = j.get("runs") or []
if runs:
    print()
    print("%-38s %-16s %-10s %-6s %s" % ("RUN", "OUTCOME", "TRIGGER", "EXIT", "STARTED"))
    for r in runs:
        ec = r.get("exitCode")
        print("%-38s %-16s %-10s %-6s %s" % (r.get("id"), r.get("outcome"), r.get("trigger"), "-" if ec is None else ec, r.get("startedAt")))
'
}

cmd_job() {
    local sub="${1:-}"
    case "$sub" in
        ""|help|--help|-h) _job_usage; return 0 ;;
    esac
    shift
    local a
    for a in "$@"; do
        [ "$a" = "--" ] && break
        case "$a" in -h|--help) _job_usage; return 0 ;; esac
    done
    local as_json=0 all=0 every="" cron="" timeout="" cwd="" ws="" run_id="" have_cwd=0 have_cmd=0
    local pos=() argv=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --) shift; have_cmd=1; argv=("$@"); break ;;
            --json) as_json=1; shift ;;
            --all) all=1; shift ;;
            --every|--cron|--timeout|--cwd|--workspace|--run)
                [ -n "${2:-}" ] || { echo "usage: $1 needs a value (k2 job --help)" >&2; exit 2; }
                case "$1" in
                    --every) every="$2" ;; --cron) cron="$2" ;; --timeout) timeout="$2" ;;
                    --cwd) cwd="$2"; have_cwd=1 ;; --workspace) ws="$2" ;; --run) run_id="$2" ;;
                esac
                shift 2 ;;
            --*) echo "usage: unknown flag '$1' (k2 job --help)" >&2; exit 2 ;;
            *) pos+=("$1"); shift ;;
        esac
    done
    # People: the workspace this folder is in unless named. Inside a K2
    # terminal the daemon pins the agent's own workspace.
    if [ -z "$ws" ] && [ -z "${SCOPED_TOKEN:-}" ]; then
        ws="$PROJECT"
    fi
    local name="${pos[0]:-}"
    local body
    body="$(python3 -c '
import json, sys
k = ["workspace", "name", "every", "cron", "timeout", "cwd"]
vals = sys.argv[1:7]
o = {kk: vv for kk, vv in zip(k, vals) if vv}
flags = sys.argv[7]
if "c" in flags:
    o["cwd"] = sys.argv[6]
if "a" in flags:
    o["all"] = True
if "x" in flags:
    o["argv"] = sys.argv[8:]
print(json.dumps(o))' "$ws" "$name" "$every" "$cron" "$timeout" "$cwd" \
        "$([ "$have_cwd" = 1 ] && printf c)$([ "$all" = 1 ] && printf a)$([ "$have_cmd" = 1 ] && printf x)" \
        ${argv[@]+"${argv[@]}"})"
    local q
    q="$(python3 -c '
import sys, urllib.parse
o = {}
for k, v in zip(["workspace", "name", "run"], sys.argv[1:4]):
    if v:
        o[k] = v
if sys.argv[4] == "1":
    o["all"] = "1"
print(urllib.parse.urlencode(o))' "$ws" "$name" "$run_id" "$all")"
    local need_name=1
    case "$sub" in
        list|ls|pause|resume) need_name=0 ;;
    esac
    if [ "$need_name" = 1 ] && [ -z "$name" ]; then
        echo "usage: k2 job $sub NAME (k2 job --help)" >&2
        exit 2
    fi
    case "$sub" in
        add)
            [ "$have_cmd" = 1 ] && [ "${#argv[@]}" -gt 0 ] || { echo "usage: k2 job add NAME --every 15m -- <cmd> [args…]" >&2; exit 2; }
            _job_call POST add "$body"
            ;;
        edit)
            _job_call POST edit "$body" ;;
        list|ls)
            _job_call GET list "$q"
            if [ "$as_json" = 1 ]; then printf '%s\n' "$_SC_BODY"; else _job_render_list; fi
            return 0 ;;
        show)
            _job_call GET show "$q"
            if [ "$as_json" = 1 ]; then printf '%s\n' "$_SC_BODY"; else _job_render_show; fi
            return 0 ;;
        logs|log)
            _job_call GET logs "$q"
            if [ "$as_json" = 1 ]; then
                printf '%s\n' "$_SC_BODY"
            else
                printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
r = v.get("run") or {}
ec = r.get("exitCode")
print("# run %s  %s  %s  exit %s  %s bytes%s" % (r.get("id"), r.get("startedAt"), r.get("outcome"), "-" if ec is None else ec, r.get("bytes"), "  (tail)" if v.get("truncated") else ""))
sys.stdout.write(v.get("log") or "")
'
            fi
            return 0 ;;
        run|run-now)
            _job_call POST run "$body" ;;
        enable|disable|kill)
            _job_call POST "$sub" "$body" ;;
        rm|remove|delete)
            _job_call POST remove "$body" ;;
        pause|resume)
            if [ "$all" != 1 ] && [ -z "$name" ]; then
                echo "usage: k2 job $sub NAME | k2 job $sub --all" >&2
                exit 2
            fi
            _job_call POST "$sub" "$body" ;;
        *) echo "usage: unknown k2 job verb '$sub' (k2 job --help)" >&2; exit 2 ;;
    esac
    if [ "$as_json" = 1 ]; then
        printf '%s\n' "$_SC_BODY"
        return 0
    fi
    printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
sub = sys.argv[1]
v = json.load(sys.stdin)
j = v.get("job") if isinstance(v.get("job"), dict) else None
if sub == "add":
    print("added %s: %s, runs %s" % (j.get("name"), j.get("schedule"), j.get("command")))
    print("next run: %s. Grant it secrets with k2 vault grant KEY --to job:%s." % (j.get("next") or "-", j.get("name")))
elif sub == "edit":
    print("changed %s: %s, runs %s" % (j.get("name"), j.get("schedule"), j.get("command")))
elif sub in ("run", "run-now"):
    print("started %s, run %s (k2 job logs %s --run %s)" % (v.get("job"), v.get("run"), v.get("job"), v.get("run")))
elif sub == "kill":
    print("stopping %s, run %s" % (v.get("job"), v.get("run")))
elif sub in ("rm", "remove", "delete"):
    print("removed %s (%s secret grant(s) removed)" % (v.get("removed"), v.get("grantsRevoked")))
elif j is not None:
    print("%s is %s" % (j.get("name"), "on" if j.get("enabled") else "off"))
elif "paused" in v:
    print("every job is %s" % ("paused" if v.get("paused") else "running again"))
else:
    print("ok")
' "$sub"
}

# k2 vault systemd-cred NAME --unit U [--cred-name C] [--restart] [--dry-run] | --remove
# People only (vault.sock + a confirm), Linux only. The value never passes
# through here: the daemon resolves it and pipes it to the root helper.
_vault_systemd_cred() {
    local name="" unit="" cred="" scope="" restart=0 dry=0 remove=0 replace=0 as_json=0 accept=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --accept-agent-value) accept=1; shift ;;
            --unit|--cred-name|--scope)
                [ -n "${2:-}" ] || { echo "usage: $1 needs a value" >&2; exit 2; }
                case "$1" in --unit) unit="$2" ;; --cred-name) cred="$2" ;; --scope) scope="$2" ;; esac
                shift 2 ;;
            --restart) restart=1; shift ;;
            --dry-run) dry=1; shift ;;
            --remove) remove=1; shift ;;
            --replace) replace=1; shift ;;
            --json) as_json=1; shift ;;
            --value|--value=*|--secret-value*)
                echo "value_on_argv: K2 takes the value from the vault, never an argument" >&2; exit 2 ;;
            --*) echo "usage: unknown flag '$1' (k2 vault systemd-cred --help)" >&2; exit 2 ;;
            *) [ -z "$name" ] || { echo "usage: k2 vault systemd-cred NAME --unit <unit.service> [--restart] [--dry-run] | --remove | --replace" >&2; exit 2; }
               name="$1"; shift ;;
        esac
    done
    local usage="usage: k2 vault systemd-cred NAME --unit <unit.service> [--cred-name <id>] [--restart] [--dry-run] | NAME --unit <unit> --remove | NAME --replace"
    [ -n "$name" ] || { echo "$usage" >&2; exit 2; }
    if [ "$replace" = 1 ]; then
        [ -z "$unit" ] && [ "$remove" = 0 ] && [ "$restart" = 0 ] && [ -z "$cred" ] || { echo "usage: --replace takes no --unit, --cred-name, --restart or --remove (each placement keeps its own)" >&2; exit 2; }
    else
        [ -n "$unit" ] || { echo "$usage" >&2; exit 2; }
    fi
    if [ "$remove" = 1 ] && [ "$restart" = 1 ]; then
        echo "usage: --remove takes no --restart (restart the unit yourself after)" >&2; exit 2
    fi
    if [ "$(uname -s)" != "Linux" ]; then
        echo "linux_only: systemd credentials are Linux only (systemd 250+); on this machine use k2 vault fetch, file or env delivery" >&2
        exit 2
    fi
    _vault_person_guard
    local sj nj body op="place" route="systemd-cred"
    [ "$remove" = 1 ] && { op="remove"; route="systemd-cred/remove"; }
    [ "$replace" = 1 ] && op="replace"
    sj="$(_vault_scope_json "$scope")" || exit 2
    nj="$(python3 -c 'import json,sys; print(json.dumps({"name": sys.argv[1]}))' "$name")"
    body="$(python3 -c 'import json,sys
o = {"op": sys.argv[4]}
if sys.argv[1]: o["unit"] = sys.argv[1]
if sys.argv[2]: o["credName"] = sys.argv[2]
if sys.argv[3] == "1": o["restart"] = True
if sys.argv[4] == "replace": o["replace"] = True
print(json.dumps(o))' "$unit" "$cred" "$restart" "$op")"
    body="$(_vault_merge "$sj" "$nj" "$body")"
    # Check everything first (unit, allow list, ceiling, helper), with no
    # confirm; the daemon writes what the person is about to approve.
    _vault_sock_call POST "$route" "$(_vault_merge "$body" '{"dryRun": true}')"
    [ "$_SC_STATUS" = "200" ] || _vault_fail "$_SC_STATUS" "$_SC_BODY"
    if [ "$dry" = 1 ]; then
        if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
        printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin); p = v.get("plan") or {}
print("would: %s" % v.get("confirmText"))
if p:
    print("file:      %s" % p.get("credPath"))
    print("drop-in:   %s" % p.get("dropin"))
    print("           %s" % p.get("dropinLine"))
    print("reads it:  %s" % p.get("readAs"))
if v.get("allowed") is False: print("the unit is not on this box allow list yet (k2 vault systemd-cred --help)")
if v.get("helperReady") is False: print("the root helper must be updated first: k2 root-helper update")'
        exit 0
    fi
    local text
    text="$(printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("confirmText") or "")')"
    if [ "$op" != "remove" ]; then
        # Bind the plan to what the dry run showed: this exact version (and,
        # for --replace, these placements). A value no person wrote needs
        # --accept-agent-value, after a look at it.
        if ! printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); sys.exit(0 if v.get("authorIsPerson", True) or v.get("behind") == 0 else 1)' && [ "$accept" != 1 ]; then
            echo "$text" >&2
            echo "agent_written_value: no person wrote this value and it would run in a root unit. Review it first (k2 vault reveal $name), then add --accept-agent-value." >&2
            exit 3
        fi
        body="$(printf '%s' "$_SC_BODY" | python3 -c 'import json,sys
d = json.load(sys.stdin); o = json.loads(sys.argv[1])
o["version"] = d.get("version")
if "rows" in d: o["rows"] = d["rows"]
if sys.argv[2] == "1": o["acceptAgentValue"] = True
print(json.dumps(o))' "$body" "$accept")"
    fi
    if [ "$op" = "replace" ] && printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; sys.exit(0 if json.load(sys.stdin).get("behind") == 0 else 1)'; then
        echo "$text"; exit 0
    fi
    if [ "$op" != "remove" ] && ! printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; sys.exit(0 if json.load(sys.stdin).get("helperReady") else 1)'; then
        _vault_sock_call POST "$route" "$body"
        _vault_fail "$_SC_STATUS" "$_SC_BODY"
    fi
    local tok
    tok="$(_vault_confirm_token_plan systemd-cred "$body" systemdCred)" || exit 3
    _vault_sock_call POST "$route" "$(_vault_merge "$body" "$(python3 -c 'import json,sys; print(json.dumps({"confirmToken": sys.argv[1]}))' "$tok")")"
    [ "$_SC_STATUS" = "200" ] || _vault_fail "$_SC_STATUS" "$_SC_BODY"
    if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
    printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
if "credRemoved" in v:
    print("took the credential %s off %s%s" % (v.get("credName"), v.get("unit"), "" if v.get("credRemoved") else " (the .cred stays: another unit loads it)"))
elif "replaced" in v:
    for r in v.get("replaced") or []:
        if r.get("ok"):
            print("re-placed v%s into %s%s" % (r.get("version"), r.get("unit"), (", restart queued" if r.get("restarted") else (" (restart refused: %s)" % r["restartError"] if r.get("restartError") else ""))))
        else:
            e = r.get("error") or {}
            print("NOT re-placed into %s: %s: %s" % (r.get("unit"), e.get("code"), e.get("hint")))
    if not v.get("replaced"): print("nothing was behind")
else:
    p = v.get("plan") or {}
    print("sealed %s v%s into %s, bound to %s%s" % (p.get("secret"), v.get("version"), p.get("credPath"), p.get("unit"), ", restart queued" if v.get("restarted") else ""))
    print("the service reads %s" % p.get("readAs"))
if v.get("next"): print(v["next"])
sys.exit(0 if v.get("ok", True) else 1)'
}

# Like _vault_confirm_token, but the token is bound to a plan the daemon
# rebuilds and words itself (systemd-cred). $2 = the plan (route body).
_vault_confirm_token_plan() {
    # $1 = action, $2 = the plan, $3 = its key (systemdCred | rollbackPlan).
    # confirm/start answers the daemon's own words for the plan; they are
    # shown before the person types.
    local action="$1" plan="$2" key="${3:-systemdCred}"
    _vault_sock_call POST "confirm/start" "$(python3 -c 'import json,sys; print(json.dumps({"action": sys.argv[1], sys.argv[3]: json.loads(sys.argv[2])}))' "$action" "$plan" "$key")"
    [ "$_SC_STATUS" = "200" ] || _vault_fail "$_SC_STATUS" "$_SC_BODY"
    local methods body text
    methods="$(printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; print(" ".join(json.load(sys.stdin).get("methods") or []))')"
    text="$(printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("confirmText") or "")')"
    [ -n "$text" ] && echo "You are approving: $text" >&2
    local m=""
    case " $methods " in
        *" vault-password "*) m="vault-password"; printf 'Vault password: ' >&2 ;;
        *" connect-password "*) m="connect-password"; printf 'Your K2 Connect password: ' >&2 ;;
        *) echo "no_confirm_method: set a vault password first: k2 vault confirm-password set" >&2; exit 3 ;;
    esac
    local pw=""
    IFS= read -rs pw || pw=""
    echo >&2
    body="$(printf '%s' "$pw" | python3 -c 'import json,sys; print(json.dumps({"action": sys.argv[1], "method": sys.argv[4], "password": sys.stdin.read(), sys.argv[3]: json.loads(sys.argv[2])}))' "$action" "$plan" "$key" "$m")"
    pw=""
    _vault_sock_call POST "confirm/finish" "$body"
    body=""
    [ "$_SC_STATUS" = "200" ] || _vault_fail "$_SC_STATUS" "$_SC_BODY"
    printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin)["token"])'
}

# k2 vault placement add|done|rm …, k2 vault placements [NAME]
_vault_placement() {
    local sub="$1"; shift
    local act=""
    if [ "$sub" = "placements" ]; then
        act="list"
    else
        act="${1:-list}"
        [ $# -gt 0 ] && shift
    fi
    local name="" where="" note="" scope="" as_json=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --where|--note|--scope)
                [ -n "${2:-}" ] || { echo "usage: $1 needs a value" >&2; exit 2; }
                case "$1" in --where) where="$2" ;; --note) note="$2" ;; --scope) scope="$2" ;; esac
                shift 2 ;;
            --json) as_json=1; shift ;;
            --*) echo "usage: unknown flag '$1' (k2 vault placement --help)" >&2; exit 2 ;;
            *) [ -z "$name" ] || { echo "usage: k2 vault placement --help" >&2; exit 2; }
               name="$1"; shift ;;
        esac
    done
    local sj
    sj="$(_vault_scope_json "$scope")" || exit 2
    case "$act" in
        add)
            [ -n "$name" ] && [ -n "$where" ] || { echo "usage: k2 vault placement add NAME --where <host>:<path> [--note …]" >&2; exit 2; }
            _vault_call POST placement/add "$(python3 -c 'import json,sys
o = json.loads(sys.argv[1]); o["name"] = sys.argv[2]; o["where"] = sys.argv[3]
if sys.argv[4]: o["note"] = sys.argv[4]
print(json.dumps(o))' "$sj" "$name" "$where" "$note")"
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; p=json.load(sys.stdin)["placement"]; print("noted %s at %s (v%s): %s" % (p.get("name"), p.get("where"), p.get("version"), p.get("id")))'
            ;;
        done|rm|remove)
            [ -n "$name" ] || { echo "usage: k2 vault placement $act ID (ids: k2 vault placements)" >&2; exit 2; }
            local route="placement/done"
            [ "$act" = "done" ] || route="placement/remove"
            _vault_call POST "$route" "$(python3 -c 'import json,sys; print(json.dumps({"id": sys.argv[1]}))' "$name")"
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            if [ "$act" = "done" ]; then
                printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; p=json.load(sys.stdin)["placement"]; print("%s at %s now holds v%s" % (p.get("name"), p.get("where"), p.get("version")))'
            else
                echo "removed placement ${name} (the value at that place is yours to delete)"
            fi
            ;;
        list)
            local q=""
            if [ -n "$name" ]; then
                q="$(python3 -c '
import json, sys, urllib.parse
o = json.loads(sys.argv[1]); o["name"] = sys.argv[2]; print(urllib.parse.urlencode(o))' "$sj" "$name")"
            fi
            _vault_call GET placements "$q"
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
rows = v.get("placements") or []
if not rows: print("no placements recorded (k2 vault placement add NAME --where <host>:<path>)")
for p in rows:
    where = ("systemd %s -> %s" % (p.get("credName"), p.get("unit"))) if p.get("target") == "systemd_cred" else (p.get("where") or "")
    note = ("  # " + p["note"]) if p.get("note") else ""
    print("%s  %-24s v%s/%s %-8s %s%s" % (p.get("id"), p.get("name"), p.get("version"), p.get("latestVersion"), p.get("state"), where, note))
if v.get("needNewerK2"): print("%d more need a newer K2 to read" % v["needNewerK2"])'
            ;;
        *) _vault_sub_usage placement; exit 2 ;;
    esac
}

