//! The Zen verbs' requests, as the shell sends them: each one in its own
//! `raw=$(…)`, so each gets a client of its own made from the settings
//! as they are now (a passport one call heals is not the next one's).
//!
//! - `cli_request` and `cli_post_json` over TCP with `$TOKEN`: the owner
//!   token outside a K2 terminal. Inside one the shell sends the owner
//!   token from disk (COMPAT-58); the binary sends the passport
//!   (`owner-token-with-passport`, PRD 13.9 #4).
//! - `_zen_session_post`, `_zen_person_post` and `_zen_session_get`: the
//!   same, but inside a K2 terminal (`HOOK_SOCK` set) with
//!   `${SCOPED_TOKEN:-$HOOK_TOKEN}`, the passport, and with none no
//!   credential at all, never the owner token. There the call is not the
//!   last command of its function, so a failed one is an empty answer
//!   (`set -e` is off inside `$(…)`); outside a K2 terminal it ends the
//!   command with curl's code, as a plain call does.
//! - `_zen_session_post_file`: `_k2_curl` with the body from a file, at
//!   most 120 s, the same credential, no note; any failure is an empty
//!   answer.
//!
//! A call with no answer prints the transport's line and ends the command
//! (exit 1). Each answer goes through the EXIT trap's notes
//! ([`Closing`]); the body comes back as `$(…)` holds it.

use super::super::without_nuls;
use crate::daemon::Settings;
use crate::exit::Exit;
use crate::out;
use crate::prelude::Closing;
use crate::transport::{Auth, Call, Channel, Client, Error, OwnCurl, Response};
use std::time::Duration;

/// A body as `$(…)` holds it: NULs and trailing newlines gone.
pub fn held(r: &Response) -> Vec<u8> {
    let mut b = without_nuls(&r.body);
    while b.last() == Some(&b'\n') {
        b.pop();
    }
    b
}

/// The settings, the notes and the main shell's client.
pub struct Zen {
    pub settings: Settings,
    pub would_refuse: bool,
    pub closing: Closing,
}

impl Zen {
    pub fn new(settings: Settings, would_refuse: bool) -> Zen {
        Zen {
            settings,
            would_refuse,
            closing: Closing::default(),
        }
    }

    /// A client as one `$(…)` starts it. `session`: the session wrappers'
    /// credential (the passport inside a K2 terminal, else none).
    fn client(&self, session: bool) -> Result<Client, Exit> {
        let mut c = self.settings.client(self.would_refuse).map_err(|e| {
            out::stderr_line(&format!("Error: {e}"));
            Exit::Error
        })?;
        if session && self.settings.session && matches!(c.auth, Auth::Owner(_)) {
            c.auth = Auth::None;
            c.no_token = true;
        }
        Ok(c)
    }

    /// The EXIT trap's lines, with the main shell's client.
    pub fn finish(self, exit: Exit) -> Exit {
        if let Ok(mut c) = self.settings.client(self.would_refuse) {
            self.closing.flush(&mut c, exit);
        }
        exit
    }

    fn took(
        &mut self,
        client: &Client,
        sent: Result<Response, Error>,
        soft: bool,
    ) -> Result<Vec<u8>, Exit> {
        let passport = client.passport().is_some();
        match sent {
            Ok(r) => {
                self.closing.answer(&r, passport);
                Ok(held(&r))
            }
            Err(Error::NoAnswer(n)) => {
                out::stderr_line(&n.to_string());
                Err(Exit::Error)
            }
            Err(_) if soft => Ok(Vec::new()),
            Err(e) => Err(Exit::from_curl(e.curl_code())),
        }
    }

    /// `raw=$(cli_request <endpoint> <pairs>)`.
    pub fn get(&mut self, endpoint: &str, pairs: &[&str]) -> Result<Vec<u8>, Exit> {
        let mut c = self.client(false)?;
        let sent = c.call(Channel::Tcp, endpoint, Call::Request(pairs));
        self.took(&c, sent, false)
    }

    /// `raw=$(cli_post_json <endpoint> <body>)`.
    pub fn post(&mut self, endpoint: &str, body: &[u8]) -> Result<Vec<u8>, Exit> {
        let mut c = self.client(false)?;
        let sent = c.call(Channel::Tcp, endpoint, Call::PostJson(body));
        self.took(&c, sent, false)
    }

    /// `raw=$(_zen_session_post <endpoint> <body>)` (and
    /// `_zen_person_post`, the same call).
    pub fn session_post(&mut self, endpoint: &str, body: &[u8]) -> Result<Vec<u8>, Exit> {
        let mut c = self.client(true)?;
        let sent = c.call(Channel::Tcp, endpoint, Call::PostJson(body));
        let soft = self.settings.session;
        self.took(&c, sent, soft)
    }

    /// `raw=$(_zen_session_get <endpoint> <pairs>)`.
    pub fn session_get(&mut self, endpoint: &str, pairs: &[&str]) -> Result<Vec<u8>, Exit> {
        let mut c = self.client(true)?;
        let sent = c.call(Channel::Tcp, endpoint, Call::Request(pairs));
        let soft = self.settings.session;
        self.took(&c, sent, soft)
    }

    /// `_zen_session_post_file <endpoint> <file> || true` in `$(…)`.
    pub fn post_file(&mut self, endpoint: &str, body: &[u8]) -> Result<Vec<u8>, Exit> {
        let mut c = self.client(true)?;
        let credential = match &c.auth {
            Auth::Passport { passport, .. } => Some(passport.as_str().to_string()),
            Auth::Owner(t) => Some(t.clone()),
            Auth::None => None,
        };
        let how = OwnCurl {
            post: true,
            credential: credential.as_deref(),
            max_time: Duration::from_secs(120),
            note: false,
        };
        Ok(match c.own_curl(endpoint, how, body) {
            Ok(r) => held(&r),
            Err(_) => Vec::new(),
        })
    }
}
