//! The socket paths: the calls that go to the cell socket first and on
//! to TCP by the shell's rules (`cli_send`, `cli_send_json`,
//! `_compute_send`, `_sidecar_send`, `_cli_use_passport_project`), the
//! calls with no passport to send there, and the vault socket
//! (`_vault_sock_call`, `_migrate_send`). The requests themselves are
//! built in the client core.

use super::*;

/// The routes the cell socket serves (`_uds_eligible`,
/// `cli/src/cell-socket.sh`). Anything else goes over TCP.
pub fn uds_eligible(endpoint: &str) -> bool {
    const EXACT: &[&str] = &[
        "/cli/workspace/msg",
        "/cli/awareness/publish",
        "/cli/folder-trust/status",
        "/cli/folder-trust/request",
        "/cli/terminal/read",
        "/cli/domains",
        "/cli/domains/names",
        "/cli/domains/names/remove",
        "/cli/certs",
        "/cli/connections",
        "/cli/federation/peers",
        "/cli/federation/peer-roster",
        "/cli/federation/peer-rosters",
        "/cli/federation/send",
        "/cli/heartbeat-log",
        "/cli/thread",
        "/cli/browser/notes",
        "/cli/browser/notes/redeliver",
        // Postal: the agent's own address, friends, cards and mail. Never
        // code, settings or owner-link (people only: TCP).
        "/cli/postal/status",
        "/cli/postal/friends",
        "/cli/postal/invite",
        "/cli/postal/remove",
        "/cli/postal/block",
        "/cli/postal/link-p5",
        "/cli/postal/claim",
        "/cli/postal/decide",
        "/cli/postal/enable",
        "/cli/postal/send",
        "/cli/postal/route",
    ];
    // Owner-only heartbeat routes, before `/cli/heartbeat/*`.
    const NOT: &[&str] = &[
        "/cli/heartbeat/install-launchd",
        "/cli/heartbeat/uninstall-launchd",
        "/cli/heartbeat/apply-wake-scheduler",
        "/cli/heartbeat/list-all",
        "/cli/heartbeat/fires-list-all",
        "/cli/heartbeat/set-show-sessions",
    ];
    const UNDER: &[&str] = &[
        "/cli/inbox/",
        "/cli/review-checklist/",
        "/cli/dns/",
        "/cli/certs/",
        "/cli/publish/",
        "/cli/heartbeat/",
        "/cli/thread/",
    ];
    if NOT.contains(&endpoint) {
        return false;
    }
    EXACT.contains(&endpoint) || UNDER.iter().any(|p| endpoint.starts_with(p))
}

impl Client {
    /// There is a passport, its cell socket is a socket (`[ -S ]`) and no
    /// remote host forces TCP.
    pub fn has_cell_socket(&self) -> bool {
        self.cell_socket().is_some()
    }

    /// `cli_send`: the cell socket when there is a passport with a live
    /// socket and the route is one it serves; its answer when it has one
    /// (a body of only newlines and NULs is none: the shell holds it in
    /// `$(…)`) that is not "not served on cell socket". Else a GET goes on
    /// to TCP, and a POST only when the connect failed (curl 7, or 75 when
    /// the OS refused it) or the daemon said "not served on cell socket"
    /// (no handler ran). Any other end of a POST there (28, 52, 56, an
    /// empty answer, 76, ...) may have run it: [`Error::MaybeApplied`],
    /// never sent again.
    pub fn send(&mut self, endpoint: &str, call: Call<'_>) -> Result<Response, Error> {
        self.send_on(endpoint, call, uds_eligible(endpoint))
    }

    /// `_K2_UDS_TRY=1 cli_send`: [`Client::send`] for any route, the cell
    /// socket first whatever [`uds_eligible`] says, so a route agents
    /// can't use answers with the socket's refusal (a permission card when
    /// cards are on), where TCP has only a plain 403.
    pub fn send_trying(&mut self, endpoint: &str, call: Call<'_>) -> Result<Response, Error> {
        self.send_on(endpoint, call, true)
    }

    fn send_on(
        &mut self,
        endpoint: &str,
        call: Call<'_>,
        eligible: bool,
    ) -> Result<Response, Error> {
        let t0 = Instant::now();
        if eligible {
            self.no_owner_in_session(endpoint)?;
        }
        if self.cell_socket().is_some() && eligible {
            match self.call_from(Channel::Uds, endpoint, call, t0) {
                Ok(r) if contains(&r.body, NOT_SERVED) => {}
                Ok(r) if r.body.iter().any(|b| !matches!(b, b'\n' | 0)) => return Ok(r),
                Ok(_) if call.is_post() => return Err(maybe_applied(endpoint, t0)),
                Ok(_) => {}
                Err(f)
                    if call.is_post()
                        && !matches!(
                            f.error.curl_code(),
                            wire::curl::COULDNT_CONNECT | LOCAL_TRANSPORT_DENIED
                        ) =>
                {
                    return Err(maybe_applied(endpoint, t0))
                }
                Err(_) => {}
            }
        }
        self.call_from(Channel::Tcp, endpoint, call, t0)
            .map_err(|f| f.error)
    }

