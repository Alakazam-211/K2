#!/bin/bash
# tools/fuzz/bundle.sh <repo dir> <rev> <out dir> <corpus root> <secs> <target>...
#
# Writes a node job (<out dir>/job.sh, <out dir>/env-args) that fuzzes <target>s of the
# cargo-fuzz crate `fuzz/` of <repo dir> at <rev>, all at once (one libFuzzer process per
# target), for <secs> seconds each. Runs on Linux with a nightly toolchain (the job installs
# cargo-fuzz). tools/README.md, "Fuzzing".
#
# - **The code (FUZZ_SHIP=repo, the default):** the runner ships <repo dir> at <rev> itself
#   (`k2 compute run --sha <rev>` from inside <repo dir>, which must sit in the workspace;
#   tools/README.md, "Test jobs", says what that needs). Only the corpus travels in the job.
#   <out dir>/submit writes the command's code options: `--sha <full sha>`, and the folder
#   to run it from (`cd` there: K2 finds the repo, worktree or not). The job starts in the
#   checkout.
# - **FUZZ_SHIP=env (the fallback):** `git archive <rev>` travels in the job as env chunks
#   too. A repo with `vendor/yrs/VENDORED` (the kit) ships only the workspace manifest and
#   lock, `crates/base` without its tests, `vendor/yrs` without `src/` (rebuilt in the job
#   from the public crate and checked, as tools/test-bundle.sh does) and `fuzz/`; any other
#   repo ships whole. Full-line comments are blanked (strip-comments.py: line numbers kept).
# - **The corpus** travels in env chunks: `<corpus root>/<target>/corpus/*` (smallest files
#   first, as many as fit). It is private: it never enters a repo.
# - **The environment limit (hard check):** a runner caps a job at 64 environment pairs
#   (k2-node 0.46.6, `env_too_many`); the job's only pairs are the chunks, and the bundler
#   never writes more than ENV_LIMIT (60): the corpus is cut to fit, and when the source
#   alone (FUZZ_SHIP=env) doesn't fit it refuses and points to FUZZ_SHIP=repo.
# - **Results:** (a job stopped early keeps `out/<target>.partial.tgz`, refreshed with each
#   4-minute heartbeat: the corpus so far, the log, any crash) artifacts `out/<target>.tgz` (the corpus after `cargo fuzz cmin`, any crash
#   and its minimized form with `cargo fuzz fmt`'s rendering, the run's log), and one line
#   per target: `FUZZ <target> exit=… execs=… cov=… ft=… corpus=… crash=…`.
#
# REPLAY=1: instead of fuzzing, run each corpus input once and print `REPLAY <target>
# <input> ok|FAIL: <the broken property>` (regression inputs).
# COVERAGE=1: instead of fuzzing, measure what the corpus covers (`cargo fuzz coverage`,
# then `llvm-cov report` of the fuzzed repo's own sources): `out/<target>-coverage.tgz`, and
# the report's per-file lines in the log (`COV <target> <file> …`).
#
# Env: CHUNK (8000), MAX_CHUNKS (64), MALLOC ("target=MB …": -malloc_limit_mb per target,
# default the RSS limit), FLAGS ("target=-flag=v,-flag=v …": more libFuzzer flags per
# target), FEATURES (the fuzz crate's cargo features), TMIN_SECS (300: minimizing a crash),
# CMIN_SECS (600: minimizing the corpus), JOB_SECS (the job's timeout, default SECS + 1200:
# each target's record and archive come right after fuzzing; tmin and cmin only in the time
# left before it, less 4 minutes, so a late crash is never lost), RSS_MB (2048), MAX_LEN (4096), TIMEOUT_S (30: one input).
set -eu
REPO=$1; REV=$2; OUT=$3; CORPUS=$4; SECS=$5; shift 5
TARGETS="$*"
[ -n "$TARGETS" ] || { echo "bundle.sh: no targets" >&2; exit 2; }
CHUNK=${CHUNK:-8000}; ENV_LIMIT=${ENV_LIMIT:-60}; MAX_CHUNKS=${MAX_CHUNKS:-$ENV_LIMIT}
SHIP=${FUZZ_SHIP:-repo}
case $SHIP in repo|env) ;; *) echo "bundle.sh: FUZZ_SHIP is repo or env" >&2; exit 2 ;; esac
[ "$MAX_CHUNKS" -le "$ENV_LIMIT" ] || { echo "bundle.sh: MAX_CHUNKS $MAX_CHUNKS is over ENV_LIMIT $ENV_LIMIT (a runner caps a job at 64 environment pairs)" >&2; exit 3; }
HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$REPO" && pwd)
SHA_REV=$(git -C "$REPO" rev-parse --short=12 "$REV")
REPO_NAME=$(basename "$REPO")
rm -rf "$OUT"; mkdir -p "$OUT/s/src"
SHA_FULL=$(git -C "$REPO" rev-parse "$REV^{commit}")
KIT=0
if [ "$SHIP" = repo ]; then
  rmdir "$OUT/s/src"
  printf -- '--sha\n%s\n' "$SHA_FULL" > "$OUT/sha-args"
  printf '%s\n' "$REPO" > "$OUT/run-from"
