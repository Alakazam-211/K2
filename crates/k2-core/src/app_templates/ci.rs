//! The Tauri template's CI and release files (0.46.11, prd-ci-on-compute-
//! for-clients-v1 §4.6, §4.7 "The Tauri path"; slice C8): what `k2 app init
//! --template tauri` writes so the App's desktop builds run on K2 Compute,
//! never on the App's own box, and reach the update feed signed by the
//! client's own signer.
//!
//! - `k2/ci/build-linux.sh`: the release build. Reproducible on purpose:
//!   `SOURCE_DATE_EPOCH` from the commit, the checkout / `CARGO_HOME` /
//!   target paths mapped out of every binary, no incremental builds, so two
//!   nodes (`L1`, `V1`) make the same bytes. No updater signing on a node:
//!   the signer signs. Outputs in `k2/out/` under names that don't carry
//!   the version.
//! - `k2/ci/build-macos.sh`: an unsigned `.app` compile check on a Mac
//!   node (Mac release builds stay on the client's own Mac: Q8, S6).
//! - `.k2/ci.toml`: a `[checks.tauri-linux]` and the `[release]` section
//!   (`linux-x86_64`, `L1` + `V1` on hosts that differ).
//! - `.k2/signer.toml`: the signer policy's starting point (the signer
//!   never reads it from the repo: a person approves it at the signer).
//!
//! Which seat, controller and owner key go in: the daemon passes what this
//! server knows ([`CiFacts`]); without them the files carry placeholders
//! and the plan says so.

use std::fmt::Write as _;

/// The Rust toolchain the template pins (ci.toml `needs` and the signer
/// policy's: the same string, so the plan matches the policy).
pub const RUST_NEED: &str = "rust@1.99.1";
/// Node for the web build and the Tauri CLI.
pub const NODE_NEED: &str = "node@22";
/// The Linux bundle's system packages beyond `profile:tauri-linux`
/// (AppImage: patchelf, FUSE 2, `file`; the node owner's package policy
/// decides).
pub const LINUX_EXTRA: &[&str] = &["sys:patchelf", "sys:libfuse2t64", "sys:file", "sys:xdg-utils"];
/// The build's memory and time (a Tauri release build with its web build).
pub const MEM_GB: u32 = 16;
pub const TIMEOUT_SECS: u64 = 5400;
/// The release build's files.
pub const OUT_DIR: &str = "k2/out";

/// What the daemon knows that goes into the CI files.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CiFacts {
    /// This server's controller fingerprint (64 hex).
    pub controller_fp: Option<String>,
    /// The workspace's id (the signer policy's `workspaces`).
    pub workspace_id: Option<String>,
    /// This server's seat: the owner's host, its lane and roster key fp.
    pub owner: Option<String>,
    pub lane: Option<String>,
    pub owner_key_fp: Option<String>,
    /// `owner/repo` of the repo's GitHub remote (the signer's `[source]`).
    pub github_repo: Option<String>,
}

/// A placeholder that parses (so the file checks) but matches nothing, so
/// a policy approved with it in place refuses every bundle.
pub const PLACEHOLDER_FP: &str = "0000000000000000000000000000000000000000000000000000000000000000";
pub const PLACEHOLDER_OWNER: &str = "your-node-owner.example";

/// `needs` of the Linux build, in order.
pub fn linux_needs() -> Vec<String> {
    let mut v = vec![RUST_NEED.to_string(), NODE_NEED.to_string(), "profile:tauri-linux".to_string()];
    v.extend(LINUX_EXTRA.iter().map(|s| s.to_string()));
    v
}

fn toml_list(v: &[String]) -> String {
    let items: Vec<String> = v.iter().map(|s| format!("{s:?}")).collect();
    format!("[{}]", items.join(", "))
}

/// The binary's file name in `k2/out/` (the crate's package name), the
/// `.deb` and the `.AppImage`.
pub fn out_files(bin: &str) -> [String; 3] {
    [format!("{OUT_DIR}/{bin}"), format!("{OUT_DIR}/{bin}.deb"), format!("{OUT_DIR}/{bin}.AppImage")]
}

