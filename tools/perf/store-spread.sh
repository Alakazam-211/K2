# Why same-seed fixtures differ in size (tools/README.md, Performance harness, known issues): a
# commands file for tools/job.sh. Grows PERF_SPREAD (default 8) fixtures at 10k and PERF_SPREAD_100K
# (default 2) at 100k from one binary and one seed, and prints each store's redb statistics.
settings PERF_SPREAD=8 PERF_SPREAD_100K=2
OUT=$ROOT/perf-out; WB=$ROOT/../perf-work; mkdir -p "$OUT/bin" "$WB"
export RUSTFLAGS="--cfg k2_mpc_untrusted_wire_ack"
build() { ( set -e; cd tools/perf; cp ../../Cargo.lock Cargo.lock
  cargo build --release; cp "$CARGO_TARGET_DIR/release/k2-mpc-perf" "$OUT/bin/plain"
  cargo build --release --features nosync; cp "$CARGO_TARGET_DIR/release/k2-mpc-perf" "$OUT/bin/nosync" ); }
timed build build
# spread SIZE COUNT: a grow or a store-stats that fails fails the step (timed runs it with set +e)
spread() { # SIZE COUNT
  [ "$2" -ge 1 ] 2>/dev/null || { echo "spread: count '$2': nothing to run"; return 2; }
  i=1
  while [ "$i" -le "$2" ]; do
    "$OUT/bin/plain" grow --work "$WB/w" --out "$WB/f$1-$i" --sizes "$1" --seed 1 --side "S$i" --name "s$1" --window 1000000 || return 1
    "$OUT/bin/nosync" store-stats --store "$WB/f$1-$i/v$1/docs.redb" --name "$1_$i" || return 1
    rm -rf "$WB/f$1-$i"
    i=$((i + 1))
  done
}
# a count of 0 leaves a size out: said as skipped, never a step that ran nothing
spread_step() { # LABEL SIZE COUNT
  if [ "$3" = 0 ]; then echo "=== $1: skipped (count 0)"; else timed "$1" spread "$2" "$3"; SPREAD_RAN=1; fi
}
spread_step spread_10k 10000 "${PERF_SPREAD:-8}"
spread_step spread_100k 100000 "${PERF_SPREAD_100K:-2}"
[ -n "${SPREAD_RAN:-}" ] || { echo "spread: both counts are 0: nothing measured"; RC=1; }
rm -rf "$WB"
