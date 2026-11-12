use super::*;
use crate::tunnel::test_support::with_temp_home;

/// A real, always-present no-op executable to stand in for `frpc` when a
/// test needs to get PAST frpc resolution (and possibly spawn a harmless
/// child). `/usr/bin/true` on macOS; `/bin/true` on Linux.
fn true_bin() -> PathBuf {
    crate::test_env::true_bin()
}

#[test]
fn stray_frpc_pattern_matches_only_our_config() {
    // The reap must target frpc launched with OUR config path and
    // nothing else — a bare `frpc` pattern would nuke an unrelated
    // tunnel the user runs. Pin the exact `pkill -f` string.
    let pat = stray_frpc_pattern(Path::new("/Users/x/.k2so/frpc.toml"));
    assert_eq!(pat, "frpc -c /Users/x/.k2so/frpc.toml");
    // Must carry the config path (so it can't match an arbitrary frpc).
    assert!(pat.contains("/.k2so/frpc.toml"));
    // Must be scoped by `-c <cfg>`, not a bare process name.
    assert!(pat.starts_with("frpc -c "));
}

#[test]
fn resolve_frpc_explicit_missing_errors_clearly() {
    let err = resolve_frpc(&FrpcBinary::Explicit(PathBuf::from(
        "/definitely/not/here/frpc",
    )))
    .unwrap_err();
    assert!(
        err.contains("frpc not found at configured path"),
        "expected configured-path error, got: {err}"
    );
}

#[test]
fn resolve_frpc_auto_missing_surfaces_install_hint() {
    // Point PATH at an empty dir and HOME at a tempdir so none of the
    // common locations resolve — we must get the install guidance,
    // not a silent success.
    with_temp_home(|| {
        let empty = crate::test_env::unique_temp_path("frpc-empty");
        std::fs::create_dir_all(&empty).expect("mk empty dir");
        // Per-thread lookup PATH: the process PATH is never changed.
        let res = crate::terminal::path_env::with_lookup_path(&empty, || {
            resolve_frpc(&FrpcBinary::Auto)
        });
        let _ = std::fs::remove_dir_all(&empty);
        let err = res.expect_err("frpc must be unresolvable with empty PATH + temp HOME");
        assert!(
            err.contains("frpc not installed"),
            "expected install hint, got: {err}"
        );
        assert!(
            err.contains("fatedier/frp"),
            "install hint should point at the frp project, got: {err}"
        );
    });
}

#[test]
fn resolve_frpc_finds_executable_on_path() {
    with_temp_home(|| {
        let bin_dir = crate::test_env::unique_temp_path("frpc-bin");
        std::fs::create_dir_all(&bin_dir).expect("mk bin dir");
        let fake = bin_dir.join("frpc");
        crate::test_env::write_executable(&fake, "#!/bin/sh\nexit 0\n");
        let res = crate::terminal::path_env::with_lookup_path(&bin_dir, || {
            resolve_frpc(&FrpcBinary::Auto)
        });
        let _ = std::fs::remove_dir_all(&bin_dir);
        assert_eq!(res.expect("should find fake frpc on PATH"), fake);
    });
}

#[test]
fn start_without_token_errors_and_does_not_spawn() {
    with_temp_home(|| {
        // Fresh config (no token).
        let res = start(None, 57839, &FrpcBinary::Explicit(PathBuf::from("/bin/true")));
        let err = res.expect_err("start must refuse without a token");
        assert!(
            err.contains("tunnel not configured"),
            "expected not-configured error, got: {err}"
        );
        assert!(!status().running, "no child should be running after failed start");
    });
}

#[test]
fn start_with_missing_frpc_surfaces_install_error() {
    with_temp_home(|| {
        // Opt out of E2E (e2e:false) so this test exercises the frpc
        // resolution path directly — with E2E on (the default) the start
        // would first try to ensure the HTTPS listener (no daemon hook in
        // this unit context) and surface that error instead.
        config::save(&TunnelConfig {
            token: "tok".to_string(),
            subdomain: "rosson".to_string(),
            e2e: false,
            ..Default::default()
        })
        .expect("seed config");
        let err = start(
            None,
            57839,
            &FrpcBinary::Explicit(PathBuf::from("/no/such/frpc")),
        )
        .expect_err("missing frpc must fail start");
        assert!(err.contains("frpc not found"), "got: {err}");
        assert!(!status().running);
    });
}

/// E2E default-on path: with no daemon ensure-hook registered (a pure
/// k2-core unit context) and no published HTTPS port, a start on an
/// E2E config must FAIL LOUD rather than forwarding cleartext to the
/// HTTP port. Proves the connector consults `ensure_https_port` and
/// propagates its error.
#[test]
fn e2e_start_without_https_listener_fails_loud_not_cleartext() {
    with_temp_home(|| {
        // Default config → e2e is ON. A real frpc path (/bin/true) so we
        // get PAST frpc resolution and reach the HTTPS-port resolution.
        let _e2e = crate::test_env::EnvVar::remove("K2_E2E"); // follow config (default-on)
        config::save(&TunnelConfig {
            token: "tok".to_string(),
            subdomain: "rosson".to_string(),
            ..Default::default() // e2e defaults true
        })
        .expect("seed config");

        let err = start(None, 57839, &FrpcBinary::Explicit(true_bin()))
            .expect_err("E2E start with no HTTPS listener must fail loud");
        assert!(
            err.contains("HTTPS listener") || err.contains("leak plaintext"),
            "expected an HTTPS-listener/anti-cleartext error, got: {err}"
        );
        assert!(!status().running, "no child should be running after the loud failure");
    });
}

/// E2E default-on + a published HTTPS port (simulating the daemon's
/// listener being up): the connector resolves frpc's localPort to the
/// HTTPS port, not the cleartext HTTP `default_local_port`. We force frpc
/// resolution to fail right AFTER port resolution so we don't actually
/// spawn, and assert the rendered config targeted the HTTPS port.
#[test]
fn e2e_start_resolves_https_port_when_published() {
    with_temp_home(|| {
        let _e2e = crate::test_env::EnvVar::remove("K2_E2E");
        config::save(&TunnelConfig {
            token: "tok".to_string(),
            subdomain: "rosson".to_string(),
            ..Default::default()
        })
        .expect("seed config");
        // Simulate the daemon having published its HTTPS listener port.
        super::super::tls::publish_https_port(48217).expect("publish https port");

        // frpc missing → start fails AFTER resolving the HTTPS port, but
        // it must have written frpc.toml targeting the HTTPS port first?
        // No: render happens after frpc resolves. So instead use a real
        // binary so we reach render, then inspect the written config.
        let _ = start(None, 57839, &FrpcBinary::Explicit(true_bin()));
        let toml = std::fs::read_to_string(frpc_config_path())
            .expect("frpc.toml must have been rendered");
        assert!(
            toml.contains("localPort = 48217"),
            "E2E start must target the published HTTPS port, not the HTTP port\n{toml}"
        );
        assert!(
            toml.contains("type = \"https\""),
            "E2E start must render an https proxy\n{toml}"
        );

        let _ = stop("test teardown");
    });
}

/// PRD connect-login-edge-only I3: with E2E OFF the connector resolves
/// frpc's `localPort` to the daemon-registered TUNNEL-INGRESS port, not
/// the cleartext main `default_local_port`; a failing hook is loud.
#[test]
fn e2e_off_start_resolves_tunnel_ingress_port_via_hook() {
    with_temp_home(|| {
        config::save(&TunnelConfig {
            token: "tok".to_string(),
            subdomain: "rosson".to_string(),
            e2e: false,
            ..Default::default()
        })
        .expect("seed config");
        let hook = super::super::ingress::TestIngressHook::install(Box::new(|| Ok(46123)));
        let st = start(None, 57839, &FrpcBinary::Explicit(true_bin()))
            .expect("start with e2e off");
        assert!(st.running);
        let toml = std::fs::read_to_string(frpc_config_path())
            .expect("frpc.toml must have been rendered");
        assert!(
            toml.contains("localPort = 46123"),
            "E2E-off start must target the tunnel-ingress port, not the main port\n{toml}"
        );
        assert!(
            !toml.contains("localPort = 57839"),
            "main loopback port must never be the tunnel target\n{toml}"
        );
        assert!(toml.contains("type = \"http\""), "{toml}");
        let _ = stop("test teardown");

        // A failing hook must abort the start (no fallback to :57839).
        drop(hook);
        let _hook = super::super::ingress::TestIngressHook::install(Box::new(|| {
            Err("tunnel-ingress bind failed".to_string())
        }));
        let err = start(None, 57839, &FrpcBinary::Explicit(true_bin()))
            .expect_err("start must fail when the ingress listener cannot bind");
        assert!(err.contains("tunnel-ingress bind failed"), "{err}");
    });
}

/// Multi-relay failover, end to end through the REAL supervise loop:
/// a two-relay config whose "frpc" (/bin/true) exits instantly — i.e.
/// every dial to the primary "fails" fast — must, after 3 consecutive
/// failures, rewrite frpc.toml to dial the SECOND relay. Everything
/// else in the rendered config (token, subdomain, proxy name) must be
/// untouched by the rotation.
#[test]
fn supervisor_rotates_to_second_relay_after_three_fast_failures() {
    with_temp_home(|| {
        config::save(&TunnelConfig {
            relays: vec![
                RelayEndpoint { host: "10.0.0.1".to_string(), port: 7000 },
                RelayEndpoint { host: "10.0.0.2".to_string(), port: 7000 },
            ],
            // 0.46.3: the old-path switch — 0.46.2 rotation, end to end.
            failover: Some("legacy".to_string()),
            token: "tok".to_string(),
            subdomain: "rosson".to_string(),
            e2e: false, // skip the HTTPS-listener path in this unit context
            ..Default::default()
        })
        .expect("seed two-relay config");

        let st = start(None, 57839, &FrpcBinary::Explicit(true_bin()))
            .expect("start with two relays");
        assert!(st.running);

        // The first render must dial the PRIMARY relay.
        let toml = std::fs::read_to_string(frpc_config_path()).expect("frpc.toml");
        assert!(
            toml.contains("serverAddr = \"10.0.0.1\""),
            "must dial the primary first\n{toml}"
        );

        // /bin/true exits immediately → each supervised run is a fast
        // failure. With SUPERVISE_POLL=1s and backoffs 0.5s/1s, the 3rd
        // failure (and the rewrite to relay #2) lands within a few
        // seconds; poll generously so slow CI can't flake this.
        let deadline = Instant::now() + Duration::from_secs(30);
        let rotated = loop {
            let toml = std::fs::read_to_string(frpc_config_path()).expect("frpc.toml");
            if toml.contains("serverAddr = \"10.0.0.2\"") {
                break toml;
            }
            if Instant::now() >= deadline {
                panic!(
                    "supervisor never rotated to the second relay; frpc.toml:\n{toml}"
                );
            }
            std::thread::sleep(Duration::from_millis(200));
        };

        // Only the server endpoint rotates — token/subdomain/proxy name
        // are byte-identical (the relays are peers).
        assert!(rotated.contains("token = \"tok\""), "{rotated}");
        assert!(rotated.contains("subdomain = \"rosson\""), "{rotated}");
        assert!(rotated.contains("name = \"k2so-rosson\""), "{rotated}");

        stop("test").expect("stop");
        assert!(!status().running);
    });
}

