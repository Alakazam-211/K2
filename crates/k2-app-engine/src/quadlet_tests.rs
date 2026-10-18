use super::*;

static RES: Resources = Resources { memory_mib: Some(512), tasks: 512, nofile: 4096, cpu_weight: None, log_bytes: 50 << 20 };
static RES_NOCAP: Resources = Resources { memory_mib: None, tasks: 256, nofile: 8192, cpu_weight: Some(200), log_bytes: 8 << 20 };

fn base_render<'a>(app: &'a AppId, argv: &'a [String], secrets: &'a [SecretMount]) -> Render<'a> {
    Render {
        app,
        deployment: "d-0123456789ab",
        wall: Wall::Guarded,
        image: "localhost/k2-base/node24:ec857ea6bb60",
        runtime: "/var/lib/k2app/engine/oci/crun",
        base: 65_537,
        seed_dir: "/var/lib/k2app/seeds/aaaa",
        data_dir: "/var/lib/k2app/apps/nsi-skin/data",
        cache_dir: "/var/lib/k2app/apps/nsi-skin/cache",
        env_file: "/var/lib/k2app/engine/run/nsi-skin/d-0123456789ab.env",
        seccomp: "/var/lib/k2app/engine/seccomp-k2-v1.json",
        gw_port: 38_601,
        px_port: 38_901,
        port: 41_234,
        app_port: 41_234,
        extra_ports: &[],
        argv,
        resources: &RES,
        drain_secs: 30,
        secrets,
        pass_dir: Some("/run/k2app/pass/nsi-skin"),
        io: &IO,
        box_forwards: &[],
        db_port: None,
    }
}

static IO: k2_root_proto::apps::IoMax = k2_root_proto::apps::IoMax {
    rbps: None,
    wbps: Some(41_943_040),
    riops: None,
    wiops: Some(2000),
};

#[test]
fn the_floor_is_in_every_unit() {
    let app = AppId::parse("nsi-skin").unwrap();
    let argv = vec!["node".to_string(), "server.mjs".to_string()];
    let secrets = vec![
        SecretMount::Env { secret: "k2-nsi-skin--d-0123456789ab--0".into(), var: "STRIPE_KEY".into() },
        SecretMount::File { secret: "k2-nsi-skin--d-0123456789ab--1".into(), target: "/run/secrets/db.json".into() },
    ];
    let u = render(&base_render(&app, &argv, &secrets));
    for want in [
        "\nContainerName=k2app-nsi-skin--d-0123456789ab\n",
        "\nPull=never\n",
        "\nGlobalArgs=--runtime=/var/lib/k2app/engine/oci/crun\n",
        "\nLabel=k2.wall=guarded\n",
        "\nUIDMap=0:65537:65536\n",
        "\nGIDMap=0:65537:65536\n",
        "\nDropCapability=all\n",
        "\nNoNewPrivileges=true\n",
        "\nSeccompProfile=/var/lib/k2app/engine/seccomp-k2-v1.json\n",
        "\nReadOnly=true\n",
        "\nTmpfs=/tmp:rw,nosuid,nodev,noexec,size=256m,mode=1777\n",
        "\nVolume=/var/lib/k2app/seeds/aaaa:/app:ro\n",
        "\nNetwork=pasta:-T,8730:38601,-T,38901\n",
        "\nVolume=/run/k2app/pass/nsi-skin:/run/k2:ro\n",
        "\nPublishPort=127.0.0.1:41234:41234\n",
        "\nRunInit=true\n",
        "\nPidsLimit=512\n",
        "\nUlimit=nofile=4096:4096\n",
        "\nPodmanArgs=--memory=512m --memory-swap=512m --log-opt=max-size=52428800\n",
        "\nLogDriver=k8s-file\n",
        "\nStopTimeout=40\n",
        "\nSecret=k2-nsi-skin--d-0123456789ab--0,type=env,target=STRIPE_KEY\n",
        "\nSecret=k2-nsi-skin--d-0123456789ab--1,type=mount,target=/run/secrets/db.json,mode=0400,uid=0,gid=0\n",
        "\nExec=\"node\" \"server.mjs\"\n",
        "\nSlice=k2apps-nsi_skin.slice\n",
        "\nRestart=no\n",
        "\nTimeoutStopSec=60\n",
        "\nOOMPolicy=kill\n",
        "\nMemorySwapMax=0\n",
        "\nIOWriteBandwidthMax=/var/lib/k2app 41943040\n",
        "\nIOWriteIOPSMax=/var/lib/k2app 2000\n",
    ] {
        assert!(u.contains(want), "missing {want:?} in\n{u}");
    }
    // Never: host namespaces, privilege, devices, host loopback, sockets.
    for never in [
        "--privileged",
        "Network=host",
        "AddDevice",
        "map-host-loopback",
        "AddCapability",
        "docker.sock",
        "podman.sock",
        "PodmanArgs=--userns",
        "HealthCmd",
        "Environment=",
        "passthrough",
        ":/app:rw",
    ] {
        assert!(!u.contains(never), "{never} in\n{u}");
    }
    assert!(!u.contains("[Install]"), "never started at boot by systemd itself (the daemon adopts)");
    assert!(!u.contains("target=/run/k2/pass"), "the pass is a mounted file, never a Podman secret (it rotates)");
    let mut r = base_render(&app, &argv, &secrets);
    r.pass_dir = None;
    assert!(!render(&r).contains(":/run/k2:ro"), "no pass, no mount");
}

