#!/bin/sh
# Shared by tools/undo-quint-job.sh and tools/undo-enum-job.sh: write a self-contained job
# for a compute node that runs with no checkout (`--no-source`). The sources travel inside
# the job as a base64 gzip tarball, split into chunks passed as environment variables
# UNDO_B64_0, UNDO_B64_1, ... (each at most CHUNK characters, default 8000: a runner may cap
# one variable's value well below Linux's 128 KiB per argument); the job joins them and
# checks the tarball's sha256 before unpacking.
#
# What travels: the workspace manifest and lock, crates/spec-undo (sources, and of its tests
# only those the build needs), spec/quint (quint jobs), tools/undo, the commands file, and
# of crates/base **only src/text**, verbatim: the oracle calls nothing of the kit but
# text::diff and text::lines, and that module uses only std and thiserror. The job builds it
# as a minimal `k2-mpc-base` (lib.rs = `pub mod text;`; the version, the thiserror
# requirement and the `conformance` feature of the real manifest), so the bundle stays small and the build skips the kit's other
# dependencies.
#
#   tools/undo-bundle.sh <quint|enum> <commands file> <out dir> [extra files...]
#
# writes <out dir>/job.sh (the job: pass it as `sh -ec "$(cat <out dir>/job.sh)"`) and
# <out dir>/env-args (the `--env NAME=VALUE` arguments that carry the chunks, one per
# line). tools/README.md shows the whole command. Nothing here is specific to one compute
# system: any runner that sets the environment variables and runs the job with sh works.
set -eu
KIND=${1:?usage: tools/undo-bundle.sh <quint|enum> <commands file> <out dir> [extra files]}
CMDS=${2:?commands file}
OUT=${3:?out dir}
shift 3
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
[ "$KIND" = quint ] && FILES="$FILES spec/quint tools/quint"
COPYFILE_DISABLE=1 tar --no-mac-metadata --no-xattrs --exclude target --exclude '*.itf.json' \
  -czf "$OUT/bundle.tgz" $FILES 2>/dev/null \
  || COPYFILE_DISABLE=1 tar --exclude target --exclude '*.itf.json' -czf "$OUT/bundle.tgz" $FILES
