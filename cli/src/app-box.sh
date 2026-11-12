# ── App box services (0.46.0) ────────────────────────────────────────
# A walled App reaches a service on this box's loopback only through an
# owner-approved forward into its wall. New ones are held for the owner;
# revoking is immediate.

_app_box_service_usage() {
    cat <<'EOF'
k2 app box-service — let a walled App reach a service on this box's loopback

  k2 app box-service <app-id> [--json]
                                       Its box services (approved / held), where
                                       each is inside the wall (K2_BOX_<NAME>),
                                       whether the running App has the forward,
                                       and the known names (presets).
  k2 app box-service request <app-id> <name> [<host-port>] [--app-port N]
                 [--reason TEXT] [--json]
                                       Ask for one. Always held for the owner:
                                       exit 2 with the request id until they
                                       approve. sew-api and sew-signal need no
                                       port (read from Sew's config). The App
                                       dials 127.0.0.1:<app-port> in its wall
                                       (the host port by default).
  k2 app box-service revoke <app-id> <name> [--json]
                                       Shut it at once (new connections are
                                       refused). No approval needed.

  k2 app approvals | approve <id> | deny <id>
                                       The owner approves in person (Touch ID or
                                       a password, outside K2). Agents never can.

In a manifest: "boxServices": [{"name": "sew-api"}, {"name": "sew-signal"},
{"name": "cache", "hostPort": 6380, "appPort": 6380}] — asked for at deploy.
Approving opens the door without a restart when the App was deployed with
it; one asked for later needs one k2 app restart for its forward.

TCP only (HTTP, websockets, long uploads ride it). Never forwarded: ports
below 1024 (ssh, mail, DNS), 38600-38999 (every App's gateway and proxy),
5432-5449 (Postgres, every major: use the App's database door), 2019 (Caddy admin), 38472
(the box router), this daemon's own ports. The process exception dials the
host port directly (same approval, same nft door).

Exit codes: 0 ok · 2 usage or held (pending the owner) · 3 refused/infra.
EOF
}

cmd_app_box_service() {
    local sub="${1:-}"
    case "$sub" in
        request|revoke|rm) shift ;;
        ""|-h|--help|help) _app_box_service_usage; return 0 ;;
        *) sub="show" ;;
    esac
    local app="" json_out=0 reason="" app_port="" rest=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            --reason) reason="${2:-}"; shift 2 ;;
            --app-port) app_port="${2:-}"; shift 2 ;;
            -h|--help) _app_box_service_usage; return 0 ;;
            -*) echo "Unknown flag: $1 (k2 app box-service --help)" >&2; exit 2 ;;
            *) if [ -z "$app" ]; then app="$1"; else rest+=("$1"); fi; shift ;;
        esac
    done
    [ -z "$app" ] && { _app_box_service_usage >&2; exit 2; }
    case "$sub" in
        show)
            [ "${#rest[@]}" -eq 0 ] || { echo "usage: k2 app box-service <app-id> (to ask: k2 app box-service request <app-id> <name> [<host-port>])" >&2; exit 2; }
            _compute_call GET /cli/app/box-service "app=$(urlencode "$app")"
            if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print("%s  box services (%s)" % (v["app"], v["wall"]))
print("  " + v.get("note", ""))
svcs = v.get("services") or []
if not svcs:
    print("  none: this App reaches nothing on this box but K2 (its gateway link)")
for s in svcs:
    fw = s.get("forwarded")
    fwt = "" if fw is None or fw == "direct" else ("  forwarded" if fw else "  NOT in the running App (k2 app restart %s)" % v["app"])
    req = ("  request " + s["requestId"]) if s.get("requestId") else ""
    print("  %-8s %-14s host 127.0.0.1:%-5s in the wall %s  %s%s%s" % (s["status"], s["name"], s["hostPort"], s["inWall"], s["env"], fwt, req))
for r in v.get("pending") or []:
    print("  pending  %s  %s (by %s)" % (r["id"], r["summary"], r["requestedBy"]))
for p in v.get("presets") or []:
    if p.get("error"):
        print("  preset   %-14s %s" % (p["name"], p["error"]))
    else:
        print("  preset   %-14s 127.0.0.1:%s (%s): %s" % (p["name"], p["hostPort"], p["from"], p["what"]))
print("  never    " + v.get("refused", ""))'
            ;;
        request)
            [ "${#rest[@]}" -ge 1 ] && [ "${#rest[@]}" -le 2 ] || { echo "usage: k2 app box-service request <app-id> <name> [<host-port>] [--app-port N] [--reason TEXT]" >&2; exit 2; }
            _app_rt_post /cli/app/box-service/request "$(_compute_json K2_CJ_app="$app" K2_CJ_name="${rest[0]}" K2_CJ_hostPort="${rest[1]:-}" K2_CJ_appPort="$app_port" K2_CJ_reason="$reason")"
            case "$_CP_STATUS" in 200|202) ;; *) _compute_fail "$_CP_STATUS" "$_CP_BODY" ;; esac
            if [ "$json_out" = 1 ]; then
                printf '%s\n' "$_CP_BODY"
                printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; sys.exit(2 if json.load(sys.stdin).get("status") == "held" else 0)'
                return $?
            fi
            local rc=0
            printf '%s' "$_CP_BODY" | _app_held_exit || rc=$?
            if [ "$rc" = 2 ] && ! _compute_in_session && [ -t 0 ] && [ "${K2_NO_APPROVE:-}" != "1" ]; then
                # The owner at a terminal outside K2: approve right here.
                local id
                id="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin)["request"]["id"])')"
                _app_approve "$id"
                return 0
            fi
            return "$rc"
            ;;
        revoke|rm)
            [ "${#rest[@]}" -eq 1 ] || { echo "usage: k2 app box-service revoke <app-id> <name>" >&2; exit 2; }
            _app_rt_post /cli/app/box-service/revoke "$(_compute_json K2_CJ_app="$app" K2_CJ_name="${rest[0]}")"
            [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
            if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print(("revoked %s (shut at once)" % v["name"]) if v.get("revoked") else v.get("hint", "not a box service of this App"))'
            ;;
    esac
}


# Canonical family. Token is nested here; leftover mint is top-level k2 skin-token.
# k2 app update is unknown (usage), not the desktop updater (k2 update --app).
