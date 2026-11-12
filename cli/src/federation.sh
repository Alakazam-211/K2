# ── Federation (cross-server agent comms) ────────────────────────────
# DARK by default: the daemon 404s every /cli/federation/* route unless it
# was started with K2_FEDERATION=1. SSOT: prd-cross-server-agent-comms.md.
#
#   k2 fed pubkey [--pem|--fingerprint|--json]   export THIS box's SPKI pubkey
#   k2 fed send <peer>::<workspace>::<agent> "message"
#   k2 fed outbox                                queued + dead-lettered per peer
#   k2 fed pair-start [--subdomain S] [--url URL]   pairing v2, step 1 (then
#   k2 fed pair-join / pair-finish / pair-verify <token>)
#   k2 fed pair-request --pubkey <spki.pem> ...   pre-0.45.2 pairing (known peers only)
#   k2 fed confirm <fingerprint> <sas-code>
#
# send + confirm are OWNER-gated (they need ~/.k2/daemon.token). pubkey is
# pure-CLI (reads the on-box tunnel key) so it works even when federation is
# dark — you need your key to bootstrap the very first pairing.
cmd_fed() {
    local owner_token=""
    local _tok_file="$K2_HOME/daemon.token"
    [ -f "$_tok_file" ] && owner_token=$(cat "$_tok_file" 2>/dev/null | tr -d '[:space:]')

    local sub="${1:-}"; shift || true
    case "$sub" in
        send)
            local to="${1:-}"; local text="${2:-}"
            if [ -z "$to" ] || [ -z "$text" ]; then
                echo "usage: k2 fed send <peer>::<workspace>::<agent> \"message\"" >&2
                exit 2
            fi
            if [ -z "$owner_token" ]; then
                echo "k2 fed send: no owner token (need $_tok_file)" >&2
                exit 1
            fi
            # GAP #3: carry the SOURCE workspace ($PROJECT, resolved at the
            # top of this script from K2_PROJECT_PATH/CWD — same source
            # cmd_connections uses). The daemon GATES the cross-daemon send
            # on `is_remote_connection(from_workspace, <agent>::<host>)`:
            # an agent may only message a peer its workspace is connected
            # to. (Owner-remote `k2 talk` is a different, ungated path.)
            local body
            body=$(python3 -c 'import json,sys; print(json.dumps({"to":sys.argv[1],"text":sys.argv[2],"from_workspace":sys.argv[3]}))' "$to" "$text" "$PROJECT")
            local _K2_CFG=""
            _k2_cfg_add url "${BASE_URL}/cli/federation/send?token=${owner_token}"
            _k2_cfg_add data-raw "$body"
            _k2_curl "$_K2_CFG" -s -X POST \
                --connect-timeout 5 --max-time 60 \
                -H "Content-Type: application/json" 2>/dev/null
            echo
            ;;
        pair-start|pair-join|pair-finish|pair-verify)
            # Pairing v2 (0.45.2): commit-then-reveal code, run by the two
            # owners on their own servers. Each step prints a one-line token
            # to send to the other owner:
            #   A: k2 fed pair-start [--subdomain S] [--url U] [--label L]  -> token 1
            #   B: k2 fed pair-join <token 1>                               -> token 2
            #   A: k2 fed pair-finish <token 2>  -> token 3 + A's code
            #   B: k2 fed pair-verify <token 3>  -> B's code
            # Both compare the codes, then each runs: k2 fed confirm <fp> <code>
            if [ -z "$owner_token" ]; then
                echo "k2 fed $sub: no owner token (need $_tok_file)" >&2
                exit 1
            fi
            local route body token_in=""
            case "$sub" in
                pair-start)
                    local label="" subdomain="" url=""
                    while [ $# -gt 0 ]; do
                        case "$1" in
                            --label) label="${2:-}"; shift 2 ;;
                            --label=*) label="${1#--label=}"; shift ;;
                            --subdomain) subdomain="${2:-}"; shift 2 ;;
                            --subdomain=*) subdomain="${1#--subdomain=}"; shift ;;
                            --url|--base-url|--baseUrl) url="${2:-}"; shift 2 ;;
                            --url=*|--base-url=*|--baseUrl=*) url="${1#*=}"; shift ;;
                            *) echo "k2 fed pair-start: unknown arg: $1" >&2; exit 2 ;;
                        esac
                    done
                    route="pair/v2/init"
                    body=$(python3 -c 'import json,sys
print(json.dumps({"label":sys.argv[1],"subdomain":sys.argv[2],"baseUrl":sys.argv[3]}))' "$label" "$subdomain" "$url")
                    ;;
                *)
                    token_in="${1:-}"
                    if [ -z "$token_in" ]; then
                        echo "usage: k2 fed $sub <token from the other server's owner>" >&2
                        exit 2
                    fi
                    case "$sub" in
                        pair-join) route="pair/v2/respond" ;;
                        pair-finish) route="pair/v2/reveal" ;;
                        pair-verify) route="pair/v2/verify" ;;
                    esac
                    body=$(python3 -c 'import base64,json,sys
t = sys.argv[1].strip()
want = {"pair-join": ("k2pair1:", "offer"), "pair-finish": ("k2pair2:", "answer"), "pair-verify": ("k2pair3:", "reveal")}[sys.argv[2]]
if not t.startswith(want[0]):
    sys.stderr.write("k2 fed %s: expected a %s... token (got %s...)\n" % (sys.argv[2], want[0], t[:8])); sys.exit(2)
raw = t[len(want[0]):]
raw += "=" * (-len(raw) % 4)
try:
    obj = json.loads(base64.urlsafe_b64decode(raw.encode()).decode())
except Exception as e:
    sys.stderr.write("k2 fed %s: the token is damaged (%s)\n" % (sys.argv[2], e)); sys.exit(2)
print(json.dumps({want[1]: obj}))' "$token_in" "$sub") || exit 2
                    ;;
            esac
            local _K2_CFG="" resp
            _k2_cfg_add url "${BASE_URL}/cli/federation/${route}?token=${owner_token}"
            _k2_cfg_add data-raw "$body"
            resp=$(_k2_curl "$_K2_CFG" -s -X POST \
                --connect-timeout 5 --max-time 30 \
                -H "Content-Type: application/json" 2>/dev/null)
            printf '%s' "$resp" | python3 -c 'import base64,json,sys
step = sys.argv[1]
try:
    d = json.load(sys.stdin)
except Exception:
    sys.stderr.write("k2 fed %s: no answer from the daemon (is federation enabled? k2 fed enable)\n" % step); sys.exit(1)
if not isinstance(d, dict) or d.get("error"):
    err = d.get("error") if isinstance(d, dict) else d
    sys.stderr.write("k2 fed %s: %s\n" % (step, err)); sys.exit(1)
def tok(prefix, obj):
    return prefix + base64.urlsafe_b64encode(json.dumps(obj, separators=(",", ":")).encode()).decode().rstrip("=")
if step == "pair-start":
    print("Send this to the other server owner (valid 10 minutes, single use):")
    print(tok("k2pair1:", d["offer"]))
    print("They run: k2 fed pair-join <token>, and send you back a k2pair2: token.")
elif step == "pair-join":
    print("Send this back to the owner who started the pairing:")
    print(tok("k2pair2:", d["answer"]))
    print("They run: k2 fed pair-finish <token>, and send you a k2pair3: token.")
elif step == "pair-finish":
    print("Send this to the other owner:")
    print(tok("k2pair3:", d["reveal"]))
    print("They run: k2 fed pair-verify <token>.")
    print("Your pairing code: %s" % d["sas"])
    print("If both codes match, run: k2 fed confirm %s %s" % (d["peer_fingerprint"], d["sas"]))
else:
    print("Your pairing code: %s" % d["sas"])
    print("If both codes match, run: k2 fed confirm %s %s" % (d["peer_fingerprint"], d["sas"]))
' "$sub"
            ;;
        pair-request)
            local pubkey_file="" label="" subdomain="" url=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --pubkey) pubkey_file="${2:-}"; shift 2 ;;
                    --pubkey=*) pubkey_file="${1#--pubkey=}"; shift ;;
                    --label) label="${2:-}"; shift 2 ;;
                    --label=*) label="${1#--label=}"; shift ;;
                    --subdomain) subdomain="${2:-}"; shift 2 ;;
                    --subdomain=*) subdomain="${1#--subdomain=}"; shift ;;
                    --url|--base-url|--baseUrl) url="${2:-}"; shift 2 ;;
                    --url=*|--base-url=*|--baseUrl=*) url="${1#*=}"; shift ;;
                    *) echo "k2 fed pair-request: unknown arg: $1" >&2; exit 2 ;;
                esac
            done
            if [ -z "$pubkey_file" ] || [ ! -f "$pubkey_file" ]; then
                echo "usage: k2 fed pair-request --pubkey <spki.pem> [--label L] [--subdomain S] [--url http://host:port]" >&2
                exit 2
            fi
            local body
            body=$(python3 -c 'import json,sys
print(json.dumps({"label":sys.argv[1],"subdomain":sys.argv[2],"baseUrl":sys.argv[3],"public_key_pem":open(sys.argv[4]).read()}))' "$label" "$subdomain" "$url" "$pubkey_file")
            # UNAUTH route (creates only a Pending peer).
            _k2_curl "" -s -X POST "${BASE_URL}/cli/federation/pair/request" \
                --connect-timeout 5 --max-time 30 \
                -H "Content-Type: application/json" \
                --data-raw "$body" 2>/dev/null
            echo
            ;;
        confirm)
            local fp="${1:-}"; local sas="${2:-}"
            if [ -z "$fp" ] || [ -z "$sas" ]; then
                echo "usage: k2 fed confirm <fingerprint> <sas-code>" >&2
                exit 2
            fi
            if [ -z "$owner_token" ]; then
                echo "k2 fed confirm: no owner token (need $_tok_file)" >&2
                exit 1
            fi
            local body
            body=$(python3 -c 'import json,sys; print(json.dumps({"fingerprint":sys.argv[1],"sas":sys.argv[2]}))' "$fp" "$sas")
            local _K2_CFG=""
            _k2_cfg_add url "${BASE_URL}/cli/federation/pair/confirm?token=${owner_token}"
            _k2_cfg_add data-raw "$body"
            _k2_curl "$_K2_CFG" -s -X POST \
                --connect-timeout 5 --max-time 30 \
                -H "Content-Type: application/json" 2>/dev/null
            echo
            ;;
        pubkey)
            # Export THIS box's federation identity — the SPKI public key PEM a
            # peer pins via `k2 fed pair-request --pubkey`. Pure-CLI: derive it
            # from the on-box ECDSA P-256 identity key with openssl, so it works
            # even when K2_FEDERATION is off (you need the key to START pairing).
            # fingerprint = sha256(SPKI DER) hex — matches the daemon's
            # federation::peers::fingerprint_of_spki_der.
            # 0.45.2: the identity is ~/.k2/federation/identity.pem. Before the
            # daemon's first 0.45.2 boot it does not exist yet; the daemon will
            # copy tunnel-key.pem into it, so that key IS the identity until then.
            local key_file="$K2_HOME/federation/identity.pem"
            [ -f "$key_file" ] || key_file="$K2_HOME/tunnel-key.pem"
            local want="${1:-}"
            if [ ! -f "$key_file" ]; then
                echo "k2 fed pubkey: no federation key at $key_file (start the daemon once to generate it)" >&2
                exit 1
            fi
            if ! command -v openssl >/dev/null 2>&1; then
                echo "k2 fed pubkey: openssl not found on PATH" >&2
                exit 1
            fi
            local pem fp
            pem=$(openssl pkey -in "$key_file" -pubout 2>/dev/null) || {
                echo "k2 fed pubkey: failed to derive SPKI public key from $key_file" >&2; exit 1; }
            fp=$(openssl pkey -in "$key_file" -pubout -outform DER 2>/dev/null | shasum -a 256 | awk '{print $1}')
            case "$want" in
                --fingerprint|--fp) printf '%s\n' "$fp" ;;
                --json) python3 -c 'import json,sys; print(json.dumps({"public_key_pem":sys.argv[1],"fingerprint":sys.argv[2]}))' "$pem" "$fp" ;;
                ""|--pem) printf '%s\n' "$pem"; printf '# fingerprint: %s\n' "$fp" >&2 ;;
                *) echo "usage: k2 fed pubkey [--pem|--fingerprint|--json]" >&2; exit 2 ;;
            esac
            ;;
        enable|disable)
            # Flip the persisted federation master switch on THIS daemon — the
            # headless equivalent of Settings → K2 Connect → Enable federation.
            # Owner-gated via /cli/settings/update; federation::set_enabled() is
            # synced so the /cli/federation/* gate goes live/dark immediately
            # (no restart). Built for Linux headless setup.
            local fval="true"; [ "$sub" = "disable" ] && fval="false"
            cli_post_json "/cli/settings/update" "{\"federationEnabled\": $fval}" \
                | python3 -c "import json,sys
