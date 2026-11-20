# Is this shell inside a K2 terminal (a k2-ptyd or k2-daemon ancestor)?
# CE-9: an agent there whose K2_* passport vars went missing falls back to
# the owner token silently; the submit says so (a warning, never a refusal).
_compute_in_k2_tree() {
    local pid="$PPID" i=0 line comm
    while [ -n "$pid" ] && [ "$pid" -gt 1 ] 2>/dev/null && [ "$i" -lt 40 ]; do
        line="$(ps -o ppid=,comm= -p "$pid" 2>/dev/null)" || return 1
        [ -n "$line" ] || return 1
        comm="${line#"${line%%[![:space:]]*}"}"
        comm="${comm#*[[:space:]]}"
        case "$comm" in *k2-ptyd*|*k2-daemon*) return 0 ;; esac
        pid="$(printf '%s' "$line" | awk '{print $1}')"
        i=$((i + 1))
    done
    return 1
}

# _compute_send METHOD PATH [payload] [tcp] → `<body>\n<status>` on stdout.
# GET payload = urlencoded query; POST payload = JSON body.
#
# 0.46.15: a POST goes again over TCP after the cell socket only when the
# socket took none of it: the connect failed (curl 7) or the daemon
# answered "not served on cell socket", as cli_send_json. Any other
# failure (a timeout, an empty or broken reply, status 000) may have run
# it, so it is not sent again: _cli_send_unanswered says so and ends the
# command with 1. GETs keep the fallback.
_compute_send() {
    local method="$1" path="$2" payload="${3:-}" force_tcp="${4:-}"
    local sock out="" _K2_CFG="" rc=0 t0="$SECONDS"
    if [ "$method" = "POST" ] && [ "$path" = "/cli/compute/run" ] && [ -z "${_CP_OWNER_WARNED:-}" ] && ! _compute_in_session && _compute_in_k2_tree; then
        _CP_OWNER_WARNED=1
        echo "[k2 compute] no passport in this shell: this job is submitted as the owner (its jobs show via: owner_token). Keep K2_* set in agent shells." >&2
    fi
    sock="$(_hook_sock)"
    if [ "$force_tcp" != "tcp" ] && [ -n "$sock" ] && [ -S "$sock" ] && ! _cli_skip_uds && [ -n "${SCOPED_TOKEN:-}" ]; then
        _k2_cfg_add header "Authorization: Bearer ${SCOPED_TOKEN}"
        if [ "$method" = "GET" ]; then
            [ -n "$payload" ] && _k2_cfg_add data "$payload"
            out="$(_k2_curl "$_K2_CFG" -s -G --unix-socket "$sock" "http://localhost${path}" \
                --connect-timeout 5 --max-time 60 -w $'\n%{http_code}' 2>/dev/null)" || rc=$?
        else
            _k2_cfg_add data-raw "$payload"
            out="$(_k2_curl "$_K2_CFG" -s -X POST --unix-socket "$sock" "http://localhost${path}" \
                --connect-timeout 5 --max-time "${K2_CP_MAX_TIME:-120}" -H "Content-Type: application/json" \
                -w $'\n%{http_code}' 2>/dev/null)" || rc=$?
        fi
        if [ "$rc" -eq 0 ] && [ -n "$out" ] && [[ "$out" != *"not served on cell socket"* ]] && [[ "$out" != *$'\n000' ]]; then
            printf '%s' "$out"
            return 0
        fi
        # Soft callers (_K2_NOANSWER_SOFT=1) get status 000 back instead.
        if [ "$method" != "GET" ] && [ "$rc" -ne 7 ] && [[ "$out" != *"not served on cell socket"* ]]; then
            _cli_send_unanswered "$path" "$t0" 0 $'\n000'
            return
        fi
    fi
    # TCP: in a session whose cell socket failed, the passport, never the
    # owner token from disk (_k2_sock_fallback_token). `tcp` callers chose
    # TCP: $TOKEN, as before.
    local _K2_TCP_TOKEN="${TOKEN:-}"
    [ "$force_tcp" = "tcp" ] || _k2_sock_fallback_token "$sock" "$out"
    _K2_CFG=""
    if [ "$method" = "GET" ]; then
        _k2_cfg_auth "$_K2_TCP_TOKEN"
        [ -n "$payload" ] && _k2_cfg_add data "$payload"
        _k2_curl "$_K2_CFG" -s -G "${BASE_URL}${path}" \
            --connect-timeout 5 --max-time 60 -w $'\n%{http_code}' 2>/dev/null || printf '\n000'
    else
        _k2_cfg_add url "${BASE_URL}${path}"
        _k2_cfg_auth "$_K2_TCP_TOKEN"
        _k2_cfg_add data-raw "$payload"
        _k2_curl "$_K2_CFG" -s -X POST --connect-timeout 5 --max-time "${K2_CP_MAX_TIME:-120}" \
            -H "Content-Type: application/json" -w $'\n%{http_code}' 2>/dev/null || printf '\n000'
    fi
}

_compute_split() {
    local raw="${1:-}"
    _CP_STATUS="${raw##*$'\n'}"
    _CP_BODY="${raw%$'\n'*}"
    [ "$_CP_STATUS" = "$raw" ] && _CP_BODY=""
    [ -z "$_CP_STATUS" ] && _CP_STATUS="000"
    # 0.46.4 (field report #11): the compute/App transport never noted the
    # refusal's (or hold's) `permission` object, so an agent refused or held
    # on `k2 app install` was never shown its "Ask with: k2 permission
    # request …" line. The flush prints it only on exit 2 or 3.
    _permission_note "$_CP_BODY"
    return 0
}

# Print the refusal and exit: 2 for a bad request, 3 for everything else
# (infra and permission problems; a test failure is never exit 3).
# _compute_fail <status> <body> [not-found exit]: the third argument is the
# exit for a 404 (default 3). `k2 compute get`, `status <job>` and `wait`
# pass 4 (not found). It comes only from the caller, never from the
# environment, so an exported variable can't change a verb's exit code.
# 0.46.7 (prd-tauri-core-0467-v1 TC-19): an App refusal (`_K2_REFUSAL_AREA`
# app) whose id is in the registry also carries `next` (the fix) and
# `explain` ("k2 explain <id>"), and, unless the command asked for --json,
# a `fix: <next>` line follows (before K2's own Ask with: line). Exit codes
# are unchanged.
_compute_fail() {
    local status="$1" body="$2" not_found="${3:-3}" line="" rows=""
    [ "${_K2_REFUSAL_AREA:-}" = app ] && rows="$(_k2_refusals_json 2>/dev/null)"
    if [ -n "$body" ]; then
        line="$(printf '%s' "$body" | K2_RF_ROWS="$rows" K2_RF_JSON="${_K2_ARGS_JSON:-0}" python3 -c '
import json, os, sys
raw = sys.stdin.read()
try:
    v = json.loads(raw)
except Exception:
    print("daemon_error: " + raw[:200]); sys.exit(0)
e = v.get("error") if isinstance(v, dict) else None
if isinstance(e, dict):
    out = {"error": e.get("code", "error"), "hint": e.get("hint", "")}
elif isinstance(e, str) and isinstance(v.get("message"), str):
    # The route floor answers {"error": <code>, "message": <text>}.
    out = {"error": e, "hint": v["message"]}
else:
    out = {"error": "error", "hint": str(e)}
row = None
try:
    reg = json.loads(os.environ.get("K2_RF_ROWS") or "null")
except Exception:
    reg = None
if isinstance(reg, dict) and out["error"] != "error":
    code = out["error"]
    row = next((r for r in reg.get("rows", []) if r["id"] == code), None) or next((r for r in reg.get("rows", []) if code in r.get("aliases", [])), None)
if row:
    out["next"] = row["fix"]
    out["explain"] = "k2 explain " + row["id"]
print(json.dumps(out))
if row and os.environ.get("K2_RF_JSON") != "1":
    print("fix: " + row["fix"])
' 2>/dev/null)"
    fi
    [ -z "$line" ] && line="{\"error\":\"daemon_unreachable\",\"hint\":\"no answer from the K2 daemon (status ${status:-none})\"}"
    echo "$line" >&2
    case "$status" in
        400|413) exit 2 ;;
        # 0.46.1: `k2 compute get <missing>` exits 4 (not found); other
        # verbs keep their documented exit 3.
        404) exit "$not_found" ;;
        *) exit 3 ;;
    esac
}

