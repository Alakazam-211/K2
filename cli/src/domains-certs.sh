# ── Custom domains / certs (prd-custom-domains-and-certs-v1) ──────────
# `k2 domain` is box inventory (attach apex + hostnames). Not
# `k2 hostmail domain` (mail MX recipe). Tool id is locked `dns`.

_dom_err() {
    local code="$1" hint="$2"
    CODE="$code" HINT="$hint" python3 -c 'import json,os
print(json.dumps({"error": {"code": os.environ["CODE"], "hint": os.environ["HINT"]}},
                 ensure_ascii=False, separators=(",", ":")))' >&2 \
        || echo "Error ($code): $hint" >&2
}

_dom_handle_resp() {
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

GATED_CODES = frozenset({
    "gated", "forbidden", "dns_manage_disabled", "owner_only",
    "dns_zone_not_attached",
})
USAGE_CODES = frozenset({"usage", "invalid_domain", "invalid_role", "hostname_not_under_apex"})

def die(err, exit_code=None):
    if isinstance(err, str):
        err = {"code": "error", "hint": err}
    code = err.get("code", "error") if isinstance(err, dict) else "error"
    hint = err.get("hint", "") if isinstance(err, dict) else str(err)
    sys.stderr.write(dumps({"error": {"code": code, "hint": hint}}) + "\n")
    if exit_code is None:
        if code in GATED_CODES:
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

err = d.get("error")
if err is not None:
    if isinstance(err, str):
        err = {"code": "error", "hint": err}
    elif not isinstance(err, dict):
        err = {"code": "error", "hint": str(err)}
    die(err)

if d.get("ok") is False:
    die({"code": "error", "hint": "unexpected daemon response: %s" % dumps(d)[:200]})

if as_json:
    print(dumps(d))
    sys.exit(0)

# A8.1: a zone k2.dev holds but whose registrar NS do not point at
# k2.dev yet. Records stay read-only until `k2 domain refresh` flips it.
def is_pending(row):
    return isinstance(row, dict) and row.get("status") == "pending_ns"

def dns_label(row):
    if is_pending(row):
        return "pending"
    if row.get("readOnly"):
        return "readonly"
    return "write" if row.get("dnsWrite") else "byo"

def pending_lines(row, indent="  "):
    apex = (row.get("apex") or "<apex>") if isinstance(row, dict) else "<apex>"
    if isinstance(row, dict) and row.get("zoneMissing"):
        # DN12d: the zone id was not in k2.dev's last list for this server.
        print("%sNot found on k2.dev for this server (removed, or moved to another "
              "account) — k2 domain refresh %s, or remove and add it again" % (indent, apex))
    if isinstance(row, dict) and row.get("readOnly"):
        print("%sRead-only: %s on k2.dev — records serve, writes are refused (zone_readonly)" % (
            indent, row.get("status") or "on hold"))
    if not is_pending(row):
        return
    ns = [str(n) for n in (row.get("nameservers") or []) if n]
    print("%sPending: point nameservers to %s" % (
        indent, ", ".join(ns) if ns else "the k2.dev nameservers (see your k2.dev dashboard)"))
    if row.get("created"):
        print("%sAuto-added to your k2.dev account" % indent)
    print("%sThis server re-checks with k2.dev by itself; to check now: k2 domain refresh %s" % (
        indent, apex))

def renew_suffix(row):
    # K2-issued certificate renewal (daemon renews 30 days before expiry).
    r = row.get("renewal") if isinstance(row, dict) else None
    if not isinstance(r, dict) or not r.get("k2Issued"):
        return ""
    s = "  renew %s" % (r.get("state") or "?")
    if r.get("failures"):
        s += " (%s failed; next %s)" % (r.get("failures"), r.get("nextAttempt") or "?")
    return s

def app_suffix(row):
    # 0.46.6 (CD-A5): an App name says whether anything serves it. A cert
    # in the store is not a served name.
    a = row.get("app") if isinstance(row, dict) else None
    if not isinstance(a, dict):
        return ""
    if a.get("error") == "unknown_app":
        return "  (not served: no App %s here)" % (a.get("app") or "?")
    if a.get("external"):
        return "  (served by your own web server, not by K2%s)" % ("; primary" if a.get("primary") else "")
    if a.get("notServedYet"):
        s = "  (not served yet: %s)" % (a.get("state") or "waiting_dns")
    else:
        s = "  (served: live%s)" % (", primary" if a.get("primary") else "")
    if a.get("served") == "elsewhere":
        s += "  :443 answers with another server's certificate"
    if a.get("warning"):
        s += "  warning: %s" % a.get("warning")
    return s

def state_words(row):
    if is_pending(row):
        return "pending nameservers"
    if row.get("readOnly"):
        return "K2-hosted DNS (read-only: %s on k2.dev)" % (row.get("status") or "on hold")
    return "K2-hosted DNS (writable)" if row.get("dnsWrite") else "BYO DNS (inventory only)"

if verb == "add":
    row = d.get("domain") if isinstance(d.get("domain"), dict) else {}
    print("attached %s — %s" % (row.get("apex") or "?", state_words(row)))
    pending_lines(row)
    sys.exit(0)

if verb == "refresh":
    rows = [r for r in (d.get("domains") or []) if isinstance(r, dict)]
    if not rows:
        print("(no pending domains to check)")
    for row in rows:
        print("%-28s  %s" % (row.get("apex") or "?", state_words(row)))
        pending_lines(row)
    if d.get("checked") is False:
        print("not checked: %s" % (d.get("hint") or "no K2 Connect tunnel token"))
    sys.exit(0)

if verb == "list":
    domains = d.get("domains") or []
    if not domains:
        print("(no custom domains attached to this server)")
        print("hint: k2 domain add <apex> — attach ≠ nameserver transfer")
        sys.exit(0)
    print("%-28s  %-8s  %s" % ("APEX", "DNS", "ZONE"))
    for row in domains:
        if not isinstance(row, dict):
            print(str(row))
            continue
        print("%-28s  %-8s  %s" % (
            row.get("apex") or "?",
            dns_label(row),
            row.get("zoneId") or "-",
        ))
        pending_lines(row)
        for n in row.get("names") or []:
            if not isinstance(n, dict):
                continue
            cert = (n.get("cert") or {}).get("state") or "missing"
            print("  %-26s  %-8s  cert %s%s%s" % (n.get("hostname") or "?", n.get("role") or "other", cert, renew_suffix(n), app_suffix(n)))
    sys.exit(0)

if verb == "certs":
    certs = d.get("certs") or []
    if not certs:
        print("(no attached hostnames)")
        sys.exit(0)
    print("%-32s  %-8s  %s" % ("HOSTNAME", "ROLE", "CERT"))
    for c in certs:
        if not isinstance(c, dict):
            print(str(c))
            continue
        cert = (c.get("cert") or {}).get("state") or "missing"
        print("%-32s  %-8s  %s%s%s" % (c.get("hostname") or "?", c.get("role") or "other", cert, renew_suffix(c), app_suffix(c)))
    sys.exit(0)

# remove / name — compact success
if verb in ("remove", "name-add", "name-remove", "issue", "renew"):
    print(dumps(d) if as_json else dumps({k: d[k] for k in d if k != "ok"} or d))
    sys.exit(0)

print(dumps(d))
sys.exit(0)
PYEOF
}

