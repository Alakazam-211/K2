# ── k2 app doctor --store (0.46.9, the Tauri App SDK) ──────────────
_app_store_doctor_usage() {
    cat <<'EOF'
%include cli/text/help/app/doctor-store.txt
EOF
}

_app_store_doctor() {
    local folder="$1" voice="$2" json_out="$3"
    [ -d "$folder" ] || { echo "k2 app doctor --store: $folder is not a folder" >&2; exit 2; }
    _app_rt_post /cli/app/doctor/store "$(_compute_json K2_CJ_folder="$(_app_rt_abs "$folder")" K2_CJ_voice_b="$voice")"
    [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
    if [ "$json_out" = 1 ]; then
        printf '%s\n' "$_CP_BODY"
    else
        printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print("Store check: %s" % v.get("folder"))
print("  calls: %s (%s)" % ("checked" if v.get("voice") else "not checked", v.get("voiceWhy")))
for k in ("tauri", "tao", "wry"):
    if v.get(k):
        print("  %s %s" % (k, v[k]))
fs = v.get("findings") or []
for f in fs:
    print("%-5s [%s] %s: %s" % (f["level"].upper(), f["platform"], f["path"], f["problem"]))
    print("      fix: %s" % f["fix"])
for n in v.get("notes") or []:
    print("note: " + n)
errs = sum(1 for f in fs if f["level"] == "error")
warns = len(fs) - errs
passed = v.get("passed") or []
if v.get("ok"):
    print("Ready: no errors (%d warning%s), %d check%s passed." % (warns, "" if warns == 1 else "s", len(passed), "" if len(passed) == 1 else "s"))
else:
    print("Not ready: %d error%s, %d warning%s." % (errs, "" if errs == 1 else "s", warns, "" if warns == 1 else "s"))'
    fi
    printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; sys.exit(0 if json.load(sys.stdin).get("ok") else 1)' || exit 1
}

cmd_app_doctor() {
    local json_out=0 store=0 folder="" voice=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            --store) store=1; shift ;;
            --voice) voice=1; shift ;;
            --no-voice) voice=0; shift ;;
            -h|--help) if [ "$store" = 1 ]; then _app_store_doctor_usage; else _app_rt_usage; fi; return 0 ;;
            -*) echo "Unknown flag: $1 (k2 app doctor --help)" >&2; exit 2 ;;
            *) if [ -z "$folder" ]; then folder="$1"; shift; else echo "Unexpected argument: $1 (k2 app doctor --help)" >&2; exit 2; fi ;;
        esac
    done
    if [ "$store" = 1 ]; then
        _app_store_doctor "${folder:-.}" "$voice" "$json_out"
        return $?
    fi
    if [ -n "$voice" ]; then
        echo "k2 app doctor: --voice and --no-voice go with --store (k2 app doctor --store --help)" >&2; exit 2
    fi
    if [ -n "$folder" ]; then
        _app_reach_doctor "$folder" "$json_out"
        return $?
    fi
    _compute_call GET /cli/app/box ""
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; else printf '%s' "$_CP_BODY" | _app_box_print; fi
    if printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; sys.exit(0 if json.load(sys.stdin).get("ready") else 1)'; then
        return 0
    fi
    exit 1
}

# k2 app engine install [--from DIR] [--json] (S11): the owner installs or
# updates k2-app-engine through the root helper.
cmd_app_engine() {
    case "${1:-}" in
        install)
            shift
            local from="" json_out=0
            while [ $# -gt 0 ]; do
                case "$1" in
                    --from) _need_value "$@"; from="${2:-}"; shift 2 ;;
                    --json) json_out=1; shift ;;
                    -h|--help) _app_rt_usage; return 0 ;;
                    *) echo "Unknown argument: $1" >&2; exit 2 ;;
                esac
            done
            [ -n "$from" ] && from="$(_app_rt_abs "$from")"
            local raw
            raw="$(K2_CP_MAX_TIME=600 _compute_send POST /cli/app/engine/install "$(_compute_json K2_CJ_from="$from")")"
            _compute_split "$raw"
            [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
            if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; else
                printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
e = v.get("engine", {})
print("k2-app-engine %s installed and answering (protocol %s, runtime %s, %s)" % (e.get("version"), e.get("protocol"), e.get("runtime"), e.get("podman")))'
            fi
            ;;
        status)
            shift
            local json_out=0
            while [ $# -gt 0 ]; do
                case "$1" in
                    --json) json_out=1; shift ;;
                    -h|--help) _app_rt_usage; return 0 ;;
                    *) echo "Unknown argument: $1" >&2; exit 2 ;;
                esac
            done
            local raw
            raw="$(_compute_send GET /cli/app/engine/status "")"
            _compute_split "$raw"
            [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
            if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; else
                printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
def b(x):
    return "v%s protocol %s" % (x.get("version"), x.get("protocol")) if x else "-"
if v.get("state") == "absent":
    print("App engine: not installed on this box")
    sys.exit(0)
print("App engine: %s" % v.get("state"))
print("  this K2:   %s" % b(v.get("want")))
print("  installed: %s" % b(v.get("installed")))
print("  running:   %s" % b(v.get("running")))
print("  linger:    %s" % ("on" if v.get("linger") else "OFF (Apps stop at the next k2app logout)"))
h = v.get("heal") or {}
if h:
    print("  heal:      %s%s" % (h.get("state"), (": " + h["why"]) if h.get("why") else ""))
w = (v.get("apps") or {}).get("waitingEngine") or 0
if w:
    print("  waiting:   %d App(s) wait for the engine" % w)'
            fi
            ;;
        -h|--help|"") _app_rt_usage ;;
        *) echo "usage: k2 app engine install [--from DIR] [--json] | k2 app engine status [--json]" >&2; exit 2 ;;
    esac
}

# k2 app rehost … (0.46.4, update continuity S2): the re-host wave, one
# App now, and its update settings.
cmd_app_rehost() {
    local json_out=0
    case "${1:-}" in
        status)
            shift
            local app=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --json) json_out=1; shift ;;
                    -h|--help) _app_rt_usage; return 0 ;;
                    -*) echo "Unknown argument: $1" >&2; exit 2 ;;
                    *) app="$1"; shift ;;
                esac
            done
            local raw
            if [ -n "$app" ]; then
                raw="$(_compute_send GET /cli/app/rehost "app=$(urlencode "$app")")"
            else
                raw="$(_compute_send GET /cli/app/rehost "")"
            fi
            _compute_split "$raw"
            [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
            if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
if "rehost" in v:
    r = v["rehost"]; fp = r.get("fingerprint") or {}
    print("%s: policy %s, when it can'"'"'t run two copies: %s" % (v["app"], r.get("policy"), r.get("whenNoOverlap")))
    print("  runs on: K2 %s, engine %s" % (fp.get("k2Version", "?"), fp.get("engineVersion", "-")))
    print("  due: %s; sessions: %s" % (r.get("due") or "no", r.get("sessions")))
    w = r.get("wave") or {}
    if w:
        print("  wave: %s%s" % (w.get("state"), (" (" + w["code"] + ")") if w.get("code") else ""))
        if w.get("detail"):
            print("    " + w["detail"])
        if w.get("mode"):
            print("  last re-host: %s (%s), %s" % (w["mode"], {"overlap": "no downtime", "pause": "stopped briefly"}.get(w["mode"], "?"), "asked for with k2 app rehost" if w.get("forced") else "started by K2 after an update"))
    sys.exit(0)
print("Re-host wave (K2 %s): %s%s" % (v.get("k2Version"), v.get("state"), (" — " + v["reason"]) if v.get("reason") else ""))
for a, s in sorted((v.get("apps") or {}).items()):
    print("  %-24s %-9s %s%s" % (a, s.get("state"), s.get("code") or "", (": " + s["detail"]) if s.get("detail") else ""))'
            ;;
        policy)
            shift
            local app="" policy="" window="" nooverlap=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --policy) _need_value "$@"; policy="${2:-}"; shift 2 ;;
                    --window) _need_value "$@"; window="${2:-}"; shift 2 ;;
                    --when-no-overlap) _need_value "$@"; nooverlap="${2:-}"; shift 2 ;;
                    --json) json_out=1; shift ;;
                    -h|--help) _app_rt_usage; return 0 ;;
                    -*) echo "Unknown argument: $1" >&2; exit 2 ;;
                    *) app="$1"; shift ;;
                esac
            done
            [ -n "$app" ] || { echo "usage: k2 app rehost policy <app> [--policy P] [--window HH:MM-HH:MM] [--when-no-overlap pause|defer]" >&2; exit 2; }
            local raw
            raw="$(_compute_send POST /cli/app/rehost/policy "$(_compute_json K2_CJ_app="$app" K2_CJ_policy="$policy" K2_CJ_window="$window" K2_CJ_whenNoOverlap="$nooverlap")")"
            _compute_split "$raw"
            [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
            if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; else
                printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin); r = v["rehost"]
print("%s: policy %s%s; when it can'"'"'t run two copies: %s" % (v["app"], r["policy"], (" " + r["window"]) if r.get("window") else "", r["whenNoOverlap"]))
if v.get("warning"):
    print("warning: " + v["warning"])'
            fi
            ;;
        -h|--help|"") _app_rt_usage ;;
        *)
            local app="" yes="" pause=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --yes) yes=1; shift ;;
                    --pause) pause=1; shift ;;
                    --json) json_out=1; shift ;;
                    -h|--help) _app_rt_usage; return 0 ;;
                    -*) echo "Unknown argument: $1" >&2; exit 2 ;;
                    *) app="$1"; shift ;;
                esac
            done
            local raw
            raw="$(K2_CP_MAX_TIME=1800 _compute_send POST /cli/app/rehost/run "$(_compute_json K2_CJ_app="$app" K2_CJ_yes_b="$yes" K2_CJ_pause_b="$pause")")"
            _compute_split "$raw"
            [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
            if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; else
                printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
r = v.get("rehost")
app = v.get("app") or sys.argv[1]
how = {"overlap": "the new instance started beside the old one and took over, with no downtime", "pause": "the App was stopped briefly (two copies of it can not run at once), then started on the new instance"}.get(v.get("mode"))
if r == "current":
    print("%s already runs on this K2 (nothing to move)" % app)
elif r == "done":
    print("%s moved to a fresh instance (%s)" % (app, (v.get("deployment") or {}).get("id")))
    if how:
        print("  how: %s: %s" % (v["mode"], how))
else:
    print("%s: the re-host failed; it keeps running on its previous instance: %s" % (app, v.get("why")))
    if how:
        print("  how it was tried: %s (%s)" % (v["mode"], {"overlap": "a new instance beside the old one", "pause": "stop first, then start"}[v["mode"]]))
    print("  why: k2 app history %s ; try again: k2 app rehost %s" % (app, app))
    sys.exit(1)' "$app"
            fi
            ;;
    esac
}

