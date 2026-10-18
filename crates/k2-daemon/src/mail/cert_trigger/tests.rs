//! Tests for the 0.46.1 live-cert re-order. The live probe is the
//! supervisor's test seam (never a socket); orders go to a recording fake
//! (never an ACME server, never Stalwart).

use super::*;
use crate::mail::supervisor::{with_test_issued_cert, with_test_served_ports, TestLeaf};

const HOST: &str = "mail.acme.dev";
const T0: i64 = 1_800_000_000;
const H: i64 = 3_600;

struct Fake {
    now: i64,
    /// `Some` = this body instead of the probe seam.
    served_json: Option<Value>,
    owner: CertOwner,
    plan: Option<String>,
    /// The K2 issuer's answer (`Ok(ordered)`).
    k2: Result<bool, IssueError>,
    stalwart: StalwartOrder,
    state: ReorderState,
    saves: usize,
    calls: Vec<String>,
}

impl Fake {
    fn stalwart() -> Self {
        Fake {
            now: T0,
            served_json: None,
            owner: CertOwner::StalwartAcme,
            plan: Some("tls-alpn".into()),
            k2: Ok(false),
            stalwart: StalwartOrder::Queued("task-1".into()),
            state: ReorderState::default(),
            saves: 0,
            calls: Vec::new(),
        }
    }
    fn k2() -> Self {
        Fake { owner: CertOwner::K2, plan: Some("dns-01".into()), ..Fake::stalwart() }
    }
    fn orders(&self) -> usize {
        self.calls.iter().filter(|c| c.starts_with("renew-k2") || c.starts_with("order-stalwart")).count()
    }
}

impl TriggerDeps for Fake {
    fn now(&self) -> i64 {
        self.now
    }
    fn served(&self, host: &str) -> Value {
        match &self.served_json {
            Some(v) => v.clone(),
            None => crate::mail::supervisor::tls_cert_status(Some(host)),
        }
    }
    fn owner(&self, _host: &str) -> CertOwner {
        self.owner
    }
    fn port_plan(&self) -> Option<String> {
        self.plan.clone()
    }
    fn renew_k2(&mut self, host: &str) -> Result<Issued, IssueError> {
        self.calls.push(format!("renew-k2 {host}"));
        self.k2.clone().map(|ordered| Issued { pem: crate::domains::acme::test_le_pem(host), ordered })
    }
    fn order_stalwart(&mut self, host: &str) -> StalwartOrder {
        self.calls.push(format!("order-stalwart {host}"));
        self.stalwart.clone()
    }
    fn load_state(&self) -> ReorderState {
        self.state.clone()
    }
    fn save_state(&mut self, rs: &ReorderState) -> Result<(), String> {
        self.saves += 1;
        self.state = rs.clone();
        Ok(())
    }
}

fn all_ports(leaf: TestLeaf<'_>) -> crate::mail::supervisor::TestTlsCertGuard {
    with_test_served_ports(&[(443, leaf), (465, leaf), (993, leaf)])
}

// ── Classification → trigger decision ───────────────────────────────────

