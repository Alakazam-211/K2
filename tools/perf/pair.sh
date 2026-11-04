# The performance harness's job (tools/README.md: Performance harness): a commands file for tools/job.sh.
#
#   sh tools/job.sh tools/perf/pair.sh
#
# Builds side A (this checkout) and, when the commit carries one (tools/perf/pair-commit.sh),
# side B (`.perf-b/`, another commit's whole tree), each twice: the plain build (no kit
# features: every row but one) and the `nosync` build (`edit_nosync` and the live-run count
# only). Both sides get the same harness source, the same flags and their own tree's
# Cargo.lock. Then each side grows its own fixtures from the same seed, and each size is
# measured in PERF_RUNS runs per side, interleaved A, B, A, B on this one node. The summary
# takes the median of the runs per side and the B/A ratio.
#
# Env (all optional):
#   PERF_SIZES    fixture sizes in versions (default 10000,100000,1000000)
#   PERF_RUNS     runs per side per size (default 5)
#   PERF_SAMPLES  samples per row per run (default 10000)
#   PERF_SEED     the growth's and the runs' seed (default 1)
#   PERF_LARGE    1: also the large-doc fixture (about 3 MiB of text at PERF_LARGE_VERSIONS,
#                 default 10000) and its content rows (default 1)
#   PERF_AA       1 with no .perf-b: side B is side A's binaries again (the noise floor)
#   PERF_A_LABEL, PERF_B_LABEL   the builds' labels in the records (default: from .perf-pair)
#   PERF_SLOW_CAP_S, PERF_COLD_CAP_S, PERF_FAST_CAP_S   a row's timed seconds per run
#                 (defaults 60, 60, 900); a row that stops short says so (n, indicative p99)
#   PERF_MAX_S    one measure run's wall-time stop in seconds (default 3600; the run records max_s_hit)
#   PERF_GROW_ONLY 1: build and grow, print the fixtures' records, measure nothing
#   PERF_WINDOW   the growth's record interval in versions (default 50000)
#   PERF_HOOKED   1: also A's kit with its test hooks on (differential + conformance, as
#                 the kit's tests and the soak build it), on A's fixture, recorded as side H:
#                 label-only, the gap to the hooked figures quoted before (perfh_hookgap_*)
#   PERF_NODE_CLASS  the node's class for every record (a disk/CPU class, never a host
#                 name; absolute figures depend on it: give it on every submit)
#   PERF_FIX_CACHE  where the fixtures live (default perf-out/fix): give a node cache
#                 (`--cache perf-fixtures`, then "$K2_JOB_CACHE/fixtures") to grow in one job and
#                 measure in a later one on the same node; keyed by build label and seed
#   PERF_NO_GROW  1: measure cached fixtures only; a missing one (a node may evict an idle
#                 cache when it needs the disk) stops the job before any measuring
#   PERF_SAME_SOURCE 1: side B is another build of side A's source, on purpose (a floor)
#   PERF_OWN_FIXTURES 1: each side grows its own fixtures (default: both sides read side A's)
#   PERF_GROW_PAR 1: the sides grow at once (with PERF_OWN_FIXTURES)
#   PERF_SKIP_H   1: no history fixtures and no history rows (a large-doc-only job)
#   PERF_SIZES_MEASURE  the sizes to measure (default PERF_SIZES): grow 1M in one job, measure
#                 it in another
#   PERF_TIMERS_A, PERF_TIMERS_B  on|off (default off): the kit's own timers in that side's
#                 plain runs (a kit with crates/base/src/timers.rs); on adds the <row>.<timer>
#                 rows and <row>.no_commit (the call less its durable commit)
#   PERF_TIMERS_CHECK  A or B: run `timers-check` on that side first (a kit with timers)
#   PERF_LINT     1: fmt --check and clippy on the harness first (the compile check)
OUT=$ROOT/perf-out
WB=$ROOT/../perf-work
mkdir -p "$OUT/bin" "$OUT/fix" "$WB"
SIZES=${PERF_SIZES:-10000,100000,1000000}
RUNS=${PERF_RUNS:-5}
SAMPLES=${PERF_SAMPLES:-10000}
SEED=${PERF_SEED:-1}
LARGE=${PERF_LARGE:-1}
LARGE_V=${PERF_LARGE_VERSIONS:-10000}
CAPS="--slow-cap-s ${PERF_SLOW_CAP_S:-60} --cold-cap-s ${PERF_COLD_CAP_S:-60} --fast-cap-s ${PERF_FAST_CAP_S:-900} --max-s ${PERF_MAX_S:-3600}"
pair_label() { if [ -f "$ROOT/.perf-pair" ]; then sed -n "s/^$1=//p" "$ROOT/.perf-pair" | head -1; fi; }
A_LABEL=${PERF_A_LABEL:-$(pair_label A)}; A_LABEL=${A_LABEL:-A}
B_LABEL=${PERF_B_LABEL:-$(pair_label B)}; B_LABEL=${B_LABEL:-B}
SIDES=A
[ -d "$ROOT/.perf-b" ] && SIDES="A B"
[ "${PERF_AA:-0}" = 1 ] && [ ! -d "$ROOT/.perf-b" ] && { SIDES="A B"; B_LABEL="$A_LABEL (A/A)"; }
echo "perf: node class ${PERF_NODE_CLASS:-unnamed}"
echo "perf: sides $SIDES (A=$A_LABEL B=$B_LABEL), sizes $SIZES, $RUNS runs, $SAMPLES samples, seed $SEED, large $LARGE"
df -h "$ROOT" "$WB" 2>/dev/null | sed 's/^/perf: disk /'
# the same flags for every build: the repo's own cargo config, given explicitly so a nested
# tree (.perf-b/) adds nothing of its own
export RUSTFLAGS="--cfg k2_mpc_untrusted_wire_ack"

