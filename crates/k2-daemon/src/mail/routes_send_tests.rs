use super::*;
use crate::mail::send::NewOutbound;

fn body_json(resp: &CliResponse) -> serde_json::Value {
    serde_json::from_str(&resp.body).expect("valid JSON body")
}

// ── attachment reading (workspace-relative, caps) ──

#[test]
fn read_workspace_attachments_reads_files_and_derives_name_and_type() {
    let root = std::env::temp_dir().join(format!(
        "mail-att-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(root.join("out")).unwrap();
    std::fs::write(root.join("report.pdf"), b"PDF-CONTENT").unwrap();
    std::fs::write(root.join("out/notes.txt"), b"hi").unwrap();
    let root_s = root.to_string_lossy().to_string();

    let atts = match read_workspace_attachments(
        &root_s,
        &["report.pdf".to_string(), "out/notes.txt".to_string()],
    ) {
        Ok(a) => a,
        Err(resp) => panic!("expected both files to read: {}", resp.body),
    };
    assert_eq!(atts.len(), 2);
    assert_eq!(atts[0].filename, "report.pdf");
    assert_eq!(atts[0].content_type, "application/pdf");
    assert_eq!(atts[0].bytes, b"PDF-CONTENT");
    assert_eq!(atts[1].filename, "notes.txt");
    assert!(atts[1].content_type.starts_with("text/plain"));

    // A missing file → 404 not_found naming the path, nothing read.
    let resp = read_workspace_attachments(&root_s, &["ghost.bin".to_string()])
        .err()
        .expect("missing file must be rejected");
    assert_eq!(resp.status, "404 Not Found");
    assert!(body_json(&resp)["error"]["hint"].as_str().unwrap().contains("ghost.bin"));

    // A traversal path → 400 usage.
    let resp = read_workspace_attachments(&root_s, &["../escape.bin".to_string()])
        .err()
        .expect("traversal must be rejected");
    assert_eq!(resp.status, "400 Bad Request");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn read_workspace_attachments_rejects_oversize_files() {
    let root = std::env::temp_dir().join(format!(
        "mail-att-big-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&root).unwrap();
    // One byte over the per-file cap — rejected via metadata, no read.
    let big = vec![0u8; (MAX_ATTACHMENT_BYTES + 1) as usize];
    std::fs::write(root.join("huge.bin"), &big).unwrap();
    let root_s = root.to_string_lossy().to_string();

    let resp = read_workspace_attachments(&root_s, &["huge.bin".to_string()])
        .err()
        .expect("oversize must be rejected");
    assert_eq!(resp.status, "400 Bad Request");
    assert!(body_json(&resp)["error"]["hint"].as_str().unwrap().contains("too large"));
    let _ = std::fs::remove_dir_all(&root);
}

/// Short names double as DOMAIN labels (`bot@<name>.example`).
fn unique(label: &str) -> (String, String) {
    let id = uuid::Uuid::new_v4().simple().to_string();
    let short = &id[..12];
    (
        format!("msr-{label}-{short}"),
        format!("/tmp/mail-send-routes-{label}-{}-{short}", std::process::id()),
    )
}

fn insert_project(name: &str, path: &str) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
        rusqlite::params![id, name, path],
    )
    .expect("insert project row");
    id
}

fn set_gating(path: &str, mode: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "UPDATE projects SET mail_agent_send = ?2 WHERE path = ?1",
        rusqlite::params![path, mode],
    )
    .expect("set gating mode");
}

fn seed_address(project_id: &str, address: &str, account: Option<&str>) {
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO mail_addresses (id, address, domain_id, stalwart_account_id, \
             owner_project_id, status, created_at) VALUES (?1, ?2, 'dom-x', ?3, ?4, 'active', 100)",
        rusqlite::params![id, address, account, project_id],
    )
    .expect("seed address");
}

fn seed_domain(domain: &str, send_mode: &str) {
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO mail_domains (id, domain, send_mode, status, created_at) \
             VALUES (?1, ?2, ?3, 'verified', 100)",
        rusqlite::params![id, domain, send_mode],
    )
    .expect("seed domain");
}

fn cleanup(project_id: &str, domain: Option<&str>) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let _ = conn.execute(
        "DELETE FROM mail_outbound WHERE owner_project_id = ?1",
        rusqlite::params![project_id],
    );
    let _ = conn.execute(
        "DELETE FROM mail_addresses WHERE owner_project_id = ?1",
        rusqlite::params![project_id],
    );
    let _ = conn.execute("DELETE FROM projects WHERE id = ?1", rusqlite::params![project_id]);
    if let Some(d) = domain {
        let _ = conn.execute("DELETE FROM mail_domains WHERE domain = ?1", rusqlite::params![d]);
    }
}

fn outbound_count(project_id: &str) -> i64 {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT COUNT(*) FROM mail_outbound WHERE owner_project_id = ?1",
        rusqlite::params![project_id],
        |r| r.get(0),
    )
    .unwrap_or(-1)
}

fn send_body(path: &str, extra: serde_json::Value) -> Vec<u8> {
    let mut v = serde_json::json!({
        "project": path,
        "to": "human@example.com",
        "subject": "Digest",
        "body": "All green.",
    });
    if let (Some(base), Some(add)) = (v.as_object_mut(), extra.as_object()) {
        for (k, val) in add {
            base.insert(k.clone(), val.clone());
        }
    }
    serde_json::to_vec(&v).expect("body")
}