# k2 app box [doctor|prepare …]. Prepare installs distro packages: allow
# up to 15 minutes for the call.
cmd_app_box() {
    case "${1:-}" in
        ""|doctor|status) shift || true; cmd_app_doctor "$@" ;;
        podman)
            # Lane P: k2 app box podman uninstall [--json] (owner).
            shift
            [ "${1:-}" = "uninstall" ] || { echo "usage: k2 app box podman uninstall [--json]" >&2; exit 2; }
            shift
            local json_out=0 raw
            while [ $# -gt 0 ]; do
                case "$1" in
                    --json) json_out=1; shift ;;
                    -h|--help) _app_rt_usage; return 0 ;;
                    *) echo "Unknown argument: $1" >&2; exit 2 ;;
                esac
            done
            raw="$(K2_CP_MAX_TIME=300 _compute_send POST /cli/app/box/podman/uninstall '{}')"
            _compute_split "$raw"
            [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
            if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; else
                printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
r = json.load(sys.stdin).get("result", {})
gone = r.get("removed") or []
print("K2 Podman removed: " + (", ".join(gone) if gone else "nothing was installed"))
for k in r.get("kept") or []:
    print("  kept " + k)
print("The distro Podman is as it was.")'
            fi
            ;;
        unprep)
            # F8: k2 app box unprep [--dry-run] [--yes] [--json] (owner).
            shift
            local json_out=0 dry_run="" yes="" raw
            while [ $# -gt 0 ]; do
                case "$1" in
                    --dry-run) dry_run=1; shift ;;
                    --yes|-y) yes=1; shift ;;
                    --json) json_out=1; shift ;;
                    -h|--help) _app_rt_usage; return 0 ;;
                    *) echo "Unknown argument: $1" >&2; exit 2 ;;
                esac
            done
            if [ -n "$dry_run" ] || [ -z "$yes" ]; then
                raw="$(K2_CP_MAX_TIME=120 _compute_send POST /cli/app/box/unprep '{"dryRun":true}')"
                _compute_split "$raw"
                [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
                if [ "$json_out" = 1 ] && [ -n "$dry_run" ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
                printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print("Undo box setup would remove:")
for u in v.get("undoes") or []:
    print("  " + u)
for k in v.get("kept") or []:
    print("  keeps " + k)
run = v.get("runningApps") or []
if run:
    print("Refused while Apps run; stop these first: " + ", ".join(run))
'
                if [ -n "$dry_run" ]; then echo "Dry run: nothing changed."; return 0; fi
                if ! { [ -t 0 ] && [ -t 1 ]; }; then
                    echo "k2 app box unprep: pass --yes to undo the box setup without a prompt (--dry-run shows the plan)" >&2
                    exit 2
                fi
                local yn=""
                printf 'Undo the box setup? [y/N] '
                read -r yn || yn=""
                case "$yn" in
                    y|Y|yes|YES) ;;
                    *) echo "Nothing changed." >&2; exit 2 ;;
                esac
            fi
            raw="$(K2_CP_MAX_TIME=300 _compute_send POST /cli/app/box/unprep '{"confirm":true}')"
            _compute_split "$raw"
            [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
            if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; else
                printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
r = v.get("result") or {}
print("Box setup undone:")
for s in r.get("steps") or []:
    print("  ok   %s%s" % (s.get("step", ""), "  (changed)" if s.get("changed") else "  (already)"))
for k in v.get("kept") or r.get("kept") or []:
    print("  kept " + k)
if v.get("note"):
    print(v["note"])
'
                printf '%s' "$_CP_BODY" | _app_box_print
            fi
            ;;
        auto-prepare)
            # F6: k2 app box auto-prepare retry [--json] (owner).
            # 0.46.16: k2 app box auto-prepare on|off|status [--json] (owner).
            shift
            local ap_verb="${1:-}"
            case "$ap_verb" in
                retry|on|off|status) shift ;;
                *) echo "usage: k2 app box auto-prepare on|off|status|retry [--json]" >&2; exit 2 ;;
            esac
            local json_out=0 raw
            while [ $# -gt 0 ]; do
                case "$1" in
                    --json) json_out=1; shift ;;
                    -h|--help) _app_rt_usage; return 0 ;;
                    *) echo "Unknown argument: $1" >&2; exit 2 ;;
                esac
            done
            if [ "$ap_verb" = status ]; then
                _compute_call GET /cli/app/box ""
                if [ "$json_out" = 1 ]; then printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin).get("autoPrepare")))'; return 0; fi
                printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
ap = json.load(sys.stdin).get("autoPrepare") or {}
print("Automatic prep: %s" % ("on" if ap.get("enabled") else "off"))
o = ap.get("outcome") or {}
if o:
    print("  last run: %s%s" % (o.get("state"), (": " + (o.get("why") or o.get("error") or "")) if (o.get("why") or o.get("error")) else ""))
'
                return 0
            fi
            if [ "$ap_verb" = on ] || [ "$ap_verb" = off ]; then
                local ap_on=false
                [ "$ap_verb" = on ] && ap_on=true
                raw="$(_compute_send POST /cli/app/box/auto-prepare "{\"on\":$ap_on}")"
                _compute_split "$raw"
                [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
                if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
                printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
ap = v.get("autoPrepare") or {}
print("Automatic prep: %s (appsAutoPrepare %s)" % ("on" if ap.get("enabled") else "off", "on" if v.get("setting") else "off"))
if v.get("note"):
    print("  " + v["note"])
if ap.get("enabled"):
    print("  K2 keeps this server set up for Apps through its root helper, after every update. k2 app box auto-prepare off turns it off.")
'
                return 0
            fi
            raw="$(_compute_send POST /cli/app/box/auto-prepare/retry '{}')"
            _compute_split "$raw"
            [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
            if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; else
                echo "Automatic box prep will run again now."
                printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
ap = json.load(sys.stdin).get("autoPrepare") or {}
print("Automatic prep: %s" % ("on" if ap.get("enabled") else "off (appsAutoPrepare): turn it on for the retry to do anything"))
'
            fi
            ;;
        prepare)
            shift
            local runtime="" json_out=0 cores="" mem="" tasks="" install_podman="" dry_run="" podman_from=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --runtime) _need_value "$@"; runtime="${2:-}"; shift 2 ;;
                    --youki) runtime="youki"; shift ;;
                    --cores-mem-min) _need_value "$@"; cores="${2:-}"; shift 2 ;;
                    --apps-mem-max) _need_value "$@"; mem="${2:-}"; shift 2 ;;
                    --apps-tasks-max) _need_value "$@"; tasks="${2:-}"; shift 2 ;;
                    --install-podman|--yes|-y) install_podman=1; shift ;;
                    --dry-run) dry_run=1; shift ;;
                    --podman-from) _need_value "$@"; podman_from="${2:-}"; shift 2 ;;
                    --json) json_out=1; shift ;;
                    -h|--help) _app_rt_usage; return 0 ;;
                    *) echo "Unknown argument: $1" >&2; exit 2 ;;
                esac
            done
            case "$runtime" in ""|crun|youki) ;; *) echo "--runtime is crun or youki" >&2; exit 2 ;; esac
            local n
            for n in "$cores" "$mem" "$tasks"; do
                case "$n" in ""|*[!0-9]*) [ -z "$n" ] || { echo "sizes are plain numbers (MiB, tasks)" >&2; exit 2; } ;; esac
            done
            [ -n "$podman_from" ] && { podman_from="$(_app_rt_abs "$podman_from")"; install_podman=1; }
            local body raw
            body="$(_compute_json K2_CJ_runtime="$runtime" K2_CJ_coresMemMinMib_n="$cores" \
                K2_CJ_appsMemMaxMib_n="$mem" K2_CJ_appsTasksMax_n="$tasks" \
                K2_CJ_installPodman_b="$install_podman" K2_CJ_dryRun_b="$dry_run" K2_CJ_podmanFrom="$podman_from")"
            raw="$(K2_CP_MAX_TIME=900 _compute_send POST /cli/app/box/prepare "$body")"
            _compute_split "$raw"
            # Lane P: Podman < 5 on Ubuntu 24.04 / Debian 12. Ask a person at
            # a terminal; scripts and agents pass --install-podman (or --yes).
            if [ "$_CP_STATUS" = "409" ] && [ "$json_out" != 1 ] && [ -z "$install_podman" ] \
                && [ -t 0 ] && [ -t 1 ] \
                && printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; sys.exit(0 if json.load(sys.stdin).get("error",{}).get("code")=="podman_install_needed" else 1)' 2>/dev/null; then
                printf '%s' "$_CP_BODY" | _app_box_podman_offer
                local yn=""
                printf 'Install it now? [y/N] '
                read -r yn || yn=""
                case "$yn" in
                    y|Y|yes|YES)
                        body="$(_compute_json K2_CJ_runtime="$runtime" K2_CJ_coresMemMinMib_n="$cores" \
                            K2_CJ_appsMemMaxMib_n="$mem" K2_CJ_appsTasksMax_n="$tasks" K2_CJ_installPodman_b=1)"
                        raw="$(K2_CP_MAX_TIME=900 _compute_send POST /cli/app/box/prepare "$body")"
                        _compute_split "$raw"
                        ;;
                    *) echo "Nothing changed." >&2; exit 2 ;;
                esac
            fi
            if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; fi
            if [ "$_CP_STATUS" = "200" ] && [ -n "$dry_run" ] && [ "$json_out" != 1 ]; then
                printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
p = v.get("podman", {})
print("Dry run: nothing changed.")
a = p.get("action")
if a == "keep":
    print("  Podman %s (%s): nothing to install" % (p.get("have"), p.get("source")))
elif a == "distro":
    print("  Podman %s: %s" % (p.get("have") or "none", p.get("note", "")))
else:
    b = p.get("bundle", {})
    print("  Podman %s on %s: %s K2 Podman %s (%s)" % (p.get("have") or "none", p.get("os"), "would install" if a == "install" else "needs --install-podman to install", b.get("version"), b.get("source")))
    print("    from   %s" % b.get("url"))
    print("    sha256 %s (%s bytes)" % (b.get("sha256"), b.get("size")))
    for w in b.get("writes", []):
        print("    writes %s" % w)
k = v.get("packages") or {}
if k.get("manager"):
    if k.get("error"):
        print("  Distro packages (%s): %s" % (k["manager"], k["error"]))
    else:
        inst = k.get("install") or []
        print("  Distro packages prepare would install (%s): %s" % (k["manager"], " ".join(inst) if inst else "none, all already installed"))
        if k.get("alreadyInstalled"):
            print("    already installed: %s" % " ".join(k["alreadyInstalled"]))
    if k.get("distroPodman"):
        print("  Podman: %s" % k["distroPodman"])
elif k:
    print("  Distro packages: none (%s)" % k.get("note", "no package manager"))
if k.get("manager") and k.get("note"):
    print("    %s" % k["note"])
print("Steps prepare would run:")
for s in v.get("steps", []):
    print("  " + s)
'
                printf '%s' "$_CP_BODY" | _app_box_print
                return 0
            fi
            if [ "$_CP_STATUS" != "200" ]; then
                if [ "$json_out" != 1 ]; then
                    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
try:
    e = json.load(sys.stdin).get("error", {})
except Exception:
    sys.exit(0)
for s in e.get("steps", []):
    print("  %-4s %s%s" % ("ok" if s.get("ok") else "FAIL", s.get("verb", ""), "" if s.get("ok") else ": " + str(s.get("error", ""))))
' >&2
                fi
                _compute_fail "$_CP_STATUS" "$_CP_BODY"
            fi
            if [ "$json_out" != 1 ]; then
                printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
for s in v.get("steps", []):
    r = s.get("result")
    ch = r.get("changed") if isinstance(r, dict) else None
    print("  ok   %s%s" % (s.get("verb", ""), "" if ch is None else ("  (changed)" if ch else "  (already)")))
    aa = r.get("apparmor") if isinstance(r, dict) else None
    if isinstance(aa, dict) and aa.get("needed"):
        if aa.get("installed"):
            print("       AppArmor: installed the distro package %s (apparmor_parser %s) for the k2-app profile" % (" ".join(aa["installed"]), aa.get("parser")))
        elif aa.get("note"):
            print("       AppArmor: %s" % aa["note"])
for n in v.get("notes") or []:
    print("Note: %s" % n)
'
                printf '%s' "$_CP_BODY" | _app_box_print
                # 0.46.16: the agents leaf waits for the daemon's next start.
                printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
b = (json.load(sys.stdin).get("box") or {})
rows = {c.get("id"): c.get("status") for c in b.get("checks") or []}
if rows.get("coresSlice") == "ok" and rows.get("agentsLeaf") not in (None, "ok"):
    print("Restart K2 once so agent sessions get their own memory budget: k2 daemon restart (it signs guests out of older App sites; k2 app sessions --live shows who is on now), or pick Tonight at 3 am on the Apps setup card.")
'
            fi
            ;;
        -h|--help) _app_rt_usage ;;
        *) echo "Unknown: k2 app box $1 (doctor|prepare|unprep|auto-prepare on|off|status|retry|podman uninstall)" >&2; exit 2 ;;
    esac
}

# POST a JSON body to an App route; on a refusal print it and exit.
# The body lands in _CP_BODY, the status in _CP_STATUS.
_app_rt_post() {
    local path="$1" body="$2" raw
    raw="$(_compute_send POST "$path" "$body")"
    _compute_split "$raw"
}

# Print a validate report (stdin = {report:{…}} or {error:{report}}).
_app_rt_report() {
    python3 -c '
import json, sys
v = json.load(sys.stdin)
r = v.get("report") or (v.get("error") or {}).get("report") or {}
for e in r.get("errors", []):
    at = (" (" + e["at"] + ")") if e.get("at") else ""
    print("error   %s%s: %s" % (e.get("code", ""), at, e.get("message", "")))
for w in r.get("warnings", []):
    at = (" (" + w["at"] + ")") if w.get("at") else ""
    print("warning %s%s: %s" % (w.get("code", ""), at, w.get("message", "")))
checks = ", ".join(r.get("checks", []))
print(("ok" if r.get("ok") else "FAILED") + (" — checks: " + checks if checks else ""))
'
}

