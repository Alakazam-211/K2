//! Workspace hosted-mail manage toggle — TCP dispatcher gates (M10 / M24).
//!
//! Template: `skin_agents_can_manage_skin_toggle_gates_mutations`.
//! Auth tests run on Mac (no skip-if-linux). Enable may 409 `unsupported`.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_core::session::SessionId;
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;
use rusqlite::params;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

const OWNER_TOKEN: &str = "owner-token-deadbeef-mail-manage";

struct Resp {
    status: u16,
    body: String,
}

fn http(port: u16, method: &str, path_and_query: &str, body: Option<&str>) -> Resp {
    let mut stream = StdTcpStream::connect(("127.0.0.1", port)).expect("connect to test daemon");
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
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
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&chunk[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                break
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::UnexpectedEof
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

fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

fn with_temp_home<F: FnOnce()>(f: F) {
    let prev = std::env::var_os("HOME");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!(
        "k2-mail-manage-it-{}-{}",
        std::process::id(),
        nanos
    ));
    std::fs::create_dir_all(&tmp).expect("create temp HOME");
    std::env::set_var("HOME", &tmp);
    f();
    match prev {
        Some(p) => std::env::set_var("HOME", p),
        None => std::env::remove_var("HOME"),
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

fn json(body: &str) -> serde_json::Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("body must be JSON ({e}): {body:?}"))
}

fn login(port: u16, username: &str, password: &str) -> String {
    let r = http(
        port,
        "POST",
        "/cli/auth/login",
        Some(&format!(
            r#"{{"username":"{username}","password":"{password}"}}"#
        )),
    );
    assert_eq!(r.status, 200, "login must succeed; body={}", r.body);
    json(&r.body)["token"]
        .as_str()
        .expect("login token")
        .to_string()
}

fn provision_role(port: u16, username: &str, password: &str, role: &str) -> String {
    let r = http(
        port,
        "POST",
        &format!("/cli/users/add?token={OWNER_TOKEN}"),
        Some(&format!(
            r#"{{"username":"{username}","password":"{password}"}}"#
        )),
    );
    assert_eq!(r.status, 200, "users/add({username}); {}", r.body);
    if role != "member" {
        let r = http(
            port,
            "POST",
            &format!("/cli/users/set-role?token={OWNER_TOKEN}"),
            Some(&format!(r#"{{"username":"{username}","role":"{role}"}}"#)),
        );
        assert_eq!(r.status, 200, "set-role; {}", r.body);
    }
    login(port, username, password)
}

fn seed_ws(handle: &str) -> (String, String) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let id = uuid::Uuid::new_v4().to_string();
    let path = format!("/tmp/mail-manage-{handle}-{id}");
    conn.execute(
        "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
        params![id, handle, path],
    )
    .expect("seed project");
    (id, path)
}

fn mint_scoped_hook_for(workspace_uuid: &str) -> String {
    let sid = SessionId::new();
    k2_daemon::session_token::mint_session_token(
        &sid,
        &sid.to_string(),
        HookPrincipal {
            workspace_uuid: workspace_uuid.to_string(),
            agent_address: "mail-it-agent".to_string(),
        },
        CredMode::ApiKey,
        Provider::Anthropic,
    )
}

fn set_mail_manage(port: u16, project: &str, enable: i64) -> Resp {
    http(
        port,
        "POST",
        &format!("/cli/mail-manage?token={OWNER_TOKEN}"),
        Some(&format!(r#"{{"project":"{project}","enable":{enable}}}"#)),
    )
}

fn assert_owner_only(r: &Resp, label: &str) {
    assert_eq!(r.status, 403, "{label}; {}", r.body);
    assert!(
        r.body.contains("owner_only"),
        "{label} must teach owner_only: {}",
        r.body
    );
    assert!(
        !r.body.contains("Invalid or missing auth token"),
        "{label} must not look like a broken passport: {}",
        r.body
    );
}

fn assert_classic_forbidden(r: &Resp, label: &str) {
    assert_eq!(r.status, 403, "{label}; {}", r.body);
    assert!(
        r.body.contains("Invalid or missing auth token"),
        "{label} must stay classic forbidden: {}",
        r.body
    );
    assert!(
        !r.body.contains("owner_only"),
        "{label} must not be owner_only: {}",
        r.body
    );
}

fn assert_not_owner_only(r: &Resp, label: &str) {
    assert!(
        !r.body.contains("owner_only"),
        "{label} must not be owner_only: {}",
        r.body
    );
    assert_ne!(r.status, 403, "{label} must not 403; {}", r.body);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mail_manage_toggle_gates_m5_not_m6() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let port = daemon.port;
        let a_handle = format!("maila{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let b_handle = format!("mailb{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (a_id, a_path) = seed_ws(&a_handle);
        let (b_id, _b_path) = seed_ws(&b_handle);

        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let col: i64 = conn
                .query_row(
                    "SELECT mail_manage_enabled FROM projects WHERE id = ?1",
                    params![a_id],
                    |r| r.get(0),
                )
                .expect("column on k2so.db projects");
            assert_eq!(col, 0, "existing projects row must default 0");
        }

        let hook_a = mint_scoped_hook_for(&a_id);
        let hook_b = mint_scoped_hook_for(&b_id);

        let status_open = http(
            port,
            "GET",
            &format!("/cli/mail/status?token={hook_a}"),
            None,
        );
        assert_eq!(
            status_open.status, 200,
            "status stays agent-open toggle-off; {}",
            status_open.body
        );
        let config_open = http(
            port,
            "GET",
            &format!("/cli/mail/config?token={hook_a}"),
            None,
        );
        assert_eq!(
            config_open.status, 200,
            "GET config stays agent-open toggle-off; {}",
            config_open.body
        );

        let disable_off = http(
            port,
            "POST",
            &format!("/cli/mail/server/disable?token={hook_a}"),
            Some("{}"),
        );
        assert_owner_only(&disable_off, "default OFF agent disable");
        assert!(
            disable_off
                .body
                .contains("Allow agents to manage hosted mail on this host"),
            "manage OFF hint must name the Settings row: {}",
            disable_off.body
        );

        let list_off = http(
            port,
            "GET",
            &format!("/cli/mail/domain/list?token={hook_a}"),
            None,
        );
        assert_owner_only(&list_off, "default OFF agent domain list");

        let missing = http(port, "POST", "/cli/mail/server/disable", Some("{}"));
        assert_classic_forbidden(&missing, "missing token disable");
        let garbage = http(
            port,
            "POST",
            "/cli/mail/server/disable?token=not-a-real-passport",
            Some("{}"),
        );
        assert_classic_forbidden(&garbage, "garbage token disable");

        let get_toggle = http(port, "GET", "/cli/mail-manage", None);
        assert_eq!(
            get_toggle.status, 405,
            "GET toggle writer 405; {}",
            get_toggle.body
        );

        let agent_toggle = http(
            port,
            "POST",
            &format!("/cli/mail-manage?token={hook_a}"),
            Some(&format!(r#"{{"project":"{a_id}","enable":1}}"#)),
        );
        assert_owner_only(&agent_toggle, "agent cannot flip the toggle");
        assert!(
            !agent_toggle
                .body
                .contains("Allow agents to manage hosted mail on this host"),
            "writer self-grant must not promise the agent can flip: {}",
            agent_toggle.body
        );

        let member = provision_role(port, "mailmember", "hunter2-strong-9", "member");
        let member_toggle = http(
            port,
            "POST",
            &format!("/cli/mail-manage?token={member}"),
            Some(&format!(r#"{{"project":"{a_id}","enable":1}}"#)),
        );
        assert_classic_forbidden(&member_toggle, "Connect member toggle");
        let member_disable = http(
            port,
            "POST",
            &format!("/cli/mail/server/disable?token={member}"),
            Some("{}"),
        );
        assert_owner_only(&member_disable, "Connect member cannot drive M5");

        let admin = provision_role(port, "mailadmin", "hunter2-strong-9", "admin");
        let admin_disable_off = http(
            port,
            "POST",
            &format!("/cli/mail/server/disable?token={admin}"),
            Some("{}"),
        );
        assert_not_owner_only(&admin_disable_off, "Admin drives M5 with flag off");

        let ws_set = http(
            port,
            "POST",
            &format!("/cli/workspace/set?token={OWNER_TOKEN}"),
            Some(&format!(
                r#"{{"project":"{a_id}","fields":{{"mail_manage_enabled":1}}}}"#
            )),
        );
        assert_eq!(
            ws_set.status, 400,
            "workspace/set unknown field; {}",
            ws_set.body
        );
        {
            let db = k2_core::db::shared();
            let col: i64 = db
                .lock()
                .query_row(
                    "SELECT mail_manage_enabled FROM projects WHERE id = ?1",
                    params![a_id],
                    |r| r.get(0),
                )
                .expect("column");
            assert_eq!(col, 0, "workspace/set must not write the column");
        }

        let on = set_mail_manage(port, &a_id, 1);
        assert_eq!(on.status, 200, "owner enable; {}", on.body);
        let on_v = json(&on.body);
        assert_eq!(on_v["mailManageEnabled"], true, "{}", on.body);

        let disable_on = http(
            port,
            "POST",
            &format!("/cli/mail/server/disable?token={hook_a}"),
            Some("{}"),
        );
        assert_not_owner_only(&disable_on, "flag ON agent disable");

        let enable_on = http(
            port,
            "POST",
            &format!("/cli/mail/server/enable?token={hook_a}"),
            Some(r#"{"hostname":"mail.example.test"}"#),
        );
        assert_not_owner_only(&enable_on, "flag ON agent enable");
        if !cfg!(target_os = "linux") {
            assert_eq!(enable_on.status, 409, "Mac enable 409; {}", enable_on.body);
            assert!(
                json(&enable_on.body)["error"]["code"] == "unsupported"
                    || enable_on.body.contains("unsupported"),
                "Mac enable is unsupported not owner_only: {}",
                enable_on.body
            );
        }

        let list_on = http(
            port,
            "GET",
            &format!("/cli/mail/domain/list?token={hook_a}"),
            None,
        );
        assert_not_owner_only(&list_on, "flag ON domain list");
        assert_eq!(list_on.status, 200, "{}", list_on.body);

        let add_on = http(
            port,
            "POST",
            &format!("/cli/mail/domain/add?token={hook_a}"),
            Some("{}"),
        );
        assert_not_owner_only(&add_on, "flag ON domain add");

        let check_on = http(
            port,
            "POST",
            &format!("/cli/mail/domain/check?token={hook_a}"),
            Some("{}"),
        );
        assert_not_owner_only(&check_on, "flag ON domain check");

        let show_on = http(
            port,
            "GET",
            &format!("/cli/mail/domain/show?token={hook_a}"),
            None,
        );
        assert_not_owner_only(&show_on, "flag ON domain show");

        let uninstall = http(
            port,
            "POST",
            &format!("/cli/mail/server/uninstall?token={hook_a}"),
            Some("{}"),
        );
        assert_owner_only(&uninstall, "flag ON uninstall stays M6");
        assert!(
            !uninstall
                .body
                .contains("Allow agents to manage hosted mail on this host"),
            "M6 hint must not name the toggle: {}",
            uninstall.body
        );

        let remove = http(
            port,
            "POST",
            &format!("/cli/mail/domain/remove?token={hook_a}"),
            Some(r#"{"domain":"example.test","confirm":true}"#),
        );
        assert_owner_only(&remove, "flag ON domain remove stays M6");

        for (method, path, body) in [
            ("GET", "/cli/mail/oauth-config", None),
            ("POST", "/cli/mail/oauth-config/set", Some("{}")),
            ("POST", "/cli/mail/access/grant", Some("{}")),
            ("POST", "/cli/mail/access/set-manage", Some("{}")),
            ("POST", "/cli/mail/link/oauth/start", Some("{}")),
            ("POST", "/cli/mail/external/add", Some("{}")),
            ("POST", "/cli/mail/doctor", Some("{}")),
            ("POST", "/cli/mail/config/set", Some("{}")),
        ] {
            let r = http(port, method, &format!("{path}?token={hook_a}"), body);
            assert_owner_only(&r, &format!("flag ON must not open {path}"));
        }

        let create = http(
            port,
            "POST",
            &format!("/cli/mail/address/create?token={hook_a}"),
            Some(&format!(
                r#"{{"project":"{a_path}","localPart":"bot","domain":"pending.example.test"}}"#
            )),
        );
        assert!(
            !create.body.contains("owner_only"),
            "create must not become owner_only: {}",
            create.body
        );
        assert!(
            create.body.contains("not_ready") || create.status == 503 || create.status == 400,
            "pending/unverified mint stays not_ready (or usage); {}",
            create.body
        );

        let spoof = http(
            port,
            "POST",
            &format!("/cli/mail/server/disable?token={hook_b}&project={a_id}"),
            Some("{}"),
        );
        assert_owner_only(&spoof, "scoped B cannot use project=A");
        {
            let db = k2_core::db::shared();
            let col: i64 = db
                .lock()
                .query_row(
                    "SELECT mail_manage_enabled FROM projects WHERE id = ?1",
                    params![a_id],
                    |r| r.get(0),
                )
                .expect("column");
            assert_eq!(col, 1, "B spoof must not change A's row");
            let col_b: i64 = db
                .lock()
                .query_row(
                    "SELECT mail_manage_enabled FROM projects WHERE id = ?1",
                    params![b_id],
                    |r| r.get(0),
                )
                .expect("column b");
            assert_eq!(col_b, 0, "B stays off");
        }

        let listed = http(
            port,
            "GET",
            &format!("/cli/projects/list?token={OWNER_TOKEN}"),
            None,
        );
        assert_eq!(listed.status, 200, "{}", listed.body);
        assert!(
            listed.body.contains("mailManageEnabled"),
            "projects list JSON camelCase: {}",
            listed.body
        );
    });
}
