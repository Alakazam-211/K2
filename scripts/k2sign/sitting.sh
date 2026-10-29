#!/bin/bash
# sitting.sh — Rosson's sittings for the signing user, on each signing Mac
# (z3mbpZ holds primary key U2, z3mbp holds U2b: one key per Mac, never a copy)
# (prd-signing-user-v1 rev 2 §7; runbook docs/release/updater-key-ceremony.md,
# "The signing user"). In a GUI iTerm on z3mbpZ (not SSH), in the release
# checkout, on the commit K2 named:
#
#   bash scripts/k2sign/sitting.sh check [--key U2|U2b]                      read-only: is this Mac ready?    1 min
#   bash scripts/k2sign/sitting.sh 0 [--run]                                 S0 spike, THROWAWAY key         ~10 min
#   bash scripts/k2sign/sitting.sh 1 --key U2|U2b --commit <sha> [--run]     user, signer, approver, token    ~25 min
#   bash scripts/k2sign/sitting.sh 2 --key U2|U2b --commit <sha> [--run]     the keys (agents stopped)        ~30 min (U2b ~15)
#   bash scripts/k2sign/sitting.sh 3 --key U2|U2b --commit <sha> [--run]     after the public keys land       ~15 min
#   bash scripts/k2sign/sitting.sh reenrol --key U2|U2b --commit <sha> [--run]  replace the approver key        ~10 min
#   bash scripts/k2sign/sitting.sh agents [--allow-agent <pid>]              read-only: which agents would block sitting 2
# Sitting 2 and `agents` take --allow-agent <pid>: ONE claude session of yours
# that keeps running (Rosson 2026-10-28: K2's coordinator drives the terminal);
# every other agent must still be stopped.
# --key: U2 = z3mbpZ, U2b = z3mbp (the script refuses the wrong Mac).
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
# Every path below is absolute; from / the _k2sign steps can read their cwd.
cd /
LIBEXEC=/usr/local/libexec/k2sign
APP="$HOME/Applications/K2 Release Approve.app"
APPBIN="$APP/Contents/MacOS/K2 Release Approve"
U1_KEY="$HOME/.tauri/k2so-updater.key"
WHICH="${1:-}"; shift || true
RUN=0 COMMIT="" KEY="" ALLOW_PID=""
while [ $# -gt 0 ]; do
    case "$1" in
        --run) RUN=1 ;;
        --commit) COMMIT="${2:-}"; shift ;;
        --key) KEY="${2:-}"; shift ;;
        --allow-agent) ALLOW_PID="${2:-<none>}"; [ -n "${2:-}" ] || ALLOW_PID="<empty>"; shift ;;
        *) echo "sitting.sh: unknown argument $1" >&2; exit 2 ;;
    esac
    shift
done

if [ -n "$ALLOW_PID" ]; then
    case "$WHICH" in 2|agents) ;; *) echo "sitting.sh $WHICH: --allow-agent is only for sitting 2 (and 'agents')" >&2; exit 2 ;; esac
fi
case "$WHICH:$KEY" in
    check:|0:|agents:) KEY="${KEY:-U2}" ;;
    *:U2|*:U2b) ;;
    1:*|2:*|3:*|reenrol:*) echo "sitting.sh $WHICH: add --key U2 (on z3mbpZ) or --key U2b (on z3mbp)" >&2; exit 2 ;;
