//! LIVE check of the root helper's `creds` verbs against the real systemd
//! on a Linux test box, as root (`scripts/test/systemd-cred-live.sh` runs
//! it). Never on a box that matters: it writes throwaway units and edits
//! the systemd-cred allow list (restored byte for byte after).
//!
//! It calls the helper's own `creds::{put, bind, restart, unbind, remove,
//! status}` with the real production context (root `/`, real systemctl,
//! busctl and systemd-creds). Only the caller walk (sudo → the daemon's
//! main pid) is skipped: a synthetic daemon caller stands in; that walk has
//! its own unit tests. It checks:
//!  1. put seals the value into /etc/k2/credstore.encrypted (0600, never
//!     the plaintext);
//!  2. bind writes the drop-in, and systemd's real D-Bus answer (busctl)
//!     confirms K2's path; the unit reads the value from %d;
//!  3. restart queues a try-restart; a second within 30 s is refused;
//!  4. a unit that already loads the same credential ID is refused;
//!  5. another daemon can't remove it;
//!  6. a re-put (rotation) reaches the unit;
//!  7. unbind and remove take it all off; status lists no orphans of ours.
//!
//! Guard: refuses unless euid 0, Linux, and K2_CREDS_LIVE_OK=1.

fn main() {
    #[cfg(unix)]
    imp::main();
    #[cfg(not(unix))]
    {
        eprintln!("creds_live: server-only feature (Linux boxes); not shipped on personal Windows machines");
        std::process::exit(2);
    }
}

// server-only feature (Linux boxes); not shipped on personal Windows machines
#[cfg(unix)]
#[path = "unix/creds_live.rs"]
mod imp;
