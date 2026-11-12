# ── k2 root-helper — K2's one root door on Linux ───────────────────────
# (prd-k2-root-helper-v1 §8.3). `status` is a read anyone (agents too) may
# do; `update` / `rollback` are owner/admin steps the daemon refuses for
# any passport — run them outside a K2 terminal. The one root install is
# `k2 root-helper install-command`, pasted by an admin in an SSH session.
_root_helper_usage() {
    cat <<'EOF'
k2 root-helper — K2's one root door on Linux (/usr/local/libexec/k2-root-helper)

  k2 root-helper status [--events] [--json]   state, version, protocol, auto-heal, legacy helpers
  k2 root-helper update [--from <dir>] [--version <x.y.z> | --match-installed]
                                              install the signed helper for this daemon's version now
                                              (--from: a dir holding k2-root-helper-linux-<arch> + .sig; air-gap)
                                              (--version: that release's helper instead; --match-installed: the
                                               version of the daemon binary on disk, e.g. after a binary swap
                                               before the restart. It warns when a newer daemon is installed
                                               or staged than the helper it fetches.)
  k2 root-helper rollback                     one step back to the previous helper
  k2 root-helper install-command              the one root command for a box without it

Agents: if hosted mail, the db sidecar or the vault says `helper_needed`,
ask your human (k2 tickets ask). Never install Stalwart or Postgres by hand.
EOF
}

# _root_helper_fail_if_error <raw "body\ncode"> — when the daemon gave no
# answer, a non-200, or an `error` body (a string like "route not found",
# or {code, hint}), print one plain line on stderr and exit 1. Otherwise
# return 0 and print nothing.
_root_helper_fail_if_error() {
    local raw="$1" code body
    code="${raw##*$'\n'}"
    body="${raw%$'\n'*}"
    if [ -z "$raw" ] || [ "$code" = "$raw" ] || [ -z "$code" ] || [ "$code" = "000" ]; then
        echo "k2 root-helper: daemon unreachable" >&2
        exit 1
    fi
    local line
    line="$(K2_RH_BODY="$body" K2_RH_CODE="$code" python3 -c '
import json, os
raw, code = os.environ["K2_RH_BODY"], os.environ["K2_RH_CODE"]
try:
    d = json.loads(raw)
except Exception:
    d = None
e = d.get("error") if isinstance(d, dict) else None
if isinstance(e, dict):
    msg = str(e.get("code") or "error")
    if e.get("hint"):
        msg += ": " + str(e["hint"])
elif e is not None:
    msg = str(e)
elif code != "200":
    msg = "HTTP " + code + (": " + raw.strip()[:200] if raw.strip() else "")
elif not isinstance(d, dict):
    msg = "unexpected answer from the daemon: " + (raw.strip()[:200] or "(empty)")
else:
    msg = ""
print(msg)
')" || line="unreadable answer from the daemon (HTTP $code)"
    if [ -n "$line" ]; then
        echo "k2 root-helper: error: $line" >&2
        exit 1
    fi
    return 0
}

_root_helper_render() {
    # _root_helper_render <as_json> <raw "body\ncode">
    # --json passes the daemon body through untouched (still exit 1 when it
    # is an error); otherwise an error is one plain stderr line + exit 1.
    local as_json="$1" raw="$2" code body
    code="${raw##*$'\n'}"
    body="${raw%$'\n'*}"
    if [ -z "$raw" ] || [ "$code" = "$raw" ] || [ "$code" = "000" ]; then
        echo "k2 root-helper: daemon unreachable" >&2
        exit 1
    fi
    if [ "$as_json" = "1" ]; then
        printf '%s\n' "$body"
        [ "$code" = "200" ] || exit 1
        return 0
    fi
    _root_helper_fail_if_error "$raw"
    K2_RH_BODY="$body" python3 -c '
import json, os
d = json.loads(os.environ["K2_RH_BODY"])
h = d.get("heal")
if h is not None and d.get("target"):
    print("helper for :", "v" + str(d["target"]))
for w in d.get("warnings") or []:
    print("warning    :", w)
if h is not None:
    print("heal       :", h.get("result"), {k: v for k, v in h.items() if k != "result"} or "")
    dr = d.get("dropin") or {}
    if dr and dr.get("result") not in ("not_applicable", "opted_out"):
        word = {"installed": "written now (through the helper; systemd reloaded)", "current": "current", "stale": "still the older helper drop-in", "failed": "not written"}.get(dr.get("result"), str(dr.get("result")))
        line = "%s %s" % (dr.get("unit") or "unit", word)
        if dr.get("reason"):
            line += ": " + str(dr["reason"])
        print("unit dropin:", line)
        if dr.get("restartNeeded"):
            print("restart    : once, when convenient: " + str(dr.get("restart")))
            print("             (" + str(dr.get("why")) + ")")
    d = d.get("status") or {}
if "state" in d:
    print("state      :", d.get("state"), "(" + str(d.get("path")) + ")")
    print("version    :", d.get("version"), " protocol", d.get("protocol"), " daemon", d.get("daemonVersion"))
    lg = d.get("legacy") or {}
    print("legacy     : mail", lg.get("mail"), "· pg", "present" if lg.get("pg") else "absent")
    print("setup      :", d.get("rootSetup"))
    ah = d.get("autoHeal") or {}
    if ah.get("lastResult"):
        print("auto-heal  :", ah["lastResult"].get("result"), ah["lastResult"].get("reason", ""))
    elif ah.get("superseded"):
        import time
        sp = ah["superseded"]
        at = time.strftime("%Y-%m-%d %H:%M UTC", time.gmtime(sp["at"])) if sp.get("at") else "?"
        print("auto-heal  : earlier attempt (%s) said %s: %s; %s" % (at, sp.get("result"), sp.get("reason", ""), sp.get("why", "")))
    c = d.get("cli") or {}
    if c:
        st = c.get("state")
        line = "%s %s" % (c.get("path"), ("v" + str(c["version"])) if c.get("version") else "")
        if st == "stale":
            line += " (%s) — older than the daemon (v%s); auto-heal refreshes it through the root helper" % (
                "root-owned" if c.get("rootOwned") else "not root-owned", c.get("daemonCliVersion"))
        elif st == "current":
            line += " — current"
        else:
            line += " — " + {"absent": "not installed there", "symlink": "a symlink (the app or a package owns it)", "not_k2": "not the K2 CLI"}.get(st, str(st))
        print("cli        :", line.replace("  ", " "))
        lr = c.get("lastResult") or {}
        if lr and lr.get("result") not in ("current", "not_managed"):
            print("cli refresh:", lr.get("result"), lr.get("reason") or ("%s -> %s" % (lr.get("from"), lr.get("to")) if lr.get("to") else ""))
    if d.get("plainSudoRestartUsed"):
        print("note       : a Stalwart restart used plain sudo systemctl (no helper)")
    if d.get("rootSetup") == "copy-command":
        print("one root step (an admin SSH session, never a K2 terminal):")
        print("  " + str(d.get("installCommand")))
    ev = (d.get("lastEvents") or {}).get("events") or []
    for e in ev[-20:]:
        x = e.get("event") or {}
        print("  event", e.get("at"), x.get("verb"), x.get("result"), (x.get("caller") or {}).get("pid"))
'
}

