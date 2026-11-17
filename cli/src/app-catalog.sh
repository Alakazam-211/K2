# ── one-click suite Apps (prd-one-click-suite-apps-v1) ──────────

_app_catalog_usage() {
    cat <<'EOF'
k2 app catalog — suite Apps this K2 deploys in one step (Sew's Hearth and Meet),
verified by K2: each version is pinned by sha256 inside this K2 release.

  k2 app catalog [list] [--json]          What this box can deploy, and why not when not.
  k2 app catalog show <entry> [--json]    What it does, what deploying approves, versions.
  k2 app catalog check <entry> <address> [--id ID] [--json]
                                          Is the address free for that App? exit 0 yes,
                                          exit 2 no (with a suggestion).
  k2 app install --catalog <entry> [--address LABEL|auto] [--id ID]
                 [--variant k2|standalone] [--version V] [--workspace WS]
                 [--dry-run] [--no-wait] [--rebind-meet] [--json]
                                          Deploy it as one daemon job: download, check,
                                          install, address, deploy, approve, sew_setup,
                                          healthy, ready. Prints the job id first, then
                                          each step; closing the terminal doesn't stop it.
    --address   the label in <label>.<box>.k2.dev (default: chat for Hearth, meet for
                Meet); auto = the default, else the suggestion when it's taken
    --id        the App id (default hearth, then hearth-2…; sew-meet); rarely needed
    --no-wait   start the job and return; follow it with k2 app catalog job <id>
    --workspace the workspace whose agent looks after it (an agent: its own)
    --variant   only to assert the variant; K2 picks it from this box's Sew sign-in
  k2 app catalog job [<job-id>] [--app APP] [--json]
                                          One job's steps, or every job.
  k2 app catalog retry <job-id>           Go on from a failed download_failed,
                                          sew_unreachable, not_ready or deploy_failed step.

Entries: sew/hearth (chat.<box>.k2.dev), sew/meet (meet.<box>.k2.dev, one per box).
A K2 address needs K2 Connect Pro (pro_required). Your own domain: deploy first,
then k2 app domain add <app> <host>.
--json: each reason, wait and error has "text" (plain words for people) and "hint"
(the command to run, or null); a done job's "next" lines carry the commands.
Exit: 0 done; 1 the job failed (its error has the fix); 2 not done: the job waits
and goes on by itself, OR the command needs changing (address taken: use the
suggestion) — the JSON's "error"/"waiting" says which; 3 refused.
More: k2 study suite-apps
EOF
}

# The job view → lines (stdin = {job:{…}}); `seen` = steps already printed.
_app_catalog_print_job() {
    K2_CJ_SEEN="${1:-}" python3 -c '
import json, os, sys
v = json.load(sys.stdin)
j = v.get("job") or v
seen = set(filter(None, os.environ.get("K2_CJ_SEEN", "").split(",")))
for s in j.get("steps", []):
    if s.get("state") == "done" and s["step"] not in seen:
        note = ("  " + s["note"]) if s.get("note") else ""
        print("  ok   %-10s%s" % (s["step"], note[:100]))
'
}

_app_catalog_done_steps() {
    python3 -c '
import json, sys
v = json.load(sys.stdin)
j = v.get("job") or v
print(",".join(s["step"] for s in j.get("steps", []) if s.get("state") == "done"))
'
}

# Print a job's end (done / failed / waiting) and exit by it.
_app_catalog_end() {
    local json_out="$1" state
    state="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print((v.get("job") or v).get("state",""))')"
    if [ "$json_out" = 1 ]; then
        printf '%s\n' "$_CP_BODY"
    else
        printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
j = v.get("job") or v
a = j.get("address") or {}
st = j.get("state")
if st == "done":
    print("%s is live at %s (App %s, %s %s)" % (j.get("entry"), a.get("url") or a.get("label"), j.get("app"), j.get("variant"), j.get("version")))
    for line in j.get("next") or []:
        print("next: %s" % line)
elif st == "failed":
    e = j.get("error") or {}
    # text = the plain sentence, hint = the command (an older daemon sent
    # only hint, the sentence with its command).
    text = e.get("text") or e.get("hint") or ""
    cmd = e.get("hint") if "text" in e else None
    print("failed at %s (%s): %s" % (j.get("step"), e.get("code"), text))
    if cmd:
        print("run: %s" % cmd)
    elif e.get("retry"):
        print("run: k2 app catalog retry %s" % j.get("id"))
elif st == "waiting":
    w = j.get("waiting") or {}
    print("waiting at %s: %s" % (j.get("step"), w.get("reason")))
    if w.get("requests"):
        print("  request(s): %s" % ", ".join(w["requests"]))
    text = w.get("text") or w.get("hint")
    if text:
        print("  %s" % text)
    if "text" in w and w.get("hint"):
        print("  run: %s" % w["hint"])
    print("The job waits in the daemon and goes on by itself; check: k2 app catalog job %s" % j.get("id"))
else:
    print("still %s at %s; check: k2 app catalog job %s" % (st, j.get("step"), j.get("id")))
'
    fi
    case "$state" in
        done) return 0 ;;
        failed) exit 1 ;;
        *) exit 2 ;;
    esac
}

