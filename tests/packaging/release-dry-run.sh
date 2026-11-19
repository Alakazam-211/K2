#!/usr/bin/env bash
# release.sh --dry-run (scripts/lib/release-dry.sh), run end to end on a
# fixture repo with every external command stubbed: git push / tag, gh,
# curl, aws, ssh, scp, sudo, osascript, minisign, k2, cargo, bun, codesign,
# xcrun (notarytool, stapler), hdiutil; and release.sh's helper scripts and
# libraries (signers, gates, compute, Windows, R2) as stand-ins that log.
#
# Asserts:
#  - a full rehearsal runs every non-publishing step (gates, bump, build,
#    Developer ID signing, notarization, VM smoke, compute stage, Windows,
#    manifests) and ends DRY_EXIT=0 with a per-step summary;
#  - no publishing stand-in is ever called (git push/tag, gh release/
#    variable set, curl, the sign-* scripts' signing modes, k2sign sign /
#    compute-evidence / approval, windows-patch-latest-json, the R2 upload
#    without --dry-run, the Step 10.6 GitHub check), and each publishing
#    step prints its "[dry] would: ..." line instead;
#  - the source checkout is untouched (HEAD, refs, status, version files),
#    no tag is made, and its uncommitted + untracked files are rehearsed in
#    the scratch worktree, where the version is bumped;
#  - the release lock is held during the run and released after, also when
#    a step fails (the summary then names the step FAIL, DRY_EXIT != 0);
#  - the guard catches a publishing git/gh/curl call that has no dry branch;
#  - outside a dry run the guard functions don't exist; a dry run refuses
#    to run anywhere but its scratch tree.
#
# release.sh is macOS-only (BSD sed -i '', ditto), so this test is too.
# Run with: bash tests/packaging/release-dry-run.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
pass=0
fail=0
pass() { echo "  PASS: $1"; pass=$((pass + 1)); }
fail() { echo "  FAIL: $1" >&2; [ -z "${2:-}" ] || printf '        %s\n' "$2" >&2; fail=$((fail + 1)); }
check() { if eval "$1"; then pass "$2"; else fail "$2" "${3:-}"; fi; }

if [ "$(uname -s)" != Darwin ]; then
    echo "FAIL: release.sh (and so this test) runs on macOS only (BSD sed -i '', ditto)." >&2
    exit 1
fi

# The test process only: no K2_* from the calling shell reaches release.sh.
for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done
REAL_GIT="$(command -v git)"
work="$(mktemp -d -t k2-release-dry-test)"
work="$(cd "$work" && pwd -P)"
V=9.8.7
trap 'rm -rf "$work" "/tmp/K2_${V}.zip" "/tmp/k2-node-macos-aarch64_${V}.zip"' EXIT
src="$work/src"
stub="$work/stub"
T_LOG="$work/log"
mkdir -p "$src" "$stub" "$T_LOG" "$work/home"
: > "$T_LOG/forbidden.log"
export T_LOG REAL_GIT
export HOME="$work/home"
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@example.invalid GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@example.invalid
export GIT_CONFIG_NOSYSTEM=1

# ── Stubs on PATH ──
# Publishing commands log to forbidden.log; the rest log and succeed.
cat > "$stub/git" <<'EOF'
#!/bin/bash
# Real git, except anything that writes to a remote or makes a tag.
sub="" skip=0 n=0 listing=0
for a in "$@"; do
    if [ "$skip" = 1 ]; then skip=0; continue; fi
    if [ -z "$sub" ]; then
        case "$a" in -C|-c|--git-dir|--work-tree) skip=1 ;; -*) ;; *) sub="$a" ;; esac
        continue
    fi
    n=$((n + 1))
    case "$a" in -l|--list|-n*|--contains|--points-at) listing=1 ;; esac
done
case "$sub" in
    push|send-pack|request-pull|update-server-info) echo "git $*" >> "$T_LOG/forbidden.log"; exit 0 ;;
    tag) if [ "$n" -gt 0 ] && [ "$listing" = 0 ]; then echo "git $*" >> "$T_LOG/forbidden.log"; exit 0; fi ;;
    update-ref) case " $* " in *" refs/tags/"*|*" refs/heads/"*) echo "git $*" >> "$T_LOG/forbidden.log"; exit 0 ;; esac ;;
