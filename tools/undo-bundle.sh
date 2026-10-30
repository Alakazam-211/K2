#!/bin/sh
# FALLBACK ONLY. The default way to run an undo job is `k2 compute run --repo ... -- sh
# tools/undo/job.sh <quint|enum> <commands file>` (tools/README.md, "The undo model check"):
# the runner ships the commit and nothing travels in the environment. Use this script only
# for a runner that can't ship a checkout.
#
# Shared by tools/undo-quint-job.sh and tools/undo-enum-job.sh: write a self-contained job for
# a compute node that runs with no checkout (`--no-source`). The sources travel inside the
# job as a base64 gzip tarball, split into chunks passed as environment variables UNDO_B64_0,
# UNDO_B64_1, ... (each at most CHUNK characters, default 8000: a runner caps one variable's
# value at 8 KiB); the job joins them, checks the tarball's sha256, unpacks, and runs
# tools/undo/job.sh (the same job a --repo run starts).
#
# **The environment limit (hard check).** A runner caps a job at 64 environment pairs
# (k2-node 0.46.6: `env_too_many`). The bundler counts every pair the job will carry, the
# chunks and each `--env NAME=VALUE` given here (PART, THREADS, CARGO_BUILD_JOBS, ...), and
# refuses to write the job when that is over ENV_LIMIT (60, a margin under 64). Pass every
# variable through `--env` here, not on the runner's command line.
#
# What travels: the workspace manifest and lock, crates/spec-undo (sources, and of its tests
# only those the build needs), spec/quint (quint jobs), tools/undo, the commands file, and
# of crates/base **only src/text**, verbatim: the oracle calls nothing of the kit but
# text::diff and text::lines, and that module uses only std and thiserror. The job builds it
# as a minimal `k2-mpc-base` (lib.rs = `pub mod text;`; the version, the thiserror
# requirement, and the `conformance` and `differential` features of the real manifest, the
# latter only declared: spec-undo's dev-dependency names it), so the bundle stays small
# and the build skips the kit's other dependencies.
#
#   tools/undo-bundle.sh <quint|enum> [--env NAME=VALUE]... <commands file> <out dir> [extra files...]
#
# writes <out dir>/job.sh (the job: pass it as `sh -ec "$(cat <out dir>/job.sh)"`) and
# <out dir>/env-args (every `--env NAME=VALUE` argument the job needs, one per line).
set -eu
KIND=${1:?usage: tools/undo-bundle.sh <quint|enum> [--env NAME=VALUE]... <commands file> <out dir> [extra files]}
shift
ENVS=""; NENV=0
while [ "${1:-}" = --env ]; do
  case "${2:-}" in [A-Za-z_]*=*) ;; *) echo "--env needs NAME=VALUE" >&2; exit 2 ;; esac
  ENVS="$ENVS$(printf -- '--env\n%s' "$2")
