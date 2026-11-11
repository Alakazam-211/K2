//! The wire between `k2-daemon` and `k2-deskd` ("K2 Screen", the remote
//! desktop helper; `prd-remote-desktop-tab-v1` §3.2–§3.3).
//!
//! **The framing and the units are K2 Render's helper link, protocol 4**
//! ([`link`], `k2_simvideo::link`, re-exported by [`wire`]); this crate adds
//! what is deskd's own: the hello's role and TCC state, the desktop
//! requests and events, fd passing and the key map.
//!
//! - [`wire`]: `[u32 BE len][u8 kind][payload]`, `J` = JSON, `F` = one
//!   encoded unit (protocol 4's header: codec, flags, layer, display).
//! - [`msg`]: hello (with protocol 4's fields), requests (`op`: deskd's own
//!   plus protocol 4's `layers`, `keyframe` and `rate`), replies (`id`),
//!   events (`event`).
//! - [`keymap`]: `KeyboardEvent.code` → macOS virtual key and evdev
//!   (X11 keycode = evdev + 8).
//! - [`fdpass`]: the frames pipe's read end over `SCM_RIGHTS` (macOS).
//!
//! **Transport.** macOS: deskd is a LaunchAgent and listens on
//! `~/.k2/run/deskd.sock` (0600 in a 0700 folder), one connection at a time,
//! with a code-signature check of the peer (ordinary hygiene, not a
//! boundary: same-user code is accepted for v1, Rosson 2026-11-10). Right
//! after `hello`, deskd sends one byte carrying the frames pipe's read end.
//! Linux: the daemon spawns deskd as a child; requests on stdin, replies and
//! events on stdout, frames on fd 3 (the k2-simhelper way).
//!
//! Both directions use the same framing (k2-simhelper reads JSON lines on
//! stdin; deskd doesn't, so a socket and a pipe read the same way).

pub mod keymap;
pub mod msg;
pub mod wire;

#[cfg(unix)]
pub mod fdpass;

/// The engine's helper link (protocol 4), for the unit header, the layer
/// list and the hello fields.
pub use k2_simvideo::link;

/// The helper protocol deskd speaks: protocol 4, the engine's. deskd
/// speaks only this one (`hello.accepts = [4]`); the daemon refuses any
/// other number with `needs_update` after one restart.
pub const PROTOCOL_VERSION: u32 = link::PROTOCOL_VERSION;
/// deskd's own build line (this crate's version; k2-deskd reports it).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// `hello.role` for a desktop helper (k2-simhelper has none).
pub const ROLE: &str = "desktop";

/// The executable name, in `K2 Screen.app/Contents/MacOS` and on Linux.
pub const BINARY_NAME: &str = "k2-deskd";
/// The nested macOS bundle (`K2.app/Contents/Helpers/K2 Screen.app`).
pub const APP_NAME: &str = "K2 Screen.app";
/// Its bundle id and code identifier (what TCC lists as "K2 Screen").
pub const BUNDLE_ID: &str = "dev.k2.app.screen";
/// The LaunchAgent that starts it, so launchd makes it its own
/// responsible process (TCC credits the grant to it, not the daemon).
pub const LAUNCH_AGENT_LABEL: &str = "dev.k2.deskd";
/// The socket's file name under `~/.k2/run`.
pub const SOCKET_NAME: &str = "deskd.sock";

/// The peer deskd serves on macOS: K2's signed daemon.
pub const DAEMON_IDENTIFIER: &str = "dev.k2.app.daemon";
/// K2's Apple team (`scripts/release.sh`).
pub const TEAM_ID: &str = "36B8R93HXV";

/// The code requirement a release deskd holds its peer to.
pub fn release_peer_requirement() -> String {
    format!(
        "anchor apple generic and certificate leaf[subject.OU] = \"{TEAM_ID}\" and identifier \"{DAEMON_IDENTIFIER}\""
    )
}

/// Debug builds of deskd also serve a daemon signed with an Apple
/// Development certificate (vs-live R7): dev mode signs with that, and an
/// ad hoc signature would lose TCC on every build. Never in a release
/// deskd (`k2_screen_helper_verify` checks the string isn't in it).
pub fn dev_peer_requirement() -> String {
    format!("anchor apple generic and identifier \"{DAEMON_IDENTIFIER}\"")
}

/// The marker a debug deskd carries; the release check refuses a binary
/// that contains it.
pub const DEV_REQUIREMENT_MARKER: &str = "K2_DESKD_DEV_PEER_REQUIREMENT";
