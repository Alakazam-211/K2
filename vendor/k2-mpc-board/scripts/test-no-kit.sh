#!/bin/sh
# check-no-kit.sh's self-test: the crate alone passes; a kit package, an engine, the bridge in the
# lock, the bridge as a workspace member, or the crate depending on the bridge, each fail, and each
# says why, so a case names the rule that fired, not only the exit.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
CHECK=$HERE/check-no-kit.sh
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
fails=0; n=0
CRATE='{"name":"k2-mpc-board","dependencies":[{"name":"miniz_oxide"},{"name":"sha2"}]}'
try() { # $1: wanted exit; $2: the case; $3: the packages; $4: the workspace members; $5: what it says
  n=$((n + 1))
  printf '{"packages":[%s],"workspace_members":[%s]}' "$3" "$4" > "$TMP/m.json"
  set +e; out=$(sh "$CHECK" "$TMP/m.json" 2>&1); rc=$?; set -e
  if [ "$rc" != "$1" ]; then echo "FAILED $2: wanted exit $1, got $rc"; fails=$((fails + 1))
  elif [ -n "${5:-}" ] && case $out in *"$5"*) false ;; *) true ;; esac; then
    echo "FAILED $2: it didn't say \"$5\": $out"; fails=$((fails + 1))
  else echo "ok   $2: exit $rc"; fi
}
try 0 crate-alone "$CRATE" '"k2-mpc-board 0.1.2 (path+file:///r/crates/board)"' "check-no-kit: clean"
try 1 kit-in-lock "$CRATE,{\"name\":\"k2-mpc-base\",\"dependencies\":[]}" '"k2-mpc-board 0.1.2 (path+file:///r/crates/board)"' "in the lock: k2-mpc-base"
try 1 engine-in-lock "$CRATE,{\"name\":\"yrs\",\"dependencies\":[]}" '"k2-mpc-board 0.1.2 (path+file:///r/crates/board)"' "in the lock: yrs"
try 1 bridge-in-lock "$CRATE,{\"name\":\"k2-mpc-board-bridge\",\"dependencies\":[]}" '"k2-mpc-board 0.1.2 (path+file:///r/crates/board)"' "the bridge is in the lock"
try 1 bridge-member "$CRATE" '"k2-mpc-board 0.1.2 (path+file:///r/crates/board)","k2-mpc-board-bridge 0.1.2 (path+file:///r/bridge)"' "the bridge is a workspace member"
try 1 bridge-member-new-form "$CRATE" '"path+file:///r/crates/board#k2-mpc-board@0.1.2","path+file:///r/bridge#k2-mpc-board-bridge@0.1.2"' "the bridge is a workspace member"
try 1 crate-uses-bridge '{"name":"k2-mpc-board","dependencies":[{"name":"miniz_oxide"},{"name":"k2-mpc-board-bridge"}]}' '"k2-mpc-board 0.1.2 (path+file:///r/crates/board)"' "direct dependency: k2-mpc-board-bridge"
try 1 no-crate '{"name":"other","dependencies":[]}' '"other 0.1.0 (path+file:///r/other)"' "no k2-mpc-board package in the metadata"
# the bridge's manifest must hold its own [workspace]: checked before cargo runs
n=$((n + 1))
mkdir -p "$TMP/repo/scripts" "$TMP/repo/bridge"
cp "$CHECK" "$TMP/repo/scripts/check-no-kit.sh"
printf '[package]\nname = "k2-mpc-board-bridge"\n' > "$TMP/repo/bridge/Cargo.toml"
set +e; out=$(sh "$TMP/repo/scripts/check-no-kit.sh" 2>&1); rc=$?; set -e
case $out in
  *"has no [workspace]"*) [ "$rc" = 1 ] && echo "ok   bridge-no-workspace: exit $rc" || { echo "FAILED bridge-no-workspace: wanted exit 1, got $rc"; fails=$((fails + 1)); } ;;
  *) echo "FAILED bridge-no-workspace: it didn't name the missing [workspace] (exit $rc)"; fails=$((fails + 1)) ;;
esac
if [ "$fails" -gt 0 ]; then echo "check-no-kit self-test: $fails of $n failed"; exit 1; fi
echo "check-no-kit self-test: ok ($n cases)"
