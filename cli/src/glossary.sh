# ── Phase 2.1: Glossary (A10) ─────────────────────────────────────────

cmd_glossary() {
    local term="${1:-}"
    if [[ -z "$term" ]]; then
        # List all terms.
        echo "K2 glossary — definitions of K2-specific terms"
        echo ""
        cli_request "/cli/glossary/list" | python3 -c "
import json, sys
try:
    entries = json.loads(sys.stdin.read() or '[]')
except Exception:
    entries = []
if not entries:
    print('  (no terms returned)')
    sys.exit(0)
for e in entries:
    term = e.get('term', '?')
    summary = e.get('summary', '')
    print('  %-16s %s' % (term, summary))
print()
print('Run \`k2 glossary <term>\` for a full definition.')
" 2>/dev/null
        return
    fi
    cli_request "/cli/glossary/get" "term=$(urlencode "$term")" | python3 -c "
import json, sys
try:
    d = json.loads(sys.stdin.read())
except Exception as e:
    sys.stderr.write('parse: %s\n' % e)
    sys.exit(1)
if 'error' in d:
    sys.stderr.write('Error: %s\n' % d['error'])
    sys.exit(1)
term = d.get('term', '?')
definition = d.get('definition', '')
print('%-16s %s' % (term, definition))
" 2>/dev/null
}

