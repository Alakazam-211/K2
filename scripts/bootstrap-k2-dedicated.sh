#!/usr/bin/env bash
# bootstrap-k2-dedicated.sh — turn a fresh Ubuntu 24.04 BARE-METAL box into a
# K2 Cloud Dedicated-tier server: the Standard provision (provision-k2-server.sh)
# PLUS the libkrun microVM sandbox stack, from prebuilt hosted artifacts
# (package-dedicated-artifacts.sh output — nothing is compiled on the box).
#
# PRD: .k2/prds/prd-k2-cloud-hosted-servers-v1.md §4.1 Dedicated.
# Runbooks transcribed: .k2/notes/runbook-self-host-sandbox-server.md,
# .k2/notes/p2b-onbox-bootstrap.md, .k2/notes/sandbox-passt/RUNBOOK-v2.md.
#
# ── LICENSE CONSENT GATE (Rosson, GPLv2 opt-in) ──────────────────────
# libkrunfw BUNDLES A LINUX KERNEL (GPLv2). Installing the sandbox stack is
# therefore an EXPLICIT operator opt-in: this script REFUSES to install it
# unless K2_SANDBOX_CONSENT=1 is set in the environment. Without consent it
# provisions the Standard tier only (sandboxes OFF) and prints why.
#
# Usage (as root on fresh Ubuntu 24.04 bare metal, post-installimage):
#
#   K2_SANDBOX_CONSENT=1 \
#   K2_TUNNEL_TOKEN=k2c_... K2_SUBDOMAIN=alice K2_OWNER_USER=alice \
#   ./bootstrap-k2-dedicated.sh
#
# Environment contract — Dedicated-specific:
#   K2_SANDBOX_CONSENT     MUST be 1 to install the sandbox stack (GPLv2
#                          libkrunfw opt-in). Anything else → Standard-only.
#   K2_ARTIFACTS_VERSION   dedicated artifact bundle version   (default: v1)
#   K2_ARTIFACTS_BASE_URL  where the bundle assets live (default:
#                          https://github.com/<repo>/releases/download/dedicated-artifacts-<ver>)
#   K2_GH_REPO             repo for the default base URL       (default: Alakazam-211/K2)
#   K2_GUEST_BASE          guest rootfs install path           (default: /opt/k2/guest-base-v2)
#   K2_CELL_USER           legacy shared cell drop-user        (default: k2cell)
#   K2_SPAWN_PROBE         1 = if boot-status lacks api.sandboxes (pre-0.40.30
#                          daemon), acceptance falls back to a REAL spawn
#                          probe (temp API key → POST /v1/sandboxes) (default: 1)
#
# Environment contract — passed through to provision-k2-server.sh (Standard):
#   K2_TUNNEL_TOKEN K2_SUBDOMAIN K2_OWNER_USER K2_OWNER_PASSWORD K2_OPS_USER
#   K2_CALLBACK_URL K2_CALLBACK_TOKEN K2_VERSION
#   K2_RUN_USER            daemon service account              (default: k2)
#   K2_UNIT_NAME           systemd unit name                   (default: k2-daemon)
#
# Layout facts this script encodes (from the validated reference box):
#   - worker MUST live NEXT TO the daemon binary (k2-core sandbox.rs
#     resolve_worker_path = current_exe sibling) → <k2 home>/.local/bin/.
#   - worker MUST be setuid-root (chown root:root && chmod u+s) — it builds
#     the jail then drops to the per-session uid.
#   - daemon needs ONLY CAP_CHOWN + CAP_NET_ADMIN (AmbientCapabilities),
#     NoNewPrivileges=false (else the setuid worker can't elevate), and
#     SupplementaryGroups=kvm.
#   - nft needs `setcap cap_net_admin+ei` (caps don't cross exec to a plain
#     binary; without this every spawn fail-closes on the egress allowlist).
#   - libkrun/libkrunfw land in /usr/local/lib64 (RPATH pre-baked) +
#     ld.so.conf.d entry + ldconfig.
#
# Idempotent: re-running converges. Artifact installs are stamped by sha and
# skipped when current; users/groups/caps/unit rewrites are guarded or safely
# re-applied; the daemon is restarted at most once per run.
set -euo pipefail

