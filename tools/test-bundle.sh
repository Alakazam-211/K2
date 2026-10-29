#!/bin/sh
# Write a self-contained job that runs the kit's tests (or any commands) on a compute node
# with no checkout (`--no-source`). The same pattern as tools/undo-bundle.sh, for any part
# of the repo: the listed paths travel as an xz tarball, base64, split into chunks of at
# most CHUNK characters (default 8000; a runner may cap one variable's value far below
# Linux's 128 KiB per argument) passed as environment variables BUNDLE_B64_0,
# BUNDLE_B64_1, ...; the job joins them and checks the tarball's sha256 before unpacking.
# A runner may also cap the number of variables (one refuses more than about 66), so the
# bundler stops with a clear message above MAX_CHUNKS (default 64): ship fewer paths. xz,
# not gzip: about a quarter fewer chunks for the kit's sources.
#
#   tools/test-bundle.sh [--rev REV] <commands file> <out dir> <path>...
#
# <path>...: what travels, relative to the repo root (e.g. Cargo.toml Cargo.lock
#   crates/base). Taken from a clean `git archive` of REV (default HEAD), never from the
#   working tree, so uncommitted files don't travel. When Cargo.toml travels and some
#   workspace member doesn't, the job narrows the workspace's `members` line to the members
#   shipped (the manifest's only change; no test or kit code changes).
# <commands file>: sourced by the job at the repo root (it travels as cmds.sh). It runs its
#   steps through `timed LABEL CMD...`, which prints `=== LABEL: exit N, wall Ss` and makes
#   the job exit non-zero if any step failed (the later steps still run).
#
# Writes <out dir>/job.sh (pass it as `sh -ec "$(cat <out dir>/job.sh)"`) and
# <out dir>/env-args (the `--env NAME=VALUE` arguments, one per line). The job prints a
# heartbeat every 4 minutes so a long silent step isn't taken for a stall. Nothing here is
# specific to one compute system: any runner that sets the variables and runs the job with
# sh on Linux works. tools/README.md shows the whole command.
set -eu
REV=HEAD
if [ "${1:-}" = --rev ]; then REV=${2:?--rev needs a revision}; shift 2; fi
CMDS=${1:?usage: tools/test-bundle.sh [--rev REV] <commands file> <out dir> <path>...}
OUT=${2:?out dir}
shift 2
[ $# -gt 0 ] || { echo "nothing to ship: list the paths that travel" >&2; exit 2; }
CHUNK=${CHUNK:-8000}
MAX_CHUNKS=${MAX_CHUNKS:-64}
command -v xz >/dev/null || { echo "xz is needed (the bundle is an xz tarball)" >&2; exit 2; }
[ -f "$CMDS" ] || { echo "no commands file $CMDS" >&2; exit 2; }
case $OUT in /*) ;; *) OUT=$PWD/$OUT ;; esac
case $CMDS in /*) ;; *) CMDS=$PWD/$CMDS ;; esac
ROOT=$(cd "$(dirname "$0")/.." && pwd)
mkdir -p "$OUT"
EXPORT="$OUT/export"
rm -rf "$EXPORT" && mkdir -p "$EXPORT"
git -C "$ROOT" archive --format=tar "$REV" -- "$@" | tar -xf - -C "$EXPORT"
cp "$CMDS" "$EXPORT/cmds.sh"
# vendor/yrs travels without src/ (and its tests, benches, README): the job rebuilds src/ from
# the upstream crate on crates.io and vendor/yrs/K2.patch, checked against VENDORED's
# src-sha256 (v0.9.3; with it the kit is over a job's limit; docs/YRS-FORK.md)
YRS=0
if [ -d "$EXPORT/vendor/yrs/src" ]; then
  YRS=1
  rm -rf "$EXPORT/vendor/yrs/src" "$EXPORT/vendor/yrs/tests" "$EXPORT/vendor/yrs/benches" "$EXPORT/vendor/yrs/README.md"
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
if [ "$NEED" -gt "$MAX_CHUNKS" ]; then
  rm -f "$OUT/bundle.b64"
  echo "bundle too big: $TOTAL base64 characters need $NEED chunks of $CHUNK, over MAX_CHUNKS=$MAX_CHUNKS (a runner refuses more variables): ship fewer paths" >&2
  exit 3
fi
: > "$OUT/env-args"
i=0; off=0
while [ "$off" -lt "$TOTAL" ]; do
  printf -- '--env\nBUNDLE_B64_%s=%s\n' "$i" "$(dd if="$OUT/bundle.b64" bs="$CHUNK" skip="$i" count=1 2>/dev/null)" >> "$OUT/env-args"
  i=$((i + 1)); off=$((off + CHUNK))
done
N=$i
rm -f "$OUT/bundle.b64"
cat > "$OUT/job.sh" <<JOB
set -e
mkdir -p job && cd job
: > b.b64
i=0; while [ \$i -lt $N ]; do eval "c=\\\${BUNDLE_B64_\$i:?chunk \$i of $N missing}"; printf '%s' "\$c" >> b.b64; i=\$((i + 1)); done
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
( cd vendor && patch -p1 -s --no-backup-if-mismatch < yrs/K2.patch ) || { echo "vendor/yrs: K2.patch does not apply" >&2; exit 1; }
YSUM=$(cd vendor/yrs/src && find . -type f | LC_ALL=C sort | sed 's|^\./||' | while read -r f; do sha256sum "$f"; done | sha256sum | cut -d' ' -f1)
[ "$YSUM" = "$(sed -n 's/^src-sha256 //p' vendor/yrs/VENDORED)" ] || { echo "vendor/yrs: rebuilt src differs from VENDORED" >&2; exit 1; }
sed -i '/^\[\[bench\]\]/,/^$/d' vendor/yrs/Cargo.toml
echo "vendor/yrs: $YC + K2.patch, src-sha256 $YSUM"
JOB
fi
if [ -n "$MEMBERS" ]; then
  echo "sed -i 's|^members = .*|members = [$MEMBERS]|' Cargo.toml && grep '^members' Cargo.toml" >> "$OUT/job.sh"
fi
cat >> "$OUT/job.sh" <<'JOB'
ROOT=$PWD
export CARGO_TARGET_DIR=$ROOT/target
uname -sm; nproc; free -g | head -2
command -v rustc >/dev/null && rustc --version || true
( while sleep 240; do echo "[hb $(date -u +%H:%M:%S)] $(cut -d' ' -f1-3 /proc/loadavg)"; done ) & HB=$!
trap 'kill $HB 2>/dev/null || true' EXIT
RC=0
timed() { l=$1; shift; s=$(date +%s); set +e; "$@"; rc=$?; set -e; echo "=== $l: exit $rc, wall $(( $(date +%s) - s ))s"; [ $rc -eq 0 ] || RC=$rc; }
. "$ROOT/cmds.sh"
exit $RC
JOB
echo "bundle: $(wc -c < "$OUT/bundle.txz" | tr -d ' ') bytes xz, $TOTAL base64, $N chunk(s) of <= $CHUNK; sha256 $SHA; job.sh $(wc -c < "$OUT/job.sh" | tr -d ' ') bytes" >&2
