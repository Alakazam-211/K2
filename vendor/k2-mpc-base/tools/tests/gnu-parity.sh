# The live GNU parity check (K-PAR-7): the golden corpus's `gnu: same` cases against the real
# GNU tools. Ships: scripts/gnu-parity.py crates/base/tests/golden. Needs python3, GNU
# coreutils, grep, sed, diffutils and patch; no Rust.
for t in cat head tail wc grep sed diff patch python3; do printf '%s: ' "$t"; command -v $t || echo MISSING; done
sed --version | head -1; grep --version | head -1; diff --version | head -1; patch --version | head -1
python3 --version
timed gnu-parity env LC_ALL=C.UTF-8 python3 -I scripts/gnu-parity.py
