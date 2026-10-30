# ITF regression traces (`*.itf` here, ITF JSON; not `.itf.json`, which the fallback bundle
# leaves out): Quint simulator traces of undo.qnt, shrunk, that
# once disagreed with spec_undo::model; each must replay with 0 mismatches. smoke.sh and
# counts.sh (PART a) source this file; alone, as an enum job at the repo root:
#   sh tools/undo/job.sh enum tools/undo/itf-regress/replay.sh
# All of them are at the bound 2 pids x 1 event + 1 undo + 1 revert, maxlen 4, seed abb.
#   kindA_purge_split  abb; p2 purges 'b': one canonical hunk "bb", one region (D2, U8)
#   kindB_purge_taint  abb; p1 inserts 'b' at 3; p2 purges 'b': the purge's one hunk
#                      deletes across p1's region's edge, which is tainted (U9)
# The first pair (exported from the model before its purge revision, per-copy purge hunks)
# caught the gap after the oracle's purge became canonical (04487c5). These are the same
# action sequences exported from the revised model (2026-10-29, seed 0x2a): replayed, they
# pin the oracle's side (a purge whose copies touch is one hunk, and it taints what it
# deletes across); the simulator replays (smoke.sh, counts.sh) cover the model's side.
export RUNREC_RUN=itf_regress
timed "ITF regression traces" R --show 10 --pids 2 --events 1 --undos 1 --reverts 1 --maxlen 4 --init abb "$ROOT"/tools/undo/itf-regress/*.itf
unset RUNREC_RUN
