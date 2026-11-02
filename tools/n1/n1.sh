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
#
# Both ways, for a previous release that reads format 6 too (v0.10.0 and later; v0.10.1):
#
#   tools/n1/n1.sh layout-both <previous tag> <dir>   no cargo: <dir>/prev/kit and
#                                                <dir>/head/kit are the two kits (`git archive`
#                                                of the tag and of HEAD), <dir>/fwd and <dir>/rev
#                                                the consumer (tools/n1/consumer-both) built
#                                                against the previous kit and against HEAD's
#   tools/n1/n1.sh node-both <dir>               on a build node: each kit's `n1_producer`, with
#                                                K2MB_N1_RESTORE (it also reverts its revert, so
#                                                the stores hold provenance rows and a record with
#                                                roots), writes text.redb and grid.redb for the
#                                                other build's consumer, which opens both, blames
#                                                the restored line, reverts the other build's
#                                                record, writes, restores and rewrites
#
# A previous release older than v0.10.1 has no K2MB_N1_RESTORE: `layout-both` adds the
# revert of the revert to its copy of the producer (python3), and stops if it can't. Ship the
# laid-out dir with the job (an untracked dir in the checkout: `--dirty`). The job prints
# `N-1 OK` once per direction, then `N-1 BOTH OK`.
set -eu
MODE=${1:?layout <tag> <dir> | node <dir> | layout-both <tag> <dir> | node-both <dir>}
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
  layout-both)
    TAG=${2:?previous release tag}; OUT=${3:?dir}
    ROOT=$(cd "$(dirname "$0")/../.." && pwd)
    rm -rf "$OUT"; mkdir -p "$OUT/prev/kit" "$OUT/head/kit"
    for side in prev head; do
      REV=$TAG; [ "$side" = head ] && REV=HEAD
      git -C "$ROOT" archive "$REV" Cargo.toml Cargo.lock .cargo crates/base vendor/yrs \
        | tar -x -C "$OUT/$side/kit"
      sed -i.bak 's|^members = .*|members = ["crates/base"]|' "$OUT/$side/kit/Cargo.toml"
      rm "$OUT/$side/kit/Cargo.toml.bak"
    done
    P="$OUT/prev/kit/crates/base/tests/grid_store.rs"
    if ! grep -q K2MB_N1_RESTORE "$P"; then
      python3 - "$P" <<'PY'
import sys
p = sys.argv[1]
s = open(p).read()
a = "        s.revert(c, 2, false).unwrap();\n        if grid {"
if s.count(a) != 1:
    sys.exit("n1.sh: the previous release's n1_producer has no revert to add the restore after")
b = ("        s.revert(c, 2, false).unwrap();\n"
     "        if std::env::var(\"K2MB_N1_RESTORE\").is_ok() {\n"
     "            let last = s.read(c).unwrap().version;\n"
     "            s.revert(c, last, false).unwrap();\n"
     "        }\n"
     "        if grid {")
open(p, "w").write(s.replace(a, b))
PY
      echo "added K2MB_N1_RESTORE to $TAG's n1_producer (layout copy only)"
    fi
    for pair in "fwd prev" "rev head"; do
      set -- $pair
      mkdir -p "$OUT/$1"
      cp -R "$ROOT/tools/n1/consumer-both/." "$OUT/$1/"
      sed -i.bak "s|\.\./kit/crates/base|../$2/kit/crates/base|" "$OUT/$1/Cargo.toml"
      rm "$OUT/$1/Cargo.toml.bak"
      cp "$OUT/$2/kit/Cargo.lock" "$OUT/$1/Cargo.lock"
    done
    echo "laid out: $OUT (prev = $TAG, head = HEAD)"
    ;;
  node-both)
    OUT=${2:?dir}
    OUT=$(cd "$OUT" && pwd)
    for pair in "fwd head prev" "rev prev head"; do
      set -- $pair
      echo "== $1: the $2 build writes, the $3 build reads"
      (cd "$OUT/$2/kit" && K2MB_N1_OUT="$OUT/$1" K2MB_N1_RESTORE=1 \
        cargo test -p k2-mpc-base --test grid_store n1_producer -- --ignored)
      ls -l "$OUT/$1/text.redb" "$OUT/$1/grid.redb"
      (cd "$OUT/$1" && cargo run -q)
    done
    echo "N-1 BOTH OK"
    ;;
  *) echo "n1.sh: unknown mode $MODE" >&2; exit 2 ;;
esac