/// Mid-session relay death, end to end through the REAL supervise
/// loop: a fake frpc that LOGS IN successfully and then spews
/// `connect to server error` lines forever WITHOUT EXITING — exactly
/// the frpc-internal-reconnect-loop state a post-login relay death
/// leaves behind, where exit-based failover alone would hang on the
/// dead relay for good. The watchdog must declare each run dead, kill
/// the stuck child, and after 3 consecutive watchdog kills the
/// supervisor must rotate frpc.toml to the SECOND relay.
#[test]
fn watchdog_kills_stuck_midsession_child_and_supervisor_rotates() {
    with_temp_home(|| {
        // 0.46.3: the standby a DNS-mode move goes to must answer.
        let _reach = TestReachability::install(|_| true);
        // 0.46.3 default (DNS) rules: k2.dev says the standby is the
        // live relay, so the 3rd watchdog kill moves there at once.
        super::super::relay_list::reset_state_for_test();
        super::super::relay_list::set_dns_for_test(
            Some(RelayEndpoint { host: "10.0.0.2".to_string(), port: 7000 }),
            Duration::ZERO,
        );
        // Fake frpc: one success line, then an error storm, never exits.
        // The storm rate (10 lines/s) crosses the watchdog threshold
        // (3 in 30s) almost immediately.
        let dir = crate::test_env::unique_temp_path("fake-frpc");
        std::fs::create_dir_all(&dir).expect("mk fake-frpc dir");
        let script = dir.join("frpc-stuck");
        crate::test_env::write_executable(&script, "#!/bin/sh\n\
                 echo '[I] [service.go:299] login to server success, get run id [test]'\n\
                 while true; do\n\
                 \techo '[W] [service.go:132] connect to server error: dial tcp 10.0.0.1:7000: i/o timeout'\n\
                 \tsleep 0.1\n\
                 done\n");

        config::save(&TunnelConfig {
            relays: vec![
                RelayEndpoint { host: "10.0.0.1".to_string(), port: 7000 },
                RelayEndpoint { host: "10.0.0.2".to_string(), port: 7000 },
            ],
            token: "tok".to_string(),
            subdomain: "rosson".to_string(),
            e2e: false, // skip the HTTPS-listener path in this unit context
            ..Default::default()
        })
        .expect("seed two-relay config");

        let st = start(None, 57839, &FrpcBinary::Explicit(script.clone()))
            .expect("start with stuck fake frpc");
        assert!(st.running);
        let toml = std::fs::read_to_string(frpc_config_path()).expect("frpc.toml");
        assert!(
            toml.contains("serverAddr = \"10.0.0.1\""),
            "must dial the primary first\n{toml}"
        );

        // Each cycle: watchdog declares dead within ~1 poll tick (the
        // error storm crosses the threshold in ~0.3s), kill, backoff
        // (0.5s/1s/2s), respawn. Three cycles land well inside the
        // deadline; poll generously so slow CI can't flake this.
        let deadline = Instant::now() + Duration::from_secs(60);
        let rotated = loop {
            let toml = std::fs::read_to_string(frpc_config_path()).expect("frpc.toml");
            if toml.contains("serverAddr = \"10.0.0.2\"") {
                break toml;
            }
            if Instant::now() >= deadline {
                panic!(
                    "watchdog never converted the stuck child into a rotation; \
                         frpc.toml:\n{toml}"
                );
            }
            std::thread::sleep(Duration::from_millis(200));
        };

        // Only the server endpoint rotates — token/subdomain/proxy name
        // are byte-identical (the relays are peers).
        assert!(rotated.contains("token = \"tok\""), "{rotated}");
        assert!(rotated.contains("subdomain = \"rosson\""), "{rotated}");
        assert!(rotated.contains("name = \"k2so-rosson\""), "{rotated}");

        // The tee contract: the stuck child's lines still landed in
        // ~/.k2/frpc.log — the watchdog reads a COPY, not the file.
        let log = std::fs::read_to_string(frpc_log_path()).expect("frpc.log");
        assert!(
            log.contains("login to server success"),
            "frpc.log must still capture child output\n{log}"
        );
        assert!(log.contains("connect to server error"), "{log}");

        stop("test").expect("stop");
        assert!(!status().running);
        let _ = std::fs::remove_dir_all(&dir);
    });
}

/// Single-relay config through the SAME start path: the rendered TOML
/// must dial the legacy `server_addr`/`server_port` pair (relay_list()
/// folds it in), and no rotation can ever occur — there is nowhere to
/// rotate to. Guards the "byte-identical when one relay" contract at
/// the connector level.
#[test]
fn single_relay_start_renders_legacy_endpoint_unchanged() {
    with_temp_home(|| {
        config::save(&TunnelConfig {
            server_addr: "9.9.9.9".to_string(),
            server_port: 7009,
            token: "tok".to_string(),
            subdomain: "rosson".to_string(),
            e2e: false,
            ..Default::default()
        })
        .expect("seed legacy config");

        let _ = start(None, 57839, &FrpcBinary::Explicit(true_bin()));
        let toml = std::fs::read_to_string(frpc_config_path()).expect("frpc.toml");
        assert!(
            toml.contains("serverAddr = \"9.9.9.9\""),
            "legacy endpoint must render unchanged\n{toml}"
        );
        assert!(toml.contains("serverPort = 7009"), "{toml}");
        // Status keeps reporting the legacy endpoint too — the live-
        // relay slot never rotates on a solo config, so the reported
        // server_addr is byte-identical to the pre-failover status.
        assert_eq!(
            status().server_addr.as_deref(),
            Some("9.9.9.9"),
            "single-relay status must report the configured endpoint"
        );
        let _ = stop("test teardown");
    });
}

/// 0.46.6 (PRD test 12): a 0.46.5 `tunnel.json` (the old built-in IP,
/// or the old provisioner list) renders and dials `relay.k2.dev` once
/// that name resolved; status names it and the replaced list.
#[test]
fn a_legacy_tunnel_json_dials_the_relay_name() {
    with_temp_home(|| {
        let _e = crate::test_env::EnvVar::remove(config::RELAY_DEFAULT_ENV);
        super::super::relay_list::set_name_ok_for_test(Some(vec![
            "147.135.65.8".parse().expect("ip"),
        ]));
        for relays in [
            vec![],
            vec![
                RelayEndpoint { host: "178.156.232.105".to_string(), port: 7000 },
                RelayEndpoint { host: "40.160.53.25".to_string(), port: 7000 },
            ],
        ] {
            config::save(&TunnelConfig {
                server_addr: "178.156.232.105".to_string(),
                relays: relays.clone(),
                token: "tok".to_string(),
                subdomain: "rosson".to_string(),
                e2e: false,
                ..Default::default()
            })
            .expect("seed a 0.46.5 tunnel.json");
            let before = std::fs::read(config::config_path()).expect("read");
            let dir = crate::test_env::unique_temp_path("fake-frpc-name");
            let st = start(None, 57839, &FrpcBinary::Explicit(long_lived_frpc(&dir)))
                .expect("start");
            let toml = std::fs::read_to_string(frpc_config_path()).expect("frpc.toml");
            assert!(toml.contains("serverAddr = \"relay.k2.dev\""), "{relays:?}\n{toml}");
            assert!(toml.contains("serverPort = 7000"), "{toml}");
            assert_eq!(st.server_addr.as_deref(), Some("relay.k2.dev"));
            assert_eq!(st.relay_primary, Some(true));
            assert_eq!(st.failover, None, "one name: nothing to fail over to");
            assert_eq!(st.relay_list.relays, vec!["relay.k2.dev:7000"]);
            assert!(st.relay_list.replaced.as_ref().is_some_and(|r| r[0] == "178.156.232.105:7000"));
            stop("test").expect("stop");
            assert_eq!(
                std::fs::read(config::config_path()).expect("read"),
                before,
                "nothing is rewritten on disk"
            );
            let _ = std::fs::remove_dir_all(&dir);
        }
    });
}

/// TunnelStatus reports the LIVE relay, not the configured primary:
/// through the REAL supervise loop, a two-relay config whose "frpc"
/// (/bin/true) fails fast must — after the 3-failure rotation — flip
/// `status().server_addr` to the SECOND relay's host, while the rest
/// of the status (URL, subdomain) is untouched by the rotation.
#[test]
fn status_reports_live_relay_after_failover_rotation() {
    with_temp_home(|| {
        // 0.46.3: the standby a DNS-mode move goes to must answer.
        let _reach = TestReachability::install(|_| true);
        // DNS rules with no k2.dev answer and a zero minimum outage:
        // the move happens at the plain 3-failure threshold.
        super::super::relay_list::reset_state_for_test();
        let _gate = TestDnsMinOutage::install(Duration::ZERO);
        config::save(&TunnelConfig {
            relays: vec![
                RelayEndpoint { host: "10.0.0.1".to_string(), port: 7000 },
                RelayEndpoint { host: "10.0.0.2".to_string(), port: 7000 },
            ],
            token: "tok".to_string(),
            subdomain: "rosson".to_string(),
            e2e: false, // skip the HTTPS-listener path in this unit context
            ..Default::default()
        })
        .expect("seed two-relay config");

        let st = start(None, 57839, &FrpcBinary::Explicit(true_bin()))
            .expect("start with two relays");
        assert!(st.running);
        // Before any rotation the status homes to the PRIMARY.
        assert_eq!(
            st.server_addr.as_deref(),
            Some("10.0.0.1"),
            "fresh start must report the preferred relay"
        );

        // /bin/true exits immediately → 3 fast failures rotate to the
        // second relay within a few seconds (see the frpc.toml twin of
        // this test); poll generously so slow CI can't flake this.
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let now = status();
            if now.server_addr.as_deref() == Some("10.0.0.2") {
                // Only the relay changed — the public identity of the
                // tunnel is untouched by a rotation.
                assert!(now.running, "rotation must not read as a stop");
                assert_eq!(now.public_url.as_deref(), Some("https://rosson.k2.dev"));
                assert_eq!(now.subdomain.as_deref(), Some("rosson"));
                break;
            }
            if Instant::now() >= deadline {
                panic!(
                    "status never reported the rotated relay; still {:?}",
                    now.server_addr
                );
            }
            std::thread::sleep(Duration::from_millis(200));
        }

        stop("test").expect("stop");
        assert!(!status().running);
    });
}

// ── 0.46.3 DNS-aware failover, end to end through the supervisor ──

fn ten(last: u8) -> RelayEndpoint {
    RelayEndpoint { host: format!("10.0.0.{last}"), port: 7000 }
}

fn seed_two_relays() {
    config::save(&TunnelConfig {
        relays: vec![ten(1), ten(2)],
        token: "tok".to_string(),
        subdomain: "rosson".to_string(),
        e2e: false, // skip the HTTPS-listener path in this unit context
        ..Default::default()
    })
    .expect("seed two-relay config");
}

fn failover_state() -> Option<(String, String, String)> {
    status().failover.map(|f| (f.mode, f.state, f.detail))
}

fn dialing(host: &str) -> bool {
    std::fs::read_to_string(frpc_config_path())
        .map(|t| t.contains(&format!("serverAddr = \"{host}\"")))
        .unwrap_or(false)
}

#[test]
fn dns_supervisor_stays_on_the_primary_while_k2dev_says_the_standby_is_inactive() {
    with_temp_home(|| {
        // 0.46.3: the standby a DNS-mode move goes to must answer.
        let _reach = TestReachability::install(|_| true);
        super::super::relay_list::reset_state_for_test();
        super::super::relay_list::set_dns_for_test(Some(ten(1)), Duration::ZERO);
        seed_two_relays();
        let st = start(None, 57839, &FrpcBinary::Explicit(true_bin())).expect("start");
        assert!(st.running);
        let f = st.failover.expect("two relays report failover");
        assert_eq!((f.mode.as_str(), f.state.as_str()), ("dns", "ready"), "{f:?}");

        // /bin/true fails every dial fast: past the threshold the
        // supervisor must hold on the primary, not rotate.
        wait_until("the DNS hold", 30, || {
            failover_state().is_some_and(|(_, s, _)| s == "waiting_for_dns")
        });
        let (_, _, detail) = failover_state().expect("status");
        assert!(detail.contains("standby inactive per k2.dev"), "{detail}");
        std::thread::sleep(Duration::from_secs(5));
        assert!(dialing("10.0.0.1"), "must keep dialing the primary");
        assert_eq!(status().server_addr.as_deref(), Some("10.0.0.1"));
        assert_eq!(status().relay_primary, Some(true));

        // The DNS watcher flips the name: the next failure follows it.
        super::super::relay_list::set_dns_for_test(Some(ten(2)), Duration::ZERO);
        wait_until("the move to the now-active standby", 60, || dialing("10.0.0.2"));
        wait_until("status to report the standby", 10, || {
            status().server_addr.as_deref() == Some("10.0.0.2")
        });
        assert_eq!(status().relay_primary, Some(false));
        assert_eq!(failover_state().expect("status").1, "on_fallback");
        stop("test").expect("stop");
        super::super::relay_list::reset_state_for_test();
    });
}

