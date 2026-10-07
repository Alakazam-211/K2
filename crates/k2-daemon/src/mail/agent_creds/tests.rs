//! Who-can-do-what for mail credentials (0.45.0). Every row: an agent
//! without mail-manage is refused, an IT agent is allowed for others and
//! refused for itself, and the owner is allowed.

use super::*;
use crate::mail::addresses::tests::{FakeAddrEngine, FakeVault};
use crate::mail::jmap::{AppPasswordInfo, CreatedAppPassword};
use crate::session_token::HookPrincipal;
use std::collections::HashMap;
use std::sync::Mutex;

/// App-password engine keyed by account id (several mailboxes per test).
/// Records destroys so a test can prove nothing was revoked.
#[derive(Default)]
struct MapEngine {
    rows: Mutex<HashMap<String, Vec<(String, String)>>>, // account → (id, label)
    destroyed: Mutex<Vec<String>>,
    next: Mutex<u32>,
    fail_list: Option<String>,
    /// Per app-password `createdAt` override (default a fixed date).
    created: Mutex<HashMap<String, serde_json::Value>>,
}

impl MapEngine {
    fn with(account_id: &str, aps: &[(&str, &str)]) -> Self {
        let e = Self::default();
        e.rows.lock().unwrap().insert(
            account_id.to_string(),
            aps.iter()
                .map(|(i, l)| (i.to_string(), l.to_string()))
                .collect(),
        );
        e
    }
}

impl AppPasswordEngine for MapEngine {
    fn create(&self, account_id: &str, description: &str) -> Result<CreatedAppPassword, String> {
        let mut n = self.next.lock().unwrap();
        *n += 1;
        let id = format!("ap-new-{n}");
        self.rows
            .lock()
            .unwrap()
            .entry(account_id.to_string())
            .or_default()
            .push((id.clone(), description.to_string()));
        Ok(CreatedAppPassword {
            id,
            secret: format!("app_secret-{n}"),
        })
    }
    fn list(&self, account_id: &str) -> Result<Vec<AppPasswordInfo>, String> {
        if let Some(e) = &self.fail_list {
            return Err(e.clone());
        }
        let rows = self
            .rows
            .lock()
            .unwrap()
            .get(account_id)
            .cloned()
            .unwrap_or_default();
        Ok(rows
            .into_iter()
            .map(|(id, description)| {
                let created_at = self
                    .created
                    .lock()
                    .unwrap()
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!("2026-10-01T00:00:00Z"));
                AppPasswordInfo {
                    id,
                    description,
                    created_at,
                }
            })
            .collect())
    }
    fn query_ids(&self, account_id: &str) -> Result<Vec<String>, String> {
        Ok(self
            .rows
            .lock()
            .unwrap()
            .get(account_id)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|(id, _)| id)
            .collect())
    }
    fn destroy(&self, account_id: &str, id: &str) -> Result<(), String> {
        self.destroyed.lock().unwrap().push(id.to_string());
        if let Some(v) = self.rows.lock().unwrap().get_mut(account_id) {
            v.retain(|(i, _)| i != id);
        }
        Ok(())
    }
}

fn body(v: serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&v).unwrap()
}

fn err_code(r: &CliResponse) -> String {
    let v: serde_json::Value =
        serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("non-JSON body {e}: {}", r.body));
    v["error"]["code"]
        .as_str()
        .unwrap_or_else(|| panic!("no error.code: {}", r.body))
        .to_string()
}

fn assert_self_elevation(r: &CliResponse) {
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    assert_eq!(err_code(r), "owner_only", "{}", r.body);
    assert!(
        r.body.contains("can't loosen mail rules on its own sends"),
        "{}",
        r.body
    );
    assert!(
        r.body.contains("ask your human") || r.body.contains("Ask your human"),
        "{}",
        r.body
    );
}

fn assert_needs_mail_manage(r: &CliResponse) {
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    assert_eq!(err_code(r), AGENT_SEND_PATH_ONLY, "{}", r.body);
    assert!(r.body.contains("k2 mail send"), "{}", r.body);
    assert!(!r.body.contains("secret"), "{}", r.body);
}

/// Test world on the shared DB: an IT workspace (mail-manage on), a plain
/// workspace, an "other" workspace id, one domain, and mailboxes.
struct World {
    it_ws: String,
    plain_ws: String,
    other_ws: String,
    domain: String,
    domain_id: String,
    rows: Vec<MailAddress>,
}

