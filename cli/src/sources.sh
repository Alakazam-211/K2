# ── k2 sources (Garden v3 data sources; contract-sources-v1 §7) ─────
# HTTP JSON checks the daemon polls; Garden cards and agents read them.
# Regions: C0 owns the noun line, help, --schema, the shared helpers and
# the dispatcher below; B1 edits only BEGIN/END_SOURCES_B1, B2 only
# BEGIN/END_SOURCES_B2. Transport is the compute sender: the session
# passport over the cell socket inside K2, the owner token outside.
# Exit codes: 0 ok; 1 failure; 2 usage; 3 needs a person (person_only,
# owner_only, confirm needed, lan_owner_only, field_person_only); 4 not
# found (source_not_found, trigger_not_found, removed).

# BEGIN_SOURCES_C0
_sources_usage() {
    cat <<'EOF'
k2 sources — data sources: HTTP JSON checks this server runs for Garden cards and agents

  k2 sources list [--all] [--json]          every source with its state
  k2 sources show <name> [--json]           definition + current state
  k2 sources read <name> [--json]           current state (the card view)
  k2 sources history <name> health|<series> [--item K] [--from <dur|epoch>] [--to …]
  k2 sources status [--json]                the poller: in flight, behind, parked sites
  k2 sources export <name>                  definition as JSON (feeds propose --from)
  k2 sources propose --from <file.json> | --name N --label L --url URL [flags…]
  k2 sources add …                          like propose; people only (Settings or outside K2)
  k2 sources edit <name> [flags…]           change a source (agents: only their own)
  k2 sources remove <name> --yes            the name is held for 7 days
  k2 sources pause|resume <name>            people, or agents with the switch below
  k2 sources mute <name> --for 30m [--reason "…"]   agents: up to 2 h, reason required
  k2 sources unmute <name>
  k2 sources refresh <name> [--wait]        check now (at most once per 15 s)
  k2 sources try [propose flags] | --name <saved> [--targets N]   fetch once (every target)
  k2 sources simulate <name> ok|warn|crit|stale|unknown|slow|off [--for 1h]
  k2 sources pending [--names a,b]          proposals waiting (agents: their own)
  k2 sources approve <name>… [--secret NAME]        one secret for every name: ONE confirm
  k2 sources secret <name>… NAME|--none     bind (or unbind) one secret: all or none, ONE confirm
  k2 sources pin|unpin <name>               pinned: agents without the switch can't change it
  k2 sources trigger list|show|log|propose|add|edit|approve|pause|resume|remove|test …
  k2 sources access on|off [--workspace <ws>]   owner/admin: the agent switch below

AGENTS AND "MANAGE GARDENS AND DATA SOURCES"
  A per-agent switch (Settings → Workspaces → <ws> → Agent, or
  `k2 sources access on --workspace <ws>` outside K2). Off (the default): an
  agent proposes; pending, approve, secret, pause/resume, pin/unpin and
  trigger approve/add/pause/resume exit 3 (person_only). On: the agent does
  all of those itself with no confirm (approve its own proposals and others'
  on this server). It binds only a secret it may already use: its own
  workspace's secret, or a server secret when it also has "Allow managing
  server-wide secrets" (else secret_not_usable, exit 3). The bind still goes
  only to the source's one site, inside the secret's own sites. It never sets
  viewers / agent-access / keep or a trigger's cooldown or cap. Every act is
  logged with the agent and its session (Settings → Logs → Access Audit).
  Only the owner or an admin flips the switch; an agent never can.

RULES AND THRESHOLDS (k2 sources propose --help has every flag)
  --rule 'field:crit:op:v,warn:op:v,ok:op:v' with eq ne in lt lte gt gte exists, ageGt/ageLt
  (a timestamp's age), versionBehind/versionAtLeast (semver), and @field to compare two
  fields (box version vs target_version). failAfter (--fail-after, default 3) is how many
  checks in a row a reading waits before it goes hard. warnAfter:N / critAfter:N on any
  rule opts the source in to per-rule thresholds: failAfter then counts FETCH failures
  only, and each rule waits its own warnAfter (default 1: at once) / critAfter (warn after
  1, crit after N). Without them a source behaves as before 0.46.1.
  --expect-status 200,401 makes a status-only check.
  More lists: --list / --list-field; an array's length: --count.

Exit codes: 0 ok, 1 failure, 2 usage, 3 needs a person, 4 not found.
--json prints the daemon's answer as is (also on errors).
Each verb has its own page: k2 sources <verb> --help. More: k2 study data-sources
EOF
}

# Per-verb help (0.46.1): `k2 sources <verb> --help`. Returns 1 for a verb
# with no page of its own (the caller then prints the noun help).
_sources_verb_help() {
    case "$1" in
        propose|add|edit|try) _sources_flags_help "$1" ;;
        history) cat <<'EOF'
k2 sources history <name> health|<series> [--item <key>] [--from 6h|epoch] [--to 1h|epoch] [--json]

  health          the source's state changes, oldest first (went_warn, recovered, …);
                  with --item <key>, that item's (a box, a cert). Kept: the newest 200.
  <series>        a trend, count or aggregate of the source (extract.trend, counts,
                  aggregates): points (t, v). Raw points 48 h, hourly 35 days.
                  An item field has no series; aggregate it (--agg) instead.

  Values are hidden from agents on an agentAccess "shape" source (0s), and a
  keep "states" source has no series (409 no_history); health still reads.
EOF
        ;;
        export) cat <<'EOF'
k2 sources export <name>

  The definition as JSON: the shape `propose --from` takes, so
    k2 sources export fleet > fleet.json && $EDITOR fleet.json && k2 sources propose --from fleet.json
  round-trips. Secret NAMES only, never values. For agents, query values that
  look secret (token/key/sig keys, or 24+ character token-shaped values) show
  as •••; plain ones (?part=boxes) stay as they are.
EOF
        ;;
        pending) cat <<'EOF'
k2 sources pending [--names a,b] [--json]

  Proposals (sources and triggers) waiting for approval.
  A person, or an agent with "Manage Gardens and data sources": every proposal.
  Any other agent: only its own (the answer says "scope": "own"); a person
  approves them.
EOF
        ;;
        read|show|list) cat <<'EOF'
k2 sources list [--all] [--json]    every source with its state
k2 sources show <name> [--json]     definition + current state
k2 sources read <name> [--json]     the card view (what sources.read gives a widget)

  The card view: health ok|warn|crit|unknown, phase pending|live|stale|paused,
  display, reason (a fixed code), fields, items[] (each with health, reason,
  soft, list for a named list), counts, soft, serverNow, updatedAt.
  soft {count, of, level}: a reading waiting to go hard, e.g. "soft crit 2/3"
  (a rule's critAfter) or "soft crit 1/3" for a fetch failure (failAfter).
EOF
        ;;
        *) return 1 ;;
    esac
}

_sources_flags_help() {
    case "$1" in
        propose) echo "k2 sources propose --from <file.json> | --name N --label L <fetch> [flags…]  (agents: it waits for approval)" ;;
        add) echo "k2 sources add --from <file.json> | --name N --label L <fetch> [flags…]  (people only; active at once without a secret)" ;;
        edit) echo "k2 sources edit <name> [flags…]  (agents: only their own; a changed definition needs approving again)" ;;
        try) echo "k2 sources try --from <file.json> | --url URL [flags…] | --name <saved> [--targets N] [--tree-depth N] [--json]
  Fetch once and show the shape (pointer · type · sample) and the card it would make. Saves nothing.
  A fan-out source fetches every target (or the first N with --targets N)." ;;
    esac
    cat <<'EOF'

FETCH (one of)
  --url URL                       one HTTPS JSON address
  --target key=URL                one fan-out target (repeat, up to 50); each answer is one item
  --url-secret NAME --origin https://host   the whole URL lives in the vault
  --secret NAME                   a vault secret sent as the auth header (bound by a person,
                                  or an agent with the switch)
  --header 'Name: value'          a plain header (no secrets: those are refused)
  --expect-status 200,401         status-only check: these statuses are healthy answers,
                                  whatever the body (HTML, 401). JSON is still read if it parses.

SCHEDULE
  --every 60s|5m|1h               how often (30 s to 1 day; default 60 s)
  --timeout 10s                   1-30 s (fan-out default 5 s)
  --max-bytes 1M                  1 KiB to 5 MiB
  --fail-after N                  checks in a row before a reading goes hard (1-10,
                                  default 3). Once any rule sets warnAfter/critAfter,
                                  failAfter counts fetch failures only.
  --on-target-error crit|unknown  what a failed fan-out target reads as

WHAT TO KEEP
  --field name=/pointer           a value from the answer
  --count name=/pointer           the length of an array or object (e.g. past_due)
  --items /pointer                the item list (array, or object keyed by name)
  --item-key /pointer             each item's key (default: index or property name)
  --item-field name=/pointer      a value from each item
  --list name=/pointer            another item list next to --items (single URL only)
  --list-key name=/pointer        that list's item key
  --list-field list.field=/pointer   a value from each of that list's items
                                  (item field names are unique across all lists; list
                                  items have the key <list>/<key> and "list": "<list>")
  --agg name=count[:list] | count-where:field:op:value | sum:f | min:f | max:f | distinct:f
  --trend name                    chart a field/count/aggregate (up to 8; history also
                                  keeps every count and aggregate)

