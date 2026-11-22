//! What an answer's head says about the daemon, read as each answer comes
//! in (a retried one too; never a request of its own): its version, for
//! the out-of-date line ([`super::skew`]), and its brand, for the line about
//! another brand's server on this machine ([`crate::brand_check`]).
//!
//! The brand headers (`X-K5-Server-Brand`, `X-K5-Server-Cli`) are read
//! only from a daemon on this computer: its cell socket, or TCP to a
//! loopback address. A daemon sends them to this machine only.

use super::skew;
use super::wire::{Answer, Target};

/// Notes `answer`'s version against ours (`version`), and its brand when
/// `target` is on this machine.
pub fn note_answer(version: &str, answer: &Answer, target: &Target) {
    skew::note(version, answer.server_version.as_deref());
    if let Some((brand, cli)) = &answer.server_brand {
        if on_this_machine(target) {
            crate::brand_check::answer(Some(brand), Some(cli));
        }
    }
}

/// A daemon target on this computer: its cell socket, or TCP to a
/// loopback address.
fn on_this_machine(target: &Target) -> bool {
    match target {
        Target::Tcp { host, .. } => {
            matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1" | "[::1]")
        }
        Target::Tls { .. } => false,
        #[cfg(unix)]
        Target::Unix(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_target_on_this_machine_is_read_for_the_brand() {
        let tcp = |host: &str| Target::Tcp {
            host: host.into(),
            port: 1,
        };
        for host in ["127.0.0.1", "localhost", "::1", "[::1]"] {
            assert!(on_this_machine(&tcp(host)), "{host}");
        }
        for host in ["10.0.0.2", "box.example.com", "127.0.0.2", "0.0.0.0"] {
            assert!(!on_this_machine(&tcp(host)), "{host}");
        }
        let tls = Target::Tls {
            host: "127.0.0.1".into(),
            port: 1,
            via: None,
        };
        assert!(!on_this_machine(&tls));
        #[cfg(unix)]
        assert!(on_this_machine(&Target::Unix(std::path::PathBuf::from(
            "/tmp/x.sock"
        ))));
    }
}
