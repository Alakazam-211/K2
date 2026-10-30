# K10's build-time guard (crates/base/src/lib.rs): the `untrusted-wire` feature doesn't build
# without `--cfg k2_mpc_untrusted_wire_ack`. Sourced by tools/tests/full.sh (PART=wire-guard),
# at the repo root, after `timed`. Each step passes when the build does what it must:
#   - in the repo with RUSTFLAGS set (it replaces .cargo/config.toml): the feature alone is
#     refused with the guard's message; with the cfg it builds; without the feature it builds;
#   - as a host builds it (a crate outside the repo depending on the kit by path, so the
#     repo's cargo config is never read): the same three.
# Needs Rust and crates.io (the host crate resolves its own lock).
refused() {
  out=$("$@" 2>&1); rc=$?
  printf '%s\n' "$out" | grep -E '^(error|warning: unused)' | head -3
  if [ "$rc" -ne 0 ] && printf '%s' "$out" | grep -q 'k2_mpc_untrusted_wire_ack'; then
    echo "refused, as it must be"
    return 0
  fi
  echo "NOT refused by the guard (exit $rc)"
  return 1
}
ACK="--cfg k2_mpc_untrusted_wire_ack"
timed repo_feature_alone_refused refused env RUSTFLAGS= cargo check -p k2-mpc-base --features untrusted-wire
timed repo_feature_with_ack env RUSTFLAGS="$ACK" cargo check -p k2-mpc-base --features untrusted-wire
timed repo_no_feature env RUSTFLAGS= cargo check -p k2-mpc-base
# a host's build, outside the repo
HOST=$(mktemp -d)
KIT_DIR=$(pwd)/crates/base
mkdir -p "$HOST/src"
echo 'fn main() {}' > "$HOST/src/main.rs"
host_manifest() {
  cat > "$HOST/Cargo.toml" <<EOT
[package]
name = "wire-guard-host"
version = "0.0.0"
edition = "2021"
publish = false

[dependencies]
k2-mpc-base = { path = "$KIT_DIR"$1 }

[workspace]
EOT
}
host_manifest ', features = ["untrusted-wire"]'
timed host_feature_alone_refused refused sh -c "cd '$HOST' && env -u RUSTFLAGS cargo check"
timed host_feature_with_ack sh -c "cd '$HOST' && RUSTFLAGS='$ACK' cargo check"
host_manifest ''
timed host_no_feature sh -c "cd '$HOST' && env -u RUSTFLAGS cargo check"
rm -rf "$HOST"