try:
    d = json.load(sys.stdin)
    print('federation: ' + ('ENABLED' if d.get('federationEnabled') else 'disabled') + ' (this server)')
except Exception:
    sys.stderr.write('k2 fed: unexpected response from /cli/settings/update\n'); sys.exit(1)"
            ;;
        status)
            cli_request "/cli/settings/get" \
                | python3 -c "import json,sys
try:
    d = json.load(sys.stdin)
    print('federation: ' + ('ENABLED' if d.get('federationEnabled') else 'disabled') + ' (this server)')
except Exception:
    sys.stderr.write('k2 fed status: unexpected response from /cli/settings/get\n'); sys.exit(1)"
            ;;
        peers)
            # GH #48 — list pinned federation peers so asymmetric trust is
            # visible (and k2 msg can name hosts that match). Dual-auth:
            # owner token or agent passport via cli_send.
            cli_send cli_request "/cli/federation/peers" | python3 -c "
import json, sys
try:
    d = json.load(sys.stdin)
except Exception:
    sys.stderr.write('k2 fed peers: could not parse response\n'); sys.exit(1)
if isinstance(d, dict) and d.get('error'):
    err = d['error']
    if isinstance(err, dict):
        sys.stderr.write('k2 fed peers: %s\n' % (err.get('hint') or err.get('code') or err))
    else:
        sys.stderr.write('k2 fed peers: %s\n' % err)
    sys.exit(1)