build_side() { # SIDE TREE
  ( set -e
    cd "$2/tools/perf"
    cp ../../Cargo.lock Cargo.lock
    # a kit with its own timers (v0.10.1 on): the harness reads them (no kit feature)
    T=""
    if [ -f ../../crates/base/src/timers.rs ]; then T=kit-timers; echo "perf: side $1's kit has timers"; fi
    build_one "$1" plain "$T"
    build_one "$1" nosync "nosync $T"
    if [ "$1" = A ] && [ "${PERF_HOOKED:-0}" = 1 ]; then build_one A hooked "hooks $T"; fi
    # the vendored engine compiles once a side (its features don't change between the kinds)
    here=$(cd ../.. && pwd)
    if [ -d ../../vendor/yrs ] && ! cat "$OUT"/build-"$1"-*.log | grep -q "Compiling yrs .*($here/vendor/yrs)"; then
      rm -f "$OUT"/bin/"$1"-*
      echo "perf: STALE BUILD: cargo didn't compile yrs for side $1 from $here in this job: not measuring it"
      exit 1
    fi )
}
# one binary, built now, from this tree: a target dir outlives a job, so cargo finding nothing to
# do would measure an earlier job's binary (source files with old times, a tree shipped by `git
# archive`). The harness alone recompiling proves nothing (a change of its own features dirties only
# it, and it then links a kit built earlier): the kit must compile too, from this tree. Not so:
# no binary, and the job says which crate.
build_one() { # SIDE NAME FEATURES
  blog="$OUT/build-$1-$2.log"
  { cargo build --release --features "$3" 2>&1; echo $? > "$blog.rc"; } | tee "$blog"
  [ "$(cat "$blog.rc")" = 0 ] || { echo "perf: side $1's $2 build failed"; return 1; }
  here=$(cd ../.. && pwd)
  for c in "k2-mpc-perf:tools/perf" "k2-mpc-base:crates/base"; do
    if ! grep -q "Compiling ${c%%:*} .*($here/${c#*:})" "$blog"; then
      echo "perf: STALE BUILD: cargo didn't compile ${c%%:*} for side $1's $2 binary from $here in this job: not measuring it"
      return 1
    fi
  done
  cp "$CARGO_TARGET_DIR/release/k2-mpc-perf" "$OUT/bin/$1-$2"
}
# what a side's measured code is, whatever the build's paths: the kit, its vendored trees, the lock
# and the harness, by content (a binary embeds its paths, so two builds of one source differ)
src_sha() { # TREE
  ( cd "$1" && find crates/base vendor Cargo.lock tools/perf/src tools/perf/Cargo.toml -type f 2>/dev/null \
      | LC_ALL=C sort | while IFS= read -r f; do sha256sum "$f"; done | sha256sum | cut -c1-16 )
}
bin_sha() { sha256sum "$1" | cut -c1-16; }
build_records() { # one record per binary; both sides' identity, in every job
  job=null; [ -z "${K2_COMPUTE_JOB:-}" ] || job="\"$K2_COMPUTE_JOB\""
  for b in "$OUT"/bin/*; do
    n=$(basename "$b"); sd=${n%%-*}
    tree=$ROOT; [ "$sd" = B ] && [ -d "$ROOT/.perf-b" ] && tree=$ROOT/.perf-b
    echo "RUNREC {\"job\":$job,\"run\":\"perfh_build_$n\",\"tool\":\"perf_harness\",\"status\":\"COMPLETE\",\"node_class\":\"${PERF_NODE_CLASS:-unnamed}\",\"bin_sha\":\"$(bin_sha "$b")\",\"src_sha\":\"$(src_sha "$tree")\",\"label\":\"$(label "$sd")\",\"bytes\":$(wc -c < "$b" | tr -d ' ')}"
  done
}
# a job meant to compare two builds must have two: fails loudly on one binary measured twice. A/A
# (PERF_AA, or one tree) is one binary by design, and says so. Two builds of one source (a floor
# of the build's own layout) must say so too: PERF_SAME_SOURCE=1.
check_two_builds() {
  if [ ! -d "$ROOT/.perf-b" ]; then
    case $SIDES in
      "A B") echo "perf: A/A: side B is side A's binary, by design (PERF_AA=1): this floor has no build-to-build difference in it" ;;
      *) echo "perf: one side only" ;;
    esac
    return 0
  fi
  [ -x "$OUT/bin/B-plain" ] && [ -x "$OUT/bin/B-nosync" ] || { echo "perf: TWO BUILDS EXPECTED: side B has no binary"; return 1; }
  for v in plain nosync; do
    if [ "$(bin_sha "$OUT/bin/A-$v")" = "$(bin_sha "$OUT/bin/B-$v")" ]; then
      echo "perf: TWO BUILDS EXPECTED: A-$v and B-$v are one binary ($(bin_sha "$OUT/bin/A-$v"))"; return 1
    fi
  done
  if [ "$(src_sha "$ROOT")" = "$(src_sha "$ROOT/.perf-b")" ]; then
    if [ "${PERF_SAME_SOURCE:-0}" = 1 ]; then
      echo "perf: two builds of ONE source, by request (PERF_SAME_SOURCE=1): a floor, not a comparison"
    else
      echo "perf: TWO BUILDS EXPECTED: sides A and B are one source ($(src_sha "$ROOT")); PERF_SAME_SOURCE=1 if that is the point"; return 1
    fi
  else
    echo "perf: two builds, two sources: A $(src_sha "$ROOT"), B $(src_sha "$ROOT/.perf-b")"
  fi
}
if [ -d "$ROOT/.perf-b" ]; then
  # the same harness source on B's kit
  rm -rf "$ROOT/.perf-b/tools/perf" && mkdir -p "$ROOT/.perf-b/tools" && cp -R "$ROOT/tools/perf" "$ROOT/.perf-b/tools/perf"
  rm -f "$ROOT/.perf-b/tools/perf/Cargo.lock"
fi
if [ "${PERF_LINT:-0}" = 1 ]; then
  # the compile check's lint: fmt and clippy on the harness (warnings printed, not fatal)
  cp "$ROOT/Cargo.lock" "$ROOT/tools/perf/Cargo.lock"
  timed fmt lint cargo fmt --manifest-path "$ROOT/tools/perf/Cargo.toml" --check
  timed clippy lint cargo clippy --manifest-path "$ROOT/tools/perf/Cargo.toml" --release --features "nosync hooks $([ -f "$ROOT/crates/base/src/timers.rs" ] && echo kit-timers)"
fi
timed build_A build_side A "$ROOT"
if [ -d "$ROOT/.perf-b" ]; then
  timed build_B build_side B "$ROOT/.perf-b"
elif [ "$SIDES" = "A B" ]; then
  cp "$OUT/bin/A-plain" "$OUT/bin/B-plain" && cp "$OUT/bin/A-nosync" "$OUT/bin/B-nosync"
fi
if [ ! -x "$OUT/bin/A-plain" ] || [ ! -x "$OUT/bin/A-nosync" ]; then
  echo "perf: side A didn't build; nothing to measure"
  return 0
fi
timers_of() { if [ "$1" = B ]; then echo "${PERF_TIMERS_B:-off}"; else echo "${PERF_TIMERS_A:-off}"; fi; }
label() { if [ "$1" = A ]; then echo "$A_LABEL"; else echo "$B_LABEL"; fi; }
build_records
if ! check_two_builds; then RC=1; echo "perf: nothing measured"; return 0; fi
# which binary each run measured, in its own record (the fixture's key stays the plain label)
tagged() { echo "$(label "$1") bin=$(bin_sha "$OUT/bin/$1-$2")"; }

if [ -n "${PERF_TIMERS_CHECK:-}" ]; then
  timed timers_check "$OUT/bin/$PERF_TIMERS_CHECK-plain" timers-check --work "$WB/timers-check"
fi
# fixtures: by default one fixture, grown by side A, read by both sides (same-seed fixtures aren't
# byte-identical: tools/README.md, Performance harness, known issues, so sides that grow their
# own compare the fixtures as well as the builds). PERF_OWN_FIXTURES=1: each side grows its
# own, for a build that can't read A's store. Into FIXROOT/<label>-s<seed>/<kind>/
# (PERF_FIX_CACHE: a node cache, so one job grows and a later job on the same node measures;
# a fixture set with its done marker is reused, never regrown; a partial one is regrown)
FIXROOT=${PERF_FIX_CACHE:-$OUT/fix}
fix_owner() { if [ "${PERF_OWN_FIXTURES:-0}" = 1 ]; then echo "$1"; else echo A; fi; }
fixdir() { printf '%s/%s' "$FIXROOT" "$(printf '%s-s%s' "$(label "$(fix_owner "$1")")" "$SEED" | tr -c 'A-Za-z0-9._-' '_')"; }
GROW_SIDES=A
[ "${PERF_OWN_FIXTURES:-0}" = 1 ] && GROW_SIDES=$SIDES
grow_one() { # SIDE KIND SIZES EXTRA...
  side=$1; kind=$2; sizes=$3; shift 3
  d=$(fixdir "$side")/$kind
  mark="$d/done-$(echo "$sizes" | tr ',' '-')"
  if [ -f "$mark" ]; then
    # a cached fixture is its grower's: keyed by label and seed, so a label reused on another kit
    # would read it silently
    want=$(src_sha "$ROOT")
    if [ -f "$d/grown-src" ] && [ "$(cat "$d/grown-src")" != "$want" ]; then
      echo "perf: FIXTURE MISMATCH: $d was grown from source $(cat "$d/grown-src"), this job's is $want"; return 1
    fi
    [ -f "$d/grown-src" ] || echo "perf: note: $d doesn't record its grower's source (made before this check)"
    echo "perf: $side $kind fixtures from the cache ($d)"; return 0
  fi
  if [ "${PERF_NO_GROW:-0}" = 1 ]; then
    echo "perf: $side $kind fixtures missing from $d (an evicted cache?), and PERF_NO_GROW=1: nothing grown, nothing measured"
    return 1
  fi
  rm -rf "$d"
  "$OUT/bin/$side-plain" grow --work "$WB/$side-$kind" --out "$d" --sizes "$sizes" --seed "$SEED" \
    --side "$side" --name "$kind" "$@" && src_sha "$ROOT" > "$d/grown-src" && touch "$mark"
}
grow_side() { # SIDE
  [ "${PERF_SKIP_H:-0}" = 1 ] || grow_one "$1" h "$SIZES" --window "${PERF_WINDOW:-50000}" || return 1
  if [ "$LARGE" = 1 ]; then
    grow_one "$1" large "$LARGE_V" --lines 39000 --line-len 80 --window 2000 || return 1
  fi
}
if [ "${PERF_GROW_PAR:-0}" = 1 ]; then
  # the sides grow at once (growth is never timed; each is one thread)
  # wait on the growths by pid: a bare `wait` would also wait on tools/job.sh's heartbeat
  # loop (a background job of the same shell: this file is sourced), which never ends
  GROW_PIDS=""
  for side in $GROW_SIDES; do ( grow_side "$side"; echo $? > "$WB/grow-$side.rc" ) & GROW_PIDS="$GROW_PIDS $!"; done
  wait $GROW_PIDS
  for side in $GROW_SIDES; do echo "=== grow_$side: exit $(cat "$WB/grow-$side.rc"), parallel"; [ "$(cat "$WB/grow-$side.rc")" = 0 ] || { RC=1; GROW_FAIL=1; }; done
else
  for side in $GROW_SIDES; do timed "grow_$side" grow_side "$side"; [ "$rc" = 0 ] || GROW_FAIL=1; done
fi
du -sh "$FIXROOT"/* "$WB"/* 2>/dev/null | sed 's/^/perf: size /'
if [ "${GROW_FAIL:-0}" = 1 ]; then
  echo "perf: a side's fixtures are missing or failed to grow; nothing measured"
  return 0
fi

measure_pair() { # FIXTURE_NAME SIZE ROWS KIND (h or large)
  k=1
  while [ "$k" -le "$RUNS" ]; do
    for side in $SIDES; do
      timed "m_$1_$side$k" "$OUT/bin/$side-plain" measure --timers "$(timers_of "$side")" --fixture "$(fixdir "$side")/$4/v$2" --work "$WB/$side-$4" \
        --name "$1" --size "$2" --side "$side" --run "$k" --build "$(tagged "$side" plain)" --rows "$3" \
        --samples "$SAMPLES" --seed "$SEED" --tsv "$OUT/samples.tsv" $CAPS
      timed "n_$1_$side$k" "$OUT/bin/$side-nosync" measure --fixture "$(fixdir "$side")/$4/v$2" --work "$WB/$side-$4" \
        --name "$1" --size "$2" --side "$side" --run "$k" --build "$(tagged "$side" nosync)" --rows nosync \
        --samples "$SAMPLES" --seed "$SEED" --tsv "$OUT/samples.tsv" $CAPS
      if [ "$side" = A ] && [ "${PERF_HOOKED:-0}" = 1 ] && [ "$3" != nosync ]; then
        # label-only: A's kit with the test hooks on, A's fixture, as side H
        timed "h_$1_H$k" "$OUT/bin/A-hooked" measure --fixture "$(fixdir A)/$4/v$2" --work "$WB/A-$4" \
          --name "$1" --size "$2" --side H --run "$k" --build "$A_LABEL+hooks bin=$(bin_sha "$OUT/bin/A-hooked")" --rows "$3" \
          --samples "$SAMPLES" --seed "$SEED" --tsv "$OUT/samples.tsv" $CAPS
      fi
    done
    k=$((k + 1))
  done
}
if [ "${PERF_GROW_ONLY:-0}" != 1 ]; then
  for size in $(echo "${PERF_SIZES_MEASURE:-$SIZES}" | tr ',' ' '); do
    [ "${PERF_SKIP_H:-0}" = 1 ] || measure_pair "h$size" "$size" history h
  done
  [ "$LARGE" = 1 ] && measure_pair "large$LARGE_V" "$LARGE_V" content large
  timed summary "$OUT/bin/A-plain" summarize --tsv "$OUT/samples.tsv"
fi
rm -rf "$WB"
