//! The proxy and tunnel paths: the Connect calls (the daemon's
//! Connect proxy route and the control plane, with the tunnel token),
//! and the proof that the person's proxy rides only a remote `https://`
//! base. The proxy itself is [`crate::transport::proxy`]; the TLS
//! target that carries it is built in the client core.

use super::*;

impl Client {
    /// `_connect_curl_daemon <method> …`'s requests ([`Kind::ConnectProxy`]):
    /// `req` (the `{"method", "endpoint"[, "body"]}` JSON) POSTed to
    /// [`CONNECT_PROXY_ENDPOINT`]. On the cell socket first when there is
    /// a passport with a live socket (any route, as the shell's `$TOKEN =
    /// $SCOPED_TOKEN` test); its answer is the answer, whatever the
    /// status, unless it holds "not served on cell socket". Then over TCP.
    /// A request that is not a `get` goes on only when the connect failed
    /// (curl 7): any other failure may have reached the daemon, which may
    /// have made the call, so it is [`Error::MaybeApplied`] and never sent
    /// again. A TCP failure comes back as it is (curl 7: no daemon).
    /// [`Error::NoCredential`]: no token for TCP (the shell's empty
    /// `$TOKEN`, no daemon either).
    pub fn connect_proxy(&mut self, get: bool, req: &[u8]) -> Result<Response, Error> {
        let t0 = Instant::now();
        let ep = CONNECT_PROXY_ENDPOINT;
        let sent_once = |e: &Error| !get && e.curl_code() != wire::curl::COULDNT_CONNECT;
        if self.cell_socket().is_some() {
            match self.run(Kind::ConnectProxy, Channel::Uds, ep, &[], req, t0) {
                Ok(r) if contains(&r.body, NOT_SERVED) => {}
                Ok(r) => return Ok(r),
                Err(f) if sent_once(&f.error) => return Err(maybe_applied(ep, t0)),
                Err(_) => {}
            }
        }
        if self.tcp_token().is_none() {
            return Err(Error::NoCredential {
                endpoint: ep.to_string(),
            });
        }
        match self.run(Kind::ConnectProxy, Channel::Tcp, ep, &[], req, t0) {
            Ok(r) => Ok(r),
            Err(f) if sent_once(&f.error) => Err(maybe_applied(ep, t0)),
            Err(f) => Err(f.error),
        }
    }

    /// `_connect_curl`'s own call to the Connect control plane
    /// ([`Kind::ControlPlane`]): `path` as it is over TCP, or TLS when the
    /// client was pointed at an `https://` base ([`super::remote::apply`]),
    /// with this client's token (the tunnel token) as the `Authorization`
    /// header and nowhere else. `put`: the repoint, else a POST. The body
    /// comes back whatever the status.
    pub fn control_plane(&mut self, put: bool, path: &str, body: &[u8]) -> Result<Response, Error> {
        let method = if put { Method::Put } else { Method::Post };
        self.control_plane_as(method, path, body)
    }