cmd_domain_list() {
    if _wants_help "$@"; then cmd_help_domain_list; exit 0; fi
    local as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _dom_err "usage" "unknown flag '$1' — usage: k2 domain list [--json]"
                exit 2 ;;
            *)
                _dom_err "usage" "unexpected argument '$1' — usage: k2 domain list [--json]"
                exit 2 ;;
        esac
    done
    local RESP
    RESP=$(cli_send cli_request "/cli/domains") || RESP=""
    local AS_JSON="$as_json" D_VERB="list"
    _dom_handle_resp
}

cmd_domain_add() {
    if _wants_help "$@"; then cmd_help_domain_add; exit 0; fi
    local apex="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _dom_err "usage" "unknown flag '$1' — usage: k2 domain add <apex> [--json]"
                exit 2 ;;
            *)
                if [ -z "$apex" ]; then apex="$1"; else
                    _dom_err "usage" "unexpected extra argument '$1' — one <apex> only"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$apex" ]; then
        _dom_err "usage" "domain add requires an <apex> (example.com) — this is not k2 hostmail domain"
        exit 2
    fi
    local body RESP
    body=$(D_APEX="$apex" python3 -c 'import json,os; print(json.dumps({"apex": os.environ["D_APEX"]}))')
    RESP=$(cli_send cli_post_json "/cli/domains" "$body") || RESP=""
    local AS_JSON="$as_json" D_VERB="add"
    _dom_handle_resp
}

