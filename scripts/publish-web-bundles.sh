#!/usr/bin/env bash
# publish-web-bundles.sh
# ---------------------------------------------------------------------------
# Publish the versioned hosted-web SPA to Cloudflare R2 (PRD phase 3).
#
# Contract (Ops handoff + PRD §3 / §7.0):
#   endpoint : https://bd46c5a3e2afd37fa4fb22064c6fd3b6.r2.cloudflarestorage.com
#              (or https://${R2_ACCOUNT_ID}.r2.cloudflarestorage.com)
#   bucket   : k2-web-bundles
#   layout   : app/<ver>/index.html + content-hashed assets under app/<ver>/
#   cache    : public, max-age=31536000, immutable  (object metadata at upload)
#
# The edge loader (web/loader/loader.js) HEADs /app/<ver>/index.html then
# navigates to /app/<ver>/ same-origin. CI must write exactly that prefix.
#
# Credentials (never commit secrets):
#   Prefer ~/.config/cloudflare/r2.env (chmod 600), else process env:
#     R2_ACCOUNT_ID
#     R2_ACCESS_KEY_ID
#     R2_SECRET_ACCESS_KEY
#     optional: R2_ENDPOINT, R2_S3_TOKEN_VALUE, R2_BUCKET
#
# CI / GitHub Actions secrets (same names):
#     R2_ACCOUNT_ID, R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY
#     optional: R2_ENDPOINT, R2_BUCKET
#
# Usage:
#   bash scripts/publish-web-bundles.sh [version] [--dry-run] [--prune]
#                                       [--skip-if-no-creds]
#
#   version               default: package.json "version"
#   --dry-run             print the aws/boto commands; no network writes
#   --prune               after (or instead of only-upload when combined)
#                         delete app/<ver>/ prefixes strictly below the floor
#   --skip-if-no-creds    exit 0 with a loud warning if creds missing
#                         (used by release.sh so desktop release is not blocked)
#
# Env:
#   K2_WEB_BUNDLE_MIN_VERSION   prune floor (default: 0.40.0 = loader
#                               MIN_SUPPORT_VERSION in web/loader/loader.js)
#   R2_BUCKET                   default k2-web-bundles
#   R2_ENDPOINT                 override S3 endpoint URL
#   R2_UPLOAD_RETRIES           TLS/handshake retries (default 3)
#
# Install aws CLI if missing:
#   macOS:  brew install awscli
#   Linux:  https://docs.aws.amazon.com/cli/latest/userguide/getting-started-install.html
# Optional fallback when aws is absent: Python 3 + boto3
#   pip3 install --user boto3
# ---------------------------------------------------------------------------
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

DEFAULT_BUCKET="k2-web-bundles"
DEFAULT_ENDPOINT_HOST="bd46c5a3e2afd37fa4fb22064c6fd3b6.r2.cloudflarestorage.com"
# Keep in lockstep with web/loader/loader.js MIN_SUPPORT_VERSION
DEFAULT_MIN_VERSION="0.40.0"
CACHE_CONTROL="public, max-age=31536000, immutable"
R2_ENV_FILE="${R2_ENV_FILE:-$HOME/.config/cloudflare/r2.env}"

# ── args ──────────────────────────────────────────────────────────────────
VERSION=""
DRY_RUN=0
DO_PRUNE=0
SKIP_IF_NO_CREDS=0

for a in "$@"; do
  case "$a" in
    --dry-run) DRY_RUN=1 ;;
    --prune) DO_PRUNE=1 ;;
    --skip-if-no-creds) SKIP_IF_NO_CREDS=1 ;;
    -h|--help)
      sed -n '2,50p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    -*)
      echo "ERROR: unknown flag: $a" >&2
      exit 2
      ;;
    *)
      if [ -n "$VERSION" ]; then
        echo "ERROR: unexpected extra arg: $a" >&2
        exit 2
      fi
      VERSION="$a"
      ;;
  esac
done

