//! The voice lane against a fake frpc (a shell script that logs the
//! registration line frps would make it log, for whichever config it got).

use super::*;
use crate::tunnel::config::{self, TunnelConfig};
use crate::tunnel::connector::{self, FrpcBinary};
use crate::tunnel::test_support::with_temp_home;
use std::sync::atomic::AtomicU64;

#[test]
fn restarts_are_immediate_until_frpc_voice_keeps_dying() {
    let mut p = RestartPolicy::default();
    let t0 = Instant::now();
    let s = Duration::from_secs;
    assert_eq!(p.on_exit(t0), Duration::ZERO, "a blip restarts at once");
    assert_eq!(p.on_exit(t0 + s(1)), Duration::ZERO);
    assert_eq!(p.on_exit(t0 + s(2)), s(2), "the third exit inside 60 s waits");
    assert_eq!(p.on_exit(t0 + s(3)), s(4));
    assert_eq!(p.on_exit(t0 + s(4)), s(8));
    assert_eq!(p.on_exit(t0 + s(5)), s(16));
    assert_eq!(p.on_exit(t0 + s(6)), s(30), "capped at 30 s");
    assert_eq!(p.on_exit(t0 + s(7)), s(30));
    // A minute of quiet forgets them all.
    let mut p = RestartPolicy::default();
    p.on_exit(t0);
    p.on_exit(t0 + s(1));
    assert_eq!(p.on_exit(t0 + s(200)), Duration::ZERO);
}

/// The fake frpc: registers the proxy its config names, then stays up.
/// Its argv stays `<sh> <…/frpc> -c <cfg>` (never `exec`s), as the adopt
/// check expects.
fn fake_frpc(dir: &Path) -> PathBuf {
    std::fs::create_dir_all(dir).expect("mk dir");
    let script = dir.join("frpc");
    crate::test_env::write_executable(
        &script,
        "#!/bin/sh\n\
         case \"$2\" in\n\
           *frpc-voice.toml) echo '[I] [proxy_wrapper.go:204] [x] [k2voice-rosson] start proxy success' ;;\n\
           *) echo '[I] [proxy_wrapper.go:204] [x] [k2so-rosson] start proxy success' ;;\n\
         esac\n\
         while :; do sleep 1; done\n",
    );
    script
}

/// An E2E tunnel for `rosson` running the fake frpc.
fn start_tunnel(script: &Path) {
    config::save(&TunnelConfig {
        server_addr: "9.9.9.9".to_string(),
        server_port: 7009,
        token: "tok_voice".to_string(),
        subdomain: "rosson".to_string(),
        device_id: Some("dev-1".to_string()),
        ..Default::default()
    })
    .expect("seed config");
    crate::tunnel::tls::publish_https_port(48217).expect("publish https port");
    connector::start(None, 57839, &FrpcBinary::Explicit(script.to_path_buf())).expect("start");
}