impl World {
    fn new(tag: &str) -> Self {
        let mk = |kind: &str, mm: i64| {
            let id = uuid::Uuid::new_v4().to_string();
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO projects (id, name, path, mail_manage_enabled) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    id,
                    format!("ac-{tag}-{kind}-{}", &id[..8]),
                    format!("/tmp/mail-ac-{tag}-{kind}-{}-{}", std::process::id(), &id[..12]),
                    mm
                ],
            )
            .expect("insert project");
            id
        };
        let it_ws = mk("it", 1);
        let plain_ws = mk("plain", 0);
        let other_ws = mk("other", 0);
        let domain = format!("{tag}-{}.example", uuid::Uuid::new_v4().simple());
        let domain_id = uuid::Uuid::new_v4().to_string();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO mail_domains (id, domain, stalwart_domain_id, status, created_at) \
                 VALUES (?1, ?2, 'stw-d', 'verified', 100)",
                rusqlite::params![domain_id, domain],
            )
            .expect("seed domain");
        }
        Self {
            it_ws,
            plain_ws,
            other_ws,
            domain,
            domain_id,
            rows: Vec::new(),
        }
    }

    fn it(&self) -> MailCaller {
        MailCaller::ItAgent {
            workspace_uuid: self.it_ws.clone(),
        }
    }
    fn plain(&self) -> MailCaller {
        MailCaller::Agent {
            workspace_uuid: self.plain_ws.clone(),
        }
    }
    fn principal(ws: &str) -> HookPrincipal {
        HookPrincipal {
            workspace_uuid: ws.to_string(),
            agent_address: "agent".to_string(),
        }
    }

    /// A mailbox bound to `ws`, created at `created_at`.
    fn mailbox_at(
        &mut self,
        local: &str,
        ws: &str,
        account_id: &str,
        created_at: i64,
    ) -> MailAddress {
        let id = uuid::Uuid::new_v4().to_string();
        let address = format!("{local}@{}", self.domain);
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO mail_addresses (id, address, domain_id, stalwart_account_id, \
                 owner_project_id, status, created_at) VALUES (?1, ?2, ?3, ?4, ?5, 'active', ?6)",
                rusqlite::params![id, address, self.domain_id, account_id, ws, created_at],
            )
            .expect("seed address");
        }
        let row = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            crate::mail::addresses::load_address(&conn, &address).expect("seeded row loads")
        };
        self.rows.push(row.clone());
        row
    }

    fn mailbox(&mut self, local: &str, ws: &str, account_id: &str) -> MailAddress {
        self.mailbox_at(local, ws, account_id, 100)
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        for r in &self.rows {
            let _ = conn.execute(
                "DELETE FROM mail_credential_marks WHERE address_id = ?1",
                rusqlite::params![r.id],
            );
        }
        let _ = conn.execute(
            "DELETE FROM mail_addresses WHERE domain_id = ?1",
            rusqlite::params![self.domain_id],
        );
        let _ = conn.execute(
            "DELETE FROM mail_domains WHERE id = ?1",
            rusqlite::params![self.domain_id],
        );
        for id in [&self.it_ws, &self.plain_ws, &self.other_ws] {
            let _ = conn.execute("DELETE FROM projects WHERE id = ?1", rusqlite::params![id]);
        }
    }
}

fn marks_for(address_id: &str) -> Vec<(String, String, String, Option<String>)> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut stmt = conn
        .prepare(
            "SELECT kind, credential_id, origin, creator_project_id FROM mail_credential_marks \
             WHERE address_id = ?1 ORDER BY kind, credential_id",
        )
        .unwrap();
    stmt.query_map(rusqlite::params![address_id], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

// ── who is calling ──

#[test]
fn mail_caller_classifies_owner_it_agent_and_plain_agent() {
    let w = World::new("cls");
    assert_eq!(mail_caller(), MailCaller::Owner);
    let c = crate::caller_workspace::with_request_principal(
        Some(World::principal(&w.it_ws)),
        mail_caller,
    );
    assert_eq!(c, w.it());
    let c = crate::caller_workspace::with_request_principal(
        Some(World::principal(&w.plain_ws)),
        mail_caller,
    );
    assert_eq!(c, w.plain());
    // An unknown workspace is never an IT agent (fail closed).
    let c = crate::caller_workspace::with_request_principal(
        Some(World::principal("no-such-workspace")),
        mail_caller,
    );
    assert!(matches!(c, MailCaller::Agent { .. }), "{c:?}");
}

// ── app-password add ──

#[test]
fn app_password_add_matrix() {
    let mut w = World::new("ap");
    let it_own = w.mailbox("it-bot", &w.it_ws.clone(), "acc-ap-own");
    let it_person = w.mailbox("staff", &w.it_ws.clone(), "acc-ap-person");
    set_person_flag(&it_person.id, true).unwrap();
    let other = w.mailbox("other-bot", &w.other_ws.clone(), "acc-ap-other");
    let engine = MapEngine::default();

    // Agent without mail-manage: refused at the handler, before any
    // lookup (its own mailbox, someone else's, or a ghost).
    for addr in [other.address.clone(), format!("ghost@{}", w.domain)] {
        let r = crate::caller_workspace::with_request_principal(
            Some(World::principal(&w.plain_ws)),
            || {
                crate::mail::app_password::handle_app_password_add(&body(
                    serde_json::json!({ "address": addr }),
                ))
            },
        );
        assert_needs_mail_manage(&r);
    }
    let r =
        crate::mail::app_password::gated_add_on(&w.plain(), &engine, &other, "acc-ap-other", "x");
    assert_needs_mail_manage(&r);

    // IT agent, its OWN mailbox: refused at the handler (no engine needed).
    let r =
        crate::caller_workspace::with_request_principal(Some(World::principal(&w.it_ws)), || {
            crate::mail::app_password::handle_app_password_add(&body(
                serde_json::json!({ "address": it_own.address }),
            ))
        });
    assert_self_elevation(&r);
    let r = crate::mail::app_password::gated_add_on(&w.it(), &engine, &it_own, "acc-ap-own", "x");
    assert_self_elevation(&r);
    assert!(
        engine.rows.lock().unwrap().is_empty(),
        "nothing minted on a refusal"
    );
    assert!(marks_for(&it_own.id).is_empty());

    // IT agent, ANOTHER workspace's mailbox and a person's mailbox in its
    // own workspace: allowed; the secret is shown; creator recorded.
    for (row, acc) in [(&other, "acc-ap-other"), (&it_person, "acc-ap-person")] {
        let r = crate::mail::app_password::gated_add_on(&w.it(), &engine, row, acc, "Mail.app");
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert!(v["secret"].as_str().unwrap().starts_with("app_"), "{v}");
        let id = v["id"].as_str().unwrap().to_string();
        assert_eq!(
            marks_for(&row.id),
            vec![(
                KIND_APP_PASSWORD.to_string(),
                id,
                ORIGIN_MINTED.to_string(),
                Some(w.it_ws.clone())
            )]
        );
    }

    // Owner: any mailbox, the IT agent's own included; creator = owner.
    let r = crate::mail::app_password::gated_add_on(
        &MailCaller::Owner,
        &engine,
        &it_own,
        "acc-ap-own",
        "k2",
    );
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let id = serde_json::from_str::<serde_json::Value>(&r.body).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        marks_for(&it_own.id),
        vec![(
            KIND_APP_PASSWORD.to_string(),
            id,
            ORIGIN_MINTED.to_string(),
            None
        )]
    );
    // Owner handler gets past every gate (ghost → 404, not 403).
    let r = crate::mail::app_password::handle_app_password_add(&body(
        serde_json::json!({ "address": format!("ghost@{}", w.domain) }),
    ));
    assert_eq!(r.status, "404 Not Found", "{}", r.body);
}