_app_rt_abs() {
    local p="$1"
    if [ -d "$p" ]; then (cd "$p" && pwd -P); else printf '%s' "$p"; fi
}

# A file's absolute path (its folder resolved; the file need not exist).
_app_rt_abs_file() {
    local p="$1" d b
    d="$(dirname -- "$p")"; b="$(basename -- "$p")"
    if [ -d "$d" ]; then printf '%s/%s' "$(cd "$d" && pwd -P)" "$b"; else printf '%s' "$p"; fi
}

# App runtime S6: POST /cli/app/validate {folder|seed, types}. Returns 0 ok,
# 1 errors in the report, 10 when Apps (beta) are off (the caller
# decides); any other refusal prints its JSON and exits 2/3.
_app_validate_seed() {
    local kind="$1" target="$2" types="$3" as_json="$4" manifest="${5-}" body
    if [ "$kind" = folder ]; then
        body="$(_compute_json K2_CJ_folder="$(_app_rt_abs "$target")" K2_CJ_types_b="$types" K2_CJ_manifest="$manifest")"
    else
        body="$(_compute_json K2_CJ_seed="$target" K2_CJ_types_b="$types")"
    fi
    _app_rt_post /cli/app/validate "$body"
    if [ "$_CP_STATUS" = "404" ] && [[ "$_CP_BODY" == *'"apps_disabled"'* ]]; then
        return 10
    fi
    [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
    if [ "$as_json" = true ]; then
        printf '%s\n' "$_CP_BODY"
    else
        printf '%s' "$_CP_BODY" | _app_rt_report
    fi
    printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; sys.exit(0 if json.load(sys.stdin).get("ok") else 1)'
}

cmd_app_build() {
    local folder="" types=0 json_out=0 dov="" manifest=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --manifest) _need_value "$@"; manifest="${2:-}"; shift 2 ;;
            --manifest=*) manifest="${1#--manifest=}"; shift ;;
            --types) types=1; shift ;;
            --disk-override) _need_value "$@"; dov="${2:-}"; shift 2 ;;
            --disk-override=*) dov="${1#--disk-override=}"; shift ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_rt_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) folder="$1"; shift ;;
        esac
    done
    [ -z "$folder" ] && { echo "usage: k2 app build <folder> [--manifest <file>] [--types] [--disk-override REASON] [--json]" >&2; exit 2; }
    [ -d "$folder" ] || { echo "k2 app build: $folder is not a folder" >&2; exit 2; }
    if [ -n "$manifest" ]; then
        [ -f "$manifest" ] || { echo "k2 app build: --manifest $manifest is not a file" >&2; exit 2; }
        manifest="$(_app_rt_abs_file "$manifest")"
    fi
    _app_rt_post /cli/app/build "$(_compute_json K2_CJ_folder="$(_app_rt_abs "$folder")" K2_CJ_types_b="$types" K2_CJ_diskOverride="$dov" K2_CJ_manifest="$manifest")"
    if [ "$_CP_STATUS" = "422" ]; then
        if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; else printf '%s' "$_CP_BODY" | _app_rt_report >&2; fi
        exit 1
    fi
    # Held for the owner (a disk override, or S42 without the switch): exit 2.
    _app_grace_done "$json_out"
    if [ "$json_out" = 1 ]; then
        printf '%s\n' "$_CP_BODY"
        return 0
    fi
    _app_disk_notes
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
s = v["seed"]
print("Seed %s" % s["hash"])
print("  %s %s  %s files, %s bytes%s" % (s["seedId"], s.get("version") or "", v.get("entries"), s["sizeBytes"],
      "  (already stored)" if v.get("reused") else ""))
if s.get("sourceRef"):
    print("  commit %s" % s["sourceRef"])
for w in (s.get("report") or {}).get("warnings", []):
    print("  warning %s: %s" % (w.get("code", ""), w.get("message", "")))
'
    _app_seed_size_note
    printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print("Deploy it: k2 app deploy <app-id> %s" % json.load(sys.stdin)["seed"]["hash"][:12])'
}

# Print one deployment answer ({deployment, done}) and exit by its result:
# live 0, failed/rolled back 1, pending/starting 2, stalled 3.
_app_rt_deployment_exit() {
    local json_out="$1"
    if [ "$json_out" = 1 ]; then
        printf '%s\n' "$_CP_BODY"
    else
        printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
d = v.get("deployment") or {}
seed = (d.get("toSeed") or "")[:12]
line = "%s %s  %s → %s  by %s  commit %s" % (d.get("kind", ""), d.get("id", ""), (d.get("fromSeed") or "-")[:12], seed,
                                               d.get("actor", ""), (d.get("commitRef") or "-")[:12])
print(line)
r = d.get("result", "")
if r == "live":
    print("live on port %s" % d.get("port") + ("  (instant: the previous instance was still warm)" if v.get("instant") else ""))
elif r in ("pending", "starting"):
    print("%s — not live yet; check: k2 app deployment %s %s" % (r, d.get("appId", ""), d.get("id", "")))
else:
    print("%s: %s" % (r, d.get("error") or ""))
'
    fi
    local result
    result="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print((json.load(sys.stdin).get("deployment") or {}).get("result",""))')"
    case "$result" in
        live) return 0 ;;
        pending|starting) exit 2 ;;
        stalled) exit 3 ;;
        *) exit 1 ;;
    esac
}

# k2 app install <app-id> [--workspace WS] [--tunnel] [--memory 8G|none] [--seed S | --folder F] [--pg-major M] [--json]
cmd_app_install() {
    # `--catalog <entry>` is the one-click install (a daemon job, app-catalog.sh).
    local _a
    for _a in "$@"; do
        case "$_a" in --catalog|--catalog=*) cmd_app_install_catalog "$@"; return ;; esac
    done
    local app="" ws="" tunnel=0 memory="" seed="" folder="" engine="" json_out=0 dov="" pgmajor="" transfer=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --transfer) transfer=1; shift ;;
            --disk-override) _need_value "$@"; dov="${2:-}"; shift 2 ;;
            --disk-override=*) dov="${1#--disk-override=}"; shift ;;
            --workspace) _need_value "$@"; ws="${2:-}"; shift 2 ;;
            --tunnel) tunnel=1; shift ;;
            --memory) _need_value "$@"; memory="${2:-}"; shift 2 ;;
            --engine) _need_value "$@"; engine="${2:-}"; shift 2 ;;
            --seed) _need_value "$@"; seed="${2:-}"; shift 2 ;;
            --folder) _need_value "$@"; folder="$(_app_rt_abs "${2:-}")"; shift 2 ;;
            --pg-major) _need_value "$@"; pgmajor="${2:-}"; shift 2 ;;
            --pg-major=*) pgmajor="${1#--pg-major=}"; shift ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_rt_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    [ -z "$app" ] && { echo "usage: k2 app install <app-id> [--workspace WS] [--tunnel [--transfer]] [--memory 8G|none] [--engine guarded|process] [--seed S|--folder F] [--pg-major 16|17]" >&2; exit 2; }
    if [ -n "$pgmajor" ]; then
        case "$pgmajor" in [1-9][0-9]) ;; *) echo "usage: --pg-major takes a Postgres major, e.g. 17" >&2; exit 2 ;; esac
    fi
    ws="$(_compute_ws "$ws")"
    local wait=""
    { [ -n "$seed" ] || [ -n "$folder" ]; } && wait=150
    _app_rt_post /cli/app/install "$(_compute_json K2_CJ_app="$app" K2_CJ_workspace="$ws" K2_CJ_tunnel_b="$tunnel" \
        K2_CJ_memory="$memory" K2_CJ_engine="$engine" K2_CJ_seed="$seed" K2_CJ_folder="$folder" K2_CJ_wait_n="$wait" \
        K2_CJ_diskOverride="$dov" K2_CJ_pgMajor_n="$pgmajor" K2_CJ_transfer_b="$transfer")"
    # An agent asking for the unwalled process engine, or a disk override
    # that waits for the owner: held for the owner.
    if [ "$_CP_STATUS" = "202" ] && [[ "$_CP_BODY" != *'"frontPort"'* ]]; then
        _app_grace_done "$json_out"
    fi
    [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    _app_disk_notes
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
a = v["app"]
print("App %s installed (front port %s, memory limit %s)" % (a["appId"], v["frontPort"],
      "none" if a.get("memoryLimitBytes") is None else "%d MiB" % (a["memoryLimitBytes"] >> 20)))
db = v.get("database")
if db:
    if db.get("created"):
        print("database %s created on Postgres %s; DATABASE_URL reference %s (put it in .env.k2)" % (db["name"], db["major"], db.get("ref")))
    else:
        print("database %s on Postgres %s (already there)" % (db["name"], db["major"]))
d = (v.get("deploy") or {}).get("deployment")
if d:
    print("deploy %s: %s %s" % (d["id"], d["result"], d.get("error") or ""))
else:
    print("Deploy it: k2 app deploy %s <folder|seed>" % a["appId"])
'
    _app_seed_size_note
}

# k2 app deploy <app-id> <folder|seed> [--reason TEXT] [--no-wait] [--types] [--json]
cmd_app_deploy() {
    local app="" target="" reason="" wait=150 types=0 json_out=0 engine="" dov="" manifest=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --disk-override) _need_value "$@"; dov="${2:-}"; shift 2 ;;
            --disk-override=*) dov="${1#--disk-override=}"; shift ;;
            --manifest) _need_value "$@"; manifest="${2:-}"; shift 2 ;;
            --manifest=*) manifest="${1#--manifest=}"; shift ;;
            --engine) _need_value "$@"; engine="${2:-}"; shift 2 ;;
            --engine=*) engine="${1#--engine=}"; shift ;;
            --reason) _need_value "$@"; reason="${2:-}"; shift 2 ;;
            --no-wait) wait=0; shift ;;
            --types) types=1; shift ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_rt_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) if [ -z "$app" ]; then app="$1"; else target="$1"; fi; shift ;;
        esac
    done
    { [ -z "$app" ] || [ -z "$target" ]; } && { echo "usage: k2 app deploy <app-id> <folder|seed> [--manifest <file>] [--engine guarded|process] [--reason TEXT] [--no-wait] [--disk-override REASON]" >&2; exit 2; }
    case "$engine" in ""|guarded|process) ;; *) echo "--engine is guarded or process" >&2; exit 2 ;; esac
    if [ -n "$manifest" ]; then
        [ -d "$target" ] || { echo "k2 app deploy: --manifest goes with a folder (a stored Seed already has its manifest)" >&2; exit 2; }
        [ -f "$manifest" ] || { echo "k2 app deploy: --manifest $manifest is not a file" >&2; exit 2; }
        manifest="$(_app_rt_abs_file "$manifest")"
    fi
    local body
    if [ -d "$target" ]; then
        body="$(_compute_json K2_CJ_app="$app" K2_CJ_folder="$(_app_rt_abs "$target")" K2_CJ_reason="$reason" K2_CJ_wait_n="$wait" K2_CJ_types_b="$types" K2_CJ_engine="$engine" K2_CJ_diskOverride="$dov" K2_CJ_manifest="$manifest")"
    else
        body="$(_compute_json K2_CJ_app="$app" K2_CJ_seed="$target" K2_CJ_reason="$reason" K2_CJ_wait_n="$wait" K2_CJ_engine="$engine" K2_CJ_diskOverride="$dov")"
    fi
    _app_rt_post /cli/app/deploy "$body"
    if [ "$_CP_STATUS" = "422" ]; then
        printf '%s' "$_CP_BODY" | _app_rt_report >&2
        exit 1
    fi
    # Held for the owner (new caps S20, no switch S42, a disk override):
    # exit 2 with the request, never exit 3 with an empty error.
    _app_grace_done "$json_out"
    [ "$json_out" = 1 ] || _app_disk_notes
    [ "$json_out" = 1 ] || _app_seed_size_note
    [ "$json_out" = 1 ] || printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
try:
    site = json.load(sys.stdin).get("devSite")
except ValueError:
    site = None
if site:
    print("note: the dev site %s keeps running from this folder (k2 publish stop %s)." % (site, site))
'
    _app_rt_deployment_exit "$json_out"
}