peers = d.get('peers', []) if isinstance(d, dict) else []
# 0.46.1: A to Z by the name shown, then host. A 0.46.1+ daemon already
# sends this order (the picker and Settings use it too); this keeps an
# older daemon's pairing order from leaking through.
def _shown(p):
    return ((p.get('label') or '').strip() or (p.get('subdomain') or '').strip()
            or (p.get('fingerprint') or '')[:12])
def _host(p):
    b = (p.get('base_url') or p.get('baseUrl') or '').strip()
    for pre in ('http://', 'https://'):
        if b.lower().startswith(pre):
            b = b[len(pre):]
    return b.split('/')[0] or (p.get('subdomain') or '').strip()
peers = sorted(peers, key=lambda p: (_shown(p).casefold(), _host(p).casefold(), p.get('fingerprint') or ''))
if not peers:
    print('No federation peers pinned on this server.')
    print('Pair with: k2 fed pair-start / k2 fed confirm (or Settings → Connections auto-pair).')
    sys.exit(0)
print('Peers (%d):' % len(peers))
for p in peers:
    sub = (p.get('subdomain') or '').strip()
    base = (p.get('base_url') or p.get('baseUrl') or '').strip()
    if base:
        host = base
    elif sub.startswith('http://') or sub.startswith('https://') or ':' in sub or sub.endswith('.ts.net'):
        host = sub
    elif sub and not sub.endswith('.k2.dev'):
        host = sub + '.k2.dev'
    else:
        host = sub or '—'
    label = p.get('label') or ''
    trust = p.get('trust') or '?'
    fp = (p.get('fingerprint') or '')[:16]
    extra = ('  label=%s' % label) if label else ''
    print('  %s  trust=%s  fp=%s…%s' % (host, trust, fp, extra))
    if p.get('repair_recommended'):
        print('      older pairing code: messages still work; re-pair recommended (Settings → Connections → Re-pair, or k2 fed pair-start)')
    # 0.45.2: this server's signed address notice to that peer.
    n = p.get('address_notice') or {}
    st = n.get('status')
    tgt = n.get('target') or {}
    new_addr = (tgt.get('subdomain') or '') or (tgt.get('base_url') or '')
    if st == 'pending':
        err = n.get('last_error')
        print('      address notice: pending -> %s (attempts %s%s)' % (new_addr, n.get('attempts', 0), ('; last error: %s' % err) if err else ''))
    elif st == 'failed':
        print('      address notice: FAILED -> %s (%s): %s' % (new_addr, n.get('failed_reason') or '?', n.get('last_error') or ''))
