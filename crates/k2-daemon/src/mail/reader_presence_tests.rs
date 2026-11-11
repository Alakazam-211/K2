//! `GET /cli/mail/reader/presence` end to end through the real dispatcher
//! (Rosson, 2026-11-10: the Mail tab only when the server has a linked
//! inbox or hosted mail enabled). Local state only: these tests never
//! start a fake mail engine for the presence calls themselves.

use crate::mail::mailbox_guard_tests::{block, enc, http, json, Fx, Resp, OWNER};
use crate::test_harness;

fn presence(port: u16, token: &str) -> Resp {
    http(port, "GET", &format!("/cli/mail/reader/presence?token={}", enc(token)), None)
}

/// `(hosting, linkedInboxes, visible)`, each required.
fn answer(r: &Resp) -> (bool, bool, u64) {
    assert_eq!(r.status, 200, "{}", r.body);
    let v = json(r);
    assert_eq!(v["ok"], true, "{}", r.body);
    (
        v["hosting"].as_bool().unwrap_or_else(|| panic!("no hosting: {}", r.body)),
        v["linkedInboxes"].as_bool().unwrap_or_else(|| panic!("no linkedInboxes: {}", r.body)),
        v["visible"].as_u64().unwrap_or_else(|| panic!("no visible: {}", r.body)),
    )
}

fn set_server(status: Option<&str>) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute("DELETE FROM mail_server WHERE id = 1", []).expect("clear mail_server");
    if let Some(status) = status {
        conn.execute(
            "INSERT INTO mail_server (id, status, pinned_version, hostname, updated_at) \
             VALUES (1, ?1, '0.16.25', 'mail.box.test', 1)",
            rusqlite::params![status],
        )
        .expect("mail_server row");
    }
}

fn login(user: &str, role: k2_core::connect_users::Role) -> String {
    k2_core::connect_users::add_user(user, "correct-horse-battery-staple-1").expect("add user");
    k2_core::connect_users::set_role(user, role).expect("set role");
    k2_core::connect_users::create_session(user)
}

/// A bare server answers false/false/0; hosting follows the mail_server
/// row (on while installed and not disabled); a Voice & Chat only login
/// and a bad token are refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn presence_is_false_on_a_bare_server_and_hosting_follows_the_server_row() {
    let _home = k2_core::test_env::TempHome::new();
    let _mail = crate::mail::mail_server_test_lock();
    let _row = crate::mail::MailServerRowCleanup;
    let d = test_harness::start(OWNER).await;
    block(|| {
        set_server(None);
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute("DELETE FROM mail_external_inboxes", []).expect("no linked inboxes");
        }
        let member = login("prmember", k2_core::connect_users::Role::Member);
        let admin = login("pradmin", k2_core::connect_users::Role::Admin);
        for token in [OWNER, member.as_str(), admin.as_str()] {
            assert_eq!(answer(&presence(d.port, token)), (false, false, 0), "bare server");
        }

        for status in ["running", "installing", "degraded", "stopped", "error"] {
            set_server(Some(status));
            let (hosting, linked, _) = answer(&presence(d.port, &member));
            assert!(hosting, "hosting is on while the server is {status}");
            assert!(!linked);
        }
        set_server(Some("disabled"));
        assert_eq!(answer(&presence(d.port, OWNER)), (false, false, 0), "disabled = off");
        set_server(Some("running"));
        assert!(answer(&presence(d.port, OWNER)).0);
        set_server(None);
        assert!(!answer(&presence(d.port, OWNER)).0, "uninstalled = off");

        let voice = login("prvoice", k2_core::connect_users::Role::VoiceChat);
        let v = presence(d.port, &voice);
        assert_eq!(v.status, 403, "{}", v.body);
        assert_eq!(json(&v)["error"], "role_required", "{}", v.body);
        let none = presence(d.port, "not-a-token");
        assert_eq!(none.status, 403, "{}", none.body);

        let post = http(d.port, "POST", &format!("/cli/mail/reader/presence?token={}", enc(OWNER)), Some("{}"));
        assert_eq!(post.status, 405, "presence is a GET: {}", post.body);
    });
}