fn params(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// The no-server tests must not race a seeded `mail_server` row.
fn clear_mail_server() {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let _ = conn.execute("DELETE FROM mail_server WHERE id = 1", []);
}

// ── send validation ──

#[test]
fn send_validates_body_shape_before_any_identity_work() {
    let resp = handle_send(b"not json");
    assert_eq!(resp.status, "400 Bad Request");

    for (bad, needle) in [
        (serde_json::json!({}), "project"),
        (serde_json::json!({ "project": "/tmp/x" }), "to"),
        (serde_json::json!({ "project": "/tmp/x", "to": "a@b.c" }), "subject"),
        (
            serde_json::json!({ "project": "/tmp/x", "to": "a@b.c", "subject": "s" }),
            "body",
        ),
        (
            serde_json::json!({ "project": "/tmp/x", "to": 42,
                                "subject": "s", "body": "b" }),
            "to",
        ),
        (
            // `attachments` must be an array of path strings (shape
            // validated before any identity work).
            serde_json::json!({ "project": "/tmp/x", "to": "a@b.c", "subject": "s",
                                "body": "b", "attachments": "f.pdf" }),
            "must be an array",
        ),
        (
            // The count cap (≤10) fires before identity work too.
            serde_json::json!({ "project": "/tmp/x", "to": "a@b.c", "subject": "s",
                                "body": "b",
                                "attachments": (0..11).map(|i| format!("f{i}.pdf"))
                                    .collect::<Vec<_>>() }),
            "too many attachments",
        ),
        (
            serde_json::json!({ "project": "/tmp/x", "to": "a@b.c", "subject": "s",
                                "body": "b", "wait": true, "timeout": 901 }),
            "timeout",
        ),
    ] {
        let resp = handle_send(&serde_json::to_vec(&bad).unwrap());
        assert_eq!(resp.status, "400 Bad Request", "{bad}");
        let v = body_json(&resp);
        assert!(
            v["error"]["hint"].as_str().unwrap().contains(needle),
            "{bad} → {}",
            resp.body
        );
    }
}

/// D4 DEFAULT: send is OFF — the teaching 403 fires and NO audit
/// row exists (the gate precedes everything).
#[test]
fn send_defaults_gated_off_with_teaching_error_and_no_row() {
    let (name, path) = unique("off");
    let project_id = insert_project(&name, &path);
    seed_address(&project_id, &format!("bot@{name}.example"), Some("acc-1"));
    let resp = handle_send(&send_body(&path, serde_json::json!({})));
    assert_eq!(resp.status, "403 Forbidden", "{}", resp.body);
    let v = body_json(&resp);
    assert_eq!(v["error"]["code"], "gated");
    assert!(
        v["error"]["hint"].as_str().unwrap().contains("`k2 mail sending <workspace> on`"),
        "{v}"
    );
    assert_eq!(outbound_count(&project_id), 0, "off mode never writes");
    cleanup(&project_id, None);
}

/// The full approval round trip through the REAL handlers: queue →
/// outbox pending → approvals list preview → deny-with-note →
/// note reaches the agent's outbox → re-decide conflicts.
#[test]
fn approval_round_trip_queue_outbox_deny_note() {
    // Reaches the singleton mail_server row: hold the mail lock.
    let _mail = crate::mail::mail_server_test_lock();
    let (name, path) = unique("appr");
    let project_id = insert_project(&name, &path);
    let domain = format!("{name}.example");
    let from = format!("bot@{domain}");
    seed_domain(&domain, "direct");
    seed_address(&project_id, &from, Some("acc-1"));
    set_gating(&path, "approval");

    // Queue (no engine dial happens in approval mode — no
    // mail_server row exists and this still succeeds).
    let resp = handle_send(&send_body(&path, serde_json::json!({})));
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    let v = body_json(&resp);
    assert_eq!(v["ok"], true);
    assert_eq!(v["queued"], true);
    assert_eq!(v["status"], "pending_approval");
    let id = v["id"].as_str().expect("id").to_string();
    assert!(id.starts_with("out_"), "{id}");
    assert!(
        v["hint"].as_str().unwrap().contains(&format!("k2 mail outbox {id}")),
        "the track-with-outbox hint names the id: {v}"
    );

    // Outbox: the caller sees it pending, with the status note.
    let resp = handle_outbox(&params(&[("project", &path)]));
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    let v = body_json(&resp);
    assert_eq!(v["count"], 1);
    assert_eq!(v["outbox"][0]["id"], id.as_str());
    assert_eq!(v["outbox"][0]["status"], "pending_approval");
    assert_eq!(v["outbox"][0]["from"], from.as_str());

    // Point lookup (§11.1.9).
    let resp = handle_outbox(&params(&[("project", &path), ("id", &id)]));
    assert_eq!(resp.status, "200 OK");
    assert_eq!(body_json(&resp)["outbound"]["subject"], "Digest");

    // The owner's queue carries the preview fields.
    let resp = handle_approvals_list(&HashMap::new());
    assert_eq!(resp.status, "200 OK");
    let v = body_json(&resp);
    let ours = v["pending"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == id.as_str())
        .expect("our item queued");
    assert_eq!(ours["bodyPreview"], "All green.");
    assert_eq!(ours["workspace"], name.as_str());
    assert_eq!(ours["to"][0], "human@example.com");

    // Deny requires a note; the note reaches the agent.
    let resp = handle_approvals_deny(
        format!(r#"{{"id":"{id}"}}"#).as_bytes(),
    );
    assert_eq!(resp.status, "400 Bad Request", "note is required: {}", resp.body);
    let resp = handle_approvals_deny(
        format!(r#"{{"id":"{id}","note":"use the newsletter address"}}"#).as_bytes(),
    );
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    assert_eq!(body_json(&resp)["status"], "rejected");
    let resp = handle_outbox(&params(&[("project", &path), ("id", &id)]));
    let v = body_json(&resp);
    assert_eq!(v["outbound"]["status"], "rejected");
    assert_eq!(v["outbound"]["note"], "use the newsletter address");
    assert_eq!(v["outbound"]["decidedBy"], "owner");

    // Re-deciding a decided row conflicts — and never submits.
    let resp = handle_approvals_deny(
        format!(r#"{{"id":"{id}","note":"again"}}"#).as_bytes(),
    );
    assert_eq!(resp.status, "409 Conflict", "{}", resp.body);
    let resp = handle_approvals_approve(format!(r#"{{"id":"{id}"}}"#).as_bytes());
    assert_eq!(resp.status, "409 Conflict", "{}", resp.body);
    assert!(body_json(&resp)["error"]["hint"].as_str().unwrap().contains("rejected"));

    cleanup(&project_id, Some(&domain));
}

/// 0.45.0 approvals: an IT agent (mail-manage) decides OTHER
/// workspaces' queued sends, like the owner, but never its own
/// (`owner_only`); an agent without mail-manage never
/// (`agent_send_path_only`); the owner always. Listing stays open.
#[test]
fn approvals_it_agent_for_others_only_owner_always() {
    let mm_on = |id: &str| {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "UPDATE projects SET mail_manage_enabled = 1 WHERE id = ?1",
            rusqlite::params![id],
        )
        .expect("mail-manage on");
    };
    // The SENDER is itself an IT agent's workspace.
    let (name, path) = unique("apprag");
    let sender_id = insert_project(&name, &path);
    mm_on(&sender_id);
    let (it_name, it_path) = unique("apprit");
    let other_it_id = insert_project(&it_name, &it_path);
    mm_on(&other_it_id);
    let (plain_name, plain_path) = unique("apprpl");
    let plain_id = insert_project(&plain_name, &plain_path);

    let domain = format!("{name}.example");
    seed_domain(&domain, "direct");
    seed_address(&sender_id, &format!("bot@{domain}"), Some("acc-1"));
    set_gating(&path, "approval");
    let queue = || {
        let resp = handle_send(&send_body(&path, serde_json::json!({})));
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        body_json(&resp)["id"].as_str().expect("id").to_string()
    };
    let (id1, id2) = (queue(), queue());

    let as_agent = |ws: &str, f: &dyn Fn() -> CliResponse| {
        let p = crate::session_token::HookPrincipal {
            workspace_uuid: ws.to_string(),
            agent_address: "agent".to_string(),
        };
        crate::caller_workspace::with_request_principal(Some(p), f)
    };
    let approve = format!(r#"{{"id":"{id1}"}}"#);
    let deny = format!(r#"{{"id":"{id1}","note":"agent says no"}}"#);
    let status_of = |id: &str| {
        let resp = handle_outbox(&params(&[("project", &path), ("id", id)]));
        body_json(&resp)["outbound"]["status"].as_str().unwrap_or("").to_string()
    };

    // The sender's own IT agent: refused (self-elevation).
    for resp in [
        as_agent(&sender_id, &|| handle_approvals_approve(approve.as_bytes())),
        as_agent(&sender_id, &|| handle_approvals_deny(deny.as_bytes())),
    ] {
        assert_eq!(resp.status, "403 Forbidden", "{}", resp.body);
        let v = body_json(&resp);
        assert_eq!(v["error"]["code"], "owner_only", "{v}");
        assert!(
            v["error"]["hint"]
                .as_str()
                .unwrap()
                .contains("can't loosen mail rules on its own sends"),
            "{v}"
        );
    }
    // An agent without mail-manage: refused.
    for resp in [
        as_agent(&plain_id, &|| handle_approvals_approve(approve.as_bytes())),
        as_agent(&plain_id, &|| handle_approvals_deny(deny.as_bytes())),
    ] {
        assert_eq!(resp.status, "403 Forbidden", "{}", resp.body);
        assert_eq!(body_json(&resp)["error"]["code"], "agent_send_path_only");
    }
    assert_eq!(status_of(&id1), "pending_approval", "refused decisions must not land");

    // Listing stays open to an IT agent.
    let resp = as_agent(&sender_id, &|| handle_approvals_list(&HashMap::new()));
    assert_eq!(resp.status, "200 OK", "{}", resp.body);

    // ANOTHER workspace's IT agent decides it.
    let resp = as_agent(&other_it_id, &|| handle_approvals_deny(deny.as_bytes()));
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    assert_eq!(body_json(&resp)["status"], "rejected");
    assert_eq!(status_of(&id1), "rejected");

    // The owner decides the other one.
    let resp = handle_approvals_deny(
        format!(r#"{{"id":"{id2}","note":"owner says no"}}"#).as_bytes(),
    );
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    assert_eq!(status_of(&id2), "rejected");

    cleanup(&sender_id, Some(&domain));
    cleanup(&other_it_id, None);
    cleanup(&plain_id, None);
}

/// #31.5: a SUCCESSFUL linked SMTP send records an outbox row (no
/// network — we call the recorder the linked success path calls, then
/// read it back through the real outbox handler). The row shows as
/// `submitted` (never "delivered") with `sent_at` stamped.
#[test]
fn linked_send_records_a_submitted_outbox_row() {
    let (name, path) = unique("linked");
    let project_id = insert_project(&name, &path);
    assert_eq!(outbound_count(&project_id), 0);

    let store = DbOutboundStore::default();
    let msg = OutboundMessage {
        from_name: Some("Agent".to_string()),
        from: "me@gmail.com".to_string(),
        to: vec!["pat@dest.example".to_string()],
        cc: Vec::new(),
        bcc: Vec::new(),
        policy_bcc: Vec::new(),
        reply_to: Vec::new(),
        subject: "Hi via linked".to_string(),
        text_body: "the linked body".to_string(),
        in_reply_to: None,
        references: None,
        reply_source: None,
        attachments: Vec::new(),
        ephemeral: false,
        notify: None,
        forward_of: None,
        source_dmarc: None,
        source_check: None,
    };
    let id = send::record_linked_submitted(&store, &project_id, "Agent", &msg, &[], now_secs())
        .expect("record linked send");
    assert!(id.starts_with("out_"), "{id}");
    assert_eq!(outbound_count(&project_id), 1, "one audit row after a linked send");

    // The agent sees it via `k2 mail outbox <id>` — submitted, to the
    // recipient, with a stamped sent_at, never "delivered".
    let resp = handle_outbox(&params(&[("project", &path), ("id", &id)]));
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    let v = body_json(&resp);
    assert_eq!(v["outbound"]["status"], "submitted");
    assert_ne!(v["outbound"]["status"], "delivered");
    assert_eq!(v["outbound"]["to"][0], "pat@dest.example");
    assert_eq!(v["outbound"]["subject"], "Hi via linked");
    assert!(
        v["outbound"]["sentAt"].as_i64().is_some(),
        "sent_at is stamped: {}",
        resp.body
    );
    // A no-attachment send surfaces NO attachments summary.
    assert!(
        v["outbound"].get("attachments").is_none(),
        "no attachments key for a plain send: {}",
        resp.body
    );

    cleanup(&project_id, None);
}

/// #31.5 follow-up: a linked send WITH attachments records the
/// filenames in the outbox trail (`attachments_ref` = the JSON of the
/// basenames), and the outbox read surfaces `{count, names}`. Never
/// bytes, never paths — filenames only.
#[test]
fn linked_send_records_attachment_filenames_in_the_outbox_trail() {
    let (name, path) = unique("linkatt");
    let project_id = insert_project(&name, &path);

    let store = DbOutboundStore::default();
    let msg = OutboundMessage {
        from_name: Some("Agent".to_string()),
        from: "me@gmail.com".to_string(),
        to: vec!["pat@dest.example".to_string()],
        cc: Vec::new(),
        bcc: Vec::new(),
        policy_bcc: Vec::new(),
        reply_to: Vec::new(),
        subject: "Here are the files".to_string(),
        text_body: "see attached".to_string(),
        in_reply_to: None,
        references: None,
        reply_source: None,
        attachments: Vec::new(),
        ephemeral: false,
        notify: None,
        forward_of: None,
        source_dmarc: None,
        source_check: None,
    };
    let names = vec!["invoice.pdf".to_string(), "photo.jpg".to_string()];
    let id = send::record_linked_submitted(
        &store, &project_id, "Agent", &msg, &names, now_secs(),
    )
    .expect("record linked send with attachments");

    // The stored row's attachments_ref is EXACTLY the two basenames —
    // no bytes, no paths.
    let stored: String = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT attachments_ref FROM mail_outbound WHERE id = ?1",
            rusqlite::params![id],
            |r| r.get(0),
        )
        .expect("attachments_ref present")
    };
    assert_eq!(stored, r#"["invoice.pdf","photo.jpg"]"#, "basenames only: {stored}");

    // The outbox read surfaces the {count, names} summary.
    let resp = handle_outbox(&params(&[("project", &path), ("id", &id)]));
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    let v = body_json(&resp);
    assert_eq!(v["outbound"]["attachments"]["count"], 2);
    assert_eq!(v["outbound"]["attachments"]["names"][0], "invoice.pdf");
    assert_eq!(v["outbound"]["attachments"]["names"][1], "photo.jpg");

    cleanup(&project_id, None);
}

/// Approving with NO mail server: the row flips approved → failed
/// (loud, audited) and nothing dials out — never silently
/// approved-forever, never a phantom send.
#[test]
fn approve_without_engine_marks_failed_loudly() {
    let _g = crate::mail::mail_server_test_lock();
    clear_mail_server();
    let (name, path) = unique("apfail");
    let project_id = insert_project(&name, &path);
    let domain = format!("{name}.example");
    let from = format!("bot@{domain}");
    seed_domain(&domain, "direct");
    seed_address(&project_id, &from, Some("acc-1"));
    set_gating(&path, "approval");

    let resp = handle_send(&send_body(&path, serde_json::json!({})));
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    let id = body_json(&resp)["id"].as_str().unwrap().to_string();

    let resp = handle_approvals_approve(format!(r#"{{"id":"{id}"}}"#).as_bytes());
    assert_eq!(resp.status, "502 Bad Gateway", "{}", resp.body);
    let v = body_json(&resp);
    assert_eq!(v["status"], "failed");
    assert!(v["error"]["hint"].as_str().unwrap().contains("submission failed"));
    let resp = handle_outbox(&params(&[("project", &path), ("id", &id)]));
    let v = body_json(&resp);
    assert_eq!(v["outbound"]["status"], "failed");
    assert!(
        v["outbound"]["note"].as_str().unwrap().contains("mail server unavailable"),
        "{v}"
    );

    // Unknown id → masked not_found.
    let resp = handle_approvals_approve(br#"{"id":"out_nope00000001"}"#);
    assert_eq!(resp.status, "404 Not Found");

    cleanup(&project_id, Some(&domain));
}

/// On-mode with no mail server: 503 BEFORE any audit row (nothing
/// submitted, nothing phantom-recorded).
#[test]
fn on_mode_without_engine_answers_503_and_writes_nothing() {
    let _g = crate::mail::mail_server_test_lock();
    clear_mail_server();
    let (name, path) = unique("onmode");
    let project_id = insert_project(&name, &path);
    let domain = format!("{name}.example");
    seed_domain(&domain, "direct");
    seed_address(&project_id, &format!("bot@{domain}"), Some("acc-1"));
    set_gating(&path, "on");
    let resp = handle_send(&send_body(&path, serde_json::json!({})));
    assert_eq!(resp.status, "503 Service Unavailable", "{}", resp.body);
    assert_eq!(body_json(&resp)["error"]["code"], "not_ready");
    assert_eq!(outbound_count(&project_id), 0);
    cleanup(&project_id, Some(&domain));
}

/// D1: a receive-only domain refuses in EVERY gating mode, with the
/// teaching error, before any row.
#[test]
fn receive_only_domain_refuses_sends() {
    let (name, path) = unique("ro");
    let project_id = insert_project(&name, &path);
    let domain = format!("{name}.example");
    seed_domain(&domain, "receive-only");
    seed_address(&project_id, &format!("bot@{domain}"), Some("acc-1"));
    set_gating(&path, "approval");
    let resp = handle_send(&send_body(&path, serde_json::json!({})));
    assert_eq!(resp.status, "409 Conflict", "{}", resp.body);
    let v = body_json(&resp);
    assert_eq!(v["error"]["code"], "send_mode");
    assert!(v["error"]["hint"].as_str().unwrap().contains("receive-only"), "{v}");
    assert_eq!(outbound_count(&project_id), 0);
    cleanup(&project_id, Some(&domain));
}

/// Sender identity is server-stamped: a foreign `from` answers the
/// S3 masked not_found; no addresses is the teaching not_found;
/// multiple addresses demand an explicit `from`.
#[test]
fn from_resolution_masks_foreign_and_demands_explicit_on_ambiguity() {
    let (name, path) = unique("from");
    let project_id = insert_project(&name, &path);
    let domain = format!("{name}.example");
    seed_domain(&domain, "direct");
    set_gating(&path, "approval");

    // No addresses yet → teaching not_found.
    let resp = handle_send(&send_body(&path, serde_json::json!({})));
    assert_eq!(resp.status, "404 Not Found", "{}", resp.body);
    assert!(body_json(&resp)["error"]["hint"]
        .as_str()
        .unwrap()
        .contains("k2 hostmail create"));

    // A FOREIGN from → masked.
    let (name2, path2) = unique("from-f");
    let project2 = insert_project(&name2, &path2);
    let foreign = format!("theirs@{name2}.example");
    seed_address(&project2, &foreign, Some("acc-f"));
    let resp = handle_send(&send_body(&path, serde_json::json!({ "from": foreign })));
    assert_eq!(resp.status, "404 Not Found", "{}", resp.body);

    // Two addresses without `from` → usage naming the fix.
    seed_address(&project_id, &format!("a@{domain}"), Some("acc-1"));
    seed_address(&project_id, &format!("b@{domain}"), Some("acc-2"));
    let resp = handle_send(&send_body(&path, serde_json::json!({})));
    assert_eq!(resp.status, "400 Bad Request", "{}", resp.body);
    assert!(body_json(&resp)["error"]["hint"].as_str().unwrap().contains("from"));

    // Recipient cap: 11 recipients refuse.
    let many: Vec<String> = (0..11).map(|i| format!("r{i}@example.com")).collect();
    let resp = handle_send(&send_body(
        &path,
        serde_json::json!({ "from": format!("a@{domain}"), "to": many }),
    ));
    assert_eq!(resp.status, "400 Bad Request", "{}", resp.body);
    assert!(body_json(&resp)["error"]["hint"].as_str().unwrap().contains("max 10"));

    cleanup(&project_id, Some(&domain));
    cleanup(&project2, None);
}

// ── fromName (`k2 mail send --from-name`) ──

/// A bad `fromName` is a usage refusal BEFORE any identity work (the
/// project here doesn't exist) — nothing is resolved, queued or sent.
#[test]
fn send_from_name_refusals_come_before_identity_work() {
    let long = "a".repeat(send::MAX_FROM_NAME_CHARS + 1);
    for (from_name, needle) in [
        (serde_json::json!("Laura\r\nBcc: evil@example.com"), "control or invisible"),
        (serde_json::json!("Laura\n"), "control or invisible"),
        (serde_json::json!(long), "too long"),
        (serde_json::json!("ceo@bank.com"), "can't contain '@'"),
        (serde_json::json!("   "), "fromName is empty"),
        (serde_json::json!(42), "must be a string"),
        (serde_json::json!(["Laura"]), "must be a string"),
    ] {
        let body = send_body(
            "/tmp/no-such-workspace-for-from-name",
            serde_json::json!({ "fromName": from_name.clone() }),
        );
        let resp = handle_send(&body);
        assert_eq!(resp.status, "400 Bad Request", "{from_name} → {}", resp.body);
        let v = body_json(&resp);
        assert_eq!(v["error"]["code"], "usage", "{from_name} → {v}");
        let hint = v["error"]["hint"].as_str().expect("hint");
        assert!(hint.contains(needle), "{from_name} → {hint}");
    }
}

/// `fromName` reaches the stored (= sent) message and the approver's
/// queue; the From ADDRESS stays the server-stamped one. Without it the
/// workspace's agent name is used, as before.
#[test]
fn send_from_name_reaches_the_message_and_the_approvals_queue() {
    let _mail = crate::mail::mail_server_test_lock();
    let (name, path) = unique("fname");
    let project_id = insert_project(&name, &path);
    let domain = format!("{name}.example");
    let from = format!("laura@{domain}");
    seed_domain(&domain, "direct");
    seed_address(&project_id, &from, Some("acc-1"));
    set_gating(&path, "approval");
    let store = DbOutboundStore::default();

    let resp = handle_send(&send_body(
        &path,
        serde_json::json!({ "fromName": "  Smith, \"LJ\" Laura  " }),
    ));
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    let id = body_json(&resp)["id"].as_str().expect("id").to_string();
    let stored = store.load_message(&id).expect("stored message");
    assert_eq!(stored.from_name.as_deref(), Some("Smith, \"LJ\" Laura"));
    assert_eq!(stored.from, from, "the address is unchanged by a display name");

    let resp = handle_approvals_list(&params(&[]));
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    let v = body_json(&resp);
    let row = v["pending"]
        .as_array()
        .expect("pending list")
        .iter()
        .find(|r| r["id"] == id.as_str())
        .unwrap_or_else(|| panic!("queued row {id} missing from approvals: {v}"))
        .clone();
    assert_eq!(row["fromName"], "Smith, \"LJ\" Laura", "{row}");
    assert_eq!(row["from"], from.as_str(), "{row}");

    // No fromName → the workspace's agent name (unchanged default).
    let resp = handle_send(&send_body(&path, serde_json::json!({})));
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    let id2 = body_json(&resp)["id"].as_str().expect("id").to_string();
    let stored2 = store.load_message(&id2).expect("stored message");
    assert_eq!(
        stored2.from_name,
        Some(k2_core::workspace::display::agent_display_name(&path))
    );
    // An explicit null is the same as absent.
    let resp = handle_send(&send_body(&path, serde_json::json!({ "fromName": null })));
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    cleanup(&project_id, Some(&domain));
}

/// Governance unchanged: a display name never widens WHO may send.
/// A foreign `from` is still masked (and nothing is audited), an
/// ambiguous sender still needs `from`, and send=off still refuses.
#[test]
fn send_from_name_does_not_change_the_sender_rules() {
    let (name, path) = unique("fname-gov");
    let project_id = insert_project(&name, &path);
    let domain = format!("{name}.example");
    seed_domain(&domain, "direct");
    set_gating(&path, "approval");
    let (name2, path2) = unique("fname-f");
    let project2 = insert_project(&name2, &path2);
    let foreign = format!("theirs@{name2}.example");
    seed_address(&project2, &foreign, Some("acc-f"));

    // Foreign from + a name that matches the foreign mailbox → masked.
    let resp = handle_send(&send_body(
        &path,
        serde_json::json!({ "from": foreign, "fromName": "Theirs" }),
    ));
    assert_eq!(resp.status, "404 Not Found", "{}", resp.body);
    assert_eq!(outbound_count(&project_id), 0);
    assert_eq!(outbound_count(&project2), 0);

    // Two own addresses, no from → still the usage refusal.
    seed_address(&project_id, &format!("a@{domain}"), Some("acc-1"));
    seed_address(&project_id, &format!("b@{domain}"), Some("acc-2"));
    let resp = handle_send(&send_body(&path, serde_json::json!({ "fromName": "Laura" })));
    assert_eq!(resp.status, "400 Bad Request", "{}", resp.body);
    assert!(body_json(&resp)["error"]["hint"].as_str().unwrap().contains("pass 'from'"));

    // send=off still refuses with the teaching 403.
    set_gating(&path, "off");
    let resp = handle_send(&send_body(
        &path,
        serde_json::json!({ "from": format!("a@{domain}"), "fromName": "Laura" }),
    ));
    assert_eq!(resp.status, "403 Forbidden", "{}", resp.body);
    assert_eq!(body_json(&resp)["error"]["code"], "gated");
    assert_eq!(outbound_count(&project_id), 0);

    cleanup(&project_id, Some(&domain));
    cleanup(&project2, None);
}

// ── fromName on replies (`k2 mail reply --from-name`, 0.46.4) ──

fn reply_body(path: &str, token: &str, extra: serde_json::Value) -> Vec<u8> {
    let mut v = serde_json::json!({ "project": path, "id": token, "body": "thanks!" });
    if let (Some(base), Some(add)) = (v.as_object_mut(), extra.as_object()) {
        for (k, val) in add {
            base.insert(k.clone(), val.clone());
        }
    }
    serde_json::to_vec(&v).expect("body")
}

/// A bad `fromName` on a reply is the same usage refusal as on send,
/// BEFORE any identity work (the project here doesn't exist).
#[test]
fn reply_from_name_refusals_come_before_identity_work() {
    for (from_name, needle) in [
        (serde_json::json!("Laura\r\nBcc: evil@example.com"), "control or invisible"),
        (serde_json::json!("ceo@bank.com"), "can't contain '@'"),
        (serde_json::json!("   "), "fromName is empty"),
        (serde_json::json!(7), "must be a string"),
    ] {
        let resp = handle_reply(&reply_body(
            "/tmp/no-such-workspace-for-reply-from-name",
            "garbage-token",
            serde_json::json!({ "fromName": from_name.clone() }),
        ));
        assert_eq!(resp.status, "400 Bad Request", "{from_name} → {}", resp.body);
        let v = body_json(&resp);
        assert_eq!(v["error"]["code"], "usage", "{from_name} → {v}");
        let hint = v["error"]["hint"].as_str().expect("hint");
        assert!(hint.contains(needle), "{from_name} → {hint}");
    }
}

/// Oaks' report (0.46.3): a reply from laura@ showed the workspace name.
/// `fromName` now reaches the stored (= sent) HOSTED reply; the From
/// ADDRESS stays locked to the receiving one and the recipient to the
/// original sender. Without it the agent name is used, as before.
#[test]
fn hosted_reply_from_name_reaches_the_stored_message() {
    use crate::mail::jmap::tests::{spawn_mock_server, NORMAL_SESSION_FIXTURE};
    // Env lock first (the guard), then the mail lock.
    let _key = k2_core::test_env::EnvVar::set("K2_TEST_MAIL_API_KEY", "k2-test-key");
    let _mail = crate::mail::mail_server_test_lock();
    let (name, path) = unique("rfname");
    let project_id = insert_project(&name, &path);
    let domain = format!("{name}.example");
    let laura = format!("laura@{domain}");
    seed_domain(&domain, "direct");
    seed_address(&project_id, &laura, Some("acc-1"));
    set_gating(&path, "approval");
    let store = DbOutboundStore::default();

    // One Email/get reply per handle_reply call (each builds a fresh
    // client: session + Email/get). Distinct threads so the per-thread
    // reply cap never interferes.
    let email_get = |id: &str, thread: &str| {
        serde_json::json!({
            "methodResponses": [["Email/get", {
                "accountId": "acc-1",
                "list": [{
                    "id": id,
                    "threadId": thread,
                    "from": [{ "name": "Customer", "email": "cust@dest.example" }],
                    "subject": "Quote",
                    "header:Message-ID:asText": format!("<{id}@dest.example>"),
                    "header:References:asText": null,
                    "header:Authentication-Results:asText:all": [],
                }],
                "notFound": [],
            }, "0"]]
        })
        .to_string()
    };
    let (port, _rx) = spawn_mock_server(vec![
        NORMAL_SESSION_FIXTURE.to_string(),
        email_get("M1", "T1"),
        NORMAL_SESSION_FIXTURE.to_string(),
        email_get("M2", "T2"),
    ]);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = conn.execute("DELETE FROM mail_server WHERE id = 1", []);
        conn.execute(
            "INSERT INTO mail_server (id, status, pinned_version, api_url, api_key_ref, updated_at) \
                 VALUES (1, 'running', '0.16.10', ?1, 'env:K2_TEST_MAIL_API_KEY', 1)",
            rusqlite::params![format!("http://127.0.0.1:{port}")],
        )
        .expect("mail_server");
    }

    let resp = handle_reply(&reply_body(
        &path,
        &messages::encode_message_id(&laura, "M1"),
        serde_json::json!({ "fromName": "  Laura  " }),
    ));
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    let id = body_json(&resp)["id"].as_str().expect("id").to_string();
    let stored = store.load_message(&id).expect("stored reply");
    assert_eq!(stored.from_name.as_deref(), Some("Laura"));
    assert_eq!(stored.from, laura, "the address stays locked to the receiving one");
    assert_eq!(stored.to, vec!["cust@dest.example".to_string()], "recipient locked");
    assert_eq!(stored.in_reply_to.as_deref(), Some("<M1@dest.example>"));

    // No fromName → the workspace's agent name (unchanged default).
    let resp = handle_reply(&reply_body(
        &path,
        &messages::encode_message_id(&laura, "M2"),
        serde_json::json!({}),
    ));
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    let id2 = body_json(&resp)["id"].as_str().expect("id").to_string();
    let stored2 = store.load_message(&id2).expect("stored reply");
    assert_eq!(
        stored2.from_name,
        Some(k2_core::workspace::display::agent_display_name(&path))
    );

    clear_mail_server();
    cleanup(&project_id, Some(&domain));
}

/// The reply route reads the verdict with this box's host name: a stamped
/// failure blocks, whatever else the line holds, and nothing is recorded.
#[test]
fn hosted_reply_gate_reads_this_boxes_stamp() {
    use crate::mail::jmap::tests::{spawn_mock_server, NORMAL_SESSION_FIXTURE};
    let _key = k2_core::test_env::EnvVar::set("K2_TEST_MAIL_API_KEY", "k2-test-key");
    let _mail = crate::mail::mail_server_test_lock();
    let _row = crate::mail::MailServerRowCleanup;
    let (name, path) = unique("rdmarc");
    let project_id = insert_project(&name, &path);
    let domain = format!("{name}.example");
    let laura = format!("laura@{domain}");
    seed_domain(&domain, "direct");
    seed_address(&project_id, &laura, Some("acc-1"));
    set_gating(&path, "on");
    let email_get = |id: &str, auth: &str| {
        serde_json::json!({
            "methodResponses": [["Email/get", {
                "accountId": "acc-1",
                "list": [{
                    "id": id,
                    "threadId": format!("t-{id}"),
                    "from": [{ "email": "cust@dest.example" }],
                    "subject": "Quote",
                    "header:Message-ID:asText": format!("<{id}@dest.example>"),
                    "header:References:asText": null,
                    "header:Authentication-Results:asText:all": [auth],
                }],
                "notFound": [],
            }, "0"]]
        })
        .to_string()
    };
    let (port, _rx) = spawn_mock_server(vec![
        NORMAL_SESSION_FIXTURE.to_string(),
        email_get(
            "M1",
            "mail.box.test; spf=none smtp.mailfrom=\"dmarc=pass@evil.example\"; dmarc=fail header.from=gmail.com",
        ),
    ]);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = conn.execute("DELETE FROM mail_server WHERE id = 1", []);
        conn.execute(
            "INSERT INTO mail_server (id, status, pinned_version, hostname, api_url, api_key_ref, updated_at) \
                 VALUES (1, 'running', '0.16.25', 'mail.box.test', ?1, 'env:K2_TEST_MAIL_API_KEY', 1)",
            rusqlite::params![format!("http://127.0.0.1:{port}")],
        )
        .expect("mail_server");
    }
    let resp = handle_reply(&reply_body(&path, &messages::encode_message_id(&laura, "M1"), serde_json::json!({})));
    assert_eq!(resp.status, "409 Conflict", "{}", resp.body);
    assert_eq!(body_json(&resp)["error"]["code"], "guardrail", "{}", resp.body);
    assert_eq!(outbound_count(&project_id), 0, "nothing recorded");
    cleanup(&project_id, Some(&domain));
}

/// The always-on hourly cap through the real store: the 21st send
/// inside an hour answers 429 with the distinct window marker.
#[test]
fn hourly_rate_limit_enforced_through_the_route() {
    let (name, path) = unique("rate");
    let project_id = insert_project(&name, &path);
    let domain = format!("{name}.example");
    let from = format!("bot@{domain}");
    seed_domain(&domain, "direct");
    seed_address(&project_id, &from, Some("acc-1"));
    set_gating(&path, "approval");

    // 20 audit rows in the current hour, straight through the
    // production store.
    let store = DbOutboundStore::default();
    let msg = OutboundMessage {
        from_name: None,
        from: from.clone(),
        to: vec!["x@example.com".to_string()],
        cc: vec![],
        bcc: Vec::new(),
        policy_bcc: Vec::new(),
        reply_to: Vec::new(),
        subject: "s".to_string(),
        text_body: "b".to_string(),
        in_reply_to: None,
        references: None,
        reply_source: None,
        attachments: Vec::new(),
        ephemeral: false,
        notify: None,
        forward_of: None,
        source_dmarc: None,
        source_check: None,
    };
    let now = now_secs();
    for _ in 0..send::RATE_LIMIT_HOURLY {
        store
            .insert(&NewOutbound {
                owner_project_id: &project_id,
                agent_name: "bot",
                message: &msg,
                status: "pending",
                decided_by: None,
                attachment_names: &[],
                attachment_files: &[],
                send_after: None,
                now,
            })
            .expect("seed audit row");
    }
    let resp = handle_send(&send_body(&path, serde_json::json!({})));
    assert_eq!(resp.status, "429 Too Many Requests", "{}", resp.body);
    let v = body_json(&resp);
    assert_eq!(v["error"]["code"], "rate_limited");
    assert_eq!(v["error"]["window"], "hour");
    cleanup(&project_id, Some(&domain));
}

/// `--wait` claims a slot BEFORE queueing: with the workspace's
/// slots held, nothing is queued and the caller gets the 429.
#[test]
fn send_wait_refuses_without_a_slot_and_queues_nothing() {
    let (name, path) = unique("waitslot");
    let project_id = insert_project(&name, &path);
    let domain = format!("{name}.example");
    seed_domain(&domain, "direct");
    seed_address(&project_id, &format!("bot@{domain}"), Some("acc-1"));
    set_gating(&path, "approval");
    let _held: Vec<_> = (0..4)
        .map(|_| WaitSlot::try_acquire(&project_id).expect("slot"))
        .collect();
    let resp = handle_send(&send_body(&path, serde_json::json!({ "wait": true })));
    assert_eq!(resp.status, "429 Too Many Requests", "{}", resp.body);
    assert_eq!(outbound_count(&project_id), 0, "no phantom queue item");
    cleanup(&project_id, Some(&domain));
}

// ── reply ──

#[test]
fn reply_gates_validates_and_masks_before_any_engine_dial() {
    let _g = crate::mail::mail_server_test_lock();
    clear_mail_server();
    let resp = handle_reply(b"{}");
    assert_eq!(resp.status, "400 Bad Request");

    let (name, path) = unique("reply");
    let project_id = insert_project(&name, &path);
    let domain = format!("{name}.example");
    seed_domain(&domain, "direct");
    let mine = format!("bot@{domain}");
    seed_address(&project_id, &mine, Some("acc-1"));

    let token = messages::encode_message_id(&mine, "M1");
    // Default gating (off) refuses the reply FIRST — even though no
    // mail server exists (no engine dial before the gate).
    let resp = handle_reply(
        serde_json::to_vec(&serde_json::json!({
            "project": path, "id": token, "body": "thanks!",
        }))
        .unwrap()
        .as_slice(),
    );
    assert_eq!(resp.status, "403 Forbidden", "{}", resp.body);
    assert_eq!(body_json(&resp)["error"]["code"], "gated");

    set_gating(&path, "approval");
    // Malformed id → usage.
    let resp = handle_reply(
        serde_json::to_vec(&serde_json::json!({
            "project": path, "id": "garbage", "body": "x",
        }))
        .unwrap()
        .as_slice(),
    );
    assert_eq!(resp.status, "400 Bad Request");

    // Foreign id → masked (ownership precedes the engine).
    let (name2, path2) = unique("reply-f");
    let project2 = insert_project(&name2, &path2);
    let foreign = format!("theirs@{name2}.example");
    seed_address(&project2, &foreign, Some("acc-f"));
    let ftoken = messages::encode_message_id(&foreign, "M1");
    let resp = handle_reply(
        serde_json::to_vec(&serde_json::json!({
            "project": path, "id": ftoken, "body": "x",
        }))
        .unwrap()
        .as_slice(),
    );
    assert_eq!(resp.status, "404 Not Found", "{}", resp.body);
    assert!(
        !body_json(&resp)["error"]["hint"].as_str().unwrap().contains(&foreign),
        "masked hint must not leak the address"
    );

    // Owned id + no mail server → 503 (the reply context needs the
    // engine); nothing queued.
    let resp = handle_reply(
        serde_json::to_vec(&serde_json::json!({
            "project": path, "id": token, "body": "x",
        }))
        .unwrap()
        .as_slice(),
    );
    assert_eq!(resp.status, "503 Service Unavailable", "{}", resp.body);
    assert_eq!(outbound_count(&project_id), 0);

    cleanup(&project_id, Some(&domain));
    cleanup(&project2, None);
}

// ── outbox reads ──

#[test]
fn outbox_validates_and_masks_point_lookups() {
    let resp = handle_outbox(&HashMap::new());
    assert_eq!(resp.status, "400 Bad Request");

    let (name, path) = unique("outbox");
    let project_id = insert_project(&name, &path);
    for bad in ["0", "abc"] {
        let resp = handle_outbox(&params(&[("project", &path), ("limit", bad)]));
        assert_eq!(resp.status, "400 Bad Request", "limit={bad}");
    }
    // Empty outbox is a 200, not an error.
    let resp = handle_outbox(&params(&[("project", &path)]));
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    assert_eq!(body_json(&resp)["count"], 0);

    // Unknown id and FOREIGN id answer the same masked not_found.
    let resp = handle_outbox(&params(&[("project", &path), ("id", "out_nope")]));
    assert_eq!(resp.status, "404 Not Found");
    let (name2, path2) = unique("outbox-f");
    let project2 = insert_project(&name2, &path2);
    let store = DbOutboundStore::default();
    let msg = OutboundMessage {
        from_name: None,
        from: format!("theirs@{name2}.example"),
        to: vec!["x@example.com".to_string()],
        cc: vec![],
        bcc: Vec::new(),
        policy_bcc: Vec::new(),
        reply_to: Vec::new(),
        subject: "their secret subject".to_string(),
        text_body: "b".to_string(),
        in_reply_to: None,
        references: None,
        reply_source: None,
        attachments: Vec::new(),
        ephemeral: false,
        notify: None,
        forward_of: None,
        source_dmarc: None,
        source_check: None,
    };
    let foreign_id = store
        .insert(&NewOutbound {
            owner_project_id: &project2,
            agent_name: "bot",
            message: &msg,
            status: "pending",
            decided_by: None,
            attachment_names: &[],
            attachment_files: &[],
            send_after: None,
            now: now_secs(),
        })
        .expect("seed foreign row");
    let resp = handle_outbox(&params(&[("project", &path), ("id", &foreign_id)]));
    assert_eq!(resp.status, "404 Not Found", "{}", resp.body);
    assert!(
        !resp.body.contains("secret subject"),
        "masked lookups never leak foreign content"
    );

    cleanup(&project_id, None);
    cleanup(&project2, None);
}

// ── LINKED send (§17.5 / E4) — routing + mail_agent_send gate ──

/// Seed a linked inbox bound to `project_id` at `primary_level`.
fn seed_linked(project_id: &str, address: &str, primary_level: &str) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO mail_external_inboxes (id, owner_project_id, email_address, host, \
             port, username, created_at, primary_level) \
             VALUES (?1, ?2, ?3, 'imap.example.com', 993, ?3, 100, ?4)",
        rusqlite::params![id, project_id, address, primary_level],
    )
    .expect("seed linked inbox");
    id
}

fn cleanup_linked(project_id: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let _ = conn.execute(
        "DELETE FROM mail_inbox_grants WHERE inbox_id IN \
             (SELECT id FROM mail_external_inboxes WHERE owner_project_id = ?1)",
        rusqlite::params![project_id],
    );
    let _ = conn.execute(
        "DELETE FROM mail_external_inboxes WHERE owner_project_id = ?1",
        rusqlite::params![project_id],
    );
    let _ = conn.execute("DELETE FROM projects WHERE id = ?1", rusqlite::params![project_id]);
}

/// E4: linked send honors `mail_agent_send` — off → 403 `gated`
/// (same teaching as hosted), never reaches SMTP/credentials.
#[test]
fn linked_send_is_gated_off_like_hosted() {
    let (name, path) = unique("lsend");
    let project_id = insert_project(&name, &path);
    let linked = format!("me-{}@linked.example", &project_id[..8]);
    seed_linked(&project_id, &linked, "send");
    set_gating(&path, "off");
    let resp = handle_send(&send_body(&path, serde_json::json!({ "from": linked })));
    assert_eq!(resp.status, "403 Forbidden", "linked send must honor off: {}", resp.body);
    let v = body_json(&resp);
    assert_eq!(v["error"]["code"], "gated");
    let hint = v["error"]["hint"].as_str().unwrap();
    assert!(hint.contains("`k2 mail sending <workspace> on`"), "{hint}");
    assert!(hint.contains("Workspaces / Agents → <workspace> → Email sending"), "{hint}");
    assert_eq!(outbound_count(&project_id), 0, "off mode never audits linked");
    cleanup_linked(&project_id);
}

/// `--from-name` is HOSTED only: a linked inbox is the human's own
/// account and keeps the display name set on it. Refused (usage) even
/// in on mode, before any credential/SMTP work; nothing is audited.
#[test]
fn linked_send_refuses_from_name() {
    let (name, path) = unique("lfname");
    let project_id = insert_project(&name, &path);
    let linked = format!("me-{}@linked.example", &project_id[..8]);
    seed_linked(&project_id, &linked, "send");
    set_gating(&path, "on");
    let resp = handle_send(&send_body(
        &path,
        serde_json::json!({ "from": linked, "fromName": "Laura" }),
    ));
    assert_eq!(resp.status, "400 Bad Request", "{}", resp.body);
    let v = body_json(&resp);
    assert_eq!(v["error"]["code"], "usage");
    assert!(
        v["error"]["hint"].as_str().unwrap().contains("hosted K2 mailboxes only"),
        "{v}"
    );
    assert_eq!(outbound_count(&project_id), 0, "a refused linked send never audits");
    cleanup_linked(&project_id);
}

/// `--from-name` on a LINKED reply is refused like on a linked send:
/// the human's own account keeps its own display name. Refused before
/// the gate, so nothing is audited or sent.
#[test]
fn linked_reply_refuses_from_name() {
    let (name, path) = unique("lrfname");
    let project_id = insert_project(&name, &path);
    let linked = format!("me-{}@linked.example", &project_id[..8]);
    seed_linked(&project_id, &linked, "send");
    set_gating(&path, "on");
    let token = messages::encode_message_id(&linked, "uid:1:42");
    let resp = handle_reply(&reply_body(&path, &token, serde_json::json!({ "fromName": "Laura" })));
    assert_eq!(resp.status, "400 Bad Request", "{}", resp.body);
    let v = body_json(&resp);
    assert_eq!(v["error"]["code"], "usage");
    assert!(
        v["error"]["hint"].as_str().unwrap().contains("hosted K2 mailboxes only"),
        "{v}"
    );
    assert_eq!(outbound_count(&project_id), 0, "a refused linked reply never audits");
    cleanup_linked(&project_id);
}

/// Appa/Baden (macOS, 0.45.4): `k2 mail reply` from a LINKED inbox
/// follows the agent-send mode the owner sets through
/// `/cli/mail/config/set` (`k2 mail sending`) — on EVERY OS. Before
/// the fix the hosted-only D3 gate refused the write off-Linux, so a
/// Mac owner could not turn sending on at all.
#[test]
fn linked_reply_follows_agent_send_set_by_the_owner_on_any_os() {
    // config/set reads the singleton mail_server row: hold the lock.
    let _mail = crate::mail::mail_server_test_lock();
    let (name, path) = unique("lreply");
    let project_id = insert_project(&name, &path);
    let linked = format!("me-{}@linked.example", &project_id[..8]);
    seed_linked(&project_id, &linked, "send");
    let token = messages::encode_message_id(&linked, "uid:1:42");
    let reply = || {
        handle_reply(
            &serde_json::to_vec(&serde_json::json!({
                "project": path, "id": token, "body": "thanks!",
            }))
            .unwrap(),
        )
    };
    // No scoped principal = the owner.
    let set = |mode: &str| {
        crate::mail::routes_server::handle_config_set(
            &serde_json::to_vec(&serde_json::json!({
                "workspace": path, "agentSend": mode,
            }))
            .unwrap(),
        )
    };

    // NULL column → the default (off): gated, and the hint names
    // controls that exist on this OS.
    let resp = reply();
    assert_eq!(resp.status, "403 Forbidden", "{}", resp.body);
    let v = body_json(&resp);
    assert_eq!(v["error"]["code"], "gated");
    assert!(
        v["error"]["hint"].as_str().unwrap().contains("`k2 mail sending <workspace> on`"),
        "{v}"
    );

    // Owner turns it on → the reply passes the gate and reaches the
    // SMTP stage (no vaulted credential → 503 not_ready; nothing sent).
    let r = set("on");
    assert_eq!(r.status, "200 OK", "agentSend must apply on every OS: {}", r.body);
    let resp = reply();
    assert_eq!(resp.status, "503 Service Unavailable", "on must pass the gate: {}", resp.body);
    assert_eq!(body_json(&resp)["error"]["code"], "not_ready");

    // approval → since MCS2 a linked reply queues too: it passes the
    // gate and reaches the same provider step (no credential → 503).
    let r = set("approval");
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let resp = reply();
    assert_eq!(resp.status, "503 Service Unavailable", "approval passes the gate: {}", resp.body);
    assert_eq!(body_json(&resp)["error"]["code"], "not_ready");

    // Back off → gated again.
    let r = set("off");
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let resp = reply();
    assert_eq!(resp.status, "403 Forbidden", "{}", resp.body);
    assert_eq!(body_json(&resp)["error"]["code"], "gated");
    assert_eq!(outbound_count(&project_id), 0, "nothing was sent or queued");
    cleanup_linked(&project_id);
}

/// E4: linked send under approval mode refuses with a clear
/// teaching (approval queue is hosted-only) — no SMTP, no queue row.
#[test]
fn linked_send_approval_mode_queues_without_sending() {
    let (name, path) = unique("lapp");
    let project_id = insert_project(&name, &path);
    let linked = format!("me-{}@linked.example", &project_id[..8]);
    let row = seed_linked(&project_id, &linked, "send");
    // An app-password row: its credential in the (test) vault, so the
    // SMTP backend resolves — and approval mode never dials it.
    FileSecretStore::default()
        .store_exact(&external::vault_key(&row), "app-pw")
        .expect("vault the test credential");
    set_gating(&path, "approval");
    let resp = handle_send(&send_body(&path, serde_json::json!({ "from": linked })));
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    let v = body_json(&resp);
    assert_eq!(v["status"], "pending_approval", "{v}");
    assert_eq!(v["queued"], true);
    let id = v["id"].as_str().unwrap().to_string();
    assert_eq!(outbound_count(&project_id), 1, "MCS2: approval queues a linked send");
    // The owner's queue lists it with its preview; nothing left the box.
    let list = body_json(&handle_approvals_list(&HashMap::new()));
    assert!(
        list["pending"].as_array().unwrap().iter().any(|p| p["id"] == id.as_str()),
        "{list}"
    );
    // Deny it (no SMTP dial in this test) and tidy up.
    let resp = handle_approvals_deny(
        serde_json::json!({ "id": id, "note": "test" }).to_string().as_bytes(),
    );
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    let _ = FileSecretStore::default().delete(&external::vault_key(&row));
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = conn.execute(
            "DELETE FROM mail_outbound WHERE owner_project_id = ?1",
            rusqlite::params![project_id],
        );
    }
    cleanup_linked(&project_id);
}

/// E4: linked send with mode `on` reaches the SMTP stage (no
/// vaulted credential → 503 not_ready — proves the gate passed).
#[test]
fn linked_send_on_mode_reaches_the_smtp_stage() {
    let (name, path) = unique("lon");
    let project_id = insert_project(&name, &path);
    let linked = format!("me-{}@linked.example", &project_id[..8]);
    seed_linked(&project_id, &linked, "send");
    set_gating(&path, "on");
    let resp = handle_send(&send_body(&path, serde_json::json!({ "from": linked })));
    assert_ne!(resp.status, "403 Forbidden", "on must allow linked: {}", resp.body);
    assert_eq!(resp.status, "503 Service Unavailable", "{}", resp.body);
    assert_eq!(body_json(&resp)["error"]["code"], "not_ready");
    cleanup_linked(&project_id);
}

/// A linked inbox the workspace can only READ/DRAFT (default 'draft')
/// can't send — the teaching 403 points at raising the level AND at
/// 'k2 mail draft' (the linked-specific alternative).
#[test]
fn linked_send_without_the_send_level_teaches_draft_alternative() {
    let (name, path) = unique("ldraft");
    let project_id = insert_project(&name, &path);
    let linked = format!("me-{}@linked.example", &project_id[..8]);
    seed_linked(&project_id, &linked, "draft"); // default: can't send
    set_gating(&path, "on");
    let resp = handle_send(&send_body(&path, serde_json::json!({ "from": linked })));
    assert_eq!(resp.status, "403 Forbidden", "{}", resp.body);
    let v = body_json(&resp);
    assert_eq!(v["error"]["code"], "gated");
    let hint = v["error"]["hint"].as_str().unwrap();
    assert!(hint.contains("read/draft-only"), "{hint}");
    assert!(hint.contains("k2 mail draft"), "linked teaches the draft alternative: {hint}");
    cleanup_linked(&project_id);
}

/// Seed a linked inbox whose credential is an OAuth token (Gmail
/// XOAUTH2) — `auth_kind='oauth'`, `provider='gmail'` — matching the
/// shape `external::add_oauth_inbox` writes. No app-password is ever
/// vaulted for such a row (the credential is the token blob).
fn seed_linked_oauth(project_id: &str, address: &str, primary_level: &str) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO mail_external_inboxes (id, owner_project_id, email_address, host, \
             port, username, created_at, primary_level, auth_kind, provider) \
             VALUES (?1, ?2, ?3, 'imap.gmail.com', 993, ?3, 100, ?4, 'oauth', 'gmail')",
        rusqlite::params![id, project_id, address, primary_level],
    )
    .expect("seed oauth linked inbox");
    id
}

/// REGRESSION (the seam the original tests missed): an OAuth-linked
/// inbox has its token vaulted under the `-oauth` key and NO
/// app-password. `linked_password` MUST NOT 503 on the absent
/// app-password — it recognises `auth_kind='oauth'` and returns an
/// empty password (the SMTP/IMAP backend mints the token itself).
#[test]
fn linked_password_skips_app_password_resolve_for_oauth_rows() {
    let (name, path) = unique("oauthpw");
    let project_id = insert_project(&name, &path);
    let linked = format!("me-{}@gmail.com", &project_id[..8]);
    let id = seed_linked_oauth(&project_id, &linked, "send");

    // Present: the OAuth token bundle (the real credential). Absent:
    // any app-password under `vault_key(id)`.
    let secrets = FileSecretStore::default();
    let tokens = crate::mail::oauth::Tokens {
        access_token: "ya29.test-access".to_string(),
        refresh_token: Some("1//test-refresh".to_string()),
        scope: Some("https://mail.google.com/".to_string()),
        token_type: "Bearer".to_string(),
        expires_in: 3600,
        id_token: None,
    };
    crate::mail::oauth::store_tokens(&secrets, &id, &tokens, 1_000)
        .expect("vault oauth tokens");

    let inbox = external::inbox_for_address(&linked).expect("row loads");
    // No app-password vaulted → a `password` row would 503 here; the
    // oauth row must instead PROCEED with an empty password.
    match linked_password(&inbox) {
        Ok(pw) => assert!(pw.is_empty(), "oauth row yields empty password, got {} bytes", pw.len()),
        Err(resp) => panic!(
            "oauth row wrongly 503'd on a missing app-password: {} {}",
            resp.status, resp.body
        ),
    }

    // Clean up the vaulted token (shared secrets file).
    let _ = secrets.delete(&crate::mail::oauth::oauth_vault_key(&id));
    cleanup_linked(&project_id);
}

/// Counterpart: a NON-oauth (app-password) row with NO vaulted
/// credential still 503s not_ready — the fix narrows the skip to
/// oauth rows only, it does not silence the real missing-credential
/// case.
#[test]
fn linked_password_still_503s_for_password_rows_missing_the_vault_entry() {
    let (name, path) = unique("pwmiss");
    let project_id = insert_project(&name, &path);
    let linked = format!("me-{}@linked.example", &project_id[..8]);
    seed_linked(&project_id, &linked, "send"); // default auth_kind (not oauth)

    let inbox = external::inbox_for_address(&linked).expect("row loads");
    match linked_password(&inbox) {
        Ok(pw) => panic!("password row with no vault entry must 503, got Ok({pw:?})"),
        Err(resp) => {
            assert_eq!(resp.status, "503 Service Unavailable", "{}", resp.body);
            assert_eq!(body_json(&resp)["error"]["code"], "not_ready");
            let hint = body_json(&resp)["error"]["hint"].as_str().unwrap().to_string();
            assert!(hint.contains("k2 mail link add"), "corrected verb in hint: {hint}");
        }
    }
    cleanup_linked(&project_id);
}

// ── BCC: agent --bcc + the owner's always-BCC policy ──

/// Whether a JMAP Email object carries a Bcc anywhere a recipient could
/// see it: the `bcc` property, a `headers` entry named Bcc, or a raw
/// `header:Bcc…` property, at any depth (any case).
fn jmap_has_bcc(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Object(o) => o.iter().any(|(k, val)| {
            let k = k.to_ascii_lowercase();
            k == "bcc"
                || k.starts_with("header:bcc")
                || (k == "name" && val.as_str().is_some_and(|n| n.eq_ignore_ascii_case("bcc")))
                || jmap_has_bcc(val)
        }),
        serde_json::Value::Array(a) => a.iter().any(jmap_has_bcc),
        _ => false,
    }
}

/// Records exactly what the production `StalwartClient` submit hands
/// the JMAP layer: the envelope rcptTo (`all_recipients`) and the
/// Email object (`submission_email_json` — the headers recipients see).
#[derive(Default)]
struct RecordingSubmit {
    seen: std::sync::Mutex<Vec<(Vec<String>, serde_json::Value)>>,
}

impl SubmitBackend for RecordingSubmit {
    fn submit(&self, _account_id: &str, msg: &OutboundMessage) -> Result<(), String> {
        self.seen
            .lock()
            .unwrap()
            .push((msg.all_recipients(), send::submission_email_json(msg)));
        Ok(())
    }
}

fn seed_approval_sender(label: &str) -> (String, String, String) {
    let (name, path) = unique(label);
    let project_id = insert_project(&name, &path);
    let domain = format!("{name}.example");
    seed_domain(&domain, "direct");
    seed_address(&project_id, &format!("bot@{domain}"), Some("acc-1"));
    set_gating(&path, "approval");
    (path, project_id, domain)
}

/// `--bcc` end to end through the REAL handlers: queued (approval),
/// audited in the outbox + the owner's approvals list, kept in the
/// stored message, and on approve it lands in the ENVELOPE while the
/// sent headers carry no trace of it.
#[test]
fn send_bcc_is_audited_survives_approval_and_rides_envelope_only() {
    let (path, project_id, domain) = seed_approval_sender("blind");
    let resp = handle_send(&send_body(
        &path,
        serde_json::json!({
            "cc": "cc@cust.example",
            "bcc": ["boss@shop.example", "qa@shop.example"],
        }),
    ));
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    let id = body_json(&resp)["id"].as_str().expect("id").to_string();

    // Outbox (agent view): the BCCs are recorded, none from policy.
    let resp = handle_outbox(&params(&[("project", &path), ("id", &id)]));
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    let v = body_json(&resp);
    assert_eq!(
        v["outbound"]["bcc"],
        serde_json::json!(["boss@shop.example", "qa@shop.example"]),
        "{v}"
    );
    assert_eq!(v["outbound"]["policyBcc"], serde_json::json!([]));
    assert_eq!(v["outbound"]["cc"], serde_json::json!(["cc@cust.example"]));

    // The owner deciding sees them too.
    let resp = handle_approvals_list(&HashMap::new());
    let v = body_json(&resp);
    let ours = v["pending"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == id.as_str())
        .expect("queued");
    assert_eq!(ours["bcc"], serde_json::json!(["boss@shop.example", "qa@shop.example"]));

    // The stored message (what approve submits) still carries them.
    let store = DbOutboundStore::default();
    let stored = store.load_message(&id).expect("stored body");
    assert_eq!(stored.bcc, vec!["boss@shop.example", "qa@shop.example"]);

    // Approve → envelope has the BCCs, the Email headers don't.
    let rec = RecordingSubmit::default();
    let mut acct = |_from: &str| Ok("acc-1".to_string());
    let out =
        send::approve_and_submit(&store, Ok(&rec), &mut acct, &id, "owner", None, now_secs())
            .expect("approve");
    assert_eq!(out, send::ApproveOutcome::Submitted);
    let seen = rec.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    let (rcpt, email) = &seen[0];
    assert!(rcpt.contains(&"boss@shop.example".to_string()), "{rcpt:?}");
    assert!(rcpt.contains(&"qa@shop.example".to_string()), "{rcpt:?}");
    assert!(rcpt.contains(&"human@example.com".to_string()), "{rcpt:?}");
    assert!(rcpt.contains(&"cc@cust.example".to_string()), "{rcpt:?}");
    let wire = email.to_string();
    assert!(!wire.contains("boss@shop.example"), "bcc in headers: {wire}");
    assert!(!wire.contains("qa@shop.example"), "bcc in headers: {wire}");
    // Structural, not a substring: a random Message-ID such as
    // `…949fbccd06@…` contains "bcc" about 1 run in 140 (seen on k2n5,
    // 0.46.5 integration), while a real leak is a key or a header.
    assert!(!jmap_has_bcc(email), "Bcc in the Email object: {wire}");
    assert!(jmap_has_bcc(&serde_json::json!({"headers": [{"name": "BCC", "value": "x"}]})), "the check sees a Bcc header");
    assert!(jmap_has_bcc(&serde_json::json!({"bcc": [{"email": "x@y.example"}]})), "the check sees the bcc property");
    assert!(jmap_has_bcc(&serde_json::json!({"header:Bcc:asAddresses": []})), "the check sees a raw header property");
    assert!(!jmap_has_bcc(&serde_json::json!({"messageId": ["55046ce76ec3430aa062ce949fbccd06@x.example"]})), "a Message-ID with bcc in its hex is fine");
    drop(seen);

    cleanup(&project_id, Some(&domain));
}

/// The always-on cap counts to + cc + bcc — 10 To + 1 Bcc is 11 and
/// refused before any row exists.
#[test]
fn send_recipient_limit_counts_to_cc_and_bcc() {
    let (path, project_id, domain) = seed_approval_sender("blindlim");
    let ten: Vec<String> = (0..10).map(|i| format!("c{i}@cust.example")).collect();
    let six: Vec<String> = (0..6).map(|i| format!("c{i}@cust.example")).collect();
    for extra in [
        serde_json::json!({ "to": ten, "bcc": "boss@shop.example" }),
        serde_json::json!({
            "to": six,
            "cc": ["d1@cust.example", "d2@cust.example"],
            "bcc": ["b1@shop.example", "b2@shop.example", "b3@shop.example"],
        }),
    ] {
        let resp = handle_send(&send_body(&path, extra.clone()));
        assert_eq!(resp.status, "400 Bad Request", "{extra} → {}", resp.body);
        let hint = body_json(&resp)["error"]["hint"].as_str().unwrap().to_string();
        assert!(hint.contains("too many recipients (11)"), "{hint}");
        assert!(hint.contains("bcc"), "{hint}");
    }
    assert_eq!(outbound_count(&project_id), 0, "a refused send writes nothing");
    cleanup(&project_id, Some(&domain));
}

/// The owner's always-BCC policy is stamped onto every agent send at
/// send time and recorded as POLICY in the outbox. Nothing the agent
/// puts in the body removes it (no field reaches the policy), and an
/// agent `--bcc` of the same address is recorded once, as policy.
#[test]
fn policy_bcc_is_added_to_agent_sends_and_the_agent_cannot_drop_it() {
    let (path, project_id, domain) = seed_approval_sender("pol");
    k2_core::workspace::settings::set_mail_always_bcc(
        &path,
        &["owner@shop.example".to_string()],
    )
    .expect("owner sets the policy");

    // Plain send: the policy copy is added.
    let resp = handle_send(&send_body(&path, serde_json::json!({})));
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    let id = body_json(&resp)["id"].as_str().unwrap().to_string();
    let v = body_json(&handle_outbox(&params(&[("project", &path), ("id", &id)])));
    assert_eq!(v["outbound"]["policyBcc"], serde_json::json!(["owner@shop.example"]), "{v}");
    assert_eq!(v["outbound"]["bcc"], serde_json::json!([]));

    // Every attempt to opt out from the request body is ignored.
    let resp = handle_send(&send_body(
        &path,
        serde_json::json!({
            "bcc": ["owner@shop.example", "qa@shop.example"],
            "policyBcc": [],
            "policy_bcc": [],
            "alwaysBcc": "",
            "skipPolicyBcc": true,
        }),
    ));
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    let id = body_json(&resp)["id"].as_str().unwrap().to_string();
    let v = body_json(&handle_outbox(&params(&[("project", &path), ("id", &id)])));
    assert_eq!(v["outbound"]["policyBcc"], serde_json::json!(["owner@shop.example"]), "{v}");
    assert_eq!(v["outbound"]["bcc"], serde_json::json!(["qa@shop.example"]), "{v}");
    let stored = DbOutboundStore::default().load_message(&id).expect("stored");
    assert!(
        stored.all_recipients().contains(&"owner@shop.example".to_string()),
        "policy copy is in the envelope the approve path submits: {stored:?}"
    );

    // The policy is still in force (the body never touched it).
    assert_eq!(
        k2_core::workspace::settings::mail_always_bcc_for_path(&path).unwrap(),
        vec!["owner@shop.example".to_string()]
    );
    cleanup(&project_id, Some(&domain));
}

/// A policy that would push a send over the cap refuses with a clear
/// error naming the policy — it never silently drops a recipient, and
/// nothing is queued.
#[test]
fn policy_bcc_over_the_limit_refuses_with_a_clear_error() {
    let (path, project_id, domain) = seed_approval_sender("pollim");
    k2_core::workspace::settings::set_mail_always_bcc(
        &path,
        &["owner@shop.example".to_string(), "audit@shop.example".to_string()],
    )
    .expect("owner sets the policy");
    let nine: Vec<String> = (0..9).map(|i| format!("c{i}@cust.example")).collect();
    let resp = handle_send(&send_body(&path, serde_json::json!({ "to": nine })));
    assert_eq!(resp.status, "400 Bad Request", "{}", resp.body);
    let v = body_json(&resp);
    assert_eq!(v["error"]["code"], "usage");
    let hint = v["error"]["hint"].as_str().unwrap();
    assert!(hint.contains("too many recipients (11)"), "{hint}");
    assert!(hint.contains("always BCCs 2"), "{hint}");
    assert!(hint.contains("can't be dropped"), "{hint}");
    assert_eq!(outbound_count(&project_id), 0, "nothing queued");

    // 8 To + 2 policy = 10 is fine.
    let eight: Vec<String> = (0..8).map(|i| format!("c{i}@cust.example")).collect();
    let resp = handle_send(&send_body(&path, serde_json::json!({ "to": eight })));
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    cleanup(&project_id, Some(&domain));
}

/// A corrupt stored policy refuses the send (fail-closed) instead of
/// quietly sending without the owner's copy.
#[test]
fn corrupt_policy_refuses_the_send() {
    let (path, project_id, domain) = seed_approval_sender("polbad");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "UPDATE projects SET mail_always_bcc = '{oops' WHERE path = ?1",
            rusqlite::params![path],
        )
        .expect("corrupt policy");
    }
    let resp = handle_send(&send_body(&path, serde_json::json!({})));
    assert_eq!(resp.status, "502 Bad Gateway", "{}", resp.body);
    assert!(resp.body.contains("always-BCC policy"), "{}", resp.body);
    assert_eq!(outbound_count(&project_id), 0);
    cleanup(&project_id, Some(&domain));
}

