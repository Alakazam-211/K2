#!/usr/bin/env bash
# Live check of the systemd side of `k2 vault systemd-cred`
# (prd-vault-gaps-v1 item 4) on a Linux box: systemd >= 250 as PID 1, run as
# root or with passwordless sudo. It does by hand exactly what the root
# helper's `creds` verbs do (k2-root-helper/src/creds.rs):
#   0. prints which stock units import credentials (review B-H2's check)
#   1. systemd-creds encrypt --name=<cred> --with-key=auto - -  (value on
#      stdin, ciphertext on stdout) -> /etc/k2/credstore.encrypted/<cred>.cred
#      0600 (K2's own store, root 0700, outside systemd's search path); a
#      unit asking for the bare ID does NOT get it
#   2. the canned drop-in (k2_root_proto::creds::cred_dropin) on a throwaway
#      unit, daemon-reload, start: the unit reads %d/<cred> = the value
#   3. a rotation: re-put, the unit reads the new value
#   4. the multi-variable env-file wrapper (env_wrapper_dropin): quoted
#      values with spaces, $, ' and = load; an unquoted value with spaces
#      does not load
#   5. THE REAL HELPER (crates/k2-root-helper/examples/creds_live.rs): its
#      own creds put -> bind (checked over D-Bus with busctl) -> the unit
#      reads it -> restart (+ the 30 s limit) -> a unit that already loads
#      the same credential ID is refused -> another daemon is refused -> a
#      re-put -> unbind -> remove; status lists no orphan of the run.
#      Build it first, as your own user (not root):
#        cargo build --release -p k2-root-helper --example creds_live
#      or point K2_CREDS_LIVE_BIN at a built copy. Without it, part 5 FAILS
#      (set K2_CREDS_LIVE_SKIP=1 to run parts 0-4 only).
# What it touches (all restored or removed at the end, also on failure):
#   /etc/systemd/system/k2test*.service[.d] and opsprobe*.service[.d],
#   /etc/k2/credstore.encrypted/<run>*.cred (the dir is created 0700 if
#   missing and removed again if it was), /etc/k2-root-helper/
#   systemd-cred-units (two lines appended, then the file is put back byte
#   for byte, or removed if it didn't exist), /etc/k2-root-helper/daemon.toml
#   (a temporary one ONLY if none exists), /var/lib/k2-root-helper/creds.json
#   (this run's entries), /run/<run>*.out. It never touches the installed
#   root helper, sudoers, a real daemon or K2's vault.
# Exit 77 = can't run here (no root, no systemd, systemd < 250).
set -euo pipefail

SUDO=()
if [ "$(id -u)" != 0 ]; then
    if sudo -n true 2>/dev/null; then
        SUDO=(sudo -n)
    else
        echo "CANNOT RUN: needs root or passwordless sudo" >&2
        exit 77
    fi
fi
[ -d /run/systemd/system ] || { echo "CANNOT RUN: systemd is not PID 1" >&2; exit 77; }
ver="$(systemctl --version | head -1 | awk '{print $2}')"
[ "${ver:-0}" -ge 250 ] || { echo "CANNOT RUN: systemd ${ver:-?} < 250" >&2; exit 77; }
command -v systemd-creds >/dev/null || { echo "CANNOT RUN: no systemd-creds" >&2; exit 77; }

R() { "${SUDO[@]}" "$@"; }
fail() { echo "FAIL: $*" >&2; exit 1; }

id="k2test$RANDOM$RANDOM"
unit="$id.service"
envunit="$id-env.service"
badunit="$id-bad.service"
bareunit="$id-bare.service"
importunit="$id-import.service"
cred="${id}_key"
envcred="$id.env"
store=/etc/k2/credstore.encrypted
work="$(mktemp -d)"
# What this run creates, recorded BEFORE anything is created, so the EXIT
# trap removes it whatever the outcome.
made_etc_k2=0; [ -e /etc/k2 ] || made_etc_k2=1
made_store=0; [ -e "$store" ] || made_store=1
cleanup() {
    for u in "$unit" "$envunit" "$badunit" "$bareunit" "$importunit"; do
        R systemctl stop "$u" 2>/dev/null || true
        R rm -rf "/etc/systemd/system/$u" "/etc/systemd/system/$u.d"
    done
    R rm -f "$store/$cred.cred" "$store/$envcred.cred" \
        "/run/$id.out" "/run/$id.env.out" "/run/$id.bad.out" "/run/$id.bare.out" "/run/$id.import.out" "/run/$id.print.sh"
    if [ "$made_store" = 1 ] && [ -d "$store" ]; then
        R rmdir "$store" 2>/dev/null || echo "cleanup: $store is not empty (left in place): $(R ls -A "$store" | tr '\n' ' ')" >&2
    fi
    if [ "$made_etc_k2" = 1 ] && [ -d /etc/k2 ]; then
        R rmdir /etc/k2 2>/dev/null || echo "cleanup: /etc/k2 is not empty (left in place): $(R ls -A /etc/k2 | tr '\n' ' ')" >&2
    fi
    R systemctl daemon-reload || true
    rm -rf "$work"
}
trap cleanup EXIT