RULES  --rule 'field:level:op:value[,level:op:value…][,warnAfter:N][,critAfter:N]'
  level: crit, warn, ok (checked in that order; none matching = unknown)
  op:    eq ne in lt lte gt gte exists
         ageGt ageLt           the value is a time (epoch s/ms, RFC 3339) older/newer than
                               a duration:  --rule 'ts:warn:ageGt:5m,crit:ageGt:15m'
         versionBehind         older than a version or @field, optionally by a distance:
                               --rule 'version:warn:versionBehind:@target_version,crit:versionBehind:@target_version~minor=1'
         versionAtLeast        at or past:  ok:versionAtLeast:@target_version
  value: a literal, a|b|c for in, or @name: another field (the item's own, else the
         source's), e.g. 'used:warn:gt:@quota'
  warnAfter:N  checks in a row before this rule's warn goes hard (1-20, default 1: at once)
  critAfter:N  checks in a row (not ok, the last one crit) before crit; until then it
               shows as warn ("soft crit 2/3"). Default: warnAfter.
  Opt-in: setting warnAfter or critAfter on ANY rule turns per-rule thresholds on for the
  whole source (rules without them then go hard at once; failAfter = fetch failures only).
  With neither set, every rule reading waits failAfter, as before 0.46.1. Opting in
  changes the definition, so the source is approved again.
  A field rule colours the source; an item field rule colours each item of its list.

WHO SEES WHAT
  --viewers member|admin  --agent-access values|shape  --keep values|states   (people only)
  --link https://…        where the card's title goes
  --preset NAME --preset-arg k=v   start from a preset (k2 study data-sources)

THE FILE FORM (--from) is the export shape; see k2 sources export <name> and
k2 study data-sources for a full example. Flags given with --from override it.
EOF
}

# The daemon's answer as is (pretty when it parses).
_sources_print_json() {
    printf '%s' "${1:-}" | python3 -c '
import json, sys
raw = sys.stdin.read()
try:
    print(json.dumps(json.loads(raw), indent=2, ensure_ascii=False))
except Exception:
    print(raw)
'
}

# Print a refusal and exit with the contract code.
_sources_fail() {
    local status="$1" body="$2" code="" daemon_hint=""
    code="$(printf '%s' "$body" | python3 -c '
import json, sys
try:
    v = json.loads(sys.stdin.read())
    e = v.get("error") if isinstance(v, dict) else None
    print(e.get("code", "") if isinstance(e, dict) else (e or ""))
except Exception:
    print("")
' 2>/dev/null)"
    # 0.46.1: did the daemon already say what to do? Then ours is noise.
    daemon_hint="$(printf '%s' "$body" | python3 -c '
import json, sys
try:
    e = json.loads(sys.stdin.read()).get("error")
    print("1" if isinstance(e, dict) and str(e.get("hint") or "").strip() else "")
except Exception:
    print("")
' 2>/dev/null)"
    if [ "${_SRC_JSON:-0}" = "1" ] && [ -n "$body" ]; then
        _sources_print_json "$body"
    elif [ -n "$body" ]; then
        printf '%s' "$body" | python3 -c '
import json, sys
raw = sys.stdin.read()
try:
    e = json.loads(raw).get("error")
    if isinstance(e, dict):
        print("Error: %s: %s" % (e.get("code", "error"), e.get("hint", "")))
    else:
        print("Error: %s" % e)
except Exception:
    print("Error: daemon returned: %s" % raw[:200])
' >&2
    else
        echo "Error: no answer from the K2 daemon (status ${status:-none})" >&2
    fi
    case "$code" in
        # vault_confirm_required is the one confirm code (0.46.0);
        # confirm_required is what a 0.45.x daemon answers.
        person_only|owner_only|vault_confirm_required|confirm_required|lan_owner_only|field_person_only|secret_not_usable|forbidden)
            # Never the agent-switch line for owner_only (no switch opens
            # it). A refusal that a person or the switch can lift always
            # names both (the daemon's hint says what, not who); any other
            # code gets ours only when the daemon gave none.
            local person_code=0
            case "$code" in person_only|vault_confirm_required|confirm_required|field_person_only) person_code=1 ;; esac
            if [ "$code" != "owner_only" ] && { [ -z "$daemon_hint" ] || [ "$person_code" = "1" ]; }; then
                if [ "$code" = "secret_not_usable" ]; then
                    echo "Bind a secret from your own workspace, or ask your human for \"Allow managing server-wide secrets\"." >&2
                else
                    echo "This needs a person (run it outside K2, or Settings → Gardens → a Garden → External Data), or the agent switch \"Manage Gardens and data sources\" (k2 sources --help)." >&2
                fi
            fi
            exit 3 ;;
        source_not_found|trigger_not_found|removed) exit 4 ;;
        *) exit 1 ;;
    esac
}

# _sources_call METHOD PATH [PAYLOAD]: GET payload is a query string, POST
# payload is JSON. Fails (and exits) on anything but 200; the body lands in
# _SRC_BODY.
_sources_call() {
    local raw
    raw="$(_compute_send "$@")"
    _SRC_STATUS="${raw##*$'\n'}"
    _SRC_BODY="${raw%$'\n'*}"
    [ "$_SRC_STATUS" = "$raw" ] && _SRC_BODY=""
    [ -z "$_SRC_STATUS" ] && _SRC_STATUS="000"
    [ "$_SRC_STATUS" = "200" ] || _sources_fail "$_SRC_STATUS" "$_SRC_BODY"
}

_sources_not_built() {
    echo "Error: not_built: k2 sources $1 is not built yet" >&2
    exit 1
}

cmd_sources() {
    local sub="${1:-}"
    case "$sub" in
        ""|help|--help|-h) _sources_usage; return 0 ;;
    esac
    shift
    if _wants_help "$@"; then
        _sources_verb_help "$sub" || _sources_usage
        return 0
    fi
    _SRC_JSON=0
    local a
    for a in "$@"; do [ "$a" = "--json" ] && _SRC_JSON=1; done
    case "$sub" in
        list|ls) cmd_sources_list "$@" ;;
        show) cmd_sources_show "$@" ;;
        read) cmd_sources_read "$@" ;;
        history) cmd_sources_history "$@" ;;
        status) cmd_sources_status "$@" ;;
        export) cmd_sources_export "$@" ;;
        propose) cmd_sources_propose "$@" ;;
        add) cmd_sources_add "$@" ;;
        edit) cmd_sources_edit "$@" ;;
        remove|rm) cmd_sources_remove "$@" ;;
        pause) cmd_sources_pause "$@" ;;
        resume) cmd_sources_resume "$@" ;;
        mute) cmd_sources_mute "$@" ;;
        unmute) cmd_sources_unmute "$@" ;;
        refresh) cmd_sources_refresh "$@" ;;
        try) cmd_sources_try "$@" ;;
        simulate) cmd_sources_simulate "$@" ;;
        approve) cmd_sources_approve "$@" ;;
        secret) cmd_sources_secret "$@" ;;
        pin) cmd_sources_pin "$@" ;;
        unpin) cmd_sources_unpin "$@" ;;
        pending) cmd_sources_pending "$@" ;;
        trigger|triggers) cmd_sources_trigger "$@" ;;
        access) cmd_sources_access "$@" ;;
        *)
            echo "Error: unknown k2 sources command '$sub'" >&2
            _sources_usage >&2
            exit 2
            ;;
    esac
}
# END_SOURCES_C0

