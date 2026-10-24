//! prd-vault-gaps-v1 item 3: the per-session ssh-agent socket, driven
//! over the wire from the test (`ssh-add` / `ssh-keygen -Y` against the
//! socket, and the `--ignored` sshd run, are in `ssh_agent_live_tests.rs`).
//!
//! Every test that starts a socket holds the env lock: the secrets root and
//! the short-path roots follow env, and the boot test sweeps the short-path
//! roots.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use k2_vault::ssh_agent::{self as proto, client, testkeys, AgentKey, SessionBind};
use k2_vault::vault::{AgentPolicyPatch, AgentSocket};
use k2_vault::{
    Actor, AuditQuery, Consumer, Delivery, GrantRequest, Kind, LocalVault, Namespace, PutRequest, Requester, Scope, SecretBytes,
    SecretMeta, SecretStore, Selector,
};

use super::*;

fn put_key(v: &LocalVault, ws: &str, name: &str, key: &str, d: Delivery) -> SecretMeta {
    let mut r = PutRequest::new(Namespace::User, Scope::workspace(ws), name, Kind::SshKey);
    r.default_delivery = Some(d);
    v.put(r, SecretBytes::from_slice(key.as_bytes()), &Actor::Owner).expect("put ssh key").meta
}

fn ws() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn sess(ws: &str, sid: &str) -> Requester {
    Requester::Session { workspace_id: ws.into(), session_id: Some(sid.into()) }
}

/// A fresh secrets dir for `sid` under a private test root.
fn session_dir(t: &k2_vault::fsutil::TestDir, sid: &str) -> PathBuf {
    super::super::spawn::make_session_dir(&t.path().join("secrets"), sid).expect("session dir")
}

struct Client {
    s: UnixStream,
}

impl Client {
    fn connect(sock: &Path) -> Client {
        let s = UnixStream::connect(sock).expect("connect to the agent socket");
        s.set_read_timeout(Some(Duration::from_secs(10))).expect("timeout");
        Client { s }
    }

    /// Send one request; `None` = the agent closed the connection (EOF or
    /// a reset). A timeout or any other error fails the test: a hung agent
    /// must never read as "closed".
    fn call(&mut self, payload: &[u8]) -> Option<Vec<u8>> {
        if let Err(e) = self.s.write_all(&proto::frame(payload)) {
            assert!(closed_kind(&e), "write failed with {e:?}, not a close");
            return None;
        }
        let mut len = [0u8; 4];
        if let Err(e) = self.s.read_exact(&mut len) {
            assert!(closed_kind(&e), "read failed with {e:?}, not a close");
            return None;
        }
        let mut buf = vec![0u8; u32::from_be_bytes(len) as usize];
        self.s.read_exact(&mut buf).expect("a whole reply");
        Some(buf)
    }

    fn identities(&mut self) -> Vec<(Vec<u8>, String)> {
        let r = self.call(&client::identities()).expect("identities answer");
        client::parse_identities(&r).expect("well-formed identities")
    }

    fn sign(&mut self, blob: &[u8], data: &[u8], flags: u32) -> Option<Vec<u8>> {
        let r = self.call(&client::sign(blob, data, flags)).expect("sign answer");
        client::parse_sign(&r).expect("well-formed sign answer")
    }

    /// The agent closed the connection: `Ok(0)` or a reset, never a timeout.
    fn closed(&mut self) -> bool {
        let mut b = [0u8; 1];
        match self.s.read(&mut b) {
            Ok(0) => true,
            Ok(_) => false,
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => true,
            Err(e) => panic!("read failed with {e:?} (a timeout is not a close)"),
        }
    }
}

/// EOF, a reset or a broken pipe: the peer closed.
fn closed_kind(e: &std::io::Error) -> bool {
    matches!(e.kind(), std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe)
}

fn blob_of(ids: &[(Vec<u8>, String)], name: &str) -> Vec<u8> {
    ids.iter().find(|(_, n)| n == name).unwrap_or_else(|| panic!("{name} not listed: {ids:?}")).0.clone()
}

