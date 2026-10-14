#!/usr/bin/env bash
# Inside the qemu container (tests/linux/Containerfile.vm-it): boot a
# throwaway Debian 13 cloud VM, copy /it into it, run
# tests/linux/apps_guarded_e2e.sh there as root, then power it off.
#   /vm  scratch (the cached base images + this run's overlay disk)
#   /it  this run's binaries, CLI, scripts and tests (read-only)
#
# K2_IT_BAKED=1 boots from a cached disk that already has the test prereqs
# AND the distro container packages (faster iterations; box prep's apt
# step is then a no-op). Default: the plain cloud image, so box prep
# installs Podman, crun, passt and skopeo from the distro for real.
set -euo pipefail
IMG=/vm/debian-13-genericcloud-amd64.qcow2
BAKED=/vm/baked.qcow2
URL=https://cloud.debian.org/images/cloud/trixie/latest/debian-13-genericcloud-amd64.qcow2
if [ ! -s "$IMG" ]; then
	curl -fsSL -o "$IMG.part" "$URL"
	mv "$IMG.part" "$IMG"
fi
RUN=/vm/run
PREREQS="sudo python3 curl git postgresql nftables iproute2 procps"
CONTAINER_PKGS="podman crun passt uidmap dbus-user-session skopeo catatonit"

boot() { # <backing image>
	rm -rf "$RUN" && mkdir -p "$RUN"
	qemu-img create -q -f qcow2 -b "$1" -F qcow2 "$RUN/disk.qcow2" 40G
	ssh-keygen -q -t ed25519 -N '' -f "$RUN/key"
	cat >"$RUN/user-data" <<EOF
#cloud-config
disable_root: false
ssh_pwauth: false
users:
  - name: root
    ssh_authorized_keys:
      - $(cat "$RUN/key.pub")
EOF
	printf 'instance-id: k2-guarded-e2e-%s\nlocal-hostname: k2-guarded-e2e\n' "$RANDOM$RANDOM" >"$RUN/meta-data"
	cloud-localds "$RUN/seed.iso" "$RUN/user-data" "$RUN/meta-data"
	qemu-system-x86_64 -enable-kvm -cpu host -m 8192 -smp 4 \
		-drive file="$RUN/disk.qcow2",if=virtio -drive file="$RUN/seed.iso",if=virtio,media=cdrom \
		-netdev user,id=n0,hostfwd=tcp:127.0.0.1:2222-:22 -device virtio-net-pci,netdev=n0 \
		-display none -serial file:"$RUN/serial.log" -daemonize -pidfile "$RUN/qemu.pid"
	SSH=(ssh -q -i "$RUN/key" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o ConnectTimeout=5 -p 2222 root@127.0.0.1)
	for _ in $(seq 1 120); do "${SSH[@]}" true 2>/dev/null && break; sleep 2; done
	"${SSH[@]}" true || { tail -n 40 "$RUN/serial.log"; echo "VM did not come up" >&2; exit 1; }
	"${SSH[@]}" 'cloud-init status --wait >/dev/null 2>&1 || true'
}
down() {
	local qpid
	qpid="$(cat "$RUN/qemu.pid" 2>/dev/null || true)"
	"${SSH[@]}" 'poweroff' >/dev/null 2>&1 || true
	for _ in $(seq 1 30); do [ -n "$qpid" ] && kill -0 "$qpid" 2>/dev/null || break; sleep 1; done
	[ -n "$qpid" ] && kill "$qpid" 2>/dev/null || true
}
apt_in() { "${SSH[@]}" "DEBIAN_FRONTEND=noninteractive apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends $* >/dev/null"; }

if [ "${K2_IT_BAKED:-0}" = 1 ]; then
	if [ ! -s "$BAKED" ]; then
		echo "== baking $BAKED (prereqs + container packages) =="
		boot "$IMG"
		apt_in "$PREREQS $CONTAINER_PKGS"
		"${SSH[@]}" 'cloud-init clean --logs >/dev/null 2>&1; sync'
		down
		qemu-img convert -O qcow2 "$RUN/disk.qcow2" "$BAKED.part"
		mv "$BAKED.part" "$BAKED"
	fi
	boot "$BAKED"
else
	boot "$IMG"
	apt_in "$PREREQS"
fi
echo "== VM up: $("${SSH[@]}" '. /etc/os-release; echo $PRETTY_NAME $(uname -r)') (baked=${K2_IT_BAKED:-0})"
tar -C /it -cf - . | "${SSH[@]}" 'mkdir -p /it && tar -C /it -xf - && chmod -R a+rX /it'
rc=0
"${SSH[@]}" 'bash /it/tests/apps_guarded_e2e.sh' || rc=$?
down
rm -rf "$RUN"
exit "$rc"