# GET/POST and fail on a refusal; the body lands in _CP_BODY.
# Print a reader route's answer: its JSON with --json, else the lines the
# daemon wrote for people (`text`).
_compute_text() {
    [ "${1:-0}" = "1" ] && { printf '%s\n' "$_CP_BODY"; return; }
    printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print("\n".join(json.load(sys.stdin).get("text") or ["(this server sent no text: k2 update)"]))'
}

_compute_call() {
    _compute_call_nf 3 "$@"
}

# _compute_call_nf <not-found exit> METHOD PATH [payload] [tcp]: _compute_call
# with its own exit for a 404 (`k2 compute get` passes 4).
_compute_call_nf() {
    local not_found="$1" raw
    shift
    raw="$(_compute_send "$@")"
    _compute_split "$raw"
    [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY" "$not_found"
}

# JSON from KEY=VALUE pairs: _compute_json K2_CJ_a=… (values are strings
# unless the key ends in _n (number), _b (bool) or _j (raw JSON)). A key
# ending in _s is a secret string: K2_CJ_code_s=… becomes "code". An empty
# value leaves its key out; when a key repeats, its last value counts.
_compute_json() {
    # Every value reaches python on stdin (_k2_stdin_secrets), never argv or
    # the environment (ps shows a command line to every user on this computer,
    # and the environment to this user's processes).
    # Secrets (K2_CJ_<name>_s, K2_CJ_<name>Token) come first in the body,
    # then the other keys, each in the order given.
    local _a _pairs=()
    for _a in "$@"; do
        case "$_a" in
            K2_CJ_?*=*) _pairs+=("${_a%%=*}" "${_a#*=}") ;;
        esac
    done
    _k2_stdin_secrets ${_pairs[@]+"${_pairs[@]}"} | python3 -c '
import json, sys
b = sys.stdin.buffer.read()
given = {}
while True:
    k, sep, b = b.partition(b"\0")
    if not sep or not k:
        break
    v, _, b = b.partition(b"\0")
    given[k.decode()[6:]] = v.decode("utf-8", "surrogateescape")
out = {}
for secret in (True, False):
    for key, v in given.items():
        if (key.endswith("_s") or key.endswith("Token")) != secret or v == "":
            continue
        if key.endswith("_s"):
            out[key[:-2]] = v
        elif key.endswith("_n"):
            out[key[:-2]] = float(v) if "." in v else int(v)
        elif key.endswith("_b"):
            out[key[:-2]] = v == "1"
        elif key.endswith("_j"):
            out[key[:-2]] = json.loads(v)
        else:
            out[key] = v
print(json.dumps(out))
'
}

# 2h / 90m / 300s / 300 → seconds.
_compute_secs() {
    # Plain seconds, or <n>h / <n>m / <n>s parts in any mix (1h30m). Only
    # digits ever reach $(( )): anything else is the usage error, never
    # arithmetic on the raw value.
    local v="$1" rest total=0 n
    if [[ "$v" =~ ^[0-9]+$ ]]; then echo "$((10#$v))"; return; fi
    if [[ ! "$v" =~ ^([0-9]+[hms])+$ ]]; then
        echo "usage: bad duration '$v' (use 2h, 90m, 1h30m, 300s)" >&2; exit 2
    fi
    rest="$v"
    while [[ "$rest" =~ ^([0-9]+)([hms])(.*)$ ]]; do
        n=$((10#${BASH_REMATCH[1]}))
        case "${BASH_REMATCH[2]}" in
            h) total=$((total + n * 3600)) ;;
            m) total=$((total + n * 60)) ;;
            s) total=$((total + n)) ;;
        esac
        rest="${BASH_REMATCH[3]}"
    done
    echo "$total"
}

# 16G / 512M / bytes → bytes.
_compute_bytes() {
    local v="$1"
    if [[ ! "$v" =~ ^([0-9]+)([GgMm]?)$ ]]; then
        echo "usage: bad size '$v' (use 16G or 512M)" >&2; exit 2
    fi
    local n=$((10#${BASH_REMATCH[1]}))
    case "${BASH_REMATCH[2]}" in
        G|g) echo $((n * 1073741824)) ;;
        M|m) echo $((n * 1048576)) ;;
        *) echo "$n" ;;
    esac
}

# Is this call carrying a session passport (inside a K2 terminal)?
_compute_in_session() {
    [ -n "${SCOPED_TOKEN:-}" ] && [ "${TOKEN:-}" = "${SCOPED_TOKEN:-}" ]
}

# The workspace to name: explicit --workspace, else this checkout (only
# outside a session; a passport's workspace comes from the passport).
_compute_ws() {
    local explicit="${1:-}"
    if [ -n "$explicit" ]; then
        printf '%s' "$explicit"
    elif ! _compute_in_session; then
        printf '%s' "$PROJECT"
    fi
}

