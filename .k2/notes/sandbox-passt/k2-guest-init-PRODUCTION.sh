#!/bin/sh
# K2 cell guest-init — INTERACTIVE agent TUI (visible in the tab) + UDS (F2)
# responses at the agent's discretion + mirror-paths workspace + passt net + persist.
#
# W7 de-Claudify: the worker hands us the HOST-RESOLVED agent argv ("$@" =
# program + args, e.g. `claude --dangerously-skip-permissions` or
# `gemini --yolo`). We run THAT, appending the request prompt positionally.
# claude additionally gets the forced session identity spliced (K2_SESSION_ID
# is the .jsonl/resume key the daemon addresses the session by). Empty "$@"
# (old image / old worker) falls back to the pre-W7 hardcoded claude line.
export PATH=/usr/local/bin:/usr/local/sbin:/usr/bin:/usr/sbin:/bin:/sbin
export HOME="${K2_HOME_DIR:-/home/k2}"; WS="${K2_WS_DIR:-/home/k2/ai}"
for d in /var/lib/dhcpcd /var/db/dhcpcd /run/dhcpcd; do mount -t tmpfs tmpfs "$d" 2>/dev/null; done
ip link set eth0 up 2>/dev/null; dhcpcd -1 -t 18 -C resolv.conf --noarp eth0 >/tmp/dh 2>&1
mkdir -p /run; /usr/local/bin/k2-hook-forwarder >/tmp/f.log 2>&1 &
i=0; while [ ! -S /run/k2-hook.sock ] && [ $i -lt 300 ]; do i=$((i+1)); sleep 0.02; done
mount -t tmpfs tmpfs "$HOME" 2>/dev/null; mkdir -p "$HOME/.claude" "$HOME/persist" "$WS"
n=0; while [ $n -lt 100 ]; do mount -t virtiofs k2home "$HOME/persist" 2>/dev/null && break; n=$((n+1)); sleep 0.05; done
rsync -rltD --no-owner --no-group "$HOME/persist/" "$HOME/.claude/" 2>/dev/null
[ -f "$HOME/.claude.json" ] || printf '{"hasCompletedOnboarding":true,"theme":"dark"}' > "$HOME/.claude.json"
mount -t virtiofs -o ro k2ws "$WS" 2>/dev/null
[ -n "$K2_MEM_DIR" ] && { mkdir -p "$K2_MEM_DIR"; mount -t virtiofs -o ro k2mem "$K2_MEM_DIR" 2>/dev/null; }
mkdir -p /run/cc-tmp; mount -t tmpfs tmpfs /run/cc-tmp 2>/dev/null
export CLAUDE_CONFIG_DIR="$HOME/.claude" CLAUDE_CODE_TMPDIR=/run/cc-tmp IS_SANDBOX=1
cd "$WS" 2>/dev/null || cd "$HOME"
k2 respond "sandbox ready in $(pwd) — launching ${1:-claude}"
SID="${K2_SESSION_ID:-$(cat /proc/sys/kernel/random/uuid)}"
# [K2 API] preamble — EXACT wording coordinated with the host-sessions door.
PREAMBLE="[K2 API] You were invoked through the K2 API. Report progress and results back to the caller by running: k2 respond '<your message>' — and send your final answer with: k2 respond --final '<your answer>'. The caller cannot see your terminal; only k2 respond output reaches them."
PROMPT="$PREAMBLE

${K2_REQUEST_PROMPT:-Hi! You are running live in a K2 sandbox. Explore this workspace.}"
# INTERACTIVE agent TUI in the cell PTY (the cockpit tab renders it live).
# The agent decides when/how to reply over the UDS via `k2 respond`.
if [ $# -gt 0 ]; then
  # Host-resolved argv. claude: splice the forced session identity (resume or
  # premint) — the same plumbing as the fallback, gated on the program
  # basename. Other agents: run their argv verbatim (auto-approve flags are
  # already in the resolved args); append the prompt positionally only when
  # the caller actually sent one (no invented default prompt for foreign CLIs).
  case "${1##*/}" in
    claude)
      if [ "${K2_RESUME:-0}" = "1" ]; then set -- "$@" --resume "$SID"; else set -- "$@" --session-id "$SID" "$PROMPT"; fi ;;
    *)
      [ -n "${K2_REQUEST_PROMPT:-}" ] && set -- "$@" "$PROMPT" ;;
  esac
  "$@"
else
  # Empty argv (old image / old worker) — the pre-W7 hardcoded claude line.
  if [ "${K2_RESUME:-0}" = "1" ]; then
    claude --dangerously-skip-permissions --resume "$SID"
  else
    claude --dangerously-skip-permissions --session-id "$SID" "$PROMPT"
  fi
fi
# agent TUI exited → persist state, keep cell observable.
rsync -rltD --no-owner --no-group "$HOME/.claude/" "$HOME/persist/" 2>/dev/null
k2 respond --final "session ended"
exec sleep 86400
