# The public-cut gate: a commands file for tools/job.sh, run at a release's CUT commit (the
# `release: public cut` commit tools/public-cut.sh makes; its parent is the pre-cut release
# commit). One part per job, chosen by PART (tools/README.md, "The public cut"):
#   k2 compute run <node> --repo <repo> --sha <cut> --needs rust@stable+clippy+rustfmt,cargo-nextest \
#     --cpus 4 --mem 16G --env PART=checker -- sh tools/job.sh tools/tests/public-cut-gate.sh
# Parts:
#   lint base-a1 base-a2 base-b base-c base-d errors spec-undo wire-guard evidence
#                tools/tests/full.sh's parts, on the cut (evidence includes oss)
#   licence      scripts/check-licences.sh, and no mpce crate in the kit's or the workspace's tree
#   lockproof    cargo's own resolution (no --locked) leaves the cut's pruned Cargo.lock unchanged
#   checker      tools/check-public-cut.sh: the cut is clean; planted copies of it are blocked,
#                the text rule's each at exactly its finding, with the node's awk (which gates)
#                and every other awk the node has (informational; never fails the part)
#   checker-precut  the pre-cut commit is blocked, naming what the cut removes; public-cut.sh on
#                planted copies of it drops derived engine-only features and stops on a mixed one
#   reproduce    public-cut.sh run here on the pre-cut commit makes exactly the cut's tree (the
#                same git tree hash) with the node's awk, which gates; every other awk the node
#                has is run too, informational only (never fails the part)
# Every part also reruns check-public-cut.sh on the cut (but checker-precut, whose subject must
# fail it), and checks Cargo.lock unchanged and the tree clean after. Every cargo step inside the
# repo gets --locked (a PATH shim; a caller passing --locked or --frozen itself gets nothing
# added). Planted and reproduced trees are `git archive` copies in GATE_TMP (default: beside the
# checkout, in the job's own directory), each with its own `git init` when the script needs one:
# never the checkout or its repository (a copy reads the repository's objects, through an alternate,
# for the history the text rule needs). Env: PART; EXPECT (optional: the sha the job must be at).
# The last line is `=== RESULT <part>: GREEN|RED`; the job fails with any step.
echo "=== public-cut gate part ${PART:-none}"
H=$(git rev-parse HEAD 2>/dev/null || echo none)
if [ -n "${EXPECT:-}" ]; then
  if [ "$H" = "$EXPECT" ]; then echo "sha ok $H"; else echo "sha MISMATCH: $H, expected $EXPECT"; RC=1; fi
else
  echo "at $H"
fi
REPO=$PWD
GATE_TMP=${GATE_TMP:-$REPO/../public-cut-gate}
mkdir -p "$GATE_TMP"
if [ "$(git log -1 --format=%s HEAD 2>/dev/null)" != "release: public cut" ]; then
  echo "HEAD is not a public cut (subject: $(git log -1 --format=%s HEAD 2>/dev/null)): run the gate at the cut commit"; RC=1
fi

