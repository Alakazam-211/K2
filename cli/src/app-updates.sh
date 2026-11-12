# ── k2 app updates (0.46.7): an App's update feed (Tauri's updater) ──
_app_updates_usage() {
    cat <<'EOF'
k2 app updates — signed desktop updates for an App (Tauri's updater)

<app> is the App's id on this K2 box (k2 app list).

How it works: you sign each build with the App's OWN updater key on your
own Mac (tauri signer sign writes <file>.sig next to each file; a push
needs it); K2 keeps only the public key. A push checks every signature,
uploads each file ONCE to K2's download host (dl.k2.dev), and then the
App's own hosts answer Tauri's updater at
  https://<app host>/_k2/updates/<channel>/{{target}}/{{arch}}/{{current_version}}
  https://<app host>/_k2/updates/<channel>/latest.json
with small JSON that points at the download host. Installers are never
downloaded from your K2 box. Put the first URL in tauri.conf.json
plugins.updater.endpoints, with <channel> = the channel that build follows
(stable for normal builds, beta for beta builds). Type the {{target}},
{{arch}} and {{current_version}} parts literally: Tauri fills them in.
k2 app updates ls <app> prints the URL with your App's host.
A download button: link https://<app host>/_k2/download/<channel>/<platform>
(linux-x86_64, darwin-aarch64, …): it redirects to the newest file (404
when that channel has none for the platform).

First, once per App:
  k2 app updates key <app> --set <pubkey>
        The App's updater PUBLIC key: the content of the .pub file that
        tauri signer generate wrote (the same text as tauri.conf.json
        plugins.updater.pubkey). --set-file <file.pub> reads it from a file.
        A secret key is refused. Changing the key after the App has
        releases is held for your owner (every installed copy would trust
        the new key).
  k2 app updates key <app>                     show the key and its id
  k2 app updates token <app> --file <path>     keep the App's download-host
  k2 app updates token <app> --stdin           token (k2dl_…, from K2 Web)
        Read from a file or stdin, never typed on the command line. With a
        vault secret: k2 app updates token <app> --file "$(k2 vault file
        --secret <NAME>)". K2 never shows it again (token <app> shows set
        or not and a fingerprint).

Releases:
  k2 app updates push <app> --channel <name> --version <semver>
        --artifact <platform>=<file> [--artifact …] [--sig <platform>=<file.sig>]
        [--notes-file <file>] [--drop-platform <platform>]
        [--evidence <ci release run>] [--no-wait] [--json]
        --evidence: the k2 ci release run that built the files (k2 study
        ci-release): it must have passed, for this --version, and each
        file must be one of its jobs' files by sha256 (else
        evidence_mismatch, exit 3). ls then shows which run built it.
        <platform> is Tauri's key: <os>-<arch> or <os>-<arch>-<installer>
        (os darwin|linux|windows; arch x86_64|aarch64|i686|armv7; installer
        app|appimage|deb|rpm|msi|nsis), e.g. darwin-aarch64, linux-x86_64,
        windows-x86_64. Files: .app.tar.gz, .AppImage, .AppImage.tar.gz,
        .deb, .rpm, .msi, .exe, .nsis.zip, .msi.zip, at most 512 MiB. A
        <file> path is relative to where you run the command (or absolute)
        and must be inside the App's workspace folder. The signature
        defaults to <file>.sig (tauri signer sign writes it).
        --version is strict SemVer 2.0.0 (1.4.1, 1.4.0-build.42; not 1.0 or
        v1.0.0) and must be newer than the channel's current one. A channel
        is stable, beta or your own name ([a-z][a-z0-9-], up to 20).
        If the channel's current release has a platform this push lacks,
        it is refused (those users would stop getting updates) unless you
        name it with --drop-platform.
        The push waits until every upload is confirmed, then the channel
        moves and prints the download URLs. --no-wait returns at once (exit
        2); follow it with k2 app updates ls <app>.
  k2 app updates ls <app> [--channel <name>] [--release <id|version>] [--json]
        Channels and their versions, every release with its files (links,
        sizes, upload state), active users, and the updater URLs.
  k2 app updates promote <app> --from beta --to stable [--version <v>] [--drop-platform] [--json]
        Point --to at a release that is already uploaded (default: --from's
        current one). No re-upload. Only to a HIGHER version than --to
        serves now (to go back, use rollback). --drop-platform (no value
        here) allows the release to have fewer platforms than --to serves.
  k2 app updates rollback <app> --channel <name> --to <version> [--json]
        Point a channel back at an older release it served before (not one
        you pulled).
        Tauri installs only NEWER versions by default, so people already on
        the bad version STAY on it: a rollback protects new installs and
        people still below. To fix people on the bad version, push a higher
        version with the fix.
  k2 app updates pull <app> --version <version> [--json]
        Stop offering a release on every channel (each channel falls back to
        its newest older release, or goes empty). A download host that can
        (k2dl 1.4.0+) stops serving its files too; an older one keeps them
        at their URLs, and the answer says which.
  k2 app updates stats <app> [--days 30] [--json]
        Downloads per version (from the download host) and version checks
        per day, platform and version (from this box). "unavailable" means
        the download host didn't answer, not zero.
  k2 app updates fleet [--json]
        Every App's channels, release counts and check-ins, aggregate only
        (owner and Admins).

Active users (shown by ls and stats): each update check counts once for
its day, platform and version. If your app adds ?install=<random id, 8-64 of A-Z a-z 0-9 -> to
its endpoint URL, distinct installs are counted too; only a hash of that
id is kept, for two days. No addresses are kept.

Who: reads, anyone who may see the App. Changes: the owner, an Admin, or
the App's agent with "Build and run Apps".

Exit codes: 0 done, 1 the push's upload failed, 2 usage error, held for
your owner, or --no-wait (still uploading), 3 refused, with JSON on stderr:
  no_update_key          set the App's key first: k2 app updates key <app> --set …
  secret_key_refused     that was a SECRET key; give the .pub content
  dl_token_missing       keep the App's token: k2 app updates token <app> --file …
  dl_token_invalid       the token is wrong or revoked: ask K2 Web for a new one
  signature_invalid      wrong key, or the file changed after signing
  signature_missing      no <file>.sig: sign it, or pass --sig
  version_invalid        not strict SemVer 2.0.0
  version_not_newer      a push or promote must be newer (go back: rollback)
  version_exists         a version is never replaced: ship a higher one
  platform_dropped       add the platform's file, or --drop-platform it
  platform_invalid       not a Tauri v2 platform key
  artifact_type_refused  not a file type the updater uses
  artifact_too_large     over 512 MiB
  file_outside_workspace files must be inside the App's workspace folder
  needs_k2_front         the App's manifest has host.clientIp: false
  updates_host_unavailable  the download host didn't answer; try later
  updates_host_refused   the download host refused a file (its reason shown)
  dl_id_invalid          the App or box name can't form dev.k2.<box>.<app>
  key_change_held        a key change waits for your owner's approval
EOF
}

cmd_app_updates() {
    local sub="${1:-}"
    shift || true
    case "$sub" in
        ls|list) _app_updates_ls "$@" ;;
        key) _app_updates_key "$@" ;;
        token) _app_updates_token "$@" ;;
        push) _app_updates_push "$@" ;;
        promote) _app_updates_promote "$@" ;;
        rollback) _app_updates_rollback "$@" ;;
        pull) _app_updates_pull "$@" ;;
        stats) _app_updates_stats "$@" ;;
        fleet) _app_updates_fleet "$@" ;;
        ""|-h|--help|help) _app_updates_usage ;;
        *) echo "usage: k2 app updates ls|key|token|push|promote|rollback|pull|stats|fleet … (k2 app updates --help)" >&2; exit 2 ;;
    esac
}

# Print a release's state and files (stdin: the ls answer, $1 release id).
_app_updates_print_release() {
    python3 -c '
import json, sys
v = json.load(sys.stdin)
rid = sys.argv[1]
for r in v.get("releases", []):
    if r["id"] != rid and r["version"] != rid:
        continue
    on = ", ".join(r.get("channels") or []) or "no channel"
    print("%s %s: %s (%s)" % (v["app"], r["version"], r["state"], on))
    if r.get("error"):
        print("  error: %s" % r["error"])
    for f in r.get("files", []):
        print("  %-24s %10s  %s" % (f["platform"], "%.1f MiB" % (f["size"] / 1048576.0), f.get("url") or f.get("upload")))
' "$1"
}

