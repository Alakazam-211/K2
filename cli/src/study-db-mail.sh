_study_page_db_upgrade() {
    cat <<'EOF'
%include cli/text/study/db-upgrade.txt
EOF
}

# 0.46.11 (prd-app-database-integrations-v1 S8): connecting an App to a
# database, and which App kinds reach which databases.
_study_page_app_databases() {
    cat <<'EOF'
%include cli/text/study/app-databases.txt
EOF
}

_study_page_db() {
    cat <<'EOF'
%include cli/text/study/db.txt
EOF
}

_study_page_mail() {
    cat <<'EOF'
%include cli/text/study/mail.txt
EOF
}

_study_page_connect_boundary() {
    cat <<'EOF'
%include cli/text/study/connect-boundary.txt
EOF
}

