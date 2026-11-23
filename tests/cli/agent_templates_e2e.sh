#!/usr/bin/env bash
# Agent templates end to end (prd-agent-templates-v1 T9, 0.46.13): the
# k2 agent template verbs and `k2 agent hire --template`, headless, against
# a real k2-daemon. It covers list and show, the hire review (--dry-run),
# a hire without --launch, a re-apply, save (v1, v2, the secret refusal,
# reserved ids), add, remove, upgrade (the .proposed rule) and diff.
#
# Sandbox daemon under a temp HOME (worktree build), air-gapped, with an
# empty agent shim dir so no real LLM session can ever start. Fake secrets
# only. Never touches the real ~/.k2.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not executable" >&2; exit 1; }
command -v jq >/dev/null || { echo "FAIL: jq is required" >&2; exit 1; }

for v in $(env | cut -d= -f1 | grep -E '^(K2|K2SO)_' || true); do unset "$v"; done
export K2_AIRGAP=1
SHIM_DIR="$(mktemp -d -t k2-tpl-shim-XXXXXX)"
export K2_TEST_AGENT_SHIM_DIR="$SHIM_DIR"

source "$SCRIPT_DIR/_sandbox_daemon.sh"
sandbox_daemon_start
trap '_sandbox_daemon_cleanup; rm -rf "$SHIM_DIR"' EXIT

WORK="$SANDBOX_HOME/work"
mkdir -p "$WORK"

k2() {
    env -i HOME="$SANDBOX_HOME" PATH="$PATH" K2_AIRGAP=1 \
        K2_PORT="$K2SO_PORT" K2_PROJECT_PATH="$WORK" "$K2_CLI" "$@"
}

pass=0
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "  PASS: $*"; pass=$((pass + 1)); }
step_status() { jq -r --arg s "$1" '[.actions[] | select(.step == $s) | .status] | first // ""'; }
# run_rc <outfile> <errfile> <args…>: run k2, keep stdout/stderr, print the exit code.
run_rc() {
    local out="$1" err="$2"
    shift 2
    set +e
    k2 "$@" >"$out" 2>"$err"
    local rc=$?
    set -e
    echo "$rc"
}

k2 agent hire-root set "$SANDBOX_HOME/agents" >/dev/null

echo "== list: built-ins, system hidden, canSave =="
list="$(k2 agent template list --json)"
[ "$(jq -r .ok <<<"$list")" = "true" ] || fail "list: $list"
[ "$(jq -r .canSave <<<"$list")" = "true" ] || fail "list carries canSave: $list"
[ "$(jq -r '.templates[0].id' <<<"$list")" = "general" ] || fail "General first: $list"
for id in general lead developer app-owner designer researcher reviewer writer support ops; do
    jq -e --arg i "$id" '.templates | any(.id == $i)' <<<"$list" >/dev/null || fail "list has $id"
done
jq -e '.templates | any(.kind == "system") | not' <<<"$list" >/dev/null || fail "system templates hidden: $list"
all="$(k2 agent template list --all --json)"
jq -e '.templates | any(.id == "kessel")' <<<"$all" >/dev/null || fail "--all shows kessel: $all"
jq -e '.templates | any(.id == "chief-of-staff")' <<<"$all" >/dev/null || fail "--all shows chief-of-staff"
k2 agent template list | grep -q "^  writer " || fail "text list shows writer"
ok "list"

echo "== show writer =="
show="$(k2 agent template show writer)"
grep -q "^Writer (writer) v" <<<"$show" || fail "show header: $show"
grep -q "Suggests:" <<<"$show" || fail "show suggestions: $show"
grep -q "audience:" <<<"$show" || fail "show questions: $show"
sj="$(k2 agent template show writer --json)"
[ "$(jq -r .template.id <<<"$sj")" = "writer" ] || fail "show --json: $sj"
[ "$(jq -r .template.kind <<<"$sj")" = "agent" ] || fail "writer is an agent template"
ok "show"

