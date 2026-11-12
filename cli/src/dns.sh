# ── K2 DNS K1 — `k2 dns` command group ───────────────────────────────
#
# Thin client over daemon `/cli/dns/*` (control-plane proxy). Agents never
# hold the tunnel token; the daemon gates on dns_manage_allowed_for_path
# (Settings → K2 Connect master, or Workspaces → Allow DNS manage) and
# stamps identity from the scoped principal (Wave 0). Prefer UDS when
# available; POST JSON for mutations.
#
# Exit codes: 0 ok · 1 error · 2 bad usage · 3 gated/forbidden
# (mirror mail: server-enforced toggle/owner surfaces).

_dns_err() {
    local code="$1" hint="$2"
    CODE="$code" HINT="$hint" python3 -c 'import json,os
print(json.dumps({"error": {"code": os.environ["CODE"], "hint": os.environ["HINT"]}},
                 ensure_ascii=False, separators=(",", ":")))' >&2 \
        || echo "Error ($code): $hint" >&2
}

_dns_is_uint() {
    case "$1" in
        ''|*[!0-9]*) return 1 ;;
        *) return 0 ;;
    esac
}

# Shared response formatter / exit-code mapper for every dns verb.
# Inputs: RESP (raw body), AS_JSON (true|false), D_VERB (access|list|records|add|remove|verify).
_dns_handle_resp() {
    RESP="${RESP:-}" AS_JSON="${AS_JSON:-false}" D_VERB="${D_VERB:-}" python3 - <<'PYEOF'
import json, os, sys

for _s in (sys.stdout, sys.stderr):
    try:
        _s.reconfigure(encoding="utf-8")
    except Exception:
        pass

raw = os.environ.get("RESP", "")
as_json = os.environ.get("AS_JSON") == "true"
verb = os.environ.get("D_VERB", "")

def dumps(obj):
    return json.dumps(obj, ensure_ascii=False, separators=(",", ":"))

# Teaching text when the local toggle is off (must name Settings path).
# GH#32 — single canonical owner path, shared with daemon DNS_DENIED_HINT
# and k2 dns --help exit-3 prose.
DNS_GATED_HINT = (
    "this agent isn't allowed to manage DNS — the owner can enable it in "
    "Settings \u2192 K2 Connect (Allow agents to manage DNS records) or "
    "Workspaces \u2192 (workspace) \u2192 Allow DNS manage"
)

GATED_CODES = frozenset({
    "gated", "forbidden", "dns_manage_disabled", "owner_only",
    "dns_zone_not_attached",
})
USAGE_CODES = frozenset({
    "usage", "type_not_allowed",
})
# A8.1: the zone is in the k2.dev account but its registrar nameservers do
# not point at k2.dev yet. Only a human can fix that (exit 3), and it is
# not a Settings toggle, so no Settings teaching is appended.
PENDING_NS_CODES = frozenset({"zone_pending_ns"})
PENDING_NS_HINT = (
    "Pending: this zone's nameservers don't point at k2.dev yet — set them at "
    "the registrar (k2 domain list shows which). The server re-checks by itself; "
    "to check now: k2 domain refresh <apex>"
)
# DN12c: k2.dev holds the zone read-only (winddown / suspended). A plan or
# account matter for a human (exit 3), not a Settings toggle.
READONLY_CODES = frozenset({"zone_readonly"})

def die(err, exit_code=None):
    if isinstance(err, str):
        err = {"code": "error", "hint": err}
    code = err.get("code", "error") if isinstance(err, dict) else "error"
    hint = err.get("hint", "") if isinstance(err, dict) else str(err)
    if code in PENDING_NS_CODES and not hint.startswith("Pending:"):
        hint = PENDING_NS_HINT + ((" — " + hint) if hint and hint != code else "")
    if code in GATED_CODES and "Settings" not in hint:
        hint = (hint + " — " if hint else "") + DNS_GATED_HINT
    sys.stderr.write(dumps({"error": {"code": code, "hint": hint}}) + "\n")
    if exit_code is None:
        if code in GATED_CODES or code in PENDING_NS_CODES or code in READONLY_CODES:
            exit_code = 3
        elif code in USAGE_CODES:
            exit_code = 2
        else:
            exit_code = 1
    sys.exit(exit_code)

if not raw or not raw.strip():
    die({"code": "daemon_unreachable",
         "hint": "no response from the K2 daemon (is it running?)"})

try:
    d = json.loads(raw)
except Exception:
    die({"code": "bad_response",
         "hint": "daemon returned an unparseable response: %r" % raw[:200]})

# Normalize string errors.
err = d.get("error")
if err is not None:
    if isinstance(err, str):
        err = {"code": "error", "hint": err}
    elif not isinstance(err, dict):
        err = {"code": "error", "hint": str(err)}
    die(err)

# Mutations / gated list paths: ok:false without error object.
if d.get("ok") is False:
    die({"code": "error", "hint": "unexpected daemon response: %s" % dumps(d)[:200]})

# access: capability probe — allowed:false is still exit 0 (info).
if verb == "access":
    if as_json:
        print(dumps(d))
        sys.exit(0)
    allowed = d.get("allowed")
    if allowed is None and isinstance(d.get("capability"), dict):
        allowed = d["capability"].get("allowed")
    print("dns manage : %s" % ("allowed" if allowed else "DENIED"))
    if not allowed:
        print("hint       : %s" % (d.get("hint") or DNS_GATED_HINT))
    ws = d.get("workspace") or {}
    if ws.get("path") or ws.get("id"):
        print("workspace  : %s" % (ws.get("path") or ws.get("id") or "?"))
    types = d.get("record_types") or []
    if types:
        print("types      : %s" % ", ".join(str(t) for t in types))
    zones = d.get("zones") or []
    print("zones      : %d" % len(zones))
    for z in zones:
        if isinstance(z, dict):
            print("  %-28s  %s" % (z.get("domain") or "?", z.get("id") or ""))
        else:
            print("  %s" % z)
    sys.exit(0)

if verb == "list":
    if as_json:
        print(dumps(d))
        sys.exit(0)
    zones = d.get("zones") or []
    if not zones:
        # GH#35 — empty list when manage is on: zone lifecycle is dashboard-only.
        print("(no DNS zones)")
        print("hint: add a zone in the K2 Connect dashboard (https://k2.dev), then delegate NS to ns1.k2.dev / ns2.k2.dev — agents cannot create zones")
        sys.exit(0)
    print("%-28s  %-12s  %s" % ("DOMAIN", "STATUS", "ID"))
    for z in zones:
        if not isinstance(z, dict):
            print(str(z))
            continue
        status = z.get("status") or z.get("state") or "?"
        print("%-28s  %-12s  %s" % (
            z.get("domain") or "?",
            status,
            z.get("id") or "",
        ))
        # k2.dev's list carries the raw word (`pending`); the box says `pending_ns`.
        if status in ("pending_ns", "pending", "error"):
            ns = [str(n) for n in (z.get("nameservers") or []) if n]
            print("  Pending: point nameservers to %s (record writes refused until active; "
                  "this server re-checks by itself)" % (
                ", ".join(ns) if ns else "the k2.dev nameservers (k2 domain list shows them)"))
    sys.exit(0)

if verb == "records":
    if as_json:
        print(dumps(d))
        sys.exit(0)
    recs = d.get("records")
    if recs is None and isinstance(d.get("zone"), dict):
        recs = d["zone"].get("records")
    if recs is None:
        recs = []
    domain = d.get("domain")
    if not domain and isinstance(d.get("zone"), dict):
        domain = d["zone"].get("domain")
    domain = domain or "?"
    print("zone: %s" % domain)
    if not recs:
        print("(no records)")
        sys.exit(0)
    print("%-6s  %-24s  %-8s  %s" % ("TYPE", "NAME", "TTL", "VALUE"))
    for r in recs:
        if not isinstance(r, dict):
            print(str(r))
            continue
        val = r.get("content") or r.get("value") or ""
        extra = ""
        if r.get("prio") is not None:
            extra = " (prio %s)" % r.get("prio")
        elif r.get("priority") is not None:
            extra = " (prio %s)" % r.get("priority")
        rid = r.get("id") or ""
        line = "%-6s  %-24s  %-8s  %s%s" % (
            r.get("type") or "?",
            r.get("name") or "@",
            r.get("ttl") if r.get("ttl") is not None else "?",
            val,
            extra,
        )
        if rid:
            line += "  [%s]" % rid
        mb = r.get("managed_by")
        if mb and mb != "user":
            line += "  (managed_by=%s)" % mb
        print(line)
    sys.exit(0)

if verb == "add":
    if as_json:
        print(dumps(d))
        sys.exit(0)
    rec = d.get("record") if isinstance(d.get("record"), dict) else d
    rid = rec.get("id") or d.get("id") or "?"
    rtype = rec.get("type") or "?"
    name = rec.get("name") or "@"
    content = rec.get("content") or rec.get("value") or ""
    print("added %s %s \u2192 %s  (id %s)" % (rtype, name, content, rid))
    if d.get("propagation"):
        print("propagation: %s" % d["propagation"])
    sys.exit(0)

if verb == "remove":
    if as_json:
        print(dumps(d))
        sys.exit(0)
    rid = d.get("id") or d.get("record_id") or "?"
    print("removed record %s" % rid)
    sys.exit(0)

if verb == "verify":
    if as_json:
        print(dumps(d))
        sys.exit(0)
    status = d.get("status") or d.get("state") or ("ok" if d.get("ok") else "?")
    domain = d.get("domain")
    if not domain and isinstance(d.get("zone"), dict):
        domain = d["zone"].get("domain")
    if domain:
        print("verify %s: %s" % (domain, status))
    else:
        print("verify: %s" % status)
    if d.get("hint"):
        print("hint: %s" % d["hint"])
    details = d.get("checks") or d.get("results") or []
    for c in details:
        if isinstance(c, dict):
            print("  %-12s %s" % (c.get("status") or c.get("ok") or "?", c.get("name") or c.get("check") or dumps(c)))
        else:
            print("  %s" % c)
    sys.exit(0)

# Fallback: raw JSON
print(dumps(d) if as_json else raw)
sys.exit(0)
PYEOF
}

