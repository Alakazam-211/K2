//! Distro-neutral system package names (protocol 7,
//! prd-compute-self-serve-environments-v1 §5).
//!
//! - A `sys:` name is a **Debian name** (the one most docs and CI files
//!   use): `sys:libssl-dev` means "OpenSSL headers" everywhere.
//! - [`ROWS`] maps it per package manager. One canonical name may become
//!   several native ones (`build-essential` on dnf), and one native name may
//!   serve several canonical ones (`pacman: mesa` for the GL dev set).
//! - [`VERSION_ROWS`] override a row on one distro version (Ubuntu 24.04's
//!   `t64` renames).
//! - apt takes any canonical name as written (Debian names are the
//!   canonical ones). Every other manager needs a row: no row = not
//!   satisfiable there (`no_package_mapping`), and placement skips the node.
//! - A native name is still possible for the long tail:
//!   `sys:pacman:protobuf`, `sys:apt:libfoo-dev`. It fits only nodes with
//!   that manager ([`SysName::Native`]).
//! - The map ships inside the signed k2-node binary and the server; it is
//!   never read from the network. Adding a row is a release.
//!
//! The dnf and zypper columns are unchecked: K2 has no Fedora or openSUSE
//! node yet ([`UNCHECKED_MANAGERS`]).

use serde::{Deserialize, Serialize};

/// Offer feature: the node maps canonical names itself, takes native
/// `sys:<manager>:<name>` needs, `profile:` needs and `Plan.packages`.
pub const FEATURE_PKG_MAP: &str = "pkg-map";

/// Managers whose map column no real node has checked yet.
pub const UNCHECKED_MANAGERS: &[Mgr] = &[Mgr::Dnf, Mgr::Zypper];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mgr {
    Apt,
    Dnf,
    Pacman,
    Zypper,
}

impl Mgr {
    pub const ALL: [Mgr; 4] = [Mgr::Apt, Mgr::Dnf, Mgr::Pacman, Mgr::Zypper];

    pub fn name(self) -> &'static str {
        match self {
            Mgr::Apt => "apt",
            Mgr::Dnf => "dnf",
            Mgr::Pacman => "pacman",
            Mgr::Zypper => "zypper",
        }
    }

    pub fn parse(s: &str) -> Option<Mgr> {
        Mgr::ALL.into_iter().find(|m| m.name() == s)
    }
}

/// A `sys:` need's package part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SysName {
    /// A canonical (Debian) name, mapped per node.
    Canonical(String),
    /// `apt:libfoo-dev`, `pacman:protobuf`: only for that manager, as written.
    Native { mgr: Mgr, name: String },
}

impl SysName {
    /// `pacman:protobuf` → Native; `libssl-dev` → Canonical. `None` for a
    /// bad manager or name.
    pub fn parse(s: &str) -> Option<SysName> {
        match s.split_once(':') {
            Some((m, n)) => {
                let mgr = Mgr::parse(m)?;
                crate::needs::valid_package(n).then(|| SysName::Native { mgr, name: n.to_string() })
            }
            None => crate::needs::valid_package(s).then(|| SysName::Canonical(s.to_string())),
        }
    }

    /// The name as a `sys:` need writes it.
    pub fn as_str(&self) -> String {
        match self {
            SysName::Canonical(n) => n.clone(),
            SysName::Native { mgr, name } => format!("{}:{name}", mgr.name()),
        }
    }

    /// The native names to install on `mgr` (distro and version for the
    /// per-version rows). `None` = not satisfiable on this manager.
    pub fn native(&self, mgr: Mgr, distro: &str, version: &str) -> Option<Vec<String>> {
        match self {
            SysName::Native { mgr: m, name } => (*m == mgr).then(|| vec![name.clone()]),
            SysName::Canonical(c) => native(c, mgr, distro, version),
        }
    }
}

