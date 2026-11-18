use super::*;
use crate::mail::jmap::MailAddr;
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn hide_links_masks_one_time_tokens_only() {
    assert_eq!(
        hide_links("Set your password: https://d.example/set#k2inv_ab12CD34 now. k2inv_x9"),
        "Set your password: https://d.example/set#k2inv_[hidden] now. k2inv_[hidden]"
    );
    assert_eq!(hide_links("no link here"), "no link here");
}

#[test]
fn an_ephemeral_message_round_trips_and_a_plain_one_stays_lean() {
    let mut m = msg();
    m.ephemeral = true;
    let s = serde_json::to_string(&m).unwrap();
    assert!(s.contains("\"ephemeral\":true"), "{s}");
    assert!(serde_json::from_str::<OutboundMessage>(&s).unwrap().ephemeral);
    let plain = serde_json::to_string(&msg()).unwrap();
    assert!(!plain.contains("ephemeral"), "old body files stay byte-identical: {plain}");
    assert!(!serde_json::from_str::<OutboundMessage>(&plain).unwrap().ephemeral);
}

fn msg() -> OutboundMessage {
    OutboundMessage {
        from_name: Some("Research Bot".to_string()),
        from: "bot@acme.dev".to_string(),
        to: vec!["human@example.com".to_string()],
        cc: vec![],
        bcc: vec![],
        policy_bcc: vec![],
        reply_to: Vec::new(),
        subject: "Weekly digest".to_string(),
        text_body: "All green.".to_string(),
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

/// Hosted send/reply: K2 hands Stalwart the body VERBATIM in
/// `bodyValues`, and Stalwart's own composer (mail-builder 1.0, the
/// version Stalwart 0.16.x pins) encodes it without a hard wrap —
/// the text part decodes to the input line for line, wire lines ≤ 998.
#[test]
fn hosted_send_and_reply_keep_paragraphs_exactly() {
    let body = crate::mail::external::tests::fidelity_body();
    for reply in [false, true] {
        let mut m = msg();
        m.text_body = body.clone();
        if reply {
            m.subject = "Re: Weekly digest".to_string();
            m.in_reply_to = Some("<src@example.com>".to_string());
            m.references = Some("<src@example.com>".to_string());
        }
        let v = submission_email_json(&m);
        let value = v["bodyValues"]["k2body"]["value"].as_str().expect("body value");
        assert_eq!(value, body, "the hosted body reaches Stalwart verbatim");
        let wire = mail_builder::MessageBuilder::new()
            .from(("Research Bot", "bot@acme.dev"))
            .to("human@example.com")
            .subject(m.subject.as_str())
            .text_body(value)
            .write_to_vec()
            .expect("compose");
        crate::mail::external::tests::assert_body_fidelity(&wire, &body);
    }
}

/// Recording fake store: counts inserts, canned failure switches.
#[derive(Default)]
struct FakeStore {
    fail_insert: bool,
    fail_count: bool,
    counts: (u32, u32), // (hour, day)
    inserts: Mutex<Vec<(String, Option<String>, Option<i64>)>>, // (status, decided_by, send_after)
    insert_attachments: Mutex<Vec<Vec<String>>>,    // attachment_names per insert
    transitions: Mutex<Vec<(String, String, String)>>,
    transition_result: Option<bool>, // None = Err
    row: Option<MailOutbound>,
    notes: Mutex<Vec<String>>,
}

impl OutboundStore for FakeStore {
    fn insert(&self, new: &NewOutbound) -> Result<String, String> {
        if self.fail_insert {
            return Err("db unreachable".to_string());
        }
        self.inserts.lock().unwrap().push((
            new.status.to_string(),
            new.decided_by.map(String::from),
            new.send_after,
        ));
        self.insert_attachments
            .lock()
            .unwrap()
            .push(new.attachment_names.to_vec());
        Ok("out_fake00000001".to_string())
    }
    fn load(&self, _id: &str) -> Result<Option<MailOutbound>, String> {
        Ok(self.row.clone())
    }
    fn count_recent(&self, _from: &str, since: i64) -> Result<u32, String> {
        if self.fail_count {
            return Err("db unreachable".to_string());
        }
        // The hourly window has the larger `since`.
        Ok(if since >= 1_000_000 - 3_600 { self.counts.0 } else { self.counts.1 })
    }
    fn transition(
        &self,
        id: &str,
        from: &str,
        to: &str,
        _decided_by: Option<&str>,
        _note: Option<&str>,
        _now: i64,
    ) -> Result<bool, String> {
        match self.transition_result {
            None => Err("db unreachable".to_string()),
            Some(ok) => {
                self.transitions
                    .lock()
                    .unwrap()
                    .push((id.to_string(), from.to_string(), to.to_string()));
                Ok(ok)
            }
        }
    }
    fn load_message(&self, _id: &str) -> Result<OutboundMessage, String> {
        Ok(msg())
    }
    fn append_note(&self, _id: &str, extra: &str, _now: i64) -> Result<(), String> {
        self.notes.lock().unwrap().push(extra.to_string());
        Ok(())
    }
}

/// Recording fake backend — the "did anything try to leave the
/// box?" probe every fail-closed test asserts against.
#[derive(Default)]
struct FakeSubmit {
    calls: AtomicUsize,
    fail: bool,
}

impl SubmitBackend for FakeSubmit {
    fn submit(&self, _account_id: &str, _msg: &OutboundMessage) -> Result<(), String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            Err("connection refused".to_string())
        } else {
            Ok(())
        }
    }
}

fn req(m: &OutboundMessage) -> OutboundRequest<'_> {
    OutboundRequest {
        project_id: "P1",
        agent_name: "bot",
        account_id: "acc-1",
        message: m,
        send_after: None,
    }
}

fn req_at(m: &OutboundMessage, send_after: i64) -> OutboundRequest<'_> {
    OutboundRequest {
        project_id: "P1",
        agent_name: "bot",
        account_id: "acc-1",
        message: m,
        send_after: Some(send_after),
    }
}

// ── the gate (fail-closed, pre-mortem #11) ──

#[test]
fn gate_reads_fail_closed_on_error_and_ambiguity() {
    assert_eq!(effective_gate(Ok("off".into())).unwrap(), Gate::Off);
    assert_eq!(effective_gate(Ok("approval".into())).unwrap(), Gate::Approval);
    assert_eq!(effective_gate(Ok("on".into())).unwrap(), Gate::On);
    // A FAILED settings read is an error, never a send.
    let err = effective_gate(Err("db locked".into())).expect_err("must refuse");
    assert!(matches!(&err, SendError::Engine(h) if h.contains("fail-closed")), "{err:?}");
    // An AMBIGUOUS stored value is an error too.
    let err = effective_gate(Ok("banana".into())).expect_err("must refuse");
    assert!(matches!(&err, SendError::Engine(h) if h.contains("banana")), "{err:?}");
}

/// E4: linked/BYO send — off blocks, on allows, and since MCS2's
/// linked approval hand-off approval queues too (approve resolves the
/// linked provider).
#[test]
fn linked_send_gate_off_blocks_on_and_approval_allow() {
    assert!(allow_linked_send(Gate::On).is_ok());
    assert!(allow_linked_send(Gate::Approval).is_ok(), "MCS2: linked approval queues");

    let err = allow_linked_send(Gate::Off).expect_err("off must block");
    match err {
        SendError::Gated(h) => {
            assert!(h.contains("disabled"), "{h}");
            // Names controls that exist on every OS — never the
            // hosted-only Email page (Appa/Baden, macOS 0.45.4).
            assert!(h.contains("`k2 mail sending <workspace> on`"), "{h}");
            assert!(h.contains("Workspaces / Agents → <workspace> → Permissions → Email sending"), "{h}");
            assert!(!h.contains("Email Hosting → Domains → Sending"), "{h}");
        }
        other => panic!("expected Gated, got {other:?}"),
    }
}

#[test]
fn off_mode_never_writes_and_never_submits() {
    let store = FakeStore { transition_result: Some(true), ..Default::default() };
    let submit = FakeSubmit::default();
    let m = msg();
    let err = gate_and_dispatch(&store, Some(&submit), Gate::Off, &req(&m), 1_000_000)
        .expect_err("gated");
    assert!(
        matches!(&err, SendError::Gated(h) if h.contains("`k2 mail sending <workspace> on`")),
        "{err:?}"
    );
    assert!(store.inserts.lock().unwrap().is_empty(), "no audit row in off mode");
    assert_eq!(submit.calls.load(Ordering::SeqCst), 0, "NOTHING submits");
}

#[test]
fn approval_mode_queues_pending_and_never_touches_the_backend() {
    let store = FakeStore { transition_result: Some(true), ..Default::default() };
    let m = msg();
    // backend deliberately None: approval mode must not need one.
    let out = gate_and_dispatch(&store, None, Gate::Approval, &req(&m), 1_000_000)
        .expect("queued");
    assert!(matches!(out, SendOutcome::Queued { .. }), "{out:?}");
    let inserts = store.inserts.lock().unwrap();
    assert_eq!(inserts.as_slice(), &[("pending".to_string(), None, None)]);
}

/// #31.5 follow-up: a linked send threads its attachment filenames
/// (basenames only) into the audit row; a send with none records an
/// empty list (→ NULL `attachments_ref`).
#[test]
fn record_linked_submitted_threads_attachment_filenames() {
    let store = FakeStore { transition_result: Some(true), ..Default::default() };
    let m = msg();
    let names = vec!["invoice.pdf".to_string(), "photo.jpg".to_string()];
    record_linked_submitted(&store, "P1", "bot", &m, &names, 1_000_000)
        .expect("recorded");
    assert_eq!(
        store.insert_attachments.lock().unwrap().as_slice(),
        &[names.clone()],
        "the two basenames rode the insert"
    );
    // No bytes / no paths — the recorded names are exactly the basenames.
    for n in store.insert_attachments.lock().unwrap().iter().flatten() {
        assert!(!n.contains('/'), "no path separators in a recorded name: {n}");
    }

    // Zero attachments → an empty list (the store turns that into a
    // NULL column, verified in the DB-backed route test).
    let store2 = FakeStore { transition_result: Some(true), ..Default::default() };
    record_linked_submitted(&store2, "P1", "bot", &m, &[], 1_000_000).expect("recorded");
    assert_eq!(
        store2.insert_attachments.lock().unwrap().as_slice(),
        &[Vec::<String>::new()],
        "no attachments → empty list"
    );
}

