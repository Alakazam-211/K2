//! prd-vault-gaps-v1 item 3, live: the real OpenSSH tools against a K2
//! agent socket. `#[ignore]`d in the default run (it needs OpenSSH ≥ 8.9
//! with sshd); run it with `--ignored` on a Linux node (openssh-client and
//! openssh-server) and on macOS. It fails loudly when a tool is missing.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use k2_vault::{Actor, AuditQuery, Delivery, Kind, LocalVault, Namespace, PutRequest, Requester, Scope, SecretBytes, SecretMeta, SecretStore};

use super::*;

fn put_key(v: &LocalVault, ws: &str, name: &str, key: &str) -> SecretMeta {
    let mut r = PutRequest::new(Namespace::User, Scope::workspace(ws), name, Kind::SshKey);
    r.default_delivery = Some(Delivery::Agent);
    v.put(r, SecretBytes::from_slice(key.as_bytes()), &Actor::Owner).expect("put ssh key").meta
}

fn tool(cmd: &str, args: &[&str], env: &[(&str, &str)]) -> std::process::Output {
    let mut c = std::process::Command::new(cmd);
    c.args(args);
    for (k, v) in env {
        c.env(k, v);
    }
    c.output().unwrap_or_else(|e| panic!("couldn't run {cmd}: {e} (the live test needs OpenSSH: ssh, ssh-add, ssh-keygen, sshd)"))
}

