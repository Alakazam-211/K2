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
#   dropped      no file is under a tree in DROPPED_TREES, and no kept text file names one
#                (DROPPED_REFS; each hit printed as `check-public-cut: dropped: PATH:LINE: ...`).
#                A CHANGELOG, vendor/ and the two cut scripts may. No file in DROPPED_FILES
#                exists, and no kept text file names one (by its name without `.sh`).
#   text         no line added since the commit recorded in TEXT_BASE (the previous host release;
#                every tracked text file git diffs, vendored trees too) uses a term in
#                TEXT_TERMS, printed as `check-public-cut: text: PATH:LINE: TERM`. Public text
#                is reviewed by this generic scan. A term matches any case, from the start of a
#                word, across `_`, `-`, camelCase and a line wrap. The rule reads prose: in .rs
#                and .ts files, the code tokens in TEXT_CODE_TOKENS are skipped first, outside
#                comments and string literals on that line. TEXT_EXCEPTIONS lists the
#                lines reviewed and kept, one `PATH: TEXT` each (the whole added line, trimmed);
#                an entry that matches no such line blocks too. A TEXT_BASE that is missing,
#                untracked, not one full sha or not an ancestor of HEAD blocks, as does a
#                root with no git history. Exempt: TEXT_EXCEPTIONS and the two lists' own lines.
#   private      tools/check-private.sh is clean.
#   windows-names  tools/check-windows-names.sh is clean: this tree is what a host's Windows
#                machines check out. The script missing blocks too.
# Each finding is printed as `check-public-cut: RULE: WHAT`, every offending path or entry
# named. Exit 0 clean, 1 findings, 2 usage.
set -eu

# ---- The allowlist: the one place to change it. -------------------------------------------
# Engine crates a public cut may carry (already public). Space-separated crate names.
PUBLIC_ENGINE_CRATES="mpce-model"
# Cargo features a public cut removes with their code (the native engine's; the entry points
# of the trees below): never declared or requested in a public cut.
NATIVE_FEATURES="native fuzzing"
# Workspace crates a public cut drops whole, as PATH:PACKAGE.
DROPPED_CRATES="crates/conformance:k2-mpc-conformance"
# Trees a public cut removes whole (tooling no host builds or runs, and the evidence run
# records: docs/evidence/runs, about 16 MB, checked on main), and what names them in a kept
# file: extended regexes, |-separated. DROPPED_REFS has no pattern for the run records: the
# evidence notes the cut keeps name the folder their citations point to.
DROPPED_TREES="fuzz tools/fuzz docs/evidence/runs"
DROPPED_REFS='(^|[^A-Za-z0-9_-])fuzz/|tools/fuzz([^A-Za-z0-9_-]|$)|(^[ \t]*|exclude.*)"fuzz"|cargo[- ]fuzz|k2[-_]mpc[-_]base[-_]fuzz|k2_mpc_base::fuzzing|(^|[^A-Z_])FUZZ_(CPUS|HOME|MAX_JOBS|MEM|NODES|SECS|SHIP|SUMMARY_DIR|TIMEOUT|TZ|USED|WINDOW|WITH|WS)'
# Single files a public cut removes (a check of what the cut takes out), paths from the root.
DROPPED_FILES="tools/check-engine-features.sh tools/test-engine-features.sh"
# Files a public cut must carry at its root.
LICENCE_FILES="LICENSE-MIT LICENSE-APACHE"
# The pinned regression tests every cut must still have (relative to the root).
REGRESSIONS=tools/public-cut-regressions.txt
# Generic defect language, |-separated, lower case; each term matches from the start of a word.
TEXT_TERMS='no longer|instead of returning|used to|was wrong|before the fix|hotfix|hot fix|an engine fix|crash|panic|overflow|out of bounds|reproduc'
# What the text rule skips in .rs and .ts code, only these shapes (a macro call, a path, a member,
# a std name): extended regexes, |-separated. A test name or a plain word in code still counts.
TEXT_CODE_TOKENS='panic!|::panic|panic::|should_panic|panic_any|panic_handler|Panic(Hook)?Info|overflowing_|(::|[.])(overflow|Overflow)'
# The commit the text rule diffs from, and the lines reviewed and kept (tools/README.md).
TEXT_BASE=tools/public-text-base
TEXT_EXCEPTIONS=tools/public-text-exceptions.txt
# ---------------------------------------------------------------------------------------------

