# ── 0.46.0 App grace (Rosson's cross-over plan, 2026-10-14) ─────────────
# Nothing removes existing services: no cut-over is set (Rosson 2026-10-26);
# each agent moves its service to an App when ready (k2 study app-move).
# These verbs are not behind the Apps (beta) switch: they serve every
# published --cmd service (and every runtime App).

_app_grace_usage() {
    cat <<'EOF'
k2 app — moving existing --cmd services to Apps (nothing removes them)

Move a legacy --cmd service to an App yourself (walkthrough: k2 study app-move):
  k2 app data put <app-id> <path> [--to <dir>]
                                       Copy a file or folder of your workspace
                                       into the App's data folder ($K2_APP_DATA,
                                       /data in its wall). The App must be live.
  k2 app data ls|get|rm|mv <app-id> …  Read back what its pages write to /data
                                       (k2 app data get <app-id> var/answers.json),
                                       list, remove, rename (k2 app data --help).
  k2 app label take <app-id> <label>   Your legacy service's public hostname
                                       now serves the App (the App must be
                                       healthy). The old service keeps running,
                                       unrouted.
  k2 app label return <app-id> <label> Move it back (the old service must answer).
  k2 app domain add|rm <app-id> <sub>  Any other subdomain of your workspace
                                       (one you pointed at the service's port)
                                       serves the App too (k2 app domain --help).
  k2 app move-plan <service> [--app <app-id>] [--request] [--json]
                                       Every step of the move, what's done, and
                                       (--request) one owner approval for all
                                       the owner's steps.

  k2 app warnings [<app>] [--json]     What breaks when it moves to an App: env
                                       names the code reads that only the daemon
                                       gives it, K2 files it touches
                                       (daemon.token, heartbeat.port), programs
                                       it finds through PATH, owner-token calls
                                       K2 saw it make (Linux), form-encoded
                                       calls to K2 routes that read JSON only.
                                       An App: writes its gateway link refused
                                       for their body, a pass sent in ?token=
                                       (deprecated). No <app>: every --cmd
                                       service you can see.
  k2 app clean-mode <app> on|off [--restart]
                                       Run it the way an App runs: clean env,
                                       system PATH, its own HOME, owner-token
                                       calls refused (Linux). On narrows (agents
                                       may); off is the owner's (held otherwise).
  k2 app key mint <app> --scopes users,rooms,fetch,push [--rooms a,b] [--roles r]
                 [--label L] [--show]
                                       An App-scoped key instead of the owner
                                       token: it opens /v1/apps/<app id>/… only.
                                       The owner confirms in person; K2 puts it
                                       in the vault and the App reads
                                       $K2_SECRETS_DIR/K2_APP_KEY at its next
                                       start (--show prints it once instead).
                                       From an agent: held for the owner.
  k2 app keys [<app>] [--json]         App keys (never the key itself).
  k2 app key revoke <key-id>           Stops it at once.
  k2 app approvals [--app A] [--all] [--json]
                                       Held requests (an agent's widening, key
                                       mint, clean mode off, new --cmd service).
  k2 app approvals revoke <request-id> | --app <name>
                                       Owner: an approved k2 publish run stops
                                       covering identical re-runs (they ask
                                       again; it lapses by itself after 30 days).
  k2 app approve <request-id>          Owner, in person (Touch ID / password),
                                       from a terminal outside K2.
  k2 app deny <request-id> [--reason R]
                                       Owner/Admin; an agent withdraws its own.
  k2 app access <workspace> [--build on|off] [--manage on|off] [--users on|off]
                 [--core db|files|thread|mail|sew=on|off[,…]] [--json]
                                       No flags: show the agent's switches and
                                       cores. --build = "Build and run Apps"
                                       (switch 1: build, install, deploy, run
                                       its own Apps, their roles and App
                                       passes), --manage = the nested "…and
                                       manage them end to end" (switch 2: it
                                       raises its own budgets, quotas, egress,
                                       box services and keys, resumes an App it
                                       suspended, sends push, moves its own
                                       labels), --users = "Let this agent
                                       manage its Apps' users" (App users and
                                       test guests of its own Apps, their
                                       passwords, email, names, unlock, roles
                                       and any room or cap on its App's roles,
                                       invites, the guests-rooms grant;
                                       never a box login; the owner, both
                                       ways). --core ticks a
                                       core: its Apps then use Database, Files
                                       and rooms, or Thread inside their own
                                       slice with no hold (Mail takes effect
                                       when App mail lands). The
                                       owner turns things on, from
                                       a terminal outside K2; an Admin may only
                                       turn them off. Anything that loosens an
                                       App's wall still waits for the owner in
                                       person. (--open is retired.)