/// `.k2/ci.toml`.
pub fn ci_toml(project: &str, f: &CiFacts) -> String {
    let needs = toml_list(&linux_needs());
    let owner = f.owner.as_deref().unwrap_or(PLACEHOLDER_OWNER);
    let lane = f.lane.as_deref().unwrap_or("ci-standard");
    let owner_note = if f.owner.is_some() { "this server's seat" } else { "PUT YOUR SEAT'S OWNER HERE (k2 compute seats)" };
    format!(
        r#"# K2 CI and release builds for this Tauri app (k2 app init --template
# tauri; k2 study ci). Commit this file: K2 reads it at each commit.
version = 1

# Each push you check (k2 ci run; k2 ci watch on): the Linux bundle builds.
[checks.tauri-linux]
argv = ["bash", "k2/ci/build-linux.sh"]
needs = {needs}
os = "linux"
mem_gb = {MEM_GB}
timeout_secs = {TIMEOUT_SECS}

# k2 ci release <version>: built twice, on two nodes of your seat on
# different hosts (L1, V1); the signer signs only when both match.
[release]
project = "{project}"
owner = "{owner}"  # {owner_note}
lane = "{lane}"

[release.builds.linux-x86_64]
argv = ["bash", "k2/ci/build-linux.sh"]
needs = {needs}
artifacts = ["{OUT_DIR}/*"]
env = ["BUILD_COMMIT", "BUILD_TREE"]
label = "L1"
verify_label = "V1"
independent = ["host"]
mem_gb = {MEM_GB}
timeout_secs = {TIMEOUT_SECS}
"#
    )
}

/// `.k2/signer.toml`: the signer policy's starting point.
pub fn signer_toml(project: &str, bin: &str, f: &CiFacts) -> String {
    let needs = toml_list(&linux_needs());
    let ctl = f.controller_fp.as_deref().unwrap_or(PLACEHOLDER_FP);
    let owner = f.owner_key_fp.as_deref().unwrap_or(PLACEHOLDER_FP);
    let ws = f.workspace_id.as_deref().unwrap_or("your-workspace-id");
    let [b, deb, appimage] = out_files(bin);
    let mut s = String::new();
    let _ = write!(
        s,
        r#"# The signer policy for {project} (docs/ci-formats-v1.md §8): a STARTING
# POINT. The signer never reads this file from the repo. A person at the
# signing machine approves it after reading every line:
#   sudo /usr/local/libexec/k2-signer/k2-signer-admin policy-approve {project} < .k2/signer.toml
# then again with the sha256 it prints. Check the three fingerprints below
# against what YOUR screens say: controllers = this K2 server
# (k2 ci signers prints controllerFp), owners = your seat owner's roster
# key (k2 compute owners, and the owner's own screen). All zeros = not
# filled in: such a policy refuses every bundle.
version = 1
project = "{project}"
controllers = ["{ctl}"]
owners = ["{owner}"]
workspaces = ["{ws}"]
receipt_max_hours = 24
roster_max_age_hours = 168
roster_changes = "ask"
seat_moves = "ask"
taint_basis = ["owner_attested", "rekeyed", "self_clean"]

"#
    );
    match f.github_repo.as_deref() {
        Some(repo) => {
            let _ = write!(s, "[source]\nkind = \"github_tag\"\nrepo = \"{repo}\"\ntag_prefix = \"v\"\n\n");
        }
        None => s.push_str("# No GitHub remote: the approval names the commit.\n[source]\nkind = \"approval_commit\"\n\n"),
    }
    let _ = write!(
        s,
        r#"# The same command, needs and files as .k2/ci.toml [release.builds.linux-x86_64].
[build.linux-x86_64]
argv = ["bash", "k2/ci/build-linux.sh"]
needs = {needs}
artifacts = ["{OUT_DIR}/*"]
env = ["BUILD_COMMIT", "BUILD_TREE"]
isolation = ["job_uid", "exclusive", "tenant_uid"]
walls = ["job_user"]
# Both nodes must make these byte for byte. The .AppImage is rebuilt with
# the commit's time too, but it carries the build machine's own libraries,
# so it matches only when both nodes run the same distro release and
# packages: add it here once your seat's nodes do (k2 ci evidence prints
# what matched).
dual = ["{b}", "{deb}"]
independent = ["host"]

# What gets an updater signature: the AppImage is what Tauri's Linux
# updater installs.
[assets."{bin}.AppImage"]
key = "updater"
build = "linux-x86_64"
artifact = "{appimage}"
check = "artifact"

# The updater key: k2-signer-admin generate-key {project} updater prints
# its public line (RW…); put it here, in src-tauri/tauri.conf.json
# plugins.updater.pubkey (the base64 of the .pub file), and in
# k2 app updates key <app> --set.
[keys.updater]
kind = "minisign"
public = "RW-PUT-THE-UPDATER-PUBLIC-KEY-HERE"
"#
    );
    s
}

