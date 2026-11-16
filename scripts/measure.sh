#!/bin/sh
# The pack's measurements, each run by name in a release build on a build node; the gate
# (scripts/check.sh) leaves them out (#[ignore]d), and none asserts a time. CHANGELOG.md
# gives what each measures and the figures cited. One test at a time: the heap figures come
# from the test binary's counting allocator.
#   tests/live_scale.rs    scale_open_verbs_reads_view_write_back  a large scene's costs (SCALE_OBJECTS)
#   tests/live_storage.rs  carrier_storage_per_write_back          the carrier's storage per write-back
#   tests/meshread_scale.rs mesh_read_decode_and_mesh_phases      the mesh read's decode and mesh phase
#   tests/meshread_scale.rs mesh_read_heap_at_the_stored_cap      the mesh read's heap at the stored cap
#
#   scripts/measure.sh
set -eu
cd "$(dirname "$0")/.."
cargo test -q --release --locked -p k2-mpc-blend --test live_scale -- --ignored --exact \
  --nocapture --test-threads 1 scale_open_verbs_reads_view_write_back
cargo test -q --release --locked -p k2-mpc-blend --test live_storage -- --ignored --exact \
  --nocapture --test-threads 1 carrier_storage_per_write_back
cargo test -q --release --locked -p k2-mpc-blend --test meshread_scale -- --ignored --exact \
  --nocapture --test-threads 1 mesh_read_decode_and_mesh_phases
cargo test -q --release --locked -p k2-mpc-blend --test meshread_scale -- --ignored --exact \
  --nocapture --test-threads 1 mesh_read_heap_at_the_stored_cap
echo "measurements: done"
