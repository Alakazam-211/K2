#!/bin/sh
# Fail closed unless this tree may be a release's PUBLIC CUT: the tree a host pulls (squashed,
# by git subtree) into a public repo. tools/public-cut.sh makes the cut and calls this last; a
# release gate runs it again on the checked-out cut. POSIX sh, git, grep and awk only.
#
#   tools/check-public-cut.sh [ROOT]          check ROOT (default: the repo root)
#   tools/check-public-cut.sh --print-config  the allowlist below, as shell assignments
#
# Allowlist rules (anything not allowed blocks the tag):
#   licence      every file in LICENCE_FILES is at the root, tracked and not empty.
#   paths        no tracked path has a component named mpce* (a directory `mpce-x`, a file
#                `mpce.rs`, `mpce_x.rs`...) unless its crate name is in PUBLIC_ENGINE_CRATES,
#                and no directory vendor/mpce* other than those crates' exists on disk.
#   manifests    no Cargo.toml and no Cargo.lock anywhere in the tree names an `mpce-*` /
#                `mpce_*` crate outside PUBLIC_ENGINE_CRATES (dependencies, [patch],
#                [workspace.dependencies], features, lock entries; comments included), each
#                finding naming its table and entry; no Cargo.toml declares or requests a
#                feature in NATIVE_FEATURES, and no feature list enables a feature its
#                manifest doesn't declare (one the cut removed).
#   sources      no .rs file names an engine crate outside PUBLIC_ENGINE_CRATES (a path
#                `mpce_x::`, `extern crate mpce_x`, or the names mpce-core / -sync / -codec /
#                -licences in any form), none has a cfg on a NATIVE_FEATURES feature, and
#                every cfg on a feature names one its crate's Cargo.toml declares.
#   tooling      no shell, Python, TOML, JSON or YAML file outside vendor/ names a crate in
#                DROPPED_CRATES (its path or package) or requests a NATIVE_FEATURES feature
#                (`--features F`, `features = [... "F" ...]`), and no `public-cut: drop-begin`
#                marker is left.
#   regressions  every pinned regression in REGRESSIONS exists: `NAME` is a Rust `fn NAME`
#                carrying a test attribute and no #[ignore]; `PATH IDENT` is the word IDENT in
#                the file PATH. A missing or empty list blocks the tag too.
#   private      tools/check-private.sh is clean.
# Each finding is printed as `check-public-cut: RULE: WHAT`, every offending path or entry
# named. Exit 0 clean, 1 findings, 2 usage.
set -eu

# ---- The allowlist: the one place to change it. -------------------------------------------
# Engine crates a public cut may carry (already public). Space-separated crate names.
PUBLIC_ENGINE_CRATES="mpce-model"
# The kit's engine-only cargo features: never declared or requested in a public cut.
NATIVE_FEATURES="native"
# Workspace crates a public cut drops whole, as PATH:PACKAGE.
DROPPED_CRATES="crates/conformance:k2-mpc-conformance"
# Files a public cut must carry at its root.
LICENCE_FILES="LICENSE-MIT LICENSE-APACHE"
# The pinned regression tests every cut must still have (relative to the root).
REGRESSIONS=tools/public-cut-regressions.txt
# ---------------------------------------------------------------------------------------------

if [ "${1:-}" = --print-config ]; then
  printf 'PUBLIC_ENGINE_CRATES=%s\nNATIVE_FEATURES=%s\nDROPPED_CRATES=%s\n' \
    "'$PUBLIC_ENGINE_CRATES'" "'$NATIVE_FEATURES'" "'$DROPPED_CRATES'"
  exit 0
fi
case ${1:-} in -*) echo "usage: tools/check-public-cut.sh [ROOT] | --print-config" >&2; exit 2 ;; esac
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=${1:-$(cd "$HERE/.." && pwd)}
[ -d "$ROOT" ] || { echo "check-public-cut: no directory $ROOT" >&2; exit 2; }
cd "$ROOT"
export LC_ALL=C

LIST=$(mktemp); OUT=$(mktemp)
trap 'rm -f "$LIST" "$OUT"' EXIT
if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  git ls-files -z | tr '\0' '\n' > "$LIST"
else
  find . -type f ! -path './target/*' ! -path './.git/*' | sed 's|^\./||' | sort > "$LIST"
fi
: > "$OUT"
bad() { echo "check-public-cut: $*" >> "$OUT"; }
# the existing files of LIST matching an extended regex
files() { grep -E "$1" "$LIST" | while IFS= read -r f; do [ -f "$f" ] && printf '%s\n' "$f"; done; true; }

# ---- licence --------------------------------------------------------------------------------
for f in $LICENCE_FILES; do
  if [ ! -s "$f" ]; then bad "licence: $f missing or empty at the root"
  elif ! grep -qx "$f" "$LIST"; then bad "licence: $f not tracked"; fi
