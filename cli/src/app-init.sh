# ── k2 app init (0.46.7, prd-tauri-core-0467-v1 TC-13) ─────────────
_app_init_usage() {
    cat <<'EOF'
%include cli/text/help/app/init.txt
EOF
}

cmd_app_init() {
    local template="" folder="" name="" app="" voice="" dry=0 json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --template) _need_value "$@"; template="${2:-}"; shift 2 ;;
            --template=*) template="${1#--template=}"; shift ;;
            --name) _need_value "$@"; name="${2:-}"; shift 2 ;;
            --name=*) name="${1#--name=}"; shift ;;
            --app) _need_value "$@"; app="${2:-}"; shift 2 ;;
            --app=*) app="${1#--app=}"; shift ;;
            --voice) voice=1; shift ;;
            --dry-run) dry=1; shift ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_init_usage; return 0 ;;
            -*) echo "Unknown flag: $1 (k2 app init --help)" >&2; exit 2 ;;
            *) if [ -z "$folder" ]; then folder="$1"; shift; else echo "Unexpected argument: $1 (k2 app init --help)" >&2; exit 2; fi ;;
        esac
    done
    case "$template" in
        tauri|static|node-api) ;;
        "") echo "k2 app init: --template is required: tauri, static or node-api (k2 app init --help)" >&2; exit 2 ;;
        *) echo "k2 app init: --template is tauri, static or node-api, not '$template'" >&2; exit 2 ;;
    esac
    if [ "$template" != tauri ] && { [ -n "$app" ] || [ -n "$voice" ]; }; then
        echo "k2 app init: --app and --voice go with --template tauri, not --template $template" >&2; exit 2
    fi
    if [ -n "$app" ] && [ -n "$name" ]; then
        echo "k2 app init: pass --app or --name, not both (--app names the App the shell updates through)" >&2; exit 2
    fi
    [ -n "$folder" ] || folder="."
    [ -d "$folder" ] || { echo "k2 app init: $folder is not a folder" >&2; exit 2; }
    local dryb=""
    [ "$dry" = 1 ] && dryb=1
    _app_rt_post /cli/app/init "$(_compute_json K2_CJ_folder="$(_app_rt_abs "$folder")" K2_CJ_template="$template" K2_CJ_name="$name" K2_CJ_app="$app" K2_CJ_voice_b="$voice" K2_CJ_dryRun_b="$dryb")"
    [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
dry = v.get("dryRun")
words = {"write": "would write" if dry else "wrote", "edit": "would edit" if dry else "edited", "skipped": "skipped", "snippet": "add by hand"}
for f in v.get("files") or []:
    print("%-12s %s  (%s)" % (words.get(f.get("action"), f.get("action")), f.get("path"), f.get("note")))
    if f.get("content") and (dry or f.get("action") == "snippet"):
        for line in f["content"].rstrip("\n").split("\n"):
            print("    | " + line)
for n in v.get("notes") or []:
    print("note: " + n)
if dry:
    print("Nothing was written (--dry-run).")
nxt = v.get("next") or []
if nxt:
    print("Next:")
    for i, c in enumerate(nxt, 1):
        print("  %d. %s" % (i, c))'
}