# k2 app updates ls <app> [--channel c] [--release r] [--json]
_app_updates_ls() {
    local app="" ch="" rel="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --channel) ch="${2:-}"; shift 2 ;;
            --release) rel="${2:-}"; shift 2 ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_updates_usage; return 0 ;;
            -*) echo "Unknown flag: $1 (k2 app updates --help)" >&2; exit 2 ;;
            *) [ -z "$app" ] || { echo "usage: k2 app updates ls <app> [--channel <name>] [--release <id|version>] [--json]" >&2; exit 2; }; app="$1"; shift ;;
        esac
    done
    [ -n "$app" ] || { echo "usage: k2 app updates ls <app> [--channel <name>] [--release <id|version>] [--json]" >&2; exit 2; }
    local q="app=$(urlencode "$app")"
    [ -n "$ch" ] && q="$q&channel=$(urlencode "$ch")"
    [ -n "$rel" ] && q="$q&release=$(urlencode "$rel")"
    _compute_call GET /cli/app/updates "$q"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
k = v.get("key")
print("%s  (download id %s; updater key %s)" % (v["app"], v.get("dlId") or "-", k["keyId"] if k else "not set: k2 app updates key %s --set <pubkey>" % v["app"]))
chans = v.get("channels") or []
if chans:
    print("Channels: " + ", ".join("%s %s" % (c["channel"], c.get("version") or "-") for c in chans))
else:
    print("Channels: none yet (k2 app updates push %s --channel stable …)" % v["app"])
u = v.get("activeUsers") or {}
print("Active users: %s today, %s at most on one day this week (counting %s)" % (u.get("today", 0), u.get("last7Days", 0), u.get("counting", "checks")))
for r in v.get("releases", []):
    on = ", ".join(r.get("channels") or [])
    print("  %-14s %-9s %-16s %6.1f MiB  %s users%s" % (r["version"], r["state"], on or "-", r.get("size", 0) / 1048576.0, r.get("activeUsers", 0), ("  " + r["error"]) if r.get("error") else ""))
    for f in r.get("files", []):
        print("      %-24s %8.1f MiB  %s" % (f["platform"], f["size"] / 1048576.0, f.get("url") or f.get("upload")))
e = v.get("endpoints")
if e:
    print("Updater endpoint (tauri.conf.json plugins.updater.endpoints): " + e["dynamic"])
'
}

# k2 app updates key <app> [--set <pubkey> | --set-file <file.pub>] [--json]
_app_updates_key() {
    local app="" key="" key_file="" json_out=0 setting=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --set) key="${2:-}"; setting=1; shift 2 ;;
            --set-file) key_file="${2:-}"; setting=1; shift 2 ;;
            --show) shift ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_updates_usage; return 0 ;;
            -*) echo "Unknown flag: $1 (k2 app updates --help)" >&2; exit 2 ;;
            *) [ -z "$app" ] || { echo "usage: k2 app updates key <app> [--set <pubkey> | --set-file <file.pub>]" >&2; exit 2; }; app="$1"; shift ;;
        esac
    done
    [ -n "$app" ] || { echo "usage: k2 app updates key <app> [--set <pubkey> | --set-file <file.pub>]" >&2; exit 2; }
    if [ "$setting" = 0 ]; then
        _compute_call GET /cli/app/updates/key "app=$(urlencode "$app")"
        if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
        printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
if not v.get("pubkey"):
    print("%s has no updater key: k2 app updates key %s --set <pubkey>" % (v["app"], v["app"]))
else:
    print("%s updater key %s (set by %s)" % (v["app"], v["keyId"], v.get("setBy")))
    print(v["pubkey"])
'
        return 0
    fi
    if [ -n "$key_file" ]; then
        [ -f "$key_file" ] || { echo "k2 app updates key: $key_file is not a file" >&2; exit 2; }
        key="$(cat "$key_file")"
    fi
    [ -n "$key" ] || { echo "k2 app updates key: the key is empty" >&2; exit 2; }
    _app_rt_post /cli/app/updates/key/set "$(_compute_json K2_CJ_app="$app" K2_CJ_pubkey="$key")"
    if [ "$_CP_STATUS" = "202" ]; then
        if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; else printf '%s' "$_CP_BODY" | _app_held_exit; fi
        return 2
    fi
    [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
if v.get("unchanged"):
    print("%s already has updater key %s" % (v["app"], v["keyId"]))
else:
    print("%s updater key %s %s" % (v["app"], v["keyId"], "changed" if v.get("changed") else "set"))
'
}

# k2 app updates token <app> --file <path> | --stdin | (show)
_app_updates_token() {
    local app="" file="" from_stdin=0 json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --file) file="${2:-}"; shift 2 ;;
            --stdin) from_stdin=1; shift ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_updates_usage; return 0 ;;
            -*) echo "Unknown flag: $1 (k2 app updates --help; the token is read from --file or --stdin, never an argument)" >&2; exit 2 ;;
            *) [ -z "$app" ] || { echo "usage: k2 app updates token <app> --file <path> | --stdin (never the token itself)" >&2; exit 2; }; app="$1"; shift ;;
        esac
    done
    [ -n "$app" ] || { echo "usage: k2 app updates token <app> --file <path> | --stdin" >&2; exit 2; }
    if [ -z "$file" ] && [ "$from_stdin" = 0 ]; then
        _compute_call GET /cli/app/updates/token "app=$(urlencode "$app")"
        if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
        printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
if v.get("set"):
    print("%s: download-host token set (fingerprint %s) for %s on %s" % (v["app"], v["fingerprint"], v.get("dlId"), v.get("host")))
else:
    print("%s: no download-host token. Ask K2 Web for one for %s, then k2 app updates token %s --file <file>" % (v["app"], v.get("dlId"), v["app"]))
'
        return 0
    fi
    local body
    if [ -n "$file" ]; then
        [ -f "$file" ] || { echo "k2 app updates token: $file is not a file" >&2; exit 2; }
        body="$(python3 -c 'import json, sys; print(json.dumps({"app": sys.argv[1], "token": sys.stdin.read().strip()}))' "$app" < "$file")"
    else
        body="$(python3 -c 'import json, sys; print(json.dumps({"app": sys.argv[1], "token": sys.stdin.read().strip()}))' "$app")"
    fi
    _app_rt_post /cli/app/updates/token/set "$body"
    body=""
    [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print("%s: download-host token kept in the vault (fingerprint %s, id %s)" % (v["app"], v["fingerprint"], v["dlId"]))
'
}