# An address_taken / one_per_box answer is a usage problem: exit 2.
_app_catalog_fail() {
    local code
    code="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys
try: print((json.load(sys.stdin).get("error") or {}).get("code",""))
except Exception: print("")' 2>/dev/null)"
    case "$code" in
        address_taken|one_per_box|address_reserved|address_invalid)
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
e = json.load(sys.stdin)["error"]
out = {"error": e.get("code"), "hint": e.get("hint")}
for k in ("suggestion", "holder", "existing"):
    if e.get(k):
        out[k] = e[k]
print(json.dumps(out))' >&2
            exit 2 ;;
    esac
    _compute_fail "$_CP_STATUS" "$_CP_BODY"
}

# k2 app install --catalog <entry> [...]
cmd_app_install_catalog() {
    local entry="" address="" id="" variant="" version="" ws="" dry=0 wait=1 rebind=0 json_out=0 domain=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --catalog) entry="${2:-}"; shift 2 ;;
            --catalog=*) entry="${1#--catalog=}"; shift ;;
            --address) address="${2:-}"; shift 2 ;;
            --address=*) address="${1#--address=}"; shift ;;
            --id) id="${2:-}"; shift 2 ;;
            --id=*) id="${1#--id=}"; shift ;;
            --variant) variant="${2:-}"; shift 2 ;;
            --version) version="${2:-}"; shift 2 ;;
            --workspace) ws="${2:-}"; shift 2 ;;
            --domain) domain="${2:-}"; shift 2 ;;
            --dry-run) dry=1; shift ;;
            --no-wait) wait=0; shift ;;
            --rebind-meet) rebind=1; shift ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_catalog_usage; return 0 ;;
            -*) echo "Unknown flag: $1 (k2 app catalog --help)" >&2; exit 2 ;;
            *) [ -z "$id" ] || { echo "usage: k2 app install --catalog <entry> [--id ID] [--address LABEL|auto] (k2 app catalog --help)" >&2; exit 2; }; id="$1"; shift ;;
        esac
    done
    [ -n "$entry" ] || { echo "usage: k2 app install --catalog <entry> [--address LABEL|auto] [--dry-run] (entries: k2 app catalog)" >&2; exit 2; }
    case "$variant" in ""|k2|standalone) ;; *) echo "--variant is k2 or standalone" >&2; exit 2 ;; esac
    ws="$(_compute_ws "$ws")"
    _app_rt_post /cli/app/catalog/install "$(_compute_json K2_CJ_entry="$entry" K2_CJ_address="$address" K2_CJ_id="$id" \
        K2_CJ_variant="$variant" K2_CJ_version="$version" K2_CJ_workspace="$ws" K2_CJ_domain="$domain" \
        K2_CJ_dryRun_b="$dry" K2_CJ_rebindMeet_b="$rebind")"
    [ "$_CP_STATUS" = "200" ] || _app_catalog_fail
    if [ "$dry" = 1 ]; then
        if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
        printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