/// A linked inbox alone makes presence true; `visible` follows the reader
/// rule; an agent is refused; unlinking the last one turns it false and
/// tells clients (`mail_changed`, `inboxes-changed`). Nothing is logged and
/// the mail engine is never asked.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_linked_inbox_alone_shows_mail_and_unlinking_the_last_one_is_heard() {
    let fx = Fx::start().await;
    block(|| {
        // Hosting off: only the fixture's linked inbox is left.
        set_server(None);
        let audit_before = fx.audit().len();
        let gets_before = fx.email_gets();

        // The Owner and an Admin see all three inboxes; a Member none; pat
        // their own hosted mailbox (the linked one records no linker).
        assert_eq!(answer(&presence(fx.d.port, OWNER)), (false, true, 3));
        assert_eq!(answer(&presence(fx.d.port, &fx.admin)), (false, true, 3));
        assert_eq!(answer(&presence(fx.d.port, &fx.member)), (false, true, 0));
        assert_eq!(answer(&presence(fx.d.port, &fx.pat)), (false, true, 1));

        // An agent passport: 403 owner_only.
        let sid = k2_core::session::SessionId::new();
        let pp = crate::session_token::mint_session_token(
            &sid,
            &sid.to_string(),
            crate::session_token::HookPrincipal {
                workspace_uuid: fx.ws_id.clone(),
                agent_address: "presence-agent".to_string(),
            },
            crate::session_token::CredMode::ApiKey,
            crate::session_token::Provider::Anthropic,
        );
        let a = presence(fx.d.port, &pp);
        assert_eq!(a.status, 403, "{}", a.body);
        assert_eq!(json(&a)["error"]["code"], "owner_only", "{}", a.body);

        assert_eq!(fx.audit().len(), audit_before, "presence opens no inbox, so it logs none");
        assert_eq!(fx.email_gets(), gets_before, "presence never asks the mail engine");

        // Unlink the last linked inbox: presence turns false, and the bus
        // carries the refetch signal.
        let mut rx = crate::session_events::subscribe();
        crate::mail::external::remove_inbox_with(&MemSecrets::default(), &NoHttp, &fx.linked_addr)
            .expect("unlink the linked inbox");
        let mut heard = false;
        while let Ok(ev) = rx.try_recv() {
            if let crate::session_events::SessionEvent::MailChanged { reason } = ev {
                if reason == "inboxes-changed" {
                    heard = true;
                }
            }
        }
        assert!(heard, "unlinking an inbox emits mail_changed (inboxes-changed)");
        assert_eq!(answer(&presence(fx.d.port, OWNER)), (false, false, 2), "the hosted rows are still listed");
        assert_eq!(answer(&presence(fx.d.port, &fx.member)), (false, false, 0));
    });
}

#[derive(Default)]
struct MemSecrets {
    map: std::sync::Mutex<std::collections::HashMap<String, String>>,
}

impl crate::mail::secrets::SecretStore for MemSecrets {
    fn store(&self, kind: &str, secret: &str) -> Result<String, String> {
        let key = format!("mem_{kind}");
        self.map.lock().unwrap().insert(key.clone(), secret.to_string());
        Ok(key)
    }
    fn resolve(&self, sref: &str) -> Result<Option<String>, String> {
        Ok(self.map.lock().unwrap().get(sref).cloned())
    }
    fn delete(&self, sref: &str) -> Result<(), String> {
        self.map.lock().unwrap().remove(sref);
        Ok(())
    }
}

/// A password row never dials a provider on unlink.
struct NoHttp;

impl crate::mail::oauth::HttpClient for NoHttp {
    fn post_form(
        &self,
        url: &str,
        _form: &[(&str, &str)],
    ) -> Result<crate::mail::oauth::HttpResponse, crate::mail::oauth::OauthError> {
        panic!("unlinking a password inbox must not call {url}");
    }
}