/// `(canonical, apt, pacman, dnf, zypper)`. `=` is the canonical name
/// itself; `` (empty) is "no package here"; several names are space
/// separated. Sorted by canonical name (a test holds it).
pub const ROWS: &[(&str, &str, &str, &str, &str)] = &[
    ("autoconf", "=", "autoconf", "autoconf", "autoconf"),
    ("automake", "=", "automake", "automake", "automake"),
    ("bison", "=", "bison", "bison", "bison"),
    ("build-essential", "=", "base-devel", "gcc gcc-c++ make", "gcc gcc-c++ make"),
    ("bzip2", "=", "bzip2", "bzip2", "bzip2"),
    ("ca-certificates", "=", "ca-certificates", "ca-certificates", "ca-certificates"),
    ("ccache", "=", "ccache", "ccache", "ccache"),
    ("clang", "=", "clang", "clang", "clang"),
    ("cmake", "=", "cmake", "cmake", "cmake"),
    ("curl", "=", "curl", "curl", "curl"),
    ("dbus-x11", "=", "dbus", "dbus-x11", "dbus-1-x11"),
    ("ffmpeg", "=", "ffmpeg", "ffmpeg-free", "ffmpeg"),
    ("file", "=", "file", "file", "file"),
    ("flex", "=", "flex", "flex", "flex"),
    ("fonts-dejavu-core", "=", "ttf-dejavu", "dejavu-sans-fonts", "dejavu-fonts"),
    ("fonts-liberation", "=", "ttf-liberation", "liberation-fonts", "liberation-fonts"),
    ("fonts-noto-color-emoji", "=", "noto-fonts-emoji", "google-noto-emoji-color-fonts", "noto-coloremoji-fonts"),
    ("fuse3", "=", "fuse3", "fuse3", "fuse3"),
    ("g++", "=", "gcc", "gcc-c++", "gcc-c++"),
    ("gcc", "=", "gcc", "gcc", "gcc"),
    ("gdb", "=", "gdb", "gdb", "gdb"),
    ("gettext", "=", "gettext", "gettext", "gettext-tools"),
    ("gfortran", "=", "gcc-fortran", "gcc-gfortran", "gcc-fortran"),
    ("ghostscript", "=", "ghostscript", "ghostscript", "ghostscript"),
    ("git", "=", "git", "git", "git"),
    ("imagemagick", "=", "imagemagick", "ImageMagick", "ImageMagick"),
    ("jq", "=", "jq", "jq", "jq"),
    ("libaio-dev", "=", "libaio", "libaio-devel", "libaio-devel"),
    ("libarchive-dev", "=", "libarchive", "libarchive-devel", "libarchive-devel"),
    ("libasound2", "=", "alsa-lib", "alsa-lib", "libasound2"),
    ("libasound2-dev", "=", "alsa-lib", "alsa-lib-devel", "alsa-devel"),
    ("libasound2t64", "=", "alsa-lib", "alsa-lib", "libasound2"),
    ("libatk-bridge2.0-0", "=", "at-spi2-core", "at-spi2-atk", "libatk-bridge-2_0-0"),
    ("libavcodec-dev", "=", "ffmpeg", "libavcodec-free-devel", "ffmpeg-7-libavcodec-devel"),
    ("libavformat-dev", "=", "ffmpeg", "libavformat-free-devel", "ffmpeg-7-libavformat-devel"),
    ("libavutil-dev", "=", "ffmpeg", "libavutil-free-devel", "ffmpeg-7-libavutil-devel"),
    ("libayatana-appindicator3-1", "=", "libayatana-appindicator", "libayatana-appindicator-gtk3", "libayatana-appindicator3-1"),
    ("libayatana-appindicator3-dev", "=", "libayatana-appindicator", "libayatana-appindicator-gtk3-devel", "libayatana-appindicator3-devel"),
    ("libblkid-dev", "=", "util-linux-libs", "libblkid-devel", "libblkid-devel"),
    ("libboost-dev", "=", "boost", "boost-devel", "boost-devel"),
    ("libbz2-dev", "=", "bzip2", "bzip2-devel", "libbz2-devel"),
    ("libcairo2-dev", "=", "cairo", "cairo-devel", "cairo-devel"),
    ("libcap-dev", "=", "libcap", "libcap-devel", "libcap-devel"),
    ("libclang-dev", "=", "clang", "clang-devel", "clang-devel"),
    ("libcups2", "=", "libcups", "cups-libs", "libcups2"),
    ("libcurl4-openssl-dev", "=", "curl", "libcurl-devel", "libcurl-devel"),
    ("libdbus-1-dev", "=", "dbus", "dbus-devel", "dbus-1-devel"),
    ("libdrm-dev", "=", "libdrm", "libdrm-devel", "libdrm-devel"),
    ("libdrm2", "=", "libdrm", "libdrm", "libdrm2"),
    ("libegl-dev", "=", "libglvnd", "libglvnd-devel", "libglvnd-devel"),
    ("libegl1", "=", "libglvnd", "libglvnd-egl", "libEGL1"),
    ("libegl1-mesa-dev", "=", "mesa libglvnd", "mesa-libEGL-devel", "Mesa-libEGL-devel"),
    ("libeigen3-dev", "=", "eigen", "eigen3-devel", "eigen3-devel"),
    ("libelf-dev", "=", "libelf", "elfutils-libelf-devel", "libelf-devel"),
    ("libevent-dev", "=", "libevent", "libevent-devel", "libevent-devel"),
    ("libffi-dev", "=", "libffi", "libffi-devel", "libffi-devel"),
    ("libfontconfig-dev", "=", "fontconfig", "fontconfig-devel", "fontconfig-devel"),
    ("libfontconfig1", "=", "fontconfig", "fontconfig", "libfontconfig1"),
    ("libfontconfig1-dev", "=", "fontconfig", "fontconfig-devel", "fontconfig-devel"),
    ("libfreetype-dev", "=", "freetype2", "freetype-devel", "freetype2-devel"),
    ("libfreetype6-dev", "=", "freetype2", "freetype-devel", "freetype2-devel"),
    ("libfuse3-dev", "=", "fuse3", "fuse3-devel", "fuse3-devel"),
    ("libgbm-dev", "=", "mesa", "mesa-libgbm-devel", "libgbm-devel"),
    ("libgbm1", "=", "mesa", "mesa-libgbm", "libgbm1"),
    ("libgdk-pixbuf-2.0-dev", "=", "gdk-pixbuf2", "gdk-pixbuf2-devel", "gdk-pixbuf-devel"),
    ("libgif-dev", "=", "giflib", "giflib-devel", "giflib-devel"),
    ("libgit2-dev", "=", "libgit2", "libgit2-devel", "libgit2-devel"),
    ("libgl-dev", "=", "libglvnd", "libglvnd-devel", "libglvnd-devel"),
    ("libgl1", "=", "libglvnd", "libglvnd-glx", "libGL1"),
    ("libgl1-mesa-dev", "=", "mesa libglvnd", "mesa-libGL-devel", "Mesa-libGL-devel"),
    ("libgl1-mesa-dri", "=", "mesa", "mesa-dri-drivers", "Mesa-dri"),
    ("libgles2-mesa-dev", "=", "mesa libglvnd", "mesa-libGLES-devel", "Mesa-libGLESv2-devel"),
    ("libglib2.0-dev", "=", "glib2", "glib2-devel", "glib2-devel"),
    ("libgmp-dev", "=", "gmp", "gmp-devel", "gmp-devel"),
    ("libgstreamer-plugins-base1.0-dev", "=", "gst-plugins-base-libs", "gstreamer1-plugins-base-devel", "gstreamer-plugins-base-devel"),
    ("libgstreamer1.0-dev", "=", "gstreamer", "gstreamer1-devel", "gstreamer-devel"),
    ("libgtk-3-0", "=", "gtk3", "gtk3", "libgtk-3-0"),
    ("libgtk-3-dev", "=", "gtk3", "gtk3-devel", "gtk3-devel"),
    ("libgtk-4-dev", "=", "gtk4", "gtk4-devel", "gtk4-devel"),
    ("libharfbuzz-dev", "=", "harfbuzz", "harfbuzz-devel", "harfbuzz-devel"),
    ("libhdf5-dev", "=", "hdf5", "hdf5-devel", "hdf5-devel"),
    ("libicu-dev", "=", "icu", "libicu-devel", "libicu-devel"),
    ("libjavascriptcoregtk-4.1-dev", "=", "webkit2gtk-4.1", "javascriptcoregtk4.1-devel", "webkit2gtk3-devel"),
    ("libjemalloc-dev", "=", "jemalloc", "jemalloc-devel", "jemalloc-devel"),
    ("libjpeg-dev", "=", "libjpeg-turbo", "libjpeg-turbo-devel", "libjpeg8-devel"),
    ("libkrb5-dev", "=", "krb5", "krb5-devel", "krb5-devel"),
    ("libldap2-dev", "=", "libldap", "openldap-devel", "openldap2-devel"),
    ("liblzma-dev", "=", "xz", "xz-devel", "xz-devel"),
    ("libmagic-dev", "=", "file", "file-devel", "file-devel"),
    ("libmount-dev", "=", "util-linux-libs", "libmount-devel", "libmount-devel"),
    ("libncurses-dev", "=", "ncurses", "ncurses-devel", "ncurses-devel"),
    ("libnotify-dev", "=", "libnotify", "libnotify-devel", "libnotify-devel"),
    ("libnss3", "=", "nss", "nss", "mozilla-nss"),
    ("libnuma-dev", "=", "numactl", "numactl-devel", "libnuma-devel"),
    ("libopenblas-dev", "=", "openblas", "openblas-devel", "openblas-devel"),
    ("libopus-dev", "=", "opus", "opus-devel", "libopus-devel"),
    ("libpango1.0-dev", "=", "pango", "pango-devel", "pango-devel"),
    ("libpcap-dev", "=", "libpcap", "libpcap-devel", "libpcap-devel"),
    ("libpcre2-dev", "=", "pcre2", "pcre2-devel", "pcre2-devel"),
    ("libpipewire-0.3-dev", "=", "libpipewire", "pipewire-devel", "pipewire-devel"),
    ("libpng-dev", "=", "libpng", "libpng-devel", "libpng16-devel"),
    ("libpq-dev", "=", "postgresql-libs", "libpq-devel", "postgresql-devel"),
    ("libprotobuf-dev", "=", "protobuf", "protobuf-devel", "protobuf-devel"),
    ("libpulse-dev", "=", "libpulse", "pulseaudio-libs-devel", "libpulse-devel"),
    ("libreadline-dev", "=", "readline", "readline-devel", "readline-devel"),
    ("librsvg2-dev", "=", "librsvg", "librsvg2-devel", "librsvg-devel"),
    ("libsdl2-dev", "=", "sdl2-compat", "SDL2-devel", "SDL2-devel"),
    ("libseccomp-dev", "=", "libseccomp", "libseccomp-devel", "libseccomp-devel"),
    ("libsecret-1-dev", "=", "libsecret", "libsecret-devel", "libsecret-devel"),
    ("libsodium-dev", "=", "libsodium", "libsodium-devel", "libsodium-devel"),
    ("libsoup-3.0-dev", "=", "libsoup3", "libsoup3-devel", "libsoup-devel"),
    ("libsqlite3-dev", "=", "sqlite", "sqlite-devel", "sqlite3-devel"),
    ("libssh2-1-dev", "=", "libssh2", "libssh2-devel", "libssh2-devel"),
    ("libssl-dev", "=", "openssl", "openssl-devel", "libopenssl-devel"),
    ("libssl3", "=", "openssl", "openssl-libs", "libopenssl3"),
    ("libswscale-dev", "=", "ffmpeg", "libswscale-free-devel", "ffmpeg-7-libswscale-devel"),
    ("libsystemd-dev", "=", "systemd-libs", "systemd-devel", "systemd-devel"),
    ("libtiff-dev", "=", "libtiff", "libtiff-devel", "libtiff-devel"),
    ("libtool", "=", "libtool", "libtool", "libtool"),
    ("libudev-dev", "=", "systemd-libs", "systemd-devel", "libudev-devel"),
    ("libunwind-dev", "=", "libunwind", "libunwind-devel", "libunwind-devel"),
    ("libusb-1.0-0-dev", "=", "libusb", "libusb1-devel", "libusb-1_0-devel"),
    ("libuv1-dev", "=", "libuv", "libuv-devel", "libuv-devel"),
    ("libvpx-dev", "=", "libvpx", "libvpx-devel", "libvpx-devel"),
    ("libvulkan-dev", "=", "vulkan-icd-loader vulkan-headers", "vulkan-loader-devel", "vulkan-devel"),
    ("libwayland-dev", "=", "wayland", "wayland-devel", "wayland-devel"),
    ("libwebkit2gtk-4.1-0", "=", "webkit2gtk-4.1", "webkit2gtk4.1", "libwebkit2gtk-4_1-0"),
    ("libwebkit2gtk-4.1-dev", "=", "webkit2gtk-4.1", "webkit2gtk4.1-devel", "webkit2gtk3-devel"),
    ("libwebp-dev", "=", "libwebp", "libwebp-devel", "libwebp-devel"),
    ("libx11-6", "=", "libx11", "libX11", "libX11-6"),
    ("libx11-dev", "=", "libx11", "libX11-devel", "libX11-devel"),
    ("libxcb1-dev", "=", "libxcb", "libxcb-devel", "libxcb-devel"),
    ("libxcomposite1", "=", "libxcomposite", "libXcomposite", "libXcomposite1"),
    ("libxcursor-dev", "=", "libxcursor", "libXcursor-devel", "libXcursor-devel"),
    ("libxdamage1", "=", "libxdamage", "libXdamage", "libXdamage1"),
    ("libxdo-dev", "=", "xdotool", "libxdo-devel", "xdotool-devel"),
    ("libxext-dev", "=", "libxext", "libXext-devel", "libXext-devel"),
    ("libxfixes3", "=", "libxfixes", "libXfixes", "libXfixes3"),
    ("libxi-dev", "=", "libxi", "libXi-devel", "libXi-devel"),
    ("libxinerama-dev", "=", "libxinerama", "libXinerama-devel", "libXinerama-devel"),
    ("libxkbcommon-dev", "=", "libxkbcommon", "libxkbcommon-devel", "libxkbcommon-devel"),
    ("libxkbcommon-x11-dev", "=", "libxkbcommon-x11", "libxkbcommon-x11-devel", "libxkbcommon-x11-devel"),
    ("libxml2-dev", "=", "libxml2", "libxml2-devel", "libxml2-devel"),
    ("libxrandr-dev", "=", "libxrandr", "libXrandr-devel", "libXrandr-devel"),
    ("libxrandr2", "=", "libxrandr", "libXrandr", "libXrandr2"),
    ("libxshmfence1", "=", "libxshmfence", "libxshmfence", "libxshmfence1"),
    ("libxslt1-dev", "=", "libxslt", "libxslt-devel", "libxslt-devel"),
    ("libxss-dev", "=", "libxss", "libXScrnSaver-devel", "libXss-devel"),
    ("libxtst-dev", "=", "libxtst", "libXtst-devel", "libXtst-devel"),
    ("libxtst6", "=", "libxtst", "libXtst", "libXtst6"),
    ("libyaml-dev", "=", "libyaml", "libyaml-devel", "libyaml-devel"),
    ("libzstd-dev", "=", "zstd", "libzstd-devel", "libzstd-devel"),
    ("lld", "=", "lld", "lld", "lld"),
    ("lldb", "=", "lldb", "lldb", "lldb"),
    ("llvm", "=", "llvm", "llvm", "llvm"),
    ("llvm-dev", "=", "llvm", "llvm-devel", "llvm-devel"),
    ("m4", "=", "m4", "m4", "m4"),
    ("make", "=", "make", "make", "make"),
    // `eglinfo` / `glxinfo` (GPU jobs, 0.46.11): Arch splits the Mesa
    // demos into `mesa-utils`; Fedora and openSUSE keep the old name.
    ("mesa-utils", "=", "mesa-utils", "mesa-demos", "Mesa-demo-x"),
    ("mesa-vulkan-drivers", "=", "vulkan-swrast", "mesa-vulkan-drivers", "libvulkan_lvp"),
    ("mold", "=", "mold", "mold", "mold"),
    ("nasm", "=", "nasm", "nasm", "nasm"),
    ("ninja-build", "=", "ninja", "ninja-build", "ninja"),
    ("openssl", "=", "openssl", "openssl", "openssl"),
    ("patch", "=", "patch", "patch", "patch"),
    ("perl", "=", "perl", "perl", "perl"),
    ("pkg-config", "=", "pkgconf", "pkgconf-pkg-config", "pkg-config"),
    ("poppler-utils", "=", "poppler", "poppler-utils", "poppler-tools"),
    ("protobuf-compiler", "=", "protobuf", "protobuf-compiler", "protobuf-devel"),
    ("python3", "=", "python", "python3", "python3"),
    ("python3-dev", "=", "python", "python3-devel", "python3-devel"),
    ("python3-pip", "=", "python-pip", "python3-pip", "python3-pip"),
    ("python3-venv", "=", "python", "python3", "python3"),
    ("rsync", "=", "rsync", "rsync", "rsync"),
    ("shellcheck", "=", "shellcheck", "ShellCheck", "ShellCheck"),
    ("sqlite3", "=", "sqlite", "sqlite", "sqlite3"),
    ("strace", "=", "strace", "strace", "strace"),
    ("unzip", "=", "unzip", "unzip", "unzip"),
    ("uuid-dev", "=", "util-linux-libs", "libuuid-devel", "libuuid-devel"),
    ("valgrind", "=", "valgrind", "valgrind", "valgrind"),
    ("vulkan-tools", "=", "vulkan-tools", "vulkan-tools", "vulkan-tools"),
    ("wayland-protocols", "=", "wayland-protocols", "wayland-protocols-devel", "wayland-protocols-devel"),
    ("wget", "=", "wget", "wget", "wget"),
    ("xauth", "=", "xorg-xauth", "xorg-x11-xauth", "xauth"),
    ("xdg-utils", "=", "xdg-utils", "xdg-utils", "xdg-utils"),
    ("xvfb", "=", "xorg-server-xvfb", "xorg-x11-server-Xvfb", "xorg-x11-server-Xvfb"),
    ("xz-utils", "=", "xz", "xz", "xz"),
    ("yasm", "=", "yasm", "yasm", "yasm"),
    ("zip", "=", "zip", "zip", "zip"),
    ("zlib1g-dev", "=", "zlib", "zlib-devel", "zlib-devel"),
    ("zstd", "=", "zstd", "zstd", "zstd"),
];

