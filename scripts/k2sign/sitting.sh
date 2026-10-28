#!/bin/bash
# sitting.sh — Rosson's sittings on z3mbpZ for the signing user
# (prd-signing-user-v1 rev 2 §7). Run from the release checkout, in a GUI
# iTerm on z3mbpZ (not SSH), at the REVIEWED commit K2 names.
#
#   bash scripts/k2sign/sitting.sh check          read-only: is z3mbpZ ready?
#   bash scripts/k2sign/sitting.sh 0              S0 spike (a)(b): Touch ID with a THROWAWAY key   ~10 min
#   bash scripts/k2sign/sitting.sh 1              the user, the signer, the approver               ~20 min
#   bash scripts/k2sign/sitting.sh 2              the keys: U2 inside _k2sign, U3, U1 import        ~30 min
#   bash scripts/k2sign/sitting.sh 3              after the pubkeys commit lands: pin, prove        ~10 min
#
# Each prints its exact commands and changes NOTHING (a dry run) unless you
# add --run; with --run it stops before every step and waits for Enter.
# Every `sudo` asks for the admin password (never Touch ID: pam_tid for sudo
# must stay off, A11). Nothing here prints a secret.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
TREE="$(cd "$HERE/../.." && pwd)"
LIBEXEC=/usr/local/libexec/k2sign
APP="$HOME/Applications/K2 Release Approve.app"
WHICH="${1:-}"; RUN=0
[ "${2:-}" = --run ] && RUN=1

ok() { printf '  \033[32mok\033[0m  %s\n' "$*"; }
bad() { printf '  \033[1;31mNO\033[0m  %s\n' "$*"; BAD=$((BAD + 1)); }
note() { printf '  --  %s\n' "$*"; }
BAD=0
# step "<what you do>" <command...>: print it; with --run, wait for Enter and run it.
step() {
    local what="$1"; shift
    printf '\n\033[1m▶ %s\033[0m\n    %s\n' "$what" "$*"
    if [ "$RUN" = 1 ]; then
        read -r -p "    Enter to run (Ctrl-C stops): " _
        "$@"
    fi
}
# expect_fail "<what>" <command...>: the command must FAIL (an isolation proof).
expect_fail() {
    local what="$1"; shift
    printf '\n\033[1m▶ check: %s\033[0m\n    %s   (must fail)\n' "$what" "$*"
    if [ "$RUN" = 1 ]; then
        if "$@" >/dev/null 2>&1; then printf '  \033[1;31mFAILED: that worked, it must not. Stop and tell K2.\033[0m\n'; exit 1; fi
        ok "refused, as it should be"
    fi
}

check() {
    echo "z3mbpZ readiness (read-only):"
    [ "$(uname -s)" = Darwin ] && ok "macOS $(sw_vers -productVersion)" || bad "not macOS"
    [ "$(scutil --get LocalHostName 2>/dev/null)" = z3mbpZ ] && ok "this is z3mbpZ" || bad "this is $(scutil --get LocalHostName 2>/dev/null), not z3mbpZ"
    if [ -n "${SSH_CONNECTION:-}" ]; then bad "this is an SSH session: run the sittings in a GUI iTerm"; else ok "a local (GUI) terminal"; fi
    free_gb="$(df -g /System/Volumes/Data | awk 'NR==2{print $4}')"
    [ "$free_gb" -ge 60 ] && ok "disk: ${free_gb} GiB free" || bad "disk: only ${free_gb} GiB free (a release needs ~60; prd §6 #5)"
    [ ! -e /etc/pam.d/sudo_local ] || ! grep -Eq '^[[:space:]]*auth.*pam_tid' /etc/pam.d/sudo_local \
        && ok "no Touch ID for sudo (A11)" || bad "/etc/pam.d/sudo_local enables pam_tid: remove that line first"
    [ -x /Library/Developer/CommandLineTools/usr/bin/python3 ] && ok "Command Line Tools python3 present" \
        || bad "no Command Line Tools: run 'xcode-select --install' (the signer can't use Xcode.app's python: /Applications is admin-writable)"
    command -v swiftc >/dev/null && ok "swiftc (for K2 Release Approve)" || bad "no swiftc"
    command -v minisign >/dev/null && ok "minisign (the release side's alarm checks)" || bad "brew install minisign"
    n="$(bioutil -c 2>/dev/null | grep -Eo '[0-9]+ biometric template' | grep -Eo '^[0-9]+' | head -1 || true)"
    [ "${n:-0}" -ge 1 ] && ok "Touch ID: ${n} fingerprint(s) enrolled (enrol a second one: .biometryCurrentSet keys die if the set changes)" || bad "no Touch ID fingerprint enrolled"
    who | grep -q "^$(id -un) *console" && ok "$(id -un) is logged in at the console" || bad "nobody is logged in at the console"
    agents="$(ps -axo user=,command= | awk -v u="$(id -un)" '$1==u' | grep -Ec 'claude|grok|codex' || true)"
    [ "$agents" = 0 ] && ok "no Claude/Grok/Codex processes as $(id -un)" || note "$agents Claude/Grok/Codex processes run as $(id -un): pause them for sitting 2 (quit K2 and those terminals)"
    if id _k2sign >/dev/null 2>&1; then note "_k2sign exists (uid $(id -u _k2sign)): sitting 1 is done or half-done"; else note "no _k2sign yet"; fi
    printf '  --  release checkout: %s at %s\n' "$TREE" "$(git -C "$TREE" rev-parse --short HEAD)"
    bash "$HERE/install.sh" --dry-run > /tmp/k2sign-dry.$$ 2>&1 && ok "install.sh --dry-run passes its checks" || { bad "install.sh --dry-run: $(tail -1 /tmp/k2sign-dry.$$)"; }
    rm -f /tmp/k2sign-dry.$$
    [ "$BAD" = 0 ] && echo "ready." || { echo "$BAD thing(s) to fix first."; return 1; }
}