#[test]
fn dns_supervisor_with_k2dev_silent_waits_the_minimum_outage_then_moves() {
    with_temp_home(|| {
        // 0.46.3: the standby a DNS-mode move goes to must answer.
        let _reach = TestReachability::install(|_| true);
        super::super::relay_list::reset_state_for_test(); // no answer: Unknown
        let _gate = TestDnsMinOutage::install(Duration::from_secs(6));
        seed_two_relays();
        let started = Instant::now();
        start(None, 57839, &FrpcBinary::Explicit(true_bin())).expect("start");
        wait_until("the DNS hold", 30, || {
            failover_state().is_some_and(|(_, s, _)| s == "waiting_for_dns")
        });
        let (_, _, detail) = failover_state().expect("status");
        assert!(detail.contains("did not say"), "{detail}");
        assert!(dialing("10.0.0.1"), "held on the primary first");
        wait_until("the move after the minimum outage", 60, || dialing("10.0.0.2"));
        assert!(
            started.elapsed() >= Duration::from_secs(6),
            "moved before the minimum outage: {:?}",
            started.elapsed()
        );
        stop("test").expect("stop");
    });
}

#[test]
fn dns_supervisor_with_k2dev_silent_never_moves_inside_150s() {
    // The production minimum (no override): a fast-failing primary for
    // ~10 s never rotates when k2.dev gives no answer.
    with_temp_home(|| {
        super::super::relay_list::reset_state_for_test();
        seed_two_relays();
        start(None, 57839, &FrpcBinary::Explicit(true_bin())).expect("start");
        wait_until("the DNS hold", 30, || {
            failover_state().is_some_and(|(_, s, _)| s == "waiting_for_dns")
        });
        std::thread::sleep(Duration::from_secs(8));
        assert!(dialing("10.0.0.1"), "must not leave the primary inside 150 s");
        assert_eq!(status().relay_primary, Some(true));
        stop("test").expect("stop");
    });
}

#[test]
fn dns_supervisor_fails_back_fast_once_k2dev_points_the_name_at_the_primary() {
    with_temp_home(|| {
        // 0.46.3: the standby a DNS-mode move goes to must answer.
        let _reach = TestReachability::install(|_| true);
        super::super::relay_list::reset_state_for_test();
        // Fast DNS-mode timing: probe every 1 s, 2 healthy probes.
        let _policy = TestFailbackPolicy::install(super::super::failover::FailbackPolicy {
            probe_every: Duration::from_secs(1),
            probe_spread: Duration::ZERO,
            healthy_probes: 2,
            failback_delay_max: Duration::ZERO,
        });
        let primary = std::net::TcpListener::bind("127.0.0.1:0").expect("bind primary");
        let p_port = primary.local_addr().expect("addr").port();
        let fallback = crate::test_env::ClosedPort::new();
        let f_port = fallback.port();
        let p_ep = RelayEndpoint { host: "127.0.0.1".to_string(), port: p_port };
        let f_ep = RelayEndpoint { host: "127.0.0.1".to_string(), port: f_port };
        // The name is on the standby (DNS flipped during the outage).
        super::super::relay_list::set_dns_for_test(Some(f_ep.clone()), Duration::ZERO);

        let dir = crate::test_env::unique_temp_path("fake-frpc-dns-failback");
        std::fs::create_dir_all(&dir).expect("mk dir");
        let script = dir.join("frpc-pick");
        crate::test_env::write_executable(
            &script,
            &format!(
                "#!/bin/sh\nport=$(grep '^serverPort' \"$2\" | tr -dc 0-9)\n\
                     if [ \"$port\" = \"{p_port}\" ] && [ ! -e '{allow}' ]; then exit 1; fi\n\
                     exec sleep 600\n",
                allow = dir.join("allow").display(),
            ),
        );
        config::save(&TunnelConfig {
            relays: vec![p_ep.clone(), f_ep.clone()],
            token: "tok".to_string(),
            subdomain: "rosson".to_string(),
            e2e: false,
            ..Default::default()
        })
        .expect("seed two-relay config");
        start(None, 57839, &FrpcBinary::Explicit(script)).expect("start");
        wait_until("failover to the active standby", 30, || {
            status().server_port == Some(f_port)
        });
        // The primary answers TCP again, but k2.dev still says the
        // name is on the standby: going back would take the box off
        // its public name, so it stays.
        std::fs::write(dir.join("allow"), b"").expect("allow primary");
        wait_until("the name-here hold", 15, || {
            failover_state().is_some_and(|(_, _, d)| d.contains("points here"))
        });
        std::thread::sleep(Duration::from_secs(4));
        assert_eq!(status().server_port, Some(f_port), "no fail-back while the name is here");

        // k2.dev: the name is back on the primary → 2 healthy probes.
        super::super::relay_list::set_dns_for_test(Some(p_ep), Duration::ZERO);
        wait_until("fail-back to the primary", 30, || status().server_port == Some(p_port));
        assert_eq!(status().relay_primary, Some(true));
        assert_eq!(failover_state().expect("status").1, "ready");
        stop("test").expect("stop");
        drop(primary);
        let _ = std::fs::remove_dir_all(&dir);
        super::super::relay_list::reset_state_for_test();
    });
}

#[test]
fn dns_supervisor_with_no_relay_reachable_stays_on_the_primary_and_backs_off() {
    // z3mbpZ on a dead network: 0.46.2 alternated every ~6 s. Even
    // with every switch rule met (k2.dev names the standby, zero
    // minimum outage) an unreachable standby is never switched to.
    with_temp_home(|| {
        super::super::relay_list::reset_state_for_test();
        super::super::relay_list::set_dns_for_test(Some(ten(2)), Duration::ZERO);
        let _gate = TestDnsMinOutage::install(Duration::ZERO);
        let _reach = TestReachability::install(|_| false);
        let dir = crate::test_env::unique_temp_path("fake-frpc-offline");
        std::fs::create_dir_all(&dir).expect("mk dir");
        let script = dir.join("frpc-offline");
        // Every dial fails; each one is recorded (host per line).
        crate::test_env::write_executable(
            &script,
            &format!(
                "#!/bin/sh\ngrep '^serverAddr' \"$2\" | tr -d '\" ' | cut -d= -f2 >> '{dials}'\nexit 1\n",
                dials = dir.join("dials").display(),
            ),
        );
        seed_two_relays();
        start(None, 57839, &FrpcBinary::Explicit(script)).expect("start");
        wait_until("the no-relay hold", 30, || {
            failover_state().is_some_and(|(_, s, _)| s == "no_relay_reachable")
        });
        std::thread::sleep(Duration::from_secs(12));
        let dials = std::fs::read_to_string(dir.join("dials")).expect("dials");
        let hosts: Vec<&str> = dials.lines().collect();
        assert!(
            hosts.iter().all(|h| *h == "10.0.0.1"),
            "must only ever dial the primary: {hosts:?}"
        );
        // Backoff 0.5, 1, 2, 4, 8 s… (never reset by a switch): a
        // handful of dials, not one every second or two.
        assert!(hosts.len() <= 8, "{} dials — the backoff must grow: {hosts:?}", hosts.len());
        assert_eq!(status().relay_primary, Some(true));
        stop("test").expect("stop");
        let _ = std::fs::remove_dir_all(&dir);
        super::super::relay_list::reset_state_for_test();
    });
}


#[test]
fn legacy_switch_reports_legacy_and_never_asks_k2dev() {
    with_temp_home(|| {
        super::super::relay_list::reset_state_for_test();
        // Even a k2.dev answer naming the primary is ignored in legacy.
        super::super::relay_list::set_dns_for_test(Some(ten(1)), Duration::ZERO);
        let _env = crate::test_env::EnvVar::set(config::FAILOVER_ENV, "legacy");
        seed_two_relays();
        let st = start(None, 57839, &FrpcBinary::Explicit(true_bin())).expect("start");
        let f = st.failover.expect("two relays report failover");
        assert_eq!((f.mode.as_str(), f.state.as_str()), ("legacy", "legacy"));
        wait_until("the 0.46.2 rotation after 3 failures", 30, || dialing("10.0.0.2"));
        stop("test").expect("stop");
        super::super::relay_list::reset_state_for_test();
    });
}

#[test]
fn solo_relay_reports_no_failover() {
    with_temp_home(|| {
        config::save(&TunnelConfig {
            server_addr: "9.9.9.9".to_string(),
            token: "tok".to_string(),
            subdomain: "rosson".to_string(),
            e2e: false,
            ..Default::default()
        })
        .expect("seed");
        let st = start(None, 57839, &FrpcBinary::Explicit(true_bin())).expect("start");
        assert_eq!(st.failover, None);
        let _ = stop("test");
    });
}


/// A fake frpc that stays up (a healthy, connected tunnel) and records
/// each launch in `<dir>/spawns`.
fn long_lived_frpc(dir: &Path) -> PathBuf {
    std::fs::create_dir_all(dir).expect("mk fake-frpc dir");
    let script = dir.join("frpc-up");
    crate::test_env::write_executable(
        &script,
        &format!(
            "#!/bin/sh\necho spawn >> '{}'\nexec sleep 600\n",
            dir.join("spawns").display()
        ),
    );
    script
}

fn spawn_count(dir: &Path) -> usize {
    std::fs::read_to_string(dir.join("spawns"))
        .expect("the fake frpc must have recorded its launches")
        .lines()
        .count()
}

fn ep(host: &str, port: u16) -> RelayEndpoint {
    RelayEndpoint { host: host.to_string(), port }
}

fn running_pid() -> i32 {
    frpc_proc::read_pid_record(&frpc_config_path())
        .expect("the running frpc's pid record")
        .pid
}

/// 0.46.6 (PRD test 14, D7): a new list that no longer has the relay in
/// use (here written through `set_config`, the route's surface) moves a
/// RUNNING tunnel exactly once to the new primary, after its jitter,
/// with no failure counted and no backoff. (Before 0.46.6 the running
/// tunnel ignored it until a reconnect.)
#[test]
fn a_dropped_relay_moves_a_running_tunnel_once_without_a_penalty() {
    with_temp_home(|| {
        config::save(&TunnelConfig {
            server_addr: "9.9.9.9".to_string(),
            server_port: 7009,
            token: "tok".to_string(),
            subdomain: "rosson".to_string(),
            e2e: false,
            ..Default::default()
        })
        .expect("seed legacy config");

        let dir = crate::test_env::unique_temp_path("fake-frpc-up");
        let frpc = long_lived_frpc(&dir);
        let st = start(None, 57839, &FrpcBinary::Explicit(frpc.clone()))
            .expect("start on legacy endpoint");
        assert!(st.running);
        wait_until("the first frpc launch", 10, || dir.join("spawns").exists());
        let pid0 = running_pid();

        super::super::set_config(super::super::TunnelConfigUpdate {
            relays: Some(vec![ep("10.0.0.1", 7000), ep("10.0.0.2", 7000)]),
            ..Default::default()
        })
        .expect("set relays while running");
        let written = Instant::now();
        wait_until("the controlled move to the new primary", 10, || {
            status().server_addr.as_deref() == Some("10.0.0.1")
                && std::fs::read_to_string(frpc_config_path())
                    .map(|t| t.contains("serverAddr = \"10.0.0.1\""))
                    .unwrap_or(false)
                && spawn_count(&dir) == 2
        });
        assert!(
            written.elapsed() < Duration::from_secs(5),
            "check (≤300 ms) + jitter (≤300 ms) + an immediate respawn, no backoff: {:?}",
            written.elapsed()
        );
        assert_ne!(running_pid(), pid0, "the old frpc was replaced");
        let st = status();
        assert_eq!(st.relay_primary, Some(true));
        assert_eq!(st.server_port, Some(7000));
        assert_eq!(st.frpc_consecutive_failures, Some(0), "a move is not a failure");
        // Exactly one move: the new list is now the one in use.
        std::thread::sleep(Duration::from_secs(2));
        assert_eq!(spawn_count(&dir), 2, "one controlled move, never a loop");
        stop("test").expect("stop");
        let _ = std::fs::remove_dir_all(&dir);
    });
}