/// Per distro version: `(canonical, distro, version prefix, manager,
/// native names)`. Ubuntu 24.04 and Debian 13 renamed many runtime
/// libraries for the 64-bit time_t move (`t64`); the old name is a
/// virtual package there.
pub const VERSION_ROWS: &[(&str, &str, &str, Mgr, &str)] = &[
    ("libasound2", "ubuntu", "24.04", Mgr::Apt, "libasound2t64"),
    ("libasound2", "debian", "13", Mgr::Apt, "libasound2t64"),
    ("libatk-bridge2.0-0", "ubuntu", "24.04", Mgr::Apt, "libatk-bridge2.0-0t64"),
    ("libatk-bridge2.0-0", "debian", "13", Mgr::Apt, "libatk-bridge2.0-0t64"),
    ("libcups2", "ubuntu", "24.04", Mgr::Apt, "libcups2t64"),
    ("libcups2", "debian", "13", Mgr::Apt, "libcups2t64"),
    ("libgtk-3-0", "ubuntu", "24.04", Mgr::Apt, "libgtk-3-0t64"),
    ("libgtk-3-0", "debian", "13", Mgr::Apt, "libgtk-3-0t64"),
    ("libssl3", "ubuntu", "24.04", Mgr::Apt, "libssl3t64"),
    ("libssl3", "debian", "13", Mgr::Apt, "libssl3t64"),
    ("libasound2t64", "debian", "12", Mgr::Apt, "libasound2"),
    ("libasound2t64", "ubuntu", "22.04", Mgr::Apt, "libasound2"),
];