"
            ;;
        outbox)
            # The truthful surface for the outbox drain: per peer, how many
            # cross-server messages are still QUEUED (waiting for the peer to
            # become reachable — the drain retries with backoff) and what got
            # DEAD-LETTERED (terminal failures kept for inspection).
            if [ -z "$owner_token" ]; then
                echo "k2 fed outbox: no owner token (need $_tok_file)" >&2
                exit 1
            fi
            local _K2_CFG=""
            _k2_cfg_add url "${BASE_URL}/cli/federation/outbox?token=${owner_token}"
            _k2_curl "$_K2_CFG" -s \
                --connect-timeout 5 --max-time 30 2>/dev/null \
                | python3 -c "import json,sys
try:
    d = json.load(sys.stdin)
except Exception:
    sys.stderr.write('k2 fed outbox: unexpected response (is federation on?)\n'); sys.exit(1)
if 'error' in d:
    sys.stderr.write('k2 fed outbox: ' + str(d['error']) + '\n'); sys.exit(1)
peers = d.get('peers', [])
if not peers:
    print('outbox: empty (no queued or dead-lettered cross-server messages)')
    sys.exit(0)
for p in peers:
    who = p.get('label') or p.get('subdomain') or p.get('fingerprint','')[:12]
    line = f\"{who}: {p.get('queued',0)} queued\"
    age = p.get('oldest_age_secs')
    if age is not None:
        line += f' (oldest {age}s)'
    line += f\", {p.get('dead',0)} dead-lettered\"
    print(line)
    for dl in p.get('dead_letters', [])[:5]:
        print(f\"  dead {dl.get('msg_uuid','')[:8]}… — {dl.get('reason','')}\")
print(f\"total queued: {d.get('total_queued',0)}\")"
            ;;
        *)
            echo "usage: k2 fed <enable|disable|status|peers|pubkey|send|outbox|pair-start|pair-join|pair-finish|pair-verify|confirm> ..." >&2
            echo "  enable/disable/status: flip the persisted federation switch (headless-friendly)" >&2
            echo "  peers: list pinned servers (use their host with k2 msg agent::host)" >&2
            echo "  pair-start [--subdomain S] [--url http://host:port]: start a pairing (prints a token for the other owner)" >&2
            echo "  pair-join / pair-finish / pair-verify <token>: the next steps; both owners then compare codes and confirm" >&2
            echo "  pair-request: the pre-0.45.2 pairing; refused for a server not paired yet" >&2
            echo "  send/outbox/pair-*/confirm need federation ON; pubkey works anytime" >&2
            echo "  outbox: queued + dead-lettered cross-server messages per peer" >&2
            exit 2
            ;;
    esac
}