/// `k2/ci/build-linux.sh`.
pub fn build_linux_sh() -> String {
    format!(
        r#"#!/usr/bin/env bash
# The Linux release build of this Tauri app (k2 app init --template tauri).
# K2 Compute runs it for k2 ci release / k2 ci run; it runs by hand too
# (Linux, Rust, Node and Tauri's system packages installed).
#
# Two nodes must make the same bytes (the signer compares them), so:
# SOURCE_DATE_EPOCH is the commit's time, the checkout, CARGO_HOME and the
# target dir are mapped out of every binary, and nothing builds
# incrementally. No updater signing here: no key ever reaches a node; the
# signer signs the files afterwards.
#
# Writes {OUT_DIR}/<bin>, {OUT_DIR}/<bin>.deb and {OUT_DIR}/<bin>.AppImage
# (<bin> = src-tauri's package name), and prints their sha256.
set -euo pipefail
cd "$(dirname "$0")/../.."
root="$(pwd -P)"

if [ -z "${{SOURCE_DATE_EPOCH:-}}" ]; then
    SOURCE_DATE_EPOCH="$(git -c safe.directory='*' log -1 --format=%ct HEAD)" || {{
        echo "build-linux.sh: no commit time from git: run it in a git checkout, or set SOURCE_DATE_EPOCH" >&2
        exit 1
    }}
fi
export SOURCE_DATE_EPOCH
target="${{CARGO_TARGET_DIR:-$root/src-tauri/target}}"
case "$target" in /*) ;; *) target="$root/$target" ;; esac
mkdir -p "$target"
target="$(cd "$target" && pwd -P)"
export CARGO_TARGET_DIR="$target"
cargo_home="${{CARGO_HOME:-$HOME/.cargo}}"
for p in "$root" "$cargo_home" "$target"; do
    case "$p" in *[[:space:]]*) echo "build-linux.sh: a build path has a space ($p)" >&2; exit 1 ;; esac
done
# The most specific map last (rustc and cc use the last one that matches).
maps="$root=/src $cargo_home=/cargo $target=/target"
rflags="" cflags=""
for m in $maps; do
    rflags="$rflags --remap-path-prefix=$m"
    cflags="$cflags -ffile-prefix-map=$m"
done
export RUSTFLAGS="${{RUSTFLAGS:-}}$rflags"
export CFLAGS="${{CFLAGS:-}}$cflags"
export CXXFLAGS="${{CXXFLAGS:-}}$cflags"
export CARGO_INCREMENTAL=0
export TZ=UTC LC_ALL=C.UTF-8

bin="$(awk '/^\[package\]/{{p=1; next}} /^\[/{{p=0}} p && /^name *=/{{gsub(/.*= *"|".*/, ""); print; exit}}' src-tauri/Cargo.toml)"
[ -n "$bin" ] || {{ echo "build-linux.sh: no [package] name in src-tauri/Cargo.toml" >&2; exit 1; }}
# Two builds must resolve the same crates: a committed Cargo.lock, used as is.
[ -f src-tauri/Cargo.lock ] || [ -f Cargo.lock ] || {{
    echo "build-linux.sh: no Cargo.lock: commit it (cargo generate-lockfile in src-tauri), or the two builds may pick different crates" >&2
    exit 1
}}

# The web build (Tauri runs beforeBuildCommand) and the Tauri CLI.
if [ -f package-lock.json ]; then
    npm ci --no-audit --no-fund
elif [ -f package.json ]; then
    npm install --no-audit --no-fund
fi
if [ -x node_modules/.bin/tauri ]; then
    tauri=(node_modules/.bin/tauri)
else
    tauri=(npx --yes @tauri-apps/cli@2)
fi
# Bundles on, updater archives off (the signer signs the AppImage itself).
"${{tauri[@]}}" build --ci --bundles deb,appimage --config '{{"bundle":{{"active":true,"createUpdaterArtifacts":false}}}}' -- --locked