K2_RUN_USER="${K2_RUN_USER:-k2}"
K2_UNIT_NAME="${K2_UNIT_NAME:-k2-daemon}"
K2_CELL_USER="${K2_CELL_USER:-k2cell}"
K2_GUEST_BASE="${K2_GUEST_BASE:-/opt/k2/guest-base-v2}"
K2_ARTIFACTS_VERSION="${K2_ARTIFACTS_VERSION:-v1}"
K2_GH_REPO="${K2_GH_REPO:-Alakazam-211/K2}"
K2_ARTIFACTS_BASE_URL="${K2_ARTIFACTS_BASE_URL:-https://github.com/${K2_GH_REPO}/releases/download/dedicated-artifacts-${K2_ARTIFACTS_VERSION}}"
K2_SPAWN_PROBE="${K2_SPAWN_PROBE:-1}"
RAW_BASE="https://raw.githubusercontent.com/Alakazam-211/K2/main"

log() { printf '\033[1;35m[dedicated]\033[0m %s\n' "$*"; }
die() { printf '\033[1;31m[dedicated] ERROR:\033[0m %s\n' "$*" >&2; exit 1; }

[ "$(id -u)" = "0" ] || die "run as root"
case "$(uname -m)" in
	x86_64|amd64) ARCH="amd64" ;;
	*) die "Dedicated artifacts are amd64-only today (got $(uname -m))" ;;
esac

K2_HOME="/home/${K2_RUN_USER}"
BIN_DIR="${K2_HOME}/.local/bin"        # provision-k2-server.sh installs k2-daemon here
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

VER="$K2_ARTIFACTS_VERSION"
LIBS_TAR="k2-libkrun-libs-${VER}-${ARCH}.tar.zst"
GUEST_TAR="k2-guest-base-${VER}-${ARCH}.tar.zst"
WORKER_TAR="k2-vmm-worker-${VER}-${ARCH}.tar.zst"

run_standard_provision() {
	local prov="$SCRIPT_DIR/provision-k2-server.sh"
	if [ ! -f "$prov" ]; then
		prov="/tmp/k2-provision-k2-server.sh"
		log "fetching provision-k2-server.sh from raw main"
		curl -fsSL "$RAW_BASE/scripts/provision-k2-server.sh" -o "$prov"
		chmod +x "$prov"
	fi
	log "running Standard provision ($prov)"
	# Inherits the K2_* provisioning env (tunnel/owner/callback/version/user).
	bash "$prov"
}

# ── CONSENT GATE — the very first decision ────────────────────────────
if [ "${K2_SANDBOX_CONSENT:-}" != "1" ]; then
	cat <<'EOF'
================================================================================
 SANDBOX STACK NOT INSTALLED — CONSENT REQUIRED
================================================================================
 The K2 Dedicated sandbox stack includes libkrunfw, which BUNDLES A LINUX
 KERNEL licensed under the GPLv2. Installing GPLv2-licensed binaries on this
 machine is an explicit operator decision, so this bootstrap refuses to do it
 implicitly.

 To opt in, re-run with:   K2_SANDBOX_CONSENT=1 ./bootstrap-k2-dedicated.sh

 Proceeding with the STANDARD provision only (sandboxes OFF).
================================================================================
EOF
	run_standard_provision
	log "DONE — Standard-only (no sandbox consent). Re-run with K2_SANDBOX_CONSENT=1 to add the sandbox stack."
	exit 0
fi

# ── 0. hard gates: bare metal w/ KVM ─────────────────────────────────
log "checking /dev/kvm (hard gate — Dedicated requires bare metal / nested virt)"
if [ ! -e /dev/kvm ]; then
	modprobe kvm_amd 2>/dev/null || modprobe kvm_intel 2>/dev/null || true
fi
[ -c /dev/kvm ] || die "/dev/kvm missing — wrong box (Hetzner Cloud cannot serve Dedicated; check BIOS SVM/VT-x)"

# ── 1. Standard base first (daemon, frpc, CLI, tunnel, owner, unit) ──
run_standard_provision
[ -x "$BIN_DIR/k2-daemon" ] || die "Standard provision did not leave a daemon at $BIN_DIR/k2-daemon"