# Absolute path of a file (relative to the caller's folder).
_app_updates_abs() {
    case "$1" in
        /*) printf '%s' "$1" ;;
        *) printf '%s/%s' "$(pwd -P)" "$1" ;;
    esac
}

# k2 app updates push <app> --channel c --version v --artifact p=f … [--sig p=f] [--notes-file f] [--drop-platform p] [--evidence <run>] [--no-wait] [--json]
_app_updates_push() {
    local app="" ch="" ver="" notes_file="" no_wait=0 json_out=0 evidence=""
    local -a arts=() sigs=() drops=()
    local usage="usage: k2 app updates push <app> --channel <name> --version <semver> --artifact <platform>=<file> [--artifact …] [--sig <platform>=<file.sig>] [--notes-file <file>] [--drop-platform <platform>] [--evidence <ci release run>] [--no-wait] [--json]"
    while [ $# -gt 0 ]; do
        case "$1" in
            --channel) ch="${2:-}"; shift 2 ;;
            --version) ver="${2:-}"; shift 2 ;;
            --artifact) arts+=("${2:-}"); shift 2 ;;
            --sig) sigs+=("${2:-}"); shift 2 ;;
            --notes-file) notes_file="${2:-}"; shift 2 ;;
            --drop-platform) drops+=("${2:-}"); shift 2 ;;
            --evidence) evidence="${2:-}"; [ -n "$evidence" ] || { echo "$usage" >&2; exit 2; }; shift 2 ;;
            --no-wait) no_wait=1; shift ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_updates_usage; return 0 ;;
            -*) echo "Unknown flag: $1 (k2 app updates --help)" >&2; exit 2 ;;
            *) [ -z "$app" ] || { echo "$usage" >&2; exit 2; }; app="$1"; shift ;;
        esac
    done
    { [ -n "$app" ] && [ -n "$ch" ] && [ -n "$ver" ] && [ "${#arts[@]}" -gt 0 ]; } || { echo "$usage" >&2; exit 2; }
    local a p f list="" notes=""
    for a in "${arts[@]}"; do
        p="${a%%=*}"; f="${a#*=}"
        { [ "$p" != "$a" ] && [ -n "$p" ] && [ -n "$f" ]; } || { echo "k2 app updates push: --artifact takes <platform>=<file> (got '$a')" >&2; exit 2; }
        [ -f "$f" ] || { echo "k2 app updates push: $f is not a file" >&2; exit 2; }
        list="$list$p"$'\t'"$(_app_updates_abs "$f")"$'\n'
    done
    local siglist=""
    for a in "${sigs[@]+"${sigs[@]}"}"; do
        p="${a%%=*}"; f="${a#*=}"
        { [ "$p" != "$a" ] && [ -n "$p" ] && [ -n "$f" ]; } || { echo "k2 app updates push: --sig takes <platform>=<file.sig> (got '$a')" >&2; exit 2; }
        siglist="$siglist$p"$'\t'"$(_app_updates_abs "$f")"$'\n'
    done
    if [ -n "$notes_file" ]; then
        [ -f "$notes_file" ] || { echo "k2 app updates push: $notes_file is not a file" >&2; exit 2; }
        notes="$(cat "$notes_file")"
    fi
    local body
    body="$(K2_UP_ARTS="$list" K2_UP_SIGS="$siglist" K2_UP_DROPS="$(printf '%s\n' "${drops[@]+"${drops[@]}"}")" K2_UP_NOTES="$notes" python3 -c '
import json, os, sys
sigs = dict(l.split("\t", 1) for l in os.environ["K2_UP_SIGS"].splitlines() if l)
arts = []
for l in os.environ["K2_UP_ARTS"].splitlines():
    if not l:
        continue
    p, f = l.split("\t", 1)
    a = {"platform": p, "path": f}
    if p in sigs:
        a["sig"] = sigs.pop(p)
    arts.append(a)
if sigs:
    sys.stderr.write("k2 app updates push: --sig for a platform with no --artifact: %s\n" % ", ".join(sigs))
    sys.exit(2)
body = {"app": sys.argv[1], "channel": sys.argv[2], "version": sys.argv[3], "artifacts": arts,
        "dropPlatforms": [d for d in os.environ["K2_UP_DROPS"].splitlines() if d]}
if os.environ.get("K2_UP_NOTES"):
    body["notes"] = os.environ["K2_UP_NOTES"]
if sys.argv[4]:
    body["evidence"] = sys.argv[4]
print(json.dumps(body))
' "$app" "$ch" "$ver" "$evidence")" || exit 2
    K2_CP_MAX_TIME=900 _app_rt_post /cli/app/updates/push "$body"
    [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
    local rid
    rid="$(printf '%s' "$_CP_BODY" | python3 -c 'import json, sys; print(json.load(sys.stdin)["release"])')"
    if [ "$no_wait" = 1 ]; then
        if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; else
            echo "Uploading $app $ver ($rid) to the download host. Follow it: k2 app updates ls $app --release $rid"
        fi
        return 2
    fi
    [ "$json_out" = 1 ] || echo "Checked every signature; uploading $app $ver ($rid) to the download host…"
    local state="uploading" waited=0
    while [ "$state" = "uploading" ]; do
        sleep 2; waited=$((waited + 2))
        _compute_call GET /cli/app/updates "app=$(urlencode "$app")&release=$(urlencode "$rid")"
        state="$(printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
r = v.get("releases") or []
print(r[0]["state"] if r else "missing")
')"
        if [ "$state" = "uploading" ] && [ $((waited % 60)) = 0 ] && [ "$json_out" = 0 ]; then echo "  still uploading (${waited}s)…"; fi
    done
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; else printf '%s' "$_CP_BODY" | _app_updates_print_release "$rid"; fi
    case "$state" in
        live) [ "$json_out" = 1 ] || echo "$ch now serves $ver." ; return 0 ;;
        *) return 1 ;;
    esac
}

# k2 app updates promote <app> --from a --to b [--version v] [--drop-platform] [--json]
_app_updates_promote() {
    local app="" from="" to="" ver="" drop=0 json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --from) from="${2:-}"; shift 2 ;;
            --to) to="${2:-}"; shift 2 ;;
            --version) ver="${2:-}"; shift 2 ;;
            --drop-platform) drop=1; shift ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_updates_usage; return 0 ;;
            -*) echo "Unknown flag: $1 (k2 app updates --help)" >&2; exit 2 ;;
            *) [ -z "$app" ] || { echo "usage: k2 app updates promote <app> --from <channel> --to <channel> [--version <v>]" >&2; exit 2; }; app="$1"; shift ;;
        esac
    done
    { [ -n "$app" ] && [ -n "$to" ] && { [ -n "$from" ] || [ -n "$ver" ]; }; } || { echo "usage: k2 app updates promote <app> --from <channel> --to <channel> [--version <v>] (or --to <channel> --version <v>)" >&2; exit 2; }
    _app_rt_post /cli/app/updates/promote "$(_compute_json K2_CJ_app="$app" K2_CJ_from="$from" K2_CJ_to="$to" K2_CJ_version="$ver" K2_CJ_dropPlatforms_b="$drop")"
    [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
if v.get("unchanged"):
    print("%s %s already serves %s" % (v["app"], v["channel"], v["version"]))
else:
    print("%s %s now serves %s (no re-upload). Fallback latest.json: %s" % (v["app"], v["channel"], v["version"], v.get("fallback")))
'
}

# k2 app updates rollback <app> --channel c --to v [--json]
_app_updates_rollback() {
    local app="" ch="" to="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --channel) ch="${2:-}"; shift 2 ;;
            --to) to="${2:-}"; shift 2 ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_updates_usage; return 0 ;;
            -*) echo "Unknown flag: $1 (k2 app updates --help)" >&2; exit 2 ;;
            *) [ -z "$app" ] || { echo "usage: k2 app updates rollback <app> --channel <name> --to <version>" >&2; exit 2; }; app="$1"; shift ;;
        esac
    done
    { [ -n "$app" ] && [ -n "$ch" ] && [ -n "$to" ]; } || { echo "usage: k2 app updates rollback <app> --channel <name> --to <version>" >&2; exit 2; }
    _app_rt_post /cli/app/updates/rollback "$(_compute_json K2_CJ_app="$app" K2_CJ_channel="$ch" K2_CJ_to="$to")"
    [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print("%s %s: %s -> %s" % (v["app"], v["channel"], v["from"], v["version"]))
print("WARNING: " + v["warning"])
'
}

# k2 app updates pull <app> --version v [--json]
_app_updates_pull() {
    local app="" ver="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --version) ver="${2:-}"; shift 2 ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_updates_usage; return 0 ;;
            -*) echo "Unknown flag: $1 (k2 app updates --help)" >&2; exit 2 ;;
            *) [ -z "$app" ] || { echo "usage: k2 app updates pull <app> --version <version>" >&2; exit 2; }; app="$1"; shift ;;
        esac
    done
    { [ -n "$app" ] && [ -n "$ver" ]; } || { echo "usage: k2 app updates pull <app> --version <version>" >&2; exit 2; }
    _app_rt_post /cli/app/updates/pull "$(_compute_json K2_CJ_app="$app" K2_CJ_version="$ver")"
    [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
if v.get("unchanged"):
    print("%s %s was already pulled" % (v["app"], v["version"])); sys.exit(0)
print("%s %s pulled." % (v["app"], v["version"]))
for c in v.get("channels", []):
    print("  %s now serves %s" % (c["channel"], c.get("now") or "nothing (empty)"))
print(v.get("note", ""))
'
}

# k2 app updates stats <app> [--days N] [--json]
_app_updates_stats() {
    local app="" days="30" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --days) days="${2:-}"; shift 2 ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_updates_usage; return 0 ;;
            -*) echo "Unknown flag: $1 (k2 app updates --help)" >&2; exit 2 ;;
            *) [ -z "$app" ] || { echo "usage: k2 app updates stats <app> [--days 30]" >&2; exit 2; }; app="$1"; shift ;;
        esac
    done
    [ -n "$app" ] || { echo "usage: k2 app updates stats <app> [--days 30]" >&2; exit 2; }
    _compute_call GET /cli/app/updates/stats "app=$(urlencode "$app")&days=$(urlencode "$days")"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
d = v["downloads"]
print("%s, %s to %s" % (v["app"], v["from"], v["to"]))
if d.get("available"):
    print("Downloads: %d" % d["total"])
    for x in d.get("byVersion", []):
        print("  %-14s %6d" % (x["version"], x["downloads"]))
else:
    print("Downloads: unavailable (%s)" % d.get("why"))
u = v["activeUsers"]
print("Active users: %s today, %s at most on one day this week (counting %s); %s checks" % (u["today"], u["last7Days"], u["counting"], u["checks"]))
for ver, n in sorted((u.get("byVersion") or {}).items()):
    print("  %-14s %6d" % (ver, n))
'
}

# k2 app updates fleet [--json]
_app_updates_fleet() {
    local json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            -h|--help) _app_updates_usage; return 0 ;;
            *) echo "usage: k2 app updates fleet [--json]" >&2; exit 2 ;;
        esac
    done
    _compute_call GET /cli/app/updates/fleet ""
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
if not v["apps"]:
    print("No App on this box has an update feed."); sys.exit(0)
for a in v["apps"]:
    ch = ", ".join("%s %s" % (c["channel"], c.get("version") or "-") for c in a["channels"]) or "no channel"
    print("%-20s %-36s %s; %d releases; %s users this week" % (a["app"], a.get("dlId") or "-", ch, a["releases"], a["activeUsers"]["last7Days"]))
'
}

# k2 app data put|ls|get|rm|mv (put 0.46.3; the rest 0.46.4).
cmd_app_data() {
    local sub="${1:-}"
    shift || true
    case "$sub" in
        put) _app_data_put "$@" ;;
        ls|list) _app_data_ls "$@" ;;
        get) _app_data_get "$@" ;;
        rm|remove) _app_data_rm "$@" ;;
        mv|move) _app_data_mv "$@" ;;
        ""|-h|--help|help) _app_data_usage ;;
        *) echo "usage: k2 app data put|ls|get|rm|mv <app-id> … (k2 app data --help)" >&2; exit 2 ;;
    esac
}

# k2 app data ls <app-id> [<path>] [--json] (0.46.4).
_app_data_ls() {
    local app="" path="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            -h|--help) _app_data_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) if [ -z "$app" ]; then app="$1"; elif [ -z "$path" ]; then path="$1"; else echo "usage: k2 app data ls <app-id> [<path>]" >&2; exit 2; fi; shift ;;
        esac
    done
    [ -n "$app" ] || { echo "usage: k2 app data ls <app-id> [<path>] [--json]" >&2; exit 2; }
    _compute_call GET /cli/app/data/ls "app=$(urlencode "$app")&path=$(urlencode "$path")"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys, time
v = json.load(sys.stdin)
def human(n):
    if n is None:
        return "-"
    for u in ("B", "KiB", "MiB", "GiB"):
        if n < 1024 or u == "GiB":
            return ("%d %s" % (n, u)) if u == "B" else ("%.1f %s" % (n, u))
        n /= 1024.0
ents = v.get("entries") or []
print("%s (%s, %d %s)" % (v.get("at"), v.get("kind"), len(ents), "entry" if len(ents) == 1 else "entries"))
for e in ents:
    t = e.get("mtime")
    when = time.strftime("%Y-%m-%d %H:%M", time.localtime(t)) if t else "-"
    name = e.get("name", "")
    if e.get("type") == "dir":
        name += "/"
    elif e.get("type") == "link":
        name += " (link: never followed)"
    print("  %-5s %10s  %16s  %s" % (e.get("type"), human(e.get("size")), when, name))
if v.get("truncated"):
    print("  … more than %s entries: list a sub-folder" % v.get("max"))'
}

# k2 app data get <app-id> <path> [--to <local path> | -] [--json] (0.46.4).
# Streams 512 KiB pieces into a temp file next to the target, then renames
# it into place: a failed get never leaves half a file.
_app_data_get() {
    local app="" path="" to="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --to) to="${2:-}"; [ -n "$to" ] || { echo "usage: --to <local path> | -" >&2; exit 2; }; shift 2 ;;
            --to=*) to="${1#--to=}"; shift ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_data_usage; return 0 ;;
            -) to="-"; shift ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) if [ -z "$app" ]; then app="$1"; elif [ -z "$path" ]; then path="$1"; else echo "usage: k2 app data get <app-id> <path> [--to <local path> | -]" >&2; exit 2; fi; shift ;;
        esac
    done
    { [ -z "$app" ] || [ -z "$path" ]; } && { echo "usage: k2 app data get <app-id> <path> [--to <local path> | -] [--json]" >&2; exit 2; }
    [ "$to" = "-" ] && [ "$json_out" = 1 ] && { echo "k2 app data get: --json and - (stdout) don't go together" >&2; exit 2; }
    local name="${path%/}"
    name="${name##*/}"
    [ -n "$to" ] || to="./$name"
    [ "$to" != "-" ] && [ -d "$to" ] && to="${to%/}/$name"
    local tmp="-"
    if [ "$to" != "-" ]; then
        tmp="$(dirname "$to")/.$(basename "$to").k2-part-$$"
        : > "$tmp" || { echo "{\"error\":\"local_write\",\"hint\":\"can't write $to\"}" >&2; exit 3; }
        # A refusal mid-way exits: drop the part file, keep the router's
        # permission-note flush.
        _K2_DG_TMP="$tmp"
        trap '_k2_exit_rc=$?; rm -f "$_K2_DG_TMP"; _permission_note_flush "$_k2_exit_rc"' EXIT
    else
        exec 3>&1
    fi
    local off=0 size="" mtime="" next
    while :; do
        _compute_call GET /cli/app/data/get "app=$(urlencode "$app")&path=$(urlencode "$path")&offset=$off"
        next="$(printf '%s' "$_CP_BODY" | K2_DG_OUT="$tmp" python3 -c '
