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
#   checker      tools/check-public-cut.sh: the cut is clean; planted copies of it are blocked
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
# never the checkout or its repository. Env: PART; EXPECT (optional: the sha the job must be at).
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
# commit on branch `gate` when the third argument is `git`
copy() {
  mkdir -p "$2" && git archive "$1" | tar -x -C "$2" || { echo "SETUP FAILED: archive of $1"; return 1; }
  [ "${3:-}" = git ] || return 0
  ( cd "$2" && git init -q -b gate && git add -A && git -c user.name=gate -c user.email=gate@invalid commit -qm "gate copy" ) \
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
    else
      RC=1
    fi
    rm -rf "$P"* ;;
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
      for x in $NATIVE_FEATURES; do grep -qE "^$x[[:space:]]*=" "$P/crates/base/Cargo.toml" && set -- "$@" "[features] declares the engine-only feature $x"; done
      for n in $(sed -n 's/^name = "\(mpce[-_][A-Za-z0-9_-]*\)"$/\1/p' "$P/Cargo.lock"); do
        case " $PUBLIC_ENGINE_CRATES " in *" $n "*) continue ;; esac
        set -- "$@" "[[package]] name: names $n"
      done
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