# ── 2. host packages the sandbox stack needs ─────────────────────────
#   nftables : per-cell fail-closed egress allowlist (`nft` subprocess)
#   passt    : per-cell userspace virtio-net proxy (worker launch_cell_passt)
#   zstd     : artifact tarballs
#   e2fsprogs: per-session ext4 writable images (RUNBOOK-v2 §5)
log "installing sandbox host packages (nftables passt zstd e2fsprogs)"
export DEBIAN_FRONTEND=noninteractive
apt-get install -y -qq nftables passt zstd e2fsprogs >/dev/null

# ── 3. fetch + verify the dedicated artifact bundle ──────────────────
DL="/var/cache/k2-dedicated-artifacts/${VER}"
mkdir -p "$DL"
log "fetching artifact manifest + SHA256SUMS from $K2_ARTIFACTS_BASE_URL"
curl -fsSL "$K2_ARTIFACTS_BASE_URL/SHA256SUMS" -o "$DL/SHA256SUMS"
curl -fsSL "$K2_ARTIFACTS_BASE_URL/manifest.json" -o "$DL/manifest.json"
(cd "$DL" && grep ' manifest.json$' SHA256SUMS | sha256sum -c --quiet) \
	|| die "manifest.json failed sha256 verification"

expected_sha() { # expected_sha <asset-name> — from SHA256SUMS, fail-closed
	local sha
	sha=$(awk -v f="$1" '$2==f {print $1}' "$DL/SHA256SUMS")
	[ -n "$sha" ] || die "SHA256SUMS has no entry for $1"
	printf '%s' "$sha"
}

fetch_asset() { # fetch_asset <asset-name> — download unless cached+valid, then verify
	local f="$1" want
	want=$(expected_sha "$f")
	if [ -f "$DL/$f" ] && [ "$(sha256sum "$DL/$f" | cut -d' ' -f1)" = "$want" ]; then
		log "  $f — cached, sha verified"
		return 0
	fi
	log "  downloading $f"
	curl -fL --retry 3 -o "$DL/$f.part" "$K2_ARTIFACTS_BASE_URL/$f"
	[ "$(sha256sum "$DL/$f.part" | cut -d' ' -f1)" = "$want" ] \
		|| die "$f failed sha256 verification (fail-closed — refusing to install)"
	mv "$DL/$f.part" "$DL/$f"
}

# ── 4. libkrun + libkrunfw → /usr/local/lib64 + ldconfig ─────────────
LIB_DIR="/usr/local/lib64"
LIBS_STAMP="$LIB_DIR/.k2-dedicated-libs.sha"
LIBS_WANT=$(expected_sha "$LIBS_TAR")
if [ -f "$LIBS_STAMP" ] && [ "$(cat "$LIBS_STAMP")" = "$LIBS_WANT" ]; then
	log "libkrun/libkrunfw already installed (stamp matches) — skipping"
else
	fetch_asset "$LIBS_TAR"
	log "installing libkrun/libkrunfw into $LIB_DIR"
	mkdir -p "$LIB_DIR"
	zstd -dcq "$DL/$LIBS_TAR" | tar --numeric-owner -xf - -C "$LIB_DIR"
	printf '%s' "$LIBS_WANT" > "$LIBS_STAMP"
fi
# ld.so.conf entry — /usr/local/lib64 is NOT on Ubuntu's default search path.
# (The libs also carry RPATH=/usr/local/lib64; this is belt-and-braces and
# makes `ldconfig -p` a truthful acceptance check.)
if [ ! -f /etc/ld.so.conf.d/k2-libkrun.conf ]; then
	echo "$LIB_DIR" > /etc/ld.so.conf.d/k2-libkrun.conf
fi
ldconfig

# ── 5. guest base image → K2_GUEST_BASE (atomic swap) ────────────────
GUEST_STAMP="${K2_GUEST_BASE}/.k2-artifact.sha"
GUEST_WANT=$(expected_sha "$GUEST_TAR")
if [ -f "$GUEST_STAMP" ] && [ "$(cat "$GUEST_STAMP")" = "$GUEST_WANT" ]; then
	log "guest base already installed (stamp matches) — skipping"
