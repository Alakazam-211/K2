# ── Phase 2.1 / file-delivery: msg --inbox-silent / --inbox-wake ───────
#
# Bare `--inbox` is a hard error (teaching copy). Tray send modes:
#   k2 msg <ws> --inbox-silent <path>   package only
#   k2 msg <ws> --inbox-wake   <path>   package + short live knock
# Legacy string form (one-release): --title/--body without a path still
# composes via /cli/inbox/compose; --inbox-wake then also deliver_live.

cmd_msg_inbox_bare_error() {
    cat >&2 <<'EOF'
Error: bare --inbox is no longer accepted.

Tray delivery requires an explicit mode:

  k2 msg <workspace> --inbox-wake   <path> [path…]   # package + short live notify (recommended)
  k2 msg <workspace> --inbox-silent <path> [path…]   # package only (does NOT notify)

Multiple paths land as ONE tray package (cover note + sidecars under
.k2/inbox/<id>.files/). Single path keeps the classic single-file shape.

--inbox-silent lands a durable package under the peer's .k2/inbox/ but
does not wake or ping them. Use --inbox-wake for package + knock, or send
a separate live `k2 msg <workspace> "..."` after silent if you want your
own notify.

Files plus a brief (cover note):
  k2 msg <workspace> --inbox-wake ./file.pdf --body "please review"
  k2 msg <workspace> --inbox-wake ./file.pdf "please review"

Legacy string form (no file):
  k2 msg <workspace> --inbox-silent --title "..." [--body "..."]
  k2 msg <workspace> --inbox-wake   --title "..." [--body "..."]
EOF
    exit 2
}