put() { # put <cred> < value  (value on stdin, never argv)
    R systemd-creds encrypt --name="$1" --with-key=auto - - >"$work/$1.cred"
    [ -s "$work/$1.cred" ] || fail "systemd-creds gave no credential for $1"
    R install -d -m 0755 /etc/k2
    R install -d -m 0700 "$store"
    R install -m 0600 -o root -g root "$work/$1.cred" "$store/$1.cred"
}
dropin() { # dropin <unit> <cred>: the canned bytes
    R mkdir -p "/etc/systemd/system/$1.d"
    printf '# Written by K2 (k2 vault systemd-cred). K2 rewrites or removes this file; don'"'"'t edit it.\n[Service]\nLoadCredentialEncrypted=%s:%s/%s.cred\n' \
        "$2" "$store" "$2" | R tee "/etc/systemd/system/$1.d/k2-cred-$2.conf" >/dev/null
}

v1="k2test-live-value-one-$id"
v2="k2test-live-value-two-$id"

# 0: which stock units import credentials (they search systemd's own
# credstores by name; K2's store is not one of them).
echo "  -- stock units' credential imports (informational):"
for u in systemd-tmpfiles-setup.service systemd-tmpfiles-setup-dev.service systemd-sysusers.service \
    systemd-sysctl.service getty@tty1.service serial-getty@ttyS0.service systemd-firstboot.service \
    systemd-network-generator.service sshd.service ssh.service; do
    lines="$(systemctl cat "$u" 2>/dev/null | grep -iE '^(ImportCredential|LoadCredential|LoadCredentialEncrypted|SetCredential)' || true)"
    if [ -n "$lines" ]; then printf '     %s: %s\n' "$u" "$(printf '%s' "$lines" | tr '\n' ' ')"; fi
done

# 1 + 2: seal, bind, read.
printf '%s' "$v1" | put "$cred"
if R grep -aqF "$v1" "$store/$cred.cred"; then fail "the plaintext is in the .cred"; fi
[ "$(R stat -c %a "$store/$cred.cred")" = 600 ] || fail ".cred mode"
printf '[Unit]\nDescription=k2 systemd-cred live test\n[Service]\nType=oneshot\nExecStart=/bin/sh -c '"'"'cat "%%d/%s" > /run/%s.out'"'"'\n' "$cred" "$id" |
    R tee "/etc/systemd/system/$unit" >/dev/null
dropin "$unit" "$cred"
R systemctl daemon-reload
R systemctl start "$unit"
[ "$(R cat "/run/$id.out")" = "$v1" ] || fail "the unit read '$(R cat "/run/$id.out")', not the value"
echo "  ok put + bind: the unit reads %d/$cred"

# 1b: K2's store is not on systemd's search path: a unit asking for the
# bare ID (or importing it by name) does not get K2's credential. A unit
# whose credential is missing may still start (systemd 255 does), so the
# probe records what it actually sees at %d/<cred>: the value, or "absent".
probe() { # probe <unit> <credential line> <out>
    printf '[Unit]\nDescription=k2 credstore probe\n[Service]\nType=oneshot\n%s\nExecStart=/bin/sh -c '"'"'if test -e "%%d/%s"; then cat "%%d/%s"; else echo absent; fi > %s'"'"'\n' \
        "$2" "$cred" "$cred" "$3" | R tee "/etc/systemd/system/$1" >/dev/null
    R systemctl daemon-reload
    R rm -f "$3"
    R systemctl start "$1" 2>/dev/null || true
    if R test -e "$3"; then
        got="$(R cat "$3")"
        [ "$got" = absent ] || fail "$1 got K2's credential ($2): K2's store is on systemd's search path"
        echo "  ok $2: the unit ran and %d/$cred is absent"
    else
        echo "  ok $2: the unit didn't run (its credential is missing), so it didn't get it"
    fi
}
probe "$bareunit" "LoadCredentialEncrypted=$cred" "/run/$id.bare.out"
if [ "$ver" -ge 254 ]; then
    probe "$importunit" "ImportCredential=$cred" "/run/$id.import.out"
