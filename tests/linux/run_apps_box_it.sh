#!/usr/bin/env bash
# Linux run for App runtime box prep (root helper protocol 6, runtime
# slice 2): from a dev Mac, on the build box, in ONE new dir and
# throwaway podman containers. Never touches the box's own services,
# users, sysctls, nftables or /etc.
#
#   bash tests/linux/run_apps_box_it.sh [user@host] [dir]
#   (default root@37.27.67.180, /root/k2-rt-s2-<date>)
#
# 1. Ships this checkout (git archive HEAD) to <dir>/src.
# 2. Builds there with <dir>/target: the test-keys root helper (current
#    and a 0.45.99 "next"), the TEST signer, k2-daemon (debug), and the
#    root-proto/root-helper unit-test binaries.
# 3. Unit tests in ubuntu:24.04 as uid 1000 (some refuse to run as root).
# 4. tests/linux/root_helper_in_container.sh (the existing helper IT, now
#    protocol 6) in an Ubuntu 24.04 systemd container.
# 5. tests/linux/apps_box_in_container.sh and apps_box_daemon_e2e.sh,
#    each in a fresh Debian 13 systemd container with its OWN user
#    namespace (container root is a high host uid, so a stray non-netns
#    sysctl write is denied by the kernel) and its own network namespace
#    (nft and ip_local_reserved_ports change only the container).
# 6. tests/cli/app_runtime_cli_smoke.sh in a plain ubuntu:24.04 container.
# 7. Removes the containers, the images it built, and <dir> (logs are
#    copied back to ./target-it-logs first).
set -euo pipefail

HOST="${1:-root@37.27.67.180}"
R="${2:-/root/k2-rt-s2-$(date +%Y%m%d)}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
STAMP="$(date +%Y%m%d%H%M%S)"
SSH=(ssh -o BatchMode=yes -o ConnectTimeout=15 -o ServerAliveInterval=30 "$HOST")
LOGS="$ROOT/target-it-logs/apps-box-$STAMP"
mkdir -p "$LOGS"

# K2_IT_KEEP=1 keeps <dir> (and its target) for another run; a dir this
# runner didn't make is never reused.
KEEP="${K2_IT_KEEP:-0}"
"${SSH[@]}" "df -h / | tail -n 1; if [ -e $R ]; then [ -f $R/.k2-apps-box-it ] || { echo 'refusing: $R exists and is not ours'; exit 1; }; fi"

HOSTSTATE="cat /proc/sys/net/ipv4/ip_local_reserved_ports; nft list tables 2>/dev/null | sort; id k2app 2>&1 | head -c 60; echo; ls /etc/systemd/system | grep -c k2app || true"
"${SSH[@]}" "$HOSTSTATE" >"$LOGS/host-before.txt"

finish() {
	"${SSH[@]}" "$HOSTSTATE" >"$LOGS/host-after.txt" || true
	if cmp -s "$LOGS/host-before.txt" "$LOGS/host-after.txt"; then
		echo "== host unchanged (reserved ports, nft tables, no k2app user or unit) =="
	else
		echo "== HOST STATE CHANGED ==" >&2
		diff "$LOGS/host-before.txt" "$LOGS/host-after.txt" >&2 || true
	fi
	echo "== cleanup on $HOST =="
	"${SSH[@]}" "podman rm -f k2rh-it-$STAMP k2apps-it-$STAMP k2apps-e2e-$STAMP >/dev/null 2>&1 || true; podman rmi -f localhost/k2rh-it:rts2 localhost/k2-apps-box-it:rts2 >/dev/null 2>&1 || true; if [ '$KEEP' = 1 ]; then echo 'kept $R'; else rm -rf $R; fi; df -h / | tail -n 1" || true
}
trap finish EXIT