/// A recording [`crate::mail::submit::MailSubmit`] double (never dials).
struct RecordingMailSubmit {
    sent: std::sync::Mutex<Vec<(Option<String>, OutboundMessage, Vec<String>)>>,
    fail: bool,
}
impl crate::mail::submit::MailSubmit for RecordingMailSubmit {
    fn backend(&self) -> &'static str {
        "graph"
    }
    fn submit(
        &self,
        msg: &OutboundMessage,
        atts: &[external_smtp::OutAttachment],
    ) -> Result<crate::mail::submit::Receipt, crate::mail::submit::SubmitError> {
        if self.fail {
            return Err(crate::mail::submit::SubmitError::Engine("provider said no".into()));
        }
        let names: Vec<String> = atts.iter().map(|a| a.filename.clone()).collect();
        self.sent.lock().unwrap().push((None, msg.clone(), names.clone()));
        Ok(crate::mail::submit::Receipt {
            to: msg.to.clone(),
            cc: msg.cc.clone(),
            subject: msg.subject.clone(),
            attachments: names,
        })
    }
    fn reply(
        &self,
        src: &str,
        msg: &OutboundMessage,
        atts: &[external_smtp::OutAttachment],
    ) -> Result<crate::mail::submit::Receipt, crate::mail::submit::SubmitError> {
        let names: Vec<String> = atts.iter().map(|a| a.filename.clone()).collect();
        self.sent.lock().unwrap().push((Some(src.to_string()), msg.clone(), names.clone()));
        Ok(crate::mail::submit::Receipt {
            to: msg.to.clone(),
            cc: Vec::new(),
            subject: msg.subject.clone(),
            attachments: names,
        })
    }
    fn reply_preview(
        &self,
        _src: &str,
    ) -> Result<Option<(String, String)>, crate::mail::submit::SubmitError> {
        Ok(Some(("pat@dest.example".into(), "Re: Hi".into())))
    }
}