cmd_dns_access() {
    if _wants_help "$@"; then cmd_help_dns_access; exit 0; fi
    local as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _dns_err "usage" "unknown flag '$1' — usage: k2 dns access [--json]"
                exit 2 ;;
            *)
                _dns_err "usage" "unexpected argument '$1' — usage: k2 dns access [--json]"
                exit 2 ;;
        esac
    done
    local RESP
    RESP=$(cli_send cli_request "/cli/dns/access") || RESP=""
    local AS_JSON="$as_json" D_VERB="access"
    _dns_handle_resp
}

cmd_dns_list() {
    if _wants_help "$@"; then cmd_help_dns_list; exit 0; fi
    local as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _dns_err "usage" "unknown flag '$1' — usage: k2 dns list [--json]"
                exit 2 ;;
            *)
                _dns_err "usage" "unexpected argument '$1' — usage: k2 dns list [--json]"
                exit 2 ;;
        esac
    done
    local RESP
    RESP=$(cli_send cli_request "/cli/dns/zones") || RESP=""
    local AS_JSON="$as_json" D_VERB="list"
    _dns_handle_resp
}

cmd_dns_records() {
    if _wants_help "$@"; then cmd_help_dns_records; exit 0; fi
    local domain="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _dns_err "usage" "unknown flag '$1' — usage: k2 dns records <domain> [--json]"
                exit 2 ;;
            *)
                if [ -z "$domain" ]; then domain="$1"; else
                    _dns_err "usage" "unexpected extra argument '$1' — one <domain> only"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$domain" ]; then
        _dns_err "usage" "records requires a <domain> (daemon resolves the zone)"
        exit 2
    fi
    local RESP
    RESP=$(cli_send cli_request "/cli/dns/records" "domain=$(urlencode "$domain")") || RESP=""
    local AS_JSON="$as_json" D_VERB="records"
    _dns_handle_resp
}