import base64, json, os, sys
v = json.load(sys.stdin)
data = base64.b64decode(v["data"])
out = os.environ["K2_DG_OUT"]
if out == "-":
    while data:
        n = os.write(3, data)
        data = data[n:]
else:
    with open(out, "ab") as f:
        f.write(data)
print("%d %d %d %d" % (v["size"], v["mtime"], 1 if v["eof"] else 0, v["offset"] + v["length"]))')" || { echo '{"error":"local_write","hint":"writing the piece failed"}' >&2; exit 3; }
        local s m eof nxt
        read -r s m eof nxt <<<"$next"
        if [ -z "$size" ]; then
            size="$s"; mtime="$m"
        elif [ "$s" != "$size" ] || [ "$m" != "$mtime" ]; then
            echo "{\"error\":\"data_changed\",\"hint\":\"$path changed while it was read (size $size → $s): get it again\"}" >&2
            exit 3
        fi
        [ "$eof" = 1 ] && break
        off="$nxt"
    done
    if [ "$to" = "-" ]; then
        exec 3>&-
        return 0
    fi
    mv -f "$tmp" "$to"
    trap _permission_note_flush EXIT
    if [ "$json_out" = 1 ]; then
        _compute_json K2_CJ_app="$app" K2_CJ_path="$path" K2_CJ_to="$to" K2_CJ_bytes_n="$size" K2_CJ_mtime_n="$mtime"
        return 0
    fi
    echo "saved $path ($size bytes) from App $app to $to"
}

# k2 app data rm <app-id> <path> [--recursive] [--json] (0.46.4).
_app_data_rm() {
    local app="" path="" rec="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            -r|--recursive) rec=1; shift ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_data_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) if [ -z "$app" ]; then app="$1"; elif [ -z "$path" ]; then path="$1"; else echo "usage: k2 app data rm <app-id> <path> [--recursive]" >&2; exit 2; fi; shift ;;
        esac
    done
    { [ -z "$app" ] || [ -z "$path" ]; } && { echo "usage: k2 app data rm <app-id> <path> [--recursive] [--json]" >&2; exit 2; }
    _app_rt_post /cli/app/data/rm "$(_compute_json K2_CJ_app="$app" K2_CJ_path="$path" K2_CJ_recursive_b="$rec")"
    _app_grace_done "$json_out"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
kind = {"dir": "folder", "link": "link (not what it pointed to)"}.get(v.get("removed"), v.get("removed"))
print("removed %s %s from App %s" % (kind, v.get("at"), v.get("app")))'
}

# k2 app data mv <app-id> <from> <to> [--force] [--json] (0.46.4).
_app_data_mv() {
    local app="" from="" to="" force="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            -f|--force) force=1; shift ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_data_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) if [ -z "$app" ]; then app="$1"; elif [ -z "$from" ]; then from="$1"; elif [ -z "$to" ]; then to="$1"; else echo "usage: k2 app data mv <app-id> <from> <to> [--force]" >&2; exit 2; fi; shift ;;
        esac
    done
    { [ -z "$app" ] || [ -z "$from" ] || [ -z "$to" ]; } && { echo "usage: k2 app data mv <app-id> <from> <to> [--force] [--json]" >&2; exit 2; }
    _app_rt_post /cli/app/data/mv "$(_compute_json K2_CJ_app="$app" K2_CJ_from="$from" K2_CJ_to="$to" K2_CJ_force_b="$force")"
    _app_grace_done "$json_out"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print("moved %s to %s in App %s%s" % (v.get("from"), v.get("to"), v.get("app"), " (replaced what was there)" if v.get("replaced") else ""))'
}

# k2 app data put <app-id> <path> [--to <dir>] [--json] (0.46.3): copy a file
# or folder of the App's workspace into its data folder.
_app_data_put() {
    local app="" from="" to="" json_out=0 dov=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --to) to="${2:-}"; shift 2 ;;
            --disk-override) dov="${2:-}"; shift 2 ;;
            --disk-override=*) dov="${1#--disk-override=}"; shift ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_grace_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) if [ -z "$app" ]; then app="$1"; else from="$1"; fi; shift ;;
        esac
    done
    [ -z "$app" ] || [ -z "$from" ] && { echo "usage: k2 app data put <app-id> <path> [--to <dir>]" >&2; exit 2; }
    [ -e "$from" ] && from="$(cd "$(dirname "$from")" && pwd -P)/$(basename "$from")"
    local raw
    raw="$(K2_CP_MAX_TIME=1800 _compute_send POST /cli/app/data/put "$(_compute_json K2_CJ_app="$app" K2_CJ_from="$from" K2_CJ_to="$to" K2_CJ_diskOverride="$dov")")"
    _compute_split "$raw"
    _app_grace_done "$json_out"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    _app_disk_notes
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print("copied %s file(s) from %s into %s of App %s" % (v.get("files"), v.get("from"), v.get("to"), v.get("app")))'
}

