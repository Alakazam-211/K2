#!/bin/sh
# Make a release's PUBLIC CUT: one commit, `release: public cut`, on a release branch, that
# removes everything engine-only, so the tag a host pulls (squashed, by git subtree, into a
# public repo) carries no native-engine source. The cut commit is never merged into main:
# main merges the pre-cut release commit.
#
#   tools/public-cut.sh [--no-commit] [--lock FILE] [--trailer TEXT]...
#
#   --no-commit   edit and stage the tree and run the checks, but commit nothing (a dry run;
#                 the edits stay in the working tree). Allowed on a detached HEAD, and on a
#                 tree with uncommitted changes (e.g. a merge --no-commit to try a branch).
#   --lock FILE   take FILE as the pruned Cargo.lock instead of the script's own prune (see
#                 "The lockfile"); refused unless it only removes lines from the current one.
#   --trailer T   a trailer for the commit message (git commit --trailer), repeatable.
#
# Run at the root of a clean checkout of the release branch (never main). Deterministic and
# idempotent: on a tree already cut it changes nothing, makes no commit and exits 0 (after
# the checks pass).
#
# What it removes (the allowlist is tools/check-public-cut.sh's, read with --print-config):
#   - every vendor/mpce* directory except PUBLIC_ENGINE_CRATES (the engine's vendored source);
#   - the crates in DROPPED_CRATES (whole: the conformance suite, which hard-depends on the
#     engine's sync layer and core), and their workspace membership;
#   - the trees in DROPPED_TREES (whole: tooling no host builds or runs);
#   - in every Cargo.toml outside vendor/ (the root, crates/*, tools/, any other):
#     dependencies on non-public mpce-* crates in every dependency table ([dependencies],
#     [dev-dependencies], [build-dependencies], their [target.'cfg(..)'.*] forms,
#     [workspace.dependencies], [patch.*], and `[...dependencies.mpce-x]` tables), each with
#     the comment block right above it. Engine-only FEATURES are derived, to a fixed point:
#     NATIVE_FEATURES, and any feature whose every element needs something removed (`dep:` or
#     `x/feature` of a removed crate, a removed feature of its own crate or of another, e.g.
#     `k2-mpc-base/native`). A feature that keeps some elements loses only the removed ones,
#     unless what it keeps includes an optional dependency (`dep:x`): then the script stops
#     (decide by hand; don't guess), as it does for a target whose required-features names a
#     removed feature. Removed features also go from dependencies' `features = [...]`;
#   - in every .rs outside vendor/: each item, statement, match arm, field or variant under
#     `#[cfg(feature = "F")]` or `#[cfg(all(..., feature = "F", ...))]` (F in NATIVE_FEATURES
#     or removed from the file's crate), with the doc comments, comments and attributes right above it; the
#     files of a module declared under one (`mod m;`: m.rs and m/); a whole file under
#     `#![cfg(feature = "F")]`; the attribute alone for `#[cfg(not(feature = "F"))]` and
#     `#[cfg_attr(feature = "F", ...)]`. Any other cfg form naming F stops the script. Code
#     goes entirely (no leftover cfg for an undeclared feature: unexpected_cfgs, -D warnings);
#   - in shell scripts under tools/ and scripts/: `case` arms for the parts DROPPED_PARTS
#     names (with their comment block) and their entries in a part list (`#   part  text`
#     with its continuation lines, `${PART:?...}`'s), `-p <dropped crate>` arguments, and
#     everything between `# public-cut: drop-begin` and `# public-cut: drop-end` lines
#     (markers included; unbalanced markers stop the script);
#   - in every .md and Cargo.toml outside vendor/: everything between `public-cut: drop-begin`
#     and `public-cut: drop-end` lines (`# ...` in TOML, `<!-- ... -->` in Markdown; the same);
#   - from Cargo.lock: see below.
# Docs, CHANGELOGs and vendor/README.md may still NAME the engine and its tags: only source
# goes. The root README.md gets one fixed paragraph saying what the cut leaves out.
#
# The lockfile. The script prunes every Cargo.lock that has a removed package itself, by
# text: the removed packages, every package no longer reachable from the workspace's members,
# their names in `dependencies` lists (a list left empty goes), and a reference made
# unambiguous by the removals written as cargo writes it (`"name"` once one version is
# left: the only changed lines). Nothing is upgraded: no version, source or checksum line
# changes. Cargo can't run
# where the cut is made (no local cargo), so the release gate PROVES the prune on a build
# node, on the cut commit:
#   cargo metadata --format-version 1 > /dev/null    (no --locked: cargo may rewrite it)
#   git diff --exit-code Cargo.lock                  (it didn't: cargo's own lock is this one)
# and every other cargo step there runs --locked. If cargo ever disagrees, take its lock
# from that job (the job prints it, or fetch it as an artifact) and rerun the cut on the
# pre-cut commit with --lock <that file>; the script refuses a lock that adds lines.
#
# Checks: tools/check-public-cut.sh last (fail-closed allowlist: licence files, no engine
# path, manifest, lock entry or identifier outside PUBLIC_ENGINE_CRATES, tooling, pinned
# regressions, check-private, check-windows-names). Any failure: non-zero exit and no commit.
# POSIX sh, git, grep, sed and awk only (LC_ALL=C).
set -eu
export LC_ALL=C