_compute_render() {
    local verb="$1"
    printf '%s' "$_CP_BODY" | K2_CP_VERB="$verb" python3 -c '
import json, os, sys, time
v = json.load(sys.stdin)
verb = os.environ["K2_CP_VERB"]
def gb(n):
    return "%d GB" % round((n or 0) / (1 << 30))
def cores(m):
    return "%g" % ((m or 0) / 1000)
def ago(t):
    if not t: return "-"
    d = int(time.time()) - int(t)
    return "now" if d < 60 else ("%dm ago" % (d // 60) if d < 3600 else ("%dh ago" % (d // 3600) if d < 86400 else "%dd ago" % (d // 86400)))
def node_line(n):
    o = n.get("offer") or {}
    st = n.get("state")
    if st == "pending":
        status = "pending (code %s)" % (n.get("sas") or "?")
    elif not n.get("online"):
        status = "offline"
    else:
        status = "online/%s" % (n.get("liveRoute") or "?")
        if o.get("control") and o["control"] != "active":
            status = "draining for update" if (o.get("controlBy") or "").startswith("k2-node node.update") else o["control"] + (" by " + o["controlBy"] if o.get("controlBy") else "")
        elif o.get("refusal"):
            status = "waiting: " + o["refusal"].get("code", "")
        elif o.get("available") is False:
            status = "unavailable: " + ",".join(o.get("unavailable") or [])
    if n.get("controllerPause"):
        status += " (server: %s)" % n["controllerPause"]
    if n.get("pinnedOnly"):
        status += " (pinned only)"
    # 0.46.9 (protocol 7): the short platform column (ubuntu24.04-x86_64,
    # arch-x86_64, macos15.5-arm64); an older node: linux-x86_64 as before.
    os_arch = n.get("platform") or "%s-%s" % (n.get("os") or "?", n.get("arch") or "?")
    caps = o.get("caps") or {}
    free = o.get("free") or {}
    # free CPU = the cap minus what running K2 jobs reserved (not idle CPU);
    # free memory = the smaller of cap minus reserved and what the OS has.
    # 0.46.7 (CE-8): idle CPU (what the OS sees free) beside unreserved.
    idle = (" · idle ~%s" % cores(n["idleCpuMillis"])) if n.get("idleCpuMillis") is not None else ""
    # The room is that of the node, for every server: CPUs no running job
    # of any server reserved, memory free, and (when full) whose jobs fill
    # it. Jobs of other servers are only counted, never named. (No
    # apostrophes in here: this Python sits in single quotes.)
    room = n.get("room") if isinstance(n.get("room"), dict) else None
    busy = {"other_servers": "busy (other servers) · ", "servers": "busy (this and other servers) · ", "this_server": "busy (this server) · "}.get((room or {}).get("busy") or "", "")
    res = ("%sall servers: cpu %s/%s unreserved%s  mem %.1f/%s free  disk %s" % (
        busy, cores(free.get("cpu_millis")), cores(caps.get("cpu_millis")), idle,
        (free.get("mem_bytes") or 0) / (1 << 30), gb(caps.get("mem_bytes")), gb(free.get("disk_free_bytes")))) if o else "-"
    par = o.get("parallelMax")
    # Whose each count is: runs = the node itself (jobs of every server),
    # then this server, your workspace and other servers. queued = jobs
    # of this server waiting there (yours = this workspace only).
    mine = n.get("mine") if isinstance(n.get("mine"), dict) else None
    yours = lambda k: (", yours %d" % mine.get(k, 0)) if mine else ""
    if room:
        others = (", other servers %d" % room["otherServers"]) if room.get("otherServers") else ""
        run = "runs %d/%s (this server %d%s%s)" % (room.get("running", 0), par if par is not None else "?", n.get("running", 0), yours("running"), others)
    else:
        run = "this server runs %d (of %s slots%s)" % (n.get("running", 0), par if par is not None else "?", yours("running"))
    queued = "queued %d (this server%s)" % (n.get("queued", 0), yours("queued"))
    g = n.get("grant")
    grant = ("  grant %s/job %s parallel %s queued%s" % (dur(g.get("maxJobSecs")), g.get("maxParallel"), g.get("maxQueued"), " ci" if g.get("allowCi") else "")) if isinstance(g, dict) else ""
    reboot = "  reboot pending" if n.get("rebootPending") else ""
    return "%-14s %-19s %-24s %-64s  %s · %s%s%s  seen %s%s" % (n["name"], os_arch, status[:24], res, run, queued, tier_counts(n.get("queueTiers")), grant, ago(n.get("lastSeen")), reboot)
def tier_counts(t, bare=False):
    # Tiers (0.46.5): `CI 1 · P 0 · N 3`; nothing when none are queued.
    if not isinstance(t, dict) or not any(t.get(k) for k in ("ci", "priority", "normal", "batch")):
        return ""
    line = "CI %d · P %d · S %d" % (t.get("ci") or 0, t.get("priority") or 0, t.get("normal") or 0)
    if t.get("batch"):
        line += " · B %d" % t["batch"]
    return line if bare else " (%s)" % line
def tier_tag(t):
    return {"ci": "CI", "priority": "P", "batch": "B"}.get(t or "", "")
def dur(s):
    s = int(s or 0)
    if s >= 3600 and s % 3600 == 0: return "%dh" % (s // 3600)
    if s >= 60 and s % 60 == 0: return "%dm" % (s // 60)
    return "%ds" % s
def update_lines(n):
    # Only while an update is under way (§5.13), or a node needs a hand install.
    u = n.get("update") if isinstance(n.get("update"), dict) else {}
    r = n.get("rollout") if isinstance(n.get("rollout"), dict) else {}
    have = n.get("nodeVersion") or "?"
    d = u.get("drain") if u.get("state") == "staged" and isinstance(u.get("drain"), dict) else None
    end = ("longest ends by " + time.strftime("%b %d %H:%M", time.localtime(d["jobsEndBy"]))) if d and d.get("jobsEndBy") else "end time unknown"
    phase = (": draining for update (%s jobs running, %s)" % (d.get("running"), end)) if d else {"offered": " (sent)", "staged": " (staged)", "swapped": " (health check)"}.get(u.get("state") or "")
    if phase and u.get("version") and u.get("version") != have:
        return ["%-14s k2-node v%s -> %s%s" % ("", have, u["version"], phase)]
    if r.get("code") in ("waiting_turn", "waiting_idle") and r.get("node") == "pending" and r.get("state") == "running":
        return ["%-14s k2-node v%s -> %s (update staged, %s)" % ("", have, r.get("toVersion") or "?", "waiting its turn" if r["code"] == "waiting_turn" else "waiting for its running jobs to end")]
    if r.get("code") == "node_needs_manual_install":
        return ["%-14s k2-node v%s: needs one manual install (the updater fix is in 0.46.4): sudo k2-node setup at the machine" % ("", have)]
    if r.get("node") == "rolled_back":
        return ["%-14s k2-node %s rolled back here; the rollout paused (k2 compute node rollout)" % ("", r.get("toVersion") or "")]
    return []
def reserve_lines(n):
    # 0.46.16: which jobs of this server hold the reservation of the node (a
    # node with no free CPU and a live job is busy, not stuck), and how
    # many jobs of other servers run there (counted, never named).
    rows = n.get("reservedBy") if isinstance(n.get("reservedBy"), list) else []
    room = n.get("room") if isinstance(n.get("room"), dict) else {}
    others = room.get("otherServers") or 0
    if not rows and not others:
        return []
    def one(r):
        bits = [r.get("state") or "?"]
        if r.get("startedAt"):
            bits.append("started " + ago(r["startedAt"]))
        elif r.get("createdAt"):
            bits.append("queued " + ago(r["createdAt"]))
        if r.get("cpuMillis"):
            bits.append("%s CPUs" % cores(r["cpuMillis"]))
        if r.get("memBytes"):
            bits.append("%.1f GB" % (r["memBytes"] / (1 << 30)))
        return "%s (%s)" % (r.get("short") or "?", ", ".join(bits))
    shown = [one(r) for r in rows[:4]]
    if len(rows) > 4:
        shown.append("%d more (k2 compute jobs --node %s)" % (len(rows) - 4, n.get("name")))
    if others:
        shown.append("%d job(s) of other servers" % others)
    return ["%-14s reserved by %s" % ("", ", ".join(shown))]
def leftover_lines(n):
    # 0.46.16: processes of the job user that no live job owns (the last
    # health report of the node). Reported, never killed by the check.
    l = n.get("leftovers") if isinstance(n.get("leftovers"), dict) else None
    if not l or not l.get("count"):
        return []
    def age(s):
        if s is None: return "?"
        return "%dd" % (s // 86400) if s >= 86400 else ("%dh" % (s // 3600) if s >= 3600 else "%dm" % (s // 60))
    out = ["%-14s %d leftover process(es) of the job user with no live job (not killed; the owner ends them at the machine)" % ("", l["count"])]
    for p in (l.get("procs") or [])[:3]:
        out.append("%-14s   pid %s, %s old%s: %s" % ("", p.get("pid"), age(p.get("age_secs")), (", from job %s" % p["job_id"][:8]) if p.get("job_id") else "", (p.get("cmd") or "")[:90]))
    return out
def when_col(j):
    # 0.46.16: from the real start (not the submission), and the queued time.
    if j.get("startedAt"):
        w = "started %s" % ago(j["startedAt"])
        if j.get("createdAt"):
            w += " (queued %s)" % ago(j["createdAt"])
        return w
    return "queued %s" % ago(j.get("createdAt")) if j.get("createdAt") else "-"
def share_lines(n):
    # Shared nodes only (0.46.3); a solo node prints exactly as before.
    s = n.get("share")
    if not isinstance(s, dict):
        return []
    until = ""
    if s.get("expiresAt"):
        until = "   until " + time.strftime("%b %d", time.localtime(s["expiresAt"]))
    if s.get("role") == "tenant":
        head = "shared by %s (%s servers)%s" % (s.get("ownerLabel") or "?", s.get("servers") or "?", until)
    else:
        head = "shared with %d other server(s)" % max((s.get("servers") or 1) - 1, 0)
    if s.get("state") and s["state"] != "active":
        head += "   your share: " + s["state"]
    if s.get("skewSecs"):
        head += "   clock skew %+ds (node vs this server)" % s["skewSecs"]
    out = ["%-14s %s" % ("", head)]
    if s.get("line"):
        # The jobs of this server against the limits of its share, not
        # the load of the node (the node line above has that, all servers).
        out.append("%-14s your share (jobs of this server only): %s" % ("", s["line"]))
    if s.get("priority") is False:
        out.append("%-14s your --tier priority runs as Normal here (its owner has not set priority=true for this server)" % "")
    return out
if verb == "nodes":
    rows = v.get("nodes", [])
    # Tiers (0.46.5): who is on the critical path on this server, so a
    # coordinator sees it before choosing (a courtesy, never enforced).
    for c in v.get("criticalPath") or []:
        print("critical path (Priority): %s (%s jobs)%s" % (c.get("submitter") or "?", c.get("jobs"), (" in %s" % c["workspace"]) if c.get("workspace") else ""))
    if not rows:
        print("(no compute nodes%s)" % ("" if v.get("waitingCodes") is None else "; add one with: k2 compute node add <name>"))
    for n in rows:
        # 0.46.8: a node off the node list of this workspace says so.
        print(node_line(n) + ("  [off the node list]" if n.get("allowed") is False else ""))
        for l in reserve_lines(n) + leftover_lines(n):
            print(l)
        for l in share_lines(n):
            print(l)
        for l in update_lines(n):
            print(l)
    for c in v.get("waitingCodes") or []:
        print("code waiting for %s (expires %s)" % (c["name"], time.strftime("%H:%M", time.localtime(c["expiresAt"]))))
    if isinstance(v.get("allowNodes"), list):
        print("node list (%s): jobs go only to %s (k2 compute allow-nodes)" % (v.get("allowNodesWorkspace") or "this workspace", ", ".join(v["allowNodes"]) or "nothing"))
elif verb == "info":
    print(node_line(v))
    for l in reserve_lines(v) + leftover_lines(v):
        print(l)
    for l in share_lines(v):
        print(l)
    # 0.46.9 (protocol 7): what the machine is, and its package manager.
    h = v.get("host")
    if isinstance(h, dict) and h.get("line"):
        print("host: " + h["line"])
    pk = v.get("packages")
    if isinstance(pk, dict):
        n_open = pk.get("openRequests") or 0
        bits = [pk.get("manager") or "?"]
        if pk.get("policy"):
            bits.append("policy for this server: " + pk["policy"])
        bits.append("%d open request%s" % (n_open, "" if n_open == 1 else "s"))
        print("packages: %s (k2 compute node packages %s)" % (" · ".join(bits), v.get("name")))
    o = v.get("offer") or {}
    nv = v.get("nodeVersion") or o.get("node_version")
    if nv or o.get("features") is not None:
        # 0.46.6: what the node supports follows its k2-node version
        # (Priority 0.46.5+; lines, turns, Batch, the per-submitter cap 0.46.6+).
        feats = o.get("features") or []
        if not v.get("shared"):
            what = "this server orders its queue: every line works"
        elif "tiers2" in feats:
            what = "shared; the node orders: lines, turns, Batch"
        elif "tiers" in feats:
            what = "shared; the node orders: Priority only (Batch waits for an idle node; no turns until k2-node 0.46.6)"
        else:
            what = "shared; the node orders without tiers (Priority has no effect there)"
        print("k2-node %s (protocol %s): %s" % (nv or "?", o.get("protocol", "?"), what))
    if o.get("tools"):
        print("tools: " + "  ".join("%s %s" % kv for kv in sorted(o["tools"].items())))
    if v.get("routes"):
        print("routes: " + " ".join(v["routes"]))
    for g in v.get("grants") or []:
        print("grant: %s  %s/job  %s parallel%s  %s queued  %s GB disk%s" % (g.get("workspace"), dur(g["maxJobSecs"]), g["maxParallel"], ("  %s per submitter" % g["submitterMaxRunning"]) if g.get("submitterMaxRunning") else "", g["maxQueued"], g["maxDiskGb"], "  CI allowed" if g.get("allowCi") else ""))
    for j in v.get("recentJobs") or []:
        print("job %s  %-9s exit %-4s %s" % (j["short"], j["state"], "-" if j.get("exitCode") is None else j["exitCode"], " ".join(j.get("argv") or [])[:60]))
elif verb == "jobs":
    rows = v.get("jobs", [])
    if not rows:
        print("(no jobs)")
    for j in rows:
        # 0.46.1: the full reason (it was cut to 10 characters).
        # Tiers (0.46.5): the TIER column (CI, P, blank for Normal).
        # 0.46.7 (CE-9): the --tag of the job and who sent it ("(owner token)"
        # when the passport of an agent was missing and the owner token went).
        who = j.get("submitter") or j.get("submittedBy") or "-"
        if j.get("via") == "owner_token":
            who += " (owner token)"
        # 0.46.8: a queued job says pinned (a node named; k2 compute move
        # <id> --auto releases it) or auto.
        state = j["state"]
        if state == "queued" and "pinned" in j:
            state += " (pin)" if j["pinned"] else " (auto)"
        # 0.46.16: WHEN = started (the real start) and queued.
        print("%s  %-2s %-12s %-15s exit %-4s %-22s %-34s %-16s %-24s %s" % (j["short"], tier_tag(j.get("tier")), (j.get("node") or "?")[:12], state, "-" if j.get("exitCode") is None else j["exitCode"], j.get("reason") or "", when_col(j), (j.get("tag") or "-")[:16], who[:24], " ".join(j.get("argv") or [])[:60]))
elif verb == "get":
    j = v
    print("job %s on %s: %s%s" % (j["short"], j.get("node"), j["state"], (" (%s)" % j["reason"]) if j.get("reason") else ""))
    # 0.46.7: a long command (an inline base64 script) is summarized;
    # --full prints all of it.
    argv = " ".join(j.get("argv") or [])
    if os.environ.get("K2_CP_FULL") == "1" or len(argv) <= 160:
        print("  argv: %s" % argv)
    else:
        print("  argv: %s… (%d chars; --full for all of it)" % (argv[:120], len(argv)))
    # Tiers (0.46.5): shown when not Normal, or when the server lowered it.
    ctl = j.get("ctl") or {}
    if j.get("tierAsked") and j.get("tierAsked") != j.get("tier"):
        print("  tier %s (asked %s: downgraded)" % (j.get("tier") or "normal", j["tierAsked"]))
    elif j.get("tier") and j["tier"] != "normal":
        print("  tier %s" % j["tier"])
    if ctl.get("why"):
        print("  why (Priority): %s" % ctl["why"])
    if ctl.get("memDefaulted"):
        print("  memory: the default reservation (no --mem); set --mem next time")
    if j.get("tag"):
        print("  tag: %s" % j["tag"])
    # 0.46.7 (CE-9): who sent it; the owner token said so.
    by = j.get("submittedBy")
    if by or j.get("submitter"):
        print("  submitted by: %s%s%s" % (by or "-", (" as %s" % j["submitter"]) if j.get("submitter") else "", " (owner token: no passport in that shell)" if j.get("via") == "owner_token" else ""))
    if j.get("supersedeKey"):
        print("  supersede key: %s" % j["supersedeKey"])
    if j.get("secrets"):
        # Secret-env: the env names it gets from the vault, never values.
        print("  secrets (env names): %s" % ", ".join(j["secrets"]))
    nw = j.get("network")
    if isinstance(nw, dict):
        # 0.46.14: the job asked for --network none (and dummy interfaces).
        print("  network: %s (asked by the job: loopback%s only, whatever the node setting)" % (nw.get("line"), " and its dummy interfaces" if nw.get("dummies") else ""))
    pl = j.get("placement") or ctl.get("placement") or {}
    if pl.get("mode") == "auto":
        # Auto-placement (0.46.6): where K2 put it and why (a move says so).
        code = pl.get("code") or ""
        how = ("moved: " + code[len("moved_"):]) if code.startswith("moved_") else "auto"
        print("  placed on %s (%s, %s): %s%s%s" % (pl.get("node"), how, pl.get("policy"), pl.get("why"), (" · " + pl["fits"]) if pl.get("fits") else "", (" · avoids " + ", ".join(j["avoid"])) if j.get("avoid") else ""))
    if (j.get("move") or {}).get("note"):
        # A waiting job: may K2 move it, and how its last tries went.
        print("  move: %s" % j["move"]["note"])
    # 0.46.16: when it was queued, started (the real start, not the
    # submission) and ended.
    hms = lambda t: time.strftime("%m-%d %H:%M:%S", time.localtime(t))
    times = ["queued %s (%s)" % (hms(j["createdAt"]), ago(j["createdAt"]))] if j.get("createdAt") else []
    if j.get("startedAt"):
        times.append("started %s (%s)" % (hms(j["startedAt"]), ago(j["startedAt"])))
    elif j.get("state") not in ("queued", "assigned", "preparing"):
        times.append("never started")
    if j.get("endedAt"):
        times.append("ended %s (%s)" % (hms(j["endedAt"]), ago(j["endedAt"])))
    if times:
        print("  " + " · ".join(times))
    print("  commit: %s  tree: %s" % (j.get("srcCommit") or j.get("submitCommit") or "-", j.get("srcTree") or "-"))
    print("  exit: %s  signal: %s  attempt %s (generation %s)" % (j.get("exitCode"), j.get("signal"), j.get("attempt"), j.get("generation")))
    rows = j.get("attempts") or []
    if len(rows) > 1:
        # Every attempt: a retry or move K2 made on its own says why.
        hm = lambda t: time.strftime("%m-%d %H:%M", time.localtime(t)) if t else "?"
        print("  attempts (%d):" % len(rows))
        for a in rows:
            when = ("%s to %s" % (hm(a.get("startedAt")), "now" if a.get("current") and not a.get("endedAt") else hm(a.get("endedAt")))) if a.get("startedAt") else "never started"
            ended = ("%s/%s" % (a["state"], a["reason"]) if a.get("reason") else a["state"])
            line = "    attempt %s · generation %s · %s · %s%s" % (a["attempt"], a["generation"], a.get("node") or "?", when, "  (this one)" if a.get("current") else "")
            print(line)
            if a.get("why") and not a.get("current"):
                print("      %s: %s" % (ended, a["why"]))
            if a.get("next"):
                print("      then: %s" % a["next"])
            lg, ar = a.get("log") or {}, a.get("artifacts") or {}
            if not a.get("current") and lg.get("available"):
                print("      log: %d KB (k2 compute logs %s --attempt %s)" % ((lg.get("bytes") or 0) // 1024, j["short"], a["attempt"]))
            if not a.get("current") and ar.get("status") in ("kept", "not_collected", "none_matched"):
                print("      artifacts: %s%s" % ("%d file(s) (k2 compute fetch %s --attempt %s)" % (len(ar.get("files") or []), j["short"], a["attempt"]) if ar["status"] == "kept" else ar["status"].replace("_", " "), (" · " + ar["note"]) if ar.get("note") and ar["status"] != "kept" else ""))
    if j.get("queue") and j["queue"].get("onNode"):
        # In the queue of a shared node: its place and ETA (0.46.3).
        q = j["queue"]
        print("  %s%s" % (q["message"], ("  [class %s]" % j["class"]) if j.get("class") else ""))
        if q.get("reason") and q["reason"] != "node_queued":
            print("  waiting for the node to answer: %s" % q["reason"])
    elif j.get("queue"):
        q = j["queue"]
        at = q.get("aheadTiers") or {}
        behind = ", ".join(x for x in [("%d CI" % at["ci"]) if at.get("ci") else "", ("%d Priority" % at["priority"]) if at.get("priority") else ""] if x)
        if q.get("line"):
            # 0.46.6 (L6): where it waits, N of M, why, and whether jobs
            # behind it may start first.
            print("  %s%s" % (q["line"], ("  [behind %s]" % behind) if behind else ""))
        else:
            print("  queued: position %s%s%s — %s (%s)" % (q["position"], (" of %s" % q["of"]) if q.get("of") else "", (", behind " + behind) if behind else "", q["reason"], q["message"]))
        if q.get("parallel"):
            print("  the node runs %s job(s) from this workspace at a time" % q["parallel"])
        for a in q.get("ahead") or []:
            age = a.get("elapsedSecs") if a.get("elapsedSecs") is not None else a.get("ageSecs")
            print("  ahead: %-9s %-9s %-14s %s  %s%s" % (
                a.get("short") or "-", a.get("state"), a.get("workspace") or "(other)",
                ("%dm%02ds" % (age // 60, age % 60)) if age is not None else "-",
                a.get("argv") or "", "  (" + a["reason"] + ")" if a.get("reason") else ""))
    wt = j.get("wait") or {}
    if wt and not j.get("queue") and wt.get("words"):
        # 0.46.7 (CE-7): every unfinished job says where it is.
        print("  %s%s" % (wt["words"], (", ~%s left (%s)" % (dur(wt["etaSecs"]), wt.get("etaBasis"))) if wt.get("etaSecs") is not None else ""))
    elif wt.get("etaSecs") is not None and j.get("queue") and not j["queue"].get("onNode"):
        print("  ETA ~%s (estimate from earlier runs)" % dur(wt["etaSecs"]))
    stl = j.get("stall") or {}
    if "stallSecs" in stl:
        # CE-10: the stall watch of a running job.
        print("  stall watch: %s" % (("ends after %s of silence (--stall-timeout)%s" % (dur(stl["stallSecs"]), "; warned" if stl.get("warned") else "")) if stl.get("stallSecs") else "off"))
    rl = j.get("reportedLate") or {}
    if rl.get("secs"):
        # CE-5: the end time of the node itself; this server heard of it late.
        hm = lambda t: time.strftime("%H:%M", time.localtime(t)) if t else "?"
        print("  the node finished at %s; this server heard at %s (%s late: it was asleep, or the node was cut off). Durations use the times of the node." % (hm(rl.get("nodeEndedAt")), hm(rl.get("heardAt")), ("%dh%02dm" % (rl["secs"] // 3600, (rl["secs"] % 3600) // 60)) if rl["secs"] >= 3600 else ("%dm" % (rl["secs"] // 60))))
    if j.get("hint"):
        print("  hint: %s" % j["hint"])
    cb = j.get("cancelledBy")
    if isinstance(cb, dict):
        print("  cancel asked by %s at %s" % (cb.get("by"), time.strftime("%H:%M:%S", time.localtime(cb.get("at") or 0))))
    r = j.get("result") or {}
    if j.get("reason") == "tenant_disk_full" and r.get("detail"):
        print("  " + r["detail"])
    for n in r.get("notes") or []:
        if n.startswith("evicted ") or n.startswith("network: no-lan proxy: ") or n.startswith("killed "):
            print("  " + n)
    nh = r.get("networkHosts") or []
    if nh:
        # Mac no-lan: every host the job reached or tried through its proxy.
        def hb(n):
            n = n or 0
            return "%d B" % n if n < 1024 else "%.0f KB" % (n / 1024) if n < 1 << 20 else "%.1f MB" % (n / (1 << 20)) if n < 1 << 30 else "%.1f GB" % (n / (1 << 30))
        cnt = lambda v: sum(1 for h in nh if h.get("verdict") == v)
        print("  hosts through the no-lan proxy (%d allowed, %d refused%s):" % (cnt("allowed"), cnt("refused"), (", %d unreachable" % cnt("failed")) if cnt("failed") else ""))
        for h in nh[:40]:
            hp = ("[%s]:%s" if ":" in (h.get("host") or "") else "%s:%s") % (h.get("host"), h.get("port"))
            if h.get("verdict") == "allowed":
                print("    allowed  %-36s %4d conn(s)  in %-9s out %s" % (hp, h.get("conns") or 0, hb(h.get("bytesIn")), hb(h.get("bytesOut"))))
            else:
                why = ", ".join(x for x in [h.get("addr") if h.get("addr") != h.get("host") else None, h.get("reason")] if x)
                print("    %-7s  %-36s %4d try(s)   %s" % (h.get("verdict"), hp, h.get("conns") or 0, why))
        if len(nh) > 40:
            print("    and %d more (k2 compute get %s --json)" % (len(nh) - 40, j["short"]))
    t = r.get("tests")
    if t:
        print("  tests: %d passed, %d failed, %d errors, %d skipped" % (t["passed"], t["failed"], t["errors"], t["skipped"]))
    for a in r.get("artifacts") or []:
        print("  artifact: %s  %d bytes  sha256 %s…" % (a["name"], a["bytes"], a["sha256"][:12]))
    if r.get("receipt"):
        print("  receipt: %s (%s)" % (r["receipt"].get("id"), "verified" if r["receipt"].get("verified") else "NOT verified"))
    tl = r.get("tools") or {}
    if tl.get("versions"):
        # CE-4: exact versions (what the job used; node-probe = an older
        # node-wide probe of an older node, not necessarily what the job used).
        print("  tools%s: %s" % (" (node-wide probe, older k2-node)" if tl.get("basis") == "node-probe" else "", ", ".join("%s %s" % kv for kv in sorted(tl["versions"].items()))))
    rs = r.get("resources") or {}
    if rs:
        # CE-14: peak memory and CPU time.
        pk, lim = rs.get("peakMemBytes"), rs.get("memLimitBytes")
        print("  resources: peak memory %s%s (%s), CPU time %s (%s)" % (
            "-" if pk is None else "%.1fG" % (pk / (1 << 30)), "" if lim is None else " of %.1fG" % (lim / (1 << 30)), rs.get("memSource") or "-",
            "-" if rs.get("cpuSecs") is None else "%ss" % round(rs["cpuSecs"]), rs.get("cpuSource") or "-"))
        g = rs.get("gpu")
        if isinstance(g, dict):
            # GPU jobs S4: the GPU of the job, the whole device (host use included).
            gb = lambda b: "%.1fG" % (b / (1 << 30))
            bits = ["busy avg %s%% (max %s%%)" % (g.get("busyAvgPct"), g.get("busyMaxPct")) if g.get("busyAvgPct") is not None else "busy % not reported"]
            if g.get("vramPeakBytes") is not None: bits.append("VRAM +" + gb(g["vramPeakBytes"]))
            if g.get("gttPeakBytes") is not None: bits.append("GTT +" + gb(g["gttPeakBytes"]))
            print("  gpu: %s: %s (whole device, %s)" % (g.get("model") or g.get("pci"), ", ".join(bits), g.get("source") or "-"))
            # GPU jobs S6: the GPU memory of the job itself (DRM fdinfo) and its budget.
            if g.get("memLimitBytes") is not None:
                own = "peak " + gb(g["jobMemPeakBytes"]) if g.get("jobMemPeakBytes") is not None else "not read"
                print("  gpu memory (the job itself): %s of --gpu-mem %s" % (own, gb(g["memLimitBytes"])))
    hs = r.get("host") or {}
    if hs:
        # CE-19: the load of the node at the start and end (other K2 jobs, steal).
        def stamp(s):
            if not s: return "-"
            return "load %s of %s CPUs, %s other job(s)%s%s%s" % (s.get("load1"), s.get("cpus"), s.get("jobs"), (", steal %s%%" % s["steal"]) if s.get("steal") is not None else "",
                (", GPU %s%% busy" % s["gpuBusy"]) if s.get("gpuBusy") is not None else "",
                (", quiet" if s["quiet"] else (", not quiet (%s)" % s["quietWhy"]) if s.get("quietWhy") else ", not quiet") if s.get("quiet") is not None else "")
        print("  node at start: %s · at end: %s" % (stamp(hs.get("start")), stamp(hs.get("end"))))
    eff = r.get("effective")
    if isinstance(eff, dict) and eff:
        # CE-3: the effective config (each setting and where it came from).
        print("  effective: %s" % json.dumps(eff, separators=(",", ":"))[:300])
    for x in r.get("shards") or []:
        t = x.get("tests")
        print("  shard %s/%d: job %s on %s · %s%s%s" % (x["index"], len(r["shards"]), x["short"], x.get("node") or "?", x["state"],
              (" exit %s" % x["exit"]) if x.get("exit") is not None else "",
              (" · %d passed, %d failed" % (t["passed"], t["failed"] + t["errors"])) if t else ""))
elif verb == "receipt":
    print(json.dumps(v.get("receipt"), indent=2))
    print("verified: %s  (%s)" % ("yes" if v.get("verified") else "NO", v.get("proves")))
elif verb == "grants":
    rows = v.get("grants", [])
    if not rows:
        print("(no grants)")
    for g in rows:
        print("%-14s %-20s max %ss/job  %s parallel  %s queued  %s GB disk%s  by %s" % (g.get("node"), g.get("workspace"), g["maxJobSecs"], g["maxParallel"], g["maxQueued"], g["maxDiskGb"], "  ci" if g.get("allowCi") else "", g["grantedBy"]))
elif verb == "usage":
    rows = v.get("usage", [])
    if not rows:
        print("(no traffic in this window)")
    for u in rows:
        print("%s  %-14s %-9s in %8.1f MB  out %8.1f MB" % (u["day"], u.get("node") or u["nodeId"][:8], u["route"], u["bytesIn"] / 1e6, u["bytesOut"] / 1e6))
    print(v.get("metered", ""))
elif verb == "events":
    for e in v.get("events", []):
        print("%s  %-16s %-18s %s %s" % (time.strftime("%m-%d %H:%M:%S", time.localtime(e["at"])), e["kind"], e["actor"][:18], (e.get("jobId") or e.get("nodeId") or "")[:8], json.dumps(e.get("detail"))[:80]))
elif verb == "queue":
    rows = v.get("entries") or []
    print("%s%s" % (v.get("node"), "" if v.get("from") == "node" else "  (jobs from this server only: the node keeps no shared queue)"))
    if not rows:
        print("(nothing running or queued)")
    else:
        print("%-3s %-34s %-4s %-12s %-8s %-5s %-10s %s" % ("#", "who", "tier", "class", "state", "cpus", "eta", "why"))
    for e in rows:
        if e.get("mine") and e.get("short"):
            # 0.46.8: a queued job of yours says pin (a node named) or auto.
            pin = ""
            if e.get("state") == "queued" and "pinned" in e:
                pin = "[pin] " if e["pinned"] else "[auto] "
            who = "you: %s%s %s" % (pin, e["short"], e.get("argv") or "")
            if e.get("workspace"):
                who += " (ws %s)" % e["workspace"]
        elif e.get("mine"):
            who = "this server (other workspace)"
        else:
            who = e.get("server") or "another server"
        if e.get("tierWhy"):
            # The Priority one-line reason (0.46.6): who holds the critical path.
            who_why = "P: %s%s" % (e["tierWhy"], (" (%s)" % e["submitter"]) if e.get("submitter") else "")
        else:
            who_why = ""
        eta = e.get("etaSecs")
        if eta is None:
            etas = "-"
        elif e.get("state") == "running":
            etas = "~%dm left" % max(1, (eta + 59) // 60)
        else:
            etas = "~%dm" % max(1, (eta + 59) // 60)
        # WHY (0.46.6, L6): the wait in words (an older daemon: the code).
        why = e.get("why") or e.get("blockedBy") or ""
        cpus = ("%g" % (e["cpuMillis"] / 1000)) if e.get("cpuMillis") else "-"
        print("%-3s %-34s %-4s %-12s %-8s %-5s %-10s %s" % (e.get("pos"), who[:34], tier_tag(e.get("tier")) or "-", e.get("class") or "-", e.get("state"), cpus, etas, why))
        if who_why:
            print("    %s" % who_why)
    if tier_counts(v.get("tiers")):
        print("queued: " + tier_counts(v.get("tiers"), bare=True))
    for t in v.get("totals") or []:
        print("server %-20s running %s  queued %s  today %.1f CPU-h" % (t["label"], t["running"], t["queued"], (t.get("cpuSecsToday") or 0) / 3600))
elif verb == "servers":
    if v.get("role") == "tenant":
        print("%s is shared with this server by %s" % (v["node"], v.get("owner") or "its owner"))
        if v.get("line"):
            print("  your share: %s" % v["line"])
        s = v.get("share") or {}
        if s.get("expires_in_secs"):
            print("  ends in %dh" % (s["expires_in_secs"] // 3600))
    else:
        print("%-4s %-24s %-9s %-5s %-6s %s" % ("slot", "server", "state", "trust", "weight", "share"))
        for s in v.get("servers") or []:
            l = s.get("limits") or {}
            share = ("run %s · queue %s · %s CPU-h/day · %s/job · %s CPUs · %s GB · net %s" % (
                l.get("max_running"), l.get("max_queued"), l.get("cpu_hours_day"), dur(l.get("max_job_secs")), l.get("max_cpus"), l.get("max_mem_gb"), l.get("network"))) if l else ("(this server; no other servers yet)" if s.get("solo") else "-")
            st = s.get("state")
            if s.get("sas"):
                st += " · SAS " + s["sas"]
            u = s.get("usage") or {}
            if u.get("refusal"):
                st = "refused: %s" % u["refusal"]
            print("%-4s %-24s %-9s %-5s %-6s %s" % (s.get("slot"), (s.get("label") or "?")[:24], st, s.get("trust"), l.get("weight", "-"), share))
            extra = []
            if u.get("running") is not None:
                extra.append("running %s, queued %s" % (u.get("running"), u.get("queued")))
            if u.get("cpu_secs_today") is not None:
                extra.append("today %.1f CPU-h" % (u["cpu_secs_today"] / 3600))
            if s.get("expiresAt"):
                extra.append("until " + time.strftime("%b %d %H:%M", time.localtime(s["expiresAt"])))
            # Tiers (0.46.5): does its --tier priority count here?
            if s.get("priority") is True:
                extra.append("Priority counts")
            elif s.get("priority") is False and s.get("slot") != 0:
                extra.append("Priority runs as Normal")
            if extra:
                print("%-4s %s" % ("", " · ".join(extra)))
        for r in v.get("requests") or []:
            print("waiting: %s  %s  %s" % (r["id"], r["state"], r["summary"]))
        if v.get("note"):
            print("note: %s" % v["note"])
elif verb == "enroll":
    print("Enroll code for %s (expires in 10 minutes):" % v["name"])
    print("  %s" % v["enroll"])
    print("Keep it off command lines (ps shows them to every user): put it in a file only you")
    print("can read, or in the vault (k2 vault set NAME --kind generic --delivery file), and pipe it in.")
    print("On the node machine, as an admin (code-file holds the code):")
    print("  Linux: %s" % v["install"]["linux"])
    print("  macOS: %s" % v["install"]["macos"])
    if v["install"].get("servers"):
        print("  Already a node for another K2 server: %s" % v["install"]["servers"])
    print("The node prints a 6-digit code. Check it, then here:")
    print("  %s" % v["confirm"])
    print("Scripts: k2 compute node add %s --json prints {code, expiresAt, name, controller}." % v["name"])
else:
    print(json.dumps(v, indent=2))
'
}

# Print a job's result object (the `result` of jobs/get) on stdout.
_compute_print_result() {
    _compute_call GET /cli/compute/jobs/get "job=$(urlencode "$1")"
    printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin).get("result"), indent=2))'
}

# Follow a job's logs until it ends; exit with its result.
# _compute_follow <job> <raw 0|1> [cursor|seq:N] [json 0|1]
# json=1: every log line goes to stderr and the result object is printed
# on stdout at the end (same exit code).
# Callers may set _CP_DEADLINE (epoch seconds: run --wait --max-wait, wait
# --follow --max-wait; past it: still_running, exit 3, the job keeps going)
# and _CP_FAILURES=1 (after a failing exit, the failing tests).
_compute_follow() {
    local job="$1" raw="${2:-0}" start="${3:-0}" as_json="${4:-0}"
    local cursor=0 query state_file fails=0 lost=0 gen=""
    state_file="$(mktemp -t k2-compute-XXXXXX)"
    # shellcheck disable=SC2064
    trap "rm -f '$state_file' '$state_file.err' '$state_file.q'; echo '[k2 compute] detached; the job keeps running. k2 compute attach ${job:0:8} | k2 compute cancel ${job:0:8} --force' >&2; exit 130" INT
    case "$start" in
        seq:*) query="since_seq=${start#seq:}" ;;
        *) cursor="$start"; query="" ;;
    esac
    while :; do
        local poll=20
        if [ -n "${_CP_DEADLINE:-}" ]; then
            local left=$(( _CP_DEADLINE - $(date +%s) ))
            if [ "$left" -le 0 ]; then
                rm -f "$state_file" "$state_file.err" "$state_file.q"
                trap - INT
                echo "{\"error\":\"still_running\",\"jobs\":[\"${job:0:8}\"],\"hint\":\"--max-wait passed; the job keeps going: k2 compute wait ${job:0:8} --follow\"}" >&2
                exit 3
            fi
            [ "$left" -lt "$poll" ] && poll="$left"
        fi
        local q="job=$(urlencode "$job")&wait=$poll"
        # Follow one attempt at a time: a retry is a new generation.
        [ -n "$gen" ] && q="$q&generation=$gen"
        if [ -n "$query" ]; then q="$q&$query"; query=""; else q="$q&cursor=$cursor"; fi
        local resp
        resp="$(_compute_send GET /cli/compute/jobs/logs "$q")"
        _compute_split "$resp"
        if [ "$_CP_STATUS" = "000" ]; then
            fails=$((fails + 1))
            if [ "$fails" -eq 1 ]; then
                echo "[k2 compute] lost the K2 server; retrying (the job keeps running on the node)" >&2
            fi
            if [ "$fails" -gt 60 ]; then
                rm -f "$state_file"
                echo "{\"error\":\"lost_contact\",\"hint\":\"no answer from the K2 server for 10 minutes; the job may still be running: k2 compute attach $job\"}" >&2
                exit 3
            fi
            lost=1
            sleep 10
            continue
        fi
        [ "$_CP_STATUS" = "200" ] || { rm -f "$state_file"; _compute_fail "$_CP_STATUS" "$_CP_BODY"; }
        fails=0
        printf '%s' "$_CP_BODY" | K2_CP_RAW="$raw" K2_CP_STATE="$state_file" K2_CP_LOST="$lost" K2_CP_JSON="$as_json" python3 -c '
import base64, json, os, re, sys
v = json.load(sys.stdin)
raw = os.environ.get("K2_CP_RAW") == "1"
to_err = os.environ.get("K2_CP_JSON") == "1"
esc = re.compile(rb"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[@-Z\\-_]|[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]")
chunks = v.get("chunks", [])
if os.environ.get("K2_CP_LOST") == "1" and chunks:
    sys.stderr.write("[k2 compute] reattached at seq %d\n" % chunks[0]["seq"])
for c in chunks:
    data = base64.b64decode(c["data"])
    if not raw:
        data = esc.sub(b"", data)
    if c["stream"] == "out" and not to_err:
        sys.stdout.buffer.write(data); sys.stdout.buffer.flush()
    else:
        sys.stderr.buffer.write(data); sys.stderr.buffer.flush()
j = v.get("job", {})
q = v.get("queue") or {}
def s(x):
    return re.sub(r"[^A-Za-z0-9_.:-]", "", str(x if x is not None else ""))
# "|", not a tab: bash `read` merges runs of IFS whitespace, so an empty
# field (no signal, no reason) shifted every later one (mini-1 smoke printed
# the start time as the job id). s() never lets "|" through.
fa = ((j.get("ctl") or {}).get("failedAttempts") or [{}])[-1]
open(os.environ["K2_CP_STATE"], "w").write("|".join([
    s(v.get("cursor", 0)), "1" if v.get("end") else "0", s(j.get("state")), s(j.get("exitCode")),
    s(j.get("signal")), s(j.get("reason")), s(j.get("short")), s(j.get("node")),
    s(j.get("startedAt")), s(j.get("endedAt")), s(q.get("position")), s(q.get("waitReason") or q.get("reason")),
    s(v.get("generation")), s(j.get("generation")), s(j.get("attempt")), s(fa.get("reason") or fa.get("state")),
]) + "\n")
# The wait line (0.46.6: where it waits, N of M, why) in its own file: it
# has spaces and punctuation the "|" fields drop.
open(os.environ["K2_CP_STATE"] + ".q", "w").write((q.get("line") or "").replace("\n", " ") + "\n")
# The teaching error line for a non-result end (JSON-safe here, not in bash).
err = {"error": j.get("state"), "reason": j.get("reason"), "detail": j.get("detail"),
       "hint": "k2 compute get %s --json; k2 compute retry %s" % (j.get("short"), j.get("short"))}
open(os.environ["K2_CP_STATE"] + ".err", "w").write(json.dumps(err) + "\n")
'
        local end st code sig reason short node started ended qpos qreason fgen jgen attempt lastfail
        IFS='|' read -r cursor end st code sig reason short node started ended qpos qreason fgen jgen attempt lastfail < "$state_file"
        lost=0
        [ -z "$gen" ] && gen="$fgen"
        # The attempt we followed ended in an infrastructure failure and a
        # new attempt is queued: keep following that one.
        if [ "$end" = "1" ] && [ -n "$jgen" ] && [ "$jgen" != "$gen" ] && [ "$st" != "done" ] && [ "$st" != "failed" ] && [ "$st" != "cancelled" ] && [ "$st" != "timeout" ] && [ "$st" != "interrupted" ] && [ "$st" != "unknown" ]; then
            echo "[k2 compute] attempt $((attempt - 1)) hit an infrastructure failure (${lastfail:-?}); retrying as attempt $attempt on $node" >&2
            gen="$jgen"
            cursor=0
            _CP_LAST_Q=""
            continue
        fi
        # A line on every position or reason change (k2 compute get <job>
        # shows the jobs ahead).
        if [ "$st" = "queued" ] && [ -n "$qreason" ] && [ "$qpos|$qreason" != "${_CP_LAST_Q:-}" ]; then
            local qline=""
            IFS= read -r qline < "$state_file.q" || true
            if [ -n "$qline" ]; then
                echo "[k2 compute] $qline" >&2
            else
                echo "[k2 compute] queued on $node: position $qpos ($qreason)" >&2
            fi
            _CP_LAST_Q="$qpos|$qreason"
        fi
        if [ "$end" = "1" ]; then
            rm -f "$state_file" "$state_file.q"
            trap - INT
            [ "$st" = "done" ] && rm -f "$state_file.err"
            [ "$as_json" = "1" ] && _compute_print_result "$job"
            local took=""
            if [ -n "$started" ] && [ -n "$ended" ]; then
                took=" after $(( (ended - started) / 60 ))m$(( (ended - started) % 60 ))s"
            fi
            case "$st" in
                done)
                    if [ -n "$code" ]; then
                        echo "[k2 compute] exit $code$took · k2 compute logs $short --failures" >&2
                        if [ "$code" != "0" ] && [ "${_CP_FAILURES:-0}" = "1" ]; then
                            echo "── failures in $short (k2 compute logs $short --failures --tail 60) ──" >&2
                            ( cmd_compute_logs "$job" --failures --tail 60 ) >&2 || true
                        fi
                        exit "$code"
                    fi
                    echo "[k2 compute] killed by signal $sig$took" >&2
                    exit $((128 + ${sig:-0}))
                    ;;
                *)
                    echo "[k2 compute] job $short $st${reason:+ ($reason)}$took" >&2
                    cat "$state_file.err" >&2
                    rm -f "$state_file.err"
                    exit 3
                    ;;
            esac
        fi
    done
}

