//! What is left of the old Companion server.
//!
//! The old server ran behind an ngrok tunnel with its own password login.
//! It is gone: the K2 Companion phone app now pairs through K2 Connect and
//! talks to the daemon's normal routes. Two pieces stay:
//!
//! - `cli_routes`: the DB-backed `/cli/companion/presets` and
//!   `/cli/companion/projects` answers the phone app and `cli/k2` read.
//! - `keychain`: settings reset still deletes a password hash an old
//!   install may have left in the macOS login keychain.

pub mod cli_routes;
pub mod keychain;