NOCOMMIT=0; LOCKFILE=; TRAILERS=
while [ $# -gt 0 ]; do
  case $1 in
    --no-commit) NOCOMMIT=1 ;;
    --lock) LOCKFILE=${2:?--lock FILE}; shift ;;
    --trailer) TRAILERS="$TRAILERS
${2:?--trailer TEXT}"; shift ;;
    *) echo "usage: tools/public-cut.sh [--no-commit] [--lock FILE] [--trailer TEXT]..." >&2; exit 2 ;;
  esac
  shift
done
[ -z "$LOCKFILE" ] || LOCKFILE=$(cd "$(dirname "$LOCKFILE")" && pwd)/$(basename "$LOCKFILE")

die() { echo "public-cut: $*" >&2; exit 1; }
TOP=$(git rev-parse --show-toplevel 2>/dev/null) || die "not in a git checkout"
cd "$TOP"
[ -f tools/check-public-cut.sh ] || die "no tools/check-public-cut.sh"
BRANCH=$(git symbolic-ref -q --short HEAD || true)
case $BRANCH in
  main|master) die "refusing to cut on $BRANCH: the cut lives on a release branch only" ;;
  "") [ "$NOCOMMIT" = 1 ] || die "detached HEAD: cut on a release branch, or pass --no-commit" ;;
esac
if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
  [ "$NOCOMMIT" = 1 ] || die "the tree has changes: commit or stash them first"
  echo "public-cut: --no-commit on a tree with changes: they are part of this dry run"
fi

eval "$(sh tools/check-public-cut.sh --print-config)"
DROPPED_PARTS="conformance $NATIVE_FEATURES"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
is_public() { case " $PUBLIC_ENGINE_CRATES " in *" $1 "*) return 0 ;; esac; return 1; }
replace() { if ! cmp -s "$1" "$2"; then cat "$2" > "$1"; echo "  edited $1"; fi; }

echo "public-cut: at $(git rev-parse --short HEAD) (${BRANCH:-detached})"

