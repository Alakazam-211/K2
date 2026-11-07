//! `k2-signer-bin`: run only through the root-owned wrappers
//! `/usr/local/libexec/k2-signer/k2-signer` (as `_k2sign`) and
//! `k2-signer-admin` (as root), which call it with `signer` or `admin`
//! first, in a fixed environment. See `k2_signer::verbs`.

fn main() {
    std::process::exit(k2_signer::verbs::main(std::env::args().collect()));
}