A new --cmd service from an agent is an open-path App: from 0.46.3 each one
is held for the owner's in-person approval (re-runs of an existing service
need only "Build and run Apps").
Owner-token calls are traced on Linux only for now: on macOS, warnings and
clean-mode say "not available on macOS" (the code scan still works).

Exit codes: 0 done, 2 held for the owner (or a usage error), 3 refused or
infra (JSON on stderr).
EOF
}

# Print the held answer (202) and exit 2, or fail on a refusal.
_app_grace_done() {
    local json_out="$1"
    if [ "$_CP_STATUS" = "202" ]; then
        if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; else
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
for line in v.get("applied") or []:
    print("applied: %s" % line)
helds = [h for h in (v.get("held"), v.get("egressHeld")) if isinstance(h, dict)] or [v]
for h in helds:
    r = h.get("request") if isinstance(h.get("request"), dict) else {}
    print("held for the owner: %s  %s" % (r.get("id"), r.get("summary", "")))
    d = h.get("diff")
    if isinstance(d, dict):
        d = d.get("lines")
    for line in (d if isinstance(d, list) else []):
        print("  %s" % line)
    for why in h.get("reasons") or []:
        print("  why: %s" % why)
    # 0.46.4: say when this is the earlier request again, and where the
    # owner sees it (the permission card, or App Approvals).
    if h.get("reused"):
        print("  this is your earlier request %s (asked %s ago), still pending; nothing new was filed" % (r.get("id"), h.get("askedAgo") or "a while"))
    c = h.get("card") if isinstance(h.get("card"), dict) else {}
    if c.get("where"):
        print("  owner: %s" % c["where"])
    if c.get("error") and (c["error"].get("hint") or c["error"].get("code")):
        print("  card not raised: %s" % (c["error"].get("hint") or c["error"].get("code")))
    print(h.get("hint", ""))
    # 0.46.5: a held test guest (no password is kept with the request).
    if h.get("afterApproval"):
        print(h["afterApproval"])
    if h.get("passwordNotUsed"):
        sys.stderr.write("note: %s\n" % (h.get("passwordNote") or "the password you sent was not used"))'
        fi
        exit 2
    fi
    [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
}

# Disk and memory warnings, and an applied disk override, on stderr (human
# output; the JSON already carries them). prd-app-resources-disk-guard-v1.
_app_disk_notes() {
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
try:
    v = json.load(sys.stdin)
except Exception:
    sys.exit(0)
if not isinstance(v, dict):
    sys.exit(0)
for key, label in (("diskWarning", "disk"), ("memoryWarning", "memory")):
    w = v.get(key)
    if isinstance(w, dict) and w.get("hint"):
        sys.stderr.write("%s warning: %s\n" % (label, w["hint"]))
o = v.get("diskOverride")
if isinstance(o, dict) and o.get("applied"):
    sys.stderr.write("disk override (%s): %s\n" % (o.get("via", ""), o.get("reason", "")))
' || true
}

# Print the box usage view (stdin: GET /cli/app/resources).
_app_resources_print() {
    python3 -c '
import json, sys, time
v = json.load(sys.stdin)
def gb(b):
    if b is None:
        return "—"
    f = float(b)
    for unit, size in (("TB", 1e12), ("GB", 1e9), ("MB", 1e6), ("KB", 1e3)):
        if f >= size:
            return ("%.1f %s" % (f / size, unit)) if f < 10 * size else ("%.0f %s" % (f / size, unit))
    return "%d B" % f
def gib(b):
    if b is None:
        return "—"
    g = float(b) / (1 << 30)
    return ("%.0f GiB" % g) if abs(g - round(g)) < 0.05 else ("%.1f GiB" % g)
def ago(t):
    if not t:
        return "never"
    s = int(time.time() - t)
    if s < 120:
        return "%d s ago" % s
    if s < 7200:
        return "%d min ago" % (s // 60)
    return "%d h ago" % (s // 3600)
b, g, scan = v.get("box") or {}, v.get("guard") or {}, v.get("scan") or {}
l = g.get("lines") or {}
mode = g.get("mode", "?") + (" (from the environment)" if g.get("modeSource") == "env" else "")
print("%s · %s cores · %s RAM · disk guard %s · checked %s (folders scanned %s)" % (
    b.get("host") or "this box", b.get("cores") or "?", gib(b.get("memBytes")), mode,
    time.strftime("%H:%M", time.localtime(b.get("checkedAt") or time.time())), ago(scan.get("workspacesAt"))))
rows = v.get("consumers") or []
print()
print("DISK")
for d in v.get("disks") or []:
    print("  %-8s %s used of %s (%s%%) · %s free · %s" % (d.get("mount") or d.get("id"), gb(d.get("usedBytes")), gb(d.get("totalBytes")),
          d.get("usedPct"), gb(d.get("freeBytes")), str(d.get("level", "?")).upper()))
    if d.get("holds"):
        print("           holds: %s" % ", ".join(d["holds"]))
print("           Lines: warn %s%% or %s free · refuse %s%% or %s left · critical %s%% or %s free" % (
    l.get("warnPct"), gib(l.get("warnFreeBytes")), l.get("refusePct"), gib(l.get("reserveBytes")),
    l.get("criticalPct"), gib(l.get("criticalFreeBytes"))))
m = v.get("mem") or {}
# Only what this OS measures (smoke r6: macOS printed "— available").
ram = ["%s total" % gib(m.get("totalBytes"))]
ram.append(("%s available" % gib(m["availableBytes"])) if m.get("availableBytes") is not None else "available not measured on this OS")
if m.get("agentsBytes") is not None:
    ram.append("agents %s" % gib(m["agentsBytes"]))
print("RAM        " + " · ".join(ram))
print("CPU        load %s (5 min) on %s cores" % ((v.get("cpu") or {}).get("load5") or "—", b.get("cores") or "?"))
def who(r):
    o = r.get("owner") or {}
    return o.get("handle") or ("server" if o.get("kind") == "server" else "—")
def parts(r, n=3):
    p = sorted((r.get("parts") or {}).items(), key=lambda kv: -kv[1])[:n]
    return " · ".join("%s %s" % (k, gb(x)) for k, x in p)
sections = ((("app", "service"), "APPS AND SERVICES"), (("database",), "DATABASES"), (("workspace",), "AGENT FOLDERS"), (("shared",), "SHARED"))
for kinds, title in sections:
    sel = [r for r in rows if r.get("kind") in kinds]
    if not sel:
        continue
    print()
    if "app" in kinds:
        print("%-28s %-15s %-14s %-9s %-9s %s" % (title, "KIND", "OWNER", "DISK", "RAM", "STATE"))
    else:
        print(title)
    for r in sel:
        if r.get("kind") in ("app", "service"):
            k = ("app · %s" % r.get("path", "?")) if r.get("kind") == "app" else "service"
            state = r.get("state") or ""
            if r.get("kind") == "service" and r.get("walled") is False:
                state += " · unwalled --cmd"
            print("%-28s %-15s %-14s %-9s %-9s %s" % (r.get("id"), k, who(r), gb(r.get("diskBytes")), gib(r.get("memBytes")), state))
            if r.get("parts"):
                print("    %s" % parts(r, 6))
        else:
            extra = parts(r)
            if r.get("unusedBytes") is not None:
                extra = (extra + " · " if extra else "") + "%s unused" % gb(r["unusedBytes"])
            if r.get("folder"):
                extra = (extra + "  " if extra else "") + r["folder"]
            print("%-44s %-14s %-9s %s" % (r.get("id"), who(r), gb(r.get("diskBytes")), extra))
        if r.get("partial"):
            print("    (partial: the scan ran out of time)")
for d in v.get("disks") or []:
    if d.get("notMeasuredBytes") is not None:
        print("%-44s %-14s ~%s  (%s)" % ("Not measured by K2 (OS, other users)", "", gb(d["notMeasuredBytes"]), d.get("mount")))
by = v.get("byOwner") or []
if by:
    print()
    print("BIGGEST OWNERS   " + " · ".join("%s %s" % ((o.get("owner") or {}).get("handle") or "?", gb(o.get("diskBytes"))) for o in by[:3]))
    top = (by[0].get("owner") or {}).get("address")
    if top:
        print("Ask an agent:    k2 msg %s \"…\"" % top)
w = v.get("k2WillClear") or {}
if w.get("seedsBytes"):
    print("K2 will clear:   %s of old Seeds (%s) at the next sweep" % (gb(w["seedsBytes"]), w.get("seeds")))
if not scan.get("workspacesAt") and not scan.get("appsAt"):
    print()
    print("No scan has measured Apps and folders yet: their disk shows as —.")
if v.get("suggestion"):
    print()
    print(v["suggestion"])
'
}

# k2 app resources clean [--dry-run] [--json] (0.46.4, disk guard Slice B):
# free only what K2 made and can make again; print each item.
cmd_app_resources_clean() {
    local json_out=0 dry=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --dry-run) dry=1; shift ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_resources_usage; return 0 ;;
            *) echo "usage: k2 app resources clean [--dry-run] [--json]" >&2; exit 2 ;;
        esac
    done
    # A long backlog of Seeds takes minutes to delete; the default 120 s
    # would report a daemon that is still working as unreachable.
    K2_CP_MAX_TIME=1800 _app_rt_post /cli/app/resources/clean "$(_compute_json K2_CJ_dryRun_b="$dry")"
    [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
def gb(b):
    if b is None:
        return "—"
    f = float(b)
    for unit, size in (("TB", 1e12), ("GB", 1e9), ("MB", 1e6), ("KB", 1e3)):
        if f >= size:
            return ("%.1f %s" % (f / size, unit)) if f < 10 * size else ("%.0f %s" % (f / size, unit))
    return "%d B" % f
WORDS = {
    "seed": "unused Seed (K2 copy)",
    "engineSeed": "unused Seed (walled copy)",
    "image": "unused image",
    "engineLeftover": "engine leftover",
    "rotatedLog": "rotated log",
    "deploymentLog": "old deployment log",
    "buildTemp": "build temp",
    "imageArchive": "old base image archive",
}
dry = v.get("dryRun")
items = v.get("items") or []
print("%s (%s)" % ("Would free" if dry else "Cleaned", "this box" if v.get("scope") == "box" else "your workspace\x27s Apps and services"))
if items:
    print()
    print("%-26s %-9s %-14s %s" % ("KIND", "SIZE", "APP", "ITEM"))
for i in items:
    state = ""
    if i.get("error"):
        state = "  (kept: %s)" % i["error"]
    elif not dry and not i.get("removed"):
        state = "  (kept)"
    print("%-26s %-9s %-14s %s%s" % (WORDS.get(i.get("kind"), i.get("kind")), gb(i.get("bytes")), i.get("app") or "—", i.get("id"), state))
t = v.get("totals") or {}
print()
if dry:
    print("Total: about %s" % gb(t.get("bytes")))
else:
    print("Freed: about %s" % gb(t.get("freedBytes")))
e = v.get("engine") or {}
if e.get("note"):
    print("Note: " + e["note"])
elif isinstance(e.get("status"), str) and e["status"].startswith("error"):
    print("Note: the walled engine: " + e["status"])
print(v.get("never") or "")
if v.get("hint"):
    print(v["hint"])
'
}

# k2 app resources trends [--app A] [--window 1h|6h|24h|7d|30d] [--json]
# (0.46.4, disk guard Slice B): CPU and memory over time, box and per App.
cmd_app_resources_trends() {
    local json_out=0 app="" window=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --app) app="${2:-}"; shift 2 ;;
            --window) window="${2:-}"; shift 2 ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_resources_usage; return 0 ;;
            *) echo "usage: k2 app resources trends [--app A] [--window 1h|6h|24h|7d|30d] [--json]" >&2; exit 2 ;;
        esac
    done
    local q=""
    [ -n "$app" ] && q="app=$(urlencode "$app")"
    [ -n "$window" ] && q="${q:+$q&}window=$(urlencode "$window")"
    _compute_call GET /cli/app/resources/trends "$q"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
