//! CL12 (0.46.0), headless: the real daemon binary in a scratch HOME (the
//! vault opens on a file key there, never the Keychain) and the real
//! `cli/k2`. `k2 vault carry import` picks the target workspace by id: a
//! workspace with the same name never gets the secrets, a name alone is
//! refused, `--workspace <id>` lands them, and the dry run prints the id.
//! No value ever reaches stdout or stderr.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const VALUE: &str = "sk_test_CACLI_CARRY_BYID_4c3b2a1f0e9d";

struct KillOnDrop(std::process::Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn scratch_home() -> PathBuf {
    let home = std::env::temp_dir().join(format!(
        "ca-carry-cli-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock").as_nanos()
    ));
    std::fs::create_dir_all(home.join(".k2")).expect("mkdir home");
    home
}

fn read_trimmed(p: &Path) -> String {
    std::fs::read_to_string(p).expect("read").trim().to_string()
}

fn curl(args: &[&str]) -> String {
    let out = Command::new("curl").args(["-sS", "--max-time", "10"]).args(args).output().expect("run curl");
    assert!(out.status.success(), "curl {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).expect("utf8 body")
}

fn k2(home: &Path, cwd: &Path, args: &[&str]) -> Output {
    let cli = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../cli/k2");
    let out = Command::new(k2_core::test_env::bash_program())
        .arg(&cli)
        .args(args)
        // A clean env: the outer shell may be a K2 agent cell whose
        // K2_*/K2SO_* vars would point the CLI at the real daemon.
        .env_clear().envs(k2_core::test_env::cli_base_env())
        .env("PATH", std::env::var("PATH").expect("PATH"))
        .env("HOME", home)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .output()
        .expect("run cli/k2");
    let all = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(!all.contains(VALUE), "k2 {args:?} printed the value:\n{all}");
    out
}

fn ok(o: &Output, what: &str) -> String {
    assert!(
        o.status.success(),
        "{what} failed: stdout={} stderr={}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8(o.stdout.clone()).expect("utf8")
}

#[tokio::test(flavor = "current_thread")]
async fn carry_import_goes_by_workspace_id_never_by_name() {
    let home = scratch_home();
    let k2_dir = home.join(".k2");
    let shim_dir = home.join("agent-shim-empty");
    std::fs::create_dir_all(&shim_dir).expect("mkdir");
    let _rt = k2_core::test_env::DaemonRuntime::new(&home);
    let _daemon = KillOnDrop(
        Command::new(env!("CARGO_BIN_EXE_k2-daemon"))
            .env("HOME", &home)
            .envs(_rt.envs())
            .env("K2_TEST_AGENT_SHIM_DIR", &shim_dir)
            .env("K2_HEARTBEAT_NO_SELF_HEAL", "1")
            .env("K2SO_WATCHDOG_DISABLED", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn daemon"),
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    while !(k2_dir.join("heartbeat.port").exists() && k2_dir.join("heartbeat.token").exists()) {
        assert!(Instant::now() < deadline, "daemon never wrote heartbeat.port/token");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let port = read_trimmed(&k2_dir.join("heartbeat.port"));
    let token = read_trimmed(&k2_dir.join("heartbeat.token"));
    let base = format!("http://127.0.0.1:{port}");
    loop {
        let out = Command::new("curl")
            .args(["-s", "-o", "/dev/null", "-w", "%{http_code}", "--max-time", "2"])
            .arg(format!("{base}/cli/vault/recipient?token={token}"))
            .output()
            .expect("curl");
        if String::from_utf8_lossy(&out.stdout) == "200" {
            break;
        }
        assert!(Instant::now() < deadline, "daemon/vault never became ready");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Three workspaces with the same folder name: an unrelated one, the
    // source, and the clone the secrets are meant for.
    let mut ids = vec![];
    for parent in ["other", "source", "clone"] {
        let p = home.join(parent).join("sales");
        std::fs::create_dir_all(&p).expect("mkdir ws");
        let path = dunce::canonicalize(&p).expect("canon").to_string_lossy().to_string();
        let r = curl(&[
            "-X",
            "POST",
            "-H",
            "Content-Type: application/json",
            "--data",
            &serde_json::json!({ "path": path, "seedWiki": false, "seedAgentsMd": false }).to_string(),
            &format!("{base}/cli/projects/add-without-git?token={token}"),
        ]);
        let v: serde_json::Value = serde_json::from_str(&r).unwrap_or_else(|_| panic!("add json: {r}"));
        let id = v["id"].as_str().or_else(|| v["project"]["id"].as_str()).unwrap_or_else(|| panic!("no id in {r}")).to_string();
        ids.push((id, path));
    }
    let (other_id, other_path) = ids[0].clone();
    let (source_id, source_path) = ids[1].clone();
    let (clone_id, _) = ids[2].clone();

    // The source workspace's secret (owner token, value in the body).
    let set = curl(&[
        "-X",
        "POST",
        "-H",
        "Content-Type: application/json",
        "--data",
        &serde_json::json!({
            "name": "STRIPE_KEY", "scope": "workspace", "workspace": source_id,
            "hosts": ["api.stripe.com"],
            "valueB64": base64_encode(VALUE.as_bytes()),
        })
        .to_string(),
        &format!("{base}/cli/vault/set?token={token}"),
    ]);
    assert!(!set.contains("\"error\""), "set: {set}");

    // A carry to this same vault (its own recipient).
    let rec = ok(&k2(&home, &home, &["vault", "recipient", "--json"]), "recipient");
    let rec: serde_json::Value = serde_json::from_str(&rec).expect("recipient json");
    let carry = home.join("carry.age");
    ok(
        &k2(
            &home,
            &home,
            &[
                "vault", "carry", "export",
                "--recipient", rec["recipient"].as_str().expect("recipient"),
                "--target", rec["vaultId"].as_str().expect("vaultId"),
                "--workspace", &source_id,
                "--out", carry.to_str().expect("path"),
            ],
        ),
        "carry export",
    );
    let file = std::fs::read(&carry).expect("carry file");
    assert!(!String::from_utf8_lossy(&file).contains(VALUE), "the carry file is ciphertext");

    // The other workspace's handle names it, not the clone.
    let other_handle = std::path::Path::new(&other_path).file_name().expect("name").to_string_lossy().to_string();
    let _ = source_path;

    // A name alone: refused, exit non-zero, teaches the id flag.
    let o = k2(&home, &home, &["vault", "carry", "import", carry.to_str().expect("path"), "--workspace", &other_handle, "--apply"]);
    assert!(!o.status.success(), "a name alone must be refused: {}", String::from_utf8_lossy(&o.stdout));
    let err = String::from_utf8_lossy(&o.stderr).to_string();
    assert!(err.contains("carry_workspace_by_name") || err.contains("--workspace"), "stderr: {err}");

    // Dry run by id prints the target id.
    let dry = ok(&k2(&home, &home, &["vault", "carry", "import", carry.to_str().expect("path"), "--workspace", &clone_id]), "dry run");
    assert!(dry.contains(&clone_id), "the dry run shows the target workspace id:\n{dry}");
    assert!(dry.contains("nothing changed yet"), "{dry}");

    // Apply by id.
    let applied = ok(
        &k2(&home, &home, &["vault", "carry", "import", carry.to_str().expect("path"), "--workspace", &clone_id, "--apply"]),
        "apply",
    );
    assert!(applied.contains("imported: STRIPE_KEY"), "{applied}");

    // Where the secrets are: the clone has one, the same-named other none.
    let list = curl(&[&format!("{base}/cli/vault/list?token={token}")]);
    let v: serde_json::Value = serde_json::from_str(&list).expect("list json");
    assert!(!list.contains(VALUE));
    let in_ws = |id: &str| {
        v["secrets"]
            .as_array()
            .expect("secrets")
            .iter()
            .filter(|m| m["scope"]["kind"] == "workspace" && m["scope"]["id"] == id && m["name"] == "STRIPE_KEY")
            .count()
    };
    assert_eq!(in_ws(&clone_id), 1, "{list}");
    assert_eq!(in_ws(&other_id), 0, "the same-named workspace never gets it: {list}");
    assert_eq!(in_ws(&source_id), 1, "the source keeps its own");

    let _ = std::fs::remove_dir_all(&home);
}

fn base64_encode(b: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(b)
}
