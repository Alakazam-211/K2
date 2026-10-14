#!/usr/bin/env bash
# The guarded App engine on Linux (runtime slice 3), from a dev Mac, on the
# build box, in ONE dir of its own:
#
#   bash tests/linux/run_apps_guarded_vm.sh <step…> [-- user@host dir]
#   steps: ship build suites vm clean   (default: all of them, in order)
#   default host root@37.27.67.180, dir /root/k2-rt-s3-<date>
#
# ship    git archive HEAD → <dir>/src
# build   <dir>/target: k2-daemon (debug), the test-keys root helper + its
#         TEST signer, k2-app-engine (release); stage <dir>/it
# suites  the full k2-core and k2-daemon suites plus the root/engine crates,
#         on the box, with their own TMPDIR and XDG_RUNTIME_DIR under <dir>
#         (never root's /run/user/0) and a clean env (no K2_* variables)
# vm      a throwaway Debian 13 VM (qemu/KVM inside a throwaway container;
#         tests/linux/guarded_vm_driver.sh) runs tests/linux/apps_guarded_e2e.sh
#         as root. Nothing runs on the box host itself but the build, the
#         suites (temp dirs only) and podman for that one container.
# clean   remove the container image this made and <dir>.
#
# Logs come back to ./target-it-logs/guarded-<stamp>/.
set -euo pipefail

STEPS=()
while [ $# -gt 0 ] && [ "$1" != "--" ]; do STEPS+=("$1"); shift; done
[ "${1:-}" = "--" ] && shift
HOST="${1:-root@37.27.67.180}"
R="${2:-/root/k2-rt-s3-$(date +%Y%m%d)}"
[ ${#STEPS[@]} -gt 0 ] || STEPS=(ship build suites vm clean)
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
STAMP="$(date +%Y%m%d%H%M%S)"
SSH=(ssh -o BatchMode=yes -o ConnectTimeout=15 -o ServerAliveInterval=30 "$HOST")
LOGS="$ROOT/target-it-logs/guarded-$STAMP"
mkdir -p "$LOGS"
IMAGE=localhost/k2-vm-it:rts3

"${SSH[@]}" "df -h / | tail -n 1; if [ -e $R ]; then [ -f $R/.k2-rt-s3 ] || { echo 'refusing: $R exists and is not ours'; exit 1; }; fi"

for step in "${STEPS[@]}"; do
	case "$step" in
	ship)
		echo "== ship this checkout to $HOST:$R/src =="
		git -C "$ROOT" archive --format=tar HEAD | "${SSH[@]}" "mkdir -p $R && touch $R/.k2-rt-s3 && rm -rf $R/src && mkdir -p $R/src && tar -xf - -C $R/src"
		;;
	build)
		echo "== build on $HOST (target $R/target) =="
		"${SSH[@]}" bash -s -- "$R" <<'REMOTE' 2>&1 | tee "$LOGS/build.log"
set -euo pipefail
R="$1"
export PATH="$HOME/.cargo/bin:$PATH" CARGO_TARGET_DIR="$R/target"
cd "$R/src"
cargo build -j 8 -p k2-daemon --bin k2-daemon 2>&1 | tail -n 1
cargo build --release -j 8 -p k2-root-helper --features test-keys --bin k2-root-helper --example k2-root-helper-testsign 2>&1 | tail -n 1
cargo build --release -j 8 -p k2-app-engine 2>&1 | tail -n 1
rm -rf "$R/it" && mkdir -p "$R/it/bin" "$R/it/cli" "$R/it/scripts" "$R/it/tests"
cp "$R/target/debug/k2-daemon" "$R/it/bin/k2-daemon"
cp "$R/target/release/k2-root-helper" "$R/it/bin/k2-root-helper-cur"
cp "$R/target/release/examples/k2-root-helper-testsign" "$R/it/bin/testsign"
cp "$R/target/release/k2-app-engine" "$R/it/bin/k2-app-engine"
cp cli/k2 "$R/it/cli/k2"
cp scripts/install-root-helper.sh "$R/it/scripts/"
cp -r tests/linux/apps_guarded_e2e.sh tests/linux/engine_client.py tests/linux/guarded-fixture tests/linux/guarded_vm_driver.sh \
   tests/linux/Containerfile.vm-it "$R/it/tests/"
chmod -R a+rX "$R/it"
file "$R/it/bin/"* | sed "s#$R/##"
REMOTE
		;;
	tests)
		# Iterate on the e2e scripts without a rebuild (working tree copies).
		echo "== refresh $R/it/tests and cli from this working tree =="
		tar -C "$ROOT/tests/linux" -cf - apps_guarded_e2e.sh engine_client.py guarded-fixture guarded_vm_driver.sh Containerfile.vm-it \
			| "${SSH[@]}" "tar -C $R/it/tests -xf - && chmod -R a+rX $R/it/tests"
		"${SSH[@]}" "cat > $R/it/cli/k2 && chmod 0755 $R/it/cli/k2" <"$ROOT/cli/k2"
		;;
	suites)
		echo "== suites on $HOST (own TMPDIR/XDG_RUNTIME_DIR under $R/rt) =="
		"${SSH[@]}" bash -s -- "$R" <<'REMOTE' 2>&1 | tee "$LOGS/suites.log"