else
	fetch_asset "$GUEST_TAR"
	log "installing guest base at $K2_GUEST_BASE (atomic swap)"
	rm -rf "${K2_GUEST_BASE}.new"
	mkdir -p "${K2_GUEST_BASE}.new"
	zstd -dcq "$DL/$GUEST_TAR" | tar --numeric-owner -xpf - -C "${K2_GUEST_BASE}.new"
	[ -x "${K2_GUEST_BASE}.new/usr/local/bin/k2-guest-init" ] \
		|| die "extracted guest base has no k2-guest-init — corrupt artifact?"
	printf '%s' "$GUEST_WANT" > "${K2_GUEST_BASE}.new/.k2-artifact.sha"
	rm -rf "${K2_GUEST_BASE}.old"
	[ -d "$K2_GUEST_BASE" ] && mv "$K2_GUEST_BASE" "${K2_GUEST_BASE}.old"
	mv "${K2_GUEST_BASE}.new" "$K2_GUEST_BASE"
	rm -rf "${K2_GUEST_BASE}.old"
fi

# ── 6. k2-vmm-worker → NEXT TO the daemon binary, setuid-root ────────
WORKER_DST="$BIN_DIR/k2-vmm-worker"
WORKER_STAMP="$BIN_DIR/.k2-vmm-worker.sha"
WORKER_WANT=$(expected_sha "$WORKER_TAR")
if [ -f "$WORKER_STAMP" ] && [ "$(cat "$WORKER_STAMP")" = "$WORKER_WANT" ] && [ -f "$WORKER_DST" ]; then
	log "worker already installed (stamp matches)"
else
	fetch_asset "$WORKER_TAR"
	log "installing k2-vmm-worker at $WORKER_DST"
	zstd -dcq "$DL/$WORKER_TAR" | tar -xf - -C "$BIN_DIR" k2-vmm-worker
	printf '%s' "$WORKER_WANT" > "$WORKER_STAMP"
fi
# ALWAYS re-apply the privilege bits (anything that replaces the file strips them):
chown root:root "$WORKER_DST"
chmod u+s,go-w "$WORKER_DST"   # -rwsr-xr-x

# ── 7. nft file-capability (caps don't cross exec to a plain binary) ─
log "setcap cap_net_admin+ei /usr/sbin/nft"
setcap cap_net_admin+ei /usr/sbin/nft

# ── 8. cell drop-user + kvm group membership ─────────────────────────
getent group kvm >/dev/null || groupadd -r kvm
if ! id "$K2_CELL_USER" >/dev/null 2>&1; then
	log "creating cell drop-user ${K2_CELL_USER}"
	useradd --system --no-create-home --shell /usr/sbin/nologin "$K2_CELL_USER"
fi
usermod -aG kvm "$K2_CELL_USER"    # /dev/kvm is root:kvm 0660 — the dropped cell uid keeps kvm
usermod -aG kvm "$K2_RUN_USER"     # daemon side (also granted via SupplementaryGroups below)

# ── 9. rewrite the systemd unit WITH the sandbox environment ─────────
UNIT_FILE="/etc/systemd/system/${K2_UNIT_NAME}.service"
log "writing $UNIT_FILE (sandboxes ON)"
cat > "$UNIT_FILE" <<EOF
[Unit]
Description=K2 daemon (Dedicated — libkrun sandbox host)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=${K2_RUN_USER}
Group=${K2_RUN_USER}
SupplementaryGroups=kvm
Environment=HOME=${K2_HOME}
Environment=PATH=/usr/local/bin:/usr/bin:/bin:${BIN_DIR}
# Standard /v1 surface + the sandbox families (all default-OFF in the binary;
# a Dedicated box is exactly these flags):
Environment=K2_API=1
Environment=K2_SANDBOX=1
Environment=K2_SANDBOX_API=1
Environment=K2_HOOK_SCOPED=1
Environment=K2_SANDBOX_GUEST_BASE=${K2_GUEST_BASE}
Environment=RUST_LOG=info
# The daemon's spawn door does two privileged per-session ops: chown the
# ephemeral workspace to the cell uid (CAP_CHOWN, in-process) and install the
# fail-closed nft egress allowlist (CAP_NET_ADMIN via the nft subprocess —
# which needs the matching file-cap, applied by this bootstrap). The libkrun
# jail itself is built by the setuid-root k2-vmm-worker, which drops to the
# per-session uid — the daemon never holds full root.
AmbientCapabilities=CAP_CHOWN CAP_NET_ADMIN
# Must stay false so the setuid-root k2-vmm-worker can elevate to build the jail:
NoNewPrivileges=false
ExecStart=${BIN_DIR}/k2-daemon
Restart=always
RestartSec=2
# Shape B self-update spawns a detached swap helper that must OUTLIVE the
# daemon across the restart; control-group kill would murder it first.
KillMode=process