echo "== ship this checkout to $HOST:$R/src =="
git -C "$ROOT" archive --format=tar HEAD | "${SSH[@]}" "rm -rf $R/src $R/it && mkdir -p $R/src $R/it/bin $R/it/scripts $R/it/tests $R/it/cli && touch $R/.k2-apps-box-it && tar -xf - -C $R/src"

echo "== build on $HOST (target $R/target) =="
"${SSH[@]}" bash -s -- "$R" <<'REMOTE' 2>&1 | tee "$LOGS/build.log"
set -euo pipefail
R="$1"
export PATH="$HOME/.cargo/bin:$PATH" CARGO_TARGET_DIR="$R/target"
cd "$R/src"
cargo build --release -j 8 -p k2-root-helper --features test-keys --bin k2-root-helper --example k2-root-helper-testsign 2>&1 | tail -n 2
cp "$R/target/release/k2-root-helper" "$R/it/bin/k2-root-helper-cur"
cp "$R/target/release/examples/k2-root-helper-testsign" "$R/it/bin/testsign"
cp crates/k2-daemon/Cargo.toml /tmp/k2-daemon-Cargo.toml.$$
sed -i 's/^version = "[0-9.]*"$/version = "0.45.99"/' crates/k2-daemon/Cargo.toml
cargo build --release -j 8 -p k2-root-helper --features test-keys --bin k2-root-helper 2>&1 | tail -n 1
cp "$R/target/release/k2-root-helper" "$R/it/bin/k2-root-helper-next"
mv /tmp/k2-daemon-Cargo.toml.$$ crates/k2-daemon/Cargo.toml
cargo build -j 8 -p k2-daemon --bin k2-daemon 2>&1 | tail -n 2
cp "$R/target/debug/k2-daemon" "$R/it/bin/k2-daemon"
bins() { python3 -c 'import sys,json
for l in sys.stdin:
    try: m=json.loads(l)
    except Exception: continue
    if m.get("reason")=="compiler-artifact" and m.get("executable") and m.get("profile",{}).get("test"): print(m["executable"])'; }
cargo test --release -j 8 -p k2-root-proto --features test-signer --no-run --message-format=json 2>/dev/null | bins >"$R/unit-bins.txt"
cargo test --release -j 8 -p k2-root-helper --no-run --message-format=json 2>/dev/null | bins >>"$R/unit-bins.txt"
cp scripts/install-root-helper.sh scripts/k2-pg-helper "$R/it/scripts/"
cp tests/linux/root_helper_in_container.sh tests/linux/apps_box_in_container.sh tests/linux/apps_box_daemon_e2e.sh tests/linux/apps_box_netns.sh \
   tests/linux/Containerfile.root-helper-it tests/linux/Containerfile.apps-box-it "$R/it/tests/"
cp cli/k2 "$R/it/cli/k2"
chmod -R a+rX "$R"
file "$R/it/bin/"* | sed "s#$R/##"
REMOTE

echo "== unit tests in ubuntu:24.04 as uid 1000 =="
"${SSH[@]}" bash -s -- "$R" <<'REMOTE' 2>&1 | tee "$LOGS/unit.log"
set -euo pipefail
R="$1"
podman run --rm -u 1000:1000 --tmpfs /root:rw,mode=0755 -v "$R:$R:ro" -w "$R/src/crates/k2-root-helper" docker.io/library/ubuntu:24.04 \
  bash -c 'set -eu; while read -r b; do echo "--- ${b##*/}"; out="$("$b" --test-threads=4 2>&1)" || { echo "$out"; echo "FAIL: $b"; exit 1; }; printf "%s\n" "$out" | grep -E "^test result"; done <'"$R"'/unit-bins.txt'
REMOTE

run_ct() { # name image script [podman flags…]
	local name="$1" image="$2" script="$3"
	shift 3
	"${SSH[@]}" bash -s -- "$R" "$name" "$image" "$script" "$@" <<'REMOTE'
set -euo pipefail
R="$1"; NAME="$2"; IMAGE="$3"; SCRIPT="$4"; shift 4
cleanup() { podman rm -f "$NAME" >/dev/null 2>&1 || true; }
trap cleanup EXIT
podman run -d --name "$NAME" --systemd=always "$@" -v "$R/it:/it:ro" "$IMAGE" /sbin/init >/dev/null
for _ in $(seq 1 90); do
  s="$(podman exec "$NAME" systemctl is-system-running 2>/dev/null || true)"
  case "$s" in running|degraded) break ;; esac
  sleep 1
done
echo "container $NAME systemd: $s"
podman exec "$NAME" bash "/it/tests/$SCRIPT"
REMOTE
}