BARS = "▁▂▃▄▅▆▇█"
def gib(b):
    if b is None:
        return "—"
    g = float(b) / (1 << 30)
    return ("%.0f GiB" % g) if g >= 10 else ("%.1f GiB" % g)
def cores(c):
    return "—" if c is None else ("%.2f" % c)
def spark(vals, top=None):
    vals = [x for x in vals]
    known = [x for x in vals if x is not None]
    if not known:
        return ""
    hi = top or max(known) or 1
    # 40 columns at most: average neighbours.
    n = len(vals)
    step = max(1, (n + 39) // 40)
    out = ""
    for i in range(0, n, step):
        chunk = [x for x in vals[i:i + step] if x is not None]
        if not chunk:
            out += " "
            continue
        x = sum(chunk) / len(chunk)
        out += BARS[min(len(BARS) - 1, int(x / hi * (len(BARS) - 1) + 0.5))]
    return out
def line(label, pts, summ, budget=None, total=None):
    s = summ or {}
    mem = "mem avg %s peak %s" % (gib(s.get("memAvgBytes")), gib(s.get("memPeakBytes")))
    if budget:
        mem += " of %s budget" % gib(budget)
    print("%-24s cpu avg %s peak %s cores · %s" % (label, cores(s.get("cpuAvgCores")), cores(s.get("cpuPeakCores")), mem))
    c = spark([p.get("cpuCores") for p in pts])
    m = spark([p.get("memPeakBytes") if p.get("memPeakBytes") is not None else p.get("memBytes") for p in pts], budget or total)
    if c:
        print("    cpu %s" % c)
    if m:
        print("    mem %s" % m)
b = v.get("box") or {}
print("CPU and memory, last %s (one point every %s)" % (v.get("window"), "minute" if v.get("resSecs") == 60 else "hour"))
print()
line("box (%s cores)" % (b.get("cores") or "?"), b.get("points") or [], b.get("summary"), None, b.get("memTotalBytes"))
apps = v.get("apps") or []
svcs = v.get("services") or []
if apps or svcs:
    print()
for a in apps:
    o = (a.get("owner") or {}).get("handle") or "server"
    line("%s (%s)" % (a.get("app"), o), a.get("points") or [], a.get("summary"), a.get("memBudgetBytes"))
for s in svcs:
    o = (s.get("owner") or {}).get("handle") or "server"
    line("%s (--cmd, %s)" % (s.get("service"), o), s.get("points") or [], s.get("summary"))
if not (b.get("points") or apps or svcs):
    print("No samples yet: K2 samples once a minute and keeps 24 h of minutes and 30 days of hours.")
print()
print("RAM budgets are shown beside use; nothing here stops an App for using more. — is not measured on this OS.")
'
}

