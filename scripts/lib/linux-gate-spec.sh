# shellcheck shell=bash
# linux-gate-spec.sh — what the Linux build gate runs and needs, shared by
# scripts/linux-build-gate.sh (release.sh Step 0.6 in K2_RELEASE_GATES=local,
# build-app.sh Step 0) and scripts/lib/release-gates.sh (the cargo-linux
# gate). Sourced; sets no shell options.
#
#   K2_LINUX_GATE_CMD            the check itself. fetch-frpc.sh auto-detects
#                                the Linux triple; the k2 crate's build script
#                                hard-requires the staged sidecar even for
#                                `cargo check`.
#   k2_linux_gate_needs <rust>   the --needs list: <rust> (rust@<pin>), cmake,
#                                and the system packages the k2 crate's build
#                                scripts find through pkg-config (GTK/WebKit,
#                                and the voice stack's ALSA), as the Linux app
#                                build does.

# shellcheck disable=SC2034  # used by the scripts that source this file
K2_LINUX_GATE_CMD="./scripts/fetch-frpc.sh && RUSTFLAGS='-D warnings' cargo check --workspace --all-targets"

k2_linux_gate_needs() {
    local rust="${1:?usage: k2_linux_gate_needs rust@<pin>}" needs
    needs="$rust,cmake@3,sys:pkg-config,sys:zlib1g-dev,sys:libssl-dev,sys:libasound2-dev"
    needs="$needs,sys:libwebkit2gtk-4.1-dev,sys:libgtk-3-dev,sys:libayatana-appindicator3-dev,sys:librsvg2-dev,sys:libxdo-dev"
    printf '%s\n' "$needs"
}