set -uo pipefail
R="$1"
RT="$R/rt/$(date +%s)"
mkdir -p "$RT/tmp" "$RT/xdg" && chmod 700 "$RT/xdg"
run() {
	env -i HOME="$RT/home" USER=root PATH="$HOME/.cargo/bin:/usr/local/bin:/usr/bin:/bin" LANG=C.UTF-8 \
		TMPDIR="$RT/tmp" XDG_RUNTIME_DIR="$RT/xdg" CARGO_TARGET_DIR="$R/target" "$@"
}
mkdir -p "$RT/home"
cd "$R/src"
rc=0
for args in "-p k2-root-proto --features test-signer" "-p k2-root-helper" "-p k2-app-engine" "-p k2-core" "-p k2-daemon"; do
	echo "--- cargo test $args"
	# shellcheck disable=SC2086
	run cargo test -j 8 $args -- --test-threads=8 >"$RT/out.txt" 2>&1
	r=$?
	grep -E "^test result|FAILED|panicked at" "$RT/out.txt" | sort | uniq -c | sort -rn | head -n 30
	[ "$r" = 0 ] || { rc=1; grep -E "^    [a-z_:]+$" "$RT/out.txt" | head -n 40; }
done
echo "--- bash tests/cli/app_runtime_cli_smoke.sh"
run bash tests/cli/app_runtime_cli_smoke.sh 2>&1 | tail -n 4 || rc=1
echo "suites rc $rc"
exit "$rc"
REMOTE
		;;
	vm)
		echo "== the guarded e2e in a throwaway Debian 13 VM =="
		"${SSH[@]}" bash -s -- "$R" "$IMAGE" "$STAMP" "${K2_IT_BAKED:-0}" <<'REMOTE' 2>&1 | tee "$LOGS/vm.log"
set -euo pipefail
R="$1"; IMAGE="$2"; STAMP="$3"; BAKED="$4"
mkdir -p "$R/vm"
podman build -q -t "$IMAGE" -f "$R/it/tests/Containerfile.vm-it" "$R/it/tests" >/dev/null
NAME="k2-vm-it-$STAMP"
trap 'podman rm -f "$NAME" >/dev/null 2>&1 || true' EXIT
podman run --rm --name "$NAME" -e K2_IT_BAKED="$BAKED" --device /dev/kvm -v "$R/vm:/vm" -v "$R/it:/it:ro" "$IMAGE" bash /it/tests/guarded_vm_driver.sh
REMOTE
		;;
	clean)
		echo "== clean up on $HOST =="
		"${SSH[@]}" "podman rm -f \$(podman ps -aq --filter name=k2-vm-it-) >/dev/null 2>&1 || true; podman rmi -f $IMAGE >/dev/null 2>&1 || true; rm -rf $R; df -h / | tail -n 1"
		;;
	*) echo "unknown step $step" >&2; exit 2 ;;
	esac
done
echo "== logs: $LOGS =="
