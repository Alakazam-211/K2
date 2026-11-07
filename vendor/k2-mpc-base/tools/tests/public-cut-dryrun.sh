# A public cut's dry run at a pre-cut commit, as a commands file for tools/job.sh (each step's
# failure is the job's exit: no ad hoc `sh -c` wrapper whose last `echo` exits 0):
#   k2 compute run --repo <repo> --sha <pre-cut> --cpus 2 --mem 4G \
#     -- sh tools/job.sh tools/tests/public-cut-dryrun.sh
# tools/public-cut.sh --no-commit, then tools/check-public-cut.sh on the tree it staged, in a
# shared clone of the checkout's repository (never the checkout: the cut rewrites the tree) with
# a throwaway HOME for its git identity. The release's real gate is tools/tests/public-cut-gate.sh,
# at the cut commit.
H=$(git rev-parse HEAD)
G=$(git rev-parse --path-format=absolute --git-common-dir)
D=$(mktemp -d)
timed clone sh -c "export HOME='$D'; git config --global --add safe.directory '*' && git config --global user.name cut \
  && git config --global user.email cut@example.invalid && git clone -q --shared --no-checkout '$G' '$D/c' \
  && git -C '$D/c' checkout -q --detach $H"
timed public_cut_no_commit sh -c "export HOME='$D'; cd '$D/c' && sh tools/public-cut.sh --no-commit"
timed check_public_cut sh -c "export HOME='$D'; cd '$D/c' && sh tools/check-public-cut.sh"
rm -rf "$D"
if [ "$RC" -eq 0 ]; then echo "=== RESULT dry run: GREEN at $H"; else echo "=== RESULT dry run: RED (rc $RC) at $H"; fi
