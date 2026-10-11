#!/usr/bin/env bash
# k2 compute — REAL Linux node install, end to end (compute polish v1).
#
# Root, on a DISPOSABLE Linux box only (it creates and deletes the system
# user k2node, /etc/k2-node, /var/lib/k2node, k2-node.service, a sudoers
# rule, a throwaway human `k2cthuman`, and installs/removes the apt package
# `hello`). Refuses without K2_DISPOSABLE_BOX=1.
#
#   cargo build -p k2-daemon -p k2-node
#   K2_DISPOSABLE_BOX=1 tests/cli/compute_node_install_linux.sh
#
# Covers (counts printed at the end):
#  - the human's home is open (0755): enable refuses without a choice
#    (private_home_choice), then --private-home makes it 0700;
#  - without sudo: enable answers needs_root with one command;
#  - with passwordless sudo for exactly k2-node setup/uninstall (how boxes
#    are provisioned): `k2 compute local enable` installs, enrolls and
#    shows the code with no human step; confirm → online;
#  - the human's K2 can pause/resume at once (ACLs, no re-login);
#  - install-rust-for-node.sh run from inside the now-0700 home (the Mac
#    mini bug) installs Rust + cargo-nextest as k2node;
#  - declared toolchains: --needs node@22,python@3.12 (real mise download,
#    pinned + sha256), --needs rust@stable,cargo-nextest (cached), and a
#    repo's .nvmrc;
#  - a system package off the approved list is refused before the job
#    runs; an approved one is installed by the root path unit (k2node has
#    no sudo); a forged request is still checked against the list;
#  - `k2 compute local disable` uninstalls everything.
set -euo pipefail

[ "${K2_DISPOSABLE_BOX:-}" = "1" ] || { echo "refusing: set K2_DISPOSABLE_BOX=1 (this installs a system service and users)" >&2; exit 2; }
[ "$(id -u)" = "0" ] || { echo "run as root" >&2; exit 2; }
[ "$(uname -s)" = "Linux" ] || { echo "Linux only" >&2; exit 2; }
if id -u k2node >/dev/null 2>&1 || [ -e /etc/k2-node ]; then
    echo "refusing: this box already has a compute node (k2node / /etc/k2-node)" >&2
    exit 2
fi

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
TARGET="${CARGO_TARGET_DIR:-$PROJECT_ROOT/target}"
HUMAN="k2cthuman"
BIN_DIR="/tmp/k2ct-bin"
PKG_OK="hello"
PKG_NO="cowsay"

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi; }
expect_has() { if printf '%s' "$2" | grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in: $(printf '%s' "$2" | tail -c 600))"; fi; }

DAEMON_PID=""
cleanup() {
    set +e
    echo "== cleanup"
    [ -n "$DAEMON_PID" ] && kill "$DAEMON_PID" 2>/dev/null
    if id -u k2node >/dev/null 2>&1; then /usr/local/libexec/k2-node uninstall >/dev/null 2>&1 || true; fi
    rm -f /etc/sudoers.d/k2ct-human
    dpkg -s "$PKG_OK" >/dev/null 2>&1 && apt-get remove -y -q "$PKG_OK" >/dev/null 2>&1
    if [ "$fail" -ne 0 ]; then
        echo "--- daemon log tail ---" >&2; tail -40 "/home/$HUMAN/daemon.log" >&2
        echo "--- node journal ---" >&2; journalctl -u k2-node -n 40 --no-pager >&2
    fi
    for _ in 1 2 3 4 5; do
        pkill -KILL -u "$HUMAN" 2>/dev/null
        sleep 1
        userdel -r "$HUMAN" >/dev/null 2>&1 && break
    done
    id "$HUMAN" >/dev/null 2>&1 && echo "WARN: $HUMAN still exists" >&2
    rm -rf "$BIN_DIR"
    echo "== left behind (should be empty):"
    ls -d /etc/k2-node /var/lib/k2node /etc/systemd/system/k2-node-packages.path /etc/k2-node-packages.toml /usr/local/libexec/k2-node 2>/dev/null
    id k2node 2>/dev/null
}
trap cleanup EXIT