cmd_dns_record_add() {
    if _wants_help "$@"; then cmd_help_dns_record_add; exit 0; fi
    local domain="" rtype="" name="" value="" ttl="" priority="" as_json=false
    local positionals=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --ttl)
                [ $# -ge 2 ] || { _dns_err "usage" "--ttl requires a number"; exit 2; }
                ttl="$2"; shift 2 ;;
            --priority|--prio)
                [ $# -ge 2 ] || { _dns_err "usage" "--priority requires a number"; exit 2; }
                priority="$2"; shift 2 ;;
            --json) as_json=true; shift ;;
            -*)
                _dns_err "usage" "unknown flag '$1' — usage: k2 dns record add <domain> <type> <name> <value> [--ttl N] [--priority N] [--json]"
                exit 2 ;;
            *)
                positionals+=("$1"); shift ;;
        esac
    done
    if [ ${#positionals[@]} -lt 4 ]; then
        _dns_err "usage" "record add requires <domain> <type> <name> <value>"
        exit 2
    fi
    if [ ${#positionals[@]} -gt 4 ]; then
        _dns_err "usage" "unexpected extra argument '${positionals[4]}' — quote multi-word values"
        exit 2
    fi
    domain="${positionals[0]}"
    rtype="${positionals[1]}"
    name="${positionals[2]}"
    value="${positionals[3]}"
    if [ -n "$ttl" ] && ! _dns_is_uint "$ttl"; then
        _dns_err "usage" "--ttl must be a non-negative integer"
        exit 2
    fi
    if [ -n "$priority" ] && ! _dns_is_uint "$priority"; then
        _dns_err "usage" "--priority must be a non-negative integer"
        exit 2
    fi
    local body RESP
    body=$(D_DOMAIN="$domain" D_TYPE="$rtype" D_NAME="$name" D_VALUE="$value" \
           D_TTL="$ttl" D_PRIO="$priority" python3 -c 'import json,os
b={"domain":os.environ["D_DOMAIN"],"type":os.environ["D_TYPE"],
   "name":os.environ["D_NAME"],"value":os.environ["D_VALUE"]}
t=os.environ.get("D_TTL","").strip()
if t: b["ttl"]=int(t)
p=os.environ.get("D_PRIO","").strip()
if p: b["priority"]=int(p)
print(json.dumps(b))')
    RESP=$(cli_send cli_post_json "/cli/dns/records/add" "$body") || RESP=""
    local AS_JSON="$as_json" D_VERB="add"
    _dns_handle_resp
}

cmd_dns_record_remove() {
    if _wants_help "$@"; then cmd_help_dns_record_remove; exit 0; fi
    local record_id="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _dns_err "usage" "unknown flag '$1' — usage: k2 dns record remove <record-id> [--json]"
                exit 2 ;;
            *)
                if [ -z "$record_id" ]; then record_id="$1"; else
                    _dns_err "usage" "unexpected extra argument '$1' — one <record-id> only"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$record_id" ]; then
        _dns_err "usage" "record remove requires a <record-id> (from 'k2 dns records <domain>')"
        exit 2
    fi
    local body RESP
    body=$(D_ID="$record_id" python3 -c 'import json,os; print(json.dumps({"id": os.environ["D_ID"]}))')
    RESP=$(cli_send cli_post_json "/cli/dns/records/remove" "$body") || RESP=""
    local AS_JSON="$as_json" D_VERB="remove"
    _dns_handle_resp
}

cmd_dns_verify() {
    if _wants_help "$@"; then cmd_help_dns_verify; exit 0; fi
    local domain="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _dns_err "usage" "unknown flag '$1' — usage: k2 dns verify <domain> [--json]"
                exit 2 ;;
            *)
                if [ -z "$domain" ]; then domain="$1"; else
                    _dns_err "usage" "unexpected extra argument '$1' — one <domain> only"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$domain" ]; then
        _dns_err "usage" "verify requires a <domain>"
        exit 2
    fi
    local body RESP
    body=$(D_DOMAIN="$domain" python3 -c 'import json,os; print(json.dumps({"domain": os.environ["D_DOMAIN"]}))')
    RESP=$(cli_send cli_post_json "/cli/dns/verify" "$body") || RESP=""
    local AS_JSON="$as_json" D_VERB="verify"
    _dns_handle_resp
}

cmd_dns_record() {
    case "${1:-}" in
        add)    shift; cmd_dns_record_add "$@" ;;
        remove|rm|delete|del) shift; cmd_dns_record_remove "$@" ;;
        --help|-h|'')
            cmd_help_dns_record
            exit 0 ;;
        *)
            _dns_err "usage" "unknown record command '${1:-}' — use add | remove (see k2 dns record --help)"
            exit 2 ;;
    esac
}