fi
tpm=no; systemd-creds has-tpm2 >/dev/null 2>&1 && tpm=yes
echo "  (systemd $ver, --with-key=auto, tpm2: $tpm)"

# 3: rotation re-puts; the next start reads the new value.
printf '%s' "$v2" | put "$cred"
R systemctl start "$unit"
[ "$(R cat "/run/$id.out")" = "$v2" ] || fail "after a rotate the unit read the old value"
echo "  ok rotate: re-put, the unit reads the new value"

# 4: the env-file wrapper.
printf '#!/bin/sh\nexec /usr/bin/printenv A B C D\n' | R tee "/run/$id.print.sh" >/dev/null
R chmod 0755 "/run/$id.print.sh"
cat >"$work/env" <<'ENV'
A='two words'
B='$HOME and `id`'
C='it'\''s'
D='x=y'
ENV
put "$envcred" <"$work/env"
printf '[Unit]\nDescription=k2 env wrapper live test\n[Service]\nType=oneshot\nExecStart=/bin/false\nStandardOutput=file:/run/%s.env.out\n' "$id" |
    R tee "/etc/systemd/system/$envunit" >/dev/null
R mkdir -p "/etc/systemd/system/$envunit.d"
printf '[Service]\nLoadCredentialEncrypted=%s:%s/%s.cred\nExecStart=\nExecStart=/bin/sh -c '"'"'set -a; . "%%d/%s"; set +a; exec /bin/sh /run/%s.print.sh'"'"'\n' \
    "$envcred" "$store" "$envcred" "$envcred" "$id" | R tee "/etc/systemd/system/$envunit.d/creds.conf" >/dev/null
R systemctl daemon-reload
R systemctl start "$envunit"
got="$(R cat "/run/$id.env.out")"
want="$(printf 'two words\n$HOME and `id`\nit'"'"'s\nx=y')"
[ "$got" = "$want" ] || fail "the wrapper loaded: $got"
echo "  ok env-file wrapper: quoted values with spaces, \$, backquotes, ' and = load from %d/$envcred"

# An unquoted value with spaces fails the unit (sh runs the second word).
printf 'A=two words\n' >"$work/bad"
put "$envcred" <"$work/bad"
sed "s/$id.env.out/$id.bad.out/" "/etc/systemd/system/$envunit" | R tee "/etc/systemd/system/$badunit" >/dev/null
R mkdir -p "/etc/systemd/system/$badunit.d"
R cp "/etc/systemd/system/$envunit.d/creds.conf" "/etc/systemd/system/$badunit.d/creds.conf"
R systemctl daemon-reload
R systemctl start "$badunit" 2>/dev/null || true
# It must have run (the output file exists), and A must not be "two words".
R test -e "/run/$id.bad.out" || fail "the unquoted-value unit never ran, so this proves nothing"
if R grep -qx "two words" "/run/$id.bad.out"; then fail "an unquoted value with spaces loaded"; fi
echo "  ok an unquoted value with spaces does not load (values must be single-quoted)"

# 5: the real helper.
if [ "${K2_CREDS_LIVE_SKIP:-0}" = 1 ]; then
    echo "  SKIPPED part 5 (the real helper): K2_CREDS_LIVE_SKIP=1"
else
    bin="${K2_CREDS_LIVE_BIN:-$(cd "$(dirname "$0")/../.." && pwd)/target/release/examples/creds_live}"
    [ -x "$bin" ] || fail "no helper live binary at $bin: cargo build --release -p k2-root-helper --example creds_live (as your user), or set K2_CREDS_LIVE_BIN"
    echo "  -- the real helper ($bin):"
    R env K2_CREDS_LIVE_OK=1 "$bin" || fail "the real helper's creds verbs (above)"
fi
echo "PASS: systemd-cred live ($unit)"
