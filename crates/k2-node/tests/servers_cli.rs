//! `k2-node servers …` at the machine (prd-compute-shared-queue-v1 §9).
//! Temp dirs only; the root-only verbs are checked to refuse without sudo
//! (the tests never run as root).
// pending k2-node Windows port: k2-node (the compute node runtime) is Unix-only
// until its Windows port (Phase 3: Job Objects instead of uids and cgroups).
#![cfg(unix)]

use std::process::Command;

use k2_node::admin::servers;
use k2_node_proto::share::{Network, ServersFile, ShareLimits, ShareState, Trust};

fn run(args: &[&str]) -> (bool, String, String) {
    let o = Command::new(env!("CARGO_BIN_EXE_k2-node")).args(args).env_clear().env("PATH", "/usr/bin:/bin").output().unwrap();
    (o.status.success(), String::from_utf8_lossy(&o.stdout).into_owned(), String::from_utf8_lossy(&o.stderr).into_owned())
}

#[test]
fn servers_list_pause_resume_and_root_only_verbs() {
    let d = k2_node::util::temp_dir("servers-cli");
    let (home, cfg) = (d.join("home"), d.join("cfg"));
    std::fs::create_dir_all(&cfg).unwrap();
    let (h, c) = (home.to_str().unwrap(), cfg.to_str().unwrap());

    let (ok, out, err) = run(&["servers", "--home", h, "--config-dir", c]);
    assert!(ok, "{err}");
    assert!(out.contains("Solo"), "{out}");

    // A shared node (as `servers add` would leave it once confirmed).
    let mut f = ServersFile::new(&"a".repeat(64), "rosson.k2.dev", ShareLimits::owner(2, 8, 16, 100, Network::On), 8, 1, "local:t");
    let mut b = f.servers[0].clone();
    b.slot = 1;
    b.label = "z3thon.k2.dev".into();
    b.fp_prefix = "0123456789abcdef".into();
    b.trust = Trust::Own;
    b.state = ShareState::Active;
    b.set_limits(&ShareLimits::new_server(Trust::Own, false));
    f.servers.push(b);
    servers::write_file(&cfg, &f).unwrap();

    let (ok, out, err) = run(&["servers", "list", "--home", h, "--config-dir", c]);
    assert!(ok, "{err}");
    assert!(out.contains("rosson.k2.dev") && out.contains("z3thon.k2.dev  active (own)"), "{out}");

    let (ok, out, err) = run(&["servers", "pause", "z3thon.k2.dev", "--home", h, "--config-dir", c]);
    assert!(ok, "{err}");
    assert!(out.contains("paused"), "{out}");
    assert_eq!(servers::read_file(&cfg).unwrap().unwrap().entry(1).unwrap().state, ShareState::Paused);
    let (ok, _, err) = run(&["servers", "pause", "z3thon.k2.dev", "--home", h, "--config-dir", c]);
    assert!(!ok && err.contains("bad_args"), "pausing twice is refused: {err}");
    let (ok, _, err) = run(&["servers", "resume", "slot:1", "--home", h, "--config-dir", c]);
    assert!(ok, "{err}");
    let (ok, out, err) = run(&["servers", "limits", "z3thon.k2.dev", "max_running=2", "cpu-hours=4", "--for", "2h", "--home", h, "--config-dir", c, "--json"]);
    assert!(ok, "{err}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!((v["limits"]["max_running"].as_u64(), v["limits"]["cpu_hours_day"].as_u64()), (Some(2), Some(4)), "{v}");
    assert!(v["expires_at"].as_i64().is_some());
    let (ok, _, err) = run(&["servers", "limits", "slot:0", "--weight", "3", "--home", h, "--config-dir", c]);
    assert!(ok, "{err}");
    assert_eq!(servers::read_file(&cfg).unwrap().unwrap().entry(0).unwrap().weight, 3, "the machine owner sets a weight");
    for bad in ["0", "101", "two"] {
        let (ok, _, err) = run(&["servers", "limits", "slot:0", "--weight", bad, "--home", h, "--config-dir", c]);
        assert!(!ok && err.contains("weight"), "--weight {bad}: {err}");
    }
    assert_eq!(servers::read_file(&cfg).unwrap().unwrap().entry(0).unwrap().weight, 3, "a bad weight changes nothing");
    let (ok, _, err) = run(&["servers", "limits", "z3thon.k2.dev", "surprise=1", "--home", h, "--config-dir", c]);
    assert!(!ok && err.contains("surprise"), "{err}");

    // 0.46.4: the add flags work on limits too ("keys as the add flags"),
    // and key=value keeps working; both reach servers.toml.
    let (ok, out, err) = run(&[
        "servers", "limits", "z3thon.k2.dev", "--cpus", "8", "--mem", "32G", "--disk", "120G", "--cpu-hours", "48", "--home", h, "--config-dir", c, "--json",
    ]);
    assert!(ok, "--flag value form: {err}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let l = &v["limits"];
    assert_eq!(
        (l["max_cpus"].as_u64(), l["max_mem_gb"].as_u64(), l["disk_gb"].as_u64(), l["cpu_hours_day"].as_u64()),
        (Some(8), Some(32), Some(120), Some(48)),
        "{v}"
    );
    let stored = servers::read_file(&cfg).unwrap().unwrap().entry(1).unwrap().clone();
    let (ok, out, err) = run(&["servers", "limits", "z3thon.k2.dev", "cpus=4", "mem=16G", "disk=60G", "cpu-hours=24", "--home", h, "--config-dir", c, "--json"]);
    assert!(ok, "key=value form: {err}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let l = &v["limits"];
    assert_eq!(
        (l["max_cpus"].as_u64(), l["max_mem_gb"].as_u64(), l["disk_gb"].as_u64(), l["cpu_hours_day"].as_u64()),
        (Some(4), Some(16), Some(60), Some(24)),
        "{v}"
    );
    assert_ne!(servers::read_file(&cfg).unwrap().unwrap().entry(1).unwrap(), &stored, "the key=value change was written");
    // A typo'd flag or key is refused, naming the right ones; nothing changes.
    let before = servers::read_file(&cfg).unwrap().unwrap();
    let (ok, _, err) = run(&["servers", "limits", "z3thon.k2.dev", "--cpu", "8", "--home", h, "--config-dir", c]);
    assert!(!ok && err.contains("unknown flag --cpu") && err.contains("--cpus"), "{err}");
    let (ok, _, err) = run(&["servers", "limits", "z3thon.k2.dev", "memory=8G", "--home", h, "--config-dir", c]);
    assert!(!ok && err.contains("unknown limit \"memory\"") && err.contains("--mem"), "{err}");
    assert_eq!(servers::read_file(&cfg).unwrap().unwrap(), before, "a refused typo changes nothing");
    // The help shows the flag form.
    let (_, out, err) = run(&["--help"]);
    let help = format!("{out}{err}");
    assert!(help.contains("servers limits <server> [--cpus N]") && help.contains("key=value works too"), "{help}");

    // add and revoke change root's records: sudo only.
    let (ok, _, err) = run(&["servers", "add", "--url", "https://b.k2.dev", "--code", "ABCDE-FGHJK.0123456789abcdef", "--home", h, "--config-dir", c]);
    assert!(!ok && err.contains("sudo"), "{err}");
    let (ok, _, err) = run(&["servers", "revoke", "z3thon.k2.dev", "--home", h, "--config-dir", c]);
    assert!(!ok && err.contains("sudo"), "{err}");
    let (ok, _, err) = run(&["servers", "prune", "--home", h, "--config-dir", c]);
    assert!(!ok && err.contains("sudo"), "prune writes root's records too: {err}");
    assert_eq!(servers::read_file(&cfg).unwrap().unwrap().entry(1).unwrap().state, ShareState::Active);
}

/// `k2-node servers add` with `--code -` / `$K2_NODE_ENROLL_CODE`
/// (2026-10-28): the enroll code never rides in argv, where `ps` shows it to
/// every user. The child blocks reading stdin while this test reads its
/// argv the way any other user could (`ps -o args=`).
#[test]
fn servers_add_takes_the_code_on_stdin_or_env_and_never_in_argv() {
    use std::io::Write as _;
    use std::process::Stdio;

    const CODE: &str = "ABCDE-FGHJK.0123456789abcdef";
    let d = k2_node::util::temp_dir("servers-code");
    let (home, cfg) = (d.join("home"), d.join("cfg"));
    std::fs::create_dir_all(&cfg).unwrap();
    let (h, c) = (home.to_str().unwrap(), cfg.to_str().unwrap());
    let args = ["servers", "add", "--url", "https://b.k2.dev", "--code", "-", "--home", h, "--config-dir", c];
    assert!(!args.iter().any(|a| a.contains("ABCDE")), "the argv this test spawns has no code");

    let spawn = |args: &[&str], env: Option<&str>| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_k2-node"));
        cmd.args(args).env_clear().env("PATH", "/usr/bin:/bin").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        if let Some(v) = env {
            cmd.env("K2_NODE_ENROLL_CODE", v);
        }
        cmd.spawn().expect("spawn k2-node")
    };
    let finish = |mut child: std::process::Child, stdin: &str| {
        {
            let mut i = child.stdin.take().expect("stdin");
            i.write_all(stdin.as_bytes()).expect("write stdin");
        }
        let o = child.wait_with_output().expect("wait");
        (o.status.success(), String::from_utf8_lossy(&o.stdout).into_owned(), String::from_utf8_lossy(&o.stderr).into_owned())
    };
    let argv_of = |pid: u32| -> String {
        let ps = Command::new("ps").args(["-ww", "-o", "args=", "-p", &pid.to_string()]).output().expect("ps");
        String::from_utf8_lossy(&ps.stdout).into_owned()
    };

    // stdin: while it waits for the code, its argv shows no code; then the
    // code is accepted (the next refusal is sudo's) and never echoed.
    let child = spawn(&args, None);
    let shown = argv_of(child.id());
    assert!(shown.contains("servers add") && shown.contains("--code -"), "ps sees the waiting process: {shown:?}");
    assert!(!shown.contains("ABCDE") && !shown.contains("0123456789abcdef"), "the code is not in argv: {shown:?}");
    let (ok, out, err) = finish(child, &format!("{CODE}\n"));
    assert!(!ok && err.contains("sudo"), "the code from stdin parsed; sudo is the next step: {err}");
    assert!(!err.contains(CODE) && !out.contains(CODE), "never echoed: {out} {err}");

    // A malformed code on stdin is named as malformed, not echoed.
    let (ok, _, err) = finish(spawn(&args, None), "not-a-code-77\n");
    assert!(!ok && err.contains("--code must look like"), "{err}");
    assert!(!err.contains("not-a-code-77"), "{err}");
    // Empty stdin says how to pipe it.
    let (ok, _, err) = finish(spawn(&args, None), "");
    assert!(!ok && err.contains("stdin was empty"), "{err}");

    // The environment form: no --code at all.
    let no_flag = ["servers", "add", "--url", "https://b.k2.dev", "--home", h, "--config-dir", c];
    let child = spawn(&no_flag, Some(CODE));
    assert!(!argv_of(child.id()).contains("ABCDE"), "the env form isn't in argv either");
    let (ok, _, err) = finish(child, "");
    assert!(!ok && err.contains("sudo"), "the code from K2_NODE_ENROLL_CODE parsed: {err}");
    assert!(!err.contains(CODE), "{err}");
    // Neither: the error names both safe forms.
    let (ok, _, err) = finish(spawn(&no_flag, None), "");
    assert!(!ok && err.contains("--code - < code-file") && err.contains("K2_NODE_ENROLL_CODE"), "{err}");
}