esac
exec "$REAL_GIT" "$@"
EOF
cat > "$stub/gh" <<'EOF'
#!/bin/bash
echo "gh $*" >> "$T_LOG/gh.log"
case "$1 $2" in
    "variable get") echo "${T_GH_VAR:-github}"; exit 0 ;;
    "api repos/Alakazam-211/K2") echo true; exit 0 ;;
esac
echo "gh $*" >> "$T_LOG/forbidden.log"
exit 0
EOF
for t in curl aws ssh scp sudo osascript minisign k2 gsutil rclone wrangler; do
    printf '#!/bin/bash\necho "%s $*" >> "$T_LOG/forbidden.log"\nexit 0\n' "$t" > "$stub/$t"
done
for t in cargo bun bunx codesign; do
    printf '#!/bin/bash\necho "%s $* (in $PWD)" >> "$T_LOG/%s.log"\nexit 0\n' "$t" "$t" > "$stub/$t"
done
cat > "$stub/hdiutil" <<'EOF'
#!/bin/bash
echo "hdiutil $*" >> "$T_LOG/hdiutil.log"
for last; do :; done
mkdir -p "$(dirname "$last")" && echo "dmg bytes" > "$last"
EOF
cat > "$stub/xcrun" <<'EOF'
#!/bin/bash
echo "xcrun $*" >> "$T_LOG/xcrun.log"
case "$1" in
    --sdk) echo /fake/MacOSX.sdk; exit 0 ;;
    stapler) exit 0 ;;
    notarytool)
        case "$2" in
            submit)
                [ -f "$3" ] || exit 1
                [ "${T_NOTARY_FAIL:-0}" = 1 ] && { echo "notarytool: refused on purpose (test)"; exit 1; }
                echo '{"id":"0b8e7c52-1f0e-4c43-9d5b-2a7c1e9f3a11","message":"Successfully uploaded file"}'
                exit 0 ;;
            wait) echo '{"id":"0b8e7c52-1f0e-4c43-9d5b-2a7c1e9f3a11","status":"Accepted"}'; exit 0 ;;
            log) echo "notary log"; exit 0 ;;
        esac ;;
