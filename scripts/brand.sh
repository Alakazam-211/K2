#!/usr/bin/env bash
# Brand tools. One for now:
#
#   bash scripts/brand.sh lint <brand folder> [--live]
#   bash scripts/brand.sh lint --name <command> [--product <name>]
#       The name policy: the command, its aliases, the brand id and the
#       product name, each on its own rules (scripts/brand/lint.py says
#       which). --live also asks this build host's shells and the package
#       indexes. Run when a brand is made and on every brand build.
set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
case "${1:-}" in
    lint) shift; exec python3 "$HERE/scripts/brand/lint.py" "$@" ;;
    *) echo "usage: brand.sh lint <brand folder> [--live] | brand.sh lint --name <command> [--product <name>]" >&2; exit 2 ;;
esac