fn linked_msg(from: &str) -> OutboundMessage {
    OutboundMessage {
        from_name: Some("Agent".into()),
        from: from.to_string(),
        to: vec!["pat@dest.example".to_string()],
        cc: Vec::new(),
        bcc: vec!["boss@shop.example".to_string()],
        policy_bcc: vec!["owner@shop.example".to_string()],
        reply_to: Vec::new(),
        subject: "Hi".to_string(),
        text_body: "body".to_string(),
        in_reply_to: None,
        references: None,
        reply_source: None,
        attachments: Vec::new(),
        ephemeral: false,
        notify: None,
        forward_of: None,
        source_dmarc: None,
        source_check: None,
    }
}

/// MCS2: a linked send with gate `on` writes its audit row FIRST, then
/// the provider runs, then the row is `sent` — agent vs policy BCCs
/// stay apart in the trail.
#[test]
fn linked_on_mode_audits_first_and_records_agent_and_policy_bcc() {
    let (name, path) = unique("linkbcc");
    let project_id = insert_project(&name, &path);
    let store = DbOutboundStore::default();
    let submit = RecordingMailSubmit { sent: Default::default(), fail: false };
    let msg = linked_msg("me@gmail.example");
    let req = OutboundRequest {
        project_id: &project_id,
        agent_name: "Agent",
        account_id: "row-x",
        message: &msg,
        send_after: None,
    };
    let atts = vec![external_smtp::OutAttachment {
        scanned: false, filename: "a.pdf".into(),
        content_type: "application/pdf".into(),
        bytes: vec![1, 2, 3],
    }];
    let (outcome, receipt) =
        send::gate_and_dispatch_linked(&store, &submit, Gate::On, &req, &atts, now_secs())
            .expect("dispatch");
    assert!(matches!(outcome, SendOutcome::Submitted { .. }), "{outcome:?}");
    assert_eq!(receipt.expect("receipt").attachments, vec!["a.pdf".to_string()]);
    assert_eq!(submit.sent.lock().unwrap().len(), 1);
    let v = body_json(&handle_outbox(&params(&[("project", &path)])));
    assert_eq!(v["count"], 1, "{v}");
    assert_eq!(v["outbox"][0]["status"], "submitted");
    assert_eq!(v["outbox"][0]["bcc"], serde_json::json!(["boss@shop.example"]));
    assert_eq!(v["outbox"][0]["policyBcc"], serde_json::json!(["owner@shop.example"]));
    assert_eq!(v["outbox"][0]["attachments"]["names"], serde_json::json!(["a.pdf"]));
    cleanup(&project_id, None);
}