[Install]
WantedBy=multi-user.target
EOF
systemctl daemon-reload
systemctl enable "$K2_UNIT_NAME" >/dev/null 2>&1
log "restarting ${K2_UNIT_NAME} with the sandbox environment"
systemctl restart "$K2_UNIT_NAME"

# ── 10. wait for readiness (port+token rotate on every boot) ─────────
log "waiting for daemon readiness"
PORT=""; TOKEN=""; BASE=""
for _ in $(seq 1 90); do
	PORT=$(cat "$K2_HOME/.k2/daemon.port" 2>/dev/null || true)
	TOKEN=$(cat "$K2_HOME/.k2/daemon.token" 2>/dev/null || true)
	if [ -n "$PORT" ] && [ -n "$TOKEN" ]; then
		BASE="http://127.0.0.1:${PORT}"
		if curl -fsS "$BASE/boot-status" 2>/dev/null | grep -q '"phase":"ready"'; then
			AUTH_CODE=$(curl -s -o /dev/null -w '%{http_code}' "$BASE/cli/auth/whoami?token=$TOKEN")
			[ "$AUTH_CODE" = "200" ] && break
		fi
	fi
	sleep 1
	PORT=""
done
[ -n "$PORT" ] || die "daemon not ready with a live token in 90s (journalctl -u ${K2_UNIT_NAME})"

# ── 11. ACCEPTANCE — every check prints PASS/FAIL; any FAIL exits 1 ──
# NB: every probe is `OK=0; <test> || OK=1` — a bare failing test under
# `set -e` would abort the script instead of recording the FAIL.
FAILS=0
check() { # check <label> <0|1 ok> [hint]
	if [ "$2" = "0" ]; then
		printf '  \033[1;32mPASS\033[0m  %s\n' "$1"
	else
		printf '  \033[1;31mFAIL\033[0m  %s%s\n' "$1" "${3:+ — $3}"
		FAILS=$((FAILS + 1))
	fi
}

log "ACCEPTANCE"

OK=0; [ -c /dev/kvm ] || OK=1
check "/dev/kvm present (char device)" "$OK"

OK=0; { ldconfig -p | grep -q libkrun.so && ldconfig -p | grep -q libkrunfw.so; } || OK=1
check "libkrun + libkrunfw visible to ldconfig" "$OK"

OK=0; { [ -u "$WORKER_DST" ] && [ "$(stat -c '%U' "$WORKER_DST")" = "root" ]; } || OK=1
check "k2-vmm-worker setuid-root next to the daemon ($WORKER_DST)" "$OK"

OK=0; getcap /usr/sbin/nft 2>/dev/null | grep -q "cap_net_admin.*ei" || OK=1
check "nft carries cap_net_admin+ei" "$OK"

OK=0; id -nG "$K2_CELL_USER" 2>/dev/null | grep -qw kvm || OK=1
check "${K2_CELL_USER} exists and is in the kvm group" "$OK"

OK=0; { [ -x "$K2_GUEST_BASE/usr/local/bin/k2-guest-init" ] \
	&& [ -f "$K2_GUEST_BASE/root/.claude.json" ]; } || OK=1
check "guest base sane at $K2_GUEST_BASE (guest-init + seeded .claude.json)" "$OK"

OK=0; [ "$(systemctl is-active "$K2_UNIT_NAME" || true)" = "active" ] || OK=1
check "daemon service active" "$OK"

# Daemon actually holds CAP_CHOWN(bit 0) + CAP_NET_ADMIN(bit 12) → CapInh & 0x1001.
DPID=$(systemctl show -p MainPID --value "$K2_UNIT_NAME" 2>/dev/null || true)
CAPS_OK=1
if [ -n "$DPID" ] && [ "$DPID" != "0" ] && [ -r "/proc/$DPID/status" ]; then
	if python3 - "$DPID" <<'PYEOF'
import sys
pid = sys.argv[1]
for line in open(f"/proc/{pid}/status"):
    if line.startswith("CapInh:"):
        v = int(line.split()[1], 16)
        sys.exit(0 if (v & 0x1001) == 0x1001 else 1)
