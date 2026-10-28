#!/bin/bash
# sitting.sh — Rosson's sittings on z3mbpZ for the signing user
# (prd-signing-user-v1 rev 2 §7; runbook docs/release/updater-key-ceremony.md,
# "The signing user"). In a GUI iTerm on z3mbpZ (not SSH), in the release
# checkout, on the commit K2 named:
#
#   bash scripts/k2sign/sitting.sh check                      read-only: is z3mbpZ ready?        1 min
#   bash scripts/k2sign/sitting.sh 0 [--run]                  S0 spike, THROWAWAY key           ~10 min
#   bash scripts/k2sign/sitting.sh 1 --commit <sha> [--run]   user, signer, approver, token      ~25 min
#   bash scripts/k2sign/sitting.sh 2 --commit <sha> [--run]   the keys (agents paused)           ~30 min
#   bash scripts/k2sign/sitting.sh 3 --commit <sha> [--run]   after the public keys land         ~15 min
#
# Without --run it only PRINTS the steps (read them first). With --run it
# stops before each step: Enter runs it, s skips it (for a re-run after a
# stop), Ctrl-C ends the sitting. A failed step stops the sitting; fix it,
# then re-run with --run and press s for the steps already done.
# `sudo` asks for your Mac admin password (never Touch ID). Touch ID is asked
# only by K2 Release Approve. Nothing here prints a secret.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
TREE="$(cd "$HERE/../.." && pwd)"
LIBEXEC=/usr/local/libexec/k2sign
APP="$HOME/Applications/K2 Release Approve.app"
APPBIN="$APP/Contents/MacOS/K2 Release Approve"
U1_KEY="$HOME/.tauri/k2so-updater.key"
WHICH="${1:-}"; shift || true
RUN=0 COMMIT=""
while [ $# -gt 0 ]; do
    case "$1" in
        --run) RUN=1 ;;
        --commit) COMMIT="${2:-}"; shift ;;
        *) echo "sitting.sh: unknown argument $1" >&2; exit 2 ;;
    esac
    shift
done

ok() { printf '  \033[32mok\033[0m  %s\n' "$*"; }
bad() { printf '  \033[1;31mNO\033[0m  %s\n' "$*"; BAD=$((BAD + 1)); }
note() { printf '  --  %s\n' "$*"; }
BAD=0
# A command as it can be pasted into zsh or bash (spaces and quotes escaped).
show() { local a out=""; for a in "$@"; do out="$out$(printf '%q' "$a") "; done; printf '%s' "${out% }"; }
ask() { # → 0 run, 1 skip
    local r
    read -r -p "    Enter = run it, s = skip it (already done), Ctrl-C = stop: " r
    [ "$r" != s ]
}
# step "<what you do / what you'll see>" <command...>
step() {
    local what="$1"; shift
    printf '\n\033[1m▶ %s\033[0m\n    %s\n' "$what" "$(show "$@")"
    if [ "$RUN" = 1 ] && ask; then "$@"; fi
}
# must_fail "<what>" <command...>: an isolation proof; the command must FAIL.
must_fail() {
    local what="$1"; shift
    printf '\n\033[1m▶ check: %s\033[0m\n    %s   (must FAIL)\n' "$what" "$(show "$@")"
    if [ "$RUN" = 1 ] && ask; then
        if "$@" >/dev/null 2>&1; then printf '  \033[1;31mThat worked; it must not. Stop here and tell K2.\033[0m\n'; exit 1; fi
        ok "refused, as it should be"
    fi
}
# must_say "<what>" "<text>" <command...>: the command must fail with <text>.
must_say() {
    local what="$1" text="$2" out; shift 2
    printf '\n\033[1m▶ check: %s\033[0m\n    %s   (must fail saying: %s)\n' "$what" "$(show "$@")" "$text"
    if [ "$RUN" = 1 ] && ask; then
        if out="$("$@" 2>&1)"; then printf '  \033[1;31mThat worked; it must not. Stop and tell K2.\033[0m\n'; exit 1; fi
        if printf '%s' "$out" | grep -qF -- "$text"; then ok "refused for the right reason"; else
            printf '  \033[1;31mRefused, but not for that reason:\033[0m %s\n  Stop and tell K2.\n' "$(printf '%s' "$out" | tail -1)"; exit 1
        fi
    fi
}
agents_running() { /usr/bin/pgrep -u "$(id -un)" -f 'claude|grok|codex|k2-ptyd' 2>/dev/null | wc -l | tr -d ' '; }
need_commit() {
    local head
    head="$(git -C "$TREE" rev-parse HEAD)"
    if [ -z "$COMMIT" ]; then
        echo "Sitting $WHICH runs on the commit K2 names. Add --commit <sha> (K2 gives it to you). This checkout is at ${head:0:12}."
        [ "$RUN" = 0 ] || exit 1
        return 0
    fi
    case "$head" in
        "$COMMIT"*) ok "this checkout is at the commit K2 named (${head:0:12})" ;;
        *) bad "this checkout is at ${head:0:12}, not ${COMMIT}. Get it: cd $(show "$TREE") && git fetch origin && git checkout --detach $COMMIT"
           [ "$RUN" = 0 ] || exit 1 ;;
    esac
}
how_to_run() {
    printf '\n(That was a dry run. To do it: cd %s && bash scripts/k2sign/sitting.sh %s%s --run)\n' \
        "$(show "$TREE")" "$WHICH" "${COMMIT:+ --commit $COMMIT}"
}