#[test]
fn revoke_drops_the_mark() {
    let mut w = World::new("rv");
    let row = w.mailbox("bot", &w.other_ws.clone(), "acc-rv");
    let engine = MapEngine::with("acc-rv", &[("ap-1", "k2")]);
    mark(&row.id, KIND_APP_PASSWORD, "ap-1", ORIGIN_MINTED, None).unwrap();
    let r = crate::mail::app_password::revoke_and_unmark(&engine, &row, "acc-rv", "ap-1");
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(engine.destroyed.lock().unwrap().as_slice(), ["ap-1"]);
    assert!(marks_for(&row.id).is_empty(), "revoke must drop the mark");
}

// ── mailbox password rotate ──

#[test]
fn password_rotate_matrix() {
    let mut w = World::new("pw");
    let it_own = w.mailbox("it-bot", &w.it_ws.clone(), "acc-pw-own");
    let other = w.mailbox("other-bot", &w.other_ws.clone(), "acc-pw-other");
    let vault = FakeVault::default();

    // Agent without mail-manage: refused at the handler before lookup.
    let r = crate::caller_workspace::with_request_principal(
        Some(World::principal(&w.plain_ws)),
        || {
            crate::mail::routes_addresses::handle_address_password(&body(
                serde_json::json!({ "address": other.address }),
            ))
        },
    );
    assert_needs_mail_manage(&r);

    // IT agent, own mailbox: refused (handler and core), engine untouched.
    let r =
        crate::caller_workspace::with_request_principal(Some(World::principal(&w.it_ws)), || {
            crate::mail::routes_addresses::handle_address_password(&body(
                serde_json::json!({ "address": it_own.address }),
            ))
        });
    assert_self_elevation(&r);
    let engine = FakeAddrEngine::ok();
    let r = crate::mail::routes_addresses::gated_rotate_on(&w.it(), &engine, &vault, &it_own);
    assert_self_elevation(&r);
    assert!(engine.passwords_set.lock().unwrap().is_empty());

    // IT agent, another workspace's mailbox: allowed, shown, recorded.
    let r = crate::mail::routes_addresses::gated_rotate_on(&w.it(), &engine, &vault, &other);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(v["password"].as_str().unwrap().len(), 64);
    assert_eq!(
        marks_for(&other.id),
        vec![(
            KIND_MAILBOX.to_string(),
            String::new(),
            ORIGIN_ROTATED.to_string(),
            Some(w.it_ws.clone())
        )]
    );

    // Owner: the IT agent's own mailbox too.
    let r = crate::mail::routes_addresses::gated_rotate_on(
        &MailCaller::Owner,
        &engine,
        &vault,
        &it_own,
    );
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(
        marks_for(&it_own.id),
        vec![(
            KIND_MAILBOX.to_string(),
            String::new(),
            ORIGIN_ROTATED.to_string(),
            None
        )]
    );
    assert_eq!(engine.passwords_set.lock().unwrap().len(), 2);
}

// ── mint (create response) ──

