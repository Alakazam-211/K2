#!/bin/sh
# FALLBACK ONLY. The default way to run a job is `k2 compute run --repo` (tools/README.md,
# "Test jobs"): the runner ships the commit, nothing travels in the environment, and the job
# is `sh tools/job.sh <commands file>`. Use this script only for a runner that can't ship a
# checkout.
#
# Write a self-contained job that runs the kit's tests (or any commands) on a compute node
# with no checkout (`--no-source`): the listed paths travel as an xz tarball, base64, split
# into chunks of at most CHUNK characters (default 8000; a runner caps one variable's value at
# 8 KiB) passed as environment variables BUNDLE_B64_0, BUNDLE_B64_1, ...; the job joins them,
# checks the tarball's sha256, unpacks, and runs tools/job.sh (shipped with it) on the
# commands file. xz, not gzip: about a quarter fewer chunks for the kit's sources.
#
# **The environment limit (hard check).** A runner caps a job at 64 environment pairs
# (k2-node 0.46.6: more is refused, `env_too_many`). The bundler counts every pair the job
# will carry, the chunks and each `--env NAME=VALUE` given here (PART, SOAK_SECS, ...), and
# refuses to write the job when that is over ENV_LIMIT (60: a margin under 64). Pass every
# variable through `--env` here, never as an extra `--env` on the runner's command line, so
# the count is the job's. INLINE=N (default 0) lets up to N base64 characters past the
# chunks travel inside job.sh itself, i.e. in the job's command argument (120,000 checked on
# a build node; Linux allows 128 KiB per argument): no environment pair, and still a
# fallback.
#
#   tools/test-bundle.sh [--rev REV] [--env NAME=VALUE]... <commands file> <out dir> <path>...
#
# <path>...: what travels, relative to the repo root (e.g. Cargo.toml Cargo.lock
#   crates/base). Taken from a clean `git archive` of REV (default HEAD), never from the
#   working tree, so uncommitted files don't travel. tools/job.sh always travels. When
#   Cargo.toml travels and some workspace member doesn't, the job narrows the workspace's
#   `members` line to the members shipped (the manifest's only change; no test or kit code
#   changes).
# <commands file>: sourced by the job at the repo root (it travels as cmds.sh): see
#   tools/job.sh.
#
# Writes <out dir>/job.sh (pass it as `sh -ec "$(cat <out dir>/job.sh)"`) and
# <out dir>/env-args (every `--env NAME=VALUE` argument the job needs, one per line: the
# --env pairs first, then the chunks). Nothing here is specific to one compute system: any
# runner that sets the variables and runs the job with sh on Linux works.
set -eu
REV=HEAD
ENVS=""; NENV=0
while :; do
  case "${1:-}" in
    --rev) REV=${2:?--rev needs a revision}; shift 2 ;;
    --env) case "${2:-}" in [A-Za-z_]*=*) ;; *) echo "--env needs NAME=VALUE" >&2; exit 2 ;; esac
           ENVS="$ENVS$(printf -- '--env\n%s' "$2")
"; NENV=$((NENV + 1)); shift 2 ;;
    *) break ;;
  esac