# 0.46.4 (field report #3): what a build or deploy packed — size, files, the
# heaviest paths — and the .k2appignore tip when the folder had none.
_app_seed_size_note() {
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
try:
    v = json.load(sys.stdin)
except ValueError:
    sys.exit(0)
m = v.get("seedSize") or (v.get("deploy") or {}).get("seedSize")
if not isinstance(m, dict):
    sys.exit(0)
def h(n):
    for unit in ("B", "KB", "MB", "GB"):
        if n < 1024 or unit == "GB":
            return ("%d %s" % (n, unit)) if unit == "B" else ("%.1f %s" % (n, unit))
        n = n / 1024.0
print("Seed: %s, %d files%s" % (h(m.get("bytes", 0)), m.get("files", 0), ", .k2appignore applied" if m.get("ignoreFile") else ""))
if m.get("largest"):
    print("  largest: " + ", ".join("%s %s" % (p["path"], h(p["bytes"])) for p in m["largest"]))
if not m.get("ignoreFile") and m.get("bytes", 0) > (20 << 20):
    print("  Leave out what the App does not run: a .k2appignore at the folder root (gitignore syntax),")
    print("  e.g. var/  .env*  node_modules/.cache  .next/cache  (k2 study app-deploys)")
'
}

# k2 app rollback <app-id> [--to SEED] [--no-wait] [--json]
cmd_app_rollback() {
    local app="" to="" wait=150 json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --to) _need_value "$@"; to="${2:-}"; shift 2 ;;
            --no-wait) wait=0; shift ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_rt_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    [ -z "$app" ] && { echo "usage: k2 app rollback <app-id> [--to SEED]" >&2; exit 2; }
    _app_rt_post /cli/app/rollback "$(_compute_json K2_CJ_app="$app" K2_CJ_to="$to" K2_CJ_wait_n="$wait")"
    [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
    _app_rt_deployment_exit "$json_out"
}

# k2 app stop|start|restart <app-id> [--json]
cmd_app_lifecycle() {
    local verb="$1"; shift
    local app="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            -h|--help) _app_rt_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    [ -z "$app" ] && { echo "usage: k2 app $verb <app-id>" >&2; exit 2; }
    local wait=150
    [ "$verb" = "stop" ] && wait=""
    _app_rt_post "/cli/app/$verb" "$(_compute_json K2_CJ_app="$app" K2_CJ_wait_n="$wait")"
    [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
    if printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; sys.exit(0 if "deployment" in json.load(sys.stdin) else 1)'; then
        _app_rt_deployment_exit "$json_out"
        return $?
    fi
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; else
        printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print("%s: %s" % (v.get("app"), v.get("state") or v.get("already") or "ok"))'
    fi
}

# k2 app link <app-id> [--rotate] [--json] — the App's gateway link (S14).
# Exit 0 open, 1 not open (the App isn't running walled), 2 usage, 3 infra.
cmd_app_link() {
    local app="" rotate=0 json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --rotate) rotate=1; shift ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_rt_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    [ -z "$app" ] && { echo "usage: k2 app link <app-id> [--rotate] [--json]" >&2; exit 2; }
    if [ "$rotate" = 1 ]; then
        _app_rt_post /cli/app/link/rotate "$(_compute_json K2_CJ_app="$app")"
        [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
    else
        _compute_call GET /cli/app/link "app=$(urlencode "$app")"
    fi
    if [ "$json_out" = 1 ]; then
        printf '%s\n' "$_CP_BODY"
    else
        printf '%s' "$_CP_BODY" | python3 -c '
import json, sys, time
v = json.load(sys.stdin)
l = v.get("link") or {}
def t(x):
    return time.strftime("%Y-%m-%d %H:%M:%S", time.localtime(x)) if x else "-"
if not l.get("open"):
    print("%s: no open gateway link (it is not running walled: k2 app deploy %s <seed> --engine guarded)" % (v.get("app"), v.get("app")))
    sys.exit(0)
print("%s  gateway link %s on 127.0.0.1:%s" % (v.get("app"), l.get("linkId"), l.get("port")))
print("  in the wall  %s  (pass file %s)" % (l.get("inWallUrl"), l.get("passFile")))
print("  pass caps    %s" % (", ".join(l.get("caps") or []) or "none (GET /gw/me only)"))
print("  rooms        %s" % ", ".join(l.get("rooms") or []))
print("  pass         expires %s, rotates %s (%s rotations, %s live)" % (t(l.get("passExpiresAt")), t(l.get("rotateAt")), l.get("rotations"), l.get("livePasses")))
if l.get("lastError"):
    print("  last error   %s" % l["lastError"])
print("  requests     %s (refused %s, wrong link %s), %s connected" % (l.get("requests"), l.get("refused"), l.get("wrongLink"), l.get("connections")))
'
    fi
    printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; sys.exit(0 if (json.load(sys.stdin).get("link") or {}).get("open") else 1)'
}

# k2 app seal <app-id> [--json]             — does the App have its seal key (K-M3)?
# k2 app seal rotate <app-id> [--yes] [--json] — a new key: signs EVERYONE out.
# The value is never shown. Exit 0 ok, 1 no key yet, 2 usage, 3 refused/infra.
cmd_app_seal() {
    local app="" rotate=0 yes=0 json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            rotate) rotate=1; shift ;;
            --yes|-y) yes=1; shift ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_rt_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    [ -z "$app" ] && { echo "usage: k2 app seal <app-id> | k2 app seal rotate <app-id> [--yes]" >&2; exit 2; }
    if [ "$rotate" = 1 ]; then
        if [ "$yes" != 1 ]; then
            echo "Rotating $app's seal key signs EVERYONE out of it (members and guests); their passwords still work." >&2
            if [ -t 0 ] && [ -t 2 ]; then
                printf 'Type the App id to go ahead: ' >&2
                local typed=""
                read -r typed
                [ "$typed" = "$app" ] || { echo "Not rotated." >&2; exit 2; }
                yes=1
            else
                echo "Not rotated: run k2 app seal rotate $app --yes" >&2
                exit 2
            fi
        fi
        _app_rt_post /cli/app/seal/rotate "$(_compute_json K2_CJ_app="$app" K2_CJ_yes_b="$yes")"
        [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
        if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
        printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print("%s: seal key rotated (version %s)" % (v.get("app"), v.get("version")))
print(v.get("warning", ""))
print(v.get("next", ""))'
        return 0
    fi
    _compute_call GET /cli/app/seal "app=$(urlencode "$app")"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; else
        printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
s = v.get("seal") or {}
if not s.get("vaultOpen"):
    print("%s: the vault is not open (%s)" % (v.get("app"), s.get("error")))
elif not s.get("present"):
    print("%s: no seal key yet (K2 makes it at the next start)" % v.get("app"))
else:
    state = "granted" if s.get("granted") else "not granted yet (next start)"
    if s.get("disabled"):
        state = "disabled in the vault"
    print("%s: seal key version %s, %s; the App reads %s" % (v.get("app"), s.get("version"), state, s.get("file")))'
    fi
    printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; sys.exit(0 if (json.load(sys.stdin).get("seal") or {}).get("present") else 1)'
}

# k2 app deployment <app-id> [<deployment-id>] [--json] — who, commit, state (L3).
cmd_app_deployment() {
    local app="" id="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            -h|--help) _app_rt_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) if [ -z "$app" ]; then app="$1"; else id="$1"; fi; shift ;;
        esac
    done
    [ -z "$app" ] && { echo "usage: k2 app deployment <app-id> [<deployment-id>]" >&2; exit 2; }
    local q="app=$(urlencode "$app")"
    [ -n "$id" ] && q="${q}&id=$(urlencode "$id")"
    _compute_call GET /cli/app/deployment "$q"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
d = v["deployment"]
print("%s %s (%s)" % (v["app"], d["id"], d["kind"]))
print("  state   %s%s" % (v["state"], ("  — " + d["error"]) if d.get("error") else ""))
print("  who     %s" % v["who"])
print("  commit  %s" % (v.get("commit") or "-"))
print("  seed    %s → %s" % ((d.get("fromSeed") or "-")[:12], (d.get("toSeed") or "-")[:12]))'
}

# k2 app config <app-id> [--memory 8G|none] [--cpu-weight N] [--tasks N] [--disk 2G]
#   [--egress add|rm HOST:PORT]… [--egress-mode allowlist|broad] [--port add|rm 5000/udp]…
#   [--recycle-every 24h|none] [--recycle-at 85%|none] [--isolation container|process]
#   [--path guarded|open] [--risk normal|high] [--db-private on|off] [--db-raw on|off] [--json]
# S13: narrowing applies now; a widening is held for the owner (exit 2).
cmd_app_config() {
    local app="" json_out=0
    local -a kv=()
    local egress_add="" egress_rm="" port_add="" port_rm=""
    local io_r="" io_w="" io_ri="" io_wi=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --memory) _need_value "$@"; kv+=("K2_CJ_memory=${2:-}"); shift 2 ;;
            --cpu-weight) _need_value "$@"; kv+=("K2_CJ_cpuWeight=${2:-}"); shift 2 ;;
            --tasks) _need_value "$@"; kv+=("K2_CJ_tasks=${2:-}"); shift 2 ;;
            --disk) _need_value "$@"; kv+=("K2_CJ_disk=${2:-}"); shift 2 ;;
            --egress)
                case "${2:-}" in
                    add) egress_add="${egress_add:+$egress_add,}${3:-}" ;;
                    rm) egress_rm="${egress_rm:+$egress_rm,}${3:-}" ;;
                    *) echo "usage: --egress add|rm HOST:PORT" >&2; exit 2 ;;
                esac
                shift 3 ;;
            --egress-mode) _need_value "$@"; kv+=("K2_CJ_egressMode=${2:-}"); shift 2 ;;
            --port)
                case "${2:-}" in
                    add) port_add="${port_add:+$port_add,}${3:-}" ;;
                    rm) port_rm="${port_rm:+$port_rm,}${3:-}" ;;
                    *) echo "usage: --port add|rm PORT[/tcp|udp]" >&2; exit 2 ;;
                esac
                shift 3 ;;
            --recycle-every) _need_value "$@"; kv+=("K2_CJ_recycleEvery=${2:-}"); shift 2 ;;
            --recycle-at) _need_value "$@"; kv+=("K2_CJ_recycleAt=${2:-}"); shift 2 ;;
            --isolation) _need_value "$@"; kv+=("K2_CJ_isolation=${2:-}"); shift 2 ;;
            --path) _need_value "$@"; kv+=("K2_CJ_path=${2:-}"); shift 2 ;;
            --risk) _need_value "$@"; kv+=("K2_CJ_risk=${2:-}"); shift 2 ;;
            --db-private) _need_value "$@"; kv+=("K2_CJ_dbPrivate=${2:-}"); shift 2 ;;
            --db-raw) _need_value "$@"; kv+=("K2_CJ_dbRaw=${2:-}"); shift 2 ;;
            --io) kv+=("K2_CJ_io=${2:-}"); case "${2:-}" in default|none) ;; *) echo "--io is default or none" >&2; exit 2 ;; esac; shift 2 ;;
            --io-read) _need_value "$@"; io_r="${2:-}"; shift 2 ;;
            --io-write) _need_value "$@"; io_w="${2:-}"; shift 2 ;;
            --io-read-iops) _need_value "$@"; io_ri="${2:-}"; shift 2 ;;
            --io-write-iops) _need_value "$@"; io_wi="${2:-}"; shift 2 ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_rt_usage; return 0 ;;
            -*) echo "Unknown flag: $1 (k2 app --help)" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    [ -n "$egress_add" ] && kv+=("K2_CJ_egressAdd=$egress_add")
    [ -n "$egress_rm" ] && kv+=("K2_CJ_egressRm=$egress_rm")
    [ -n "$port_add" ] && kv+=("K2_CJ_portAdd=$port_add")
    [ -n "$port_rm" ] && kv+=("K2_CJ_portRm=$port_rm")
    if [ -n "$io_r$io_w$io_ri$io_wi" ]; then
        local io_json
        io_json="$(K2_R="$io_r" K2_W="$io_w" K2_RI="$io_ri" K2_WI="$io_wi" python3 -c '
import json, os
o = {}
for k, e in (("rbps", "K2_R"), ("wbps", "K2_W"), ("riops", "K2_RI"), ("wiops", "K2_WI")):
    if os.environ.get(e):
        o[k] = os.environ[e]
print(json.dumps(o))')"
        kv+=("K2_CJ_io_j=$io_json")
    fi
    { [ -z "$app" ] || [ "${#kv[@]}" = 0 ]; } && { echo "usage: k2 app config <app-id> --memory 8G|512M|none [--egress add HOST:PORT] … (k2 app --help)" >&2; exit 2; }
    _app_rt_post /cli/app/config "$(_compute_json K2_CJ_app="$app" "${kv[@]}")"
    _app_grace_done "$json_out"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