/// L1 (Rosson, Q4): no memory cap unless the owner set one; then no
/// `--memory` at all (the Apps slice is the bound), swap still off.
#[test]
fn no_memory_cap_means_no_memory_flag() {
    let app = AppId::parse("jobs").unwrap();
    let argv = vec!["node".to_string()];
    let mut r = base_render(&app, &argv, &[]);
    r.resources = &RES_NOCAP;
    let u = render(&r);
    assert!(!u.contains("--memory"), "{u}");
    assert!(u.contains("\nPodmanArgs=--log-opt=max-size=8388608\n"), "{u}");
    assert!(u.contains("\nMemorySwapMax=0\n") && u.contains("\nOOMPolicy=kill\n"), "{u}");
    assert!(u.contains("\nPidsLimit=256\n") && u.contains("\nUlimit=nofile=8192:8192\n"), "{u}");
    assert_eq!(
        slice_unit(&app, &RES_NOCAP).as_deref(),
        Some("# K2 App runtime (k2-app-engine): App jobs's slice. Rendered; edits are overwritten.\n[Slice]\nCPUWeight=200\n")
    );
    assert_eq!(slice_unit(&app, &RES), None, "no weight, no slice file");
}

/// S13: the relaxed container keeps the floor except the read-only root
/// and code, the noexec /tmp and the one port.
#[test]
fn the_relaxed_container_keeps_the_floor_but_the_listed_four() {
    let app = AppId::parse("shop").unwrap();
    let argv = vec!["sh".to_string(), "-c".to_string(), "npm start".to_string()];
    let fwd = [k2_root_proto::engine::BoxForward { app_port: 8470, host_port: 8470 }];
    let mut r = base_render(&app, &argv, &[]);
    r.wall = Wall::Relaxed;
    r.seed_dir = "/var/lib/k2app/apps/shop/work/d-0123456789ab";
    r.app_port = 3000;
    r.extra_ports = &[9000, 9001];
    r.box_forwards = &fwd;
    r.db_port = Some(5432);
    let u = render(&r);
    for want in [
        "\nLabel=k2.wall=relaxed\n",
        "\nVolume=/var/lib/k2app/apps/shop/work/d-0123456789ab:/app:rw\n",
        "\nTmpfs=/tmp:rw,nosuid,nodev,size=1g,mode=1777\n",
        "\nPublishPort=127.0.0.1:41234:3000\n",
        "\nPublishPort=127.0.0.1:9000:9000\n",
        "\nPublishPort=127.0.0.1:9001:9001\n",
        "\nNetwork=pasta:-T,8730:38601,-T,38901,-T,8470:8470,-T,5432:5432\n",
        // The floor stays.
        "\nDropCapability=all\n",
        "\nNoNewPrivileges=true\n",
        "\nSeccompProfile=/var/lib/k2app/engine/seccomp-k2-v1.json\n",
        "\nUIDMap=0:65537:65536\n",
        "\nGlobalArgs=--runtime=/var/lib/k2app/engine/oci/crun\n",
        "\nOOMPolicy=kill\n",
    ] {
        assert!(u.contains(want), "missing {want:?} in\n{u}");
    }
    for never in [
        "ReadOnly=true",
        ":/app:ro",
        "noexec",
        "Network=host",
        "map-host-loopback",
        "AddCapability",
        "--privileged",
        "PublishPort=0.0.0.0",
        "PublishPort=9000",
    ] {
        assert!(!u.contains(never), "{never} in\n{u}");
    }
}