    /// `cli_send_json METHOD PATH [payload]`: GET with `payload` as a
    /// pre-encoded query, or POST with it as a JSON body. The cell socket
    /// when there is a passport with a live socket (any route), then TCP
    /// when that failed, said "not served on cell socket" or gave status
    /// 0. The shell prints `<body>\n<status>`; here [`Response`] holds both
    /// and an error stands for its `000`. A POST that went out on the cell
    /// socket and failed is [`Error::MaybeApplied`], never sent again over
    /// TCP; a GET goes to TCP, as the shell's does.
    pub fn send_json(&mut self, post: bool, path: &str, payload: &str) -> Result<Response, Error> {
        let kind = if post {
            Kind::SendJsonPost
        } else {
            Kind::SendJsonGet
        };
        self.send_json_as(kind, path, payload)
    }

    /// `_sidecar_send GET <path> <query>`: `cli_send_json GET`'s requests
    /// and channels (the cell socket, then TCP when that failed, said "not
    /// served on cell socket" or gave status 0), at most 120 s each, with
    /// no heal and no note. The shell prints `<body>\n<status>`; an error
    /// here stands for its `000`.
    pub fn sidecar_get(&mut self, path: &str, payload: &str) -> Result<Response, Error> {
        self.send_json_as(Kind::SidecarGet, path, payload)
    }

    /// `_sidecar_send POST <path> <json>`: as [`Client::sidecar_get`], a
    /// POST with `payload` as its JSON body. Sent once: it goes on to TCP
    /// only after a failed connect on the cell socket (curl 7) or "not
    /// served on cell socket"; any other failure there may have run it
    /// ([`Error::MaybeApplied`]).
    pub fn sidecar_post(&mut self, path: &str, payload: &str) -> Result<Response, Error> {
        self.send_json_as(Kind::SidecarPost, path, payload)
    }

    /// `_vault_sock_call METHOD <route> [json]`: one request to the vault
    /// socket at `sock` (`<K2 home>/vault.sock`), for `/cli/vault/<route>`
    /// (`route` may carry its `?query`), with `X-K2-Tty: 1` when `tty`.
    /// No credential. A GET is tried again after a refused connect as
    /// `_k2_curl` does; a POST (its body from stdin) never. The shell holds
    /// `<body>\n<status>`, and any failure is its `000` with an empty body;
    /// that is the caller's.
    pub fn vault_sock(
        &mut self,
        sock: &Path,
        post: bool,
        route: &str,
        payload: &[u8],
        tty: bool,
    ) -> Result<Response, Error> {
        self.sock_at(sock, post, &format!("/cli/vault/{route}"), payload, tty)
    }

    /// `_migrate_send METHOD <route> [json]`'s vault socket arm
    /// (`cli/src/migrate.sh`): [`Client::vault_sock`]'s request for
    /// `/cli/migrate/secrets/<route>`, at most 600 s, and a GET is tried
    /// once (`_K2_CURL_TRIES=1`).
    pub fn migrate_sock(
        &mut self,
        sock: &Path,
        post: bool,
        route: &str,
        payload: &[u8],
        tty: bool,
    ) -> Result<Response, Error> {
        self.migrate_sock = true;
        let endpoint = format!("/cli/migrate/secrets/{route}");
        let r = self.sock_at(sock, post, &endpoint, payload, tty);
        self.migrate_sock = false;
        r
    }

    fn sock_at(
        &mut self,
        sock: &Path,
        post: bool,
        endpoint: &str,
        payload: &[u8],
        tty: bool,
    ) -> Result<Response, Error> {
        let kind = if post {
            Kind::VaultSockPost
        } else {
            Kind::VaultSockGet
        };
        self.vault_sock = Some((sock.to_path_buf(), tty));
        let r = self
            .run(kind, Channel::Uds, endpoint, &[], payload, Instant::now())
            .map_err(|f| f.error);
        self.vault_sock = None;
        r
    }

    /// `cli_post_form_uds <endpoint> <pairs>` with no passport (0.46.17):
    /// the same form POST on the cell socket at `sock`, with no
    /// `Authorization` line at all (never an empty Bearer). Everything
    /// else is [`Call::PostForm`]'s on [`Channel::Uds`]; with no passport
    /// there is nothing to heal.
    pub fn post_form_uds_bare(
        &mut self,
        sock: &Path,
        endpoint: &str,
        pairs: &[&str],
    ) -> Result<Response, Error> {
        self.bare_sock = Some(sock.to_path_buf());
        let r = self.call(Channel::Uds, endpoint, Call::PostForm(pairs));
        self.bare_sock = None;
        r
    }

