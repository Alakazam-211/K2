# ── k2 front — the box's :443 as an SNI pass-through router (front443) ──
# (0.46.1, plan-turn-443-front-v1). The daemon holds :443 through a socket
# the root helper installs (protocol 8, `front install`); TURN (Sew voice),
# Skin Direct's Caddy and, opt-in, Stalwart sit behind it, each reached
# without the front ending TLS. Owner/admin verbs: run them outside K2.
_front_usage() {
    cat <<'EOF'
k2 front — K2's front on this box's port 443 (Linux)

The front holds the box's public :443 (IPv4 and IPv6). For each visitor
it reads the name they asked for (the TLS server name) and passes the
connection, still encrypted, to the server for that name: custom App
names (k2 app domain), K2's own Caddy (the App front door in direct
mode), hosted mail's web pages, Chat / Voice Hosting calls (their TURN relay), and
names you pass through to your own web server. A name nothing serves is
closed.

  k2 front status [--json]          who holds :443, the routes, counters
  k2 front enable [--mail] [--json] take :443 (the daemon restarts once: a few seconds)
                                    voice calls move onto :443 (calls in progress drop once);
                                    K2's Caddy moves to 127.0.0.1:38473 behind it
                    --mail          also put hosted mail's web pages behind the front:
                                    they move to 127.0.0.1:8443 (PROXY v2); mail's own
                                    certificate renewal (TLS-ALPN-01) keeps working;
                                    the mail server restarts once
  k2 front disable [--json]         give :443 back (voice to TCP 5349, mail to :443)
  k2 front http on|off [--json]     the https redirect for people who type http://: on
                                    K2's relays for names through K2 Connect, and on this
                                    box's :80 where K2's front runs (K2's own HTTP-01
                                    certificates answer there too). On by default:
                                    k2 front enable installs :80 when it is free. On takes
                                    :80 (the daemon restarts once); off frees it.
  k2 front passthrough [--json]     names passed through to your own web server
  k2 front passthrough add <host>|'*' --to 127.0.0.1:<port> [--proxy-v2] [--label <why>]
                                    pass <host> (or '*': every name K2 doesn't serve),
                                    still encrypted, to a web server you run on this
                                    box; K2's own names always win. Works before
                                    enable: saved now, routed once the front is on.
                    --proxy-v2      only if that server reads the PROXY protocol
                                    (nginx: listen ... proxy_protocol): it then sees
                                    visitors' real IPs (without it, every visitor is
                                    127.0.0.1). A server that doesn't read it breaks.
                                    Give every name on one port the same choice.
  k2 front passthrough rm <host>|'*'

Who (every verb above): the owner (a terminal outside K2, e.g. over
SSH), an Admin login, or a person typing in a K2 shell tab. An agent may
only list the passthrough names (k2 front passthrough); anything else is
refused (exit 3, owner_only; the error names the command, e.g. "The
owner runs: k2 front enable [--mail]"): ask the owner to run it.

KEEP A WEB SERVER THAT ISN'T K2'S (a system nginx or Caddy on :443)
status names it, and enable refuses while it holds :443: K2 never takes
the port from it. To keep its sites:
  0. k2 front status: a "helper" line means the root helper is too old
     for the front; run k2 root-helper update before going on
  1. k2 front passthrough add <its host> --to 127.0.0.1:9443
     once per site; all its sites can share the one port (it picks the
     site by name). Or '*' once: every name K2 doesn't serve.
  2. in each site, replace its :443 listen lines (IPv4 and IPv6) with
     one TLS listen on 127.0.0.1:9443 (nginx: listen 127.0.0.1:9443 ssl;)
     and reload it (its certificates stay; leave its :80 as it is)
  3. k2 front enable (a refusal names its cause: port_443_taken names
     what still holds :443; also helper_needed, not_systemd, user_scope,
     unsupported)
Its sites are down from step 2 until the front holds :443 (a few
seconds). If enable refuses, put the server back on :443. Its own
certificate renewal keeps working: TLS-ALPN-01 passes through the front
untouched, and HTTP-01 still reaches its :80, as long as :80 stays
public: k2 firewall open 80/tcp --label "nginx" unless k2 firewall
already lists it.

Linux boxes under systemd with k2-root-helper protocol 8; the :80 front needs
protocol 12 (k2 root-helper update). An App on your own domain that doesn't
need this box's :443 at all: Path B (k2 app domain --help).
EOF
}