done
CMDS=${1:?usage: tools/test-bundle.sh [--rev REV] <commands file> <out dir> <path>...}
OUT=${2:?out dir}
shift 2
[ $# -gt 0 ] || { echo "nothing to ship: list the paths that travel" >&2; exit 2; }
CHUNK=${CHUNK:-8000}
ENV_LIMIT=${ENV_LIMIT:-60}
MAX_CHUNKS=${MAX_CHUNKS:-$(( ENV_LIMIT - NENV ))}
INLINE=${INLINE:-0}
command -v xz >/dev/null || { echo "xz is needed (the bundle is an xz tarball)" >&2; exit 2; }
[ -f "$CMDS" ] || { echo "no commands file $CMDS" >&2; exit 2; }
case $OUT in /*) ;; *) OUT=$PWD/$OUT ;; esac
case $CMDS in /*) ;; *) CMDS=$PWD/$CMDS ;; esac
ROOT=$(cd "$(dirname "$0")/.." && pwd)
mkdir -p "$OUT"
EXPORT="$OUT/export"
rm -rf "$EXPORT" && mkdir -p "$EXPORT"
git -C "$ROOT" archive --format=tar "$REV" -- "$@" tools/job.sh | tar -xf - -C "$EXPORT"
cp "$CMDS" "$EXPORT/cmds.sh"
# vendor/yrs travels without src/ (and its tests, benches, README): the job rebuilds src/ from
# the upstream crate on crates.io and vendor/yrs/FORK.patch, checked against VENDORED's
# src-sha256 (v0.9.3; with it the kit is over a job's limit; docs/YRS-FORK.md)
YRS=0
if [ -d "$EXPORT/vendor/yrs/src" ]; then
  YRS=1
  rm -rf "$EXPORT/vendor/yrs/src" "$EXPORT/vendor/yrs/tests" "$EXPORT/vendor/yrs/benches" "$EXPORT/vendor/yrs/README.md"
fi
# STRIP_DOCS=1 (v0.9.7, for a part that runs no doctests): every crate's sources travel with
# their doc-comment lines (`///`, `//!`) blanked, not deleted, so every line number (a panic's,
# a test's) is the repository's. The code is the same; only documentation stays behind.
# tools/fuzz/strip-comments.py refuses (and the bundle with it) when such a line sits inside a
# string literal.
if [ "${STRIP_DOCS:-0}" = 1 ]; then
  for d in "$EXPORT"/crates/*/src "$EXPORT"/crates/*/tests; do
    if [ -d "$d" ]; then python3 -I "$ROOT/tools/fuzz/strip-comments.py" --docs "$d"; fi
  done
fi
# the workspace members that travel (when the workspace manifest does)
MEMBERS=""
if [ -f "$EXPORT/Cargo.toml" ]; then
  for m in $(sed -n 's/^members = \[\(.*\)\]$/\1/p' "$EXPORT/Cargo.toml" | tr -d '",'); do
    [ -d "$EXPORT/$m" ] && MEMBERS="$MEMBERS${MEMBERS:+, }\"$m\""
  done
fi
( cd "$EXPORT" && { COPYFILE_DISABLE=1 tar --no-mac-metadata --no-xattrs -cf "$OUT/bundle.tar" . 2>/dev/null \
  || COPYFILE_DISABLE=1 tar -cf "$OUT/bundle.tar" .; } )
rm -rf "$EXPORT"
xz -9e -c "$OUT/bundle.tar" > "$OUT/bundle.txz"; rm -f "$OUT/bundle.tar"
if command -v sha256sum >/dev/null; then SHA=$(sha256sum "$OUT/bundle.txz" | cut -d' ' -f1)
else SHA=$(shasum -a 256 "$OUT/bundle.txz" | cut -d' ' -f1); fi
base64 < "$OUT/bundle.txz" | tr -d '\n' > "$OUT/bundle.b64"
TOTAL=$(wc -c < "$OUT/bundle.b64" | tr -d ' ')
NEED=$(( (TOTAL + CHUNK - 1) / CHUNK ))
ENVMAX=$(( MAX_CHUNKS * CHUNK ))
TAIL=0
if [ "$NEED" -gt "$MAX_CHUNKS" ] && [ $(( TOTAL - ENVMAX )) -le "$INLINE" ]; then
  TAIL=$(( TOTAL - ENVMAX )); NEED=$MAX_CHUNKS
fi
refuse() {
  rm -f "$OUT/bundle.b64" "$OUT/env-args" "$OUT/job.sh"
  echo "test-bundle.sh: REFUSED: $1" >&2
  echo "  A runner caps a job at 64 environment pairs; this fallback allows $ENV_LIMIT (ENV_LIMIT)." >&2
  echo "  Run it with --repo instead: the runner ships the commit and nothing travels in the" >&2
  echo "  environment (tools/README.md, \"Test jobs\"):" >&2
  echo "    k2 compute run <node> --repo <this repo, relative to the workspace> [--sha REV] \\" >&2
  echo "      --mem 8G --cpus 4 [--env PART=...] -- sh tools/job.sh <commands file>" >&2
  exit 3
}
if [ "$NEED" -gt "$MAX_CHUNKS" ]; then
  refuse "$TOTAL base64 characters need $NEED chunks of $CHUNK, over MAX_CHUNKS=$MAX_CHUNKS (INLINE=$(( TOTAL - ENVMAX )) or more would carry the rest in the command argument)"
fi
# the hard check: every environment pair the job carries
if [ $(( NEED + NENV )) -gt "$ENV_LIMIT" ]; then
  refuse "the job would carry $(( NEED + NENV )) environment pairs ($NEED chunks + $NENV --env), over $ENV_LIMIT"
fi
printf '%s' "$ENVS" > "$OUT/env-args"
i=0; off=0
while [ "$off" -lt "$TOTAL" ] && [ "$i" -lt "$NEED" ]; do
  printf -- '--env\nBUNDLE_B64_%s=%s\n' "$i" "$(dd if="$OUT/bundle.b64" bs="$CHUNK" skip="$i" count=1 2>/dev/null)" >> "$OUT/env-args"
  i=$((i + 1)); off=$((off + CHUNK))
done
N=$i
INLINE_PART=""
if [ "$TAIL" -gt 0 ]; then
  INLINE_PART=$(tail -c +$(( ENVMAX + 1 )) "$OUT/bundle.b64")
fi
rm -f "$OUT/bundle.b64"
cat > "$OUT/job.sh" <<JOB
set -e
mkdir -p job && cd job
: > b.b64
i=0; while [ \$i -lt $N ]; do eval "c=\\\${BUNDLE_B64_\$i:?chunk \$i of $N missing}"; printf '%s' "\$c" >> b.b64; i=\$((i + 1)); done
printf '%s' '$INLINE_PART' >> b.b64
base64 -d < b.b64 > b.txz
echo "$SHA  b.txz" | sha256sum -c - >/dev/null || { echo "bundle sha256 mismatch" >&2; exit 1; }
tar xJf b.txz && rm -f b.b64 b.txz
echo "bundle ok: sha256 $SHA, $N chunk(s)"
JOB
if [ "$YRS" = 1 ]; then
cat >> "$OUT/job.sh" <<'JOB'
# vendor/yrs/src: the upstream crate (its sha256 checked) plus the fork's patch, then checked
# file by file against VENDORED's src-sha256; the benches (not shipped) leave the manifest
set -- $(sed -n 's/^crate //p' vendor/yrs/VENDORED); YC=$1; YSHA=$2
curl -sSfL --retry 3 -o yrs.crate "https://static.crates.io/crates/${YC%-*}/$YC.crate"
echo "$YSHA  yrs.crate" | sha256sum -c - >/dev/null || { echo "vendor/yrs: $YC.crate sha256 mismatch" >&2; exit 1; }
mkdir -p yrs-crate && tar xzf yrs.crate -C yrs-crate && mv "yrs-crate/$YC/src" vendor/yrs/src && rm -rf yrs.crate yrs-crate
( cd vendor && patch -p1 -s --no-backup-if-mismatch < yrs/FORK.patch ) || { echo "vendor/yrs: FORK.patch does not apply" >&2; exit 1; }
YSUM=$(cd vendor/yrs/src && find . -type f | LC_ALL=C sort | sed 's|^\./||' | while read -r f; do sha256sum "$f"; done | sha256sum | cut -d' ' -f1)
[ "$YSUM" = "$(sed -n 's/^src-sha256 //p' vendor/yrs/VENDORED)" ] || { echo "vendor/yrs: rebuilt src differs from VENDORED" >&2; exit 1; }
sed -i '/^\[\[bench\]\]/,/^$/d' vendor/yrs/Cargo.toml
echo "vendor/yrs: $YC + FORK.patch, src-sha256 $YSUM"
JOB
fi
if [ -n "$MEMBERS" ]; then
  echo "sed -i 's|^members = .*|members = [$MEMBERS]|' Cargo.toml && grep '^members' Cargo.toml" >> "$OUT/job.sh"
fi
cat >> "$OUT/job.sh" <<'JOB'
# a fresh target dir for the unpacked tree, then the harness (tools/job.sh) on the commands
export CARGO_TARGET_DIR=$PWD/target
exec sh tools/job.sh cmds.sh
JOB
echo "bundle: $(wc -c < "$OUT/bundle.txz" | tr -d ' ') bytes xz, $TOTAL base64, $N chunk(s) of <= $CHUNK + $TAIL inline; $(( N + NENV )) environment pairs of $ENV_LIMIT; sha256 $SHA; job.sh $(wc -c < "$OUT/job.sh" | tr -d ' ') bytes" >&2