esac
exit 1
EOF
chmod +x "$stub"/*

# ── The fixture repository: the real release.sh, release-lock.sh,
# release-dry.sh, daemon-manifest.sh and notary.sh; stand-ins for the rest ──
cd "$src"
mkdir -p scripts/lib scripts/ci cli/src src-tauri crates/k2-core crates/k2-daemon crates/k2-menubar
cp "$ROOT/scripts/release.sh" "$ROOT/scripts/release-lock.sh" "$ROOT/scripts/daemon-manifest.sh" scripts/
cp "$ROOT/scripts/lib/release-dry.sh" "$ROOT/scripts/lib/notary.sh" scripts/lib/
printf '{\n  "name": "k2",\n  "version": "0.0.1"\n}\n' > package.json
printf '{\n  "version": "0.0.1"\n}\n' > src-tauri/tauri.conf.json
for c in src-tauri crates/k2-core crates/k2-daemon crates/k2-menubar; do
    printf '[package]\nname = "x"\nversion = "0.0.1"\n' > "$c/Cargo.toml"
done
printf '# lock\n' > Cargo.lock
printf 'K2_CLI_VERSION="0.0.1"\n' > cli/src/prelude.sh
cp cli/src/prelude.sh cli/k2
printf '## %s — Rehearsal\n\nWhat a rehearsal ships.\n\n## 0.0.1 — Old\n\nOld.\n' "$V" > WHATS_NEW.md
printf '<plist/>\n' > src-tauri/entitlements.plist
printf '<plist/>\n' > src-tauri/entitlements-daemon.plist
printf 'target/\nnode_modules/\ndist-windows/\n.env\nsrc-tauri/binaries/\n.claude/\n' > .gitignore

cat > scripts/lib/rust-pin.sh <<'EOF'
k2_rust_pin_check() { echo "rust-pin" >> "$T_LOG/ran.log"; }
EOF
: > scripts/lib/compute-tier.sh
: > scripts/lib/linux-gate-spec.sh
cat > scripts/lib/release-gates.sh <<'EOF'
k2rg_mode() { echo local; }
k2rg_reachable() { return 0; }
_k2rg_mac_where() { echo local; }
k2rg_local_all() {
    echo "gates" >> "$T_LOG/ran.log"
    bun run typecheck:web && bunx vitest run && bun run vite:build:web && cargo check --workspace --all-targets
}
k2rg_local() { cargo check --workspace --all-targets; }
k2rg_cancel_open() { :; }
k2rg_summary() { :; }
k2rg_gates_only() { echo gates-only; }
EOF
cat > scripts/lib/wait-progress.sh <<'EOF'
k2_wait_tick() { :; }
k2_wait_done() { :; }
EOF
cat > scripts/lib/linux-sums-step.sh <<'EOF'
k2_linux_sums_step() { echo "k2_linux_sums_step $*" >> "$T_LOG/forbidden.log"; }
EOF
cat > scripts/require-mail-oauth-build-env.sh <<'EOF'
require_mail_oauth_build_env() { :; }
assert_daemon_oauth_not_placeholder() { echo "oauth-check $1" >> "$T_LOG/ran.log"; }
EOF
cat > scripts/lib/sign-updater.sh <<'EOF'
k2u_backend() { printf '%s' "${K2_SIGNER:-k2sign}"; }
k2u_env_guard() { :; }
k2u_fail() { echo "[sign-updater] ERROR: $*" >&2; return 1; }
k2u_sign() { echo "k2u_sign $*" >> "$T_LOG/forbidden.log"; return 1; }
k2u_verify_u1() { echo "k2u_verify_u1 $*" >> "$T_LOG/forbidden.log"; return 1; }
k2u_verify_u2() { echo "k2u_verify_u2 $*" >> "$T_LOG/forbidden.log"; return 1; }
k2u_slot_key_id() { printf 'RWTESTU2KEYID'; }
k2u_b64() { base64 < "$1" | tr -d '\n'; }
k2u_k2sign() {
    echo "k2sign $1" >> "$T_LOG/k2sign.log"
    case "$1" in
        status) echo "{\"version\": \"$2\", \"session\": null}" ;;
        compute-pins) echo '{"controller": "c0ffee00c0ffee00c0ffee00", "nodes": [{"name": "t-l1"}, {"name": "t-v1"}]}' ;;
        compute-rehearse) cat > "$T_LOG/rehearse.in"; echo '{"ok": true, "rehearsal": true}' ;;
        *) echo "k2sign $*" >> "$T_LOG/forbidden.log"; return 1 ;;
    esac
}
k2u_compute_pins() { k2u_k2sign compute-pins; }
k2u_compute_evidence() { echo "k2u_compute_evidence $*" >> "$T_LOG/forbidden.log"; return 1; }
EOF
cat > scripts/lib/sign-approve.sh <<'EOF'
k2sa_open_session() { echo "k2sa_open_session $*" >> "$T_LOG/forbidden.log"; return 1; }
EOF
cat > scripts/lib/build-binaries.sh <<'EOF'
K2_STANDALONE_PID=""
K2_STANDALONE_RC=""
k2_build_mac_binaries() {
    local app="$1" b
    echo "build $app" >> "$T_LOG/ran.log"
    bun run tauri build && cargo build --release -p k2-daemon || return 1
    mkdir -p "$app/Contents/MacOS" "$app/Contents/Helpers/K2 Daemon.app"
    printf '#!/bin/sh\nexit 0\n' > "$app/Contents/MacOS/k2"
    for b in k2-menubar k2-power-helper k2-node frpc; do printf 'bin %s\n' "$b" > "$app/Contents/MacOS/$b"; done
    chmod +x "$app/Contents/MacOS/"*
}
k2_daemon_helper_sign() { codesign --force --options runtime --sign "$2" "$1/Contents/Helpers/K2 Daemon.app"; }
k2_screen_helper_sign() { :; }
for _f in k2_daemon_helper_verify k2_daemon_helper_verify_signed k2_screen_helper_verify k2_screen_helper_verify_signed \
    k2_daemon_helper_verify_shipped_ptyd k2_daemon_helper_verify_shipped_export k2_daemon_helper_verify_shipped_deckrender \
    k2_daemon_helper_verify_shipped_scenerender k2_screen_helper_verify_shipped k2_daemon_helper_verify_tarball_ptyd \
    k2_daemon_helper_verify_tarball_export k2_daemon_helper_verify_tarball_deckrender k2_daemon_helper_verify_tarball_scenerender \
    k2_screen_helper_verify_tarball k2_standalone_macos_daemon_poll k2_standalone_macos_daemon_wait k2_standalone_macos_daemon_kill; do
    eval "$_f() { :; }"
done
k2_standalone_macos_daemon_start() { K2_STANDALONE_RC=0; echo "standalone-start" >> "$T_LOG/ran.log"; }
k2_standalone_macos_daemon_finish() { printf 'macos daemon %s\n' "$VERSION" > "$3"; }
k2_build_standalone_macos_daemon() { printf 'macos daemon %s\n' "$VERSION" > "$2"; }
EOF
cat > scripts/lib/compute-release.sh <<'EOF'
k2cr_x86_node() { printf t-l1; }
k2cr_verify_node() { printf t-v1; }
k2cr_arm_on() { return 1; }
k2cr_arm_node() { return 0; }
k2cr_arm_verify_node() { return 0; }
k2cr_check_arm_nodes() { return 0; }
k2cr_init() {
    K2CR_TREE="$(git rev-parse "$2^{tree}")" K2CR_STATE="$3" K2CR_FILES="$4"
    mkdir -p "$3" && printf 'L1\tjob-l1\nV1\tjob-v1\nL3\tjob-l3\n' > "$3/jobs.tsv"
    echo "compute-init $2" >> "$T_LOG/ran.log"
}
k2cr_submit_release_set() { echo "compute-submit" >> "$T_LOG/ran.log"; K2CR_ARM_SUBMIT_OK=""; }
k2cr_wait() { return 0; }
k2cr_collect() {
    K2CR_COLLECTED=1 K2CR_X86_OK=1
    mkdir -p "$K2CR_STATE/L1" && echo '{}' > "$K2CR_STATE/L1/receipt.json" && echo '{"ok": true}' > "$K2CR_STATE/V1.verdict.json"
}
k2cr_stage() {
    local d="$1" b f
    mkdir -p "$d" && : > "$K2CR_STATE/staged.txt"
    for b in k2-daemon k2-node k2-root-helper k2-app-engine; do
        f="$b-linux-x86_64"
        printf '%s %s\n' "$b" "$VERSION" > "$d/$f"
        (cd "$d" && shasum -a 256 "$f" > "$f.sha256")
        printf '%s\n%s\n' "$f" "$f.sha256" >> "$K2CR_STATE/staged.txt"
    done
    printf 'deb\n' > "$d/K2_${VERSION}_amd64.deb"
    (cd "$d" && shasum -a 256 "K2_${VERSION}_amd64.deb" > SHA256SUMS-linux-x86_64.txt)
    printf 'K2_%s_amd64.deb\nSHA256SUMS-linux-x86_64.txt\n' "$VERSION" >> "$K2CR_STATE/staged.txt"
}
k2cr_cancel_open() { :; }
k2cr_cancel_arm() { :; }
k2cr_job_times() { :; }
k2cr_compare_github() { echo "k2cr_compare_github $*" >> "$T_LOG/forbidden.log"; }
EOF
cat > scripts/lib/ci-client-release.sh <<'EOF'
k2ccr_cancel_open() { :; }
k2ccr_runs() { :; }
EOF
cat > scripts/sign-daemon.sh <<'EOF'
#!/bin/bash
[ "${1:-}" = --preflight ] && { echo "sign-daemon --preflight" >> "$T_LOG/ran.log"; exit 0; }
echo "sign-daemon.sh $*" >> "$T_LOG/forbidden.log"; exit 1
EOF
for s in sign-root-helper sign-k2-node sign-app-engine; do
    cat > "scripts/$s.sh" <<EOF
#!/bin/bash
case "\${1:-}" in --preflight|--check) echo "$s \$1" >> "\$T_LOG/ran.log"; exit 0 ;; esac
echo "$s.sh \$*" >> "\$T_LOG/forbidden.log"; exit 1
EOF
done
cat > scripts/linux-build-gate.sh <<'EOF'
#!/bin/bash
echo "linux-gate" >> "$T_LOG/ran.log"
EOF
cat > scripts/build-cli.sh <<'EOF'
#!/bin/bash
cd "$(dirname "$0")/.." && cp cli/src/prelude.sh cli/k2
EOF
cat > scripts/vm-newuser-pair-smoke.sh <<'EOF'
#!/bin/bash
[ -f "$1" ] || { echo "no dmg $1" >&2; exit 1; }
echo "vm-smoke $2" >> "$T_LOG/ran.log"
cat "$K2_RELEASE_LOCK" > "$T_LOG/lock-during-run" 2>/dev/null || true
EOF
cat > scripts/publish-web-bundles.sh <<'EOF'
#!/bin/bash
case " $* " in *" --dry-run "*) echo "publish-web $*" >> "$T_LOG/ran.log"; exit 0 ;; esac
echo "publish-web-bundles.sh $*" >> "$T_LOG/forbidden.log"
EOF
cat > scripts/windows-nsis-build.sh <<'EOF'
#!/bin/bash
set -euo pipefail
V="$1" ref="${K2_WINDOWS_REF:-v$1}"
cd "$(dirname "$0")/.."
c="$(git rev-parse --verify --quiet "$ref^{commit}")" || { echo "windows: $ref is not a commit" >&2; exit 1; }
git show "$c:package.json" | grep -q "\"version\": \"$V\"" || { echo "windows: $ref is not version $V" >&2; exit 1; }
echo "windows $c" >> "$T_LOG/ran.log"
mkdir -p dist-windows
echo exe > "dist-windows/K2_${V}_x64-setup.exe"
echo sums > dist-windows/SHA256SUMS-windows-x86_64.txt
EOF
cat > scripts/windows-patch-latest-json.sh <<'EOF'
#!/bin/bash
echo "windows-patch-latest-json.sh $*" >> "$T_LOG/forbidden.log"; exit 1
EOF
cat > scripts/ci/compute_release.py <<'EOF'
import os, sys
open(os.path.join(os.environ["T_LOG"], "ran.log"), "a").write("evidence-bundle %s\n" % " ".join(sys.argv[1:]))
print("{}")
EOF
cat > scripts/lib/check-daemon-asset.py <<'EOF'
import os, sys
open(os.path.join(os.environ["T_LOG"], "ran.log"), "a").write("check-daemon-asset %s\n" % " ".join(sys.argv[1:]))
EOF
chmod +x scripts/*.sh
"$REAL_GIT" init -q -b main .
"$REAL_GIT" add -A
"$REAL_GIT" commit -q -m fixture
# Gitignored inputs the scratch tree gets from here, and the work in
# progress a rehearsal carries over (uncommitted + untracked).
printf 'T_FROM_DOTENV=1\n' > .env
mkdir -p src-tauri/binaries && printf 'frpc\n' > src-tauri/binaries/frpc-aarch64-apple-darwin
printf '\nA line still being written.\n' >> WHATS_NEW.md
mkdir -p notes && printf 'untracked\n' > notes/untracked.txt

snapshot() { # what must not change in the source checkout
    {
        "$REAL_GIT" -C "$src" rev-parse HEAD
        "$REAL_GIT" -C "$src" for-each-ref
        "$REAL_GIT" -C "$src" status --porcelain --untracked-files=all
        "$REAL_GIT" -C "$src" stash list
        shasum package.json src-tauri/tauri.conf.json cli/k2 cli/src/prelude.sh Cargo.lock WHATS_NEW.md \
            src-tauri/Cargo.toml crates/*/Cargo.toml
    } 2>&1
}
before="$(cd "$src" && snapshot)"