    /// `cli_send` in a session with no passport (0.46.17): when `sock` is a
    /// live socket, the route one it serves and no remote host forces TCP,
    /// the call goes on the cell socket first with no `Authorization` line
    /// at all (the fixed `cli_request_uds`, `cli_post_json_uds` and
    /// `cli_post_form_uds`). Its answer when it has one, as
    /// [`Client::send`] reads it. "Not served on cell socket" goes on to TCP
    /// with this client's own credential (`_k2_sock_fallback_token` keeps
    /// `$TOKEN` then); any other failure goes on as [`Client::send`]'s does,
    /// where the owner token from disk is never sent
    /// ([`Error::NoOwnerInSession`]). With a passport, or no live socket,
    /// it is [`Client::send`].
    pub fn send_bare(
        &mut self,
        sock: &Path,
        endpoint: &str,
        call: Call<'_>,
    ) -> Result<Response, Error> {
        if self.passport().is_some()
            || self.force_tcp
            || !is_socket(sock)
            || !uds_eligible(endpoint)
        {
            return self.send(endpoint, call);
        }
        let t0 = Instant::now();
        self.bare_sock = Some(sock.to_path_buf());
        let r = self.call_from(Channel::Uds, endpoint, call, t0);
        self.bare_sock = None;
        match r {
            Ok(r) if contains(&r.body, NOT_SERVED) => {
                return self
                    .call_from(Channel::Tcp, endpoint, call, t0)
                    .map_err(|f| f.error)
            }
            Ok(r) if r.body.iter().any(|b| !matches!(b, b'\n' | 0)) => return Ok(r),
            Ok(_) if call.is_post() => return Err(maybe_applied(endpoint, t0)),
            Ok(_) => {}
            Err(f)
                if call.is_post()
                    && !matches!(
                        f.error.curl_code(),
                        wire::curl::COULDNT_CONNECT | LOCAL_TRANSPORT_DENIED
                    ) =>
            {
                return Err(maybe_applied(endpoint, t0))
            }
            Err(_) => {}
        }
        self.no_owner_in_session(endpoint)?;
        self.call_from(Channel::Tcp, endpoint, call, t0)
            .map_err(|f| f.error)
    }

    /// `_cli_use_passport_project`'s request: `/cli/whoami` with the
    /// passport, on the cell socket when there is a live one, then over
    /// TCP (the passport as Bearer) when that failed or its answer, as
    /// `$(…)` holds it, is empty or says "not served on cell socket".
    /// At most 15 s each; no heal, no line, no note. What `out` holds
    /// after it: the answer's bytes with the NULs and trailing newlines
    /// dropped, or nothing when it failed. Needs a passport.
    pub fn passport_project(&mut self) -> Vec<u8> {
        let held = |r: &Response| {
            let mut b: Vec<u8> = r.body.iter().copied().filter(|b| *b != 0).collect();
            while b.last() == Some(&b'\n') {
                b.pop();
            }
            b
        };
        let t0 = Instant::now();
        let kind = Kind::PassportProject;
        if self.cell_socket().is_some() {
            if let Ok(r) = self.run(kind, Channel::Uds, PROBE_ENDPOINT, &[], &[], t0) {
                let out = held(&r);
                if !out.is_empty() && !contains(&out, NOT_SERVED) {
                    return out;
                }
            }
        }
        match self.run(kind, Channel::Tcp, PROBE_ENDPOINT, &[], &[], t0) {
            Ok(r) => held(&r),
            Err(_) => Vec::new(),
        }
    }

    /// `_compute_send METHOD PATH [payload] [tcp]`: `cli_send_json`'s
    /// requests and channels (the cell socket when there is a passport with
    /// a live socket and `tcp` is false, then TCP when that failed, said
    /// "not served on cell socket" or gave status 0), with `_compute_send`'s
    /// time limits ([`COMPUTE_GET_MAX_TIME`], [`Client::compute_post_max_time`])
    /// and no heal and no note. A POST goes on to TCP only after a failed
    /// connect (curl 7: `_compute_send` reads `_k2_curl`'s own code, so a
    /// connect the OS refused is 7 too) or "not served on cell socket"; any
    /// other failure there is [`Error::MaybeApplied`], never sent again. The
    /// shell prints `<body>\n<status>`; [`Response`] holds both, and an
    /// error stands for its `000` (see [`Client::refused_before_answer`]
    /// for the `-w` lines of the tries before).
    ///
    /// The credential is the shell's: the passport on the socket, and over
    /// TCP the passport when there is one, else the owner token. In a
    /// session (`HOOK_SOCK` set) whose cell socket failed, TCP never carries
    /// the owner token from disk (`_k2_sock_fallback_token`): with no
    /// passport nothing is sent ([`Error::NoOwnerInSession`], exit 1), as
    /// for every socket-first sender. A `tcp` caller, or a remote host,
    /// keeps its own token, as the shell's does.
    pub fn compute_send(
        &mut self,
        post: bool,
        path: &str,
        payload: &str,
        tcp: bool,
    ) -> Result<Response, Error> {
        let kind = if post {
            Kind::ComputePost
        } else {
            Kind::ComputeGet
        };
        let before = self.force_tcp;
        self.force_tcp = before || tcp;
        let r = self.send_json_as(kind, path, payload);
        self.force_tcp = before;
        r
    }