echo "== hire review: a Writer named Max on Codex, --dry-run =="
plan="$(k2 agent hire --template writer --name "Max" --agent codex --dry-run --json)"
[ "$(jq -r .dryRun <<<"$plan")" = "true" ] || fail "dry run: $plan"
mpath="$(jq -r .path <<<"$plan")"
[ "$mpath" = "$SANDBOX_HOME/agents/max" ] || fail "folder under the hire root: $mpath"
[ ! -e "$mpath" ] || fail "a dry run writes nothing"
[ "$(step_status apply-template <<<"$plan")" = "pending" ] || fail "template step pending: $plan"
jq -e '.actions[] | select(.step == "apply-template") | .rows | any(.step == "role" and .status == "write")' <<<"$plan" >/dev/null \
    || fail "review names ROLE.md: $plan"
jq -e '.actions[] | select(.step == "set-default-agent") | .preset' <<<"$plan" >/dev/null || fail "codex preset resolved: $plan"
k2 agent hire --template writer --name "Max" --agent codex --dry-run | grep -q "DRY RUN" || fail "text dry run"
ok "review"

echo "== hire without --launch: nothing starts, the lock is written =="
hired="$(k2 agent hire --template writer --name "Max" --agent codex --json)"
[ "$(jq -r .ok <<<"$hired")" = "true" ] || fail "hire: $hired"
[ "$(jq -r .path <<<"$hired")" = "$mpath" ] || fail "same folder as the review: $hired"
[ "$(step_status apply-template <<<"$hired")" = "done" ] || fail "template applied: $hired"
[ "$(jq '[.actions[] | select(.step == "launch")] | length' <<<"$hired")" = "0" ] || fail "no launch step: $hired"
[ "$(step_status hello <<<"$hired")" != "done" ] || fail "no hello without --launch: $hired"
[ -f "$mpath/.k2/agent/template.lock" ] || fail "lock written"
grep -q '^id = "writer"' "$mpath/.k2/agent/template.lock" || fail "lock names writer"
grep -q '^hello_pending = ' "$mpath/.k2/agent/template.lock" || fail "the hello waits in the lock for the first start"
grep -q "You are Max. You write for" "$mpath/.k2/agent/ROLE.md" || fail "charter rendered: $(cat "$mpath/.k2/agent/ROLE.md")"
[ -f "$mpath/.k2/wiki/Voice and style.md" ] || fail "starter note"
[ -z "$(ls -A "$SHIM_DIR")" ] || fail "nothing touched the agent shim dir"
ok "hire (not launched)"

echo "== re-apply is idempotent =="
again="$(k2 agent hire --template writer --name "Max" --agent codex --json)"
[ "$(jq -r .changed <<<"$again")" = "false" ] || fail "re-apply converges: $again"
[ "$(step_status apply-template <<<"$again")" = "skipped" ] || fail "template skipped on re-apply: $again"
set +e; k2 agent hire "$mpath" --template ops >/dev/null 2>&1; rc=$?; set -e
[ "$rc" = "3" ] || fail "another template on Max: template_conflict exit 3 (got $rc)"
ok "re-apply"

echo "== permissions (T3): the owner grants, --decline leaves one off =="
lplan="$(k2 agent hire --template lead --name "Lea" --dry-run)"
grep -q "would grant  permission agents.hire" <<<"$lplan" || fail "the owner would grant agents.hire: $lplan"
dplan="$(k2 agent hire --template developer --name "Dev" --decline compute.use --dry-run --json)"
jq -e '.actions[] | select(.step == "apply-template") | .rows | any(.step == "permission" and .target == "compute.use" and .status == "declined")' <<<"$dplan" >/dev/null \
    || fail "declined in the review: $dplan"
set +e; k2 agent hire --template developer --name "Dev" --decline compute.usee --dry-run >/dev/null 2>"$WORK/decline.err"; rc=$?; set -e
[ "$rc" = "2" ] || fail "a decline the template doesn't suggest is usage, exit 2 (got $rc)"
grep -q "it suggests: permission-cards, compute.use" "$WORK/decline.err" || fail "the usage error names what it suggests: $(cat "$WORK/decline.err")"
dev="$(k2 agent hire --template developer --name "Dev" --decline compute.use --json)"
jq -e '.actions[] | select(.step == "apply-template") | .permissions.declined == ["compute.use"]' <<<"$dev" >/dev/null || fail "declined on the hire: $dev"
set +e; k2 agent hire --decline compute.use "$WORK/plain" >/dev/null 2>&1; rc=$?; set -e
[ "$rc" = "2" ] || fail "--decline without --template is usage, exit 2 (got $rc)"
ok "permissions"