/// 0.46.6 (PRD test 13, D7, RD-A6): a running multi-relay tunnel on X
/// whose list becomes [Y, X] adopts it live — same frpc, "primary
/// relay" turns false at once — then the probe-driven fail-back moves
/// it to Y.
#[test]
fn a_running_tunnel_adopts_a_reordered_list_live_then_fails_back() {
    with_temp_home(|| {
        let _policy = fast_failback();
        let x = std::net::TcpListener::bind("127.0.0.1:0").expect("bind X");
        let x_port = x.local_addr().expect("addr").port();
        let y = std::net::TcpListener::bind("127.0.0.1:0").expect("bind Y");
        let y_port = y.local_addr().expect("addr").port();
        let z = crate::test_env::ClosedPort::new();
        let dir = crate::test_env::unique_temp_path("fake-frpc-reorder");
        let frpc = long_lived_frpc(&dir);
        config::save(&TunnelConfig {
            relays: vec![ep("127.0.0.1", x_port), ep("127.0.0.1", z.port())],
            token: "tok".to_string(),
            subdomain: "rosson".to_string(),
            e2e: false,
            ..Default::default()
        })
        .expect("seed [X, Z]");
        start(None, 57839, &FrpcBinary::Explicit(frpc)).expect("start");
        wait_until("the first frpc launch", 10, || dir.join("spawns").exists());
        let pid0 = running_pid();
        assert_eq!(status().relay_primary, Some(true));

        config::update(|c| c.relays = vec![ep("127.0.0.1", y_port), ep("127.0.0.1", x_port)])
            .expect("the list changes to [Y, X]");
        wait_until("the live adopt", 5, || status().relay_primary == Some(false));
        let st = status();
        assert_eq!(st.server_port, Some(x_port), "still on X, frpc untouched");
        assert_eq!(running_pid(), pid0, "same frpc");
        assert_eq!(spawn_count(&dir), 1, "an adopt never restarts frpc");
        assert_eq!(
            st.failover.as_ref().map(|f| f.state.as_str()),
            Some("on_fallback"),
            "{:?}",
            st.failover
        );

        wait_until("the fail-back to Y", 30, || status().server_port == Some(y_port));
        assert_eq!(status().relay_primary, Some(true));
        // Status flips before the new frpc writes its launch line.
        wait_until("the fail-back's one reconnect", 10, || spawn_count(&dir) == 2);
        std::thread::sleep(Duration::from_secs(1));
        assert_eq!(spawn_count(&dir), 2, "one reconnect, the fail-back");
        stop("test").expect("stop");
        drop((x, y));
        let _ = std::fs::remove_dir_all(&dir);
    });
}

/// 0.46.6 (PRD test 15): a SOLO tunnel whose relay is still listed in
/// a new 2-relay list becomes a multi-relay tunnel without an frpc
/// restart; status shows the failover rules from then on.
#[test]
fn a_solo_tunnel_becomes_multi_relay_without_a_restart() {
    with_temp_home(|| {
        let dir = crate::test_env::unique_temp_path("fake-frpc-solo-multi");
        let frpc = long_lived_frpc(&dir);
        config::save(&TunnelConfig {
            server_addr: "10.0.0.1".to_string(),
            token: "tok".to_string(),
            subdomain: "rosson".to_string(),
            e2e: false,
            ..Default::default()
        })
        .expect("seed solo");
        let st = start(None, 57839, &FrpcBinary::Explicit(frpc)).expect("start");
        assert_eq!(st.failover, None, "solo: no failover");
        wait_until("the first frpc launch", 10, || dir.join("spawns").exists());
        let pid0 = running_pid();

        config::update(|c| c.relays = vec![ep("10.0.0.1", 7000), ep("10.0.0.2", 7000)])
            .expect("k2.dev hands out a second relay");
        wait_until("the solo selector to adopt the 2-relay list", 5, || {
            status().failover.is_some()
        });
        let st = status();
        assert_eq!(st.server_addr.as_deref(), Some("10.0.0.1"));
        assert_eq!(st.relay_primary, Some(true));
        assert_eq!(st.failover.as_ref().map(|f| f.mode.as_str()), Some("dns"));
        std::thread::sleep(Duration::from_secs(1));
        assert_eq!(running_pid(), pid0, "same frpc");
        assert_eq!(spawn_count(&dir), 1, "no restart");
        stop("test").expect("stop");
        let _ = std::fs::remove_dir_all(&dir);
    });
}

