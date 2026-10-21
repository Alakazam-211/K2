//! `k2-node servers …` at the machine (prd-compute-shared-queue-v1 §9).
//! Temp dirs only; the root-only verbs are checked to refuse without sudo
//! (the tests never run as root).

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
    let (ok, _, err) = run(&["servers", "limits", "z3thon.k2.dev", "surprise=1", "--home", h, "--config-dir", c]);
    assert!(!ok && err.contains("surprise"), "{err}");

    // add and revoke change root's records: sudo only.
    let (ok, _, err) = run(&["servers", "add", "--url", "https://b.k2.dev", "--code", "ABCDE-FGHJK.0123456789abcdef", "--home", h, "--config-dir", c]);
    assert!(!ok && err.contains("sudo"), "{err}");
    let (ok, _, err) = run(&["servers", "revoke", "z3thon.k2.dev", "--home", h, "--config-dir", c]);
    assert!(!ok && err.contains("sudo"), "{err}");
    assert_eq!(servers::read_file(&cfg).unwrap().unwrap().entry(1).unwrap().state, ShareState::Active);
}