_front_render() {
    # _front_render <as_json> <raw "body\ncode">
    local as_json="$1" raw="$2" code body
    code="${raw##*$'\n'}"
    body="${raw%$'\n'*}"
    if [ -z "$raw" ] || [ "$code" = "$raw" ] || [ "$code" = "000" ]; then
        echo "k2 front: daemon unreachable" >&2
        exit 1
    fi
    if [ "$as_json" = "1" ]; then
        printf '%s\n' "$body"
        # 403 (not allowed: an agent's owner_only) is exit 3, as for firewall.
        [ "$code" = "200" ] || { [ "$code" = "403" ] && exit 3; exit 1; }
        return 0
    fi
    K2_FR_BODY="$body" K2_FR_CODE="$code" python3 -c '
import json, os, sys
raw, code = os.environ["K2_FR_BODY"], os.environ["K2_FR_CODE"]
try:
    d = json.loads(raw)
except Exception:
    print("k2 front: error: HTTP " + code + ": " + raw.strip()[:200], file=sys.stderr); sys.exit(3 if code == "403" else 1)
if code != "200" or d.get("ok") is False or (isinstance(d.get("error"), str) and "serving" not in d):
    e = d.get("error")
    msg = d.get("message") or (e.get("hint") if isinstance(e, dict) else None) or ""
    print("k2 front: error: " + str(e if not isinstance(e, dict) else e.get("code")) + (": " + msg if msg else ""), file=sys.stderr)
    p = d.get("permission")
    if isinstance(p, dict):
        if p.get("command"):
            print("Ask with: " + p["command"], file=sys.stderr)
        elif p.get("requestable") is False:
            print("It can\x27t be requested.", file=sys.stderr)
    sys.exit(3 if code == "403" else 1)
if "serving" in d:
    print("front      :", "on" if d.get("enabled") else "off", "· serving" if d.get("serving") else "· not serving",
          "(socket from " + str((d.get("socket") or {}).get("source")) + ")" if d.get("serving") else "")
    if d.get("restartNeeded"):
        print("note       : enabled but not serving: the daemon takes the socket at its next start")
    print("mail       :", "behind the front" if d.get("mail") else "not behind the front")
    ht = d.get("http") or {}
    if ht:
        print(":80        :", ("on" if ht.get("on") else "off"), "· serving" if ht.get("serving") else "· not serving", "(" + ht["next"] + ")" if ht.get("next") else "")
        if ht.get("helper"):
            print("note       :", ht["helper"])
    print("cap        :", d.get("cap"), " (E2E listener", str(d.get("e2eCap")) + "; RLIMIT_NOFILE", str(d.get("nofileSoftLimit")) + ")")
    for r in d.get("routes") or []:
        print("route      :", r.get("host"), "->", r.get("kind"), r.get("backend"), "PROXY v2" if r.get("proxyV2") else "no PROXY")
    h = d.get("port443") or {}
    if h.get("holder") and h.get("holder") != "free":
        print(":443       :", h.get("holder"), "-", h.get("message"))
    pt = d.get("passthrough")
    if isinstance(pt, list) and pt and not d.get("enabled"):
        print("passthrough:", ", ".join(x.get("host") + " -> " + x.get("to") for x in pt), "(routes once the front is on)")
    g = d.get("guard") or {}
    if g:
        print("App guard  :", "refused connections", g.get("perIpRefused"), "· limited writes", g.get("guardLimited"))
    c = d.get("connections")
    if isinstance(c, dict):
        cl = c.get("closed") or {}
        print("conns      : accepted", c.get("accepted"), "· live", c.get("live"), "· over cap", c.get("overCap"))
        print("closed     :", ", ".join("%s %s" % (k, v) for k, v in cl.items() if v))
    if d.get("helper"):
        print("helper     :", d.get("helper"))
    su = d.get("socketUnit") or {}
    if su:
        st = su.get("state") or {}
        print("socket unit:", st.get("ActiveState"), st.get("SubState") or "", "(" + str(st.get("Result")) + ")")
        for line in su.get("journal") or []:
            print("  systemd  :", line)
    sys.exit(0)
if "on" in d:
    print(":80 front  :", "on" if d.get("on") else "off")
    if d.get("next"):
        print("next       :", d.get("next"))
    sys.exit(0)
print("front      :", "on" if d.get("enabled") else "off")
if d.get("enabled"):
    print("mail       :", "behind the front" if d.get("mail") else "not behind the front")
    if d.get("restarting"):
        print("restarting : K2 will restart its daemon once to take port 443")
    elif d.get("restartNeeded"):
        print("restart    : needed to take port 443 (see warning)")
for w in d.get("warnings") or []:
    print("warning    :", w)
if d.get("next"):
    print("next       :", d.get("next"))
'
}