fn row(canon: &str) -> Option<&'static (&'static str, &'static str, &'static str, &'static str, &'static str)> {
    ROWS.binary_search_by(|r| r.0.cmp(canon)).ok().map(|i| &ROWS[i])
}

/// Is there a row (or a version row) for `canon` at all?
pub fn known(canon: &str) -> bool {
    row(canon).is_some() || VERSION_ROWS.iter().any(|r| r.0 == canon)
}

/// `canon` on `mgr`, for a node running `distro` at `version`. apt takes
/// an unknown canonical name as written; the others need a row.
pub fn native(canon: &str, mgr: Mgr, distro: &str, version: &str) -> Option<Vec<String>> {
    if let Some(v) = VERSION_ROWS.iter().find(|r| r.0 == canon && r.3 == mgr && r.1 == distro && crate::host::version_prefix(version, r.2)) {
        return Some(v.4.split_whitespace().map(str::to_string).collect());
    }
    let col = match row(canon) {
        Some(r) => match mgr {
            Mgr::Apt => r.1,
            Mgr::Pacman => r.2,
            Mgr::Dnf => r.3,
            Mgr::Zypper => r.4,
        },
        None if mgr == Mgr::Apt => "=",
        None => "",
    };
    match col {
        "" => None,
        "=" => Some(vec![canon.to_string()]),
        names => Some(names.split_whitespace().map(str::to_string).collect()),
    }
}

