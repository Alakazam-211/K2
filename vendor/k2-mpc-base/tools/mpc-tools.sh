#!/bin/sh
# Run the kit's tools binary (tools/mpc-tools; tools/README.md, "mpc-tools"): the kit's scripts
# and the packs' call it as `<kit>/tools/mpc-tools.sh <subcommand> ...`.
#
# The binary: $MPC_TOOLS when set (a binary built elsewhere), else one built here with `cargo
# build --locked` (a no-op when it is up to date) in a target folder of its own:
# $CARGO_TARGET_DIR/mpc-tools (a job's warm target dir keeps it between jobs), else
# tools/mpc-tools/target. It is built on Linux only, unless MPC_TOOLS_BUILD=1: a shared Mac
# never builds, and says what to set instead (exit 4). MPC_TOOLS_FEATURES: cargo features (a
# tool a public cut leaves out is behind one). MPC_TOOLS_SRC: the crate to build, when
# this copy of the script sits outside the kit (a self-test's stand-in kit).
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
if [ -n "${MPC_TOOLS:-}" ]; then
  [ -x "$MPC_TOOLS" ] || { echo "mpc-tools: MPC_TOOLS=$MPC_TOOLS is not an executable" >&2; exit 4; }
  exec "$MPC_TOOLS" "$@"
fi
SRC=${MPC_TOOLS_SRC:-$HERE/mpc-tools}
T=${CARGO_TARGET_DIR:+$CARGO_TARGET_DIR/mpc-tools}
T=${T:-$SRC/target}
BIN="$T/debug/mpc-tools"
if [ "$(uname -s)" = Linux ] || [ "${MPC_TOOLS_BUILD:-0}" = 1 ]; then
  cargo build --quiet --locked --manifest-path "$SRC/Cargo.toml" --target-dir "$T" \
    ${MPC_TOOLS_FEATURES:+--features "$MPC_TOOLS_FEATURES"} >&2
elif [ ! -x "$BIN" ]; then
  echo "mpc-tools: no binary at $BIN, and this machine doesn't build it (not Linux): set MPC_TOOLS to a built one, or MPC_TOOLS_BUILD=1 to build here" >&2
  exit 4
fi
exec "$BIN" "$@"
