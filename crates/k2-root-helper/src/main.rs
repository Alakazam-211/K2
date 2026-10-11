//! `/usr/local/libexec/k2-root-helper`. Not setuid; the K2 daemon user runs
//! it only through `sudo -n` (one sudoers line). Everything is in the
//! library so it is unit-tested over a temp root.

fn main() {
    std::process::exit(k2_root_helper::main_entry());
}