export PATH="$stub:$PATH"
export K2_RELEASE_LOCK="$work/k2-release.lock"
export K2_RELEASE_NODE_BIN="$work/no-k2-node"
export K2_RELEASE_DRY_ROOT="$work/dry"
tree="$work/dry/tree"

echo "A full rehearsal (compute mode, k2sign, Windows):"
set +e
(cd "$src" && bash scripts/release.sh --dry-run "$V") > "$work/run1.out" 2>&1
rc=$?
set -e
out="$(cat "$work/run1.out")"
has() { grep -qF -- "$1" "$work/run1.out"; }
check '[ "$rc" = 0 ]' "the rehearsal exits 0" "rc=$rc; tail: $(tail -30 "$work/run1.out")"
check 'has "DRY_EXIT=0"' "it ends with DRY_EXIT=0"
check '[ ! -s "$T_LOG/forbidden.log" ]' "no publishing stand-in was called" "$(cat "$T_LOG/forbidden.log")"
check '! has "GUARD"' "every publishing line has its own dry branch (the guard never fired)" "$(grep GUARD "$work/run1.out")"
for w in \
    "[dry] would: git push origin HEAD:main" \
    "[dry] would: git tag -fa v$V" \
    "[dry] would: git push origin refs/tags/v$V --force" \
    "[dry] would: gh release create v$V" \
    "[dry] would: gh variable set LINUX_BUILDS -b compute" \
    "[dry] would: gh release upload v$V" \
    "[dry] would: bash scripts/windows-patch-latest-json.sh $V" \
    "[dry] would: bash scripts/publish-web-bundles.sh $V" \
    "[dry] would: bash scripts/sign-daemon.sh $V --from-dir" \
    "[dry] would: bash scripts/sign-root-helper.sh $V" \
    "[dry] would: register the evidence with the signing user" \
    "[dry] skipped: the signing approval" \
    "[dry] skipped: update-key signature u1 of K2.app.tar.gz"; do
    check 'has "$w"' "prints: $w"
