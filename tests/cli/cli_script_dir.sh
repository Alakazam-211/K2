#!/usr/bin/env bash
# Files shipped beside the CLI are found beside the program people ran,
# links followed, on the bash CLI and on the k5-cli binary alike. The
# binary runs the bash from an open file (/dev/fd/N), so it passes that
# folder in K5_CLI_SCRIPT_DIR; the bash looks beside its own file.
#
#   - `k2 approve key-id` runs the k2-approve beside the CLI, with no
#     daemon, and passes its arguments on; K5_CLI_SCRIPT_DIR never reaches
#     it (the bash takes it out of the environment at start);
#   - a k2-approve beside a link to the CLI is not that: PATH's is run;
#   - `k2 zen widget preview --shot` finds k2-widget-shot.mjs beside the
#     CLI, and says it has none when it is only beside the link;
#   - `k2 agent team --help` and `k2 agent instances --help` print the
#     verbs' help (once read back from the script's own file, which the
#     binary's /dev/fd/N can't give twice).
#
# The CLI under test is copied (links followed) into a temp folder with
# the stand-ins beside it; nothing is written into the repo.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env
unset K5_APPROVE_BIN K2_APPROVE_BIN K2SO_APPROVE_BIN

T="$(mktemp -d -t k2-script-dir-XXXXXX)"
trap 'rm -rf "$T"; hermetic_cli_cleanup' EXIT
pass=0
die() { echo "FAIL: $*" >&2; exit 1; }
ok() { pass=$((pass + 1)); }

mkdir -p "$T/inst" "$T/bin" "$T/path" "$T/widget"
cp "$CLI_UNDER_TEST" "$T/inst/k2"
chmod 0755 "$T/inst/k2"
ln -s "$T/inst/k2" "$T/bin/k2"

# stand_in <path> <label>: a program that prints its label and arguments.
stand_in() {
    cat > "$1" <<EOF
#!/bin/sh
printf '%s:' '$2'
for a in "\$@"; do printf ' [%s]' "\$a"; done
echo " env:\${K5_CLI_SCRIPT_DIR-unset}"
EOF
    chmod 0755 "$1"
}
stand_in "$T/inst/k2-approve" beside-cli
stand_in "$T/bin/k2-approve" beside-link
stand_in "$T/path/k2-approve" on-path
export PATH="$T/path:$PATH"

# run <program> <args...>: its stdout and stderr in $out, its code in $rc.
run() { rc=0; out="$("$@" 2>&1)" || rc=$?; }

# k2 approve runs the k2-approve beside the CLI, called by the link or
# straight, with the arguments as they came.
for k2 in "$T/bin/k2" "$T/inst/k2"; do
    run "$k2" approve key-id "$T/a key.pem"
    [ "$rc" = 0 ] || die "$k2 approve key-id: exit $rc: $out"
    [ "$out" = "beside-cli: [key-id] [$T/a key.pem] env:unset" ] || die "$k2 approve key-id ran something else: $out"
    ok
done

# Handed in by hand, the folder is looked in first, and still never
# reaches the program it runs.
mkdir -p "$T/other"
stand_in "$T/other/k2-approve" from-script-dir
run env K5_CLI_SCRIPT_DIR="$T/other" "$T/inst/k2" approve key-id y
if [ -n "${K5_CLI_UNDER_TEST:-}" ]; then
    # The binary sets its own folder over it.
    [ "$out" = "beside-cli: [key-id] [y] env:unset" ] || die "the binary with K5_CLI_SCRIPT_DIR set: exit $rc: $out"
else
    [ "$out" = "from-script-dir: [key-id] [y] env:unset" ] || die "K5_CLI_SCRIPT_DIR=$T/other: exit $rc: $out"
fi
[ "$rc" = 0 ] || die "K5_CLI_SCRIPT_DIR set: exit $rc: $out"
ok

# Not beside the CLI, only beside the link: the link's folder is never
# looked in, so the one on PATH runs.
rm "$T/inst/k2-approve"
run "$T/bin/k2" approve key-id x
[ "$rc" = 0 ] && [ "$out" = "on-path: [key-id] [x] env:unset" ] || die "with k2-approve only beside the link: exit $rc: $out"
ok

# The shot runner beside the CLI is found: whatever happens next (no
# Node, or no daemon on the dead port), it is not "no shot runner".
printf '// stand-in\n' > "$T/inst/k2-widget-shot.mjs"
run "$T/bin/k2" zen widget preview --dir "$T/widget" --shot "$T/out.png"
[ "$rc" != 0 ] || die "zen widget preview --shot with no daemon succeeded: $out"
case "$out" in *"has no shot runner"*) die "the runner beside the CLI was not found: $out" ;; esac
ok
rm "$T/inst/k2-widget-shot.mjs"
printf '// stand-in\n' > "$T/bin/k2-widget-shot.mjs"
run "$T/bin/k2" zen widget preview --dir "$T/widget" --shot "$T/out.png"
[ "$rc" = 3 ] || die "zen widget preview --shot with the runner only beside the link: exit $rc, wanted 3: $out"
case "$out" in *"Error: this K2 has no shot runner ("*"/k2-widget-shot.mjs)"*) ;; *) die "no 'no shot runner' error: $out" ;; esac
ok

# The agent hire verbs' help.
for verb in team instances; do
    run "$T/bin/k2" agent "$verb" --help
    [ "$rc" = 0 ] || die "agent $verb --help: exit $rc: $out"
    first="${out%%$'\n'*}"
    [ "$first" = "── Agent hire: teams, instances, hire root, sender check (0.45.6) ────" ] || die "agent $verb --help starts: $first"
    case "$out" in *"  k2 agent instances retire <instance> [--force]"*) ;; *) die "agent $verb --help: $out" ;; esac
    [ "$(printf '%s\n' "$out" | wc -l | tr -d ' ')" = 15 ] || die "agent $verb --help is not 15 lines: $out"
    ok
done

echo "cli_script_dir: $pass passed"