if command -v sha256sum >/dev/null; then SHA=$(sha256sum "$OUT/bundle.tgz" | cut -d' ' -f1)
else SHA=$(shasum -a 256 "$OUT/bundle.tgz" | cut -d' ' -f1); fi
base64 < "$OUT/bundle.tgz" | tr -d '\n' > "$OUT/bundle.b64"
TOTAL=$(wc -c < "$OUT/bundle.b64" | tr -d ' ')
: > "$OUT/env-args"
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
ROOT=\$PWD
# the workspace without the crates not shipped (the kit's other crates, vendor/), and the
# kit's text module (verbatim) as a minimal k2-mpc-base
sed -i 's|^members = .*|members = ["crates/base", "crates/spec-undo"]|' Cargo.toml
printf '[package]\nname = "k2-mpc-base"\nversion = "$VERSION"\nedition = "2021"\npublish = false\n\n[dependencies]\nthiserror = "$THISERROR"\n\n[features]\nconformance = []\n' > crates/base/Cargo.toml
echo 'pub mod text;' > crates/base/src/lib.rs
export CARGO_TARGET_DIR=\$ROOT/target
rustc --version; nproc; free -g | head -2
s=\$(date +%s)
cargo build -q --release --features mutants -p k2-mpc-spec-undo --bins
echo "=== build: wall \$(( \$(date +%s) - s ))s"
E() { \$ROOT/target/release/undo_enum "\$@"; }
R() { \$ROOT/target/release/undo_itf_replay "\$@"; }
timed() { l=\$1; shift; s=\$(date +%s); set +e; "\$@"; rc=\$?; set -e; echo "=== \$l: exit \$rc, wall \$(( \$(date +%s) - s ))s"; }
JOB
if [ "$KIND" = quint ]; then
cat >> "$OUT/job.sh" <<'JOB'
java -version 2>&1 | head -1; node --version
mkdir -p q && ( cd q && npm init -y >/dev/null && npm install -s --no-audit --no-fund @informalsystems/quint@${QUINT_VERSION:-0.33.0} >/dev/null )
export PATH=$ROOT/q/node_modules/.bin:$PATH
export NODE_OPTIONS=--max-old-space-size=${NODE_HEAP_MB:-4096}
echo "quint $(quint --version)"
q() { quint "$@"; }
# inst NAME PIDS EVENTS UNDOS REVERTS MAXLEN "INIT" MULTI PURGE MUTANT [RESTARTS]: an instance
# of `undo` (INIT like "0, 1, 1"; MULTI, PURGE true|false; MUTANT none or a mutant's name;
# RESTARTS default 0)
params() { printf 'PIDS = %s, EVENTS = %s, UNDOS = %s, REVERTS = %s, RESTARTS = %s, MAXLEN = %s, INIT = [%s], MULTI = %s, PURGE = %s, REDUCE = true, MUTANT = "%s"' "$2" "$3" "$4" "$5" "${11:-0}" "$6" "$7" "$8" "$9" "${10}"; }
inst() { printf 'module %s {\n  import undo(%s).* from "./undo"\n}\n' "$1" "$(params "$@")" > "$1.qnt"; }
# itfinst NAME ...same...: the instance plus `act` and `out` (the ITF wrapper; init winit, step wstep)
itfinst() { sed -e "s|__NAME__|$1|" -e "s|__PARAMS__|$(params "$@")|" "$ROOT/tools/undo/itf.qnt.in" > "$1.qnt"; }
cd spec/quint
# The guard against a Quint -> TLA+ export issue (tools/quint, v0.9.3), before anything else
# runs on the model: the source lint, then the exact export check of an instance of undo.qnt
# (its `quint compile --target tlaplus` fetches Apalache, so no TLC run comes first). A
# finding (exit 1) or a tool error (exit 2) stops the job; tools/undo/tlc.sh refuses to run
# TLC unless the check passed.
s=$(date +%s); set +e
sh "$ROOT/tools/quint/lint.sh" undo.qnt; rc=$?
set -e; echo "=== quint lint (undo.qnt): exit $rc, wall $(( $(date +%s) - s ))s"
[ $rc -eq 0 ] || { echo "=== the Quint export guard stopped the job (lint)"; exit 1; }
inst q_export_check 2 1 1 1 4 "0, 1, 1" true true none 1
s=$(date +%s); set +e
quint compile --target tlaplus --main=q_export_check q_export_check.qnt > /dev/null 2> q_export_check.cerr
# quint's own Apalache server (4 GiB) fails this model's transpile at random ("error:" and
# nothing else; tools/undo/tlc.sh): run the check against a server with tlc.sh's heap, then
# stop it so tlc.sh starts its own as before
APA_JAR=$(ls ~/.quint/apalache-dist-*/apalache/lib/apalache.jar 2>/dev/null | head -1)
EC_APA=""
if [ -n "$APA_JAR" ]; then
  # quint stops the server it started after its compile: wait for the port (as tlc.sh does)
  i=0; while bash -c 'exec 3<>/dev/tcp/127.0.0.1/8822' 2>/dev/null && [ $i -lt 60 ]; do sleep 1; i=$((i + 1)); done
  java -Xmx${APA_HEAP:-6g} -jar "$APA_JAR" server --port=8822 > "$ROOT/apalache-export-check.log" 2>&1 & EC_APA=$!
  i=0; until grep -q "running on port" "$ROOT/apalache-export-check.log" 2>/dev/null || [ $i -ge 120 ]; do sleep 1; i=$((i + 1)); done
fi
# a tool error (exit 2; quint's compile can fail at random on a cold Apalache server, as
# tools/undo/tlc.sh notes) is retried, up to 3 attempts; a finding (exit 1) is never retried
k=1
while :; do
  sh "$ROOT/tools/quint/lint.sh" --export --quiet q_export_check.qnt q_export_check; rc=$?
  [ $rc -eq 2 ] && [ $k -lt 3 ] || break
  echo "=== quint export check: tool error (attempt $k of 3), retrying"; k=$((k + 1)); sleep 5
done
if [ -n "$EC_APA" ]; then kill $EC_APA 2>/dev/null; wait $EC_APA 2>/dev/null; fi
set -e; echo "=== quint export check (undo.qnt): exit $rc, wall $(( $(date +%s) - s ))s"
[ $rc -eq 0 ] || { echo "=== the Quint export guard stopped the job (export check)"; exit 1; }
export QUINT_EXPORT_CHECKED=1
JOB
else
echo 'cd "$ROOT"' >> "$OUT/job.sh"
fi
echo ". \"\$ROOT/$CMDS\"" >> "$OUT/job.sh"
JOBSZ=$(wc -c < "$OUT/job.sh" | tr -d ' ')
echo "bundle: $(wc -c < "$OUT/bundle.tgz" | tr -d ' ') bytes gzip, $TOTAL base64, $NCHUNKS chunk(s) of <= $CHUNK; sha256 $SHA; job.sh $JOBSZ bytes" >&2
rm -f "$OUT/bundle.b64"