esac
case "$KEY" in U2) HOST=z3mbpZ ;; U2b) HOST=z3mbp ;; *) HOST="" ;; esac
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
# Agent processes of this user (sitting 2 needs them stopped). Tests narrow
# the pattern with K2SIGN_SITTING_AGENT_RE (printed loudly when set).
AGENT_RE="${K2SIGN_SITTING_AGENT_RE:-claude|grok|codex|k2-ptyd}"
descendants() { # <pid>: it and every process below it
    /bin/ps -axo pid=,ppid= | awk -v root="$1" '{kids[$2]=kids[$2] " " $1} END{q=root; out=root; while (q != "") { split(q, a, " "); q=""; for (i in a) if (a[i] != "") { n=split(kids[a[i]], c, " "); for (j=1;j<=n;j++) { out=out " " c[j]; q=q " " c[j] } } } print out}'
}
ancestors() { # <pid>: it and every process above it
    local p="$1" out=""
    while [ -n "$p" ] && [ "$p" -gt 1 ] 2>/dev/null; do out="$out $p"; p="$(/bin/ps -o ppid= -p "$p" 2>/dev/null | tr -d ' ')"; done
    printf '%s' "$out"
}
# agents_check: refuse while any agent of this user runs, except the one
# --allow-agent pid (a live claude process of this user) and what it started,
# and this sitting's own process tree. Lists everything it found.
agents_check() {
    local me pids skip others="" p line allowed=""
    me="$(id -un)"
    [ "$AGENT_RE" = "claude|grok|codex|k2-ptyd" ] || printf '  \033[1;33mTEST: agent pattern overridden: %s\033[0m\n' "$AGENT_RE"
    if [ -n "$ALLOW_PID" ]; then
        line=""
        [[ "$ALLOW_PID" =~ ^[0-9]+$ ]] && line="$(/bin/ps -o user=,args= -p "$ALLOW_PID" 2>/dev/null || true)"
        if [ -z "$line" ] || [ "$(awk '{print $1}' <<<"$line")" != "$me" ] || [ "$(awk '{n=split($2,a,"/"); print a[n]}' <<<"$line")" != claude ]; then
            printf '  \033[1;31mNO\033[0m  --allow-agent %s is not a live claude process of %s%s\n' "$ALLOW_PID" "$me" "${line:+ (it is: $(cut -c1-80 <<<"$line"))}"
            return 1
        fi
        allowed="$(descendants "$ALLOW_PID")"
    fi
    skip=" $allowed $(ancestors $$) $(descendants $$) "
    pids="$({ /usr/bin/pgrep -u "$me" -f "$AGENT_RE" 2>/dev/null || true; } | tr '\n' ' ')"
    for p in $pids; do
        case "$skip" in *" $p "*) continue ;; esac
        others="$others $p"
    done
    if [ -n "$ALLOW_PID" ]; then
        printf '  \033[1;33mALLOWED\033[0m  one agent keeps running at Rosson'"'"'s request: pid %s (%s) and the %s processes it started. Every other agent must be stopped.\n' \
            "$ALLOW_PID" "$(/bin/ps -o args= -p "$ALLOW_PID" | cut -c1-60)" "$(( $(wc -w <<<"$allowed") - 1 ))"
    fi
    if [ -n "$others" ]; then
        printf '  \033[1;31mNO\033[0m  %s other agent process(es) still run as %s:\n' "$(wc -w <<<"$others" | tr -d ' ')" "$me"
        for p in $others; do printf '        %s  %s\n' "$p" "$(/bin/ps -o args= -p "$p" 2>/dev/null | cut -c1-90)"; done
        echo "      Stop them, then run again."
        return 1
    fi
    ok "no other agent runs as $me"
}
agents_running() { { /usr/bin/pgrep -u "$(id -un)" -f "$AGENT_RE" 2>/dev/null || true; } | wc -l | tr -d ' '; }
# The approval app must close its own sheet (--timeout): Step 0.46 refuses an
# older build, so the sittings rebuild it and check before going on.
app_check() {
    printf '\n\033[1m▶ check: K2 Release Approve supports --timeout\033[0m\n    %s --capabilities   (must list timeout)\n' "$(show "$APPBIN")"
    if [ "$RUN" = 1 ] && ask; then
        case " $("$APPBIN" --capabilities 2>/dev/null || true) " in
            *" timeout "*) ok "this build closes its own sheet (--timeout)" ;;
            *) printf '  \033[1;31mToo old (no --timeout): run the rebuild step (s to skip is not allowed here). Stop.\033[0m\n'; exit 1 ;;
        esac
    fi
}
# The installed signer must be this checkout's (install.sh from this commit):
# k2sign.py, check-daemon-asset.py, the policy and both wrappers (with the
# python path install.sh wrote into them). pubkeys is not compared: it names
# its commit and changes when the public keys land (sitting 3 re-installs).
signer_current() { # [ETC_ROOT] (tests only)
    local r="${1:-}" lib etc py bad="" f want got
    lib="$r/usr/local/libexec/k2sign" etc="$r/usr/local/etc/k2sign"
    [ -f "$lib/k2sign" ] || { echo "no signer installed at $lib"; return 1; }
    py="$(sed -n "s/^K2SIGN_PYTHON='\(.*\)'$/\1/p" "$lib/k2sign")"
    for f in k2sign.py:"$HERE/k2sign.py" check-daemon-asset.py:"$TREE/scripts/lib/check-daemon-asset.py" policy:"$HERE/policy"; do
        want="$(/usr/bin/shasum -a 256 "${f#*:}" | awk '{print $1}')"
        case "${f%%:*}" in policy) got="$etc/policy" ;; *) got="$lib/${f%%:*}" ;; esac
        [ "$(/usr/bin/shasum -a 256 "$got" 2>/dev/null | awk '{print $1}')" = "$want" ] || bad="$bad ${f%%:*}"
    done
    for w in k2sign k2sign-admin; do
        want="$(sed "s#@PYTHON@#$py#" "$HERE/$w" | /usr/bin/shasum -a 256 | awk '{print $1}')"
        [ "$(/usr/bin/shasum -a 256 "$lib/$w" | awk '{print $1}')" = "$want" ] || bad="$bad $w"
    done
    [ -z "$bad" ] && return 0
    echo "the installed signer differs from this checkout in:$bad"
    return 1
}
signer_check() {
    printf '\n\033[1m▶ check: the installed signer is this commit'"'"'s\033[0m\n    (compares /usr/local/libexec/k2sign and the policy with this checkout; reads only)\n'
    if [ "$RUN" = 1 ] && ask; then
        local why
        if why="$(signer_current)"; then ok "the installed signer matches this checkout"; else
            printf '  \033[1;31m%s. Run: sudo bash %s, send K2 the hashes, then run this sitting again.\033[0m\n' "$why" "$(show "$HERE/install.sh")"
            exit 1
        fi
    fi
}
# Re-check right before a password step (a heartbeat could have woken an agent).
agents_gate() {
    printf '\n\033[1m▶ check: still no other agent running (before a password step)\033[0m\n'
    if [ "$RUN" = 1 ]; then agents_check || exit 1; fi
}
need_host() {
    local h
    h="$(scutil --get LocalHostName 2>/dev/null || true)"
    if [ "$h" = "$HOST" ]; then ok "this is $HOST, the Mac that holds key $KEY"; return 0; fi
    echo "Key $KEY belongs to $HOST; this is $h."
    [ "$RUN" = 0 ] || exit 1
}
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
        "$(show "$TREE")" "$WHICH --key $KEY" "${COMMIT:+ --commit $COMMIT}"
}