if [ -z "$VERSION" ]; then
  if [ -f "$ROOT/package.json" ]; then
    VERSION="$(
      bun -e 'console.log(JSON.parse(require("fs").readFileSync("package.json","utf8")).version)' \
        2>/dev/null \
      || node -e 'console.log(JSON.parse(require("fs").readFileSync("package.json","utf8")).version)' \
        2>/dev/null \
      || python3 -c 'import json; print(json.load(open("package.json"))["version"])'
    )"
  fi
fi
if [ -z "$VERSION" ]; then
  echo "ERROR: could not determine version (pass as arg or set package.json)" >&2
  exit 1
fi

# Strip leading v if present
VERSION="${VERSION#v}"

# ── load creds ────────────────────────────────────────────────────────────
if [ -f "$R2_ENV_FILE" ]; then
  # shellcheck disable=SC1090
  set -a
  # shellcheck source=/dev/null
  source "$R2_ENV_FILE"
  set +a
  echo "Loaded R2 env from $R2_ENV_FILE"
fi

BUCKET="${R2_BUCKET:-$DEFAULT_BUCKET}"
MIN_VERSION="${K2_WEB_BUNDLE_MIN_VERSION:-$DEFAULT_MIN_VERSION}"
UPLOAD_RETRIES="${R2_UPLOAD_RETRIES:-3}"

if [ -n "${R2_ENDPOINT:-}" ]; then
  ENDPOINT="$R2_ENDPOINT"
elif [ -n "${R2_ACCOUNT_ID:-}" ]; then
  ENDPOINT="https://${R2_ACCOUNT_ID}.r2.cloudflarestorage.com"
else
  ENDPOINT="https://${DEFAULT_ENDPOINT_HOST}"
fi

missing=()
[ -z "${R2_ACCESS_KEY_ID:-}" ] && missing+=(R2_ACCESS_KEY_ID)
[ -z "${R2_SECRET_ACCESS_KEY:-}" ] && missing+=(R2_SECRET_ACCESS_KEY)

if [ "${#missing[@]}" -gt 0 ]; then
  msg="Missing R2 credentials: ${missing[*]}
  Set them in $R2_ENV_FILE (chmod 600) or the environment.
  For CI, store GitHub secrets: R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY,
  R2_ACCOUNT_ID (optional R2_ENDPOINT, R2_BUCKET)."
  if [ "$SKIP_IF_NO_CREDS" -eq 1 ]; then
    echo "WARNING: $msg" >&2
    echo "WARNING: skipping web-bundle publish (--skip-if-no-creds)." >&2
    exit 0
  fi
  echo "ERROR: $msg" >&2
  exit 1
fi

export AWS_ACCESS_KEY_ID="$R2_ACCESS_KEY_ID"
export AWS_SECRET_ACCESS_KEY="$R2_SECRET_ACCESS_KEY"
# R2 does not use AWS session tokens for S3 keys; leave unset unless provided
if [ -n "${AWS_SESSION_TOKEN:-}" ] && [ -z "${R2_S3_TOKEN_VALUE:-}" ]; then
  :
fi
# Prefer a clean AWS region for the S3 client talking to R2
export AWS_DEFAULT_REGION="${AWS_DEFAULT_REGION:-auto}"
export AWS_EC2_METADATA_DISABLED=true

LOCAL_DIR="$ROOT/out/web/app/${VERSION}"
S3_URI="s3://${BUCKET}/app/${VERSION}/"
PUBLIC_PATH="app/${VERSION}/index.html"

echo "═══════════════════════════════════════════════════"
echo "  K2 web-bundle publish → R2"
echo "  version : ${VERSION}"
echo "  local   : ${LOCAL_DIR}"
echo "  remote  : ${S3_URI}"
echo "  endpoint: ${ENDPOINT}"
echo "  cache   : ${CACHE_CONTROL}"
[ "$DRY_RUN" -eq 1 ] && echo "  mode    : DRY-RUN"
[ "$DO_PRUNE" -eq 1 ] && echo "  prune   : below ${MIN_VERSION} (keep ${VERSION})"
echo "═══════════════════════════════════════════════════"

