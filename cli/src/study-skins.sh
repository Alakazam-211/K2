_study_page_skins() {
    # `#if-cards` / `#if-no-cards` lines: `_perm_lines` (permission.sh).
    cat <<'EOF' | _perm_lines
%include cli/text/study/apps.txt
EOF
}