cmd_domain_remove() {
    if _wants_help "$@"; then cmd_help_domain_remove; exit 0; fi
    local apex="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _dom_err "usage" "unknown flag '$1' — usage: k2 domain remove <apex> [--json]"
                exit 2 ;;
            *)
                if [ -z "$apex" ]; then apex="$1"; else
                    _dom_err "usage" "unexpected extra argument '$1'"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$apex" ]; then
        _dom_err "usage" "domain remove requires an <apex>"
        exit 2
    fi
    local body RESP
    body=$(D_APEX="$apex" python3 -c 'import json,os; print(json.dumps({"apex": os.environ["D_APEX"]}))')
    RESP=$(cli_send cli_post_json "/cli/domains/remove" "$body") || RESP=""
    local AS_JSON="$as_json" D_VERB="remove"
    _dom_handle_resp
}

cmd_domain_refresh() {
    if _wants_help "$@"; then cmd_help_domain_refresh; exit 0; fi
    local apex="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _dom_err "usage" "unknown flag '$1' — usage: k2 domain refresh [<apex>] [--json]"
                exit 2 ;;
            *)
                if [ -z "$apex" ]; then apex="$1"; else
                    _dom_err "usage" "unexpected extra argument '$1' — one <apex> only"
                    exit 2
                fi
                shift ;;
        esac
    done
    local body RESP
    body=$(D_APEX="$apex" python3 -c 'import json,os
a=os.environ.get("D_APEX","").strip()
print(json.dumps({"apex": a} if a else {}))')
    RESP=$(cli_send cli_post_json "/cli/domains/refresh" "$body") || RESP=""
    local AS_JSON="$as_json" D_VERB="refresh"
    _dom_handle_resp
}

cmd_domain_name_add() {
    if _wants_help "$@"; then cmd_help_domain_name_add; exit 0; fi
    local hostname="" role="" as_json=false external=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --role)
                [ $# -ge 2 ] || { _dom_err "usage" "--role requires mail|publish|direct|other|app:<name>"; exit 2; }
                role="$2"; shift 2 ;;
            --external) external=1; shift ;;
            --json) as_json=true; shift ;;
            -*)
                _dom_err "usage" "unknown flag '$1' — usage: k2 domain name add <hostname> [--role mail|publish|direct|other|app:<name>] [--external] [--json]"
                exit 2 ;;
            *)
                if [ -z "$hostname" ]; then hostname="$1"; else
                    _dom_err "usage" "unexpected extra argument '$1'"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$hostname" ]; then
        _dom_err "usage" "domain name add requires a <hostname>"
        exit 2
    fi
    local body RESP
    body=$(D_HOST="$hostname" D_ROLE="$role" D_EXTERNAL="$external" python3 -c 'import json,os
b={"hostname": os.environ["D_HOST"]}
r=os.environ.get("D_ROLE","").strip()
if r: b["role"]=r
if os.environ.get("D_EXTERNAL") == "1": b["external"]=True
print(json.dumps(b))')
    RESP=$(cli_send cli_post_json "/cli/domains/names" "$body") || RESP=""
    local AS_JSON="$as_json" D_VERB="name-add"
    _dom_handle_resp
}

cmd_domain_name_remove() {
    if _wants_help "$@"; then cmd_help_domain_name_remove; exit 0; fi
    local hostname="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _dom_err "usage" "unknown flag '$1' — usage: k2 domain name remove <hostname> [--json]"
                exit 2 ;;
            *)
                if [ -z "$hostname" ]; then hostname="$1"; else
                    _dom_err "usage" "unexpected extra argument '$1'"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$hostname" ]; then
        _dom_err "usage" "domain name remove requires a <hostname>"
        exit 2
    fi
    local body RESP
    body=$(D_HOST="$hostname" python3 -c 'import json,os; print(json.dumps({"hostname": os.environ["D_HOST"]}))')
    RESP=$(cli_send cli_post_json "/cli/domains/names/remove" "$body") || RESP=""
    local AS_JSON="$as_json" D_VERB="name-remove"
    _dom_handle_resp
}

cmd_domain_name() {
    case "${1:-}" in
        add) shift; cmd_domain_name_add "$@" ;;
        remove|rm|delete|del) shift; cmd_domain_name_remove "$@" ;;
        --help|-h|'') cmd_help_domain_name; exit 0 ;;
        *)
            _dom_err "usage" "unknown name command '${1:-}' — use add | remove (see k2 domain name --help)"
            exit 2 ;;
    esac
}

