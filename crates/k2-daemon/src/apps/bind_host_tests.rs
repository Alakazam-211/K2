use super::*;
use k2_core::deploy::engine::{AppSpec, Isolation};
use std::io::{Read, Write};
use std::time::Duration;

fn spec(host_in_manifest: &str) -> AppSpec {
    AppSpec {
        app_id: "acme-skin".into(),
        service_id: "6f1c0d7e-1111-4222-8333-944455556666".into(),
        workspace_id: "ws-1".into(),
        deployment_id: "d-0123456789ab".into(),
        isolation: Isolation::Container,
        argv: vec!["./bin".into(), "serve".into()],
        env: [("HOST".to_string(), host_in_manifest.to_string()), ("API_BASE".to_string(), "https://x".to_string())].into_iter().collect(),
        memory_limit_bytes: None,
        drain_secs: 30,
        io_max_json: None,
        devices: vec![],
        public_origins: vec![],
        data_dir: "/x/data".into(),
        cache_dir: "/x/cache".into(),
        home_dir: "/x/home".into(),
        log_path: "/x/log".into(),
        wall: Default::default(),
    }
}

/// Guarded and relaxed containers: `HOST=0.0.0.0` inside the namespace (the
/// published port never reaches the container's loopback), whatever the
/// manifest says; `K2_HOST` stays where K2 is.
#[test]
fn a_container_app_binds_every_interface_of_its_namespace() {
    let e = crate::apps::guarded::guarded_env(&spec("127.0.0.1"), None, None);
    assert_eq!(e[HOST_ENV], IN_CONTAINER);
    assert_eq!(e["K2_HOST"], "127.0.0.1", "K2_HOST is the gateway link, not the App's address");
    assert_eq!(e["API_BASE"], "https://x");
}

/// The process engine: on the host's network, so loopback, never what the
/// manifest asked for.
#[test]
fn a_process_app_binds_loopback() {
    let env: std::collections::BTreeMap<String, String> = crate::apps::process_engine::clean_env(&spec("0.0.0.0"), 4321).into_iter().collect();
    assert_eq!(env.get(HOST_ENV).map(String::as_str), Some(ON_HOST_NETWORK));
    assert_eq!(env.get("PORT").map(String::as_str), Some("4321"));
    assert_eq!(env.iter().filter(|(k, _)| k.as_str() == HOST_ENV).count(), 1);
}

/// K2's health probe reaches an App on the host's network that binds
/// exactly `$HOST:$PORT` as given.
#[test]
fn the_probe_reaches_an_app_bound_to_the_host_it_was_given() {
    let env: std::collections::BTreeMap<String, String> = crate::apps::process_engine::clean_env(&spec("0.0.0.0"), 0).into_iter().collect();
    let host = env[HOST_ENV].clone();
    let l = std::net::TcpListener::bind((host.as_str(), 0)).expect("bind $HOST");
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for c in l.incoming().flatten() {
            let mut c = c;
            let mut b = [0u8; 1024];
            let _ = c.read(&mut b);
            let _ = c.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
        }
    });
    let p = k2_core::deploy::health::probe_once(port, "/health", &[], Duration::from_secs(2));
    assert!(p.healthy(), "{p:?}");
}