done

# ---- paths ----------------------------------------------------------------------------------
awk -v allow="$PUBLIC_ENGINE_CRATES" '
  BEGIN { n = split(allow, a, " "); for (i = 1; i <= n; i++) ok[a[i]] = 1 }
  {
    p = $0; k = split(p, c, "/")
    if (c[1] == "vendor" && (c[2] in ok)) next
    for (i = 1; i <= k; i++) {
      if (c[i] !~ /^mpce([-_.]|$)/) continue
      name = c[i]; sub(/\.[A-Za-z0-9]+$/, "", name); gsub(/_/, "-", name)
      if (!(name in ok)) { print "check-public-cut: paths: " p " (engine-only path component " c[i] ")"; break }
    }
  }' "$LIST" >> "$OUT"
for d in vendor/mpce*; do
  [ -e "$d" ] || continue
  n=${d#vendor/}
  case " $PUBLIC_ENGINE_CRATES " in *" $n "*) ;; *) bad "paths: $d exists (engine-only vendor directory)" ;; esac
done

# ---- manifests and lockfiles ----------------------------------------------------------------
# every mpce-* / mpce_* name outside the allowlist, comments included
for f in $(files '(^|/)Cargo\.(toml|lock)$'); do
  awk -v allow="$PUBLIC_ENGINE_CRATES" -v f="$f" '
    BEGIN { n = split(allow, a, " "); for (i = 1; i <= n; i++) ok[a[i]] = 1; table = "(top)" }
    /^[ \t]*\[/ { table = $0; sub(/[ \t]*#.*/, "", table); gsub(/^[ \t]*|[ \t]*$/, "", table); key = "" }
    /^[ \t]*"?[A-Za-z0-9_.-]+"?[ \t]*=/ { key = $0; sub(/^[ \t]*/, "", key); sub(/[ \t]*=.*/, "", key); gsub(/"/, "", key) }
    {
      s = $0
      while (match(s, /mpce[-_][A-Za-z0-9_-]*[A-Za-z0-9]/)) {
        t = substr(s, RSTART, RLENGTH); s = substr(s, RSTART + RLENGTH)
        u = t; gsub(/_/, "-", u)
        if (!(u in ok)) print "check-public-cut: manifests: " f ":" NR ": " table (key != "" ? " " key : "") ": names " t (($0 ~ /^[ \t]*#/) ? " (in a comment)" : "")
      }
    }' "$f" >> "$OUT"
done
# every plain element of a feature list must be a feature or an optional dependency of the same
# manifest (a removed feature left behind; cargo would refuse it too)
for f in $(files '(^|/)Cargo\.toml$' | grep -v '^vendor/'); do
  awk -v f="$f" '
    function tbl(l) { sub(/[ \t]*#.*/, "", l); gsub(/[ \t\[\]"]/, "", l); return l }
    FNR == NR {
      if ($0 ~ /^[ \t]*\[/) { t = tbl($0); next }
      if ($0 ~ /^[ \t]*#/) next
      if (t == "features" && $0 ~ /^[ \t]*"?[A-Za-z0-9_.-]+"?[ \t]*=/) { k = $0; sub(/^[ \t]*/, "", k); sub(/[ \t]*=.*/, "", k); gsub(/"/, "", k); have[k] = 1 }
      if (t ~ /(^|\.)(dev-|build-)?dependencies$/ && $0 ~ /optional[ \t]*=[ \t]*true/) { k = $0; sub(/^[ \t]*/, "", k); sub(/[ \t]*=.*/, "", k); gsub(/"/, "", k); have[k] = 1 }
      if (t ~ /dependencies\.[^.]+$/ && $0 ~ /^[ \t]*optional[ \t]*=[ \t]*true/) { k = t; sub(/.*\./, "", k); have[k] = 1 }
      next
    }
    /^[ \t]*\[/ { t = tbl($0); inf = 0; next }
    t == "features" && /^[ \t]*"?[A-Za-z0-9_.-]+"?[ \t]*=/ { k = $0; sub(/^[ \t]*/, "", k); sub(/[ \t]*=.*/, "", k); gsub(/"/, "", k); inf = 1 }
    t == "features" && inf {
      s = $0; sub(/#.*/, "", s)
      while (match(s, /"[^"]*"/)) {
        e = substr(s, RSTART + 1, RLENGTH - 2); s = substr(s, RSTART + RLENGTH)
        if (e !~ /[:\/]/ && !(e in have)) print "check-public-cut: manifests: " f ":" FNR ": [features] " k ": enables " e ", which this manifest does not declare"
      }
      if ($0 ~ /\]/) inf = 0
    }' "$f" "$f" >> "$OUT"
done
# native features: declared in [features], or requested by name ("native", "x/native")
for f in $(files '(^|/)Cargo\.toml$'); do
  awk -v feats="$NATIVE_FEATURES" -v f="$f" '
    BEGIN { n = split(feats, a, " ") }
    /^[ \t]*\[/ { t = $0; gsub(/[ \t\[\]]/, "", t) }
    {
      s = $0; sub(/^[ \t]*#.*/, "", s)
      for (i = 1; i <= n; i++) {
        if (t == "features" && s ~ "^[ \t]*\"?" a[i] "\"?[ \t]*=")
          print "check-public-cut: manifests: " f ":" NR ": [" t "] declares the engine-only feature " a[i]
        else if (index(s, "\"" a[i] "\"") || index(s, "/" a[i] "\""))
          print "check-public-cut: manifests: " f ":" NR ": [" t "] requests the engine-only feature " a[i]
      }
    }' "$f" >> "$OUT"
done

# ---- sources --------------------------------------------------------------------------------
for f in $(files '\.rs$'); do
  awk -v allow="$PUBLIC_ENGINE_CRATES" -v feats="$NATIVE_FEATURES" -v f="$f" '
    BEGIN {
      n = split(allow, a, " "); for (i = 1; i <= n; i++) { u = a[i]; gsub(/-/, "_", u); ok[u] = 1 }
      m = split(feats, nf, " ")
    }
    {
      s = $0
      if (match(s, /mpce[-_](core|sync|codec|licences)/))
        print "check-public-cut: sources: " f ":" NR ": names " substr(s, RSTART, RLENGTH)
      while (match(s, /(extern[ \t]+crate[ \t]+mpce_[A-Za-z0-9_]+|mpce_[A-Za-z0-9_]+[ \t]*::)/)) {
        t = substr(s, RSTART, RLENGTH); s = substr(s, RSTART + RLENGTH)
        c = t; sub(/^extern[ \t]+crate[ \t]+/, "", c); sub(/[ \t]*::$/, "", c)
        if (!(c in ok)) print "check-public-cut: sources: " f ":" NR ": uses the engine crate " c
      }
      for (i = 1; i <= m; i++)
        if ($0 ~ "feature[ \t]*=[ \t]*\"" nf[i] "\"")
          print "check-public-cut: sources: " f ":" NR ": a cfg on the engine-only feature " nf[i]
    }' "$f" >> "$OUT"
done

# every cfg on a feature names one its crate declares (a feature or an optional dependency):
# a cfg left on a removed feature is unexpected_cfgs, and the builds deny warnings
CR=$(mktemp)
for m in $(files '(^|/)Cargo\.toml$' | grep -v '^vendor/'); do
  grep -q '^\[package\]' "$m" || continue
  d=$(dirname "$m")
  awk -v d="$d" '
    function tbl(l) { sub(/[ \t]*#.*/, "", l); gsub(/[ \t\[\]"]/, "", l); return l }
    /^[ \t]*\[/ { t = tbl($0); next }
    /^[ \t]*#/ { next }
    t == "features" && /^[ \t]*"?[A-Za-z0-9_.-]+"?[ \t]*=/ { k = $0; sub(/^[ \t]*/, "", k); sub(/[ \t]*=.*/, "", k); gsub(/"/, "", k); print d, k }
    t ~ /(^|\.)(dev-|build-)?dependencies$/ && /optional[ \t]*=[ \t]*true/ { k = $0; sub(/^[ \t]*/, "", k); sub(/[ \t]*=.*/, "", k); gsub(/"/, "", k); print d, k }
    END { print d, "" }' "$m" >> "$CR"
done
files '\.rs$' | grep -v '^vendor/' | awk -v cr="$CR" '
  BEGIN { while ((getline l < cr) > 0) { split(l, x, " "); dirs[x[1]] = 1; if (x[2] != "") have[x[1], x[2]] = 1 } }
  {
    f = $0; best = ""
    for (d in dirs) if ((d == "." || index(f, d "/") == 1) && length(d) > length(best)) best = d
    if (best == "") next
    n = 0
    while ((getline line < f) > 0) {
      n++
      if (line ~ /^[ \t]*\/\// || line !~ /cfg/) continue
      s = line
      while (match(s, /feature[ \t]*=[ \t]*"[^"]*"/)) {
        e = substr(s, RSTART, RLENGTH); s = substr(s, RSTART + RLENGTH); sub(/^[^"]*"/, "", e); sub(/"$/, "", e)
        if (!((best, e) in have)) print "check-public-cut: sources: " f ":" n ": a cfg on feature " e ", which " (best == "." ? "" : best "/") "Cargo.toml does not declare"
      }
    }
    close(f)
  }' >> "$OUT"
rm -f "$CR"

# ---- tooling --------------------------------------------------------------------------------
for f in $(files '\.(sh|py|toml|json|ya?ml)$' | grep -v '^vendor/'); do
  [ "$f" = tools/check-public-cut.sh ] && continue
  [ "$f" = tools/public-cut.sh ] && continue
  # comment lines may name them (docs may); code may not
  for dc in $DROPPED_CRATES; do
    dpath=${dc%%:*}; dpkg=${dc#*:}
    grep -nF -e "$dpath" -e "$dpkg" "$f" | grep -vE '^[0-9]+:[[:space:]]*#' | sed "s|^|check-public-cut: tooling: $f:|; s|\$| (a dropped crate)|" >> "$OUT" || true
  done
  for nf in $NATIVE_FEATURES; do
    grep -nE -e "--features[ =]+[\"']?([a-z0-9_-]+,)*$nf([,\"' ]|\$)" -e "features[[:space:]]*=[[:space:]]*\[[^]]*\"$nf\"" "$f" | grep -vE '^[0-9]+:[[:space:]]*#' | sed "s|^|check-public-cut: tooling: $f:|; s|\$| (an engine-only feature)|" >> "$OUT" || true
  done
  grep -n 'public-cut: drop-begin' "$f" | sed "s|^|check-public-cut: tooling: $f:|; s|\$| (a marker the cut must drop)|" >> "$OUT" || true
done

# ---- regressions ----------------------------------------------------------------------------
if [ ! -s "$REGRESSIONS" ]; then
  bad "regressions: $REGRESSIONS missing or empty"
else
  RS=$(mktemp)
  files '\.rs$' > "$RS"
  awk '
    # pass 1: the list (NAME, or PATH IDENT)
    FNR == NR {
      sub(/#.*/, ""); gsub(/^[ \t]+|[ \t]+$/, "")
      if ($0 == "") next
      if (NF == 1) { fn[$1] = 0; order[++n] = $1 }
      else if (NF == 2) { idp[++m] = $1; idw[m] = $2 }
      else print "check-public-cut: regressions: unreadable line \"" $0 "\""
      next
    }
    # pass 2: the .rs files, one name per line
    {
      file = $0; blk = ""
      while ((getline line < file) > 0) {
        if (line ~ /^[ \t]*(#\[|\/\/)/) { blk = blk "\n" line; continue }
        if (match(line, /fn[ \t]+[A-Za-z0-9_]+[ \t]*[(<]/)) {
          w = substr(line, RSTART, RLENGTH); sub(/^fn[ \t]+/, "", w); sub(/[ \t]*[(<]$/, "", w)
          if (w in fn) {
            if (blk ~ /#\[ignore/) { if (fn[w] < 1) fn[w] = -1 }
            else if (blk ~ /#\[([A-Za-z0-9_]+::)*test(_case)?(\]|\()/) fn[w] = 1
            else if (fn[w] == 0) fn[w] = -2
          }
        }
        blk = ""
      }
      close(file)
    }
    END {
      if (n + m == 0) print "check-public-cut: regressions: the list names no test"
      for (i = 1; i <= n; i++) {
        w = order[i]
        if (fn[w] == 0) print "check-public-cut: regressions: test fn " w " missing"
        else if (fn[w] == -1) print "check-public-cut: regressions: test fn " w " is #[ignore]d"
        else if (fn[w] == -2) print "check-public-cut: regressions: fn " w " is not a test (no test attribute)"
      }
      for (i = 1; i <= m; i++) {
        found = 0
        while ((getline line < idp[i]) > 0)
          if (line ~ ("(^|[^A-Za-z0-9_])" idw[i] "([^A-Za-z0-9_]|$)")) found = 1
        close(idp[i])
        if (!found) print "check-public-cut: regressions: " idw[i] " missing from " idp[i]
      }
    }' "$REGRESSIONS" "$RS" >> "$OUT"
  rm -f "$RS"
fi

# ---- private --------------------------------------------------------------------------------
if [ -f tools/check-private.sh ]; then
  if ! P=$(sh tools/check-private.sh . 2>&1); then
    printf '%s\n' "$P" | sed 's|^|check-public-cut: private: |' >> "$OUT"
  fi
else
  bad "private: tools/check-private.sh missing"
fi

FILES=$(wc -l < "$LIST" | tr -d ' ')
if [ -s "$OUT" ]; then
  cat "$OUT"
  echo "check-public-cut: BLOCKED: $(wc -l < "$OUT" | tr -d ' ') finding(s) ($FILES files checked)" >&2
  exit 1
fi
echo "check-public-cut: clean ($FILES files checked; engine crates allowed: $PUBLIC_ENGINE_CRATES)"
