# ── k2 app init (0.46.7, prd-tauri-core-0467-v1 TC-13) ─────────────
_app_init_usage() {
    cat <<'EOF'
k2 app init — write the K2 side of an App into a repo. Never overwrites a
file: one that is already there is listed as skipped and left as it is.

  k2 app init --template tauri [<folder>] [--name <name> | --app <app>] [--voice] [--dry-run] [--json]
      In a Tauri v2 app's repo root (default: this folder):
        k2/web.manifest.json    the web build as a static App, <name>-web
        src-tauri/tauri.conf.json
                                gains the updater wiring: plugins.updater
                                (endpoints on the App's URL, an empty pubkey
                                to fill in) and bundle.createUpdaterArtifacts.
                                Only missing keys are added; tauri.conf.json5
                                and Tauri.toml are not edited (the lines to
                                add are printed).
        k2/README.md            the next commands, where the signing keys
                                live (your own Mac, never K2), the runtime
                                fixes, store review, and what user data
                                your App stores and how to declare it
        src-tauri/PrivacyInfo.xcprivacy
                                Apple's privacy manifest (add it to the iOS
                                target: the README says how)
        src-tauri/Info.ios.plist
                                the scene manifest iOS 27 requires, and the
                                export answer (no encryption beyond HTTPS)
        src-tauri/src/k2_http.rs
                                one shared HTTP client for your Rust code
        .k2/compute.toml        a tauri-linux preset, if the file already
                                has presets
        k2/ci/build-linux.sh    the release build K2 Compute runs (deb and
                                AppImage in k2/out/), made so two nodes
                                build the same bytes; it signs nothing
        k2/ci/build-macos.sh    an unsigned macOS compile check (Mac
                                releases are signed on your own Mac)
        .k2/ci.toml             a check, and [release]: k2 ci release
                                <version> builds it twice on your seat
                                (two hosts), k2 ci evidence hands it to
                                your signer (k2 study ci-release). An
                                existing .k2/ci.toml gets [release] printed
        .k2/signer.toml         the signer policy's starting point: this
                                server's and your seat owner's fingerprints
                                when K2 knows them (all zeros otherwise);
                                a person approves it AT the signer
      Printed for you to add (K2 never edits these files): the Cargo lines
      it lacks (Tauri 2.12.5 or newer, K2's patched wry, reqwest), and the
      AndroidManifest lines (POST_NOTIFICATIONS). Then check it all with
      k2 app doctor --store.
      --app <app>   a thin shell over an App already on this box, loading its
                    live site: no web manifest and no <name>-web. The updater
                    asks that App's host first, then the download host. The
                    App must be yours and K2 must sit in front of it
                    (app_not_found, needs_k2_front otherwise).
      --voice       what calls in the webview need. New files:
                    src-tauri/Info.plist (macOS: mic and camera sentences,
                    no App Nap), Info.ios.plist (iOS: the same sentences,
                    background audio, and the export answer "yes": calls
                    are end-to-end encrypted), Entitlements.plist
                    (macOS), and src-tauri/src/k2_voice.rs (the mic and
                    camera for your App's own origin only; you add two lines
                    to lib.rs, printed under Next). In tauri.conf.json:
                    bundle.macOS.entitlements, and on the first window
                    backgroundThrottling and additionalBrowserArgs
                    (WebView2). A plist that is there is left alone and the
                    keys it lacks are printed. So are the AndroidManifest
                    lines (mic, camera, and the foreground-service types a
                    call needs). k2/README.md has the CSP and the export
                    rules; k2 study tauri-voice says what is tested where.
  k2 app init --template static [<folder>] [--name <name>]
      k2/web.manifest.json only, for any built site (docs, a PWA).
  k2 app init --template node-api [<folder>] [--name <name>]
      k2/api/: a small server for the App (<name>-api): a health route and an
      upload route that takes a phone's file in parts of at most 16 MiB.

  <name>        the App name, used as <name>-web or <name>-api. Default: the
                folder's name made into one (lower case, anything else
                becomes -: "Field Notes" is field-notes).
  --dry-run     print every file and change; write nothing.

Then: the commands it prints (Next:), and k2 study tauri.
EOF
}

cmd_app_init() {
    local template="" folder="" name="" app="" voice="" dry=0 json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --template) template="${2:-}"; shift 2 ;;
            --template=*) template="${1#--template=}"; shift ;;
            --name) name="${2:-}"; shift 2 ;;
            --name=*) name="${1#--name=}"; shift ;;
            --app) app="${2:-}"; shift 2 ;;
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