echo "== save Max as a template: v1, then v2 =="
dry="$(k2 agent template save "$mpath" --id max-writer --summary "Writes like Max" --dry-run --json)"
[ "$(jq -r .dryRun <<<"$dry")" = "true" ] || fail "save dry run: $dry"
[ ! -e "$SANDBOX_HOME/.k2/agent-templates/max-writer" ] || fail "a save dry run writes nothing"
s1="$(k2 agent template save "$mpath" --id max-writer --summary "Writes like Max" --wiki "Voice and style" --json)"
[ "$(jq -r .version <<<"$s1")" = "1" ] || fail "first save is v1: $s1"
jq -e '.files | any(.path == "ROLE.md")' <<<"$s1" >/dev/null || fail "ROLE.md taken: $s1"
jq -e '.files | any(.path == "wiki/Voice and style.md")' <<<"$s1" >/dev/null || fail "--wiki note taken: $s1"
jq -e '.files | any(.path == "wiki/About my person.md") | not' <<<"$s1" >/dev/null || fail "unpicked notes not taken: $s1"
case "$(jq -r .author <<<"$s1")" in user:*) ;; *) fail "a person saves as user:<login>: $s1" ;; esac
s2="$(k2 agent template save "$mpath" --id max-writer --json)"
[ "$(jq -r .version <<<"$s2")" = "2" ] || fail "second save is v2: $s2"
[ -d "$SANDBOX_HOME/.k2/agent-templates/max-writer/v1" ] || fail "v1 kept"
k2 agent template show max-writer | grep -q "Versions: v1, v2" || fail "show lists both versions"
ok "save v1 → v2"

echo "== save . names the workspace you are in, from a subfolder too =="
dot="$(cd "$mpath/.k2/wiki" && env -i HOME="$SANDBOX_HOME" PATH="$PATH" K2_AIRGAP=1 K2_PORT="$K2SO_PORT" \
    "$K2_CLI" agent template save . --id max-writer --dry-run --json)" || fail "save . from a subfolder: $dot"
[ "$(jq -r .version <<<"$dot")" = "3" ] || fail "save . resolves to Max (next is v3): $dot"
[ ! -e "$SANDBOX_HOME/.k2/agent-templates/max-writer/v3" ] || fail "a dry run writes nothing"
ok "save ."

echo "== save refuses a secret, naming the file and line, never echoing it =="
role="$mpath/.k2/agent/ROLE.md"
cp "$role" "$WORK/ROLE.md.bak"
fake="ghp_FAKEfake1234567890abcdef""GHIJ"
printf 'Deploy key for later: %s\n' "$fake" >>"$role"
line="$(grep -n "Deploy key for later" "$role" | cut -d: -f1)"
rc="$(run_rc "$WORK/sec.out" "$WORK/sec.err" agent template save "$mpath" --id max-writer)"
[ "$rc" = "1" ] || fail "secret_found exits 1 (got $rc): $(cat "$WORK/sec.err")"
grep -q "secret_found" "$WORK/sec.err" || fail "names secret_found: $(cat "$WORK/sec.err")"
grep -q ".k2/agent/ROLE.md line $line" "$WORK/sec.err" || fail "names the file and line $line: $(cat "$WORK/sec.err")"
if grep -q "FAKEfake" "$WORK/sec.err" "$WORK/sec.out"; then fail "the secret was echoed"; fi
[ ! -e "$SANDBOX_HOME/.k2/agent-templates/max-writer/v3" ] || fail "nothing written on a refusal"
rc="$(run_rc "$WORK/secd.out" "$WORK/secd.err" agent template save "$mpath" --id max-writer --dry-run)"
[ "$rc" = "1" ] || fail "a dry run refuses the secret too (got $rc): $(cat "$WORK/secd.out" "$WORK/secd.err")"
grep -q ".k2/agent/ROLE.md line $line" "$WORK/secd.err" || fail "dry run names the file and line"
if grep -q "FAKEfake" "$WORK/secd.err" "$WORK/secd.out"; then fail "the secret was echoed (dry run)"; fi
rc="$(run_rc "$WORK/secj.out" "$WORK/secj.err" agent template save "$mpath" --id max-writer --json)"
[ "$rc" = "1" ] || fail "secret_found --json exits 1 (got $rc)"
if grep -q "FAKEfake" "$WORK/secj.err" "$WORK/secj.out"; then fail "the secret was echoed (--json)"; fi
cp "$WORK/ROLE.md.bak" "$role"
ok "secret refused"

