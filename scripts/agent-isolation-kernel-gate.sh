#!/usr/bin/env bash
# agent-isolation-kernel-gate.sh: the named gate for Agent Isolation's
# real-kernel tests (review F13: they are #[ignore], so nothing runs them
# unless a gate does).
#
# Builds the test binary and the daemon at <commit> on K2 Compute (a Linux
# x86 node K2 picks), copies them to the isolation test box, runs
# scripts/agent-isolation-kernel-tests.sh there inside a delegated unit, and
# passes only when every test ran and passed. Its last line is
# "isolation-kernel: ok." on a pass.
#
# usage: scripts/agent-isolation-kernel-gate.sh <commit>
# env:
#   K2_ISO_HOST      required: the box, user@host (Linux, cgroup v2,
#                    systemd 254+, Landlock, passwordless sudo for
#                    systemd-run). Isolation tests only; no customer data.
#   K2_ISO_SSH_OPTS  extra ssh/scp options (an identity file, a pinned
#                    known_hosts), split on spaces
#   K2_ISO_TESTS     how many tests must pass (default: the count of
#                    #[test] in the test file at <commit>)
#   K2_BIN           the k2 CLI (default k2)
set -euo pipefail
sha="${1:?usage: $0 <commit>}"
host="${K2_ISO_HOST:?K2_ISO_HOST=user@host: the isolation test box}"
k2="${K2_BIN:-k2}"
repo="$(git rev-parse --show-toplevel)"
sha="$(git -C "$repo" rev-parse --verify "$sha^{commit}")"
read -r -a ssh_opts <<< "${K2_ISO_SSH_OPTS:-}"
ssh_opts+=(-o BatchMode=yes)
want="${K2_ISO_TESTS:-$(git -C "$repo" show "$sha:crates/k2-daemon/tests/agent_isolation_kernel_integration.rs" | grep -c '^#\[test\]')}"
[ "$want" -gt 0 ] || { echo "isolation-kernel: FAIL: no tests in the file at ${sha:0:12}" >&2; exit 1; }

work="$(mktemp -d "${TMPDIR:-/tmp}/k2-iso-gate.XXXXXX")"
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
cp cli/k2 scripts/agent-isolation-kernel-tests.sh iso-art/
for f in agent_isolation_kernel_integration k2-daemon k2-ptyd k2-ptyd-test-agent; do strip --strip-debug "iso-art/$f"; done
echo "iso-art: built"'

echo "isolation-kernel: building ${sha:0:12} on K2 Compute"
out="$("$k2" compute run --sha "$sha" --any --os linux --arch x86_64 --tier standard \
    --needs rust@stable,cmake@3 --mem 32G --stall-timeout off --artifact 'iso-art/*' --detach \
    -- bash -c "$build" 2>&1 </dev/null)" || { echo "$out" >&2; echo "isolation-kernel: FAIL: the build job was not accepted" >&2; exit 1; }
job="$(printf '%s\n' "$out" | sed -n 's/.*k2 compute wait \([0-9a-f]*\).*/\1/p' | head -1)"
[ -n "$job" ] || { echo "$out" >&2; echo "isolation-kernel: FAIL: no job id" >&2; exit 1; }
"$k2" compute wait "$job" --max-wait 90m </dev/null || { echo "isolation-kernel: FAIL: build job $job failed (k2 compute logs $job)" >&2; exit 1; }
"$k2" compute logs "$job" </dev/null | grep -q '^iso-art: built$' \
    || { echo "isolation-kernel: FAIL: build job $job did not finish (k2 compute logs $job)" >&2; exit 1; }
"$k2" compute fetch "$job" --out "$work" --over-relay </dev/null >/dev/null
art="$work/iso-art"

remote="iso-gate-${sha:0:12}"
ssh "${ssh_opts[@]}" "$host" "rm -rf ~/$remote && mkdir -p ~/$remote"
scp -q "${ssh_opts[@]}" "$art"/* "$host:$remote/"
ssh "${ssh_opts[@]}" "$host" "chmod +x ~/$remote/*"
set +e
ssh "${ssh_opts[@]}" "$host" "cd ~/$remote && bash agent-isolation-kernel-tests.sh ~/$remote" 2>&1 | tee "$work/run.log"
rc="${PIPESTATUS[0]}"
set -e
ssh "${ssh_opts[@]}" "$host" "rm -rf ~/$remote" || true
if [ "$rc" -ne 0 ]; then
    echo "isolation-kernel: FAIL: the kernel tests exited $rc (build job $job)" >&2
    exit 1
fi
if ! grep -Eq "^test result: ok\. $want passed; 0 failed; 0 ignored" "$work/run.log"; then
    echo "isolation-kernel: FAIL: expected $want tests to pass (build job $job): $(grep '^test result' "$work/run.log")" >&2
    exit 1
fi
echo "isolation-kernel: ok."
