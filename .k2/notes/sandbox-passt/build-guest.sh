#!/usr/bin/env bash
# Reproducible K2 sandbox GUEST-IMAGE build (Debian bookworm + node + AGENT CLIs
# + net tools) — the passt-era recipe, PROVEN 2026-07-01 (claude-only image).
# Run ON THE BOX as a sudo-capable admin user. Produces /opt/k2/guest-base-v2
# (the active guest).
#
# W7 de-Claudify: the image now ships every agent CLI whose install is a plain
# `npm install -g` one-liner alongside claude-code, so a codex/gemini/copilot/pi
# workspace can actually run ITS agent in a cell (the policy resolver + guest-init
# stopped hardcoding claude in the same slice).
#   HONESTY: the multi-agent additions below are VALIDATED ONLY AT NEXT IMAGE
#   BAKE on a Linux box (the Dedicated bootstrap) — no Linux build runs on the
#   dev Mac. `set -euo pipefail` + the verify loop make a bad package name /
#   missing bin fail the bake loudly.
#   DELIBERATELY EXCLUDED (not one-liner npm/pip; no kitchen-sink image):
#     grok, opencode, goose, ollama, cursor-agent  (curl|bash installers / servers)
#     aider, interpreter                            (pip + a large Python dep tree)
#     hermes                                        (no public one-liner install)
#   Cells for those presets will fail at spawn with command-not-found inside the
#   cell — honest, visible, and fixable by a future image rev.
#
# Pairs with:
#   - RUNBOOK-v2.md            (host provisioning + libkrun NET=1 + worker setuid)
#   - k2-guest-init-PRODUCTION.sh   (the PID-1 cell init this installs)
#   - k2-vmm-worker.PASST.rs   (the worker with launch_cell_passt + net_unixstream)
#
# WHY each dep (learned the hard way — see memory project_sandbox_live_e2e):
#   glibc MUST be Debian bookworm (matches libkrunfw kernel) — NEVER hand-copy host libs.
#   curl+python3+openssl : the `k2` CLI is a bash script whose `respond` needs them (else rc=127, F2 dead).
#   iproute2 dhcpcd-base : passt gives DHCP; guest brings up eth0 (dhcpcd --noarp).
#   libstdc++6 libgcc-s1 : node/claude runtime.
#   /run 0777            : the F2 hook-forwarder binds /run/k2-hook.sock as the cell uid.
#   seeded .claude.json  : claude 2.1.195 needs {"hasCompletedOnboarding":true} or it exits rc=2 silently.
set -euo pipefail

GUEST=/opt/k2/guest-base-v2
NODE_VER=v22.11.0
CLAUDE_VER=2.1.195
# W7 npm-installable agent CLIs (unpinned — latest at bake; pin after the first
# validated bake if reproducibility bites). Package names cross-checked against
# the upstream registries, NOT the Settings install table
# (src/renderer/components/Settings/sections/EditorsAgentsSection.tsx), which
# lists gemini/copilot under an @anthropic-ai/ scope that does not exist:
#   codex   -> @openai/codex                     (bin: codex)
#   gemini  -> @google/gemini-cli                (bin: gemini)
#   copilot -> @github/copilot                   (bin: copilot)
#   pi      -> @mariozechner/pi-coding-agent     (bin: pi; name per that table)
EXTRA_AGENT_PKGS="@openai/codex @google/gemini-cli @github/copilot @mariozechner/pi-coding-agent"
# K2 binaries to overlay (adjust to your build outputs):
K2_CLI="${K2_CLI:-/opt/k2/cli/k2}"                        # the CLI bash script (repo cli/k2)
K2_HOOK_FWD="${K2_HOOK_FWD:-/opt/k2/target/debug/k2-hook-forwarder}"  # compiled forwarder
GUEST_INIT="${GUEST_INIT:-$(dirname "$0")/k2-guest-init-PRODUCTION.sh}"

echo "== 1. build base (debian:12 + deps + node + agent CLIs) via podman =="
WORK=$(mktemp -d)
cat > "$WORK/Containerfile" <<DOCKER
FROM debian:12
RUN apt-get update && apt-get install -y --no-install-recommends \
    curl python3 openssl ca-certificates libstdc++6 libgcc-s1 \
    procps coreutils util-linux e2fsprogs rsync bash dash xz-utils git ripgrep \
    iproute2 dhcpcd-base \
 && rm -rf /var/lib/apt/lists/*
RUN curl -fsSL https://nodejs.org/dist/${NODE_VER}/node-${NODE_VER}-linux-x64.tar.xz | tar -xJ -C /usr/local --strip-components=1
RUN npm install -g @anthropic-ai/claude-code@${CLAUDE_VER}
# W7: the npm-one-liner agents (see EXTRA_AGENT_PKGS above for why these four).
RUN npm install -g ${EXTRA_AGENT_PKGS}
RUN printf '{"hasCompletedOnboarding":true,"theme":"dark"}' > /root/.claude.json \
 && mkdir -p /work /home/k2 && chmod 0777 /work /home/k2 /run
DOCKER
podman build -t k2guest:build "$WORK"

echo "== 2. export to rootfs =="
CID=$(podman create k2guest:build true)
rm -rf "$GUEST"; mkdir -p "$GUEST"
podman export "$CID" | tar -C "$GUEST" -xf -
podman rm "$CID" >/dev/null

echo "== 3. overlay K2 binaries + production guest-init =="
install -m 0755 "$K2_CLI"      "$GUEST/usr/local/bin/k2"
install -m 0755 "$K2_HOOK_FWD" "$GUEST/usr/local/bin/k2-hook-forwarder"
install -m 0755 "$GUEST_INIT"  "$GUEST/usr/local/bin/k2-guest-init"

echo "== 4. verify =="
# codex/gemini/copilot/pi joined the loop in W7 — a bake that silently dropped
# one of them must fail HERE, not at first non-claude cell spawn.
for t in curl python3 node claude codex gemini copilot pi ip dhcpcd bash; do
  printf '%s: ' "$t"; chroot "$GUEST" which "$t" 2>/dev/null || { echo MISSING; exit 1; }
done
chroot "$GUEST" /usr/local/bin/node --version
grep -q 'dhcpcd.*--noarp' "$GUEST/usr/local/bin/k2-guest-init" && echo "guest-init: production (passt) ✓"
# W7 guest-init must honor argv (exec the host-resolved agent), not hardcode claude.
grep -q 'K2 API' "$GUEST/usr/local/bin/k2-guest-init" && echo "guest-init: W7 argv+preamble ✓"
rm -rf "$WORK"
echo "== DONE — active guest at $GUEST. Restart k2-daemon.service to use it. =="