/// MCS2: a provider failure leaves an audited `failed` row with the
/// reason; nothing is reported as sent.
#[test]
fn linked_on_mode_provider_failure_is_a_failed_row() {
    let (name, path) = unique("linkfail");
    let project_id = insert_project(&name, &path);
    let store = DbOutboundStore::default();
    let submit = RecordingMailSubmit { sent: Default::default(), fail: true };
    let msg = linked_msg("me@gmail.example");
    let req = OutboundRequest {
        project_id: &project_id,
        agent_name: "Agent",
        account_id: "row-x",
        message: &msg,
        send_after: None,
    };
    let (outcome, _) =
        send::gate_and_dispatch_linked(&store, &submit, Gate::On, &req, &[], now_secs())
            .expect("dispatch");
    assert!(matches!(outcome, SendOutcome::SubmitFailed { .. }), "{outcome:?}");
    let v = body_json(&handle_outbox(&params(&[("project", &path)])));
    assert_eq!(v["outbox"][0]["status"], "failed");
    assert!(v["outbox"][0]["note"].as_str().unwrap().contains("provider said no"));
    cleanup(&project_id, None);
}

/// MCS2 (MCS-A5): approval mode queues a LINKED message (attachments
/// captured into the body file); approve hands it to the resolved
/// provider — the reply keeps its source id; nothing left before.
#[test]
fn linked_approval_queues_then_approve_hands_off_to_the_provider() {
    let (name, path) = unique("linkappr");
    let project_id = insert_project(&name, &path);
    let store = DbOutboundStore::default();
    let queued = RecordingMailSubmit { sent: Default::default(), fail: false };
    let mut msg = linked_msg("me@contoso.example");
    msg.reply_source = Some("graph:AAA".to_string());
    let req = OutboundRequest {
        project_id: &project_id,
        agent_name: "Agent",
        account_id: "row-x",
        message: &msg,
        send_after: None,
    };
    let atts = vec![external_smtp::OutAttachment {
        scanned: false, filename: "q.txt".into(),
        content_type: "text/plain".into(),
        bytes: b"quarterly".to_vec(),
    }];
    let (outcome, receipt) =
        send::gate_and_dispatch_linked(&store, &queued, Gate::Approval, &req, &atts, now_secs())
            .expect("queue");
    let SendOutcome::Queued { id, .. } = outcome else { panic!("{outcome:?}") };
    assert!(receipt.is_none());
    assert!(queued.sent.lock().unwrap().is_empty(), "nothing leaves before approval");
    // The stored message carries the source + the attachment bytes.
    let stored = store.load_message(&id).expect("stored");
    assert_eq!(stored.reply_source.as_deref(), Some("graph:AAA"));
    assert_eq!(stored.attachments.len(), 1);
    assert!(!format!("{stored:?}").contains("quarterly"), "Debug never shows bytes");

    let at_approve = RecordingMailSubmit { sent: Default::default(), fail: false };
    let mut resolve = |m: &OutboundMessage| -> Result<Box<dyn crate::mail::submit::MailSubmit + '_>, String> {
        assert_eq!(m.from, "me@contoso.example");
        Ok(Box::new(RecordingMailSubmitRef(&at_approve)))
    };
    let out = send::approve_and_submit_with(&store, &mut resolve, &id, "owner", None, now_secs())
        .expect("approve");
    assert_eq!(out, send::ApproveOutcome::Submitted);
    let sent = at_approve.sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0.as_deref(), Some("graph:AAA"), "a reply goes out as a reply");
    assert_eq!(sent[0].2, vec!["q.txt".to_string()], "the captured attachment rode along");
    let v = body_json(&handle_outbox(&params(&[("project", &path), ("id", &id)])));
    assert_eq!(v["outbound"]["status"], "submitted");
    cleanup(&project_id, None);
}

/// Borrowing wrapper so a test can keep the recorder after approve.
struct RecordingMailSubmitRef<'a>(&'a RecordingMailSubmit);
impl crate::mail::submit::MailSubmit for RecordingMailSubmitRef<'_> {
    fn backend(&self) -> &'static str {
        self.0.backend()
    }
    fn submit(
        &self,
        msg: &OutboundMessage,
        atts: &[external_smtp::OutAttachment],
    ) -> Result<crate::mail::submit::Receipt, crate::mail::submit::SubmitError> {
        self.0.submit(msg, atts)
    }
    fn reply(
        &self,
        src: &str,
        msg: &OutboundMessage,
        atts: &[external_smtp::OutAttachment],
    ) -> Result<crate::mail::submit::Receipt, crate::mail::submit::SubmitError> {
        self.0.reply(src, msg, atts)
    }
    fn reply_preview(
        &self,
        src: &str,
    ) -> Result<Option<(String, String)>, crate::mail::submit::SubmitError> {
        self.0.reply_preview(src)
    }
}

// ── hosted attachments through the real handlers ──

fn seed_files_sender(label: &str, mode: &str) -> (String, String, String) {
    let (path, project_id, domain) = seed_approval_sender(label);
    std::fs::create_dir_all(&path).expect("workspace folder");
    set_gating(&path, mode);
    (path, project_id, domain)
}

fn done(path: &str, project_id: &str, domain: &str) {
    cleanup(project_id, Some(domain));
    let _ = std::fs::remove_dir_all(path);
}

#[derive(Default)]
struct KeepSubmit {
    seen: std::sync::Mutex<Vec<OutboundMessage>>,
}

impl SubmitBackend for KeepSubmit {
    fn submit(&self, _account_id: &str, msg: &OutboundMessage) -> Result<(), String> {
        self.seen.lock().unwrap().push(msg.clone());
        Ok(())
    }
}

fn decoded(f: &send::StoredAttachment) -> Vec<u8> {
    f.to_out().expect("stored file decodes").bytes
}

/// Off: refused before a file is read, nothing recorded.
#[test]
fn attach_gated_off_exits_3_and_reads_nothing() {
    let (path, project_id, domain) = seed_files_sender("attoff", "off");
    // The path doesn't exist: reading it first would answer 404.
    let resp = handle_send(&send_body(&path, serde_json::json!({ "attachments": ["ghost.bin"] })));
    assert_eq!(resp.status, "403 Forbidden", "{}", resp.body);
    assert_eq!(body_json(&resp)["error"]["code"], "gated");
    assert_eq!(outbound_count(&project_id), 0);
    done(&path, &project_id, &domain);
}

/// Approval: the file is captured when queued. The approver sees its name
/// and size and can open the bytes; changing or deleting the workspace
/// file changes nothing; what leaves is what was captured; afterwards the
/// body file holds no bytes and the outbox keeps the name.
#[test]
fn attach_under_approval_sends_the_captured_bytes_whatever_happens_to_the_file() {
    let (path, project_id, domain) = seed_files_sender("attappr", "approval");
    let file = std::path::Path::new(&path).join("report.pdf");
    std::fs::write(&file, b"the bytes the approver sees").unwrap();

    let resp = handle_send(&send_body(&path, serde_json::json!({ "attachments": ["report.pdf"] })));
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    let id = body_json(&resp)["id"].as_str().expect("id").to_string();

    // The swap the approver must not be fooled by.
    std::fs::write(&file, b"swapped after queueing, must not leave").unwrap();

    let list = body_json(&handle_approvals_list(&HashMap::new()));
    let ours = list["pending"].as_array().unwrap().iter().find(|p| p["id"] == id.as_str()).expect("queued").clone();
    assert_eq!(ours["files"][0]["name"], "report.pdf", "{ours}");
    assert_eq!(ours["files"][0]["size"], 27, "{ours}");
    assert_eq!(ours["attachments"]["names"], serde_json::json!(["report.pdf"]), "{ours}");

    let shown = body_json(&handle_approvals_show(&params(&[("id", &id)])));
    assert_eq!(shown["message"]["text"], "All green.", "{shown}");
    assert_eq!(shown["message"]["files"][0]["size"], 27, "{shown}");
    let one = handle_approvals_show(&params(&[("id", &id), ("file", "1")]));
    assert_eq!(one.status, "200 OK", "{}", one.body);
    use base64::Engine as _;
    let got = base64::engine::general_purpose::STANDARD
        .decode(body_json(&one)["dataBase64"].as_str().expect("data"))
        .expect("base64");
    assert_eq!(got, b"the bytes the approver sees");
    let none = handle_approvals_show(&params(&[("id", &id), ("file", "2")]));
    assert_eq!(none.status, "404 Not Found", "{}", none.body);

    let store = DbOutboundStore::default();
    let rec = KeepSubmit::default();
    let mut acct = |_from: &str| Ok("acc-1".to_string());
    let out = send::approve_and_submit(&store, Ok(&rec), &mut acct, &id, "owner", None, now_secs()).expect("approve");
    assert_eq!(out, send::ApproveOutcome::Submitted);
    let seen = rec.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(decoded(&seen[0].attachments[0]), b"the bytes the approver sees", "not the swapped file");
    drop(seen);

    assert!(store.load_message(&id).unwrap().attachments.is_empty(), "no bytes once decided");
    let outbox = body_json(&handle_outbox(&params(&[("project", &path), ("id", &id)])));
    assert_eq!(outbox["outbound"]["attachments"]["names"], serde_json::json!(["report.pdf"]), "{outbox}");
    // Decided: it can't be opened here any more.
    let late = handle_approvals_show(&params(&[("id", &id)]));
    assert_eq!(late.status, "409 Conflict", "{}", late.body);
    done(&path, &project_id, &domain);
}

/// Denied: the note goes back, the bytes go away.
#[test]
fn attach_denied_keeps_no_bytes() {
    let (path, project_id, domain) = seed_files_sender("attdeny", "approval");
    std::fs::write(std::path::Path::new(&path).join("a.bin"), [1u8, 2, 3]).unwrap();
    let resp = handle_send(&send_body(&path, serde_json::json!({ "attachments": ["a.bin"] })));
    let id = body_json(&resp)["id"].as_str().expect("id").to_string();
    let store = DbOutboundStore::default();
    assert_eq!(store.load_message(&id).unwrap().attachments.len(), 1);
    send::deny(&store, &id, "owner", "no", now_secs()).expect("deny");
    assert!(store.load_message(&id).unwrap().attachments.is_empty());
    done(&path, &project_id, &domain);
}

/// On, through the handler: a good file gets as far as the engine (no
/// server here: 503, no row), a refused one never does. The On round trip
/// itself is `send::tests::on_mode_uploads_the_files_and_keeps_only_their_names`.
#[test]
fn attach_on_mode_reads_the_file_then_needs_the_engine() {
    let _mail = crate::mail::mail_server_test_lock();
    clear_mail_server();
    let (path, project_id, domain) = seed_files_sender("atton", "on");
    std::fs::write(std::path::Path::new(&path).join("a.bin"), [1u8]).unwrap();
    let resp = handle_send(&send_body(&path, serde_json::json!({ "attachments": ["a.bin"] })));
    assert_eq!(resp.status, "503 Service Unavailable", "{}", resp.body);
    let resp = handle_send(&send_body(&path, serde_json::json!({ "attachments": ["../a.bin"] })));
    assert_eq!(resp.status, "400 Bad Request", "{}", resp.body);
    assert_eq!(outbound_count(&project_id), 0);
    done(&path, &project_id, &domain);
}

/// Point the singleton `mail_server` row at the fake engine. The caller
/// holds the env guard, then the mail lock, then a `MailServerRowCleanup`.
fn seed_fake_engine(fake: &crate::mail::fake_jmap::FakeJmap) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let _ = conn.execute("DELETE FROM mail_server WHERE id = 1", []);
    conn.execute(
        "INSERT INTO mail_server (id, status, pinned_version, hostname, api_url, api_key_ref, updated_at) \
             VALUES (1, 'running', '0.16.25', 'mail.box.test', ?1, 'env:K2_TEST_MAIL_API_KEY', 1)",
        rusqlite::params![format!("http://127.0.0.1:{}", fake.port)],
    )
    .expect("mail_server");
}

/// A stored message as the reply path reads it.
fn reply_source(id: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "threadId": format!("t-{id}"),
        "from": [{ "name": "Customer", "email": "cust@dest.example" }],
        "subject": "Quote",
        "header:Message-ID:asText": format!("<{id}@dest.example>"),
        "header:References:asText": null,
        "header:Authentication-Results:asText:all": ["mail.box.test; dmarc=pass header.from=dest.example"],
    })
}

/// On, all the way through the handler against the fake engine: the file
/// reaches the engine, no bytes stay, and the outbox keeps the name.
#[test]
fn attach_on_mode_goes_all_the_way_through_the_handler() {
    let _key = k2_core::test_env::EnvVar::set("K2_TEST_MAIL_API_KEY", "k2-test-key");
    let _mail = crate::mail::mail_server_test_lock();
    let _row = crate::mail::MailServerRowCleanup;
    let fake = crate::mail::fake_jmap::FakeJmap::start();
    seed_fake_engine(&fake);
    let (path, project_id, domain) = seed_files_sender("attfull", "on");
    std::fs::write(std::path::Path::new(&path).join("report.pdf"), b"pdf bytes").unwrap();

    let resp = handle_send(&send_body(&path, serde_json::json!({ "attachments": ["report.pdf"] })));
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    let v = body_json(&resp);
    assert_eq!(v["status"], "submitted", "{v}");
    let id = v["id"].as_str().expect("id").to_string();

    let st = fake.state.lock().unwrap();
    assert_eq!(st.uploads, vec![b"pdf bytes".to_vec()]);
    assert_eq!(st.created[0]["attachments"][0]["name"], "report.pdf", "{}", st.created[0]);
    assert_eq!(st.created[0]["from"][0]["email"], format!("bot@{domain}"), "{}", st.created[0]);
    drop(st);
    let store = DbOutboundStore::default();
    assert!(store.load_message(&id).unwrap().attachments.is_empty(), "no bytes at rest");
    let outbox = body_json(&handle_outbox(&params(&[("project", &path), ("id", &id)])));
    assert_eq!(outbox["outbound"]["attachments"]["names"], serde_json::json!(["report.pdf"]), "{outbox}");
    done(&path, &project_id, &domain);
}

