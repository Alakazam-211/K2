#!/usr/bin/env bash
# Brand tools. One for now:
#
#   bash scripts/brand.sh lint <brand folder> [--live] [--hosted] [--registry <file>]
#   bash scripts/brand.sh lint --name <command> [--product <name>] [--registry <file>]
#       The name policy: the command, its aliases, the brand id and the
#       product name, each on its own rules (scripts/brand/lint.py says
#       which), and the shapes of the brand file's other fields. --live
#       also asks this build host's shells and the package indexes.
#       --hosted also asks that every field a hosted box of the brand needs
#       is set, and is not our own value. --registry also checks the names
#       against the private list of brands reserved for distributors
#       (lint.py gives its format; only our onboarding passes it). Run when
#       a brand is made and on every brand build.
set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
case "${1:-}" in
    lint) shift; exec python3 "$HERE/scripts/brand/lint.py" "$@" ;;
    *) echo "usage: brand.sh lint <brand folder> [--live] [--hosted] [--registry <file>] | brand.sh lint --name <command> [--product <name>] [--registry <file>]" >&2; exit 2 ;;
esac
