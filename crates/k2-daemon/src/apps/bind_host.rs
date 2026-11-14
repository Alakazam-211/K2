//! Where an App listens: `HOST` and `PORT`, set by K2 in every lane. The
//! contract is one line: **bind `$HOST:$PORT` as given**.
//!
//! K2 reaches an App from the box itself: the health probe and the App's
//! front both dial `127.0.0.1:<port>` on the host.
//!
//! - The process engine and the process exception run the App on the
//!   host's own network: `HOST=127.0.0.1`, so the App answers K2 and is
//!   never on the box's other interfaces.
//! - A container (the guarded wall and the open path's relaxed one) has
//!   its own network namespace. Its port is published on host loopback
//!   only, and those connections arrive on the container's own interface,
//!   not on the container's loopback: an App bound to `127.0.0.1` inside
//!   never sees them, and the probe gets "connection reset" until the
//!   deploy fails. There `HOST=0.0.0.0`: inside the namespace the only
//!   way in is that published port, which sits on host loopback.
//!
//! `K2_HOST` is a different variable: where K2 is (the daemon, or the
//! gateway link inside a wall), never the App's own address.
//!
//! A manifest can't set `HOST` (a manifest's `127.0.0.1` would break the
//! container lanes, and its `0.0.0.0` would put a host-network App on the
//! LAN); K2's value always wins.

/// The variable an App binds (with `PORT`).
pub const HOST_ENV: &str = "HOST";

/// `HOST` for an App on the host's own network (process engine, process
/// exception).
pub const ON_HOST_NETWORK: &str = "127.0.0.1";

/// `HOST` inside a container (guarded or relaxed wall).
pub const IN_CONTAINER: &str = "0.0.0.0";

#[cfg(test)]
#[path = "bind_host_tests.rs"]
mod tests;
