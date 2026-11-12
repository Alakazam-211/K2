# ── k2 app sdk (0.46.6): @k2/server (and Sew's App kit) into an App ──
_app_sdk_usage() {
    cat <<'EOF'
k2 app sdk — K2's server SDK (and Sew's) inside your App folder

  k2 app sdk add <app-folder> [--sew] [--json]
        <app-folder> is an existing folder (mkdir it first; a relative
        path is fine). Copies @k2/server (this K2's version) into
        <app-folder>/vendor/ and adds "@k2/server":
        "file:vendor/k2-server-<version>.tgz" to the App's package.json
        (made when there is none: private, "type": "module"). Then, in
        <app-folder>: npm install. Your server code:
          import { fromEnv } from '@k2/server'
        --sew does all that AND copies Sew's packages (this box needs Sew
        0.5.0+): @sew/client (Sew's API), @sew/channel (chat), @sew/voice
        (voice and video calls), @sew/e2ee (call encryption), @sew/bff
        (your server's side), and from Sew 0.7.1 @sew/noise (the mic noise
        filter @sew/voice uses). It pins their links to each other in npm
        "overrides" and prints an npm i line for what comes from the npm
        registry (React, LiveKit, …): run that line instead of npm
        install. Adding --sew later to an App that has @k2/server is
        fine. Run add again after a K2 or Sew update: it replaces the
        older files and never duplicates.
  k2 app sdk status <app-folder> [--json]
        What the folder has in vendor/ against what this box carries:
        current, older (run add again), newer, missing.
  k2 app sdk kit <sew-app-kit-<version>.tar.gz> [--json]
        Keep Sew's App kit on this box for --sew when k2 hostchat
        enable|upgrade could not download it (a box without internet) and
        it wasn't beside --tarball. Only the Sew version this K2 pins,
        checked against its pinned sha256.

Keep vendor/ with your App (commit it; never list it in .k2appignore): the
Seed carries it, so the App installs offline and a rollback keeps its SDK.
K2 never runs npm and never uses the network here; it writes only
vendor/ and package.json inside <app-folder>.

Who: the owner, an Admin, a K2 terminal, the App's agent with "Build and
run Apps" (folders inside its own workspace).

An agent's <app-folder> must be inside its own workspace folder (the
owner, an Admin and a K2 terminal may name any folder), on purpose: it is
the same rule as k2 app deploy (an agent builds and writes
only in its own workspace, never in another agent's or the owner's
folders). A folder elsewhere is refused (folder_outside_workspace), however
it is named; a link that points outside counts as outside. To use it there:
keep the App's source inside your workspace, or ask a person to run k2 app
sdk add on that folder from their own terminal, or copy vendor/ and the
package.json lines from a folder inside your workspace (they are ordinary
files).

Exit codes: 0 done, 2 usage, 3 refused, with JSON on stderr:
  apps_manage_off           ask your owner for "Build and run Apps"
  folder_outside_workspace  an agent's folder must be in its workspace
  sew_kit_unavailable       this K2 pins a Sew older than 0.5.0 (no kit
                            until a K2 update); use add without --sew
  sew_kit_missing           the kit isn't on this box: k2 app sdk kit
                            <file>, or ask your owner for it
  sew_kit_tampered          the kit doesn't match K2's pin: refused
  sdk_package_json_invalid  fix package.json (not valid JSON) and rerun
  sdk_folder_invalid        vendor/ or package.json is a symlink or odd
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