# BEGIN_SOURCES_B1
# Spec flags → JSON (propose/add/try body, or edit's `set`). argv: mode
# (`full` | `set`), then the flags. Usage errors exit 2 with a message.
_sources_spec_json() {
    python3 - "$@" <<'PY'
import json, re, sys

mode, args = sys.argv[1], sys.argv[2:]

def die(msg):
    sys.stderr.write("Error: %s\n" % msg)
    sys.exit(2)

def dur(v, flag):
    m = re.fullmatch(r"(\d+)([smhd]?)", v.strip())
    if not m:
        die("%s takes a duration like 30s, 5m, 2h, 1d" % flag)
    return int(m.group(1)) * {"": 1, "s": 1, "m": 60, "h": 3600, "d": 86400}[m.group(2)]

def size(v):
    m = re.fullmatch(r"(\d+)([KkMm]?)", v.strip())
    if not m:
        die("--max-bytes takes a size like 512K or 1M")
    return int(m.group(1)) * {"": 1, "k": 1024, "m": 1 << 20}[m.group(2).lower()]

def val(s):
    try:
        return json.loads(s)
    except Exception:
        return s

def cond(op, value):
    # 0.46.1: @name is a field reference (the item's own field, else a
    # source field); ageGt/ageLt take a duration; versionBehind takes a
    # version or @field, with an optional ~minor=N / ~patch=N / ~major=N.
    ops = {"eq", "ne", "in", "lt", "lte", "gt", "gte", "exists", "ageGt", "ageLt", "versionBehind", "versionAtLeast"}
    if op not in ops:
        die("unknown operator %r (eq ne in lt lte gt gte exists ageGt ageLt versionBehind versionAtLeast)" % op)
    ref = value.startswith("@")
    if op == "in":
        return {"in": [val(x) for x in value.split("|")]}
    if op in ("ageGt", "ageLt"):
        return {op: dur(value, op)}
    if op in ("versionBehind", "versionAtLeast"):
        target, _, dist = value.partition("~")
        v = {"field": target[1:]} if target.startswith("@") else {"value": target}
        if dist:
            lvl, _, n = dist.partition("=")
            if lvl not in ("major", "minor", "patch") or not n.isdigit():
                die("%s distance is ~major=N, ~minor=N or ~patch=N" % op)
            v[lvl] = int(n)
        if list(v) == ["value"]:
            return {op: v["value"]}
        return {op: v}
    if op in ("lt", "lte", "gt", "gte"):
        if ref:
            return {op: {"field": value[1:]}}
        try:
            return {op: float(value) if "." in value else int(value)}
        except ValueError:
            die("%s needs a number or @field" % op)
    if op == "exists":
        return {"exists": value.strip().lower() not in ("false", "0", "no")}
    if ref:
        return {op: {"field": value[1:]}}
    return {op: val(value)}

def kv(v, flag, sep="="):
    if sep not in v:
        die("%s takes name%s/pointer" % (flag, sep))
    k, x = v.split(sep, 1)
    return k.strip(), x

out = {}
extract = {}
items = {}
targets = []
headers = []
rules = []
aggs = {}
fields = {}
item_fields = {}
lists = {}
counts = {}
trend = []
try_targets = None
preset_args = {}
from_file = None
i = 0
def nxt(flag):
    global i
    if i + 1 >= len(args):
        die("%s needs a value" % flag)
    i += 1
    return args[i]
while i < len(args):
    a = args[i]
    if a == "--json" or a == "--yes":
        pass
    elif a == "--from":
        from_file = nxt(a)
    elif a == "--name":
        out["name"] = nxt(a)
    elif a == "--label":
        out["label"] = nxt(a)
    elif a == "--url":
        out["fetch"] = {"url": nxt(a)}
    elif a == "--target":
        k, u = kv(nxt(a), a)
        targets.append({"key": k, "url": u})
    elif a == "--url-secret":
        out.setdefault("fetch", {})
        out["fetch"] = {"urlSecret": {"name": nxt(a), "origin": out.get("_origin", "")}}
    elif a == "--origin":
        o = nxt(a)
        out["_origin"] = o
        if "urlSecret" in out.get("fetch", {}):
            out["fetch"]["urlSecret"]["origin"] = o
    elif a == "--header":
        h = nxt(a)
        if ":" not in h:
            die("--header takes Name:value")
        n, v = h.split(":", 1)
        headers.append([n.strip(), v.strip()])
    elif a == "--secret":
        out["secret"] = nxt(a)
    elif a == "--every":
        out["everySecs"] = dur(nxt(a), a)
    elif a == "--timeout":
        out["timeoutSecs"] = dur(nxt(a), a)
    elif a == "--max-bytes":
        out["maxBytes"] = size(nxt(a))
    elif a == "--field":
        k, p = kv(nxt(a), a)
        fields[k] = p
    elif a == "--items":
        items["pointer"] = nxt(a)
    elif a == "--item-key":
        items["itemKey"] = nxt(a)
    elif a == "--item-field":
        k, p = kv(nxt(a), a)
        item_fields[k] = p
    elif a == "--agg":
        k, spec = kv(nxt(a), a)
        parts = spec.split(":")
        op = parts[0]
        if op == "count" and len(parts) == 1:
            aggs[k] = {"op": "count"}
        elif op == "count" and len(parts) == 2:
            aggs[k] = {"op": "count", "list": parts[1]}
        elif op == "count-where" and len(parts) >= 4:
            aggs[k] = {"op": "countWhere", "field": parts[1], "cond": cond(parts[2], ":".join(parts[3:]))}
        elif op in ("sum", "min", "max", "distinct") and len(parts) == 2:
            aggs[k] = {"op": op, "field": parts[1]}
        else:
            die("--agg takes name=count[:list] | count-where:field:op:value | sum:f | min:f | max:f | distinct:f")
    elif a == "--rule":
        r = nxt(a)
        head, _, rest = r.partition(":")
        if not rest:
            die("--rule takes 'field:crit:op:value[,warn:op:value][,ok:op:value]'")
        rule = {"field": head}
        for part in rest.split(","):
            bits = part.split(":", 2)
            if len(bits) == 2 and bits[0] in ("warnAfter", "critAfter"):
                if not bits[1].isdigit():
                    die("--rule %s takes a number of checks (1-20)" % bits[0])
                rule[bits[0]] = int(bits[1])
                continue
            if len(bits) != 3 or bits[0] not in ("crit", "warn", "ok"):
                die("--rule parts are level:op:value with level crit, warn or ok, or warnAfter:N / critAfter:N")
            rule[bits[0]] = cond(bits[1], bits[2])
        rules.append(rule)
    elif a == "--list":
        k, p = kv(nxt(a), a)
        lists.setdefault(k, {})["pointer"] = p
    elif a == "--list-key":
        k, p = kv(nxt(a), a)
        lists.setdefault(k, {})["itemKey"] = p
    elif a == "--list-field":
        k, p = kv(nxt(a), a)
        if "." not in k:
            die("--list-field takes list.field=/pointer")
        ln, fn = k.split(".", 1)
        lists.setdefault(ln, {}).setdefault("itemFields", {})[fn] = p
    elif a == "--count":
        k, p = kv(nxt(a), a)
        counts[k] = p
    elif a == "--expect-status":
        v = nxt(a)
        try:
            out["expect"] = {"status": [int(x) for x in v.split(",") if x.strip()]}
        except ValueError:
            die("--expect-status takes statuses like 200,401")
    elif a == "--targets":
        v = nxt(a)
        if not v.isdigit():
            die("--targets takes a number")
        try_targets = int(v)
    elif a == "--trend":
        trend.append(nxt(a))
    elif a == "--fail-after":
        out["failAfter"] = int(nxt(a))
    elif a == "--on-target-error":
        out["onTargetError"] = nxt(a)
    elif a == "--link":
        out["link"] = nxt(a)
    elif a == "--preset":
        out["preset"] = nxt(a)
    elif a == "--preset-arg":
        k, v = kv(nxt(a), a)
        preset_args[k] = v
    elif a == "--viewers":
        out["viewers"] = nxt(a)
    elif a == "--agent-access":
        out["agentAccess"] = nxt(a)
    elif a == "--keep":
        out["keep"] = nxt(a)
    else:
        die("unknown flag %s (k2 sources --help)" % a)
    i += 1
out.pop("_origin", None)
if from_file:
    try:
        base = json.load(sys.stdin if from_file == "-" else open(from_file))
    except Exception as e:
        die("--from: %s" % e)
    if not isinstance(base, dict):
        die("--from: the file holds one JSON object")
    base.update(out)
    out = base
if targets:
    out["fetch"] = {"targets": targets}
if headers:
    out["headers"] = headers
if fields:
    extract["fields"] = fields
if items or item_fields:
    if item_fields:
        items["itemFields"] = item_fields
    if "pointer" not in items and "targets" not in out.get("fetch", {}):
        items["pointer"] = ""
    extract["items"] = items
if lists:
    for ln, l in lists.items():
        if "pointer" not in l:
            die("--list %s=/pointer says where the list %r is" % (ln, ln))
    extract["lists"] = lists
if counts:
    extract["counts"] = counts
if aggs:
    extract["aggregates"] = aggs
if trend:
    extract["trend"] = trend
if extract:
    out["extract"] = extract
if rules:
    out["rules"] = rules
if preset_args:
    out["presetArgs"] = preset_args
if try_targets is not None:
    if mode != "draft":
        die("--targets is for k2 sources try")
    out["tryTargets"] = try_targets
if mode == "full":
    if "preset" not in out:
        for need in ("name", "label"):
            if need not in out:
                die("--%s is required" % need)
        if "fetch" not in out:
            die("say what to fetch: --url, --target key=url, or --url-secret NAME --origin https://host")
        out.setdefault("everySecs", 60)
    else:
        for need in ("name", "label"):
            if need not in out:
                die("--%s is required" % need)
print(json.dumps(out))
PY
}

# Seconds from a duration (30s, 5m, 2h, 1d); exits 2 on a bad one.
_sources_secs() {
    local v="$1" flag="$2"
    case "$v" in
        *[!0-9smhd]*|"") echo "Error: $flag takes a duration like 30s, 5m, 2h, 1d" >&2; exit 2 ;;
        *d) echo $(( ${v%d} * 86400 )) ;;
        *h) echo $(( ${v%h} * 3600 )) ;;
        *m) echo $(( ${v%m} * 60 )) ;;
        *s) echo "${v%s}" ;;
        *) echo "$v" ;;
    esac
}