# Tauri's .deb keeps each file's build-time mtime and the folder's read
# order, so two builds of one commit differ only there: repack it with the
# commit's time, names sorted, root as owner, and no gzip name or time.
repack_deb() {{
    local deb="$1" w
    w="$(mktemp -d)"
    (cd "$w" && ar x "$deb" && mkdir c d && tar -xpzf control.tar.gz -C c && tar -xpzf data.tar.gz -C d)
    local t=(--sort=name --mtime="@$SOURCE_DATE_EPOCH" --owner=0 --group=0 --numeric-owner --format=gnu)
    (cd "$w/c" && tar "${{t[@]}}" -cf - . | gzip -9n > ../control.tar.gz)
    (cd "$w/d" && tar "${{t[@]}}" -cf - . | gzip -9n > ../data.tar.gz)
    rm -f "$deb"
    (cd "$w" && ar -rcD "$deb" debian-binary control.tar.gz data.tar.gz)
    rm -rf "$w"
}}

# The AppImage is a runtime (an ELF) with a squashfs after it. The squashfs
# keeps each file's build-time mtime, and the runtime's .digest_md5 section
# is the MD5 of the whole image, so both differ between builds. Rebuild
# the squashfs from the same files with the commit's time, root as owner
# and no xattrs (mksquashfs sorts each folder), splice it back onto the
# runtime, and zero .digest_md5 (an AppImage with no digest is valid; Tauri's
# updater and the runtime never read it, and the signer's .sig covers the
# whole file). Needs python3 and
# mksquashfs 4.4+: on PATH, else the one inside Tauri's cached appimage
# plugin. The AppImage also carries the build machine's own libraries, so
# two builds match only on the same distro release and packages.
normalise_appimage() {{
    local img="$1" w mks plugin
    w="$(mktemp -d)"
    mks="$(command -v mksquashfs || true)"
    if [ -z "$mks" ]; then
        plugin="$(ls "$HOME"/.cache/tauri/*plugin-appimage*.AppImage 2>/dev/null | head -1)"
        [ -n "$plugin" ] || {{ echo "build-linux.sh: no mksquashfs (and no Tauri appimage plugin to take one from)" >&2; exit 1; }}
        (cd "$w" && "$plugin" --appimage-extract >/dev/null && mv squashfs-root plugin)
        mks="$(find "$w/plugin" -type f -name mksquashfs | head -1)"
        [ -x "$mks" ] || {{ echo "build-linux.sh: Tauri's appimage plugin holds no mksquashfs" >&2; exit 1; }}
    fi
    command -v python3 >/dev/null || {{ echo "build-linux.sh: python3 is needed to rebuild the AppImage" >&2; exit 1; }}
    local off
    off="$(python3 -c 'import struct,sys; b=open(sys.argv[1],"rb").read(64); o,=struct.unpack_from("<Q",b,0x28); e,n=struct.unpack_from("<HH",b,0x3A); print(o+e*n)' "$img")"
    head -c "$off" "$img" > "$w/image"
    (cd "$w" && "$img" --appimage-extract >/dev/null)
    "$mks" "$w/squashfs-root" "$w/fs" -noappend -quiet -no-progress -all-root -no-xattrs \
        -comp zstd -b 131072 -mkfs-time "$SOURCE_DATE_EPOCH" -all-time "$SOURCE_DATE_EPOCH" >/dev/null
    cat "$w/fs" >> "$w/image"
    python3 - "$w/image" <<'PY'
import struct, sys
p = sys.argv[1]
b = bytearray(open(p, "rb").read())
shoff, = struct.unpack_from("<Q", b, 0x28)
entsize, num, strndx = struct.unpack_from("<HHH", b, 0x3A)
secs = []
for i in range(num):
    name, _t, _f, _a, off, size = struct.unpack_from("<IIQQQQ", b, shoff + i * entsize)
    secs.append((name, off, size))
stroff = secs[strndx][1]
named = {{bytes(b[stroff + n:b.index(0, stroff + n)]).decode(): (off, size) for n, off, size in secs}}
off, size = named[".digest_md5"]
b[off:off + size] = bytes(size)
open(p, "wb").write(b)
PY
    chmod 0755 "$w/image"
    mv "$w/image" "$img"
    rm -rf "$w"
}}