a = v["app"]
for line in v.get("applied") or []:
    print("%s: %s" % (a["appId"], line))
m = a.get("memoryLimitBytes")
print("%s: memory limit %s" % (a["appId"], "none (the App may use any memory outside the K2 reserve)" if m is None else "%d MiB" % (m >> 20)))
io = v.get("io")
if io:
    f = lambda n, iops: "none" if n is None else (str(n) if iops else "%d MiB/s" % (n >> 20))
    print("disk ceilings: read %s, write %s, read IOPS %s, write IOPS %s" % (f(io.get("rbps"), 0), f(io.get("wbps"), 0), f(io.get("riops"), 1), f(io.get("wiops"), 1)))
print(v.get("note", ""))'
}

# k2 app list [--workspace WS] [--box] [--json]
cmd_app_list() {
    local ws="" json_out=0 box=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --workspace) _need_value "$@"; ws="${2:-}"; shift 2 ;;
            --box) box=1; shift ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_rt_usage; return 0 ;;
            *) echo "Unknown argument: $1" >&2; exit 2 ;;
        esac
    done
    local q=""
    [ -n "$ws" ] && q="workspace=$(urlencode "$ws")"
    [ "$box" = 1 ] && q="box=1"
    _compute_call GET /cli/app/list "$q"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    if [ "$box" = 1 ]; then
        printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
mib = lambda b: "-" if b is None else "%d MiB" % (b >> 20)
m = v.get("memory", {})
pool = m.get("pool", {}).get("poolBytes")
print("App memory on this box: %s used of %s (%s free)" % (mib(m.get("usedBytes")), mib(pool) if pool and pool < 1 << 60 else "unknown", mib(m.get("freeBytes")) if pool and pool < 1 << 60 else "-"))
for a in v.get("apps", []):
    who = a.get("maintainer") or {}
    by = ("%s (%s)" % (who.get("agent"), who.get("address"))) if who.get("kind") == "agent" else "parked: held by the owner"
    print("%-20s %-12s %s" % (a["appId"], a["state"], by))
    extra = ["memory " + mib(a["memory"].get("limitBytes"))]
    if a.get("disk", {}).get("budgetBytes") is not None:
        extra.append("disk " + mib(a["disk"]["budgetBytes"]))
    if a.get("hostnames"):
        extra.append("hosts " + ", ".join(a["hostnames"]))
    if a.get("ports"):
        extra.append("ports " + ", ".join("%s/%s" % (p["port"], p["proto"]) for p in a["ports"]))
    if a.get("boxServices"):
        extra.append("box services " + ", ".join(a["boxServices"]))
    print("%-20s %s" % ("", "  ".join(extra)))
'
        return 0
    fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
apps = v.get("apps", [])
legacy = v.get("legacy") or []
if not apps and not legacy:
    print("No Apps on this box. Start with: k2 study app-deploys")
for a in apps:
    # 0.46.4: the engine column goes last, so the columns before it keep
    # their place; a daemon before 0.46.4 sends no engine block.
    e = a.get("engine")
    eng = ""
    if e is not None:
        mark = ""
        if e.get("rehostDue"):
            w = e.get("rehost")
            mark = " [re-host %s]" % (w if w in ("running", "deferred", "failed") else "due")
        eng = "  engine %s%s" % (e.get("label") or "-", mark)
    print("%-20s %-12s %-14s seed %s  %s%s" % (a["appId"], a["state"], a.get("workspaceHandle") or "",
          (a.get("liveSeed") or "-")[:12], a.get("url") or ("127.0.0.1:%s" % a.get("frontPort")), eng))
    print("%-20s %s" % ("", a.get("label", "")))
# 0.46.3: --cmd services no wall runs.
for l in legacy:
    moved = ("  (its label now serves App %s)" % l["labelMovedTo"]) if l.get("labelMovedTo") else ""
    print("%-20s %-12s %-14s %s%s" % (l.get("name"), l.get("desired", ""), l.get("workspaceHandle") or "", l.get("label", ""), moved))
    print("%-20s how: %s" % ("", l.get("hint", "")))
for n in v.get("notices", []):
    print("note: " + n.get("hint", ""))
'
}

# k2 app inspect <app-id> [--json]
cmd_app_inspect() {
    local app="" json_out=0 res=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            --resources) res=1; shift ;;
            -h|--help) _app_rt_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    [ -z "$app" ] && { echo "usage: k2 app inspect <app-id> [--resources] [--json]" >&2; exit 2; }
    if [ "$res" = 1 ]; then
        # The App's row of the box usage view (k2 app resources).
        _compute_call GET /cli/app/resources "app=$(urlencode "$app")"
        if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
        printf '%s' "$_CP_BODY" | _app_resources_print
        return 0
    fi
    _compute_call GET /cli/app/inspect "app=$(urlencode "$app")"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
a, rt, s = v["app"], v["runtime"], v.get("liveSeed") or {}
print("%s  %s  (%s)" % (a["appId"], a["state"], a["label"]))
if a.get("stateReason"):
    print("  why       %s" % a["stateReason"])
print("  workspace %s" % a.get("workspaceHandle"))
print("  url       %s" % (a.get("url") or "local only: http://127.0.0.1:%s" % a.get("frontPort")))
if a.get("frontPort"):
    print("  front     127.0.0.1:%s (stable across deploys; your own web server proxies here)" % a["frontPort"])
print("  memory   %s" % ("no per-App limit" if a.get("memoryLimitBytes") is None else "%d MiB" % (a["memoryLimitBytes"] >> 20)))
live = rt.get("live")
print("  live      %s" % ("%s on :%s (pid %s)" % (live["deployment"], live["port"], live.get("pid")) if live else "-"))
warm = rt.get("warm")
if warm:
    print("  warm      %s on :%s (watch window: %ss left)" % (warm["deployment"], warm["port"], rt.get("watchUntilSecs")))
if s:
    print("  seed      %s  %s %s  signer %s" % (s["hash"][:12], s["seedId"], s.get("version") or "", s["signer"]))
    cw = s.get("createdWith") or {}
    print("  created   K2 %s, catalog %s, %s" % (cw.get("k2"), cw.get("catalog"), cw.get("checker")))
    if s.get("sourceRef"):
        print("  commit    %s" % s["sourceRef"])
d = v.get("lastDeployment")
if d:
    print("  last      %s %s by %s: %s%s" % (d["kind"], d["id"], d["actor"], d["result"], (" — " + d["error"]) if d.get("error") else ""))
au = v.get("audit", {})
print("  audit     %s" % ("chain verifies (%s rows)" % au.get("rows") if au.get("verified") else "CHAIN BROKEN at row %s: %s" % (au.get("brokenAt"), au.get("reason"))))
'
}

# k2 app history <app-id> [--limit N] [--json]   |   k2 app history --widget <name>
cmd_app_history() {
    local app="" widget="" limit="" json_out=0 users=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --widget) _need_value "$@"; widget="${2:-}"; shift 2 ;;
            --limit) _need_value "$@"; limit="${2:-}"; shift 2 ;;
            --users) users=1; shift ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_rt_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    local q
    if [ -n "$widget" ]; then q="widget=$(urlencode "$widget")"; else
        [ -z "$app" ] && { echo "usage: k2 app history <app-id> [--users] [--limit N]  |  k2 app history --widget <name>" >&2; exit 2; }
        q="app=$(urlencode "$app")"
    fi
    [ -n "$limit" ] && q="${q}&limit=$(urlencode "$limit")"
    [ "$users" = 1 ] && q="${q}&users=1"
    _compute_call GET /cli/app/history "$q"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys, time
v = json.load(sys.stdin)
def t(x):
    return time.strftime("%Y-%m-%d %H:%M:%S", time.localtime(x)) if x else "-"
if v.get("users"):
    for r in v.get("audit", []):
        print("%s  %-26s %-20s %s" % (t(r["at"]), r["event"], r["actor"], json.dumps(r["payload"], sort_keys=True)))
    if not v.get("audit"):
        print("Nothing about this App\u0027s people yet.")
    sys.exit(0)
if "widget" in v:
    for r in v.get("audit", []):
        print("%s  %-10s %-12s %s" % (t(r["at"]), r["event"], r["actor"], json.dumps(r["payload"], sort_keys=True)))
    if not v.get("audit"):
        print("No deploy-audit rows for widget %s yet." % v["widget"])
    sys.exit(0)
for d in v.get("deployments", []):
    live = "  <- live" if d.get("toSeed") == v.get("liveSeed") and d["result"] == "live" else ""
    if d.get("rollbackable"):
        live += "  can roll back (freed soon)" if d.get("freedSoon") else "  can roll back"
    elif d.get("crossesMigration"):
        live += "  across a data migration: refused"
    elif d.get("liveAt") and d.get("toSeed") and d.get("seedStored") is False:
        live += "  Seed freed"
    print("%s  %-13s %-11s %s -> %s  by %-18s commit %s%s" % (t(d["startedAt"]), d["kind"], d["result"],
          (d.get("fromSeed") or "-")[:12], (d.get("toSeed") or "-")[:12], d["actor"], (d.get("commitRef") or "-")[:12], live))
    if d.get("reason"):
        print("    why: %s" % d["reason"])
    if d.get("error"):
        print("    error: %s" % d["error"])
if "keepDeployments" in v:
    to = v.get("rollbackTo") or []
    if to:
        print("roll back to: %s (k2 app rollback %s --to <seed>)" % (", ".join("%s (%s)" % (o["seed"][:12], t(o["startedAt"])) for o in to), v["app"]))
    else:
        print("roll back: nothing K2 keeps can be rolled back to")
    print("K2 keeps the Seeds of the last %d deployments of each App and frees older ones %d days after they last ran." % (v["keepDeployments"], v["keepUnusedDays"]))
print("audit chain: %s" % ("verifies" if v.get("auditVerified") else "BROKEN (k2 app inspect)"))
'
}

# k2 app logs <app-id> [--deployment D] [--tail N] [--follow] — framed as untrusted.
# k2 app logs <app> --client [--user U] [--kind crash|log] [--since 1d] [--limit N] [--json]
cmd_app_logs() {
    local app="" dep="" tail=200 follow=0 client=0 cuser="" ckind="" csince="" climit="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --deployment) _need_value "$@"; dep="${2:-}"; shift 2 ;;
            --tail) _need_value "$@"; tail="${2:-}"; shift 2 ;;
            --follow|-f) follow=1; shift ;;
            --client) client=1; shift ;;
            --user) _need_value "$@"; cuser="${2:-}"; shift 2 ;;
            --kind) _need_value "$@"; ckind="${2:-}"; shift 2 ;;
            --since) _need_value "$@"; csince="${2:-}"; shift 2 ;;
            --limit) _need_value "$@"; climit="${2:-}"; shift 2 ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_rt_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    if [ "$client" = 1 ]; then
        _app_client_logs "$app" "$cuser" "$ckind" "$csince" "$climit" "$json_out"
        return $?
    fi
    if [ -n "$cuser$ckind$csince$climit" ] || [ "$json_out" = 1 ]; then
        echo "usage: --user, --kind, --since, --limit and --json go with --client (k2 app logs <app> --client)" >&2; exit 2
    fi
    [ -z "$app" ] && { echo "usage: k2 app logs <app-id> [--deployment D] [--tail N] [--follow]  |  k2 app logs <app> --client" >&2; exit 2; }
    local q="app=$(urlencode "$app")&tail=$(urlencode "$tail")"
    [ -n "$dep" ] && q="${q}&deployment=$(urlencode "$dep")"
    _compute_call GET /cli/app/logs "$q"
    printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin)["framed"])'
    [ "$follow" = 1 ] || return 0
    # Poll: print only what is new since the last answer.
    local last="$_CP_BODY"
    while sleep 2; do
        _compute_call GET /cli/app/logs "$q"
        K2_PREV="$last" python3 -c '
import json, os, sys
old = json.loads(os.environ["K2_PREV"])["framed"].splitlines()[2:-1]
new = json.loads(sys.stdin.read())["framed"].splitlines()[2:-1]
i = 0
if old:
    for k in range(len(new), 0, -1):
        if new[:k] == old[-k:]:
            i = k
            break
for l in new[i:]:
    print(l)
' <<<"$_CP_BODY"
        last="$_CP_BODY"
    done
}