check() {
    echo "z3mbpZ readiness (read-only):"
    [ "$(uname -s)" = Darwin ] && ok "macOS $(sw_vers -productVersion)" || bad "not macOS"
    [ "$(scutil --get LocalHostName 2>/dev/null)" = z3mbpZ ] && ok "this is z3mbpZ" || bad "this is $(scutil --get LocalHostName 2>/dev/null), not z3mbpZ"
    if [ -n "${SSH_CONNECTION:-}" ]; then bad "this is an SSH session: run the sittings in a GUI iTerm"; else ok "a local (GUI) terminal"; fi
    free_gb="$(df -g /System/Volumes/Data | awk 'NR==2{print $4}')"
    [ "$free_gb" -ge 60 ] && ok "disk: ${free_gb} GiB free" || bad "disk: only ${free_gb} GiB free (a release needs ~60; runbook cleanup)"
    if [ -e /etc/pam.d/sudo_local ] && grep -Eq '^[[:space:]]*auth.*pam_tid' /etc/pam.d/sudo_local; then
        bad "/etc/pam.d/sudo_local turns on Touch ID for sudo: remove that line first (a sudo Touch ID sheet could pass for the release approval)"
    else ok "no Touch ID for sudo"; fi
    [ -x /Library/Developer/CommandLineTools/usr/bin/python3 ] && ok "Command Line Tools python3 present" \
        || bad "no Command Line Tools: run 'xcode-select --install' (the signer can't use Xcode.app's python: /Applications is admin-writable)"
    command -v swiftc >/dev/null && ok "swiftc (to build K2 Release Approve)" || bad "no swiftc"
    command -v minisign >/dev/null && ok "minisign (the release side's checks)" || bad "brew install minisign"
    n="$(bioutil -c 2>/dev/null | grep -Eo '[0-9]+ biometric template' | grep -Eo '^[0-9]+' | head -1 || true)"
    [ "${n:-0}" -ge 1 ] && ok "Touch ID: ${n} fingerprint(s). Add any extra fingerprint NOW, before sitting 1: adding or removing one later kills the approver key (then: re-enrol + set-approver)" \
        || bad "no Touch ID fingerprint enrolled"
    who | grep -q "^$(id -un) *console" && ok "$(id -un) is logged in at the console" || bad "nobody is logged in at the console"
    a="$(agents_running)"
    [ "$a" = 0 ] && ok "no Claude/Grok/Codex/k2-ptyd process as $(id -un)" || note "$a Claude/Grok/Codex/k2-ptyd processes run as $(id -un): fine now, but sitting 2 needs them stopped"
    [ -f "$U1_KEY" ] && ok "U1's key file is at $U1_KEY" || bad "no $U1_KEY (sitting 2 imports it from there)"
    if id _k2sign >/dev/null 2>&1; then note "_k2sign exists (uid $(id -u _k2sign)): sitting 1 is done or half-done"; else note "no _k2sign yet"; fi
    note "this checkout: $TREE at $(git -C "$TREE" rev-parse --short HEAD)"
    if bash "$HERE/install.sh" --dry-run > "${TMPDIR:-/tmp}/k2sign-dry.$$" 2>&1; then ok "install.sh --dry-run passes its checks"; else
        bad "install.sh --dry-run: $(tail -1 "${TMPDIR:-/tmp}/k2sign-dry.$$")"; fi
    rm -f "${TMPDIR:-/tmp}/k2sign-dry.$$"
    [ "$BAD" = 0 ] && echo "ready." || { echo "$BAD thing(s) to fix first."; return 1; }
}

