# ── k2 app sdk (0.46.6): @k2/server (and Sew's App kit) into an App ──
_app_sdk_usage() {
    cat <<'EOF'
%include cli/text/help/app/sdk.txt
EOF
}

cmd_app_sdk() {
    local sub="${1:-}"
    shift || true
    case "$sub" in
        add) _app_sdk_add "$@" ;;
        status) _app_sdk_status "$@" ;;
        kit) _app_sdk_kit "$@" ;;
        ""|-h|--help|help) _app_sdk_usage ;;
        *) echo "usage: k2 app sdk add|status|kit … (k2 app sdk --help)" >&2; exit 2 ;;
    esac
}

# k2 app sdk add <app-folder> [--sew] [--json]
_app_sdk_add() {
    local folder="" sew=0 json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --sew) sew=1; shift ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_sdk_usage; return 0 ;;
            -*) echo "Unknown flag: $1 (k2 app sdk --help)" >&2; exit 2 ;;
            *) [ -z "$folder" ] || { echo "usage: k2 app sdk add <app-folder> [--sew] [--json]" >&2; exit 2; }; folder="$1"; shift ;;
        esac
    done
    [ -n "$folder" ] || { echo "usage: k2 app sdk add <app-folder> [--sew] [--json]" >&2; exit 2; }
    [ -d "$folder" ] || { echo "k2 app sdk add: $folder is not a folder" >&2; exit 2; }
    _app_rt_post /cli/app/sdk/add "$(_compute_json K2_CJ_folder="$(_app_rt_abs "$folder")" K2_CJ_sew_b="$sew")"
    [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, shlex, sys
v = json.load(sys.stdin)
print("Vendored into %s/vendor:" % v["folder"])
w = max(len(x["name"]) + len(x["version"]) for x in v["vendored"]) + 2
for x in v["vendored"]:
    print("  %-*s %s%s" % (w, x["name"] + " " + x["version"], x["file"], "" if x["written"] else "  (already there)"))
for r in v.get("removed") or []:
    print("  removed the older %s" % r)
deps = ", ".join(x["name"] for x in v["vendored"])
line = "package.json: %s (dependencies: %s" % (v["packageJson"], deps)
if v.get("overrides"):
    line += "; overrides: " + ", ".join(v["overrides"])
print(line + ")")
print("Next, in %s:" % v["folder"])
if v.get("install"):
    print("  " + v["install"])
    print("  (the peers and npm-registry dependencies of the Sew packages; it also installs the vendored ones)")
else:
    print("  npm install")
print("  import { fromEnv } from '@k2/server'")
print("Keep vendor/ with the App (not in .k2appignore): its Seed carries it.")
'
}

# k2 app sdk status <app-folder> [--json]
_app_sdk_status() {
    local folder="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            -h|--help) _app_sdk_usage; return 0 ;;
            -*) echo "Unknown flag: $1 (k2 app sdk --help)" >&2; exit 2 ;;
            *) [ -z "$folder" ] || { echo "usage: k2 app sdk status <app-folder> [--json]" >&2; exit 2; }; folder="$1"; shift ;;
        esac
    done
    [ -n "$folder" ] || { echo "usage: k2 app sdk status <app-folder> [--json]" >&2; exit 2; }
    [ -d "$folder" ] || { echo "k2 app sdk status: $folder is not a folder" >&2; exit 2; }
    _compute_call GET /cli/app/sdk/status "folder=$(urlencode "$(_app_rt_abs "$folder")")"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
b = v["box"]
print("This box: @k2/server %s; Sew App kit %s" % (b["k2Server"], b.get("sewKit") or ("none (k2 app sdk kit)" if b.get("sewKitPinned") else "none (Sew %s has no App kit; 0.5.0+)" % b.get("sewPin"))))
for r in v["packages"]:
    print("  %-14s vendored %-10s box %-10s %s" % (r["name"], r.get("vendored") or "-", r.get("onBox") or "-", r["state"]))
if any(r["state"] in ("older", "missing") for r in v["packages"]):
    print("Update: k2 app sdk add %s%s" % (v["folder"], " --sew" if any(r["name"].startswith("@sew/") for r in v["packages"]) else ""))
'
}

# k2 app sdk kit <sew-app-kit-<v>.tar.gz> [--json]
_app_sdk_kit() {
    local path="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            -h|--help) _app_sdk_usage; return 0 ;;
            -*) echo "Unknown flag: $1 (k2 app sdk --help)" >&2; exit 2 ;;
            *) [ -z "$path" ] || { echo "usage: k2 app sdk kit <sew-app-kit-<version>.tar.gz> [--json]" >&2; exit 2; }; path="$1"; shift ;;
        esac
    done
    [ -n "$path" ] || { echo "usage: k2 app sdk kit <sew-app-kit-<version>.tar.gz> [--json]" >&2; exit 2; }
    [ -f "$path" ] || { echo "k2 app sdk kit: $path is not a file" >&2; exit 2; }
    local abs
    abs="$(cd "$(dirname "$path")" && pwd -P)/$(basename "$path")"
    _app_rt_post /cli/app/sdk/kit "$(_compute_json K2_CJ_path="$abs")"
    [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print("Kept Sew App kit %s (sha256 %s): %s" % (v["version"], v["sha256"][:12], ", ".join(p["name"] for p in v["packages"])))
print("Now: k2 app sdk add <app-folder> --sew")
'
}