fn audit_outcomes(v: &LocalVault, secret_id: &str) -> Vec<(String, String, Option<String>, u64)> {
    v.flush_rollups();
    v.audit(&AuditQuery { secret_id: Some(secret_id.into()), ..Default::default() }, &Actor::Owner)
        .expect("audit")
        .into_iter()
        .filter(|r| r.action == "sign")
        .map(|r| (r.action, r.outcome, r.target_host, r.count))
        .collect()
}

#[test]
fn the_socket_lists_only_granted_keys_and_signatures_verify() {
    let _env = k2_core::test_env::lock();
    let v = super::super::ensure_test_vault();
    let t = k2_vault::fsutil::TestDir::new("agent-list");
    let w = ws();
    let ed = put_key(&v, &w, "ED_KEY", &testkeys::ed25519(), Delivery::Agent);
    put_key(&v, &w, "RSA_KEY", &testkeys::rsa2048(), Delivery::Agent);
    put_key(&v, &w, "FILE_KEY", &testkeys::ed25519(), Delivery::File);
    let other = put_key(&v, &ws(), "OTHER_KEY", &testkeys::ed25519(), Delivery::Agent);
    let other_blob = AgentKey::parse(
        v.resolve(&Selector::Id(other.id.clone()), k2_vault::Use::System, &Requester::System("test".into())).expect("other").value.expose(),
    )
    .expect("parse")
    .public_blob();
    let sid = uuid::Uuid::new_v4().to_string();
    let dir = session_dir(&t, &sid);
    let sock = start(&v, sess(&w, &sid), &sid, &dir).expect("start");
    assert!(sock.as_os_str().len() <= MAX_SOCK_PATH, "{}", sock.display());
    assert_eq!(std::fs::symlink_metadata(&sock).expect("sock").permissions().mode() & 0o777, 0o600);

    let mut c = Client::connect(&sock);
    let ids = c.identities();
    let mut names: Vec<&str> = ids.iter().map(|(_, n)| n.as_str()).collect();
    names.sort();
    assert_eq!(names, vec!["ED_KEY", "RSA_KEY"], "file keys and other workspaces' keys never appear");

    let ed_blob = blob_of(&ids, "ED_KEY");
    let sig = c.sign(&ed_blob, b"challenge-1", 0).expect("ed25519 signs");
    assert!(proto::verify_blob(&ed_blob, b"challenge-1", &sig), "ed25519 signature verifies");
    let rsa_blob = blob_of(&ids, "RSA_KEY");
    for flags in [proto::RSA_SHA2_256, proto::RSA_SHA2_512] {
        let sig = c.sign(&rsa_blob, b"challenge-2", flags).expect("rsa signs");
        assert!(proto::verify_blob(&rsa_blob, b"challenge-2", &sig), "rsa flags {flags} verifies");
    }
    assert_eq!(c.sign(&rsa_blob, b"challenge-3", 0), None, "ssh-rsa (SHA-1) is refused");
    assert_eq!(c.sign(&other_blob, b"challenge-4", 0), None, "another workspace's key is refused");
    assert_eq!(c.sign(b"not-a-blob", b"challenge-5", 0), None, "an unknown blob is refused");
    // Add, remove, remove-all, lock, unlock, smartcard: FAILURE, connection stays.
    for t in [17u8, 18, 19, 20, 22, 23, 25] {
        assert_eq!(c.call(&[t]).expect("answer"), vec![proto::FAILURE], "request {t}");
    }
    let mut ext = vec![proto::EXTENSION];
    ext.extend_from_slice(&5u32.to_be_bytes());
    ext.extend_from_slice(b"query");
    assert_eq!(c.call(&ext).expect("answer"), vec![proto::FAILURE]);
    assert_eq!(c.identities().len(), 2, "still serving after refusals");

    // Revoking takes effect on the next call: disable ED_KEY.
    v.disable(&Selector::Id(ed.id.clone()), &Actor::Owner).expect("disable");
    assert_eq!(c.identities().len(), 1);
    assert_eq!(c.sign(&ed_blob, b"challenge-6", 0), None, "a disabled key no longer signs");
    v.enable(&Selector::Id(ed.id.clone()), &Actor::Owner).expect("enable");

    // A frame over 256 KiB closes the connection.
    let mut big = Client::connect(&sock);
    big.s.write_all(&((proto::MAX_FRAME as u32) + 1).to_be_bytes()).expect("write");
    assert!(big.closed(), "oversized frame closes");
    // So does a frame that doesn't parse.
    let mut bad = Client::connect(&sock);
    assert_eq!(bad.call(&[proto::SIGN_REQUEST, 0, 0]), None, "malformed frame closes");

    let rows = audit_outcomes(&v, &ed.id);
    assert_eq!(rows.iter().filter(|r| r.1 == "ok").map(|r| r.3).sum::<u64>(), 1, "one entry per signature: {rows:?}");
    stop_dir(&dir);
    assert!(!sock.exists());
}

