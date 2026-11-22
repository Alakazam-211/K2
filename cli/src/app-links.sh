# ── k2 app links (0.46.7, prd-tauri-core-0467-v1 TC-20…TC-22) ───────
_app_links_usage() {
    cat <<'EOF'
%include cli/text/help/app/links.txt
EOF
}

cmd_app_links() {
    local sub="${1:-}"
    case "$sub" in
        ""|-h|--help|help) _app_links_usage; return 0 ;;
        set|off|check|status) shift ;;
        --) sub=status ;;
        -*) echo "usage: k2 app links <app> | set|off|check <app> (k2 app links --help)" >&2; exit 2 ;;
        *) sub=status ;;
    esac
    local app="" json_out=0 ios="" android="" paths="" drops=""
    local nl='
'
    local _dd=""
    if _app_says_help "$@"; then set -- --help; fi
    while [ $# -gt 0 ]; do
        case "$_dd$1" in
            --) _dd=:; shift ;;
            --ios) [ "$sub" = set ] || { echo "--ios goes with k2 app links set" >&2; exit 2; }; _need_value "$@"; ios="$ios${2:-}$nl"; shift 2 ;;
            --android) [ "$sub" = set ] || { echo "--android goes with k2 app links set" >&2; exit 2; }; _need_value "$@"; android="$android${2:-}$nl"; shift 2 ;;
            --path) [ "$sub" = set ] || { echo "--path goes with k2 app links set" >&2; exit 2; }; _need_value "$@"; paths="$paths${2:-}$nl"; shift 2 ;;
            --drop) [ "$sub" = set ] || { echo "--drop goes with k2 app links set" >&2; exit 2; }; _need_value "$@"; drops="$drops${2:-}$nl"; shift 2 ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_links_usage; return 0 ;;
            -*) echo "Unknown flag: $1 (k2 app links --help)" >&2; exit 2 ;;
            *) if [ -z "$app" ]; then app="$1"; shift; else echo "Unexpected argument: $1 (k2 app links --help)" >&2; exit 2; fi ;;
        esac
    done
    [ -n "$app" ] || { echo "usage: k2 app links $sub <app> (k2 app links --help)" >&2; exit 2; }
    case "$sub" in
        status)
            _compute_call GET /cli/app/links "app=$(urlencode "$app")"
            ;;
        set)
            { [ -z "$ios" ] && [ -z "$android" ]; } && { echo "name at least one phone app: --ios and/or --android (k2 app links --help)" >&2; exit 2; }
            local body
            body="$(K2_AL_APP="$app" K2_AL_IOS="$ios" K2_AL_ANDROID="$android" K2_AL_PATHS="$paths" K2_AL_DROPS="$drops" python3 -c '
import json, os, sys
e = os.environ
lines = lambda k: [x.strip() for x in e.get(k, "").split("\n") if x.strip()]
b = {"app": e["K2_AL_APP"], "paths": lines("K2_AL_PATHS")}
if lines("K2_AL_IOS"):
    b["ios"] = lines("K2_AL_IOS")
android = []
for spec in lines("K2_AL_ANDROID"):
    if ":" not in spec:
        sys.stderr.write("--android is <package>:<SHA-256 fingerprint>[,<fingerprint>…]\n")
        sys.exit(2)
    pkg, fprs = spec.split(":", 1)
    android.append({"package": pkg, "sha256": [f.strip() for f in fprs.split(",") if f.strip()]})
if android:
    b["android"] = android
if lines("K2_AL_DROPS"):
    b["drop"] = lines("K2_AL_DROPS")
print(json.dumps(b))')" || exit 2
            _app_rt_post /cli/app/links/set "$body"
            [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
            ;;
        off)
            _compute_call POST /cli/app/links/off "$(_compute_json K2_CJ_app="$app")"
            ;;
        check)
            _compute_call POST /cli/app/links/check "$(_compute_json K2_CJ_app="$app")" tcp
            if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
bad = 0
words = {"ok": "ok", "none": "not used", "stale": "another file is served there (DNS or a proxy elsewhere, or a cached copy)",
         "not_served": "not served", "tls_error": "TLS error"}
for h in v.get("hosts") or []:
    print(h.get("host"))
    for k, n in (("aasa", "iOS file"), ("assetlinks", "Android file"), ("appleCdn", "Apple CDN copy")):
        s = h.get(k)
        if s not in ("ok", "none") and k != "appleCdn":
            bad = 1
        print("  %-15s %s" % (n + ":", words.get(s, s)))
sys.exit(1 if bad else 0)'
            return
            ;;
    esac
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys, time
v = json.load(sys.stdin)
print("%s: link files %s%s" % (v.get("name") or v.get("app"), v.get("state"), "" if v.get("served", True) else " (" + (v.get("notServed") or "not served") + ")"))
if v.get("pairing"):
    print("  phone pairing is on too: K2 serves one file for both")
ios = v.get("ios") or []
android = [a.get("package") for a in v.get("android") or []]
print("  iOS apps:     " + (", ".join(ios) if ios else "-"))
print("  Android apps: " + (", ".join(android) if android else "-"))
paths = v.get("paths") or []
if paths:
    print("  paths (first shipped):")
    for p in paths:
        print("    %-40s %s" % (p.get("spec"), time.strftime("%Y-%m-%d", time.localtime(p.get("since") or 0))))
else:
    print("  paths: -")
print("  hosts:        " + (", ".join(v.get("hosts") or []) or "- (the App has no public host yet)"))
lc = v.get("lastCheck")
if lc:
    print("  last check (%s):" % time.strftime("%Y-%m-%d %H:%M", time.localtime(lc.get("at") or 0)))
    for h in lc.get("hosts") or []:
        print("    %s: iOS file %s, Android file %s, Apple CDN %s" % (h.get("host"), h.get("aasa"), h.get("assetlinks"), h.get("appleCdn")))
for w in v.get("warnings") or []:
    print("note: " + w)'
}