done
ran="$(cat "$T_LOG/ran.log")"
for r in rust-pin "sign-daemon --preflight" gates "sign-root-helper --preflight" "sign-k2-node --preflight" \
    "sign-app-engine --preflight" linux-gate compute-init compute-submit build standalone-start "vm-smoke $V" \
    "publish-web $V --skip-if-no-creds --dry-run" "windows " "check-daemon-asset" "oauth-check" "sign-k2-node --check" \
    "sign-app-engine --check" "evidence-bundle"; do
    check 'printf "%s\n" "$ran" | grep -qF -- "$r"' "ran for real: $r"
done
check 'grep -q "cargo test -p k2-release-keys" "$T_LOG/cargo.log"' "ran for real: the key copies test (Step 0.45)"
check 'grep -q "codesign --force --options runtime --timestamp" "$T_LOG/codesign.log"' "ran for real: Developer ID signing (Step 3)"
check '[ "$(grep -c "notarytool submit" "$T_LOG/xcrun.log")" = 3 ] && grep -q "stapler staple" "$T_LOG/xcrun.log"' \
    "ran for real: three notarizations and the staples" "$(cat "$T_LOG/xcrun.log")"
check 'grep -q "bun install --frozen-lockfile (in $tree)" "$T_LOG/bun.log"' "bun install --frozen-lockfile ran in the scratch tree" "$(cat "$T_LOG/bun.log")"
check 'grep -qx "k2sign compute-rehearse" "$T_LOG/k2sign.log" && grep -qx "k2sign status" "$T_LOG/k2sign.log"' \
    "the signer was asked only read-only things (status, compute-pins, compute-rehearse)" "$(cat "$T_LOG/k2sign.log")"