# k2 app resources [--json] [--app A] [--owner AGENT] [--rescan [--wait]]
# k2 app resources settings [--mode on|warn|off] [--warn 85] [--warn-free 20G]
#     [--refuse 95] [--reserve 10G] [--critical 98] [--critical-free 2G] [--json]
# k2 app resources clean [--dry-run] [--json]                       (0.46.4)
# k2 app resources trends [--app A] [--window 24h] [--json]         (0.46.4)
# Box usage and the disk guard (prd-app-resources-disk-guard-v1, 0.46.3).
cmd_app_resources() {
    if [ "${1:-}" = settings ]; then shift; cmd_app_resources_settings "$@"; return; fi
    if [ "${1:-}" = clean ]; then shift; cmd_app_resources_clean "$@"; return; fi
    if [ "${1:-}" = trends ]; then shift; cmd_app_resources_trends "$@"; return; fi
    local json_out=0 app="" owner="" rescan=0 wait=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            --app) app="${2:-}"; shift 2 ;;
            --owner) owner="${2:-}"; shift 2 ;;
            --rescan) rescan=1; shift ;;
            --wait) wait=1; shift ;;
            -h|--help) _app_resources_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) echo "usage: k2 app resources [--json] [--app A] [--owner AGENT] [--rescan [--wait]]" >&2; exit 2 ;;
        esac
    done
    [ "$wait" = 1 ] && [ "$rescan" = 0 ] && { echo "--wait goes with --rescan" >&2; exit 2; }
    if [ "$rescan" = 1 ]; then
        _compute_call POST /cli/app/resources/rescan '{}'
        local started
        started="$(printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
