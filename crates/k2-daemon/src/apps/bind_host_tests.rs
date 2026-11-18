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

/// 0.46.16: the server `k2 app init --template node-api` writes passes K2's
/// health gate and its manifest checks in BOTH lanes, because it listens on
/// exactly the `$HOST` K2 gives: `0.0.0.0` inside a wall (where a
/// `127.0.0.1` bind never answers K2: the bug), `127.0.0.1` on the host's
/// network. A preload records where `listen` was called; a stand-in
/// `@k2/server` takes the place of `k2 app sdk add` (the health route never
/// calls it). Needs `node` (22+), like the template does.
#[test]
fn the_node_api_template_listens_on_host_and_passes_the_health_gate() {
    let node = std::process::Command::new("node")
        .arg("--version")
        .output()
        .unwrap_or_else(|e| panic!("this test needs node (22+) on PATH: {e}"));
    assert!(node.status.success(), "node --version failed");
    let dir = std::env::temp_dir().join(format!("k2-node-api-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let plan = k2_core::app_templates::plan(&dir, "node-api", Some("jobs"), None).expect("plan");
    k2_core::app_templates::apply(&dir, &plan).expect("apply");
    let api = dir.join("k2/api");
    let sdk = api.join("node_modules/@k2/server");
    std::fs::create_dir_all(&sdk).unwrap();
    std::fs::write(sdk.join("package.json"), r#"{"name":"@k2/server","type":"module","exports":"./index.mjs"}"#).unwrap();
    std::fs::write(sdk.join("index.mjs"), "export function fromEnv() { return { asGuest() { throw new Error('stand-in') } } }\n").unwrap();
    let spy = dir.join("spy.mjs");
    std::fs::write(
        &spy,
        "import net from 'node:net'\nconst listen = net.Server.prototype.listen\nnet.Server.prototype.listen = function (...a) {\n  process.stderr.write('K2_LISTEN ' + JSON.stringify(a.filter((x) => typeof x !== 'function')) + '\\n')\n  return listen.apply(this, a)\n}\n",
    )
    .unwrap();
    let manifest: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(api.join("manifest.json")).unwrap()).unwrap();
    let checks = crate::apps::checks::parse(&manifest).expect("the template's checks parse");
    assert!(!checks.is_empty(), "the template ships its health check");

    for host in [IN_CONTAINER, ON_HOST_NETWORK] {
        let port = k2_core::test_env::loopback_test_port();
        let mut child = std::process::Command::new("node")
            .arg("--import")
            .arg(format!("file://{}", spy.display()))
            .arg("server.mjs")
            .current_dir(&api)
            .env("PORT", port.to_string())
            .env(HOST_ENV, host)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("node server.mjs");
        let gate = k2_core::deploy::health::wait_healthy(
            port,
            "/healthz",
            &[],
            Duration::from_millis(100),
            Duration::from_secs(1),
            Duration::from_secs(20),
            || true,
        );
        let outcomes = crate::apps::checks::run("jobs-api", &format!("http://127.0.0.1:{port}"), &checks);
        let _ = child.kill();
        let out = child.wait_with_output().expect("node output");
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(gate.is_ok(), "HOST={host}: the health gate: {:?}; node said: {stderr}", gate.err().map(|g| g.message));
        assert!(outcomes.iter().all(|o| o.ok), "HOST={host}: the manifest checks: {outcomes:?}");
        assert!(
            stderr.contains(&format!("K2_LISTEN [{port},\"{host}\"]")),
            "HOST={host}: the server must listen on exactly $HOST:$PORT; node said: {stderr}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
