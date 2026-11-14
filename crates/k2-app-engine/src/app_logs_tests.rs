use super::*;
use k2_root_proto::engine::{IoMax, Request, Resources, StartSpec, Wall};

fn app() -> AppId {
    AppId::parse("acme-skin").unwrap()
}

#[test]
fn k8s_lines_become_the_apps_text() {
    let raw = "2026-01-02T03:04:05.000000001+00:00 stdout F listening addr=0.0.0.0:41234\n\
               2026-01-02T03:04:05.000000002+00:00 stderr P open /run/secrets/\n\
               2026-01-02T03:04:05.000000003+00:00 stderr F db.json: no such file\n\
               2026-01-02T03:04:05.000000004+00:00 stdout F \n\
               not a k8s line\n";
    assert_eq!(k8s_text(raw), "listening addr=0.0.0.0:41234\nopen /run/secrets/db.json: no such file\n\nnot a k8s line\n");
}

#[test]
fn tail_keeps_the_last_lines_and_the_last_bytes() {
    assert_eq!(tail("a\nb\nc\n", 2, 100), "b\nc\n");
    assert_eq!(tail("a\nb\nc\n", 10, 100), "a\nb\nc\n");
    assert_eq!(tail("", 10, 100), "");
    assert_eq!(tail("aaaa\nbbbb\n", 10, 5), "bbbb\n", "the end, never the start");
    // A cut never lands inside a character.
    assert_eq!(tail("ééé\n", 10, 4), "é\n");
}

#[test]
fn prepare_makes_the_folder_and_keeps_the_newest_logs() {
    let d = tempfile::tempdir().unwrap();
    let state = d.path();
    let p = prepare(state, &app(), "d-000000000000").unwrap();
    assert_eq!(p, state.join("logs/acme-skin/d-000000000000.log"));
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(dir(state, &app())).unwrap().permissions().mode() & 0o777, 0o700);
    // KEEP_PER_APP + 4 older logs, oldest first.
    let n = KEEP_PER_APP + 4;
    let base = std::time::SystemTime::now() - std::time::Duration::from_secs(10_000);
    for i in 0..n {
        let f = dir(state, &app()).join(format!("d-{i:012x}.log"));
        std::fs::write(&f, "x").unwrap();
        let t = base + std::time::Duration::from_secs(i as u64 * 10);
        std::fs::File::options().write(true).open(&f).unwrap().set_modified(t).unwrap();
    }
    // The new deployment's own (absent yet) plus the newest KEEP_PER_APP - 1.
    prepare(state, &app(), "d-ffffffffffff").unwrap();
    let mut left: Vec<String> =
        std::fs::read_dir(dir(state, &app())).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
    left.sort();
    assert_eq!(left.len(), KEEP_PER_APP - 1, "{left:?}");
    assert!(!left.contains(&"d-000000000000.log".to_string()), "the oldest goes: {left:?}");
    assert!(left.contains(&format!("d-{:012x}.log", n - 1)), "the newest stays: {left:?}");
    remove_app(state, &app());
    assert!(!dir(state, &app()).exists());
}

/// A start that exited before it was healthy: its container is gone (Quadlet
/// removes it), so there is no `podman logs` to ask, but the deployment's
/// own file still answers the `logs` verb, stderr included.
#[test]
fn logs_verb_reads_the_kept_file_after_the_container_is_gone() {
    let d = tempfile::tempdir().unwrap();
    let e = crate::server::Engine::new(d.path().to_path_buf(), "/tmp".into());
    let p = prepare(&d.path().join("engine"), &app(), "d-0123456789ab").unwrap();
    std::fs::write(
        &p,
        "2026-01-02T03:04:05.1+00:00 stdout F starting\n\
         2026-01-02T03:04:05.2+00:00 stderr F fatal: open /run/secrets/db.json: no such file or directory\n",
    )
    .unwrap();
    let v = e
        .handle(Request::Logs { app: "acme-skin".into(), deployment: "d-0123456789ab".into(), lines: 200 }, &mut std::io::empty())
        .expect("the logs verb answers from the kept file");
    assert_eq!(v["text"], "starting\nfatal: open /run/secrets/db.json: no such file or directory\n");
    let v = e.handle(Request::Logs { app: "acme-skin".into(), deployment: "d-0123456789ab".into(), lines: 1 }, &mut std::io::empty()).unwrap();
    assert_eq!(v["text"], "fatal: open /run/secrets/db.json: no such file or directory\n");
}

fn start_spec(env: &[(&str, &str)]) -> StartSpec {
    StartSpec {
        app: "acme-skin".into(),
        deployment: "d-0123456789ab".into(),
        seed: "a".repeat(64),
        image: "localhost/k2-base/node24:ec857ea6bb60".into(),
        wall: Wall::Guarded,
        port: 41234,
        app_port: None,
        extra_ports: vec![],
        argv: vec!["./bin".into(), "serve".into()],
        env: env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        resources: Resources::k2_default(),
        drain_secs: 30,
        secrets: vec![],
        pass_hex: None,
        io: IoMax::k2_default(),
        box_forwards: vec![],
        db_port: None,
    }
}

/// The container's App binds `$HOST:$PORT`: `0.0.0.0` (its published port
/// never reaches the container's own loopback), whatever the daemon sent.
#[test]
fn a_container_binds_every_interface_of_its_own_namespace() {
    let env = crate::server::container_env(&start_spec(&[("HOST", "127.0.0.1"), ("API_BASE", "https://x")]), 38_901);
    assert_eq!(env["HOST"], "0.0.0.0");
    assert_eq!(env["PORT"], "41234");
    assert_eq!(env["API_BASE"], "https://x");
    let mut relaxed = start_spec(&[]);
    relaxed.wall = Wall::Relaxed;
    relaxed.app_port = Some(3000);
    let env = crate::server::container_env(&relaxed, 38_901);
    assert_eq!((env["HOST"].as_str(), env["PORT"].as_str()), ("0.0.0.0", "3000"), "the relaxed App's own port inside");
}

/// The process exception's unit is on the host's network: loopback only.
#[test]
fn the_process_exception_binds_loopback() {
    let s = k2_root_proto::engine::ProcessSpec {
        app: "gpu-job".into(),
        deployment: "d-0123456789ab".into(),
        seed: "c".repeat(64),
        port: 41_000,
        argv: vec!["python3".into(), "serve.py".into()],
        env: [("HOST".to_string(), "0.0.0.0".to_string())].into(),
        memory_mib: 2048,
        secrets: vec![],
        pass_hex: None,
    };
    let secret_env: std::collections::BTreeMap<String, String> = [("HOST".to_string(), "0.0.0.0".to_string())].into();
    let env = crate::process::resolve_env(&s, 38_601, &secret_env, false, false);
    assert_eq!(env["HOST"], "127.0.0.1", "never the LAN, whatever the spec or a secret says");
    assert_eq!(env["PORT"], "41000");
}