/// 0.46.6 (D7 at an update): the new daemon finds the handed-off frpc
/// still dialing the old baked IP while the dial list is now
/// relay.k2.dev. It adopts that frpc (no reconnect at the update) and
/// the live list check moves it once, with jitter.
#[test]
fn an_adopted_frpc_on_the_old_ip_moves_to_the_name_with_jitter() {
    with_temp_home(|| {
        let _e = crate::test_env::EnvVar::remove(config::RELAY_DEFAULT_ENV);
        let dir = crate::test_env::unique_temp_path("adopt-frpc-old-ip");
        std::fs::create_dir_all(&dir).expect("mk dir");
        let script = registering_frpc(&dir);
        config::save(&TunnelConfig {
            server_addr: "178.156.232.105".to_string(),
            token: "tok".to_string(),
            subdomain: "rosson".to_string(),
            e2e: false,
            ..Default::default()
        })
        .expect("seed a 0.46.5 tunnel.json");
        super::super::ingress::register_ensure_tunnel_ingress(Box::new(|| Ok(46124)));
        let _lease = crate::test_env::EnvVar::set(super::super::lease::LEASE_ENV_VAR, "off");

        // The 0.46.5 daemon: the name never resolved, frpc dials the IP.
        start(None, 57839, &FrpcBinary::Explicit(script.clone())).expect("start");
        let toml = std::fs::read_to_string(frpc_config_path()).expect("frpc.toml");
        assert!(toml.contains("serverAddr = \"178.156.232.105\""), "{toml}");
        let old = hand_off_and_forget();

        // The updated daemon: the name resolves.
        super::super::relay_list::set_name_ok_for_test(Some(vec![
            "147.135.65.8".parse().expect("ip"),
        ]));
        let st = start(None, 57839, &FrpcBinary::Explicit(script.clone())).expect("adopting start");
        assert_eq!(st.server_addr.as_deref(), Some("178.156.232.105"), "adopted as it is");
        assert_eq!(st.relay_primary, Some(false), "not on the list's primary");
        assert_eq!(
            frpc_proc::read_pid_record(&frpc_config_path()).expect("record").pid,
            old,
            "adopted, not respawned"
        );
        wait_until("the jittered move to relay.k2.dev", 10, || {
            status().server_addr.as_deref() == Some("relay.k2.dev")
                && std::fs::read_to_string(frpc_config_path())
                    .map(|t| t.contains("serverAddr = \"relay.k2.dev\""))
                    .unwrap_or(false)
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while frpc_proc::pid_alive(old) {
            assert!(Instant::now() < deadline, "the old frpc must be stopped by the move");
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(status().relay_primary, Some(true));
        stop("test").expect("stop");
        let _ = std::fs::remove_dir_all(&dir);
    });
}

/// 0.45.7: a relay list written while the tunnel runs (k2.dev refresh,
/// pairing) is applied at the next RECONNECT — here /bin/true exits at
/// once, so the supervisor reconnects continually and must dial the new
/// primary without anyone calling stop/start.
#[test]
fn a_changed_relay_list_is_applied_at_the_next_reconnect() {
    with_temp_home(|| {
        config::save(&TunnelConfig {
            server_addr: "9.9.9.9".to_string(),
            server_port: 7009,
            token: "tok".to_string(),
            subdomain: "rosson".to_string(),
            e2e: false,
            ..Default::default()
        })
        .expect("seed legacy config");
        start(None, 57839, &FrpcBinary::Explicit(true_bin())).expect("start");
        config::update(|c| {
            c.relays = vec![
                RelayEndpoint { host: "10.0.0.1".to_string(), port: 7000 },
                RelayEndpoint { host: "10.0.0.2".to_string(), port: 7000 },
            ];
        })
        .expect("write a new list");
        wait_until("the reconnect to adopt the new list", 30, || {
            std::fs::read_to_string(frpc_config_path())
                .map(|t| t.contains("serverAddr = \"10.0.0."))
                .unwrap_or(false)
        });
        let st = status();
        assert!(st.running);
        assert!(
            st.server_addr.as_deref().is_some_and(|a| a.starts_with("10.0.0.")),
            "status must follow the adopted list: {:?}",
            st.server_addr
        );
        assert_eq!(st.relay_list.relays, vec!["10.0.0.1:7000", "10.0.0.2:7000"]);
        stop("test").expect("stop");
    });
}

/// 0.45.7, the 2026-10-15 shape: a SOLO tunnel whose only relay died
/// mid-session (frpc alive, storming reconnect errors) stays put while
/// the list is unchanged — frpc's own retry, as before — and reconnects
/// to the new list once `tunnel.json` gains relays.
#[test]
fn solo_dead_session_reconnects_only_when_the_list_gains_relays() {
    with_temp_home(|| {
        let dir = crate::test_env::unique_temp_path("fake-frpc-solo");
        std::fs::create_dir_all(&dir).expect("mk dir");
        let script = dir.join("frpc-stuck");
        crate::test_env::write_executable(
            &script,
            &format!(
                "#!/bin/sh\necho spawn >> '{}'\n\
                     echo '[I] [service.go:299] login to server success, get run id [test]'\n\
                     while true; do\n\
                     \techo '[W] [service.go:132] connect to server error: dial tcp 9.9.9.9:7009: i/o timeout'\n\
                     \tsleep 0.1\n\
                     done\n",
                dir.join("spawns").display()
            ),
        );
        config::save(&TunnelConfig {
            server_addr: "9.9.9.9".to_string(),
            server_port: 7009,
            token: "tok".to_string(),
            subdomain: "rosson".to_string(),
            e2e: false,
            ..Default::default()
        })
        .expect("seed legacy config");
        // Launches recorded so far (0 before the fake has written its
        // line — a polling condition, never an assertion default).
        let launches = || {
            std::fs::read_to_string(dir.join("spawns"))
                .map(|s| s.lines().count())
                .unwrap_or(0)
        };
        let checks_before = SOLO_LIST_UNCHANGED_CHECKS.load(Ordering::SeqCst);
        start(None, 57839, &FrpcBinary::Explicit(script)).expect("start");
        wait_until("the first fake frpc launch", 10, || launches() >= 1);
        // Wait on the DECISION, not a clock: the watchdog declares the
        // session dead, and the solo supervisor re-reads the (unchanged)
        // list three times and leaves frpc's own retry alone each time.
        // (A stale supervisor from an earlier test can add at most one.)
        wait_until("three unchanged-list checks on the dead solo session", 30, || {
            SOLO_LIST_UNCHANGED_CHECKS.load(Ordering::SeqCst) >= checks_before + 4
        });
        assert_eq!(spawn_count(&dir), 1, "unchanged list → leave frpc's own retry alone");
        assert_eq!(status().server_addr.as_deref(), Some("9.9.9.9"));

        config::update(|c| {
            c.relays = vec![
                RelayEndpoint { host: "10.0.0.1".to_string(), port: 7000 },
                RelayEndpoint { host: "10.0.0.2".to_string(), port: 7000 },
            ];
        })
        .expect("the refreshed list lands");
        // The supervisor renders frpc.toml for the new relay BEFORE it
        // launches the new child, so wait for both (2026-10-15 flake:
        // the launch count was read between the two).
        wait_until("the dead solo session to reconnect on the new list", 30, || {
            launches() >= 2
                && std::fs::read_to_string(frpc_config_path())
                    .map(|t| t.contains("serverAddr = \"10.0.0.1\""))
                    .unwrap_or(false)
        });
        stop("test").expect("stop");
        let _ = std::fs::remove_dir_all(&dir);
    });
}

/// Fast fail-back timing for the supervisor tests: probe every 1 s, 3
/// healthy probes, no random delay.
fn fast_failback() -> TestFailbackPolicy {
    TestFailbackPolicy::install(super::super::failover::FailbackPolicy {
        probe_every: Duration::from_secs(1),
        probe_spread: Duration::ZERO,
        healthy_probes: 3,
        failback_delay_max: Duration::ZERO,
    })
}

/// 0.45.7 fail-back, end to end: the primary refuses frpc at first (the
/// fake exits at once for its port), the supervisor fails over, then —
/// with the primary's port answering TCP — 3 healthy probes bring it
/// back to the primary with one reconnect.
#[test]
fn supervisor_fails_back_to_the_primary_after_healthy_probes() {
    with_temp_home(|| {
        let _policy = fast_failback();
        // The primary "relay": a bound loopback port (TCP connects
        // succeed from the backlog; nothing needs to accept).
        let primary = std::net::TcpListener::bind("127.0.0.1:0").expect("bind primary");
        let p_port = primary.local_addr().expect("addr").port();
        let fallback = crate::test_env::ClosedPort::new();
        let f_port = fallback.port();

        let dir = crate::test_env::unique_temp_path("fake-frpc-failback");
        std::fs::create_dir_all(&dir).expect("mk dir");
        let script = dir.join("frpc-pick");
        // Dials to the primary fail fast until `allow` exists; dials
        // anywhere else stay up. Every dial is recorded.
        crate::test_env::write_executable(
            &script,
            &format!(
                "#!/bin/sh\nport=$(grep '^serverPort' \"$2\" | tr -dc 0-9)\n\
                     echo $port >> '{dials}'\n\
                     if [ \"$port\" = \"{p_port}\" ] && [ ! -e '{allow}' ]; then exit 1; fi\n\
                     exec sleep 600\n",
                dials = dir.join("dials").display(),
                allow = dir.join("allow").display(),
            ),
        );
        config::save(&TunnelConfig {
            relays: vec![
                RelayEndpoint { host: "127.0.0.1".to_string(), port: p_port },
                RelayEndpoint { host: "127.0.0.1".to_string(), port: f_port },
            ],
            // The 0.45.7/0.46.2 probe-driven fail-back, end to end.
            failover: Some("legacy".to_string()),
            token: "tok".to_string(),
            subdomain: "rosson".to_string(),
            e2e: false,
            ..Default::default()
        })
        .expect("seed two-relay config");
        start(None, 57839, &FrpcBinary::Explicit(script)).expect("start");

        wait_until("failover to the fallback", 30, || {
            status().server_port == Some(f_port)
        });
        let st = status();
        assert_eq!(st.relay_primary, Some(false), "on the fallback");
        // The primary now takes frpc again; its port answers probes.
        std::fs::write(dir.join("allow"), b"").expect("allow primary");
        wait_until("fail-back to the primary", 30, || {
            status().server_port == Some(p_port)
        });
        assert_eq!(status().relay_primary, Some(true));
        wait_until("the primary to be dialed again", 10, || {
            std::fs::read_to_string(dir.join("dials"))
                .map(|d| d.lines().last() == Some(p_port.to_string().as_str()))
                .unwrap_or(false)
        });
        stop("test").expect("stop");
        drop(primary);
        let _ = std::fs::remove_dir_all(&dir);
    });
}

/// 0.45.7: while the primary is the relay in use nothing probes it and
/// nothing fails back.
#[test]
fn no_failback_probes_while_on_the_primary() {
    with_temp_home(|| {
        let _policy = fast_failback();
        let primary = std::net::TcpListener::bind("127.0.0.1:0").expect("bind primary");
        primary.set_nonblocking(true).expect("nonblocking");
        let p_port = primary.local_addr().expect("addr").port();
        let dir = crate::test_env::unique_temp_path("fake-frpc-onprimary");
        let frpc = long_lived_frpc(&dir);
        config::save(&TunnelConfig {
            relays: vec![
                RelayEndpoint { host: "127.0.0.1".to_string(), port: p_port },
                RelayEndpoint { host: "127.0.0.1".to_string(), port: 9 },
            ],
            token: "tok".to_string(),
            subdomain: "rosson".to_string(),
            e2e: false,
            ..Default::default()
        })
        .expect("seed");
        start(None, 57839, &FrpcBinary::Explicit(frpc)).expect("start");
        std::thread::sleep(Duration::from_secs(4));
        match primary.accept() {
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Ok((_, from)) => panic!("the primary in use was probed (connection from {from})"),
            Err(e) => panic!("accept: {e}"),
        }
        let st = status();
        assert_eq!(st.relay_primary, Some(true));
        assert_eq!(st.server_port, Some(p_port));
        assert_eq!(spawn_count(&dir), 1, "no reconnect while on the primary");
        stop("test").expect("stop");
        let _ = std::fs::remove_dir_all(&dir);
    });
}

/// PRD tunnel-disable-unpair §2A — DISABLE BLOCKS SPAWN: with the
/// persisted pause flag down, `start()` must refuse before any side
/// effect (no frpc.toml render, no child), whoever calls it (boot
/// autostart, route, CLI — all funnel here).
#[test]
fn start_refuses_when_disabled_and_never_renders() {
    with_temp_home(|| {
        config::save(&TunnelConfig {
            token: "tok".to_string(),
            subdomain: "rosson".to_string(),
            e2e: false,
            enabled: false,
            ..Default::default()
        })
        .expect("seed disabled config");

        let err = start(None, 57839, &FrpcBinary::Explicit(true_bin()))
            .expect_err("disabled tunnel must refuse to start");
        assert!(
            err.contains("disabled"),
            "error must name the persisted disable, got: {err}"
        );
        assert!(!status().running, "no child after a refused start");
        assert!(
            !frpc_config_path().exists(),
            "a refused start must not render frpc.toml (gate sits ABOVE the connector)"
        );
        // Status carries the tri-state truth for the UI.
        let st = status();
        assert!(!st.enabled, "status must report the persisted disable");
        assert!(!st.released);

        // Re-enable is symmetric: same config, flag up, start works.
        config::update(|c| c.enabled = true).expect("re-enable");
        let st = start(None, 57839, &FrpcBinary::Explicit(true_bin()))
            .expect("re-enabled tunnel must start");
        assert!(st.running);
        stop("test").expect("stop");
    });
}

/// PRD tunnel-disable-unpair §2B — a RELEASED identity can never
/// re-arm the connector: a planted copy of the old tunnel.json (the
/// stale-backup / zombie case) is refused by the local gate even
/// before the relay gets a say.
#[test]
fn released_identity_cannot_rearm_connector() {
    with_temp_home(|| {
        // The planted stale backup: a fully connectable config…
        config::save(&TunnelConfig {
            token: "tok-old".to_string(),
            subdomain: "rosson".to_string(),
            e2e: false,
            ..Default::default()
        })
        .expect("plant stale identity");
        // …whose identity was RELEASED on this device.
        super::super::unpair::save(&super::super::unpair::UnpairTombstone {
            released_at: "2026-07-12T00:00:00+00:00".to_string(),
            subdomain: "rosson".to_string(),
            device_id: None,
            token_sha256: super::super::unpair::token_fingerprint("tok-old"),
            upstream_reported: true,
            pending_token: None,
            ..Default::default()
        })
        .expect("save tombstone");

        let err = start(None, 57839, &FrpcBinary::Explicit(true_bin()))
            .expect_err("released identity must never re-arm");
        assert!(
            err.contains("released"),
            "error must name the release, got: {err}"
        );
        assert!(!status().running);
        assert!(status().released, "status must report the released state");

        // A FRESH identity (normal re-pair) passes the gate and starts.
        config::save(&TunnelConfig {
            token: "tok-new".to_string(),
            subdomain: "rosson".to_string(),
            e2e: false,
            ..Default::default()
        })
        .expect("re-pair with fresh identity");
        let st = start(None, 57839, &FrpcBinary::Explicit(true_bin()))
            .expect("fresh identity must start");
        assert!(st.running);
        stop("test").expect("stop");
    });
}

/// The incident's KILL-THE-CLASS requirement, end to end through the
/// REAL supervisor: a disable persisted ON DISK while the tunnel is up
/// must stop the respawn loop at the next child exit — the supervisor
/// re-reads the flag at spawn time (no cached copy), exits terminally,
/// and flips the connector to stopped. This is exactly what an
/// orphaned daemon does after the user disables from anywhere else.
#[test]
fn supervisor_stops_respawning_after_on_disk_disable() {
    with_temp_home(|| {
        config::save(&TunnelConfig {
            token: "tok".to_string(),
            subdomain: "rosson".to_string(),
            e2e: false,
            ..Default::default()
        })
        .expect("seed config");

        // /bin/true exits instantly → the supervisor is in a steady
        // exit→backoff→respawn cycle.
        let st = start(None, 57839, &FrpcBinary::Explicit(true_bin()))
            .expect("start");
        assert!(st.running);

        // The disable lands ON DISK only — no stop() call, exactly
        // like a second daemon / another surface flipping the flag.
        config::update(|c| c.enabled = false).expect("persist disable");

        // The next respawn attempt must observe the flag and exit the
        // supervisor terminally. Backoff starts at 0.5s; poll
        // generously so slow CI can't flake this.
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if !status().running {
                break;
            }
            if Instant::now() >= deadline {
                panic!("supervisor kept respawning after an on-disk disable");
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        let st = status();
        assert!(!st.enabled, "status must show the persisted disable");
        stop("test").expect("stop is idempotent after the gate exit");
    });
}

#[test]
fn proxy_registration_tracks_only_the_current_run() {
    let reg = ProxyRegistration::new(&TunnelConfig {
        subdomain: "rosson".to_string(), confirmed_label: Some("rosson".to_string()),
        ..Default::default()
    });
    assert!(!reg.is_registered(), "never spawned");
    let r1 = reg.begin_run();
    let logins = frpc_login_count();
    reg.observe_line(r1, "[I] [service.go:299] login to server success, get run id [x]");
    assert!(!reg.is_registered(), "login alone is not registration");
    assert!(frpc_login_count() > logins, "a login moves the login count (Sew TURN DNS retry)");
    reg.observe_line(r1, "[I] [proxy_wrapper.go:204] [x] [k2so-other] start proxy success");
    assert!(!reg.is_registered(), "another proxy's success does not count");
    reg.observe_line(r1, "[I] [proxy_wrapper.go:204] [x] [k2so-rosson2] start proxy success");
    assert!(!reg.is_registered(), "a longer name sharing the prefix does not count");
    reg.observe_line(r1, "[I] [proxy_wrapper.go:204] [x] [k2so-rosson] start proxy success");
    assert!(reg.is_registered());
    // A later refusal for our proxy (re-login, name taken) clears it.
    reg.observe_line(r1, "[W] [proxy_wrapper.go:180] [x] [k2so-rosson] start error: proxy [k2so-rosson] already exists");
    assert!(!reg.is_registered());
    reg.observe_line(r1, "[I] [proxy_wrapper.go:204] [x] [k2so-rosson] start proxy success");
    assert!(reg.is_registered());
    // A respawn clears it; the previous child's late output is ignored.
    let r2 = reg.begin_run();
    assert!(!reg.is_registered(), "a new run starts unregistered");
    reg.observe_line(r1, "[I] [proxy_wrapper.go:204] [x] [k2so-rosson] start proxy success");
    assert!(!reg.is_registered(), "a stale run's success must not count");
    reg.observe_line(r2, "[I] [proxy_wrapper.go:204] [x] [k2so-rosson] start proxy success");
    assert!(reg.is_registered());
}

#[test]
fn a_dead_runs_late_error_line_does_not_mark_a_healthy_run() {
    let reg = Arc::new(ProxyRegistration::new(&TunnelConfig {
        subdomain: "rosson".to_string(), confirmed_label: Some("rosson".to_string()),
        ..Default::default()
    }));
    let health = FrpcHealth::new();
    let r1 = reg.begin_run();
    let r2 = reg.begin_run();
    health.record_up(r2);
    assert_eq!(health.last_error(), None);
    let err = "[E] [sub/root.go:142] open /tmp/x/.k2/frpc.toml: no such file or directory";
    handle_line(err, None, &(reg.clone(), r1), &health);
    assert_eq!(health.last_error(), None, "a dead run's late error must be ignored");
    // Already past the current-run check when the next run came up:
    // the health itself drops it.
    health.observe_line(r1, err);
    assert_eq!(health.last_error(), None, "an older run's line never lands on a run that came up");
    assert_eq!(health.snapshot(), (0, None));
    handle_line(err, None, &(reg.clone(), r2), &health);
    assert_eq!(health.last_error().as_deref(), Some(err));
    assert_eq!(health.failures(), 0, "an error line alone is not a failed run");
}

#[test]
fn the_registration_clears_when_the_frps_session_is_lost() {
    // Real frp 0.61 lines (from a K2 box's frpc.log, ANSI colour kept).
    const OK: &str = "\u{1b}[0m\u{1b}[1;34m2026-06-03 16:20:11.804 [I] [client/control.go:165] [a7c3215c5a5056eb] [k2so-rosson] start proxy success";
    let lost = [
        "\u{1b}[0m\u{1b}[1;33m2026-06-03 22:06:31.021 [W] [client/service.go:319] [c8e85694758a289f] connect to server error: dial tcp 178.156.232.105:7000: connect: network is unreachable",
        "\u{1b}[0m\u{1b}[1;34m2026-06-03 17:19:51.768 [I] [client/service.go:308] [b3b887f43fa3967e] try to connect to server...",
        "\u{1b}[0m\u{1b}[1;33m2026-06-03 18:32:27.570 [W] [client/control.go:119] [b3b887f43fa3967e] start new connection to server error: session shutdown",
        "[W] [client/control.go:80] [b3b887f43fa3967e] work connection write to server error: session shutdown",
        "[W] [client/service.go:200] [x] login to server failed: i/o timeout",
        "[I] [client/service.go:150] [x] try to reconnect to server...",
        "[W] [client/control.go:90] [x] work connection closed before response StartWorkConn message: EOF",
    ];
    for line in lost {
        let reg = ProxyRegistration::new(&TunnelConfig {
            subdomain: "rosson".to_string(), confirmed_label: Some("rosson".to_string()),
            ..Default::default()
        });
        let run = reg.begin_run();
        reg.observe_line(run, OK);
        assert!(reg.is_registered(), "the success line registers");
        reg.observe_line(run, line);
        assert!(!reg.is_registered(), "a lost session must clear the flag: {line}");
        // Re-login and a new success line register again.
        reg.observe_line(run, "[I] [client/service.go:287] [d1] login to server success, get run id [d1]");
        assert!(!reg.is_registered(), "logging in again is not registration");
        reg.observe_line(run, OK);
        assert!(reg.is_registered());
    }
}

#[test]
fn a_respawn_clears_the_proxy_registration() {
    with_temp_home(|| {
        // Run 1 registers then exits; every later run is refused.
        let dir = crate::test_env::unique_temp_path("fake-frpc");
        std::fs::create_dir_all(&dir).expect("mk fake-frpc dir");
        let counter = dir.join("runs");
        let script = dir.join("frpc");
        crate::test_env::write_executable(
            &script,
            &format!(
                "#!/bin/sh\n\
                     n=$(cat '{c}' 2>/dev/null || echo 0); n=$((n+1)); echo $n > '{c}'\n\
                     if [ \"$n\" = 1 ]; then\n\
                     \techo '[I] [proxy_wrapper.go:204] [x] [k2so-rosson] start proxy success'\n\
                     \tsleep 1\n\
                     \texit 1\n\
                     fi\n\
                     echo '[W] [proxy_wrapper.go:180] [x] [k2so-rosson] start error: proxy [k2so-rosson] already exists'\n\
                     exec sleep 60\n",
                c = counter.display()
            ),
        );
        config::save(&TunnelConfig {
            token: "tok".to_string(),
            subdomain: "rosson".to_string(),
            e2e: false,
            ..Default::default()
        })
        .expect("seed config");
        let _hook = super::super::ingress::TestIngressHook::install(Box::new(|| Ok(46124)));
        let _lease = crate::test_env::EnvVar::set(super::super::lease::LEASE_ENV_VAR, "off");
        start(None, 57839, &FrpcBinary::Explicit(script.clone())).expect("start");

        let deadline = Instant::now() + Duration::from_secs(10);
        while !proxy_registered() {
            assert!(Instant::now() < deadline, "run 1's success line was never seen");
            std::thread::sleep(Duration::from_millis(10));
        }
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let runs = std::fs::read_to_string(&counter).unwrap_or_default();
            let refused = std::fs::read_to_string(frpc_log_path())
                .unwrap_or_default()
                .contains("already exists");
            if runs.trim() == "2" && refused {
                break;
            }
            assert!(Instant::now() < deadline, "the supervisor never respawned (runs={runs:?})");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(status().running, "the respawned frpc runs");
        assert!(!proxy_registered(), "a respawn must clear the registration");
        assert!(registered_config().is_none());

        stop("test").expect("stop");
        assert!(!proxy_registered());
        // The hook is process-global: leaving it installed made
        // `supervisor_rotation_keeps_frozen_local_port` (which expects
        // the default port) order-dependent.
        super::super::ingress::clear_ensure_tunnel_ingress();
        let _ = std::fs::remove_dir_all(&dir);
    });
}

fn wait_until(what: &str, secs: u64, mut ok: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while !ok() {
        assert!(Instant::now() < deadline, "timed out waiting for: {what}\nstatus: {:?}", status());
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// 2026-10-14 incident, end to end through the REAL supervisor: the
/// rendered config is deleted under a running tunnel. The fake frpc
/// behaves like the real one — it refuses to start without its config
/// (`open …: no such file or directory`) and a live one exits when the
/// config is gone (standing in for its next restart). The supervisor
/// must re-render before every respawn, status must say UNHEALTHY while
/// runs fail (not a bare "running"), and `start()` on a running-but-
/// broken tunnel must repair it instead of reporting success.
#[test]
fn supervisor_rerenders_a_deleted_config_and_status_reports_a_crash_loop() {
    with_temp_home(|| {
        let dir = crate::test_env::unique_temp_path("fake-frpc");
        std::fs::create_dir_all(&dir).expect("mk fake-frpc dir");
        let starts = dir.join("starts");
        let broken = dir.join("broken");
        let hold = dir.join("hold");
        let script = dir.join("frpc");
        crate::test_env::write_executable(
            &script,
            &format!(
                "#!/bin/sh\n\
                     cfg=\"$2\"\n\
                     if [ -f '{b}' ] || [ ! -f \"$cfg\" ]; then\n\
                     \techo \"[E] [sub/root.go:142] open $cfg: no such file or directory\" >&2\n\
                     \texit 1\n\
                     fi\n\
                     grep -q 'token = \"tok\"' \"$cfg\" || {{ echo '[E] bad config' >&2; exit 1; }}\n\
                     echo started >> '{s}'\n\
                     echo '[I] [proxy_manager.go:173] [x] [k2so-rosson] start proxy success'\n\
                     while [ -f \"$cfg\" ] || [ -f '{h}' ]; do sleep 0.1; done\n\
                     echo '[E] config vanished' >&2\n\
                     exit 1\n",
                b = broken.display(),
                s = starts.display(),
                h = hold.display(),
            ),
        );
        let start_count = || {
            std::fs::read_to_string(&starts).unwrap_or_default().lines().count()
        };
        config::save(&TunnelConfig {
            token: "tok".to_string(),
            subdomain: "rosson".to_string(),
            e2e: false,
            ..Default::default()
        })
        .expect("seed config");
        // No ingress hook: a registered one outlives the test and
        // changes other tests' resolved port.
        super::super::ingress::clear_ensure_tunnel_ingress();
        let _lease = crate::test_env::EnvVar::set(super::super::lease::LEASE_ENV_VAR, "off");
        let bin = FrpcBinary::Explicit(script.clone());
        start(None, 57839, &bin).expect("start");

        wait_until("first clean frpc start", 10, || start_count() == 1 && proxy_registered());
        let st = status();
        assert_eq!(st.frpc_healthy, Some(true), "{st:?}");
        assert_eq!(st.frpc_consecutive_failures, Some(0), "{st:?}");
        assert_eq!(st.frpc_config_present, Some(true), "{st:?}");

        // 1) Delete the config under the running tunnel: the fake exits,
        //    and the respawn must find a freshly rendered config.
        std::fs::remove_file(frpc_config_path()).expect("delete frpc.toml");
        wait_until("a clean respawn after the config was deleted", 15, || {
            start_count() == 2 && proxy_registered()
        });
        let toml = std::fs::read_to_string(frpc_config_path()).expect("re-rendered frpc.toml");
        assert!(toml.contains("token = \"tok\""), "{toml}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(frpc_config_path()).expect("stat").permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "re-rendered config must stay 0600");
        }
        assert_eq!(status().frpc_healthy, Some(true));

        // 2) A crash loop: every run fails. Status must say so, with
        //    frpc's own error line, while `running` stays the desired
        //    state.
        std::fs::write(&broken, "1").expect("break the fake");
        std::fs::remove_file(frpc_config_path()).expect("delete frpc.toml again");
        wait_until("status to report the crash loop", 15, || {
            status().frpc_consecutive_failures.unwrap_or(0) >= 2
        });
        let st = status();
        assert!(st.running, "{st:?}");
        assert_eq!(st.frpc_healthy, Some(false), "{st:?}");
        let err = st.frpc_last_error.clone().expect("a last error while failing");
        assert!(err.contains("no such file or directory"), "{err}");
        assert_eq!(st.frpc_config_present, Some(true), "every respawn re-renders: {st:?}");

        // 3) `start()` on a running-but-failing tunnel repairs it.
        let st = start(None, 57839, &bin).expect("repair start");
        let why = st.repaired.clone().expect("start must repair, not report success");
        assert!(why.contains("consecutive failures"), "{why}");
        std::fs::remove_file(&broken).expect("unbreak the fake");
        wait_until("recovery after the repair", 20, || {
            start_count() == 3 && status().frpc_healthy == Some(true)
        });
        assert_eq!(status().frpc_last_error, None);

        // 4) Config gone while frpc keeps running AND frps has its proxy
        //    registered: status says so, and `start()` re-renders the
        //    config in place WITHOUT restarting the live frpc (0.45.7
        //    pause-safety: frpc read the file at spawn; the supervisor
        //    re-renders before any respawn).
        std::fs::write(&hold, "1").expect("hold the fake up");
        wait_until("the held run is registered", 10, || proxy_registered());
        std::fs::remove_file(frpc_config_path()).expect("delete frpc.toml a third time");
        let st = status();
        assert_eq!(st.frpc_healthy, Some(false), "{st:?}");
        assert_eq!(st.frpc_config_present, Some(false), "{st:?}");
        let gen = spawn_generation();
        let st = start(None, 57839, &bin).expect("start over a registered frpc");
        assert_eq!(st.repaired, None, "a registered frpc must not be restarted: {st:?}");
        assert_eq!(spawn_generation(), gen, "no new supervisor");
        assert!(frpc_config_path().exists(), "start re-rendered the config in place");
        assert_eq!(st.frpc_healthy, Some(true), "{st:?}");
        assert_eq!(start_count(), 3, "the live frpc kept running");
        std::fs::remove_file(&hold).expect("release the hold");

        stop("test").expect("stop");
        let _ = std::fs::remove_dir_all(&dir);
    });
}

#[test]
fn status_is_stopped_before_any_start() {
    with_temp_home(|| {
        // Ensure a clean singleton for this test.
        let _ = stop("test teardown");
        assert_eq!(status(), TunnelStatus::stopped());
    });
}

#[test]
fn stop_is_idempotent_on_stopped_connector() {
    with_temp_home(|| {
        stop("test").expect("first stop ok");
        stop("test").expect("second stop ok");
        assert!(!status().running);
    });
}

#[test]
fn stop_records_and_logs_its_reason() {
    with_temp_home(|| {
        stop("POST /cli/tunnel/stop by owner-token via loopback").expect("stop");
        assert_eq!(
            last_stop_reason().as_deref(),
            Some("POST /cli/tunnel/stop by owner-token via loopback")
        );
    });
    assert_eq!(
        stop_log_line("daemon shutdown", Some("grokbot"), true),
        "[tunnel] stop (daemon shutdown): stopping the tunnel for \"grokbot\" \
             (frps proxy registered: true)"
    );
    assert_eq!(
        stop_log_line("update apply", None, false),
        "[tunnel] stop (update apply): no tunnel running; reaping any stray frpc"
    );
}

/// stop() always reaps by config pattern even when the Child slot is
/// empty (supervisor took the handle) — the orphan-kill path that
/// used to only run on start(). Spawns a long-lived stand-in whose
/// cmdline matches `stray_frpc_pattern`, never installs connector
/// STATE, then stop() must kill it.
#[cfg(unix)]
#[test]
fn stop_reaps_stray_frpc_even_when_child_slot_empty() {
    with_temp_home(|| {
        let cfg_path = frpc_config_path();
        if let Some(dir) = cfg_path.parent() {
            std::fs::create_dir_all(dir).expect("mk .k2");
        }
        std::fs::write(&cfg_path, "# orphan-reap test config\n").expect("write frpc.toml");

        // Binary MUST be named `frpc` so the full cmdline contains the
        // pkill pattern `frpc -c <cfg>` (same as a real frpc spawn).
        let dir = crate::test_env::unique_temp_path("orphan-frpc");
        std::fs::create_dir_all(&dir).expect("mk orphan-frpc dir");
        let frpc = dir.join("frpc");
        crate::test_env::write_executable(&frpc, "#!/bin/sh\nsleep 300\n");

        let mut child = crate::bg_proc::command(&frpc)
            .arg("-c")
            .arg(&cfg_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn orphan stand-in");
        let pid = child.id();
        // Let the process table settle so pkill -f can see it.
        std::thread::sleep(Duration::from_millis(150));
        // Sanity: still running before stop (not yet exited).
        assert!(
            child.try_wait().expect("try_wait before").is_none(),
            "orphan stand-in (pid {pid}) must be alive before stop"
        );

        // No connector STATE — slot is empty by construction.
        stop("test").expect("stop must succeed with empty STATE");
        assert!(!status().running);

        // pkill SIGTERMs the stand-in; the Child handle reaps the
        // zombie. Do NOT use `kill -0` — zombies still "exist" and
        // would false-pass as alive.
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut gone = false;
        while Instant::now() < deadline {
            match child.try_wait() {
                Ok(Some(_)) => {
                    gone = true;
                    break;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(_) => {
                    gone = true;
                    break;
                }
            }
        }
        assert!(
            gone,
            "stop() must reap the stray frpc (pid {pid}) matching {}",
            stray_frpc_pattern(&cfg_path)
        );
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&dir);
    });
}

/// Solo-mode stop must kill a long-lived supervised child (the
/// wait_for_exit poll observes `running=false`). Before the fix, solo
/// blocked on bare `child.wait()` and stop only killed an empty slot.
#[cfg(unix)]
#[test]
fn solo_stop_kills_long_lived_supervised_child() {
    with_temp_home(|| {
        let dir = crate::test_env::unique_temp_path("solo-stop");
        std::fs::create_dir_all(&dir).expect("mk solo-stop dir");
        // Named `frpc` so stop()'s pattern reap is a second line of defense.
        let script = dir.join("frpc");
        crate::test_env::write_executable(&script, "#!/bin/sh\nsleep 300\n");

        config::save(&TunnelConfig {
            token: "tok".to_string(),
            subdomain: "rosson".to_string(),
            e2e: false,
            ..Default::default()
        })
        .expect("seed solo config");

        let st = start(None, 57839, &FrpcBinary::Explicit(script.clone()))
            .expect("start long-lived solo frpc");
        assert!(st.running, "solo start must report running");
        // Give the supervisor a tick to .take() the Child so the slot
        // is empty — the historical bug path.
        std::thread::sleep(Duration::from_millis(200));

        stop("test").expect("stop solo tunnel");
        assert!(!status().running, "status must be stopped after stop()");

        // Child must not still be holding the config path (reap + poll kill).
        // pgrep -f: exit 0 = match still alive, exit 1 = no match (desired).
        // Fail loudly if pgrep is missing or cannot run — never soft-pass.
        let pattern = stray_frpc_pattern(&frpc_config_path());
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut clean = false;
        while Instant::now() < deadline {
            let status = crate::bg_proc::command("pgrep")
                .arg("-f")
                .arg(&pattern)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .expect("pgrep -f must be available to assert solo stop killed frpc");
            // Unix: 0 = found, 1 = not found. Anything else is a tool error.
            let code = status.code().expect("pgrep must exit with a status code");
            assert!(
                code == 0 || code == 1,
                "pgrep -f unexpected exit {code} for pattern `{pattern}`"
            );
            if code == 1 {
                clean = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            clean,
            "solo stop must leave no process matching `{pattern}`"
        );
        let _ = std::fs::remove_dir_all(&dir);
    });
}

/// A launched-frpc count that can't miss one: a child killed before its
/// shell runs records nothing in `<dir>/spawns`.
fn frpc_runs() -> u64 {
    state()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .expect("a started tunnel")
        .proxy
        .current_run()
}

/// Longer than a restart loop's period with the belt in `wait_for_exit`
/// (SETTLE, then the next 1 s poll), so the loop shows in the count.
const NO_MORE_RESTARTS: Duration = Duration::from_secs(5);

/// Stops the tunnel and clears the name list on the way out, also when an
/// assert fails: a supervisor left in the hot loop would run into the
/// next test.
struct PathBTunnel {
    dir: PathBuf,
    _e2e: crate::test_env::EnvVar,
    _lease: crate::test_env::EnvVar,
    _listener: std::net::TcpListener,
}

impl Drop for PathBTunnel {
    fn drop(&mut self) {
        let _ = stop("test");
        super::super::custom_names::reset_for_test();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// An E2E tunnel on the real supervisor with a fake frpc that stays up;
/// `names` is the list it starts with. The listener keeps the localPort
/// self-heal quiet.
fn start_path_b_tunnel(names: Vec<String>) -> PathBTunnel {
    super::super::custom_names::reset_for_test();
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind E2E stand-in");
    let port = listener.local_addr().expect("listener address").port();
    let e2e_env = crate::test_env::EnvVar::remove("K2_E2E");
    let lease = crate::test_env::EnvVar::set(super::super::lease::LEASE_ENV_VAR, "off");
    super::super::tls::publish_https_port(port).expect("publish https port");
    config::save(&TunnelConfig {
        server_addr: "9.9.9.9".to_string(),
        server_port: 7009,
        token: "tok".to_string(),
        subdomain: "rosson".to_string(),
        e2e: true,
        ..Default::default()
    })
    .expect("seed config");
    super::super::custom_names::set(names);
    let dir = crate::test_env::unique_temp_path("fake-frpc-names");
    let frpc = long_lived_frpc(&dir);
    let tunnel = PathBTunnel { dir, _e2e: e2e_env, _lease: lease, _listener: listener };
    start(None, 57839, &FrpcBinary::Explicit(frpc)).expect("start");
    wait_until("the first frpc launch", 10, || tunnel.dir.join("spawns").exists());
    tunnel
}

fn wait_for_new_pid(old: i32) -> i32 {
    wait_until("a new frpc pid", 10, || {
        frpc_proc::read_pid_record(&frpc_config_path()).is_some_and(|r| r.pid != old)
    });
    running_pid()
}

/// `want` launches so far, a child that stayed up, and it is the one
/// recorded as carrying generation `g`. The launch count comes first: it
/// is what the hot loop breaks.
fn assert_settled_on(tunnel: &PathBTunnel, want: u64, pid: i32, g: u64) {
    std::thread::sleep(NO_MORE_RESTARTS);
    assert_eq!(frpc_runs(), want, "frpc launches: a restart loop, not one restart per change");
    assert_eq!(spawn_count(&tunnel.dir) as u64, want, "launches the fake frpc recorded");
    assert_eq!(running_pid(), pid, "the fresh frpc must not be replaced");
    assert!(frpc_proc::pid_alive(pid), "the fresh frpc (pid {pid}) must still be running");
    assert!(status().running);
    assert_eq!(super::super::custom_names::running_generation(), g);
    // What custom_domain_flow's certificate order waits on.
    assert!(super::super::custom_names::wait_running(g, Duration::from_millis(1)));
}

/// The 0.46.8 hot loop, through the REAL supervisor: a Path B name joining
/// restarted frpc, and the respawn didn't record the generation it
/// rendered, so every new child was killed at once (125 spawns a second)
/// and the tunnel stayed down until the daemon restarted. One change is
/// one restart, the new child stays up, and a later change is one more.
#[cfg(unix)]
#[test]
fn a_changed_custom_name_restarts_frpc_once_and_the_new_child_stays() {
    use super::super::custom_names as names;
    with_temp_home(|| {
        // Names set before the tunnel starts (a daemon boot) are in the
        // first render: no restart for them.
        let tunnel = start_path_b_tunnel(vec!["boot.example.com".to_string()]);
        let pid = running_pid();
        assert_settled_on(&tunnel, 1, pid, names::generation());

        assert!(names::set(vec!["boot.example.com".to_string(), "a.example.com".to_string()]));
        let changed = Instant::now();
        wait_until("the restart for the first change", 10, || frpc_runs() >= 2);
        assert!(changed.elapsed() >= names::SETTLE, "a restart waits for the list to settle");
        let pid = wait_for_new_pid(pid);
        assert_settled_on(&tunnel, 2, pid, names::generation());

        assert!(names::set(vec!["a.example.com".to_string()]));
        let changed = Instant::now();
        wait_until("the restart for the second change", 10, || frpc_runs() >= 3);
        assert!(changed.elapsed() >= names::SETTLE, "a restart waits for the list to settle");
        let pid = wait_for_new_pid(pid);
        assert_settled_on(&tunnel, 3, pid, names::generation());
    });
}

/// The other respawns share the post-exit spawn site. Here frpc dies on
/// its own just after a change: the respawn already renders the new list,
/// so the change costs no second restart.
#[cfg(unix)]
#[test]
fn a_respawn_after_frpc_exits_carries_a_changed_name_without_another_restart() {
    use super::super::custom_names as names;
    with_temp_home(|| {
        let tunnel = start_path_b_tunnel(Vec::new());
        let pid = running_pid();
        assert!(names::set(vec!["a.example.com".to_string()]));
        let killed = crate::bg_proc::command("kill")
            .arg("-9")
            .arg(pid.to_string())
            .status()
            .expect("run kill");
        assert!(killed.success(), "kill -9 {pid}: {killed:?}");
        wait_until("the respawn after the exit", 10, || frpc_runs() >= 2);
        let pid = wait_for_new_pid(pid);
        assert_settled_on(&tunnel, 2, pid, names::generation());
    });
}

/// A long-lived frpc stand-in that registers its proxy, never `exec`s
/// (its argv stays `<sh> <…/frpc> -c <cfg>`), and logs straight into
/// the file the connector hands it.
fn registering_frpc(dir: &Path) -> PathBuf {
    let script = dir.join("frpc");
    crate::test_env::write_executable(
        &script,
        "#!/bin/sh\n\
             echo '[I] [proxy_wrapper.go:204] [x] [k2so-rosson] start proxy success'\n\
             while :; do sleep 1; done\n",
    );
    script
}

/// Simulate the daemon process going away after a hand-off: the
/// supervisor lets go (no kill), and the connector singleton is
/// forgotten as a fresh daemon would have it.
fn hand_off_and_forget() -> i32 {
    let pid = detach_for_handoff().expect("a live frpc to hand off");
    // The supervisor polls every second; give it time to let go.
    std::thread::sleep(SUPERVISE_POLL * 2);
    *state().lock().unwrap_or_else(|p| p.into_inner()) = None;
    DETACHED.store(false, Ordering::SeqCst);
    pid
}

/// Runtime stage H (AR3/K37): a new daemon adopts the frpc the previous
/// one left running — same pid, no reap, registration recovered from
/// the log — and a later stop() still kills it.
#[test]
fn a_restarted_daemon_adopts_the_running_frpc() {
    with_temp_home(|| {
        let dir = crate::test_env::unique_temp_path("adopt-frpc");
        std::fs::create_dir_all(&dir).expect("mk dir");
        let script = registering_frpc(&dir);
        config::save(&TunnelConfig {
            token: "tok".to_string(),
            subdomain: "rosson".to_string(),
            e2e: false,
            ..Default::default()
        })
        .expect("seed config");
        super::super::ingress::register_ensure_tunnel_ingress(Box::new(|| Ok(46124)));
        let _lease = crate::test_env::EnvVar::set(super::super::lease::LEASE_ENV_VAR, "off");

        start(None, 57839, &FrpcBinary::Explicit(script.clone())).expect("start");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !proxy_registered() {
            assert!(Instant::now() < deadline, "registration line never seen");
            std::thread::sleep(Duration::from_millis(20));
        }
        let rec = frpc_proc::read_pid_record(&frpc_config_path()).expect("pid record");

        let pid = hand_off_and_forget();
        assert_eq!(pid, rec.pid);
        assert!(frpc_proc::pid_alive(pid), "hand-off must leave frpc running");
        assert!(!status().running, "the forgotten connector reports stopped");

        let st = start(None, 57839, &FrpcBinary::Explicit(script.clone())).expect("adopting start");
        assert!(st.running);
        assert_eq!(
            frpc_proc::read_pid_record(&frpc_config_path()).expect("record").pid,
            pid,
            "the same frpc is supervised — no respawn"
        );
        assert!(frpc_proc::pid_alive(pid), "adoption must not reap it");
        assert!(
            proxy_registered(),
            "the run's `start proxy success` line was replayed from the log"
        );
        let log = std::fs::read_to_string(frpc_log_path()).expect("log");
        assert_eq!(
            log.matches("start proxy success").count(),
            1,
            "exactly one frpc ever ran:\n{log}"
        );

        stop("test").expect("stop");
        let deadline = Instant::now() + Duration::from_secs(5);
        while frpc_proc::pid_alive(pid) {
            assert!(Instant::now() < deadline, "stop() must kill the adopted frpc");
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            frpc_proc::read_pid_record(&frpc_config_path()).is_none(),
            "a stopped tunnel leaves nothing to adopt"
        );
        let _ = std::fs::remove_dir_all(&dir);
    });
}

/// A changed config (here: a moved `localPort`) is never adopted: the
/// old frpc is reaped and a fresh one dials with the new config.
#[test]
fn a_changed_config_is_not_adopted() {
    with_temp_home(|| {
        let dir = crate::test_env::unique_temp_path("adopt-frpc-changed");
        std::fs::create_dir_all(&dir).expect("mk dir");
        let script = registering_frpc(&dir);
        config::save(&TunnelConfig {
            token: "tok".to_string(),
            subdomain: "rosson".to_string(),
            e2e: false,
            ..Default::default()
        })
        .expect("seed config");
        super::super::ingress::register_ensure_tunnel_ingress(Box::new(|| Ok(46124)));
        let _lease = crate::test_env::EnvVar::set(super::super::lease::LEASE_ENV_VAR, "off");
        start(None, 57839, &FrpcBinary::Explicit(script.clone())).expect("start");
        let old = hand_off_and_forget();

        // The ingress seam is first-write-wins, so move the port by
        // editing what the rendered config must match: a different
        // subdomain renders a different proxy name.
        config::update(|c| c.subdomain = "rosson2".to_string()).expect("edit config");
        start(None, 57839, &FrpcBinary::Explicit(script.clone())).expect("fresh start");
        let new = frpc_proc::read_pid_record(&frpc_config_path()).expect("record").pid;
        assert_ne!(new, old, "a changed config must spawn a fresh frpc");
        let deadline = Instant::now() + Duration::from_secs(5);
        while frpc_proc::pid_alive(old) {
            assert!(Instant::now() < deadline, "the stale frpc must be reaped");
            std::thread::sleep(Duration::from_millis(50));
        }
        stop("test").expect("stop");
        let _ = std::fs::remove_dir_all(&dir);
    });
}

/// R1 port freeze: `resolved_local_port` is captured once at start and
/// reused on every multi-relay rewrite — rotation must not change
/// localPort even as serverAddr flips.
#[test]
fn supervisor_rotation_keeps_frozen_local_port() {
    with_temp_home(|| {
        // E2E off with no ingress hook: start freezes the daemon port
        // it was given. Never inherit another test's stub.
        let _no_hook = super::super::ingress::TestIngressHook::none();
        config::save(&TunnelConfig {
            relays: vec![
                RelayEndpoint {
                    host: "10.0.0.1".to_string(),
                    port: 7000,
                },
                RelayEndpoint {
                    host: "10.0.0.2".to_string(),
                    port: 7000,
                },
            ],
            // Rotation-timing independent of the DNS gate.
            failover: Some("legacy".to_string()),
            token: "tok".to_string(),
            subdomain: "rosson".to_string(),
            e2e: false,
            ..Default::default()
        })
        .expect("seed two-relay config");

        // Pin an explicit non-default daemon port so a freeze bug would
        // be obvious (would rebind to 0 / missing / wrong port). No
        // ingress hook (another test's would answer its own port).
        super::super::ingress::clear_ensure_tunnel_ingress();
        let st = start(None, 48123, &FrpcBinary::Explicit(true_bin()))
            .expect("start with two relays");
        assert!(st.running);
        assert_eq!(
            st.local_port,
            Some(48123),
            "start must freeze the resolved daemon port into status"
        );
        let first = std::fs::read_to_string(frpc_config_path()).expect("frpc.toml");
        assert!(
            first.contains("localPort = 48123"),
            "initial render must use the frozen port\n{first}"
        );

        let deadline = Instant::now() + Duration::from_secs(30);
        let rotated = loop {
            let toml = std::fs::read_to_string(frpc_config_path()).expect("frpc.toml");
            if toml.contains("serverAddr = \"10.0.0.2\"") {
                break toml;
            }
            if Instant::now() >= deadline {
                panic!("supervisor never rotated; frpc.toml:\n{toml}");
            }
            std::thread::sleep(Duration::from_millis(200));
        };
        assert!(
            rotated.contains("localPort = 48123"),
            "relay rotation must keep the frozen localPort from start\n{rotated}"
        );
        assert_eq!(
            status().local_port,
            Some(48123),
            "status must still report the frozen port after rotation"
        );
        // Invariant: frpc localPort still matches frozen after rotation.
        tunnel_port_invariant_ok(
            false,
            None,
            None,
            parse_local_port_from_frpc_toml(&rotated),
            Some(48123),
        )
        .expect("port invariant must hold after relay rotation");

        stop("test").expect("stop");
    });
}

#[test]
fn parse_local_port_from_frpc_toml_reads_renderer_shape() {
    assert_eq!(
        parse_local_port_from_frpc_toml("localPort = 44407\n"),
        Some(44407)
    );
    assert_eq!(
        parse_local_port_from_frpc_toml("  localPort = 42265  \n"),
        Some(42265)
    );
    assert_eq!(parse_local_port_from_frpc_toml("serverAddr = \"x\"\n"), None);
    assert_eq!(parse_local_port_from_frpc_toml("localPort = 0\n"), None);
}

#[test]
fn tunnel_port_invariant_ok_detects_e2e_desync() {
    // Happy path: live == published == frpc == frozen.
    tunnel_port_invariant_ok(true, Some(44407), Some(44407), Some(44407), Some(44407))
        .expect("aligned ports must pass");
    // Field-report class: live 44407 vs frpc/file 42265.
    let err = tunnel_port_invariant_ok(
        true,
        Some(44407),
        Some(42265),
        Some(42265),
        Some(42265),
    )
    .expect_err("live vs published desync must fail");
    assert!(
        err.contains("44407") && err.contains("42265"),
        "error must name both ports: {err}"
    );
    let err = tunnel_port_invariant_ok(
        true,
        Some(44407),
        Some(44407),
        Some(42265),
        Some(42265),
    )
    .expect_err("frpc localPort desync must fail");
    assert!(
        err.contains("42265") && err.contains("44407"),
        "error must name both ports: {err}"
    );
    // E2E off: only frpc vs frozen matter.
    tunnel_port_invariant_ok(false, Some(1), Some(2), Some(48123), Some(48123))
        .expect("E2E off ignores live/published HTTPS mismatch");
    let err =
        tunnel_port_invariant_ok(false, None, None, Some(1), Some(2)).expect_err("mismatch");
    assert!(err.contains("E2E off"), "got: {err}");
}

/// Pure re-resolve path: when E2E is on and only the published port
/// file is available (no daemon hook), re_resolve returns the file —
/// never invents a free port.
#[test]
fn re_resolve_live_local_port_uses_published_not_invented() {
    with_temp_home(|| {
        super::super::tls::publish_https_port(44407).expect("publish");
        let got = re_resolve_live_local_port(true, 9999, 42265);
        assert_eq!(
            got, 44407,
            "must prefer published live port over stale freeze {got}"
        );
        // E2E off: freeze is sticky (no re-pick).
        assert_eq!(re_resolve_live_local_port(false, 9999, 42265), 42265);
    });
}

/// When the frozen localPort is dead and ensure cannot restore a live
/// listener, probe must NOT thrash (return false). After the daemon hook
/// re-binds (live port differs), probe returns true so frpc rewrites.
#[test]
fn probe_local_desync_false_when_no_live_listener() {
    let port = AtomicU16::new(39999); // almost certainly closed
    let mut ticks = LOCAL_TARGET_PROBE_EVERY;
    // e2e=false → re_resolve returns current; still unreachable → false.
    assert!(
        !probe_local_desync(&mut ticks, &port, false, 18080),
        "must not self-heal when no live alternate port exists"
    );
    assert_eq!(port.load(Ordering::SeqCst), 39999);
}

#[test]
fn local_port_reachable_false_for_closed_port() {
    // Reserved, not listening (a dropped listener's port can be
    // re-bound by a parallel test before we probe it).
    let closed = crate::test_env::ClosedPort::new();
    let port = closed.port();
    assert!(
        !local_port_reachable(port),
        "closed port {port} must report unreachable"
    );
    assert!(!local_port_reachable(0), "port 0 is never reachable");
}

#[test]
fn local_port_reachable_true_for_open_listener() {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
    let port = listener.local_addr().expect("addr").port();
    // Accept in background so connect completes (some platforms need it).
    let _accept = std::thread::spawn(move || {
        let _ = listener.accept();
    });
    assert!(
        local_port_reachable(port),
        "open listener on {port} must report reachable"
    );
}

/// The spec-example frpc TOML (was a print-only `#[ignore]`d
/// diagnostic): pin its proxy shape. Token is a placeholder.
#[test]
fn spec_example_toml_has_the_proxy_shape() {
    let cfg = TunnelConfig {
        token: "REDACTED".to_string(),
        subdomain: "rosson".to_string(),
        local_port: Some(57839), confirmed_label: Some("rosson".to_string()),
        ..Default::default()
    };
    let toml = super::super::render::render_frpc_toml(&cfg, 57839, false);
    assert!(toml.starts_with("# K2SO tunnel connector"), "{toml}");
    let proxy = toml.split("[[proxies]]\n").nth(1).expect("one proxy section");
    assert_eq!(
        proxy,
        "name = \"k2so-rosson\"\n\
             type = \"http\"\n\
             localIP = \"127.0.0.1\"\n\
             localPort = 57839\n\
             subdomain = \"rosson\"\n"
    );
    assert!(toml.contains("[metadatas]\ntoken = \"REDACTED\"\n"), "{toml}");
}

/// REAL-PROCESS, REAL-NETWORK test — gated `#[ignore]` so `cargo
/// test` never spawns a live frpc against the production frps box
/// (which would collide with the parent's live validation). Run
/// manually only, with a real token in ~/.k2/tunnel.json and frpc
/// installed.
#[test]
#[ignore = "spawns a real frpc against the live K2 Connect server"]
fn live_start_stop_roundtrip() {
    let st = start(Some("rosson".to_string()), 57839, &FrpcBinary::Auto)
        .expect("start live tunnel");
    assert!(st.running);
    std::thread::sleep(Duration::from_secs(2));
    assert!(status().running);
    stop("test").expect("stop live tunnel");
    assert!(!status().running);
}
