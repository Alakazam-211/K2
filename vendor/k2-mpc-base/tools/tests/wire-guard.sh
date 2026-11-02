# K10's build-time guard (crates/base/src/lib.rs): the `untrusted-wire` feature doesn't build
# without `--cfg k2_mpc_untrusted_wire_ack`. Sourced by tools/tests/full.sh (PART=wire-guard),
# at the repo root, after `timed`. Each step passes when the build does what it must:
#   - in the repo with RUSTFLAGS set (it replaces .cargo/config.toml): the feature alone is
#     refused with the guard's message; with the cfg it builds; without the feature it builds;
#   - as a host builds it (a crate outside the repo depending on the kit by path, so the
#     repo's cargo config is never read): the same three;
#   - the conformance hook (`conformance::replica::{check_from, deliver_from}`, the suite's
#     way into the one client check) can't leak: the `conformance` feature builds without the
#     cfg, and a host build without the features has no way to hand the kit client bytes:
#     calling `apply_client_update` or the hook fails to compile, and with `conformance` alone
#     `apply_client_update` still doesn't exist.
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
# `$@` must fail to build, with an error naming `$1` (the missing item).
no_api() {
  item=$1; shift
  out=$("$@" 2>&1); rc=$?
  printf '%s\n' "$out" | grep -E '^error' | head -3
  if [ "$rc" -ne 0 ] && printf '%s' "$out" | grep -E '^error' | grep -q "$item"; then
    echo "no $item in this build, as it must be"
    return 0
  fi
  echo "$item is reachable (exit $rc)"
  return 1
}
# `$@` (a `cargo tree`) must succeed and list no `mpce-*` crate.
no_mpce() {
  out=$("$@" 2>&1); rc=$?
  if [ "$rc" -ne 0 ]; then
    printf '%s\n' "$out" | tail -3
    echo "cargo tree failed (exit $rc)"
    return 1
  fi
  if printf '%s\n' "$out" | grep -q 'mpce-'; then
    printf '%s\n' "$out" | grep 'mpce-' | head -3
    echo "an mpce-* crate is in this build"
    return 1
  fi
  echo "no mpce-* crate, as it must be"
  return 0
}
ACK="--cfg k2_mpc_untrusted_wire_ack"
timed repo_feature_alone_refused refused env RUSTFLAGS= cargo check -p k2-mpc-base --features untrusted-wire
timed repo_feature_with_ack env RUSTFLAGS="$ACK" cargo check -p k2-mpc-base --features untrusted-wire
timed repo_no_feature env RUSTFLAGS= cargo check -p k2-mpc-base
timed repo_conformance_without_ack env RUSTFLAGS= cargo check -p k2-mpc-base --features conformance
timed repo_tree_no_native no_mpce env RUSTFLAGS= cargo tree -p k2-mpc-base -e normal
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
timed host_tree_no_native no_mpce sh -c "cd '$HOST' && env -u RUSTFLAGS cargo tree -e normal"
# no client bytes reach a plain build, through the API or the conformance hook
host_main() {
  cat > "$HOST/src/main.rs" <<EOT
fn main() {
    let schema = k2_mpc_base::Schema { name: "guard", roots: &[("t", k2_mpc_base::Kind::Text)] };
    let mut doc = k2_mpc_base::Doc::new(schema).unwrap();
    let _ = $1;
}
EOT
}
host_main 'doc.apply_client_update(&[], 7)'
timed host_plain_no_client_api no_api apply_client_update sh -c "cd '$HOST' && env -u RUSTFLAGS cargo check"
host_main 'k2_mpc_base::conformance::replica::deliver_from(&mut doc, &[], 7)'
timed host_plain_no_hook no_api replica sh -c "cd '$HOST' && env -u RUSTFLAGS cargo check"
host_manifest ', features = ["conformance"]'
host_main 'doc.apply_client_update(&[], 7)'
timed host_conformance_no_client_api no_api apply_client_update sh -c "cd '$HOST' && env -u RUSTFLAGS cargo check"
host_main 'k2_mpc_base::conformance::replica::deliver_from(&mut doc, &[], 7)'
timed host_conformance_hook_builds sh -c "cd '$HOST' && env -u RUSTFLAGS cargo check"
rm -rf "$HOST"