"; NENV=$((NENV + 1)); shift 2
done
CMDS=${1:?commands file}
OUT=${2:?out dir}
shift 2
ENV_LIMIT=${ENV_LIMIT:-60}
CHUNK=${CHUNK:-8000}
case $KIND in quint|enum) ;; *) echo "kind: quint or enum" >&2; exit 2 ;; esac
ROOT=$(cd "$(dirname "$0")/.." && pwd)
case $OUT in /*) ;; *) OUT=$PWD/$OUT ;; esac
mkdir -p "$OUT"
cd "$ROOT"
[ -f "$CMDS" ] || { echo "no commands file $CMDS" >&2; exit 2; }
VERSION=$(sed -n 's/^version = "\(.*\)"$/\1/p' crates/base/Cargo.toml | head -1)
THISERROR=$(sed -n 's/^thiserror = "\(.*\)"$/\1/p' crates/base/Cargo.toml | head -1)
FILES="Cargo.toml Cargo.lock crates/base/src/text crates/spec-undo/Cargo.toml crates/spec-undo/src crates/spec-undo/tests/mutants.rs tools/undo $CMDS $*"
[ "$KIND" = quint ] && FILES="$FILES spec/quint tools/quint tools/quint-jvm"
COPYFILE_DISABLE=1 tar --no-mac-metadata --no-xattrs --exclude target --exclude '*.itf.json' \
  -czf "$OUT/bundle.tgz" $FILES 2>/dev/null \
  || COPYFILE_DISABLE=1 tar --exclude target --exclude '*.itf.json' -czf "$OUT/bundle.tgz" $FILES
if command -v sha256sum >/dev/null; then SHA=$(sha256sum "$OUT/bundle.tgz" | cut -d' ' -f1)
else SHA=$(shasum -a 256 "$OUT/bundle.tgz" | cut -d' ' -f1); fi
base64 < "$OUT/bundle.tgz" | tr -d '\n' > "$OUT/bundle.b64"
TOTAL=$(wc -c < "$OUT/bundle.b64" | tr -d ' ')
NEED=$(( (TOTAL + CHUNK - 1) / CHUNK ))
if [ $(( NEED + NENV )) -gt "$ENV_LIMIT" ]; then
  rm -f "$OUT/bundle.b64" "$OUT/env-args" "$OUT/job.sh"
  echo "undo-bundle.sh: REFUSED: the job would carry $(( NEED + NENV )) environment pairs ($NEED chunks + $NENV --env), over $ENV_LIMIT" >&2
  echo "  A runner caps a job at 64 environment pairs; this fallback allows $ENV_LIMIT (ENV_LIMIT)." >&2
  echo "  Run it with --repo instead: the runner ships the commit and nothing travels in the" >&2
  echo "  environment (tools/README.md, \"The undo model check\"):" >&2
  echo "    k2 compute run <node> --repo <this repo, relative to the workspace> [--sha REV] \\" >&2
  echo "      --mem 12G [--env ...] -- sh tools/undo/job.sh $KIND $CMDS" >&2
  exit 3
fi
printf '%s' "$ENVS" > "$OUT/env-args"
i=0; off=0
while [ "$off" -lt "$TOTAL" ]; do
  printf -- '--env\nUNDO_B64_%s=%s\n' "$i" "$(dd if="$OUT/bundle.b64" bs="$CHUNK" skip="$i" count=1 2>/dev/null)" >> "$OUT/env-args"
  i=$((i + 1)); off=$((off + CHUNK))
done
NCHUNKS=$i
cat > "$OUT/job.sh" <<JOB
set -e
mkdir -p job && cd job
: > bundle.b64
i=0; while [ \$i -lt $NCHUNKS ]; do eval "c=\\\${UNDO_B64_\$i:?chunk \$i of $NCHUNKS missing}"; printf '%s' "\$c" >> bundle.b64; i=\$((i + 1)); done
base64 -d < bundle.b64 > bundle.tgz
echo "$SHA  bundle.tgz" | sha256sum -c - >/dev/null || { echo "bundle sha256 mismatch" >&2; exit 1; }
tar xzf bundle.tgz && rm -f bundle.b64 bundle.tgz
# the workspace without the crates not shipped (the kit's other crates, vendor/), and the
# kit's text module (verbatim) as a minimal k2-mpc-base
sed -i 's|^members = .*|members = ["crates/base", "crates/spec-undo"]|' Cargo.toml
printf '[package]\nname = "k2-mpc-base"\nversion = "$VERSION"\nedition = "2021"\npublish = false\n\n[dependencies]\nthiserror = "$THISERROR"\n\n[features]\nconformance = []\ndifferential = []\n' > crates/base/Cargo.toml
echo 'pub mod text;' > crates/base/src/lib.rs
export CARGO_TARGET_DIR=\$PWD/target
exec sh tools/undo/job.sh $KIND "$CMDS"
JOB
JOBSZ=$(wc -c < "$OUT/job.sh" | tr -d ' ')
echo "bundle: $(wc -c < "$OUT/bundle.tgz" | tr -d ' ') bytes gzip, $TOTAL base64, $NCHUNKS chunk(s) of <= $CHUNK, $(( NCHUNKS + NENV )) environment pairs of $ENV_LIMIT; sha256 $SHA; job.sh $JOBSZ bytes" >&2
rm -f "$OUT/bundle.b64"
