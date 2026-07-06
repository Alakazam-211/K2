#!/bin/sh
# K2 production cell guest-init — passt network + workspace mounts + agent + F2.
# BATCH flavor: one-shot run, answer captured and sent via `k2 respond --final`.
#
# W7 de-Claudify: the worker hands us the HOST-RESOLVED agent argv ("$@" =
# program + args). We run THAT, appending the request prompt positionally.
# claude additionally gets its headless batch grammar spliced (-p +
# --session-id, K2_SESSION_ID = the .jsonl/resume key); foreign agents run
# their argv verbatim — any headless grammar they need must already be in the
# resolved preset args. Empty "$@" (old image / old worker) falls back to the
# pre-W7 hardcoded claude line.
export PATH=/usr/local/bin:/usr/local/sbin:/usr/bin:/usr/sbin:/bin:/sbin
export HOME="${K2_HOME_DIR:-/home/k2}"
WS="${K2_WS_DIR:-/home/k2/ai}"
# 1. Network via passt (offers host IP; --noarp accepts it).
for d in /var/lib/dhcpcd /var/db/dhcpcd /run/dhcpcd; do mount -t tmpfs tmpfs "$d" 2>/dev/null; done
ip link set eth0 up 2>/dev/null
dhcpcd -1 -t 18 -C resolv.conf --noarp eth0 >/tmp/dh 2>&1
# 2. F2 hook forwarder.
mkdir -p /run; /usr/local/bin/k2-hook-forwarder >/tmp/f.log 2>&1 &
i=0; while [ ! -S /run/k2-hook.sock ] && [ $i -lt 300 ]; do i=$((i+1)); sleep 0.02; done
# 3. HOME tmpfs FIRST (writable agent state), THEN workspace RO over $HOME/<ws> subdir.
mount -t tmpfs tmpfs "$HOME" 2>/dev/null; mkdir -p "$HOME/.claude" "$WS"
printf '{"hasCompletedOnboarding":true,"theme":"dark"}' > "$HOME/.claude.json"
mount -t virtiofs -o ro k2ws "$WS" 2>/dev/null
[ -n "$K2_MEM_DIR" ] && { mkdir -p "$K2_MEM_DIR"; mount -t virtiofs -o ro k2mem "$K2_MEM_DIR" 2>/dev/null; }
mkdir -p /run/cc-tmp; mount -t tmpfs tmpfs /run/cc-tmp 2>/dev/null
export CLAUDE_CONFIG_DIR="$HOME/.claude" CLAUDE_CODE_TMPDIR=/run/cc-tmp IS_SANDBOX=1
cd "$WS" 2>/dev/null || cd "$HOME"
k2 respond "working in $(pwd)"
SID="${K2_SESSION_ID:-$(cat /proc/sys/kernel/random/uuid)}"
# [K2 API] preamble — EXACT wording coordinated with the host-sessions door.
PREAMBLE="[K2 API] You were invoked through the K2 API. Report progress and results back to the caller by running: k2 respond '<your message>' — and send your final answer with: k2 respond --final '<your answer>'. The caller cannot see your terminal; only k2 respond output reaches them."
PROMPT="$PREAMBLE

${K2_REQUEST_PROMPT:-List the files here and say what this project is.}"
if [ $# -gt 0 ]; then
  case "${1##*/}" in
    claude) set -- "$@" -p --session-id "$SID" "$PROMPT" ;;
    *)      set -- "$@" "$PROMPT" ;;
  esac
  timeout -s KILL 120 "$@" </dev/null >/tmp/ans 2>/tmp/err
else
  # Empty argv (old image / old worker) — the pre-W7 hardcoded claude line.
  timeout -s KILL 120 claude --dangerously-skip-permissions -p --session-id "$SID" "$PROMPT" </dev/null >/tmp/ans 2>/tmp/err
fi
RC=$?; ANS="$(cat /tmp/ans 2>/dev/null)"
[ -n "$ANS" ] && k2 respond --final "$ANS" || k2 respond --final "(rc=$RC err=$(head -c 200 /tmp/err))"
exec sleep 300