if v.get("started"):
    print("started")
elif v.get("running"):
    print("running")
elif v.get("skipped"):
    print("skipped: " + v["skipped"])
else:
    print("noscanner")')"
        case "$started" in
            started|running) echo "box usage scan: $started" >&2 ;;
            noscanner) echo "box usage scan: no scanner runs on this daemon yet; the numbers below are the disks only" >&2; wait=0 ;;
            *) echo "box usage scan $started (one every 5 minutes at most)" >&2; wait=0 ;;
        esac
        if [ "$wait" = 1 ]; then
            local i running=1
            for i in $(seq 1 120); do
                sleep 5
                _compute_call GET /cli/app/resources ""
                running="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print(1 if (json.load(sys.stdin).get("scan") or {}).get("running") else 0)')"
                [ "$running" = 0 ] && break
            done
            [ "$running" = 0 ] || echo "the scan is still running after 10 minutes; showing what is known" >&2
        fi
    fi
    local q=""
    [ -n "$app" ] && q="app=$(urlencode "$app")"
    [ -n "$owner" ] && q="${q:+$q&}owner=$(urlencode "$owner")"
    _compute_call GET /cli/app/resources "$q"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | _app_resources_print
}

cmd_app_resources_settings() {
    local json_out=0 mode="" warn="" warn_free="" refuse="" reserve="" critical="" critical_free="" changed=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            --mode) mode="${2:-}"; changed=1; shift 2 ;;
            --warn) warn="${2:-}"; changed=1; shift 2 ;;
            --warn-free) warn_free="${2:-}"; changed=1; shift 2 ;;
            --refuse) refuse="${2:-}"; changed=1; shift 2 ;;
            --reserve) reserve="${2:-}"; changed=1; shift 2 ;;
            --critical) critical="${2:-}"; changed=1; shift 2 ;;
            --critical-free) critical_free="${2:-}"; changed=1; shift 2 ;;
            -h|--help) _app_resources_usage; return 0 ;;
            *) echo "Unknown argument: $1 (k2 app resources --help)" >&2; exit 2 ;;
        esac
    done
    case "$mode" in ""|on|warn|off) ;; *) echo "--mode is on, warn or off" >&2; exit 2 ;; esac
    local p
    for p in "$warn" "$refuse" "$critical"; do
        case "$p" in ""|[0-9]|[0-9][0-9]) ;; *) echo "percent lines are whole numbers 1-99 (got $p)" >&2; exit 2 ;; esac
    done
    local sizes
    sizes="$(K2_S_WF="$warn_free" K2_S_RS="$reserve" K2_S_CF="$critical_free" python3 -c '