#[test]
fn mint_password_matrix() {
    let mut w = World::new("mint");
    let mint = |row: &MailAddress| {
        serde_json::json!({
            "ok": true, "id": row.id, "address": row.address, "existing": false,
            "username": row.address, "password": "p".repeat(64),
        })
    };
    let finish = crate::mail::routes_addresses::finish_mint_response;

    // Agent without mail-manage: withheld.
    let row = w.mailbox("plain-bot", &w.plain_ws.clone(), "acc-m1");
    let v = finish(mint(&row), &w.plain(), false);
    assert!(v.get("password").is_none(), "{v}");
    assert_eq!(v["passwordWithheld"], true);
    assert_eq!(marks_for(&row.id)[0].2, ORIGIN_WITHHELD);

    // IT agent, its own (agent) mailbox: withheld.
    let row = w.mailbox("it-bot", &w.it_ws.clone(), "acc-m2");
    let v = finish(mint(&row), &w.it(), false);
    assert!(v.get("password").is_none(), "{v}");
    assert!(!v.to_string().contains(&"p".repeat(64)), "{v}");
    assert_eq!(marks_for(&row.id)[0].2, ORIGIN_WITHHELD);
    assert!(!is_person_mailbox(&row.id));

    // IT agent, a person's mailbox: shown once; flag set; creator recorded.
    let row = w.mailbox("staff", &w.it_ws.clone(), "acc-m3");
    let v = finish(mint(&row), &w.it(), true);
    assert_eq!(v["password"].as_str().unwrap().len(), 64, "{v}");
    assert_eq!(v["person"], true);
    assert!(is_person_mailbox(&row.id));
    assert_eq!(
        marks_for(&row.id),
        vec![(
            KIND_MAILBOX.to_string(),
            String::new(),
            ORIGIN_MINTED.to_string(),
            Some(w.it_ws.clone())
        )]
    );

    // Owner: shown.
    let row = w.mailbox("owner-made", &w.other_ws.clone(), "acc-m4");
    let v = finish(mint(&row), &MailCaller::Owner, false);
    assert_eq!(v["password"].as_str().unwrap().len(), 64);
    assert_eq!(marks_for(&row.id)[0].3, None);

    // Idempotent hit: nothing changes.
    let row = w.mailbox("hit", &w.it_ws.clone(), "acc-m5");
    let mut hit = mint(&row);
    hit.as_object_mut().unwrap().remove("password");
    let v = finish(hit, &w.it(), true);
    assert!(v.get("passwordWithheld").is_none(), "{v}");
    assert!(marks_for(&row.id).is_empty());
    assert!(!is_person_mailbox(&row.id));

    // A person's mailbox needs mail-manage at the handler.
    let r = crate::caller_workspace::with_request_principal(
        Some(World::principal(&w.plain_ws)),
        || {
            crate::mail::routes_addresses::handle_address_create(&body(serde_json::json!({
                "project": w.plain_ws, "localPart": "someone", "person": true
            })))
        },
    );
    assert_needs_mail_manage(&r);
}

// ── keep ──

#[test]
fn keep_matrix() {
    let mut w = World::new("keep");
    let it_own = w.mailbox("it-bot", &w.it_ws.clone(), "acc-k-own");
    let other = w.mailbox("staff", &w.other_ws.clone(), "acc-k-other");
    let engine = MapEngine::with("acc-k-other", &[("ap-old", "iPhone")]);

    // Agent without mail-manage: refused at the handler.
    let r = crate::caller_workspace::with_request_principal(
        Some(World::principal(&w.plain_ws)),
        || handle_credentials_keep(&body(serde_json::json!({ "address": other.address }))),
    );
    assert_needs_mail_manage(&r);

    // IT agent, own mailbox: refused (handler + core).
    let r =
        crate::caller_workspace::with_request_principal(Some(World::principal(&w.it_ws)), || {
            handle_credentials_keep(&body(serde_json::json!({ "address": it_own.address })))
        });
    assert_self_elevation(&r);
    assert!(marks_for(&it_own.id).is_empty());

    // IT agent, another workspace's mailbox: kept with its creator.
    let r =
        crate::caller_workspace::with_request_principal(Some(World::principal(&w.it_ws)), || {
            handle_credentials_keep(&body(serde_json::json!({ "address": other.address })))
        });
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let r = keep_on(
        &w.it(),
        Some(&engine),
        &other,
        "acc-k-other",
        Some("ap-nope"),
    );
    assert_eq!(r.status, "404 Not Found", "{}", r.body);
    let r = keep_on(
        &w.it(),
        Some(&engine),
        &other,
        "acc-k-other",
        Some("ap-old"),
    );
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(
        marks_for(&other.id),
        vec![
            (
                KIND_APP_PASSWORD.to_string(),
                "ap-old".to_string(),
                ORIGIN_KEPT.to_string(),
                Some(w.it_ws.clone())
            ),
            (
                KIND_MAILBOX.to_string(),
                String::new(),
                ORIGIN_KEPT.to_string(),
                Some(w.it_ws.clone())
            ),
        ]
    );

    // Owner: the IT agent's own mailbox too.
    let r = handle_credentials_keep(&body(serde_json::json!({ "address": it_own.address })));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(marks_for(&it_own.id)[0].3, None);
    assert!(engine.destroyed.lock().unwrap().is_empty());
}

// ── person flag ──

