
# k2 app storage … (prd-app-supabase-compat-v1 §10, §11.3; lane D): an App's
# Supabase-compatible /storage/v1 door, its buckets and the import from a
# Supabase project. `k2 app data` is the App's /data folder, a different thing.

_app_storage_usage() {
    cat <<'EOF'
k2 app storage: an App's Supabase-compatible file storage (/storage/v1 on the App's own host).

  k2 app storage status <app-id> [--json]
      The door (on while the Supabase-compatible layer is on), its database,
      the buckets, the last import, and warnings (public buckets).
  k2 app storage enable <app-id> [--json]
      Put the storage schema (storage.buckets, storage.objects,
      storage.foldername() …) into the App's compat database. Needs
      `k2 app compat on <app-id> --db <db>` first. Safe to run again.
  k2 app storage buckets <app-id> [--json]
  k2 app storage bucket create|update|rm <app-id> <bucket> [--public|--private]
                        [--file-size-limit 50MB] [--mime image/* …] [--json]
      A public bucket answers /storage/v1/object/public/<bucket>/<path> to
      anyone with the URL: K2 says so when you make one.
  k2 app storage ls <app-id> <bucket> [<prefix>] [--limit N] [--json]
  k2 app storage import <app-id> --from https://<ref>.supabase.co --key-secret <name>
                        [--bucket <id>]… [--concurrency N] [--json]
      Copy every bucket and object from a Supabase project, each object
      checked by size and MD5, resumable (run it again: only what changed
      moves). <name> is a vault secret in the App's workspace holding the
      project's service-role key, with the project's host allowed for it
      (k2 vault hosts <name> --add <ref>.supabase.co). Runs in the daemon;
      watch it with import-status. Policies on storage.objects come with
      the database dump (k2 db import supabase).
  k2 app storage import-status <app-id> [--json]
  k2 app storage verify <app-id> [--from <url> --key-secret <name>] [--json]
      Re-hash every object's bytes against what the import recorded, check
      every row has its bytes, list bytes no row names (after a database
      restore to an earlier moment), and (with --from) that nothing at the
      source is missing here. Exit 1 when a row lacks its bytes or a hash
      is off.
  k2 app storage key rotate <app-id> --yes [--json]
      A new signing key: every signed URL handed out before stops at once.

Who: the owner, an Admin, or the App's agent with "Build and run Apps".
More: k2 study app-storage
EOF
}

cmd_app_storage() {
    local sub="${1:-}"
    shift || true
    case "$sub" in
        status) _app_storage_get status /cli/app/storage/status "$@" ;;
        enable) _app_storage_enable "$@" ;;
        buckets) _app_storage_get buckets /cli/app/storage/buckets "$@" ;;
        bucket) _app_storage_bucket "$@" ;;
        ls|list) _app_storage_ls "$@" ;;
        import) _app_storage_import "$@" ;;
        import-status) _app_storage_get import-status /cli/app/storage/import/status "$@" ;;
        verify) _app_storage_verify "$@" ;;
        key) _app_storage_key "$@" ;;
        ""|-h|--help|help) _app_storage_usage ;;
        *) _app_storage_bad "k2 app storage status|enable|buckets|bucket|ls|import|import-status|verify|key …" ;;
    esac
}

# A usage error, the shape compat and cron use: {"error":"usage","hint":…}
# on stderr, exit 1, nothing sent.
_app_storage_bad() {
    printf '{"error":"usage","hint":%s}\n' "$(printf '%s' "$1 (k2 app storage --help)" | python3 -c 'import json,sys; print(json.dumps(sys.stdin.read()))')" >&2
    exit 1
}

# Print a storage answer: --json as is, else a short readable form.
_app_storage_print() {
    local json_out="$1"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
def human(n):
    if n is None:
        return "-"
    n = float(n)
    for u in ("B", "KiB", "MiB", "GiB"):
        if n < 1024 or u == "GiB":
            return ("%d %s" % (n, u)) if u == "B" else ("%.1f %s" % (n, u))
        n /= 1024.0
if "on" in v:
    if not v["on"]:
        print("storage is off for %s: %s" % (v.get("app"), v.get("hint")))
    else:
        print("storage for %s: database %s, schema %s, anon lane %s" % (v["app"], v["database"], "in" if v.get("schema") else "MISSING", "on" if v.get("anon") else "off"))
        for b in v.get("buckets") or []:
            print("  bucket %-24s %s  limit %s" % (b["id"], "PUBLIC " if b.get("public") else "private", human(b.get("file_size_limit"))))
    imp = v.get("import")
    if imp:
        print("last import from %s: %s (%d copied, %d skipped, %d failed of %d)" % (imp["from"], imp["status"], imp["copied"], imp["skipped"], imp["failed"], imp["planned"]))
    for w in v.get("warnings") or []:
        print("warning: %s" % w)
elif "buckets" in v:
    for b in v["buckets"]:
        print("%-24s %s  limit %s  types %s" % (b["id"], "PUBLIC " if b.get("public") else "private", human(b.get("file_size_limit")), ",".join(b.get("allowed_mime_types") or []) or "any"))
    if not v["buckets"]:
        print("no buckets")
elif "entries" in v:
    for e in v["entries"]:
        if e.get("id") is None:
            print("  %s/" % e["name"])
        else:
            md = e.get("metadata") or {}
            print("  %-48s %10s  %s" % (e["name"], human(md.get("size")), md.get("mimetype") or ""))
    if not v["entries"]:
        print("(empty)")
elif "state" in v:
    s = v.get("state")
    if not s:
        print("no import has run for %s" % v.get("app"))
    else:
        print("%s import from %s: %s" % ("running:" if v.get("running") else "last", s["from"], s["status"]))
        print("  buckets %d, objects %d: %d copied (%s), %d skipped (unchanged), %d failed" % (s["buckets"], s["planned"], s["copied"], human(s["bytesCopied"]), s["skipped"], s["failed"]))
        for e in s.get("errors") or []:
            print("  error: %s" % e)
elif "verify" in v:
    r = v["verify"]
    print("%s: %d objects checked, %d ok" % ("clean" if v["clean"] else "NOT CLEAN", r["checked"], r["ok"]))
    for b in r.get("bad") or []:
        print("  bad: %s" % b)
    for m in r.get("missing") or []:
        print("  missing here: %s" % m)
    for o in r.get("retired") or []:
        print("  retired, deleted after the grace: %s" % o)
    for o in r.get("orphans") or []:
        print("  bytes no row names (kept): %s" % o)
else:
    for k in ("warning", "note"):
        if v.get(k):
            print("%s: %s" % (k, v[k]))
    print(json.dumps({k: x for k, x in v.items() if k not in ("warning", "note")}))
'
}

_app_storage_get() {
    local verb="$1" path="$2" app="" json_out=0
    shift 2
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            -h|--help) _app_storage_usage; return 0 ;;
            -*) _app_storage_bad "unknown flag $1" ;;
            *) if [ -z "$app" ]; then app="$1"; else _app_storage_bad "one App at a time: k2 app storage $verb <app-id> (got $1 too)"; fi; shift ;;
        esac
    done
    [ -n "$app" ] || _app_storage_bad "name the App: k2 app storage $verb <app-id> [--json]"
    _compute_call GET "$path" "app=$(urlencode "$app")"
    _app_storage_print "$json_out"
}

_app_storage_enable() {
    local app="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            -h|--help) _app_storage_usage; return 0 ;;
            -*) _app_storage_bad "unknown flag $1" ;;
            *) app="$1"; shift ;;
        esac
    done
    [ -n "$app" ] || _app_storage_bad "name the App: k2 app storage enable <app-id> [--json]"
    _app_rt_post /cli/app/storage/enable "$(_compute_json K2_CJ_app="$app")"
    _app_grace_done "$json_out"
    _app_storage_print "$json_out"
}

_app_storage_bucket() {
    local op="${1:-}" app="" bucket="" public="" limit="" json_out=0 mimes=()
    shift || true
    case "$op" in
        create|update|rm) ;;
        ""|-h|--help) _app_storage_usage; return 0 ;;
        *) _app_storage_bad "k2 app storage bucket create|update|rm <app-id> <bucket> …" ;;
    esac
    while [ $# -gt 0 ]; do
        case "$1" in
            --public) public=true; shift ;;
            --private) public=false; shift ;;
            --file-size-limit) _need_value "$@"; limit="${2:-}"; shift 2 ;;
            --file-size-limit=*) limit="${1#*=}"; shift ;;
            --mime) _need_value "$@"; mimes+=("${2:-}"); shift 2 ;;
            --mime=*) mimes+=("${1#*=}"); shift ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_storage_usage; return 0 ;;
            -*) _app_storage_bad "unknown flag $1" ;;
            *) if [ -z "$app" ]; then app="$1"; elif [ -z "$bucket" ]; then bucket="$1"; else _app_storage_bad "unexpected '$1': k2 app storage bucket $op <app-id> <bucket>"; fi; shift ;;
        esac
    done
    { [ -n "$app" ] && [ -n "$bucket" ]; } || _app_storage_bad "k2 app storage bucket $op <app-id> <bucket> [--public|--private] [--file-size-limit 50MB] [--mime TYPE]…"
    local mimes_j=""
    if [ "${#mimes[@]}" -gt 0 ]; then
        mimes_j="$(printf '%s\n' "${mimes[@]}" | python3 -c 'import json,sys; print(json.dumps([l.strip() for l in sys.stdin if l.strip()]))')"
    fi
    _app_rt_post /cli/app/storage/bucket "$(_compute_json K2_CJ_app="$app" K2_CJ_op="$op" K2_CJ_id="$bucket" K2_CJ_public_j="$public" K2_CJ_fileSizeLimit="$limit" K2_CJ_allowedMimeTypes_j="$mimes_j")"
    _app_grace_done "$json_out"
    _app_storage_print "$json_out"
}

_app_storage_ls() {
    local app="" bucket="" prefix="" limit="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --limit) _need_value "$@"; limit="${2:-}"; shift 2 ;;
            --limit=*) limit="${1#*=}"; shift ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_storage_usage; return 0 ;;
            -*) _app_storage_bad "unknown flag $1" ;;
            *) if [ -z "$app" ]; then app="$1"; elif [ -z "$bucket" ]; then bucket="$1"; elif [ -z "$prefix" ]; then prefix="$1"; else _app_storage_bad "unexpected '$1': k2 app storage ls <app-id> <bucket> [<prefix>]"; fi; shift ;;
        esac
    done
    { [ -n "$app" ] && [ -n "$bucket" ]; } || _app_storage_bad "k2 app storage ls <app-id> <bucket> [<prefix>] [--limit N] [--json]"
    _compute_call GET /cli/app/storage/ls "app=$(urlencode "$app")&bucket=$(urlencode "$bucket")&prefix=$(urlencode "$prefix")&limit=$(urlencode "$limit")"
    _app_storage_print "$json_out"
}

_app_storage_import() {
    local app="" from="" secret="" conc="" json_out=0 buckets=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --from) _need_value "$@"; from="${2:-}"; shift 2 ;;
            --from=*) from="${1#*=}"; shift ;;
            --key-secret) _need_value "$@"; secret="${2:-}"; shift 2 ;;
            --key-secret=*) secret="${1#*=}"; shift ;;
            --bucket) _need_value "$@"; buckets+=("${2:-}"); shift 2 ;;
            --bucket=*) buckets+=("${1#*=}"); shift ;;
            --concurrency) _need_value "$@"; conc="${2:-}"; shift 2 ;;
            --concurrency=*) conc="${1#*=}"; shift ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_storage_usage; return 0 ;;
            -*) _app_storage_bad "unknown flag $1" ;;
            *) app="$1"; shift ;;
        esac
    done
    { [ -n "$app" ] && [ -n "$from" ] && [ -n "$secret" ]; } || _app_storage_bad "k2 app storage import <app-id> --from https://<ref>.supabase.co --key-secret <name> [--bucket <id>]… [--concurrency N] [--json]"
    # A number goes into the body as a number: anything else is refused here,
    # before the body is built (it would fail there and send nothing useful).
    case "$conc" in
        "") ;;
        *[!0-9]*|0*) _app_storage_bad "--concurrency takes a whole number from 1, like 8 (got '$conc')" ;;
    esac
    local buckets_j=""
    if [ "${#buckets[@]}" -gt 0 ]; then
        buckets_j="$(printf '%s\n' "${buckets[@]}" | python3 -c 'import json,sys; print(json.dumps([l.strip() for l in sys.stdin if l.strip()]))')"
    fi
    _app_rt_post /cli/app/storage/import "$(_compute_json K2_CJ_app="$app" K2_CJ_from="$from" K2_CJ_keySecret="$secret" K2_CJ_buckets_j="$buckets_j" K2_CJ_concurrency_n="$conc")"
    _app_grace_done "$json_out"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    echo "import started for $app from $from; watch it: k2 app storage import-status $app"
}

_app_storage_verify() {
    local app="" from="" secret="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --from) _need_value "$@"; from="${2:-}"; shift 2 ;;
            --from=*) from="${1#*=}"; shift ;;
            --key-secret) _need_value "$@"; secret="${2:-}"; shift 2 ;;
            --key-secret=*) secret="${1#*=}"; shift ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_storage_usage; return 0 ;;
            -*) _app_storage_bad "unknown flag $1" ;;
            *) app="$1"; shift ;;
        esac
    done
    [ -n "$app" ] || _app_storage_bad "name the App: k2 app storage verify <app-id> [--from <url> --key-secret <name>] [--json]"
    _app_rt_post /cli/app/storage/verify "$(_compute_json K2_CJ_app="$app" K2_CJ_from="$from" K2_CJ_keySecret="$secret")"
    _app_grace_done "$json_out"
    _app_storage_print "$json_out"
    printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; sys.exit(0 if json.load(sys.stdin).get("clean") else 1)'
}

_app_storage_key() {
    local sub="${1:-}" app="" yes="" json_out=0
    shift || true
    [ "$sub" = "rotate" ] || _app_storage_bad "k2 app storage key rotate <app-id> --yes"
    while [ $# -gt 0 ]; do
        case "$1" in
            --yes|-y) yes=1; shift ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_storage_usage; return 0 ;;
            -*) _app_storage_bad "unknown flag $1" ;;
            *) app="$1"; shift ;;
        esac
    done
    [ -n "$app" ] || _app_storage_bad "name the App: k2 app storage key rotate <app-id> --yes"
    _app_rt_post /cli/app/storage/key/rotate "$(_compute_json K2_CJ_app="$app" K2_CJ_yes_b="$yes")"
    _app_grace_done "$json_out"
    _app_storage_print "$json_out"
}
