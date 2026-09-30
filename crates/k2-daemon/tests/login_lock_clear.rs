//! Login-lock clear: `POST /cli/skin/users/unlock` and `POST /cli/users/unlock`.
//!
//! Real dispatcher. Temp `$HOME`. Fail loud — a missing row, hash, or
//! session is a panic, not a default.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_core::connect_users::{self, Role};
use k2_daemon::test_harness;
use rusqlite::{params, OptionalExtension};

const OWNER_TOKEN: &str = "owner-token-login-lock-clear";

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

struct Resp {
    status: u16,
    body: String,
}

fn http(port: u16, method: &str, path_and_query: &str, body: Option<&str>) -> Resp {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to test daemon");
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .expect("set read timeout");
    let req = match body {
        Some(b) => format!(
            "{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None => format!("{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"),
    };
    stream.write_all(req.as_bytes()).expect("write request");
    stream.flush().expect("flush");
    let mut raw: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some((status, body, complete)) = try_parse(&raw) {
            if complete {
                return Resp { status, body };
            }
        }
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&chunk[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::UnexpectedEof
                        | std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                ) =>
            {
                break
            }
            Err(e) => panic!("read response: {e:?}"),
        }
    }
    let text = String::from_utf8_lossy(&raw);
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("could not parse status from response: {text:?}"));
    let body = match text.split_once("\r\n\r\n") {
        Some((_h, b)) => b.to_string(),
        None => String::new(),
    };
    Resp { status, body }
}

fn try_parse(raw: &[u8]) -> Option<(u16, String, bool)> {
    let text = String::from_utf8_lossy(raw);
    let (headers, body) = text.split_once("\r\n\r\n")?;
    let status = headers
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())?;
    let content_len = headers.lines().find_map(|l| {
        l.to_ascii_lowercase()
            .strip_prefix("content-length:")
            .and_then(|v| v.trim().parse::<usize>().ok())
    });
    let complete = match content_len {
        Some(clen) => body.len() >= clen,
        None => true,
    };
    Some((status, body.to_string(), complete))
}

fn with_temp_home<F: FnOnce()>(f: F) {
    struct Restore {
        prev: Option<std::ffi::OsString>,
        tmp: std::path::PathBuf,
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            match self.prev.take() {
                Some(p) => std::env::set_var("HOME", p),
                None => std::env::remove_var("HOME"),
            }
            let _ = std::fs::remove_dir_all(&self.tmp);
        }
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!(
        "k2-lock-clear-{}-{}",
        std::process::id(),
        nanos
    ));
    std::fs::create_dir_all(tmp.join(".k2")).expect("temp HOME");
    let restore = Restore {
        prev: std::env::var_os("HOME"),
        tmp: tmp.clone(),
    };
    std::env::set_var("HOME", &tmp);
    let _ = k2_core::db::init_for_tests();
    f();
    drop(restore);
}

fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

fn json(body: &str) -> serde_json::Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("body must be JSON ({e}): {body:?}"))
}

fn assert_unlock_body(body: &str, username: &str, cleared: bool) {
    let v = json(body);
    let obj = v
        .as_object()
        .unwrap_or_else(|| panic!("unlock body must be an object: {body}"));
    let mut keys: Vec<_> = obj.keys().cloned().collect();
    keys.sort();
    assert_eq!(
        keys,
        vec![
            "cleared".to_string(),
            "ok".to_string(),
            "username".to_string()
        ],
        "body must be only ok, username, cleared: {body}"
    );
    assert_eq!(v["ok"], true, "{body}");
    assert_eq!(v["username"], username, "{body}");
    assert_eq!(v["cleared"], cleared, "{body}");
}

fn skin_conn() -> rusqlite::Connection {
    let path = k2_core::paths::k2_home().join("skin.db");
    let conn = rusqlite::Connection::open(&path).expect("open skin.db");
    conn.busy_timeout(Duration::from_secs(2))
        .expect("skin.db busy timeout");
    conn
}