case "$WHICH" in
check) check ;;
0)
    echo "Sitting 0 — S0 spike (a)(b)(c): does Touch ID approval work on this Mac? A THROWAWAY Secure Enclave key"
    echo "in a temp folder; nothing is installed or kept. Touch ID: 4 times. Wait for K2's go before sitting 1."
    S0="${TMPDIR:-/tmp}/k2-s0"
    CH="{\"assets\":[\"K2.app.tar.gz\"],\"base_commit\":\"0000000000000000000000000000000000000000\",\"expires\":\"2099-01-01T00:00:00Z\",\"kind\":\"k2-release-approval\",\"reapproval\":false,\"version\":\"9.9.9\",\"windows\":false}"
    step "Build the approval app (ad hoc signed) into a temp folder" bash "$HERE/approve/build.sh" --out "$S0/adhoc"
    step "(a) Enrol a throwaway key: TOUCH the sensor when the sheet appears" "$S0/adhoc/K2 Release Approve.app/Contents/MacOS/K2 Release Approve" --enrol --key-dir "$S0/key"
    step "Write a test challenge" /bin/sh -c 'printf "%s" "$1" > "$2"' sh "$CH" "$S0/c.json"
    step "(b) Approve it through 'open -W', as release.sh does: TOUCH the sensor (the app prints nothing here)" \
        /usr/bin/open -W -n -a "$S0/adhoc/K2 Release Approve.app" --args --key-dir "$S0/key" --challenge "$S0/c.json" --out "$S0/c.der"
    step "(c) Check it. 'Verified OK' = pass. 'No such file' = the approval failed: the reason is in c.der.err (next step)" \
        /usr/bin/openssl dgst -sha256 -verify "$S0/key/approver.pem" -signature "$S0/c.der" "$S0/c.json"
    step "(only if the last step failed) Show why" /bin/cat "$S0/c.der.err"
    step "(a) Build it again, signed with the Developer ID (may ask for your login keychain password)" bash "$HERE/approve/build.sh" --out "$S0/devid" --developer-id
    step "(a) Approve with the Developer ID build: TOUCH the sensor" \
        /usr/bin/open -W -n -a "$S0/devid/K2 Release Approve.app" --args --key-dir "$S0/key" --challenge "$S0/c.json" --out "$S0/c2.der"
    step "(c) 'Verified OK' = pass" /usr/bin/openssl dgst -sha256 -verify "$S0/key/approver.pem" -signature "$S0/c2.der" "$S0/c.json"
    printf '\n\033[1m▶ (b) over SSH, BEFORE the next step: from another Mac run this one line, then TOUCH the sensor on z3mbpZ:\033[0m\n    ssh z3thon@z3mbpZ %q\n' \
        "$(show /usr/bin/open -W -n -a "$S0/adhoc/K2 Release Approve.app" --args --key-dir "$S0/key" --challenge "$S0/c.json" --out "$S0/c3.der")"
    step "(b) Check the SSH approval: 'Verified OK' = pass" /usr/bin/openssl dgst -sha256 -verify "$S0/key/approver.pem" -signature "$S0/c3.der" "$S0/c.json"
    step "Remove everything the spike made" /bin/rm -rf "$S0"
    echo; echo "Tell K2 pass/fail for: (a) ad hoc, (a) Developer ID, (b) iTerm, (b) SSH, (c). Wait for K2's go before sitting 1."
    [ "$RUN" = 1 ] || how_to_run
    ;;