    /// [`Client::control_plane`] with any method: `_connect_curl GET`
    /// (the list), `POST`, `PUT` or `DELETE` (the remove). A body that is
    /// empty is no body (no `Content-Type` either), as the shell's.
    pub fn control_plane_as(
        &mut self,
        method: Method,
        path: &str,
        body: &[u8],
    ) -> Result<Response, Error> {
        let kind = Kind::ControlPlane { method };
        self.run(kind, Channel::Tcp, path, &[], body, Instant::now())
            .map_err(|f| f.error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every proxy variable curl reads, pointed at a proxy.
    const PROXY_VARS: [&str; 8] = [
        "http_proxy",
        "HTTP_PROXY",
        "https_proxy",
        "HTTPS_PROXY",
        "all_proxy",
        "ALL_PROXY",
        "no_proxy",
        "NO_PROXY",
    ];

    /// The daemon's paths never read a proxy setting. The proof: with
    /// every proxy variable set in this process, every kind of daemon
    /// call builds a target with no place for a proxy (TCP or the cell
    /// socket), on loopback and on a LAN host,
    /// with the owner token and with a passport. Only a client that
    /// `remote::apply` pointed at a remote `https://` base carries one.
    #[cfg(unix)]
    #[test]
    fn daemon_targets_never_carry_a_proxy() {
        for n in PROXY_VARS {
            let v = if n.ends_with("no_proxy") || n.ends_with("NO_PROXY") {
                "nothing.example"
            } else {
                "http://user:pw@proxy.example:3128"
            };
            std::env::set_var(n, v);
        }
        let kinds = [
            Kind::Request,
            Kind::RequestPost,
            Kind::PostForm,
            Kind::PostJson,
            Kind::PostJsonStdin,
            Kind::SendJsonGet,
            Kind::SendJsonPost,
            Kind::Public,
            Kind::Probe,
            Kind::BearerForm,
            Kind::SidecarGet,
            Kind::ComputeGet,
            Kind::ComputePost,
            Kind::SidecarPost,
            Kind::PassportProject,
            Kind::Cards,
            Kind::FeedbackOptin,
            Kind::UpdateVersion,
            Kind::SetupGet,
            Kind::SetupPost,
            Kind::QueryPost,
            Kind::Own {
                post: false,
                note: false,
            },
            Kind::Own {
                post: true,
                note: true,
            },
            Kind::BootStatus,
            Kind::SendOnce,
            Kind::ConnectProxy,
            Kind::ControlPlane {
                method: Method::Get,
            },
            Kind::VaultSockGet,
            Kind::VaultSockPost,
        ];
        let passport = || Auth::Passport {
            passport: Passport::new("s.x").unwrap(),
            sock: Some(PathBuf::from("/tmp/k5-noproxy.sock")),
        };
        for host in ["127.0.0.1", "localhost", "10.0.0.5", "box.example"] {
            for auth in [Auth::Owner("o".into()), passport()] {
                let mut c = Client::new(host, 4000, "/p", auth);
                c.own = Some((Some("o".into()), Duration::from_secs(5)));
                c.vault_sock = Some((PathBuf::from("/tmp/k5-noproxy-vault.sock"), false));
                assert!(c.via.is_none(), "a new client has no proxy");
                for kind in kinds {
                    for channel in [Channel::Tcp, Channel::Uds] {
                        let Ok((target, _)) = c.build(kind, channel, "/cli/x", &[], b"{}") else {
                            continue;
                        };
                        assert!(
                            matches!(target, Target::Tcp { .. } | Target::Unix(_)),
                            "{host} {kind:?} {channel:?}: {target:?}"
                        );
                    }
                }
            }
        }
        // A remote http:// base (a LAN K2 over plain http) has none either.
        let mut c = Client::new("127.0.0.1", 4000, "", Auth::None);
        let b = crate::transport::remote::routed("http://127.0.0.1:4100").unwrap();
        crate::transport::remote::apply(&mut c, &b, "t".into());
        let (target, _) = c
            .build(Kind::Request, Channel::Tcp, "/cli/x", &[], &[])
            .unwrap();
        assert!(matches!(target, Target::Tcp { .. }), "{target:?}");
        // A remote https:// base does: the one place it rides.
        let mut c = Client::new("127.0.0.1", 4000, "", Auth::None);
        let b = crate::transport::remote::routed("box.example").unwrap();
        crate::transport::remote::apply(&mut c, &b, "t".into());
        let (target, r) = c
            .build(Kind::Request, Channel::Tcp, "/cli/x", &[], &[])
            .unwrap();
        let Target::Tls { via: Some(t), .. } = target else {
            panic!("no proxy on a remote https base: {target:?}");
        };
        assert_eq!(
            (t.proxy.host.as_str(), t.proxy.port),
            ("proxy.example", 3128)
        );
        assert_eq!(t.user_agent, r.user_agent);
        for n in PROXY_VARS {
            std::env::remove_var(n);
        }
    }

    /// The proxy settings are read from the process environment in ONE
    /// place, the remote base ([`crate::transport::remote::routed`]). Every
    /// other file of the binary is checked for the reader's name and for
    /// a proxy variable's.
    #[test]
    fn only_the_remote_base_reads_proxy_settings() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        let mut dirs = vec![src.clone()];
        while let Some(d) = dirs.pop() {
            for e in std::fs::read_dir(&d).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    dirs.push(p);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    files.push(p);
                }
            }
        }
        assert!(
            files.len() > 100,
            "the walk found the sources: {}",
            files.len()
        );
        let allowed = [
            src.join("transport/proxy.rs"),
            src.join("transport/proxy/tests.rs"),
            src.join("transport/remote.rs"),
            src.join("transport/client/tunnel.rs"),
        ];
        // curl's reading, and the shell's Python's (the urllib families'
        // https daemon).
        let readers = [
            ["from_", "process_env"].concat(),
            ["python_", "environ"].concat(),
        ];
        let vars = PROXY_VARS.map(|v| format!("\"{v}\""));
        let mut found = Vec::new();
        for f in &files {
            let text = std::fs::read_to_string(f).unwrap();
            let rel = f.strip_prefix(&src).unwrap().display().to_string();
            for reader in &readers {
                if text.contains(reader.as_str()) && !allowed[..3].contains(f) {
                    found.push(format!("{rel}: names the proxy reader {reader}"));
                }
            }
            for v in &vars {
                if text.contains(v.as_str()) && !allowed.contains(f) {
                    found.push(format!("{rel}: names {v}"));
                }
            }
        }
        assert_eq!(found, Vec::<String>::new());
        // And in remote.rs a reader is called only by `process_env` (curl's
        // reading, for `routed`, a remote base) and `python_proxy_env` (the
        // shell's Python's reading, for the urllib families' https daemon
        // and `python_route`, clone's urllib to a host it names); and
        // `process_env` only by `routed`.
        let remote = std::fs::read_to_string(src.join("transport/remote.rs")).unwrap();
        let curl_reader = "process_env";
        let mut callers = Vec::new();
        let mut users = Vec::new();
        let mut within = String::new();
        for line in remote.lines() {
            if let Some(rest) = line.strip_prefix("pub fn ").or(line.strip_prefix("fn ")) {
                within = rest.split('(').next().unwrap_or("").to_string();
            }
            let code = line.trim_start();
            if code.starts_with("//") {
                continue;
            }
            if readers.iter().any(|r| code.contains(r.as_str())) {
                callers.push(within.clone());
            } else if code.contains(curl_reader) && within != curl_reader {
                users.push(within.clone());
            }
        }
        callers.dedup();
        users.dedup();
        assert_eq!(callers, [curl_reader, "python_proxy_env"], "remote.rs");
        assert_eq!(users, ["routed"], "remote.rs");
    }
}
