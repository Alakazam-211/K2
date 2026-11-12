# ── k2 firewall — the box's public ports and the owner's options (0.46.6) ──
# prd-custom-domains-path-a-0466-v1 §4.3. The daemon computes the view
# from its own state (never a scan) and renders the text; this is thin.
# Reads: owner, Admin, any agent. Changes: owner / Admin / a shell tab; an
# agent's change is owner_only (exit 3), except tightening its own App's
# guard.
_firewall_usage() {
    cat <<'EOF'
k2 firewall — the ports this server serves publicly, and your options

  k2 firewall [--json]                    the public list, who applies it, the App guard
  k2 firewall ports [--json]              the view (K2 Web's collector reads --json):
                                          public + knownNotPublic, hash, box, upstream
  k2 firewall rules [--format nft|ufw]    a complete default-deny rule set (SSH first) for
                                          the whole list, for a server nobody applies it
                                          for; review it and load it yourself (K2 doesn't)
  k2 firewall ssh any                     SSH from anywhere (the default)
  k2 firewall ssh allow <cidr>...         SSH only from these (never an empty list)
  k2 firewall ssh port <n>                record where sshd listens (K2 doesn't move sshd)
  k2 firewall open <port>/<tcp|udp> --label "<why>" [--from <cidr>]...
                                          add a port for something you run to the public
                                          list (it opens where the list is applied:
                                          k2 firewall says who; nobody = k2 firewall rules)
  k2 firewall close <port>/<tcp|udp>      your ports only (never SSH, never K2's)
  k2 firewall guard [on|off] [--conns-per-ip N] [--writes-per-min N] [--reset]
                                          the App guard's box defaults (on, 64, 120)
  k2 firewall guard <app> [on|off] [--writes-per-min N] [--reset]
                                          one App's write limit (its own agent may tighten it)
  k2 firewall upstream --hash <sha256:…> --provider <name> [--applied-at <unix>]
                       [--managed-by k2|owner|auto]
                                          record that the list was applied at the provider
                                          (owner; K2 Web runs it after each apply; the hash
                                          is k2 firewall ports --json's)
                       --managed-by       who applies the list:
                                          k2     K2 applies it at your provider's firewall
                                          owner  nobody does: apply k2 firewall rules yourself
                                          auto   k2 once an apply was recorded, else owner
                                                 (the default; a new box is owner)

K2 never changes this box's own firewall (nft, ufw). The list takes
effect where it is applied: at your provider when K2 applies it
(k2 firewall says who, and shows "last applied ... (this list)" once
your change is in), else only once you apply k2 firewall rules. A
firewall already on this box (ufw, nft) needs the port opened there too.
SSH is always public (narrow it, never close it). A port K2 opened for a
feature (the :443 front, hosted mail, Chat / Voice Hosting) is read-only here: turn
the feature off to close it. Changes are the owner's, an Admin login's
or a K2 shell tab's; an agent gets owner_only (exit 3) and can't request
it. Also: Settings → Network → Custom Domains → Firewall.
EOF
}

_firewall_render() {
    # _firewall_render <as_json> <mode: status|json|text> <raw "body\ncode">
    local as_json="$1" mode="$2" raw="$3" code body
    code="${raw##*$'\n'}"
    body="${raw%$'\n'*}"
    if [ -z "$raw" ] || [ "$code" = "$raw" ] || [ "$code" = "000" ]; then
        echo "k2 firewall: daemon unreachable" >&2
        exit 1
    fi
    if [ "$code" != "200" ]; then
        K2_FW_BODY="$body" K2_FW_CODE="$code" python3 -c '
import json, os, sys
raw, code = os.environ["K2_FW_BODY"], os.environ["K2_FW_CODE"]
try:
    d = json.loads(raw)
except Exception:
    print("k2 firewall: error: HTTP " + code + ": " + raw.strip()[:300], file=sys.stderr); sys.exit(0)
e = d.get("error")
if isinstance(e, dict):
    print("k2 firewall: error: %s: %s" % (e.get("code"), e.get("hint") or ""), file=sys.stderr)
else:
    print("k2 firewall: error: %s%s" % (e, (": " + d["message"]) if d.get("message") else ""), file=sys.stderr)
p = d.get("permission")
if isinstance(p, dict):
    if p.get("command"):
        print("Ask with: " + p["command"], file=sys.stderr)
    elif p.get("requestable") is False:
        print("It can\x27t be requested.", file=sys.stderr)
'
        [ "$as_json" = "1" ] && printf '%s\n' "$body"
        exit "$(_sidecar_exit_for "$code")"
    fi
    if [ "$as_json" = "1" ] || [ "$mode" = "json" ]; then
        printf '%s\n' "$body"
        return 0
    fi
    if [ "$mode" = "text" ]; then
        printf '%s' "$body"
        return 0
    fi
    K2_FW_BODY="$body" python3 -c '
import json, os
d = json.loads(os.environ["K2_FW_BODY"])
if "public" in d and "version" in d:
    print("view %s (v%s)" % (d.get("hash"), d.get("version")))
    for r in d.get("public") or []:
        print("  %5s/%-3s  %-11s  from %-20s  %s" % (r["port"], r["proto"], r["source"], ", ".join(r.get("allowFrom") or []), r.get("why", "")))
    for r in d.get("knownNotPublic") or []:
        print("  %5s/%-3s  %-11s  keep closed: %s" % (r["port"], r["proto"], r["source"], r.get("why", "")))
    up = d.get("upstream") or {}
    print("applied by: %s" % ("K2 at your provider" if up.get("managedBy") == "k2" else "nobody (k2 firewall rules)"))
elif "guard" in d and "counters" in d:
    g = d["guard"]
    print("App guard: %s, %s connections per visitor, %s writes per visitor per App a minute" % ("on" if g.get("enabled") else "off", g.get("connsPerIp"), g.get("writesPerMin")))
    for app, o in sorted((g.get("apps") or {}).items()):
        print("  %s: %s%s" % (app, {True: "on", False: "off"}.get(o.get("enabled"), "box default"), (", %s writes a minute" % o["writesPerMin"]) if o.get("writesPerMin") else ""))
elif "upstream" in d:
    up = d["upstream"]
    a = up.get("applied") or {}
    print("managed by: %s" % up.get("managedBy"))
    if a:
        print("applied   : %s at %s (%s)" % (a.get("hash"), a.get("provider"), "this list" if a.get("current") else "an older list"))
else:
    for k in ("opened", "closed"):
        if d.get(k):
            print("%s %s" % (k, d[k]))
    if d.get("ssh"):
        s = d["ssh"]
        print("ssh: port %s from %s" % (s.get("port"), ", ".join(s.get("allowFrom") or []) or "anyone"))
    if d.get("note"):
        print("note: " + d["note"])
    if d.get("hash"):
        print("view hash now %s (K2 Web applies it on its next pass, where it manages this server)" % d["hash"])
'
}

cmd_firewall() {
    local sub="${1:-status}"
    [ $# -gt 0 ] && shift
    case "$sub" in
        help|--help|-h) _firewall_usage; return 0 ;;
        --json) sub=status; set -- --json "$@" ;;
    esac
    local as_json=0 fmt="" label="" reset=0 conns="" writes="" hash="" provider="" applied="" managed="" raw body
    local args=() froms=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=1; shift ;;
            --format) fmt="${2:-}"; shift 2 || { echo "usage: --format nft|ufw" >&2; exit 2; } ;;
            --format=*) fmt="${1#--format=}"; shift ;;
            --label) label="${2:-}"; shift 2 || { echo "usage: --label \"<why>\"" >&2; exit 2; } ;;
            --from) froms+=("${2:-}"); shift 2 || { echo "usage: --from <cidr>" >&2; exit 2; } ;;
            --conns-per-ip) conns="${2:-}"; shift 2 || { echo "usage: --conns-per-ip N" >&2; exit 2; } ;;
            --writes-per-min) writes="${2:-}"; shift 2 || { echo "usage: --writes-per-min N" >&2; exit 2; } ;;
            --reset) reset=1; shift ;;
            --hash) hash="${2:-}"; shift 2 || { echo "usage: --hash sha256:…" >&2; exit 2; } ;;
            --provider) provider="${2:-}"; shift 2 || { echo "usage: --provider <name>" >&2; exit 2; } ;;
            --applied-at) applied="${2:-}"; shift 2 || { echo "usage: --applied-at <unix>" >&2; exit 2; } ;;
            --managed-by) managed="${2:-}"; shift 2 || { echo "usage: --managed-by k2|owner|auto" >&2; exit 2; } ;;
            -h|--help) _firewall_usage; return 0 ;;
            -*) echo "usage: unknown flag '$1' (k2 firewall --help)" >&2; exit 2 ;;
            *) args+=("$1"); shift ;;
        esac
    done
    _fw_body() { python3 -c 'import json,sys; print(json.dumps(json.loads(sys.argv[1])))' "$1"; }
    case "$sub" in
        status)
            if [ "$as_json" = 1 ]; then raw="$(_sidecar_send GET /cli/firewall "")"; _firewall_render 1 json "$raw"
            else raw="$(_sidecar_send GET /cli/firewall "format=text")"; _firewall_render 0 text "$raw"; fi
            ;;
        ports)
            raw="$(_sidecar_send GET /cli/firewall/ports "")"
            _firewall_render "$as_json" status "$raw"
            ;;
        rules)
            case "${fmt:-nft}" in nft|nftables|ufw) ;; *) echo "usage: k2 firewall rules --format nft|ufw" >&2; exit 2 ;; esac
            raw="$(_sidecar_send GET /cli/firewall/rules "format=${fmt:-nft}")"
            _firewall_render 0 text "$raw"
            ;;
        ssh)
            case "${args[0]:-}" in
                any) body='{"any":true}' ;;
                allow)
                    [ "${#args[@]}" -ge 2 ] || { echo "usage: k2 firewall ssh allow <cidr>... (SSH is never closed; for anyone: k2 firewall ssh any)" >&2; exit 2; }
                    body="$(python3 -c 'import json,sys; print(json.dumps({"allow": sys.argv[1:]}))' "${args[@]:1}")" ;;
                port)
                    [[ "${args[1]:-}" =~ ^[0-9]+$ ]] || { echo "usage: k2 firewall ssh port <n>" >&2; exit 2; }
                    body="{\"port\":${args[1]}}" ;;
                *) echo "usage: k2 firewall ssh any | allow <cidr>... | port <n>" >&2; exit 2 ;;
            esac
            raw="$(_sidecar_send POST /cli/firewall/ssh "$body")"
            _firewall_render "$as_json" status "$raw"
            ;;
        open|close)
            [ "${#args[@]}" -eq 1 ] || { echo "usage: k2 firewall $sub <port>/<tcp|udp>" >&2; exit 2; }
            if [ "$sub" = open ]; then
                [ -n "$label" ] || { echo "usage: k2 firewall open <port>/<tcp|udp> --label \"<why>\" [--from <cidr>]..." >&2; exit 2; }
                body="$(python3 -c 'import json,sys; print(json.dumps({"port": sys.argv[1], "label": sys.argv[2], "from": sys.argv[3:]}))' "${args[0]}" "$label" ${froms[@]+"${froms[@]}"})"
            else
                body="$(python3 -c 'import json,sys; print(json.dumps({"port": sys.argv[1]}))' "${args[0]}")"
            fi
            raw="$(_sidecar_send POST "/cli/firewall/$sub" "$body")"
            _firewall_render "$as_json" status "$raw"
            ;;
        guard)
            body="$(python3 -c '