check() {
    echo "z3mbpZ readiness (read-only):"
    [ "$(uname -s)" = Darwin ] && ok "macOS $(sw_vers -productVersion)" || bad "not macOS"
    [ "$(scutil --get LocalHostName 2>/dev/null)" = "$HOST" ] && ok "this is $HOST (key $KEY)" \
        || bad "this is $(scutil --get LocalHostName 2>/dev/null), not $HOST: key $KEY belongs to $HOST (U2 = z3mbpZ, U2b = z3mbp)"
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
    [ "${n:-0}" -ge 1 ] && ok "Touch ID: ${n} fingerprint(s) (approvals also accept your Mac login password)" \
        || note "no Touch ID fingerprint: approvals will ask for your Mac login password"
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
agents) agents_check ;;
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
    printf '\n\033[1m▶ (b) over SSH, BEFORE the next step: from another Mac run this one line, then TOUCH the sensor on %s:\033[0m\n    ssh z3thon@%s %q\n' "$HOST" "$HOST" \
        "$(show /usr/bin/open -W -n -a "$S0/adhoc/K2 Release Approve.app" --args --key-dir "$S0/key" --challenge "$S0/c.json" --out "$S0/c3.der")"
    step "(b) Check the SSH approval: 'Verified OK' = pass" /usr/bin/openssl dgst -sha256 -verify "$S0/key/approver.pem" -signature "$S0/c3.der" "$S0/c.json"
    step "Remove everything the spike made" /bin/rm -rf "$S0"
    echo; echo "Tell K2 pass/fail for: (a) ad hoc, (a) Developer ID, (b) iTerm, (b) SSH, (c). Wait for K2's go before sitting 1."
    [ "$RUN" = 1 ] || how_to_run
    ;;