p = v["plan"]
a = p["address"]
print("Dry run: nothing changed.")
print("%s %s (%s variant) as App %s at %s" % (p["name"], p["version"], p["variant"], p["app"], a.get("url") or a["label"]))
print("It will:")
for line in p.get("willDo", []):
    print("  - %s" % line)
if p.get("approves"):
    print("Deploying approves:")
    for x in p["approves"]:
        print("  - %s %s%s" % ("box service" if x["kind"] == "boxService" else "cap", x["name"], (": " + x["why"]) if x.get("why") else ""))
if p.get("setup"):
    print("Sew setup: %s" % ", ".join(p["setup"]))
print("Steps: %s" % " → ".join(p.get("steps", [])))
for w in p.get("waits", []):
    print("It will wait at: %s: %s" % (w["reason"], w.get("text") or w.get("hint")))
    if w.get("text") and w.get("hint"):
        print("  run: %s" % w["hint"])
print("Pro: %s" % v.get("tier"))
f = p.get("files") or {}
print("File: %s/%s sha256 %s" % (f.get("tag"), f.get("file"), f.get("sha256")))
'
        return 0
    fi
    local job
    job="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin)["job"]["id"])')"
    if [ "$wait" = 0 ]; then
        if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; else
            echo "Started job $job (it runs in the daemon). Follow it: k2 app catalog job $job"
        fi
        return 0
    fi
    [ "$json_out" = 1 ] || echo "Job $job: the daemon runs it; Ctrl-C stops watching, not the job."
    local seen="" state="" n=0
    while :; do
        _compute_call GET /cli/app/catalog/job "id=$(urlencode "$job")"
        if [ "$json_out" != 1 ]; then
            printf '%s' "$_CP_BODY" | _app_catalog_print_job "$seen"
            seen="$(printf '%s' "$_CP_BODY" | _app_catalog_done_steps)"
        fi
        state="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin)["job"]["state"])')"
        case "$state" in done|failed|waiting) break ;; esac
        n=$((n + 1))
        [ "$n" -ge 600 ] && break
        sleep 2
    done
    _app_catalog_end "$json_out"
}

# k2 app catalog [list|show|check|job|retry]
cmd_app_catalog() {
    local sub="${1:-list}"
    case "$sub" in
        -h|--help|help) _app_catalog_usage; return 0 ;;
        list) [ $# -gt 0 ] && shift || true ;;
        --json) ;;
        show|check|job|jobs|retry) shift ;;
        *) echo "Unknown: k2 app catalog $sub (list|show|check|job|retry; k2 app catalog --help)" >&2; exit 2 ;;
    esac
    local json_out=0 args=() id="" app=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            --id) id="${2:-}"; shift 2 ;;
            --app) app="${2:-}"; shift 2 ;;
            -h|--help) _app_catalog_usage; return 0 ;;
            -*) echo "Unknown flag: $1 (k2 app catalog --help)" >&2; exit 2 ;;
            *) args+=("$1"); shift ;;
        esac
    done
    case "$sub" in
        list|--json)
            _compute_call GET /cli/app/catalog ""
            if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
for e in v.get("entries", []):
    a = e.get("defaultAddress") or {}
    ver = e.get("version") or "not pinned yet"
    print("%-12s %-7s %-14s %s  (%s, %s)" % (e["entry"], e["name"], ver, a.get("url") or (a.get("label", "") + ".<box>.k2.dev"), e.get("verified") or "not verified yet: nothing pinned", e.get("license")))
    print("             %s" % e.get("summary", ""))
    if e.get("alreadyDeployed"):
        print("             already deployed as %s (one per box)" % e["alreadyDeployed"])
    elif e.get("canDeploy"):
        print("             ready: k2 app install --catalog %s" % e["entry"])
    for r in e.get("reasons", []):
        print("             can'\''t yet: %s (%s)" % (r.get("text"), r.get("code")))
        if r.get("hint"):
            print("               run: %s" % r["hint"])
    for i in e.get("installed", []):
        print("             installed: %s %s (%s)" % (i["app"], i["version"], i["variant"]))
