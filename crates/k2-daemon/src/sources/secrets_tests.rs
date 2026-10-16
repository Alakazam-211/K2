//! Secret-bound sources against the test vault and a fake upstream on
//! loopback (an Owner-typed exact IP).

use super::*;
use crate::egress::test_util::AirgapEnv;
use crate::sources::fetch::tests::{http, plan, Fake};
use crate::sources::fetch::fetch_once;
use crate::sources::test_engine::Harness;
use k2_core::sources::spec::SourceSpec;
use k2_core::sources::store::{self, NewSource};
use k2_core::sources::{FetchOutcome, SourceStatus};
use k2_vault::{Actor, Kind, Namespace, PutRequest, SecretBytes};

fn confirmed() -> k2_vault::HumanProof {
    k2_vault::HumanProof::Confirmed { method: "test".into() }
}

fn put(name: &str, kind: Kind, value: &str, host: &str) {
    let v = crate::vault::ensure_test_vault();
    let mut r = PutRequest::new(Namespace::User, Scope::server(), name, kind);
    r.hosts = vec![host.to_string()];
    v.put(r, SecretBytes::from_slice(value.as_bytes()), &Actor::Owner).expect("put");
}

/// Echoes the Authorization header and the path back in a JSON body.
fn echo() -> Fake {
    Fake::start(|head| {
        let auth = head
            .lines()
            .find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case("authorization")).map(|(_, v)| v.trim().to_string()))
            .unwrap_or_default();
        let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
        let token = auth.strip_prefix("Bearer ").unwrap_or(&auth).to_string();
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD.encode(token.as_bytes());
        let body = serde_json::json!({"auth": auth, "raw": token, "b64": b64, "path": path, "state": "READY"}).to_string();
        http("200 OK", &[("Content-Type", "application/json"), ("X-Echo", &auth)], body.as_bytes())
    })
}

#[test]
fn a_bound_secret_goes_in_the_sealed_header_and_the_echo_is_redacted() {
    let _a = AirgapEnv::set(false);
    const VAL: &str = "k2test_SRCBIND_0f1e2d3c4b5a69788776";
    let up = echo();
    put("SRC_BIND_KEY", Kind::ApiKey, VAL, &up.origin());
    let mut p = plan(&up.origin(), "/v1/x", true);
    p.source_id = Some("src-bind-1".into());
    p.secret = Some("SRC_BIND_KEY".into());
    // Not bound yet: the vault refuses this source.
    match fetch_once(&p) {
        FetchOutcome::Err { reason, .. } => assert_eq!(reason, Reason::SecretDisabled),
        other => panic!("{other:?}"),
    }
    assert_eq!(up.conns(), 0, "no request without the grant");
    // Binding needs a person's confirm.
    assert_eq!(bind("src-bind-1", "SRC_BIND_KEY", &up.origin(), &k2_vault::HumanProof::None).unwrap_err().code, "vault_confirm_required");
    assert_eq!(bind("src-bind-1", "SRC_BIND_KEY", "https://api.example.com:443", &confirmed()).unwrap_err().code, "secret_origin_mismatch");
    assert_eq!(bind("src-bind-1", "NO_SUCH_KEY", &up.origin(), &confirmed()).unwrap_err().code, "secret_not_found");
    bind("src-bind-1", "SRC_BIND_KEY", &up.origin(), &confirmed()).expect("bind");
    match fetch_once(&p) {
        FetchOutcome::Ok { body, .. } => {
            let text = body.to_string();
            assert!(!text.contains(VAL), "value echoed back: {text}");
            use base64::Engine as _;
            assert!(!text.contains(&base64::engine::general_purpose::STANDARD.encode(VAL)), "{text}");
            assert!(text.contains("[k2v:SRC_BIND_KEY]"), "{text}");
            assert_eq!(body["state"], "READY");
        }
        other => panic!("{other:?}"),
    }
    // Another source can't use this grant.
    p.source_id = Some("src-bind-2".into());
    assert!(matches!(fetch_once(&p), FetchOutcome::Err { reason: Reason::SecretDisabled, .. }));
    // Unbind revokes it.
    p.source_id = Some("src-bind-1".into());
    unbind("src-bind-1").expect("unbind");
    assert!(matches!(fetch_once(&p), FetchOutcome::Err { reason: Reason::SecretDisabled, .. }));
    let mut missing = p.clone();
    missing.secret = Some("NOT_IN_VAULT".into());
    assert!(matches!(fetch_once(&missing), FetchOutcome::Err { reason: Reason::SecretMissing, .. }));
}