import json, sys
args, conns, writes, reset = sys.argv[1].split("\x1f") if sys.argv[1] else [], sys.argv[2], sys.argv[3], sys.argv[4] == "1"
b = {}
for a in args:
    if a in ("on", "off"):
        b["on"] = a == "on"
    elif "app" not in b:
        b["app"] = a
    else:
        print("usage: k2 firewall guard [<app>] [on|off] [--conns-per-ip N] [--writes-per-min N] [--reset]", file=sys.stderr); sys.exit(2)
for k, v in (("connsPerIp", conns), ("writesPerMin", writes)):
    if v:
        if not v.isdigit():
            print("usage: %s takes a number" % k, file=sys.stderr); sys.exit(2)
        b[k] = int(v)
if reset:
    b["reset"] = True
print(json.dumps(b))' "$(IFS=$'\x1f'; printf '%s' "${args[*]-}")" "$conns" "$writes" "$reset")" || exit 2
            raw="$(_sidecar_send POST /cli/firewall/guard "$body")"
            _firewall_render "$as_json" status "$raw"
            ;;
        upstream)
            body="$(python3 -c '
import json, sys
h, p, a, m = sys.argv[1:5]
b = {}
if h:
    b.update({"hash": h, "provider": p})
    if a:
        if not a.isdigit():
            print("usage: --applied-at <unix seconds>", file=sys.stderr); sys.exit(2)
        b["appliedAt"] = int(a)
if m:
    b["managedBy"] = m
if not b:
    print("usage: k2 firewall upstream --hash <sha256:…> --provider <name> [--applied-at <unix>] | --managed-by k2|owner|auto", file=sys.stderr); sys.exit(2)
print(json.dumps(b))' "$hash" "$provider" "$applied" "$managed")" || exit 2
            raw="$(_sidecar_send POST /cli/firewall/upstream "$body")"
            _firewall_render "$as_json" status "$raw"
            ;;
        *)
            echo "usage: unknown firewall verb '$sub' (status|ports|rules|ssh|open|close|guard|upstream)" >&2
            exit 2
            ;;
    esac
}