echo "== binaries"
install -d -m 0755 "$BIN_DIR"
install -m 0755 "$TARGET/debug/k2-daemon" "$BIN_DIR/k2-daemon"
install -m 0755 "$TARGET/debug/k2-node" "$BIN_DIR/k2-node"
install -m 0755 "$PROJECT_ROOT/cli/k2" "$BIN_DIR/k2"
install -d -m 0755 "$BIN_DIR/scripts"
cp "$PROJECT_ROOT"/scripts/node/*.sh "$BIN_DIR/scripts/"

echo "== a throwaway human with an open home (0755)"
useradd -m -s /bin/bash "$HUMAN"
H="/home/$HUMAN"
chmod 755 "$H"
mkdir -p "$H/.ssh" && chown "$HUMAN" "$H/.ssh"
as_human() { runuser -u "$HUMAN" -- env -i HOME="$H" USER="$HUMAN" PATH=/usr/local/bin:/usr/bin:/bin "$@"; }

echo "== the human's K2 daemon (sandbox: K2_COMPUTE=1, loopback)"
runuser -u "$HUMAN" -- env -i HOME="$H" USER="$HUMAN" PATH=/usr/bin:/bin K2_COMPUTE=1 K2_AIRGAP=1 \
    K2_NODE_BINARY="$BIN_DIR/k2-node" "$BIN_DIR/k2-daemon" > "$H/daemon.log" 2>&1 &
DAEMON_PID=$!
for _ in $(seq 1 150); do [ -s "$H/.k2/heartbeat.port" ] && [ -s "$H/.k2/heartbeat.token" ] && break; sleep 0.2; done
PORT="$(cat "$H/.k2/heartbeat.port")"
for _ in $(seq 1 100); do curl -sf "http://127.0.0.1:$PORT/boot-status" | grep -q '"ready"' && break; sleep 0.2; done
ok "daemon up on $PORT"
k2() { as_human "$BIN_DIR/k2" "$@"; }

WS="$H/ws"
as_human mkdir -p "$WS"
gw() { as_human git -C "$WS" -c user.name=t -c user.email=t@t -c commit.gpgsign=false -c init.defaultBranch=main "$@"; }
gw init -q
as_human sh -c "echo 20 > '$WS/.nvmrc'; echo hi > '$WS/a.txt'"
gw add -A
gw commit -q -m one
TOKEN="$(cat "$H/.k2/heartbeat.token")"
curl -s -X POST -H 'Content-Type: application/json' -d "{\"path\":\"$WS\"}" "http://127.0.0.1:$PORT/cli/projects/add-from-path?token=$TOKEN" >/dev/null
kws() { (cd "$WS" && runuser -u "$HUMAN" -- env -i HOME="$H" USER="$HUMAN" PATH=/usr/bin:/bin K2_PROJECT_PATH="$WS" "$BIN_DIR/k2" "$@"); }
expect_has "preview on" "$(k2 compute preview on)" ": on"

echo "== This computer: status, the home question, needs_root"
st="$(k2 compute local status)"
expect_has "status: not a node yet" "$st" "This computer is not a compute node"
expect_has "status: home is open" "$st" "readable by other users"
CODE_OUT="$(k2 compute node add ct-node --url "http://127.0.0.1:$PORT")"
ENROLL="$(printf '%s\n' "$CODE_OUT" | sed -n '2p' | tr -d ' ')"
set +e
out="$(k2 compute local enable --controller "http://127.0.0.1:$PORT" --enroll "$ENROLL" --name ct-node 2>&1)"; rc=$?
set -e
expect_eq "no private-home choice → exit 2 (usage: say which)" "$rc" "2"
expect_has "private_home_choice" "$out" "private_home_choice"
out="$(k2 compute local enable --controller "http://127.0.0.1:$PORT" --enroll "$ENROLL" --name ct-node --private-home)"
expect_has "no sudo → one owner command" "$out" "sudo $BIN_DIR/k2-node setup --human $HUMAN --private-home"
[ ! -e /etc/k2-node ] && ok "nothing installed without root" || bad "nothing installed without root"

echo "== passwordless sudo for exactly k2-node setup/uninstall → fully autonomous"
printf '%s ALL=(root) NOPASSWD: %s setup *, %s uninstall, /usr/local/libexec/k2-node uninstall\n' "$HUMAN" "$BIN_DIR/k2-node" "$BIN_DIR/k2-node" > /etc/sudoers.d/k2ct-human
chmod 0440 /etc/sudoers.d/k2ct-human
visudo -cf /etc/sudoers.d/k2ct-human >/dev/null
out="$(k2 compute local enable --controller "http://127.0.0.1:$PORT" --enroll "$ENROLL" --name ct-node --private-home)"
expect_has "enable installs + enrolls with no human step" "$out" "Installed and enrolled"
SAS="$(printf '%s\n' "$out" | sed -n 's/^Code: \([0-9]\{3\} [0-9]\{3\}\).*/\1/p')"
[ -n "$SAS" ] && ok "the node's code came back ($SAS)" || bad "the node's code came back: $out"
expect_eq "home made private" "$(stat -c %a "$H")" "700"
expect_has "k2node exists" "$(id k2node)" "k2node"
expect_has "service enabled" "$(systemctl is-enabled k2-node)" "enabled"
expect_has "the human got an ACL on the config dir" "$(getfacl -p /etc/k2-node 2>/dev/null)" "user:$HUMAN:rwx"
out="$(k2 compute node confirm ct-node "$SAS")"
expect_has "confirmed" "$out" "ct-node is active"
for _ in $(seq 1 150); do k2 compute nodes | grep -q "online" && break; sleep 0.2; done
expect_has "node online" "$(k2 compute nodes)" "online"
out="$(k2 compute grant ct-node --workspace "$WS" --max-time 30m)"
expect_has "granted" "$out" "granted ct-node"