/// A native package name as a manager writes it: the canonical charset
/// plus upper case (dnf/zypper `libX11-devel`). Never an option or a path.
pub fn valid_native(n: &str) -> bool {
    n.chars().next().is_some_and(|c| c.is_ascii_alphanumeric()) && crate::needs::valid_package(&n.to_ascii_lowercase())
}

/// How a virtual name resolved on a node (§5.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// A real package: install and check it as written.
    Same,
    /// Virtual with one provider: install and check that one.
    Provider(String),
    /// Virtual with several providers: refused (`package_ambiguous`).
    Ambiguous(Vec<String>),
    /// Nothing provides it (the dry run will say so).
    Unknown,
}

/// Parse `apt-cache showpkg <name>`: a name with versions is real; one
/// without versions but with reverse provides is virtual.
pub fn parse_apt_showpkg(text: &str) -> Resolved {
    #[derive(PartialEq)]
    enum Sec {
        None,
        Versions,
        Provides,
        Other,
    }
    let mut sec = Sec::None;
    let mut versions = 0;
    let mut providers: Vec<String> = Vec::new();
    for line in text.lines() {
        let t = line.trim_end();
        if t.ends_with(':') && !t.starts_with(' ') {
            sec = match t {
                "Versions:" => Sec::Versions,
                "Reverse Provides:" => Sec::Provides,
                _ => Sec::Other,
            };
            continue;
        }
        if t.trim().is_empty() {
            continue;
        }
        match sec {
            Sec::Versions => versions += 1,
            Sec::Provides => {
                if let Some(n) = t.split_whitespace().next() {
                    if crate::needs::valid_package(n) && !providers.iter().any(|p| p == n) {
                        providers.push(n.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    match (versions, providers.len()) {
        (v, _) if v > 0 => Resolved::Same,
        (_, 0) => Resolved::Unknown,
        (_, 1) => Resolved::Provider(providers.remove(0)),
        _ => {
            providers.sort();
            Resolved::Ambiguous(providers)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_sorted_unique_and_valid() {
        for w in ROWS.windows(2) {
            assert!(w[0].0 < w[1].0, "{} then {}: keep ROWS sorted (binary search)", w[0].0, w[1].0);
        }
        for r in ROWS {
            assert!(crate::needs::valid_package(r.0), "{}", r.0);
            for col in [r.1, r.2, r.3, r.4] {
                for n in col.split_whitespace().filter(|n| *n != "=") {
                    // Native names keep the same charset, except upper case
                    // (dnf/zypper `libX11-devel`).
                    let lower = n.to_ascii_lowercase();
                    assert!(crate::needs::valid_package(&lower), "{}: native {n:?}", r.0);
                }
            }
        }
        assert!(ROWS.len() >= 150, "the map covers the CI dev set ({} rows)", ROWS.len());
    }

    #[test]
    fn the_prd_examples_map() {
        assert_eq!(native("libssl-dev", Mgr::Pacman, "arch", "rolling"), Some(vec!["openssl".into()]));
        assert_eq!(native("libssl-dev", Mgr::Dnf, "fedora", "41"), Some(vec!["openssl-devel".into()]));
        assert_eq!(native("libssl-dev", Mgr::Zypper, "opensuse-tumbleweed", "20260101"), Some(vec!["libopenssl-devel".into()]));
        assert_eq!(native("pkg-config", Mgr::Pacman, "arch", "rolling"), Some(vec!["pkgconf".into()]));
        assert_eq!(native("zlib1g-dev", Mgr::Pacman, "arch", "rolling"), Some(vec!["zlib".into()]));
        assert_eq!(native("libasound2-dev", Mgr::Pacman, "arch", "rolling"), Some(vec!["alsa-lib".into()]));
        assert_eq!(native("build-essential", Mgr::Dnf, "fedora", "41"), Some(vec!["gcc".into(), "gcc-c++".into(), "make".into()]));
        assert_eq!(native("build-essential", Mgr::Apt, "ubuntu", "24.04"), Some(vec!["build-essential".into()]));
    }

    /// GPU jobs (0.46.11): `sys:mesa-utils` (`eglinfo`) installs on Arch,
    /// so the GPU acceptance command runs there.
    #[test]
    fn mesa_utils_maps_on_arch() {
        assert_eq!(native("mesa-utils", Mgr::Pacman, "arch", "rolling"), Some(vec!["mesa-utils".into()]));
        assert_eq!(native("mesa-utils", Mgr::Apt, "ubuntu", "24.04"), Some(vec!["mesa-utils".into()]));
    }

    #[test]
    fn apt_takes_any_name_others_need_a_row() {
        assert_eq!(native("libfoo-dev", Mgr::Apt, "debian", "12"), Some(vec!["libfoo-dev".into()]));
        assert_eq!(native("libfoo-dev", Mgr::Pacman, "arch", "rolling"), None, "no row: not satisfiable on Arch");
    }

    #[test]
    fn many_canonical_names_share_one_native() {
        for c in ["libgl1-mesa-dev", "libegl1-mesa-dev", "libgles2-mesa-dev"] {
            assert!(native(c, Mgr::Pacman, "arch", "rolling").unwrap().contains(&"mesa".to_string()), "{c}");
        }
    }

    #[test]
    fn t64_renames_are_per_version() {
        assert_eq!(native("libasound2", Mgr::Apt, "ubuntu", "24.04"), Some(vec!["libasound2t64".into()]));
        assert_eq!(native("libasound2", Mgr::Apt, "ubuntu", "22.04"), Some(vec!["libasound2".into()]));
        assert_eq!(native("libasound2t64", Mgr::Apt, "debian", "12"), Some(vec!["libasound2".into()]));
        assert_eq!(native("libasound2t64", Mgr::Apt, "debian", "13"), Some(vec!["libasound2t64".into()]));
    }

    #[test]
    fn sys_names_parse_native_and_canonical() {
        assert_eq!(SysName::parse("libssl-dev"), Some(SysName::Canonical("libssl-dev".into())));
        let n = SysName::parse("pacman:protobuf").unwrap();
        assert_eq!(n, SysName::Native { mgr: Mgr::Pacman, name: "protobuf".into() });
        assert_eq!(n.as_str(), "pacman:protobuf");
        assert_eq!(n.native(Mgr::Pacman, "arch", "rolling"), Some(vec!["protobuf".into()]));
        assert_eq!(n.native(Mgr::Apt, "ubuntu", "24.04"), None, "a native name fits only its manager");
        assert_eq!(SysName::parse("brew:protobuf"), None);
        assert_eq!(SysName::parse("apt:-y"), None);
    }

    #[test]
    fn showpkg_tells_virtual_from_real() {
        let virt = "Package: libfreetype6-dev\nVersions: \n\nReverse Depends: \n  libfoo,libfreetype6-dev\nDependencies: \nProvides: \nReverse Provides: \nlibfreetype-dev 2.13.2+dfsg-1build3 (= )\n";
        assert_eq!(parse_apt_showpkg(virt), Resolved::Provider("libfreetype-dev".into()));
        let real = "Package: libssl-dev\nVersions: \n3.0.13-0ubuntu3.4 (/var/lib/apt/lists/x_Packages)\n Description Language: \n\nReverse Depends: \nReverse Provides: \n";
        assert_eq!(parse_apt_showpkg(real), Resolved::Same);
        let two = "Package: mail-transport-agent\nVersions: \n\nReverse Provides: \npostfix 3.8 (= )\nexim4-daemon-light 4.97 (= )\n";
        assert_eq!(parse_apt_showpkg(two), Resolved::Ambiguous(vec!["exim4-daemon-light".into(), "postfix".into()]));
        assert_eq!(parse_apt_showpkg("N: Unable to locate package nope\n"), Resolved::Unknown);
    }
}
