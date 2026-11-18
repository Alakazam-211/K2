# ── Zen Mode — k2 zen (prd-zen-mode-v1 Z17; prd-zen-gardens-v1 G35) ──
# `~/.k2/zen/` belongs to THIS computer's K2. Every verb talks to the
# local daemon's /cli/zen/* routes, which answer only this computer's own
# owner token. There is no `grant` verb: widgets in your own Gardens need no
# permissions (Rosson 2026-10-08). The `k2-zen` skill
# documents Gardens, the page grammar, tokens, motion and bridge verbs.

cmd_help_zen() {
    cat <<'EOF'
%include cli/text/help/zen.txt
EOF
}

