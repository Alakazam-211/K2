#!/usr/bin/env bash
# Inside the qemu container (tests/linux/Containerfile.vm-it): boot a
# throwaway Debian 13 cloud VM, copy /it into it, run
# tests/linux/apps_guarded_e2e.sh there as root, then power it off.
#   /vm  scratch (the cached base image + this run's overlay disk)
#   /it  this run's binaries, CLI, scripts and tests (read-only)
set -euo pipefail
IMG=/vm/debian-13-genericcloud-amd64.qcow2
URL=https://cloud.debian.org/images/cloud/trixie/latest/debian-13-genericcloud-amd64.qcow2
if [ ! -s "$IMG" ]; then
	curl -fsSL -o "$IMG.part" "$URL"
	mv "$IMG.part" "$IMG"
fi
RUN=/vm/run
rm -rf "$RUN" && mkdir -p "$RUN"
qemu-img create -q -f qcow2 -b "$IMG" -F qcow2 "$RUN/disk.qcow2" 40G
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
printf 'instance-id: k2-guarded-e2e\nlocal-hostname: k2-guarded-e2e\n' >"$RUN/meta-data"
cloud-localds "$RUN/seed.iso" "$RUN/user-data" "$RUN/meta-data"
qemu-system-x86_64 -enable-kvm -cpu host -m 8192 -smp 4 \
	-drive file="$RUN/disk.qcow2",if=virtio -drive file="$RUN/seed.iso",if=virtio,media=cdrom \
	-netdev user,id=n0,hostfwd=tcp:127.0.0.1:2222-:22 -device virtio-net-pci,netdev=n0 \
	-display none -serial file:"$RUN/serial.log" -daemonize -pidfile "$RUN/qemu.pid"
SSH=(ssh -q -i "$RUN/key" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o ConnectTimeout=5 -p 2222 root@127.0.0.1)
for _ in $(seq 1 120); do "${SSH[@]}" true 2>/dev/null && break; sleep 2; done
"${SSH[@]}" true || { tail -n 40 "$RUN/serial.log"; echo "VM did not come up" >&2; exit 1; }
echo "== VM up: $("${SSH[@]}" 'cloud-init status --wait >/dev/null 2>&1; . /etc/os-release; echo $PRETTY_NAME $(uname -r)')"
"${SSH[@]}" 'DEBIAN_FRONTEND=noninteractive apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends sudo python3 curl git postgresql nftables iproute2 procps >/dev/null && echo prereqs-ok'
tar -C /it -cf - . | "${SSH[@]}" 'mkdir -p /it && tar -C /it -xf - && chmod -R a+rX /it'
rc=0
"${SSH[@]}" 'bash /it/tests/apps_guarded_e2e.sh' || rc=$?
QPID="$(cat "$RUN/qemu.pid" 2>/dev/null || true)"
"${SSH[@]}" 'poweroff' >/dev/null 2>&1 || true
sleep 3
[ -n "$QPID" ] && kill "$QPID" 2>/dev/null || true
rm -rf "$RUN"
exit "$rc"