cmd_help_dns() {
    cat <<'EOF'
k2 dns — manage DNS records for zones on this K2 Connect account (toggle-gated)

Agents can list zones, list/add/remove user-managed records, and verify
delegation when the owner opts in. Zone create/delete is owner/dashboard-only.

PRODUCT BOUNDARY (agents: pick the right tool)
  Custom zone records (A/TXT/MX/…)     → k2 dns (this family; zone must already
                                           exist, be attached on this server, and
                                           be delegated; manage toggle ON)
  Attach apex / hostnames on this box  → k2 domain (owner; not hostmail domain)
  Box certs for attached hostnames     → k2 cert (not hostmail cert renew)
  Nested label.user.k2.dev + target    → k2 publish subdomain
  Device tunnel to primary *.k2.dev    → k2 tunnel
  Hosted mail domain / MX recipe       → k2 hostmail domain (owner)

USAGE
  k2 dns <command> [args] [flags]

COMMANDS
  access                      Capability envelope (allowed? types? zones summary)
  list                        List DNS zones for the account
  records <domain>            List records (daemon resolves zone from domain)
  record add <domain> <type> <name> <value> [--ttl N] [--priority N]
                              Insert a user-managed record (A|AAAA|CNAME|TXT|MX|SRV|CAA)
  record remove <record-id>   Delete a user-managed record by id
  verify <domain>             On-demand delegation check

GLOBAL FLAGS
  --json                      Machine-readable output (single JSON object)

OUTPUT CONTRACT
  Success output is human-readable by default; --json switches it to JSON.
  ERRORS are ALWAYS JSON on stderr — {"error":{"code","hint"}} — regardless
  of --json.

EXIT CODES
  0 ok
  1 error (proxy/upstream/not found/etc.)
  2 bad usage (missing args, disallowed record type)
  3 gated/forbidden — DNS manage is off; ask your human to enable it in
    Settings → K2 Connect (Allow agents to manage DNS records) or
    Workspaces → (workspace) → Allow DNS manage
    (same path as the access-denied `hint` — GH#32)

NS and zone lifecycle are not available to agents.

Run 'k2 dns <command> --help' for details on a command.
EOF
}

cmd_help_dns_access() {
    cat <<'EOF'
Capability probe for DNS manage on this workspace.

USAGE
  k2 dns access [--json]

When the toggle is OFF, returns allowed:false without dialing the control
plane (no zone existence oracle). When ON, returns live capability + zones
summary from the control plane.

Exit 0 even when allowed:false (capability read succeeded). Mutations
still exit 3 when gated.

FLAGS
  --json                 JSON capability envelope
EOF
}

cmd_help_dns_list() {
    cat <<'EOF'
List DNS zones for the connected K2 account.

USAGE
  k2 dns list [--json]

Requires DNS manage allowed for this workspace (else exit 3).

FLAGS
  --json                 JSON zones list
EOF
}

cmd_help_dns_records() {
    cat <<'EOF'
List DNS records for a zone. Pass a domain; the daemon resolves the zone id.

USAGE
  k2 dns records <domain> [--json]

ARGUMENTS
  <domain>               Zone domain (e.g. example.com)

FLAGS
  --json                 JSON records list
EOF
}

cmd_help_dns_record() {
    cat <<'EOF'
Add or remove a user-managed DNS record.

USAGE
  k2 dns record add <domain> <type> <name> <value> [--ttl N] [--priority N] [--json]
  k2 dns record remove <record-id> [--json]

See 'k2 dns record add --help' / 'k2 dns record remove --help'.
EOF
}

cmd_help_dns_record_add() {
    cat <<'EOF'
Insert a user-managed DNS record. Agents may use: A AAAA CNAME TXT MX SRV CAA.
NS and zone apex lifecycle are owner-only.

USAGE
  k2 dns record add <domain> <type> <name> <value> [flags]

ARGUMENTS
  <domain>               Zone domain (daemon resolves the zone)
  <type>                 A | AAAA | CNAME | TXT | MX | SRV | CAA
  <name>                 Relative name (@ for apex)
  <value>                Record content (IP, host, text, …)

FLAGS
  --ttl <N>              TTL seconds (default 300 server-side)
  --priority <N>         Priority / prio (MX/SRV)
  --json                 JSON response

EXAMPLES
  k2 dns record add example.com A www 203.0.113.10 --ttl 60
  k2 dns record add example.com MX @ mail.example.com --priority 10
  k2 dns record add example.com TXT @ "v=spf1 include:_spf.example.com ~all"
EOF
}

cmd_help_dns_record_remove() {
    cat <<'EOF'
Delete a user-managed DNS record by id (from 'k2 dns records <domain>').
Automation-managed rows (managed_by != user) are refused.

USAGE
  k2 dns record remove <record-id> [--json]

ARGUMENTS
  <record-id>            Record id from the records list

FLAGS
  --json                 JSON response
EOF
}

cmd_help_dns_verify() {
    cat <<'EOF'
On-demand DNS delegation check for a zone domain.

USAGE
  k2 dns verify <domain> [--json]

ARGUMENTS
  <domain>               Zone domain to verify

FLAGS
  --json                 JSON verification result
EOF
}

