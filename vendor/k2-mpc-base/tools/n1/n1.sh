#!/bin/sh
# The N−1 check of store format 6 (K-FMT-10 item 22; C1c): a store this build used for text
# docs only must open, read, write, revert and rewrite on the previous release, and a store
# holding a grid doc must be refused there with one line. It prints `N-1 OK`.
#
# Two halves, so the build half runs on a build node and the other needs no cargo:
#
#   tools/n1/n1.sh layout <previous tag> <dir>   no cargo: <dir>/old is the previous release's
#                                                kit (`git archive <tag>`, its own crates/base and
#                                                vendor/yrs), <dir>/n1 the consumer
#                                                (tools/n1/consumer with the old Cargo.lock)
#   tools/n1/n1.sh node <dir>                    on a build node, from the repo root: writes
#                                                text.redb and grid.redb with this build (the
#                                                ignored IT `n1_producer`) into <dir>/n1, then
#                                                runs the consumer against the old kit
#
# With a compute runner that ships the checkout, lay out inside the checkout (an untracked dir)
# and send it with the job, e.g. from the workspace folder:
#   tools/n1/n1.sh layout v0.9.7.1 n1-run
#   k2 compute run <node> --repo <repo> --dirty --needs rust@stable --cpus 4 --mem 8G \
#     -- sh tools/n1/n1.sh node n1-run
# The job's exit is non-zero if any step fails, and the last line is `N-1 OK` when it passes.
set -eu
MODE=${1:?layout <tag> <dir> | node <dir>}
case $MODE in
  layout)
    TAG=${2:?previous release tag}; OUT=${3:?dir}
    ROOT=$(cd "$(dirname "$0")/../.." && pwd)
    rm -rf "$OUT"; mkdir -p "$OUT/old" "$OUT/n1"
    git -C "$ROOT" archive "$TAG" Cargo.toml Cargo.lock crates/base/Cargo.toml crates/base/src \
      vendor/yrs | tar -x -C "$OUT/old"
    sed -i.bak 's|^members = .*|members = ["crates/base"]|' "$OUT/old/Cargo.toml"
    rm "$OUT/old/Cargo.toml.bak"
    cp -R "$ROOT/tools/n1/consumer/." "$OUT/n1/"
    cp "$OUT/old/Cargo.lock" "$OUT/n1/Cargo.lock"
    echo "laid out: $OUT (old = $TAG)"
    ;;
  node)
    OUT=${2:?dir}
    OUT=$(cd "$OUT" && pwd)
    echo "== producer (this build)"
    K2MB_N1_OUT="$OUT/n1" cargo test -p k2-mpc-base --test grid_store n1_producer -- --ignored
    ls -l "$OUT/n1/text.redb" "$OUT/n1/grid.redb"
    echo "== consumer (the previous release)"
    cd "$OUT/n1"
    cargo run -q
    ;;
  *) echo "n1.sh: unknown mode $MODE" >&2; exit 2 ;;
esac