# Print a source's card view (or a list row's view) for people.
_sources_render() {
    local kind="$1"
    printf '%s' "$_SRC_BODY" | python3 -c '
import json, sys
kind = sys.argv[1]
d = json.load(sys.stdin)

def ago(now, t):
    if t is None:
        return "never"
    s = max(0, now - t)
    if s < 90:
        return "%d s ago" % s
    if s < 5400:
        return "%d min ago" % (s // 60)
    if s < 172800:
        return "%d h ago" % (s // 3600)
    return "%d d ago" % (s // 86400)

DOT = {"ok": "●", "warn": "▲", "crit": "✖", "unknown": "?", "pending": "○", "paused": "‖", "stale": "◌"}

def soft_hint(s):
    # 0.46.1: "soft crit 2/3" names the reading waiting to go hard.
    s = s or {}
    if not s.get("count"):
        return ""
    if s.get("level"):
        return "soft %s %d/%d" % (s["level"], s["count"], s["of"])
    return "retrying %d/%d" % (s["count"], s["of"])

def line(v):
    c = v.get("counts") or {}
    counts = ", ".join("%d %s" % (c[k], k) for k in ("ok", "warn", "crit", "unknown") if c.get(k))
    bits = [v.get("display", "?")]
    if counts:
        bits.append(counts)
    bits.append("updated " + ago(v["serverNow"], v.get("updatedAt")))
    if v.get("reason"):
        bits.append(v["reason"])
    if soft_hint(v.get("soft")):
        bits.append(soft_hint(v.get("soft")))
    if v.get("muted"):
        bits.append("muted by %s" % v["muted"]["by"])
    if v.get("simulated"):
        bits.append("TEST " + v["simulated"]["state"])
    if v.get("pinned"):
        bits.append("pinned")
    return " · ".join(bits)

def card(v):
    print("%s %s (%s)" % (DOT.get(v.get("display"), "?"), v["name"], v["label"]))
    print("  " + line(v))
    if v.get("reasonText"):
        print("  " + v["reasonText"])
    if v.get("gap"):
        g = v["gap"]
        print("  gap: %s (%s s)" % (g["kind"], g["to"] - g["from"]))
    for k, x in sorted((v.get("fields") or {}).items()):
        print("  %-18s %s" % (k, json.dumps(x)))
    for it in v.get("items") or []:
        f = " ".join("%s=%s" % (k, json.dumps(x)) for k, x in sorted((it.get("fields") or {}).items()))
        r = (" (" + it["reason"] + ")") if it.get("reason") else ""
        sh = (" [" + soft_hint(it.get("soft")) + "]") if soft_hint(it.get("soft")) else ""
        print("  %s %-24s %s%s%s %s" % (DOT.get(it["health"], "?"), it["key"], it["health"], r, sh, f))
    if v.get("masked"):
        print("  (values hidden from agents on this source)")

if kind == "list":
    rows = d.get("sources") or []
    if not rows:
        print("No data sources on this server. Propose one: k2 sources propose --help")
    for r in rows:
        v = r["view"]
        extra = []
        if r["status"] != "active":
            extra.append(r["status"])
        if r.get("secret"):
            extra.append("secret " + r["secret"])
        if r.get("triggers"):
            extra.append("%d trigger%s" % (r["triggers"], "" if r["triggers"] == 1 else "s"))
        if r.get("keep") == "states":
            extra.append("states only")
        print("%s %-24s %s%s" % (DOT.get(v.get("display"), "?"), r["name"], line(v), (" · " + " · ".join(extra)) if extra else ""))
elif kind == "read":
    card(d)
elif kind == "show":
    s = d["source"]
    card(d["view"])
    f = s.get("fetch") or {}
    if "url" in f:
        print("  fetch: %s" % f["url"])
    elif "targets" in f:
        print("  fetch: %d targets" % len(f["targets"]))
    elif "urlSecret" in f:
        print("  fetch: URL in the vault (%s) at %s" % (f["urlSecret"]["name"], f["urlSecret"]["origin"]))
    print("  status: %s · every %s s · made by %s · viewers %s · agents see %s · keep %s"
          % (s["status"], s["everySecs"], s["madeBy"], s.get("viewers"), s.get("agentAccess"), s.get("keep")))
    if s.get("secret"):
        print("  secret: %s (%s)" % (s["secret"], "bound" if s.get("secretBound") else "not bound"))
    for w in d.get("warnings") or []:
        print("  warning: %s" % w["hint"])
elif kind == "status":
    print("Poller: %s · %d sources (%d active) · %d targets · %d in flight · %s"
          % ("running" if d.get("running") else "not running", d["sources"], d["active"], d["targets"], d["inFlight"],
             "on time" if not d.get("behindSecs") else "behind by %d s" % d["behindSecs"]))
    for p in d.get("parkedOrigins") or []:
        print("  parked: %s until %s (%s)" % (p["origin"], p["untilAt"], p["reason"]))
    if d.get("lastGap"):
        g = d["lastGap"]
        print("  last gap: %s (%d s)" % (g["kind"], g["to"] - g["from"]))
    if d.get("airgap"):
        print("  air-gap is on: only exact-IP sources run")
elif kind == "history":
    if d.get("field") == "health":
        evs = d.get("events") or []
        if not evs:
            print("No state changes in this window.")
        for e in evs:
            who = (" " + e["item"]) if e.get("item") else ""
            why = (" (" + e["reason"] + ")") if e.get("reason") else ""
            print("%d\t%s%s\t%s → %s%s" % (e["t"], e["kind"], who, e.get("from") or "-", e.get("to") or "-", why))
    else:
        for p in d.get("points") or []:
            print("%d\t%s" % (p["t"], "gap" if p["v"] is None else p["v"]))
' "$kind"
}

_sources_name_arg() {
    local verb="$1"; shift
    local a
    for a in "$@"; do
        case "$a" in
            --*) ;;
            *) printf '%s' "$a"; return 0 ;;
        esac
    done
    echo "Error: usage: k2 sources $verb <name>" >&2
    exit 2
}

_sources_out() {
    if [ "${_SRC_JSON:-0}" = "1" ]; then
        _sources_print_json "$_SRC_BODY"
        exit 0
    fi
}

cmd_sources_list() {
    _sources_call GET /cli/sources/list ""
    _sources_out
    _sources_render list
}

cmd_sources_show() {
    local name; name="$(_sources_name_arg show "$@")"
    _sources_call GET /cli/sources/show "name=$(urlencode "$name")"
    _sources_out
    _sources_render show
}

cmd_sources_read() {
    local name; name="$(_sources_name_arg read "$@")"
    _sources_call GET /cli/sources/read "name=$(urlencode "$name")"
    _sources_out
    _sources_render read
}

cmd_sources_history() {
    local name="" field="" from="" to="" item=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --from) from="${2:-}"; shift 2 ;;
            --to) to="${2:-}"; shift 2 ;;
            --item) item="${2:-}"; shift 2 ;;
            --json) shift ;;
            --*) echo "Error: unknown flag $1" >&2; exit 2 ;;
            *) if [ -z "$name" ]; then name="$1"; elif [ -z "$field" ]; then field="$1"; fi; shift ;;
        esac
    done
    [ -n "$name" ] && [ -n "$field" ] || { echo "Error: usage: k2 sources history <name> health|<trend|count|aggregate> [--item <key>] [--from 6h|epoch] [--to …]" >&2; exit 2; }
    local q="name=$(urlencode "$name")&field=$(urlencode "$field")" t now
    [ -n "$item" ] && q="$q&item=$(urlencode "$item")"
    now="$(date +%s)"
    for t in from to; do
        local v; if [ "$t" = from ]; then v="$from"; else v="$to"; fi
        [ -z "$v" ] && continue
        case "$v" in
            *[smhd]) v=$(( now - $(_sources_secs "$v" "--$t") )) ;;
            *[!0-9]*) echo "Error: --$t takes a duration (6h) or epoch seconds" >&2; exit 2 ;;
        esac
        q="$q&$t=$v"
    done
    _sources_call GET /cli/sources/history "$q"
    _sources_out
    _sources_render history
}

cmd_sources_status() {
    _sources_call GET /cli/sources/status ""
    _sources_out
    _sources_render status
}

cmd_sources_export() {
    local name; name="$(_sources_name_arg export "$@")"
    _sources_call GET /cli/sources/export "name=$(urlencode "$name")"
    _sources_print_json "$_SRC_BODY"
}

_sources_create() {
    local verb="$1"; shift
    local body
    body="$(_sources_spec_json full "$@")" || exit 2
    _sources_call POST "/cli/sources/$verb" "$body"
    _sources_out
    printf '%s' "$_SRC_BODY" | python3 -c '
import json, sys
d = json.load(sys.stdin)
how = {"one_click": "one click in Settings → Gardens → a Garden → External Data (or k2 sources approve)",
       "needs_confirm": "a person binds the secret with a confirm (Settings, or k2 sources approve --secret)",
       "owner_only": "the server owner approves it"}.get(d.get("approval"), "")
print("%s: %s" % (d["name"], d["status"]))
if d["status"] == "proposed" and how:
    print("Waiting for approval: " + how)
for w in d.get("warnings") or []:
    print("warning: " + w["hint"])
'
}

cmd_sources_propose() { _sources_create propose "$@"; }
cmd_sources_add() { _sources_create add "$@"; }

cmd_sources_edit() {
    local name="" rest=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --*) rest+=("$1"); case "$1" in --json|--yes) ;; *) [ $# -gt 1 ] && { rest+=("$2"); shift; } ;; esac; shift ;;
            *) if [ -z "$name" ]; then name="$1"; else rest+=("$1"); fi; shift ;;
        esac
    done
    [ -n "$name" ] || { echo "Error: usage: k2 sources edit <name> [flags]" >&2; exit 2; }
    local set body
    set="$(_sources_spec_json set ${rest[@]+"${rest[@]}"})" || exit 2
    body="$(K2_SRC_NAME="$name" K2_SRC_SET="$set" python3 -c 'import json, os; s = json.loads(os.environ["K2_SRC_SET"]); s.pop("name", None); print(json.dumps({"name": os.environ["K2_SRC_NAME"], "set": s}))')"
    _sources_call POST /cli/sources/edit "$body"
    _sources_out
    printf '%s' "$_SRC_BODY" | python3 -c '
import json, sys
d = json.load(sys.stdin)
print("%s: %s" % (d["name"], d["status"]))
if d.get("unbound"):
    print("The secret was unbound; a person approves it again with a confirm.")
for w in d.get("warnings") or []:
    print("warning: " + w["hint"])
'
}

cmd_sources_remove() {
    local name; name="$(_sources_name_arg remove "$@")"
    local yes=0 a
    for a in "$@"; do [ "$a" = "--yes" ] && yes=1; done
    if [ "$yes" != "1" ]; then
        if [ -n "${K2_HOOK_SOCK:-}${K2SO_HOOK_SOCK:-}" ] || [ ! -t 0 ]; then
            echo "Error: k2 sources remove needs --yes here" >&2
            exit 2
        fi
        printf 'Remove %s? Gardens on other computers that show it will say removed; the name is held for 7 days. [y/N] ' "$name" >&2
        local ans; read -r ans
        case "$ans" in y|Y|yes) ;; *) echo "Not removed." >&2; exit 1 ;; esac
    fi
    _sources_call POST /cli/sources/remove "$(K2_SRC_NAME="$name" python3 -c 'import json, os; print(json.dumps({"name": os.environ["K2_SRC_NAME"]}))')"
    _sources_out
    echo "Removed $name (name held for 7 days)."
}

_sources_name_post() {
    local verb="$1" name="$2"
    _sources_call POST "/cli/sources/$verb" "$(K2_SRC_NAME="$name" python3 -c 'import json, os; print(json.dumps({"name": os.environ["K2_SRC_NAME"]}))')"
}

cmd_sources_pause() {
    local name; name="$(_sources_name_arg pause "$@")"
    _sources_name_post pause "$name"
    _sources_out
    echo "$name: paused"
}

cmd_sources_resume() {
    local name; name="$(_sources_name_arg resume "$@")"
    _sources_name_post resume "$name"
    _sources_out
    echo "$name: active"
}