cmd_domain() {
    case "${1:-}" in
        list) shift; cmd_domain_list "$@" ;;
        add) shift; cmd_domain_add "$@" ;;
        remove|rm|delete|del) shift; cmd_domain_remove "$@" ;;
        refresh|check) shift; cmd_domain_refresh "$@" ;;
        name) shift; cmd_domain_name "$@" ;;
        --help|-h|'') cmd_help_domain; exit 0 ;;
        *)
            _dom_err "usage" "unknown command '${1:-}' — see k2 domain --help (not k2 hostmail domain)"
            exit 2 ;;
    esac
}

cmd_cert_list() {
    if _wants_help "$@"; then cmd_help_cert_list; exit 0; fi
    local as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _dom_err "usage" "unknown flag '$1' — usage: k2 cert list [--json]"
                exit 2 ;;
            *)
                _dom_err "usage" "unexpected argument '$1' — usage: k2 cert list [--json]"
                exit 2 ;;
        esac
    done
    local RESP
    RESP=$(cli_send cli_request "/cli/certs") || RESP=""
    local AS_JSON="$as_json" D_VERB="certs"
    _dom_handle_resp
}

cmd_cert_issue() {
    if _wants_help "$@"; then cmd_help_cert_issue; exit 0; fi
    local hostname="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _dom_err "usage" "unknown flag '$1' — usage: k2 cert issue <hostname> [--json]"
                exit 2 ;;
            *)
                if [ -z "$hostname" ]; then hostname="$1"; else
                    _dom_err "usage" "unexpected extra argument '$1'"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$hostname" ]; then
        _dom_err "usage" "cert issue requires an attached <hostname>"
        exit 2
    fi
    local body RESP
    body=$(D_HOST="$hostname" python3 -c 'import json,os; print(json.dumps({"hostname": os.environ["D_HOST"]}))')
    RESP=$(cli_send cli_post_json "/cli/certs/issue" "$body") || RESP=""
    local AS_JSON="$as_json" D_VERB="issue"
    _dom_handle_resp
}

cmd_cert_renew() {
    if _wants_help "$@"; then cmd_help_cert_renew; exit 0; fi
    local hostname="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _dom_err "usage" "unknown flag '$1' — usage: k2 cert renew <hostname> [--json]"
                exit 2 ;;
            *)
                if [ -z "$hostname" ]; then hostname="$1"; else
                    _dom_err "usage" "unexpected extra argument '$1'"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$hostname" ]; then
        _dom_err "usage" "cert renew requires an attached <hostname>"
        exit 2
    fi
    local body RESP
    body=$(D_HOST="$hostname" python3 -c 'import json,os; print(json.dumps({"hostname": os.environ["D_HOST"]}))')
    RESP=$(cli_send cli_post_json "/cli/certs/renew" "$body") || RESP=""
    local AS_JSON="$as_json" D_VERB="renew"
    _dom_handle_resp
}

cmd_cert() {
    case "${1:-}" in
        list) shift; cmd_cert_list "$@" ;;
        issue) shift; cmd_cert_issue "$@" ;;
        renew) shift; cmd_cert_renew "$@" ;;
        --help|-h|'') cmd_help_cert; exit 0 ;;
        *)
            _dom_err "usage" "unknown command '${1:-}' — see k2 cert --help (box inventory; not k2 hostmail cert)"
            exit 2 ;;
    esac
}

cmd_help_domain() {
    cat <<'EOF'
k2 domain — attach custom domains to THIS server

This is box inventory: apex + hostnames this machine may terminate.
It is NOT `k2 hostmail domain` (mail MX recipe) and NOT a nameserver
cutover. Apex add/remove is owner (exit 3). Hostname add/remove and
list are dns_manage (same grant as k2 dns / k2 cert); without it an
agent's `k2 domain list` exits 3.

USAGE
  k2 domain list [--json]
  k2 domain add <apex> [--json]
  k2 domain remove <apex> [--json]
  k2 domain refresh [<apex>] [--json]
  k2 domain name add <hostname> [--role mail|publish|direct|other|app:<name>] [--json]
  k2 domain name remove <hostname> [--json]

A domain k2.dev holds but whose nameservers don't point at k2.dev yet
lists as "pending" with the nameservers to set at your registrar. DNS
record writes are refused (zone_pending_ns) until the zone is active.
The server re-checks pending domains with k2.dev by itself (every
minute at first, backing off to daily for long-pending ones) and once
more before refusing a write; `k2 domain refresh` checks right now.
A zone k2.dev winds down or suspends lists as "readonly"; writes are
refused with zone_readonly.

See also: k2 dns (records on attached K2-hosted zones), k2 cert (certs
for attached hostnames), k2 hostmail domain (mail).
EOF
}

