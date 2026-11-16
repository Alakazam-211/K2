#!/bin/sh
# Fail if any Rust dependency (transitive, all targets and features) of any tracked Cargo.lock
# has a licence outside the allow-list: `mpc-tools licences` (tools/README.md, "mpc-tools").
# Allowed: MIT, Apache-2.0, Apache-2.0 WITH LLVM-exception, BSD-2/3-Clause, ISC, Zlib, and
# Unicode-3.0 (unicode-ident, build-time); NCSA only in a fuzz lock (libFuzzer); Unlicense only
# as one side of an OR whose other side is allowed. "A OR B" passes if either side does; "A AND
# B" needs both. Every lock: the workspace's, the fuzz crate's, the tools'.
#
#   scripts/check-licences.sh                     every dependency of every tracked lock
#   scripts/check-licences.sh --expr NAME=EXPR…   each licence expression, with no cargo
#                                                 metadata (a vendored crate's own manifest,
#                                                 checked before it is vendored)
# The tools binary builds on Linux (a job), not on a shared Mac (tools/mpc-tools.sh).
set -eu
cd "$(dirname "$0")/.."
if [ "${1:-}" = --expr ]; then
  shift
  exec sh tools/mpc-tools.sh licences --expr "$@"
fi
exec sh tools/mpc-tools.sh licences --root .