/// A hosted reply with a file, approval then on, against the fake engine:
/// the recipient stays locked to the original sender and the file leaves.
#[test]
fn a_hosted_reply_carries_its_file_under_approval_and_on() {
    let _key = k2_core::test_env::EnvVar::set("K2_TEST_MAIL_API_KEY", "k2-test-key");
    let _mail = crate::mail::mail_server_test_lock();
    let _row = crate::mail::MailServerRowCleanup;
    let fake = crate::mail::fake_jmap::FakeJmap::start();
    seed_fake_engine(&fake);
    {
        let mut st = fake.state.lock().unwrap();
        st.emails.insert("M1".to_string(), reply_source("M1"));
        st.emails.insert("M2".to_string(), reply_source("M2"));
    }
    let (path, project_id, domain) = seed_files_sender("attreply2", "approval");
    let file = std::path::Path::new(&path).join("a.bin");
    std::fs::write(&file, b"captured").unwrap();
    let reply = |id: &str| {
        handle_reply(
            &serde_json::to_vec(&serde_json::json!({
                "project": path,
                "id": messages::encode_message_id(&format!("bot@{domain}"), id),
                "body": "thanks",
                "attachments": ["a.bin"],
            }))
            .unwrap(),
        )
    };

    let resp = reply("M1");
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    let id = body_json(&resp)["id"].as_str().expect("id").to_string();
    std::fs::write(&file, b"swapped").unwrap();
    let store = DbOutboundStore::default();
    let stored = store.load_message_full(&id).unwrap();
    assert_eq!(stored.to, vec!["cust@dest.example".to_string()], "recipient locked");
    assert_eq!(decoded(&stored.attachments[0]), b"captured");
    assert!(fake.state.lock().unwrap().uploads.is_empty(), "nothing left before approval");
    let client = fake.client();
    let mut acct = |_: &str| Ok("acc-1".to_string());
    send::approve_and_submit(&store, Ok(&client), &mut acct, &id, "owner", None, now_secs()).expect("approve");
    assert_eq!(fake.state.lock().unwrap().uploads, vec![b"captured".to_vec()], "not the swapped file");

    set_gating(&path, "on");
    let resp = reply("M2");
    assert_eq!(body_json(&resp)["status"], "submitted", "{}", resp.body);
    let st = fake.state.lock().unwrap();
    assert_eq!(st.uploads[1], b"swapped".to_vec(), "on mode reads the file as it is now");
    assert_eq!(st.created[1]["to"], serde_json::json!([{ "email": "cust@dest.example" }]), "{}", st.created[1]);
    assert_eq!(st.created[1]["attachments"][0]["name"], "a.bin", "{}", st.created[1]);
    drop(st);
    done(&path, &project_id, &domain);
}