import os, re, sys
out = []
for k in ("K2_S_WF", "K2_S_RS", "K2_S_CF"):
    s = os.environ.get(k, "").strip()
    if not s:
        out.append("-")
        continue
    m = re.fullmatch(r"(\d+(?:\.\d+)?)\s*([kKmMgGtT]?)(i?[bB])?", s)
    if not m:
        sys.stderr.write("bad size %r (use 20G, 512M or bytes)\n" % s)
        sys.exit(2)
    mult = {"": 1, "k": 1 << 10, "m": 1 << 20, "g": 1 << 30, "t": 1 << 40}[m.group(2).lower()]
    out.append(str(int(round(float(m.group(1)) * mult))))
print(" ".join(out))')" || exit 2
    local wfb rsb cfb
    read -r wfb rsb cfb <<<"$sizes"
    [ "$wfb" = "-" ] && wfb=""
    [ "$rsb" = "-" ] && rsb=""
    [ "$cfb" = "-" ] && cfb=""
    if [ "$changed" = 0 ]; then
        # No flags: print the lines and the mode (anyone who can read the
        # view; changing them is the owner's).
        _compute_call GET /cli/app/resources ""
        _CP_BODY="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print(json.dumps({"ok": True, "guard": json.load(sys.stdin).get("guard")}))')"
    else
        _app_rt_post /cli/app/resources/settings "$(_compute_json K2_CJ_mode="$mode" K2_CJ_warnPct_n="$warn" \
            K2_CJ_warnFreeBytes_n="$wfb" K2_CJ_refusePct_n="$refuse" K2_CJ_reserveBytes_n="$rsb" \
            K2_CJ_criticalPct_n="$critical" K2_CJ_criticalFreeBytes_n="$cfb")"
        [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
    fi
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
g = v.get("guard") or {}
l = g.get("lines") or {}
def gib(b):
    if b is None:
        return "—"
    x = b / 1073741824.0
    return ("%.0f GiB" % x) if abs(x - round(x)) < 0.05 else ("%.1f GiB" % x)
if v.get("changed"):
    print("disk guard saved")
print("mode      %s (%s)" % (g.get("mode"), g.get("modeSource")))
print("warn      %s%% used or under %s free" % (l.get("warnPct"), gib(l.get("warnFreeBytes"))))
print("refuse    %s%% used, or an action that would leave under %s" % (l.get("refusePct"), gib(l.get("reserveBytes"))))
print("critical  %s%% used or under %s free" % (l.get("criticalPct"), gib(l.get("criticalFreeBytes"))))
if v.get("note"):
    print("note: " + v["note"])'
}