# ── build if missing ──────────────────────────────────────────────────────
ensure_build() {
  if [ -f "${LOCAL_DIR}/index.html" ]; then
    echo "  Found ${LOCAL_DIR}/index.html"
    return 0
  fi
  echo "  ${LOCAL_DIR}/index.html missing — running bun run vite:build:web ..."
  if [ "$DRY_RUN" -eq 1 ]; then
    echo "  [dry-run] would run: bun run vite:build:web"
    return 0
  fi
  bun run vite:build:web
  if [ ! -f "${LOCAL_DIR}/index.html" ]; then
    echo "ERROR: build finished but ${LOCAL_DIR}/index.html still missing." >&2
    echo "  Check vite.config.web.ts outDir / package.json version match (${VERSION})." >&2
    exit 1
  fi
}

# ── semver helpers (core x.y.z only; pre-release < release numerically equal) ─
# Returns 0 if $1 < $2, 1 otherwise.
semver_lt() {
  local a="$1" b="$2"
  # strip leading v and anything after first pre-release/build marker for core compare
  local ac bc
  ac="$(printf '%s' "$a" | sed -E 's/^v//; s/([0-9]+\.[0-9]+\.[0-9]+).*/\1/')"
  bc="$(printf '%s' "$b" | sed -E 's/^v//; s/([0-9]+\.[0-9]+\.[0-9]+).*/\1/')"
  local a1 a2 a3 b1 b2 b3
  IFS=. read -r a1 a2 a3 <<<"$ac"
  IFS=. read -r b1 b2 b3 <<<"$bc"
  a1=${a1:-0}; a2=${a2:-0}; a3=${a3:-0}
  b1=${b1:-0}; b2=${b2:-0}; b3=${b3:-0}
  if [ "$a1" -lt "$b1" ]; then return 0; fi
  if [ "$a1" -gt "$b1" ]; then return 1; fi
  if [ "$a2" -lt "$b2" ]; then return 0; fi
  if [ "$a2" -gt "$b2" ]; then return 1; fi
  if [ "$a3" -lt "$b3" ]; then return 0; fi
  return 1
}

# ── upload backends ───────────────────────────────────────────────────────
have_aws=0
have_boto=0
if command -v aws >/dev/null 2>&1; then
  have_aws=1
elif python3 -c 'import boto3' 2>/dev/null; then
  have_boto=1
fi

if [ "$have_aws" -eq 0 ] && [ "$have_boto" -eq 0 ]; then
  cat >&2 <<'EOF'
ERROR: neither `aws` CLI nor Python boto3 is available.

Install one of:
  macOS:   brew install awscli
  Linux:   https://docs.aws.amazon.com/cli/latest/userguide/getting-started-install.html
  fallback: pip3 install --user boto3

Then re-run: bash scripts/publish-web-bundles.sh
EOF
  exit 1
fi

# region: R2 wants "auto"; some aws CLI builds reject it — fall back to us-east-1
aws_region_args() {
  # Prefer auto when the CLI accepts it; force us-east-1 otherwise via env already set.
  printf '%s' "auto"
}

run_with_tls_retry() {
  # Runs a command, retrying on common TLS / connection failures (brand-new R2 warmup).
  local attempt=1
  local max="$UPLOAD_RETRIES"
  local rc=0
  local out=""
  while [ "$attempt" -le "$max" ]; do
    set +e
    out="$("$@" 2>&1)"
    rc=$?
    set -e
    if [ "$rc" -eq 0 ]; then
      printf '%s\n' "$out"
      return 0
    fi
    if printf '%s' "$out" | grep -qiE 'SSL|TLS|handshake|Connection reset|Connection refused|Could not connect|timeout|Timed out|Name or service not known|Temporary failure'; then
      echo "  ⚠ TLS/connection error (attempt ${attempt}/${max}):" >&2
      echo "    $(printf '%s' "$out" | head -c 400)" >&2
      if [ "$attempt" -lt "$max" ]; then
        sleep $((attempt * 3))
        attempt=$((attempt + 1))
        continue
      fi
      echo "ERROR: R2 endpoint still unreachable after ${max} attempts." >&2
      echo "  Ops caveat: brand-new R2 S3 endpoints can reject TLS until warmed." >&2
      echo "  Retry later; do not treat this as a credential problem." >&2
      printf '%s\n' "$out" >&2
      return "$rc"
    fi
    # Non-TLS failure — fail immediately
    printf '%s\n' "$out" >&2
    return "$rc"
  done
  return "$rc"
}