#[test]
fn signatures_are_rate_limited_per_session_and_secret() {
    let _env = k2_core::test_env::lock();
    let v = super::super::ensure_test_vault();
    let t = k2_vault::fsutil::TestDir::new("agent-rate");
    let w = ws();
    let a = put_key(&v, &w, "KEY_A", &testkeys::ed25519(), Delivery::Agent);
    put_key(&v, &w, "KEY_B", &testkeys::ed25519(), Delivery::Agent);
    v.set_agent_policy(&Selector::Id(a.id.clone()), AgentPolicyPatch { burst: Some(5), ..Default::default() }, &Actor::Owner)
        .expect("burst 5");
    let sid = uuid::Uuid::new_v4().to_string();
    let dir = session_dir(&t, &sid);
    let sock = start(&v, sess(&w, &sid), &sid, &dir).expect("start");
    let mut c = Client::connect(&sock);
    let ids = c.identities();
    let (blob_a, blob_b) = (blob_of(&ids, "KEY_A"), blob_of(&ids, "KEY_B"));
    let t0 = Instant::now();
    for i in 0..5 {
        assert!(c.sign(&blob_a, format!("m{i}").as_bytes(), 0).is_some(), "signature {} of the burst", i + 1);
    }
    let sixth = c.sign(&blob_a, b"m5", 0);
    assert!(t0.elapsed() < Duration::from_millis(900), "the burst test ran too slowly to mean anything");
    assert_eq!(sixth, None, "the 6th in a second is refused after --burst 5");
    assert!(c.sign(&blob_b, b"other secret", 0).is_some(), "another secret in the same session has its own budget");
    // Another session with the same key has its own budget too.
    let sid2 = uuid::Uuid::new_v4().to_string();
    let dir2 = session_dir(&t, &sid2);
    let sock2 = start(&v, sess(&w, &sid2), &sid2, &dir2).expect("start 2");
    assert!(Client::connect(&sock2).sign(&blob_a, b"s2", 0).is_some());
    assert!(audit_outcomes(&v, &a.id).iter().any(|r| r.1 == "deny:rate_limited"));
    stop_dir(&dir);
    stop_dir(&dir2);
}

/// The defaults with a clock under the test's control: 60 in one second
/// pass and the 61st fails; 240 in a minute pass and the 241st fails.
#[test]
fn the_default_limits_are_60_a_second_and_240_a_minute() {
    let l = crate::vault_fetch::RateLimiter::new(1, 1);
    let d = k2_vault::vault::AgentPolicy::default();
    let (burst, per_min) = (d.burst as usize, d.per_minute as usize);
    assert_eq!((burst, per_min), (60, 240));
    let t0 = Instant::now();
    for i in 0..60 {
        assert!(l.allow_at("s/k", burst, per_min, t0 + Duration::from_millis(i)), "{i}");
    }
    assert!(!l.allow_at("s/k", burst, per_min, t0 + Duration::from_millis(500)), "61st in the second");
    // Spread the rest over the minute: 4 a second.
    let mut n = 60;
    let mut at = t0 + Duration::from_secs(1);
    while n < 240 {
        assert!(l.allow_at("s/k", burst, per_min, at), "signature {}", n + 1);
        n += 1;
        at += Duration::from_millis(250);
    }
    assert!(at < t0 + Duration::from_secs(60));
    assert!(!l.allow_at("s/k", burst, per_min, at), "the 241st in the minute");
    assert!(l.allow_at("s/other", burst, per_min, at), "another key has its own budget");
    assert!(l.allow_at("s/k", burst, per_min, t0 + Duration::from_secs(61)), "a minute later it's open again");
}