_app_resources_usage() {
    cat <<'EOF'
k2 app resources — box usage (not k2 workspace resources, not k2 app usage)

  k2 app resources [--json] [--app A] [--owner AGENT] [--rescan [--wait]]
      Disk per filesystem (used, free, level) and per consumer: each App,
      --cmd service, Postgres database, agent folder, the mail store, K2's
      own files and the agent tools' caches. Every row names its owner and
      the address k2 msg takes. RAM and load too. Unknown shows as —.
      Agents see names and addresses, never folder paths.
      --rescan asks for a fresh folder scan (one at a time, at most every
      5 minutes); --wait waits up to 10 minutes for it.
  k2 app inspect <app> --resources
      One App's row.
  k2 app resources settings [--mode on|warn|off] [--warn 85] [--warn-free 20G]
                            [--refuse 95] [--reserve 10G] [--critical 98]
                            [--critical-free 2G] [--json]
      No flags: print the lines. With flags: change them (the owner only).
  k2 app resources clean [--dry-run] [--json]
      Free only what K2 made and can make again, and print each item:
      unused Seeds (not live, not preview, not one of the App's last 5
      that stayed live, 3 days since last used), unused container images,
      the engine's old leftovers, rotated logs, old deployment logs, build
      temps, old base image archives.
      Never App data, databases, mail, uploads, workspaces or the Trash.
      The owner and Admins clean the box; an agent only its own Apps and
      --cmd services (a real clean needs "Build and run Apps").
      --dry-run lists what it would free and removes nothing.
  k2 app resources trends [--app A] [--window 1h|6h|24h|7d|30d] [--json]
      CPU (cores busy) and memory over time for the box, each App (with its
      RAM budget beside it) and each --cmd service. One sample a minute,
      kept 24 h; hourly averages and peaks kept 30 days. Unknown is —.

The disk guard: before new disk work starts (k2 app build, install, deploy,
data put, a new k2 publish run --cmd service, uploads) K2 checks the disk.
  warn      85% or under 20 GiB free: it goes through, with a warning.
  refuse    95%, or the action would leave under 10 GiB: refused (exit 3,
            disk_low). --disk-override "<reason>": the owner's goes through
            once; anyone else's waits for the owner (exit 2, a request).
  critical  98% or under 2 GiB free: every override waits for the owner
            in person.
It never stops a running App, never blocks a restart, start, stop or
rollback, and never deletes App data, databases, mail or folders.
--mode off is today's behaviour; K2_DISK_GUARD=off|warn|on in the daemon's
environment wins over the setting.

Exit codes: 0 ok, 2 usage error or held for the owner, 3 refused.
More: k2 study app-resources.
EOF
}