fn wait_until(what: &str, secs: u64, mut ok: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while !ok() {
        assert!(Instant::now() < deadline, "timed out waiting for: {what}\nstatus: {:?}", status());
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn voice_pid() -> i32 {
    frpc_proc::read_pid_record(&voice_config_path()).expect("frpc-voice's pid record").pid
}

static HEARD: AtomicU64 = AtomicU64::new(0);
fn heard(up: bool) {
    if up {
        HEARD.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn with_the_preview_off_nothing_starts() {
    with_temp_home(|| {
        let _lease = crate::test_env::EnvVar::set(crate::tunnel::lease::LEASE_ENV_VAR, "off");
        let _env = crate::test_env::EnvVar::remove(PREVIEW_ENV);
        let dir = crate::test_env::unique_temp_path("voice-lane-off");
        start_tunnel(&fake_frpc(&dir));
        let err = arm().expect_err("arming needs the preview");
        assert!(err.contains("preview is off"), "{err}");
        std::thread::sleep(Duration::from_millis(600));
        assert!(!voice_config_path().exists(), "no frpc-voice config without the preview");
        assert!(frpc_proc::read_pid_record(&voice_config_path()).is_none());
        let st = status();
        assert_eq!((st.state, st.armed, st.preview, st.up), ("off", false, false, false));
        let main = std::fs::read_to_string(connector::frpc_config_path()).expect("main config");
        assert!(!main.contains("turn.") && !main.contains("k2voice"), "the main frpc never carries a turn proxy\n{main}");
        let _ = std::fs::remove_dir_all(&dir);
    });
}

/// S9b end to end against the fake frpc: armed, the lane follows the
/// tunnel's relay with the M4 config, comes up, restarts at once when
/// frpc-voice dies, follows a relay move, and stops with the tunnel.
#[test]
fn the_lane_follows_the_tunnel_restarts_at_once_and_stops_with_it() {
    with_temp_home(|| {
        let _lease = crate::test_env::EnvVar::set(crate::tunnel::lease::LEASE_ENV_VAR, "off");
        let dir = crate::test_env::unique_temp_path("voice-lane-run");
        let script = fake_frpc(&dir);
        start_tunnel(&script);
        set_listener(heard);
        let heard_before = HEARD.load(Ordering::SeqCst);
        crate::voice_rooms::set_tunnel_enabled(true);
        arm().expect("arm");
        wait_until("the voice lane comes up", 10, is_up);
        assert!(HEARD.load(Ordering::SeqCst) > heard_before, "the listener heard `up`");

        let toml = std::fs::read_to_string(voice_config_path()).expect("frpc-voice.toml");
        for want in [
            "serverAddr = \"9.9.9.9\"",
            "serverPort = 7009",
            "token = \"tok_voice\"",
            "device_id = \"dev-1\"",
            "role = \"voice\"",
            "name = \"k2voice-rosson\"",
            "customDomains = [\"turn.rosson.k2.dev\"]",
            "localPort = 5349",
            "transport.proxyProtocolVersion = \"v2\"",
        ] {
            assert!(toml.contains(want), "missing {want:?}\n{toml}");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(voice_config_path()).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "the config holds the token");
        }
        let main_pid = frpc_proc::read_pid_record(&connector::frpc_config_path()).expect("main record").pid;
        let pid = voice_pid();
        assert_ne!(pid, main_pid, "two frpcs, two records");
        let st = status();
        assert_eq!(st.state, "up");
        assert_eq!(st.proxy_name.as_deref(), Some("k2voice-rosson"));
        assert_eq!(st.turn_host.as_deref(), Some("turn.rosson.k2.dev"));
        assert_eq!((st.server_addr.as_deref(), st.server_port), (Some("9.9.9.9"), Some(7009)));
        let log = std::fs::read_to_string(voice_log_path()).expect("frpc-voice.log");
        assert!(log.contains("[k2voice-rosson] start proxy success"), "{log}");
        let main_log = std::fs::read_to_string(connector::frpc_log_path()).expect("frpc.log");
        assert!(!main_log.contains("k2voice"), "frpc-voice writes its own log\n{main_log}");

        // frpc-voice dies: restarted at once (no backoff for a blip).
        #[cfg(unix)]
        {
            unsafe { libc::kill(pid, libc::SIGKILL) };
            wait_until("a new frpc-voice", 3, || {
                frpc_proc::read_pid_record(&voice_config_path()).is_some_and(|r| r.pid != pid && frpc_proc::pid_alive(r.pid))
            });
            wait_until("up again", 5, is_up);
            assert!(frpc_proc::pid_alive(main_pid), "the main frpc is untouched");
        }

        // The main supervisor moves relays: the lane follows at once.
        let before = voice_pid();
        connector::set_current_relay_for_test(RelayEndpoint { host: "5.6.7.8".to_string(), port: 7001 });
        kick();
        wait_until("frpc-voice on the new relay", 5, || {
            std::fs::read_to_string(voice_config_path()).is_ok_and(|t| t.contains("serverAddr = \"5.6.7.8\""))
                && frpc_proc::read_pid_record(&voice_config_path()).is_some_and(|r| r.pid != before)
        });
        wait_until("up on the new relay", 5, is_up);
        assert!(!frpc_proc::pid_alive(before), "the old relay's frpc-voice is gone");

        // The tunnel stops: so does the lane.
        let last = voice_pid();
        connector::stop("test").expect("stop");
        wait_until("frpc-voice killed with the tunnel", 5, || !frpc_proc::pid_alive(last));
        assert!(!is_up());
        assert!(frpc_proc::read_pid_record(&voice_config_path()).is_none(), "nothing left to adopt");
        std::thread::sleep(Duration::from_millis(600));
        assert!(frpc_proc::read_pid_record(&voice_config_path()).is_none(), "no respawn without the tunnel");
        assert_eq!(status().state, "waiting_for_tunnel", "still armed: back with the next start");

        // Disarm drops the config (it holds the token).
        start_tunnel(&script);
        wait_until("up with the restarted tunnel", 10, is_up);
        let p = voice_pid();
        disarm("test");
        wait_until("frpc-voice killed on disarm", 5, || !frpc_proc::pid_alive(p));
        assert!(!voice_config_path().exists());
        assert_eq!(status().state, "off");
        let _ = std::fs::remove_dir_all(&dir);
    });
}

fn confirms_rosson(_: &TunnelConfig) -> Result<String, String> {
    Ok("rosson".to_string())
}

fn confirms_nothing(_: &TunnelConfig) -> Result<String, String> {
    Err("the fake control plane is unreachable".to_string())
}

fn set_source(f: fn(&TunnelConfig) -> Result<String, String>) {
    *crate::tunnel::confirmed_label::TEST_SOURCE.lock().unwrap() = Some(f);
}

/// Relay name binding (0.46.8): both frpcs are named from the label the
/// control plane confirmed, whatever tunnel.json requests; with none, the
/// main frpc renders no proxy (never `k2so-daemon`) and the tunnel restarts
/// with its proxy once the control plane answers.
#[test]
fn both_frpcs_are_named_from_the_confirmed_label_and_none_means_no_proxy() {
    with_temp_home(|| {
        let _lease = crate::test_env::EnvVar::set(crate::tunnel::lease::LEASE_ENV_VAR, "off");
        let _naming = crate::test_env::EnvVar::remove(crate::tunnel::confirmed_label::PROXY_NAME_ENV);
        let dir = crate::test_env::unique_temp_path("voice-lane-names");
        let script = fake_frpc(&dir);
        set_source(confirms_rosson);
        config::save(&TunnelConfig {
            server_addr: "9.9.9.9".to_string(),
            server_port: 7009,
            token: "tok_names".to_string(),
            subdomain: "staging.rosson".to_string(),
            ..Default::default()
        })
        .expect("seed config");
        crate::tunnel::tls::publish_https_port(48217).expect("publish https port");
        connector::start(None, 57839, &FrpcBinary::Explicit(script.clone())).expect("start");
        let main = std::fs::read_to_string(connector::frpc_config_path()).expect("frpc.toml");
        assert!(main.contains("name = \"k2so-rosson\""), "{main}");
        assert!(main.contains("subdomain = \"staging.rosson\""), "the request still goes out\n{main}");
        assert!(!main.contains("user ="), "frp `user` stays empty\n{main}");
        wait_until("the main proxy registers", 10, connector::proxy_registered);
        crate::voice_rooms::set_tunnel_enabled(true);
        arm().expect("arm");
        wait_until("voice up", 10, is_up);
        let voice = std::fs::read_to_string(voice_config_path()).expect("frpc-voice.toml");
        assert!(voice.contains("name = \"k2voice-rosson\"") && voice.contains("turn.rosson.k2.dev"), "{voice}");
        assert!(!voice.contains("user ="), "{voice}");
        connector::stop("test").expect("stop");
        disarm("test");

        // No confirmation (another token, control plane unreachable): no
        // proxy, no voice lane; then the control plane answers.
        set_source(confirms_nothing);
        config::save(&TunnelConfig {
            server_addr: "9.9.9.9".to_string(),
            server_port: 7009,
            token: "tok_unconfirmed".to_string(),
            subdomain: String::new(),
            ..Default::default()
        })
        .expect("seed config");
        connector::start(None, 57839, &FrpcBinary::Explicit(script.clone())).expect("start without a label");
        let main = std::fs::read_to_string(connector::frpc_config_path()).expect("frpc.toml");
        assert!(!main.contains("[[proxies]]") && !main.contains("k2so-"), "{main}");
        assert!(connector::status().running);
        arm().expect("arm");
        std::thread::sleep(Duration::from_millis(600));
        assert!(!voice_config_path().exists(), "no confirmed label, no voice lane");
        set_source(confirms_rosson);
        wait_until("the retry restarts the tunnel with its proxy", 10, || {
            std::fs::read_to_string(connector::frpc_config_path()).is_ok_and(|t| t.contains("name = \"k2so-rosson\""))
        });
        wait_until("registered", 10, connector::proxy_registered);
        wait_until("voice up after the confirmation", 10, is_up);
        let _ = std::fs::remove_dir_all(&dir);
    });
}

/// Runtime stage H for the voice lane: a hand-off leaves frpc-voice
/// running, and the next daemon adopts it (same pid, registration replayed
/// from its log), so a daemon update doesn't end calls.
#[test]
fn a_restarted_daemon_adopts_the_running_frpc_voice() {
    with_temp_home(|| {
        let _lease = crate::test_env::EnvVar::set(crate::tunnel::lease::LEASE_ENV_VAR, "off");
        let dir = crate::test_env::unique_temp_path("voice-lane-adopt");
        let script = fake_frpc(&dir);
        start_tunnel(&script);
        crate::voice_rooms::set_tunnel_enabled(true);
        arm().expect("arm");
        wait_until("up", 10, is_up);
        let pid = voice_pid();

        connector::detach_for_handoff().expect("a live frpc to hand off");
        wait_until("the voice supervisor lets go", 5, || !SUPERVISING.load(Ordering::SeqCst));
        assert!(frpc_proc::pid_alive(pid), "hand-off leaves frpc-voice running");
        // The main supervisor polls every second: give it time to let go.
        std::thread::sleep(Duration::from_millis(2100));
        connector::forget_after_handoff_for_test();
        forget_for_test();
        assert!(frpc_proc::pid_alive(pid));

        start_tunnel(&script);
        arm().expect("arm in the new daemon");
        wait_until("adopted and up", 10, is_up);
        assert_eq!(voice_pid(), pid, "the same frpc-voice: no respawn");
        let st = status();
        assert!(st.adopted, "{st:?}");
        assert_eq!(st.pid, Some(pid));
        let log = std::fs::read_to_string(voice_log_path()).expect("log");
        assert_eq!(log.matches("start proxy success").count(), 1, "one frpc-voice ever ran\n{log}");

        connector::stop("test").expect("stop");
        wait_until("the adopted frpc-voice is killed with the tunnel", 5, || !frpc_proc::pid_alive(pid));
        let _ = std::fs::remove_dir_all(&dir);
    });
}