echo "== built-in ids are reserved =="
rc="$(run_rc "$WORK/res.out" "$WORK/res.err" agent template save "$mpath" --id writer)"
[ "$rc" = "1" ] || fail "id_reserved exits 1 (got $rc)"
grep -q "id_reserved" "$WORK/res.err" || fail "save --id writer: $(cat "$WORK/res.err")"
rc="$(run_rc "$WORK/res2.out" "$WORK/res2.err" agent template save "$mpath" --id qa)"
grep -q "id_reserved" "$WORK/res2.err" || fail "an old id is reserved too: $(cat "$WORK/res2.err")"
rc="$(run_rc "$WORK/res3.out" "$WORK/res3.err" agent template remove writer)"
grep -q "id_reserved" "$WORK/res3.err" || fail "remove writer: $(cat "$WORK/res3.err")"
rc="$(run_rc "$WORK/bad.out" "$WORK/bad.err" agent template save "$mpath" --id "Not An Id")"
[ "$rc" = "2" ] || fail "bad_id exits 2 (got $rc)"
ok "id_reserved, bad_id"

echo "== add from a folder =="
tdir="$WORK/e2e-checker"
mkdir -p "$tdir/wiki"
cat >"$tdir/template.toml" <<'TOML'
schema = 1
id = "e2e-checker"
version = 1
name = "E2E checker"
summary = "Checks things in the end-to-end test."
kind = "agent"
aliases = []
author = "user:whoever"
shared_block = true

[agent]
role = "checker"
tool = "default"

[context]
catalog = ["wiki:index"]

[permissions]
cards = true
suggest = []

[params]

[hello]
brief = "Say hello to {{person}}."
TOML
printf '## Your role\n\nYou are {{name}}. You check things for {{person}}.\n' >"$tdir/ROLE.md"
printf '# Checklist\n\n## Before\n\n## After\n' >"$tdir/wiki/Checklist.md"
ad="$(k2 agent template add "$tdir" --dry-run --json)"
[ "$(jq -r .dryRun <<<"$ad")" = "true" ] || fail "add dry run: $ad"
[ ! -e "$SANDBOX_HOME/.k2/agent-templates/e2e-checker" ] || fail "an add dry run writes nothing"
a1="$(k2 agent template add "$tdir" --json)"
[ "$(jq -r .version <<<"$a1")" = "1" ] || fail "add is v1: $a1"
case "$(jq -r .author <<<"$a1")" in user:whoever) fail "the installer is the author, not the file's: $a1" ;; user:*) ;; *) fail "author: $a1" ;; esac
k2 agent template list | grep -q "^Yours" || fail "list has a Yours part"
k2 agent template list | grep -q "^  e2e-checker " || fail "list shows the added template"
ok "add"

