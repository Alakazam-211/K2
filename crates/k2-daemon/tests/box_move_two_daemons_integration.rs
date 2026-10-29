//! prd-box-move-carry-v1 S5, headless: two real daemons in scratch HOMEs
//! (each vault on a file key there, never the Keychain or a root helper's
//! keeper) and the real `cli/k2`. The acv case end to end:
//!
//! - the new box booted on a /home copy of the old vault.db WITHOUT its key:
//!   its vault is closed (`vault_key_missing`) and `tunnel status` says the
//!   token is locked in the vault, not "not paired";
//! - the export from a script (no terminal) is held for the owner; the
//!   owner approves it as a person with the vault confirm; the old vault
//!   freezes (writes 423, reads work);
//! - the import's dry run lists the tunnel token; the apply works on the
//!   closed vault and the moved vault opens there with the old vault's id,
//!   no rollback, and the tunnel token.
//!
//! "A person" here is the box owner's K2 Connect login (a person's channel,
//! confirming with the login's own password). The vault socket's terminal
//! check can't be used: on a Linux node the test and its daemons share one
//! cgroup, which that check rightly reads as "inside K2".
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

const VALUE: &str = "sk_test_BOXMOVE_E2E_value_9f8e7d6c";
const TUNNEL_TOKEN: &str = "k2tok_BOXMOVE_E2E_tunnel_0a1b2c3d";
const OWNER: &str = "bm-owner";
const PASSWORD: &str = "box move e2e owner password";