check '! grep -qvE "^k2sign (status|compute-pins|compute-rehearse)$" "$T_LOG/k2sign.log"' "and nothing else" "$(cat "$T_LOG/k2sign.log")"
check '! grep -qE "^gh (release|variable set|workflow|secret)" "$T_LOG/gh.log"' "gh: reads only" "$(cat "$T_LOG/gh.log")"

after="$(cd "$src" && snapshot)"
check '[ "$before" = "$after" ]' "the source checkout is untouched (HEAD, refs, status, version files)" "$(diff <(echo "$before") <(echo "$after"))"
check '[ -z "$("$REAL_GIT" -C "$src" tag -l)" ]' "no tag was made"
check '[ ! -e "$K2_RELEASE_LOCK" ] && grep -q "^pid=" "$T_LOG/lock-during-run"' "the release lock was held during the run and released after"
check '[ "$("$REAL_GIT" -C "$tree" rev-parse HEAD)" = "$("$REAL_GIT" -C "$src" rev-parse HEAD)" ]' "the scratch tree's HEAD stays at the base (no commit on it)"
check 'grep -q "\"version\": \"$V\"" "$tree/package.json" && grep -qx "K2_CLI_VERSION=\"$V\"" "$tree/cli/k2" && grep -qx "version = \"$V\"" "$tree/crates/k2-daemon/Cargo.toml"' \
    "the version is bumped in the scratch tree"