sys.exit(1)
PYEOF
	then CAPS_OK=0; fi
fi
check "daemon CapInh includes CAP_CHOWN+CAP_NET_ADMIN" "$CAPS_OK"

# Sandbox capability — primary: boot-status api.sandboxes (0.40.30+);
# fallback: a REAL spawn probe through the public API.
BOOT=$(curl -fsS "$BASE/boot-status" 2>/dev/null || echo '{}')
SBX=$(printf '%s' "$BOOT" | python3 -c \
	'import json,sys
v=json.load(sys.stdin)
api=v.get("api") or {}
print(api.get("sandboxes","ABSENT"))' 2>/dev/null || echo "ABSENT")
case "$SBX" in
	microvm)
		check "boot-status api.sandboxes == microvm" 0 ;;
	none)
		check "boot-status api.sandboxes == microvm" 1 \
			"daemon reports 'none' — this daemon build lacks the sandbox-microvm feature (needs a sandbox-enabled Linux release, 0.40.30+)" ;;
	*)
		if [ "$K2_SPAWN_PROBE" = "1" ]; then
			log "boot-status has no api.sandboxes (pre-0.40.30 daemon) — running spawn probe"
			PROBE_OK=1
			CREATE=$(curl -sS -X POST "$BASE/cli/api-keys/create?token=$TOKEN" \
				-H 'Content-Type: application/json' \
				-d '{"label":"k2-dedicated-bootstrap-probe"}' 2>/dev/null || echo '{}')
			PKEY=$(printf '%s' "$CREATE" | python3 -c 'import json,sys;print(json.load(sys.stdin).get("key",""))' 2>/dev/null || true)
			PID_KEY=$(printf '%s' "$CREATE" | python3 -c 'import json,sys;print(json.load(sys.stdin).get("id",""))' 2>/dev/null || true)
			if [ -n "$PKEY" ]; then
				PRES=$(mktemp)
				PCODE=$(curl -sS -o "$PRES" -w '%{http_code}' -X POST "$BASE/v1/sandboxes" \
					-H "Authorization: Bearer $PKEY" -H 'Content-Type: application/json' \
					-d '{"prompt":"k2 dedicated bootstrap acceptance probe","timeout_secs":60}' \
					|| echo 000)
				if [ "$PCODE" = "200" ] && grep -q '"sessionId"' "$PRES"; then
					PROBE_OK=0
				fi
				rm -f "$PRES"
				# revoke the probe key (best-effort)
				[ -n "$PID_KEY" ] && curl -sS -X POST "$BASE/cli/api-keys/revoke?token=$TOKEN" \
					-H 'Content-Type: application/json' -d "{\"id\":\"$PID_KEY\"}" >/dev/null 2>&1 || true
			fi
			check "spawn probe: POST /v1/sandboxes spawned a real microVM cell" "$PROBE_OK" \
				"got HTTP ${PCODE:-n/a} (409 = daemon fail-closed: sandbox stack or feature unavailable)"
		else
			check "sandbox capability (api.sandboxes absent, probe disabled)" 1 \
				"set K2_SPAWN_PROBE=1 or update the daemon to 0.40.30+"
		fi ;;
esac

# ── 12. summary ───────────────────────────────────────────────────────
echo
if [ "$FAILS" = "0" ]; then
	log "DEDICATED BOOTSTRAP: PASS — all acceptance checks green"
else
	log "DEDICATED BOOTSTRAP: FAIL — $FAILS check(s) failed (see above)"
fi
echo "  daemon:     $(systemctl is-active "$K2_UNIT_NAME") on 127.0.0.1:${PORT} (unit ${K2_UNIT_NAME})"
[ -n "${K2_SUBDOMAIN:-}" ] && echo "  address:    https://${K2_SUBDOMAIN}.k2.dev"
echo "  sandboxes:  $([ "$FAILS" = "0" ] && echo "ON (microvm)" || echo "NOT VERIFIED")"
echo "  artifacts:  ${K2_ARTIFACTS_BASE_URL}"
echo "  guest base: ${K2_GUEST_BASE}"
echo "  worker:     ${WORKER_DST} (setuid-root — re-run this script after any daemon update)"
[ "$FAILS" = "0" ] || exit 1