/// The outbox read surfaces a `{count, names}` summary when the row
/// carries attachment filenames, and omits it entirely otherwise.
#[test]
fn outbound_json_surfaces_attachments_summary() {
    let base = MailOutbound {
        id: "out_x".into(),
        owner_project_id: "P1".into(),
        agent_name: "bot".into(),
        from_address: "bot@acme.dev".into(),
        to_json: r#"["human@example.com"]"#.into(),
        cc_json: None,
        subject: "Files".into(),
        body_ref: Some("/tmp/out_x.json".into()),
        attachments_ref: Some(r#"["invoice.pdf","photo.jpg"]"#.into()),
        status: "sent".into(),
        decided_by: None,
        note: None,
        created_at: 1,
        updated_at: 1,
        decided_at: None,
        sent_at: Some(1),
        send_after: None,
        bcc_json: None,
        policy_bcc_json: None,
        reply_to_json: None,
        body_sha256: None,
        forward_of_json: None,
        source_check: None,
    };
    let v = outbound_json(&base);
    assert_eq!(v["attachments"]["count"], 2);
    assert_eq!(v["attachments"]["names"][0], "invoice.pdf");
    assert_eq!(v["attachments"]["names"][1], "photo.jpg");

    // No attachments → no key at all.
    let plain = MailOutbound { attachments_ref: None, ..base };
    let v = outbound_json(&plain);
    assert!(v.get("attachments").is_none(), "omitted when none: {v}");
}

/// PRE-MORTEM #11 (BINDING): the audit row write FAILS → nothing is
/// handed to Stalwart. No row, no send.
#[test]
fn failed_audit_row_insert_never_submits() {
    let store = FakeStore {
        fail_insert: true,
        transition_result: Some(true),
        ..Default::default()
    };
    let submit = FakeSubmit::default();
    let m = msg();
    let err = gate_and_dispatch(&store, Some(&submit), Gate::On, &req(&m), 1_000_000)
        .expect_err("must refuse");
    assert!(matches!(err, SendError::Engine(_)));
    assert_eq!(
        submit.calls.load(Ordering::SeqCst),
        0,
        "no row, no send — the fail-closed invariant"
    );
    // Approval mode refuses on the same broken store (queue table
    // unreachable NEVER bypasses approval — pre-mortem #11).
    let err = gate_and_dispatch(&store, None, Gate::Approval, &req(&m), 1_000_000)
        .expect_err("must refuse");
    assert!(matches!(err, SendError::Engine(_)));
}

/// FAIL-CLOSED: an unreadable rate counter refuses the send before
/// any row exists and before any submit.
#[test]
fn unreadable_rate_counter_refuses_send() {
    let store = FakeStore {
        fail_count: true,
        transition_result: Some(true),
        ..Default::default()
    };
    let submit = FakeSubmit::default();
    let m = msg();
    let err = gate_and_dispatch(&store, Some(&submit), Gate::On, &req(&m), 1_000_000)
        .expect_err("must refuse");
    assert!(matches!(&err, SendError::Engine(h) if h.contains("fail-closed")), "{err:?}");
    assert!(store.inserts.lock().unwrap().is_empty());
    assert_eq!(submit.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn on_mode_audits_then_submits_then_marks_sent() {
    let store = FakeStore { transition_result: Some(true), ..Default::default() };
    let submit = FakeSubmit::default();
    let m = msg();
    let out = gate_and_dispatch(&store, Some(&submit), Gate::On, &req(&m), 1_000_000)
        .expect("submitted");
    assert!(matches!(out, SendOutcome::Submitted { .. }), "{out:?}");
    assert_eq!(submit.calls.load(Ordering::SeqCst), 1);
    // Row inserted as policy-approved BEFORE the submit; then
    // approved → sent.
    assert_eq!(
        store.inserts.lock().unwrap().as_slice(),
        &[("approved".to_string(), Some("policy:agent-send-on".to_string()), None)]
    );
    assert_eq!(
        store.transitions.lock().unwrap().as_slice(),
        &[("out_fake00000001".into(), "approved".into(), "sent".into())]
    );
    // On mode WITHOUT a backend refuses (fail-closed), no row.
    let store2 = FakeStore { transition_result: Some(true), ..Default::default() };
    let err = gate_and_dispatch(&store2, None, Gate::On, &req(&m), 1_000_000)
        .expect_err("must refuse");
    assert!(matches!(err, SendError::Engine(_)));
    assert!(store2.inserts.lock().unwrap().is_empty());
}

#[test]
fn on_mode_submit_failure_marks_failed_with_note() {
    let store = FakeStore { transition_result: Some(true), ..Default::default() };
    let submit = FakeSubmit { fail: true, ..Default::default() };
    let m = msg();
    let out = gate_and_dispatch(&store, Some(&submit), Gate::On, &req(&m), 1_000_000)
        .expect("audited outcome");
    match out {
        SendOutcome::SubmitFailed { error, .. } => {
            assert!(error.contains("connection refused"))
        }
        other => panic!("expected SubmitFailed, got {other:?}"),
    }
    assert_eq!(
        store.transitions.lock().unwrap().as_slice(),
        &[("out_fake00000001".into(), "approved".into(), "failed".into())]
    );
    assert!(store.notes.lock().unwrap()[0].contains("submit failed"));
}

// ── rate limits ──

#[test]
fn hourly_and_daily_caps_produce_distinct_errors() {
    let m = msg();
    let submit = FakeSubmit::default();
    // Hourly cap.
    let store = FakeStore {
        counts: (20, 20),
        transition_result: Some(true),
        ..Default::default()
    };
    let err = gate_and_dispatch(&store, Some(&submit), Gate::On, &req(&m), 1_000_000)
        .expect_err("capped");
    assert!(
        matches!(&err, SendError::RateLimited { window: "hour", .. }),
        "{err:?}"
    );
    // Daily cap (hour under, day over).
    let store = FakeStore {
        counts: (5, 100),
        transition_result: Some(true),
        ..Default::default()
    };
    let err = gate_and_dispatch(&store, Some(&submit), Gate::On, &req(&m), 1_000_000)
        .expect_err("capped");
    assert!(
        matches!(&err, SendError::RateLimited { window: "day", .. }),
        "{err:?}"
    );
    assert_eq!(submit.calls.load(Ordering::SeqCst), 0, "caps precede any submit");
    assert!(store.inserts.lock().unwrap().is_empty(), "caps precede the audit row");
}

// ── approve / deny (atomic + fail-closed) ──

fn pending_row(id: &str) -> MailOutbound {
    MailOutbound {
        id: id.to_string(),
        owner_project_id: "P1".to_string(),
        agent_name: "bot".to_string(),
        from_address: "bot@acme.dev".to_string(),
        to_json: r#"["human@example.com"]"#.to_string(),
        cc_json: None,
        subject: "Weekly digest".to_string(),
        body_ref: Some("/tmp/x.json".to_string()),
        attachments_ref: None,
        status: "pending".to_string(),
        decided_by: None,
        note: None,
        created_at: 1_000_000,
        updated_at: 1_000_000,
        decided_at: None,
        sent_at: None,
        send_after: None,
        bcc_json: None,
        policy_bcc_json: None,
        reply_to_json: None,
        body_sha256: None,
        forward_of_json: None,
        source_check: None,
    }
}

#[test]
fn approve_submits_only_on_the_atomic_pending_transition() {
    let submit = FakeSubmit::default();
    let mut acct = |_from: &str| Ok("acc-1".to_string());

    // Happy path.
    let store = FakeStore {
        row: Some(pending_row("out_a")),
        transition_result: Some(true),
        ..Default::default()
    };
    let out = approve_and_submit(&store, Ok(&submit), &mut acct, "out_a", "owner", None, 2_000)
        .expect("approved");
    assert_eq!(out, ApproveOutcome::Submitted);
    assert_eq!(submit.calls.load(Ordering::SeqCst), 1);

    // AMBIGUOUS/decided row: transition doesn't fire → NO submit
    // (pre-mortem #11 — an unknown approval state never sends).
    let submit = FakeSubmit::default();
    let mut decided = pending_row("out_b");
    decided.status = "denied".to_string();
    let store = FakeStore {
        row: Some(decided),
        transition_result: Some(false),
        ..Default::default()
    };
    let err = approve_and_submit(&store, Ok(&submit), &mut acct, "out_b", "owner", None, 2_000)
        .expect_err("conflict");
    assert!(matches!(&err, SendError::Conflict(h) if h.contains("rejected")), "{err:?}");
    assert_eq!(submit.calls.load(Ordering::SeqCst), 0, "no submit on a decided row");

    // TRANSITION ERROR (db died mid-flow): refuse, NO submit.
    let submit = FakeSubmit::default();
    let store = FakeStore {
        row: Some(pending_row("out_c")),
        transition_result: None,
        ..Default::default()
    };
    let err = approve_and_submit(&store, Ok(&submit), &mut acct, "out_c", "owner", None, 2_000)
        .expect_err("engine");
    assert!(matches!(&err, SendError::Engine(h) if h.contains("nothing sent")), "{err:?}");
    assert_eq!(submit.calls.load(Ordering::SeqCst), 0, "db failure never fails open");

    // Unknown id: masked not_found, no submit.
    let submit = FakeSubmit::default();
    let store = FakeStore { row: None, transition_result: Some(true), ..Default::default() };
    let err = approve_and_submit(&store, Ok(&submit), &mut acct, "out_d", "owner", None, 2_000)
        .expect_err("not found");
    assert!(matches!(err, SendError::NotFound(_)));
    assert_eq!(submit.calls.load(Ordering::SeqCst), 0);

    // Engine unavailable: row flips to failed, nothing dials out.
    let store = FakeStore {
        row: Some(pending_row("out_e")),
        transition_result: Some(true),
        ..Default::default()
    };
    let out = approve_and_submit(
        &store,
        Err("not installed".to_string()),
        &mut acct,
        "out_e",
        "owner",
        None,
        2_000,
    )
    .expect("failed outcome");
    assert!(matches!(out, ApproveOutcome::FailedToSubmit(_)));
    assert!(store
        .transitions
        .lock()
        .unwrap()
        .iter()
        .any(|(_, f, t)| f == "approved" && t == "failed"));
}

#[test]
fn deny_is_atomic_and_conflicts_loudly() {
    let store = FakeStore {
        row: Some(pending_row("out_a")),
        transition_result: Some(true),
        ..Default::default()
    };
    deny(&store, "out_a", "owner", "not now", 2_000).expect("denied");
    let store = FakeStore { row: None, transition_result: Some(true), ..Default::default() };
    assert!(matches!(
        deny(&store, "out_x", "owner", "n", 2_000),
        Err(SendError::NotFound(_))
    ));
    let mut sent = pending_row("out_s");
    sent.status = "sent".to_string();
    let store = FakeStore {
        row: Some(sent),
        transition_result: Some(false),
        ..Default::default()
    };
    assert!(matches!(
        deny(&store, "out_s", "owner", "n", 2_000),
        Err(SendError::Conflict(_))
    ));
}

// ── recipients / sizes / message shape ──

#[test]
fn recipients_normalize_punycode_and_dedupe() {
    let got = normalize_recipients(&[
        "Human@Example.COM ".to_string(),
        "human@example.com".to_string(),
        "kollege@münchen.example".to_string(),
    ])
    .expect("valid");
    assert_eq!(
        got,
        vec![
            "human@example.com".to_string(),
            "kollege@xn--mnchen-3ya.example".to_string(),
        ]
    );
    for bad in ["not-an-address", "@nolocal.example", "x@"] {
        assert!(
            matches!(
                normalize_recipients(&[bad.to_string()]),
                Err(SendError::Usage(_))
            ),
            "{bad}"
        );
    }
}

#[test]
fn submission_email_json_carries_threading_only_when_set() {
    let mut m = msg();
    let v = submission_email_json(&m);
    assert_eq!(v["from"][0]["email"], "bot@acme.dev");
    assert_eq!(v["from"][0]["name"], "Research Bot");
    assert_eq!(v["to"][0]["email"], "human@example.com");
    assert_eq!(v["bodyValues"]["k2body"]["value"], "All green.");
    assert_eq!(v["textBody"][0]["partId"], "k2body");
    assert!(v.get("cc").is_none());
    assert!(v.get("header:In-Reply-To:asText").is_none());

    m.cc = vec!["boss@example.com".to_string()];
    m.in_reply_to = Some("<orig@github.com>".to_string());
    m.references = Some("<r1@x> <orig@github.com>".to_string());
    let v = submission_email_json(&m);
    assert_eq!(v["cc"][0]["email"], "boss@example.com");
    assert_eq!(v["header:In-Reply-To:asText"], "<orig@github.com>");
    assert_eq!(v["header:References:asText"], "<r1@x> <orig@github.com>");
    assert_eq!(m.all_recipients().len(), 2);
}

// ── From display name (`k2 mail send --from-name`) + Message-ID ──

/// Names an agent may pick: kept verbatim (trimmed). Specials that
/// matter in an address header (`"`, `,`, `<`, `>`, `\`, `;`, `:`)
/// are allowed — the mail server's composer quotes them (see the
/// Stalwart contract test below).
#[test]
fn from_name_accepts_real_names_verbatim() {
    for (raw, want) in [
        ("Laura", "Laura"),
        ("  Laura  ", "Laura"),
        ("Smith, John", "Smith, John"),
        (r#"Laura "LJ" Jones"#, r#"Laura "LJ" Jones"#),
        ("Laura <Sales>", "Laura <Sales>"),
        (r"Back\slash; colon:", r"Back\slash; colon:"),
        ("Zoë Ünal", "Zoë Ünal"),
        ("李雷", "李雷"),
        // ZWJ emoji sequence (woman technologist) — ZWJ stays allowed.
        ("Cassi \u{1F469}\u{200D}\u{1F4BB}", "Cassi \u{1F469}\u{200D}\u{1F4BB}"),
    ] {
        assert_eq!(validate_from_name(raw).expect(raw), want, "{raw:?}");
    }
}

fn usage_hint(r: Result<String, SendError>) -> String {
    match r {
        Err(SendError::Usage(h)) => h,
        other => panic!("expected a usage refusal, got {other:?}"),
    }
}

/// CR/LF (header injection) and every other control or invisible
/// formatting character are refused — on the RAW value, so a trailing
/// newline is refused, not trimmed away.
#[test]
fn from_name_refuses_cr_lf_and_control_characters() {
    for bad in [
        "Laura\r\nBcc: evil@example.com",
        "Laura\nX-Injected: 1",
        "Laura\r",
        "Laura\n",
        "\r\nLaura",
        "La\tura",
        "La\u{0}ura",
        "La\u{7F}ura",
        "La\u{85}ura",
        "La\u{2028}ura",
        "\u{202E}arual",
        "La\u{200B}ura",
        "\u{FEFF}Laura",
    ] {
        let hint = usage_hint(validate_from_name(bad));
        assert!(hint.contains("control or invisible"), "{bad:?} → {hint}");
    }
}

#[test]
fn from_name_refuses_over_long_names_at_the_character_cap() {
    let at_cap = "a".repeat(MAX_FROM_NAME_CHARS);
    assert_eq!(validate_from_name(&at_cap).expect("128 chars is allowed"), at_cap);
    // The cap counts characters, not bytes: 128 multi-byte chars pass.
    let wide = "é".repeat(MAX_FROM_NAME_CHARS);
    assert_eq!(validate_from_name(&wide).expect("128 wide chars"), wide);

    let over = "a".repeat(MAX_FROM_NAME_CHARS + 1);
    let hint = usage_hint(validate_from_name(&over));
    assert!(hint.contains("too long (129 characters)"), "{hint}");
    assert!(hint.contains("max 128"), "{hint}");
}

/// Decision: a display name that looks like an address is refused —
/// `"ceo@bank.com" <bot@acme.dev>` shows as ceo@bank.com in clients
/// that hide the address. Fullwidth/small `@` lookalikes too.
#[test]
fn from_name_refuses_at_signs_and_empty_names() {
    for bad in ["ceo@bank.com", "Laura @ Acme", "ceo\u{FF20}bank.com", "ceo\u{FE6B}bank.com"] {
        let hint = usage_hint(validate_from_name(bad));
        assert!(hint.contains("can't contain '@'"), "{bad:?} → {hint}");
    }
    for empty in ["", "   "] {
        let hint = usage_hint(validate_from_name(empty));
        assert!(hint.contains("fromName is empty"), "{empty:?} → {hint}");
    }
}

/// The name reaches the JMAP Email object as a STRUCTURED value (never
/// pre-quoted or pre-encoded by us), next to the unchanged address.
#[test]
fn submission_email_json_carries_the_chosen_from_name_unquoted() {
    let mut m = msg();
    m.from_name = Some(r#"Smith, "LJ" <Laura>"#.to_string());
    let v = submission_email_json(&m);
    let from = v["from"].as_array().expect("from is an array");
    assert_eq!(from.len(), 1, "exactly one From mailbox: {v}");
    assert_eq!(from[0]["name"], r#"Smith, "LJ" <Laura>"#);
    assert_eq!(from[0]["email"], "bot@acme.dev");
}

/// Message-ID uses the SENDING address's domain (not the box's
/// hostname, which Stalwart's composer would otherwise stamp), in the
/// JMAP bracket-less form, fresh per message.
#[test]
fn submission_email_json_stamps_a_message_id_on_the_sender_domain() {
    let m = msg();
    let a = submission_email_json(&m);
    let b = submission_email_json(&m);
    let ids = a["messageId"].as_array().expect("messageId is an array");
    assert_eq!(ids.len(), 1, "{a}");
    let id = ids[0].as_str().expect("string id");
    let (local, domain) = id.split_once('@').expect("id has an @");
    assert_eq!(domain, "acme.dev", "{id}");
    assert_eq!(local.len(), 32, "uuid simple form: {id}");
    assert!(local.chars().all(|c| c.is_ascii_hexdigit()), "{id}");
    assert!(!id.contains('<') && !id.contains('>'), "JMAP form has no brackets: {id}");
    assert_ne!(a["messageId"], b["messageId"], "fresh per message");
    let other = new_message_id_for("x@Sub.Example.org");
    assert_eq!(other.split_once('@').expect("@").1, "Sub.Example.org");
}

/// CONTRACT with the hosted path: Stalwart composes the From header
/// from our JMAP `from[0]` with its own builder (mail-builder 1.0, the
/// version Stalwart 0.16 pins). Render every allowed kind of name the
/// same way and parse it back: exactly ONE mailbox, the exact name, the
/// unchanged address — a `,`, `"` or `<…>` in the name never creates a
/// second address, and non-ASCII is RFC 2047-encoded on the wire.
#[test]
fn hosted_from_header_contract_one_mailbox_exact_name() {
    let from_addr = "laura@alakazam-labs.com";
    for raw in [
        "Laura",
        "Smith, John",
        r#"Laura "LJ" Jones"#,
        "Laura <ceo-office>, Sales",
        r"Back\slash",
        "Zoë Ünal",
        "李雷",
    ] {
        let name = validate_from_name(raw).expect(raw);
        let mut m = msg();
        m.from = from_addr.to_string();
        m.from_name = Some(name.clone());
        let v = submission_email_json(&m);
        let jname = v["from"][0]["name"].as_str().expect("name").to_string();
        let jemail = v["from"][0]["email"].as_str().expect("email").to_string();
        let wire = mail_builder::MessageBuilder::new()
            .from((jname, jemail))
            .to("human@example.com")
            .subject("Digest")
            .text_body("All green.")
            .write_to_string()
            .expect("compose");
        let header_block = wire.split("\r\n\r\n").next().expect("headers").to_string();
        let from_headers = header_block
            .split("\r\n")
            .filter(|l| l.to_ascii_lowercase().starts_with("from:"))
            .count();
        assert_eq!(from_headers, 1, "exactly one From header for {raw:?}:\n{header_block}");
        if !raw.is_ascii() {
            assert!(
                header_block.to_ascii_lowercase().contains("=?utf-8?"),
                "non-ASCII name is RFC 2047-encoded for {raw:?}:\n{header_block}"
            );
            assert!(header_block.is_ascii(), "wire headers stay ASCII:\n{header_block}");
        }
        let parsed = mail_parser::MessageParser::default()
            .parse(wire.as_bytes())
            .expect("parses");
        let list = parsed
            .from()
            .and_then(|a| a.as_list())
            .expect("From is a mailbox list");
        assert_eq!(list.len(), 1, "one mailbox for {raw:?}: {list:?}\n{header_block}");
        assert_eq!(list[0].name(), Some(name.as_str()), "{header_block}");
        assert_eq!(list[0].address(), Some(from_addr), "{header_block}");
    }
}

// ── BCC (agent --bcc + owner always-BCC policy) ──

/// A real BCC: every blind copy is in the SMTP envelope the hosted
/// submit hands Stalwart (`all_recipients` → rcptTo), and NOTHING
/// about it is in the Email object (the headers To/Cc recipients
/// see). Duplicates across lists collapse to one RCPT.
#[test]
fn bcc_rides_the_envelope_and_never_the_email_headers() {
    let mut m = msg();
    m.cc = vec!["cc@example.com".to_string()];
    m.bcc = vec!["boss@shop.example".to_string(), "human@example.com".to_string()];
    m.policy_bcc = vec!["owner@shop.example".to_string()];

    let rcpt = m.all_recipients();
    assert_eq!(
        rcpt,
        vec![
            "human@example.com".to_string(),
            "cc@example.com".to_string(),
            "boss@shop.example".to_string(),
            "owner@shop.example".to_string(),
        ],
        "to + cc + bcc + policy bcc, deduped"
    );

    let v = submission_email_json(&m);
    assert!(v.get("bcc").is_none(), "no bcc field on the Email object: {v}");
    let wire = v.to_string();
    assert!(!wire.to_ascii_lowercase().contains("bcc"), "no Bcc header anywhere: {wire}");
    assert!(!wire.contains("boss@shop.example"), "agent bcc leaked into headers: {wire}");
    assert!(!wire.contains("owner@shop.example"), "policy bcc leaked into headers: {wire}");
    assert_eq!(v["cc"][0]["email"], "cc@example.com");
}

/// The policy stamps over an agent BCC of the same address (recorded
/// once, as policy) and the cap counts the final envelope; when the
/// policy pushes it over, the error names the policy — no recipient
/// is ever dropped to make room.
#[test]
fn policy_bcc_merges_and_the_limit_names_the_policy() {
    let mut m = msg();
    m.bcc = vec!["owner@shop.example".to_string(), "qa@shop.example".to_string()];
    apply_policy_bcc(&mut m, vec!["owner@shop.example".to_string()]);
    assert_eq!(m.bcc, vec!["qa@shop.example".to_string()]);
    assert_eq!(m.policy_bcc, vec!["owner@shop.example".to_string()]);
    check_recipient_limit(&m).expect("3 recipients is under the cap");

    // 9 To + 2 policy = 11 > 10 → refused, naming the policy.
    let mut m = msg();
    m.to = (0..9).map(|i| format!("c{i}@cust.example")).collect();
    apply_policy_bcc(
        &mut m,
        vec!["owner@shop.example".to_string(), "audit@shop.example".to_string()],
    );
    let err = check_recipient_limit(&m).expect_err("policy pushes over the cap");
    let SendError::Usage(hint) = err else { panic!("usage expected: {err:?}") };
    assert!(hint.contains("too many recipients (11)"), "{hint}");
    assert!(hint.contains("always BCCs 2"), "{hint}");
    assert!(hint.contains("owner@shop.example"), "{hint}");
    assert_eq!(m.policy_bcc.len(), 2, "nothing was silently dropped");

    // Without a policy the plain to + cc + bcc wording.
    let mut m = msg();
    m.to = (0..8).map(|i| format!("c{i}@cust.example")).collect();
    m.bcc = (0..3).map(|i| format!("b{i}@cust.example")).collect();
    let err = check_recipient_limit(&m).expect_err("11 > 10");
    let SendError::Usage(hint) = err else { panic!("usage expected: {err:?}") };
    assert!(hint.contains("to + cc + bcc"), "{hint}");
}

/// Body-file round trip: the stored composed message keeps both BCC
/// lists (the approval queue and the scheduler submit exactly this),
/// and a pre-0131 body file without them still parses.
#[test]
fn stored_message_keeps_bcc_lists_and_old_bodies_still_parse() {
    let mut m = msg();
    m.bcc = vec!["boss@shop.example".to_string()];
    m.policy_bcc = vec!["owner@shop.example".to_string()];
    let raw = serde_json::to_string(&m).expect("serialize");
    let back: OutboundMessage = serde_json::from_str(&raw).expect("parse");
    assert_eq!(back.bcc, m.bcc);
    assert_eq!(back.policy_bcc, m.policy_bcc);

    let old = r#"{"from":"bot@acme.dev","to":["a@b.example"],"subject":"s","textBody":"t"}"#;
    let back: OutboundMessage = serde_json::from_str(old).expect("old body parses");
    assert!(back.bcc.is_empty() && back.policy_bcc.is_empty());
}

/// The outbox wire shape reports agent and policy BCCs apart.
#[test]
fn outbound_json_reports_agent_and_policy_bcc_apart() {
    let mut row = pending_row("out_bcc");
    row.bcc_json = Some(r#"["boss@shop.example"]"#.to_string());
    row.policy_bcc_json = Some(r#"["owner@shop.example"]"#.to_string());
    let v = outbound_json(&row);
    assert_eq!(v["bcc"], serde_json::json!(["boss@shop.example"]));
    assert_eq!(v["policyBcc"], serde_json::json!(["owner@shop.example"]));
    let v = outbound_json(&pending_row("out_plain"));
    assert_eq!(v["bcc"], serde_json::json!([]));
    assert_eq!(v["policyBcc"], serde_json::json!([]));
}

// ── reply guardrails ──

fn mx() -> messages::AuthAuthority {
    messages::AuthAuthority::ThisBox(Some("mx".to_string()))
}

fn ctx(from_email: &str, dmarc: &str, refs: Option<&str>) -> ReplyContext {
    ReplyContext {
        from: vec![MailAddr { name: None, email: from_email.to_string() }],
        to: Vec::new(),
        cc: Vec::new(),
        reply_to: Vec::new(),
        subject: "Verify".to_string(),
        thread_id: Some("T1".to_string()),
        message_id: Some("<m1@x>".to_string()),
        references: refs.map(String::from),
        auth_results: vec![format!("mx; spf=pass; dkim=pass; dmarc={dmarc}")],
    }
}

#[test]
fn reply_guardrails_lock_recipient_and_refuse_loops_and_dmarc_fail() {
    // Recipient is LOCKED to the original sender, normalized.
    let c = ctx("Sender@Example.COM", "pass", None);
    let rcpt = check_reply_guardrails(&c, "bot@acme.dev", Gate::On, &mx(), false).expect("allowed");
    assert_eq!(rcpt, "sender@example.com");

    // Reply-to-self refused.
    let c = ctx("bot@acme.dev", "pass", None);
    let err = check_reply_guardrails(&c, "bot@acme.dev", Gate::On, &mx(), false).expect_err("loop");
    assert!(matches!(&err, SendError::Guardrail(h) if h.contains("loop")), "{err:?}");

    // No sender refused.
    let mut c = ctx("x@y.z", "pass", None);
    c.from.clear();
    assert!(matches!(
        check_reply_guardrails(&c, "bot@acme.dev", Gate::On, &mx(), false),
        Err(SendError::Guardrail(_))
    ));

    // >100 references refused (thread-depth loop cap).
    let deep = "<r@x> ".repeat(101);
    let c = ctx("sender@example.com", "pass", Some(deep.trim()));
    let err = check_reply_guardrails(&c, "bot@acme.dev", Gate::On, &mx(), false).expect_err("deep");
    assert!(matches!(&err, SendError::Guardrail(h) if h.contains("References")), "{err:?}");
    // 100 exactly is allowed.
    let ok = "<r@x> ".repeat(100);
    let c = ctx("sender@example.com", "pass", Some(ok.trim()));
    check_reply_guardrails(&c, "bot@acme.dev", Gate::On, &mx(), false).expect("at the cap");

    // DMARC fail: blocked in on-mode, allowed under approval (the
    // human overrides, §8.4).
    let c = ctx("sender@example.com", "fail", None);
    let err = check_reply_guardrails(&c, "bot@acme.dev", Gate::On, &mx(), false).expect_err("dmarc");
    assert!(matches!(&err, SendError::Guardrail(h) if h.contains("DMARC")), "{err:?}");
    check_reply_guardrails(&c, "bot@acme.dev", Gate::Approval, &mx(), false)
        .expect("approval mode lets the human decide");
}

/// The gate reads this box's own verdict and nothing else.
#[test]
fn the_dmarc_gate_only_believes_this_boxes_stamp() {
    let with = |lines: Vec<String>| {
        let mut c = ctx("sender@example.com", "pass", None);
        c.auth_results = lines;
        c
    };
    let host = messages::AuthAuthority::ThisBox(Some("mail.example.test".to_string()));
    // The sender chose smtp.mailfrom; the box's real verdict is `fail`.
    let crafted = with(vec![
        "mail.example.test; spf=none smtp.mailfrom=\"dmarc=pass@evil.example\"; iprev=pass; \
             dmarc=fail (policy not aligned) header.from=gmail.com"
            .to_string(),
    ]);
    let err = check_reply_guardrails(&crafted, "bot@acme.dev", Gate::On, &host, false).expect_err("blocked");
    assert!(matches!(&err, SendError::Guardrail(h) if h.contains("DMARC")), "{err:?}");
    check_reply_guardrails(&crafted, "bot@acme.dev", Gate::Approval, &host, false).expect("a person decides");
    // More than one answer is a failure.
    let two = with(vec!["mail.example.test; dmarc=pass; dmarc=fail".to_string()]);
    assert!(check_reply_guardrails(&two, "bot@acme.dev", Gate::On, &host, false).is_err());
    // A failure below this box's stamp doesn't count; a header that came
    // in with the message isn't this box's, whatever it says.
    let forged_pass_below = with(vec![
        "mail.example.test; dmarc=fail".to_string(),
        "mx.evil.example; dmarc=pass".to_string(),
    ]);
    assert!(check_reply_guardrails(&forged_pass_below, "bot@acme.dev", Gate::On, &host, false).is_err());
    let v = messages::parse_auth_results(
        &["mx.evil.example; dmarc=pass".to_string()],
        &host,
    );
    assert!(v["dmarc"].is_null(), "not reported as passing: {v}");
    // Real stamps: two From headers read `none` with an empty header.from,
    // and a lookup that broke reads neither pass nor fail.
    let mut two = with(vec!["mail.example.test; dmarc=none header.from= policy.dmarc=none".to_string()]);
    two.from = vec![MailAddr { name: None, email: "ceo@paypal.com".to_string() }];
    let err = check_reply_guardrails(&two, "bot@acme.dev", Gate::On, &host, false).expect_err("blocked");
    assert!(matches!(&err, SendError::Guardrail(h) if h.contains("DMARC")), "{err:?}");
    check_reply_guardrails(&two, "bot@acme.dev", Gate::Approval, &host, false).expect("a person decides");
    let broken = with(vec!["mail.example.test; dmarc=permerror (x) header.from=example.com".to_string()]);
    assert!(check_reply_guardrails(&broken, "bot@acme.dev", Gate::On, &host, false).is_err());
    // No stamp: a mailbox on this box is fine, anyone else isn't.
    let bare = with(Vec::new());
    assert!(check_reply_guardrails(&bare, "bot@acme.dev", Gate::On, &host, false).is_err());
    check_reply_guardrails(&bare, "bot@acme.dev", Gate::On, &host, true).expect("between mailboxes");
}

#[test]
fn reply_subject_and_references_chain() {
    assert_eq!(reply_subject("Hello"), "Re: Hello");
    assert_eq!(reply_subject("  RE: Hello"), "RE: Hello");
    assert_eq!(reply_subject("re: hello"), "re: hello");
    assert_eq!(count_references(None), 0);
    assert_eq!(count_references(Some("<a@x> <b@x>")), 2);
    assert_eq!(
        build_out_references(Some("<a@x>"), Some("<b@x>")).as_deref(),
        Some("<a@x> <b@x>")
    );
    assert_eq!(build_out_references(None, Some("<b@x>")).as_deref(), Some("<b@x>"));
    assert_eq!(build_out_references(Some("<a@x>"), None).as_deref(), Some("<a@x>"));
    assert_eq!(build_out_references(None, None), None);
}

#[test]
fn thread_reply_cap_is_a_sliding_hour_window() {
    let key = format!("test-thread-{}", uuid::Uuid::new_v4());
    for i in 0..MAX_THREAD_REPLIES_PER_HOUR {
        check_and_record_thread_reply(&key, 1_000_000 + i as i64).expect("under cap");
    }
    let err = check_and_record_thread_reply(&key, 1_000_100).expect_err("capped");
    assert!(matches!(err, SendError::RateLimited { window: "thread", .. }));
    // An hour later the window slid open again.
    check_and_record_thread_reply(&key, 1_000_000 + 3_601).expect("window slid");
}

// ── wire statuses ──

#[test]
fn wire_statuses_match_the_gate_findings() {
    assert_eq!(wire_status("pending"), "pending_approval");
    assert_eq!(wire_status("approved"), "approved");
    assert_eq!(wire_status("denied"), "rejected");
    assert_eq!(wire_status("sent"), "submitted", "never 'delivered' (pre-mortem #9)");
    assert_eq!(wire_status("failed"), "failed");
    assert_eq!(wire_status("garbage"), "unknown");
    assert!(status_note("sent").contains("accepted-for-delivery"));
}

// ── wait_for_decision (fake clock — no real sleeps) ──

#[test]
fn wait_for_decision_returns_decision_or_timeout_on_fake_clock() {
    // Decided on the 3rd poll.
    let mut polls = 0;
    let mut poll = || {
        polls += 1;
        Ok(if polls >= 3 {
            ("denied".to_string(), Some("not now".to_string()))
        } else {
            ("pending".to_string(), None)
        })
    };
    let t = std::cell::Cell::new(0i64);
    let mut now = || t.get();
    let mut sleep = |d: std::time::Duration| t.set(t.get() + d.as_secs() as i64);
    let got = wait_for_decision(&mut poll, 300, 2, &mut now, &mut sleep)
        .expect("ok")
        .expect("decided");
    assert_eq!(got, ("denied".to_string(), Some("not now".to_string())));

    // Never decided → timeout (None), bounded polls.
    let mut polls = 0usize;
    let mut poll = || {
        polls += 1;
        Ok(("pending".to_string(), None))
    };
    let t = std::cell::Cell::new(0i64);
    let mut now = || t.get();
    let mut sleep = |d: std::time::Duration| t.set(t.get() + d.as_secs() as i64);
    let got =
        wait_for_decision(&mut poll, 10, 2, &mut now, &mut sleep).expect("ok");
    assert!(got.is_none(), "timeout → None (message still pending)");
    assert_eq!(polls, 6, "bounded poll count on the fake clock");

    // Poll errors surface loudly.
    let mut poll = || Err("row vanished".to_string());
    let t = std::cell::Cell::new(0i64);
    let mut now = || t.get();
    let mut sleep = |_: std::time::Duration| {};
    let err = wait_for_decision(&mut poll, 10, 2, &mut now, &mut sleep)
        .expect_err("loud");
    assert!(err.contains("vanished"));
}

// ── the production store (shared test DB + temp body dir) ──

fn temp_store() -> (DbOutboundStore, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "k2-mail-outbound-test-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    (DbOutboundStore::at(dir.clone()), dir)
}

fn cleanup_rows(project_id: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let _ = conn.execute(
        "DELETE FROM mail_outbound WHERE owner_project_id = ?1",
        rusqlite::params![project_id],
    );
}

/// Verification review 2026-10-26, item 4: a one-time-link mail never
/// keeps its link at rest. Sent at once: only the link-hidden copy is
/// written. Held for approval: the full body waits (the approver sends
/// it), then is rewritten link-hidden once sent or denied.
#[test]
fn a_one_time_link_mail_keeps_no_link_at_rest() {
    struct Keep(Mutex<Vec<OutboundMessage>>);
    impl SubmitBackend for Keep {
        fn submit(&self, _account_id: &str, msg: &OutboundMessage) -> Result<(), String> {
            self.0.lock().unwrap().push(msg.clone());
            Ok(())
        }
    }
    let (store, dir) = temp_store();
    let project = format!("P-{}", uuid::Uuid::new_v4());
    let link = "https://box.example/cli/skin/password/set#k2inv_abcDEF123";
    let mut m = msg();
    m.from = format!("bot-{}@acme.dev", uuid::Uuid::new_v4().simple());
    m.subject = "Your invite".to_string();
    m.text_body = format!("Set your password: {link}\n");
    m.ephemeral = true;
    let body_of = |id: &str| std::fs::read_to_string(dir.join(format!("{id}.json"))).expect("body file");
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;

    // On: the person gets the link; the file never had it.
    let keep = Keep(Mutex::new(Vec::new()));
    let req = OutboundRequest { project_id: &project, agent_name: "bot", account_id: "acct", message: &m, send_after: None };
    let SendOutcome::Submitted { id } = gate_and_dispatch(&store, Some(&keep), Gate::On, &req, now).expect("send") else {
        panic!("expected a submit")
    };
    assert!(keep.0.lock().unwrap()[0].text_body.contains("k2inv_abcDEF123"), "the mail itself carries the link");
    let at_rest = body_of(&id);
    assert!(!at_rest.contains("k2inv_abcDEF123"), "{at_rest}");
    assert!(at_rest.contains("k2inv_[hidden]"), "{at_rest}");

    // Approval: the waiting body keeps it until the decision, then not.
    for approve in [true, false] {
        let SendOutcome::Queued { id, .. } = gate_and_dispatch(&store, None, Gate::Approval, &req, now).expect("queue") else {
            panic!("expected a queued mail")
        };
        assert!(body_of(&id).contains("k2inv_abcDEF123"), "the approver sends the real link");
        if approve {
            let keep = Keep(Mutex::new(Vec::new()));
            let out = approve_and_submit(&store, Ok(&keep), &mut |_: &str| Ok::<String, String>("acct".to_string()), &id, "owner", None, now).expect("approve");
            assert_eq!(out, ApproveOutcome::Submitted);
            assert!(keep.0.lock().unwrap()[0].text_body.contains("k2inv_abcDEF123"));
        } else {
            deny(&store, &id, "owner", "no", now).expect("deny");
        }
        let at_rest = body_of(&id);
        assert!(!at_rest.contains("k2inv_abcDEF123"), "approve={approve}: {at_rest}");
    }

    // A plain mail is kept as is.
    let mut plain = m.clone();
    plain.ephemeral = false;
    let req = OutboundRequest { project_id: &project, agent_name: "bot", account_id: "acct", message: &plain, send_after: None };
    let keep = Keep(Mutex::new(Vec::new()));
    let SendOutcome::Submitted { id } = gate_and_dispatch(&store, Some(&keep), Gate::On, &req, now).expect("send") else {
        panic!("expected a submit")
    };
    assert!(body_of(&id).contains("k2inv_abcDEF123"), "only one-time-link mails are rewritten");
    cleanup_rows(&project);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn db_store_roundtrip_transitions_counts_and_0600_bodies() {
    let (store, dir) = temp_store();
    let project = format!("P-{}", uuid::Uuid::new_v4());
    // Unique From: rate counting is per address and the DB is the
    // shared test DB — a fixed address would race sibling tests.
    let mut m = msg();
    m.from = format!("bot-{}@acme.dev", uuid::Uuid::new_v4().simple());
    // Real-clock timestamps: auto_deny_expired sweeps the WHOLE table
    // (expired_pending_rows_auto_deny with now=10_000_000, and every
    // send route with now_secs()). A 1970 created_at made this pending
    // row look 7+ days old, so a concurrent sweep flipped it to denied.
    let t0 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_secs() as i64;
    let id = store
        .insert(&NewOutbound {
            owner_project_id: &project,
            agent_name: "bot",
            message: &m,
            status: "pending",
            decided_by: None,
            attachment_names: &[],
            attachment_files: &[],
            send_after: None,
            now: t0,
        })
        .expect("insert");
    assert!(id.starts_with("out_"), "{id}");
    // A no-attachment insert leaves attachments_ref NULL.
    assert!(store.load(&id).unwrap().unwrap().attachments_ref.is_none());

    // The body file is 0600 and rounds the message back.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.join(format!("{id}.json")))
            .expect("body file")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "body file must be owner-only");
    }
    assert_eq!(store.load_message(&id).expect("message"), m);

    // The row loads with the audit fields.
    let row = store.load(&id).expect("load").expect("row");
    assert_eq!(row.status, "pending");
    assert_eq!(row.from_address, m.from);
    assert_eq!(row.subject, "Weekly digest");
    assert!(row.body_ref.as_deref().unwrap().ends_with(".json"));
    assert!(store.load("out_nope").expect("load").is_none());

    // Rate counting respects the window boundary.
    assert_eq!(store.count_recent(&m.from, t0).unwrap(), 1);
    assert_eq!(store.count_recent(&m.from, t0 + 1).unwrap(), 0);
    assert_eq!(store.count_recent("other@acme.dev", 0).unwrap(), 0);

    // Transitions are ATOMIC on the current status.
    assert!(store
        .transition(&id, "pending", "denied", Some("owner"), Some("no"), t0 + 500)
        .unwrap());
    assert!(
        !store
            .transition(&id, "pending", "approved", Some("owner"), None, t0 + 600)
            .unwrap(),
        "a decided row never re-transitions from pending"
    );
    let row = store.load(&id).expect("load").expect("row");
    assert_eq!(row.status, "denied");
    assert_eq!(row.decided_by.as_deref(), Some("owner"));
    assert_eq!(row.note.as_deref(), Some("no"));
    assert_eq!(row.decided_at, Some(t0 + 500));

    store.append_note(&id, "extra line", t0 + 700).expect("note");
    let row = store.load(&id).expect("load").expect("row");
    assert_eq!(row.note.as_deref(), Some("no\nextra line"));

    cleanup_rows(&project);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn expired_pending_rows_auto_deny() {
    let (store, dir) = temp_store();
    let project = format!("P-{}", uuid::Uuid::new_v4());
    let m = msg();
    let now = 10_000_000;
    let old = store
        .insert(&NewOutbound {
            owner_project_id: &project,
            agent_name: "bot",
            message: &m,
            status: "pending",
            decided_by: None,
            attachment_names: &[],
            attachment_files: &[],
            send_after: None,
            now: now - APPROVAL_EXPIRE_SECS - 10,
        })
        .expect("insert old");
    let fresh = store
        .insert(&NewOutbound {
            owner_project_id: &project,
            agent_name: "bot",
            message: &m,
            status: "pending",
            decided_by: None,
            attachment_names: &[],
            attachment_files: &[],
            send_after: None,
            now: now - 100,
        })
        .expect("insert fresh");
    // NOTE: auto_deny_expired sweeps the WHOLE table; other tests'
    // rows are younger than 7 days by construction (now_secs), so
    // only ours can flip here.
    assert!(auto_deny_expired(now) >= 1);
    let old_row = store.load(&old).expect("load").expect("row");
    assert_eq!(old_row.status, "denied");
    assert_eq!(old_row.decided_by.as_deref(), Some("system:expired"));
    assert!(old_row.note.as_deref().unwrap().contains("expired"));
    let fresh_row = store.load(&fresh).expect("load").expect("row");
    assert_eq!(fresh_row.status, "pending", "fresh items stay queued");
    cleanup_rows(&project);
    let _ = std::fs::remove_dir_all(dir);
}


#[test]
fn on_mode_with_future_send_after_inserts_scheduled_not_submitted() {
    let store = FakeStore {
        transition_result: Some(true),
        ..Default::default()
    };
    let submit = FakeSubmit::default();
    let m = msg();
    let now = 1_000_000i64;
    let after = now + 3_600;
    let out = gate_and_dispatch(&store, Some(&submit), Gate::On, &req_at(&m, after), now)
        .expect("scheduled");
    match out {
        SendOutcome::Scheduled { send_after, .. } => assert_eq!(send_after, after),
        other => panic!("expected Scheduled, got {other:?}"),
    }
    assert_eq!(submit.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        store.inserts.lock().unwrap().as_slice(),
        &[(
            "scheduled".to_string(),
            Some("policy:agent-send-on".to_string()),
            Some(after)
        )]
    );
}

#[test]
fn approval_with_send_after_then_approve_schedules() {
    let m = msg();
    let now = 1_000_000i64;
    let after = now + 7_200;
    let store = FakeStore {
        transition_result: Some(true),
        ..Default::default()
    };
    let out = gate_and_dispatch(&store, None, Gate::Approval, &req_at(&m, after), now)
        .expect("queued");
    assert!(matches!(out, SendOutcome::Queued { send_after: Some(t), .. } if t == after));

    let mut pending = pending_row("out_sched");
    pending.send_after = Some(after);
    let store = FakeStore {
        row: Some(pending),
        transition_result: Some(true),
        ..Default::default()
    };
    let submit = FakeSubmit::default();
    let mut acct = |_from: &str| Ok("acc-1".to_string());
    let out = approve_and_submit(&store, Ok(&submit), &mut acct, "out_sched", "owner", None, now + 10)
        .expect("ok");
    assert_eq!(out, ApproveOutcome::Scheduled { send_after: after });
    assert_eq!(submit.calls.load(Ordering::SeqCst), 0);
}

// ── domain send-mode / relay validation (shared DB seeds) ──

struct MapSecrets(HashMap<String, String>, bool /* fail */);
impl SecretStore for MapSecrets {
    fn store(&self, _k: &str, _s: &str) -> Result<String, String> {
        unreachable!()
    }
    fn resolve(&self, sref: &str) -> Result<Option<String>, String> {
        if self.1 {
            return Err("store corrupt".to_string());
        }
        Ok(self.0.get(sref).cloned())
    }
    fn delete(&self, _sref: &str) -> Result<(), String> {
        unreachable!()
    }
}

fn seed_domain(domain: &str, send_mode: &str, relay_config_id: Option<&str>) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO mail_domains (id, domain, send_mode, relay_config_id, status, \
             created_at) VALUES (?1, ?2, ?3, ?4, 'verified', 100)",
        rusqlite::params![id, domain, send_mode, relay_config_id],
    )
    .expect("seed domain");
    id
}

fn seed_relay(
    kind: &str,
    host: Option<&str>,
    port: Option<i64>,
    secret_ref: Option<&str>,
) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO mail_relay_configs (id, kind, host, port, username, secret_ref, \
             tls_kind, created_at) VALUES (?1, ?2, ?3, ?4, 'u', ?5, 'starttls', 100)",
        rusqlite::params![id, kind, host, port, secret_ref],
    )
    .expect("seed relay");
    id
}

fn cleanup_domain(domain: &str, relay_id: Option<&str>) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let _ = conn.execute(
        "DELETE FROM mail_domains WHERE domain = ?1",
        rusqlite::params![domain],
    );
    if let Some(r) = relay_id {
        let _ = conn.execute(
            "DELETE FROM mail_relay_configs WHERE id = ?1",
            rusqlite::params![r],
        );
    }
}

#[test]
fn domain_send_modes_enforce_per_prd() {
    let secrets = MapSecrets(HashMap::new(), false);
    let uniq = uuid::Uuid::new_v4().simple().to_string();

    // Unknown domain → fail-closed engine error.
    let err = check_domain_send_mode(&secrets, &format!("x@ghost-{uniq}.example"))
        .expect_err("refuse");
    assert!(matches!(&err, SendError::Engine(h) if h.contains("fail-closed")), "{err:?}");

    // receive-only → the teaching error.
    let d = format!("ro-{uniq}.example");
    seed_domain(&d, "receive-only", None);
    let err = check_domain_send_mode(&secrets, &format!("bot@{d}")).expect_err("refuse");
    assert!(
        matches!(&err, SendError::SendMode(h) if h.contains("receive-only")
            && h.contains("Settings → Cores → Email Hosting")),
        "{err:?}"
    );
    cleanup_domain(&d, None);

    // direct → allowed.
    let d = format!("dir-{uniq}.example");
    seed_domain(&d, "direct", None);
    check_domain_send_mode(&secrets, &format!("bot@{d}")).expect("direct sends");
    cleanup_domain(&d, None);
}

#[test]
fn relay_mode_validates_config_kind_agnostically_and_fail_closed() {
    let uniq = uuid::Uuid::new_v4().simple().to_string();

    // Relay mode without a config → teaching send_mode error.
    let d = format!("r0-{uniq}.example");
    seed_domain(&d, "relay", None);
    let secrets = MapSecrets(HashMap::new(), false);
    let err = check_domain_send_mode(&secrets, &format!("bot@{d}")).expect_err("refuse");
    assert!(matches!(&err, SendError::SendMode(h) if h.contains("no relay")), "{err:?}");
    cleanup_domain(&d, None);

    // Complete smtp relay with resolvable creds → allowed.
    let d = format!("r1-{uniq}.example");
    let relay = seed_relay("smtp", Some("smtp.relay.example"), Some(587), Some("ref-ok"));
    seed_domain(&d, "relay", Some(&relay));
    let secrets = MapSecrets(
        HashMap::from([("ref-ok".to_string(), "s3cret".to_string())]),
        false,
    );
    check_domain_send_mode(&secrets, &format!("bot@{d}")).expect("relay sends");

    // Missing creds in the store → refuse; a FAILING store →
    // fail-closed engine error.
    let empty = MapSecrets(HashMap::new(), false);
    let err = check_domain_send_mode(&empty, &format!("bot@{d}")).expect_err("refuse");
    assert!(matches!(&err, SendError::SendMode(h) if h.contains("credentials")), "{err:?}");
    let broken = MapSecrets(HashMap::new(), true);
    let err = check_domain_send_mode(&broken, &format!("bot@{d}")).expect_err("refuse");
    assert!(matches!(&err, SendError::Engine(h) if h.contains("fail-closed")), "{err:?}");
    cleanup_domain(&d, Some(&relay));

    // Incomplete smtp config (no host) → refuse.
    let d = format!("r2-{uniq}.example");
    let relay = seed_relay("smtp", None, Some(587), Some("ref-ok"));
    seed_domain(&d, "relay", Some(&relay));
    let secrets = MapSecrets(
        HashMap::from([("ref-ok".to_string(), "s3cret".to_string())]),
        false,
    );
    let err = check_domain_send_mode(&secrets, &format!("bot@{d}")).expect_err("refuse");
    assert!(matches!(&err, SendError::SendMode(h) if h.contains("incomplete")), "{err:?}");
    cleanup_domain(&d, Some(&relay));

    // A non-smtp kind refuses with the teaching error (V1) —
    // nothing ASSUMED kind == smtp on the way here.
    let d = format!("r3-{uniq}.example");
    let relay = seed_relay("mailgun", None, None, None);
    seed_domain(&d, "relay", Some(&relay));
    let err = check_domain_send_mode(&secrets, &format!("bot@{d}")).expect_err("refuse");
    assert!(matches!(&err, SendError::SendMode(h) if h.contains("mailgun")), "{err:?}");
    cleanup_domain(&d, Some(&relay));
}

// ── hosted attachments ──

fn with_file(mut m: OutboundMessage, name: &str, ct: &str, bytes: &[u8]) -> OutboundMessage {
    use crate::mail::external_smtp::OutAttachment;
    m.attachments.push(StoredAttachment::from_out(&OutAttachment {
        scanned: false, filename: name.to_string(),
        content_type: ct.to_string(),
        bytes: bytes.to_vec(),
    }));
    m
}

fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

fn unique_from(mut m: OutboundMessage) -> OutboundMessage {
    m.from = format!("bot-{}@acme.dev", uuid::Uuid::new_v4().simple());
    m
}

#[test]
fn a_stored_file_knows_its_size_without_decoding() {
    for n in [0usize, 1, 2, 3, 4, 5, 1000, 1001, 1002] {
        let m = with_file(msg(), "f.bin", "application/octet-stream", &vec![7u8; n]);
        assert_eq!(m.attachments[0].size(), n as u64, "{n} bytes");
    }
}

/// On: the files go up, the composed message names them, the outbox keeps
/// only their names, and the body file never held the bytes.
#[test]
fn on_mode_uploads_the_files_and_keeps_only_their_names() {
    let fake = crate::mail::fake_jmap::FakeJmap::start();
    let client = fake.client();
    let (store, dir) = temp_store();
    let project = format!("P-{}", uuid::Uuid::new_v4());
    let secret_bytes: &[u8] = b"%PDF the real report bytes";
    let m = with_file(
        with_file(unique_from(msg()), "report.pdf", "application/pdf", secret_bytes),
        "notes.txt",
        "text/plain; charset=utf-8",
        b"hello\n",
    );
    let req = OutboundRequest { project_id: &project, agent_name: "bot", account_id: "acct", message: &m, send_after: None };
    let out = gate_and_dispatch(&store, Some(&client), Gate::On, &req, now_unix()).expect("dispatch");
    let SendOutcome::Submitted { id } = out else { panic!("expected a submit, got {out:?}") };

    let st = fake.state.lock().unwrap();
    assert_eq!(st.uploads, vec![secret_bytes.to_vec(), b"hello\n".to_vec()], "the bytes, in order");
    let e = &st.created[0];
    assert_eq!(e["attachments"][0]["blobId"], "up-1", "{e}");
    assert_eq!(e["attachments"][0]["name"], "report.pdf", "{e}");
    assert_eq!(e["attachments"][0]["type"], "application/pdf", "{e}");
    assert!(e["attachments"][0].get("charset").is_none(), "{e}");
    assert_eq!(e["attachments"][1]["type"], "text/plain", "a bare type: {e}");
    assert_eq!(e["attachments"][1]["charset"], "utf-8", "{e}");
    assert_eq!(e["attachments"][1]["disposition"], "attachment", "{e}");
    assert_eq!(st.envelopes.len(), 1, "one submission");
    drop(st);

    let row = store.load(&id).unwrap().expect("row");
    assert_eq!(row.status, "sent");
    let v = outbound_json(&row);
    assert_eq!(v["attachments"]["names"], serde_json::json!(["report.pdf", "notes.txt"]), "{v}");
    let at_rest = std::fs::read_to_string(dir.join(format!("{id}.json"))).expect("body file");
    assert!(!at_rest.contains("dataB64") && !at_rest.contains("JVBERi"), "no bytes at rest: {at_rest}");
    assert!(!dir.join(format!("{id}.files")).exists(), "on mode never holds the files");
    cleanup_rows(&project);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A refused upload sends nothing and says which file.
#[test]
fn a_refused_upload_fails_the_row_and_composes_nothing() {
    let fake = crate::mail::fake_jmap::FakeJmap::start();
    fake.state.lock().unwrap().upload_status = Some(413);
    let client = fake.client();
    let (store, dir) = temp_store();
    let project = format!("P-{}", uuid::Uuid::new_v4());
    let m = with_file(unique_from(msg()), "big.zip", "application/zip", b"zip");
    let req = OutboundRequest { project_id: &project, agent_name: "bot", account_id: "acct", message: &m, send_after: None };
    let out = gate_and_dispatch(&store, Some(&client), Gate::On, &req, now_unix()).expect("dispatch");
    let SendOutcome::SubmitFailed { id, error } = out else { panic!("expected a failure, got {out:?}") };
    assert!(error.contains("big.zip"), "{error}");
    assert_eq!(store.load(&id).unwrap().unwrap().status, "failed");
    let st = fake.state.lock().unwrap();
    assert!(st.created.is_empty() && st.envelopes.is_empty(), "nothing composed or submitted");
    drop(st);
    cleanup_rows(&project);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Scheduled: the bytes stay until the flush sends them, then go.
#[test]
fn a_scheduled_message_holds_its_files_until_it_leaves() {
    let fake = crate::mail::fake_jmap::FakeJmap::start();
    let client = fake.client();
    let (store, dir) = temp_store();
    let project = format!("P-{}", uuid::Uuid::new_v4());
    let now = now_unix();
    let m = with_file(unique_from(msg()), "a.bin", "application/octet-stream", b"scheduled bytes");
    let req = OutboundRequest { project_id: &project, agent_name: "bot", account_id: "acct", message: &m, send_after: Some(now + 3600) };
    let SendOutcome::Scheduled { id, .. } = gate_and_dispatch(&store, Some(&client), Gate::On, &req, now).expect("dispatch") else {
        panic!("expected a schedule")
    };
    assert_eq!(store.load_message(&id).unwrap().attachments.len(), 1, "held for the flush");
    assert!(fake.state.lock().unwrap().uploads.is_empty(), "nothing left yet");
    let mut acct = |_: &str| Ok("acct".to_string());
    let mut rules = |_: &str, _: &str| Ok(crate::mail::schedule::FlushRules { gate: Gate::On, policy_bcc: Vec::new(), refused: None });
    let out = crate::mail::schedule::flush_one_scheduled(&store, Ok(&client), &mut acct, &mut rules, &id, now + 3601).expect("flush");
    assert_eq!(out, Some(crate::mail::schedule::FlushOutcome::Sent));
    assert_eq!(fake.state.lock().unwrap().uploads, vec![b"scheduled bytes".to_vec()]);
    assert!(store.load_message(&id).unwrap().attachments.is_empty(), "gone once sent");
    cleanup_rows(&project);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Cancelling a schedule drops the bytes too.
#[test]
fn a_cancelled_schedule_drops_its_files() {
    let (store, dir) = temp_store();
    let project = format!("P-{}", uuid::Uuid::new_v4());
    let now = now_unix();
    let m = with_file(unique_from(msg()), "a.bin", "application/octet-stream", b"x");
    let req = OutboundRequest { project_id: &project, agent_name: "bot", account_id: "acct", message: &m, send_after: Some(now + 3600) };
    let keep = crate::mail::fake_jmap::FakeJmap::start().client();
    let SendOutcome::Scheduled { id, .. } = gate_and_dispatch(&store, Some(&keep), Gate::On, &req, now).expect("dispatch") else {
        panic!("expected a schedule")
    };
    crate::mail::schedule::cancel_scheduled(&store, &id, &project, "agent:bot", now).expect("cancel");
    assert!(store.load_message(&id).unwrap().attachments.is_empty());
    cleanup_rows(&project);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The hosted backend never drops a file it was handed apart from the
/// message: it refuses.
#[test]
fn hosted_submit_refuses_files_that_did_not_ride_the_message() {
    use crate::mail::submit::{HostedSubmit, MailSubmit, SubmitError};
    let client = crate::mail::fake_jmap::FakeJmap::start().client();
    let hosted = HostedSubmit { backend: &client, account_id: "acct".to_string() };
    let stray = crate::mail::external_smtp::OutAttachment {
        scanned: false, filename: "a".to_string(),
        content_type: "text/plain".to_string(),
        bytes: vec![1],
    };
    assert!(matches!(hosted.submit(&msg(), &[stray]), Err(SubmitError::Engine(_))));
}

#[test]
fn hosted_text_limit_counts_the_json_form() {
    assert!(check_hosted_text_size("s", &"y".repeat(MAX_HOSTED_TEXT_JSON_BYTES - 100)).is_ok());
    let err = check_hosted_text_size("s", &"y".repeat(MAX_HOSTED_TEXT_JSON_BYTES + 1)).unwrap_err();
    assert!(matches!(err, SendError::Usage(ref h) if h.contains("too large")), "{err:?}");
    // Control characters take six bytes each once escaped.
    let err = check_hosted_text_size("s", &"\u{1}".repeat(MAX_HOSTED_TEXT_JSON_BYTES / 4)).unwrap_err();
    assert!(matches!(err, SendError::Usage(_)), "{err:?}");
}

#[cfg(unix)]
#[test]
fn a_waiting_body_file_is_private_and_never_overwrites() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("k2-body-mode-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("out_x.json");
    write_private_new(&f, b"{}").expect("first write");
    let mode = std::fs::metadata(&f).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "{mode:o}");
    assert!(write_private_new(&f, b"again").is_err(), "an id is never reused");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn normalize_reply_to_caps_and_names_the_flag() {
    assert_eq!(normalize_reply_to(&["A@B.example".to_string()]).unwrap(), vec!["a@b.example"]);
    let four: Vec<String> = (0..4).map(|i| format!("r{i}@x.example")).collect();
    assert!(matches!(normalize_reply_to(&four), Err(SendError::Usage(h)) if h.contains("max 3")));
    match normalize_reply_to(&["a@b.example\r\nBcc: c@d.example".to_string()]) {
        Err(SendError::Usage(h)) => assert!(h.contains("--reply-to"), "{h}"),
        other => panic!("expected a refusal: {other:?}"),
    }
}

/// The header goes to the engine as a structured list, next to the rest.
#[test]
fn on_mode_reply_to_reaches_the_engine_as_a_header_not_a_recipient() {
    let fake = crate::mail::fake_jmap::FakeJmap::start();
    let client = fake.client();
    let (store, dir) = temp_store();
    let project = format!("P-{}", uuid::Uuid::new_v4());
    let mut m = unique_from(msg());
    m.reply_to = vec!["answers@acme.dev".to_string()];
    let req = OutboundRequest { project_id: &project, agent_name: "bot", account_id: "acct", message: &m, send_after: None };
    let SendOutcome::Submitted { id } = gate_and_dispatch(&store, Some(&client), Gate::On, &req, now_unix()).expect("dispatch") else {
        panic!("expected a submit")
    };
    let st = fake.state.lock().unwrap();
    assert_eq!(st.created[0]["replyTo"], serde_json::json!([{ "email": "answers@acme.dev" }]), "{}", st.created[0]);
    let rcpt = st.envelopes[0]["rcptTo"].to_string();
    assert!(!rcpt.contains("answers@acme.dev"), "{rcpt}");
    drop(st);
    assert_eq!(outbound_json(&store.load(&id).unwrap().unwrap())["replyTo"], serde_json::json!(["answers@acme.dev"]));
    cleanup_rows(&project);
    let _ = std::fs::remove_dir_all(&dir);
}

// ── holding files: disk, tampering, purges ──

fn queued_with_file(store: &DbOutboundStore, project: &str, bytes: &[u8]) -> String {
    let m = with_file(unique_from(msg()), "a.bin", "application/octet-stream", bytes);
    let req = OutboundRequest { project_id: project, agent_name: "bot", account_id: "acct", message: &m, send_after: None };
    match gate_and_dispatch(store, None, Gate::Approval, &req, now_unix()) {
        Ok(SendOutcome::Queued { id, .. }) => id,
        other => panic!("expected a queued message: {other:?}"),
    }
}

/// A held file is its own 0600 file, the body names it, and the queue can
/// be listed without reading it.
#[cfg(unix)]
#[test]
fn held_bytes_live_beside_the_body() {
    use std::os::unix::fs::PermissionsExt;
    let (store, dir) = temp_store();
    let project = format!("P-{}", uuid::Uuid::new_v4());
    let id = queued_with_file(&store, &project, b"held bytes");
    let file = dir.join(format!("{id}.files/0"));
    assert_eq!(std::fs::read(&file).unwrap(), b"held bytes");
    assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(std::fs::metadata(dir.join(format!("{id}.files"))).unwrap().permissions().mode() & 0o777, 0o700);
    let body = std::fs::read_to_string(dir.join(format!("{id}.json"))).unwrap();
    assert!(!body.contains("dataB64"), "{body}");
    // The list's read doesn't touch the bytes: gone, it still answers.
    std::fs::remove_file(&file).unwrap();
    let m = store.load_message(&id).expect("named, not read");
    assert_eq!(m.attachments[0].size(), 10);
    assert!(store.load_message_full(&id).is_err(), "a send needs them");
    cleanup_rows(&project);
    let _ = std::fs::remove_dir_all(&dir);
}

/// An edited body or held file fails closed: the row ends failed, nothing leaves.
#[test]
fn a_body_or_file_changed_after_queueing_never_leaves() {
    for edit_file in [false, true] {
        let (store, dir) = temp_store();
        let project = format!("P-{}", uuid::Uuid::new_v4());
        let id = queued_with_file(&store, &project, b"what was approved");
        if edit_file {
            std::fs::write(dir.join(format!("{id}.files/0")), b"swapped").unwrap();
        } else {
            let path = dir.join(format!("{id}.json"));
            let raw = std::fs::read_to_string(&path).unwrap();
            let edited = raw.replacen("\"to\":[\"", "\"replyTo\":[\"x@y.example\\r\\nBcc: z@y.example\"],\"to\":[\"", 1);
            assert_ne!(raw, edited);
            std::fs::write(&path, edited).unwrap();
            assert!(store.load_message(&id).is_err(), "not shown either");
        }
        let fake = crate::mail::fake_jmap::FakeJmap::start();
        let client = fake.client();
        let mut acct = |_: &str| Ok("acct".to_string());
        let out = approve_and_submit(&store, Ok(&client), &mut acct, &id, "owner", None, now_unix()).expect("decided");
        assert!(matches!(&out, ApproveOutcome::FailedToSubmit(r) if r.contains("changed")), "{out:?}");
        assert_eq!(store.load(&id).unwrap().unwrap().status, "failed");
        let st = fake.state.lock().unwrap();
        assert!(st.created.is_empty() && st.uploads.is_empty(), "nothing left");
        drop(st);
        cleanup_rows(&project);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// The last check before the mail server, for a body from anywhere.
#[test]
fn an_address_that_isnt_plain_never_reaches_the_engine() {
    let fake = crate::mail::fake_jmap::FakeJmap::start();
    let client = fake.client();
    let mut m = msg();
    m.reply_to = vec!["x@y.example\r\nBcc: z@y.example".to_string()];
    let err = client.submit("acct", &m).expect_err("refused");
    assert!(err.contains("plain local@domain"), "{err}");
    let mut m = msg();
    m.to = vec!["Not@Normal.Example".to_string()];
    assert!(client.submit("acct", &m).is_err());
    assert!(fake.state.lock().unwrap().created.is_empty());
}

#[test]
fn the_box_refuses_to_hold_past_its_cap_or_its_disk() {
    let (mut store, dir) = temp_store();
    let project = format!("P-{}", uuid::Uuid::new_v4());
    store.max_held_bytes = 15;
    let first = queued_with_file(&store, &project, &[1u8; 10]);
    assert!(!first.is_empty());
    let m = with_file(unique_from(msg()), "b.bin", "application/octet-stream", &[2u8; 10]);
    let req = OutboundRequest { project_id: &project, agent_name: "bot", account_id: "acct", message: &m, send_after: None };
    let err = gate_and_dispatch(&store, None, Gate::Approval, &req, now_unix()).expect_err("over the cap");
    assert!(matches!(&err, SendError::Guardrail(h) if h.contains("already holding")), "{err:?}");
    // Deciding what's waiting makes room.
    deny(&store, &first, "owner", "no", now_unix()).expect("deny");
    assert!(gate_and_dispatch(&store, None, Gate::Approval, &req, now_unix()).is_ok());

    store.max_held_bytes = MAX_HELD_BYTES;
    store.min_free_after_hold = u64::MAX / 2;
    let err = store.check_can_hold(&project, 1, 0).expect_err("disk");
    assert!(err.contains("too full"), "{err}");
    // On mode holds nothing, so it needs neither.
    let fake = crate::mail::fake_jmap::FakeJmap::start();
    let client = fake.client();
    assert!(matches!(
        gate_and_dispatch(&store, Some(&client), Gate::On, &req, now_unix()),
        Ok(SendOutcome::Submitted { .. })
    ));
    cleanup_rows(&project);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_message_with_files_is_scheduled_at_most_7_days_ahead() {
    let (store, dir) = temp_store();
    let project = format!("P-{}", uuid::Uuid::new_v4());
    let now = now_unix();
    let m = with_file(unique_from(msg()), "a.bin", "application/octet-stream", b"x");
    for gate in [Gate::On, Gate::Approval] {
        let req = OutboundRequest { project_id: &project, agent_name: "bot", account_id: "acct", message: &m, send_after: Some(now + 8 * 86_400) };
        let err = gate_and_dispatch(&store, None, gate, &req, now).expect_err("too far");
        assert!(matches!(&err, SendError::Usage(h) if h.contains("7 days")), "{err:?}");
    }
    let no_file = unique_from(msg());
    let req = OutboundRequest { project_id: &project, agent_name: "bot", account_id: "acct", message: &no_file, send_after: Some(now + 8 * 86_400) };
    assert!(matches!(gate_and_dispatch(&store, None, Gate::On, &req, now), Ok(SendOutcome::Scheduled { .. })), "text alone keeps 30 days");
    cleanup_rows(&project);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_removed_mailbox_ends_its_waiting_mail_and_keeps_no_files() {
    let store = DbOutboundStore::default();
    let project = format!("P-{}", uuid::Uuid::new_v4());
    let m = with_file(unique_from(msg()), "a.bin", "application/octet-stream", b"x");
    let req = OutboundRequest { project_id: &project, agent_name: "bot", account_id: "acct", message: &m, send_after: None };
    let SendOutcome::Queued { id, .. } = gate_and_dispatch(&store, None, Gate::Approval, &req, now_unix()).expect("queue") else {
        panic!("expected a queued message")
    };
    assert_eq!(purge_held_for_address(&m.from, now_unix()), 1);
    let row = store.load(&id).unwrap().unwrap();
    assert_eq!(row.status, "denied");
    assert!(row.note.as_deref().unwrap_or("").contains("mailbox was removed"), "{:?}", row.note);
    assert!(store.load_message(&id).unwrap().attachments.is_empty());
    assert!(!store.files_dir(&id).exists());
    cleanup_rows(&project);
}

/// A linked message queued by an older K2 keeps working.
#[test]
fn a_body_from_an_older_k2_still_reads() {
    let raw = r#"{"from":"a@b.example","to":["c@d.example"],"subject":"s","textBody":"t",
            "attachments":[{"filename":"f.txt","contentType":"text/plain","dataB64":"aGk="}]}"#;
    let m: OutboundMessage = serde_json::from_str(raw).expect("parse");
    assert_eq!(m.attachments[0].size(), 2);
    assert_eq!(m.attachments[0].bytes_now().unwrap(), b"hi");
}

/// The outbox can show later which bytes left: names, sizes and hashes. A
/// row from before this keeps reading as names.
#[test]
fn the_outbox_keeps_each_files_size_and_hash_never_its_bytes() {
    let fake = crate::mail::fake_jmap::FakeJmap::start();
    let client = fake.client();
    let (store, dir) = temp_store();
    let project = format!("P-{}", uuid::Uuid::new_v4());
    let m = with_file(unique_from(msg()), "a.bin", "application/octet-stream", b"the bytes");
    let req = OutboundRequest { project_id: &project, agent_name: "bot", account_id: "acct", message: &m, send_after: None };
    let SendOutcome::Submitted { id } = gate_and_dispatch(&store, Some(&client), Gate::On, &req, now_unix()).expect("send") else {
        panic!("expected a submit")
    };
    let row = store.load(&id).unwrap().unwrap();
    assert!(!row.attachments_ref.as_deref().unwrap_or("").contains("dGhl"), "no bytes");
    let v = outbound_json(&row);
    assert_eq!(v["attachments"]["names"], serde_json::json!(["a.bin"]), "{v}");
    assert_eq!(v["attachments"]["files"][0]["size"], 9, "{v}");
    assert_eq!(v["attachments"]["files"][0]["sha256"], sha256_hex(b"the bytes"), "{v}");
    let old = MailOutbound { attachments_ref: Some(r#"["old.pdf"]"#.to_string()), ..row };
    let v = outbound_json(&old);
    assert_eq!(v["attachments"]["names"], serde_json::json!(["old.pdf"]), "{v}");
    assert!(v["attachments"].get("files").is_none(), "{v}");
    cleanup_rows(&project);
    let _ = std::fs::remove_dir_all(&dir);
}

// ── the second review ──

/// A stuck reader mustn't hold the flusher: a FIFO in a held file's place
/// fails at once, and so does a link.
#[cfg(unix)]
#[test]
fn a_fifo_or_link_in_a_held_files_place_fails_at_once() {
    let (store, dir) = temp_store();
    let project = format!("P-{}", uuid::Uuid::new_v4());
    let id = queued_with_file(&store, &project, b"bytes");
    let file = dir.join(format!("{id}.files/0"));
    std::fs::remove_file(&file).unwrap();
    let c = std::ffi::CString::new(file.to_string_lossy().as_bytes()).unwrap();
    // SAFETY: a NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    let (tx, rx) = std::sync::mpsc::channel();
    let (store2, id2) = (DbOutboundStore::at(dir.clone()), id.clone());
    std::thread::spawn(move || {
        let _ = tx.send(store2.load_message_full(&id2).map(|_| ()));
    });
    let out = rx.recv_timeout(std::time::Duration::from_secs(20)).expect("hung on a FIFO");
    assert!(out.is_err(), "{out:?}");
    std::fs::remove_file(&file).unwrap();
    let elsewhere = dir.join("elsewhere");
    std::fs::write(&elsewhere, b"bytes").unwrap();
    std::os::unix::fs::symlink(&elsewhere, &file).unwrap();
    assert!(store.load_message_full(&id).is_err(), "a link isn't the held file");
    cleanup_rows(&project);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Finding 10: a body changed and its hash cleared still doesn't leave, as
/// what the approver was shown came from the row.
#[test]
fn a_body_that_disagrees_with_its_row_never_leaves() {
    let (store, dir) = temp_store();
    let project = format!("P-{}", uuid::Uuid::new_v4());
    let id = queued_with_file(&store, &project, b"x");
    let path = dir.join(format!("{id}.json"));
    let raw = std::fs::read_to_string(&path).unwrap();
    let edited = raw.replace("human@example.com", "evil@x.example");
    assert_ne!(raw, edited);
    std::fs::write(&path, edited).unwrap();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE mail_outbound SET body_sha256 = NULL WHERE id = ?1", rusqlite::params![id]).unwrap();
    }
    let fake = crate::mail::fake_jmap::FakeJmap::start();
    let client = fake.client();
    let mut acct = |_: &str| Ok("acct".to_string());
    let out = approve_and_submit(&store, Ok(&client), &mut acct, &id, "owner", None, now_unix()).expect("decided");
    assert!(matches!(&out, ApproveOutcome::FailedToSubmit(r) if r.contains("changed")), "{out:?}");
    assert!(fake.state.lock().unwrap().created.is_empty(), "nothing left");
    cleanup_rows(&project);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn row_and_message_agree_on_everything_the_approver_sees() {
    let (store, dir) = temp_store();
    let project = format!("P-{}", uuid::Uuid::new_v4());
    let mut m = unique_from(msg());
    m.cc = vec!["cc@x.example".to_string()];
    m.bcc = vec!["b@x.example".to_string()];
    m.reply_to = vec!["r@x.example".to_string()];
    let req = OutboundRequest { project_id: &project, agent_name: "bot", account_id: "acct", message: &m, send_after: None };
    let SendOutcome::Queued { id, .. } = gate_and_dispatch(&store, None, Gate::Approval, &req, now_unix()).expect("queue") else {
        panic!("expected a queued message")
    };
    let row = store.load(&id).unwrap().unwrap();
    assert!(row_matches_message(&row, &m));
    for change in [
        (|m: &mut OutboundMessage| m.to.push("x@y.example".to_string())) as fn(&mut OutboundMessage),
        |m| m.cc.clear(),
        |m| m.bcc.push("x@y.example".to_string()),
        |m| m.reply_to = vec!["x@y.example".to_string()],
        |m| m.from = "x@y.example".to_string(),
        |m| m.subject = "other".to_string(),
    ] {
        let mut changed = m.clone();
        change(&mut changed);
        assert!(!row_matches_message(&row, &changed), "{changed:?}");
    }
    cleanup_rows(&project);
    let _ = std::fs::remove_dir_all(&dir);
}

/// N4: parallel sends can't all pass a limit with room for one.
#[test]
fn parallel_holds_cannot_overrun_the_cap() {
    let (mut store, dir) = temp_store();
    store.max_held_bytes = 1000;
    let store = std::sync::Arc::new(store);
    let project = format!("P-{}", uuid::Uuid::new_v4());
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let (store, project) = (store.clone(), project.clone());
            std::thread::spawn(move || {
                let m = with_file(unique_from(msg()), "a.bin", "application/octet-stream", &[1u8; 400]);
                let req = OutboundRequest { project_id: &project, agent_name: "bot", account_id: "acct", message: &m, send_after: None };
                gate_and_dispatch(&*store, None, Gate::Approval, &req, now_unix()).is_ok()
            })
        })
        .collect();
    let ok = handles.into_iter().map(|h| h.join().unwrap()).filter(|b| *b).count();
    assert_eq!(ok, 2, "two fit under 1000 bytes");
    assert_eq!(store.held_bytes(), 800);
    cleanup_rows(&project);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn one_workspace_cannot_take_the_whole_cap() {
    let (mut store, dir) = temp_store();
    store.max_held_per_workspace = 15;
    let (a, b) = (format!("P-{}", uuid::Uuid::new_v4()), format!("P-{}", uuid::Uuid::new_v4()));
    queued_with_file(&store, &a, &[1u8; 10]);
    let m = with_file(unique_from(msg()), "b.bin", "application/octet-stream", &[2u8; 10]);
    let req = OutboundRequest { project_id: &a, agent_name: "bot", account_id: "acct", message: &m, send_after: None };
    let err = gate_and_dispatch(&store, None, Gate::Approval, &req, now_unix()).expect_err("over its share");
    assert!(matches!(&err, SendError::Guardrail(h) if h.contains("this workspace")), "{err:?}");
    let other = OutboundRequest { project_id: &b, agent_name: "bot", account_id: "acct", message: &m, send_after: None };
    assert!(gate_and_dispatch(&store, None, Gate::Approval, &other, now_unix()).is_ok(), "another workspace has room");
    cleanup_rows(&a);
    cleanup_rows(&b);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn long_text_needs_disk_too_and_short_text_doesnt() {
    let (mut store, dir) = temp_store();
    store.min_free_after_hold = u64::MAX / 2;
    let project = format!("P-{}", uuid::Uuid::new_v4());
    let mut long = unique_from(msg());
    long.text_body = "y".repeat(TEXT_FLOOR_FROM_BYTES as usize + 1);
    let req = OutboundRequest { project_id: &project, agent_name: "bot", account_id: "acct", message: &long, send_after: None };
    let err = gate_and_dispatch(&store, None, Gate::Approval, &req, now_unix()).expect_err("no room");
    assert!(matches!(&err, SendError::Guardrail(h) if h.contains("too full")), "{err:?}");
    let short = unique_from(msg());
    let req = OutboundRequest { project_id: &project, agent_name: "bot", account_id: "acct", message: &short, send_after: None };
    assert!(gate_and_dispatch(&store, None, Gate::Approval, &req, now_unix()).is_ok());
    cleanup_rows(&project);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A busy database at boot says nothing about a row: however old, nothing goes.
#[test]
fn the_boot_sweep_deletes_nothing_when_a_row_cant_be_read() {
    let (store, dir) = temp_store();
    let project = format!("P-{}", uuid::Uuid::new_v4());
    let waiting = queued_with_file(&store, &project, b"waiting");
    std::fs::create_dir_all(dir.join("out_noread0000000.files")).unwrap();
    std::fs::write(dir.join("out_noread0000000.json"), b"{}").unwrap();
    let busy = || {
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
            Some("database is locked".to_string()),
        )
    };
    let years_later = now_unix() + 10 * 365 * 86_400;
    assert_eq!(sweep_store_with(&store, years_later, &|_| Err(busy())), 0);
    assert!(store.files_dir(&waiting).exists() && store.body_path(&waiting).exists());
    assert!(dir.join("out_noread0000000.files").exists() && dir.join("out_noread0000000.json").exists());
    // Told plainly that there's no such row, the same files do go.
    assert!(sweep_store_with(&store, years_later, &|_| Ok(None)) >= 2);
    assert!(!dir.join("out_noread0000000.files").exists());
    cleanup_rows(&project);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The boot sweep: what's waiting stays, an in-flight orphan stays until it's
/// old, and a decided body goes after 30 days with its row kept.
#[test]
fn the_boot_sweep_keeps_what_waits_and_removes_the_rest() {
    let (store, dir) = temp_store();
    let project = format!("P-{}", uuid::Uuid::new_v4());
    let waiting = queued_with_file(&store, &project, b"waiting");
    let decided = queued_with_file(&store, &project, b"decided");
    deny(&store, &decided, "owner", "no", now_unix()).expect("deny");
    // A crash: files and a body with no row, and files left on a decided row.
    let orphan_dir = dir.join("out_orphan000000.files");
    std::fs::create_dir_all(&orphan_dir).unwrap();
    std::fs::write(orphan_dir.join("0"), b"x").unwrap();
    std::fs::write(dir.join("out_orphan000000.json"), b"{}").unwrap();
    let stale_dir = store.files_dir(&decided);
    std::fs::create_dir_all(&stale_dir).unwrap();
    std::fs::write(stale_dir.join("0"), b"x").unwrap();

    let now = now_unix();
    assert_eq!(sweep_store(&store, now), 1, "only the decided row's stale files, for now");
    assert!(orphan_dir.exists(), "a send in progress writes files first");
    assert!(store.files_dir(&waiting).exists());
    assert!(!stale_dir.exists());

    let later = now + DECIDED_BODY_KEEP_SECS + 60;
    // Nothing here touches `waiting`, however late.
    assert_eq!(sweep_store(&store, later), 3, "orphan files, orphan body, old decided body");
    assert!(!orphan_dir.exists() && !dir.join("out_orphan000000.json").exists());
    assert!(!store.body_path(&decided).exists());
    assert!(store.load(&decided).unwrap().is_some(), "the audit row stays");
    assert!(store.load_message_full(&waiting).is_ok(), "what's waiting is untouched");
    cleanup_rows(&project);
    let _ = std::fs::remove_dir_all(&dir);
}
