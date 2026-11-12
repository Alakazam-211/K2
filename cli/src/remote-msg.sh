# ── Cross-server agent msg over FEDERATION ────────────────────────────
# `k2 msg <agent>::<host> "text"` (canonical) or legacy `agent@host` routes a
# CONNECTED cross-server peer over federation — the SAME verb agents use for
# a local peer. This is the federation send-surface (gated on
# `is_remote_connection`), NOT the owner-remote drive-the-box path that
# `k2 talk <ws>@<host>` uses.
#
# PR1: uses scoped passport via cli_send (UDS when eligible, else TCP with
# TOKEN which for msg/ID is SCOPED_TOKEN — never disk-owner inversion).
# Daemon forces from_workspace from principal when scoped.
# PR2: prefer agent::host; wire form fp::ws::agent stays on `k2 fed send`.
#
# Args: <agent> <host> <text> <from_project>. Returns:
#   0  delivered ("sent") or durably queued ("queued")
#   3  not_connected / forbidden (mail/C2 teaching parity)
#   2  actionable user error (fed off, no peer, not trusted, no agent)
#   1  unexpected/unparseable daemon response
#
# Steps: resolve the peer by matching "<subdomain>.k2.dev" == <host> in
# /cli/federation/peers (capture fingerprint) → resolve the agent's
# workspace_id from that peer's /cli/federation/peer-roster → POST
# /cli/federation/send with to="<fp>::<wsid>::<agent>". All parsing is
# python3 (consistent with the rest of this CLI).
_send_remote_msg() {
    local agent="$1" host="$2" text="$3" from_project="$4"

    if [ -z "$agent" ]; then
        echo "k2 msg: empty agent name in '<agent>::<host>' target." >&2
        return 2
    fi
    if [ -z "$host" ]; then
        echo "k2 msg: empty host in '<agent>::<host>' target." >&2
        return 2
    fi

    # 0.46.11: `<agent>/<chat>::<host>` reaches a sidecar chat in that
    # workspace (the chat's name is slugged like a local address:
    # "Field Notes" -> field-notes). The roster lookup and the connection
    # gate use the workspace half; the receiver checks the chat exists.
    local sidecar=""
    case "$agent" in
        */*)
            sidecar="${agent#*/}"
            agent="${agent%%/*}"
            sidecar=$(printf '%s' "$sidecar" | python3 -c 'import sys
s = sys.stdin.read().strip().lower()
print("-".join(p for p in s.split() if p))')
            if [ -z "$agent" ] || [ -z "$sidecar" ] || [[ "$sidecar" == */* ]] || [[ "$sidecar" == *:* ]]; then
                echo "k2 msg: a chat on another server is <agent>/<chat>::<host> (one slash), e.g. ada/notes::box.example.k2.dev." >&2
                return 2
            fi
            ;;
    esac

    # Optional one-shot compat hint when the caller used legacy @ form.
    if [ "${_USED_LEGACY_AT:-0}" = "1" ] && [ "${_FED_AT_HINT_SHOWN:-0}" = "0" ]; then
        echo "hint: prefer \`k2 msg ${agent}::${host}\` — '@' federated addresses are legacy compat (not email)." >&2
        _FED_AT_HINT_SHOWN=1
    fi

    # 1. Resolve the peer by host (subdomain.k2.dev == host) → fingerprint.
    # cli_send: prefer cell UDS + scoped Bearer when present.
    local peers_json
    peers_json=$(cli_send cli_request "/cli/federation/peers")
    local pmatch
    pmatch=$(printf '%s' "$peers_json" | python3 -c '
import json, sys
host = (sys.argv[1] or "").strip().lower()
# Accept bare subdomain or full host.k2.dev (and collapse accidental doubles).
def bare(h):
    h = (h or "").strip().lower()
    while h.endswith(".k2.dev.k2.dev"):
        h = h[: -len(".k2.dev")]
    if h.endswith(".k2.dev"):
        h = h[: -len(".k2.dev")]
    return h
want = bare(host)
try:
    d = json.load(sys.stdin)
except Exception:
    print("ERR\tparse"); sys.exit(0)
if not isinstance(d, dict) or not isinstance(d.get("peers"), list):
    # federation off (404 {"error":...}) or unexpected shape
    err = d.get("error") if isinstance(d, dict) else None
    if isinstance(err, dict) and err.get("code") in ("forbidden", "not_connected"):
        print("FORBIDDEN\t%s" % (err.get("hint") or err.get("code") or ""))
        sys.exit(0)
    if isinstance(d, dict) and d.get("error"):
        print("NOFED\t%s" % (d.get("error") if not isinstance(d.get("error"), dict) else d["error"].get("hint") or d["error"]))
        sys.exit(0)
    print("NOFED"); sys.exit(0)
def host_port(u):
    u = (u or "").strip()
    if u.lower().startswith("https://"):
        u = u[8:]
    elif u.lower().startswith("http://"):
        u = u[7:]
    return u.split("/")[0] if u else ""
for p in d["peers"]:
    sub = p.get("subdomain") or ""
    label = p.get("label") or ""
    base = p.get("base_url") or p.get("baseUrl") or ""
    hp = host_port(base)
    if want and bare(sub) == want:
        print("OK\t%s\t%s" % (p.get("fingerprint", ""), p.get("trust", "")))
        sys.exit(0)
    # Also match full host forms stored as subdomain
    if host and sub and sub.lower() in (host, host + ".k2.dev", want + ".k2.dev"):
        print("OK\t%s\t%s" % (p.get("fingerprint", ""), p.get("trust", "")))
        sys.exit(0)
    # F5: LAN/Tailscale saved host:port (do not require .k2.dev)
    if host and (sub.lower() == host or hp.lower() == host or hp.lower() == want):
        print("OK\t%s\t%s" % (p.get("fingerprint", ""), p.get("trust", "")))
        sys.exit(0)
    if want and bare(label) == want:
        print("OK\t%s\t%s" % (p.get("fingerprint", ""), p.get("trust", "")))
        sys.exit(0)
# List known peer hosts so the NONE error is actionable (GH #48).
known = []
for p in d["peers"]:
    base = (p.get("base_url") or p.get("baseUrl") or "").strip()
    sub = (p.get("subdomain") or "").strip()
    if base:
        known.append(host_port(base) or base)
    elif sub:
        if ":" in sub or sub.endswith(".ts.net") or sub.endswith(".k2.dev"):
            known.append(sub)
        else:
            known.append(sub + ".k2.dev")
    elif p.get("label"):
        known.append(str(p.get("label")))
print("NONE\t%s" % (", ".join(known) if known else ""))
' "$host")

    local pkind fp trust
    IFS=$'\t' read -r pkind fp trust <<<"$pmatch"
    case "$pkind" in
        FORBIDDEN)
            # Scoped token rejected (should not happen post-PR1) or other gate.
            printf '%s\n' "${fp:-forbidden}" >&2
            return 3 ;;
        NOFED|ERR)
            echo "federation is off or unreachable on $host — enable it and pair first (k2 fed enable / k2 fed pair-start)." >&2
            [ -n "$fp" ] && [ "$pkind" = "NOFED" ] && echo "detail: $fp" >&2
            return 2 ;;
        NONE)
            # GH #48: inbound may have landed while this side never stored a
            # return pin, or the host string doesn't match the peer's subdomain.
            if [ -n "$fp" ]; then
                echo "no paired peer matching $host — known peers: $fp" >&2
                echo "hint: run \`k2 fed peers\` to inspect trust; if you received a message from this host but have no pin, complete pairing (k2 fed pair-start / confirm, or Settings → Connections → Pair) so replies can go back." >&2
            else
                echo "no paired peer for $host — pair it first (k2 fed pair-start / confirm), or check the host." >&2
                echo "hint: run \`k2 fed peers\` to list trusted servers on this machine." >&2
            fi
            return 2 ;;
        OK) ;;
        *)
            echo "k2 msg: could not read federation peers from the local daemon." >&2
            return 2 ;;
    esac
    if [ -z "$fp" ]; then
        echo "no paired peer for $host — pair it first (k2 fed pair-start / confirm), or check the host." >&2
        return 2
    fi
    # Trust serializes lowercase ("trusted") on the wire — compare case-insensitively.
    local trust_lc
    trust_lc=$(printf '%s' "$trust" | tr '[:upper:]' '[:lower:]')
    if [ -n "$trust_lc" ] && [ "$trust_lc" != "trusted" ]; then
        echo "peer $host is not trusted yet (state=$trust) — confirm the pairing (k2 fed confirm)." >&2
        return 2
    fi

    # 2. Resolve the agent's workspace_id from that peer's roster.
    local roster_json
    roster_json=$(cli_send cli_request "/cli/federation/peer-roster" "peer=$(urlencode "$fp")")
    local rres
    rres=$(printf '%s' "$roster_json" | python3 -c '
import json, sys
agent = sys.argv[1]
try:
    d = json.load(sys.stdin)
except Exception:
    print("ERR\tparse"); sys.exit(0)
if not isinstance(d, dict):
    print("ERR\t"); sys.exit(0)
if "error" in d:
    err = d.get("error")
    if isinstance(err, dict) and err.get("code") in ("forbidden", "not_connected"):
        print("FORBIDDEN\t%s" % (err.get("hint") or err.get("code") or ""))
        sys.exit(0)
    print("ERR\t%s" % (err if not isinstance(err, dict) else err.get("hint") or err.get("code") or err))
    sys.exit(0)
roster = d.get("roster") or {}
agents = roster.get("agents") or []

def slugify(s):
    if not s:
        return ""
    for ch in s:
        if ch in "/:\\" or ch == "\0" or (ord(ch) < 32):
            return (s or "").strip().lower()
    t = s.strip().lower().replace("_", "-")
    t = "-".join(p for p in t.split() if p)
    while "--" in t:
        t = t.replace("--", "-")
    t = t.strip("-")
    t = "".join(c for c in t if c.isalnum() or c == "-")
    while "--" in t:
        t = t.replace("--", "-")
    return t.strip("-")

def norm(s):
    g = slugify(s)
    return g if g else (s or "").strip().lower()

want = norm(agent)
matches = []
for a in agents:
    handle = (a.get("agent") or "").strip()
    aliases = a.get("aliases") or []
    wname = a.get("workspace_name") or ""
    hit = norm(handle) == want or any(norm(x) == want for x in aliases)
    if not hit and wname.strip() and norm(wname) == want:
        hit = True
    if hit:
        matches.append(a)
# workspace_name convenience: if two UUIDs match, AMBIG
ids = []
seen = set()
for a in matches:
    wid = a.get("workspace_id") or ""
    if wid not in seen:
        seen.add(wid)
        ids.append(a)
if not ids:
    names = ", ".join(a.get("agent", "?") for a in agents)
    print("NONE\t%s" % names); sys.exit(0)
if len(ids) > 1:
    info = "; ".join("%s (%s)" % (a.get("workspace_name", "?"), a.get("workspace_id", "?")) for a in ids)
    print("AMBIG\t%s" % info); sys.exit(0)
# A7: also hand back the handle when the typed name is another name of
# that workspace (alias / display name), for the note line.
handle = (ids[0].get("agent") or "").strip()
note_handle = handle if handle and norm(handle) != want else ""
print("OK\t%s\t%s" % ((ids[0].get("workspace_id", "") or ""), note_handle))
' "$agent")

    local rkind rdetail
    rkind="${rres%%$'\t'*}"
    rdetail="${rres#*$'\t'}"
    [ "$rdetail" = "$rres" ] && rdetail=""
    case "$rkind" in
        FORBIDDEN)
            # Teaching JSON shape parity with local msg/read exit 3.
            python3 -c 'import json,sys
hint=sys.argv[1] or "forbidden"
sys.stderr.write(json.dumps({"error":{"code":"forbidden","hint":hint}},
                            ensure_ascii=False,separators=(",",":"))+"\n")
' "$rdetail" 2>/dev/null || echo "${rdetail:-forbidden}" >&2
            return 3 ;;
        ERR)
            echo "could not reach $host's roster: ${rdetail:-$roster_json}" >&2
            return 2 ;;
        NONE)
            if [ -n "$rdetail" ]; then
                echo "no agent \"$agent\" on $host. Available: $rdetail." >&2
            else
                echo "no agent \"$agent\" on $host (its roster is empty)." >&2
            fi
            echo "hint: if you were connected to it before, its workspace may have been removed there. Handles are never reused; \`k2 connections list\` marks such connections GONE." >&2
            return 2 ;;
        AMBIG)
            echo "ambiguous agent \"$agent\" on $host — multiple workspaces expose it: $rdetail. Disambiguate on the host." >&2
            return 2 ;;
        OK) ;;
        *)
            echo "could not parse $host's roster." >&2
            return 2 ;;
    esac
    local wsid="${rdetail%%$'\t'*}" note_handle=""
    [ "$wsid" != "$rdetail" ] && note_handle="${rdetail#*$'\t'}"
    if [ -z "$wsid" ]; then
        echo "no resolvable workspace for agent \"$agent\" on $host." >&2
        return 2
    fi

    # 3. Send. Build the body safely with python3 (mirrors cmd_fed send).
    # from_workspace is best-effort for owner path; scoped principal forces it
    # server-side from the passport (body spoof ignored).
    local body resp wire_agent="$agent" shown="$agent"
    if [ -n "$sidecar" ]; then
        wire_agent="${agent}/${sidecar}"
        shown="${agent}/${sidecar}"
    fi
    body=$(python3 -c 'import json,sys; print(json.dumps({"to":sys.argv[1],"text":sys.argv[2],"from_workspace":sys.argv[3]}))' \
        "${fp}::${wsid}::${wire_agent}" "$text" "$from_project")
    resp=$(cli_send cli_post_json "/cli/federation/send" "$body")

    # 4. Surface the response.
    # not_connected / forbidden → exit 3 + teaching (mail/C2 parity).
    local sres skind sdetail
    sres=$(printf '%s' "$resp" | python3 -c '
import json, sys
try:
    d = json.load(sys.stdin)
except Exception:
    print("RAW"); sys.exit(0)
if not isinstance(d, dict):
    print("RAW"); sys.exit(0)
err = d.get("error")
if isinstance(err, dict):
    code = err.get("code") or ""
    hint = err.get("hint") or code
    if code in ("not_connected", "gated", "forbidden"):
        # Emit teaching JSON on stderr via GATE line; CLI maps to exit 3.
        print("GATE\t%s\t%s" % (code, hint)); sys.exit(0)
    print("ERROR\t%s" % (hint or code or err)); sys.exit(0)
if err is not None and not isinstance(err, dict):
    print("ERROR\t%s" % err); sys.exit(0)
st = d.get("status", "")
if st == "sent":
    print("SENT"); sys.exit(0)
if st == "queued":
    # 0.46.11: the reason alone (transport error); the CLI words the line.
    why = d.get("reason") or d.get("hint") or ""
    if why.startswith("delivery deferred to retry: "):
        why = why[len("delivery deferred to retry: "):]
    print("QUEUED\t%s" % " ".join(str(why).split())); sys.exit(0)
if st == "declined":
    print("DECLINED\t%s" % (d.get("hint") or d.get("reason") or "declined by peer")); sys.exit(0)
print("OTHER")
')
    skind="${sres%%$'\t'*}"
    sdetail="${sres#*$'\t'}"
    [ "$sdetail" = "$sres" ] && sdetail=""
    case "$skind" in
        SENT)
            [ -n "$note_handle" ] && _k2_addr_note "${agent}::${host}" "${note_handle}::${host}"
            echo "delivered to ${shown}::${host} (federation)"
            return 0 ;;
        QUEUED)
            # Exit 0 (the message is safe in the outbox and will go), but
            # the line says plainly it has NOT landed yet (0.46.11).
            [ -n "$note_handle" ] && _k2_addr_note "${agent}::${host}" "${note_handle}::${host}"
            if [ -n "$sdetail" ]; then
                echo "QUEUED (not delivered yet) for ${shown}::${host}: $sdetail" >&2
            else
                echo "QUEUED (not delivered yet) for ${shown}::${host}: the peer is briefly unreachable" >&2
            fi
            echo "  K2 retries it from this server's outbox (next try in about 30 s). The owner can watch it with: k2 fed outbox" >&2
            return 0 ;;
        GATE)
            # sdetail is "code\thint" after first tab split above left "code\thint"
            local gcode ghint
            gcode="${sdetail%%$'\t'*}"
            ghint="${sdetail#*$'\t'}"
            [ "$ghint" = "$sdetail" ] && ghint="$gcode"
            python3 -c 'import json,sys
code,hint=sys.argv[1],sys.argv[2]
sys.stderr.write(json.dumps({"error":{"code":code,"hint":hint}},
                            ensure_ascii=False,separators=(",",":"))+"\n")
' "$gcode" "$ghint"
            return 3 ;;
        DECLINED)
            # The address resolved and reached the peer; it said no.
            [ -n "$note_handle" ] && _k2_addr_note "${agent}::${host}" "${note_handle}::${host}"
            echo "${sdetail:-$resp}" >&2
            return 2 ;;
        ERROR)
            echo "${sdetail:-$resp}" >&2
            return 2 ;;
        *)
            echo "k2 msg: unexpected daemon response: $resp" >&2
            return 1 ;;
    esac
}

# Resolve the owner/session token for a remote Connect host. Precedence:
#   1. explicit --token (arg $2)
#   2. $K2_REMOTE_TOKEN
#   3. ~/.k2/connect-tokens.json  { "<hostname>": "<token>", ... } with
#      flexible key match (case, bare subdomain vs *.k2.dev)
#   4. OS keychain (desktop "Remember" path): resolve host id from
#      connect-hosts.json, then:
#        macOS  → security find-generic-password (dev.k2.connect.host-token)
#        Linux  → secret-tool lookup (same service name)
# Echoes the token (empty when none found). connect-hosts.json is the
# non-secret address book; connect-tokens.json is the CLI-readable store
# (0600). GH #60: tray file send used to only exact-match connect-tokens
# and ignored keychain, so a signed-in Servers host still failed.
_remote_token_for_host() {
    local host="$1" explicit="${2:-}"
    if [ -n "$explicit" ]; then printf '%s' "$explicit"; return 0; fi
    if [ -n "${K2_REMOTE_TOKEN:-}" ]; then printf '%s' "$K2_REMOTE_TOKEN"; return 0; fi
    if ! command -v python3 >/dev/null 2>&1; then
        return 0
    fi

    # Flexible connect-tokens.json + host-id from connect-hosts.json.
    # Prints either "TOKEN\t<tok>" or "HOSTID\t<id>" or nothing.
    local resolved host_id tok=""
    resolved=$(K2_HOME="$K2_HOME" python3 - "$host" <<'PY' 2>/dev/null
import json, os, sys

def bare(h: str) -> str:
    h = (h or "").strip().lower()
    while h.endswith(".k2.dev.k2.dev"):
        h = h[: -len(".k2.dev")]
    if h.endswith(".k2.dev"):
        h = h[: -len(".k2.dev")]
    return h

def host_keys(h: str):
    h = (h or "").strip()
    if not h:
        return []
    b = bare(h)
    keys = {h, h.lower(), b, f"{b}.k2.dev", f"{b}.K2.DEV"}
    return [k for k in keys if k]

want = sys.argv[1] if len(sys.argv) > 1 else ""
home = os.environ.get("K2_HOME") or os.path.join(os.path.expanduser("~"), ".k2")
# also try sibling .k2so if K2_HOME is .k2 (legacy layout)
homes = [home]
alt = os.path.join(os.path.expanduser("~"), ".k2so")
if alt not in homes and os.path.isdir(alt):
    homes.append(alt)

want_keys = set(host_keys(want))
want_bare = bare(want)

# 1) connect-tokens.json flexible match
for base in homes:
    path = os.path.join(base, "connect-tokens.json")
    if not os.path.isfile(path):
        continue
    try:
        data = json.load(open(path))
    except Exception:
        continue
    if not isinstance(data, dict):
        continue
    for k, v in data.items():
        if not v or not isinstance(v, str):
            continue
        if k in want_keys or bare(k) == want_bare:
            print("TOKEN\t%s" % v)
            sys.exit(0)

# 2) connect-hosts.json → host id for keychain lookup
for base in homes:
    path = os.path.join(base, "connect-hosts.json")
    if not os.path.isfile(path):
        continue
    try:
        data = json.load(open(path))
    except Exception:
        continue
    if not isinstance(data, list):
        continue
    for entry in data:
        if not isinstance(entry, dict):
            continue
        hn = entry.get("hostname") or ""
        label = entry.get("label") or ""
        hid = entry.get("id") or ""
        if not hid:
            continue
        if hn in want_keys or bare(hn) == want_bare or bare(label) == want_bare:
            print("HOSTID\t%s" % hid)
            sys.exit(0)
PY
)
    if [ -n "$resolved" ]; then
        local kind val
        IFS=$'\t' read -r kind val <<<"$resolved"
        if [ "$kind" = "TOKEN" ] && [ -n "$val" ]; then
            printf '%s' "$val"
            return 0
        fi
        if [ "$kind" = "HOSTID" ] && [ -n "$val" ]; then
            host_id="$val"
        fi
    fi

    # 3) OS keychain — same service/account desktop rememberToken uses.
    if [ -n "${host_id:-}" ]; then
        if command -v security >/dev/null 2>&1; then
            local svc
            for svc in "dev.k2.connect.host-token" "com.k2so.connect.host-token"; do
                tok=$(security find-generic-password -s "$svc" -a "$host_id" -w 2>/dev/null || true)
                if [ -n "$tok" ]; then
                    printf '%s' "$tok"
                    return 0
                fi
            done
        fi
        if command -v secret-tool >/dev/null 2>&1; then
            local svc
            for svc in "dev.k2.connect.host-token" "com.k2so.connect.host-token"; do
                tok=$(secret-tool lookup service "$svc" account "$host_id" 2>/dev/null || true)
                if [ -n "$tok" ]; then
                    printf '%s' "$tok"
                    return 0
                fi
            done
        fi
    fi
}

# Diagnose why tray file send could not resolve a Connect token for <host>.
# Prints multi-line stderr teaching for GH #60 (misleading "add/sign-in" text).
_tray_token_missing_error() {
    local host="$1"
    local registered="no" has_tokens_file="no" has_host_id="no"
    if command -v python3 >/dev/null 2>&1; then
        local diag
        diag=$(K2_HOME="$K2_HOME" python3 - "$host" <<'PY' 2>/dev/null
import json, os, sys

def bare(h):
    h = (h or "").strip().lower()
    while h.endswith(".k2.dev.k2.dev"):
        h = h[: -len(".k2.dev")]
    if h.endswith(".k2.dev"):
        h = h[: -len(".k2.dev")]
    return h

want = sys.argv[1] if len(sys.argv) > 1 else ""
want_bare = bare(want)
home = os.environ.get("K2_HOME") or os.path.join(os.path.expanduser("~"), ".k2")
homes = [home]
alt = os.path.join(os.path.expanduser("~"), ".k2so")
if alt not in homes and os.path.isdir(alt):
    homes.append(alt)

registered = False
host_id = ""
tokens_file = False
for base in homes:
    p = os.path.join(base, "connect-tokens.json")
    if os.path.isfile(p):
        tokens_file = True
    p = os.path.join(base, "connect-hosts.json")
    if not os.path.isfile(p):
        continue
    try:
        data = json.load(open(p))
    except Exception:
        continue
    if not isinstance(data, list):
        continue
    for entry in data:
        if not isinstance(entry, dict):
            continue
        hn = entry.get("hostname") or ""
        label = entry.get("label") or ""
        if bare(hn) == want_bare or bare(label) == want_bare or hn.lower() == want.lower():
            registered = True
            host_id = entry.get("id") or ""
            break
print("registered=%s" % ("yes" if registered else "no"))
print("tokens_file=%s" % ("yes" if tokens_file else "no"))
print("host_id=%s" % ("yes" if host_id else "no"))
PY
)
        while IFS= read -r line; do
            case "$line" in
                registered=*) registered="${line#registered=}" ;;
                tokens_file=*) has_tokens_file="${line#tokens_file=}" ;;
                host_id=*) has_host_id="${line#host_id=}" ;;
            esac
        done <<<"$diag"
    fi

    cat >&2 <<EOF
Error: cannot upload tray files to ${host} — no Connect session token for that host.
  host in address book:  ${registered}
  connect-tokens.json:   ${has_tokens_file}
  host id (for keychain): ${has_host_id}
  active account / federation trust is NOT enough — file packages must
  authenticate to the *destination* daemon (upload + inbox deliver).
  Live text \`k2 msg agent::host "…"\` uses federation; tray files do not.

Fix (pick one):
  1. In the desktop app: Servers → sign in to ${host} with Remember on
     (desktop mirrors the token for CLI), then retry.
  2. Or put an owner/session token in \${K2_HOME}/connect-tokens.json:
       { "${host}": "<token>" }   (chmod 600)
  3. Or pass --token <tok> / set \$K2_REMOTE_TOKEN for this command.
  Folders are not supported — send a file or a .zip.
EOF
}

# Point the HTTP helpers at a remote host for the rest of this command
# (overrides the globals BASE_URL + TOKEN). No-op for a local target.
# Non-zero + stderr hint when no token is available.
_apply_remote_host() {
    local host="$1" explicit_token="${2:-}"
    [ -z "$host" ] && return 0
    local tok
    tok="$(_remote_token_for_host "$host" "$explicit_token")"
    if [ -z "$tok" ]; then
        echo "Error: no owner token for remote host '$host'." >&2
        echo "Pass --token <tok>, set \$K2_REMOTE_TOKEN, or add it to ${K2_HOME}/connect-tokens.json ({\"$host\": \"<token>\"})." >&2
        return 1
    fi
    case "$host" in
        http://*|https://*) BASE_URL="${host%/}" ;;
        *)                  BASE_URL="https://${host}" ;;
    esac
    TOKEN="$tok"
    # Inbox/fs/resolve must not UDS-shortcut to this box after a remote
    # BASE_URL swap (federated tray Bug A).
    _K2_FORCE_TCP=1
    return 0
}

# 0.39.0f Phase 2.1b: format_agents / format_work removed. Their
# only callers (cmd_agents_list / cmd_agents_work / cmd_work_inbox)
# are also gone; the inbox surface uses format_inbox instead.

