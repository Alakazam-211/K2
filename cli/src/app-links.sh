# ── k2 app links (0.46.7, prd-tauri-core-0467-v1 TC-20…TC-22) ───────
_app_links_usage() {
    cat <<'EOF'
k2 app links — "open in app" links for an App's own iOS and Android apps.
K2 serves the two link files (/.well-known/apple-app-site-association and
/.well-known/assetlinks.json) on every host of the App, with or without phone
pairing (k2 app pairing). With pairing on too, its apps and paths and these
are served in the same two files.

  k2 app links <app> [--json]          The phone apps, the paths (each with the
                                       date K2 first served it), the hosts,
                                       on/off.
  k2 app links set <app> [--ios TEAMID.com.you.app]…
                 [--android com.you.app:AB:CD:…[,AB:CD:…]]…
                 [--path '/tickets/*']… [--path '/messages?addr=*']…
                 [--drop '<path>']… [--json]
                                       Name the phone apps and the paths they
                                       open, and turn the files on. Each set
                                       replaces the whole list: give every
                                       --ios, --android and --path again. A set
                                       that leaves out a path already set is
                                       refused (link_path_dropped) unless
                                       --drop names it, written as k2 app links
                                       shows it (query pairs sorted by name):
                                       phones and Apple's CDN keep the old
                                       file for days. An app left out is
                                       removed. Quote paths so the shell
                                       keeps * and ?.
  k2 app links off <app>               Stop serving them (the apps and paths
                                       stay; set turns them on again).
  k2 app links check <app> [--json]    Fetch both files from each host over the
                                       internet (and Apple's CDN copy) and
                                       compare them with what K2 serves. The
                                       owner only; the last result shows in
                                       k2 app links <app>.

Paths: a path (A-Z a-z 0-9 . _ ~ * / -, never under /.well-known/), and an
optional query pattern after ?: name=value pairs joined by &, where * in a
value matches anything. /messages?addr=* opens /messages?addr=… links in the
app (iOS 14 and newer; older iOS matches the path alone). At most 20 paths.

--android takes the package, a colon, then the full SHA-256 fingerprint of
the certificate that signs the app as installed (from Play: the app signing
key), colons and all; several fingerprints are comma separated.

In your app, list each host exactly (applinks:jobs.you.k2.dev, and each
custom domain you add), never a wildcard. K2 answers those two paths in
front of the App, so an App whose manifest says host.clientIp: false (its
connections reach it raw) can't have them: refused with needs_k2_front.

Who: every form needs the owner, an Admin, or the App's own agent with the
workspace's "Build and run Apps" switch (Settings → Apps); check is the
owner's. Every change is recorded in the App's deploy audit. Exit codes: 0
ok, 2 bad usage, 3 refused (JSON with a code on stderr); check exits 1 when a
host serves a different file.
EOF
}

cmd_app_links() {
    local sub="${1:-}"
    case "$sub" in
        ""|-h|--help|help) _app_links_usage; return 0 ;;
        set|off|check|status) shift ;;
        -*) echo "usage: k2 app links <app> | set|off|check <app> (k2 app links --help)" >&2; exit 2 ;;
        *) sub=status ;;
    esac
    local app="" json_out=0 ios="" android="" paths="" drops=""
    local nl='
'
    while [ $# -gt 0 ]; do
        case "$1" in
            --ios) [ "$sub" = set ] || { echo "--ios goes with k2 app links set" >&2; exit 2; }; ios="$ios${2:-}$nl"; shift 2 ;;
            --android) [ "$sub" = set ] || { echo "--android goes with k2 app links set" >&2; exit 2; }; android="$android${2:-}$nl"; shift 2 ;;
            --path) [ "$sub" = set ] || { echo "--path goes with k2 app links set" >&2; exit 2; }; paths="$paths${2:-}$nl"; shift 2 ;;
            --drop) [ "$sub" = set ] || { echo "--drop goes with k2 app links set" >&2; exit 2; }; drops="$drops${2:-}$nl"; shift 2 ;;
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