/// Self-signed, expired and mismatch order — on both owner paths.
#[test]
fn self_signed_expired_and_mismatch_order() {
    let cases: [(&str, TestLeaf<'_>); 3] = [
        ("self-signed", TestLeaf::SelfSigned),
        ("expired", TestLeaf::Expired(&[HOST])),
        ("mismatch", TestLeaf::Issued(&["other.acme.dev"])),
    ];
    for (want, leaf) in cases {
        let _g = all_ports(leaf);
        assert_eq!(trigger_state(&crate::mail::supervisor::tls_cert_status(Some(HOST))), want);

        let mut f = Fake::stalwart();
        let out = run_once(&mut f, HOST, false);
        assert!(out.ordered, "{want}: {}", out.line);
        assert_eq!(f.calls, vec![format!("order-stalwart {HOST}")], "{want}");
        assert_eq!(f.state.last_state.as_deref(), Some(want));
        assert_eq!(f.state.last_result.as_deref(), Some("queued"));
        assert_eq!(f.state.path.as_deref(), Some("stalwart-acme"));
        assert_eq!(f.state.next_attempt, Some(T0 + ORDER_WINDOW_SECS));

        let mut f = Fake::k2();
        let out = run_once(&mut f, HOST, false);
        assert!(out.ordered, "{want}: {}", out.line);
        assert_eq!(f.calls, vec![format!("renew-k2 {HOST}")], "{want}");
        assert_eq!(f.state.last_result.as_deref(), Some("planted"));
        assert_eq!(f.state.path.as_deref(), Some("k2"));
    }
}

/// A valid issued certificate (the issued-cert test seam) never orders.
#[test]
fn a_valid_issued_cert_never_orders() {
    let _g = with_test_issued_cert(&[HOST]);
    for mut f in [Fake::stalwart(), Fake::k2()] {
        let out = run_once(&mut f, HOST, false);
        assert!(!out.ordered, "{}", out.line);
        assert_eq!(f.orders(), 0, "{:?}", f.calls);
        assert_eq!(f.saves, 0, "nothing to reset: {}", out.line);
    }
    // A wildcard that covers the host is issued too.
    drop(_g);
    let _g = all_ports(TestLeaf::Issued(&["*.acme.dev"]));
    let mut f = Fake::stalwart();
    assert!(!run_once(&mut f, HOST, false).ordered);
    assert_eq!(f.orders(), 0);
}

/// Unknown (unparseable leaf) and missing (nothing answers) never order.
#[test]
fn unknown_and_no_answer_never_order() {
    let _g = all_ports(TestLeaf::Missing);
    for mut f in [Fake::stalwart(), Fake::k2()] {
        let out = run_once(&mut f, HOST, false);
        assert!(!out.ordered, "{}", out.line);
        assert!(out.line.contains("reads missing"), "{}", out.line);
        assert_eq!(f.orders(), 0);
        assert_eq!(f.saves, 0);
    }
    let unknown = serde_json::json!({
        "state": "unknown",
        "ports": [
            { "port": 443, "state": "unknown" },
            { "port": 465, "state": "unknown" },
            { "port": 993, "state": "unknown" },
        ],
    });
    for mut f in [Fake::stalwart(), Fake::k2()] {
        f.served_json = Some(unknown.clone());
        let out = run_once(&mut f, HOST, false);
        assert!(!out.ordered, "{}", out.line);
        assert!(out.line.contains("reads unknown"), "{}", out.line);
        assert_eq!(f.orders(), 0);
    }
}

/// Stalwart's own listeners decide: a wrong certificate on 443 alone (a
/// front) is not the mail certificate's to fix; a wrong one on 465/993 is,
/// even when 443 is fine. With neither 465 nor 993 answering, the overall
/// state decides.
#[test]
fn stalwart_ports_decide_not_443_alone() {
    let _g = with_test_served_ports(&[
        (443, TestLeaf::SelfSigned),
        (465, TestLeaf::Issued(&[HOST])),
        (993, TestLeaf::Issued(&[HOST])),
    ]);
    let mut f = Fake::stalwart();
    assert!(!run_once(&mut f, HOST, false).ordered);
    assert_eq!(f.orders(), 0);
    drop(_g);

    let _g = with_test_served_ports(&[
        (443, TestLeaf::Issued(&[HOST])),
        (465, TestLeaf::Issued(&[HOST])),
        (993, TestLeaf::Expired(&[HOST])),
    ]);
    let mut f = Fake::stalwart();
    assert!(run_once(&mut f, HOST, false).ordered);
    assert_eq!(f.state.last_state.as_deref(), Some("expired"));
    drop(_g);

    let _g = with_test_served_ports(&[
        (443, TestLeaf::SelfSigned),
        (465, TestLeaf::Missing),
        (993, TestLeaf::Missing),
    ]);
    let mut f = Fake::stalwart();
    assert!(run_once(&mut f, HOST, false).ordered);
    assert_eq!(f.state.last_state.as_deref(), Some("self-signed"));
}

// ── Ownership ───────────────────────────────────────────────────────────

/// Neither K2 nor Stalwart ACME owns the certificate → never an order,
/// never a write, whatever the live state.
#[test]
fn never_orders_where_neither_k2_nor_stalwart_acme_owns_the_cert() {
    let _g = all_ports(TestLeaf::SelfSigned);
    let not_owned = [
        Fake { owner: CertOwner::Unknown, ..Fake::stalwart() },
        Fake { owner: CertOwner::Unknown, plan: None, ..Fake::stalwart() },
        Fake { plan: Some("http-01".into()), ..Fake::stalwart() },
        Fake { plan: None, ..Fake::stalwart() },
    ];
    for mut f in not_owned {
        let out = run_once(&mut f, HOST, false);
        assert!(!out.ordered, "{}", out.line);
        assert_eq!(f.orders(), 0, "{:?}", f.calls);
        assert_eq!(f.saves, 0);
    }
    // Stalwart ACME switched to Manual (or not Stalwart's at all): the
    // order step says so and nothing is recorded as an order.
    let mut f = Fake {
        stalwart: StalwartOrder::NotEligible("Stalwart ACME is Manual".into()),
        ..Fake::stalwart()
    };
    let out = run_once(&mut f, HOST, false);
    assert!(!out.ordered, "{}", out.line);
    assert!(out.line.contains("Manual"), "{}", out.line);
    assert_eq!(f.saves, 0);
    assert_eq!(f.state, ReorderState::default());
}

#[test]
fn order_path_owner_matrix() {
    assert_eq!(order_path(CertOwner::K2, None), Ok(OrderPath::K2));
    assert_eq!(order_path(CertOwner::K2, Some("tls-alpn")), Ok(OrderPath::K2));
    assert_eq!(order_path(CertOwner::StalwartAcme, Some("tls-alpn")), Ok(OrderPath::StalwartAcme));
    assert!(order_path(CertOwner::StalwartAcme, Some("http-01")).is_err());
    assert!(order_path(CertOwner::Unknown, Some("tls-alpn")).is_err());
}

// ── Backoff ─────────────────────────────────────────────────────────────

#[test]
fn wait_doubles_from_the_window_to_the_cap() {
    assert_eq!(wait_after(0), H);
    assert_eq!(wait_after(1), 2 * H);
    assert_eq!(wait_after(2), 4 * H);
    assert_eq!(wait_after(3), 8 * H);
    assert_eq!(wait_after(4), 16 * H);
    assert_eq!(wait_after(5), BACKOFF_CAP_SECS);
    assert_eq!(wait_after(40), BACKOFF_CAP_SECS);
    assert_eq!(BACKOFF_CAP_SECS, 24 * H);
}

/// A second trigger inside the window does not order again; once the
/// window passes with the cert still wrong, the next order counts the
/// first as failed and waits twice as long.
#[test]
fn a_second_trigger_inside_the_window_does_not_order() {
    let _g = all_ports(TestLeaf::SelfSigned);
    let mut f = Fake::stalwart();
    assert!(run_once(&mut f, HOST, false).ordered);
    assert_eq!(f.orders(), 1);
    for later in [1, H / 2, H - 1] {
        f.now = T0 + later;
        let out = run_once(&mut f, HOST, false);
        assert!(!out.ordered, "{}", out.line);
        assert!(out.line.contains("backoff until"), "{}", out.line);
    }
    assert_eq!(f.orders(), 1, "{:?}", f.calls);
    f.now = T0 + H;
    assert!(run_once(&mut f, HOST, false).ordered);
    assert_eq!(f.orders(), 2);
    assert_eq!(f.state.failures, 1, "the first order did not fix it");
    assert_eq!(f.state.next_attempt, Some(T0 + H + 2 * H));
}

/// Failed orders double the wait up to the cap; a live `issued` resets it
/// and the next wrong cert orders at once with the base window.
#[test]
fn failure_doubles_the_wait_to_a_cap_and_success_resets() {
    let _g = all_ports(TestLeaf::Expired(&[HOST]));
    let mut f = Fake { stalwart: StalwartOrder::Failed("queueing the ACME order failed: 503".into()), ..Fake::stalwart() };
    let mut waits = Vec::new();
    for _ in 0..7 {
        let out = run_once(&mut f, HOST, false);
        assert!(out.ordered, "{}", out.line);
        assert!(out.line.contains("FAILED") && out.line.contains("(backoff)"), "{}", out.line);
        let next = f.state.next_attempt.expect("next attempt");
        waits.push(next - f.now);
        // Inside the wait: nothing.
        let before = f.orders();
        f.now = next - 1;
        assert!(!run_once(&mut f, HOST, false).ordered);
        assert_eq!(f.orders(), before);
        f.now = next;
    }
    assert_eq!(waits, vec![2 * H, 4 * H, 8 * H, 16 * H, 24 * H, 24 * H, 24 * H]);
    assert_eq!(f.state.failures, 7);
    assert_eq!(f.state.last_result.as_deref(), Some("failed"));
    assert!(f.state.last_error.as_deref().is_some_and(|e| e.contains("503")));
    drop(_g);

    // The certificate comes good: reset.
    let _g = with_test_issued_cert(&[HOST]);
    let out = run_once(&mut f, HOST, false);
    assert!(!out.ordered);
    assert!(out.line.contains("backoff reset"), "{}", out.line);
    assert_eq!(f.state.failures, 0);
    assert_eq!(f.state.next_attempt, None);
    assert_eq!(f.state.last_order, None);
    drop(_g);

    // Wrong again: orders immediately, base window.
    let _g = all_ports(TestLeaf::SelfSigned);
    f.stalwart = StalwartOrder::Queued("task-2".into());
    f.now += 60;
    assert!(run_once(&mut f, HOST, false).ordered);
    assert_eq!(f.state.failures, 0);
    assert_eq!(f.state.next_attempt, Some(f.now + ORDER_WINDOW_SECS));
}

/// The K2 path: the renewer's scan already handled the name → no second
/// issuer run; busy / no-longer-qualifies are not attempts; an order error
/// is a failure.
#[test]
fn k2_path_attempts_and_non_attempts() {
    let _g = all_ports(TestLeaf::Issued(&["old.acme.dev"]));
    let mut f = Fake::k2();
    let out = run_once(&mut f, HOST, true);
    assert!(!out.ordered && out.line.contains("scan handled"), "{}", out.line);
    assert_eq!(f.orders(), 0);

    for e in [IssueError::Busy(HOST.into()), IssueError::NotQualified("apex detached".into())] {
        let mut f = Fake { k2: Err(e), ..Fake::k2() };
        let out = run_once(&mut f, HOST, false);
        assert!(!out.ordered, "{}", out.line);
        assert_eq!(f.orders(), 1);
        assert_eq!(f.saves, 0, "not an attempt");
    }

    let mut f = Fake { k2: Err(IssueError::Order("DNS-01 record not visible".into())), ..Fake::k2() };
    assert!(run_once(&mut f, HOST, false).ordered);
    assert_eq!(f.state.failures, 1);
    assert_eq!(f.state.next_attempt, Some(T0 + 2 * H));

    let mut f = Fake { k2: Ok(true), ..Fake::k2() };
    assert!(run_once(&mut f, HOST, false).ordered);
    assert_eq!(f.state.last_result.as_deref(), Some("renewed"));
}

/// Stalwart path non-attempts (host elsewhere, a live task, names not
/// locked) leave the backoff alone.
#[test]
fn stalwart_not_now_is_not_an_attempt() {
    let _g = all_ports(TestLeaf::SelfSigned);
    let mut f = Fake {
        stalwart: StalwartOrder::NotNow("Stalwart already has a Pending ACME task (t-1)".into()),
        ..Fake::stalwart()
    };
    let out = run_once(&mut f, HOST, false);
    assert!(!out.ordered && out.line.contains("t-1"), "{}", out.line);
    assert_eq!(f.saves, 0);
    assert_eq!(f.state.next_attempt, None);
}

/// A state recorded for another mail host does not hold this one back.
#[test]
fn another_hosts_backoff_does_not_apply() {
    let _g = all_ports(TestLeaf::SelfSigned);
    let mut f = Fake::stalwart();
    f.state = ReorderState {
        host: Some("mail.old.dev".into()),
        failures: 5,
        next_attempt: Some(T0 + 10 * H),
        last_order: Some(T0 - H),
        ..Default::default()
    };
    assert!(run_once(&mut f, HOST, false).ordered);
    assert_eq!(f.state.host.as_deref(), Some(HOST));
    assert_eq!(f.state.failures, 0);
    assert_eq!(f.state.next_attempt, Some(T0 + H));
}

/// The backoff persists in renewal.json under `mailLiveReorder`; a file
/// written before 0.46.1 (no key) still reads.
#[test]
fn reorder_state_persists_in_renewal_json() {
    let mut file = crate::domains::renew::RenewFile::default();
    let mut rs = ReorderState::default();
    record_order(&mut rs, HOST, OrderPath::StalwartAcme, "expired", T0, Err("503".into()));
    file.mail_live_reorder = Some(rs.clone());
    let json = serde_json::to_value(&file).expect("serialize");
    assert_eq!(json["mailLiveReorder"]["nextAttempt"], T0 + 2 * H, "{json}");
    assert_eq!(json["mailLiveReorder"]["failures"], 1, "{json}");
    let back: crate::domains::renew::RenewFile = serde_json::from_value(json).expect("parse");
    assert_eq!(back.mail_live_reorder, Some(rs));
    let old: crate::domains::renew::RenewFile =
        serde_json::from_str(r#"{"names":{},"lastScan":1}"#).expect("pre-0.46.1 file");
    assert_eq!(old.mail_live_reorder, None);
    let empty = serde_json::to_value(crate::domains::renew::RenewFile::default()).expect("serialize");
    assert!(empty.get("mailLiveReorder").is_none(), "{empty}");
}
