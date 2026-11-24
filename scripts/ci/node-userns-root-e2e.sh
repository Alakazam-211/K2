#!/usr/bin/env bash
# node-userns-root-e2e.sh — the job syscall wall's root tests (0.46.18:
# user namespaces, the per-job namespace limit, the denylist for owner,
# own-trust and team jobs, --userns and --no-addr-filter) on a Linux box,
# leaving the box as it found it.
#
#   scripts/ci/node-userns-root-e2e.sh <ssh-host> [--sha REV]
#
# 1. Builds k2-node and the three test binaries (job_wall_linux,
#    tenant_wall_linux, job_userns_linux) for x86_64 Linux on K2 Compute
#    (never on this Mac): one job at REV (default HEAD) on a node of
#    NODE_E2E_DISTRO (default ubuntu@24.04, so glibc matches the box), kept
#    as artifacts and fetched.
# 2. On <ssh-host>, as root (the ssh user needs passwordless sudo):
#    records what the tests may change (users, groups, the machine's
#    max_user_namespaces, job units, network namespaces, /var/lib and /run
#    test leftovers, installed packages), installs what the tests need
#    (acl, podman, crun, a Chrome headless shell and its libraries), runs
#    the tests one at a time (incl. rootless Podman and Chromium's
#    namespace sandbox in a --userns job), then removes everything that
#    wasn't there before (packages purged) and checks the recorded state
#    again. The machine's max_user_namespaces must be the
#    same after the tests, or the run fails.
#
# Exit status: the tests' (a restore that can't put the box back fails too).
# Inside the compute job this same script runs with --build-job.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT_REL="node-userns-root-e2e"
TESTS=(job_wall_linux tenant_wall_linux job_userns_linux)