#[test]
fn a_forwarded_agent_is_refused_unless_the_secret_allows_it() {
    let _env = k2_core::test_env::lock();
    let v = super::super::ensure_test_vault();
    let t = k2_vault::fsutil::TestDir::new("agent-fwd");
    let w = ws();
    let k = put_key(&v, &w, "FWD_KEY", &testkeys::ed25519(), Delivery::Agent);
    let sid = uuid::Uuid::new_v4().to_string();
    let dir = session_dir(&t, &sid);
    let sock = start(&v, sess(&w, &sid), &sid, &dir).expect("start");
    let host = AgentKey::parse(testkeys::ed25519().as_bytes()).expect("host key");
    let bind = |forwarding: bool, sid_bytes: &[u8]| SessionBind {
        host_key: host.public_blob(),
        session_id: sid_bytes.to_vec(),
        signature: client::host_sign(&host, sid_bytes),
        forwarding,
    };

    // No bind: served (it can't be told apart).
    let mut plain = Client::connect(&sock);
    let blob = blob_of(&plain.identities(), "FWD_KEY");
    assert!(plain.sign(&blob, b"plain", 0).is_some());

    // A local bind: served, audited with the host key.
    let mut local = Client::connect(&sock);
    assert_eq!(local.call(&client::session_bind(&bind(false, b"sid-local"))).expect("bind"), vec![proto::SUCCESS]);
    assert!(local.sign(&blob, b"local", 0).is_some());

    // A forwarded bind: refused, and it sticks (a later local bind can't undo it).
    let mut fwd = Client::connect(&sock);
    assert_eq!(fwd.call(&client::session_bind(&bind(true, b"sid-fwd"))).expect("bind"), vec![proto::SUCCESS]);
    assert_eq!(fwd.sign(&blob, b"forwarded", 0), None, "forwarded signing is refused by default");
    assert_eq!(fwd.call(&client::session_bind(&bind(false, b"sid-next-hop"))).expect("bind"), vec![proto::SUCCESS]);
    assert_eq!(fwd.sign(&blob, b"forwarded-2", 0), None, "still forwarded");

    // A bind whose host signature fails closes the connection.
    let mut forged = Client::connect(&sock);
    let mut b = bind(false, b"sid-a");
    b.session_id = b"sid-b".to_vec();
    assert_eq!(forged.call(&client::session_bind(&b)), None, "a bad bind signature closes");

    let rows = audit_outcomes(&v, &k.id);
    assert!(rows.iter().any(|r| r.1 == "deny:forwarded" && r.2.as_deref() == Some(&format!("ssh:{}", host.fingerprint()))), "{rows:?}");
    assert!(rows.iter().any(|r| r.1 == "ok" && r.2.as_deref() == Some(&format!("ssh:{}", host.fingerprint()))), "{rows:?}");

    // The person allows forwarded signing: the same forwarded bind signs.
    v.set_agent_policy(&Selector::Id(k.id.clone()), AgentPolicyPatch { allow_forwarded: Some(true), ..Default::default() }, &Actor::Owner)
        .expect("allow");
    let mut fwd2 = Client::connect(&sock);
    assert_eq!(fwd2.call(&client::session_bind(&bind(true, b"sid-fwd-2"))).expect("bind"), vec![proto::SUCCESS]);
    let sig = fwd2.sign(&blob, b"forwarded-ok", 0).expect("allowed");
    assert!(proto::verify_blob(&blob, b"forwarded-ok", &sig));
    stop_dir(&dir);
}