#[test]
fn person_flag_matrix() {
    let mut w = World::new("person");
    let it_own = w.mailbox("it-bot", &w.it_ws.clone(), "acc-p-own");
    let other = w.mailbox("other", &w.other_ws.clone(), "acc-p-other");

    let r = crate::caller_workspace::with_request_principal(
        Some(World::principal(&w.plain_ws)),
        || {
            handle_address_person(&body(
                serde_json::json!({ "address": other.address, "person": true }),
            ))
        },
    );
    assert_needs_mail_manage(&r);
    assert!(!is_person_mailbox(&other.id));

    // IT agent can't make its own mailbox a person's (that would hand it
    // the secrets) — but may turn it back off.
    let r =
        crate::caller_workspace::with_request_principal(Some(World::principal(&w.it_ws)), || {
            handle_address_person(&body(
                serde_json::json!({ "address": it_own.address, "person": true }),
            ))
        });
    assert_self_elevation(&r);
    assert!(!is_person_mailbox(&it_own.id));
    let r = set_person_on(&w.it(), &it_own, false);
    assert_eq!(r.status, "200 OK", "{}", r.body);

    // IT agent, another workspace's mailbox: allowed.
    let r = set_person_on(&w.it(), &other, true);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert!(is_person_mailbox(&other.id));

    // Owner: the IT agent's own mailbox too — after which the IT agent
    // may get its secrets.
    let r = handle_address_person(&body(
        serde_json::json!({ "address": it_own.address, "person": true }),
    ));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert!(is_person_mailbox(&it_own.id));
    assert!(credential_gate(&w.it(), &it_own, "x").is_ok());

    let r = handle_address_person(&body(serde_json::json!({ "address": it_own.address })));
    assert_eq!(r.status, "400 Bad Request", "{}", r.body);
}

/// Record that `ws`'s agent sent from `from` (its send identity).
fn seed_sent_from(ws: &str, from: &str) -> String {
    let id = format!("out_{}", uuid::Uuid::new_v4().simple());
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO mail_outbound (id, owner_project_id, agent_name, from_address, to_json, \
         subject, status, created_at, updated_at) \
         VALUES (?1, ?2, 'agent', ?3, '[\"x@dest.example\"]', 's', 'sent', 100, 100)",
        rusqlite::params![id, ws, from],
    )
    .expect("seed outbound");
    id
}

fn drop_outbound(id: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "DELETE FROM mail_outbound WHERE id = ?1",
        rusqlite::params![id],
    )
    .expect("cleanup outbound");
}

#[test]
fn bulk_person_flags_a_workspace_but_never_its_send_identities() {
    let mut w = World::new("pall");
    let it_ws = w.it_ws.clone();
    let other_ws = w.other_ws.clone();
    // The IT workspace: its agent's send identity (has sent from it), its
    // agent's own mint from 0.45.0 (withheld), and three imported staff
    // mailboxes.
    let identity = w.mailbox("it-desk", &it_ws, "acc-pa-id");
    let minted = w.mailbox("it-bot", &it_ws, "acc-pa-minted");
    mark(&minted.id, KIND_MAILBOX, "", ORIGIN_WITHHELD, Some(&it_ws)).unwrap();
    let staff: Vec<MailAddress> = (0..3)
        .map(|i| w.mailbox(&format!("staff{i}"), &it_ws, &format!("acc-pa-s{i}")))
        .collect();
    let out = seed_sent_from(&it_ws, &identity.address.to_uppercase());
    let elsewhere = w.mailbox("other", &other_ws, "acc-pa-other");

    // Before: the doctor names the one owner command for the IT workspace.
    let ws_name = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT name FROM projects WHERE id = ?1",
            rusqlite::params![it_ws],
            |r| r.get::<_, String>(0),
        )
        .unwrap()
    };
    let c = person_check_for(&[(it_ws.clone(), ws_name.clone())]);
    assert_eq!(c.id, PERSON_CHECK_ID);
    assert_eq!(c.status, ST_INFO, "{}", c.detail);
    assert!(
        c.detail.contains(&format!("3 mailbox(es) in '{ws_name}'")),
        "{}",
        c.detail
    );
    assert!(
        c.detail.contains(&format!(
            "k2 hostmail person --all --workspace {ws_name} on"
        )),
        "{}",
        c.detail
    );
    assert!(
        !c.detail.contains(&identity.address),
        "send identity is never listed: {}",
        c.detail
    );

    // An agent without mail-manage: refused. The IT agent on its OWN
    // workspace: refused (self-elevation). Nothing changed.
    assert_needs_mail_manage(&set_person_all_on(&w.plain(), &it_ws, true));
    let r = crate::caller_workspace::with_request_principal(Some(World::principal(&it_ws)), || {
        handle_address_person(&body(serde_json::json!({
            "all": true, "workspace": it_ws, "person": true
        })))
    });
    assert_self_elevation(&r);
    assert!(staff.iter().all(|s| !is_person_mailbox(&s.id)));

    // The IT agent for ANOTHER workspace: allowed.
    let r = set_person_all_on(&w.it(), &other_ws, true);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert!(is_person_mailbox(&elsewhere.id));

    // The owner, once, for the IT workspace (handler, by workspace id).
    let r = handle_address_person(&body(serde_json::json!({
        "all": true, "workspace": it_ws, "person": true
    })));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(v["changed"].as_array().unwrap().len(), 3, "{v}");
    let excluded: Vec<&str> = v["excludedSendIdentities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    assert!(excluded.contains(&identity.address.as_str()), "{v}");
    assert!(excluded.contains(&minted.address.as_str()), "{v}");
    assert!(staff.iter().all(|s| is_person_mailbox(&s.id)));
    assert!(
        !is_person_mailbox(&identity.id),
        "send identity stays the agent's own"
    );
    assert!(
        !is_person_mailbox(&minted.id),
        "the agent's own mint stays its own"
    );

    // Now the IT agent manages staff passwords with no owner step, and its
    // own identity is still protected.
    assert!(credential_gate(&w.it(), &staff[0], "x").is_ok());
    assert_self_elevation(&credential_gate(&w.it(), &identity, "x").unwrap_err());
    assert_self_elevation(&credential_gate(&w.it(), &minted, "x").unwrap_err());
    let c = person_check_for(&[(it_ws.clone(), ws_name.clone())]);
    assert_eq!(c.status, ST_PASS, "{}", c.detail);

    // Usage: 'all' needs a workspace and no address; unknown workspace 404.
    for b in [
        serde_json::json!({ "all": true, "person": true }),
        serde_json::json!({ "all": true, "workspace": it_ws, "address": staff[0].address, "person": true }),
    ] {
        let r = handle_address_person(&body(b.clone()));
        assert_eq!(r.status, "400 Bad Request", "{b}: {}", r.body);
    }
    let r = handle_address_person(&body(serde_json::json!({
        "all": true, "workspace": "no-such-workspace-xyz", "person": true
    })));
    assert_eq!(r.status, "404 Not Found", "{}", r.body);

    // The IT agent may turn its own workspace's flags OFF (tightening).
    let r = set_person_all_on(&w.it(), &it_ws, false);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert!(staff.iter().all(|s| !is_person_mailbox(&s.id)));

    drop_outbound(&out);
}