cmd_sources_mute() {
    local name="" for_="" reason=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --for) for_="${2:-}"; shift 2 ;;
            --reason) reason="${2:-}"; shift 2 ;;
            --json) shift ;;
            --*) echo "Error: unknown flag $1" >&2; exit 2 ;;
            *) name="$1"; shift ;;
        esac
    done
    [ -n "$name" ] && [ -n "$for_" ] || { echo "Error: usage: k2 sources mute <name> --for 30m [--reason \"…\"]" >&2; exit 2; }
    local secs; secs="$(_sources_secs "$for_" --for)"
    _sources_call POST /cli/sources/mute "$(K2_SRC_NAME="$name" K2_SRC_FOR="$secs" K2_SRC_REASON="$reason" python3 -c '
import json, os
b = {"name": os.environ["K2_SRC_NAME"], "forSecs": int(os.environ["K2_SRC_FOR"])}
if os.environ.get("K2_SRC_REASON"):
    b["reason"] = os.environ["K2_SRC_REASON"]
print(json.dumps(b))')"
    _sources_out
    echo "$name: muted for $for_"
}

cmd_sources_unmute() {
    local name; name="$(_sources_name_arg unmute "$@")"
    _sources_name_post unmute "$name"
    _sources_out
    echo "$name: unmuted"
}

cmd_sources_refresh() {
    local name; name="$(_sources_name_arg refresh "$@")"
    local wait=0 a
    for a in "$@"; do [ "$a" = "--wait" ] && wait=1; done
    local before=""
    if [ "$wait" = "1" ]; then
        _sources_call GET /cli/sources/read "name=$(urlencode "$name")"
        before="$(printf '%s' "$_SRC_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("checkedAt"))')"
    fi
    _sources_name_post refresh "$name"
    if [ "$wait" != "1" ]; then
        _sources_out
        echo "$name: check queued"
        return 0
    fi
    local i
    for i in $(seq 1 30); do
        sleep 1
        _sources_call GET /cli/sources/read "name=$(urlencode "$name")"
        local now_checked
        now_checked="$(printf '%s' "$_SRC_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("checkedAt"))')"
        if [ "$now_checked" != "$before" ]; then
            _sources_out
            _sources_render read
            return 0
        fi
    done
    echo "Error: no new check within 30 s" >&2
    exit 1
}
# END_SOURCES_B1

# BEGIN_SOURCES_B2
# Try, simulate, approvals, secrets, pins, pending and triggers (B2).

# 30s / 5m / 2h / 1d / bare seconds → seconds.
_sources_b2_secs() {
    local v="$1"
    case "$v" in
        *d) echo $(( ${v%d} * 86400 )) ;;
        *h) echo $(( ${v%h} * 3600 )) ;;
        *m) echo $(( ${v%m} * 60 )) ;;
        *s) echo "${v%s}" ;;
        ''|*[!0-9]*) echo "usage: bad duration '$v' (use 30s, 5m, 2h, 1d)" >&2; exit 2 ;;
        *) echo "$v" ;;
    esac
}

# A query string from k=v pairs (values url-encoded; empty values dropped).
_sources_b2_query() {
    python3 -c '
import sys, urllib.parse
pairs = [a.split("=", 1) for a in sys.argv[1:]]
print(urllib.parse.urlencode([(k, v) for k, v in pairs if v != ""]))
' "$@"
}

# A draft source (export shape) from propose-style flags, or a file. Used by
# Try. Prints JSON. 0.46.1: the same parser as propose (every flag).
_sources_b2_draft() {
    _sources_spec_json draft "$@"
}

cmd_sources_try() {
    local name="" depth="" draft_args=()
    _SRC_JSON=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) _SRC_JSON=1; shift ;;
            --tree-depth) [ -n "${2:-}" ] || { echo "usage: --tree-depth N" >&2; exit 2; }; depth="$2"; shift 2 ;;
            --name)
                # `--name X` alone tries the saved source X; with other flags
                # it names the draft.
                draft_args+=("$1" "${2:-}"); name="${2:-}"; shift 2 ;;
            *) draft_args+=("$1"); shift ;;
        esac
    done
    local body
    if [ "${#draft_args[@]}" -eq 2 ] && [ -n "$name" ]; then
        body="$(python3 -c 'import json,sys; print(json.dumps({"name": sys.argv[1]}))' "$name")"
    else
        [ "${#draft_args[@]}" -gt 0 ] || { echo "usage: k2 sources try --url URL [flags…] | --from file.json | --name <saved>" >&2; exit 2; }
        body="$(_sources_b2_draft "${draft_args[@]}")" || exit $?
    fi
    _sources_call POST /cli/sources/try "$body"
    if [ "$_SRC_JSON" = "1" ]; then _sources_print_json "$_SRC_BODY"; return 0; fi
    printf '%s' "$_SRC_BODY" | python3 -c '
import json, sys
depth = int(sys.argv[1]) if sys.argv[1] else 99
v = json.load(sys.stdin)
head = "ok" if v.get("ok") else ("failed: %s" % (v.get("reason") or "error"))
print("%s · HTTP %s · %s ms%s" % (head, v.get("status"), v.get("elapsedMs"), " · truncated" if v.get("truncated") else ""))
for t in v.get("targets") or []:
    print("  target %-20s %s %s %s ms" % (t.get("key"), t.get("status"), t.get("reason") or "", t.get("elapsedMs")))
for w in v.get("warnings") or []:
    print("warning: %s: %s" % (w.get("code"), w.get("hint")))
if v.get("rootKey"):
    print("warning: %s" % v["rootKey"].get("hint"))
tree = v.get("tree") or []
if tree:
    print("shape (pointer · type · sample):")
for n in tree:
    p = n.get("pointer") or "/"
    if p.count("/") > depth:
        continue
    extra = ""
    if n.get("len") is not None:
        extra = " len %s" % n["len"]
    if "sample" in n:
        extra = " " + json.dumps(n["sample"], ensure_ascii=False)[:80]
    print("  %s · %s%s" % (p, n.get("type"), extra))
if v.get("treeTruncated"):
    print("  … (the tree stops at 500 nodes)")
view = v.get("view")
if view:
    print("would read: %s · %s" % (view.get("display"), ", ".join("%s %s" % (k, view.get("counts", {}).get(k)) for k in ("ok", "warn", "crit", "unknown"))))
print("Nothing was saved. Propose it with: k2 sources propose …")
' "$depth"
}

cmd_sources_simulate() {
    local name="${1:-}" state="${2:-}" for_secs=""
    case "$name" in ""|-*) echo "usage: k2 sources simulate <name> ok|warn|crit|stale|unknown|slow|off [--for 1h]" >&2; exit 2 ;; esac
    case "$state" in
        ok|warn|crit|stale|unknown|slow|off) ;;
        *) echo "usage: k2 sources simulate <name> ok|warn|crit|stale|unknown|slow|off [--for 1h]" >&2; exit 2 ;;
    esac
    shift 2
    _SRC_JSON=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --for) [ -n "${2:-}" ] || { echo "usage: --for 1h" >&2; exit 2; }; for_secs="$(_sources_b2_secs "$2")" || exit 2; shift 2 ;;
            --json) _SRC_JSON=1; shift ;;
            *) echo "Error: unknown flag '$1'" >&2; exit 2 ;;
        esac
    done
    _sources_call POST /cli/sources/simulate "$(_compute_json K2_CJ_name="$name" K2_CJ_state="$state" K2_CJ_forSecs_n="$for_secs")"
    if [ "$_SRC_JSON" = "1" ]; then _sources_print_json "$_SRC_BODY"; return 0; fi
    printf '%s' "$_SRC_BODY" | python3 -c '
import json, sys, datetime
v = json.load(sys.stdin); s = v.get("simulated")
if not s:
    print("%s: simulate off (real state shows again)" % v.get("name"))
else:
    until = datetime.datetime.fromtimestamp(s["until"], datetime.timezone.utc).strftime("%H:%M UTC")
    print("%s: TEST %s until %s. Triggers never fire for it; nothing is written to history." % (v.get("name"), s["state"], until))
'
}

# pending?names=… → JSON (persons only).
_sources_b2_pending() {
    local names="${1:-}"
    _sources_call GET /cli/sources/pending "$(_sources_b2_query "names=$names")"
}

cmd_sources_pending() {
    local names=""
    _SRC_JSON=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --names) names="${2:-}"; shift 2 ;;
            --json) _SRC_JSON=1; shift ;;
            *) echo "Error: unknown flag '$1'" >&2; exit 2 ;;
        esac
    done
    _sources_b2_pending "$names"
    if [ "$_SRC_JSON" = "1" ]; then _sources_print_json "$_SRC_BODY"; return 0; fi
    printf '%s' "$_SRC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
src, trg = v.get("sources") or [], v.get("triggers") or []
own = v.get("scope") == "own"
if not src and not trg:
    print("None of your proposals is waiting." if own else "Nothing is waiting for approval."); sys.exit(0)
if own:
    print("Your proposals waiting for a person (or an agent with the switch):")
for s in src:
    m = s.get("maker") or {}
    by = m.get("handle") or m.get("user") or s.get("madeBy")
    sec = s.get("urlSecret") or s.get("secret")
    print("%-20s %-11s every %ss · %s target(s) · %s · by %s%s" % (s["name"], s["approval"], s["everySecs"], s["targets"], ", ".join(s.get("origins") or []), by, (" · wants secret " + sec) if sec else ""))
for t in trg:
    on = t["on"]["kind"] + ((":%ss" % t["on"]["secs"]) if t["on"].get("secs") else "")
    print("trigger %s  %s on %s → %s (by %s)" % (t["id"][:8], t["source"], on, t["agent"].get("handle"), t.get("madeBy")))
print()
if own:
    print("A person approves them in Settings → Gardens → a Garden → External Data (or k2 sources approve outside K2).")
else:
    print("Approve: k2 sources approve <name>… · k2 sources trigger approve <id>")
'
}