upload_with_aws() {
  # R2 documents region=auto; some aws CLI builds prefer us-east-1.
  local regions=("auto" "us-east-1")
  local region cmd
  if [ "$DRY_RUN" -eq 1 ]; then
    echo "  [dry-run] aws s3 sync $LOCAL_DIR $S3_URI --endpoint-url $ENDPOINT --region auto --cache-control '$CACHE_CONTROL'"
    return 0
  fi
  echo "  Uploading via aws s3 sync ..."
  for region in "${regions[@]}"; do
    cmd=(
      aws s3 sync "$LOCAL_DIR" "$S3_URI"
      --endpoint-url "$ENDPOINT"
      --region "$region"
      --cache-control "$CACHE_CONTROL"
      --only-show-errors
    )
    echo "  trying region=${region} ..."
    if run_with_tls_retry "${cmd[@]}"; then
      echo "  upload ok (region=${region})"
      return 0
    fi
    echo "  region=${region} failed; trying next if any ..."
  done
  echo "ERROR: aws s3 sync failed for all regions." >&2
  return 1
}

upload_with_boto() {
  if [ "$DRY_RUN" -eq 1 ]; then
    echo "  [dry-run] python boto3 upload_filetree ${LOCAL_DIR} → ${S3_URI}"
    return 0
  fi
  echo "  Uploading via Python boto3 (aws CLI not found) ..."
  R2_ENDPOINT="$ENDPOINT" R2_BUCKET="$BUCKET" R2_VERSION="$VERSION" \
  R2_LOCAL_DIR="$LOCAL_DIR" R2_CACHE_CONTROL="$CACHE_CONTROL" \
  R2_UPLOAD_RETRIES="$UPLOAD_RETRIES" \
  python3 - <<'PY'
import os, sys, time
from pathlib import Path

try:
    import boto3
    from botocore.config import Config
    from botocore.exceptions import BotoCoreError, ClientError, EndpointConnectionError, SSLError
except ImportError:
    print("boto3 missing", file=sys.stderr)
    sys.exit(1)

endpoint = os.environ["R2_ENDPOINT"]
bucket = os.environ["R2_BUCKET"]
version = os.environ["R2_VERSION"]
local = Path(os.environ["R2_LOCAL_DIR"])
cache = os.environ["R2_CACHE_CONTROL"]
retries = int(os.environ.get("R2_UPLOAD_RETRIES", "3"))
prefix = f"app/{version}/"

def client_for(region):
    return boto3.client(
        "s3",
        endpoint_url=endpoint,
        region_name=region,
        aws_access_key_id=os.environ["AWS_ACCESS_KEY_ID"],
        aws_secret_access_key=os.environ["AWS_SECRET_ACCESS_KEY"],
        config=Config(signature_version="s3v4", s3={"addressing_style": "path"}),
    )

files = [p for p in local.rglob("*") if p.is_file()]
if not files:
    print(f"ERROR: no files under {local}", file=sys.stderr)
    sys.exit(1)

last_err = None
for region in ("auto", "us-east-1"):
    for attempt in range(1, retries + 1):
        try:
            s3 = client_for(region)
            for p in files:
                key = prefix + p.relative_to(local).as_posix()
                extra = {"CacheControl": cache}
                # rough content-type hints
                suffix = p.suffix.lower()
                if suffix == ".html":
                    extra["ContentType"] = "text/html; charset=utf-8"
                elif suffix == ".js":
                    extra["ContentType"] = "application/javascript"
                elif suffix == ".css":
                    extra["ContentType"] = "text/css"
                elif suffix == ".svg":
                    extra["ContentType"] = "image/svg+xml"
                elif suffix == ".json":
                    extra["ContentType"] = "application/json"
                elif suffix == ".woff2":
                    extra["ContentType"] = "font/woff2"
                s3.upload_file(str(p), bucket, key, ExtraArgs=extra)
            print(f"  uploaded {len(files)} objects (region={region})")
            sys.exit(0)
        except Exception as e:  # noqa: BLE001 — surface + retry TLS
            last_err = e
            name = type(e).__name__
            msg = str(e)
            tlsy = any(x in name or x.lower() in msg.lower() for x in (
                "SSL", "TLS", "EndpointConnection", "Connection", "timeout", "Timeout"
            ))
            print(f"  ⚠ upload error (region={region} attempt {attempt}/{retries}): {e}", file=sys.stderr)
            if tlsy and attempt < retries:
                time.sleep(attempt * 3)
                continue
            break

print("ERROR: boto3 upload failed after retries.", file=sys.stderr)
print("  Ops caveat: brand-new R2 S3 endpoints can reject TLS until warmed.", file=sys.stderr)
if last_err:
    print(f"  last error: {last_err}", file=sys.stderr)
sys.exit(1)
PY
}