build_job() {
    cd "$ROOT"
    export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
    cargo build --locked -p k2-node --bin k2-node
    rm -rf "$OUT_REL"
    mkdir -p "$OUT_REL"
    local t exe
    for t in "${TESTS[@]}"; do
        exe="$(cargo test --locked -p k2-node --test "$t" --no-run --message-format=json \
            | python3 -c 'import json,sys
want = sys.argv[1]
for l in sys.stdin:
    try: m = json.loads(l)
    except ValueError: continue
    if m.get("reason") == "compiler-artifact" and m.get("target", {}).get("name") == want and m.get("executable"):
        print(m["executable"])' "$t")"
        [ -n "$exe" ] && [ -x "$exe" ] || { echo "FATAL: no $t test binary" >&2; exit 1; }
        cp "$exe" "$OUT_REL/$t"
    done
    cp "${CARGO_TARGET_DIR:-target}/debug/k2-node" "$OUT_REL/k2-node"
    if command -v strip >/dev/null; then
        strip --strip-debug "$OUT_REL"/*
    fi
    (cd "$OUT_REL" && sha256sum ./*)
}

if [ "${1:-}" = "--build-job" ]; then
    build_job
    exit 0
fi

HOST="${1:-}"
[ -n "$HOST" ] || { echo "usage: scripts/ci/node-userns-root-e2e.sh <ssh-host> [--sha REV]" >&2; exit 2; }
shift
SHA="HEAD"
while [ $# -gt 0 ]; do
    case "$1" in
        --sha) SHA="${2:?}"; shift 2 ;;
        *) echo "usage: unknown argument $1" >&2; exit 2 ;;
    esac
done
DISTRO="${NODE_E2E_DISTRO-ubuntu@24.04}"

tmp="$(mktemp -d "${TMPDIR:-/tmp}/k2-node-userns-e2e.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT

echo "== build on K2 Compute (${DISTRO:-any distro}, $SHA)"
distro_arg=()
[ -n "$DISTRO" ] && distro_arg=(--distro "$DISTRO")
if ! k2 compute run --sha "$SHA" ${distro_arg[@]+"${distro_arg[@]}"} --needs cmake@3 --cpus 4 --mem 16G \
    --stall-timeout off --artifact "$OUT_REL/*" \
    -- bash scripts/ci/node-userns-root-e2e.sh --build-job 2>&1 | tee "$tmp/build.log"; then
    echo "FATAL: the build job didn't run or failed" >&2
    exit 1
fi
job="$(sed -n -E 's/^\[k2 compute\] job ([0-9a-f]+) on .*/\1/p' "$tmp/build.log" | head -1)"
[ -n "$job" ] || { echo "FATAL: no job id in the build output" >&2; exit 1; }
grep -q '^\[k2 compute\] exit 0 ' "$tmp/build.log" || { echo "FATAL: the build job $job failed" >&2; exit 1; }
mkdir -p "$tmp/bin"
k2 compute fetch "$job" --out "$tmp/bin" --over-relay
bin_dir="$(dirname "$(find "$tmp/bin" -name k2-node -type f | head -1)")"
for f in k2-node "${TESTS[@]}"; do
    [ -f "$bin_dir/$f" ] || { echo "FATAL: fetched artifacts lack $f" >&2; exit 1; }
done

REMOTE=/tmp/k2-node-userns-e2e
echo "== copy to $HOST:$REMOTE"
ssh "$HOST" "rm -rf $REMOTE && mkdir -p $REMOTE"
scp -q "$bin_dir/k2-node" "${TESTS[@]/#/$bin_dir/}" "$HOST:$REMOTE/"
ssh "$HOST" "chmod 0755 $REMOTE/*"

# Everything below runs on the box as root, in one script, so the restore
# runs whatever the tests did.
cat > "$tmp/remote.sh" <<'REMOTE_SH'
set -uo pipefail
R=/tmp/k2-node-userns-e2e
S="$R/state"
# Chrome lives outside /tmp and /home: a job unit sees neither.
CHROME_DIR=/var/lib/k2-userns-e2e-chrome
mkdir -p "$S"
snapshot() {
    getent passwd | cut -d: -f1 | sort > "$S/users.$1"
    getent group | cut -d: -f1 | sort > "$S/groups.$1"
    cat /proc/sys/user/max_user_namespaces > "$S/maxns.$1"
    systemctl list-units --all --plain --no-legend 'k2job-*' | awk '{print $1}' | sort > "$S/units.$1"
    ip netns list 2>/dev/null | awk '{print $1}' | sort > "$S/netns.$1"
    ls -d /var/lib/k2-nodesec-test-* /run/k2na-test-*.sock "$CHROME_DIR" 2>/dev/null | sort > "$S/paths.$1"
    dpkg-query -W -f '${Package}\n' 2>/dev/null | sort > "$S/packages.$1"
}
snapshot before
rc=0
apt_install() {
    DEBIAN_FRONTEND=noninteractive apt-get install -y -q --no-install-recommends "$@" >/dev/null
}
echo "== set up (what isn't here is installed now and removed at the end)"
apt-get update -q >/dev/null
apt_install acl podman crun uidmap unzip ca-certificates curl python3 || { echo "FATAL: can't install the test packages" >&2; rc=1; }
# Chrome headless shell's libraries (the sound one is named per release).
apt_install libnss3 libatk-bridge2.0-0 libatk1.0-0 libcups2 libdrm2 libgbm1 libxkbcommon0 libxcomposite1 libxdamage1 libxrandr2 libxfixes3 libpango-1.0-0 libcairo2 \
    || { echo "FATAL: can't install Chrome's libraries" >&2; rc=1; }
apt_install libasound2t64 2>/dev/null || apt_install libasound2 || { echo "FATAL: no libasound" >&2; rc=1; }
mkdir -p "$CHROME_DIR"
url="$(curl -fsSL https://googlechromelabs.github.io/chrome-for-testing/last-known-good-versions-with-downloads.json \
    | python3 -c 'import json,sys
d = json.load(sys.stdin)["channels"]["Stable"]
print(next(x["url"] for x in d["downloads"]["chrome-headless-shell"] if x["platform"] == "linux64"))')"
[ -n "$url" ] || { echo "FATAL: no Chrome headless shell download" >&2; rc=1; }
echo "chrome: $url"
curl -fsSL "$url" -o "$CHROME_DIR/chs.zip" && unzip -q "$CHROME_DIR/chs.zip" -d "$CHROME_DIR" || { echo "FATAL: Chrome download" >&2; rc=1; }
CHROME="$(find "$CHROME_DIR" -name chrome-headless-shell -type f | head -1)"
chmod -R a+rX "$CHROME_DIR"
if ldd "$CHROME" | grep -q 'not found'; then
    echo "FATAL: Chrome's libraries are missing:" >&2
    ldd "$CHROME" | grep 'not found' >&2
    rc=1
fi
cd /
run() {
    echo "== $*"
    env K2_NODE_ROOT_E2E=1 K2_NODE_E2E_BIN="$R/k2-node" K2_E2E_CHROME="$CHROME" "$@" --test-threads=1 --nocapture || rc=1
}
if [ "$rc" = 0 ]; then
    # The shim tests (default and the ones that need a plain box).
    run "$R/job_userns_linux" --include-ignored
    run "$R/job_wall_linux" --ignored --exact a_job_makes_no_user_namespace_unless_userns_and_its_limit_is_its_own
    run "$R/tenant_wall_linux" --ignored --exact userns_and_the_denylist_are_walled_for_team_own_and_owner_jobs
    # --userns end to end: rootless Podman (as it comes, and keyring = false)
    # and Chromium's namespace sandbox, in a real unit.
    run "$R/job_wall_linux" --ignored --exact a_userns_job_runs_rootless_podman_and_chromiums_sandbox
fi

echo "== restore"
snapshot after
for u in $(comm -13 "$S/users.before" "$S/users.after"); do userdel "$u" && echo "removed user $u"; done
for g in $(comm -13 "$S/groups.before" "$S/groups.after"); do getent group "$g" >/dev/null && groupdel "$g" && echo "removed group $g"; done
for u in $(comm -13 "$S/units.before" "$S/units.after"); do systemctl stop "$u" 2>/dev/null; systemctl reset-failed "$u" 2>/dev/null; echo "stopped unit $u"; done
for n in $(comm -13 "$S/netns.before" "$S/netns.after"); do ip netns delete "$n" && echo "removed netns $n"; done
for p in $(comm -13 "$S/paths.before" "$S/paths.after"); do rm -rf -- "$p" && echo "removed $p"; done
added="$(comm -13 "$S/packages.before" "$S/packages.after" | tr '\n' ' ')"
if [ -n "${added// /}" ]; then
    # shellcheck disable=SC2086 # one word per package
    DEBIAN_FRONTEND=noninteractive apt-get purge -y -q $added >/dev/null && echo "removed packages: $added"
fi
snapshot restored
for k in users groups maxns units netns paths packages; do
    if ! cmp -s "$S/$k.before" "$S/$k.restored"; then
        echo "FATAL: $k differs from before the run:" >&2
        diff "$S/$k.before" "$S/$k.restored" >&2
        rc=1
    fi
done
cmp -s "$S/maxns.before" "$S/maxns.after" || { echo "FATAL: the machine's max_user_namespaces changed during the run" >&2; rc=1; }
echo "machine max_user_namespaces: $(cat "$S/maxns.before") before, $(cat "$S/maxns.after") after the tests"
exit $rc
REMOTE_SH
scp -q "$tmp/remote.sh" "$HOST:$REMOTE/remote.sh"
echo "== run on $HOST (root)"
rc=0
ssh "$HOST" "sudo bash $REMOTE/remote.sh" || rc=$?
ssh "$HOST" "sudo rm -rf $REMOTE" || { echo "FATAL: couldn't remove $REMOTE on $HOST" >&2; rc=1; }
exit $rc