check '[ -f "$tree/notes/untracked.txt" ] && grep -q "A line still being written." "$tree/WHATS_NEW.md"' \
    "the source's untracked and uncommitted files are rehearsed"
check '[ -L "$tree/.env" ] && [ -f "$tree/src-tauri/binaries/frpc-aarch64-apple-darwin" ]' ".env is linked and src-tauri/binaries copied into the scratch tree"
wc="$(grep "^windows " "$T_LOG/ran.log" | awk '{print $2}' || true)"
check '[ -n "$wc" ] && [ "$("$REAL_GIT" -C "$src" show "$wc:package.json" | grep -c "\"version\": \"$V\"")" = 1 ] && [ -z "$("$REAL_GIT" -C "$src" for-each-ref --points-at "$wc")" ]' \
    "Windows built the dry release commit (bumped; no ref points at it)"
run="$(ls -d "$work/dry/runs/$V-"* | head -1)"
check '[ -f "$run/summary.txt" ] && grep -q "^DRY_EXIT=0$" "$run/summary.txt"' "the summary is saved in the run dir"
check 'grep -qE "^  8\.9 +pass .*would: 5" "$run/summary.txt" && grep -qE "^  1\.55 +skipped" "$run/summary.txt" && grep -qE "^  4 +pass" "$run/summary.txt"' \
    "the summary has each step's status, time and would count" "$(cat "$run/summary.txt")"
check 'python3 -c "import json,sys; m=json.load(open(sys.argv[1])); e=m[\"platforms\"][\"darwin-aarch64\"]; sys.exit(0 if m[\"version\"]==sys.argv[2] and e[\"signature\"].startswith(\"K2-DRY-RUN-NOT-SIGNED\") else 1)" "$run/latest.json" "$V"' \
    "latest.json is in the run dir, with a placeholder where the signature goes"
check 'grep -q "\"notes\"" "$run/daemon-latest.json"' "daemon-latest.json is in the run dir"
check 'has "✓ latest.json and daemon-latest.json"' "the manifests as written are checked (Step 10)"
check 'has "✓ the signing user'"'"'s compute-rehearse passed"' "the signer's compute rehearsal ran (Step 10.5)"
check 'has "✓ the staged deb/rpm/AppImage match SHA256SUMS-linux-x86_64.txt"' "the staged Linux packages are checked against their sums (Step 10.6)"
check 'has "Release lock kept"' "the scratch run keeps the lock the source run took (same pid)"

echo "A failing step (notarization refused):"
: > "$T_LOG/forbidden.log"
set +e
(cd "$src" && T_NOTARY_FAIL=1 bash scripts/release.sh --dry-run "$V") > "$work/run2.out" 2>&1
rc=$?
set -e
check '[ "$rc" != 0 ]' "the rehearsal fails" "rc=$rc"
check 'grep -qE "^  4 +FAIL" "$work/run2.out" && grep -qE "^DRY_EXIT=[1-9]" "$work/run2.out"' "the summary names Step 4 FAIL and DRY_EXIT != 0" "$(tail -30 "$work/run2.out")"
check '[ ! -e "$K2_RELEASE_LOCK" ]' "the lock is released after a failure"
check '[ ! -s "$T_LOG/forbidden.log" ]' "nothing publishing was called" "$(cat "$T_LOG/forbidden.log")"
check '[ "$before" = "$(cd "$src" && snapshot)" ]' "the source checkout is still untouched"
check 'grep -q "reset to" "$work/run2.out"' "a second rehearsal reuses (and resets) the scratch tree"