# k2 app logs <app> --client: the crash and log bundles the App's people's
# devices sent (diagnostics.send, 0.46.9), framed as untrusted.
_app_client_logs() {
    local app="$1" user="$2" kind="$3" since="$4" limit="$5" json_out="$6"
    [ -z "$app" ] && { echo "usage: k2 app logs <app> --client [--user U] [--kind crash|log] [--since 1d|12h|30m] [--limit N] [--json]" >&2; exit 2; }
    case "$kind" in ""|crash|log) ;; *) echo "usage: --kind takes crash or log" >&2; exit 2 ;; esac
    if [ -n "$limit" ] && ! [[ "$limit" =~ ^[0-9]+$ ]]; then echo "usage: --limit takes a number (1 to 500)" >&2; exit 2; fi
    local q="app=$(urlencode "$app")" from=""
    if [ -n "$since" ]; then
        local secs
        if [[ "$since" =~ ^([0-9]+)d$ ]]; then secs=$(( 10#${BASH_REMATCH[1]} * 86400 )); else secs="$(_compute_secs "$since")"; fi
        from=$(( $(date +%s) - secs ))
        q="${q}&since=${from}"
    fi
    [ -n "$user" ] && q="${q}&user=$(urlencode "$user")"
    [ -n "$kind" ] && q="${q}&kind=${kind}"
    [ -n "$limit" ] && q="${q}&limit=${limit}"
    _compute_call GET /cli/app/client-logs "$q"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
if not v.get("count"):
    print("No client log bundles for %s (kept %s days)." % (v.get("app"), v.get("keepDays")))
    sys.exit(0)
print("%s bundle(s) for %s, oldest first (kept %s days; --limit for more):" % (v["count"], v["app"], v.get("keepDays")))
print(v["framed"])'
}

# k2 app errors <app-id> [--since 1h|30m|2d|300] [--kind crash|health|build|deploy] [--json]
# The App's failure history (crashes, failed health gates, deploys that
# couldn't start, failed builds), then failed deploys and error-looking log
# lines, framed as untrusted.
cmd_app_errors() {
    local app="" since=3600 kind="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --since) _need_value "$@"; since="${2:-}"; shift 2 ;;
            --kind)
                _need_value "$@"
                kind="${2:-}"; shift 2
                case "$kind" in
                    crash|health|build|deploy) ;;
                    *) echo "usage: --kind crash|health|build|deploy" >&2; exit 2 ;;
                esac ;;
            --json) json_out=1; shift ;;
            --follow) echo "k2 app errors --follow arrives with the runtime error pipe; loop on k2 app errors for now" >&2; exit 2 ;;
            -h|--help) _app_rt_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    [ -z "$app" ] && { echo "usage: k2 app errors <app-id> [--since 1h] [--kind crash|health|build|deploy] [--json]" >&2; exit 2; }
    local q="app=$(urlencode "$app")&since=$(urlencode "$since")"
    [ -n "$kind" ] && q="${q}&kind=$(urlencode "$kind")"
    _compute_call GET /cli/app/errors "$q"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys, time
v = json.load(sys.stdin)
fs = v.get("failures", [])
print("Failures (newest first):" if fs else "No crashes, failed health gates or failed builds in the window.")
for f in fs:
    how = []
    if f.get("exitCode") is not None:
        how.append("exit %s" % f["exitCode"])
    if f.get("signal") is not None:
        how.append("signal %s" % f["signal"])
    if f.get("restartCount"):
        how.append("restart %s" % f["restartCount"])
    if f.get("rolledBack"):
        how.append("rolled back")
    print("%s  %-6s %s%s: %s" % (time.strftime("%m-%d %H:%M:%S", time.localtime(f["atMs"] / 1000)), f["kind"], f.get("deployment") or "-",
          (" (" + ", ".join(how) + ")") if how else "", f["reason"]))
if v.get("kind"):
    sys.exit(0)
print()
for e in v.get("events", []):
    print("%s  %s %s: %s — %s" % (time.strftime("%H:%M:%S", time.localtime(e["at"])), e["kind"], e["deployment"], e["result"], e.get("error") or ""))
if not v.get("events"):
    print("No failed, rolled-back or stalled deploys in the window.")
print(v["framed"])
'
}

# k2 app uninstall <app-id> [--purge] [--drop-access] [--json]
# 0.46.7: an App that still has people's logins, grants, shared secrets or
# chat Spaces is refused (exit 3, uninstall_drops_access, with the counts)
# unless --drop-access; --purge (the owner in person) is already explicit.
cmd_app_uninstall() {
    local app="" purge=0 drop=0 json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --purge) purge=1; shift ;;
            --drop-access) drop=1; shift ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_uninstall_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    [ -z "$app" ] && { echo "usage: k2 app uninstall <app-id> [--purge] [--drop-access]" >&2; exit 2; }
    local tok=""
    if [ "$purge" = 1 ]; then
        # Purge deletes the App's data: the owner in person, every time
        # (vault action app-purge bound to the App; 0.46.3).
        echo "This deletes App $app and its data folder (~/.k2/apps/$app). It can't be undone." >&2
        tok="$(_vault_confirm_token app-purge "$app" "Delete App $app and its data")" || exit 3
    fi
    _app_rt_post /cli/app/uninstall "$(_compute_json K2_CJ_app="$app" K2_CJ_purge_b="$purge" K2_CJ_dropAccess_b="$drop" K2_CJ_confirmToken="$tok")"
    [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    if [ "$purge" = 1 ]; then echo "App $app uninstalled; its data folder is gone."; else
        echo "App $app uninstalled. Its data folder stays (~/.k2/apps/$app); its Seeds go with the nightly sweep."; fi
}

_app_uninstall_usage() {
    cat <<'EOF'
%include cli/text/help/app/uninstall.txt
EOF
}

# k2 app expose <app-id> --tunnel [--transfer] | --local [--json]
# (App-governed Spaces S7): turn an installed App's public URL on or off
# with no reinstall. POST /cli/app/expose {app, tunnel, transfer?}.
cmd_app_expose() {
    local app="" mode="" transfer=0 json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --tunnel) [ "$mode" = "local" ] && { echo "usage: --tunnel or --local, not both" >&2; exit 2; }; mode="tunnel"; shift ;;
            --local) [ "$mode" = "tunnel" ] && { echo "usage: --tunnel or --local, not both" >&2; exit 2; }; mode="local"; shift ;;
            --transfer) transfer=1; shift ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_expose_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) [ -n "$app" ] && { echo "usage: k2 app expose <app-id> --tunnel|--local (one App)" >&2; exit 2; }; app="$1"; shift ;;
        esac
    done
    if [ -z "$app" ] || [ -z "$mode" ]; then
        echo "usage: k2 app expose <app-id> --tunnel [--transfer] | --local" >&2; exit 2
    fi
    if [ "$transfer" = 1 ] && [ "$mode" != "tunnel" ]; then
        echo "usage: --transfer goes with --tunnel" >&2; exit 2
    fi
    local tunnel_b=0
    [ "$mode" = "tunnel" ] && tunnel_b=1
    _app_rt_post /cli/app/expose "$(_compute_json K2_CJ_app="$app" K2_CJ_tunnel_b="$tunnel_b" K2_CJ_transfer_b="$transfer")"
    _app_grace_done "$json_out"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
if v.get("changed"):
    if v.get("expose") == "tunnel":
        print("App %s now has its public URL%s" % (v.get("app"), (": " + v["url"]) if v.get("url") else ""))
    else:
        print("App %s is local only now (no public URL)" % v.get("app"))
if v.get("hostnameLeft"):
    print(v["hostnameLeft"])
print(v.get("next", ""))'
}

_app_expose_usage() {
    cat <<'EOF'
%include cli/text/help/app/expose.txt
EOF
}

# POST <route> with the _compute_json pairs that follow; when the daemon
# asks for the owner in person (403 vault_confirm_required naming the
# action and the id it binds), confirm once and POST again with the token.
# A refusal before that (409 handoff_needs_v1, owner_only) asks nothing.
_app_post_confirming() {
    local route="$1" label="$2"; shift 2
    _app_rt_post "$route" "$(_compute_json "$@")"
    [ "$_CP_STATUS" = "403" ] || return 0
    local ask action sid tok
    ask="$(printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
try:
    e = json.load(sys.stdin).get("error") or {}
except Exception:
    e = {}
if isinstance(e, dict) and e.get("code") == "vault_confirm_required" and e.get("action") and e.get("secretId"):
    print(e["action"]); print(e["secretId"])
' 2>/dev/null)"
    [ -n "$ask" ] || return 0
    action="$(printf '%s\n' "$ask" | sed -n 1p)"
    sid="$(printf '%s\n' "$ask" | sed -n 2p)"
    tok="$(_vault_confirm_token "$action" "$sid" "$label")" || exit 3
    _app_rt_post "$route" "$(_compute_json "$@" K2_CJ_confirmToken="$tok")"
}

# k2 app handover — one App's maintenance from one agent to another
# (prd-app-handoff-v1; Rosson 2026-10-26: one verb, improved).
#   k2 app handover <app> --to <ws> [--brief FILE] [--db share|wait]
#        [--collisions keep|use_b] [--reset-shared] [--copy-source]
#        [--reason T] [--dry-run] [--json]
#   k2 app handover accept|decline|cancel|undo|finish|rekey <app> [--reason T] [--json]
#   k2 app handover status [<app>] [--detail] [--json]
#   k2 app handover brief <app>
#   k2 app unpark <app> --to <ws> [...]   (a parked App: the same card)
# Exit: 0 done, 2 held (an offer, or a card for the owner), 3 refused, 1 failed.
_app_handover_usage() {
    cat >&2 <<'USAGE'
%include cli/text/help/app/handover.txt
USAGE
}

_app_handover_print() {
    # stdin: a handover answer (plan, held card, offer or row).
    python3 -c '
import json, sys
v = json.load(sys.stdin)
plan = v.get("plan") if isinstance(v.get("plan"), dict) else (v if "lines" in v else None)
h = v.get("handover") or {}
if v.get("already"):
    print("Already: %s" % (v.get("workspace") or h.get("toHandle") or "this workspace"))
if plan and v.get("status") != "held":
    for b in plan.get("blockers") or []:
        print("blocked (%s): %s" % (b.get("code"), b.get("hint")))
if plan:
    for l in plan.get("lines") or []:
        print("  - " + l)
if h:
    print("handover %s: %s (%s -> %s)" % (h.get("id"), h.get("state"), h.get("fromHandle") or "parked", h.get("toHandle")))
r = v.get("request")
if isinstance(r, dict) and r.get("id"):
    print("card: %s" % r["id"])
if v.get("hint"):
    print(v["hint"])
'
}