fn ok(o: &std::process::Output, what: &str) -> String {
    assert!(
        o.status.success(),
        "{what} failed: {}\nstdout: {}\nstderr: {}",
        o.status,
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// A fresh unencrypted key from the real ssh-keygen: (private key path,
/// SHA256 fingerprint).
fn keygen(dir: &Path, name: &str, kind: &[&str]) -> (PathBuf, String) {
    let f = dir.join(name);
    let fs = f.to_str().expect("utf8").to_string();
    let mut args = vec!["-q", "-N", "", "-C", name, "-f", fs.as_str()];
    args.extend_from_slice(kind);
    ok(&tool("ssh-keygen", &args, &[]), "ssh-keygen");
    let fp = ok(&tool("ssh-keygen", &["-l", "-E", "sha256", "-f", &format!("{fs}.pub")], &[]), "ssh-keygen -l");
    (f, fp.split_whitespace().nth(1).expect("fingerprint").to_string())
}

fn openssh_version() -> (u32, u32, String) {
    let o = tool("ssh", &["-V"], &[]);
    let v = String::from_utf8_lossy(&o.stderr).into_owned();
    let n = v.split("OpenSSH_").nth(1).unwrap_or_else(|| panic!("not OpenSSH: {v}"));
    let mut it = n.split(|c: char| !c.is_ascii_digit());
    let a: u32 = it.next().and_then(|x| x.parse().ok()).expect("major");
    let b: u32 = it.next().and_then(|x| x.parse().ok()).expect("minor");
    (a, b, v.trim().to_string())
}

fn sign_audit(v: &LocalVault, secret_id: &str) -> Vec<(String, Option<String>)> {
    v.flush_rollups();
    v.audit(&AuditQuery { secret_id: Some(secret_id.into()), ..Default::default() }, &Actor::Owner)
        .expect("audit")
        .into_iter()
        .filter(|r| r.action == "sign")
        .map(|r| (r.outcome, r.target_host))
        .collect()
}

/// `ssh-add -l` lists exactly the granted keys; `ssh-keygen -Y sign`
/// through `SSH_AUTH_SOCK` makes signatures `ssh-keygen -Y verify`
/// accepts (ed25519 and RSA); a key that isn't granted can't sign; add and
/// remove-all are refused; `ssh` logs in to a local sshd with the agent
/// (the audit names the sshd's host key); over `ssh -A` the forwarded
/// agent lists the key but can't sign with it.
#[test]
#[ignore = "live: needs OpenSSH >= 8.9 (ssh, ssh-add, ssh-keygen, sshd); run with --ignored"]
fn live_openssh_tools_use_the_agent_socket() {
    let (major, minor, version) = openssh_version();
    assert!((major, minor) >= (8, 9), "the live test needs OpenSSH >= 8.9 (session-bind): {version}");
    let v = super::super::ensure_test_vault();
    let t = k2_vault::fsutil::TestDir::new("agent-live");
    let keys = t.path().join("keys");
    std::fs::create_dir_all(&keys).expect("keys dir");
    std::fs::set_permissions(&keys, std::fs::Permissions::from_mode(0o700)).expect("chmod");
    let (ed, ed_fp) = keygen(&keys, "live_ed", &["-t", "ed25519"]);
    let (rsa, rsa_fp) = keygen(&keys, "live_rsa", &["-t", "rsa", "-b", "2048"]);
    let (stranger, _) = keygen(&keys, "stranger", &["-t", "ed25519"]);
    let w = uuid::Uuid::new_v4().to_string();
    let ed_m = put_key(&v, &w, "LIVE_ED", &std::fs::read_to_string(&ed).expect("key"));
    put_key(&v, &w, "LIVE_RSA", &std::fs::read_to_string(&rsa).expect("key"));
    // The keys live in the vault now: without the private files, ssh-keygen
    // can only sign through the agent (it falls back to a key file next to
    // the .pub otherwise).
    std::fs::remove_file(&ed).expect("rm ed");
    std::fs::remove_file(&rsa).expect("rm rsa");
    let sid = uuid::Uuid::new_v4().to_string();
    let dir = super::super::spawn::make_session_dir(&t.path().join("secrets"), &sid).expect("dir");
    let sock = start(Requester::Session { workspace_id: w.clone(), session_id: Some(sid.clone()) }, &sid, &dir).expect("start");
    let sock_s = sock.to_str().expect("utf8").to_string();
    let env = [("SSH_AUTH_SOCK", sock_s.as_str())];

    // ssh-add -l: exactly the two granted keys, by fingerprint and name.
    let listed = ok(&tool("ssh-add", &["-l", "-E", "sha256"], &env), "ssh-add -l");
    let mut fps: Vec<&str> = listed.lines().filter_map(|l| l.split_whitespace().nth(1)).collect();
    fps.sort();
    let mut want = vec![ed_fp.as_str(), rsa_fp.as_str()];
    want.sort();
    assert_eq!(fps, want, "ssh-add -l: {listed}");
    assert!(listed.contains("LIVE_ED") && listed.contains("LIVE_RSA"), "{listed}");
    // ssh-add can't add or remove keys through this agent.
    assert!(!tool("ssh-add", &[stranger.to_str().expect("utf8")], &env).status.success(), "add refused");
    assert!(!tool("ssh-add", &["-D"], &env).status.success(), "remove-all refused");
    assert_eq!(ok(&tool("ssh-add", &["-l"], &env), "ssh-add -l").lines().count(), 2, "still two keys");
    std::fs::remove_file(&stranger).expect("rm stranger");

    // ssh-keygen -Y sign / verify, ed25519 and RSA.
    let data = t.path().join("data.txt");
    std::fs::write(&data, b"k2 agent live test\n").expect("data");
    let data_s = data.to_str().expect("utf8").to_string();
    for (key, ident) in [(&ed, "live_ed"), (&rsa, "live_rsa")] {
        let pubf = format!("{}.pub", key.display());
        let sigf = format!("{data_s}.sig");
        let _ = std::fs::remove_file(&sigf);
        ok(&tool("ssh-keygen", &["-Y", "sign", "-f", &pubf, "-n", "file", &data_s], &env), "ssh-keygen -Y sign");
        let pubkey = std::fs::read_to_string(&pubf).expect("pub");
        let allowed = t.path().join(format!("allowed_{ident}"));
        std::fs::write(&allowed, format!("{ident} {}\n", pubkey.trim())).expect("allowed signers");
        let o = std::process::Command::new("ssh-keygen")
            .args(["-Y", "verify", "-f", allowed.to_str().expect("utf8"), "-I", ident, "-n", "file", "-s", &sigf])
            .stdin(std::fs::File::open(&data).expect("data"))
            .output()
            .expect("ssh-keygen -Y verify");
        ok(&o, &format!("ssh-keygen -Y verify ({ident})"));
    }
    // A key the agent doesn't hold can't sign.
    let o = tool("ssh-keygen", &["-Y", "sign", "-f", &format!("{}.pub", stranger.display()), "-n", "file", &data_s], &env);
    assert!(!o.status.success(), "an ungranted key must not sign");

    // A local sshd as this user. Its sessions run `internal-sftp` (no login
    // shell needed: the compute users are service accounts with nologin).
    let host_dir = t.path().join("sshd");
    std::fs::create_dir_all(&host_dir).expect("sshd dir");
    let (hostkey, host_fp) = keygen(&host_dir, "host_ed25519", &["-t", "ed25519"]);
    let authorized = host_dir.join("authorized_keys");
    std::fs::write(&authorized, std::fs::read_to_string(format!("{}.pub", ed.display())).expect("pub")).expect("authorized_keys");
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").expect("port");
        l.local_addr().expect("addr").port()
    };
    let cfg = host_dir.join("sshd_config");
    std::fs::write(
        &cfg,
        format!(
            "ListenAddress 127.0.0.1:{port}\nHostKey {}\nAuthorizedKeysFile {}\nPidFile {}\nStrictModes no\nUsePAM no\n\
             PasswordAuthentication no\nKbdInteractiveAuthentication no\nPubkeyAuthentication yes\nAllowAgentForwarding yes\n\
             Subsystem sftp internal-sftp\nLogLevel VERBOSE\n",
            hostkey.display(),
            authorized.display(),
            host_dir.join("sshd.pid").display()
        ),
    )
    .expect("sshd_config");
    let sshd_bin = ["/usr/sbin/sshd", "/usr/local/sbin/sshd"]
        .into_iter()
        .find(|p| Path::new(p).exists())
        .expect("an sshd binary (openssh-server)");
    let mut sshd = std::process::Command::new(sshd_bin)
        .args(["-D", "-e", "-f", cfg.to_str().expect("utf8")])
        .stdout(std::process::Stdio::null())
        .stderr(std::fs::File::create(host_dir.join("sshd.log")).expect("log"))
        .spawn()
        .expect("start sshd");
    let up = (0..100).any(|_| {
        std::thread::sleep(Duration::from_millis(100));
        std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
    });
    let user = String::from_utf8(tool("id", &["-un"], &[]).stdout).expect("user").trim().to_string();
    let port_s = port.to_string();
    let target = format!("{user}@127.0.0.1");
    let ssh_args = |extra: &[&str]| -> Vec<String> {
        let mut a: Vec<String> = [
            "-F", "/dev/null", "-p", port_s.as_str(), "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=no", "-o",
            "UserKnownHostsFile=/dev/null", "-o", "PreferredAuthentications=publickey",
        ]
        .iter()
        .map(|x| x.to_string())
        .collect();
        a.extend(extra.iter().map(|x| x.to_string()));
        a.push(target.clone());
        a.push("sftp".into());
        a
    };
    // 1. Log in with the agent (an sftp session; stdin closed, so it ends).
    let login = up.then(|| {
        let a = ssh_args(&["-s"]);
        tool("ssh", &a.iter().map(String::as_str).collect::<Vec<_>>(), &env)
    });
    // 2. ssh -A: while the session is open, sshd's forwarded agent socket
    //    (`ssh-*/agent.*` under /tmp, or the per-user temp dir on macOS)
    //    leads back through ssh to the K2 agent. ssh binds that connection
    //    with is_forwarding, so listing works and signing is refused.
    let mut roots = vec![PathBuf::from("/tmp"), std::env::temp_dir()];
    if let Ok(o) = std::process::Command::new("getconf").arg("DARWIN_USER_TEMP_DIR").output() {
        let d = String::from_utf8_lossy(&o.stdout).trim().to_string();
        if o.status.success() && !d.is_empty() {
            roots.push(PathBuf::from(d));
        }
    }
    let agent_socks = || -> Vec<PathBuf> {
        let mut v = vec![];
        for root in &roots {
            for d in std::fs::read_dir(root).into_iter().flatten().flatten() {
                if !d.file_name().to_string_lossy().starts_with("ssh-") {
                    continue;
                }
                for f in std::fs::read_dir(d.path()).into_iter().flatten().flatten() {
                    if f.file_name().to_string_lossy().starts_with("agent.") && !v.contains(&f.path()) {
                        v.push(f.path());
                    }
                }
            }
        }
        v
    };
    let mut forwarded: Option<(Vec<(Vec<u8>, String)>, Option<Vec<u8>>, Vec<u8>)> = None;
    if up {
        let before = agent_socks();
        let a = ssh_args(&["-A", "-s"]);
        let mut child = std::process::Command::new("ssh")
            .args(&a)
            .env("SSH_AUTH_SOCK", &sock_s)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("ssh -A");
        let fwd = (0..100).find_map(|_| {
            std::thread::sleep(Duration::from_millis(100));
            agent_socks().into_iter().find(|p| !before.contains(p))
        });
        if let Some(fwd) = fwd {
            use std::io::{Read, Write};
            let call = |s: &mut std::os::unix::net::UnixStream, payload: &[u8]| -> Vec<u8> {
                s.write_all(&k2_vault::ssh_agent::frame(payload)).expect("write");
                let mut len = [0u8; 4];
                s.read_exact(&mut len).expect("len");
                let mut buf = vec![0u8; u32::from_be_bytes(len) as usize];
                s.read_exact(&mut buf).expect("body");
                buf
            };
            let mut s = std::os::unix::net::UnixStream::connect(&fwd).expect("connect to the forwarded agent");
            s.set_read_timeout(Some(Duration::from_secs(10))).expect("timeout");
            let ids = k2_vault::ssh_agent::client::parse_identities(&call(&mut s, &k2_vault::ssh_agent::client::identities())).expect("ids");
            let blob = ids.iter().find(|(_, n)| n == "LIVE_ED").map(|(b, _)| b.clone()).unwrap_or_default();
            let signed = k2_vault::ssh_agent::client::parse_sign(&call(&mut s, &k2_vault::ssh_agent::client::sign(&blob, b"via ssh -A", 0))).expect("sign answer");
            forwarded = Some((ids, signed, blob));
        }
        let _ = child.kill();
        let _ = child.wait();
    }
    let _ = sshd.kill();
    let _ = sshd.wait();
    let log = std::fs::read_to_string(host_dir.join("sshd.log")).unwrap_or_default();
    assert!(up, "sshd didn't come up on port {port}: {log}");
    let login = login.expect("login ran");
    // The login: sshd accepted the key the agent signed with, the session
    // started, and the audit names the sshd's host key (the client's bind).
    assert!(log.contains(&format!("Accepted publickey for {user}")), "sshd never accepted the agent's signature: {log}\n{login:?}");
    assert!(log.contains("subsystem 'sftp'") || log.contains("subsystem \"sftp\"") || log.contains("Starting session"), "no session: {log}");
    let rows = sign_audit(&v, &ed_m.id);
    let host = format!("ssh:{host_fp}");
    assert!(rows.iter().any(|r| r.0 == "ok" && r.1.as_deref() == Some(host.as_str())), "the login is audited with the sshd host key: {rows:?}");
    // ssh -A: listed, not signed, audited.
    match forwarded {
        Some((ids, signed, blob)) => {
            assert!(!blob.is_empty(), "the forwarded agent lists LIVE_ED: {ids:?}");
            assert_eq!(signed, None, "signing over a forwarded agent must be refused ({version})");
            let rows = sign_audit(&v, &ed_m.id);
            assert!(
                rows.iter().any(|r| r.0 == "deny:forwarded" && r.1.as_deref() == Some(host.as_str())),
                "the forwarded refusal is audited: {rows:?}"
            );
            println!("live: ssh-add, ssh-keygen -Y, ssh login and the ssh -A refusal checked ({version})");
        }
        // Apple's sshd run as a non-root service account never creates the
        // forwarded agent socket (checked on k2n3-m1, OpenSSH_10.2p1). The
        // ssh -A refusal is required on Linux; on macOS the wire-level test
        // (a real host-signed bind with is_forwarding) covers it.
        None if cfg!(target_os = "macos") => {
            println!("live: ssh-add, ssh-keygen -Y and ssh login checked; ssh -A not reproducible with this sshd ({version})")
        }
        None => panic!("sshd's forwarded agent socket never appeared ({version}): {log}"),
    }
    stop_dir(&dir);
    assert!(!sock.exists());
}
