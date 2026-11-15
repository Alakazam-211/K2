#!/bin/sh
# Fail closed when public text holds an internal label: a requirement id (`K-BR-12`), an ask
# label (`K5`), a plan slice, a numbered contract gap, a `KC` item, a pointer to a PRD, a
# release-line name (`C1e`), an amendment label (`A13`), an unnumbered plan slice, an engine-answer
# label (`E8`), a plan stage, a parity-row or plan-item label (`P24`; a table's first cell may hold
# parity row ids), a finding label (`F2`) or a run label (`V1`) in prose; a branch name or an
# engine code anywhere; or one of this repo's own plan labels (PACK_LABELS below). Every tracked text file is read but this
# script. POSIX sh, git, grep and awk only.
#
#   scripts/check-labels.sh [ROOT]     exit 0 clean, 1 findings (each `check-labels: PATH:LINE: KIND`)
set -eu
export LC_ALL=C
cd "${1:-$(dirname "$0")/..}"
# KIND=extended regex, one per line.
LABEL_PATTERNS='requirement id=(^|[^A-Za-z0-9_-])[KEX]-[A-Z][A-Z0-9]*-[0-9]
ask label=(^|[^A-Za-z0-9_.-])K([13-9]|1[01])([^A-Za-z0-9_,]|$)
plan slice=(^|[^A-Za-z])[Ss]lices? [0-9]
contract gap=rendering-data contract|contract.s gaps? [0-9]|contract gaps? [0-9]|[(;] ?gaps? [0-9]
KC item=(^|[^A-Za-z0-9_-])KC[0-9]
PRD pointer=(^|[^A-Za-z])PRDs?([^A-Za-z]|$)
plan pointer=the plan.s [A-Z]
branch name=(^|[^A-Za-z0-9_./-])(feat|fix|chore|rel|release|exp|hygiene|bump|probe)/[a-z0-9]'
# Read in prose only (a Markdown line, or the comment part of a code line, its `code` spans
# taken out), where a cell reference or an option value can't be meant.
PROSE_PATTERNS='release-line name=(^|[^A-Za-z0-9_.!:$'"'"'"=/-])C[1-9][0-9]?[a-z]?([^A-Za-z0-9_:'"'"'"]|: |$)
amendment label=(^|[^A-Za-z0-9_.!:$'"'"'"=/-])(A([2-9]|1[0-9])|A1[-–]A[0-9])([^A-Za-z0-9_:'"'"'"]|: |$)
unnumbered plan slice=(^|[^A-Za-z])([Tt]his|[Tt]hat|[Nn]ext|[Ee]arlier|[Ll]ater) slice([^A-Za-z]|$)
engine-answer label=(^|[^A-Za-z0-9_.!:$'"'"'"=/-])E[1-9]([^A-Za-z0-9_:'"'"'"]|: |$)
plan stage=(^|[^A-Za-z])stage [A-Z]([^A-Za-z]|$)
parity or plan item label=(^|[^A-Za-z0-9_.!:$'"'"'"=/-])P[0-9][0-9]?[a-z]?([^A-Za-z0-9_:'"'"'"]|: |$)
finding label=(^|[^A-Za-z0-9_.!:$'"'"'"=/-])F[0-9][0-9]?([^A-Za-z0-9_:'"'"'"]|: |$)
run label=(^|[^A-Za-z0-9_.!:$'"'"'"=/-])V([0-79]|[1-9][0-9])([^A-Za-z0-9_:'"'"'"]|: |$)'
# This repo's own plan labels (KIND=regex lines, or empty).
PACK_LABELS='plan label=(^|[^A-Za-z0-9_])(F[1-5]|M1)([^A-Za-z0-9_]|$)'
OUT=$(mktemp); trap 'rm -f "$OUT"' EXIT
git ls-files | grep -v '^scripts/check-labels\.sh$' | while IFS= read -r f; do
  [ -f "$f" ] && grep -Iq . "$f" 2>/dev/null && printf '%s\n' "$f"
done | PATS="$LABEL_PATTERNS
$PACK_LABELS" PROSE="$PROSE_PATTERNS" awk '
  BEGIN { n = 0; m = split(ENVIRON["PATS"], row, "\n")
          for (i = 1; i <= m; i++) { k = index(row[i], "="); if (k) { n++; kind[n] = substr(row[i], 1, k - 1); rx[n] = substr(row[i], k + 1) } }
          q = split(ENVIRON["PROSE"], row, "\n")
          for (i = 1; i <= q; i++) { k = index(row[i], "="); pkind[i] = substr(row[i], 1, k - 1); prx[i] = substr(row[i], k + 1) } }
  { file = $0; ln = 0; md = (file ~ /\.md$/)
    while ((getline line < file) > 0) {
      ln++; for (i = 1; i <= n; i++) if (line ~ rx[i]) print "check-labels: " file ":" ln ": " kind[i]
      t = ""
      if (md) t = line
      else if (line ~ /^[ \t]*(\/\/|#|\*|\/\*)/) t = line
      else if ((p = index(line, "// ")) > 0) t = substr(line, p)
      # a table row whose first cell holds parity row ids keeps them (the kit prints them)
      if (md && t ~ /^[ \t]*\| *P[0-9]/) sub(/\| *P[0-9][^|]*\|/, "|", t)
      if (t != "") { gsub(/`[^`]*`/, "", t); for (i = 1; i <= q; i++) if (t ~ prx[i]) print "check-labels: " file ":" ln ": " pkind[i] }
      # an engine code (`MPCE-Ennn`): its registry is in the kit, so a pack names none in its
      # text (a code alone in a string literal, as code matches it, passes)
      u = line; gsub(/"MPCE-E[0-9]+"/, "", u); if (u ~ /MPCE-E[0-9]/) print "check-labels: " file ":" ln ": engine code"
    }
    close(file) }' > "$OUT"
if [ -s "$OUT" ]; then cat "$OUT"; echo "check-labels: $(wc -l < "$OUT" | tr -d ' ') finding(s)" >&2; exit 1; fi
echo "check-labels: clean ($(git ls-files | wc -l | tr -d ' ') files)"