list_versions_aws() {
  local region="${1:-auto}"
  aws s3api list-objects-v2 \
    --bucket "$BUCKET" \
    --prefix "app/" \
    --delimiter "/" \
    --endpoint-url "$ENDPOINT" \
    --region "$region" \
    --query 'CommonPrefixes[].Prefix' \
    --output text 2>/dev/null \
  || aws s3 ls "s3://${BUCKET}/app/" --endpoint-url "$ENDPOINT" --region "$region" \
    | awk '{print $2}' | sed 's|/$||' | sed 's|^|app/|;s|$|/|'
}

delete_prefix_aws() {
  local prefix="$1"  # e.g. app/0.39.0/
  local region="${2:-auto}"
  if [ "$DRY_RUN" -eq 1 ]; then
    echo "  [dry-run] aws s3 rm s3://${BUCKET}/${prefix} --recursive"
    return 0
  fi
  aws s3 rm "s3://${BUCKET}/${prefix}" --recursive \
    --endpoint-url "$ENDPOINT" \
    --region "$region" \
    --only-show-errors
}

prune_with_aws() {
  local region="auto"
  echo "  Listing app/ prefixes for prune (floor=${MIN_VERSION}, keep=${VERSION}) ..."
  local listing
  set +e
  listing="$(list_versions_aws auto)"
  local rc=$?
  set -e
  if [ "$rc" -ne 0 ] || [ -z "$listing" ]; then
    set +e
    listing="$(list_versions_aws us-east-1)"
    rc=$?
    set -e
    region="us-east-1"
  fi
  if [ -z "$listing" ]; then
    echo "  No app/ prefixes found (or list failed) — nothing to prune."
    return 0
  fi
  local p ver
  for p in $listing; do
    # prefixes look like app/0.40.1/ or bare 0.40.1/
    ver="$(printf '%s' "$p" | sed -E 's|^app/||; s|/$||')"
    [ -z "$ver" ] && continue
    # Never delete current version
    if [ "$ver" = "$VERSION" ]; then
      echo "  keep  ${ver} (current)"
      continue
    fi
    if ! printf '%s' "$ver" | grep -qE '^[0-9]+\.[0-9]+\.[0-9]+'; then
      echo "  skip  ${ver} (not semver-ish)"
      continue
    fi
    if semver_lt "$ver" "$MIN_VERSION"; then
      echo "  DELETE app/${ver}/  (< floor ${MIN_VERSION})"
      delete_prefix_aws "app/${ver}/" "$region" || {
        echo "ERROR: failed to delete app/${ver}/" >&2
        return 1
      }
    else
      echo "  keep  ${ver}"
    fi
  done
}