if [ "${1:-}" = --print-config ]; then
  printf 'PUBLIC_ENGINE_CRATES=%s\nNATIVE_FEATURES=%s\nDROPPED_CRATES=%s\nDROPPED_TREES=%s\nDROPPED_FILES=%s\nDROPPED_REFS=%s\nTEXT_TERMS=%s\nTEXT_CODE_TOKENS=%s\nTEXT_BASE=%s\nTEXT_EXCEPTIONS=%s\n' \
    "'$PUBLIC_ENGINE_CRATES'" "'$NATIVE_FEATURES'" "'$DROPPED_CRATES'" "'$DROPPED_TREES'" "'$DROPPED_FILES'" "'$DROPPED_REFS'" "'$TEXT_TERMS'" "'$TEXT_CODE_TOKENS'" "'$TEXT_BASE'" "'$TEXT_EXCEPTIONS'"
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
          print "check-public-cut: manifests: " f ":" NR ": [" t "] declares the cut-only feature " a[i]
        else if (index(s, "\"" a[i] "\"") || index(s, "/" a[i] "\""))
          print "check-public-cut: manifests: " f ":" NR ": [" t "] requests the cut-only feature " a[i]
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
          print "check-public-cut: sources: " f ":" NR ": a cfg on the cut-only feature " nf[i]
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
    grep -nE -e "--features[ =]+[\"']?([a-z0-9_-]+,)*$nf([,\"' ]|\$)" -e "features[[:space:]]*=[[:space:]]*\[[^]]*\"$nf\"" "$f" | grep -vE '^[0-9]+:[[:space:]]*#' | sed "s|^|check-public-cut: tooling: $f:|; s|\$| (a cut-only feature)|" >> "$OUT" || true
  done
  grep -n 'public-cut: drop-begin' "$f" | sed "s|^|check-public-cut: tooling: $f:|; s|\$| (a marker the cut must drop)|" >> "$OUT" || true
done

# a marker block left in a Markdown file
for f in $(files '\.md$' | grep -v '^vendor/'); do
  grep -nE '^[[:space:]]*<!-- public-cut: drop-' "$f" | sed "s|^|check-public-cut: tooling: $f:|; s|\$| (a marker the cut must drop)|" >> "$OUT" || true
done

# ---- dropped --------------------------------------------------------------------------------
for t in $DROPPED_TREES; do
  if grep -q "^$t/" "$LIST"; then
    grep "^$t/" "$LIST" | while IFS= read -r f; do bad "dropped: $f (under $t, which the cut removes)"; done
  elif [ -e "$t" ]; then
    bad "dropped: $t exists (a tree the cut removes)"
  fi
done
for f in $DROPPED_FILES; do
  if grep -qxF "$f" "$LIST" || [ -e "$f" ]; then bad "dropped: $f exists (a file the cut removes)"; fi