1)
    echo "Sitting 1 — the signing user, the signer, the approver, the signer's GitHub token."
    echo "You'll type: your Mac admin password (2-3 times) and paste one new GitHub token. Touch ID: once."
    need_commit
    if [ "$RUN" = 1 ]; then check || { echo "fix the above first"; exit 1; }; fi
    step "Dry run of the installer (no sudo; it lists every change it will make)" bash "$HERE/install.sh" --dry-run
    step "Install: the _k2sign user, folders, pinned minisign and gh, the signer, the sudoers rule (Password: = your Mac admin password)" sudo bash "$HERE/install.sh"
    note "Send K2 the lines under 'Hashes' (they are also in /usr/local/etc/k2sign/pins.sha256). Don't start sitting 2 until K2 says they match."
    step "Forget the cached admin password (so the next checks prove something)" sudo -k
    must_fail "this user can't read the signer's folder" /bin/ls /var/k2sign
    must_fail "this user can't run anything else as _k2sign without a password" sudo -n -u _k2sign /usr/bin/true
    must_say "the signer answers, and refuses (no keys yet)" "no U2 public key pinned" sudo -n -u _k2sign "$LIBEXEC/k2sign" preflight 0.46.6
    step "Build K2 Release Approve into ~/Applications" bash "$HERE/approve/build.sh"
    step "Enrol the approver: TOUCH the sensor when the sheet appears" "$APPBIN" --enrol
    step "Give the signer the approver's public key (Password: = your Mac admin password)" sudo "$LIBEXEC/k2sign-admin" set-approver "$HOME/.k2-release-approve/approver.pem"
    printf '\n\033[1m▶ In a browser: make the signer'"'"'s own GitHub token (read-only, about 3 min)\033[0m\n'
    echo "    github.com → your picture → Settings → Developer settings → Personal access tokens → Fine-grained tokens → Generate new token"
    echo "    Token name: k2sign z3mbpZ    Resource owner: Alakazam-211    Expiration: 1 year (or the longest allowed)"
    echo "    Repository access: Only select repositories → K2"
    echo "    Repository permissions: Contents → Read-only; Attestations → Read-only (Metadata read-only is added for you). Nothing else."
    echo "    Generate token and copy it (GitHub shows it once). If it says the token is pending, approve it:"
    echo "    github.com/organizations/Alakazam-211/settings/personal-access-token-requests"
    step "Give the token to the signer. If you see 'Password:', that's your Mac admin password; at 'GitHub token (not shown):' paste the token" \
        sudo "$LIBEXEC/k2sign-admin" set-gh-token
    note "Now copy any other text, so the token isn't left on the clipboard."
    step "S0 (d): the signer's gh, token and network verify an attestation (must end 'gh-selfcheck ok')" sudo -n -u _k2sign "$LIBEXEC/k2sign" gh-selfcheck
    step "Forget the cached admin password" sudo -k
    echo; echo "Sitting 1 done. Next: sitting 2, which K2 schedules with you (agents must be stopped)."
    [ "$RUN" = 1 ] || how_to_run
    ;;
2)
    echo "Sitting 2 — the keys. U2 is made inside _k2sign (nobody sees its password); U3 goes to your password manager; U1 moves in."
    echo "BEFORE you start:"
    echo "  - U1's password must be in your password manager. Today it's the TAURI_SIGNING_PRIVATE_KEY_PASSWORD= value in the K2"
    echo "    checkout's .env: open .env in an editor (don't cat it), copy the value into a password-manager entry 'K2 update key U1'."
    echo "  - Make a NEW long password for U3 in your password manager (entry 'K2 update key U3 - password')."
    echo "  - Stop every agent: quit K2 from its menu-bar icon, then run 'k2 daemon stop' (it ends agent sessions), and close every"
    echo "    Claude/Grok/Codex terminal. The script checks none is left."
    echo "You'll type: your Mac admin password (1-2 times), U3's new password (twice), 'saved', U1's password (once)."
    echo "After pasting a password, copy any other text so it isn't left on the clipboard."
    need_commit
    if [ "$RUN" = 1 ]; then
        a="$(agents_running)"
        [ "$a" = 0 ] || { echo "  $a Claude/Grok/Codex/k2-ptyd processes still run as $(id -un) (pgrep -fl 'claude|grok|codex|k2-ptyd' lists them). Stop them, then run again."; exit 1; }
        [ -f "$U1_KEY" ] || { echo "  no $U1_KEY: tell K2 where U1's key file is."; exit 1; }
        base64 -D < "$U1_KEY.pub" 2>/dev/null | head -1 | grep -q E9111446F65EBB05 \
            || { echo "  $U1_KEY.pub is not U1 (E9111446F65EBB05): tell K2."; exit 1; }
        ok "agents stopped; U1's key file is $U1_KEY (id E9111446F65EBB05)"
    fi
    step "U2 inside _k2sign (random password, never shown); prints U2's PUBLIC line (Password: = Mac admin password)" sudo "$LIBEXEC/k2sign-admin" generate-u2
    step "U3: type U3's new password twice at minisign's prompts; attach the key file to a password-manager entry; type 'saved'" \
        bash "$TREE/scripts/updater-key-ceremony.sh" generate --signer k2sign
    note "The two PUBLIC lines are saved in ~/k2-update-pubkeys.txt (safe to share). If the U3 step stopped, re-run with --run and press s for the U2 step."
    step "U1 into _k2sign: 'Password:' = Mac admin password, then 'U1's password (not shown):' = U1's password" sudo "$LIBEXEC/k2sign-admin" import u1 "$U1_KEY"
    step "Forget the cached admin password" sudo -k
    echo
    echo "Cleanup, by hand (copy each line):"
    echo "  # 1. U1's password out of every .env under ~/DevProjects (it's in your password manager now):"
    echo "  find ~/DevProjects -maxdepth 4 -name .env -exec grep -l TAURI_SIGNING {} +   # lists them"
    echo "  find ~/DevProjects -maxdepth 4 -name .env -exec sed -i '' '/TAURI_SIGNING/d' {} \\; -exec chmod 600 {} \\;"
    echo "  find ~/DevProjects -maxdepth 4 -name .env -exec grep -l TAURI_SIGNING {} +   # must print nothing"
    echo "  # 2. U1's key file: owner-only now; delete ~/.tauri after the first green release through the signer"
    echo "  chmod 600 ~/.tauri/k2so-updater.key"
    echo "  # 3. Old stopgap leftovers, if they exist (each may say 'not found': fine):"
    echo "  rm -f ~/.k2-updater/*.key; security delete-generic-password -s k2-updater-signing-u1; security delete-generic-password -s k2-updater-signing-u2"
    echo "  # 4. The old GitHub signing secrets (the release preflight refuses while they exist):"
    echo "  gh secret delete TAURI_SIGNING_PRIVATE_KEY -R Alakazam-211/K2; gh secret delete TAURI_SIGNING_PRIVATE_KEY_PASSWORD -R Alakazam-211/K2"
    echo "  # 5. App Store Connect key: keep one AuthKey_*.p8 in one folder, chmod 700 the folder and 600 the file,"
    echo "  #    and point ASC_API_KEY_P8= in the K2 checkout's .env at it"
    echo "Then start K2 again (open it from Applications) and send K2 the file ~/k2-update-pubkeys.txt."
    echo "(On z3mbp, separately: once both entries are in the password manager, rm -P ~/k2-root-helper-keys-SAVE-THEN-DELETE.txt)"
    [ "$RUN" = 1 ] || how_to_run
    ;;
