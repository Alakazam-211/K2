#!/usr/bin/env bash
# agent-isolation-b4-prerows-run.sh: build <commit> on K2 Compute, copy the
# artifacts to the isolation test box, run scripts/agent-isolation-b4-
# prerows.sh there, and print the recorded rows (design-agent-isolation-
# m2-b4-nftables rev 3.1, "Kernel rows first"). The rows RECORD what the
# box does; this fails only when the build or a row couldn't run.
#
# usage: scripts/agent-isolation-b4-prerows-run.sh <commit>
# env: K2_ISO_HOST (required, user@host: isolation tests only, set up once
#      with agent-isolation-m1b-kernel-tests.sh --setup), K2_ISO_SSH_OPTS,
#      K2_ISO_BUILD_NODE, K2_BIN (as for agent-isolation-kernel-gate.sh).
# The rows land in iso-prerows-<commit>.log in the current folder.
set -euo pipefail
sha="${1:?usage: $0 <commit>}"
host="${K2_ISO_HOST:?K2_ISO_HOST=user@host: the isolation test box}"
k2="${K2_BIN:-k2}"
repo="$(git rev-parse --show-toplevel)"
sha="$(git -C "$repo" rev-parse --verify "$sha^{commit}")"
read -r -a ssh_opts <<< "${K2_ISO_SSH_OPTS:-}"
ssh_opts+=(-o BatchMode=yes)
work="$(mktemp -d "${TMPDIR:-/tmp}/k2-iso-prerows.XXXXXX")"
trap 'rm -rf "$work"' EXIT

build='set -euo pipefail
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-8}"
T="${CARGO_TARGET_DIR:-target}"
mkdir -p iso-art
cargo build -p k2-daemon --bin k2-daemon
cargo build -p k2-ptyd --bins
bin="$(cargo test -p k2-daemon --test agent_isolation_kernel_integration --no-run --message-format json 2>/dev/null | python3 -c "import sys, json
for l in sys.stdin:
    try: v = json.loads(l)
    except Exception: continue
    if v.get(\"reason\") == \"compiler-artifact\" and v.get(\"executable\") and v[\"target\"][\"name\"] == \"agent_isolation_kernel_integration\": print(v[\"executable\"])")"
[ -x "$bin" ] || { echo "no test binary"; exit 1; }
cp "$bin" iso-art/agent_isolation_kernel_integration
cp "$T/debug/k2-daemon" "$T/debug/k2-ptyd" "$T/debug/k2-ptyd-test-agent" iso-art/
cp cli/k2 scripts/agent-isolation-b4-prerows.sh iso-art/
for f in agent_isolation_kernel_integration k2-daemon k2-ptyd k2-ptyd-test-agent; do strip --strip-debug "iso-art/$f"; done
echo "iso-art: built"'

# shellcheck source=lib/rust-pin.sh
. "$repo/scripts/lib/rust-pin.sh"
rust="$(K2_RUST_PIN_REPO="$repo" k2_rust_need "$sha")"
place=(--any --os linux --arch x86_64)
[ -n "${K2_ISO_BUILD_NODE:-}" ] && place=("$K2_ISO_BUILD_NODE")
echo "b4-prerows: building ${sha:0:12} on K2 Compute ($rust)"
out="$("$k2" compute run "${place[@]}" --sha "$sha" --tier standard \
    --needs "$rust,cmake@3" --mem 32G --stall-timeout off --artifact 'iso-art/*' --detach \
    -- bash -c "$build" 2>&1 </dev/null)" || { echo "$out" >&2; echo "b4-prerows: FAIL: the build job was not accepted" >&2; exit 1; }
job="$(printf '%s\n' "$out" | sed -n 's/.*k2 compute wait \([0-9a-f]*\).*/\1/p' | head -1)"
[ -n "$job" ] || { echo "$out" >&2; echo "b4-prerows: FAIL: no job id" >&2; exit 1; }
"$k2" compute wait "$job" --max-wait 90m </dev/null || { echo "b4-prerows: FAIL: build job $job failed (k2 compute logs $job)" >&2; exit 1; }
"$k2" compute logs "$job" </dev/null | grep -q '^iso-art: built$' \
    || { echo "b4-prerows: FAIL: build job $job did not finish (k2 compute logs $job)" >&2; exit 1; }
"$k2" compute fetch "$job" --out "$work" --over-relay </dev/null >/dev/null
art="$work/iso-art"
# This checkout's rows script (a fix to it needs no rebuild of the rest).
cp "$repo/scripts/agent-isolation-b4-prerows.sh" "$art/"

remote="iso-prerows-${sha:0:12}"
ssh "${ssh_opts[@]}" "$host" "rm -rf ~/$remote && mkdir -p ~/$remote"
scp -q "${ssh_opts[@]}" "$art"/* "$host:$remote/"
ssh "${ssh_opts[@]}" "$host" "chmod +x ~/$remote/*"
log="iso-prerows-${sha:0:12}.log"
set +e
ssh "${ssh_opts[@]}" "$host" "cd ~/$remote && bash agent-isolation-b4-prerows.sh ~/$remote" 2>&1 | tee "$log"
rc="${PIPESTATUS[0]}"
set -e
ssh "${ssh_opts[@]}" "$host" "rm -rf ~/$remote" || true
[ "$rc" -eq 0 ] || { echo "b4-prerows: FAIL: the rows script exited $rc (build job $job; log $log)" >&2; exit 1; }
echo "b4-prerows: rows recorded in $log (build job $job)"
