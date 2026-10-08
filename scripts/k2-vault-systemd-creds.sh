#!/usr/bin/env bash
# k2-vault-systemd-creds.sh — optional (K2 vault Q24 / VS22).
#
# Seals the K2 vault's box key as a systemd encrypted credential, so it is
# no longer a file in the daemon user's home folder. PID 1 decrypts it at
# daemon start (root-only host key, plus the TPM2 when the machine has
# one) into $CREDENTIALS_DIRECTORY, visible to the daemon's service only.
# Home backups and provider snapshots of /home then stop carrying the key;
# with a TPM2, so does a stolen disk. It does NOT keep the key from agents
# running as the daemon's user while K2 runs (they can read that directory).
#
# The existing key is sealed as-is, so nothing is re-encrypted; on its next
# start the daemon sees the credential, checks it opens the vault, and
# removes the old key file.
#
# Root, once, on a box whose daemon runs as a SYSTEM unit (provisioned
# boxes). Not for `systemd --user` units.
#
#   sudo scripts/k2-vault-systemd-creds.sh --user k2 --unit k2-daemon.service
set -euo pipefail

user="" unit=""
while [ $# -gt 0 ]; do
    case "$1" in
        --user) user="${2:-}"; shift 2 ;;
        --unit) unit="${2:-}"; shift 2 ;;
        -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
        *) echo "usage: $0 --user <daemon user> --unit <system unit>" >&2; exit 2 ;;
    esac
done
[ -n "$user" ] && [ -n "$unit" ] || { echo "usage: $0 --user <daemon user> --unit <system unit>" >&2; exit 2; }
[ "$(id -u)" -eq 0 ] || { echo "run as root (sudo)" >&2; exit 1; }
command -v systemd-creds >/dev/null || { echo "systemd-creds not found (systemd 250 or newer needed)" >&2; exit 1; }
case "$user" in *[!a-z0-9_-]*|"") echo "bad user name" >&2; exit 2 ;; esac
case "$unit" in *[!A-Za-z0-9_.@-]*|"") echo "bad unit name" >&2; exit 2 ;; esac
systemctl cat "$unit" >/dev/null 2>&1 || { echo "no system unit named $unit" >&2; exit 1; }

home="$(getent passwd "$user" | cut -d: -f6)"
[ -n "$home" ] || { echo "no such user: $user" >&2; exit 1; }
kek="$home/.k2/vault/key/kek"
[ -f "$kek" ] && [ ! -L "$kek" ] || { echo "no file key at $kek (already moved, or the vault isn't on the file key)" >&2; exit 1; }
[ "$(stat -c %s "$kek")" -eq 32 ] || { echo "$kek has the wrong length" >&2; exit 1; }

install -d -m 0700 -o root -g root /etc/k2
cred="/etc/k2/k2-vault-kek.cred"
umask 077
systemd-creds encrypt --name=k2-vault-kek --with-key=auto "$kek" "$cred"
chmod 0600 "$cred"

dropin_dir="/etc/systemd/system/${unit}.d"
install -d -m 0755 "$dropin_dir"
cat > "$dropin_dir/k2-vault-creds.conf" <<EOF
# K2 vault: the box key as an encrypted credential (scripts/k2-vault-systemd-creds.sh)
[Service]
LoadCredentialEncrypted=k2-vault-kek:${cred}
EOF
chmod 0644 "$dropin_dir/k2-vault-creds.conf"
systemctl daemon-reload
systemctl restart "$unit"
echo "sealed. After the restart, k2 vault status should say: key systemd-creds."