echo "== the human's K2 pauses and resumes at once (no re-login)"
expect_has "local pause" "$(k2 compute local pause)" "paused"
for _ in $(seq 1 100); do k2 compute nodes | grep -q "paused" && break; sleep 0.1; done
expect_has "controller sees the pause" "$(k2 compute nodes)" "paused"
k2 compute local resume >/dev/null
for _ in $(seq 1 100); do k2 compute nodes | grep -q "paused" || break; sleep 0.1; done

echo "== install-rust-for-node.sh from inside the 0700 home (the Mac mini bug)"
set +e
rust_out="$(cd "$H" && "$BIN_DIR/scripts/install-rust-for-node.sh" --nextest 2>&1)"; rc=$?
set -e
expect_eq "rust recipe works from a 0700 cwd" "$rc" "0"
expect_has "rust ready" "$rust_out" "Rust is ready"
expect_has "rustc as k2node" "$(runuser -u k2node -- /var/lib/k2node/toolchains/cargo/bin/rustc --version 2>&1)" "rustc "

echo "== declared toolchains"
set +e
out="$(kws compute run ct-node --no-source --needs node@22,python@3.12 -- sh -c 'node --version; python3 --version; command -v python3' 2>"$H/err")"; rc=$?
set -e
expect_eq "node+python job exit 0" "$rc" "0"
expect_has "node 22 from mise" "$out" "v22."
expect_has "python 3.12 from mise" "$out" "Python 3.12"
expect_has "python3 is the node's, not the system's" "$out" "/var/lib/k2node/toolchains/mise/installs/python"
set +e
out="$(kws compute run ct-node --no-source --needs rust@stable,cargo-nextest -- sh -c 'rustc --version; cargo nextest --version' 2>"$H/err")"; rc=$?
set -e
expect_eq "rust job exit 0" "$rc" "0"
expect_has "rustc" "$out" "rustc 1."
expect_has "nextest" "$out" "cargo-nextest"
set +e
out="$(kws compute run ct-node -- sh -c 'node --version' 2>"$H/err")"; rc=$?
set -e
expect_eq "repo .nvmrc job exit 0" "$rc" "0"
expect_has "the repo's .nvmrc picked node 20" "$out" "v20."
expect_has "offer lists installed tools" "$(k2 compute info ct-node --json)" "mise:node"

echo "== system packages"
set +e
kws compute run ct-node --no-source --needs "sys:$PKG_NO" -- true >/dev/null 2>"$H/err"; rc=$?
set -e
expect_eq "unapproved package → exit 3" "$rc" "3"
expect_has "package_not_approved, with the owner's command" "$(cat "$H/err")" "package_not_approved"
dpkg -s "$PKG_NO" >/dev/null 2>&1 && bad "$PKG_NO was NOT installed" || ok "$PKG_NO was NOT installed"
/usr/local/libexec/k2-node packages allow "$PKG_OK" >/dev/null
expect_eq "approved list is root-owned 0644" "$(stat -c '%U %a' /etc/k2-node-packages.toml)" "root 644"
set +e
out="$(kws compute run ct-node --no-source --needs "sys:$PKG_OK" -- hello 2>"$H/err")"; rc=$?
set -e
expect_eq "approved package job exit 0" "$rc" "0"
expect_has "the approved package runs" "$out" "Hello, world!"
expect_has "it was installed by the root path unit" "$(dpkg -s "$PKG_OK" 2>&1)" "install ok installed"
# A forged request straight into the queue is still checked against the list.
printf '{"id":"pkg-forged-0001","packages":["%s"]}' "$PKG_NO" > /tmp/k2ct-forged.json
chown k2node /tmp/k2ct-forged.json
mv /tmp/k2ct-forged.json /var/lib/k2node/run/pkg-requests/forged.json
for _ in $(seq 1 100); do [ -f /run/k2-node-packages/pkg-forged-0001.json ] && break; sleep 0.2; done
expect_has "a forged request for an unapproved package is refused" "$(cat /run/k2-node-packages/pkg-forged-0001.json 2>&1)" "not approved"
dpkg -s "$PKG_NO" >/dev/null 2>&1 && bad "$PKG_NO still not installed" || ok "$PKG_NO still not installed"
set +e
runuser -u k2node -- sudo -n true >/dev/null 2>&1; rc=$?
set -e
[ "$rc" != "0" ] && ok "k2node has no sudo at all" || bad "k2node has no sudo at all"

echo "== disable"
out="$(k2 compute local disable)"
expect_has "disable uninstalls" "$out" "no longer a compute node"
id k2node >/dev/null 2>&1 && bad "k2node removed" || ok "k2node removed"
[ ! -e /etc/systemd/system/k2-node-packages.path ] && ok "package units removed" || bad "package units removed"

echo
echo "compute_node_install_linux: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