cmd_app_handover() {
    local verb="$1"; shift
    local sub=""
    if [ "$verb" = handover ]; then
        case "${1:-}" in
            accept|decline|cancel|status|brief|undo|finish|rekey|plan) sub="$1"; shift ;;
            -h|--help|"") _app_handover_usage; [ -z "${1:-}" ] && exit 2; return 0 ;;
        esac
    fi
    local app="" to="" brief_file="" db="" coll="" reset_shared="" copy_source="" reason="" dry=0 json_out=0 detail=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --to) _need_value "$@"; to="${2:-}"; shift 2 ;;
            --detail) detail=1; shift ;;
            --brief) _need_value "$@"; brief_file="${2:-}"; shift 2 ;;
            --db) _need_value "$@"; db="${2:-}"; shift 2 ;;
            --collisions) _need_value "$@"; coll="${2:-}"; shift 2 ;;
            --reset-shared) reset_shared=1; shift ;;
            --copy-source) copy_source=1; shift ;;
            --reason) _need_value "$@"; reason="${2:-}"; shift 2 ;;
            --dry-run) dry=1; shift ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_handover_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    local brief=""
    if [ -n "$brief_file" ]; then
        [ -f "$brief_file" ] || { echo "{\"error\":\"bad_request\",\"hint\":\"no brief file $brief_file\"}" >&2; exit 2; }
        brief="$(cat "$brief_file")"
    fi
    case "$sub" in
        status)
            local q=""
            [ -n "$app" ] && q="app=$(urlencode "$app")"
            _compute_call GET /cli/app/handover/status "$q"
            [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
            if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
            printf '%s' "$_CP_BODY" | K2_HANDOVER_DETAIL="$detail" python3 -c '
import json, os, sys
rows = json.load(sys.stdin).get("handovers") or []
detail = os.environ.get("K2_HANDOVER_DETAIL") == "1"
if not rows:
    print("No handovers."); sys.exit(0)
for h in rows:
    extra = ""
    if h.get("inWindow"):
        extra = " (undo until %s)" % h.get("windowUntil")
    if h.get("rekeyState"):
        extra += " re-key: %s" % h["rekeyState"]
    print("%s  %-9s %s  %s -> %s%s" % (h["id"], h["state"], h["appId"], h.get("fromHandle") or "parked", h.get("toHandle"), extra))
    if detail:
        print("  card: %s" % (h.get("requestId") or "none yet (the card is filed once the new agent accepts)"))
        plan = h.get("plan") if isinstance(h.get("plan"), dict) else {}
        lines = plan.get("lines") or []
        if lines:
            print("  what moves (the card'"'"'s own words, to the owner):")
            for l in lines:
                print("    - " + l)
        else:
            print("  what moves: not worked out yet (k2 app handover %s --to %s --dry-run)" % (h["appId"], h.get("toHandle")))'
            return 0 ;;
        brief)
            [ -n "$app" ] || { _app_handover_usage; exit 2; }
            _compute_call GET /cli/app/handover/brief "app=$(urlencode "$app")"
            [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
            if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print(v.get("brief") or "(no brief)")'
            return 0 ;;
        accept|decline|cancel|undo|finish|rekey)
            [ -n "$app" ] || { _app_handover_usage; exit 2; }
            _app_rt_post "/cli/app/handover/$sub" "$(_compute_json K2_CJ_app="$app" K2_CJ_reason="$reason")"
            case "$_CP_STATUS" in 200|202) ;; *) _compute_fail "$_CP_STATUS" "$_CP_BODY" ;; esac
            if [ "$json_out" = 1 ]; then
                printf '%s\n' "$_CP_BODY"
                [ "$_CP_STATUS" = "202" ] && return 2
                return 0
            fi
            printf '%s' "$_CP_BODY" | _app_handover_print
            if [ "$_CP_STATUS" = "202" ]; then
                if [ "$sub" = undo ] && ! _compute_in_session && [ -t 0 ] && [ "$(_envn NO_APPROVE)" != "1" ]; then
                    _app_approve "$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin)["request"]["id"])')"
                    return 0
                fi
                return 2
            fi
            return 0 ;;
    esac
    if [ -z "$app" ] || [ -z "$to" ]; then _app_handover_usage; exit 2; fi
    if [ "$dry" = 1 ] || [ "$sub" = plan ]; then
        local q="app=$(urlencode "$app")&to=$(urlencode "$to")"
        [ -n "$db" ] && q="$q&db=$(urlencode "$db")"
        [ -n "$coll" ] && q="$q&collisions=$(urlencode "$coll")"
        [ -n "$reset_shared" ] && q="$q&resetShared=1"
        [ -n "$copy_source" ] && q="$q&copySource=1"
        _compute_call GET /cli/app/handover/plan "$q"
        [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
        if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
        printf '%s' "$_CP_BODY" | _app_handover_print
        printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("ready" if v.get("ready") else "not ready: see blocked lines")'
        return 0
    fi
    # K2_APP_HANDOVER=legacy answers 403 naming a confirm: asked once here.
    _app_post_confirming "/cli/app/$verb" "Give App $app to $to" K2_CJ_app="$app" K2_CJ_to="$to" K2_CJ_brief="$brief" K2_CJ_db="$db" \
        K2_CJ_collisions="$coll" K2_CJ_resetShared_b="$reset_shared" K2_CJ_copySource_b="$copy_source" K2_CJ_reason="$reason"
    case "$_CP_STATUS" in 200|202) ;; *) _compute_fail "$_CP_STATUS" "$_CP_BODY" ;; esac
    if [ "$json_out" = 1 ]; then
        printf '%s\n' "$_CP_BODY"
        [ "$_CP_STATUS" = "202" ] && return 2
        return 0
    fi
    if [ "$_CP_STATUS" = "200" ]; then
        # K2_APP_HANDOVER=legacy (or already there).
        printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("App %s now belongs to %s." % (v.get("app"), v.get("workspace")))'
        return 0
    fi
    printf '%s' "$_CP_BODY" | _app_handover_print
    if [ -z "$brief_file" ] && _compute_in_session; then
        echo "tip: give the next agent a brief: k2 app handover $app --to $to --brief NOTES.md (once this offer is cancelled)" >&2
    fi
    local card
    card="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; r=json.load(sys.stdin).get("request"); print(r["id"] if isinstance(r, dict) else "")')"
    if [ -n "$card" ] && ! _compute_in_session && [ -t 0 ] && [ "$(_envn NO_APPROVE)" != "1" ]; then
        # The owner at a terminal outside K2: approve the card right here.
        _app_approve "$card"
        return 0
    fi
    return 2
}

# k2 app park <app-id> [--reason T] [--json] (prd-agent-app-permissions-v1
# Q8: people only, never a passport). handover/unpark: cmd_app_handover.
cmd_app_ownership() {
    local verb="$1"; shift
    local app="" to="" reason="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --to) _need_value "$@"; to="${2:-}"; shift 2 ;;
            --reason) _need_value "$@"; reason="${2:-}"; shift 2 ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_rt_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    if [ -z "$app" ] || { [ "$verb" != park ] && [ -z "$to" ]; }; then
        if [ "$verb" = park ]; then echo "usage: k2 app park <app-id> [--reason TEXT]" >&2
        else echo "usage: k2 app $verb <app-id> --to <workspace>" >&2; fi
        exit 2
    fi
    if [ "$verb" = park ]; then
        _app_rt_post "/cli/app/$verb" "$(_compute_json K2_CJ_app="$app" K2_CJ_reason="$reason")"
    else
        _app_post_confirming "/cli/app/$verb" "Give App $app to $to" K2_CJ_app="$app" K2_CJ_to="$to"
    fi
    [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    case "$verb" in
        park) echo "App $app is parked: stopped and held for the owner (k2 app unpark $app --to <workspace>)." ;;
        *) echo "App $app now belongs to $(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("workspace",""))')." ;;
    esac
}

# k2 app release <workspace> --apps handover:<ws>|uninstall|park [--json]
cmd_app_release() {
    local ws="" apps="" drop=0 json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --apps) _need_value "$@"; apps="${2:-}"; shift 2 ;;
            --drop-access) drop=1; shift ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_rt_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) ws="$1"; shift ;;
        esac
    done
    if [ -z "$ws" ] || [ -z "$apps" ]; then
        echo "usage: k2 app release <workspace> --apps handover:<workspace>|uninstall|park [--drop-access]" >&2
        exit 2
    fi
    if [ "$drop" = 1 ] && [ "$apps" != "uninstall" ]; then
        echo "usage: --drop-access goes with --apps uninstall" >&2
        exit 2
    fi
    _app_post_confirming /cli/app/release "Give $ws's Apps to ${apps#handover:}" K2_CJ_workspace="$ws" K2_CJ_apps="$apps" K2_CJ_dropAccess_b="$drop"
    case "$_CP_STATUS" in 200|202) ;; *) _compute_fail "$_CP_STATUS" "$_CP_BODY" ;; esac
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; [ "$_CP_STATUS" = "202" ] && return 2; return 0; fi
    if [ "$_CP_STATUS" = "202" ]; then
        # One handover card per App; the owner at a terminal outside K2
        # approves each here.
        local cards
        cards="$(printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
for r in v.get("results", []):
    if r.get("request"):
        print(r["request"])')"
        printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print(v.get("hint",""))'
        if ! _compute_in_session && [ -t 0 ] && [ "$(_envn NO_APPROVE)" != "1" ]; then
            local c
            for c in $cards; do _app_approve "$c"; done
            return 0
        fi
        return 2
    fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
r = v.get("results", [])
print("%s: %d App%s released (%s)." % (v.get("workspace"), len(r), "" if len(r) == 1 else "s", v.get("way")))
for x in r:
    print("  " + x["app"])
'
}

# POST a JSON body from FILE (too big for argv or a curl config line: an
# exec's stdin). Same transports as _compute_send; sets _CP_STATUS/_CP_BODY.
#
# 0.46.15: the send-once rule of _compute_send. The POST goes again over
# TCP after the cell socket only when the socket took none of it: the
# connect failed (curl 7) or the daemon answered "not served on cell
# socket". Any other failure may have run the command, so it is not sent
# again: _cli_send_unanswered says so and ends the command with 1.
_app_rt_post_file() {
    local path="$1" file="$2" sock out="" _K2_CFG="" rc=0 t0="$SECONDS"
    path="$(_compute_assign_path POST "$path")"
    sock="$(_hook_sock)"
    if [ -n "$sock" ] && [ -S "$sock" ] && ! _cli_skip_uds && [ -n "${SCOPED_TOKEN:-}" ]; then
        _k2_cfg_add header "Authorization: Bearer ${SCOPED_TOKEN}"
        _k2_cfg_add data-binary "@$file"
        out="$(_k2_curl "$_K2_CFG" -s -X POST --unix-socket "$sock" "http://localhost${path}" \
            --connect-timeout 5 --max-time "${K2_CP_MAX_TIME:-120}" -H "Content-Type: application/json" \
            -w $'\n%{http_code}' 2>/dev/null)" || rc=$?
        if [ "$rc" -eq 0 ] && [ -n "$out" ] && [[ "$out" != *"not served on cell socket"* ]] && [[ "$out" != *$'\n000' ]]; then
            _compute_split "$out"
            return 0
        fi
        if [ "$rc" -ne 7 ] && [[ "$out" != *"not served on cell socket"* ]]; then
            _cli_send_unanswered "$path" "$t0" 0 ""
            # Soft callers (_K2_NOANSWER_SOFT=1) get status 000 instead.
            _compute_split $'\n000'
            return 0
        fi
    fi
    # TCP: in a session whose cell socket failed, the passport, never the
    # owner token from disk (_k2_sock_fallback_token).
    local _K2_TCP_TOKEN="${TOKEN:-}"
    _k2_sock_fallback_token "$sock" "$out"
    _K2_CFG=""
    _k2_cfg_add url "${BASE_URL}${path}"
    _k2_cfg_auth "$_K2_TCP_TOKEN"
    _k2_cfg_add data-binary "@$file"
    out="$(_k2_curl "$_K2_CFG" -s -X POST --connect-timeout 5 --max-time "${K2_CP_MAX_TIME:-120}" \
        -H "Content-Type: application/json" -w $'\n%{http_code}' 2>/dev/null || printf '\n000')"
    _compute_split "$out"
}