    fn send_json_as(&mut self, kind: Kind, path: &str, payload: &str) -> Result<Response, Error> {
        let post = matches!(
            kind,
            Kind::SendJsonPost | Kind::ComputePost | Kind::SidecarPost
        );
        let t0 = Instant::now();
        self.no_owner_in_session(path)?;
        if self.cell_socket().is_some() {
            // A POST goes on to TCP only after a failed connect (curl 7) or
            // "not served on cell socket", as `cli_send`'s.
            match self.run(kind, Channel::Uds, path, &[], payload.as_bytes(), t0) {
                Ok(r) if contains(&r.body, NOT_SERVED) => {}
                Ok(r) if r.status != 0 => return Ok(r),
                Ok(_) if post => return Err(maybe_applied(path, t0)),
                Ok(_) => {}
                Err(f) if post && f.error.curl_code() != wire::curl::COULDNT_CONNECT => {
                    return Err(maybe_applied(path, t0))
                }
                Err(_) => {}
            }
        }
        self.run(kind, Channel::Tcp, path, &[], payload.as_bytes(), t0)
            .map_err(|f| f.error)
    }

    /// `_k2_sock_fallback_token`'s refusal: a session (`HOOK_SOCK` set)
    /// with no passport, on this machine, whose TCP request would carry
    /// the owner token from disk. With a passport there is no owner token
    /// here at all ([`Auth`]), so the TCP request carries the passport, as
    /// the shell's does since 0.46.17.
    fn no_owner_in_session(&self, endpoint: &str) -> Result<(), Error> {
        if self.session
            && !self.force_tcp
            && self.owner_from_disk
            && matches!(self.auth, Auth::Owner(_))
        {
            return Err(Error::NoOwnerInSession {
                endpoint: endpoint.to_string(),
            });
        }
        Ok(())
    }

    /// The cell socket, when there is a passport, the socket is a socket
    /// (`[ -S ]`) and no remote host forces TCP.
    pub(super) fn cell_socket(&self) -> Option<&Path> {
        if self.force_tcp {
            return None;
        }
        match &self.auth {
            Auth::Passport {
                sock: Some(sock), ..
            } if is_socket(sock) => Some(sock),
            _ => None,
        }
    }

    /// The cell socket that a request carrying the session's passport goes
    /// over first in an identity session (Agent Isolation M1): with
    /// [`Client::passport_on_socket`], a passport and its socket a socket
    /// (`[ -S ]`), as the shell's `_k2_via_session_sock` and
    /// `_k2_py_session_sock_lines` read them. `None` outside an identity
    /// session. [`Client::force_tcp`] doesn't count: a call that keeps off
    /// the cell socket's own routes (a `tcp` compute call, `focus-group
    /// list`, the publish PIN) still carries the passport to this
    /// machine's daemon, and a remote host's call carries its own token
    /// to its own address, never the passport ([`super::super::remote::apply`]).
    pub fn session_socket(&self) -> Option<&Path> {
        if !self.passport_on_socket {
            return None;
        }
        match &self.auth {
            Auth::Passport {
                sock: Some(sock), ..
            } if is_socket(sock) => Some(sock),
            _ => None,
        }
    }

    /// `_k2_via_session_sock` for one curl request: the cell socket and
    /// the request with [`VIA_TCP_HEADER`] added, when
    /// [`Client::session_socket`] has one, the call is a daemon call (not
    /// `_K2_CURL_NOT_DAEMON`), it goes to plain http on `127.0.0.1`,
    /// `localhost` or `[::1]` (the shell's `BASE_URL` spelled that way),
    /// and its curl config holds `Authorization: Bearer <passport>` (a
    /// longer token that starts with the passport holds it too). The mark
    /// is a config line after the credential and a Codex call's
    /// conversation headers, so it goes before the command line's `-H`s.
    /// `cli_post_json_stdin`'s credential is on its `-K /dev/fd/3`, after
    /// the hand-over headers: the mark follows it there.
    #[cfg(unix)]
    pub(super) fn session_route(
        &self,
        kind: Kind,
        target: &Target,
        request: &Request,
    ) -> Option<(Target, Request)> {
        if matches!(kind, Kind::ControlPlane { .. }) {
            return None;
        }
        let Target::Tcp { host, .. } = target else {
            return None;
        };
        if !matches!(host.as_str(), "127.0.0.1" | "localhost" | "[::1]") {
            return None;
        }
        let sock = self.session_socket()?;
        let passport = self.passport()?.as_str();
        let at = request.headers.iter().position(|(n, v)| {
            n == "Authorization"
                && v.strip_prefix("Bearer ")
                    .is_some_and(|t| t.starts_with(passport))
        })?;
        let conversation = request.headers[at + 1..]
            .iter()
            .zip(&self.conversation)
            .take_while(|(a, b)| a == b)
            .count();
        let mut marked = request.clone();
        marked
            .headers
            .insert(at + 1 + conversation, (VIA_TCP_HEADER.into(), "1".into()));
        Some((Target::Unix(sock.to_path_buf()), marked))
    }

