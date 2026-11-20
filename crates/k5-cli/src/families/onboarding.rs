//! `k5 onboarding`: the workspace-add flow's steps, as the shell CLI's
//! `onboarding` arm (`cli/src/router.sh`) and `cmd_onboarding_*`
//! (`cli/src/onboarding.sh`).
//!
//! 1. The settings and the two gates ([`crate::prelude`]); an open tool.
//! 2. The verb: `scan [folder]` (a GET), `adopt <file> [folder]`,
//!    `defer [folder]` and `start-fresh [folder]` (a POST each); `later`
//!    and `fresh` are the old names, with their warning first. The folder
//!    is `cd <it> && pwd` from this one (the logical path), the file is
//!    under it when relative. Any other verb is the usage line, exit 1.
//! 3. One request over TCP, its answer as it came (a refusal's body with
//!    exit 0, as the shell's), and the EXIT trap's lines.
//!
//! The shell answers a folder `cd` can't enter (its own line names the
//! script) and a set `CDPATH`, a word that is not UTF-8, a port curl
//! won't take and a `python3` the binary can't tell.

use super::{curl_port, without_nuls};
use crate::daemon;
use crate::exit::Exit;
use crate::out;
use crate::passport;
use crate::prelude::{self, Closing};
use crate::transport::encode::quote;
use crate::transport::{Call, Channel, Error};
use k2_client::cli_name;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["onboarding"];
/// The binary answers every form; [`run`] hands the ones it names to
/// the shell.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// The warning line first (an old name), the endpoint, whether it is
    /// a POST, the folder word, and `adopt`'s file.
    Send {
        warn: Option<&'static str>,
        endpoint: &'static str,
        post: bool,
        folder: String,
        file: Option<String>,
    },
    /// Lines on stderr, exit 1.
    Stop(Vec<&'static str>),
}

/// The words after `onboarding`.
pub fn parse(words: &[&str]) -> Ask {
    let arg = |i: usize| words.get(i).copied().unwrap_or("").to_string();
    let send = |warn, endpoint, post| Ask::Send {
        warn,
        endpoint,
        post,
        folder: arg(1),
        file: None,
    };
    match words.first().copied().unwrap_or("") {
        "scan" => send(None, "/cli/onboarding/scan", false),
        "defer" => send(None, "/cli/onboarding/skip", true),
        "start-fresh" => send(None, "/cli/onboarding/start-fresh", true),
        "later" => send(
            Some(concat!(
                "warning: 'onboarding later' is deprecated; use '",
                cli_name!(),
                " onboarding defer' instead."
            )),
            "/cli/onboarding/skip",
            true,
        ),
        "fresh" => send(
            Some(concat!(
                "warning: 'onboarding fresh' is deprecated; use '",
                cli_name!(),
                " onboarding start-fresh' instead."
            )),
            "/cli/onboarding/start-fresh",
            true,
        ),
        "adopt" => {
            let file = arg(1);
            if file.is_empty() {
                return Ask::Stop(vec![
                    concat!(
                        "Usage: ",
                        cli_name!(),
                        " onboarding adopt <file> [project-path]"
                    ),
                    concat!(
                        "Run '",
                        cli_name!(),
                        " onboarding scan' to see candidate files."
                    ),
                ]);
            }
            Ask::Send {
                warn: None,
                endpoint: "/cli/onboarding/adopt",
                post: true,
                folder: arg(2),
                file: Some(file),
            }
        }
        _ => Ask::Stop(vec![concat!(
            "Usage: ",
            cli_name!(),
            " onboarding <scan|adopt|defer|start-fresh> [args]"
        )]),
    }
}

/// `$(cd "$dir" && pwd)` from the logical folder `pwd`: `.` and `..`
/// taken off the words, links kept. `None`: `cd` can't enter it.
pub fn cd_pwd(pwd: &Path, dir: &str) -> Option<String> {
    let base = if dir.starts_with('/') {
        PathBuf::from(dir)
    } else {
        pwd.join(dir)
    };
    let mut parts: Vec<String> = Vec::new();
    for c in base.to_string_lossy().split('/') {
        match c {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other.to_string()),
        }
    }
    let joined = format!("/{}", parts.join("/"));
    Path::new(&joined)
        .is_dir()
        .then(|| joined.trim_end_matches('\n').to_string())
}

