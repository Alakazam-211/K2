#!/bin/sh
# The N−1 check of store format 6: a store this build used for text
# docs only must open, read, write, revert and rewrite on the previous release, and a store
# holding a grid doc must be refused there with one line. It prints `N-1 OK`.
#
# Two halves, so the build half runs on a build node and the other needs no cargo:
#
#   tools/n1/n1.sh layout <previous tag> <dir>   no cargo: <dir>/old is the previous release's
#                                                kit (`git archive <tag>`, the whole tree: its
#                                                own crates/base and every vendor/ dir its
#                                                manifest names), <dir>/n1 the consumer
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
#                                                record, writes, restores and rewrites; with
#                                                K2MB_N1_AGENTS (after v0.10.2) the producer also
#                                                has an own replica and an agent join, so the
#                                                store holds `client_kinds`: a build that doesn't
#                                                know it reports its rewrite refused (NewerData);
#                                                the consumer then deletes the doc, and HEAD's
#                                                build (`n1_kinds_after_delete`) reads the
#                                                file as the delete left it, opens it, and fails
#                                                if a deleted doc's client_kinds rows remain;
#                                                with K2MB_N1_PERSONS (v0.14.0) it also leaves
#                                                person-device writers (persons-<store>): the
#                                                consumer never hands one out, and this build's
#                                                `n1_persons_after` takes keep.md's back; with
#                                                K2MB_N1_TURNS (after v0.14.0) a.md's change and
#                                                a third doc's (turns.md) carry host-given turns
#                                                (`change_turns`): a build that doesn't know the
#                                                table reports its rewrite refused (NewerData),
#                                                and this build's `n1_turns_after` finds a.md's
#                                                rows gone after its open and reads turns.md's
#                                                turn back; with K2MB_N1_VERBS (after v0.14.0) a
#                                                fourth doc (verbs.md) has a pack verb's change
#                                                (`change_verbs`), refused the same way, and this
#                                                build's `n1_verbs_after` reads its verb back
#   tools/n1/n1.sh node-wire <dir>               the live wire's N-1 (wire 3, after v0.14.0), from
#                                                the same dir, node only: the previous client
#                                                reads this kit's goldens, this client the
#                                                previous goldens (client/test/n1-wire.test.ts)
#
# The version an app opened (after v0.20.0): its pin is slot 3 of a doc's sync rows, which a
# kit that doesn't know it carries:
#
#   tools/n1/n1.sh layout-opened <previous tag> <dir>   no cargo: <dir>/old is the previous
#                                                release's kit, <dir>/opened the consumer
#                                                (tools/n1/consumer-opened)
#   tools/n1/n1.sh node-opened <dir>             on a build node, from the repo root: this
#                                                build's `n1_opened_producer` pins a blob doc and
#                                                two live docs; the previous release opens both
#                                                kinds, writes one, rewrites the store and
#                                                deletes a doc; this build's `n1_opened_after`
#                                                reads the two pins left and finds the deleted
#                                                doc's gone. The last line is `N-1 OPENED OK`.
#
# The blob references (after v0.10.2), both the case that holds and the documented limit:
#
#   tools/n1/n1.sh layout-refs <previous tag> <dir>   no cargo: <dir>/old is the previous
#                                                release's kit, <dir>/refs the consumer
#                                                (tools/n1/consumer-refs)
#   tools/n1/n1.sh node-refs <dir>               on a build node, from the repo root: this
#                                                build's `n1_refs_producer` writes a store whose
#                                                docs hold references; the previous release's
#                                                gc keeps those blobs (HELD), collects one named
#                                                only by a value it wrote (LIMIT), refuses a
#                                                rewrite and leaves rows on a delete; this
#                                                build's `n1_refs_after` then opens it: the rows
#                                                and the count go, and a load takes a hold on
#                                                what's still there and reports what isn't.
#                                                The last line is `N-1 REFS OK`.
#
# The carrier (after v0.14.0), both ways:
#
#   tools/n1/n1.sh layout-carrier <previous tag> <dir>   no cargo: <dir>/old is the previous
#                                                release's kit, <dir>/carrier the consumer
#                                                (tools/n1/consumer-carrier)
#   tools/n1/n1.sh node-carrier <dir>            on a build node, from the repo root: this
#                                                build's `n1_carrier_producer` writes three live
#                                                docs with a carrier each; the previous release
#                                                keeps them past its GC's grace (HELD), writes
#                                                one with its own encode, refuses a rewrite,
#                                                deletes one (row and count left) and writes a
#                                                store of its own; this build's
#                                                `n1_carrier_after` then drops the stale carrier
#                                                and takes the file, reads the untouched one from
#                                                the store, gives back the deleted doc's count,
#                                                and opens the older store with the file as the
#                                                carrier. The last line is `N-1 CARRIER OK`.
#
# The carrier's pairing (after v0.15.1), both ways:
#
#   tools/n1/n1.sh layout-pairing <previous tag> <dir>   no cargo: <dir>/old is the previous
#                                                release's kit, <dir>/pairing the consumer
#                                                (tools/n1/consumer-pairing)
#   tools/n1/n1.sh node-pairing <dir>            on a build node, from the repo root: this
#                                                build's `n1_pairing_producer` writes three live
#                                                docs of a pack that pairs, each with a carrier
#                                                and its pairing (`carrier_pairings`); the
#                                                previous release (it knows the carrier, not the
#                                                pairing) ignores the pairing rows: it writes one
#                                                doc with its own encode, refuses a rewrite naming
#                                                `carrier_pairings`, deletes one doc (leaving its
#                                                pairing rows) and writes a store of its own;
#                                                this build's `n1_pairing_after` opens the
#                                                untouched doc on its stored pairing, makes the
#                                                written doc's pairing again, finds the deleted
#                                                doc's rows swept after its rewrite, and opens the
#                                                older store with the pairing made from its
#                                                synced state; each with a verb spliced exactly.
#                                                The last line is `N-1 PAIRING OK`.
#
# The structure index's row format 2 (the view items), both ways:
#
#   tools/n1/n1.sh layout-index <previous tag> <dir>   no cargo: <dir>/old is the previous
#                                                release's kit, <dir>/index-consumer the consumer
#                                                (tools/n1/consumer-index)
#   tools/n1/n1.sh node-index <dir>              on a build node, from the repo root: this
#                                                build's `n1_index_producer` writes <dir>/index
#                                                (a doc whose only row is of format 2, under a
#                                                pack's bumped format, and two of format 1); the
#                                                previous release reads the format-1 rows,
#                                                finds the format-2 doc pending (it never reads
#                                                that row), indexes it under its own format, and
#                                                writes <dir>/prev; this build's
#                                                `n1_index_after` reads both stores, every
#                                                format-1 row with its view items absent, and
#                                                indexes them again. The last line is
#                                                `N-1 INDEX OK`.
#
# The text → tier 0 demotion (`demoted_changes`, after v0.16.0), both ways:
#
#   tools/n1/n1.sh layout-demote <previous tag> <dir>   no cargo: <dir>/old is the previous
#                                                release's kit, <dir>/demote the consumer
#                                                (tools/n1/consumer-demote)
#   tools/n1/n1.sh node-demote <dir>             on a build node, from the repo root: this
#                                                build's `n1_demote_producer` writes
#                                                <dir>/demote/fwd (two demoted docs, one kept
#                                                both ways); the previous release reads every
#                                                version exactly, is refused a text open
#                                                (PackMismatch), a purge and an erasure
#                                                (NewerData; v0.17.0 and later, which know the
#                                                table, purge and erase, the erasure renaming
#                                                the participant in the live log: K2MB_N1_PREV),
#                                                writes a version and deletes a doc; this
#                                                build's `n1_demote_after` finds the deleted
#                                                doc's live log gone, reads the other's (its
#                                                authors as that release left them) and purges. Then the previous release writes
#                                                <dir>/demote/rev (a text doc with a change the
#                                                file never got and an outside save after), and
#                                                this build's `n1_demote_prev_store` demotes it,
#                                                keeping both. The last line is `N-1 DEMOTE OK`.
#
# Store format 8, the required-features word, both ways:
#
#   tools/n1/n1.sh layout-format <previous tag> <dir>   no cargo: <dir>/old is the previous
#                                                release's kit, <dir>/format the consumer
#                                                (tools/n1/consumer-format)
#   tools/n1/n1.sh node-format <dir>             on a build node, from the repo root: this
#                                                build's `n1_format_producer` makes three stores
#                                                and stamps them format 8 by hand (the word naming
#                                                chunked, native, and both); the previous release
#                                                checks them by its read range: one that reads up
#                                                to format 7 refuses each at open with one line;
#                                                one that reads format 8 opens the chunked store
#                                                and reads its doc as written (versions and text)
#                                                and refuses native and both; a refused file's
#                                                bytes unchanged, the opened one's format and
#                                                versions kept; then it writes a
#                                                store of its own; this build's `n1_format_after`
#                                                opens that store as before (its format kept, a
#                                                write, a rewrite), opens the chunked one and
#                                                refuses the two that set native. The last line
#                                                is `N-1 FORMAT OK`.
#
# The rows depend on the previous release: the node modes read each kit's crate version from
# its manifest and hand it to the rows (K2MB_N1_READER, the build whose consumer ran on a store;
# K2MB_N1_PREV, the previous release). Against a release that doesn't know a table (v0.14.0 and
# the turn, verb and carrier tables), they assert what that build does with rows it can't read
# (a rewrite refused, rows and counts its delete leaves); against one that knows them (v0.15.0
# and later), plain round trips (its rewrite and delete take them as this build's do).
# Each table's first release: doc_blob_refs and client_kinds v0.11.0; change_turns, blob_turns,
# change_verbs and the carrier tables v0.15.0; carrier_pairings and demoted_changes v0.17.0.
# Format 6 is read from v0.10.0. Every consumer asserts one outcome for its version, never
# "either".
#
# The previous release's tree is `git archive <tag>` of this repo, or, with
# N1_PREV_ARCHIVE=<file> (a gzipped `git archive` of the tag, made where the tag is: a job's
# checkout has no tags off its history), that file unpacked; `HEAD` is always this checkout's.
#
# A previous release older than v0.10.1 has no K2MB_N1_RESTORE: `node-both` adds the
# revert of the revert to its copy of the producer (mpc-tools edit replace-once), and stops if
# it can't. Ship the
# laid-out dir with the job (an untracked dir in the checkout: `--dirty`). The job prints
# `N-1 OK` once per direction, then `N-1 BOTH OK`.
set -eu
# kit_version DIR: the crate version in DIR/crates/base/Cargo.toml (the kit's own manifest)
kit_version() { sed -n 's/^version = "\(.*\)"$/\1/p' "$1/crates/base/Cargo.toml" | head -1; }
# tree_of ROOT REV DEST: the tree of REV in DEST (`git archive`; for a REV other than HEAD,
# N1_PREV_ARCHIVE's file when it is set)
tree_of() {
  if [ "$2" != HEAD ] && [ -n "${N1_PREV_ARCHIVE:-}" ]; then
    tar -xzf "$N1_PREV_ARCHIVE" -C "$3"
  else
    git -C "$1" archive "$2" | tar -x -C "$3"
  fi
}
MODE=${1:?layout <tag> <dir> | node <dir> | layout-both <tag> <dir> | node-both <dir> | node-wire <dir> | layout-refs <tag> <dir> | node-refs <dir> | layout-carrier <tag> <dir> | node-carrier <dir> | layout-pairing <tag> <dir> | node-pairing <dir> | layout-index <tag> <dir> | node-index <dir> | layout-demote <tag> <dir> | node-demote <dir> | layout-format <tag> <dir> | node-format <dir> | layout-opened <tag> <dir> | node-opened <dir>}
case $MODE in
  layout)
    TAG=${2:?previous release tag}; OUT=${3:?dir}
    ROOT=$(cd "$(dirname "$0")/../.." && pwd)
    rm -rf "$OUT"; mkdir -p "$OUT/old" "$OUT/n1"
    # the whole tree: cargo resolves every path dependency, optional ones too (vendor/mpce-*
    # since the native engine's seam), so a list of dirs would miss the next one
    tree_of "$ROOT" "$TAG" "$OUT/old"
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
    # v0.10.0 and later read format 6, so the grid store opens there
    PREV=$(kit_version "$OUT/old")
    echo "== consumer (the previous release, $PREV)"
    cd "$OUT/n1"
    K2MB_N1_PREV="$PREV" cargo run -q
    ;;
  layout-both)
    TAG=${2:?previous release tag}; OUT=${3:?dir}
    ROOT=$(cd "$(dirname "$0")/../.." && pwd)
    rm -rf "$OUT"; mkdir -p "$OUT/prev/kit" "$OUT/head/kit"
    for side in prev head; do
      REV=$TAG; [ "$side" = head ] && REV=HEAD
      # the whole tree (as `layout`): every path dependency the kit's manifest names, optional
      # ones too, whatever the release
      tree_of "$ROOT" "$REV" "$OUT/$side/kit"
      sed -i.bak 's|^members = .*|members = ["crates/base"]|' "$OUT/$side/kit/Cargo.toml"
      rm "$OUT/$side/kit/Cargo.toml.bak"
    done
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
    # a previous release older than v0.10.1: its producer gets the revert of its revert here,
    # on the node (the layout copy only; `mpc-tools edit replace-once` refuses unless the
    # revert it follows is there exactly once)
    P="$OUT/prev/kit/crates/base/tests/grid_store.rs"
    if ! grep -q K2MB_N1_RESTORE "$P"; then
      KIT=$(cd "$(dirname "$0")/../.." && pwd)
      A=$(printf '%s\n' '        s.revert(c, 2, false).unwrap();' '        if grid {')
      B=$(printf '%s\n' '        s.revert(c, 2, false).unwrap();' \
        '        if std::env::var("K2MB_N1_RESTORE").is_ok() {' \
        '            let last = s.read(c).unwrap().version;' \
        '            s.revert(c, last, false).unwrap();' \
        '        }' \
        '        if grid {')
      sh "$KIT/tools/mpc-tools.sh" edit replace-once "$P" "$A" "$B" \
        || { echo "n1.sh: the previous release's n1_producer has no revert to add the restore after" >&2; exit 1; }
      echo "added K2MB_N1_RESTORE to the previous release's n1_producer (layout copy only)"
    fi
    for pair in "fwd head prev" "rev prev head"; do
      set -- $pair
      READER=$(kit_version "$OUT/$3/kit")
      WRITER=$(kit_version "$OUT/$2/kit")
      echo "== $1: the $2 build ($WRITER) writes, the $3 build ($READER) reads"
      (cd "$OUT/$2/kit" && K2MB_N1_OUT="$OUT/$1" K2MB_N1_RESTORE=1 K2MB_N1_AGENTS=1 \
        K2MB_N1_PERSONS=1 K2MB_N1_TURNS=1 K2MB_N1_VERBS=1 \
        cargo test -p k2-mpc-base --test grid_store n1_producer -- --ignored --nocapture)
      ls -l "$OUT/$1/text.redb" "$OUT/$1/grid.redb"
      (cd "$OUT/$1" && K2MB_N1_WRITER="$WRITER" K2MB_N1_READER="$READER" cargo run -q)
      # after v0.14.0: a.md's turns (deleted by the other build) swept, turns.md's read back.
      # First: it reads the file as the other build left it, before any open here sweeps it
      (cd "$OUT/head/kit" && K2MB_N1_OUT="$OUT/$1" K2MB_N1_READER="$READER" \
        cargo test -p k2-mpc-base --test grid_store n1_turns_after -- --ignored --nocapture)
      # after v0.14.0: verbs.md's pack verb read back after the other build
      (cd "$OUT/head/kit" && K2MB_N1_OUT="$OUT/$1" \
        cargo test -p k2-mpc-base --test grid_store n1_verbs_after -- --ignored --nocapture)
      (cd "$OUT/head/kit" && K2MB_N1_OUT="$OUT/$1" \
        cargo test -p k2-mpc-base --test grid_store n1_kinds_after_delete -- --ignored --nocapture)
      # v0.14.0: keep.md's person-device writers, taken back by this build after the other's
      (cd "$OUT/head/kit" && K2MB_N1_OUT="$OUT/$1" \
        cargo test -p k2-mpc-base --test grid_store n1_persons_after -- --ignored --nocapture)
    done
    echo "N-1 BOTH OK"
    ;;
  node-wire)
    # The live wire's N-1, both ways (client/WIRE.md, "Wire versions"; after v0.14.0, wire 3),
    # from `layout-both`'s dir (no cargo: node only): the previous release's client code and
    # goldens go to client/test/n1-prev/, where client/test/n1-wire.test.ts finds them, and the
    # previous client reads this kit's goldens while this client reads the previous ones.
    # It prints `N-1 WIRE OK`.
    OUT=${2:?dir}
    OUT=$(cd "$OUT" && pwd)
    ROOT=$(cd "$(dirname "$0")/../.." && pwd)
    rm -rf "$ROOT/client/test/n1-prev"
    mkdir -p "$ROOT/client/test/n1-prev"
    cp -R "$OUT/prev/kit/client/src" "$ROOT/client/test/n1-prev/src"
    cp -R "$OUT/prev/kit/client/test/golden" "$ROOT/client/test/n1-prev/golden"
    (cd "$ROOT/client" && npm ci --no-audit --no-fund && npx vitest run test/n1-wire.test.ts)
    echo "N-1 WIRE OK"
    ;;
  layout-refs)
    TAG=${2:?previous release tag}; OUT=${3:?dir}
    ROOT=$(cd "$(dirname "$0")/../.." && pwd)
    rm -rf "$OUT"; mkdir -p "$OUT/old" "$OUT/refs"
    tree_of "$ROOT" "$TAG" "$OUT/old"
    sed -i.bak 's|^members = .*|members = ["crates/base"]|' "$OUT/old/Cargo.toml"
    rm "$OUT/old/Cargo.toml.bak"
    cp -R "$ROOT/tools/n1/consumer-refs/." "$OUT/refs/"
    cp "$OUT/old/Cargo.lock" "$OUT/refs/Cargo.lock"
    echo "laid out: $OUT (old = $TAG)"
    ;;
  node-refs)
    OUT=${2:?dir}
    OUT=$(cd "$OUT" && pwd)
    echo "== this build writes the references"
    K2MB_N1_OUT="$OUT/refs" cargo test -p k2-mpc-base --test blob_refs n1_refs_producer -- --ignored --nocapture
    # v0.11.0 and later know doc_blob_refs, so their rewrite and delete take it
    PREV=$(kit_version "$OUT/old")
    echo "== the previous release ($PREV)"
    (cd "$OUT/refs" && K2MB_N1_PREV="$PREV" cargo run -q)
    echo "== this build again"
    K2MB_N1_OUT="$OUT/refs" cargo test -p k2-mpc-base --test blob_refs n1_refs_after -- --ignored --nocapture
    echo "N-1 REFS OK"
    ;;
  layout-carrier)
    TAG=${2:?previous release tag}; OUT=${3:?dir}
    ROOT=$(cd "$(dirname "$0")/../.." && pwd)
    rm -rf "$OUT"; mkdir -p "$OUT/old" "$OUT/carrier"
    tree_of "$ROOT" "$TAG" "$OUT/old"
    sed -i.bak 's|^members = .*|members = ["crates/base"]|' "$OUT/old/Cargo.toml"
    rm "$OUT/old/Cargo.toml.bak"
    cp -R "$ROOT/tools/n1/consumer-carrier/." "$OUT/carrier/"
    cp "$OUT/old/Cargo.lock" "$OUT/carrier/Cargo.lock"
    echo "laid out: $OUT (old = $TAG)"
    ;;
  node-carrier)
    OUT=${2:?dir}
    OUT=$(cd "$OUT" && pwd)
    PREV=$(kit_version "$OUT/old")
    echo "== this build writes the carriers"
    K2MB_N1_OUT="$OUT/carrier" cargo test -p k2-mpc-base --test live_carrier n1_carrier_producer -- --ignored --nocapture
    echo "== the previous release ($PREV)"
    (cd "$OUT/carrier" && K2MB_N1_PREV="$PREV" cargo run -q)
    echo "== this build again"
    K2MB_N1_OUT="$OUT/carrier" K2MB_N1_PREV="$PREV" cargo test -p k2-mpc-base --test live_carrier n1_carrier_after -- --ignored --nocapture
    echo "N-1 CARRIER OK"
    ;;
  layout-pairing)
    TAG=${2:?previous release tag}; OUT=${3:?dir}
    ROOT=$(cd "$(dirname "$0")/../.." && pwd)
    rm -rf "$OUT"; mkdir -p "$OUT/old" "$OUT/pairing"
    tree_of "$ROOT" "$TAG" "$OUT/old"
    sed -i.bak 's|^members = .*|members = ["crates/base"]|' "$OUT/old/Cargo.toml"
    rm "$OUT/old/Cargo.toml.bak"
    cp -R "$ROOT/tools/n1/consumer-pairing/." "$OUT/pairing/"
    cp "$OUT/old/Cargo.lock" "$OUT/pairing/Cargo.lock"
    echo "laid out: $OUT (old = $TAG)"
    ;;
  node-pairing)
    OUT=${2:?dir}
    OUT=$(cd "$OUT" && pwd)
    echo "== this build writes the pairings"
    K2MB_N1_OUT="$OUT/pairing" cargo test -p k2-mpc-base --test live_paired n1_pairing_producer -- --ignored --nocapture
    # v0.17.0 and later know carrier_pairings, so their rewrite takes the store
    PREV=$(kit_version "$OUT/old")
    echo "== the previous release ($PREV)"
    (cd "$OUT/pairing" && K2MB_N1_PREV="$PREV" cargo run -q)
    echo "== this build again"
    K2MB_N1_OUT="$OUT/pairing" cargo test -p k2-mpc-base --test live_paired n1_pairing_after -- --ignored --nocapture
    echo "N-1 PAIRING OK"
    ;;
  layout-index)
    TAG=${2:?previous release tag}; OUT=${3:?dir}
    ROOT=$(cd "$(dirname "$0")/../.." && pwd)
    rm -rf "$OUT"; mkdir -p "$OUT/old" "$OUT/index-consumer"
    tree_of "$ROOT" "$TAG" "$OUT/old"
    sed -i.bak 's|^members = .*|members = ["crates/base"]|' "$OUT/old/Cargo.toml"
    rm "$OUT/old/Cargo.toml.bak"
    cp -R "$ROOT/tools/n1/consumer-index/." "$OUT/index-consumer/"
    cp "$OUT/old/Cargo.lock" "$OUT/index-consumer/Cargo.lock"
    echo "laid out: $OUT (old = $TAG)"
    ;;
  node-index)
    OUT=${2:?dir}
    OUT=$(cd "$OUT" && pwd)
    echo "== this build writes the index rows"
    K2MB_N1_OUT="$OUT" cargo test -p k2-mpc-base --test blob_index_v2 n1_index_producer -- --ignored --nocapture
    echo "== the previous release ($(kit_version "$OUT/old"))"
    (cd "$OUT/index-consumer" && K2MB_N1_OUT="$OUT" cargo run -q)
    echo "== this build again"
    K2MB_N1_OUT="$OUT" cargo test -p k2-mpc-base --test blob_index_v2 n1_index_after -- --ignored --nocapture
    echo "N-1 INDEX OK"
    ;;
  layout-demote)
    TAG=${2:?previous release tag}; OUT=${3:?dir}
    ROOT=$(cd "$(dirname "$0")/../.." && pwd)
    rm -rf "$OUT"; mkdir -p "$OUT/old" "$OUT/demote"
    tree_of "$ROOT" "$TAG" "$OUT/old"
    sed -i.bak 's|^members = .*|members = ["crates/base"]|' "$OUT/old/Cargo.toml"
    rm "$OUT/old/Cargo.toml.bak"
    cp -R "$ROOT/tools/n1/consumer-demote/." "$OUT/demote/"
    cp "$OUT/old/Cargo.lock" "$OUT/demote/Cargo.lock"
    echo "laid out: $OUT (old = $TAG)"
    ;;
  node-demote)
    OUT=${2:?dir}
    OUT=$(cd "$OUT" && pwd)
    echo "== forward: this build demotes"
    K2MB_N1_OUT="$OUT/demote" cargo test -p k2-mpc-base --test demote n1_demote_producer -- --ignored --nocapture
    # the previous release's version: v0.17.0 and later know the demotion's table, so their purge
    # and erasure go through (the consumer and n1_demote_after read it)
    PREV=$(kit_version "$OUT/old")
    echo "== forward: the previous release ($PREV) reads"
    (cd "$OUT/demote" && K2MB_N1_PREV="$PREV" cargo run -q -- read "$OUT/demote")
    echo "== forward: this build again"
    K2MB_N1_OUT="$OUT/demote" K2MB_N1_PREV="$PREV" cargo test -p k2-mpc-base --test demote n1_demote_after -- --ignored --nocapture
    echo "== reverse: the previous release writes a text doc"
    (cd "$OUT/demote" && cargo run -q -- produce "$OUT/demote")
    echo "== reverse: this build demotes it"
    K2MB_N1_OUT="$OUT/demote" cargo test -p k2-mpc-base --test demote n1_demote_prev_store -- --ignored --nocapture
    echo "N-1 DEMOTE OK"
    ;;
  layout-format)
    TAG=${2:?previous release tag}; OUT=${3:?dir}
    ROOT=$(cd "$(dirname "$0")/../.." && pwd)
    rm -rf "$OUT"; mkdir -p "$OUT/old" "$OUT/format"
    tree_of "$ROOT" "$TAG" "$OUT/old"
    sed -i.bak 's|^members = .*|members = ["crates/base"]|' "$OUT/old/Cargo.toml"
    rm "$OUT/old/Cargo.toml.bak"
    cp -R "$ROOT/tools/n1/consumer-format/." "$OUT/format/"
    cp "$OUT/old/Cargo.lock" "$OUT/format/Cargo.lock"
    echo "laid out: $OUT (old = $TAG)"
    ;;
  node-format)
    OUT=${2:?dir}
    OUT=$(cd "$OUT" && pwd)
    echo "== this build stamps three stores format 8"
    K2MB_N1_OUT="$OUT/format" cargo test -p k2-mpc-base --test engine_stamp n1_format_producer -- --ignored --nocapture
    PREV=$(kit_version "$OUT/old")
    echo "== the previous release ($PREV) reads or refuses each, by its read range"
    (cd "$OUT/format" && cargo run -q -- read "$OUT/format")
    echo "== the previous release ($PREV) writes a store of its own"
    (cd "$OUT/format" && cargo run -q -- produce "$OUT/format")
    echo "== this build opens it as before"
    K2MB_N1_OUT="$OUT/format" cargo test -p k2-mpc-base --test engine_stamp n1_format_after -- --ignored --nocapture
    echo "N-1 FORMAT OK"
    ;;
  layout-opened)
    TAG=${2:?previous release tag}; OUT=${3:?dir}
    ROOT=$(cd "$(dirname "$0")/../.." && pwd)
    rm -rf "$OUT"; mkdir -p "$OUT/old" "$OUT/opened"
    tree_of "$ROOT" "$TAG" "$OUT/old"
    sed -i.bak 's|^members = .*|members = ["crates/base"]|' "$OUT/old/Cargo.toml"
    rm "$OUT/old/Cargo.toml.bak"
    cp -R "$ROOT/tools/n1/consumer-opened/." "$OUT/opened/"
    cp "$OUT/old/Cargo.lock" "$OUT/opened/Cargo.lock"
    echo "laid out: $OUT (old = $TAG)"
    ;;
  node-opened)
    OUT=${2:?dir}
    OUT=$(cd "$OUT" && pwd)
    echo "== this build writes the pins"
    K2MB_N1_OUT="$OUT/opened" cargo test -p k2-mpc-base --test opened n1_opened_producer -- --ignored --nocapture
    echo "== the previous release ($(kit_version "$OUT/old"))"
    (cd "$OUT/opened" && cargo run -q)
    echo "== this build again"
    K2MB_N1_OUT="$OUT/opened" cargo test -p k2-mpc-base --test opened n1_opened_after -- --ignored --nocapture
    echo "N-1 OPENED OK"
    ;;
  *) echo "n1.sh: unknown mode $MODE" >&2; exit 2 ;;
esac