    /// No Unix sockets: every request goes as it is.
    #[cfg(not(unix))]
    pub(super) fn session_route(
        &self,
        _kind: Kind,
        _target: &Target,
        _request: &Request,
    ) -> Option<(Target, Request)> {
        None
    }
}

#[cfg(unix)]
pub(super) fn uds_target(sock: &Path, _endpoint: &str) -> Result<Target, Error> {
    Ok(Target::Unix(sock.to_path_buf()))
}

#[cfg(not(unix))]
pub(super) fn uds_target(_sock: &Path, endpoint: &str) -> Result<Target, Error> {
    Err(Error::NoCellSocket {
        endpoint: endpoint.to_string(),
    })
}

/// `[ -S path ]`: a socket, through any links.
#[cfg(unix)]
pub fn is_socket(path: &Path) -> bool {
    use std::os::unix::fs::FileTypeExt;
    std::fs::metadata(path).is_ok_and(|m| m.file_type().is_socket())
}

/// `[ -S path ]`: never, where there are no Unix sockets.
#[cfg(not(unix))]
pub fn is_socket(_path: &Path) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `send_trying` checks the owner rule for any route, as
    /// `_K2_UDS_TRY=1 cli_send` makes every route socket-first; `send`
    /// only for the routes the socket serves.
    #[test]
    fn send_trying_is_socket_first_for_any_route() {
        let mut c = Client::new("127.0.0.1", 1, "/p", Auth::Owner("o".into()));
        c.owner_from_disk = true;
        c.session = true;
        c.pause = Duration::ZERO;
        assert!(matches!(
            c.send_trying("/cli/projects/list", Call::Request(&[])),
            Err(Error::NoOwnerInSession { .. })
        ));
        assert!(!matches!(
            c.send("/cli/projects/list", Call::Request(&[])),
            Err(Error::NoOwnerInSession { .. })
        ));
    }

    /// A session with no passport (`HOOK_SOCK` set) and the owner token
    /// from disk: a socket-first call sends nothing and ends with 1
    /// (`_k2_session_tcp_token`). A remote host, or an owner token that
    /// did not come from disk, goes on to TCP as before (port 1: nothing
    /// listens, so that call fails to connect).
    #[test]
    fn a_session_with_no_passport_never_sends_the_owner_token() {
        let session_owner = || {
            let mut c = Client::new("127.0.0.1", 1, "/p", Auth::Owner("o".into()));
            c.owner_from_disk = true;
            c.session = true;
            c.pause = Duration::ZERO;
            c
        };
        let mut c = session_owner();
        let refused = |r: Result<Response, Error>| matches!(r, Err(Error::NoOwnerInSession { .. }));
        assert!(refused(c.send_json(false, "/cli/x", "")));
        assert!(refused(c.send_json(true, "/cli/x", "{}")));
        assert!(refused(c.sidecar_get("/cli/x", "")));
        assert!(refused(c.send("/cli/inbox/list", Call::Request(&[]))));
        let stats = |c: &mut Client, tcp| c.compute_send(false, "/cli/compute/stats", "", tcp);
        assert!(refused(stats(&mut c, false)));
        assert!(refused(c.compute_send(true, "/cli/compute/x", "{}", false)));
        // `_compute_send ... tcp` chose TCP: `$TOKEN`, as the shell's.
        assert!(!refused(stats(&mut c, true)));
        let e = Error::NoOwnerInSession {
            endpoint: "/cli/x".into(),
        };
        assert_eq!(e.to_string(), NO_OWNER_IN_SESSION);
        assert_eq!(e.curl_code(), 1);
        // A request function that is not socket-first keeps its rule.
        assert!(!refused(c.call(Channel::Tcp, "/cli/x", Call::Request(&[]))));
        let mut remote = session_owner();
        remote.force_tcp = true;
        assert!(!refused(remote.send_json(false, "/cli/x", "")));
        let mut typed = session_owner();
        typed.owner_from_disk = false;
        assert!(!refused(typed.send_json(false, "/cli/x", "")));
        let mut no_session = session_owner();
        no_session.session = false;
        assert!(!refused(no_session.send_json(false, "/cli/x", "")));
        // The line is the shell's.
        let policy = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../cli/src/cli-tools-policy.sh"),
        )
        .unwrap();
        let shell = NO_OWNER_IN_SESSION.replacen(k2_client::product_name!(), "K2", 1);
        assert!(
            policy.contains(&format!("_K2_NO_OWNER_WRITE_LINE=\"{shell}\"\n")),
            "_K2_NO_OWNER_WRITE_LINE moved"
        );
    }

    /// `_vault_sock_call`: the vault socket, no credential of any kind,
    /// the hand-over headers then `X-K2-Tty`, and a POST's JSON type
    /// last; the route under `/cli/vault/` with its query as given.
    #[cfg(unix)]
    #[test]
    fn the_vault_socket_carries_no_credential() {
        let mut c = Client::new(
            "127.0.0.1",
            9,
            "/p",
            Auth::Passport {
                passport: Passport::new("s.t").unwrap(),
                sock: Some(PathBuf::from("/tmp/cell.sock")),
            },
        );
        c.vault_sock = Some((PathBuf::from("/tmp/vault.sock"), true));
        let (t, r) = c
            .build(
                Kind::VaultSockGet,
                Channel::Uds,
                "/cli/vault/show?name=A",
                &[],
                &[],
            )
            .unwrap();
        assert_eq!(t, Target::Unix(PathBuf::from("/tmp/vault.sock")));
        assert_eq!(
            (r.method, r.target.as_str(), r.host.as_str()),
            (Method::Get, "/cli/vault/show?name=A", "localhost")
        );
        assert!(r.body.is_none());
        let heads: Vec<(&str, &str)> = r
            .headers
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str()))
            .collect();
        assert_eq!(
            heads,
            [
                (VERSION_HEADER, c.version.as_str()),
                (VIA_HEADER, VIA_NATIVE),
                (TTY_HEADER, "1")
            ]
        );
        c.vault_sock = Some((PathBuf::from("/tmp/vault.sock"), false));
        let (_, r) = c
            .build(
                Kind::VaultSockPost,
                Channel::Uds,
                "/cli/vault/reveal",
                &[],
                b"{}",
            )
            .unwrap();
        let names: Vec<&str> = r.headers.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            [VERSION_HEADER, VIA_HEADER, TTY_HEADER, "Content-Type"]
        );
        assert_eq!(r.headers[2].1, "0");
        assert_eq!(r.method, Method::Post);
        assert_eq!(
            r.body.as_ref().map(|b| b.bytes.as_slice()),
            Some(&b"{}"[..])
        );
        // The owner from disk: the shell's hand-over header, still no token.
        let mut o = Client::new("127.0.0.1", 9, "/p", Auth::Owner("o".into()));
        o.owner_from_disk = true;
        o.vault_sock = Some((PathBuf::from("/tmp/vault.sock"), false));
        let (_, r) = o
            .build(
                Kind::VaultSockGet,
                Channel::Uds,
                "/cli/vault/status",
                &[],
                &[],
            )
            .unwrap();
        let names: Vec<&str> = r.headers.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            [VERSION_HEADER, VIA_HEADER, OWNER_SOURCE_HEADER, TTY_HEADER]
        );
        // No socket named: nothing to build.
        o.vault_sock = None;
        assert!(o
            .build(
                Kind::VaultSockGet,
                Channel::Uds,
                "/cli/vault/status",
                &[],
                &[]
            )
            .is_err());
    }

    /// `codex_headers_go_where_k2_curl_puts_them` (client.rs) on the cell
    /// socket and the vault socket: Unix only (no Unix sockets elsewhere,
    /// `uds_target`).
    #[cfg(unix)]
    #[test]
    fn codex_headers_on_the_cell_and_vault_sockets() {
        let conv = vec![
            (CONVERSATION_HEADER.to_string(), "root-1".to_string()),
            (THREAD_HEADER.to_string(), "thread-1".to_string()),
        ];
        let names =
            |r: &Request| -> Vec<String> { r.headers.iter().map(|(n, _)| n.clone()).collect() };
        // The cell socket: after the passport.
        let mut pass = Client::new(
            "127.0.0.1",
            9,
            "/p",
            Auth::Passport {
                passport: Passport::new("s.t").unwrap(),
                sock: Some(PathBuf::from("/tmp/cell.sock")),
            },
        );
        pass.conversation = conv.clone();
        for kind in [Kind::Request, Kind::SendJsonPost] {
            let (_, r) = pass
                .build(kind, Channel::Uds, "/cli/x", &[], b"{}")
                .unwrap();
            let n = names(&r);
            assert_eq!(
                &n[..5],
                [
                    "Authorization",
                    CONVERSATION_HEADER,
                    THREAD_HEADER,
                    VERSION_HEADER,
                    VIA_HEADER
                ],
                "{kind:?}"
            );
        }
        // The vault socket: GET first; POST (stdin) after the hand-over ones.
        pass.vault_sock = Some((PathBuf::from("/tmp/vault.sock"), true));
        let (_, r) = pass
            .build(
                Kind::VaultSockGet,
                Channel::Uds,
                "/cli/vault/status",
                &[],
                &[],
            )
            .unwrap();
        assert_eq!(
            names(&r),
            [
                CONVERSATION_HEADER,
                THREAD_HEADER,
                VERSION_HEADER,
                VIA_HEADER,
                TTY_HEADER
            ]
        );
        let (_, r) = pass
            .build(
                Kind::VaultSockPost,
                Channel::Uds,
                "/cli/vault/reveal",
                &[],
                b"{}",
            )
            .unwrap();
        assert_eq!(
            names(&r),
            [
                VERSION_HEADER,
                VIA_HEADER,
                CONVERSATION_HEADER,
                THREAD_HEADER,
                TTY_HEADER,
                "Content-Type"
            ]
        );
    }
}