# k2 app label take|return <app-id> <label> [--json] (0.46.3).
cmd_app_label() {
    local verb="${1:-}"
    shift || true
    case "$verb" in
        take|return) ;;
        *) echo "usage: k2 app label take|return <app-id> <label> [--json]" >&2; exit 2 ;;
    esac
    local app="" label="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            -h|--help) _app_grace_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) if [ -z "$app" ]; then app="$1"; else label="$1"; fi; shift ;;
        esac
    done
    [ -z "$app" ] || [ -z "$label" ] && { echo "usage: k2 app label $verb <app-id> <label>" >&2; exit 2; }
    _app_rt_post "/cli/app/label/$verb" "$(_compute_json K2_CJ_app="$app" K2_CJ_label="$label")"
    _app_grace_done "$json_out"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
if v.get("already"):
    print("%s already serves %s" % (v.get("app"), v.get("label")))
elif v.get("returnedTo"):
    print("%s serves the legacy service again (port %s); App %s no longer has it" % (v.get("label"), v["returnedTo"].get("port"), v.get("app")))
else:
    print("%s now serves App %s (front port %s)" % (v.get("label"), v.get("app"), v.get("frontPort")))
    if v.get("note"):
        print(v["note"])'
}

# Test guests (0.46.4): k2 app user add <app> <name> --test [--role R]
# [--room H]... [--expires 24h] [--password-stdin] [--json]; k2 app user
# remove <name> --test; k2 app test-guests <app>. A person adds and
# removes; the App's own agent adds with "Let this agent manage its Apps'
# users" (else one held request with a card for the owner, exit 2), removes
# its own with that switch or "Build and run Apps", and sets its own test
# guest's password (k2 app user password <name> --test --password-stdin:
# with the switch, or once after the owner approved its request).
_app_test_guest() {
    local action="$1"
    shift
    local app="" name="" role="" expires="" json_out=0 rooms_csv="" pw_stdin=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --test) shift ;;
            --role) role="${2:-}"; shift 2 ;;
            --role=*) role="${1#--role=}"; shift ;;
            --room) rooms_csv="${rooms_csv:+$rooms_csv,}${2:-}"; shift 2 ;;
            --room=*) rooms_csv="${rooms_csv:+$rooms_csv,}${1#--room=}"; shift ;;
            --expires) expires="${2:-}"; shift 2 ;;
            --expires=*) expires="${1#--expires=}"; shift ;;
            --app) app="${2:-}"; shift 2 ;;
            --password-stdin) pw_stdin=1; shift ;;
            --json) json_out=1; shift ;;
            -h|--help)
                cat <<'EOF'
k2 app user add <app-id> <name> --test [--role R] [--room H]... [--expires 24h] [--password-stdin] [--json]
k2 app user remove <name> --test
k2 app test-guests <app-id> [--json]

  A test guest of ONE App, for testing it: it signs in only through that
  App's gateway link (POST /cli/skin/login with the App pass), never the
  box login or another App. A role only when the App already defines it;
  --room (without a role) only your own workspace's room. It ends on its
  own: 24 h by default, at most 7 days. At most 5 per App. Audited and
  noted on the workspace's Thread. K2 makes the password and shows it once
  (or --password-stdin; later: k2 app user password <name> --test
  --password-stdin).

  Who: a person; or the App's own agent with "Let this agent manage its
  Apps' users" (its test guests are tied to it). Without that switch an
  agent's add is one held request: the owner approves it from a card on
  your Thread or Settings → App Approvals (exit 2); then you set its
  password once: k2 app user password <name> --test --password-stdin.
EOF
                return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) if [ "$action" = add ] && [ -z "$app" ]; then app="$1"; else name="$1"; fi; shift ;;
        esac
    done
    if [ "$action" = add ]; then
        { [ -z "$app" ] || [ -z "$name" ]; } && { echo "usage: k2 app user add <app-id> <name> --test [--role R] [--expires 24h]" >&2; exit 2; }
        local pw="" rooms_j=""
        if [ "$pw_stdin" = 1 ]; then
            IFS= read -r pw || true
            [ -n "$pw" ] || { echo "--password-stdin: no password on stdin" >&2; exit 2; }
        fi
        if [ -n "$rooms_csv" ]; then
            rooms_j="$(K2_GR_ROOMS="$rooms_csv" python3 -c 'import json,os; print(json.dumps([r.strip() for r in os.environ["K2_GR_ROOMS"].split(",") if r.strip()]))')"
        fi
        _app_rt_post /cli/app/test-guest/add "$(_compute_json K2_CJ_app="$app" K2_CJ_username="$name" K2_CJ_role="$role" K2_CJ_expires="$expires" K2_CJ_password="$pw" K2_CJ_rooms_j="$rooms_j")"
        _app_grace_done "$json_out"
    elif [ "$action" = password ]; then
        [ -z "$name" ] && { echo "usage: k2 app user password <name> --test --password-stdin" >&2; exit 2; }
        [ "$pw_stdin" = 1 ] || { echo "usage: k2 app user password <name> --test --password-stdin (the password on stdin)" >&2; exit 2; }
        local pw=""
        IFS= read -r pw || true
        [ -n "$pw" ] || { echo "--password-stdin: no password on stdin" >&2; exit 2; }
        _app_rt_post /cli/app/test-guest/password "$(_compute_json K2_CJ_username="$name" K2_CJ_password="$pw")"
        [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
        if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
        printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print("Test guest %s: password set%s." % (v.get("username"), " (the one set after the owner approved it)" if v.get("once") else ""))'
        return 0
    else
        [ -z "$name" ] && { echo "usage: k2 app user remove <name> --test" >&2; exit 2; }
        _app_rt_post /cli/app/test-guest/rm "$(_compute_json K2_CJ_username="$name")"
    fi
    [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys, time
v = json.load(sys.stdin)
if "removed" in v:
    print("Test guest %s %s." % (v.get("username"), "removed" if v.get("removed") else "was already gone"))
    sys.exit(0)
rooms = ", ".join(v.get("rooms") or [])
print("Test guest %s of App %s (role %s%s), ends %s" % (v["username"], v["app"], v.get("role") or "none",
      ", rooms " + rooms if rooms else "", time.strftime("%Y-%m-%d %H:%M", time.localtime(v["expiresAt"]))))
if v.get("password"):
    print("password (shown once): %s" % v["password"])
if v.get("tied"):
    print("tied to your workspace: it never counts as a person for person-only cards")
print(v.get("note", ""))'
}

cmd_app_test_guests() {
    local app="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            -h|--help) _app_test_guest add --help; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    [ -n "$app" ] || { echo "usage: k2 app test-guests <app-id> [--json]" >&2; exit 2; }
    _compute_call GET /cli/app/test-guests "app=$(urlencode "$app")"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys, time
v = json.load(sys.stdin)
g = v.get("testGuests") or []
print("%s: %d test guest%s (at most %s)" % (v["app"], len(g), "" if len(g) == 1 else "s", v.get("max")))
for t in g:
    print("  %s  %s  by %s" % (t["username"], "expired" if t.get("expired") else "ends " + time.strftime("%Y-%m-%d %H:%M", time.localtime(t["expiresAt"])), t.get("createdBy")))'
}

# k2 app move-plan <service> [--app <app-id>] [--request [--reason TEXT]] [--json]
# (0.46.4): every step of moving a --cmd service onto the App runtime, what's
# done, and with --request ONE held request for all the owner's steps.
cmd_app_move_plan() {
    local svc="" app="" request=0 reason="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --app) app="${2:-}"; shift 2 ;;
            --request) request=1; shift ;;
            --reason) reason="${2:-}"; shift 2 ;;
            --json) json_out=1; shift ;;
            -h|--help)
                cat <<'EOF'