#[test]
fn send_identities_are_the_addresses_a_workspace_sent_from() {
    let w = World::new("sid");
    let a = seed_sent_from(&w.it_ws, "Desk@Shop.Example");
    let b = seed_sent_from(&w.it_ws, "desk@shop.example");
    let c = seed_sent_from(&w.other_ws, "other@shop.example");
    let ids = send_identities(&w.it_ws);
    assert_eq!(
        ids.into_iter().collect::<Vec<_>>(),
        vec!["desk@shop.example".to_string()]
    );
    for id in [a, b, c] {
        drop_outbound(&id);
    }
}

// ── approvals / policy gates (handlers are tested in routes_send /
// routes_server; these pin the pure rule) ──

#[test]
fn approval_and_policy_gates() {
    let w = World::new("gates");
    assert!(approval_gate(&MailCaller::Owner, Some(&w.it_ws)).is_ok());
    assert!(approval_gate(&w.it(), Some(&w.other_ws)).is_ok());
    assert!(
        approval_gate(&w.it(), None).is_ok(),
        "unknown id → the handler 404s"
    );
    assert_self_elevation(&approval_gate(&w.it(), Some(&w.it_ws)).unwrap_err());
    assert_needs_mail_manage(&approval_gate(&w.plain(), Some(&w.other_ws)).unwrap_err());

    let own_path = crate::workspace_msg::resolve_workspace(&w.it_ws).unwrap();
    let other_path = crate::workspace_msg::resolve_workspace(&w.other_ws).unwrap();
    assert!(policy_gate(&MailCaller::Owner, &own_path).is_ok());
    assert!(policy_gate(&w.it(), &other_path).is_ok());
    assert_self_elevation(&policy_gate(&w.it(), &own_path).unwrap_err());
    assert_needs_mail_manage(&policy_gate(&w.plain(), &other_path).unwrap_err());
}

// ── doctor ──

