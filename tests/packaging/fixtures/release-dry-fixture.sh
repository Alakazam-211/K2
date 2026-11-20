#!/usr/bin/env bash
# The fixture the release.sh dry-run tests share (tests/packaging/
# release-dry-run.sh and release-timings.sh): stubs on PATH for every
# external command and a fixture repository holding the real release.sh,
# release-lock.sh, daemon-manifest.sh, release-dry.sh, notary.sh,
# release-timer.sh and release-timings.sh, with stand-ins that log for the
# rest (signers, gates, compute, Windows, R2).
#
#   k2_release_dry_fixture
# needs: ROOT (this repo), V (the version), src (the fixture repo dir, made
# here), stub (the stub dir, on PATH later), T_LOG (logs), REAL_GIT; HOME
# and the git identity set by the caller. Leaves the caller in "$src", with
# one commit, plus an uncommitted WHATS_NEW.md line, an untracked file,
# .env and src-tauri/binaries (what a rehearsal carries over).
# Publishing commands log to $T_LOG/forbidden.log; the rest log and succeed.
k2_release_dry_fixture() {
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
# release-dry.sh, daemon-manifest.sh, notary.sh and release-repo.sh;
# stand-ins for the rest ──
cd "$src"
mkdir -p scripts/lib scripts/ci cli/src src-tauri crates/k2-core crates/k2-daemon crates/k2-menubar
cp "$ROOT/scripts/release.sh" "$ROOT/scripts/release-lock.sh" "$ROOT/scripts/daemon-manifest.sh" "$ROOT/scripts/release-timings.sh" scripts/
cp "$ROOT/scripts/lib/release-dry.sh" "$ROOT/scripts/lib/notary.sh" "$ROOT/scripts/lib/release-timer.sh" "$ROOT/scripts/lib/release-repo.sh" scripts/lib/
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
    k2_screen_helper_verify_tarball k2_standalone_macos_daemon_poll k2_standalone_macos_daemon_wait k2_standalone_macos_daemon_kill \
    k2_mac_k5_cli_targets_check k2_k5_cli_sign k2_k5_cli_verify_signed k2_k5_cli_verify_shipped k2_k5_cli_verify_tarball; do
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
}