/// Every path the shared reader refuses, through the hosted handler under
/// approval (the mode where a slip would be queued). Nothing is recorded.
#[cfg(unix)]
#[test]
fn attach_refusals_record_nothing() {
    use std::os::unix::fs::symlink;
    let (path, project_id, domain) = seed_files_sender("attrefuse", "approval");
    let ws = std::path::Path::new(&path);
    let outside = std::env::temp_dir().join(format!("mail-att-outside-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("private.txt"), b"outside the workspace").unwrap();
    symlink(outside.join("private.txt"), ws.join("link-out.txt")).unwrap();
    symlink(&outside, ws.join("dir-out")).unwrap();
    for (rel, bytes) in [
        (".k2/wiki/note.md", &b"note"[..]),
        ("sub/.k2/x.md", b"note"),
        (".k2so/old.md", b"note"),
        (".env", b"A=1"),
        ("cfg/.env.local", b"A=1"),
        (".ssh/config", b"Host x"),
        (".auth/session", b"x"),
        ("id_ed25519", b"x"),
        ("server.pem", b"x"),
        ("cred.txt", b"password = hunter2hunter2\n"),
        ("ok.txt", b"fine"),
    ] {
        let p = ws.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }
    // A link INSIDE the workspace that dresses up a private file.
    symlink(ws.join(".k2/wiki/note.md"), ws.join("innocent.md")).unwrap();
    symlink(ws.join("server.pem"), ws.join("innocent2.txt")).unwrap();
    std::fs::create_dir_all(ws.join("real")).unwrap();
    symlink(ws.join("real"), ws.join(".ssh-link")).unwrap();
    let abs = ws.join("ok.txt").to_string_lossy().into_owned();
    for (spec, status, code) in [
        ("../escape.bin", "400 Bad Request", "usage"),
        ("a/../../b", "400 Bad Request", "usage"),
        (abs.as_str(), "400 Bad Request", "usage"),
        ("link-out.txt", "400 Bad Request", "usage"),
        ("dir-out/private.txt", "400 Bad Request", "usage"),
        ("missing.txt", "404 Not Found", "not_found"),
        ("sub", "400 Bad Request", "usage"),
        (".k2/wiki/note.md", "400 Bad Request", "attachment_secret"),
        ("sub/.k2/x.md", "400 Bad Request", "attachment_secret"),
        (".k2so/old.md", "400 Bad Request", "attachment_secret"),
        (".env", "400 Bad Request", "attachment_secret"),
        ("cfg/.env.local", "400 Bad Request", "attachment_secret"),
        (".ssh/config", "400 Bad Request", "attachment_secret"),
        (".auth/session", "400 Bad Request", "attachment_secret"),
        ("id_ed25519", "400 Bad Request", "attachment_secret"),
        ("server.pem", "400 Bad Request", "attachment_secret"),
        ("cred.txt", "400 Bad Request", "attachment_secret"),
        ("innocent.md", "400 Bad Request", "attachment_secret"),
        ("innocent2.txt", "400 Bad Request", "attachment_secret"),
    ] {
        let resp = handle_send(&send_body(&path, serde_json::json!({ "attachments": ["ok.txt", spec] })));
        assert_eq!(resp.status, status, "{spec}: {}", resp.body);
        assert_eq!(body_json(&resp)["error"]["code"], code, "{spec}: {}", resp.body);
    }
    assert_eq!(outbound_count(&project_id), 0, "every refusal refuses the whole send");
    let _ = std::fs::remove_dir_all(&outside);
    done(&path, &project_id, &domain);
}

/// The same refusals hold for a LINKED sender and `k2 mail draft`: there
/// is one reader.
#[test]
fn the_reader_every_sender_shares_refuses_secrets() {
    let root = std::env::temp_dir().join(format!("mail-att-shared-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(root.join(".k2")).unwrap();
    std::fs::write(root.join(".k2/x.md"), b"x").unwrap();
    std::fs::write(root.join(".env"), b"A=1").unwrap();
    let root_s = root.to_string_lossy().into_owned();
    for spec in [".k2/x.md", ".env"] {
        let resp = read_workspace_attachments(&root_s, &[spec.to_string()]).err().expect("refused");
        assert_eq!(body_json(&resp)["error"]["code"], "attachment_secret", "{spec}");
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn attach_over_the_limits_is_refused_with_one_clear_line() {
    let (path, project_id, domain) = seed_files_sender("attbig", "approval");
    let ws = std::path::Path::new(&path);
    let write_sparse = |name: &str, len: u64| {
        let f = std::fs::File::create(ws.join(name)).unwrap();
        f.set_len(len).unwrap();
    };
    write_sparse("huge.bin", MAX_ATTACHMENT_BYTES + 1);
    write_sparse("half-a.bin", MAX_ATTACHMENT_BYTES / 2 + 1);
    write_sparse("half-b.bin", MAX_ATTACHMENT_BYTES / 2 + 1);
    write_sparse("exact.bin", MAX_ATTACHMENT_BYTES);

    let resp = handle_send(&send_body(&path, serde_json::json!({ "attachments": ["huge.bin"] })));
    assert_eq!(resp.status, "400 Bad Request", "{}", resp.body);
    let v = body_json(&resp);
    assert_eq!(v["error"]["code"], "attachment_too_large", "{v}");
    let hint = v["error"]["hint"].as_str().unwrap();
    assert!(hint.contains("huge.bin") && hint.contains("25 MiB per file"), "{hint}");

    let resp = handle_send(&send_body(&path, serde_json::json!({ "attachments": ["half-a.bin", "half-b.bin"] })));
    let v = body_json(&resp);
    assert_eq!(v["error"]["code"], "attachment_too_large", "{v}");
    assert!(v["error"]["hint"].as_str().unwrap().contains("in total"), "{v}");

    let many: Vec<String> = (0..11).map(|i| format!("f{i}")).collect();
    let resp = handle_send(&send_body(&path, serde_json::json!({ "attachments": many })));
    assert!(body_json(&resp)["error"]["hint"].as_str().unwrap().contains("too many attachments"), "{}", resp.body);
    assert_eq!(outbound_count(&project_id), 0);

    // Exactly the limit is fine.
    let resp = handle_send(&send_body(&path, serde_json::json!({ "attachments": ["exact.bin"] })));
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    let id = body_json(&resp)["id"].as_str().expect("id").to_string();
    let store = DbOutboundStore::default();
    assert_eq!(store.load_message(&id).unwrap().attachments[0].size(), MAX_ATTACHMENT_BYTES);
    send::deny(&store, &id, "owner", "cleanup", now_secs()).expect("deny");
    done(&path, &project_id, &domain);
}

#[test]
fn hosted_send_refuses_text_the_engine_could_not_take() {
    let (path, project_id, domain) = seed_files_sender("attlong", "approval");
    let long = "y".repeat(send::MAX_HOSTED_TEXT_JSON_BYTES + 1);
    let resp = handle_send(&send_body(&path, serde_json::json!({ "body": long })));
    assert_eq!(resp.status, "400 Bad Request", "{}", resp.body);
    assert!(body_json(&resp)["error"]["hint"].as_str().unwrap().contains("too large"), "{}", resp.body);
    assert_eq!(outbound_count(&project_id), 0);
    done(&path, &project_id, &domain);
}

/// A hosted reply takes files now: a good one gets as far as the engine,
/// a refused one never does, and neither uses the thread's reply budget.
#[test]
fn hosted_reply_takes_attachments_under_the_same_refusals() {
    let _mail = crate::mail::mail_server_test_lock();
    clear_mail_server();
    let (path, project_id, domain) = seed_files_sender("attreply", "approval");
    std::fs::write(std::path::Path::new(&path).join("a.bin"), [1u8]).unwrap();
    std::fs::write(std::path::Path::new(&path).join(".env"), b"A=1").unwrap();
    let token = messages::encode_message_id(&format!("bot@{domain}"), "e1");
    let reply = |attachments: serde_json::Value| {
        handle_reply(
            &serde_json::to_vec(&serde_json::json!({
                "project": path, "id": token, "body": "thanks", "attachments": attachments,
            }))
            .unwrap(),
        )
    };
    let resp = reply(serde_json::json!([".env"]));
    assert_eq!(body_json(&resp)["error"]["code"], "attachment_secret", "{}", resp.body);
    let resp = reply(serde_json::json!(["a.bin"]));
    assert_eq!(resp.status, "503 Service Unavailable", "the next step is the engine: {}", resp.body);
    assert_eq!(outbound_count(&project_id), 0);
    done(&path, &project_id, &domain);
}

#[cfg(unix)]
#[test]
fn a_link_inside_the_workspace_sends_its_target_under_the_target_name() {
    let root = std::env::temp_dir().join(format!("mail-att-link-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("real.txt"), b"real").unwrap();
    std::os::unix::fs::symlink(root.join("real.txt"), root.join("alias.txt")).unwrap();
    let root_s = root.to_string_lossy().into_owned();
    let atts = match read_workspace_attachments(&root_s, &["alias.txt".to_string()]) {
        Ok(a) => a,
        Err(resp) => panic!("expected it to read: {}", resp.body),
    };
    assert_eq!(atts[0].filename, "real.txt");
    assert_eq!(atts[0].bytes, b"real");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn attachment_names_are_safe_to_show_and_send() {
    for (raw, want) in [
        ("report.pdf", "report.pdf"),
        ("a/b/c.txt", "c.txt"),
        ("..\\..\\win.exe", "win.exe"),
        ("a\r\nX-Evil: 1.txt", "a__X-Evil: 1.txt"),
        ("inv\u{202E}gpj.exe", "inv_gpj.exe"),
        ("Zoë résumé.pdf", "Zoë résumé.pdf"),
        ("", "attachment"),
        ("  ", "attachment"),
        ("..", "attachment"),
        ("a/", "attachment"),
    ] {
        assert_eq!(safe_attachment_name(raw), want, "{raw:?}");
    }
    assert_eq!(safe_attachment_name(&"x".repeat(500)).chars().count(), 200);
}

// ── --reply-to ──

/// Reply-To shows wherever the send can be checked: the outbox, the
/// approvals list and the opened message; it reaches the header, and only
/// the header: no envelope recipient, and no use of the recipient cap.
#[test]
fn reply_to_is_audited_shown_to_the_approver_and_rides_the_header_only() {
    let (path, project_id, domain) = seed_files_sender("rto", "approval");
    let resp = handle_send(&send_body(
        &path,
        serde_json::json!({ "replyTo": ["Replies@Acme.Example", "second@acme.example"] }),
    ));
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    let id = body_json(&resp)["id"].as_str().expect("id").to_string();
    let want = serde_json::json!(["replies@acme.example", "second@acme.example"]);

    let outbox = body_json(&handle_outbox(&params(&[("project", &path), ("id", &id)])));
    assert_eq!(outbox["outbound"]["replyTo"], want, "{outbox}");
    let list = body_json(&handle_approvals_list(&HashMap::new()));
    let ours = list["pending"].as_array().unwrap().iter().find(|p| p["id"] == id.as_str()).expect("queued").clone();
    assert_eq!(ours["replyTo"], want, "{ours}");
    let shown = body_json(&handle_approvals_show(&params(&[("id", &id)])));
    assert_eq!(shown["message"]["replyTo"], want, "{shown}");

    let store = DbOutboundStore::default();
    let rec = RecordingSubmit::default();
    let mut acct = |_from: &str| Ok("acc-1".to_string());
    send::approve_and_submit(&store, Ok(&rec), &mut acct, &id, "owner", None, now_secs()).expect("approve");
    let seen = rec.seen.lock().unwrap();
    let (rcpts, email) = &seen[0];
    assert_eq!(rcpts, &vec!["human@example.com".to_string()], "nothing is sent to a Reply-To");
    assert_eq!(email["replyTo"], serde_json::json!([{ "email": "replies@acme.example" }, { "email": "second@acme.example" }]), "{email}");
    drop(seen);
    done(&path, &project_id, &domain);
}

#[test]
fn a_send_without_reply_to_sets_no_header_and_stores_no_column() {
    let (path, project_id, domain) = seed_files_sender("rtonone", "approval");
    let resp = handle_send(&send_body(&path, serde_json::json!({})));
    let id = body_json(&resp)["id"].as_str().expect("id").to_string();
    let store = DbOutboundStore::default();
    assert!(store.load(&id).unwrap().unwrap().reply_to_json.is_none());
    let stored = store.load_message(&id).unwrap();
    assert!(send::submission_email_json(&stored).get("replyTo").is_none());
    done(&path, &project_id, &domain);
}

/// Ten recipients and a Reply-To: the cap counts who gets mail.
#[test]
fn reply_to_does_not_count_toward_the_recipient_cap() {
    let (path, project_id, domain) = seed_files_sender("rtocap", "approval");
    let ten: Vec<String> = (0..10).map(|i| format!("r{i}@dest.example")).collect();
    let resp = handle_send(&send_body(&path, serde_json::json!({ "to": ten, "replyTo": "here@dest.example" })));
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    done(&path, &project_id, &domain);
}

#[test]
fn reply_to_refuses_anything_but_a_plain_address() {
    let (path, project_id, domain) = seed_files_sender("rtobad", "approval");
    for bad in [
        serde_json::json!("x@y.example\r\nBcc: evil@z.example"),
        serde_json::json!("x@y.example\nX-Evil: 1"),
        serde_json::json!("x@y.example\r"),
        serde_json::json!("Evil <x@y.example>"),
        serde_json::json!("x@y.example, z@y.example"),
        serde_json::json!("not-an-address"),
        serde_json::json!(""),
        serde_json::json!(42),
        serde_json::json!(["a@b.example", "c@d.example", "e@f.example", "g@h.example"]),
    ] {
        let resp = handle_send(&send_body(&path, serde_json::json!({ "replyTo": bad.clone() })));
        assert_eq!(resp.status, "400 Bad Request", "{bad}: {}", resp.body);
        assert_eq!(body_json(&resp)["error"]["code"], "usage", "{bad}");
    }
    assert_eq!(outbound_count(&project_id), 0, "nothing queued");
    done(&path, &project_id, &domain);
}

#[test]
fn linked_send_refuses_reply_to() {
    let (name, path) = unique("lrto");
    let project_id = insert_project(&name, &path);
    let linked = format!("me-{}@linked.example", &project_id[..8]);
    seed_linked(&project_id, &linked, "send");
    set_gating(&path, "on");
    let resp = handle_send(&send_body(&path, serde_json::json!({ "from": linked, "replyTo": "x@y.example" })));
    assert_eq!(resp.status, "400 Bad Request", "{}", resp.body);
    assert!(body_json(&resp)["error"]["hint"].as_str().unwrap().contains("hosted K2 mailboxes only"), "{}", resp.body);
    assert_eq!(outbound_count(&project_id), 0);
    cleanup_linked(&project_id);
}

/// `k2 mail reply` stays locked to the original sender: a Reply-To is
/// refused, not dropped, before anything else happens; a recipient in the
/// body is ignored, and the reply carries no Reply-To.
#[test]
fn reply_stays_locked_and_refuses_reply_to() {
    let _key = k2_core::test_env::EnvVar::set("K2_TEST_MAIL_API_KEY", "k2-test-key");
    let _mail = crate::mail::mail_server_test_lock();
    let _row = crate::mail::MailServerRowCleanup;
    let fake = crate::mail::fake_jmap::FakeJmap::start();
    seed_fake_engine(&fake);
    fake.state.lock().unwrap().emails.insert("e1".to_string(), reply_source("e1"));
    let (path, project_id, domain) = seed_files_sender("rtolock", "approval");
    let token = messages::encode_message_id(&format!("bot@{domain}"), "e1");
    let resp = handle_reply(
        &serde_json::to_vec(&serde_json::json!({
            "project": path, "id": token, "body": "thanks", "replyTo": "elsewhere@evil.example",
        }))
        .unwrap(),
    );
    assert_eq!(resp.status, "400 Bad Request", "{}", resp.body);
    assert!(body_json(&resp)["error"]["hint"].as_str().unwrap().contains("original sender"), "{}", resp.body);
    assert_eq!(outbound_count(&project_id), 0);
    let resp = handle_reply(
        &serde_json::to_vec(&serde_json::json!({
            "project": path, "id": token, "body": "thanks", "to": "elsewhere@evil.example",
            "cc": "elsewhere@evil.example",
        }))
        .unwrap(),
    );
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    let id = body_json(&resp)["id"].as_str().expect("id").to_string();
    let stored = DbOutboundStore::default().load_message(&id).unwrap();
    assert_eq!(stored.all_recipients(), vec!["cust@dest.example".to_string()], "locked to the original sender");
    assert!(stored.reply_to.is_empty());
    assert!(send::submission_email_json(&stored).get("replyTo").is_none());
    send::deny(&DbOutboundStore::default(), &id, "owner", "cleanup", now_secs()).expect("deny");
    done(&path, &project_id, &domain);
}

// ── k2 mail forward-message: the route's gates. The composed message and
// its round trip against the fake engine: mail::fwd_message::tests. ──

fn forward_body_json(path: &str, token: &str, extra: serde_json::Value) -> Vec<u8> {
    let mut v = serde_json::json!({ "project": path, "id": token, "to": "dest@else.example" });
    if let (Some(base), Some(add)) = (v.as_object_mut(), extra.as_object()) {
        for (k, val) in add {
            base.insert(k.clone(), val.clone());
        }
    }
    serde_json::to_vec(&v).expect("body")
}

#[test]
fn forward_validates_its_body_first() {
    let tok = messages::encode_message_id("a@b.example", "e1");
    for (bad, needle) in [
        (serde_json::json!({}), "project"),
        (serde_json::json!({ "project": "/tmp/x" }), "id"),
        (serde_json::json!({ "project": "/tmp/x", "id": tok }), "to"),
        (serde_json::json!({ "project": "/tmp/x", "id": tok, "to": "a@b.example", "note": 3 }), "note"),
        (
            serde_json::json!({ "project": "/tmp/x", "id": tok, "to": "a@b.example", "replyTo": "c@d.example", "replyToOriginal": true }),
            "only one",
        ),
        (
            serde_json::json!({ "project": "/tmp/x", "id": tok, "to": "a@b.example", "replyTo": "c@d.example\r\nBcc: e@f.example" }),
            "--reply-to",
        ),
    ] {
        let resp = handle_forward_message(&serde_json::to_vec(&bad).unwrap());
        assert_eq!(resp.status, "400 Bad Request", "{bad}: {}", resp.body);
        assert!(body_json(&resp)["error"]["hint"].as_str().unwrap().contains(needle), "{needle}: {}", resp.body);
    }
}

/// A message the caller may not read answers exactly like one that doesn't
/// exist, under every mode, and nothing is recorded. Not reading is the
/// same check as `k2 mail read`: the same id gets the same answer there.
#[test]
fn forward_of_a_message_the_caller_cannot_read_is_masked() {
    let _mail = crate::mail::mail_server_test_lock();
    clear_mail_server();
    let (owner_path, owner_id, domain) = seed_files_sender("fwdowner", "on");
    let token = messages::encode_message_id(&format!("bot@{domain}"), "e1");
    let (stranger_path, stranger_id, stranger_domain) = seed_files_sender("fwdstranger", "on");
    for mode in ["off", "approval", "on"] {
        set_gating(&stranger_path, mode);
        let resp = handle_forward_message(&forward_body_json(&stranger_path, &token, serde_json::json!({})));
        assert_eq!(resp.status, "404 Not Found", "{mode}: {}", resp.body);
        let v = body_json(&resp);
        assert_eq!(v["error"]["code"], "not_found", "{mode}");
        assert_eq!(v["error"]["hint"], format!("no message '{token}' in this workspace"), "{mode}: masked");
        // Naming their own mailbox as the sender doesn't open it either.
        let resp = handle_forward_message(&forward_body_json(
            &stranger_path,
            &token,
            serde_json::json!({ "from": format!("bot@{stranger_domain}") }),
        ));
        assert_eq!(resp.status, "404 Not Found", "{mode}: {}", resp.body);
    }
    let read = crate::mail::routes_messages::handle_read(&params(&[("project", &stranger_path), ("id", &token)]));
    assert_eq!(read.status, "404 Not Found", "k2 mail read answers the same: {}", read.body);
    assert_eq!(body_json(&read)["error"]["hint"], format!("no message '{token}' in this workspace"));
    assert_eq!(outbound_count(&stranger_id), 0);
    assert_eq!(outbound_count(&owner_id), 0);
    done(&stranger_path, &stranger_id, &stranger_domain);
    done(&owner_path, &owner_id, &domain);
}

#[test]
fn forward_with_sending_off_exits_3_before_reading_the_original() {
    let _mail = crate::mail::mail_server_test_lock();
    clear_mail_server();
    let (path, project_id, domain) = seed_files_sender("fwdoff", "off");
    let token = messages::encode_message_id(&format!("bot@{domain}"), "e1");
    let resp = handle_forward_message(&forward_body_json(&path, &token, serde_json::json!({})));
    assert_eq!(resp.status, "403 Forbidden", "{}", resp.body);
    assert_eq!(body_json(&resp)["error"]["code"], "gated");
    assert_eq!(outbound_count(&project_id), 0);
    done(&path, &project_id, &domain);
}

/// Past every gate the next step is the engine; with none here that is
/// where it stops, having recorded nothing.
#[test]
fn forward_reaches_the_engine_only_past_the_gates() {
    let _mail = crate::mail::mail_server_test_lock();
    clear_mail_server();
    for mode in ["approval", "on"] {
        let (path, project_id, domain) = seed_files_sender(&format!("fwdok{mode}"), mode);
        let token = messages::encode_message_id(&format!("bot@{domain}"), "e1");
        let resp = handle_forward_message(&forward_body_json(&path, &token, serde_json::json!({})));
        assert_eq!(resp.status, "503 Service Unavailable", "{mode}: {}", resp.body);
        // The same caps as a send come first.
        let many: Vec<String> = (0..11).map(|i| format!("r{i}@dest.example")).collect();
        let resp = handle_forward_message(&forward_body_json(&path, &token, serde_json::json!({ "to": many })));
        assert_eq!(resp.status, "400 Bad Request", "{mode}: {}", resp.body);
        assert!(body_json(&resp)["error"]["hint"].as_str().unwrap().contains("too many recipients"), "{}", resp.body);
        assert_eq!(outbound_count(&project_id), 0);
        done(&path, &project_id, &domain);
    }
}

#[test]
fn forward_from_or_of_a_linked_inbox_is_refused_with_a_clear_line() {
    let (name, path) = unique("fwdlinked");
    let project_id = insert_project(&name, &path);
    let linked = format!("me-{}@linked.example", &project_id[..8]);
    seed_linked(&project_id, &linked, "send");
    set_gating(&path, "on");
    let token = messages::encode_message_id(&linked, "uid:1");
    let resp = handle_forward_message(&forward_body_json(&path, &token, serde_json::json!({})));
    assert_eq!(resp.status, "400 Bad Request", "{}", resp.body);
    assert!(body_json(&resp)["error"]["hint"].as_str().unwrap().contains("hosted K2 mailboxes only"), "{}", resp.body);
    assert_eq!(outbound_count(&project_id), 0);
    cleanup_linked(&project_id);
}

#[test]
fn forward_needs_a_person_when_nothing_vouches_for_the_sender() {
    use crate::mail::jmap::MailAddr;
    let host = messages::AuthAuthority::ThisBox(Some("mx.example".to_string()));
    let one = vec![MailAddr { name: None, email: "a@x.example".to_string() }];
    let gate = |lines: &[&str], from: &[MailAddr], on_box: bool, g: Gate| {
        let lines: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        forward_sender_gate(&lines, &host, from, on_box, g)
    };
    let fail = ["mx.example; dmarc=fail header.from=x.example"];
    assert!(matches!(gate(&fail, &one, false, Gate::On), Err(SendError::Guardrail(_))));
    assert!(gate(&fail, &one, false, Gate::Approval).is_ok());
    assert!(gate(&["mx.example; dmarc=pass header.from=x.example"], &one, false, Gate::On).is_ok());
    assert!(gate(&["mx.example; dmarc=none header.from=x.example"], &one, false, Gate::On).is_ok());
    // No stamp: a mailbox on this box, not an outside sender.
    assert!(gate(&[], &one, true, Gate::On).is_ok());
    assert!(gate(&[], &one, false, Gate::On).is_err());
    // A crafted envelope sender: the stamp's real verdict still blocks.
    let crafted = ["mx.example; smtp.mailfrom=\"dmarc=pass@x.example\"; dmarc=fail"];
    assert!(matches!(gate(&crafted, &one, false, Gate::On), Err(SendError::Guardrail(_))));
    // Several From addresses.
    let many = vec![one[0].clone(), MailAddr { name: None, email: "ceo@paypal.com".to_string() }];
    let empty = ["mx.example; dmarc=none header.from= policy.dmarc=none"];
    assert!(gate(&empty, &many, false, Gate::On).is_err());
    assert!(gate(&empty, &one, false, Gate::On).is_err());
}

/// What the approver sees for a forward: what it forwards, its files, any
/// Reply-To; and a body file that holds the captured bytes until decided.
#[test]
fn a_held_forward_says_what_it_forwards() {
    let (path, project_id, domain) = seed_files_sender("fwdheld", "approval");
    let store = DbOutboundStore::default();
    let msg = OutboundMessage {
        from_name: Some("Bot".to_string()),
        from: format!("bot@{domain}"),
        to: vec!["dest@else.example".to_string()],
        cc: Vec::new(),
        bcc: Vec::new(),
        policy_bcc: Vec::new(),
        reply_to: vec!["sam@outside.example".to_string()],
        subject: "Fwd: Quote".to_string(),
        text_body: "The original message is attached.".to_string(),
        in_reply_to: None,
        references: None,
        reply_source: None,
        attachments: vec![send::StoredAttachment::from_out(&external_smtp::OutAttachment {
            scanned: false, filename: "original.eml".to_string(),
            content_type: "application/octet-stream".to_string(),
            bytes: b"raw".to_vec(),
        })],
        ephemeral: false,
        notify: None,
        forward_of: Some(send::ForwardOf {
            mailbox: format!("bot@{domain}"),
            sender: "sam@outside.example".to_string(),
            as_eml: true,
        }),
        source_dmarc: None,
        source_check: None,
    };
    let req = OutboundRequest { project_id: &project_id, agent_name: "bot", account_id: "a", message: &msg, send_after: None };
    let SendOutcome::Queued { id, .. } = send::gate_and_dispatch(&store, None, Gate::Approval, &req, now_secs()).expect("queue") else {
        panic!("expected a queued message")
    };
    let list = body_json(&handle_approvals_list(&HashMap::new()));
    let ours = list["pending"].as_array().unwrap().iter().find(|p| p["id"] == id.as_str()).expect("queued").clone();
    assert_eq!(ours["forwardOf"]["sender"], "sam@outside.example", "{ours}");
    assert_eq!(ours["forwardOf"]["asEml"], true, "{ours}");
    assert_eq!(ours["files"][0]["name"], "original.eml", "{ours}");
    assert_eq!(ours["replyTo"], serde_json::json!(["sam@outside.example"]), "{ours}");
    let shown = body_json(&handle_approvals_show(&params(&[("id", &id)])));
    assert_eq!(shown["message"]["forwardOf"]["mailbox"], format!("bot@{domain}"), "{shown}");
    let outbox = body_json(&handle_outbox(&params(&[("project", &path), ("id", &id)])));
    assert_eq!(outbox["outbound"]["attachments"]["names"], serde_json::json!(["original.eml"]), "{outbox}");
    send::deny(&store, &id, "owner", "cleanup", now_secs()).expect("deny");
    done(&path, &project_id, &domain);
}

// ── forward-message, all the way through the handler, fake engine ──

fn forward_source(id: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "blobId": "raw-1",
        "threadId": format!("t-{id}"),
        "from": [{ "name": "Sam", "email": "Sam@Outside.Example" }],
        "to": [{ "email": "bot@acme.dev" }],
        "subject": "Quote\r\nBcc: evil@x.example",
        "sentAt": "2026-11-01T10:00:00Z",
        "receivedAt": "2026-11-01T10:00:05Z",
        "size": 40,
        "keywords": {},
        "hasAttachment": true,
        "textBody": [{ "partId": "1", "type": "text/plain" }],
        "htmlBody": [],
        "bodyValues": { "1": { "value": "Please see the quote.\n" } },
        "attachments": [{ "blobId": "att-1", "name": "quote.pdf", "type": "application/pdf", "size": 9 }],
        "header:Authentication-Results:asText:all": ["mail.box.test; dmarc=pass header.from=outside.example"],
    })
}

fn seed_forward_fake() -> crate::mail::fake_jmap::FakeJmap {
    let fake = crate::mail::fake_jmap::FakeJmap::start();
    {
        let mut st = fake.state.lock().unwrap();
        st.emails.insert("e1".to_string(), forward_source("e1"));
        st.blobs.insert("att-1".to_string(), b"pdf bytes".to_vec());
        st.blobs.insert("raw-1".to_string(), b"Subject: Quote\r\n\r\nraw bytes\r\n".to_vec());
    }
    seed_fake_engine(&fake);
    fake
}

/// Approval: nothing leaves and nothing about the original is lost if it
/// goes away; the owner's BCC rides; From is the mailbox; approving sends
/// exactly what was captured.
#[test]
fn a_forward_under_approval_holds_a_copy_and_sends_it_on_approve() {
    let _key = k2_core::test_env::EnvVar::set("K2_TEST_MAIL_API_KEY", "k2-test-key");
    let _mail = crate::mail::mail_server_test_lock();
    let _row = crate::mail::MailServerRowCleanup;
    let fake = seed_forward_fake();
    let (path, project_id, domain) = seed_files_sender("fwdappr", "approval");
    k2_core::workspace::settings::set_mail_always_bcc(&path, &["boss@acme.example".to_string()]).expect("policy");
    let mailbox = format!("bot@{domain}");
    let token = messages::encode_message_id(&mailbox, "e1");

    let resp = handle_forward_message(&forward_body_json(&path, &token, serde_json::json!({ "note": "FYI" })));
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    let v = body_json(&resp);
    assert_eq!(v["status"], "pending_approval", "{v}");
    let id = v["id"].as_str().expect("id").to_string();

    let store = DbOutboundStore::default();
    let stored = store.load_message_full(&id).unwrap();
    assert_eq!(stored.from, mailbox, "the mailbox, never the original sender");
    assert_eq!(stored.to, vec!["dest@else.example".to_string()]);
    assert_eq!(stored.policy_bcc, vec!["boss@acme.example".to_string()], "the owner's BCC");
    assert_eq!(stored.subject, "Fwd: Quote Bcc: evil@x.example", "one line");
    assert!(stored.text_body.starts_with("FYI\n\n---------- Forwarded message"), "{}", stored.text_body);
    assert!(stored.reply_to.is_empty(), "no Reply-To by default");
    assert_eq!(decoded(&stored.attachments[0]), b"pdf bytes");
    assert_eq!(stored.forward_of.as_ref().map(|f| f.sender.as_str()), Some("Sam@Outside.Example"));
    assert!(fake.state.lock().unwrap().uploads.is_empty(), "nothing left yet");

    let list = body_json(&handle_approvals_list(&HashMap::new()));
    let ours = list["pending"].as_array().unwrap().iter().find(|p| p["id"] == id.as_str()).expect("queued").clone();
    assert_eq!(ours["files"][0]["name"], "quote.pdf", "{ours}");
    assert_eq!(ours["files"][0]["size"], 9, "{ours}");
    assert_eq!(ours["forwardOf"]["mailbox"], mailbox.as_str(), "{ours}");

    {
        let mut st = fake.state.lock().unwrap();
        st.emails.clear();
        st.blobs.clear();
    }
    let client = fake.client();
    let mut acct = |_: &str| Ok("acc-1".to_string());
    send::approve_and_submit(&store, Ok(&client), &mut acct, &id, "owner", None, now_secs()).expect("approve");
    let st = fake.state.lock().unwrap();
    assert_eq!(st.uploads, vec![b"pdf bytes".to_vec()]);
    let e = &st.created[0];
    assert_eq!(e["from"][0]["email"], mailbox.as_str(), "{e}");
    assert!(e.get("replyTo").is_none(), "{e}");
    assert!(!e.to_string().contains("boss@acme.example"), "a BCC is no header: {e}");
    let rcpt = st.envelopes[0]["rcptTo"].to_string();
    assert!(rcpt.contains("dest@else.example") && rcpt.contains("boss@acme.example"), "{rcpt}");
    drop(st);
    assert!(store.load_message(&id).unwrap().attachments.is_empty(), "no bytes once decided");
    k2_core::workspace::settings::set_mail_always_bcc(&path, &[]).expect("policy");
    done(&path, &project_id, &domain);
}

/// On + --as-eml + --reply-to-original: the raw message goes unchanged, and
/// Reply-To is the original sender, in the header and in the outbox.
#[test]
fn an_eml_forward_on_mode_with_reply_to_original() {
    let _key = k2_core::test_env::EnvVar::set("K2_TEST_MAIL_API_KEY", "k2-test-key");
    let _mail = crate::mail::mail_server_test_lock();
    let _row = crate::mail::MailServerRowCleanup;
    let fake = seed_forward_fake();
    let (path, project_id, domain) = seed_files_sender("fwdeml", "on");
    let token = messages::encode_message_id(&format!("bot@{domain}"), "e1");

    let resp = handle_forward_message(&forward_body_json(
        &path,
        &token,
        serde_json::json!({ "asEml": true, "replyToOriginal": true }),
    ));
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    let v = body_json(&resp);
    assert_eq!(v["status"], "submitted", "{v}");
    let id = v["id"].as_str().expect("id").to_string();

    let st = fake.state.lock().unwrap();
    assert_eq!(st.uploads, vec![b"Subject: Quote\r\n\r\nraw bytes\r\n".to_vec()]);
    let e = &st.created[0];
    assert_eq!(e["attachments"][0]["type"], "application/octet-stream", "{e}");
    assert_eq!(e["attachments"][0]["name"], "original.eml", "{e}");
    assert_eq!(e["replyTo"], serde_json::json!([{ "email": "sam@outside.example" }]), "{e}");
    assert_eq!(e["from"][0]["email"], format!("bot@{domain}"), "{e}");
    drop(st);
    let outbox = body_json(&handle_outbox(&params(&[("project", &path), ("id", &id)])));
    assert_eq!(outbox["outbound"]["replyTo"], serde_json::json!(["sam@outside.example"]), "{outbox}");
    assert_eq!(outbox["outbound"]["forwardOf"]["sender"], "Sam@Outside.Example", "{outbox}");
    assert_eq!(outbox["outbound"]["forwardOf"]["asEml"], true, "{outbox}");
    assert_eq!(outbox["outbound"]["attachments"]["names"], serde_json::json!(["original.eml"]), "{outbox}");
    done(&path, &project_id, &domain);
}

/// Reply-To on the forward verb, with injection refused before any fetch;
/// and an original with a broken sender can't become a Reply-To.
#[test]
fn forward_reply_to_is_checked_and_a_bad_original_sender_is_refused() {
    let _key = k2_core::test_env::EnvVar::set("K2_TEST_MAIL_API_KEY", "k2-test-key");
    let _mail = crate::mail::mail_server_test_lock();
    let _row = crate::mail::MailServerRowCleanup;
    let fake = seed_forward_fake();
    let (path, project_id, domain) = seed_files_sender("fwdrto", "approval");
    let token = messages::encode_message_id(&format!("bot@{domain}"), "e1");

    let resp = handle_forward_message(&forward_body_json(&path, &token, serde_json::json!({ "replyTo": "here@acme.example" })));
    let id = body_json(&resp)["id"].as_str().expect("id").to_string();
    let store = DbOutboundStore::default();
    assert_eq!(store.load_message(&id).unwrap().reply_to, vec!["here@acme.example".to_string()]);
    send::deny(&store, &id, "owner", "cleanup", now_secs()).expect("deny");

    fake.state.lock().unwrap().emails.get_mut("e1").unwrap()["from"] =
        serde_json::json!([{ "email": "x@y.example\r\nBcc: evil@z.example" }]);
    let before = outbound_count(&project_id);
    let resp = handle_forward_message(&forward_body_json(&path, &token, serde_json::json!({ "replyToOriginal": true })));
    assert_eq!(resp.status, "400 Bad Request", "{}", resp.body);
    assert!(body_json(&resp)["error"]["hint"].as_str().unwrap().contains("--reply-to-original"), "{}", resp.body);
    assert_eq!(outbound_count(&project_id), before, "nothing recorded");
    done(&path, &project_id, &domain);
}

/// DMARC-fail originals need a person; the same message queues under approval.
#[test]
fn forward_of_a_dmarc_failure_is_refused_on_and_queued_under_approval() {
    let _key = k2_core::test_env::EnvVar::set("K2_TEST_MAIL_API_KEY", "k2-test-key");
    let _mail = crate::mail::mail_server_test_lock();
    let _row = crate::mail::MailServerRowCleanup;
    let fake = seed_forward_fake();
    fake.state.lock().unwrap().emails.get_mut("e1").unwrap()["header:Authentication-Results:asText:all"] =
        serde_json::json!(["mail.box.test; dmarc=fail header.from=outside.example"]);
    let (path, project_id, domain) = seed_files_sender("fwddmarc", "on");
    let token = messages::encode_message_id(&format!("bot@{domain}"), "e1");
    let resp = handle_forward_message(&forward_body_json(&path, &token, serde_json::json!({})));
    assert_eq!(resp.status, "409 Conflict", "{}", resp.body);
    assert_eq!(body_json(&resp)["error"]["code"], "guardrail");
    assert_eq!(outbound_count(&project_id), 0);
    set_gating(&path, "approval");
    let resp = handle_forward_message(&forward_body_json(&path, &token, serde_json::json!({})));
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    let id = body_json(&resp)["id"].as_str().expect("id").to_string();
    send::deny(&DbOutboundStore::default(), &id, "owner", "cleanup", now_secs()).expect("deny");
    done(&path, &project_id, &domain);
}

/// A workspace with a read grant can read the message, and may forward it
/// from its own mailbox; it can't send as the mailbox it only reads.
#[test]
fn a_read_grant_forwards_only_from_a_mailbox_of_its_own() {
    let _key = k2_core::test_env::EnvVar::set("K2_TEST_MAIL_API_KEY", "k2-test-key");
    let _mail = crate::mail::mail_server_test_lock();
    let _row = crate::mail::MailServerRowCleanup;
    let fake = seed_forward_fake();
    let (owner_path, owner_id, owner_domain) = seed_files_sender("fwdsrc", "on");
    let (reader_path, reader_id, reader_domain) = seed_files_sender("fwdrdr", "approval");
    let source = format!("bot@{owner_domain}");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_inbox_grants (source, inbox_id, project_id, level, created_at) \
                 SELECT 'hosted', id, ?1, 'read', 100 FROM mail_addresses WHERE address = ?2",
            rusqlite::params![reader_id, source],
        )
        .expect("grant");
    }
    let token = messages::encode_message_id(&source, "e1");
    let resp = handle_forward_message(&forward_body_json(&reader_path, &token, serde_json::json!({})));
    assert_eq!(resp.status, "403 Forbidden", "{}", resp.body);
    assert!(body_json(&resp)["error"]["hint"].as_str().unwrap().contains("--from"), "{}", resp.body);
    let resp = handle_forward_message(&forward_body_json(&reader_path, &token, serde_json::json!({ "from": source })));
    assert_eq!(resp.status, "403 Forbidden", "{}", resp.body);
    assert_eq!(outbound_count(&reader_id), 0);

    let own = format!("bot@{reader_domain}");
    let resp = handle_forward_message(&forward_body_json(&reader_path, &token, serde_json::json!({ "from": own })));
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    let id = body_json(&resp)["id"].as_str().expect("id").to_string();
    let store = DbOutboundStore::default();
    let stored = store.load_message(&id).unwrap();
    assert_eq!(stored.from, own);
    assert_eq!(stored.forward_of.as_ref().map(|f| f.mailbox.as_str()), Some(source.as_str()));
    send::deny(&store, &id, "owner", "cleanup", now_secs()).expect("deny");
    let _ = fake;
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("DELETE FROM mail_inbox_grants WHERE project_id = ?1", rusqlite::params![reader_id]).unwrap();
    }
    done(&reader_path, &reader_id, &reader_domain);
    done(&owner_path, &owner_id, &owner_domain);
}

/// The send's hourly cap counts forwards; a message gone from the server
/// answers as a stranger's does.
#[test]
fn forward_counts_toward_the_rate_limit_and_masks_a_missing_message() {
    let _key = k2_core::test_env::EnvVar::set("K2_TEST_MAIL_API_KEY", "k2-test-key");
    let _mail = crate::mail::mail_server_test_lock();
    let _row = crate::mail::MailServerRowCleanup;
    let _fake = seed_forward_fake();
    let (path, project_id, domain) = seed_files_sender("fwdrate", "approval");
    let token = messages::encode_message_id(&format!("bot@{domain}"), "e1");
    let missing = messages::encode_message_id(&format!("bot@{domain}"), "gone");
    let resp = handle_forward_message(&forward_body_json(&path, &missing, serde_json::json!({})));
    assert_eq!(resp.status, "404 Not Found", "{}", resp.body);
    assert_eq!(body_json(&resp)["error"]["hint"], format!("no message '{missing}' in this workspace"));
    for i in 0..send::RATE_LIMIT_HOURLY {
        let resp = handle_forward_message(&forward_body_json(&path, &token, serde_json::json!({})));
        assert_eq!(resp.status, "200 OK", "{i}: {}", resp.body);
    }
    let resp = handle_forward_message(&forward_body_json(&path, &token, serde_json::json!({})));
    assert_eq!(resp.status, "429 Too Many Requests", "{}", resp.body);
    done(&path, &project_id, &domain);
}

/// Over the limit, a send is refused before a file is read: the path here
/// doesn't exist, and reading it first would answer 404.
#[test]
fn the_rate_limit_comes_before_any_file_is_read() {
    let (path, project_id, domain) = seed_files_sender("attrate", "approval");
    let store = DbOutboundStore::default();
    let m = OutboundMessage {
        from_name: None,
        from: format!("bot@{domain}"),
        to: vec!["a@b.example".to_string()],
        cc: Vec::new(),
        bcc: Vec::new(),
        policy_bcc: Vec::new(),
        reply_to: Vec::new(),
        subject: "s".to_string(),
        text_body: "t".to_string(),
        in_reply_to: None,
        references: None,
        reply_source: None,
        attachments: Vec::new(),
        ephemeral: false,
        notify: None,
        forward_of: None,
        source_dmarc: None,
        source_check: None,
    };
    for _ in 0..send::RATE_LIMIT_HOURLY {
        let req = OutboundRequest { project_id: &project_id, agent_name: "bot", account_id: "a", message: &m, send_after: None };
        send::gate_and_dispatch(&store, None, Gate::Approval, &req, now_secs()).expect("queue");
    }
    let resp = handle_send(&send_body(&path, serde_json::json!({ "attachments": ["ghost.bin"] })));
    assert_eq!(resp.status, "429 Too Many Requests", "{}", resp.body);
    done(&path, &project_id, &domain);
}

/// The approver reads exactly what leaves: only a one-time-link mail hides
/// anything, and an agent can't hide data by dressing it as a link.
#[test]
fn the_approver_sees_what_leaves() {
    let (path, project_id, domain) = seed_files_sender("seeall", "approval");
    let secret = "k2inv_ThisIsTheDataTheAgentWantsToHide";
    let resp = handle_send(&send_body(&path, serde_json::json!({ "body": secret, "fromName": "Rosson" })));
    let id = body_json(&resp)["id"].as_str().expect("id").to_string();
    let list = body_json(&handle_approvals_list(&HashMap::new()));
    let ours = list["pending"].as_array().unwrap().iter().find(|p| p["id"] == id.as_str()).expect("queued").clone();
    assert_eq!(ours["bodyPreview"], secret, "{ours}");
    assert_eq!(ours["fromName"], "Rosson", "{ours}");
    let shown = body_json(&handle_approvals_show(&params(&[("id", &id)])));
    assert_eq!(shown["message"]["text"], secret, "{shown}");
    assert_eq!(shown["message"]["fromName"], "Rosson", "{shown}");

    let store = DbOutboundStore::default();
    let stored = store.load_message(&id).unwrap();
    let link_mail = OutboundMessage { ephemeral: true, ..stored };
    assert_eq!(send::text_for_approver(&link_mail), "k2inv_[hidden]");
    done(&path, &project_id, &domain);
}

/// A reply or forward carries the original's verdict to the approver.
#[test]
fn the_approver_sees_the_originals_dmarc_verdict() {
    let _key = k2_core::test_env::EnvVar::set("K2_TEST_MAIL_API_KEY", "k2-test-key");
    let _mail = crate::mail::mail_server_test_lock();
    let _row = crate::mail::MailServerRowCleanup;
    let fake = seed_forward_fake();
    let failed = "mail.box.test; spf=none; dmarc=fail header.from=outside.example";
    fake.state.lock().unwrap().emails.get_mut("e1").unwrap()["header:Authentication-Results:asText:all"] =
        serde_json::json!([failed]);
    let (path, project_id, domain) = seed_files_sender("seedmarc", "approval");
    let token = messages::encode_message_id(&format!("bot@{domain}"), "e1");
    let resp = handle_forward_message(&forward_body_json(&path, &token, serde_json::json!({})));
    let id = body_json(&resp)["id"].as_str().expect("id").to_string();
    let list = body_json(&handle_approvals_list(&HashMap::new()));
    let ours = list["pending"].as_array().unwrap().iter().find(|p| p["id"] == id.as_str()).expect("queued").clone();
    assert_eq!(ours["sourceDmarc"], "fail", "{ours}");
    let shown = body_json(&handle_approvals_show(&params(&[("id", &id)])));
    assert_eq!(shown["message"]["sourceDmarc"], "fail", "{shown}");
    done(&path, &project_id, &domain);
}

#[test]
fn who_decided_is_recorded_as_who_they_are() {
    use crate::mail::agent_creds::MailCaller;
    let (name, path) = unique("decider");
    let project_id = insert_project(&name, &path);
    let it = MailCaller::ItAgent { workspace_uuid: project_id.clone() };
    assert_eq!(decided_by_for(&it, Some("owner")), format!("agent:{name}"), "an agent can't claim to be the owner");
    assert_eq!(decided_by_for(&it, None), format!("agent:{name}"));
    assert_eq!(decided_by_for(&MailCaller::Owner, Some("rosson")), "rosson");
    assert_eq!(decided_by_for(&MailCaller::Owner, None), "owner");
    cleanup(&project_id, None);
}

/// What the check read is shown to the approver.
#[test]
fn the_approver_is_told_what_wasnt_scanned() {
    let (path, project_id, domain) = seed_files_sender("scanned", "approval");
    let ws = std::path::Path::new(&path);
    std::fs::write(ws.join("note.txt"), b"plain words").unwrap();
    std::fs::write(ws.join("photo.bin"), [0u8, 1, 2, 3]).unwrap();
    std::fs::write(ws.join("help.txt"), b"The password: field is required.\n").unwrap();
    let resp = handle_send(&send_body(&path, serde_json::json!({ "attachments": ["note.txt", "photo.bin", "help.txt"] })));
    assert_eq!(resp.status, "200 OK", "a sentence about passwords isn't one: {}", resp.body);
    let id = body_json(&resp)["id"].as_str().expect("id").to_string();
    let list = body_json(&handle_approvals_list(&HashMap::new()));
    let ours = list["pending"].as_array().unwrap().iter().find(|p| p["id"] == id.as_str()).expect("queued").clone();
    assert_eq!(ours["files"][0]["scanned"], true, "{ours}");
    assert_eq!(ours["files"][1]["scanned"], false, "{ours}");
    assert_eq!(ours["files"][2]["scanned"], true, "{ours}");
    send::deny(&DbOutboundStore::default(), &id, "owner", "cleanup", now_secs()).expect("deny");
    done(&path, &project_id, &domain);
}

/// The approver and the card see an original the check didn't clear, and a
/// forward leaves one audit line.
#[test]
fn a_forward_records_the_sender_check_and_an_audit_line() {
    let _key = k2_core::test_env::EnvVar::set("K2_TEST_MAIL_API_KEY", "k2-test-key");
    let _home = crate::test_support::TempHome::new();
    let _mail = crate::mail::mail_server_test_lock();
    let _row = crate::mail::MailServerRowCleanup;
    let fake = seed_forward_fake();
    fake.state.lock().unwrap().emails.get_mut("e1").unwrap()["header:Authentication-Results:asText:all"] =
        serde_json::json!(["mail.box.test; dmarc=none header.from= policy.dmarc=none"]);
    let (path, project_id, domain) = seed_files_sender("fwdcheck", "approval");
    let token = messages::encode_message_id(&format!("bot@{domain}"), "e1");
    let resp = handle_forward_message(&forward_body_json(&path, &token, serde_json::json!({})));
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    let id = body_json(&resp)["id"].as_str().expect("id").to_string();
    let list = body_json(&handle_approvals_list(&HashMap::new()));
    let ours = list["pending"].as_array().unwrap().iter().find(|p| p["id"] == id.as_str()).expect("queued").clone();
    assert!(ours["sourceCheck"].as_str().unwrap_or("").contains("different domain"), "{ours}");
    let lines = k2_core::auth_audit::read_all().expect("audit log");
    let line = lines
        .iter()
        .find(|l| l.to_string().contains("mail.forward_message") && l.to_string().contains(&id))
        .unwrap_or_else(|| panic!("no audit line for {id}: {lines:?}"));
    let text = line.to_string();
    assert!(text.contains("dest@else.example") && text.contains("source_sender="), "{text}");
    assert!(!text.contains("Please see the quote"), "never a body: {text}");
    send::deny(&DbOutboundStore::default(), &id, "owner", "cleanup", now_secs()).expect("deny");
    done(&path, &project_id, &domain);
}

/// Once a domain has gone, its mailboxes' waiting mail ends and its files go.
#[test]
fn removing_a_domain_ends_its_waiting_mail() {
    let _mail = crate::mail::mail_server_test_lock();
    let (path, project_id, domain) = seed_files_sender("rmdom", "approval");
    {
        // `seed_address` doesn't tie the address to its domain's row.
        let db = k2_core::db::shared();
        let conn = db.lock();
        let tied = conn
            .execute(
                "UPDATE mail_addresses SET domain_id = (SELECT id FROM mail_domains WHERE domain = ?1) \
                     WHERE address = ?2",
                rusqlite::params![domain, format!("bot@{domain}")],
            )
            .expect("tie the address to its domain");
        assert_eq!(tied, 1);
    }
    let resp = handle_send(&send_body(&path, serde_json::json!({})));
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    let id = body_json(&resp)["id"].as_str().expect("id").to_string();
    let store = DbOutboundStore::default();
    assert!(matches!(
        crate::mail::domains::remove_domain(None, &domain, false, false),
        Err(crate::mail::domains::OpError::ConfirmRequired(_))
    ));
    assert_eq!(store.load(&id).unwrap().unwrap().status, "pending", "not confirmed, not removed");
    crate::mail::domains::remove_domain(None, &domain, true, false).expect("removed");
    let row = store.load(&id).unwrap().unwrap();
    assert_eq!(row.status, "denied");
    assert!(row.note.as_deref().unwrap_or("").contains("mailbox was removed"), "{:?}", row.note);
    done(&path, &project_id, &domain);
}

/// A linked inbox's remove that works ends its waiting mail.
#[test]
fn removing_a_linked_inbox_ends_its_waiting_mail() {
    let (name, path) = unique("rmlink");
    let project_id = insert_project(&name, &path);
    let linked = format!("me-{}@linked.example", &project_id[..8]);
    let row = seed_linked(&project_id, &linked, "send");
    FileSecretStore::default().store_exact(&external::vault_key(&row), "app-pw").expect("vault");
    set_gating(&path, "approval");
    let resp = handle_send(&send_body(&path, serde_json::json!({ "from": linked })));
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    let id = body_json(&resp)["id"].as_str().expect("id").to_string();
    let body = serde_json::json!({ "address": linked }).to_string();
    let r = crate::mail::routes_external::handle_external_remove(body.as_bytes());
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(DbOutboundStore::default().load(&id).unwrap().unwrap().status, "denied");
    cleanup_linked(&project_id);
    cleanup(&project_id, None);
}

/// A linked inbox's remove that fails leaves the waiting mail alone.
#[test]
fn removal_of_an_address_purges_only_after_it_worked() {
    let (path, project_id, domain) = seed_files_sender("rmfail", "approval");
    let resp = handle_send(&send_body(&path, serde_json::json!({})));
    let id = body_json(&resp)["id"].as_str().expect("id").to_string();
    let body = serde_json::json!({ "address": format!("bot@{domain}") }).to_string();
    let r = crate::mail::routes_external::handle_external_remove(body.as_bytes());
    assert_ne!(r.status, "200 OK", "{}", r.body);
    let store = DbOutboundStore::default();
    assert_eq!(store.load(&id).unwrap().unwrap().status, "pending", "nothing denied: {}", r.body);
    send::deny(&store, &id, "owner", "cleanup", now_secs()).expect("deny");
    done(&path, &project_id, &domain);
}