prune_with_boto() {
  if [ "$DRY_RUN" -eq 1 ]; then
    echo "  [dry-run] boto3 list+delete prefixes below ${MIN_VERSION}"
    return 0
  fi
  R2_ENDPOINT="$ENDPOINT" R2_BUCKET="$BUCKET" R2_VERSION="$VERSION" \
  R2_MIN_VERSION="$MIN_VERSION" \
  python3 - <<'PY'
import os, sys, re
import boto3
from botocore.config import Config

endpoint = os.environ["R2_ENDPOINT"]
bucket = os.environ["R2_BUCKET"]
current = os.environ["R2_VERSION"]
floor = os.environ["R2_MIN_VERSION"]

def core(v):
    m = re.match(r"v?(\d+)\.(\d+)\.(\d+)", v)
    if not m:
        return None
    return tuple(int(x) for x in m.groups())

def lt(a, b):
    ca, cb = core(a), core(b)
    if ca is None or cb is None:
        return False
    return ca < cb

s3 = None
for region in ("auto", "us-east-1"):
    try:
        s3 = boto3.client(
            "s3",
            endpoint_url=endpoint,
            region_name=region,
            aws_access_key_id=os.environ["AWS_ACCESS_KEY_ID"],
            aws_secret_access_key=os.environ["AWS_SECRET_ACCESS_KEY"],
            config=Config(signature_version="s3v4", s3={"addressing_style": "path"}),
        )
        # probe
        s3.list_objects_v2(Bucket=bucket, Prefix="app/", Delimiter="/", MaxKeys=1)
        break
    except Exception as e:
        print(f"  list probe region={region} failed: {e}", file=sys.stderr)
        s3 = None
if s3 is None:
    print("ERROR: could not list bucket for prune", file=sys.stderr)
    sys.exit(1)

resp = s3.list_objects_v2(Bucket=bucket, Prefix="app/", Delimiter="/")
prefixes = [p["Prefix"] for p in resp.get("CommonPrefixes", [])]
# paginate
while resp.get("IsTruncated"):
    resp = s3.list_objects_v2(
        Bucket=bucket, Prefix="app/", Delimiter="/",
        ContinuationToken=resp["NextContinuationToken"],
    )
    prefixes.extend(p["Prefix"] for p in resp.get("CommonPrefixes", []))

for pref in prefixes:
    ver = pref[len("app/"):].rstrip("/")
    if ver == current:
        print(f"  keep  {ver} (current)")
        continue
    if lt(ver, floor):
        print(f"  DELETE {pref}  (< floor {floor})")
        # delete all objects under prefix
        token = None
        while True:
            kw = {"Bucket": bucket, "Prefix": pref}
            if token:
                kw["ContinuationToken"] = token
            page = s3.list_objects_v2(**kw)
            objs = [{"Key": o["Key"]} for o in page.get("Contents", [])]
            if objs:
                for i in range(0, len(objs), 1000):
                    s3.delete_objects(Bucket=bucket, Delete={"Objects": objs[i:i+1000]})
            if not page.get("IsTruncated"):
                break
            token = page.get("NextContinuationToken")
    else:
        print(f"  keep  {ver}")
PY
}

# ── main ──────────────────────────────────────────────────────────────────
ensure_build

if [ "$DRY_RUN" -eq 0 ] && [ ! -f "${LOCAL_DIR}/index.html" ]; then
  echo "ERROR: ${LOCAL_DIR}/index.html not found after build." >&2
  exit 1
fi

if [ "$have_aws" -eq 1 ]; then
  upload_with_aws
else
  upload_with_boto
fi

if [ "$DO_PRUNE" -eq 1 ]; then
  echo ""
  echo "Pruning app/ prefixes strictly below ${MIN_VERSION} ..."
  if [ "$have_aws" -eq 1 ]; then
    prune_with_aws
  else
    prune_with_boto
  fi
fi

echo ""
echo "✓ Published web bundle"
echo "  public path expectation: ${PUBLIC_PATH}"
echo "  (loader HEADs /${PUBLIC_PATH} then navigates to /app/${VERSION}/)"
if [ "$DRY_RUN" -eq 1 ]; then
  echo "  (dry-run — no objects written)"
fi