# k2 app exec <app-id> [--timeout 60|10m] [--json] [--no-stdin] -- <command…>   (S28)
# Runs one command inside the App's guarded container, as the App. The
# output is framed as untrusted App text. Exit: the command's own exit code;
# 3 (JSON) when K2 refuses or the App isn't reachable; 124 on timeout.
# 0.46.11: a pipe or a file on stdin is the command's stdin (64 MiB at
# most, exit 2 past it); a terminal, /dev/null or --no-stdin sends none.
cmd_app_exec() {
    local app="" timeout="" json_out=0 no_stdin=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --timeout) timeout="$(_compute_secs "${2:-}")"; shift 2 ;;
            --json) json_out=1; shift ;;
            --no-stdin) no_stdin=1; shift ;;
            -h|--help) _app_rt_usage; return 0 ;;
            --) shift; break ;;
            -*) echo "Unknown flag: $1 (put the command after --)" >&2; exit 2 ;;
            *) if [ -z "$app" ]; then app="$1"; shift; else break; fi ;;
        esac
    done
    { [ -z "$app" ] || [ $# -eq 0 ]; } && { echo "usage: k2 app exec <app-id> [--timeout 60] [--no-stdin] -- <command…>" >&2; exit 2; }
    local argv_json
    argv_json="$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' "$@")"
    if [ "$no_stdin" = 0 ] && [ ! -t 0 ] && { [ -p /dev/stdin ] || [ -f /dev/stdin ]; }; then
        # The body carries the stdin as base64: from a temp file, never argv.
        local tmpd rc=0
        tmpd="$(mktemp -d "${TMPDIR:-/tmp}/k2-exec-XXXXXX")"
        # The copy of stdin goes when the command ends, also when K2 didn't
        # answer and the send ended it.
        _K2_EXEC_TMPD="$tmpd"
        trap '_k2_exit_rc=$?; rm -rf "$_K2_EXEC_TMPD"; _permission_note_flush "$_k2_exit_rc"' EXIT
        python3 -c '
import base64, json, sys
out, app, argv, timeout = sys.argv[1:5]
cap = 64 << 20
data = sys.stdin.buffer.read(cap + 1)
if len(data) > cap:
    sys.stderr.write("k2 app exec: stdin is over 64 MiB, the most one exec carries (split it, or move files in with k2 app data put)\n")
    sys.exit(2)
body = {"app": app, "argv": json.loads(argv), "stdin": base64.b64encode(data).decode("ascii")}
if timeout:
    body["timeout"] = int(timeout)
with open(out, "w") as f:
    json.dump(body, f)
' "$tmpd/body.json" "$app" "$argv_json" "$timeout" || rc=$?
        if [ "$rc" -ne 0 ]; then rm -rf "$tmpd"; exit "$rc"; fi
        K2_CP_MAX_TIME=$(( ${timeout:-60} + 120 )) _app_rt_post_file /cli/app/exec "$tmpd/body.json"
        rm -rf "$tmpd"
        trap _permission_note_flush EXIT
    else
        local body
        body="$(_compute_json K2_CJ_app="$app" K2_CJ_argv_j="$argv_json" K2_CJ_timeout_n="$timeout")"
        K2_CP_MAX_TIME=$(( ${timeout:-60} + 60 )) _app_rt_post /cli/app/exec "$body"
    fi
    [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
    if [ "$json_out" = 1 ]; then
        printf '%s\n' "$_CP_BODY"
    else
        printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print(v["framed"])
if v.get("timedOut"):
    print("[k2] the command was stopped at its timeout", file=sys.stderr)'
    fi
    local code
    code="$(printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
c = v.get("exitCode")
print(124 if v.get("timedOut") else (c if isinstance(c, int) and 0 <= c <= 255 else 1))')"
    return "${code:-1}"
}

# k2 app shell <app-id> [-- <command…>]   (S28; default /bin/sh)
# An interactive terminal in the App's guarded container over a WebSocket.
# Terminal commands in what the App prints (titles, clipboard writes) are
# stripped by K2. A shell idle for 15 minutes is hung up. Piped stdin works
# too (echo ls | k2 app shell jobs).
cmd_app_shell() {
    local app=""
    while [ $# -gt 0 ]; do
        case "$1" in
            -h|--help) _app_rt_usage; return 0 ;;
            --) shift; break ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) if [ -z "$app" ]; then app="$1"; shift; else break; fi ;;
        esac
    done
    [ -z "$app" ] && { echo "usage: k2 app shell <app-id> [-- <command…>]" >&2; exit 2; }
    local argv_json=""
    if [ $# -gt 0 ]; then
        argv_json="$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' "$@")"
    fi
    K2_SH_BASE="$BASE_URL" K2_SH_TOKEN="$TOKEN" K2_SH_APP="$app" K2_SH_ARGV="$argv_json" python3 -c "$_K2_APP_SHELL_PY"
}

# The `k2 app shell` WebSocket client (stdlib only). The token rides the
# Authorization header, never argv or the URL.
_K2_APP_SHELL_PY='
import base64, json, os, select, signal, socket, ssl, struct, sys, urllib.parse

base = os.environ["K2_SH_BASE"]
tok = os.environ["K2_SH_TOKEN"]
app = os.environ["K2_SH_APP"]
argv = os.environ.get("K2_SH_ARGV", "")
u = urllib.parse.urlparse(base)
tty = os.isatty(0)
cols, rows = 80, 24
if tty:
    try:
        cols, rows = os.get_terminal_size(0)
    except OSError:
        pass
q = {"app": app, "cols": str(cols), "rows": str(rows)}
if argv:
    q["argv"] = argv
path = "/cli/app/shell?" + urllib.parse.urlencode(q)
host = u.hostname or "127.0.0.1"
port = u.port or (443 if u.scheme == "https" else 80)
s = socket.create_connection((host, port), timeout=15)
if u.scheme == "https":
    s = ssl.create_default_context().wrap_socket(s, server_hostname=host)
key = base64.b64encode(os.urandom(16)).decode()
req = ("GET %s HTTP/1.1\r\nHost: %s\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
       "Sec-WebSocket-Key: %s\r\nSec-WebSocket-Version: 13\r\nAuthorization: Bearer %s\r\n\r\n") % (path, host, key, tok)
s.sendall(req.encode())
head = b""
while b"\r\n\r\n" not in head:
    c = s.recv(4096)
    if not c:
        break
    head += c
hdr, _, rest = head.partition(b"\r\n\r\n")
status = hdr.split(b"\r\n", 1)[0]
if b" 101 " not in status + b" ":
    body = rest
    try:
        s.settimeout(5)
        while True:
            c = s.recv(65536)
            if not c:
                break
            body += c
    except Exception:
        pass
    try:
        e = json.loads(body.decode() or "{}").get("error") or {}
        print(json.dumps({"error": e.get("code", "error"), "hint": e.get("hint", status.decode())}), file=sys.stderr)
    except Exception:
        print(json.dumps({"error": "shell_refused", "hint": status.decode()}), file=sys.stderr)
    sys.exit(3)
s.settimeout(None)
buf = bytearray(rest)

def send(op, data):
    m = os.urandom(4)
    n = len(data)
    if n < 126:
        h = struct.pack("!BB", 0x80 | op, 0x80 | n)
    elif n < 65536:
        h = struct.pack("!BBH", 0x80 | op, 0x80 | 126, n)
    else:
        h = struct.pack("!BBQ", 0x80 | op, 0x80 | 127, n)
    s.sendall(h + m + bytes(b ^ m[i % 4] for i, b in enumerate(data)))

def frames():
    while True:
        if len(buf) >= 2:
            op = buf[0] & 0x0F
            n = buf[1] & 0x7F
            off = 2
            if n == 126:
                if len(buf) < 4: return
                n = struct.unpack("!H", bytes(buf[2:4]))[0]; off = 4
            elif n == 127:
                if len(buf) < 10: return
                n = struct.unpack("!Q", bytes(buf[2:10]))[0]; off = 10
            if len(buf) >= off + n:
                data = bytes(buf[off:off + n])
                del buf[:off + n]
                yield op, data
                continue
        return

old = None
if tty:
    import termios, tty as ttym
    old = termios.tcgetattr(0)
    ttym.setraw(0)
    def winch(*_):
        try:
            c, r = os.get_terminal_size(0)
            send(1, json.dumps({"type": "resize", "cols": c, "rows": r}).encode())
        except OSError:
            pass
    signal.signal(signal.SIGWINCH, winch)
code = None
stdin_open = True
out = sys.stdout.buffer
err = sys.stderr
def handle():
    global code
    for op, data in frames():
        if op == 2:
            out.write(data); out.flush()
        elif op == 1:
            try:
                v = json.loads(data.decode())
            except Exception:
                continue
            t = v.get("type")
            if t in ("banner", "notice"):
                err.write("\r\n" + v.get("text", "") + "\r\n"); err.flush()
            elif t == "error":
                err.write(json.dumps({"error": v.get("code"), "hint": v.get("hint")}) + "\r\n")
                code = 3
            elif t == "exit":
                code = v.get("code")
        elif op == 8:
            return True
        elif op == 9:
            send(10, data)
    return False

try:
    done = handle()
    while not done:
        rl = [s] + ([0] if stdin_open else [])
        try:
            r, _, _ = select.select(rl, [], [])
        except InterruptedError:
            continue
        if 0 in r:
            d = os.read(0, 4096)
            if d:
                send(2, d)
            else:
                stdin_open = False
                if not tty:
                    send(2, b"\x04")
        if s in r:
            c = s.recv(65536)
            if not c:
                break
            buf += c
            done = handle()
finally:
    if old is not None:
        import termios
        termios.tcsetattr(0, termios.TCSADRAIN, old)
    try:
        send(8, b"")
    except Exception:
        pass
sys.exit(code if isinstance(code, int) and 0 <= code <= 255 else (0 if code is None else 1))
'

# k2 app exception <app-id> on [--device D]... [--reason TEXT] | off
# The process exception (§4.3): the App leaves its guarded container and
# runs as a sandboxed system process (for a device a container can't
# reach). `on` is the server owner's, in person, with an owner confirm;
# agents get owner_only. `off` (back to the guarded container) is anyone
# who may manage the App.
cmd_app_exception() {
    local app="" mode="" reason="" json_out=0 devices=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --device) _need_value "$@"; devices+=("${2:-}"); shift 2 ;;
            --reason) _need_value "$@"; reason="${2:-}"; shift 2 ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_rt_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            on|off) if [ -n "$app" ]; then mode="$1"; else app="$1"; fi; shift ;;
            *) app="$1"; shift ;;
        esac
    done
    { [ -z "$app" ] || [ -z "$mode" ]; } && { echo "usage: k2 app exception <app-id> on [--device dri-render128]... [--reason TEXT] | off" >&2; exit 2; }
    local dev_json="[]"
    if [ "${#devices[@]}" -gt 0 ]; then
        dev_json="$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' "${devices[@]}")"
    fi
    local on_b=0 tok=""
    if [ "$mode" = on ]; then
        on_b=1
        echo "This moves App $app out of its guarded container into the process exception (a sandboxed system process${devices:+ with ${devices[*]}})." >&2
        tok="$(_vault_confirm_token app-process-exception "$app" "process exception for App $app")" || exit 3
    fi
    _app_rt_post /cli/app/exception "$(_compute_json K2_CJ_app="$app" K2_CJ_on_b="$on_b" K2_CJ_devices_j="$dev_json" K2_CJ_reason="$reason" K2_CJ_confirmToken="$tok")"
    [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print("%s: %s%s" % (v["app"], v.get("label", ""), "" if v.get("changed") else " (no change)"))
if v.get("warning"):
    print(v["warning"])
if v.get("next"):
    print(v["next"])'
}

_study_page_agent_templates() {
    cat <<'EOF'
%include cli/text/study/agent-templates.txt
EOF
}

_study_page_organization() {
    cat <<'EOF'
%include cli/text/study/organization.txt
EOF
}

_study_page_app_handover() {
    cat <<'EOF'
%include cli/text/study/app-handover.txt
EOF
}

_study_page_app_chat() {
    cat <<'EOF'
%include cli/text/study/app-chat.txt
EOF
}

_study_page_chat_voice() {
    cat <<'EOF'
%include cli/text/study/chat-voice.txt
EOF
}

_study_page_chat_voice_rooms() {
    cat <<'EOF'
%include cli/text/study/chat-voice-rooms.txt
EOF
}

_study_page_voice_rooms() {
    cat <<'EOF'
%include cli/text/study/voice-rooms.txt
EOF
}

_study_page_app_recording() {
    cat <<'EOF'
%include cli/text/study/app-recording.txt
EOF
}

_study_page_app_move() {
    cat <<'EOF'
%include cli/text/study/app-move.txt
EOF
}

_study_page_app_deploys() {
    cat <<'EOF'
%include cli/text/study/app-deploys.txt
EOF
}


# k2 app sessions --live [--count] [--json] (0.46.16, owner): guests with a
# socket open right now and guests signed in, per App. Counts only.
cmd_app_sessions() {
    local live=0 count=0 json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --live) live=1; shift ;;
            --count) count=1; shift ;;
            --json) json_out=1; shift ;;
            -h|--help)
                cat <<'EOF'
k2 app sessions --live [--count] [--json]   (the server's owner)
  Guests on this server's Apps right now, per App: connected (a page with
  a live socket open) and signed in (a login that hasn't ended). Counts
  only, never names. A K2 restart signs connected guests out of older App
  sites, so read this before one.
  --count   one line: the totals.
  --json    {connected, signedIn, apps:[{app, connected, signedIn}]}
EOF
                return 0 ;;
            *) echo "Unknown argument: $1 (k2 app sessions --live [--count] [--json])" >&2; exit 2 ;;
        esac
    done
    [ "$live" = 1 ] || { echo "usage: k2 app sessions --live [--count] [--json]  (one person's sessions: k2 app user sessions <username> --app <app>)" >&2; exit 2; }
    _compute_call GET /cli/app/sessions/live ""
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | K2_SESS_COUNT="$count" python3 -c '
import json, os, sys
v = json.load(sys.stdin)
c, s = v.get("connected", 0), v.get("signedIn", 0)
print("%d guest%s connected now, %d signed in" % (c, "" if c == 1 else "s", s))
if os.environ.get("K2_SESS_COUNT") == "1":
    sys.exit(0)
for a in v.get("apps") or []:
    print("  %-24s %4d connected  %4d signed in" % (a.get("app"), a.get("connected", 0), a.get("signedIn", 0)))
if c == 0:
    print("Nobody is connected now: a restart signs nobody out.")
'
}
