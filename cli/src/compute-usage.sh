# ── k2 compute — run commands on your compute nodes ───────────────────
# (prd-k2-compute-nodes-v1 §15.2). `compute` is an `id` tool and locked:
# inside a K2 terminal the request carries this session's passport, so an
# agent only ever sees its own workspace and needs "Let this agent use compute nodes"
# plus a grant. Owner verbs (node add/confirm/remove, grant, local pause…)
# refuse a passport with `owner_only`: run them outside K2.
# Agent verbs ride the per-cell socket when present; owner verbs go TCP.

_compute_usage() {
    cat <<'EOF'
%include cli/text/help/compute.txt
EOF
}