# Inside a K2 terminal (a session passport): no vault confirm is asked
# for. An agent with "Manage Gardens and data sources" needs none; any other
# passport is refused by the daemon (person_only, exit 3).
_sources_in_k2() {
    [ -n "${K2_HOOK_TOKEN:-}${K2SO_HOOK_TOKEN:-}" ]
}

# A vault confirm (Touch ID / vault password) for ACTION bound to ID.
# Inside K2 it prints nothing (see _sources_in_k2).
_sources_b2_confirm() {
    local action="$1" id="$2" label="$3"
    if _sources_in_k2; then
        return 0
    fi
    _vault_confirm_token "$action" "$id" "$label"
}

# k2 sources access on|off [--workspace <ws>]: the per-agent "Manage Gardens
# and data sources" switch (0181). Owner/admin only: inside a K2 terminal
# this sends the session passport, which the daemon refuses (an agent never
# turns it on for itself).
cmd_sources_access() {
    local state="" workspace="" as_json=0
    while [ $# -gt 0 ]; do
        case "$1" in
            on|off)
                [ -z "$state" ] || { echo "usage: k2 sources access on|off [--workspace <ws>]" >&2; exit 2; }
                state="$1"; shift ;;
            --workspace)
                [ -n "${2:-}" ] || { echo "usage: --workspace <workspace name or path>" >&2; exit 2; }
                workspace="$2"; shift 2 ;;
            --json) as_json=1; shift ;;
            *) echo "usage: k2 sources access on|off [--workspace <ws>] [--json]" >&2; exit 2 ;;
        esac
    done
    [ -n "$state" ] || { echo "usage: k2 sources access on|off [--workspace <ws>] [--json]" >&2; exit 2; }
    [ -n "$workspace" ] || workspace="$(pwd)"
    local raw
    raw="$(_compute_send POST /cli/agent-access/set "$(_compute_json K2_CJ_workspace="$workspace" K2_CJ_toggle=gardens_manage K2_CJ_value_b="$([ "$state" = on ] && echo 1 || echo 0)")" tcp)"
    _SRC_STATUS="${raw##*$'\n'}"
    _SRC_BODY="${raw%$'\n'*}"
    [ "$_SRC_STATUS" = "$raw" ] && _SRC_BODY=""
    if [ "$_SRC_STATUS" != "200" ]; then
        if [ "$_SRC_STATUS" = "403" ]; then
            echo "Error: forbidden: only the owner or an admin turns this on or off (outside K2, or Settings → Workspaces → <ws> → Agent). An agent never can." >&2
            exit 3
        fi
        _sources_fail "$_SRC_STATUS" "$_SRC_BODY"
    fi
    if [ "$as_json" = "1" ]; then _sources_print_json "$_SRC_BODY"; return 0; fi
    printf '%s' "$_SRC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
if v.get("agentsCanManageGardens"):
    print("Manage Gardens and data sources is ON for %s: its agent approves sources, binds secrets it may use, pauses, pins, enables triggers and edits Gardens with no confirm (each act is logged)." % v.get("workspace"))
else:
    print("Manage Gardens and data sources is OFF for %s: its agent proposes; a person approves." % v.get("workspace"))'
}

cmd_sources_approve() {
    local names=() secret="" viewers="" access="" keep="" use_anyway=""
    _SRC_JSON=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --secret) [ -n "${2:-}" ] || { echo "usage: --secret NAME" >&2; exit 2; }; secret="$2"; shift 2 ;;
            --viewers) viewers="${2:-}"; shift 2 ;;
            --agent-access) access="${2:-}"; shift 2 ;;
            --keep) keep="${2:-}"; shift 2 ;;
            --use-anyway) use_anyway=1; shift ;;
            --json) _SRC_JSON=1; shift ;;
            -*) echo "Error: unknown flag '$1'" >&2; exit 2 ;;
            *) names+=("$1"); shift ;;
        esac
    done
    [ "${#names[@]}" -gt 0 ] || { echo "usage: k2 sources approve <name>… [--secret NAME] [--viewers member|admin] [--agent-access values|shape] [--keep values|states] [--use-anyway]" >&2; exit 2; }
    local joined
    joined="$(IFS=,; printf '%s' "${names[*]}")"
    _sources_b2_pending "$joined"
    local plan
    plan="$(printf '%s' "$_SRC_BODY" | python3 -c '
import json, sys, hashlib
v = json.load(sys.stdin)
want = sys.argv[1].split(",")
picked = sys.argv[2]
rows = {s["name"]: s for s in v.get("sources") or []}
missing = [n for n in want if n not in rows]
if missing:
    print("missing\t" + ",".join(missing)); sys.exit(0)
items, pairs = [], []
for n in want:
    s = rows[n]
    sec = s.get("urlSecret") or picked or s.get("secret")
    it = {"name": n, "specHash": s["specHash"]}
    if sec:
        it["secret"] = sec
        pairs.append("%s:%s" % (s["id"], sec))
    items.append(it)
pairs.sort()
h = hashlib.sha256("\n".join(pairs).encode()).hexdigest() if pairs else ""
first = rows[want[0]]
print("ok\t%s\t%s\t%s\t%s" % (json.dumps(items), h, first["id"], "1" if pairs else ""))
' "$joined" "$secret")"
    local kind items hash first_id needs
    IFS=$'\t' read -r kind items hash first_id needs <<<"$plan"
    if [ "$kind" = "missing" ]; then
        echo "Error: source_not_found: not waiting for approval: $items (k2 sources pending)" >&2
        exit 4
    fi
    local tok="" body
    if [ "${#names[@]}" -eq 1 ]; then
        if [ -n "$needs" ]; then
            tok="$(_sources_b2_confirm source-secret "$first_id" "${names[0]}")" || exit 3
        fi
        body="$(printf '%s' "$items" | python3 -c '
import json, sys
it = json.load(sys.stdin)[0]
a = sys.argv[1:]
for k, v in (("confirmToken", a[0]), ("viewers", a[1]), ("agentAccess", a[2]), ("keep", a[3])):
    if v: it[k] = v
if a[4]: it["useAnyway"] = True
print(json.dumps(it))
' "$tok" "$viewers" "$access" "$keep" "$use_anyway")"
        _sources_call POST /cli/sources/approve "$body"
    else
        # One --secret for every name: one batch confirm covers them all
        # (0.46.0); viewers/agent-access/keep are still one at a time.
        if [ -n "$viewers$access$keep" ]; then
            echo "usage: --viewers/--agent-access/--keep apply to one source at a time" >&2
            exit 2
        fi
        if [ -n "$needs" ]; then
            tok="$(_sources_b2_confirm source-batch "$hash" "${#names[@]} sources")" || exit 3
        fi
        body="$(printf '%s' "$items" | python3 -c '
import json, sys
b = {"items": json.load(sys.stdin), "triggers": []}
if sys.argv[1]: b["confirmToken"] = sys.argv[1]
if sys.argv[2]: b["useAnyway"] = True
print(json.dumps(b))
' "$tok" "$use_anyway")"
        _sources_call POST /cli/sources/batch-approve "$body"
    fi
    tok=""
    if [ "$_SRC_JSON" = "1" ]; then _sources_print_json "$_SRC_BODY"; return 0; fi
    printf '%s' "$_SRC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
if "approved" in v:
    print("approved: " + ", ".join(v["approved"]))
else:
    print("%s: active%s" % (v.get("name"), " · secret bound" if v.get("secretBound") else ""))
'
}

cmd_sources_secret() {
    # k2 sources secret <name>… NAME|--none [--use-anyway]: one secret for
    # every name (0.46.1): all bound or none, ONE confirm for a person.
    local pos=() none="" use_anyway=""
    _SRC_JSON=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --none) none=1; shift ;;
            --use-anyway) use_anyway=1; shift ;;
            --json) _SRC_JSON=1; shift ;;
            -*) echo "Error: unknown flag '$1'" >&2; exit 2 ;;
            *) pos+=("$1"); shift ;;
        esac
    done
    local secret="" names=()
    if [ -n "$none" ]; then
        names=(${pos[@]+"${pos[@]}"})
    elif [ "${#pos[@]}" -ge 2 ]; then
        secret="${pos[${#pos[@]}-1]}"
        names=("${pos[@]:0:${#pos[@]}-1}")
    fi
    [ "${#names[@]}" -gt 0 ] || { echo "usage: k2 sources secret <name>… NAME|--none [--use-anyway]" >&2; exit 2; }
    local body
    if [ "${#names[@]}" -gt 1 ]; then
        local namelist
        namelist="$(printf '%s\n' "${names[@]}")"
        if [ -n "$none" ] || _sources_in_k2; then
            # Unbinding needs no confirm; an agent with "Manage Gardens and
            # data sources" binds with none (others get person_only, exit 3).
            body="$(K2_SRC_NAMES="$namelist" python3 -c '
import json, os, sys
b = {"names": os.environ["K2_SRC_NAMES"].splitlines(), "secret": sys.argv[1] or None}
if sys.argv[2]: b["useAnyway"] = True
print(json.dumps(b))' "$secret" "$use_anyway")"
        else
            # One confirm bound to the whole list: sha256 of the sorted
            # "<sourceId>:<secret>" lines (as approve --secret uses).
            local ids="" n sid
            for n in "${names[@]}"; do
                _sources_call GET /cli/sources/show "$(_sources_b2_query "name=$n")"
                sid="$(printf '%s' "$_SRC_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin)["source"].get("id") or "")')"
                [ -n "$sid" ] || { echo "Error: the daemon didn't name the source id for $n (run this outside K2)" >&2; exit 3; }
                ids="$ids$sid"$'\n'
            done
            local hash
            hash="$(K2_SRC_IDS="$ids" python3 -c '