# cargo: --locked for every build step inside the repo
SHIM=$GATE_TMP/cargo-shim
mkdir -p "$SHIM"
REAL=$(command -v cargo || echo cargo)
cat > "$SHIM/cargo" <<EOS
#!/bin/sh
for a in "\$@"; do case "\$a" in --locked|--frozen) exec "$REAL" "\$@" ;; esac; done
case "\$PWD/" in
  "$REPO"/*)
    case "\$1" in
      build|test|check|clippy|tree|metadata|run|doc|bench) sub=\$1; shift; exec "$REAL" "\$sub" --locked "\$@" ;;
      nextest) if [ "\$2" = run ]; then shift 2; exec "$REAL" nextest run --locked "\$@"; fi ;;
    esac ;;
esac
exec "$REAL" "\$@"
EOS
chmod +x "$SHIM/cargo"
export PATH="$SHIM:$PATH"
# The target dir stays the runner's, as for tools/tests/full.sh: never K2_JOB_CACHE, whatever the
# job's --cache. Parts running side by side in one shared target reuse each other's test binaries,
# which hold the building job's checkout path (env!("CARGO_MANIFEST_DIR")), gone when it ends.
echo "target: ${CARGO_TARGET_DIR:-./target}"

# copy COMMIT DIR [git]: COMMIT's tree into DIR (a git archive), with its own repository and one
# commit on branch `gate`, a child of COMMIT with the same tree, when the third argument is `git`.
# COMMIT's history is read through an alternate, so check-public-cut.sh's text rule runs on it there
# as it does at the cut. Nothing is written to the checkout's repository.
copy() {
  mkdir -p "$2" && git archive "$1" | tar -x -C "$2" || { echo "SETUP FAILED: archive of $1"; return 1; }
  [ "${3:-}" = git ] || return 0
  par=$(git rev-parse --verify "$1^{commit}") && obj=$(cd "$(git rev-parse --git-path objects)" && pwd) \
    || { echo "SETUP FAILED: $1 or the objects directory not found"; return 1; }
  ( cd "$2" && git init -q -b gate && mkdir -p .git/objects/info && echo "$obj" > .git/objects/info/alternates && git add -A \
      && c=$(git -c user.name=gate -c user.email=gate@invalid commit-tree "$(git write-tree)" -p "$par" -m "gate copy") \
      && git update-ref HEAD "$c" ) \
    || { echo "SETUP FAILED: git init in $2"; return 1; }
  [ -z "$(cd "$2" && git status --porcelain)" ] || { echo "SETUP FAILED: $2 not clean"; return 1; }
}
# expect_block LABEL DIR PATTERN...: check-public-cut.sh on DIR exits 1 and names every PATTERN
expect_block() {
  lbl=$1; d=$2; shift 2
  out=$(sh "$d/tools/check-public-cut.sh" "$d" 2>&1); r=$?
  printf '%s\n' "$out" | grep -v '^check-public-cut: sources:' | grep -v '^check-public-cut: paths: vendor/mpce-[a-z]*/' | head -60
  echo "(sources findings: $(printf '%s\n' "$out" | grep -c '^check-public-cut: sources:'); vendor file findings: $(printf '%s\n' "$out" | grep -c '^check-public-cut: paths: vendor/mpce-[a-z]*/'))"
  printf '%s\n' "$out" | tail -1
  [ "$r" -eq 1 ] || { echo "PLANT $lbl: exit $r, expected 1"; return 1; }
  for p in "$@"; do printf '%s\n' "$out" | grep -qF -- "$p" || { echo "PLANT $lbl: not named: $p"; return 1; }; done
  echo "PLANT $lbl: blocked, every expected finding named"
}
# a plant under crates/base/Cargo.toml's [features] (lines separated by |)
under_features() { awk -v ins="$1" '{ print } /^\[features\]/ { n = split(ins, L, "|"); for (i = 1; i <= n; i++) print L[i] }' "$2"; }