# ---- 1. paths -------------------------------------------------------------------------------
ENGINE=   # the engine package names removed (for the manifests and the lock)
for d in vendor/mpce*; do
  [ -d "$d" ] || continue
  n=${d#vendor/}
  is_public "$n" && continue
  for m in $(git ls-files "$d" | grep -E '(^|/)Cargo\.toml$' || true); do
    p=$(awk '/^\[package\]/{s=1;next} /^\[/{s=0} s && /^name[ \t]*=/{gsub(/.*=[ \t]*"|".*/,""); print; exit}' "$m")
    [ -n "$p" ] && ENGINE="$ENGINE $p"
  done
  if [ -n "$(git ls-files "$d")" ]; then git rm -r -q -f -- "$d"; echo "  removed $d"; fi
  rm -rf -- "$d"
done
DROPPKGS=
for dc in $DROPPED_CRATES; do
  dpath=${dc%%:*}; DROPPKGS="$DROPPKGS ${dc#*:}"
  if [ -n "$(git ls-files "$dpath")" ]; then git rm -r -q -f -- "$dpath"; echo "  removed $dpath"; fi
done
for t in $DROPPED_TREES; do
  if [ -n "$(git ls-files "$t")" ]; then git rm -r -q -f -- "$t"; echo "  removed $t"; fi
done

# ---- 2. manifests ---------------------------------------------------------------------------
# Every Cargo.toml outside vendor/, in passes until nothing changes (a feature can name one
# defined later, or another crate's). $WORK/removed collects the removed features as
# PACKAGE/FEATURE (`-` for a manifest without [package]); step 3 strips their cfgs.
DROPPATHS=$(for dc in $DROPPED_CRATES; do printf '%s ' "${dc%%:*}"; done); DROPPATHS=${DROPPATHS% }
pkgname() { awk '/^\[package\]/{s=1;next} /^\[/{s=0} s && /^name[ \t]*=/{gsub(/.*=[ \t]*"|".*/,""); print; exit}' "$1"; }
: > "$WORK/removed"; : > "$WORK/medited"
MANIFESTS=$(git ls-files '*Cargo.toml' | grep -v '^vendor/' || true)
pass=0
while :; do
  pass=$((pass + 1)); changed=0
  [ $pass -le 8 ] || die "the manifests didn't settle in 8 passes"
  for f in $MANIFESTS; do
    [ -f "$f" ] || continue
    pkg=$(pkgname "$f"); [ -n "$pkg" ] || pkg=-
    awk -v allow="$PUBLIC_ENGINE_CRATES" -v feats="$NATIVE_FEATURES" -v dropm="$DROPPATHS" -v f="$f" \
        -v pkg="$pkg" -v rmfile="$WORK/removed" '
      BEGIN {
        n = split(allow, a, " "); for (i = 1; i <= n; i++) ok[a[i]] = 1
        nf = split(feats, fa, " "); for (i = 1; i <= nf; i++) isfeat[fa[i]] = 1
        nd = split(dropm, da, " ")
        while ((getline l < rmfile) > 0) rm[l] = 1
        close(rmfile)
      }
      function engine(x,  u) { u = x; gsub(/_/, "-", u); return (u ~ /^mpce-/ && !(u in ok)) }
      function bal(s,  t, o, c) {
        t = s; gsub(/"([^"\\]|\\.)*"/, "", t); sub(/#.*/, "", t)
        o = gsub(/[\[{]/, "", t); c = gsub(/[\]}]/, "", t); return o - c
      }
      function deptable(t) { return t ~ /(^|\.)(dev-|build-)?dependencies$/ || t ~ /^patch\./ }
      function out(s) { printf "%s", s }
      function gone_feat(x) { return (x in isfeat) || ((pkg "/" x) in rm) }
      # a feature list element that needs something removed
      function native_el(e,   x, y) {
        if (e ~ /^dep:/) return engine(substr(e, 5))
        if (index(e, "/")) {
          x = e; sub(/\/.*/, "", x); sub(/\?$/, "", x); y = e; sub(/^[^\/]*\//, "", y)
          return engine(x) || ((x "/" y) in rm)
        }
        return gone_feat(e)
      }
      function record(x) { if (!((pkg "/" x) in rm)) { rm[pkg "/" x] = 1; print pkg "/" x >> rmfile } }
      function finish(   key, lhs, v, drop, i, x, p, e, n0, nk, kept) {
        lhs = ent; sub(/=.*/, "", lhs); sub(/[ \t]+$/, "", lhs)
        key = lhs; sub(/^[ \t]*/, "", key); gsub(/"/, "", key)
        v = ent; sub(/^[^=]*=/, "", v)
        drop = 0
        if (deptable(table)) {
          p = key
          if (match(v, /package[ \t]*=[ \t]*"[^"]*"/)) { p = substr(v, RSTART, RLENGTH); sub(/^[^"]*"/, "", p); sub(/"$/, "", p) }
          if (engine(key) || engine(p)) drop = 1
          if (match(v, /vendor\/mpce[A-Za-z0-9_-]*/)) { x = substr(v, RSTART + 7, RLENGTH - 7); if (engine(x)) drop = 1 }
          if (!drop && match(v, /features[ \t]*=[ \t]*\[[^\]]*\]/)) {
            x = substr(v, RSTART, RLENGTH)
            while (match(x, /"[^"]*"/)) {
              e = substr(x, RSTART + 1, RLENGTH - 2); x = substr(x, RSTART + RLENGTH)
              if ((p "/" e) in rm) { gsub("\"" e "\"[ \t]*,[ \t]*", "", ent); gsub("[ \t]*,[ \t]*\"" e "\"", "", ent); gsub("\"" e "\"", "", ent) }
            }
          }
        } else if (table == "features") {
          if (gone_feat(key)) drop = 1
          else {
            x = v; sub(/#[^"]*$/, "", x); n0 = 0; nk = 0; kept = ""
            while (match(x, /"[^"]*"/)) {
              e = substr(x, RSTART + 1, RLENGTH - 2); x = substr(x, RSTART + RLENGTH); n0++
              if (!native_el(e)) { nk++; kept = kept (nk > 1 ? ", " : "") "\"" e "\"" }
            }
            if (n0 > 0 && nk == 0) drop = 1
            else if (nk < n0 && kept ~ /"dep:/) err = err "\n" f ": feature " key " mixes engine parts with optional dependencies (" kept "): decide by hand"
            else if (nk < n0) ent = lhs " = [" kept "]\n"
          }
          if (drop) record(key)
        } else if (table == "workspace" && (key == "members" || key == "default-members")) {
          for (i = 1; i <= nd; i++) { gsub("\"" da[i] "\"[ \t]*,[ \t]*", "", ent); gsub("[ \t]*,[ \t]*\"" da[i] "\"", "", ent); gsub("\"" da[i] "\"", "", ent) }
        }
        if (key == "required-features") {
          x = v
          while (match(x, /"[^"]*"/)) { e = substr(x, RSTART + 1, RLENGTH - 2); x = substr(x, RSTART + RLENGTH); if (gone_feat(e)) err = err "\n" f ": a target requires the removed feature " e }
        }
        if (drop) { cbuf = ""; return }
        out(cbuf); cbuf = ""; out(ent)
      }
      {
        line = $0 "\n"
        if (inent) { ent = ent line; b += bal($0); if (b <= 0) { inent = 0; finish() }; next }
        if ($0 ~ /^[ \t]*#/) { cbuf = cbuf line; next }
        if ($0 ~ /^[ \t]*$/) { if (!skip) out(cbuf); cbuf = ""; if (!skip) out(line); next }
        if ($0 ~ /^[ \t]*\[/) {
          table = $0; sub(/[ \t]*#.*/, "", table); gsub(/^[ \t]*\[+|\]+[ \t]*$/, "", table); gsub(/[ \t"]/, "", table)
          skip = 0
          if (match(table, /(dependencies|^patch\.[^.]+)\.[^.]+$/)) { x = table; sub(/.*\./, "", x); if (engine(x)) skip = 1 }
          if (skip) { cbuf = ""; next }
          out(cbuf); cbuf = ""; out(line); next
        }
        if (skip) { cbuf = ""; next }
        ent = line; b = bal($0)
        if (b > 0) { inent = 1; next }
        finish()
      }
      END {
        out(cbuf)
        if (err != "") { print substr(err, 2) > "/dev/stderr"; exit 3 }
      }' "$f" > "$WORK/m" || die "a manifest needs a decision by hand (above)"
    if ! cmp -s "$f" "$WORK/m"; then cat "$WORK/m" > "$f"; changed=1; grep -qxF "$f" "$WORK/medited" || echo "$f" >> "$WORK/medited"; fi
  done
  [ $changed = 1 ] || break
done
sed 's/^/  edited /' "$WORK/medited"
[ -s "$WORK/removed" ] && echo "  removed features: $(tr '\n' ' ' < "$WORK/removed")"

# ---- 3. Rust sources ------------------------------------------------------------------------
# strip.awk: one file in, the stripped file out; side lines to $WORK/side as
#   DELETE                 the whole file goes (#![cfg(feature = "F")])
#   MOD <name>             a module declared under the cfg: its files go
#   ERROR <line> <text>    a cfg form the script doesn't handle: stop
cat > "$WORK/strip.awk" <<'AWK'
function idc(ch) { return ch ~ /^[A-Za-z0-9_]$/ }
function skip_line(i,   j) { j = index(substr(S, i), "\n"); return j ? i + j - 1 : N + 1 }
function skip_block(i,   d, j) {
  d = 0; j = i
  while (j <= N) {
    if (substr(S, j, 2) == "/*") { d++; j += 2; continue }
    if (substr(S, j, 2) == "*/") { d--; j += 2; if (d == 0) return j; continue }
    j++
  }
  return N + 1
}
function skip_str(i,   j, ch) {
  j = i + 1
  while (j <= N) { ch = substr(S, j, 1); if (ch == "\\") { j += 2; continue }; if (ch == "\"") return j + 1; j++ }
  return N + 1
}
function raw_at(i,   p, j) {
  if (i > 1) { p = substr(S, i - 1, 1); if (idc(p) && !(p == "b" && (i == 2 || !idc(substr(S, i - 2, 1))))) return 0 }
  j = i + 1; while (substr(S, j, 1) == "#") j++
  return substr(S, j, 1) == "\""
}
function skip_raw(i,   j, h, cl, k) {
  j = i + 1; h = ""; while (substr(S, j, 1) == "#") { h = h "#"; j++ }
  cl = "\"" h; k = index(substr(S, j + 1), cl)
  return k ? j + k + length(cl) : N + 1
}
function skip_char(i,   k, m, ok) {
  if (substr(S, i + 1, 1) == "\\") { k = index(substr(S, i + 3), "'"); return k ? i + 2 + k : N + 1 }
  for (k = 2; k <= 5; k++) if (substr(S, i + k, 1) == "'") {
    if (k == 2) return i + 3
    ok = 1; for (m = 1; m < k; m++) if (substr(S, i + m, 1) < "\200") ok = 0
    if (ok) return i + k + 1
    return i + 1
  }
  return i + 1
}
# the end (position after) of the token or construct at i that isn't code, else 0
function noncode(i,   c) {
  c = substr(S, i, 1)
  if (c == "/" && substr(S, i + 1, 1) == "/") return skip_line(i)
  if (c == "/" && substr(S, i + 1, 1) == "*") return skip_block(i)
  if (c == "\"") return skip_str(i)
  if (c == "r" && raw_at(i)) return skip_raw(i)
  if (c == "'") return skip_char(i)
  return 0
}
function match_close(i,   d, c, e) {   # i at an opening bracket; returns the position after its match
  d = 0
  while (i <= N) {
    e = noncode(i); if (e) { i = e; continue }
    c = substr(S, i, 1)
    if (c ~ /[\[({]/) d++
    else if (c ~ /[\])}]/) { d--; if (d == 0) return i + 1 }
    i++
  }
  return N + 1
}
function line_of(p,   t) { t = substr(S, 1, p); return gsub(/\n/, "", t) + 1 }
function lstart(p,   q) { q = p; while (q > 1 && substr(S, q - 1, 1) != "\n") q--; return q }
function lend(p,   k) { k = index(substr(S, p), "\n"); return k ? p + k : N + 1 }   # after the newline
function blank(s) { return s ~ /^[ \t]*$/ }
# back over whole lines above p (a line start) that are comments (mode 1: also doc comments and
# one-line attributes)
function back(p, mode,   q, t) {
  while (p > 1 && p > floor) {
    q = lstart(p - 1); t = substr(S, q, p - 1 - q)
    if (q < floor) break
    if (t ~ /^[ \t]*\/\/([^\/!]|$)/ || (mode && (t ~ /^[ \t]*\/\/\/([^\/]|$)/ || t ~ /^[ \t]*#\[.*\][ \t]*$/))) p = q
    else break
  }
  return p
}
# the end of the element starting at p (after a removed attribute)
function element_end(p,   q, w, item, d, ang, last, c, e, r, nx) {
  q = p
  while (1) {
    while (substr(S, q, 1) ~ /[ \t\n]/) q++
    e = noncode(q); if (e && substr(S, q, 1) == "/") { q = e; continue }
    if (substr(S, q, 2) == "#[") { q = match_close(q + 1); continue }
    break
  }
  FIRST = q
  w = substr(S, q, 40); match(w, /^[A-Za-z_][A-Za-z0-9_]*/); w = RSTART ? substr(w, 1, RLENGTH) : ""
  item = (w ~ /^(fn|impl|struct|enum|union|trait|mod|use|const|static|type|extern|unsafe|async|macro_rules|let|pub)$/)
  d = 0; ang = 0; last = p - 1; q = p
  while (q <= N) {
    e = noncode(q); if (e) { if (substr(S, q, 1) != "/") last = e - 1; q = e; continue }
    c = substr(S, q, 1)
    if (c !~ /[ \t\n]/) last = q
    if (c ~ /[\[({]/) d++
    else if (c ~ /[\])}]/) {
      if (d == 0) return last + 1
      d--
      if (c == "}" && d == 0) {
        r = q + 1; while (substr(S, r, 1) ~ /[ \t]/) r++
        nx = substr(S, r, 1)
        if (nx == "," || nx == ";") return r + 1
        if (substr(S, r, 4) == "else" && !idc(substr(S, r + 4, 1))) { q = r; continue }
        if (nx == "\n" || nx == "" || substr(S, r, 2) == "//") return q + 1
      }
    } else if (d == 0 && c == ";") return q + 1
    else if (d == 0 && c == "," && !item && ang == 0) return q + 1
    else if (c == "<" && (idc(substr(S, q - 1, 1)) || substr(S, q - 1, 1) == ":") && substr(S, q + 1, 1) !~ /[=<]/) ang++
    else if (c == ">" && ang > 0 && substr(S, q - 1, 1) !~ /[-=]/) ang--
    q++
  }
  return 0
}
function cut(s, e) { R++; RS_[R] = s; RE_[R] = e; floor = e }
function whole_lines(s, e) {   # widen [s, e) to whole lines when only blanks surround it
  if (blank(substr(S, lstart(s), s - lstart(s))) && blank(substr(S, e, lend(e) - 1 - e))) { s = lstart(s); e = lend(e) }
  SS = s; EE = e
}
{ S = S $0 "\n" }
END {
  N = length(S); nf = split(feats, F, " "); R = 0; floor = 0; i = 1
  while (i <= N) {
    e = noncode(i); if (e) { i = e; continue }
    if (substr(S, i, 2) == "#[" || substr(S, i, 3) == "#![") {
      inner = (substr(S, i, 2) == "#!")
      ae = match_close(i + (inner ? 2 : 1))
      A = substr(S, i, ae - i); An = A; gsub(/[ \t\n]/, "", An)
      hit = ""
      for (k = 1; k <= nf; k++) if (index(An, "feature=\"" F[k] "\"")) hit = F[k]
      if (hit == "") { i = i + 2; continue }
      q = "feature=\"" hit "\""
      if (inner) {
        if (An == "#![cfg(" q ")]") { print "DELETE" > side; exit }
        print "ERROR " line_of(i) " " A > side; i = ae; continue
      }
      if (An == "#[cfg(" q ")]" || (An ~ /^#\[cfg\(all\(/ && An !~ /(not|any)\(/)) {
        s = i
        if (blank(substr(S, lstart(i), i - lstart(i)))) s = back(lstart(i), 1)
        e = element_end(ae)
        if (!e) { print "ERROR " line_of(i) " no end for the element under " A > side; i = ae; continue }
        if (substr(S, e - 1, 1) == ";" && match(substr(S, FIRST, e - FIRST), /^(pub(\([a-z: ]+\))?[ \t]+)?mod[ \t]+[A-Za-z0-9_]+[ \t]*;$/)) {
          m = substr(S, FIRST, e - FIRST); sub(/;$/, "", m); sub(/^.*mod[ \t]+/, "", m); sub(/[ \t]+$/, "", m)
          if (substr(S, s, FIRST - s) ~ /#\[path/) print "ERROR " line_of(i) " a #[path] module under " A > side
          else print "MOD " m > side
        }
        whole_lines(s, e); cut(SS, EE); i = EE; continue
      }
      if (An == "#[cfg(not(" q "))]" || An ~ ("^#\\[cfg_attr\\(" q ",")) {
        whole_lines(i, ae); s = SS; e = EE
        if (s == lstart(i) && An ~ /^#\[cfg_attr/) s = back(s, 0)
        cut(s, e); i = e; continue
      }
      print "ERROR " line_of(i) " " A > side; i = ae; continue
    }
    i++
  }
  # the output, each whole-line cut as a marker line, then the blank-line junctions fixed
  o = ""; p = 1
  for (r = 1; r <= R; r++) {
    o = o substr(S, p, RS_[r] - p)
    if ((RS_[r] == 1 || substr(S, RS_[r] - 1, 1) == "\n") && substr(S, RE_[r] - 1, 1) == "\n") o = o "\001CUT\001\n"
    p = RE_[r]
  }
  o = o substr(S, p)
  n = split(o, L, "\n"); if (L[n] == "") n--
  k = 0; pend = 0
  for (j = 1; j <= n; j++) {
    if (L[j] == "\001CUT\001") { pend = 1; continue }
    if (pend) {
      pend = 0
      P = k ? O[k] : ""
      if ((k == 0 || blank(P) || P ~ /\{[ \t]*$/) && blank(L[j])) continue
      if (k && blank(P) && L[j] ~ /^[ \t]*[\])}]/) k--
    }
    O[++k] = L[j]
  }
  if (pend && k && blank(O[k])) k--
  for (j = 1; j <= k; j++) print O[j]
}
AWK
# the features whose cfgs go in the .rs file $1: NATIVE_FEATURES and the ones step 2 removed
# from its crate's manifest (the nearest Cargo.toml above it with a [package])
strip_feats() {
  d=$(dirname "$1"); p=
  while :; do
    if [ -f "$d/Cargo.toml" ] && grep -q '^\[package\]' "$d/Cargo.toml"; then p=$(pkgname "$d/Cargo.toml"); break; fi
    [ "$d" = . ] && break
    d=$(dirname "$d")
  done
  { for x in $NATIVE_FEATURES; do echo "$x"; done; [ -z "$p" ] || sed -n "s|^$p/||p" "$WORK/removed"; } | sort -u | tr '\n' ' '
}
for f in $(git ls-files '*.rs' | grep -v '^vendor/'); do
  [ -f "$f" ] || continue
  grep -q 'feature[[:space:]]*=' "$f" || continue
  SF=$(strip_feats "$f")
  FEATRE=$(echo $SF | sed 's/ /|/g')
  grep -qE "feature[[:space:]]*=[[:space:]]*\"($FEATRE)\"" "$f" || continue
  : > "$WORK/side"
  awk -v feats="$SF" -v side="$WORK/side" -f "$WORK/strip.awk" "$f" > "$WORK/r"
  if grep -q '^ERROR' "$WORK/side"; then
    sed "s|^ERROR |public-cut: $f:|; s|\$| (a cfg form this script doesn't handle: extend it, don't guess)|" "$WORK/side" >&2
    exit 1
  fi
  if grep -qx DELETE "$WORK/side"; then git rm -q -f -- "$f"; echo "  removed $f (#![cfg] on a removed feature)"; continue; fi
  replace "$f" "$WORK/r"
  case $(basename "$f") in mod.rs|lib.rs|main.rs) dir=$(dirname "$f") ;; *) dir=${f%.rs} ;; esac
  for m in $(sed -n 's/^MOD //p' "$WORK/side"); do
    for t in "$dir/$m.rs" "$dir/$m"; do
      if [ -n "$(git ls-files "$t")" ]; then git rm -r -q -f -- "$t"; echo "  removed $t (module under the cfg)"; fi
    done
  done
done

# ---- 4. tooling -----------------------------------------------------------------------------
for f in $(git ls-files 'tools/*.sh' 'scripts/*.sh' 'tools/**/*.sh' 'scripts/**/*.sh' | sort -u); do
  [ -f "$f" ] || continue
  [ "$f" = tools/public-cut.sh ] && continue
  [ "$f" = tools/check-public-cut.sh ] && continue
  awk -v parts="$DROPPED_PARTS" -v pkgs="$DROPPKGS" -v f="$f" '
    { L[NR] = $0 }
    END {
      np = split(parts, P, " "); nk = split(pkgs, K, " ")
      alt = ""; for (i = 1; i <= np; i++) alt = alt (i > 1 ? "|" : "") P[i]
      d = 0
      for (i = 1; i <= NR; i++) {
        if (L[i] ~ /^[ \t]*# public-cut: drop-begin/) { if (d) { print f ":" i ": nested drop-begin" > "/dev/stderr"; exit 3 }; d = i }
        if (d) gone[i] = 1
        if (L[i] ~ /^[ \t]*# public-cut: drop-end/) { if (!d) { print f ":" i ": drop-end without drop-begin" > "/dev/stderr"; exit 3 }; d = 0 }
      }
      if (d) { print f ":" d ": drop-begin without drop-end" > "/dev/stderr"; exit 3 }
      for (i = 1; i <= NR; i++) {
        if (gone[i]) continue
        if (L[i] ~ ("^[ \t]+(" alt ")\\)")) {
          for (j = i; j <= NR && L[j] !~ /;;[ \t]*(#.*)?$/; j++) ;
          if (j > NR) { print f ":" i ": a case arm with no ;;" > "/dev/stderr"; exit 3 }
          for (x = i; x <= j; x++) gone[x] = 1
          for (x = i - 1; x >= 1 && !gone[x] && L[x] ~ /^[ \t]*#/; x--) gone[x] = 1
          i = j; continue
        }
        if (L[i] ~ ("^#   (" alt ")  ")) {
          gone[i] = 1
          for (x = i + 1; x <= NR && L[x] ~ /^#      +[^ ]/; x++) gone[x] = 1
        }
      }
      for (i = 1; i <= NR; i++) {
        if (gone[i]) continue
        s = L[i]
        for (k = 1; k <= nk; k++) gsub("[ \t]+-p[ \t]+" K[k] "([ \t]|$)", " ", s)
        if (s != L[i]) sub(/[ \t]+$/, "", s)
        if (s ~ /\$\{[A-Za-z0-9_]+:\?/) for (k = 1; k <= np; k++) gsub(" " P[k] ",", "", s)
        print s
      }
    }' "$f" > "$WORK/t" || die "$f needs a decision by hand (above)"
  replace "$f" "$WORK/t"
done

# the marker blocks of Markdown files and manifests
for f in $(git ls-files '*.md' '*Cargo.toml' | grep -v '^vendor/'); do
  [ -f "$f" ] || continue
  grep -qE '^[[:space:]]*(#|<!--) public-cut: drop-' "$f" || continue
  awk -v f="$f" '
    /^[ \t]*(#|<!--) public-cut: drop-begin/ { if (d) { print f ":" NR ": nested drop-begin" > "/dev/stderr"; bad = 1; exit 3 }; d = NR; next }
    /^[ \t]*(#|<!--) public-cut: drop-end/ { if (!d) { print f ":" NR ": drop-end without drop-begin" > "/dev/stderr"; bad = 1; exit 3 }; d = 0; next }
    !d { print }
    END { if (bad) exit 3; if (d) { print f ":" d ": drop-begin without drop-end" > "/dev/stderr"; exit 3 } }' "$f" > "$WORK/t" || die "$f needs a decision by hand (above)"
  replace "$f" "$WORK/t"
done

# ---- 5. the README paragraph -----------------------------------------------------------------
NOTE='**Public cut.** This tree is a release'"'"'s public cut (`tools/public-cut.sh`): the native engine'"'"'s vendored source, the kit'"'"'s adapter to it (feature `native`) and the engine-generic conformance suite (`crates/conformance`) are not in it. Docs and the CHANGELOG may still name them.'
if [ -f README.md ] && ! grep -qF '**Public cut.**' README.md; then
  printf '\n%s\n' "$NOTE" >> README.md; echo "  edited README.md (the public-cut paragraph)"
fi

# ---- 6. the lockfile ------------------------------------------------------------------------
# the workspace's members' package names (the roots of the lock)
roots() {   # $1: the directory of a Cargo.lock
  (
    cd "$1"
    awk '/^\[package\]/{s=1;next} /^\[/{s=0} s && /^name[ \t]*=/{gsub(/.*=[ \t]*"|".*/,""); print; exit}' Cargo.toml
    for m in $(awk '/^\[workspace\]/{w=1;next} /^\[/{w=0} w && /^members[ \t]*=/{f=1} f{print; if (/\]/) exit}' Cargo.toml | sed 's/.*=//' | tr -d '[]",' ); do
      case $m in *'*'*) echo "public-cut: $1/Cargo.toml: a glob member ($m): list members by path" >&2; exit 3 ;; esac
      awk '/^\[package\]/{s=1;next} /^\[/{s=0} s && /^name[ \t]*=/{gsub(/.*=[ \t]*"|".*/,""); print; exit}' "$m/Cargo.toml"
    done
  )
}
for lf in $(git ls-files '*Cargo.lock' | grep -v '^vendor/'); do
  [ -f "$lf" ] || continue
  DROP=
  for n in $ENGINE $DROPPKGS $(grep -oE '^name = "mpce[-_][A-Za-z0-9_-]*"' "$lf" | sed 's/.*"\(.*\)"/\1/'); do
    is_public "$n" && continue
    grep -qx "name = \"$n\"" "$lf" && DROP="$DROP $n"
  done
  [ -n "$DROP" ] || continue
  if [ -n "$LOCKFILE" ] && [ "$lf" = Cargo.lock ]; then
    # a prune only removes, but for a dependency reference cargo shortens once it is
    # unambiguous ("name 1.2.3" to "name"): compare with every reference shortened to its name
    sed 's/^ "\([^ "]*\)[^"]*",$/ "\1",/' "$lf" > "$WORK/la"
    sed 's/^ "\([^ "]*\)[^"]*",$/ "\1",/' "$LOCKFILE" > "$WORK/lb"
    diff "$WORK/la" "$WORK/lb" | grep -q '^>' && die "--lock $LOCKFILE adds lines to $lf: refused (a prune only removes)"
    cat "$LOCKFILE" > "$lf"; echo "  replaced $lf from $LOCKFILE"; continue
  fi
  R=$(roots "$(dirname "$lf")") || die "no roots for $lf"
  awk -v drop="$DROP" -v roots="$(echo $R)" '
    function flush() { if (cur) { sub(/\n+$/, "", blk); np++; T[np] = blk; NM[np] = nm; VR[np] = vr; SR[np] = sr; ND[np] = nd; for (z = 1; z <= nd; z++) DP[np, z] = dl[z] }; cur = 0; blk = ""; nm = vr = sr = ""; nd = 0 }
    /^\[\[package\]\]/ { flush(); cur = 1; blk = $0 "\n"; next }
    /^\[/ && !/^\[\[package\]\]/ { flush(); tail = tail $0 "\n"; intail = 1; next }
    intail { tail = tail $0 "\n"; next }
    !cur { head = head $0 "\n"; next }
    {
      if ($0 ~ /^name = /) { nm = $0; gsub(/^name = "|"$/, "", nm) }
      if ($0 ~ /^version = /) { vr = $0; gsub(/^version = "|"$/, "", vr) }
      if ($0 ~ /^source = /) { sr = $0; gsub(/^source = "|"$/, "", sr) }
      if ($0 ~ /^replace = /) { print "public-cut: a lock entry with replace: prune it by hand" > "/dev/stderr"; bad = 1 }
      if ($0 ~ /^dependencies = \[/) { indeps = 1; next }
      if (indeps) { if ($0 ~ /^\]/) { indeps = 0; next }; d = $0; gsub(/^ *"|",?$/, "", d); dl[++nd] = d; next }
      blk = blk $0 "\n"
    }
    function resolve(ref,   a, k, z, hit, cnt) {
      k = split(ref, a, " "); cnt = 0
      for (z = 1; z <= np; z++) {
        if (NM[z] != a[1]) continue
        if (k >= 2 && VR[z] != a[2]) continue
        if (k >= 3 && "(" SR[z] ")" != a[3]) continue
        hit = z; cnt++
      }
      return cnt == 1 ? hit : 0
    }
    END {
      flush()
      if (bad) exit 3
      sub(/\n+$/, "", head)
      nd0 = split(drop, X, " "); for (z = 1; z <= nd0; z++) gone[X[z]] = 1
      nr = split(roots, ROOTS, " ")
      for (z = 1; z <= np; z++) for (y = 1; y <= nr; y++) if (NM[z] == ROOTS[y] && SR[z] == "") { keep[z] = 1; Q[++qn] = z }
      if (!qn) { print "public-cut: no root package found in the lock" > "/dev/stderr"; exit 3 }
      for (qi = 1; qi <= qn; qi++) {
        z = Q[qi]
        for (y = 1; y <= ND[z]; y++) {
          t = resolve(DP[z, y])
          if (!t) { print "public-cut: an unresolved lock reference: " DP[z, y] > "/dev/stderr"; exit 3 }
          if (gone[NM[t]]) continue
          if (!keep[t]) { keep[t] = 1; Q[++qn] = t }
        }
      }
      for (z = 1; z <= np; z++) if (keep[z]) { byname[NM[z]]++; bynv[NM[z] " " VR[z]]++ }
      printf "%s\n", head
      for (z = 1; z <= np; z++) {
        if (!keep[z]) continue
        printf "\n%s\n", T[z]
        nk = 0
        for (y = 1; y <= ND[z]; y++) {
          t = resolve(DP[z, y]); if (!keep[t]) continue
          if (byname[NM[t]] == 1) r = NM[t]; else if (bynv[NM[t] " " VR[t]] == 1) r = NM[t] " " VR[t]; else r = NM[t] " " VR[t] " (" SR[t] ")"
          K[++nk] = r
        }
        if (nk) { print "dependencies = ["; for (y = 1; y <= nk; y++) print " \"" K[y] "\","; print "]" }
      }
      if (tail != "") printf "\n%s", tail
    }' "$lf" > "$WORK/l" || die "the prune of $lf failed (above)"
  replace "$lf" "$WORK/l"
done

# ---- 7. checks and the commit ---------------------------------------------------------------
git add -u
if git diff --cached --quiet; then
  echo "public-cut: nothing to cut (the tree is already a public cut)"
  sh tools/check-public-cut.sh .
  exit 0
fi
echo "public-cut: the cut:"
git diff --cached --stat | tail -1
sh tools/check-public-cut.sh . || { echo "public-cut: checks failed: no commit (the edits are staged; git reset --hard to drop them)" >&2; exit 1; }
if [ "$NOCOMMIT" = 1 ]; then echo "public-cut: --no-commit: staged, not committed"; exit 0; fi
MSG=$WORK/msg
{
  echo "release: public cut"
  echo
  echo "tools/public-cut.sh on $(git rev-parse --short HEAD): the engine's vendored source, the"
  echo "kit's native-engine adapter (features: $NATIVE_FEATURES) and the crates"
  echo "$DROPPATHS and the trees $DROPPED_TREES removed; the lockfile pruned (packages removed, no"
  echo "version changed)."
  echo "Never merged into main."
  echo
  git diff --cached --stat | tail -1
} > "$MSG"
set --
if [ -n "$TRAILERS" ]; then
  while IFS= read -r t; do [ -n "$t" ] && set -- "$@" --trailer "$t"; done <<EOF
$TRAILERS
EOF
fi
git commit -q -F "$MSG" "$@"
echo "public-cut: committed $(git rev-parse --short HEAD)"