import hashlib, os, sys
lines = sorted("%s:%s" % (i, sys.argv[1]) for i in os.environ["K2_SRC_IDS"].splitlines() if i)
print(hashlib.sha256("\n".join(lines).encode()).hexdigest())' "$secret")"
            local tok
            tok="$(_sources_b2_confirm source-batch "$hash" "${#names[@]} sources")" || exit 3
            body="$(K2_SRC_NAMES="$namelist" python3 -c '
import json, os, sys
b = {"names": os.environ["K2_SRC_NAMES"].splitlines(), "secret": sys.argv[1], "confirmToken": sys.argv[2]}
if sys.argv[3]: b["useAnyway"] = True
print(json.dumps(b))' "$secret" "$tok" "$use_anyway")"
            tok=""
        fi
        _sources_call POST /cli/sources/secret "$body"
        if [ "$_SRC_JSON" = "1" ]; then _sources_print_json "$_SRC_BODY"; return 0; fi
        printf '%s' "$_SRC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print("%s: %s" % (", ".join(v["names"]), ("use " + v["secret"]) if v.get("secretBound") else "no secret"))'
        return 0
    fi
    local name="${names[0]}"
    if [ -n "$none" ]; then
        body="$(python3 -c 'import json,sys; print(json.dumps({"name": sys.argv[1], "secret": None}))' "$name")"
    elif _sources_in_k2; then
        # An agent with "Manage Gardens and data sources" binds with no
        # confirm; any other passport gets person_only (exit 3).
        body="$(python3 -c '
import json, sys
b = {"name": sys.argv[1], "secret": sys.argv[2]}
if sys.argv[3]: b["useAnyway"] = True
print(json.dumps(b))' "$name" "$secret" "$use_anyway")"
    else
        # The confirm is bound to the source id (persons see it in show).
        _sources_call GET /cli/sources/show "$(_sources_b2_query "name=$name")"
        local sid
        sid="$(printf '%s' "$_SRC_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin)["source"].get("id") or "")')"
        [ -n "$sid" ] || { echo "Error: the daemon didn't name the source id (run this outside K2)" >&2; exit 3; }
        local tok
        tok="$(_sources_b2_confirm source-secret "$sid" "$name")" || exit 3
        body="$(python3 -c '
import json, sys
b = {"name": sys.argv[1], "secret": sys.argv[2], "confirmToken": sys.argv[3]}
if sys.argv[4]: b["useAnyway"] = True
print(json.dumps(b))' "$name" "$secret" "$tok" "$use_anyway")"
        tok=""
    fi
    _sources_call POST /cli/sources/secret "$body"
    if [ "$_SRC_JSON" = "1" ]; then _sources_print_json "$_SRC_BODY"; return 0; fi
    printf '%s' "$_SRC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print("%s: %s" % (v["name"], ("uses " + v["secret"]) if v.get("secretBound") else "no secret"))'
}

_sources_b2_pin() {
    local verb="$1" name="${2:-}"
    case "$name" in ""|-*) echo "usage: k2 sources $verb <name>" >&2; exit 2 ;; esac
    _SRC_JSON=0
    [ "${3:-}" = "--json" ] && _SRC_JSON=1
    _sources_call POST "/cli/sources/$verb" "$(_compute_json K2_CJ_name="$name")"
    if [ "$_SRC_JSON" = "1" ]; then _sources_print_json "$_SRC_BODY"; return 0; fi
    printf '%s' "$_SRC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print("%s: %s" % (v["name"], "pinned (agents can no longer edit or remove it)" if v.get("pinned") else "not pinned"))'
}

cmd_sources_pin() { _sources_b2_pin pin "$@"; }
cmd_sources_unpin() { _sources_b2_pin unpin "$@"; }

# went_crit | stale_for:30m → the JSON `on`.
_sources_b2_on() {
    local v="$1" kind secs=""
    kind="${v%%:*}"
    case "$kind" in
        went_crit|went_warn|recovered|item_went_crit) [ "$kind" = "$v" ] || { echo "usage: --on $kind (no duration)" >&2; exit 2; } ;;
        stale_for)
            [ "$kind" != "$v" ] || { echo "usage: --on stale_for:30m" >&2; exit 2; }
            secs="$(_sources_b2_secs "${v#*:}")" || exit 2 ;;
        *) echo "usage: --on went_crit|went_warn|recovered|item_went_crit|stale_for:30m" >&2; exit 2 ;;
    esac
    if [ -n "$secs" ]; then printf '{"kind":"stale_for","secs":%s}' "$secs"; else printf '{"kind":"%s"}' "$kind"; fi
}

_sources_b2_trigger_line() {
    python3 -c '
import json, sys, time
v = json.load(sys.stdin)
rows = v.get("triggers") if "triggers" in v else [v["trigger"]]
if not rows:
    print("No triggers."); sys.exit(0)
for t in rows:
    on = t["on"]["kind"] + ((":%ss" % t["on"]["secs"]) if t["on"].get("secs") else "")
    a = t["agent"]
    who = a.get("handle") + (" (missing workspace)" if a.get("missing") else "")
    last = ""
    if t.get("lastFireAt"):
        last = " · last fire %s" % time.strftime("%Y-%m-%d %H:%M UTC", time.gmtime(t["lastFireAt"]))
    print("%s  %-14s %-18s → %-20s %-13s cooldown %ss · cap %s/day · %s today%s" % (
        t["id"][:8], t["source"], on, who, t["status"], t["cooldownSecs"], t["dailyCap"], t.get("firesToday", 0), last))
    if t.get("note"):
        print("          note: %s" % t["note"])
'
}

_sources_b2_fires() {
    python3 -c '
import json, sys, time
v = json.load(sys.stdin)
fires = v.get("fires") or []
if not fires:
    print("No fires yet."); sys.exit(0)
for f in fires:
    t = time.strftime("%m-%d %H:%M", time.gmtime(f["at"]))
    what = f["outcome"]
    if f.get("suppressReason"): what += " (%s)" % f["suppressReason"]
    if f.get("detail"): what += " [%s]" % f["detail"]
    if f.get("woke"): what += " · woke"
    print("%s  %-36s incident %s%s" % (t, what, f["incidentId"][:12], (" item " + f["itemKey"]) if f.get("itemKey") else ""))
'
}

cmd_sources_trigger() {
    local sub="${1:-}"
    [ $# -gt 0 ] && shift
    _SRC_JSON=0
    case "$sub" in
        ""|help|--help|-h)
            cat <<'EOF'
k2 sources trigger — wake an agent on this server when a source changes

  k2 sources trigger list [--source NAME] [--json]
  k2 sources trigger show <id> [--json]
  k2 sources trigger log <id> [--limit 50] [--json]
  k2 sources trigger propose --source NAME --on KIND [--agent HANDLE] [--note "…"]
  k2 sources trigger add --source NAME --on KIND --agent HANDLE [--note "…"] [--cooldown 30m] [--daily-cap 12]
  k2 sources trigger edit <id> [--on KIND] [--agent HANDLE] [--note "…"|--no-note] [--cooldown 30m] [--daily-cap 12]
  k2 sources trigger approve|pause|resume|remove|test <id>

KIND: went_crit · went_warn · recovered · item_went_crit · stale_for:30m
Agents propose (waking only themselves) and may remove their own proposal;
a person approves. A proposed trigger never fires. An agent with "Manage
Gardens and data sources" also approves, adds (waking itself), pauses and
resumes triggers, always with the default cooldown and daily cap (only a
person sets those; edit and test stay a person's).
EOF
            return 0 ;;
    esac
    local id="" source="" on="" agent="" note="" no_note="" cooldown="" cap="" limit=""
    case "$sub" in
        show|log|edit|approve|pause|resume|remove|rm|test)
            id="${1:-}"
            case "$id" in ""|-*) echo "usage: k2 sources trigger $sub <id>" >&2; exit 2 ;; esac
            shift ;;
    esac
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) _SRC_JSON=1; shift ;;
            --source) source="${2:-}"; shift 2 ;;
            --on) on="$(_sources_b2_on "${2:-}")" || exit 2; shift 2 ;;
            --agent) agent="${2:-}"; shift 2 ;;
            --note) note="${2:-}"; shift 2 ;;
            --no-note) no_note=1; shift ;;
            --cooldown) cooldown="$(_sources_b2_secs "${2:-}")" || exit 2; shift 2 ;;
            --daily-cap) cap="${2:-}"; shift 2 ;;
            --limit) limit="${2:-}"; shift 2 ;;
            *) echo "Error: unknown flag '$1' (k2 sources trigger --help)" >&2; exit 2 ;;
        esac
    done
    # Full ids only on the wire; a short prefix is matched from the list.
    if [ -n "$id" ] && [ "${#id}" -lt 36 ]; then
        _sources_call GET /cli/sources/triggers/list ""
        local full
        full="$(printf '%s' "$_SRC_BODY" | python3 -c '
import json, sys
m = [t["id"] for t in json.load(sys.stdin).get("triggers") or [] if t["id"].startswith(sys.argv[1])]
print(m[0] if len(m) == 1 else "")' "$id")"
        [ -n "$full" ] || { echo "Error: trigger_not_found: no single trigger starts with '$id' (k2 sources trigger list)" >&2; exit 4; }
        id="$full"
    fi
    case "$sub" in
        list|ls)
            _sources_call GET /cli/sources/triggers/list "$(_sources_b2_query "source=$source")"
            if [ "$_SRC_JSON" = "1" ]; then _sources_print_json "$_SRC_BODY"; else printf '%s' "$_SRC_BODY" | _sources_b2_trigger_line; fi ;;
        show)
            _sources_call GET /cli/sources/triggers/show "$(_sources_b2_query "id=$id")"
            if [ "$_SRC_JSON" = "1" ]; then _sources_print_json "$_SRC_BODY"; else
                printf '%s' "$_SRC_BODY" | _sources_b2_trigger_line
                printf '%s' "$_SRC_BODY" | _sources_b2_fires
            fi ;;
        log)
            _sources_call GET /cli/sources/triggers/log "$(_sources_b2_query "id=$id" "limit=$limit")"
            if [ "$_SRC_JSON" = "1" ]; then _sources_print_json "$_SRC_BODY"; else printf '%s' "$_SRC_BODY" | _sources_b2_fires; fi ;;
        propose|add)
            [ -n "$source" ] && [ -n "$on" ] || { echo "usage: k2 sources trigger $sub --source NAME --on KIND [--agent HANDLE]" >&2; exit 2; }
            _sources_call POST "/cli/sources/triggers/$sub" "$(_compute_json K2_CJ_source="$source" K2_CJ_on_j="$on" K2_CJ_agent="$agent" K2_CJ_note="$note" K2_CJ_cooldownSecs_n="$cooldown" K2_CJ_dailyCap_n="$cap")"
            if [ "$_SRC_JSON" = "1" ]; then _sources_print_json "$_SRC_BODY"; else
                printf '%s' "$_SRC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print("trigger %s: %s" % (v["id"], v["status"]))