k2 app move-plan <service> [--app <app-id>] [--request [--reason TEXT]] [--json]

  Every step that moves your --cmd service onto the App runtime, in order,
  marked done / todo / blocked (waits on an earlier step) / n/a, with who
  does it (owner or agent) and the command:
    box prep, the App engine, "Build and run Apps", install, deploy,
    copy-host (the box-login users who'd be copied), the guests-rooms grant
    (with the rooms the old service hands out, when K2 can tell), data put,
    label take, every other subdomain pointing at the service's port
    (k2 app domain add), stopping the old service.
  --app names the App (default: the App that took the label, else your
  workspace's only App, else a suggested id).
  --request files ONE held request for every owner step still to do: your
  owner approves it once, in person (k2 app approve <id> outside K2, or
  Settings → App Approvals), then K2 runs them in order as the owner, each
  audited. An agent never approves it. Exit 2 = held; 0 = nothing for the
  owner to do yet.
EOF
                return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) svc="$1"; shift ;;
        esac
    done
    [ -n "$svc" ] || { echo "usage: k2 app move-plan <service> [--app <app-id>] [--request] [--json]" >&2; exit 2; }
    if [ "$request" = 1 ]; then
        _app_rt_post /cli/app/move-plan/request "$(_compute_json K2_CJ_service="$svc" K2_CJ_app="$app" K2_CJ_reason="$reason")"
        case "$_CP_STATUS" in 200|202) ;; *) _compute_fail "$_CP_STATUS" "$_CP_BODY" ;; esac
    else
        local q="service=$(urlencode "$svc")"
        [ -n "$app" ] && q="${q}&app=$(urlencode "$app")"
        _compute_call GET /cli/app/move-plan "$q"
    fi
    if [ "$json_out" = 1 ]; then
        printf '%s\n' "$_CP_BODY"
    else
        printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
p = v.get("plan") or v
s, a = p["service"], p["app"]
print("Move %s/%s (port %s) into App %s%s" % (s["workspaceHandle"], s["name"], s["port"], a["id"],
      "" if a["installed"] else " (not installed; a suggested id: --app to pick)"))
mark = {"done": "[x]", "todo": "[ ]", "blocked": "[.]", "n/a": "[-]"}
for st in p["steps"]:
    print("%s %-6s %s: %s" % (mark.get(st["state"], "[?]"), st["who"], st["title"], st["detail"]))
    if st.get("users"):
        print("             users: %s" % ", ".join(st["users"]))
    if st.get("roomHandles"):
        print("             rooms: %s" % ", ".join(st["roomHandles"]))
    if st["state"] in ("todo", "blocked") and st.get("command"):
        print("             %s" % st["command"])
print("[x] done  [ ] to do  [.] waits on an earlier step  [-] n/a")
if v.get("status") == "held":
    print("Held for the owner: request %s (one approval for %d step(s)). %s" % (v["request"]["id"], len(p.get("ownerStepsTodo") or []), v.get("hint", "")))
elif v.get("status") == "nothing_to_request":
    print(v.get("note"))
elif p.get("request"):
    print("Owner steps to do: %s. One approval for all of them: %s" % (", ".join(p["ownerStepsTodo"]), p["request"]))
elif p.get("done"):
    print("The move is done.")'
    fi
    if [ "$request" = 1 ]; then
        printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; sys.exit(2 if json.load(sys.stdin).get("status") == "held" else 0)' || exit $?
    fi
}

# k2 app domain add|rm <app-id> <subdomain> [--json], k2 app domains <app-id>
# (0.46.4): any nested label of the App's workspace serves the App, routed
# by its id (its front port), and joins K2_APP_ORIGINS at the next start.
cmd_app_domain() {
    local verb="${1:-}"
    shift || true
    case "$verb" in
        add|rm) ;;
        check|primary) cmd_app_domain_custom "$verb" "$@"; return ;;
        external) cmd_app_domain_external "$@"; return ;;
        list) cmd_app_domains "$@"; return ;;
        -h|--help|"")
            cat <<'EOF'
k2 app domain — more hostnames for an App (0.46.4; custom domains 0.46.6)

  k2 app domain add <app-id> <subdomain> [--transfer]
                                           <subdomain>.<box>.k2.dev serves the App
  k2 app domain rm <app-id> <subdomain>    back to what it served before (or removed)
  k2 app domains <app-id>                  its own label, taken labels, attached ones,
                                           custom domains (state + the DNS to set)
  <box> is this box's K2 Connect name (<box>.k2.dev).

CUSTOM DOMAINS (your own name, e.g. shop.example.com)
  k2 app domain add <app-id> shop.example.com
                                           bind it; K2 prints the DNS records to set
  k2 app domain check <app-id> shop.example.com
                                           check its DNS now (K2 also re-checks by itself)
  k2 app domain primary <app-id> shop.example.com
                                           make it K2_APP_ORIGIN (fine to run early:
                                           it applies once the name is live, at the
                                           App's next start after that)
  k2 app domain rm <app-id> shop.example.com
                                           no longer served (cert files stay)
  k2 app domain add <app-id> shop.example.com --external
                                           your own web server serves it, not K2
  k2 app domain external <app-id> shop.example.com on|off
                                           switch a bound name to your own web
                                           server (on) or to K2 (off)

  Who (every verb here): the owner, an Admin login, or the App's own agent
  with "Build and run Apps" on (an agent binds only its own workspace's
  Apps).

  YOUR OWN WEB SERVER (--external): if your own nginx, Caddy or other web
  server serves the name (not K2's :443 front), K2 never makes it live,
  so it is not one of the App's origins and the App refuses forms from
  it. Run k2 app domain add <app-id> <name> --external (also for a name
  you already added; same as k2 app domain external <app-id> <name> on),
  then k2 app restart <app-id>: from that start the App treats it as its
  own origin. Your server proxies the name to the App's front port
  (k2 app inspect <app-id>: front). K2 sets no records, runs no DNS check
  and gets no certificate for such a name. Its domain must still be
  attached (the owner runs k2 domain add <apex>). Pending in k2 domain
  list is fine: your nameservers can stay where they are. Point the name
  (A/AAAA) at this box; your server gets its own certificate. If K2's
  front holds :443, pass the name to your server: k2 front passthrough
  add <name> --to 127.0.0.1:<port>.

  A dotted name outside k2.dev is a custom domain; anything else is a
  subdomain (below).

  TWO WAYS IN, chosen per name by where its DNS points (k2 app domains
  shows both, and which one DNS shows now):
    Path B, through K2 Connect (Pro): the name points at K2's relays. No
      sudo, nothing on this box's :443; another web server may keep :443.
      Works on a Linux server, behind a router, on a Mac or Windows
      computer. Needs this box on K2 Connect with a Pro plan.
    Path A, this box's own address (free; Linux with a public IP): the
      name points here and K2's :443 front serves it.

  First, once per box and domain (the owner or an Admin):
    k2 domain add example.com attach the domain: this box may serve names
                              under it. It doesn't move your nameservers:
                              keep your DNS where it is. (If k2 domain list
                              shows it pending with k2.dev nameservers, that
                              only lets K2 write your records; ignore it.)

  Then:
    k2 front enable           Path A only: K2's :443 front (k2 front --help).
                              It puts 443/tcp on the public list itself;
                              k2 firewall says whether that list is applied.

  Path B, then:
    1. k2 app domain add <app-id> shop.example.com claims the name at K2
       Connect and prints (when Path B is open to this box; k2 app domains
       <app-id> shows them again):
         CNAME  shop.example.com              <app-id>.<box>.k2.dev
         TXT    _k2-domain.shop.example.com   "k2-verify=<token>"
       K2 Connect picks that TXT value: set exactly what K2 prints. If the
       claim doesn't go through yet, K2 prints the CNAME alone and says why;
       it tries again, and prints the TXT once K2 Connect has given it.
       At an apex (example.com itself), which can't hold a CNAME: ALIAS,
       ANAME or CNAME flattening to <app-id>.<box>.k2.dev if your DNS host
       has it (Cloudflare, DNSimple, Route 53 alias...). If it has none, use
       www and a redirect at your DNS host. K2 gives no relay addresses for
       A records: they aren't promised to stay.
    2. Set them, DNS-only (no proxy; Cloudflare: grey cloud), and wait for
       state live: K2 checks every 10 minutes, or k2 app domain check
       <app-id> shop.example.com now. K2 Connect also checks the TXT itself.
       The tunnel restarts once when a name joins or leaves (a few seconds'
       blip for everything on the tunnel), and K2 gets the certificate
       through the relay.
    3. Primary and restart as in step 4 below.
    The box holds the key and ends TLS: the relay only passes the encrypted
    stream through by name. At most 20 names per box through K2's relays
    (100 custom names per box on both paths together; K2 Connect also caps
    the account). Not Pro: the refusal has the upgrade link and says so.
    K2 Connect decides whether it routes a name: its release (after 7 days
    of failing its own check) moves the name to failing, and a failed check
    on this box alone only says so.

  Path A, then:
    1. k2 app domain add <app-id> shop.example.com. It prints the records,
       with this box's real addresses (k2 app domains <app-id> shows them
       again):
         A      shop.example.com              <this box's public IPv4>
         AAAA   shop.example.com              <its public IPv6, if it has one>
         TXT    _k2-domain.shop.example.com   "k2-verify=<token>"
    2. Set exactly those at your DNS host, DNS-only (no proxy; Cloudflare:
       grey cloud). Remove any other A or AAAA at that name, and any CNAME
       there (a name can't have both). Keep the TXT: K2 re-checks it.
       Only if your nameservers already point at k2.dev does K2 set the
       TXT itself (an agent needs DNS write, dns_manage, for that); you
       still set the A/AAAA.
    3. Wait for state live (k2 app domains <app-id>). Until then K2
       checks every 10 minutes; k2 app domain check <app-id>
       shop.example.com checks now. Once DNS passes, K2 gets the
       certificate itself through :443 (no :80 needed).
    4. Once it is live: k2 app domain primary <app-id> shop.example.com,
       then k2 app restart <app-id>. The App reads its origins only at
       start, so restart after the name is live even if you ran primary
       earlier. The App's k2.dev names stay.

  States: waiting_dns -> verifying -> issuing -> live. failing: a live
  name whose daily DNS check failed 7 days in a row (DNS points elsewhere,
  the TXT is gone, or a proxy is on). K2 then stops serving and renewing
  it; until day 7 it keeps serving and k2 app domains says what's wrong.
  To recover: fix the records, then k2 app domain check <app-id> <name>.
  external: your own web server serves it (--external); K2 leaves it be.

  Where this works: Path A on a Linux server with its own public IP (a
  datacentre box). Path B anywhere on K2 Connect with Pro: a Mac or Windows
  computer has only Path B (without Pro, add prints no records and says
  so). A Linux server behind a home or office router: Path B is the way
  (your address stays hidden); without Pro, if you forward the router's
  port 443 to it, the Path A records still work (shown under "Advanced:
  I've forwarded port 443"). Another web server already on :443: Path B,
  or k2 front --help, or let it keep serving the name (--external).

SUBDOMAINS (k2 app domain add <app-id> <subdomain>)
  Any nested label your workspace holds, or one nobody holds yet (a label
  you pointed at a port with k2 publish subdomain create, say). K2 points
  it at the App's front port, so deploys and rollbacks never touch it. The
  App must be live. It is in K2_APP_ORIGINS from the App's next start
  (k2 app restart <app-id>). Your --cmd service's own label is
  k2 app label take; another workspace's label is refused. A hostname that
  already serves something and that no workspace holds is repointed only
  by the owner, deliberately: --transfer (never by an agent).
EOF
            [ -n "$verb" ] && return 0 || exit 2 ;;
        *) echo "usage: k2 app domain add|rm <app-id> <subdomain> [--json]" >&2; exit 2 ;;
    esac
    local app="" label="" json_out=0 transfer="" external=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            --transfer) transfer=1; shift ;;
            --external) external=1; shift ;;
            -h|--help) cmd_app_domain --help; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) if [ -z "$app" ]; then app="$1"; else label="$1"; fi; shift ;;
        esac
    done
    { [ -z "$app" ] || [ -z "$label" ]; } && { echo "usage: k2 app domain $verb <app-id> <subdomain>" >&2; exit 2; }
    [ -n "$external" ] && [ "$verb" != add ] && { echo "--external goes with k2 app domain add (to switch a bound name: k2 app domain external <app-id> <host> on|off)" >&2; exit 2; }
    _app_rt_post "/cli/app/domain/$verb" "$(_compute_json K2_CJ_app="$app" K2_CJ_label="$label" K2_CJ_transfer_b="$transfer" K2_CJ_external_b="$external")"
    _app_grace_done "$json_out"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    if printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; d = json.load(sys.stdin); sys.exit(0 if ("custom" in d or "removed" in d) else 1)'; then
        printf '%s' "$_CP_BODY" | _app_custom_print
        return 0
    fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