out="$root/{OUT_DIR}"
rm -rf "$out"
mkdir -p "$out"
cp "$target/release/$bin" "$out/$bin"
debs=("$target"/release/bundle/deb/*.deb)
imgs=("$target"/release/bundle/appimage/*.AppImage)
[ "${{#debs[@]}}" = 1 ] && [ -f "${{debs[0]}}" ] || {{ echo "build-linux.sh: want one .deb, found: ${{debs[*]}}" >&2; exit 1; }}
[ "${{#imgs[@]}}" = 1 ] && [ -f "${{imgs[0]}}" ] || {{ echo "build-linux.sh: want one .AppImage, found: ${{imgs[*]}}" >&2; exit 1; }}
cp "${{debs[0]}}" "$out/$bin.deb"
repack_deb "$out/$bin.deb"
cp "${{imgs[0]}}" "$out/$bin.AppImage"
normalise_appimage "$out/$bin.AppImage"
echo "SOURCE_DATE_EPOCH=$SOURCE_DATE_EPOCH"
(cd "$out" && sha256sum -- *)
"#
    )
}

/// `k2/ci/build-macos.sh`.
pub fn build_macos_sh() -> String {
    format!(
        r#"#!/usr/bin/env bash
# An UNSIGNED macOS build of this Tauri app (k2 app init --template tauri):
# a compile check on a Mac compute node. Release builds for macOS are
# signed and notarized on your own Mac (Developer ID stays there), so this
# makes no updater archive. Writes {OUT_DIR}/<bin> and {OUT_DIR}/<bin>.app.zip.
set -euo pipefail
cd "$(dirname "$0")/../.."
root="$(pwd -P)"
target="${{CARGO_TARGET_DIR:-$root/src-tauri/target}}"
case "$target" in /*) ;; *) target="$root/$target" ;; esac
export CARGO_TARGET_DIR="$target"
bin="$(awk '/^\[package\]/{{p=1; next}} /^\[/{{p=0}} p && /^name *=/{{gsub(/.*= *"|".*/, ""); print; exit}}' src-tauri/Cargo.toml)"
[ -n "$bin" ] || {{ echo "build-macos.sh: no [package] name in src-tauri/Cargo.toml" >&2; exit 1; }}
if [ -f package-lock.json ]; then
    npm ci --no-audit --no-fund
elif [ -f package.json ]; then
    npm install --no-audit --no-fund
fi
if [ -x node_modules/.bin/tauri ]; then
    tauri=(node_modules/.bin/tauri)
else
    tauri=(npx --yes @tauri-apps/cli@2)