elif git -C "$REPO" cat-file -e "$REV:vendor/yrs/VENDORED" 2>/dev/null; then
  KIT=1
  members=$(git -C "$REPO" ls-tree -r --name-only "$REV" crates/base/src \
    | grep -vE '(^|/)([a-z0-9_]+_tests|tests)\.rs$')
  # shellcheck disable=SC2086
  git -C "$REPO" archive "$REV" Cargo.toml Cargo.lock crates/base/Cargo.toml $members \
    vendor/yrs fuzz | tar -x -C "$OUT/s/src"
  rm -rf "$OUT/s/src/vendor/yrs/"{src,tests,benches,README.md}
else
  git -C "$REPO" archive "$REV" | tar -x -C "$OUT/s/src"
fi
if [ "$SHIP" = env ]; then
  [ -d "$OUT/s/src/crates" ] && python3 -I "$HERE/strip-comments.py" "$OUT/s/src/crates" "$OUT/s/src/fuzz" \
    || python3 -I "$HERE/strip-comments.py" "$OUT/s/src/fuzz"
fi

# the corpus: as much as fits, smallest files first
pack() {
  ( cd "$OUT/s" && COPYFILE_DISABLE=1 tar --no-mac-metadata --no-xattrs -cf - . 2>/dev/null \
    | xz -9e > "$OUT/b.txz" )
  echo $(( ( $(wc -c < "$OUT/b.txz") * 4 / 3 + CHUNK - 1 ) / CHUNK ))
}
for t in $TARGETS; do mkdir -p "$OUT/s/corpus/$t"; done
CAP=1000000
while :; do
  for t in $TARGETS; do
    rm -rf "$OUT/s/corpus/$t"; mkdir -p "$OUT/s/corpus/$t"
    d="$CORPUS/$t/corpus"
    [ -d "$d" ] || continue
    total=0
    while IFS= read -r line; do
      sz=${line%% *}; f=${line#* }
      total=$(( total + sz )); [ $total -le $CAP ] || break
      cp "$d/$f" "$OUT/s/corpus/$t/"
    done < <(cd "$d" && find . -maxdepth 1 -type f -exec stat -f '%z %N' {} + 2>/dev/null \
               | sed 's| \./| |' | sort -n)
  done
  n=$(pack)
  [ "$n" -le "$MAX_CHUNKS" ] && break
  if [ $CAP -le 4096 ]; then
    echo "bundle.sh: REFUSED: the source alone needs $n chunks, over $MAX_CHUNKS (a runner caps a job at 64 environment pairs; ENV_LIMIT $ENV_LIMIT)." >&2
    echo "  Ship the code with the runner instead: FUZZ_SHIP=repo (the default), <repo dir> inside the workspace (tools/README.md, \"Fuzzing\")." >&2
    exit 3
  fi
  CAP=$(( CAP * 2 / 3 ))
done
SHA=$(shasum -a 256 "$OUT/b.txz" | cut -d' ' -f1)
base64 < "$OUT/b.txz" | tr -d '\n' > "$OUT/b.b64"
TOTAL=$(wc -c < "$OUT/b.b64" | tr -d ' ')
: > "$OUT/env-args"; i=0; off=0
while [ $off -lt "$TOTAL" ]; do
  printf -- '--env\nBUNDLE_B64_%s=%s\n' $i "$(dd if="$OUT/b.b64" bs=$CHUNK skip=$i count=1 2>/dev/null)" >> "$OUT/env-args"
  i=$((i+1)); off=$((off+CHUNK))
done
N=$i; rm -f "$OUT/b.b64"
SEEDS=$(for t in $TARGETS; do printf '%s=%s ' "$t" "$(ls "$OUT/s/corpus/$t" | wc -l | tr -d ' ')"; done)

cat > "$OUT/job.sh" <<JOB
set -eu
: > b.b64; i=0; while [ \$i -lt $N ]; do eval "c=\\\${BUNDLE_B64_\$i}"; printf '%s' "\$c" >> b.b64; i=\$((i+1)); done
base64 -d < b.b64 > b.txz; echo "$SHA  b.txz" | sha256sum -c - >/dev/null
mkdir -p w out && tar xJf b.txz -C w && rm -f b.b64 b.txz
OUTD=\$PWD/out
W=\$PWD/w
if [ $SHIP = repo ]; then
  # the runner's checkout is the job's folder: the code is here, the corpus in w/ (w/src
  # points back here, for the heartbeat's archive)
  SRC=\$PWD; ln -sfn "\$SRC" "\$W/src"
else
  SRC=\$W/src
fi
cd "\$SRC"
if [ $KIT = 1 ]; then
  set -- \$(sed -n 's/^crate //p' vendor/yrs/VENDORED); YC=\$1; YSHA=\$2
  curl -sSfL --retry 3 -o yrs.crate "https://static.crates.io/crates/\${YC%-*}/\$YC.crate"
  echo "\$YSHA  yrs.crate" | sha256sum -c - >/dev/null
  mkdir -p yc && tar xzf yrs.crate -C yc && mv "yc/\$YC/src" vendor/yrs/src && rm -rf yrs.crate yc
  ( cd vendor && patch -p1 -s --no-backup-if-mismatch < yrs/FORK.patch )
  YSUM=\$(cd vendor/yrs/src && find . -type f | LC_ALL=C sort | sed 's|^\./||' | while read -r f; do sha256sum "\$f"; done | sha256sum | cut -d' ' -f1)
  [ "\$YSUM" = "\$(sed -n 's/^src-sha256 //p' vendor/yrs/VENDORED)" ] || { echo "vendor/yrs: rebuilt src differs" >&2; exit 1; }
  sed -i '/^\[\[bench\]\]/,/^\$/d' vendor/yrs/Cargo.toml
  sed -i 's|^members = .*|members = ["crates/base"]|' Cargo.toml
fi
END=\$(( \$(date +%s) + ${JOB_SECS:-$(( SECS + 1200 ))} - 240 ))
export CARGO_TARGET_DIR=\$W/target CARGO_BUILD_JOBS=4
FF="${FEATURES:+--features $FEATURES}"
rustc --version
command -v cargo-fuzz >/dev/null || cargo install -q --locked cargo-fuzz@0.13.2
echo "fuzz: $REPO_NAME at $SHA_REV; targets $TARGETS; ${SECS}s each; seeds $SEEDS"
if [ "${COVERAGE:-0}" = 1 ]; then
  # llvm-tools must match rustc's LLVM; a read-only shared toolchain can't take the
  # component, so the job then installs its own nightly with it
  if ! rustup component add llvm-tools-preview >/dev/null 2>&1; then
    export RUSTUP_HOME=\$W/rustup
    rustup toolchain install nightly --profile minimal -c llvm-tools-preview >/dev/null 2>&1
    rustup default nightly >/dev/null
  fi
  rustc --version
  LLVM=\$(ls -d "\$(rustc --print sysroot)"/lib/rustlib/*/bin | head -1)
  ls "\$LLVM"/llvm-profdata >/dev/null || { echo "coverage: no llvm-profdata" >&2; exit 1; }
  for t in $TARGETS; do
    mkdir -p "\$W/corpus/\$t" "\$W/logs"
    cargo fuzz coverage -a \$FF "\$t" "\$W/corpus/\$t" > "\$W/logs/\$t.cov.log" 2>&1 || { tail -30 "\$W/logs/\$t.cov.log"; continue; }
    bin=\$(find "\$W" -path '*/coverage/*/release/*' -type f -name "\$t" -perm -u+x | head -1)
    prof=\$(find . "\$W" -name coverage.profdata -path "*\$t*" | head -1)
    "\$LLVM/llvm-cov" report "\$bin" -instr-profile="\$prof" \
      -ignore-filename-regex='(/rustc/|\.cargo/registry|/vendor/|/fuzz/)' > "\$W/logs/\$t.report.txt" 2>&1 || true
    sed -n "s|^.*/src/\(crates/[^ ]*\)|COV \$t \1|p" "\$W/logs/\$t.report.txt"
    grep '^TOTAL' "\$W/logs/\$t.report.txt" | sed "s/^/COV \$t /"
    "\$LLVM/llvm-cov" export "\$bin" -instr-profile="\$prof" -format=lcov \
      -ignore-filename-regex='(/rustc/|\.cargo/registry|/vendor/|/fuzz/)' > "\$W/logs/\$t.lcov" 2>/dev/null || true
    tar czf "\$OUTD/\$t-coverage.tgz" -C "\$W/logs" "\$t.report.txt" "\$t.lcov" "\$t.cov.log"
  done
  exit 0
fi
s=\$(date +%s); cargo fuzz build -O -a \$FF; echo "=== build: wall \$(( \$(date +%s) - s ))s"
if [ "${REPLAY:-0}" = 1 ]; then
  # each input of the corpus once, on its own: ok, or the property it breaks
  RC=0
  for t in $TARGETS; do for f in "\$W/corpus/\$t"/*; do
    out=\$(cargo fuzz run -O -a \$FF "\$t" "\$f" -- -runs=1 2>&1) && echo "REPLAY \$t \$(basename "\$f") ok" && continue
    RC=1; echo "REPLAY \$t \$(basename "\$f") FAIL: \$(echo "\$out" | grep -aA2 'panicked at' | grep -av 'panicked at' | head -2 | tr '\n' ' ' | cut -c1-400)"
  done; done
  exit \$RC
fi
malloc() { for kv in ${MALLOC:-}; do [ "\${kv%%=*}" = "\$1" ] && { echo "\${kv#*=}"; return; }; done; echo ${RSS_MB:-2048}; }
flags() { for kv in ${FLAGS:-}; do [ "\${kv%%=*}" = "\$1" ] && echo "\${kv#*=}" | tr , ' '; done; }
run() {
  t=\$1; mkdir -p "\$W/corpus/\$t" "\$W/logs"
  set +e
  cargo fuzz run -O -a \$FF "\$t" "\$W/corpus/\$t" -- -max_total_time=$SECS -rss_limit_mb=${RSS_MB:-2048} \
    -malloc_limit_mb=\$(malloc "\$t") -max_len=${MAX_LEN:-4096} -timeout=${TIMEOUT_S:-30} \
    -use_value_profile=1 -print_final_stats=1 \$(flags "\$t") > "\$W/logs/\$t.log" 2>&1
  echo \$? > "\$W/logs/\$t.rc"
}
PIDS=""; for t in $TARGETS; do run "\$t" & PIDS="\$PIDS \$!"; done
# a heartbeat every 4 minutes (the node stops a job that prints nothing): each target's last
# status line
( while sleep 240; do for t in $TARGETS; do
    echo "beat \$t: \$(grep -aE '^#[0-9]+' "\$W/logs/\$t.log" 2>/dev/null | tail -1 | cut -c1-150)"
    # a job stopped early still ships its corpus so far (and any crash)
    tar czf "\$OUTD/\$t.partial.tgz" -C "\$W" "corpus/\$t" "logs/\$t.log" \
      \$(cd "\$W" && ls -d "src/fuzz/artifacts/\$t" 2>/dev/null) 2>/dev/null || true
  done; done ) & BEAT=\$!
for p in \$PIDS; do wait \$p || true; done
RC=0
# the job's deadline (its timeout less a margin to pack and ship): each target's record and
# archive come first, right after fuzzing; minimizing the crash and the corpus only in the
# time left, each under a hard cap, so a late crash is never lost to the job's timeout
left() { echo \$(( END - \$(date +%s) )); }
cap() { l=\$(( \$(left) - 120 )); [ \$l -lt \$1 ] && echo \$l || echo \$1; }
pack() {
  rm -rf "\$W/pack/\$1"; mkdir -p "\$W/pack/\$1"; cp -r "\$W/corpus/\$1" "\$W/pack/\$1/corpus"
  cp "\$W/logs/\$1".* "\$W/pack/\$1/" 2>/dev/null || true
  [ -d "fuzz/artifacts/\$1" ] && cp -r "fuzz/artifacts/\$1" "\$W/pack/\$1/crashes"
  tar czf "\$OUTD/\$1.tgz" -C "\$W/pack" "\$1"
}
for t in $TARGETS; do
  rc=\$(cat "\$W/logs/\$t.rc"); crash=-
  a=fuzz/artifacts/\$t
  # the input libFuzzer reported (fork mode and tmin write others beside it)
  c=\$(grep -ao 'Test unit written to [^ ]*' "\$W/logs/\$t.log" | head -1 | sed 's|.*/||' || true)
  [ -n "\$c" ] && [ -f "\$a/\$c" ] || c=\$(ls -tr "\$a" 2>/dev/null | grep -E '^(crash|leak|timeout|oom|slow-unit)-' | head -1 || true)
  if [ -n "\$c" ]; then crash=\$c; RC=1
  elif [ "\$rc" != 0 ]; then RC=1; crash="exit-\$rc"
  fi
  st=\$(grep -E '^#[0-9]+' "\$W/logs/\$t.log" | tail -1)
  execs=\$(sed -n 's/^stat::number_of_executed_units: //p' "\$W/logs/\$t.log" | tail -1)
  cov=\$(echo "\$st" | sed -n 's/.* cov: \([0-9]*\).*/\1/p'); ft=\$(echo "\$st" | sed -n 's/.* ft: \([0-9]*\).*/\1/p')
  rss=\$(sed -n 's/^stat::peak_rss_mb: //p' "\$W/logs/\$t.log" | tail -1)
  echo "FUZZ \$t exit=\$rc execs=\${execs:-?} cov=\${cov:-?} ft=\${ft:-?} corpus=\$(ls "\$W/corpus/\$t" | wc -l) rss_mb=\${rss:-?} crash=\$crash"
  [ "\$crash" = - ] || { tail -60 "\$W/logs/\$t.log"; }
  pack "\$t"
done
for t in $TARGETS; do
  a=fuzz/artifacts/\$t
  c=\$(grep -ao 'Test unit written to [^ ]*' "\$W/logs/\$t.log" | head -1 | sed 's|.*/||' || true)
  if [ -n "\$c" ] && [ -f "\$a/\$c" ]; then
    cargo fuzz fmt -O -a \$FF "\$t" "\$a/\$c" > "\$a/\$c.txt" 2>&1 || true
    cat "\$a/\$c.txt" 2>/dev/null | head -60
    if [ \$(left) -gt 240 ]; then
      timeout \$(cap ${TMIN_SECS:-300}) cargo fuzz tmin -O -a \$FF "\$t" "\$a/\$c" -r 20000 \
        > "\$W/logs/\$t.tmin.log" 2>&1 || true
      m=\$(ls -t "\$a" | grep '^minimized-from-' | head -1 || true)
      [ -n "\$m" ] && cargo fuzz fmt -O -a \$FF "\$t" "\$a/\$m" > "\$a/\$m.txt" 2>&1 || true
    else echo "tmin \$t: skipped (\$(left) s left)"; fi
  fi
  # the corpus minimized on a copy, kept only when it finished in time
  if [ \$(left) -gt 300 ]; then
    rm -rf "\$W/cmin"; cp -r "\$W/corpus/\$t" "\$W/cmin"
    if timeout \$(cap ${CMIN_SECS:-600}) cargo fuzz cmin -O -a \$FF "\$t" "\$W/cmin" > "\$W/logs/\$t.cmin.log" 2>&1; then
      rm -rf "\$W/corpus/\$t"; mv "\$W/cmin" "\$W/corpus/\$t"
    else echo "cmin \$t: not finished in time; the corpus ships as fuzzed"; fi
  else echo "cmin \$t: skipped (\$(left) s left)"; fi
  pack "\$t"
done
kill \$BEAT 2>/dev/null || true
rm -f "\$OUTD"/*.partial.tgz
exit \$RC
JOB
echo "bundle: $(( $(wc -c < "$OUT/b.txz") / 1024 )) KB xz, $N chunks; seeds: $SEEDS"