print("Trust: %s" % v.get("trust"))
'
            ;;
        show)
            [ "${#args[@]}" = 1 ] || { echo "usage: k2 app catalog show <entry> [--json]" >&2; exit 2; }
            _compute_call GET /cli/app/catalog/show "entry=$(urlencode "${args[0]}")"
            if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
e = json.load(sys.stdin)
a = e.get("defaultAddress") or {}
print("%s — %s (%s)" % (e["name"], e["summary"], e["entry"]))
print("  publisher %s · licence %s · %s" % (e["publisher"]["name"], e["license"], e.get("verified") or "not verified yet: nothing pinned"))
print("  installs as App %s · default address %s%s" % (e["installId"], a.get("url") or a.get("label"), " · one per box" if e.get("onePerBox") else ""))
print("  version %s (pinned: %s)" % (e.get("version") or "none yet", ", ".join(e.get("versions") or []) or "none"))
print("  it will:")
for line in e.get("willDo", []):
    print("    - %s" % line)
for x in e.get("approves", []):
    print("  deploying approves %s %s%s" % ("box service" if x["kind"] == "boxService" else "cap", x["name"], (": " + x["why"]) if x.get("why") else ""))
for r in e.get("reasons", []):
    print("  can'\''t yet: %s (%s)" % (r.get("text"), r.get("code")))
    if r.get("hint"):
        print("    run: %s" % r["hint"])
'
            ;;
        check)
            [ "${#args[@]}" = 2 ] || { echo "usage: k2 app catalog check <entry> <address> [--id ID] [--json]" >&2; exit 2; }
            local q="entry=$(urlencode "${args[0]}")&address=$(urlencode "${args[1]}")"
            [ -n "$id" ] && q="$q&id=$(urlencode "$id")"
            [ -n "$(_compute_ws "")" ] && q="$q&workspace=$(urlencode "$(_compute_ws "")")"
            _compute_call GET /cli/app/catalog/check-address "$q"
            local ok
            ok="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print("1" if json.load(sys.stdin).get("ok") else "0")')"
            if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; else
                printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
t = v.get("tier") or {}
if v.get("error"):
    e = v["error"]
    print("not free: %s: %s" % (e.get("code"), e.get("hint")))
    if e.get("suggestion"):
        print("suggestion: %s" % e["suggestion"])
else:
    a = v["address"]
    print("free: %s (%s) for App %s" % (a.get("url") or a["label"], a["kind"], v.get("app")))
if t.get("hint"):
    print("pro: %s" % t["hint"])
'
            fi
            [ "$ok" = 1 ] || exit 2
            ;;
        job|jobs)
            local q=""
            [ "${#args[@]}" -ge 1 ] && q="id=$(urlencode "${args[0]}")"
            [ -n "$app" ] && q="app=$(urlencode "$app")"
            _compute_call GET /cli/app/catalog/job "$q"
            if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
            if [ -n "$q" ] && [ "${q%%=*}" = id ]; then
                printf '%s' "$_CP_BODY" | _app_catalog_print_job ""
                _app_catalog_end 0
            else
                printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
for j in json.load(sys.stdin).get("jobs", []):
    w = (j.get("waiting") or {}).get("reason") or (j.get("error") or {}).get("code") or ""
    print("%s  %-11s %-8s %-9s %-10s %s" % (j["id"], j["entry"], j["app"], j["state"], j["step"], w))
'
            fi
            ;;
        retry)
            [ "${#args[@]}" = 1 ] || { echo "usage: k2 app catalog retry <job-id>" >&2; exit 2; }
            _app_rt_post /cli/app/catalog/retry "$(_compute_json K2_CJ_job="${args[0]}")"
            [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
            if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; else echo "Job ${args[0]} goes on; follow it: k2 app catalog job ${args[0]}"; fi
            ;;
    esac
}