# The text and dropped rules' plants. TP is a copy with its own base: commit X, then a commit recording X, so
# the only added lines are a plant's and every plant's finding is the only one. The terms and trees
# are read from the checker: this file must pass both rules too, so it doesn't spell them.
# expect_text LABEL DIR EXIT LINE...: check-public-cut.sh on DIR exits EXIT, and its findings are
# exactly the LINEs (whole lines); with EXIT 0, it says clean.
expect_text() {
  lbl=$1; d=$2; want=$3; shift 3
  out=$(sh "$d/tools/check-public-cut.sh" "$d" 2>&1); r=$?
  printf '%s\n' "$out" | sed "s|^|  [$lbl] |"
  [ "$r" -eq "$want" ] || { echo "PLANT $lbl: exit $r, expected $want"; return 1; }
  n=$(printf '%s\n' "$out" | grep '^check-public-cut: ' | grep -vc '^check-public-cut: \(BLOCKED\|clean\)')
  [ "$n" -eq $# ] || { echo "PLANT $lbl: $n finding(s), expected $#"; return 1; }
  for p in "$@"; do printf '%s\n' "$out" | grep -qxF -- "$p" || { echo "PLANT $lbl: no line: $p"; return 1; }; done
  if [ "$want" -eq 0 ]; then printf '%s\n' "$out" | grep -q '^check-public-cut: clean' || { echo "PLANT $lbl: not clean"; return 1; }; fi
  echo "PLANT $lbl: exit $r, exactly the expected finding(s)"
}
tp_commit() { ( cd "$D" && git add -A && git -c user.name=gate -c user.email=gate@invalid -c commit.gpgsign=false commit -qm "$1" ) || { echo "SETUP FAILED: commit in $D"; return 1; }; }
tp_new() { D=$GATE_TMP/text-$1; rm -rf "$D"; cp -R "$TP" "$D" || { echo "SETUP FAILED: copy to $D"; return 1; }; }
# tp_line FILE TEXT: append a line to FILE; L is its number
tp_line() { printf '%s\n' "$2" >> "$D/$1" && L=$(wc -l < "$D/$1" | tr -d ' '); }
tp_rebase() { printf '%s\n' "$(cd "$D" && git rev-parse HEAD)" > "$D/$TEXT_BASE"; }
text_start() {
  TP=$GATE_TMP/text-start; rm -rf "$TP"
  copy HEAD "$TP" git || return 1
  eval "$(sh "$TP/tools/check-public-cut.sh" --print-config)"
  TA=$(printf '%s\n' "$TEXT_TERMS" | tr '|' '\n' | awk 'NF == 1' | sed -n 1p)
  TB=$(printf '%s\n' "$TEXT_TERMS" | tr '|' '\n' | awk 'NF == 1' | sed -n 2p)
  TM=$(printf '%s\n' "$TEXT_TERMS" | tr '|' '\n' | awk 'NF == 2' | sed -n 1p)
  [ -n "$TA" ] && [ -n "$TB" ] && [ -n "$TM" ] || { echo "SETUP FAILED: terms not read from the checker"; return 1; }
  T1=${DROPPED_TREES%% *}; T2=${DROPPED_TREES#* }
  [ -n "$T1" ] && [ -n "$T2" ] && [ "$T1" != "$T2" ] || { echo "SETUP FAILED: two trees not read from the checker"; return 1; }
  D=$TP
  printf '# none\n' > "$D/$TEXT_EXCEPTIONS"
  tp_commit "base" && tp_rebase && tp_commit "base recorded" || return 1
  expect_text start "$D" 0
}
FIND="check-public-cut: text:"
RECORD="record the previous host release's pre-cut commit (tools/README.md, \"The public cut\")"
UPDATE="update it to the previous host release's pre-cut commit (tools/README.md, \"The public cut\")"
tp_md() { tp_new md && tp_line docs/POSITIONS.md "A planted line with $TA in it." && tp_commit plant && expect_text md "$D" 1 "$FIND docs/POSITIONS.md:$L: $TA"; }
tp_comment() { tp_new comment && tp_line crates/base/src/lib.rs "// a planted comment: $TB" && tp_commit plant && expect_text comment "$D" 1 "$FIND crates/base/src/lib.rs:$L: $TB"; }
tp_test_name() { tp_new test-name && tp_line crates/base/src/lib.rs "fn $(printf '%s' "$TM" | tr ' ' _)_planted() {}" && tp_commit plant && expect_text test-name "$D" 1 "$FIND crates/base/src/lib.rs:$L: $TM"; }
tp_wrap() {
  tp_new wrap && tp_line docs/POSITIONS.md "A planted line ending ${TM%% *}" && tp_line docs/POSITIONS.md "${TM#* } and the next." && tp_commit plant \
    && expect_text wrap "$D" 1 "$FIND docs/POSITIONS.md:$L: $TM"
}
# the same line is blocked while it is added, and passes once the base has it: a line beside it added
tp_not_added() {
  tp_new not-added && tp_line docs/POSITIONS.md "A planted line with $TA in it." && N=$L && tp_commit plant \
    && expect_text not-added-while-added "$D" 1 "$FIND docs/POSITIONS.md:$N: $TA" \
    && tp_rebase && tp_line docs/POSITIONS.md "A plain line beside it." && tp_commit "base moved" \
    && [ "$(sed -n "${N}p" "$D/docs/POSITIONS.md")" = "A planted line with $TA in it." ] \
    && expect_text not-added "$D" 0
}
# an entry passes its own line, and not another line of that file with the same term
tp_exception() {
  tp_new exception && tp_line docs/POSITIONS.md "The reviewed line: $TA." && tp_line docs/POSITIONS.md "Another line: $TA." \
    && printf 'docs/POSITIONS.md: The reviewed line: %s.\n' "$TA" >> "$D/$TEXT_EXCEPTIONS" && tp_commit plant \
    && expect_text exception-other-line "$D" 1 "$FIND docs/POSITIONS.md:$L: $TA" \
    && sed '$d' "$D/docs/POSITIONS.md" > "$D/positions.tmp" && mv "$D/positions.tmp" "$D/docs/POSITIONS.md" && tp_commit "only the reviewed line" \
    && expect_text exception-covered "$D" 0
}
tp_exception_stale() {
  tp_new exception-stale && tp_line "$TEXT_EXCEPTIONS" "docs/POSITIONS.md: a line nobody added" && tp_commit plant \
    && expect_text exception-stale "$D" 1 "$FIND $TEXT_EXCEPTIONS:$L: no added line with a term matches this entry"
}
tp_own_script() { tp_new own-script && tp_line tools/check-public-cut.sh "# a planted comment: $TA" && tp_commit plant && expect_text own-script "$D" 1 "$FIND tools/check-public-cut.sh:$L: $TA"; }
tp_base_missing() { tp_new base-missing && rm -f "$D/$TEXT_BASE" && tp_commit plant && expect_text base-missing "$D" 1 "$FIND $TEXT_BASE: missing or not tracked: $RECORD"; }
tp_base_short() { tp_new base-short && printf '%s\n' abc123 > "$D/$TEXT_BASE" && tp_commit plant && expect_text base-short "$D" 1 "$FIND $TEXT_BASE: not one full commit sha: $RECORD"; }
# a commit of this repository that HEAD doesn't descend from
tp_base_foreign() {
  tp_new base-foreign && side=$(cd "$D" && git -c user.name=gate -c user.email=gate@invalid commit-tree 'HEAD^{tree}' -m side) && [ -n "$side" ] \
    && printf '%s\n' "$side" > "$D/$TEXT_BASE" && tp_commit plant \
    && expect_text base-foreign "$D" 1 "$FIND $TEXT_BASE: $side is not an ancestor of HEAD: $UPDATE"
}
# the removed trees: a file under each, and references to them, read from the checker's config
tp_dropped_file() {
  tp_new dropped-file && mkdir -p "$D/$T1" && tp_line "$T1/planted.rs" "// planted" && tp_commit plant \
    && expect_text dropped-file "$D" 1 "check-public-cut: dropped: $T1/planted.rs (under $T1, which the cut removes)"
}
tp_dropped_file2() {
  tp_new dropped-file2 && mkdir -p "$D/$T2" && tp_line "$T2/planted.sh" "# planted" && tp_commit plant \
    && expect_text dropped-file2 "$D" 1 "check-public-cut: dropped: $T2/planted.sh (under $T2, which the cut removes)"
}
tp_dropped_untracked() {
  tp_new dropped-untracked && mkdir -p "$D/$T1" && tp_line "$T1/planted.txt" "planted" \
    && (cd "$D" && git status --porcelain --untracked-files=all | grep -q planted.txt) \
    && expect_text dropped-untracked "$D" 1 "check-public-cut: dropped: $T1 exists (a tree the cut removes)"
}
tp_dropped_ref_doc() { tp_new dropped-ref-doc && tp_line docs/POSITIONS.md "Run sh $T1/planted.sh to see it." && tp_commit plant && expect_text dropped-ref-doc "$D" 1 "check-public-cut: dropped: docs/POSITIONS.md:$L: names a tree the cut removes"; }
tp_dropped_ref_script() { tp_new dropped-ref-script && tp_line tools/test-bundle.sh "python3 \$ROOT/$T2/planted.py" && tp_commit plant && expect_text dropped-ref-script "$D" 1 "check-public-cut: dropped: tools/test-bundle.sh:$L: names a tree the cut removes"; }
tp_dropped_ref_manifest() {
  tp_new dropped-ref-manifest && awk -v t="$T1" '{ print } /^exclude = \[/ { print "  \"" t "\"," }' "$TP/Cargo.toml" > "$D/Cargo.toml" && tp_commit plant \
    && L=$(grep -n "^  \"$T1\",\$" "$D/Cargo.toml" | cut -d: -f1) && [ -n "$L" ] \
    && expect_text dropped-ref-manifest "$D" 1 "check-public-cut: dropped: Cargo.toml:$L: names a tree the cut removes"
}
tp_dropped_ref_changelog() { tp_new dropped-ref-changelog && tp_line CHANGELOG.md "A planted note on $T1/planted.rs." && tp_commit plant && expect_text dropped-ref-changelog "$D" 0; }
tp_marker_left() {
  m="<!-- public-cut: drop-""begin -->"
  tp_new marker-left && tp_line docs/POSITIONS.md "$m" && tp_commit plant \
    && expect_text marker-left "$D" 1 "check-public-cut: tooling: docs/POSITIONS.md:$L:$m (a marker the cut must drop)"
}
# the text rule reads prose: code tokens pass, their words in a comment, a string, a test name or a
# .md block (TK, TO and TU are spelled in pieces, as this file must pass the rule too)
TK=pan''ic; TO=over''flow; TU="us""ed to"; RSF=crates/base/src/lib.rs; TSF=client/src/hunk.ts
tp_code_passes() {
  tp_new code-passes && tp_line "$RSF" "fn planted_call() { $TK!(\"x\"); }" && tp_line "$RSF" "#[should_$TK]" && tp_line "$RSF" "fn planted_path() { let _ = std::$TK::catch_unwind(|| 1u8.${TO}ing_add(1)); }" \
    && tp_line "$RSF" "// a planted comment" && tp_line "$TSF" "const planted = e.style.$TO;" && tp_commit plant && expect_text code-passes "$D" 0
}
tp_code_comment() {
  tp_new code-comment && tp_line "$RSF" "// this $TU $TK" && tp_commit plant \
    && expect_text code-comment "$D" 1 "$FIND $RSF:$L: $TU" "$FIND $RSF:$L: $TK"
}
# the macro's token goes, and what its string says still counts: not the term $TK, the other two
tp_code_string() {
  tp_new code-string && tp_line "$RSF" "fn planted_call() { $TK!(\"this $TU $TB\"); }" && tp_commit plant \
    && expect_text code-string "$D" 1 "$FIND $RSF:$L: $TU" "$FIND $RSF:$L: $TB"
}
tp_code_string_token() { tp_new code-string-token && tp_line "$RSF" "const PLANTED: &str = \"see $TK!(\";" && tp_commit plant && expect_text code-string-token "$D" 1 "$FIND $RSF:$L: $TK"; }
tp_code_ts_string() { tp_new code-ts-string && tp_line "$TSF" "const planted = 'a $TK!( here';" && tp_commit plant && expect_text code-ts-string "$D" 1 "$FIND $TSF:$L: $TK"; }
tp_code_md() { tp_new code-md && tp_line docs/POSITIONS.md "$TK!(\"x\")" && tp_commit plant && expect_text code-md "$D" 1 "$FIND docs/POSITIONS.md:$L: $TK"; }
tp_code_test_name() { tp_new code-test-name && tp_line "$RSF" "fn ${TK}s_on_planted() {}" && tp_commit plant && expect_text code-test-name "$D" 1 "$FIND $RSF:$L: $TK"; }
tp_code_plain_word() { tp_new code-plain-word && tp_line "$RSF" "    and a $TO in a wrapped literal\"," && tp_commit plant && expect_text code-plain-word "$D" 1 "$FIND $RSF:$L: $TO"; }
# text_plants STEP: every plant, each run by STEP LABEL CMD (timed gates, info_step doesn't)
text_plants() {
  "$1" plant_text_md tp_md; "$1" plant_text_comment tp_comment; "$1" plant_text_test_name tp_test_name; "$1" plant_text_wrap tp_wrap
  "$1" plant_text_not_added tp_not_added; "$1" plant_text_exception tp_exception; "$1" plant_text_exception_stale tp_exception_stale
  "$1" plant_text_own_script tp_own_script; "$1" plant_text_base_missing tp_base_missing; "$1" plant_text_base_short tp_base_short
  "$1" plant_text_base_foreign tp_base_foreign
  "$1" plant_dropped_file tp_dropped_file; "$1" plant_dropped_file2 tp_dropped_file2; "$1" plant_dropped_untracked tp_dropped_untracked
  "$1" plant_dropped_ref_doc tp_dropped_ref_doc; "$1" plant_dropped_ref_script tp_dropped_ref_script; "$1" plant_dropped_ref_manifest tp_dropped_ref_manifest
  "$1" plant_dropped_ref_changelog tp_dropped_ref_changelog; "$1" plant_marker_left tp_marker_left
  "$1" plant_code_passes tp_code_passes; "$1" plant_code_comment tp_code_comment; "$1" plant_code_string tp_code_string
  "$1" plant_code_string_token tp_code_string_token; "$1" plant_code_ts_string tp_code_ts_string; "$1" plant_code_md tp_code_md
  "$1" plant_code_test_name tp_code_test_name; "$1" plant_code_plain_word tp_code_plain_word
}
info_step() {
  l=$1; shift
  o=$(set +e; "$@" 2>&1); r=$?
  if [ "$r" -eq 0 ]; then INFO_OK=$((INFO_OK + 1)); else INFO_BAD=$((INFO_BAD + 1)); printf '%s\n' "$o" | tail -20; echo "INFO: $l: exit $r"; fi
}
# every other awk the node has, on the same plants; never gating, and never fatal (an || list
# turns set -e off), one verdict line each
info_text_awks() {
  seen=" $(readlink -f "$(command -v awk)" 2>/dev/null) "
  for alt in gawk mawk original-awk nawk; do
    a=$(command -v "$alt" 2>/dev/null) || continue
    t=$(readlink -f "$a" 2>/dev/null)
    case $seen in *" $t "*) echo "INFO: $alt: $t, a binary already run: skipped"; continue ;; esac
    seen="$seen$t "
    B=$GATE_TMP/text-awk-$alt; rm -rf "$B"; mkdir -p "$B"; ln -s "$a" "$B/awk"
    ( PATH="$B:$PATH"; export PATH; INFO_OK=0; INFO_BAD=0; text_plants info_step; echo "INFO: $alt: text plants: $INFO_OK as expected, $INFO_BAD not" )
    rm -rf "$B"
  done
}

case ${PART:-none} in
  base-a1|base-a2|base-b|base-c|base-d|errors|spec-undo|wire-guard|evidence|lint)
    timed lock_locked_metadata sh -c 'cargo metadata --format-version 1 > /dev/null && echo "cargo metadata --locked: the lock resolves as is"'
    . tools/tests/full.sh ;;
  licence)
    timed licences_metadata sh -c 'if cargo metadata --format-version 1 --locked > /dev/null; then echo "cargo metadata --locked: ok"; else echo "cargo metadata --locked FAILED: the licence check cannot run"; exit 1; fi'
    timed licences sh scripts/check-licences.sh
    timed tree_no_engine sh -c 'out=$(cargo tree -p k2-mpc-base -e normal) || exit 1; if printf "%s\n" "$out" | grep -E "mpce[-_]"; then echo "an mpce crate in the kit tree"; exit 1; fi; echo "kit normal tree: no mpce crate"'
    timed tree_workspace_no_engine sh -c 'out=$(cargo tree --workspace -e all) || exit 1; if printf "%s\n" "$out" | grep -E "mpce[-_]"; then echo "an mpce crate in the workspace tree"; exit 1; fi; echo "workspace tree (all edges): no mpce crate"' ;;
  lockproof)
    export REAL
    timed lock_cargo_resolves sh -c '"$REAL" metadata --format-version 1 > /dev/null && echo "cargo metadata (no --locked) ran"'
    timed lock_cargo_agrees git diff --exit-code --stat Cargo.lock
    timed lock_packages sh -c 'echo "packages: $(grep -c "^\[\[package\]\]" Cargo.lock)"; grep -E "^name = \"(mpce|k2-mpc)" Cargo.lock' ;;
  checker)
    timed checker_cut sh tools/check-public-cut.sh .
    P=$GATE_TMP/cut; rm -rf "$P"*
    if copy HEAD "$P"; then
      REG=$(sed 's/#.*//' tools/public-cut-regressions.txt | awk 'NF == 1 { print $1; exit }')
      REGF=$(grep -rlE "fn[[:space:]]+$REG[[:space:]]*[(<]" --include='*.rs' "$P" | head -1)
      cp -R "$P" "$P-reg"; sed "s/fn $REG/fn renamed_away/" "$REGF" > "$P-reg${REGF#"$P"}"
      timed plant_regression_missing expect_block regression "$P-reg" "regressions: test fn $REG missing"
      cp -R "$P" "$P-lic"; rm -f "$P-lic/LICENSE-MIT"
      timed plant_licence_missing expect_block licence "$P-lic" "licence: LICENSE-MIT missing or empty at the root"
      cp -R "$P" "$P-vend"; mkdir -p "$P-vend/vendor/mpce-newthing/src"; printf '[package]\nname = "mpce-newthing"\n' > "$P-vend/vendor/mpce-newthing/Cargo.toml"
      timed plant_new_engine_crate expect_block vendor "$P-vend" "paths: vendor/mpce-newthing exists" "vendor/mpce-newthing/Cargo.toml:2: [package] name: names mpce-newthing"
      cp -R "$P" "$P-dev"; awk '{ print } /^\[dev-dependencies\]/ { print "mpce-core = { path = \"../../vendor/mpce-core\", features = [\"verify-shortcuts\"] }" }' "$P/crates/base/Cargo.toml" > "$P-dev/crates/base/Cargo.toml"
      timed plant_dev_dependency expect_block dev-dependency "$P-dev" "[dev-dependencies] mpce-core: names mpce-core"
      cp -R "$P" "$P-tgt"; printf "\n[target.'cfg(unix)'.dependencies]\nmpce-sync = { path = \"../../vendor/mpce-sync\" }\n" >> "$P-tgt/crates/base/Cargo.toml"
      timed plant_target_dependency expect_block target-dependency "$P-tgt" "[target.'cfg(unix)'.dependencies] mpce-sync: names mpce-sync"
      cp -R "$P" "$P-fea"; under_features 'planted-x = ["mpce-core/planted-shortcuts"]' "$P/crates/base/Cargo.toml" > "$P-fea/crates/base/Cargo.toml"
      timed plant_engine_feature_ref expect_block engine-feature "$P-fea" "[features] planted-x: names mpce-core"
      cp -R "$P" "$P-der"; under_features 'foo = ["planted-shortcuts"]' "$P/crates/base/Cargo.toml" > "$P-der/crates/base/Cargo.toml"
      timed plant_derived_feature_left expect_block derived-feature "$P-der" "[features] foo: enables planted-shortcuts, which this manifest does not declare"
      cp -R "$P" "$P-win"; printf 'x\n' > "$P-win/nul.txt"
      timed plant_windows_name expect_block windows-name "$P-win" "windows-names: nul.txt: reserved name nul"
      cp -R "$P" "$P-wmi"; rm -f "$P-wmi/tools/check-windows-names.sh"
      timed plant_windows_check_missing expect_block windows-missing "$P-wmi" "windows-names: tools/check-windows-names.sh missing"
    else
      RC=1
    fi
    rm -rf "$P"*
    timed plant_text_start text_start
    text_plants timed
    info_text_awks || echo "INFO: the informational runs stopped early (rc $?); not gating"
    rm -rf "$GATE_TMP"/text-* ;;
  checker-precut)
    # the pre-cut commit must be blocked, naming each engine vendor directory, the engine-only
    # feature and each engine package of its lockfile (all read from that commit)
    P=$GATE_TMP/pre; rm -rf "$P"*
    if copy HEAD~1 "$P"; then
      set --
      eval "$(sh "$P/tools/check-public-cut.sh" --print-config)"
      for d in "$P"/vendor/mpce*; do
        n=${d##*/}; case " $PUBLIC_ENGINE_CRATES " in *" $n "*) continue ;; esac
        [ -d "$d" ] && set -- "$@" "paths: vendor/$n exists"
      done
      for x in $NATIVE_FEATURES; do grep -qE "^$x[[:space:]]*=" "$P/crates/base/Cargo.toml" && set -- "$@" "[features] declares the cut-only feature $x"; done
      for n in $(sed -n 's/^name = "\(mpce[-_][A-Za-z0-9_-]*\)"$/\1/p' "$P/Cargo.lock"); do
        case " $PUBLIC_ENGINE_CRATES " in *" $n "*) continue ;; esac
        set -- "$@" "[[package]] name: names $n"
      done
      for t in $DROPPED_TREES; do set -- "$@" "dropped: $(cd "$P" && find "$t" -type f | sort | sed -n 1p) (under $t, which the cut removes)"; done
      echo "expected findings: $#"
      [ $# -gt 0 ] || { echo "SETUP FAILED: no expectation read from the pre-cut commit"; RC=1; }
      timed checker_precut_blocks expect_block precut "$P" "$@"
      plant_script() {   # LABEL EXPECT-EXIT PLANT PATTERN...: public-cut.sh --no-commit on a planted copy
        lbl=$1; want=$2; ins=$3; shift 3
        W=$GATE_TMP/script-$lbl; rm -rf "$W"
        copy HEAD~1 "$W" git || return 1
        under_features "$ins" "$P/crates/base/Cargo.toml" > "$W/crates/base/Cargo.toml"
        out=$(cd "$W" && sh tools/public-cut.sh --no-commit 2>&1); r=$?
        printf '%s\n' "$out" | grep -v '^  removed vendor' | grep -v '^check-public-cut: sources' | tail -25
        ok=1
        [ "$r" -eq "$want" ] || { echo "PLANT $lbl: exit $r, expected $want"; ok=0; }
        for p in "$@"; do printf '%s\n' "$out" | grep -qF -- "$p" || { echo "PLANT $lbl: not in the output: $p"; ok=0; }; done
        if [ "$want" -eq 0 ]; then
          if grep -nE '^(foo|planted-shortcuts|native)[[:space:]]*=' "$W/crates/base/Cargo.toml"; then echo "PLANT $lbl: a derived feature survived the cut"; ok=0; fi
          grep -n '^bar' "$W/crates/base/Cargo.toml"
          grep -qx 'bar = \["differential"\]' "$W/crates/base/Cargo.toml" || { echo "PLANT $lbl: bar did not keep only differential"; ok=0; }
        fi
        rm -rf "$W"
        [ $ok = 1 ] && echo "PLANT $lbl: as expected (its asserted lines reached)"
        [ $ok = 1 ]
      }
      timed plant_script_derived_features plant_script derived 0 'planted-shortcuts = ["native", "mpce-core/planted-shortcuts"]|foo = ["planted-shortcuts"]|bar = ["foo", "differential"]' \
        "k2-mpc-base/native " "k2-mpc-base/planted-shortcuts " "k2-mpc-base/foo " "check-public-cut: clean"
      timed plant_script_mixed_stops plant_script mixed 1 'mix = ["native", "dep:redb"]' "feature mix mixes engine parts with optional dependencies" "a manifest needs a decision by hand"
    else
      RC=1
    fi
    rm -rf "$P"* ;;
  reproduce)
    WANT=$(git rev-parse 'HEAD^{tree}')
    # reproduce_with LABEL AWK-PATH|default: public-cut.sh on a copy of the pre-cut commit with
    # that awk first on PATH; prints `tree <hash>`; 1 on any setup or script failure
    reproduce_with() {
      lbl=$1; impl=$2
      W=$GATE_TMP/repro-$lbl; B=$W.bin; rm -rf "$W" "$B"
      copy HEAD~1 "$W" git || return 1
      mkdir -p "$B"; [ "$impl" = default ] || ln -s "$impl" "$B/awk"
      echo "REPRO $lbl: awk on PATH = $(PATH="$B:$PATH" command -v awk)"
      out=$(cd "$W" && PATH="$B:$PATH" sh tools/public-cut.sh --no-commit 2>&1); r=$?
      printf '%s\n' "$out" | grep -v '^  removed vendor' | tail -4
      if [ "$r" -ne 0 ]; then echo "REPRO $lbl: the script failed (exit $r)"; rm -rf "$W" "$B"; return 1; fi
      TREE=$(cd "$W" && git write-tree)
      if [ "$TREE" != "$WANT" ]; then
        X=$GATE_TMP/cut-tree; rm -rf "$X"; copy HEAD "$X"
        diff -r -x .git "$X" "$W" | head -60; rm -rf "$X"
      fi
      rm -rf "$W" "$B"; echo "tree $TREE"
    }
    # GATE: the node's awk (the build platform's) must make the committed cut exactly
    # the default awk's identity, never fatal (busybox refuses --version): its resolved path and
    # the first line of its version
    awk_id() {
      p=$(readlink -f "$(command -v awk)" 2>/dev/null || command -v awk)
      v=$(awk --version </dev/null 2>/dev/null | head -1)
      [ -n "$v" ] || v=$(awk -W version </dev/null 2>/dev/null | head -1)
      [ -n "$v" ] || v="version unknown"
      echo "$p ($v)"
    }
    gate_awk() {
      o=$(reproduce_with default default); r=$?; printf '%s\n' "$o"
      got=$(printf '%s\n' "$o" | sed -n 's/^tree //p')
      id=$(awk_id)
      if [ "$r" -eq 0 ] && [ "$got" = "$WANT" ]; then echo "GATE: $id tree $got == cut $WANT"; return 0; fi
      echo "GATE: FAILED: $id made tree '${got:-none}', the cut is $WANT"; return 1
    }
    timed reproduce_gate gate_awk
    # INFO: every other awk, once per distinct binary; never gating and never fatal (an || list
    # turns set -e off), one verdict line each
    info_one() {   # NAME AWK-PATH
      o=$(reproduce_with "$1" "$2" 2>&1); r=$?
      got=$(printf '%s\n' "$o" | sed -n 's/^tree //p')
      if [ "$r" -eq 0 ] && [ "$got" = "$WANT" ]; then echo "INFO: $1: tree == cut"
      elif [ "$r" -eq 0 ]; then echo "INFO: $1: DIFFERS (tree $got; $(printf '%s\n' "$o" | grep -c '^diff \|^Only in ') differing paths)"
      else echo "INFO: $1: script failed rc=$r: $(printf '%s\n' "$o" | grep -v '^REPRO ' | grep -m1 -iE 'awk|error|fatal|public-cut:|failed' || printf '%s\n' "$o" | grep -v '^REPRO ' | tail -1)"; fi
    }
    info_awks() {
      seen=" $(readlink -f "$(command -v awk)") "
      for alt in gawk mawk original-awk nawk; do
        a=$(command -v "$alt" 2>/dev/null) || continue
        t=$(readlink -f "$a")
        case $seen in *" $t "*) echo "INFO: $alt: $t, a binary already run: skipped"; continue ;; esac
        seen="$seen$t "
        info_one "$alt" "$a"
      done
      if command -v busybox >/dev/null 2>&1 && [ "$(busybox awk 'BEGIN { print "ok" }' 2>/dev/null)" = ok ]; then
        printf '#!/bin/sh\nexec busybox awk "$@"\n' > "$GATE_TMP/bb-awk"; chmod +x "$GATE_TMP/bb-awk"
        echo "INFO: busybox is $(busybox 2>&1 | head -1)"
        info_one busybox "$GATE_TMP/bb-awk"
      fi
    }
    info_awks || echo "INFO: the informational runs stopped early (rc $?); not gating" ;;
  *) echo "unknown PART ${PART:-none} (tools/tests/public-cut-gate.sh's header lists the parts)"; RC=2 ;;
esac

[ "${PART:-}" = checker-precut ] || timed end_checks sh tools/check-public-cut.sh .
timed lock_unchanged git diff --exit-code --stat Cargo.lock
timed tree_clean sh -c 's=$(git status --porcelain --untracked-files=all | grep -v "^?? junit/"); if [ -n "$s" ]; then printf "%s\n" "$s" | head -20; exit 1; fi; echo "tree clean"'
if [ "$RC" -eq 0 ]; then echo "=== RESULT ${PART:-none}: GREEN at $H"; else echo "=== RESULT ${PART:-none}: RED (rc $RC) at $H"; fi
