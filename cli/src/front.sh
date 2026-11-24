# ── k2 front — the box's :443 as an SNI pass-through router (front443) ──
# (0.46.1, plan-turn-443-front-v1). The daemon holds :443 through a socket
# the root helper installs (protocol 8, `front install`); TURN (Sew voice),
# Skin Direct's Caddy and, opt-in, Stalwart sit behind it, each reached
# without the front ending TLS. Owner/admin verbs: run them outside K2.
_front_usage() {
    cat <<'EOF'
%include cli/text/help/front.txt
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
    p = d.get("permission"); p = dict(p, command="k2 <tail>".replace("<tail>", p["tail"])) if isinstance(p, dict) and isinstance(p.get("tail"), str) and p["tail"] else p
    if isinstance(p, dict):
        if p.get("command"):
            print("Ask with: " + p["command"], file=sys.stderr)
        elif p.get("requestable") is False:
            print("It can\x27t be requested.", "If you think this should be possible, tell the K2 team: k2 send-feedback \"<what you were trying to do>\"", sep="\n", file=sys.stderr)
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