#[test]
fn a_url_secret_stays_in_the_vault_and_out_of_the_answer() {
    let _a = AirgapEnv::set(false);
    let up = echo();
    let hook = format!("{}/hook/T0000ABCDEF/abcdefghijklmnopqrstuvwx", up.origin());
    put("SRC_URL_HOOK", Kind::Url, &hook, &up.origin());
    let mut p = plan(&up.origin(), "/ignored", true);
    p.source_id = Some("src-url-1".into());
    p.url = UrlRef::Secret { name: "SRC_URL_HOOK".into() };
    bind("src-url-1", "SRC_URL_HOOK", &up.origin(), &confirmed()).expect("bind");
    match fetch_once(&p) {
        FetchOutcome::Ok { body, .. } => {
            let text = body.to_string();
            assert!(!text.contains("abcdefghijklmnopqrstuvwx"), "the URL secret's path came back: {text}");
        }
        other => panic!("{other:?}"),
    }
    // A URL secret whose origin isn't the source's pinned origin: refused.
    let other = Fake::start(|_| http("200 OK", &[], b"{}"));
    put("SRC_URL_ELSEWHERE", Kind::Url, &format!("{}/x", other.origin()), &up.origin());
    bind("src-url-2", "SRC_URL_ELSEWHERE", &up.origin(), &confirmed()).expect("bind");
    let mut q = plan(&up.origin(), "/", true);
    q.source_id = Some("src-url-2".into());
    q.url = UrlRef::Secret { name: "SRC_URL_ELSEWHERE".into() };
    assert!(matches!(fetch_once(&q), FetchOutcome::Err { .. }));
    assert_eq!(other.conns(), 0);
}

#[test]
fn root_key_hints_read_the_value_inside_the_daemon() {
    let _a = AirgapEnv::set(false);
    put("SRC_ROOT_STRIPE", Kind::ApiKey, "sk_live_k2testabcdef", "https://api.stripe.com");
    put("SRC_RESTRICTED", Kind::ApiKey, "rk_live_k2testabcdef", "https://api.stripe.com");
    let h = root_key_hint_for("SRC_ROOT_STRIPE").expect("hint").expect("a root key");
    assert_eq!(h.code, "stripe_secret");
    assert!(!h.hint.contains("k2testabcdef"));
    assert!(root_key_hint_for("SRC_RESTRICTED").expect("hint").is_none());
    assert_eq!(root_key_hint_for("SRC_NOPE").unwrap_err().code, "secret_not_found");
}