#[test]
fn a_long_dir_uses_the_short_socket_path_of_this_vaults_namespace() {
    let _env = k2_core::test_env::lock();
    let v = super::super::ensure_test_vault();
    let ns = namespace(&v);
    assert_eq!(ns.len(), 8);
    assert!(ns.bytes().all(|b| b.is_ascii_hexdigit()));
    let t = k2_vault::fsutil::TestDir::new("agent-path");
    let long = t.path().join("x".repeat(120));
    std::fs::create_dir_all(&long).expect("long dir");
    let sp = sock_path_for(&ns, &long).expect("path");
    let (fb, root) = sp.short.expect("a short path for a long dir");
    assert!(sp.sock.as_os_str().len() <= MAX_SOCK_PATH, "{}", sp.sock.display());
    assert!(fallback_roots(&ns).contains(&root), "{}", sp.sock.display());
    assert!(root_name_ok(root.file_name().and_then(|n| n.to_str()).expect("name"), &ns));
    assert_eq!(std::fs::metadata(&fb).expect("fb").permissions().mode() & 0o777, 0o700);
    assert_eq!(std::fs::metadata(&root).expect("root").permissions().mode() & 0o777, 0o700);
    std::fs::remove_dir(&fb).expect("cleanup");
    // $TMPDIR/k2a-<ns> first (per user), /tmp/k2a-<uid>-<ns> after it.
    let roots = fallback_roots(&ns);
    assert_eq!(roots.last().expect("last"), &PathBuf::from(format!("/tmp/k2a-{}-{ns}", unsafe { libc::geteuid() })));
    if let Some(t) = std::env::var_os("TMPDIR").filter(|t| !t.is_empty()) {
        assert_eq!(roots[0], PathBuf::from(t).join(format!("k2a-{ns}")));
    }
    let short = t.path().join("s");
    std::fs::create_dir_all(&short).expect("short dir");
    if short.join(SOCK_NAME).as_os_str().len() <= MAX_SOCK_PATH {
        let sp = sock_path_for(&ns, &short).expect("path");
        assert_eq!(sp.sock, short.join(SOCK_NAME));
        assert!(sp.short.is_none());
    }
    // A record is checked against the root it names (not today's env),
    // which must be this namespace's; the path is exactly <root>/<8>/a.sock.
    let rec = |root: &Path, sock: PathBuf| AgentSocket {
        tag: "t1".into(),
        kind: "session".into(),
        secrets_dir: "/run/k2/secrets/t1-abcdefgh".into(),
        sock_path: sock.to_string_lossy().into_owned(),
        short_root: root.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let moved_root = PathBuf::from(format!("/var/folders/zz/old-tmpdir/T/k2a-{ns}"));
    let ok = rec(&moved_root, moved_root.join("Ab3dE6gH").join(SOCK_NAME));
    assert!(record_socket(&ok, std::slice::from_ref(&ok), &ns).is_some(), "a root $TMPDIR no longer names still validates");
    let foreign = PathBuf::from("/tmp/k2a-0badc0de");
    let r = rec(&foreign, foreign.join("Ab3dE6gH").join(SOCK_NAME));
    assert!(record_socket(&r, std::slice::from_ref(&r), &ns).is_none(), "another namespace's root");
    for bad in [
        moved_root.join("..").join("x").join(SOCK_NAME),
        moved_root.join("short").join(SOCK_NAME),
        PathBuf::from("/etc/Ab3dE6gH/a.sock"),
        moved_root.join("Ab3dE6gH").join("other.sock"),
    ] {
        let r = rec(&moved_root, bad.clone());
        assert!(record_socket(&r, std::slice::from_ref(&r), &ns).is_none(), "{}", bad.display());
    }
    let relative = rec(Path::new("k2a-x"), PathBuf::from("k2a-x/Ab3dE6gH/a.sock"));
    assert!(record_socket(&relative, std::slice::from_ref(&relative), &ns).is_none());
}

/// A session's socket goes when the session ends; a daemon restart (k2-ptyd
/// keeps the session) re-serves the same path.
#[test]
fn the_socket_goes_at_session_end_and_is_re_served_after_a_restart() {
    // The secrets root follows env (K2_RUNTIME_DIR, XDG_RUNTIME_DIR): no
    // test may point it at a temp dir that is deleted under this one.
    let _env = k2_core::test_env::lock();
    let v = super::super::ensure_test_vault();
    let w = ws();
    put_key(&v, &w, "LIFE_KEY", &testkeys::ed25519(), Delivery::Agent);
    let sid = uuid::Uuid::new_v4().to_string();
    let mut env = HashMap::new();
    let out = super::super::spawn::materialize(&mut env, &w, &sid, false);
    let Some(dir) = out.dir.clone() else {
        assert!(out.skipped.iter().any(|s| s.contains("no_memory_fs")), "{out:?}");
        return;
    };
    let sock = out.agent_sock.clone().expect("an agent socket");
    assert_eq!(out.agent_keys, vec!["LIFE_KEY".to_string()]);
    assert_eq!(env.get("SSH_AUTH_SOCK").map(String::as_str), Some(sock.to_str().expect("utf8")));
    assert!(sock.as_os_str().len() <= MAX_SOCK_PATH);
    assert_eq!(Client::connect(&sock).identities().len(), 1);

    let rec = v.agent_socket_get(&sid).expect("record").expect("the daemon recorded it");
    assert_eq!((rec.kind.as_str(), rec.workspace_id.as_str()), ("session", w.as_str()));
    assert_eq!(rec.sock_path, sock.to_string_lossy());
    assert!(rec.ptyd_boot.is_empty(), "not bound until ptyd spawns the session");
    // What the ptyd spawn hook does once ptyd has started the process.
    let (mut child, cpid, cstart, ckey) = live_child_for_tests();
    assert!(v.agent_socket_bind(&sid, "boot-life", cpid, cstart, ckey).expect("bind"));
    let proc = Adopted { boot_id: "boot-life", pid: cpid, started_at: cstart };

    // Restart: the server is gone but the session (and its dir) survive;
    // after adoption it is re-served from the record.
    abandon_for_tests(&dir);
    std::thread::sleep(Duration::from_millis(700));
    assert!(UnixStream::connect(&sock).is_err(), "nobody serves the old socket");
    let again = resume_adopted(&v, &sid, proc, Some(&w), Some(&dir), Some(&dir)).expect("resume");
    assert_eq!(again, sock, "same path: SSH_AUTH_SOCK in the session stays valid");
    assert_eq!(Client::connect(&sock).identities().len(), 1, "re-served");
    // A session without a socket record: nothing to resume.
    let t = k2_vault::fsutil::TestDir::new("agent-noresume");
    let empty = session_dir(&t, "s-empty");
    assert_eq!(resume_adopted(&v, "s-empty", proc, Some(&w), Some(&empty), Some(&empty)).expect_err("none"), ResumeRefused::NoRecord);

    // Session end: socket, server, short-path dir and record go.
    let fallback = (!rec.short_root.is_empty()).then(|| sock.parent().expect("dir").to_path_buf());
    super::super::spawn::session_ended(&sid);
    assert!(!sock.exists(), "socket removed at session end");
    assert!(!dir.exists(), "dir removed at session end");
    assert!(socket_for(&dir).is_none());
    if let Some(f) = fallback {
        assert!(!f.exists(), "fallback dir removed");
    }
    assert!(UnixStream::connect(&sock).is_err());
    assert!(v.agent_socket_get(&sid).expect("record").is_none(), "record dropped at session end");
    let _ = child.kill();
    let _ = child.wait();

    // A sandboxed cell gets no socket.
    let mut env3 = HashMap::new();
    let out3 = super::super::spawn::materialize(&mut env3, &w, &uuid::Uuid::new_v4().to_string(), true);
    assert!(out3.agent_sock.is_none() && !env3.contains_key("SSH_AUTH_SOCK"), "{out3:?}");
    assert!(out3.skipped.iter().any(|s| s.contains("sandboxed")), "{out3:?}");
}

/// Item 5 hook: a `k2 job` run with an agent grant gets its own socket
/// (only its `job:` grants, not the workspace's keys), removed at run end.
#[test]
fn a_job_run_gets_an_agent_socket_removed_at_run_end() {
    let _env = k2_core::test_env::lock();
    let v = super::super::ensure_test_vault();
    let w = ws();
    let job_id = uuid::Uuid::new_v4().to_string();
    let k = put_key(&v, &w, "JOB_KEY", &testkeys::ed25519(), Delivery::File);
    put_key(&v, &w, "WS_ONLY_KEY", &testkeys::ed25519(), Delivery::Agent);
    v.grant(
        GrantRequest {
            selector: Selector::Id(k.id.clone()),
            consumer: Consumer::job(&w, &job_id),
            delivery: Delivery::Agent,
            env_name: None,
            file_name: None,
            hosts_narrowed: None,
            expires_at: None,
            version: None,
        },
        &Actor::Owner,
    )
    .expect("job grant");
    let run_sid = format!("job-{}", uuid::Uuid::new_v4());
    let mut env = HashMap::new();
    let out = super::super::spawn::materialize_job(&mut env, &w, &job_id, "deploy", &run_sid);
    let Some(dir) = out.dir.clone() else {
        assert!(out.skipped.iter().any(|s| s.contains("no_memory_fs")), "{out:?}");
        return;
    };
    let sock = out.agent_sock.clone().expect("the run's agent socket");
    assert_eq!(env.get("SSH_AUTH_SOCK").map(String::as_str), Some(sock.to_str().expect("utf8")));
    let mut c = Client::connect(&sock);
    let ids = c.identities();
    assert_eq!(ids.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>(), vec!["JOB_KEY"], "only the job's grant");
    let blob = blob_of(&ids, "JOB_KEY");
    let sig = c.sign(&blob, b"job", 0).expect("signs");
    assert!(proto::verify_blob(&blob, b"job", &sig));
    // The 60 s session sweep never touches a run's socket (only this run's
    // id is "dead", so parallel tests' sessions stay).
    super::super::spawn::sweep_dead(&|s| s != run_sid);
    assert!(sock.exists());
    super::super::spawn::job_ended(&out);
    assert!(!sock.exists() && !dir.exists(), "removed at run end");
    assert!(UnixStream::connect(&sock).is_err());
}


/// Round 2 (4): a bind whose host key doesn't parse (a host certificate)
/// can't be verified: FAILURE, the forwarding flag still counts, and the
/// connection keeps serving. A P-256 bind verifies.
#[test]
fn an_unparseable_host_key_bind_keeps_the_connection() {
    let _env = k2_core::test_env::lock();
    let v = super::super::ensure_test_vault();
    let t = k2_vault::fsutil::TestDir::new("agent-cert");
    let w = ws();
    put_key(&v, &w, "CERT_KEY", &testkeys::ed25519(), Delivery::Agent);
    let sid = uuid::Uuid::new_v4().to_string();
    let dir = session_dir(&t, &sid);
    let sock = start(&v, sess(&w, &sid), &sid, &dir).expect("start");
    let host = AgentKey::parse(testkeys::ed25519().as_bytes()).expect("host");
    let mut cert = Vec::new();
    for part in [&b"ssh-ed25519-cert-v01@openssh.com"[..], &[7u8; 32][..], &host.public_blob()[..]] {
        cert.extend_from_slice(&(part.len() as u32).to_be_bytes());
        cert.extend_from_slice(part);
    }
    let mut c = Client::connect(&sock);
    let b = SessionBind { host_key: cert, session_id: b"sid".to_vec(), signature: client::host_sign(&host, b"sid"), forwarding: true };
    assert_eq!(c.call(&client::session_bind(&b)).expect("still open"), vec![proto::FAILURE]);
    let blob = blob_of(&c.identities(), "CERT_KEY");
    assert_eq!(c.sign(&blob, b"x", 0), None, "the forwarding flag of an unverifiable bind still counts");
    // Not forwarded: served.
    let mut c2 = Client::connect(&sock);
    let b2 = SessionBind { forwarding: false, ..b };
    assert_eq!(c2.call(&client::session_bind(&b2)).expect("still open"), vec![proto::FAILURE]);
    assert!(c2.sign(&blob, b"x", 0).is_some());
    stop_dir(&dir);
}

/// Round 2 (9): a frame that stalls part-way, and a connection that sends
/// nothing, are closed at their deadlines (so idle peers can't hold slots).
#[test]
fn stalled_and_idle_connections_are_closed() {
    let _env = k2_core::test_env::lock();
    let v = super::super::ensure_test_vault();
    let t = k2_vault::fsutil::TestDir::new("agent-deadline");
    let w = ws();
    put_key(&v, &w, "DL_KEY", &testkeys::ed25519(), Delivery::Agent);
    let sid = uuid::Uuid::new_v4().to_string();
    let dir = session_dir(&t, &sid);
    let d = Deadlines { frame: Duration::from_millis(600), idle: Duration::from_millis(1500) };
    let sock = start_with(&v, sess(&w, &sid), &sid, &dir, d).expect("start");
    // Half a header, then nothing: closed after the frame deadline.
    let mut stalled = Client::connect(&sock);
    stalled.s.write_all(&[0, 0]).expect("write");
    let t0 = Instant::now();
    assert!(stalled.closed(), "a stalled frame is closed");
    assert!(t0.elapsed() < Duration::from_secs(5));
    // Nothing at all: closed after the idle deadline, not before.
    let mut idle = Client::connect(&sock);
    let t0 = Instant::now();
    assert!(idle.closed(), "an idle connection is closed");
    assert!(t0.elapsed() >= Duration::from_millis(1200), "not before the idle deadline: {:?}", t0.elapsed());
    // A live client keeps working.
    assert_eq!(Client::connect(&sock).identities().len(), 1);
    stop_dir(&dir);
}

/// Round 2 (5): listings and refused signs are throttled per session
/// before any vault work; a session that keeps getting refused is refused
/// even a good signature until its window passes.
#[test]
fn listings_and_refusals_are_throttled_per_session() {
    let _env = k2_core::test_env::lock();
    let v = super::super::ensure_test_vault();
    let t = k2_vault::fsutil::TestDir::new("agent-throttle");
    let w = ws();
    put_key(&v, &w, "TH_KEY", &testkeys::ed25519(), Delivery::Agent);
    let sid = uuid::Uuid::new_v4().to_string();
    let dir = session_dir(&t, &sid);
    let sock = start(&v, sess(&w, &sid), &sid, &dir).expect("start");
    let mut c = Client::connect(&sock);
    let blob = blob_of(&c.identities(), "TH_KEY");
    let mut failed = 0;
    for _ in 0..30 {
        let r = c.call(&client::identities()).expect("answer");
        if r == vec![proto::FAILURE] {
            failed += 1;
        }
    }
    assert!(failed >= 10, "listings past {IDS_PER_SECOND}/s are refused ({failed} of 30)");
    // Ten refused signs (an unknown key), then a good one is refused too.
    let t0 = Instant::now();
    for i in 0..REFUSALS_PER_SECOND {
        assert_eq!(c.sign(b"not-a-key", format!("r{i}").as_bytes(), 0), None);
    }
    assert_eq!(c.sign(&blob, b"good", 0), None, "refusal budget spent");
    assert!(t0.elapsed() < Duration::from_millis(900), "too slow to mean anything");
    std::thread::sleep(Duration::from_millis(1100));
    let mut c2 = Client::connect(&sock);
    assert!(c2.sign(&blob, b"good", 0).is_some(), "the next second it signs again");
    stop_dir(&dir);
}
