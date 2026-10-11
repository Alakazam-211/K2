#!/usr/bin/env bash
# Linux run for the root helper (prd-k2-root-helper-v1 §14): from a dev Mac,
# on the build box, in an isolated dir and throwaway podman containers.
# Never touches the box's own services, users or /etc.
#
#   bash tests/linux/run_root_helper_it.sh [user@host]   (default root@37.27.67.180)
#
# 1. Ships a minimal workspace (k2-root-proto + k2-root-helper + the daemon
#    manifest build.rs reads) to /root/k2-roothelper-it-<stamp>/.
# 2. Builds there with its own CARGO_TARGET_DIR: the current-version
#    test-keys helper, a 0.45.99 test-keys helper, the TEST signer, and the
#    unit-test binaries.
# 3. Runs the unit tests in an ubuntu:24.04 container as uid 1000.
# 4. Runs tests/linux/root_helper_in_container.sh as root in a systemd
#    container (Containerfile.root-helper-it), then removes it.
set -euo pipefail

HOST="${1:-root@37.27.67.180}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
STAMP="$(date +%Y%m%d%H%M%S)"
R="/root/k2-roothelper-it-$STAMP"
SSH=(ssh -o BatchMode=yes -o ConnectTimeout=15 "$HOST")

echo "== ship a minimal workspace to $HOST:$R =="
STAGE="$(mktemp -d -t k2rh-it)"
trap 'rm -rf "$STAGE"' EXIT
mkdir -p "$STAGE/src/crates/k2-daemon" "$STAGE/it/scripts" "$STAGE/it/tests"
cp -R "$ROOT/crates/k2-root-proto" "$ROOT/crates/k2-root-helper" "$STAGE/src/crates/"
cp "$ROOT/crates/k2-daemon/Cargo.toml" "$STAGE/src/crates/k2-daemon/Cargo.toml"
cp "$ROOT/Cargo.lock" "$STAGE/src/Cargo.lock"
cp "$ROOT/scripts/k2-pg-helper" "$STAGE/src/k2-pg-helper"
cat >"$STAGE/src/Cargo.toml" <<'EOF'
[workspace]
resolver = "2"
members = ["crates/k2-root-proto", "crates/k2-root-helper"]
[profile.release]
strip = true
lto = true
codegen-units = 1
EOF
mkdir -p "$STAGE/src/scripts"
cp "$ROOT/scripts/k2-pg-helper" "$STAGE/src/scripts/k2-pg-helper"
cp "$ROOT/scripts/install-root-helper.sh" "$ROOT/scripts/k2-pg-helper" "$STAGE/it/scripts/"
cp "$ROOT/tests/linux/root_helper_in_container.sh" "$ROOT/tests/linux/Containerfile.root-helper-it" "$STAGE/it/tests/"
COPYFILE_DISABLE=1 tar --no-xattrs -C "$STAGE" -czf - src it | "${SSH[@]}" "mkdir -p $R && tar -xzf - -C $R"

echo "== build on $HOST (isolated target dir) =="
"${SSH[@]}" bash -s -- "$R" <<'REMOTE'
set -euo pipefail
R="$1"
export PATH="$HOME/.cargo/bin:$PATH" CARGO_TARGET_DIR="$R/target"
cd "$R/src"
cargo build --release -p k2-root-helper --features test-keys --bin k2-root-helper --example k2-root-helper-testsign 2>&1 | tail -n 3
mkdir -p "$R/it/bin"
cp "$R/target/release/k2-root-helper" "$R/it/bin/k2-root-helper-cur"
cp "$R/target/release/examples/k2-root-helper-testsign" "$R/it/bin/testsign"
# A "next" build: same source, the daemon manifest says 0.45.99.
cp crates/k2-daemon/Cargo.toml crates/k2-daemon/Cargo.toml.orig
sed -i 's/^version = "[0-9.]*"$/version = "0.45.99"/' crates/k2-daemon/Cargo.toml
cargo build --release -p k2-root-helper --features test-keys --bin k2-root-helper 2>&1 | tail -n 1
cp "$R/target/release/k2-root-helper" "$R/it/bin/k2-root-helper-next"
# Unit-test binaries (current version again).
mv crates/k2-daemon/Cargo.toml.orig crates/k2-daemon/Cargo.toml
cargo test --release -p k2-root-proto --features test-signer --no-run --message-format=json 2>/dev/null \
  | python3 -c 'import sys,json
for l in sys.stdin:
    try: m=json.loads(l)
    except Exception: continue
    if m.get("reason")=="compiler-artifact" and m.get("executable") and m.get("profile",{}).get("test"): print(m["executable"])' >"$R/unit-bins.txt"
cargo test --release -p k2-root-helper --no-run --message-format=json 2>/dev/null \
  | python3 -c 'import sys,json
for l in sys.stdin:
    try: m=json.loads(l)
    except Exception: continue
    if m.get("reason")=="compiler-artifact" and m.get("executable") and m.get("profile",{}).get("test"): print(m["executable"])' >>"$R/unit-bins.txt"
echo "unit test binaries:"; cat "$R/unit-bins.txt"
file "$R/it/bin/"*
REMOTE

echo "== unit tests in ubuntu:24.04 as uid 1000 =="
"${SSH[@]}" bash -s -- "$R" <<'REMOTE'
set -euo pipefail
R="$1"
chmod -R a+rX "$R"
podman run --rm -u 1000:1000 --tmpfs /root:rw,mode=0755 -v "$R:$R:ro" -w "$R/src/crates/k2-root-helper" docker.io/library/ubuntu:24.04 \
  bash -c 'set -eu; while read -r b; do echo "--- $b"; out="$("$b" --test-threads=4 2>&1)" || { echo "$out"; echo "FAIL: $b"; exit 1; }; printf "%s\n" "$out" | grep -E "^test result"; done <'"$R"'/unit-bins.txt'
REMOTE

echo "== integration in a systemd container =="
"${SSH[@]}" bash -s -- "$R" "$STAMP" <<'REMOTE'
set -euo pipefail
R="$1"; STAMP="$2"; NAME="k2rh-it-$STAMP"
podman build -q -t localhost/k2rh-it:latest -f "$R/it/tests/Containerfile.root-helper-it" "$R/it/tests" >/dev/null
cleanup() { podman rm -f "$NAME" >/dev/null 2>&1 || true; }
trap cleanup EXIT
# SYS_PTRACE: the caller walk reads /proc/<sudo pid>/exe (PTRACE_MODE_READ).
# Root on a real box has it; podman drops it by default, and without it
# every daemon verb fails closed with 77. AUDIT_CONTROL: pam_loginuid for
# user@<uid> (the user-scope case).
podman run -d --name "$NAME" --systemd=always --cap-add SYS_PTRACE --cap-add AUDIT_CONTROL -v "$R/it:/it:ro" localhost/k2rh-it:latest /sbin/init >/dev/null
for _ in $(seq 1 60); do
  s="$(podman exec "$NAME" systemctl is-system-running 2>/dev/null || true)"
  case "$s" in running|degraded) break ;; esac
  sleep 1
done
echo "container systemd: $s"
podman exec "$NAME" bash /it/tests/root_helper_in_container.sh
REMOTE
echo "== done ($HOST:$R kept for inspection; remove with: ssh $HOST rm -rf $R) =="