fi
"${{tauri[@]}}" build --ci --bundles app --config '{{"bundle":{{"active":true,"createUpdaterArtifacts":false}}}}'
out="$root/{OUT_DIR}"
rm -rf "$out"
mkdir -p "$out"
cp "$target/release/$bin" "$out/$bin"
apps=("$target"/release/bundle/macos/*.app)
[ "${{#apps[@]}}" = 1 ] && [ -d "${{apps[0]}}" ] || {{ echo "build-macos.sh: want one .app, found: ${{apps[*]}}" >&2; exit 1; }}
ditto -c -k --keepParent "${{apps[0]}}" "$out/$bin.app.zip"
(cd "$out" && shasum -a 256 -- *)
"#
    )
}

/// The README section on releases.
pub fn readme_section(app_id: &str, project: &str) -> String {
    format!(
        r#"
## Release builds on K2 Compute

This box never builds the app: K2 Compute does, on your seat's nodes, and
your own signer signs. Files: `.k2/ci.toml` (what runs), `k2/ci/build-linux.sh`
(the build, reproducible), `.k2/signer.toml` (the signer policy's start).

Once (a person): a seat on your node owner's lane (`k2 compute seats
request <owner-host>`), each seat node granted to this workspace with
`--ci`, the signer installed and paired (`k2 ci signer add`), the owner's
roster key trusted at the signer, `.k2/signer.toml` checked and approved
there (`k2-signer-admin policy-approve {project}`), and the updater key
made there (`k2-signer-admin generate-key {project} updater`): its public
line goes in `.k2/signer.toml`, `src-tauri/tauri.conf.json` and
`k2 app updates key {app_id} --set`.

Each release:
1. `k2 ci release 1.2.0 --plan`, then `k2 ci release 1.2.0 --wait`
   (L1 and V1: the same build on two hosts).
2. `k2 ci evidence <run> --out rel/` (the bundle and the files).
3. At the signing machine (each verb as `sudo -n -u _k2sign
   /usr/local/libexec/k2-signer/k2-signer …`): `rehearse {project}` and
   `evidence {project} 1.2.0`, both with the evidence file on stdin, then
   `challenge {project} 1.2.0 <commit> <bin>.AppImage`, `approve` (Touch ID),
   and `sign {project} updater <bin>.AppImage 1.2.0 b64 < <bin>.AppImage > <bin>.AppImage.sig`
   (the `.sig` form Tauri reads).
4. `k2 app updates push {app_id} --channel beta --version 1.2.0 --artifact linux-x86_64=rel/L1/k2/out/<bin>.AppImage --sig linux-x86_64=<bin>.AppImage.sig --evidence <run>`,
   then `k2 app updates promote {app_id} --from beta --to stable`.

`rel/` must be inside this repo (a push reads files there only): add it to
`.gitignore`.

A "Download the desktop app" button on the web App: link
`/_k2/download/stable/linux-x86_64` on the App's own host. It redirects to
the newest stable file (404 when there is none);
`/_k2/updates/stable/latest.json` has the version and notes.

macOS: `k2/ci/build-macos.sh` checks the app compiles on a Mac node,
unsigned. Mac releases are built, signed and notarized on your own Mac.
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::proto::ci_config::CiConfig;
    use crate::compute::proto::signer_policy::SignerPolicy;

    fn full() -> CiFacts {
        CiFacts {
            controller_fp: Some("a".repeat(64)),
            workspace_id: Some("ws-1".into()),
            owner: Some("owner.k2.dev".into()),
            lane: Some("ci-standard".into()),
            owner_key_fp: Some("b".repeat(64)),
            github_repo: Some("acme/desktop".into()),
        }
    }

    #[test]
    fn the_ci_file_parses_and_matches_the_policy() {
        for f in [full(), CiFacts::default()] {
            let c = CiConfig::parse(&ci_toml("acme-desktop", &f)).expect("ci.toml parses");
            let rel = c.release.as_ref().unwrap();
            assert_eq!(rel.project, "acme-desktop");
            let rb = &rel.builds["linux-x86_64"];
            assert_eq!((rb.label.as_str(), rb.verify_label.as_deref()), ("L1", Some("V1")));
            assert_eq!(rb.mem_gb, Some(MEM_GB));
            let text = signer_toml("acme-desktop", "acme", &f).replace("RW-PUT-THE-UPDATER-PUBLIC-KEY-HERE", "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3");
            let p: SignerPolicy = SignerPolicy::parse(&text).expect("signer.toml parses");
            let b = &p.build["linux-x86_64"];
            // The plan the controller sends is what the policy checks.
            assert_eq!((&b.argv, &b.needs, &b.artifacts, &b.env), (&rb.argv, &rb.needs, &rb.artifacts, &rb.env));
            assert_eq!(b.independent, rb.independent);
            assert!(b.dual.iter().all(|d| d.starts_with("k2/out/")));
            assert_eq!(p.assets["acme.AppImage"].artifact, "k2/out/acme.AppImage");
        }
        assert!(ci_toml("acme-desktop", &CiFacts::default()).contains(PLACEHOLDER_OWNER));
        assert!(signer_toml("acme-desktop", "acme", &CiFacts::default()).contains(PLACEHOLDER_FP));
        assert!(signer_toml("x", "x", &CiFacts::default()).contains("approval_commit"));
    }

    #[test]
    fn the_scripts_are_reproducible_and_sign_nothing() {
        let l = build_linux_sh();
        for want in ["SOURCE_DATE_EPOCH", "--remap-path-prefix", "-ffile-prefix-map", "CARGO_INCREMENTAL=0", "createUpdaterArtifacts\":false", "set -euo pipefail"] {
            assert!(l.contains(want), "{want}");
        }
        assert!(!l.contains("TAURI_SIGNING_PRIVATE_KEY"), "no key on a node");
        assert!(build_macos_sh().contains("--bundles app"));
    }
}