_app_data_usage() {
    cat <<'EOF'
k2 app data — an App's data folder ($K2_APP_DATA, /data in its wall)

Your App's pages write to /data (answers, uploads); read them back here.
Paths are inside the data folder: relative (var/answers.json) or as the App
sees them (/data/var/answers.json). Never .., never through a symlink. A
walled App must be running (these go through its own exec, as the App).

  k2 app data put <app-id> <path> [--to <dir>] [--json]
                                       Copy a file or folder of your workspace
                                       into the data folder (64 MiB compressed
                                       per put; any bytes: PDFs, images, tars).
  k2 app data ls <app-id> [<path>] [--json]
                                       List a folder (at most 2000 entries) or
                                       show one file. Links show as links,
                                       never followed.
  k2 app data get <app-id> <path> [--to <local path> | -] [--json]
                                       Read a file back (256 MiB at most), in
                                       512 KiB pieces into a temp file, then
                                       renamed into place. Default: ./<name>;
                                       --to a folder: into it; - : stdout.
  k2 app data rm <app-id> <path> [--recursive] [--json]
                                       Remove a file (a link: the link only). A
                                       folder needs --recursive.
  k2 app data mv <app-id> <from> <to> [--force] [--json]
                                       Rename or move inside the data folder
                                       (missing folders are made). Never
                                       replaces a file without --force, never
                                       lands inside or replaces a folder.

Who: the App's workspace agent with "Build and run Apps" (without it, rm and
mv wait for your owner; ls and get are refused), the owner, an Admin.

Exit codes: 0 done, 2 usage error or held for your owner, 3 refused (JSON on
stderr: data_not_found, data_symlink, data_is_folder, data_exists,
data_too_large, app_not_running, …).
EOF
}