#[test]
fn doctor_flags_unknown_and_self_made_credentials_only() {
    let mut w = World::new("doc");
    let unknown = w.mailbox("old-bot", &w.other_ws.clone(), "acc-d-unknown");
    let it_for_other = w.mailbox("staff", &w.other_ws.clone(), "acc-d-other");
    let self_made = w.mailbox("it-bot", &w.it_ws.clone(), "acc-d-self");
    let withheld = w.mailbox("it-bot2", &w.it_ws.clone(), "acc-d-withheld");
    let person = w.mailbox("person", &w.it_ws.clone(), "acc-d-person");
    set_person_flag(&person.id, true).unwrap();

    let engine = MapEngine::default();
    engine.rows.lock().unwrap().insert(
        "acc-d-other".to_string(),
        vec![
            ("ap-it".to_string(), "Mail.app".to_string()),
            ("ap-unknown".to_string(), "k2".to_string()),
        ],
    );
    engine.rows.lock().unwrap().insert(
        "acc-d-self".to_string(),
        vec![("ap-self".to_string(), "k2".to_string())],
    );
    // An IT agent made these for others / a person: legitimate.
    mark(
        &it_for_other.id,
        KIND_MAILBOX,
        "",
        ORIGIN_ROTATED,
        Some(&w.it_ws),
    )
    .unwrap();
    mark(
        &it_for_other.id,
        KIND_APP_PASSWORD,
        "ap-it",
        ORIGIN_MINTED,
        Some(&w.it_ws),
    )
    .unwrap();
    mark(&person.id, KIND_MAILBOX, "", ORIGIN_MINTED, Some(&w.it_ws)).unwrap();
    // An agent made these for its OWN mailbox (can't happen from 0.45.0).
    mark(
        &self_made.id,
        KIND_MAILBOX,
        "",
        ORIGIN_MINTED,
        Some(&w.it_ws),
    )
    .unwrap();
    mark(
        &self_made.id,
        KIND_APP_PASSWORD,
        "ap-self",
        ORIGIN_MINTED,
        Some(&w.it_ws),
    )
    .unwrap();
    // Withheld: nobody saw it.
    mark(
        &withheld.id,
        KIND_MAILBOX,
        "",
        ORIGIN_WITHHELD,
        Some(&w.it_ws),
    )
    .unwrap();

    let rows = vec![
        unknown.clone(),
        it_for_other.clone(),
        self_made.clone(),
        withheld.clone(),
        person.clone(),
    ];
    let c = credential_check_for(&rows, Some(&engine));
    assert_eq!(c.id, CHECK_ID);
    assert_eq!(c.status, ST_WARN, "{}", c.detail);
    assert!(!c.gates_direct);
    assert!(
        c.detail.starts_with("4 credential(s) to review"),
        "{}",
        c.detail
    );
    assert!(
        c.detail.contains(&format!(
            "mailbox password of {} — unknown creator",
            unknown.address
        )),
        "{}",
        c.detail
    );
    assert!(c.detail.contains("app password ap-unknown"), "{}", c.detail);
    assert!(
        c.detail.contains(&format!(
            "mailbox password of {} — an agent made it for its own mailbox",
            self_made.address
        )),
        "{}",
        c.detail
    );
    assert!(c.detail.contains("app password ap-self"), "{}", c.detail);
    assert!(!c.detail.contains("ap-it "), "{}", c.detail);
    for ok in [&it_for_other, &withheld, &person] {
        assert!(
            !c.detail
                .contains(&format!("mailbox password of {}", ok.address)),
            "{} must not be flagged: {}",
            ok.address,
            c.detail
        );
    }
    assert!(c.detail.contains("Nothing was revoked"), "{}", c.detail);
    assert!(
        engine.destroyed.lock().unwrap().is_empty(),
        "doctor never revokes"
    );

    // The owner reviews the rest → pass.
    for row in [&unknown, &self_made] {
        mark(&row.id, KIND_MAILBOX, "", ORIGIN_KEPT, None).unwrap();
    }
    mark(
        &it_for_other.id,
        KIND_APP_PASSWORD,
        "ap-unknown",
        ORIGIN_KEPT,
        None,
    )
    .unwrap();
    mark(
        &self_made.id,
        KIND_APP_PASSWORD,
        "ap-self",
        ORIGIN_KEPT,
        None,
    )
    .unwrap();
    let c = credential_check_for(&rows, Some(&engine));
    assert_eq!(c.status, ST_PASS, "{}", c.detail);

    // Engine down: mailbox passwords still checked; app passwords unknown.
    let c = credential_check_for(&rows, None);
    assert_eq!(c.status, ST_UNKNOWN, "{}", c.detail);
    let failing = MapEngine {
        fail_list: Some("JMAP down".to_string()),
        ..Default::default()
    };
    let c = credential_check_for(&rows, Some(&failing));
    assert_eq!(c.status, ST_UNKNOWN, "{}", c.detail);
    assert!(c.detail.contains("JMAP down"), "{}", c.detail);
}

#[test]
fn doctor_detail_caps_the_item_list_and_names_the_bulk_review_first() {
    let mut w = World::new("doc-cap");
    let ws = w.other_ws.clone();
    let rows: Vec<MailAddress> = (0..(DETAIL_ITEM_CAP + 3))
        .map(|i| w.mailbox(&format!("m{i}"), &ws, &format!("acc-cap-{i}")))
        .collect();
    let c = credential_check_for(&rows, Some(&MapEngine::default()));
    assert_eq!(c.status, ST_WARN);
    assert!(c.detail.contains("and 3 more"), "{}", c.detail);
    assert!(
        c.detail
            .starts_with(&format!("{} credential(s)", DETAIL_ITEM_CAP + 3)),
        "{}",
        c.detail
    );
    let bulk = c
        .detail
        .find("k2 hostmail password keep --all-existing")
        .expect("doctor names the bulk review");
    let first_single = c.detail.find("rotate: ").expect("per-item commands listed");
    assert!(
        bulk < first_single,
        "bulk command must come first: {}",
        c.detail
    );
    assert!(c
        .detail
        .contains("k2 hostmail app-password keep --all-existing"));
}

// ── bulk review (--all-existing) ──

#[test]
fn migration_records_the_bulk_review_cutoff() {
    let cutoff = baseline_cutoff().expect("0133 writes the baseline row");
    assert!(cutoff > 1_700_000_000, "cutoff is a unix time: {cutoff}");
    assert!(
        cutoff <= now_secs(),
        "cutoff is when the migration ran: {cutoff}"
    );
}