1)
    echo "Sitting 1 on $HOST (key $KEY) — the signing user, the signer, the approver, the signer's GitHub token."
    echo "You'll type: your Mac admin password (2-3 times) and paste one new GitHub token. Touch ID (or your login password): once."
    need_host
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
    step "Enrol the approver: TOUCH the sensor (or click 'Use Password…' and type your Mac login password)" "$APPBIN" --enrol
    step "Give the signer the approver's public key (Password: = your Mac admin password)" sudo "$LIBEXEC/k2sign-admin" set-approver "$HOME/.k2-release-approve/approver.pem"
    printf '\n\033[1m▶ In a browser: make the signer'"'"'s own GitHub token (read-only, about 3 min)\033[0m\n'
    echo "    github.com → your picture → Settings → Developer settings → Personal access tokens → Fine-grained tokens → Generate new token"
    echo "    Token name: k2sign $HOST    Resource owner: Alakazam-211    Expiration: 1 year (or the longest allowed)"
    echo "    (one token per Mac: revoking one Mac's token leaves the other working)"
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
    if [ "$KEY" = U2 ]; then
        echo "Sitting 2 on z3mbpZ — the keys. U2 is made inside _k2sign (nobody sees its password); U3 (made ONCE, here) goes to your password manager; U1 moves in."
    else
        echo "Sitting 2 on z3mbp — the keys. U2b, this Mac's own primary, is made inside its _k2sign (nobody sees its password); U1 moves in. U3 is not made here."
    fi
    echo "BEFORE you start:"
    echo "  - U1's password must be in your password manager. Today it's the value on the TAURI_SIGNING_PRIVATE_KEY_PASSWORD line in the K2"
    echo "    checkout's .env: open .env in an editor (don't cat it), copy the value into a password-manager entry 'K2 update key U1'."
    [ "$KEY" = U2 ] && echo "  - Make a NEW long password for U3 in your password manager (entry 'K2 update key U3 - password')."
    if [ -n "$ALLOW_PID" ]; then
        echo "  - ONE agent stays (--allow-agent $ALLOW_PID, at your request: it drives this terminal). Every OTHER agent must stop:"
        echo "    end their sessions in K2 (keep the K2 daemon running: 'k2 daemon stop' would end the allowed one too) and pause"
        echo "    their heartbeats; close every other Claude/Grok/Codex terminal. The script checks before it starts and again"
        echo "    right before each password step. The allowed agent runs as you and can read what you can, so: type passwords"
        echo "    ONLY at the hidden prompts in this iTerm window (turn on iTerm → Secure Keyboard Entry), never in a K2 tab or"
        echo "    the K2 compose bar."
    else
        echo "  - Stop every agent: quit K2 from its menu-bar icon, then run 'k2 daemon stop' (it ends agent sessions), and close every"
        echo "    Claude/Grok/Codex terminal. The script checks none is left."
    fi
    if [ "$KEY" = U2 ]; then echo "You'll type: your Mac admin password (1-2 times), U3's new password (twice), 'saved', U1's password (once)."
    else echo "You'll type: your Mac admin password (1-2 times) and U1's password (once)."; fi
    echo "After pasting a password, copy any other text so it isn't left on the clipboard."
    need_host
    need_commit
    if [ "$RUN" = 1 ]; then
        agents_check || exit 1
        [ -f "$U1_KEY" ] || { echo "  no $U1_KEY: tell K2 where U1's key file is."; exit 1; }
        base64 -D < "$U1_KEY.pub" 2>/dev/null | head -1 | grep -q E9111446F65EBB05 \
            || { echo "  $U1_KEY.pub is not U1 (E9111446F65EBB05): tell K2."; exit 1; }
        ok "U1's key file is $U1_KEY (id E9111446F65EBB05)"
    fi
    signer_check
    step "Rebuild K2 Release Approve from this commit (keeps your approver key; adds --timeout)" bash "$HERE/approve/build.sh"
    app_check
    step "$KEY inside _k2sign (random password, never shown); prints its PUBLIC line (Password: = Mac admin password)" sudo "$LIBEXEC/k2sign-admin" generate-u2
    if [ "$KEY" = U2 ]; then
        agents_gate
        step "U3: type U3's new password twice at minisign's prompts; attach the key file to a password-manager entry; type 'saved'" \
            bash "$TREE/scripts/updater-key-ceremony.sh" generate --signer k2sign --key-name U2
        note "The PUBLIC lines (U2, U3) are saved in ~/k2-update-pubkeys-U2.txt (safe to share). If the U3 step stopped, re-run with --run and press s for the U2 step."
    else
        step "Save U2b's PUBLIC line to a file (no U3 here)" bash "$TREE/scripts/updater-key-ceremony.sh" generate --signer k2sign --key-name U2b --skip-u3
        note "U2b's PUBLIC line is saved in ~/k2-update-pubkeys-U2b.txt (safe to share)."
    fi
    agents_gate
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
    echo "  # 4. The old GitHub signing secrets (once, on either Mac; the release preflight refuses while they exist):"
    echo "  gh secret delete TAURI_SIGNING_PRIVATE_KEY -R Alakazam-211/K2; gh secret delete TAURI_SIGNING_PRIVATE_KEY_PASSWORD -R Alakazam-211/K2"
    echo "  # 5. App Store Connect key: keep one AuthKey_*.p8 in one folder, chmod 700 the folder and 600 the file,"
    echo "  #    and point ASC_API_KEY_P8= in the K2 checkout's .env at it"
    if [ -n "$ALLOW_PID" ]; then
        echo "Then let the other agents resume, and send K2 the file ~/k2-update-pubkeys-$KEY.txt."
        printf '\033[1;33mSummary: one agent was allowed to keep running (pid %s, at your request: it drives this terminal); every other agent was stopped.\033[0m\n' "$ALLOW_PID"
    else
        echo "Then start K2 again (open it from Applications) and send K2 the file ~/k2-update-pubkeys-$KEY.txt."
    fi
    echo "(On z3mbp, separately: once both entries are in the password manager, rm -P ~/k2-root-helper-keys-SAVE-THEN-DELETE.txt)"
    [ "$RUN" = 1 ] || how_to_run
    ;;