echo "Where the scratch tree goes by default:"
# Under the main checkout's .claude/worktrees/ (ignored there), beside the
# builders' worktrees: inside the workspace K2 Compute ships code from.
set +e
(cd "$src" && unset K2_RELEASE_DRY_ROOT && T_NOTARY_FAIL=1 bash scripts/release.sh --dry-run "$V") > "$work/run3.out" 2>&1
set -e
check '[ -f "$src/.claude/worktrees/release-dry/tree/scripts/release.sh" ] && grep -qF "scratch: $src/.claude/worktrees/release-dry/tree" "$work/run3.out"' \
    "default: <main checkout>/.claude/worktrees/release-dry/tree" "$(head -12 "$work/run3.out")"
check '[ "$before" = "$(cd "$src" && snapshot)" ]' "the source checkout's status is unchanged (the folder is gitignored)"
set +e
(cd "$src" && K2_RELEASE_DRY_ROOT="$src/not-ignored" bash scripts/release.sh --dry-run "$V") > "$work/run4.out" 2>&1
rc=$?
set -e
check '[ "$rc" = 1 ] && grep -q "where git doesn.t ignore it" "$work/run4.out" && [ ! -e "$src/not-ignored" ] && [ ! -e "$K2_RELEASE_LOCK" ]' \
    "a scratch root inside the checkout that git doesn't ignore is refused" "$(cat "$work/run4.out")"

echo "The guard (a publishing command without a dry branch):"
: > "$T_LOG/forbidden.log"
gout="$(cd "$tree" && K2_RELEASE_DRY=1 K2_RELEASE_DRY_RUN_DIR="$work/guard" bash -c '
    set -euo pipefail
    mkdir -p "$K2_RELEASE_DRY_RUN_DIR"; : > "$K2_RELEASE_DRY_RUN_DIR/notes.tsv"
    source scripts/lib/release-dry.sh
    git push origin HEAD:main
    git -C . tag -fa v1 -m x
    git tag -l > /dev/null
    git rev-parse HEAD > /dev/null
    gh release create v1 a.dmg
    gh api -X POST repos/x/y/releases
    gh api repos/x/y -f name=z
    gh variable get LINUX_BUILDS > /dev/null
    curl -T file https://uploads.example.invalid/x
    curl -X PUT https://example.invalid/x
    k2dry_failed && echo FAILED-NOTED' 2>&1)"
check '[ ! -s "$T_LOG/forbidden.log" ]' "the guard caught git push/tag, gh release/api writes and curl uploads before the stand-ins" "$(cat "$T_LOG/forbidden.log")"
check '[ "$(grep -c "GUARD" <<<"$gout")" = 7 ] && grep -q FAILED-NOTED <<<"$gout"' "each is reported and fails the rehearsal" "$gout"
check '[ "$(grep -c "^gh variable get" "$T_LOG/gh.log")" -ge 1 ]' "reads pass through"

echo "Outside a dry run:"
check '[ "$(K2_RELEASE_DRY=0 bash -c "source \"$ROOT/scripts/lib/release-dry.sh\"; type -t git; type -t gh; type -t curl")" = "$(printf "file\nfile\nfile")" ]' \
    "no guard functions: git, gh and curl are the commands themselves"
check '! (cd "$src" && K2_RELEASE_DRY=2 bash scripts/release.sh "$V") > "$work/bad.out" 2>&1 && grep -q "K2_RELEASE_DRY must be 0 or 1" "$work/bad.out"' \
    "K2_RELEASE_DRY other than 0/1 is refused"
: > "$T_LOG/forbidden.log"
set +e
(cd "$src" && K2_RELEASE_DRY=1 K2_RELEASE_DRY_INNER=1 K2_RELEASE_DRY_TREE="$src" K2_RELEASE_DRY_SRC="$src" \
    K2_RELEASE_DRY_RUN_DIR="$work/inner" bash scripts/release.sh "$V") > "$work/inner.out" 2>&1
rc=$?
set -e
check '[ "$rc" = 1 ] && grep -q "runs only in its scratch tree" "$work/inner.out" && [ ! -s "$T_LOG/forbidden.log" ] && [ "$before" = "$(cd "$src" && snapshot)" ]' \
    "a hand-set K2_RELEASE_DRY_INNER can't make the rehearsal run in the real checkout" "$(cat "$work/inner.out")"
check '[ ! -e "$K2_RELEASE_LOCK" ]' "and the lock is released"

echo ""
echo "release-dry-run: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