#[test]
fn bulk_keep_covers_only_credentials_from_before_the_cutoff() {
    let cutoff = 1_000_000;
    let mut w = World::new("bulk");
    let ws = w.other_ws.clone();
    let old = w.mailbox_at("old", &ws, "acc-bulk-old", cutoff - 50);
    let new = w.mailbox_at("new", &ws, "acc-bulk-new", cutoff + 50);
    let withheld = w.mailbox_at("minted", &ws, "acc-bulk-minted", cutoff - 10);
    mark(&withheld.id, KIND_MAILBOX, "", ORIGIN_WITHHELD, Some(&ws)).unwrap();
    let engine = MapEngine::default();
    engine.rows.lock().unwrap().insert(
        "acc-bulk-old".to_string(),
        vec![
            ("ap-before".to_string(), "iPhone".to_string()),
            ("ap-after".to_string(), "laptop".to_string()),
            ("ap-nodate".to_string(), "k2".to_string()),
            ("ap-owner".to_string(), "Mail.app".to_string()),
        ],
    );
    {
        let mut c = engine.created.lock().unwrap();
        let before = chrono::DateTime::from_timestamp(cutoff - 100, 0).unwrap();
        c.insert(
            "ap-before".to_string(),
            serde_json::json!(before.to_rfc3339()),
        );
        c.insert("ap-after".to_string(), serde_json::json!(cutoff + 100));
        c.insert("ap-nodate".to_string(), serde_json::Value::Null);
    }
    mark(&old.id, KIND_APP_PASSWORD, "ap-owner", ORIGIN_MINTED, None).unwrap();
    let rows = vec![old.clone(), new.clone(), withheld.clone()];

    let r = keep_all_existing_on(&MailCaller::Owner, Some(&engine), &rows, cutoff, true, true);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(v["kept"]["mailboxPasswords"], 1, "{v}");
    assert_eq!(v["kept"]["appPasswords"], 1, "{v}");
    assert_eq!(
        v["leftFlagged"]["createdAfterCutoff"], 2,
        "new mailbox + ap-after: {v}"
    );
    assert_eq!(v["leftFlagged"]["noCreationTime"], 1, "{v}");
    assert!(!r.body.contains("secret"), "{}", r.body);

    assert_eq!(mark_of(&old.id, KIND_MAILBOX, "").unwrap().0, ORIGIN_KEPT);
    assert_eq!(
        mark_of(&old.id, KIND_APP_PASSWORD, "ap-before").unwrap().0,
        ORIGIN_KEPT
    );
    assert!(mark_of(&old.id, KIND_APP_PASSWORD, "ap-after").is_none());
    assert!(mark_of(&old.id, KIND_APP_PASSWORD, "ap-nodate").is_none());
    assert!(
        mark_of(&new.id, KIND_MAILBOX, "").is_none(),
        "newer mailbox stays flagged"
    );
    assert_eq!(
        mark_of(&withheld.id, KIND_MAILBOX, "").unwrap().0,
        ORIGIN_WITHHELD,
        "an existing mark is never overwritten"
    );
    assert_eq!(
        mark_of(&old.id, KIND_APP_PASSWORD, "ap-owner").unwrap().0,
        ORIGIN_MINTED
    );
    assert!(
        engine.destroyed.lock().unwrap().is_empty(),
        "bulk keep never revokes"
    );

    let c = credential_check_for(&rows, Some(&engine));
    assert!(c.detail.starts_with("3 credential(s)"), "{}", c.detail);
    assert!(!c.detail.contains("ap-before"), "{}", c.detail);

    // Idempotent; mailbox-only needs no engine; app passwords need one.
    let r = keep_all_existing_on(&MailCaller::Owner, Some(&engine), &rows, cutoff, true, true);
    let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(v["kept"]["mailboxPasswords"], 0, "{v}");
    let r = keep_all_existing_on(&MailCaller::Owner, None, &rows, cutoff, true, false);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let r = keep_all_existing_on(&MailCaller::Owner, None, &rows, cutoff, false, true);
    assert_eq!(r.status, "503 Service Unavailable", "{}", r.body);
}

#[test]
fn bulk_keep_by_an_it_agent_skips_its_own_mailboxes() {
    let cutoff = 1_000_000;
    let mut w = World::new("bulk-it");
    let (it_ws, other_ws) = (w.it_ws.clone(), w.other_ws.clone());
    let own = w.mailbox_at("it-bot", &it_ws, "acc-bi-own", cutoff - 5);
    let person = w.mailbox_at("staff", &it_ws, "acc-bi-person", cutoff - 5);
    set_person_flag(&person.id, true).unwrap();
    let other = w.mailbox_at("other", &other_ws, "acc-bi-other", cutoff - 5);
    let rows = vec![own.clone(), person.clone(), other.clone()];

    let r = keep_all_existing_on(&w.plain(), None, &rows, cutoff, true, false);
    assert_needs_mail_manage(&r);

    let r = keep_all_existing_on(&w.it(), None, &rows, cutoff, true, false);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(v["kept"]["mailboxPasswords"], 2, "person + other: {v}");
    assert_eq!(v["leftFlagged"]["ownMailboxesForTheOwner"], 1, "{v}");
    assert!(
        mark_of(&own.id, KIND_MAILBOX, "").is_none(),
        "own mailbox left for the owner"
    );
    assert_eq!(
        mark_of(&other.id, KIND_MAILBOX, "").unwrap().1.as_deref(),
        Some(it_ws.as_str())
    );

    // The handler: an agent without mail-manage is refused; bad bodies 400.
    let r = crate::caller_workspace::with_request_principal(
        Some(World::principal(&w.plain_ws)),
        || handle_credentials_keep(&body(serde_json::json!({ "allExisting": true }))),
    );
    assert_needs_mail_manage(&r);
    let r = handle_credentials_keep(&body(
        serde_json::json!({ "allExisting": true, "address": own.address }),
    ));
    assert_eq!(r.status, "400 Bad Request", "{}", r.body);
    let r = handle_credentials_keep(&body(
        serde_json::json!({ "allExisting": true, "kind": "everything" }),
    ));
    assert_eq!(r.status, "400 Bad Request", "{}", r.body);
    // (The owner's handler success path would mark every row in the
    // shared test DB and race the other doctor tests; the core above
    // covers it.)
}