_study_page_suite_apps() {
    cat <<'EOF'
k2 study suite-apps — one-click suite Apps (Sew's Hearth and Meet)

WHAT IT IS
  A suite App is an App from a known publisher that K2 deploys in one step.
  The catalog is compiled into this K2 release: each version is pinned by
  sha256 and by K2's own hash of the stored files ("Verified by K2", shown
  only once a version is pinned; until then "verified" is null). A new
  version arrives with a K2 update; moving an installed App to it is a later
  K2. After deploy it is an ordinary App: every k2 app verb, rollback,
  audit, domains and users apply. Hearth comes in "looks" (front ends); this
  K2 pins one. Variant = how people sign in (k2 = K2 logins, standalone =
  Sew's own); K2 picks the one that matches this box's Sew.

THE FLOW
  k2 app catalog                          what this box can deploy, and why not
  k2 app catalog check sew/meet team-meet   is the address free? (exit 0/2)
  k2 app install --catalog sew/meet --address team-meet --dry-run   the plan
  k2 app install --catalog sew/meet --address team-meet
    prints "Job sj-…", then each step as it finishes:
    download → check → install → address → deploy → approve → sew_setup
    → healthy → ready
  The job runs in the daemon: closing the terminal doesn't stop it, and a
  daemon restart resumes it. Find it again: k2 app catalog job (all jobs) or
  k2 app catalog job <job-id>.

ADDRESSES
  The install id is not the address: Hearth installs as App hearth (then
  hearth-2), Meet as sew-meet; neither gets hearth.<box>.k2.dev.
  Defaults: chat.<box>.k2.dev (Hearth), meet.<box>.k2.dev (Meet). Any free
  ordinary label works (team-meet). Taken: address_taken, exit 2, nothing
  made; it names the holder and a suggestion (the look's own name, else
  <app>-chat). Rerun with --address <suggestion> or --address auto.
  chat, meet, mail, voice, skin are reserved: only their own family's App.
  One level only (x.chat.<box>.k2.dev is refused).
  A K2 address needs K2 Connect Pro (pro_required). In this K2 every
  one-click deploy needs one, so no Pro = no one-click deploy yet. Your own
  domain goes on afterwards: k2 app domain add <app-id> <host>, where
  <app-id> is the App id (hearth, sew-meet), not the entry; it prints the DNS
  records. A second Hearth: k2 app install --catalog sew/hearth --address auto.

WHO MAY, AND THE WAITS
  An agent needs "Build and run Apps". Binding chat/meet and Meet's Sew setup
  also need "Manage chat and voice". Without it the job waits
  (waiting_sew_rights): the owner runs k2 agent access set <workspace>
  chat_manage on, or deploys from Settings → Cores → Apps → Add an App. Nothing to
  rerun: the job goes on by itself.
  Box services or caps not yet approved: the job waits (waiting_approval) on
  the request it filed (k2 app catalog job <job-id> shows its id); the owner
  approves it in Settings → Cores → App Approvals or k2 app approve <request>.
  Then the job goes on. Denied: the job ends; ask again with a new install.

MEET
  One per box (one_per_box). Sew setup: meet init with the App's service id,
  the Meetings Space recorded as the App's Space, meetings.enabled. A Space
  still bound to a removed Meet stops the job (meet_space_bound); the owner
  (a person, never an agent) reruns the install with --rebind-meet.

NEVER AUTOMATIC
  Rotating the seal key, changing the sign-in variant, re-binding Meet's
  Space, turning meetings off, deleting the App's data, approving what a
  version adds, changing the primary address after first use.

CODES
  Refused, nothing made: exit 2 address_taken, address_reserved,
  address_invalid, one_per_box (change the command); exit 3 pro_required,
  catalog_not_pinned, variant_mismatch (Sew's sign-in doesn't match a pinned
  variant), requires_unmet (Sew too old), sew_not_installed, needs_linux.
  The job failed, exit 1: catalog_mismatch (<field>): the named field
  (sha256, caps, build.json…) isn't what K2 pinned, nothing installed, never
  retried; unpack_refused; approval_denied (ask the owner again);
  meet_slug_taken (the owner decides in Sew); version_mismatch (/readyz names
  another version: k2 app logs <app>). Retry with k2 app catalog retry <job>:
  download_failed, sew_unreachable, not_ready (/readyz not 200 yet),
  deploy_failed.
EOF
}