done
grep -Ev '^vendor/|(^|/)CHANGELOG\.md$|^tools/(check-public-cut|public-cut)\.sh$' "$LIST" | awk -v refs="$DROPPED_REFS" -v trees="$DROPPED_TREES" -v dfiles="$DROPPED_FILES" '
  BEGIN {
    nt = split(trees, tree, " ")
    nf = split(dfiles, df, " ")
    for (i = 1; i <= nf; i++) { s = df[i]; sub(/.*\//, "", s); sub(/\.sh$/, "", s); stem[i] = s }
  }
  {
    file = $0
    for (i = 1; i <= nt; i++) if (index(file, tree[i] "/") == 1) next
    for (i = 1; i <= nf; i++) if (file == df[i]) next
    n = 0
    while ((getline line < file) > 0) {
      n++
      if (line ~ refs) print "check-public-cut: dropped: " file ":" n ": names a tree the cut removes"
      for (i = 1; i <= nf; i++) if (index(line, stem[i])) print "check-public-cut: dropped: " file ":" n ": names a file the cut removes"
    }
    close(file)
  }' >> "$OUT"

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

# ---- text -----------------------------------------------------------------------------------
# text_base: print the recorded commit, or record why there is none and return 1
text_base() {
  if [ "$(git rev-parse --show-cdup 2>&1 || echo no)" != "" ]; then
    bad "text: no git history at this root: the scan needs it, back to the commit in $TEXT_BASE"; return 1
  fi
  if [ ! -f "$TEXT_BASE" ] || ! grep -qxF "$TEXT_BASE" "$LIST"; then
    bad "text: $TEXT_BASE: missing or not tracked: record the previous host release's pre-cut commit (tools/README.md, \"The public cut\")"; return 1
  fi
  sha=$(awk '!/^[ \t]*(#|$)/ { n++; s = $0 } END { if (n == 1 && length(s) == 40 && s ~ /^[0-9a-f]+$/) print s }' "$TEXT_BASE")
  if [ -z "$sha" ]; then
    bad "text: $TEXT_BASE: not one full commit sha: record the previous host release's pre-cut commit (tools/README.md, \"The public cut\")"; return 1
  fi
  if ! git merge-base --is-ancestor "$sha" HEAD 2>/dev/null; then
    bad "text: $TEXT_BASE: $sha is not an ancestor of HEAD: update it to the previous host release's pre-cut commit (tools/README.md, \"The public cut\")"; return 1
  fi
  printf '%s\n' "$sha"
}
if [ -f "$TEXT_EXCEPTIONS" ] && [ -z "$(git ls-files -- "$TEXT_EXCEPTIONS" 2>/dev/null)" ]; then
  bad "text: $TEXT_EXCEPTIONS: not tracked"
fi
TBASE=$(mktemp)
if text_base > "$TBASE"; then
  TD=$(mktemp)
  # the tree (the cut, staged or committed) against the base; -U0, so only added lines count.
  # Every option that a user's git config could change is given here.
  if git -c core.quotepath=off diff --no-color --no-ext-diff --no-textconv --no-renames --no-relative \
       --diff-filter=d --src-prefix=a/ --dst-prefix=b/ -U0 --inter-hunk-context=0 "$(cat "$TBASE")" -- > "$TD" 2>/dev/null; then
    awk -v terms="$TEXT_TERMS" -v tokens="$TEXT_CODE_TOKENS" -v exfile="$TEXT_EXCEPTIONS" \
        -v selfline="TEXT_TERMS='$TEXT_TERMS'" -v selftokens="TEXT_CODE_TOKENS='$TEXT_CODE_TOKENS'" '
      function trim(s) { gsub(/^[ \t\r]+|[ \t\r]+$/, "", s); return s }
      # the words of a line, one space apart, between spaces: camelCase, `_`, `-` and every other
      # non-alphanumeric are word breaks (bytes: LC_ALL=C)
      function words(s) {
        while (match(s, /[a-z][A-Z]/)) s = substr(s, 1, RSTART) " " substr(s, RSTART + 1)
        s = tolower(s); gsub(/[^a-z0-9]+/, " ", s)
        if (substr(s, 1, 1) != " ") s = " " s
        if (substr(s, length(s), 1) != " ") s = s " "
        return s
      }
      function has(s, t) { return index(s, " " t) > 0 }
      function uncoded(s) { gsub(tokens, " ", s); return s }
      # a line of code without its code tokens: a comment (to the end of the line) and each string
      # literal stay whole. One line at a time, no parser: an unclosed literal runs to the end of
      # the line, and whatever is misread is only ever scanned more, as nothing but the tokens goes
      function prose(raw, starts,    out, s, c, p, n) {
        out = ""; s = raw
        while (match(s, starts)) {
          c = substr(s, RSTART, 1); p = RSTART
          out = out uncoded(substr(s, 1, p - 1)); s = substr(s, p)
          if (c == "/") return out " " s
          n = length(s); p = 2
          while (p <= n && substr(s, p, 1) != c) p += (substr(s, p, 1) == "\\" ? 2 : 1)
          if (p > n) return out " " s
          out = out " " substr(s, 1, p) " "; s = substr(s, p + 1)
        }
        return out uncoded(s)
      }
      function added(raw,    cur, key, i) {
        if (file ~ /\.rs$/) cur = words(prose(raw, rs_starts))
        else if (file ~ /\.ts$/) cur = words(prose(raw, ts_starts))
        else cur = words(raw)
        if (file != exfile && !(file == "tools/check-public-cut.sh" && (raw == selfline || raw == selftokens))) {
          key = file ": " trim(raw)
          for (i = 1; i <= nterms; i++) {
            # a phrase may wrap from the added line right above; one wholly above was reported there
            if (has(cur, term[i]) || (index(term[i], " ") && prev != "" && !has(prev, term[i]) && has(prev substr(cur, 2), term[i]))) {
              if (key in exline) used[key] = 1
              else print "check-public-cut: text: " file ":" ln ": " term[i]
            }
          }
        }
        prev = cur
      }
      BEGIN {
        # what starts a comment or a literal; a Rust quote may be a lifetime, a TS one never is
        rs_starts = "//|/[*]|\""
        ts_starts = rs_starts "|`|\047"
        nterms = split(terms, term, "|")
        nex = 0
        while ((getline row < exfile) > 0) {
          nr++
          row = trim(row)
          if (row == "" || substr(row, 1, 1) == "#") continue
          exline[row] = nr; exkey[++nex] = row
        }
        close(exfile)
      }
      rem_old > 0 || rem_new > 0 {
        c = substr($0, 1, 1)
        if (c == "+") { added(substr($0, 2)); ln++; rem_new-- }
        else if (c == "-") rem_old--
        next
      }
      /^\+\+\+ / {
        file = substr($0, 5); sub(/\t$/, "", file)
        if (substr(file, 1, 1) == "\"") { file = substr(file, 2); sub(/"$/, "", file) }
        sub(/^b\//, "", file)
        next
      }
      /^@@ -/ {
        h = $0; sub(/^@@ -/, "", h); split(h, side, " ")
        rem_old = (index(side[1], ",") ? substr(side[1], index(side[1], ",") + 1) : 1) + 0
        rem_new = (index(side[2], ",") ? substr(side[2], index(side[2], ",") + 1) : 1) + 0
        ln = substr(side[2], 2) + 0
        prev = ""
      }
      END {
        for (i = 1; i <= nex; i++)
          if (!(exkey[i] in used))
            print "check-public-cut: text: " exfile ":" exline[exkey[i]] ": no added line with a term matches this entry"
      }' "$TD" >> "$OUT"
  else
    bad "text: git diff from $(cat "$TBASE") failed"
  fi
  rm -f "$TD"
fi
rm -f "$TBASE"

# ---- private --------------------------------------------------------------------------------
if [ -f tools/check-private.sh ]; then
  if ! P=$(sh tools/check-private.sh . 2>&1); then
    printf '%s\n' "$P" | sed 's|^|check-public-cut: private: |' >> "$OUT"
  fi
else
  bad "private: tools/check-private.sh missing"
fi

# ---- windows-names --------------------------------------------------------------------------
if [ -f tools/check-windows-names.sh ]; then
  if ! P=$(sh tools/check-windows-names.sh . 2>&1); then
    printf '%s\n' "$P" | sed '/^check-windows-names: reading names from /d; s|^|check-public-cut: windows-names: |' >> "$OUT"
    bad "windows-names: tools/check-windows-names.sh failed"
  fi
else
  bad "windows-names: tools/check-windows-names.sh missing"
fi

FILES=$(wc -l < "$LIST" | tr -d ' ')
if [ -s "$OUT" ]; then
  cat "$OUT"
  echo "check-public-cut: BLOCKED: $(wc -l < "$OUT" | tr -d ' ') finding(s) ($FILES files checked)" >&2
  exit 1
fi
echo "check-public-cut: clean ($FILES files checked; engine crates allowed: $PUBLIC_ENGINE_CRATES)"
