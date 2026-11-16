#!/bin/sh
# The pack's measurements, each run by name in a release build on a build node; the gate
# (scripts/check.sh) leaves them out (#[ignore]d), and none asserts a time. CHANGELOG.md
# gives what each measures. One test at a time: the heap figures come from the test binary's
# counting allocator.
#   tests/live_scale.rs  a_long_timeline                     the live pack at size (SCALE_ITEMS)
#   tests/live_scale.rs  one_clip_nudged_ten_thousand_times  the heap after many sets of one clip
#
#   scripts/measure.sh
set -eu
cd "$(dirname "$0")/.."
cargo test -q --release --locked -p k2-mpc-timeline --test live_scale -- --ignored --exact \
  --nocapture --test-threads 1 a_long_timeline one_clip_nudged_ten_thousand_times
echo "measurements: done"