fn lock_row(username: &str) -> Option<(i64, Option<i64>)> {
    skin_conn()
        .query_row(
            "SELECT failed_count, locked_until FROM login_lockouts WHERE username = ?1",
            params![username],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .expect("query login_lockouts")
}

fn password_hash(username: &str) -> String {
    let hash: Option<String> = skin_conn()
        .query_row(
            "SELECT password_hash FROM principals WHERE username = ?1",
            params![username],
            |r| r.get(0),
        )
        .unwrap_or_else(|e| panic!("password hash query for {username}: {e}"));
    hash.unwrap_or_else(|| panic!("principal {username} has no password hash"))
}

fn live_sessions(username: &str) -> i64 {
    skin_conn()
        .query_row(
            "SELECT COUNT(*) FROM tokens t
             JOIN principals p ON p.id = t.principal_id
             WHERE p.username = ?1 AND t.session = 1 AND t.revoked_at IS NULL",
            params![username],
            |r| r.get(0),
        )
        .expect("session count")
}

fn principal_exists(username: &str) -> bool {
    let n: i64 = skin_conn()
        .query_row(
            "SELECT COUNT(*) FROM principals WHERE username = ?1",
            params![username],
            |r| r.get(0),
        )
        .expect("principal count");
    n > 0
}

fn post_unlock_skin(port: u16, token: &str, username: &str) -> Resp {
    http(
        port,
        "POST",
        &format!("/cli/skin/users/unlock?token={token}"),
        Some(&format!(r#"{{"username":"{username}"}}"#)),
    )
}

fn bad_skin_login(port: u16, username: &str) {
    let r = http(
        port,
        "POST",
        "/cli/skin/login",
        Some(&format!(
            r#"{{"username":"{username}","password":"wrong-pass"}}"#
        )),
    );
    assert_eq!(r.status, 401, "bad login {username}; {}", r.body);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn app_unlock_deletes_only_the_login_row() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let port = daemon.port;

        let added = http(
            port,
            "POST",
            &format!("/cli/skin/users?token={OWNER_TOKEN}"),
            Some(r#"{"username":"baden"}"#),
        );
        assert_eq!(added.status, 200, "add principal; {}", added.body);
        let pw = http(
            port,
            "POST",
            &format!("/cli/skin/users/password?token={OWNER_TOKEN}"),
            Some(r#"{"username":"baden","password":"s3cret-horse"}"#),
        );
        assert_eq!(pw.status, 200, "set password; {}", pw.body);
        let login = http(
            port,
            "POST",
            "/cli/skin/login",
            Some(r#"{"username":"baden","password":"s3cret-horse"}"#),
        );
        assert_eq!(login.status, 200, "login; {}", login.body);
        let session = json(&login.body)["token"]
            .as_str()
            .unwrap_or_else(|| panic!("login token; {}", login.body))
            .to_string();
        let hash_before = password_hash("baden");
        let sessions_before = live_sessions("baden");
        assert!(sessions_before >= 1, "login must leave a session");
        assert!(
            k2_core::skin::resolve_skin_token(&session).is_some(),
            "session resolves before unlock"
        );

        for _ in 0..3 {
            bad_skin_login(port, "baden");
        }
        let live = lock_row("baden").unwrap_or_else(|| panic!("live lock row missing"));
        assert_eq!(live.0, 0, "three failures zero the count");
        let until = live.1.unwrap_or_else(|| panic!("live lock must set locked_until"));
        assert!(until > 0, "locked_until must be set");

        let cleared = post_unlock_skin(port, OWNER_TOKEN, "baden");
        assert_eq!(cleared.status, 200, "{}", cleared.body);
        assert_unlock_body(&cleared.body, "baden", true);
        assert!(lock_row("baden").is_none(), "live row must be deleted");
        assert_eq!(password_hash("baden"), hash_before, "hash must stay");
        assert_eq!(live_sessions("baden"), sessions_before, "sessions must stay");
        assert!(
            k2_core::skin::resolve_skin_token(&session).is_some(),
            "session must still resolve"
        );

        let again = post_unlock_skin(port, OWNER_TOKEN, "baden");
        assert_eq!(again.status, 200, "{}", again.body);
        assert_unlock_body(&again.body, "baden", false);

        let get = http(
            port,
            "GET",
            &format!("/cli/skin/users/unlock?token={OWNER_TOKEN}"),
            None,
        );
        assert_eq!(get.status, 405, "{}", get.body);
        assert_eq!(get.body.trim(), r#"{"error":"POST required"}"#);

        bad_skin_login(port, "held");
        assert!(lock_row("held").is_some(), "held row before refused token");
        connect_users::add_user("deputy", "password123").expect("add admin");
        connect_users::set_role("deputy", Role::Admin).expect("admin role");
        let deputy = connect_users::create_session("deputy");
        let refused = post_unlock_skin(port, &deputy, "held");
        assert_eq!(refused.status, 403, "{}", refused.body);
        assert_eq!(
            refused.body.trim(),
            r#"{"error":"Invalid or missing auth token"}"#
        );
        assert!(lock_row("held").is_some(), "403 must leave the row");

        let principals_before = {
            let n: i64 = skin_conn()
                .query_row("SELECT COUNT(*) FROM principals", [], |r| r.get(0))
                .expect("principal count");
            n
        };
        bad_skin_login(port, "jane@clinic.com");
        bad_skin_login(port, "jane");
        let email = lock_row("jane@clinic.com").unwrap_or_else(|| panic!("email row"));
        let user = lock_row("jane").unwrap_or_else(|| panic!("username row"));
        assert!(email.1.is_some() || email.0 >= 1, "email row stored");
        assert_eq!(user.0, 1, "one failure");
        assert!(user.1.is_none(), "partial row has locked_until NULL");
        assert!(!principal_exists("jane"), "jane must not be a principal");

        let email_clear = post_unlock_skin(port, OWNER_TOKEN, "jane@clinic.com");
        assert_eq!(email_clear.status, 200, "{}", email_clear.body);
        assert_unlock_body(&email_clear.body, "jane@clinic.com", true);
        assert!(lock_row("jane@clinic.com").is_none(), "email row deleted");
        assert!(lock_row("jane").is_some(), "username row must stay");
        assert!(!principal_exists("jane"));

        let name_clear = post_unlock_skin(port, OWNER_TOKEN, "jane");
        assert_eq!(name_clear.status, 200, "{}", name_clear.body);
        assert_unlock_body(&name_clear.body, "jane", true);
        assert!(lock_row("jane").is_none(), "username row deleted");
        let principals_after: i64 = skin_conn()
            .query_row("SELECT COUNT(*) FROM principals", [], |r| r.get(0))
            .expect("principal count after");
        assert_eq!(principals_after, principals_before);
        assert!(!principal_exists("jane"));
        assert!(!principal_exists("jane@clinic.com"));

        bad_skin_login(port, "partial");
        let partial = lock_row("partial").unwrap_or_else(|| panic!("partial row"));
        assert_eq!(partial.0, 1);
        assert!(partial.1.is_none(), "locked_until NULL");
        let partial_clear = post_unlock_skin(port, OWNER_TOKEN, "partial");
        assert_eq!(partial_clear.status, 200, "{}", partial_clear.body);
        assert_unlock_body(&partial_clear.body, "partial", true);
        assert!(lock_row("partial").is_none());
        assert!(!principal_exists("partial"));

        bad_skin_login(port, "baden");
        assert!(lock_row("baden").is_some(), "lock before password set");
        let hash_mid = password_hash("baden");
        let reset = http(
            port,
            "POST",
            &format!("/cli/skin/users/password?token={OWNER_TOKEN}"),
            Some(r#"{"username":"baden","password":"other-password-1"}"#),
        );
        assert_eq!(reset.status, 200, "password set; {}", reset.body);
        assert!(
            lock_row("baden").is_some(),
            "set_principal_password must leave the lock row"
        );
        assert_ne!(password_hash("baden"), hash_mid, "password set still writes the hash");
    });
}

fn post_unlock_box(port: u16, token: &str, username: &str) -> Resp {
    http(
        port,
        "POST",
        &format!("/cli/users/unlock?token={token}"),
        Some(&format!(r#"{{"username":"{username}"}}"#)),
    )
}

fn users_file() -> Vec<u8> {
    std::fs::read(connect_users::store_path()).expect("read connect-users.json")
}

fn user_field(username: &str, field: &str) -> serde_json::Value {
    let raw = String::from_utf8(users_file()).expect("users file utf8");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("users json");
    v["users"]
        .as_array()
        .unwrap_or_else(|| panic!("users array: {raw}"))
        .iter()
        .find(|u| u["username"] == username)
        .unwrap_or_else(|| panic!("user {username} missing: {raw}"))
        .get(field)
        .unwrap_or_else(|| panic!("{username} missing {field}: {raw}"))
        .clone()
}

fn session_records() -> serde_json::Value {
    let path = connect_users::sessions_store_path();
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let v: serde_json::Value = serde_json::from_str(&raw).expect("sessions json");
    v["sessions"].clone()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn box_unlock_deletes_only_the_lock_entry() {
    let _g = lock();
    with_temp_home(|| {
        connect_users::add_user("baden", "password123").expect("add baden");
        let session = connect_users::create_session("baden");
        assert_eq!(
            connect_users::validate_session(&session).as_deref(),
            Some("baden")
        );
        for _ in 0..3 {
            assert_eq!(
                connect_users::check_and_record("baden", "nope"),
                connect_users::LoginOutcome::BadCreds
            );
        }
        assert!(connect_users::is_locked("baden"));
        let users_before = users_file();
        let hash_before = user_field("baden", "password_hash");
        let epoch_before = user_field("baden", "token_epoch");
        let sessions_before = session_records();

        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let port = daemon.port;
        let cleared = post_unlock_box(port, OWNER_TOKEN, "Baden");
        assert_eq!(cleared.status, 200, "{}", cleared.body);
        assert_unlock_body(&cleared.body, "baden", true);
        assert!(!connect_users::is_locked("baden"), "lock entry gone");
        assert_eq!(users_file(), users_before, "users file must not be rewritten");
        assert_eq!(user_field("baden", "password_hash"), hash_before);
        assert_eq!(user_field("baden", "token_epoch"), epoch_before);
        assert_eq!(user_field("baden", "disabled"), false);
        assert_eq!(user_field("baden", "must_change_password"), false);
        assert_eq!(session_records(), sessions_before, "session records must stay");
        assert_eq!(
            connect_users::validate_session(&session).as_deref(),
            Some("baden"),
            "unlock must not revoke the session"
        );

        let again = post_unlock_box(port, OWNER_TOKEN, "baden");
        assert_eq!(again.status, 200, "{}", again.body);
        assert_unlock_body(&again.body, "baden", false);
        assert_eq!(users_file(), users_before);

        let get = http(
            port,
            "GET",
            &format!("/cli/users/unlock?token={OWNER_TOKEN}"),
            None,
        );
        assert_eq!(get.status, 405, "{}", get.body);
        assert_eq!(get.body.trim(), r#"{"error":"POST required"}"#);

        connect_users::add_user("deputy", "password123").expect("add admin");
        connect_users::set_role("deputy", Role::Admin).expect("admin role");
        let deputy = connect_users::create_session("deputy");
        assert_eq!(
            connect_users::check_and_record("stuck", "nope"),
            connect_users::LoginOutcome::BadCreds
        );
        assert_eq!(connect_users::lockout_failed_count("stuck"), 1);
        assert!(
            connect_users::list_users()
                .expect("list")
                .iter()
                .all(|u| u.username != "stuck"),
            "stuck is not a connect-user"
        );
        let refused = post_unlock_box(port, &deputy, "stuck");
        assert_eq!(refused.status, 403, "{}", refused.body);
        assert_eq!(refused.body.trim(), r#"{"error":"invalid or missing token"}"#);
        assert_eq!(
            connect_users::lockout_failed_count("stuck"),
            1,
            "admin 403 must leave the entry"
        );

        let users_mid = users_file();
        assert_eq!(
            connect_users::check_and_record("ghostlock", "nope"),
            connect_users::LoginOutcome::BadCreds
        );
        assert_eq!(connect_users::lockout_failed_count("ghostlock"), 1);
        let ghost = post_unlock_box(port, OWNER_TOKEN, "ghostlock");
        assert_eq!(ghost.status, 200, "{}", ghost.body);
        assert_unlock_body(&ghost.body, "ghostlock", true);
        assert_eq!(connect_users::lockout_failed_count("ghostlock"), 0);
        assert!(
            connect_users::list_users()
                .expect("list after ghost")
                .iter()
                .all(|u| u.username != "ghostlock"),
            "unlock must not create a connect-user"
        );
        assert_eq!(users_file(), users_mid, "ghost unlock must not write users");

        connect_users::add_user("resetme", "password123").expect("add resetme");
        for _ in 0..3 {
            assert_eq!(
                connect_users::check_and_record("resetme", "nope"),
                connect_users::LoginOutcome::BadCreds
            );
        }
        assert!(connect_users::is_locked("resetme"));
        let reset = http(
            port,
            "POST",
            &format!("/cli/users/set-password?token={OWNER_TOKEN}"),
            Some(r#"{"username":"resetme","password":"newpassword1"}"#),
        );
        assert_eq!(reset.status, 200, "set-password; {}", reset.body);
        assert!(
            !connect_users::is_locked("resetme"),
            "set_password still clears the lock"
        );
        assert_eq!(connect_users::lockout_failed_count("resetme"), 0);
        assert!(connect_users::verify("resetme", "newpassword1"));
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn box_unlock_session_store_write_failure_is_500() {
    let _g = lock();
    with_temp_home(|| {
        for _ in 0..3 {
            assert_eq!(
                connect_users::check_and_record("stuck", "nope"),
                connect_users::LoginOutcome::BadCreds
            );
        }
        assert!(connect_users::is_locked("stuck"));
        assert!(
            connect_users::list_users()
                .expect("list")
                .iter()
                .all(|u| u.username != "stuck")
        );
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let port = daemon.port;
        let dir = connect_users::sessions_store_path()
            .parent()
            .expect("sessions dir")
            .to_path_buf();
        let mode = std::fs::metadata(&dir)
            .expect("stat .k2")
            .permissions()
            .mode();
        struct Restore {
            dir: std::path::PathBuf,
            mode: u32,
        }
        impl Drop for Restore {
            fn drop(&mut self) {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(
                    &self.dir,
                    std::fs::Permissions::from_mode(self.mode),
                );
            }
        }
        let restore = Restore {
            dir: dir.clone(),
            mode,
        };
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555))
            .expect("chmod 0555");
        let r = post_unlock_box(port, OWNER_TOKEN, "stuck");
        assert_eq!(r.status, 500, "write failure must be 500; {}", r.body);
        let v = json(&r.body);
        assert!(v.get("error").is_some(), "500 body: {}", r.body);
        assert!(
            v.get("cleared").is_none(),
            "must not answer cleared:false for a lock that is still there: {}",
            r.body
        );
        drop(restore);
        assert!(
            connect_users::is_locked("stuck"),
            "failed write must leave the lock"
        );
        assert!(
            connect_users::list_users()
                .expect("list after 500")
                .iter()
                .all(|u| u.username != "stuck"),
            "500 must not create a user"
        );
    });
}
