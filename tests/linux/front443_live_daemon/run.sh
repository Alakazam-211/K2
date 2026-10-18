#!/usr/bin/env bash
# front443 with a LIVE daemon under a real systemd (255, Ubuntu 24.04) in a
# throwaway podman container: the field case of pool-09 (the daemon is
# active when the front is enabled). Needs root + podman on the host;
# touches nothing outside the container (its own network namespace, so
# :443 inside it is free whatever the host runs). SYS_PTRACE: the root
# helper reads /proc/<pid>/exe of the k2 user's sudo (a real box's root has it).
#
# Usage: run.sh <k2-daemon> <k2-root-helper> <cli/k2> <evidence dir>
set -euo pipefail
D="$(readlink -f "$1")"; H="$(readlink -f "$2")"; C="$(readlink -f "$3")"; E="$(readlink -f "$4")"
HERE="$(cd "$(dirname "$0")" && pwd)"
NAME="k2-front-live-$$"
mkdir -p "$E" "$E/opt"
cp "$D" "$E/opt/k2-daemon"; cp "$H" "$E/opt/k2-root-helper"; cp "$C" "$E/opt/k2"; chmod 755 "$E/opt/"*
cp "$HERE/inside.sh" "$E/inside.sh"
podman build -q -t localhost/k2-front-live:24.04 -f "$HERE/Containerfile" "$HERE" >/dev/null
cleanup() { podman rm -f "$NAME" >/dev/null 2>&1 || true; }
trap cleanup EXIT
podman run -d --name "$NAME" --systemd=always --cap-add=NET_BIND_SERVICE --cap-add=SYS_PTRACE \
	-v "$E/opt:/opt/k2:ro" -v "$E:/evidence" localhost/k2-front-live:24.04 >/dev/null
for _ in $(seq 100); do
	podman exec "$NAME" systemctl is-system-running 2>/dev/null | grep -qE "running|degraded" && break
	sleep 0.2
done
podman exec "$NAME" bash -c 'systemctl --version | head -1'
podman exec "$NAME" bash /evidence/inside.sh