cmd_root_helper() {
    local sub="${1:-status}"
    [ $# -gt 0 ] && shift
    local as_json=0 events=0 from="" version="" match_installed=0 raw
    case "$sub" in
        help|--help|-h) _root_helper_usage; return 0 ;;
    esac
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=1; shift ;;
            --events) events=1; shift ;;
            --from) from="${2:-}"; shift 2 ;;
            --version) version="${2:-}"; [ -n "$version" ] || { echo "usage: --version needs x.y.z" >&2; exit 2; }; shift 2 ;;
            --match-installed) match_installed=1; shift ;;
            -h|--help) _root_helper_usage; return 0 ;;
            *) echo "usage: unknown argument '$1' (k2 root-helper --help)" >&2; exit 2 ;;
        esac
    done
    case "$sub" in
        status)
            local q="refresh=1"
            [ "$events" = 1 ] && q="$q&events=1"
            raw="$(_sidecar_send GET /cli/root-helper/status "$q")" || true
            _root_helper_render "$as_json" "$raw"
            ;;
        update)
            local body="{}"
            if [ -n "$version" ] && [ "$match_installed" = 1 ]; then
                echo "usage: give --version or --match-installed, not both" >&2
                exit 2
            fi
            if [ -n "$from" ]; then
                case "$from" in /*) ;; *) from="$(cd "$from" 2>/dev/null && pwd)" || { echo "--from: no such directory" >&2; exit 2; } ;; esac
            fi
            body="$(K2_RH_FROM="$from" K2_RH_VERSION="$version" K2_RH_MATCH="$match_installed" python3 -c '
import json, os
b = {}
if os.environ["K2_RH_FROM"]:
    b["fromDir"] = os.environ["K2_RH_FROM"]
if os.environ["K2_RH_VERSION"]:
    b["version"] = os.environ["K2_RH_VERSION"]
if os.environ["K2_RH_MATCH"] == "1":
    b["matchInstalled"] = True
print(json.dumps(b))')"
            raw="$(_sidecar_send POST /cli/root-helper/update "$body")" || true
            _root_helper_render "$as_json" "$raw"
            ;;
        rollback)
            raw="$(_sidecar_send POST /cli/root-helper/rollback "{}")" || true
            _root_helper_render "$as_json" "$raw"
            ;;
        install-command)
            raw="$(_sidecar_send GET /cli/root-helper/status "")" || true
            if [ "$as_json" = "1" ]; then
                _root_helper_render 1 "$raw"
                return 0
            fi
            _root_helper_fail_if_error "$raw"
            local body="${raw%$'\n'*}" cmd
            cmd="$(K2_RH_BODY="$body" python3 -c 'import json, os; print(json.loads(os.environ["K2_RH_BODY"]).get("installCommand") or "")')"
            if [ -z "$cmd" ]; then
                echo "k2 root-helper: error: the daemon gave no install command" >&2
                exit 1
            fi
            printf '%s\n' "$cmd"
            ;;
        *)
            echo "usage: unknown root-helper verb '$sub' (status|update|rollback|install-command)" >&2
            exit 2
            ;;
    esac
}