/// Agent Isolation M1 (`_k2_via_session_sock`): in an identity session a
/// request that carries the session's passport goes over the cell socket,
/// marked, and falls back to TCP only when the socket takes nothing.
#[cfg(all(test, unix))]
mod session_tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::os::unix::net::UnixListener;

    const PASSPORT: &str = "sess91.passportfe12";

    /// A short folder for sockets (a Unix socket path has a small limit).
    fn sock_dir() -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = PathBuf::from(format!(
            "/tmp/k5ps-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).unwrap();
        }
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The head read from `c`, answered with `{}`.
    fn answer_once(mut c: impl Read + Write) -> String {
        let mut got = Vec::new();
        let mut buf = [0u8; 4096];
        while !got.windows(4).any(|w| w == b"\r\n\r\n") {
            match c.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => got.extend_from_slice(&buf[..n]),
                Err(e) => panic!("read: {e}"),
            }
        }
        c.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
            .unwrap();
        String::from_utf8(got).unwrap()
    }

    fn identity_client(port: u16, sock: &Path) -> Client {
        let mut c = Client::new(
            "127.0.0.1",
            port,
            "/p",
            Auth::Passport {
                passport: Passport::new(PASSPORT).unwrap(),
                sock: Some(sock.to_path_buf()),
            },
        );
        c.passport_on_socket = true;
        c.pause = Duration::ZERO;
        c
    }

    /// The custom header names of a head, in order (after `Accept`).
    fn names(head: &str) -> Vec<String> {
        head.split("\r\n")
            .skip(4)
            .take_while(|l| !l.is_empty())
            .map(|l| l.split(':').next().unwrap_or_default().to_string())
            .collect()
    }

    fn nothing_accepted(l: &TcpListener) {
        std::thread::sleep(Duration::from_millis(100));
        match l.accept() {
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Ok((_, from)) => panic!("a TCP connection from {from}"),
            Err(e) => panic!("accept: {e}"),
        }
    }

    /// The TCP request as it is, over the cell socket, with the mark after
    /// the credential and a Codex call's conversation headers; nothing
    /// reaches TCP.
    #[test]
    fn an_identity_session_sends_its_passport_over_the_cell_socket() {
        let dir = sock_dir();
        let path = dir.join("c.sock");
        let uds = UnixListener::bind(&path).unwrap();
        let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
        tcp.set_nonblocking(true).unwrap();
        let port = tcp.local_addr().unwrap().port();
        let served = std::thread::spawn(move || answer_once(uds.accept().unwrap().0));
        let mut c = identity_client(port, &path);
        c.conversation = vec![
            (CONVERSATION_HEADER.to_string(), "root-1".to_string()),
            (THREAD_HEADER.to_string(), "thread-1".to_string()),
        ];
        let r = c
            .call(Channel::Tcp, "/cli/x", Call::Request(&["a=1"]))
            .unwrap();
        assert_eq!((r.status, r.body.as_slice()), (200, &b"{}"[..]));
        let head = served.join().unwrap();
        assert!(head.starts_with("GET /cli/x?project="), "{head}");
        assert!(
            head.contains(&format!("\r\nHost: 127.0.0.1:{port}\r\n")),
            "{head}"
        );
        assert!(
            head.contains(&format!("\r\nAuthorization: Bearer {PASSPORT}\r\n")),
            "{head}"
        );
        assert!(head.contains("\r\nX-K2-Via-Tcp: 1\r\n"), "{head}");
        assert_eq!(
            &names(&head)[..5],
            [
                "Authorization",
                CONVERSATION_HEADER,
                THREAD_HEADER,
                VIA_TCP_HEADER,
                VERSION_HEADER
            ]
        );
        assert_eq!(c.refused_before_answer(), 0);
        nothing_accepted(&tcp);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Outside an identity session the call goes over TCP as before, with
    /// no mark, and the cell socket gets nothing.
    #[test]
    fn outside_an_identity_session_the_passport_stays_on_tcp() {
        let dir = sock_dir();
        let path = dir.join("c.sock");
        let uds = UnixListener::bind(&path).unwrap();
        uds.set_nonblocking(true).unwrap();
        let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = tcp.local_addr().unwrap().port();
        let served = std::thread::spawn(move || answer_once(tcp.accept().unwrap().0));
        let mut c = identity_client(port, &path);
        c.passport_on_socket = false;
        let r = c
            .call(Channel::Tcp, "/cli/x", Call::Request(&[]))
            .unwrap();
        assert_eq!(r.status, 200);
        let head = served.join().unwrap();
        assert!(!head.contains("X-K2-Via-Tcp"), "{head}");
        assert!(
            head.contains(&format!("\r\nAuthorization: Bearer {PASSPORT}\r\n")),
            "{head}"
        );
        match uds.accept() {
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Ok(_) => panic!("the cell socket got a connection"),
            Err(e) => panic!("accept: {e}"),
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A socket that takes nothing (its server gone, the file left): the
    /// same call over TCP at once, unmarked, counted as one refused try.
    #[test]
    fn a_socket_that_takes_nothing_falls_back_to_tcp_unmarked() {
        let dir = sock_dir();
        let path = dir.join("c.sock");
        drop(UnixListener::bind(&path).unwrap());
        assert!(is_socket(&path));
        let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = tcp.local_addr().unwrap().port();
        let served = std::thread::spawn(move || answer_once(tcp.accept().unwrap().0));
        let mut c = identity_client(port, &path);
        let r = c
            .call(Channel::Tcp, "/cli/x", Call::PostJson(b"{}"))
            .unwrap();
        assert_eq!(r.status, 200);
        let head = served.join().unwrap();
        assert!(head.starts_with("POST /cli/x HTTP/1.1\r\n"), "{head}");
        assert!(!head.contains("X-K2-Via-Tcp"), "{head}");
        assert_eq!(c.refused_before_answer(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The shell's conditions, each on its own: which requests go over
    /// the socket.
    #[test]
    fn which_requests_go_over_the_cell_socket() {
        let dir = sock_dir();
        let path = dir.join("c.sock");
        let _live = UnixListener::bind(&path).unwrap();
        let file = dir.join("not-a-socket");
        std::fs::write(&file, "").unwrap();
        let routed = |c: &Client, kind: Kind| -> bool {
            let (target, request) = c.build(kind, Channel::Tcp, "/cli/x", &[], b"{}").unwrap();
            c.session_route(kind, &target, &request).is_some()
        };
        let base = identity_client(4100, &path);
        assert!(routed(&base, Kind::Request));
        assert!(routed(&base, Kind::SendJsonPost));
        assert!(routed(&base, Kind::RequestPost));
        // `cli_post_json_stdin`'s URL and credential are on fd 3: the mark
        // right after its late credential, before its Content-Type.
        assert!(routed(&base, Kind::PostJsonStdin));
        let (target, request) = base
            .build(Kind::PostJsonStdin, Channel::Tcp, "/cli/x", &[], b"{}")
            .unwrap();
        let (_, marked) = base
            .session_route(Kind::PostJsonStdin, &target, &request)
            .unwrap();
        let names: Vec<&str> = marked.headers.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            [VERSION_HEADER, VIA_HEADER, "Authorization", VIA_TCP_HEADER, "Content-Type"]
        );
        // `_K2_CURL_NOT_DAEMON`.
        assert!(!routed(&base, Kind::ControlPlane { method: Method::Get }));
        // The host as `BASE_URL` spells it.
        for (host, want) in [
            ("localhost", true),
            ("[::1]", true),
            ("::1", false),
            ("LOCALHOST", false),
            ("10.0.0.2", false),
            ("box.example.com", false),
        ] {
            let mut c = identity_client(4100, &path);
            c.host = host.to_string();
            assert_eq!(routed(&c, Kind::Request), want, "{host}");
        }
        let mut c = identity_client(4100, &path);
        c.passport_on_socket = false;
        assert!(!routed(&c, Kind::Request));
        // A call kept off the cell socket's own routes still carries the
        // passport here: it goes over the socket, marked.
        let mut c = identity_client(4100, &path);
        c.force_tcp = true;
        assert!(routed(&c, Kind::Request));
        assert!(routed(&c, Kind::ComputeGet));
        // A remote host: its own token, never the passport.
        crate::transport::remote::apply(
            &mut c,
            &crate::transport::remote::base("http://127.0.0.1:4200").unwrap(),
            "remote-tok".into(),
        );
        assert!(!routed(&c, Kind::Request));
        let c = identity_client(4100, &file);
        assert!(!routed(&c, Kind::Request));
        let mut c = identity_client(4100, &path);
        c.auth = Auth::Owner(PASSPORT.into());
        assert!(!routed(&c, Kind::Request));
        // A caller's own credential: only the passport.
        let own = Kind::Own {
            post: false,
            note: false,
        };
        let mut c = identity_client(4100, &path);
        c.own = Some((Some("someone-else".into()), MAX_TIME));
        assert!(!routed(&c, own));
        c.own = Some((Some(PASSPORT.into()), MAX_TIME));
        assert!(routed(&c, own));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