echo "== images =="
"${SSH[@]}" "set -e; podman build -q -t localhost/k2rh-it:rts2 -f $R/it/tests/Containerfile.root-helper-it $R/it/tests >/dev/null; podman build -q -t localhost/k2-apps-box-it:rts2 -f $R/it/tests/Containerfile.apps-box-it $R/it/tests >/dev/null; printf 'N\n' >$R/it/apparmor-off; echo built"

# Own user namespace (8M ids at 2^31: no host user or other container
# lives there), so every capability is held in THAT namespace only.
# --network=none: the runtime makes the netns inside that user namespace,
# so nft and the net sysctls box prep sets are the container's own.
# /proc/sys unmasked and AppArmor off (Podman's profile denies every
# /proc/sys write): host-wide sysctls stay owned by the host's root
# ("nobody" in here) and the kernel refuses them — checked in the probes
# and by host-before/after below. The host kernel's AppArmor is hidden
# (enabled = N): a container can't load profiles; the k2-app profile is
# checked in the guarded VM e2e (tests/linux/apps_guarded_e2e.sh).
USERNS=(--network=none --uidmap 0:2147483648:8388608 --gidmap 0:2147483648:8388608
	--security-opt unmask=/proc/sys --security-opt apparmor=unconfined
	--cap-add NET_ADMIN --cap-add SYS_PTRACE
	-v "$R/it/apparmor-off:/sys/module/apparmor/parameters/enabled:ro")

rc=0
echo "== existing helper IT (protocol 6) in Ubuntu 24.04 =="
run_ct "k2rh-it-$STAMP" localhost/k2rh-it:rts2 root_helper_in_container.sh --cap-add SYS_PTRACE --cap-add AUDIT_CONTROL \
	2>&1 | tee "$LOGS/root-helper-it.log" || rc=1
echo "== apps box prep, helper level, Debian 13 =="
run_ct "k2apps-it-$STAMP" localhost/k2-apps-box-it:rts2 apps_box_in_container.sh "${USERNS[@]}" \
	2>&1 | tee "$LOGS/apps-box-it.log" || rc=1
echo "== apps box prep, daemon end to end, Debian 13 =="
run_ct "k2apps-e2e-$STAMP" localhost/k2-apps-box-it:rts2 apps_box_daemon_e2e.sh "${USERNS[@]}" \
	2>&1 | tee "$LOGS/apps-box-e2e.log" || rc=1

echo "== CLI smoke (sandbox daemon) in ubuntu:24.04 =="
"${SSH[@]}" bash -s -- "$R" <<'REMOTE' 2>&1 | tee "$LOGS/cli-smoke.log" || rc=1
set -euo pipefail
R="$1"
podman run --rm -v "$R/src:/src:ro" -v "$R/it/bin:/bin-it:ro" docker.io/library/ubuntu:24.04 bash -c '
set -e
apt-get update -qq >/dev/null && apt-get install -y -qq --no-install-recommends git python3 curl ca-certificates >/dev/null
mkdir -p /t/debug && cp /bin-it/k2-daemon /t/debug/k2-daemon
CARGO_TARGET_DIR=/t bash /src/tests/cli/app_runtime_cli_smoke.sh'
REMOTE

echo "== logs: $LOGS (rc $rc) =="
exit "$rc"
