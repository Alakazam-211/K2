use super::*;
use crate::test_support::Scratch;

fn row(path: &str, isolation: &str, source: &str) -> AppRow {
    AppRow {
        app_id: "shop".into(),
        service_id: "svc".into(),
        seed_id: Some("shop".into()),
        owner_workspace: "ws".into(),
        path: path.into(),
        source: source.into(),
        folder_path: None,
        live_seed: None,
        live_tree_hash: None,
        live_deployment: None,
        preview_seed: None,
        isolation: isolation.into(),
        memory_limit_bytes: None,
        front_port: None,
        state: "running".into(),
        state_reason: None,
        created_by: "owner".into(),
        created_at: 0,
        updated_at: 0,
        io_max_json: None,
        devices_json: "[]".into(),
        exception_json: None,
        rehost_policy: "auto".into(),
        rehost_window: None,
        rehost_when_no_overlap: "pause".into(),
        warm_deployment: None,
        warm_until: None,
    }
}

/// §4.5: the label is computed from the row, the config and the egress
/// mode; an open label names why and how to go back.
#[test]
fn labels_are_computed_never_claimed() {
    let cfg = AppConfig::default();
    assert_eq!(label(&row("guarded", "container", "seed"), &cfg, "allowlist").text, "Guarded (container)");
    assert_eq!(label(&row("guarded", "vm", "seed"), &cfg, "allowlist").text, "Guarded (VM)");
    assert_eq!(label(&row("guarded", "process", "seed"), &cfg, "allowlist").text, "Preview, unwalled (process)");
    let g = label(&row("guarded", "container", "seed"), &cfg, "allowlist");
    assert_eq!((g.reason, g.back), (None, None));
    let mut cfg2 = AppConfig::default();
    cfg2.ports.push(appcfg::parse_port("9000").unwrap());
    cfg2.ports.push(appcfg::parse_port("5000/udp").unwrap());
    let o = label(&row("open", "container", "folder"), &cfg2, "broad");
    assert_eq!(o.text, "Open (container)");
    assert_eq!(
        o.reason.as_deref(),
        Some("the code is the workspace folder (copied at each start); egress broad (logged); 1 extra port"),
        "UDP isn't a relaxed-container port in 0.46.1"
    );
    assert_eq!(o.back.as_deref(), Some("k2 app config shop --path guarded"));
    let o = label(&row("open", "container", "seed"), &cfg, "allowlist");
    assert_eq!(o.reason.as_deref(), Some("a writable copy of the code"));
    assert_eq!(label(&row("open", "vm", "seed"), &cfg, "allowlist").text, "Open (no engine on this box)");
    // open + process is the owner's process exception.
    let ex = row("open", "process", "seed");
    let l = label(&ex, &cfg, "allowlist");
    assert_eq!(l.text, "Open (process)");
    assert_eq!(l.back.as_deref(), Some("k2 app exception shop off"));
}

fn run(name: &str, cmd: &str, port: u16, cwd: &std::path::Path) -> Run {
    Run { project_id: "ws".into(), name: name.into(), cmd: cmd.into(), cwd: cwd.to_path_buf(), port, no_tunnel: true }
}

/// K5: a new `--cmd` label must fit an App id and its port a relaxed
/// container, refused before anything is written.
#[test]
fn a_cmd_run_is_checked_before_anything_is_written() {
    let d = Scratch::new();
    assert!(check_run(&run("shop", "node server.js", 3000, d.path())).is_ok());
    assert!(check_run(&run("shop", "node server.js", 0, d.path())).is_ok(), "no --port: $PORT");
    for (name, cmd, port, code) in [
        ("a--b", "x", 3000, codes::APP_ID_INVALID),
        ("this-label-is-far-too-long-for-an-app-id", "x", 3000, codes::APP_ID_INVALID),
        ("shop", "  ", 3000, codes::BAD_REQUEST),
        ("shop", "x", 80, codes::BAD_REQUEST),
        ("shop", "x", 8730, codes::BAD_REQUEST),
        ("shop", "x", 38_700, codes::BAD_REQUEST),
    ] {
        let e = check_run(&run(name, cmd, port, d.path())).unwrap_err();
        assert_eq!(e.code, code, "{name} {cmd:?} {port}: {}", e.hint);
    }
    let e = check_run(&run("shop", "x", 3000, &d.path().join("missing"))).unwrap_err();
    assert!(e.hint.contains("is not a folder"), "{}", e.hint);
}

#[test]
fn relaxed_handles_are_routed_by_their_prefix() {
    let h = RunHandle { app_id: "shop".into(), deployment_id: "d-0123456789ab".into(), port: 41000, pid: None, engine_ref: "relaxed:k2app-shop--d-0123456789ab.service".into() };
    assert!(owns(&h));
    assert!(!super::super::guarded::owns(&h), "never mistaken for a guarded instance");
    let g = RunHandle { engine_ref: "podman:k2app-shop--d-0123456789ab.service".into(), ..h };
    assert!(!owns(&g));
    let caps = engine().caps();
    assert!(caps.egress_gate && caps.separate_user && !caps.read_only_seed);
    assert_eq!(engine().isolation(), Isolation::Container);
    assert!(engine().adopt().unwrap().is_empty(), "the guarded engine adopts relaxed instances");
}

/// Off Linux (dev boxes, macOS) the open path is never ready: `k2 publish
/// run --cmd` keeps 0.46.0's behaviour there.
#[cfg(not(target_os = "linux"))]
#[test]
fn the_open_path_is_linux_only() {
    assert!(ready().unwrap_err().contains("Linux"));
}