if v["status"] == "proposed":
    print("A person approves it in Settings → Gardens → a Garden → External Data (or k2 sources trigger approve %s)." % v["id"][:8])'
            fi ;;
        edit)
            local set
            set="$(_compute_json K2_CJ_on_j="$on" K2_CJ_agent="$agent" K2_CJ_note="$note" K2_CJ_cooldownSecs_n="$cooldown" K2_CJ_dailyCap_n="$cap")"
            if [ -n "$no_note" ]; then set="$(printf '%s' "$set" | python3 -c 'import json,sys; d=json.load(sys.stdin); d["note"]=None; print(json.dumps(d))')"; fi
            _sources_call POST /cli/sources/triggers/edit "$(python3 -c 'import json,sys; print(json.dumps({"id": sys.argv[1], "set": json.loads(sys.argv[2])}))' "$id" "$set")"
            if [ "$_SRC_JSON" = "1" ]; then _sources_print_json "$_SRC_BODY"; else printf '%s' "$_SRC_BODY" | _sources_b2_trigger_line; fi ;;
        approve|pause|resume|remove|rm|test)
            local verb="$sub"
            [ "$verb" = "rm" ] && verb="remove"
            _sources_call POST "/cli/sources/triggers/$verb" "$(_compute_json K2_CJ_id="$id")"
            if [ "$_SRC_JSON" = "1" ]; then _sources_print_json "$_SRC_BODY"; else
                printf '%s' "$_SRC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
if "outcome" in v:
    print("test wake: %s%s" % (v["outcome"], " (woke the agent)" if v.get("woke") else ""))
elif v.get("removed"):
    print("trigger %s removed" % v["id"])
else:
    print("trigger %s: %s" % (v["id"], v.get("status")))'
            fi ;;
        *) echo "Error: unknown k2 sources trigger command '$sub' (k2 sources trigger --help)" >&2; exit 2 ;;
    esac
}
# k2 study data-sources (GV67; not `sources`: `k2 study source` is the
# licence page).
_study_page_data_sources() {
    cat <<'EOF'
k2 study data-sources — checks this server runs for cards and agents

WHAT
  A source is an HTTP JSON check that THIS server runs on a schedule (it
  keeps running with no window open). It keeps a few fields, rates each
  item ok / warn / crit / unknown, and a short history. A Garden card only
  shows a source; it never fetches. A trigger wakes an agent on this server
  when the state changes (hard changes only, never soft blips).

THE LOOP
  1. k2 sources try --url https://… [--items /list --item-field state=/state]
     Fetch once, see the shape (pointer · type · sample). Nothing is saved.
  2. k2 sources propose --name fleet --label "Fleet" --target a=https://… …
     (or --preset k2-fleet --preset-arg boxes=a,b). It waits as "proposed".
  3. A person approves it: one click in Settings → Gardens → a Garden → External Data
     for a secretless public source; a vault confirm when it uses a secret
     (one confirm for many: k2 sources approve a b c --secret NAME). An
     agent with "Manage Gardens and data sources" approves it itself.
  4. k2 sources trigger propose --source fleet --on item_went_crit
     wakes YOU when a box goes red. A person approves it too.
  5. k2 sources show fleet   the definition + the exact card view.

KEYS
  Never paste a key into a URL, header or file. Keys live in the vault; a
  person binds one to a source (with a confirm) for that exact site, or an
  agent with the switch below binds one it may use (no confirm). Query
  keys like token/key/sig are refused; a long token in the path is warned.
  Webhook-style URLs that carry a token are vault "url" secrets (urlSecret).

WHAT YOU SEE
  agentAccess "shape": you see field names and states, values as ••• .
  That is a guardrail, not a wall. keep "states" is the wall: the server
  stores no values at all (item keys, states, counts and reasons only).
  Trigger notes are written by K2 from fixed parts: no values for those.

LIMITS
  Agents may: propose, edit/remove their own (not pinned, not read by a
  locked Garden: 423), mute up to 2 h with a reason, simulate up to 1 h,
  refresh (once per 15 s), propose triggers that wake themselves.
  Without the switch below, agents may not: see pending, approve, bind a
  secret, pin, pause, enable a trigger (exit 3, person_only).
  Never, switch or not: set viewers / agentAccess / keep, raise a trigger's
  cooldown/cap, point a trigger at another agent. LAN and plain http are
  the owner's only.

"MANAGE GARDENS AND DATA SOURCES" (THE AGENT SWITCH)
  The owner or an admin turns it on per agent (Settings → Workspaces → <ws>
  → Agent, or k2 sources access on --workspace <ws> outside K2); an agent
  never can. With it you act with no person and no confirm: pending,
  approve (yours and others' on this server), approve a b c --secret NAME
  (all bound or none), secret, pause/resume, pin/unpin, trigger approve /
  add / pause / resume. You bind only a secret you may already use: your
  own workspace's (k2 vault set NAME: scope ws:<you> by default), or a
  server secret if you also have "Allow managing server-wide secrets"
  (else exit 3, secret_not_usable). The bind goes only to the source's one
  site, inside the secret's own sites; it never widens them. Each act is
  logged with you, your session, the target and the secret's NAME (Access
  Audit).

RULES (0.46.1)
  A rule rates one field: crit → warn → ok, else unknown. Operators: eq ne
  in lt lte gt gte exists, and:
    ageGt / ageLt  N     the value is a time (epoch s or ms, RFC 3339) more /
                         less than N seconds old: stale data that is still served
    versionBehind  V     older than a version, or {"field": "target_version"};
                         a distance: {"field": …, "minor": 1} (or major/patch)
    versionAtLeast V     at or past it
    {"field": "name"}    in eq/ne/lt/lte/gt/gte: compare to another field (the
                         item's own first, then the source's)
  Thresholds per rule (opt-in): "warnAfter": 1 (default, at once) and
  "critAfter": N (checks in a row that aren't ok, the last one crit; until then
  it shows warn with soft {count, of, level}). Setting either on any rule turns
  them on for the source: failAfter then counts fetch failures only. A source
  with neither keeps the old behaviour: every rule reading waits failAfter.
  Status-only checks: "expect": {"status": [200, 401]} (k2.dev HTML, a 401 login
  page); any listed status is healthy, JSON is read if it parses.
  More lists: "extract.lists": {"certs": {"pointer": "/certs", "itemKey": "/host",
  "itemFields": {"cert_days": "/days_left"}}} (single URL; item field names unique
  across lists; keys "certs/<host>"). An array's length: "extract.counts":
  {"past_due_n": "/past_due"}. Count one list: {"op": "count", "list": "certs"}.

  Example (a fleet summary with a target version):
    {"name": "k2-ops-boxes", "label": "K2 boxes", "everySecs": 60,
     "fetch": {"url": "https://k2.dev/api/ops/fleet?part=boxes"}, "secret": "K2_OPS_READ_TOKEN",
     "extract": {"fields": {"target_version": "/target_version", "ts": "/ts"},
       "items": {"pointer": "/boxes", "itemKey": "/name", "itemFields": {"version": "/version"}},
       "aggregates": {"behind": {"op": "countWhere", "field": "version",
         "cond": {"versionBehind": {"field": "target_version"}}}}},
     "rules": [
       {"field": "ts", "warn": {"ageGt": 300}, "crit": {"ageGt": 900}, "ok": {"exists": true}},
       {"field": "version", "ok": {"versionAtLeast": {"field": "target_version"}},
        "warn": {"versionBehind": {"field": "target_version"}},
        "crit": {"versionBehind": {"field": "target_version", "minor": 1}}, "critAfter": 3}]}

HISTORY
  k2 sources history <name> health [--item <key>]   when the state changed
  k2 sources history <name> <trend|count|aggregate> the numbers over time

GOTCHAS
  Redirects must stay on the same origin (a GitHub release ASSET redirects
  elsewhere and fails; read the API). Timeouts 10 s (fan-out 5 s), 1 MiB.
  A down box in a fan-out source is a red ITEM; the source stays live.
  k2 sources simulate fleet crit --for 10m shows TEST on cards; triggers
  never fire for it.

CARDS
  Built-in: k2:fleet-grid@3, k2:version-spread@3, k2:relays-dns@3,
  k2:deploys@3, k2:money-alerts@3 (config.source = "name" or
  "name::host"; config.sources = "a, b" for several, merged by item key).
  @3 (0.46.4) is @2 without "Ask an agent" (k2.sources.ask is retired).
  @2 (0.46.1) says "soft crit 2/3", reads K2 Web's ops shapes (boxes with
  customer / needs_go, edge relays, billing), shows a named list with
  config.list, and subscribes again by itself after a refusal. @1 and @2
  still run as shipped. On its first start 0.46.4 moved them to @3 once,
  in each Garden file it safely could; a locked Garden or one with errors
  was left as it was and is listed in k2 zen news. A custom widget asks
  for cap sources:read. Its errors: k2 zen widget errors <name>.
EOF
}
# END_SOURCES_B2

