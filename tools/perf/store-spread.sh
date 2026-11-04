# Why same-seed fixtures differ in size (tools/README.md, Performance harness, known issues): a
# commands file for tools/job.sh. Grows PERF_SPREAD (default 8) fixtures at 10k and PERF_SPREAD_100K
# (default 2) at 100k from one binary and one seed, and prints each store's redb statistics.
OUT=$ROOT/perf-out; WB=$ROOT/../perf-work; mkdir -p "$OUT/bin" "$WB"
export RUSTFLAGS="--cfg k2_mpc_untrusted_wire_ack"
build() { ( set -e; cd tools/perf; cp ../../Cargo.lock Cargo.lock
  cargo build --release; cp "$CARGO_TARGET_DIR/release/k2-mpc-perf" "$OUT/bin/plain"
  cargo build --release --features nosync; cp "$CARGO_TARGET_DIR/release/k2-mpc-perf" "$OUT/bin/nosync" ); }
timed build build
spread() { # SIZE COUNT
  i=1
  while [ "$i" -le "$2" ]; do
    "$OUT/bin/plain" grow --work "$WB/w" --out "$WB/f$1-$i" --sizes "$1" --seed 1 --side "S$i" --name "s$1" --window 1000000
    "$OUT/bin/nosync" store-stats --store "$WB/f$1-$i/v$1/docs.redb" --name "$1_$i"
    rm -rf "$WB/f$1-$i"
    i=$((i + 1))
  done
}
timed spread_10k spread 10000 "${PERF_SPREAD:-8}"
timed spread_100k spread 100000 "${PERF_SPREAD_100K:-2}"
rm -rf "$WB"
