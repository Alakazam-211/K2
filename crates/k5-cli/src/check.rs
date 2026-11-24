//! `k5 --version --check`: the version line, then the server's version.
//!
//! It prints what `--version` prints, then makes one `GET /boot-status`
//! with no credential (on this machine the daemon shows its version to
//! anyone) and answers on stdout: the skew line ([`skew::line`]) when the
//! two differ, or `server <v>: same version` when they match. Exit 0
//! either way; the version alone never fails a command.
//!
//! When the daemon can't be asked, one line on stderr says why: nothing
//! accepted the connection or no port is known is exit 7 (the shell's
//! code for a daemon that is not there), any other failure exit 1. A
//! daemon that answers without a version (a stranger's ghost view, a
//! status other than 200, a body that isn't JSON) is exit 1 too.
//!
//! The shell CLI takes `--check` for nothing: `k2 --version --check`
//! prints the version line alone.

use crate::exit::Exit;
use crate::out::json::{self, Value};
use crate::transport::{skew, Client, Error, Response};
use k2_client::brand::PRODUCT_NAME;

/// The route it reads.
pub const PATH: &str = "/boot-status";

/// What the check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The line for stdout: the server's version against ours.
    Answer(String),
    /// The line for stderr, and the exit.
    Failed(String, Exit),
}

/// The stdout line for the server's version `theirs` against `ours`.
pub fn answer_line(ours: &str, theirs: &str) -> String {
    if theirs == ours {
        format!("server {theirs}: same version")
    } else {
        skew::line(ours, theirs)
    }
}

/// The version in a `/boot-status` body.
pub fn version_of(body: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(body).ok()?;
    let Ok(Value::Object(pairs)) = json::parse(text) else {
        return None;
    };
    let (_, value) = pairs.iter().find(|(k, _)| k.as_str() == Some("version"))?;
    match value {
        Value::Str(v) => skew::clean(v.as_str()?).map(str::to_string),
        _ => None,
    }
}

/// What a `/boot-status` call came to.
pub fn outcome(ours: &str, result: Result<Response, Error>) -> Outcome {
    let r = match result {
        Ok(r) => r,
        Err(e @ Error::Unreachable { .. }) => {
            return Outcome::Failed(e.to_string(), Exit::DaemonUnreachable)
        }
        Err(e) => return Outcome::Failed(e.to_string(), Exit::Error),
    };
    if r.status != 200 {
        return Outcome::Failed(
            format!("{PRODUCT_NAME}'s server answered {PATH} with {}", r.status),
            Exit::Error,
        );
    }
    match version_of(&r.body) {
        Some(theirs) => Outcome::Answer(answer_line(ours, &theirs)),
        None => Outcome::Failed(
            format!("{PRODUCT_NAME}'s server didn't say its version at {PATH}"),
            Exit::Error,
        ),
    }
}

/// Asks `client` and says what it found. No line from the transport's
/// own compare: this answer is the line.
pub fn run(ours: &str, client: &mut Client) -> Outcome {
    skew::mark_shown();
    outcome(ours, client.get_public(PATH))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::Channel;

    fn ok(status: u16, body: &str) -> Result<Response, Error> {
        Ok(Response {
            status,
            body: body.as_bytes().to_vec(),
            channel: Channel::Tcp,
            permission: None,
            server_version: None,
        })
    }

    #[test]
    fn same_or_skewed() {
        assert_eq!(
            outcome(
                "0.46.16",
                ok(200, r#"{"phase":"ready","version":"0.46.16"}"#)
            ),
            Outcome::Answer("server 0.46.16: same version".into())
        );
        assert_eq!(
            outcome("0.46.16", ok(200, r#"{"version":"0.46.17"}"#)),
            Outcome::Answer(skew::line("0.46.16", "0.46.17"))
        );
    }

    #[test]
    fn no_version_is_exit_1() {
        for body in [
            r#"{"view":"public","phase":"ready"}"#,
            r#"{"version":46}"#,
            r#"{"version":"0.46 16"}"#,
            "not json",
            r#"["version"]"#,
        ] {
            let Outcome::Failed(line, exit) = outcome("0.46.16", ok(200, body)) else {
                panic!("{body}")
            };
            let want = format!("{PRODUCT_NAME}'s server didn't say its version at /boot-status");
            assert_eq!(
                (line.as_str(), exit),
                (want.as_str(), Exit::Error),
                "{body}"
            );
        }
        assert_eq!(
            outcome("0.46.16", ok(503, r#"{"version":"0.46.16"}"#)),
            Outcome::Failed(
                format!("{PRODUCT_NAME}'s server answered /boot-status with 503"),
                Exit::Error
            )
        );
    }

    #[test]
    fn a_daemon_that_is_not_there_is_exit_7() {
        let err = Error::Unreachable {
            endpoint: PATH.into(),
            err: std::io::Error::from(std::io::ErrorKind::ConnectionRefused),
        };
        let Outcome::Failed(line, exit) = outcome("0.46.16", Err(err)) else {
            panic!()
        };
        assert_eq!(exit, Exit::DaemonUnreachable);
        assert!(
            line.starts_with(&format!(
                "can't reach {PRODUCT_NAME}'s server for /boot-status: "
            )),
            "{line}"
        );
        let other = Error::Empty {
            endpoint: PATH.into(),
        };
        assert_eq!(
            outcome("0.46.16", Err(other)),
            Outcome::Failed(
                format!("{PRODUCT_NAME}'s server gave an empty answer to /boot-status"),
                Exit::Error
            )
        );
    }
}