case "$WHICH" in
check) check ;;
0)
    echo "S0 spike (a)(b) — a THROWAWAY Secure Enclave key in a temp folder; nothing installed, nothing kept."
    S0="${TMPDIR:-/tmp}/k2-s0"
    step "Build the approval app ad hoc into a temp folder" bash "$HERE/approve/build.sh" --out "$S0/adhoc"
    step "(a) Enrol a throwaway key: TOUCH the sensor when asked" "$S0/adhoc/K2 Release Approve.app/Contents/MacOS/K2 Release Approve" --enrol --key-dir "$S0/key"
    step "(a+c) Make a test challenge" /bin/sh -c "printf '%s' '{\"assets\":[\"K2.app.tar.gz\"],\"base_commit\":\"0000000000000000000000000000000000000000\",\"expires\":\"2099-01-01T00:00:00Z\",\"kind\":\"k2-release-approval\",\"version\":\"9.9.9\",\"windows\":false,\"reapproval\":false}' > '$S0/c.json'"
    step "(b) Approve it through 'open -W' (as release.sh does): TOUCH the sensor" /usr/bin/open -W -n -a "$S0/adhoc/K2 Release Approve.app" --args --key-dir "$S0/key" --challenge "$S0/c.json" --out "$S0/c.der"
    step "(c) LibreSSL verifies the Secure Enclave signature (must say Verified OK)" /usr/bin/openssl dgst -sha256 -verify "$S0/key/approver.pem" -signature "$S0/c.der" "$S0/c.json"
    step "(a) The same with a Developer ID-signed build (no entitlement; AMFI must not kill it)" bash "$HERE/approve/build.sh" --out "$S0/devid" --developer-id
    step "(a) Approve with the Developer ID build: TOUCH the sensor" /usr/bin/open -W -n -a "$S0/devid/K2 Release Approve.app" --args --key-dir "$S0/key" --challenge "$S0/c.json" --out "$S0/c2.der"
    step "(c) Verified OK again" /usr/bin/openssl dgst -sha256 -verify "$S0/key/approver.pem" -signature "$S0/c2.der" "$S0/c.json"
    printf '\n▶ (b) from SSH: from another Mac, ssh z3thon@z3mbpZ and run the "open -W" line above;\n    the Touch ID sheet must appear on z3mbpZ'\''s screen. Then:\n'
    step "Remove everything the spike made" /bin/rm -rf "$S0"
    echo; echo "Tell K2 which of (a) ad hoc, (a) Developer ID, (b) iTerm, (b) SSH, (c) passed."
    ;;
1)
    echo "Sitting 1 (prd §7): the signing user, the signer, the approver. Admin password: 2-3 times. Touch ID: 1-2 times."
    [ "$RUN" = 1 ] && { check || { echo "fix the above first"; exit 1; }; }
    step "Dry run of the installer (no sudo; shows every change it will make)" bash "$HERE/install.sh" --dry-run
    step "Install: create _k2sign, the folders, pinned minisign, the signer, the sudoers rule (ADMIN PASSWORD)" sudo bash "$HERE/install.sh"
    note "K2 checks the printed hashes against the branch: paste them to K2."
    expect_fail "S1-1 this user can't list /var/k2sign" ls /var/k2sign
    expect_fail "S1-2 this user can't run anything else as _k2sign without a password" sudo -n -u _k2sign /bin/cat /var/k2sign/policy
    step "The signer answers (its preflight must refuse: no keys yet)" /bin/sh -c "sudo -n -u _k2sign $LIBEXEC/k2sign preflight 0.46.6; echo \"exit \$? (3 expected)\""
    step "Build K2 Release Approve into ~/Applications" bash "$HERE/approve/build.sh"
    step "Enrol the approver: TOUCH the sensor" "$APP/Contents/MacOS/K2 Release Approve" --enrol
    step "Install the approver's public key (ADMIN PASSWORD)" sudo "$LIBEXEC/k2sign-admin" set-approver "$HOME/.k2-release-approve/approver.pem"
    echo; echo "Sitting 1 done. Next: sitting 2 (the keys), with agents paused."
    ;;