echo "== upgrade and diff: the .proposed rule =="
up_ws="$WORK/checker-ada"
h="$(k2 agent hire "$up_ws" --template e2e-checker --name "Ada" --json)"
[ "$(jq -r .ok <<<"$h")" = "true" ] || fail "hire from a library template: $h"
[ -f "$up_ws/.k2/wiki/Checklist.md" ] || fail "library starter note written"
# The agent edits its checklist; K2 then ships v2 of the template.
printf '\n- the agent added this line\n' >>"$up_ws/.k2/wiki/Checklist.md"
printf '## Your role\n\nYou are {{name}}. You check things for {{person}}, twice.\n' >"$tdir/ROLE.md"
printf '# Checklist\n\n## Before\n\n## During\n\n## After\n' >"$tdir/wiki/Checklist.md"
a2="$(k2 agent template add "$tdir" --json)"
[ "$(jq -r .version <<<"$a2")" = "2" ] || fail "second add is v2: $a2"
vd="$(k2 agent template diff e2e-checker 1 2 --json)"
jq -e '.files | any(.path == "ROLE.md")' <<<"$vd" >/dev/null || fail "version diff names ROLE.md: $vd"
d="$(k2 agent template diff "$up_ws" --json)"
[ "$(jq -r .dryRun <<<"$d")" = "true" ] || fail "diff is a dry run: $d"
[ "$(jq -r .from <<<"$d")/$(jq -r .to <<<"$d")" = "1/2" ] || fail "diff 1 → 2: $d"
jq -e '.rows | any(.step == "wiki" and .status == "keep")' <<<"$d" >/dev/null || fail "the edited note is kept: $d"
jq -e '.rows | any(.step == "role" and .status == "replace")' <<<"$d" >/dev/null || fail "the managed block is replaced: $d"
ud="$(k2 agent template upgrade "$up_ws" --dry-run --json)"
[ "$(jq -r .dryRun <<<"$ud")" = "true" ] || fail "upgrade --dry-run: $ud"
[ ! -e "$up_ws/.k2/wiki/Checklist.md.proposed" ] || fail "a dry run writes no .proposed"
grep -q "twice" "$up_ws/.k2/agent/ROLE.md" && fail "a dry run leaves ROLE.md"
u="$(k2 agent template upgrade "$up_ws" --json)"
[ "$(jq -r .changed <<<"$u")" = "true" ] || fail "upgrade changed: $u"
grep -q "the agent added this line" "$up_ws/.k2/wiki/Checklist.md" || fail "the agent's edit stays"
[ -f "$up_ws/.k2/wiki/Checklist.md.proposed" ] || fail ".proposed written beside it"
grep -q "## During" "$up_ws/.k2/wiki/Checklist.md.proposed" || fail ".proposed holds the new text"
grep -q "twice" "$up_ws/.k2/agent/ROLE.md" || fail "the K2:MANAGED block took v2"
grep -q '^version = 2' "$up_ws/.k2/agent/template.lock" || fail "lock on v2"
k2 agent template upgrade "$up_ws" --dry-run | grep -q "already on v2" || fail "a second upgrade has nothing to do"
# The built-in Max is on the newest Writer already.
md="$(k2 agent template diff "$mpath" --json)"
[ "$(jq -r .from <<<"$md")" = "$(jq -r .to <<<"$md")" ] || fail "Max is on the newest writer: $md"
k2 agent template upgrade "$mpath" --dry-run | grep -q "already on v" || fail "Max: nothing to upgrade"
ok "upgrade, diff, .proposed"

echo "== remove =="
r1="$(k2 agent template remove max-writer --version 1 --json)"
[ "$(jq -c .removed <<<"$r1")" = "[1]" ] && [ "$(jq -c .remaining <<<"$r1")" = "[2]" ] || fail "remove v1: $r1"
k2 agent template remove max-writer | grep -q "Removed max-writer" || fail "remove the rest"
set +e; k2 agent template show max-writer >/dev/null 2>&1; rc=$?; set -e
[ "$rc" = "4" ] || fail "a removed template is unknown_template, exit 4 (got $rc)"
k2 agent template remove e2e-checker >/dev/null
[ -f "$up_ws/.k2/wiki/Checklist.md" ] || fail "an agent hired from a removed template keeps its files"
rc="$(run_rc "$WORK/rm.out" "$WORK/rm.err" agent template upgrade "$up_ws")"
[ "$rc" = "4" ] || fail "upgrade after its template is gone: exit 4 (got $rc): $(cat "$WORK/rm.err")"
rc="$(run_rc "$WORK/nt.out" "$WORK/nt.err" agent template diff "$WORK")"
[ "$rc" = "4" ] || fail "diff of a folder not made from a template: exit 4 (got $rc): $(cat "$WORK/nt.err")"
ok "remove"

[ -z "$(ls -A "$SHIM_DIR")" ] || fail "nothing touched the agent shim dir"
echo
echo "Results: $pass passed"