/// `k5 onboarding ...`: `args` is the whole command line after
/// `argv[0]`. `None`: the shell CLI answers it.
pub fn run(args: &[OsString]) -> Option<Exit> {
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    let ask = parse(&words);
    if !crate::python::family_native(crate::python::here()) {
        return None;
    }
    if std::env::var_os("CDPATH").is_some_and(|v| !v.is_empty()) {
        return None;
    }
    let pwd = daemon::logical_cwd(std::env::var_os("PWD")).ok()?;
    // Where `cd` would fail, the shell's own line is the answer.
    let target = match &ask {
        Ask::Send { folder, .. } => {
            let f = if folder.is_empty() {
                pwd.to_string_lossy().into_owned()
            } else {
                folder.clone()
            };
            Some(cd_pwd(&pwd, &f)?)
        }
        Ask::Stop(_) => None,
    };
    let mut settings = match daemon::current() {
        Ok(s) => s,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    if let Some(gate) = prelude::gate(&settings) {
        return Some(gate.stop());
    }
    settings.port = Some(settings.port_text.as_deref().and_then(curl_port)?);
    let (warn, endpoint, post, file) = match ask {
        Ask::Stop(lines) => {
            for l in lines {
                out::stderr_line(l);
            }
            return Some(Exit::Error);
        }
        Ask::Send {
            warn,
            endpoint,
            post,
            file,
            ..
        } => (warn, endpoint, post, file),
    };
    if let Some(w) = warn {
        out::stderr_line(w);
    }
    let target = target.unwrap_or_default();
    let mut param = format!("project={}", quote(&target));
    if let Some(f) = file {
        let source = if f.starts_with('/') {
            f
        } else {
            format!("{target}/{f}")
        };
        param.push_str(&format!("&source={}", quote(&source)));
    }
    let would_refuse = passport::current(args).would_refuse();
    let mut client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    let passport = client.passport().is_some();
    let mut closing = Closing::default();
    let call = if post {
        Call::RequestPost(&[&param])
    } else {
        Call::Request(&[&param])
    };
    let exit = match client.call(Channel::Tcp, endpoint, call) {
        Ok(r) => {
            closing.answer(&r, passport);
            match out::write_flushed(&mut std::io::stdout().lock(), &without_nuls(&r.body)) {
                Ok(()) => Exit::Ok,
                Err(e) => out::after_stdout_error(&e),
            }
        }
        Err(e @ Error::NoAnswer(_)) => {
            out::stderr_line(&e.to_string());
            Exit::Error
        }
        Err(e) => Exit::from_curl(e.curl_code()),
    };
    closing.flush(&mut client, exit);
    Some(exit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_the_shells() {
        assert!(matches!(parse(&[]), Ask::Stop(l) if l.len() == 1));
        assert!(matches!(parse(&["adopt"]), Ask::Stop(l) if l.len() == 2));
        assert!(matches!(
            parse(&["later", "x"]),
            Ask::Send { warn: Some(_), endpoint: "/cli/onboarding/skip", post: true, folder, .. } if folder == "x"
        ));
        assert!(matches!(
            parse(&["adopt", "CLAUDE.md", "/p"]),
            Ask::Send { file: Some(f), folder, .. } if f == "CLAUDE.md" && folder == "/p"
        ));
    }

    #[test]
    fn cd_then_pwd() {
        let tmp = std::env::temp_dir();
        let t = tmp.to_string_lossy().trim_end_matches('/').to_string();
        assert_eq!(cd_pwd(Path::new("/"), &t).as_deref(), Some(t.as_str()));
        assert_eq!(cd_pwd(&tmp, ".").as_deref(), Some(t.as_str()));
        assert_eq!(cd_pwd(&tmp, "no-such-folder-k5"), None);
    }
}