#[test]
fn a_build_runs_under_the_floor_with_only_the_proxy() {
    let app = AppId::parse("hearth").unwrap();
    let argv = vec!["npm".to_string(), "ci".to_string()];
    let b = BuildRender {
        app: &app,
        build: "b-0123456789ab",
        image: "localhost/k2-base/node24:ec857ea6bb60",
        runtime: "/var/lib/k2app/engine/oci/crun",
        base: 131_073,
        work_dir: "/var/lib/k2app/apps/hearth/build/b-0123456789ab",
        env_file: "/var/lib/k2app/engine/builds/hearth/b-0123456789ab.env",
        seccomp: "/var/lib/k2app/engine/seccomp-k2-v1.json",
        px_port: 38_902,
        argv: &argv,
        resources: &RES_NOCAP,
        log_file: "/var/lib/k2app/engine/builds/hearth/b-0123456789ab.log",
        timeout_secs: 900,
    };
    let u = render_build(&b);
    assert_eq!(build_stem(&app, "b-0123456789ab"), "k2app-hearth--b-0123456789ab");
    for want in [
        "\nContainerName=k2app-hearth--b-0123456789ab\n",
        "\nUIDMap=0:131073:65536\n",
        "\nDropCapability=all\n",
        "\nNoNewPrivileges=true\n",
        "\nSeccompProfile=/var/lib/k2app/engine/seccomp-k2-v1.json\n",
        "\nReadOnly=true\n",
        "\nVolume=/var/lib/k2app/apps/hearth/build/b-0123456789ab:/app:rw\n",
        "\nNetwork=pasta:-T,38902\n",
        "\nExec=\"npm\" \"ci\"\n",
        "\nSlice=k2apps-hearth.slice\n",
        "\nRuntimeMaxSec=900\n",
        "\nStandardOutput=truncate:/var/lib/k2app/engine/builds/hearth/b-0123456789ab.log\n",
        "\nLogDriver=passthrough\n",
    ] {
        assert!(u.contains(want), "missing {want:?} in\n{u}");
    }
    for never in ["8730", "PublishPort", "Secret=", "/run/k2", "Network=host", "AddCapability", "--privileged", "--memory"] {
        assert!(!u.contains(never), "{never} in\n{u}");
    }
}

#[test]
fn box_forwards_are_extra_pasta_forwards() {
    let app = AppId::parse("hearth").unwrap();
    let argv = vec!["node".to_string()];
    let fwd = [
        k2_root_proto::engine::BoxForward { app_port: 8470, host_port: 8470 },
        k2_root_proto::engine::BoxForward { app_port: 9000, host_port: 7880 },
    ];
    let mut r = base_render(&app, &argv, &[]);
    r.box_forwards = &fwd;
    let u = render(&r);
    assert!(u.contains("\nNetwork=pasta:-T,8730:38601,-T,38901,-T,8470:8470,-T,9000:7880\n"), "{u}");
    for never in ["map-host-loopback", "Network=host", "--map-gw"] {
        assert!(!u.contains(never), "{never} in\n{u}");
    }
}

#[test]
fn exec_words_are_quoted_for_systemd() {
    assert_eq!(sd_quote("a b"), "\"a b\"");
    assert_eq!(sd_quote("$HOME"), "\"$$HOME\"");
    assert_eq!(sd_quote("100%"), "\"100%%\"");
    assert_eq!(sd_quote("say \"hi\""), "\"say \\\"hi\\\"\"");
    assert_eq!(sd_quote("C:\\x"), "\"C:\\\\x\"");
    let app = AppId::parse("jobs").unwrap();
    let argv = vec!["sh".to_string(), "-c".to_string(), "echo $X %h".to_string()];
    let u = render(&base_render(&app, &argv, &[]));
    assert!(u.contains("\nExec=\"sh\" \"-c\" \"echo $$X %%h\"\n"), "{u}");
}

#[test]
fn stop_timeout_covers_the_drain() {
    assert!(stop_timeout(30) >= 30);
    assert_eq!(stop_timeout(0), 10);
}