/// Contract §10.4 #20: the value and its encodings never reach the
/// projection, card view, events, audit, or any 0163 table.
#[test]
fn secret_never_in_outputs() {
    let _a = AirgapEnv::set(false);
    const VAL: &str = "k2test_NEVEROUT_a1b2c3d4e5f6a7b8c9";
    let up = echo();
    put("SRC_NEVER_OUT", Kind::ApiKey, VAL, &up.origin());
    let g = k2_core::db::scoped_for_test();
    let db = g.handle();
    let conn = db.lock();
    let spec: SourceSpec = serde_json::from_value(serde_json::json!({
        "name": "secret-echo", "label": "Echo", "everySecs": 30, "secret": "SRC_NEVER_OUT",
        "fetch": {"url": format!("{}/v1/echo", up.origin())},
        "extract": {"fields": {"auth": "/auth", "raw": "/raw", "b64": "/b64", "state": "/state"}},
        "rules": [{"field": "state", "ok": {"eq": "READY"}, "crit": {"ne": "READY"}}]
    }))
    .unwrap();
    let v = spec.validate(true).unwrap();
    let row = store::insert_source(
        &conn,
        &NewSource { v: &v, status: SourceStatus::Active, made_by: "owner", made_by_project: None, approved_by: Some("owner"), now: 1_760_000_000 },
    )
    .unwrap();
    bind(&row.id, "SRC_NOVER_OUT_TYPO", &up.origin(), &confirmed()).expect_err("typo refused");
    bind(&row.id, "SRC_NEVER_OUT", &up.origin(), &confirmed()).expect("bind");
    let mut h = Harness::new(|p| crate::sources::fetch::fetch_detailed(p), 1_760_000_000);
    h.step(&conn);
    h.advance(31);
    h.step(&conn);
    let (st, proj) = store::load_state(&conn, &row.id).unwrap().unwrap();
    assert_eq!(st.phase, k2_core::sources::Phase::Live, "{:?} {:?}", st.reason, st.phase);
    let proj = proj.expect("projection");
    assert_eq!(proj.fields["state"], "READY");
    let view = k2_core::sources::view::build(&row, &st, Some(&proj), k2_core::sources::Viewer::Person, 1_760_000_100);
    use base64::Engine as _;
    let forms = [VAL.to_string(), base64::engine::general_purpose::STANDARD.encode(VAL), format!("Bearer {VAL}")];
    let mut dumps = vec![serde_json::to_string(&proj).unwrap(), serde_json::to_string(&view).unwrap()];
    for t in ["sources", "source_state", "source_items", "source_points", "source_events", "source_audit"] {
        let mut stmt = conn.prepare(&format!("SELECT * FROM {t}")).unwrap();
        let n = stmt.column_count();
        let rows = stmt
            .query_map([], |r| {
                let mut s = String::new();
                for i in 0..n {
                    let v: rusqlite::types::Value = r.get(i)?;
                    s.push_str(&format!("{v:?}|"));
                }
                Ok(s)
            })
            .unwrap();
        for r in rows {
            dumps.push(r.unwrap());
        }
    }
    for d in &dumps {
        for f in &forms {
            assert!(!d.contains(f.as_str()), "a form of the secret leaked: {d}");
        }
    }
}

/// GV68: the first vault denial parks the source (paused, the reason, one
/// audit line); it stays parked until a Refresh or a bind.
#[test]
fn a_denial_parks_the_source_until_refresh() {
    let _a = AirgapEnv::set(false);
    let up = echo();
    put("SRC_PARK_KEY", Kind::ApiKey, "k2test_PARK_00112233445566", &up.origin());
    let g = k2_core::db::scoped_for_test();
    let db = g.handle();
    let conn = db.lock();
    let spec: SourceSpec = serde_json::from_value(serde_json::json!({
        "name": "parked", "label": "Parked", "everySecs": 30, "secret": "SRC_PARK_KEY",
        "fetch": {"url": format!("{}/x", up.origin())}
    }))
    .unwrap();
    let v = spec.validate(true).unwrap();
    let row = store::insert_source(
        &conn,
        &NewSource { v: &v, status: SourceStatus::Active, made_by: "owner", made_by_project: None, approved_by: Some("owner"), now: 1_760_000_000 },
    )
    .unwrap();
    let mut h = Harness::new(|p| crate::sources::fetch::fetch_detailed(p), 1_760_000_000);
    h.step(&conn);
    h.advance(31);
    h.step(&conn);
    let (st, _) = store::load_state(&conn, &row.id).unwrap().unwrap();
    assert_eq!((st.phase, st.reason, st.vault_denied), (k2_core::sources::Phase::Paused, Some(Reason::SecretDisabled), true));
    assert_eq!(st.health, k2_core::sources::Health::Unknown, "a vault refusal isn't a failed check");
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM source_audit WHERE action = 'vault_denied_parked'", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 1);
    // Parked: later cycles don't even ask the vault.
    let before = up.conns();
    for _ in 0..3 {
        h.advance(31);
        h.step(&conn);
    }
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM source_audit WHERE action = 'vault_denied_parked'", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 1, "parked once, not every cycle");
    // A person binds it and presses Refresh: it runs again.
    bind(&row.id, "SRC_PARK_KEY", &up.origin(), &confirmed()).expect("bind");
    h.sched.poll_now(&row.id);
    h.step(&conn);
    let (st, _) = store::load_state(&conn, &row.id).unwrap().unwrap();
    assert_eq!(st.phase, k2_core::sources::Phase::Live, "{:?}", st.reason);
    assert!(!st.vault_denied);
    assert_eq!(up.conns(), before + 1);
}