struct KillOnDrop(std::process::Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A short scratch HOME (the vault socket path must fit SUN_LEN).
fn scratch_home(tag: &str) -> PathBuf {
    let home = k2_core::test_env::short_tmp(format!("k2bm-{tag}-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]));
    std::fs::create_dir_all(home.join(".k2")).expect("mkdir home");
    home
}

struct Box {
    home: PathBuf,
    base: String,
    token: String,
    /// The owner's Connect login session.
    login: String,
    _rt: k2_core::test_env::DaemonRuntime,
    _daemon: KillOnDrop,
}

async fn start(home: &Path, wait_vault_open: bool) -> Box {
    let k2_dir = home.join(".k2");
    // The owner's Connect login, in this box's user store (written before
    // the daemon starts; one test per binary, so HOME is ours to point).
    std::env::set_var("HOME", home);
    k2_core::connect_users::add_user(OWNER, PASSWORD).expect("add owner login");
    k2_core::connect_users::set_role(OWNER, k2_core::connect_users::Role::Owner).expect("owner role");
    let login = k2_core::connect_users::create_session(OWNER);
    let shim_dir = home.join("agent-shim-empty");
    std::fs::create_dir_all(&shim_dir).expect("mkdir");
    let rt = k2_core::test_env::DaemonRuntime::new(home);
    let daemon = KillOnDrop(
        Command::new(env!("CARGO_BIN_EXE_k2-daemon"))
            .env_clear()
            .env("PATH", std::env::var("PATH").expect("PATH"))
            .env("HOME", home)
            .envs(rt.envs())
            .env("K2_TEST_AGENT_SHIM_DIR", &shim_dir)
            .env("K2_HEARTBEAT_NO_SELF_HEAL", "1")
            .env("K2SO_WATCHDOG_DISABLED", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn daemon"),
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    while !(k2_dir.join("heartbeat.port").exists() && k2_dir.join("heartbeat.token").exists() && k2_dir.join("vault.sock").exists()) {
        assert!(Instant::now() < deadline, "daemon never came up in {}", home.display());
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let port = std::fs::read_to_string(k2_dir.join("heartbeat.port")).expect("port").trim().to_string();
    let token = std::fs::read_to_string(k2_dir.join("heartbeat.token")).expect("token").trim().to_string();
    let b = Box { home: home.to_path_buf(), base: format!("http://127.0.0.1:{port}"), token, login, _rt: rt, _daemon: daemon };
    loop {
        let s = b.get("/cli/vault/status");
        if s.get("open").is_some() && (!wait_vault_open || s["open"] == true) {
            break;
        }
        assert!(Instant::now() < deadline, "vault status never answered: {s}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    b
}

fn curl(args: &[&str]) -> (u16, String) {
    let out = Command::new("curl")
        .env_clear()
        .env("PATH", std::env::var("PATH").expect("PATH"))
        .args(["-sS", "--max-time", "60", "-w", "\n%{http_code}"])
        .args(args)
        .output()
        .expect("run curl");
    let s = String::from_utf8(out.stdout).expect("utf8");
    let (body, code) = s.rsplit_once('\n').expect("status line");
    (code.trim().parse().expect("status"), body.to_string())
}

fn json_of(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or_else(|_| panic!("not json: {body}"))
}

impl Box {
    fn get(&self, path: &str) -> Value {
        json_of(&curl(&[&format!("{}{path}?token={}", self.base, self.token)]).1)
    }

    fn post(&self, path: &str, body: Value) -> (u16, Value) {
        let (c, b) = curl(&[
            "-X", "POST", "-H", "Content-Type: application/json", "--data", &body.to_string(),
            &format!("{}{path}?token={}", self.base, self.token),
        ]);
        (c, json_of(&b))
    }

    /// The owner as a person: their Connect login (see the module doc).
    fn person(&self, path: &str, body: Value) -> (u16, Value) {
        let (c, b) = curl(&[
            "-X", "POST", "-H", "Content-Type: application/json", "--data", &body.to_string(),
            &format!("{}{path}?token={}", self.base, self.login),
        ]);
        (c, json_of(&b))
    }

    /// A `vault-move` confirm for `binding`, with the login's password.
    fn confirm(&self, binding: &str) -> Value {
        let (c, t) = self.person(
            "/cli/vault/confirm/finish",
            json!({ "action": "vault-move", "secretId": binding, "method": "connect-password", "password": PASSWORD }),
        );
        assert_eq!(c, 200, "confirm {binding}: {t}");
        t["token"].clone()
    }

    /// The real CLI, as a script would run it: no terminal, clean env.
    fn k2(&self, args: &[&str]) -> Output {
        let cli = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../cli/k2");
        let out = Command::new("bash")
            .arg(&cli)
            .args(args)
            .env_clear()
            .env("PATH", std::env::var("PATH").expect("PATH"))
            .env("HOME", &self.home)
            .current_dir(&self.home)
            .stdin(Stdio::null())
            .output()
            .expect("run cli/k2");
        let all = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert!(!all.contains(VALUE) && !all.contains(TUNNEL_TOKEN), "k2 {args:?} printed a secret:\n{all}");
        out
    }
}

fn stdout_json(o: &Output) -> Value {
    json_of(&String::from_utf8_lossy(&o.stdout))
}

#[tokio::test(flavor = "current_thread")]
async fn a_box_move_carries_the_vault_and_the_tunnel_token_to_a_box_stuck_on_vault_key_missing() {
    // ── the old box: a paired tunnel (the token moves into the vault at
    // boot), one of the owner's secrets, a vault password.
    let old_home = scratch_home("old");
    std::fs::write(
        old_home.join(".k2/tunnel.json"),
        json!({ "token": TUNNEL_TOKEN, "subdomain": "boxmove-e2e", "enabled": false }).to_string(),
    )
    .expect("seed tunnel.json");
    let old = start(&old_home, true).await;
    let (c, set) = old.post(
        "/cli/vault/set",
        json!({ "name": "STRIPE_KEY", "scope": "server", "hosts": ["api.stripe.com"],
                "valueB64": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, VALUE) }),
    );
    assert_eq!(c, 200, "set: {set}");
    let tunnel_json = std::fs::read_to_string(old_home.join(".k2/tunnel.json")).expect("tunnel.json");
    assert!(!tunnel_json.contains(TUNNEL_TOKEN), "the token moved into the vault: {tunnel_json}");
    assert!(tunnel_json.contains("\"tokenHome\": \"vault\""), "the marker says where it went: {tunnel_json}");
    let old_id = old.get("/cli/vault/status")["vaultId"].as_str().expect("vault id").to_string();

    // ── the new box: a /home copy of the old one WITHOUT the key (the acv
    // move). Its vault can't open, and the tunnel token is locked in it.
    let new_home = scratch_home("new");
    std::fs::create_dir_all(new_home.join(".k2/vault")).expect("mkdir");
    // A consistent copy of the old vault.db (VACUUM INTO through sqlite).
    let copy = new_home.join(".k2/vault/vault.db");
    {
        let conn = rusqlite::Connection::open_with_flags(old_home.join(".k2/vault/vault.db"), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("open old vault.db");
        conn.execute("VACUUM INTO ?1", rusqlite::params![copy.to_string_lossy().to_string()]).expect("copy vault.db");
    }
    std::fs::write(new_home.join(".k2/tunnel.json"), &tunnel_json).expect("copy tunnel.json");
    let new = start(&new_home, false).await;
    let st = new.get("/cli/vault/status");
    assert_eq!((st["open"].clone(), st["error"]["code"].clone()), (json!(false), json!("vault_key_missing")), "{st}");
    let tun = new.get("/cli/tunnel/status");
    let locked = tun["token_locked"].as_str().unwrap_or_else(|| panic!("tunnel status says why: {tun}"));
    assert!(locked.contains("isn't open (vault_key_missing)"), "{locked}");

    // 1. New box: the move key (a script may make it).
    let o = new.k2(&["migrate", "import-secrets", "begin", "--json"]);
    assert!(o.status.success(), "begin: {}", String::from_utf8_lossy(&o.stderr));
    let begin = stdout_json(&o);
    let recipient = begin["moveKey"]["recipient"].as_str().expect("recipient").to_string();
    let fp = begin["moveKey"]["fingerprint"].as_str().expect("fp").to_string();

    // 2. Old box: a script's export is HELD (exit 2), nothing written.
    let out = old_home.join("move.k2mv");
    let o = old.k2(&["migrate", "export-secrets", "--to", &recipient, "--out", out.to_str().expect("out"), "--json"]);
    assert_eq!(o.status.code(), Some(2), "held: {}", String::from_utf8_lossy(&o.stdout));
    let held = stdout_json(&o);
    assert_eq!(held["status"], "held");
    let mv = held["request"]["id"].as_str().expect("mv").to_string();
    assert!(!out.exists());
    // The owner, as a person: the confirm is asked, then given.
    let (c, a) = old.person("/cli/migrate/secrets/approve", json!({ "request": mv }));
    assert_eq!((c, a["error"]["code"].clone()), (403, json!("vault_confirm_required")), "{a}");
    let t = old.confirm(&mv);
    let (c, done) = old.person("/cli/migrate/secrets/approve", json!({ "request": mv, "confirmToken": t }));
    assert_eq!(c, 200, "approve: {done}");
    assert_eq!(done["moveKey"], json!(fp));
    assert!(out.exists());
    let file = std::fs::read(&out).expect("move file");
    assert!(!String::from_utf8_lossy(&file).contains(VALUE), "the move file is ciphertext");
    // Frozen: writes 423, reads work, status says so.
    let (c, w) = old.post(
        "/cli/vault/set",
        json!({ "name": "LATE", "scope": "server", "hosts": ["api.stripe.com"],
                "valueB64": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, "late_value_0123456") }),
    );
    assert_eq!((c, w["error"]["code"].clone()), (423, json!("vault_moved_out")), "{w}");
    assert_eq!(old.get("/cli/vault/status")["movedOut"]["moveKey"], json!(fp));

    // 3. Copy the file over; 4. the dry run (a script may run it).
    let moved = new_home.join("move.k2mv");
    std::fs::copy(&out, &moved).expect("copy the file");
    let o = new.k2(&["migrate", "import-secrets", moved.to_str().expect("p"), "--json"]);
    assert!(o.status.success(), "dry run: {}", String::from_utf8_lossy(&o.stderr));
    let dry = stdout_json(&o);
    assert_eq!((dry["dryRun"].clone(), dry["tunnelToken"].clone(), dry["vaultId"].clone()), (json!(true), json!("present"), json!(old_id)));
    assert_eq!(new.get("/cli/vault/status")["open"], json!(false), "the dry run changed nothing");
    // A script's apply is held too.
    let o = new.k2(&["migrate", "import-secrets", moved.to_str().expect("p"), "--apply", "--json"]);
    assert_eq!(o.status.code(), Some(2));

    // 5. The owner applies, as a person (a closed vault has no confirm).
    let (c, imp) = new.person("/cli/migrate/secrets/import", json!({ "file": moved, "apply": true }));
    assert_eq!(c, 200, "apply: {imp}");
    assert_eq!((imp["tunnelToken"].clone(), imp["rollback"].clone()), (json!("present"), json!(false)));
    let st = new.get("/cli/vault/status");
    assert_eq!((st["open"].clone(), st["vaultId"].clone(), st["rollback"].clone()), (json!(true), json!(old_id), json!(false)), "{st}");
    assert_eq!(st["counts"]["user"], json!(1));
    // The token is readable again: the tunnel no longer says locked.
    let tun = new.get("/cli/tunnel/status");
    assert!(tun.get("token_locked").is_none(), "{tun}");
    // The pre-sync copy was kept aside, never deleted.
    let replaced: Vec<_> = std::fs::read_dir(new_home.join(".k2/vault"))
        .expect("ls")
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("replaced-"))
        .collect();
    assert_eq!(replaced.len(), 1);
    assert!(replaced[0].path().join("vault.db").exists());

    // The old box can still call it off (its key and vault were untouched).
    let t = old.confirm("unfreeze");
    let (c, u) = old.person("/cli/migrate/secrets/unfreeze", json!({ "confirmToken": t }));
    assert_eq!(c, 200, "unfreeze: {u}");
    assert!(old.get("/cli/vault/status").get("movedOut").is_none());

    drop(new);
    drop(old);
    let _ = std::fs::remove_dir_all(&old_home);
    let _ = std::fs::remove_dir_all(&new_home);
}