if v.get("already"):
    print("%s already serves %s%s" % (v.get("app"), v.get("label"), (" (" + v["note"] + ")") if v.get("note") else ""))
elif v.get("now"):
    print("%s no longer serves %s (now: %s)" % (v.get("app"), v.get("label"), v.get("now")))
else:
    print("%s now serves App %s (front port %s)" % (v.get("label"), v.get("app"), v.get("frontPort")))
    if v.get("note"):
        print(v["note"])'
}

cmd_app_domains() {
    local app="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            -h|--help) cmd_app_domain --help; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    [ -n "$app" ] || { echo "usage: k2 app domains <app-id> [--json]" >&2; exit 2; }
    _compute_call GET /cli/app/domains "app=$(urlencode "$app")"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
def host(r):
    return r.get("host") or (r.get("label") + ".<box>.k2.dev")
print("%s  (its own label)" % host(v["own"]))
for t in v.get("taken") or []:
    print("%s  (taken from --cmd service %s; k2 app label return to undo)" % (host(t), t.get("from")))
for a in v.get("attached") or []:
    print("%s  (attached by %s%s)" % (host(a), a.get("addedBy"), ("; was " + a["previousTarget"]) if a.get("previousTarget") else ""))
for c in v.get("custom") or []:
    st = c.get("state")
    print("%s   %s%s" % (c.get("hostname"), "external (served by your own server)" if st == "external" else st, "  primary" if c.get("primary") else ""))
    print("  %s" % c.get("words"))
    if c.get("lastError"):
        print("  last check: %s" % c["lastError"])
    if c.get("path"):
        print("  DNS shows: %s" % ("Path B, through K2 Connect" if c["path"] == "connect" else "Path A, this box"))
    pr = ((c.get("setup") or {}).get("pathB") or {}).get("proxy") or {}
    if c.get("path") == "connect" and pr.get("state") == "waiting":
        print("  %s" % pr.get("words"))
    rs = (c.get("relay") or {}).get("state")
    if rs:
        words = {"claimed": "claimed, waiting for its own check of the TXT", "verified": "verified: its relays route this name here", "released": "released: its relays no longer route this name", "refused": "refused (see last check)"}
        print("  K2 Connect: %s%s" % (words.get(rs, rs), " (" + c["relay"]["error"] + ")" if c["relay"].get("error") and c["relay"]["error"] != c.get("lastError") else ""))
    su = c.get("setup") or {}
    if st not in ("live", "external"):
        if su.get("message"):
            print("  %s" % su["message"])
        if su.get("records") == "advanced":
            print("  %s" % (su.get("advanced") or "Advanced:"))
        if su.get("records") != "none":
            for r in c.get("records") or []:
                print("    %-5s %-40s %s" % (r.get("type"), r.get("name"), r.get("value")))
    if c.get("next"):
        print("  %s" % c["next"])
    if c.get("ownServer"):
        print("  %s" % c["ownServer"])
pb = v.get("pathB") or {}
if pb.get("available"):
    print("Path B (through K2 Connect): %s (%s custom names at most per box, both paths)" % (pb.get("meter"), v.get("maxCustomNames")))
elif pb.get("why"):
    print("Path B (through K2 Connect): not open here: %s" % pb["why"])'
}

# k2 app domain external <app-id> <host> on|off (0.46.6): your own web
# server serves the name (on: an origin from the App's next start; K2
# makes no check or certificate for it), or K2's front does (off).
cmd_app_domain_external() {
    local app="" host="" mode="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            -h|--help) cmd_app_domain --help; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) if [ -z "$app" ]; then app="$1"; elif [ -z "$host" ]; then host="$1"; else mode="$1"; fi; shift ;;
        esac
    done
    local on
    case "$mode" in
        on) on=1 ;;
        off) on=0 ;;
        *) echo "usage: k2 app domain external <app-id> <host> on|off" >&2; exit 2 ;;
    esac
    [ -n "$app" ] && [ -n "$host" ] || { echo "usage: k2 app domain external <app-id> <host> on|off" >&2; exit 2; }
    _app_rt_post /cli/app/domain/external "$(_compute_json K2_CJ_app="$app" K2_CJ_host="$host" K2_CJ_external_b="$on")"
    _app_grace_done "$json_out"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | _app_custom_print
}

# k2 app domain check|primary <app-id> <host> (0.46.6): re-run a custom
# domain's DNS + TXT checks now; make it the App's primary origin.
cmd_app_domain_custom() {
    local verb="$1"
    shift
    local app="" host="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            -h|--help) cmd_app_domain --help; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) if [ -z "$app" ]; then app="$1"; else host="$1"; fi; shift ;;
        esac
    done
    { [ -z "$app" ] || [ -z "$host" ]; } && { echo "usage: k2 app domain $verb <app-id> <host>" >&2; exit 2; }
    _app_rt_post "/cli/app/domain/$verb" "$(_compute_json K2_CJ_app="$app" K2_CJ_host="$host")"
    _app_grace_done "$json_out"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | _app_custom_print
}

