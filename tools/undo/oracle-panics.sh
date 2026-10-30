# Does the oracle ever panic WITHOUT a mutant (v0.9.3, after `window.rs`'s shared-edge
# assertion fired under mutant iv in a G1 walk)? 1M random walks at the literal G1 bound, then
# 1M with one restart, the oracle of record; then mutant iv on walks, which must now end
# promptly (a panic counts as `panic@<depth>`, exit 1). Read each SUMMARY's violations=[...]
# and panic="...". `sh tools/undo/job.sh enum tools/undo/oracle-panics.sh` in a --repo job; about 20-40 min.
T=$(nproc)
timed "walks at G1, no mutant" E walk --g1 --probes ${WALKS:-1000000} --threads "$T" --progress 120 --run walk_g1_none
timed "walks at G1 + 1 restart, no mutant" E walk --g1 --restarts 1 --probes ${WALKS:-1000000} --threads "$T" --progress 120 --run walk_g1_rs1_none
timed "walks at G1, mutant iv (ends promptly)" E walk --g1 --probes ${IV_WALKS:-200000} --threads "$T" --mutant iv-redo-retries-skipped --progress 60 --run walk_g1_iv