3)
    echo "Sitting 3 on $HOST (key $KEY) — after K2 says the public keys landed: pin them, prove the signer end to end, one approval."
    echo "First: cd $(show "$TREE") && git fetch origin && git checkout --detach <the commit K2 names>"
    need_host
    need_commit
    step "Re-install from this commit: pins U2/U3; refuses if the tree's U2 isn't the one generate-u2 made (Password: = Mac admin password)" sudo bash "$HERE/install.sh"
    note "Send K2 the 'Hashes' lines again and wait for 'they match'."
    signer_check
    app_check
    step "The signer's GitHub side (must end 'gh-selfcheck ok')" sudo -n -u _k2sign "$LIBEXEC/k2sign" gh-selfcheck
    step "Preflight as the release does it (must end 'preflight ok')" bash "$TREE/scripts/sign-daemon.sh" --preflight 0.46.6
    # Version 0.0.1 never uses up a real release's one session (versions only
    # go up; 0.0.1 is below every real one; this works before the first real approval).
    step "One approval opens a session (throwaway version 0.0.1): TOUCH the sensor or use your login password" /bin/bash -c \
        'source "$1/scripts/lib/sign-updater.sh"; source "$1/scripts/lib/sign-approve.sh"; k2sa_open_session 0.0.1 "$(git -C "$1" rev-parse HEAD)" K2.app.tar.gz k2-daemon-macos-aarch64 k2-daemon-linux-x86_64 k2-daemon-linux-aarch64' sh "$TREE"
    step "Status shows the session, its 4 files and a 12 h expiry" sudo -n -u _k2sign "$LIBEXEC/k2sign" status 0.0.1
    must_fail "a file with no evidence is refused even inside the session" /bin/sh -c "echo x | sudo -n -u _k2sign $LIBEXEC/k2sign sign u2 K2.app.tar.gz 0.0.1 raw"
    step "Fetch 0.46.5's real (notarized) app update" /bin/sh -c 'rm -rf "$1" && mkdir -p "$1" && gh release download v0.46.5 -R Alakazam-211/K2 -p K2.app.tar.gz -D "$1"' sh "${TMPDIR:-/tmp}/k2-s3proof"
    must_say "the signer checks a real app as _k2sign: team, notarization, then refuses only for the version" "K2.app is 0.46.5, session is 0.0.1" \
        /bin/sh -c 'sudo -n -u _k2sign "$1/k2sign" sign u2 K2.app.tar.gz 0.0.1 raw < "$2/K2.app.tar.gz"' sh "$LIBEXEC" "${TMPDIR:-/tmp}/k2-s3proof"
    step "Remove the downloaded app" /bin/rm -rf "${TMPDIR:-/tmp}/k2-s3proof"
    note "Done. A release run on this Mac asks for its own approval at Step 0.46 and signs with $KEY."
    [ "$RUN" = 1 ] || how_to_run
    ;;