2)
    echo "Sitting 2 (prd §7): the keys. PAUSE same-user agents first: quit K2 (app + daemon sessions) and every Claude/Grok/Codex terminal."
    echo "Have ready: U1's password (from your password manager), and a NEW long password for U3."
    if [ "$RUN" = 1 ]; then
        agents="$(ps -axo user=,command= | awk -v u="$(id -un)" '$1==u' | grep -Ec 'claude|grok|codex' || true)"
        [ "$agents" = 0 ] || { echo "  $agents Claude/Grok/Codex processes still run as $(id -un). Quit them, then run again."; exit 1; }
    fi
    step "U2 inside _k2sign: random password, never shown; prints U2's PUBLIC line (ADMIN PASSWORD)" sudo "$LIBEXEC/k2sign-admin" generate-u2
    step "U3: the existing ceremony's U3 part (password manager: the key FILE in one entry, its password in ANOTHER)" bash "$TREE/scripts/updater-key-ceremony.sh" generate --signer k2sign
    note "Send K2 the two PUBLIC lines it printed (safe to share). K2 lands them with the signing lane."
    step "U1 into _k2sign: it asks for U1's password, not shown (ADMIN PASSWORD first)" sudo "$LIBEXEC/k2sign-admin" import u1 "$HOME/.tauri/k2so-updater.key"
    echo; echo "Cleanup (prd §6 items 1-4), by hand, in this order:"
    echo "    # 2. U1's password out of every .env (check it's in your password manager first):"
    echo "    for d in K2 K2-0460 K2-zen-s0 K2SO; do f=~/DevProjects/Alakazam\\ Labs/\$d/.env; [ -f \"\$f\" ] && sed -i '' '/TAURI_SIGNING_PRIVATE_KEY/d' \"\$f\" && chmod 600 \"\$f\"; done"
    echo "    grep -l TAURI_SIGNING ~/DevProjects/*/*/.env    # must print nothing"
    echo "    # 3. chmod 600 ~/.tauri/k2so-updater.key   (delete ~/.tauri after the first green k2sign release)"
    echo "    # 4. ASC keys: keep one AuthKey_*.p8 in one 0700 folder, chmod 600, point .env there"
    echo "    # 1. on z3mbp (not here): confirm both entries are in the password manager, then rm -P ~/k2-root-helper-keys-SAVE-THEN-DELETE.txt"
    echo; echo "Then un-pause the agents."
    ;;
3)
    echo "Sitting 3 (after K2 says the pubkeys commit landed): pin U2/U3, prove it, one Touch ID."
    step "Update the checkout to the commit K2 names" git -C "$TREE" pull --ff-only
    step "Re-install from it: pins U2/U3, refuses if the tree's U2 isn't the one generate-u2 made (ADMIN PASSWORD)" sudo bash "$HERE/install.sh"
    step "Preflight as the release does it (must end 'preflight ok')" bash "$TREE/scripts/sign-daemon.sh" --preflight 0.46.6
    # S2-6 uses version 0.0.1 so it never uses up a real release's one
    # session (versions only go up: 0.0.1 is below every real one, and this
    # works only before the first real approval).
    step "S2-6: one Touch ID opens a session (throwaway version 0.0.1): TOUCH the sensor" /bin/bash -c "source '$TREE/scripts/lib/sign-updater.sh'; source '$TREE/scripts/lib/sign-approve.sh'; k2sa_open_session 0.0.1 \$(git -C '$TREE' rev-parse HEAD) K2.app.tar.gz k2-daemon-macos-aarch64 k2-daemon-linux-x86_64 k2-daemon-linux-aarch64"
    step "Status shows the session, its assets and its 12 h expiry" sudo -n -u _k2sign "$LIBEXEC/k2sign" status 0.0.1
    expect_fail "a real file is still refused (the S3 checks aren't built yet)" /bin/sh -c "echo x | sudo -n -u _k2sign $LIBEXEC/k2sign sign u2 K2.app.tar.gz 0.0.1 raw"
    note "Done. The first real release asks for its own Touch ID at Step 0.46 (once S3 has landed)."
    ;;
*) sed -n '2,17p' "$0"; exit 2 ;;
esac