cmd_help_domain_list() {
    cat <<'EOF'
List custom domains attached to this server. A pending row prints
"Pending: point nameservers to …" and whether k2.dev auto-added the
zone to your account. Agents need dns_manage (exit 3 without it).

USAGE
  k2 domain list [--json]
EOF
}

cmd_help_domain_refresh() {
    cat <<'EOF'
Check now with k2.dev: re-bind an attached apex so a pending zone
(nameservers not yet pointed at k2.dev) flips to active. The server
also re-checks pending domains by itself, so this is only needed to
skip the wait. With no apex,
checks every pending domain. Owner/admin; agents exit 3.

USAGE
  k2 domain refresh [<apex>] [--json]
EOF
}

cmd_help_domain_add() {
    cat <<'EOF'
Attach an apex to this server (idempotent). Does not transfer NS.
If k2.dev auto-adds the zone to your account it starts pending: point
the registrar nameservers at k2.dev, then `k2 domain refresh <apex>`.
Exit 1 with zone_owned_elsewhere when another k2.dev account owns it;
bind_api_unavailable when domain linking isn't live on k2.dev yet.

USAGE
  k2 domain add <apex> [--json]
EOF
}

cmd_help_domain_remove() {
    cat <<'EOF'
Detach an apex from this server (cascades hostnames).

USAGE
  k2 domain remove <apex> [--json]
EOF
}

cmd_help_domain_name() {
    cat <<'EOF'
Add or remove a hostname under an attached apex.

USAGE
  k2 domain name add <hostname> [--role mail|publish|direct|other|app:<name>]
  k2 domain name remove <hostname>
EOF
}

cmd_help_domain_name_add() {
    cat <<'EOF'
Add a hostname under an attached apex (dns_manage). Apex must already
be on this server (`k2 domain add` is owner). Then `k2 cert issue`.

--external (with --role app:<name>): your own web server serves this
name, not K2's front; the App treats it as its origin from its next
start (k2 app restart <name>). Same as k2 app domain add … --external.

USAGE
  k2 domain name add <hostname> [--role mail|publish|direct|other|app:<name>] [--external] [--json]
EOF
}

cmd_help_domain_name_remove() {
    cat <<'EOF'
Remove a hostname from this server's inventory.

USAGE
  k2 domain name remove <hostname> [--json]
EOF
}

cmd_help_cert() {
    cat <<'EOF'
k2 cert — certificates for hostnames attached on THIS server

This is box inventory (`k2 cert list`) and the issuer. `k2 hostmail cert
renew` uses this issuer for the mail host when it's attached here.

A role-mail hostname goes into the mail server too. Only the mail host in
`k2 hostmail status` is its default certificate, the one clients without SNI
get; any other mail host is served to clients that ask for its name.

USAGE
  k2 cert list [--json]
  k2 cert issue <hostname> [--json]
  k2 cert renew <hostname> [--json]

RENEWAL
  The daemon renews certificates K2 issued once fewer than 30 days remain
  (hourly check, per-name backoff 1h/6h/24h, skipped under air-gap).
  issue/renew by hand re-order only inside that window; outside it they
  re-plant the stored certificate. Uploaded certificates are never renewed.
  `k2 cert list` shows the renewal state per hostname.
EOF
}

cmd_help_cert_issue() {
    cat <<'EOF'
Issue a Let's Encrypt cert for an attached hostname (on-box ACME). Adding
a second mail host (`k2 domain name add mail.<apex> --role mail` first)
doesn't change the mail server's default certificate.

USAGE
  k2 cert issue <hostname> [--json]

DNS
  K2-hosted zone (ns1/ns2.k2.dev, dns_write): DNS-01 plants
  _acme-challenge here.
  Any other DNS: you must add the hostname (and _acme-challenge if
  using DNS-01) on THAT platform pointing at this box. HTTP-01 follows
  a CNAME (often onto a closed :80). K2 cannot plant on Vercel/etc.
EOF
}

cmd_help_cert_renew() {
    cat <<'EOF'
Renew the cert for an attached hostname. Never hostmail disable/enable.

USAGE
  k2 cert renew <hostname> [--json]

DNS
  Same as issue: if the zone is not on ns1/ns2.k2.dev, add records on
  the platform that actually serves DNS, then rerun. K2 cannot plant
  there.
EOF
}

cmd_help_cert_list() {
    cat <<'EOF'
List cert state for attached hostnames.

USAGE
  k2 cert list [--json]
EOF
}

