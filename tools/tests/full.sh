# The workspace's tests, in parts that each fit one job's bundle (tools/README.md, "Test
# jobs": the kit's sources alone are over half of what a job can carry). PART picks one;
# tools/tests/full-paths.sh prints what each part ships. Debug builds, as CI runs them. fmt,
# clippy and the licence check run locally (some nodes have no rustfmt or clippy).
#   base-a       the kit: unit tests, doctests, every integration test but tier 0's and base-c's
#   evidence     the evidence tools' self-test and check (tools/evidence/; Python, no cargo)
#   base-c       the kit: soak (CI variant), edit_paths, edit_cost, history, presence, sessions
#   base-b       the kit: tier 0's integration tests (tier0, tier0_large, promote), the parity harnesses (parity, parity_blob) and erasure
#   conformance  the conformance suite (with the vendored reference model)
#   spec-undo    the undo spec suite, its mutant tests included
case ${PART:?PART: base-a, base-b, base-c, conformance, spec-undo or evidence} in
  base-a) timed build cargo test -p k2-mpc-base --no-run
          timed base_a cargo test -p k2-mpc-base ;;
  evidence) timed evidence_selftest sh tools/evidence/test.sh
          timed evidence_check python3 tools/evidence/check.py ;;
  base-b) timed build cargo test -p k2-mpc-base --test tier0 --test tier0_large --test promote --test parity --test parity_blob --test erasure --no-run
          timed base_b cargo test -p k2-mpc-base --test tier0 --test tier0_large --test promote --test parity --test parity_blob --test erasure ;;
  base-c) timed build cargo test -p k2-mpc-base --test soak --test edit_paths --test edit_cost --test history --test presence --test sessions --no-run
          timed base_c cargo test -p k2-mpc-base --test soak --test edit_paths --test edit_cost --test history --test presence --test sessions ;;
  conformance) timed build cargo test -p k2-mpc-conformance --no-run
          timed conformance cargo test -p k2-mpc-conformance ;;
  spec-undo) timed build cargo test -p k2-mpc-spec-undo --no-run
          timed spec_undo cargo test -p k2-mpc-spec-undo
          timed mutants cargo test -p k2-mpc-spec-undo --features mutants --test mutants ;;
  *) echo "unknown PART $PART" >&2; exit 2 ;;
esac