# Print one custom domain answer (add / check / primary).
_app_custom_print() {
    python3 -c '
import json, sys
v = json.load(sys.stdin)
if v.get("removed"):
    print("%s no longer serves %s (%s)" % (v.get("app"), v["removed"], v.get("note") or ""))
    sys.exit(0)
if v.get("primary"):
    print("primary: %s" % v["primary"])
    print(v.get("note") or "")
    sys.exit(0)
c = v.get("custom") or {}
st = c.get("state")
already = ""
if v.get("already"):
    already = "  (no change)" if "external" in v else "  (already bound)"
print("%s   %s%s" % (c.get("hostname"), "external (served by your own server)" if st == "external" else st, already))
print("  %s" % c.get("words"))
if c.get("lastError"):
    print("  last check: %s" % c["lastError"])
rs = (c.get("relay") or {}).get("state")
if rs:
    words = {"claimed": "claimed, waiting for its own check of the TXT", "verified": "verified: its relays route this name here", "released": "released: its relays no longer route this name", "refused": "refused (see last check)"}
    print("  K2 Connect: %s" % words.get(rs, rs))
t = v.get("txt")
if isinstance(t, dict) and t.get("planted"):
    print("  K2 set the TXT record in your k2.dev zone")
elif isinstance(t, dict) and t.get("why"):
    print("  TXT not set by K2: %s" % t["why"])
su = c.get("setup") or {}
if su.get("message") and st not in ("live", "external"):
    print("  %s" % su["message"])
if st not in ("live", "external") and su.get("records") != "none":
    if su.get("records") == "advanced":
        print("  %s" % (su.get("advanced") or "Advanced:"))
    for r in c.get("records") or []:
        print("    %-5s %-40s %s" % (r.get("type"), r.get("name"), r.get("value")))
        if r.get("type") == "ALIAS" and r.get("note"):
            print("          %s" % r["note"])
if c.get("next"):
    print("  %s" % c["next"])
if c.get("ownServer"):
    print("  %s" % c["ownServer"])
if v.get("note") and not v.get("already"):
    print("  %s" % v["note"])'
}

cmd_app_warnings() {
    local app="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            -h|--help) _app_grace_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    local q=""
    [ -n "$app" ] && q="app=$(urlencode "$app")"
    _compute_call GET /cli/app/warnings "$q"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
reports = v.get("services") if "services" in v else [v]
if not reports:
    print("No server Apps (--cmd services) to check.")
tracing = v.get("ownerTokenAttribution") or next((r.get("ownerTokenAttribution") for r in reports if r.get("ownerTokenAttribution")), None)
for r in reports:
    a = r.get("app", {})
    mode = a.get("envMode") or "-"
    if a.get("cleanMode"):
        mode = "clean mode"
    # `cutover` is null: no release is set (Rosson 2026-10-26).
    if r.get("readyForCutover"):
        status = "ready to move to an App"
    else:
        status = "%d thing(s) break when it moves to an App" % r.get("breaks", 0)
    print("%s  [%s]  %s" % (a.get("handle") or a.get("name"), mode, status))
    for w in r.get("warnings", []):
        sev = w.get("severity")
        print("  %-8s %s" % ({"breaks": "BREAKS", "expected": "expected"}.get(sev, "check"), w.get("message")))
        if w.get("fix"):
            print("           fix: %s" % w["fix"])
    sc = r.get("scan") or {}
    if sc.get("truncated"):
        print("  (the code scan stopped early: %s files)" % sc.get("filesScanned"))
if tracing and tracing != "on":
    # Linux only for now (Rosson 2026-10-16): say so, never stay silent.
    print("owner-token tracing: %s (Linux only for now; the code scan still names reads of ~/.k2/daemon.token)" % tracing)
'
}

cmd_app_clean_mode() {
    local app="" onoff="" restart="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --restart) restart=1; shift ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_grace_usage; return 0 ;;
            on|off) onoff="$1"; shift ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    { [ -z "$app" ] || [ -z "$onoff" ]; } && { echo "usage: k2 app clean-mode <app> on|off [--restart]" >&2; exit 2; }
    _app_rt_post /cli/app/clean-mode "$(_compute_json K2_CJ_app="$app" K2_CJ_on="$onoff" K2_CJ_restart_b="${restart:-0}")"
    _app_grace_done "$json_out"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print("%s: clean mode %s (env %s); %s" % (v["app"].get("handle"), "on" if v.get("cleanMode") else "off", v.get("envMode"), v.get("restart")))
if v.get("ownerTokenAttributionNote"):
    print("owner-token tracing: %s (%s)" % (v.get("ownerTokenAttribution"), v["ownerTokenAttributionNote"]))'
}

# k2 app key mint|revoke …
cmd_app_key() {
    local sub="${1:-}"
    shift || true
    case "$sub" in
        mint) cmd_app_key_mint "$@" ;;
        revoke|rm)
            local id="${1:-}"
            [ -n "$id" ] || { echo "usage: k2 app key revoke <key-id>" >&2; exit 2; }
            _app_rt_post /cli/app/key/revoke "$(_compute_json K2_CJ_id="$id")"
            _app_grace_done 0
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print("%s %s (%s)" % ("revoked" if v.get("revoked") else "already revoked:", v.get("id"), v["app"].get("handle")))'
            ;;
        list|ls) cmd_app_keys "$@" ;;
        *) _app_grace_usage >&2; exit 2 ;;
    esac
}

cmd_app_key_mint() {
    local app="" scopes="" rooms="" roles="" label="" show=0 json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --scopes|--scope) scopes="${2:-}"; shift 2 ;;
            --rooms|--room) rooms="${2:-}"; shift 2 ;;
            --roles|--role) roles="${2:-}"; shift 2 ;;
            --label) label="${2:-}"; shift 2 ;;
            --show) show=1; shift ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_grace_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    { [ -z "$app" ] || [ -z "$scopes" ]; } && { echo "usage: k2 app key mint <app> --scopes users,rooms,fetch,push [--rooms a,b] [--roles r] [--label L] [--show]" >&2; exit 2; }
    local deliver=vault
    [ "$show" = 1 ] && deliver=show
    local body
    body="$(K2_AK_APP="$app" K2_AK_SCOPES="$scopes" K2_AK_ROOMS="$rooms" K2_AK_ROLES="$roles" K2_AK_LABEL="$label" K2_AK_DELIVER="$deliver" python3 -c '
import json, os
sp = lambda k: [x.strip() for x in os.environ.get(k, "").split(",") if x.strip()]
b = {"app": os.environ["K2_AK_APP"], "scopes": sp("K2_AK_SCOPES"), "rooms": sp("K2_AK_ROOMS"), "roles": sp("K2_AK_ROLES"),
     "deliver": os.environ["K2_AK_DELIVER"]}
if os.environ.get("K2_AK_LABEL"):
    b["label"] = os.environ["K2_AK_LABEL"]
print(json.dumps(b))')"
    _app_rt_post /cli/app/key/mint "$body"
    if [ "$_CP_STATUS" = "403" ] && [[ "$_CP_BODY" == *'"vault_confirm_required"'* ]] && ! _compute_in_session; then
        # The owner, outside K2: confirm in person, bound to this App.
        local svc tok
        svc="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print((json.load(sys.stdin).get("error") or {}).get("serviceId") or "")')"
        [ -n "$svc" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
        tok="$(_vault_confirm_token app-key-mint "$svc" "App key for $app")" || exit 3
        body="$(printf '%s' "$body" | K2_AK_TOK="$tok" python3 -c 'import json,os,sys; b=json.load(sys.stdin); b["confirmToken"]=os.environ["K2_AK_TOK"]; print(json.dumps(b))')"
        tok=""
        _app_rt_post /cli/app/key/mint "$body"
    fi
    _app_grace_done "$json_out"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print("App key %s for %s: scopes %s" % (v.get("id"), v["app"].get("handle"), ",".join(v.get("scopes") or [])))
print("  calls: %s (Authorization: Bearer <key>)" % v.get("routes"))
d = v.get("delivered")
if d:
    print("  in the vault as %s; the App reads $K2_SECRETS_DIR/%s" % (d.get("vaultSecret"), d.get("file")))
    print("  %s" % d.get("note"))
if v.get("key"):
    print("  key (shown once): %s" % v["key"])'
}

cmd_app_keys() {
    local app="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            -h|--help) _app_grace_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    local q=""
    [ -n "$app" ] && q="app=$(urlencode "$app")"
    _compute_call GET /cli/app/keys "$q"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
ks = v.get("keys") or []
if not ks:
    print("No App keys."); sys.exit(0)
for k in ks:
    print("%s  %-8s %-24s scopes=%s rooms=%s" % (k["id"], k["state"], (k.get("label") or "-")[:24], ",".join(k.get("scopes") or []), ",".join(k.get("rooms") or []) or "-"))'
}

# k2 app pairing … (prd-app-phone-pairing-v1, 0.46.5): a person signed in to
# a custom App on the web pairs the App's own phone app by QR, no password
# on the phone. The owner names the phone apps here; K2 serves their link
# files on the App's own k2.dev labels and the App's web page shows the QR
# (pairing.start). Nothing here prints a code.
