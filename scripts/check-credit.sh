#!/bin/sh
# Fail if Board's credit is missing anywhere it's owed. Parts of this crate are ported from Board
# (MIT), and its notice has to travel with them:
#  - every ported source file carries "Copyright (c) 2026 Baden Ballard, MIT" in its header, the
#    comment block it starts with;
#  - every file generated from ported data carries it;
#  - THIRD-PARTY-NOTICES.md holds the full MIT notice, and NOTICE.md points to it;
#  - a source file that names Board as a source, by a phrase such as "ported from Board" or
#    "derived from Board", is on the list below, so a new port can't land with a partial credit.
#
#   scripts/check-credit.sh [ROOT]       (default: the repo root)
#
# Exit 0 clean, 1 findings (each printed as FAILED <path>: <why>), 2 usage.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=${1:-$(cd "$HERE/.." && pwd)}
[ -d "$ROOT" ] || { echo "check-credit: no directory $ROOT" >&2; exit 2; }
exec python3 -I - "$ROOT" <<'PY'
import os, re, subprocess, sys
root = sys.argv[1]
CREDIT = 'Copyright (c) 2026 Baden Ballard, MIT'
PORTED = [
    'crates/board/src/schema.rs',
    'crates/board/src/verbs/templates.rs',
    'crates/board/src/verbs/edit.rs',
    'crates/board/src/verbs/arrange.rs',
    'crates/board/src/verbs/clone.rs',
    'crates/board/src/parity.rs',
    'crates/board/tests/support/cases.rs',
]
GENERATED = [
    'docs/FORMAT.md',
    'schema/board-schema.json',
    'docs/PARITY.md',
    'vectors/widget/README.md',
]
MIT = [
    'Copyright (c) 2026 Baden Ballard',
    'Permission is hereby granted, free of charge, to any person obtaining a copy',
    'The above copyright notice and this permission notice shall be included in all',
    'THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR',
]
# A source names Board as where it came from. Matched on comment text joined across lines.
MARKER = re.compile(r'\b(ports?|ported|derived?|derives|comes?|taken|adapted|copied)\s+from\s+Board\b', re.I)
SOURCES = ('.rs', '.mjs', '.js', '.py', '.sh')
SKIP = {'scripts/check-credit.sh', 'scripts/test-credit.sh', 'tools/sweep.py'}
COMMENT = re.compile(r'^\s*(//[/!]?|#)\s?')

def read(path):
    try:
        return open(os.path.join(root, path), encoding='utf8').read()
    except OSError:
        return None

def flat(text):
    return ' '.join(COMMENT.sub('', l).strip() for l in text.split('\n'))

def header(text):
    lines = []
    for l in text.split('\n'):
        if l.strip() == '' or COMMENT.match(l) and not l.lstrip().startswith('#['):
            lines.append(l)
        else:
            break
    return flat('\n'.join(lines))

try:
    files = subprocess.run(['git', '-C', root, 'ls-files'], capture_output=True, text=True,
                           check=True).stdout.split('\n')
except (OSError, subprocess.CalledProcessError):
    files = []
files = [f for f in files if f]
if not files:  # not a checkout, or a tree git doesn't track: every file under it
    files = [os.path.relpath(os.path.join(d, f), root) for d, _, fs in os.walk(root)
             if '/target' not in d and '/.git' not in d for f in fs]
if not files:
    print(f'check-credit: no files in {root}', file=sys.stderr); sys.exit(2)

found = []
for path in PORTED:
    text = read(path)
    if text is None:
        found.append((path, 'a ported file on the list is missing'))
    elif CREDIT not in header(text):
        found.append((path, f'its header lacks "{CREDIT}"'))
for path in GENERATED:
    text = read(path)
    if text is None:
        found.append((path, 'a generated file is missing'))
    elif CREDIT not in text.replace('\n', ' '):
        found.append((path, f'generated from ported data, and it lacks "{CREDIT}"'))
notices = read('THIRD-PARTY-NOTICES.md') or ''
for line in MIT:
    if line not in notices:
        found.append(('THIRD-PARTY-NOTICES.md', f'the MIT notice lacks "{line[:40]}"'))
        break
notice = read('NOTICE.md') or ''
if 'THIRD-PARTY-NOTICES.md' not in notice or 'Board' not in notice:
    found.append(('NOTICE.md', "it doesn't point to THIRD-PARTY-NOTICES.md for Board"))
for path in files:
    if path in SKIP or path in PORTED or not path.endswith(SOURCES):
        continue
    text = read(path)
    if text is not None and MARKER.search(flat(text)):
        found.append((path, 'it names Board as a source, and it is not on the list of ported files'))

for path, why in found:
    print(f'FAILED {path}: {why}')
if found:
    print(f'check-credit: {len(found)} finding(s) ({len(files)} files checked)', file=sys.stderr)
    sys.exit(1)
print(f'check-credit: clean ({len(PORTED)} ported, {len(GENERATED)} generated, {len(files)} files checked)')
PY