3)
    echo "Sitting 3 — after K2 says the public keys landed: pin them, prove the signer end to end, one Touch ID."
    echo "First: cd $(show "$TREE") && git fetch origin && git checkout --detach <the commit K2 names>"
    need_commit
    step "Re-install from this commit: pins U2/U3; refuses if the tree's U2 isn't the one generate-u2 made (Password: = Mac admin password)" sudo bash "$HERE/install.sh"
    note "Send K2 the 'Hashes' lines again and wait for 'they match'."
    step "The signer's GitHub side (must end 'gh-selfcheck ok')" sudo -n -u _k2sign "$LIBEXEC/k2sign" gh-selfcheck
    step "Preflight as the release does it (must end 'preflight ok')" bash "$TREE/scripts/sign-daemon.sh" --preflight 0.46.6
    # Version 0.0.1 never uses up a real release's one session (versions only
    # go up; 0.0.1 is below every real one; this works before the first real approval).
    step "One Touch ID opens a session (throwaway version 0.0.1): TOUCH the sensor" /bin/bash -c \
        'source "$1/scripts/lib/sign-updater.sh"; source "$1/scripts/lib/sign-approve.sh"; k2sa_open_session 0.0.1 "$(git -C "$1" rev-parse HEAD)" K2.app.tar.gz k2-daemon-macos-aarch64 k2-daemon-linux-x86_64 k2-daemon-linux-aarch64' sh "$TREE"
    step "Status shows the session, its 4 files and a 12 h expiry" sudo -n -u _k2sign "$LIBEXEC/k2sign" status 0.0.1
    must_fail "a file with no evidence is refused even inside the session" /bin/sh -c "echo x | sudo -n -u _k2sign $LIBEXEC/k2sign sign u2 K2.app.tar.gz 0.0.1 raw"
    step "Fetch 0.46.5's real (notarized) app update" /bin/sh -c 'rm -rf "$1" && mkdir -p "$1" && gh release download v0.46.5 -R Alakazam-211/K2 -p K2.app.tar.gz -D "$1"' sh "${TMPDIR:-/tmp}/k2-s3proof"
    must_say "the signer checks a real app as _k2sign: team, notarization, then refuses only for the version" "K2.app is 0.46.5, session is 0.0.1" \
        /bin/sh -c 'sudo -n -u _k2sign "$1/k2sign" sign u2 K2.app.tar.gz 0.0.1 raw < "$2/K2.app.tar.gz"' sh "$LIBEXEC" "${TMPDIR:-/tmp}/k2-s3proof"
    step "Remove the downloaded app" /bin/rm -rf "${TMPDIR:-/tmp}/k2-s3proof"
    note "Done. The 0.46.6 release asks for its own Touch ID at Step 0.46."
    [ "$RUN" = 1 ] || how_to_run
    ;;
*) sed -n '2,19p' "$0"; exit 2 ;;
esac