reenrol)
    echo "Re-enrol on $HOST (key $KEY): replace the approver key, e.g. to accept your Mac login password as well as"
    echo "Touch ID. The old key is renamed, not deleted, until the new one is installed. Admin password: once."
    need_host
    need_commit
    RE="${TMPDIR:-/tmp}/k2-reenrol"
    CH="{\"assets\":[\"K2.app.tar.gz\"],\"base_commit\":\"0000000000000000000000000000000000000000\",\"expires\":\"2099-01-01T00:00:00Z\",\"kind\":\"k2-release-approval\",\"reapproval\":false,\"version\":\"9.9.9\",\"windows\":false}"
    step "Rebuild K2 Release Approve from this commit into ~/Applications" bash "$HERE/approve/build.sh"
    step "Enrol a new key (the old one is renamed *.old-<time>): TOUCH the sensor, or 'Use Password…'" "$APPBIN" --enrol --replace
    step "Give the signer the new public key (Password: = Mac admin password)" sudo "$LIBEXEC/k2sign-admin" set-approver "$HOME/.k2-release-approve/approver.pem"
    step "Forget the cached admin password" sudo -k
    step "Write a test challenge (not a release; the signer would never accept it)" /bin/sh -c 'mkdir -p "$2" && printf "%s" "$1" > "$2/c.json"' sh "$CH" "$RE"
    step "Proof 1 — Touch ID: TOUCH the sensor" /usr/bin/open -W -n -a "$APP" --args --challenge "$RE/c.json" --out "$RE/touch.der"
    step "  'Verified OK' = pass" /usr/bin/openssl dgst -sha256 -verify /usr/local/etc/k2sign/approver.pem -signature "$RE/touch.der" "$RE/c.json"
    step "Proof 2 — password: do NOT touch the sensor; click 'Use Password…' and type your Mac login password" /usr/bin/open -W -n -a "$APP" --args --challenge "$RE/c.json" --out "$RE/pw.der"
    step "  'Verified OK' = pass" /usr/bin/openssl dgst -sha256 -verify /usr/local/etc/k2sign/approver.pem -signature "$RE/pw.der" "$RE/c.json"
    printf '\n\033[1m▶ Proof 3 — nobody at the screen, no approval. From another Mac run this one line (it starts the app over SSH):\033[0m\n    ssh z3thon@%s %q\n' \
        "$HOST" "$(show "$APPBIN" --challenge "$RE/c.json" --out "$RE/ssh.der" --timeout 30)"
    echo "    On $HOST: do NOT touch the sensor or the keyboard. An approval sheet appears on the screen (an SSH-started"
    echo "    app can show it; only the person at the screen can answer it). Either click Cancel with the mouse, or leave"
    echo "    it alone: after 30 s it closes by itself. Expected on the other Mac: 'no approval (timed out after 30 s)' or"
    echo "    'no approval: …canceled…', a non-zero exit, and no ssh.der."
    must_fail "  …it made no approval" /bin/test -s "$RE/ssh.der"
    step "Remove the test files" /bin/rm -rf "$RE"
    note "Done. Tell K2 the three results. The renamed old key files in ~/.k2-release-approve can be deleted."
    [ "$RUN" = 1 ] || how_to_run
    ;;
*) sed -n '2,23p' "$0"; exit 2 ;;
esac
