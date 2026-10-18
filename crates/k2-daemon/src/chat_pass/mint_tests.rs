//! K7 unit tests: pass claims (incl. the optional caps ceiling), identity
//! per caller type and the Owner-role Connect ruling (both settings), the
//! mint handler's gates and order, rate limits, link cache, owner handoff
//! (person confirm, single use, 60 s, ≤ 5 outstanding), and no secret in
//! the audit. Each HOME-touching test runs under `TempHome`; the keyring is
//! a temp vault (fake key store).

use std::sync::Arc;

use k2_core::tool_hook::ConnectChatGate;
use k2_vault::keystore::fake::FakeKeyStore;
use k2_vault::{Actor, AgentActor, KeyStore, LocalVault, Role, VaultError};
use serde_json::Value;

use super::handoff::{self, RedeemError};
use super::identity::{self, Provenance, SewRole};
use super::keyring::{Keyring, PASS_ENGINE};
use super::link::{self, SewStatus};
use super::mint::{self, Deps, PassRequest};
use super::ratelimit::{Limiter, PER_DAEMON, PER_SID};
use super::{verify_with_jwks, ChatPassClaims, KeyError, CHAT_CAPS};
use crate::test_support::TempHome;
use crate::vault::actor::{Channel, VaultCaller};

const T0: i64 = 1_790_510_400;
const OWNER: &str = "0123456789abcdef0123456789abcdef";
const INSTALL: &str = "123456789";
const AUD: &str = "sew:123456789";

struct Ring {
    _dir: k2_vault::fsutil::TestDir,
    ring: Keyring,
}

fn ring(tag: &str) -> Ring {
    let dir = k2_vault::fsutil::TestDir::new(tag);
    let ks = Arc::new(FakeKeyStore::new());
    let vault = Arc::new(
        LocalVault::open(&dir.path().join("vault"), ks as Arc<dyn KeyStore>, k2_vault::OpenOptions { kdf: k2_vault::confirm::KdfCost::Floor })
            .expect("open temp vault"),
    );
    Ring { _dir: dir, ring: Keyring::new(vault, PASS_ENGINE) }
}

struct FixedStatus(Result<String, String>);
impl SewStatus for FixedStatus {
    fn install_id(&self) -> Result<String, String> {
        self.0.clone()
    }
}

struct Calls(std::sync::Mutex<usize>, Result<String, String>);
impl SewStatus for Calls {
    fn install_id(&self) -> Result<String, String> {
        *self.0.lock().unwrap() += 1;
        self.1.clone()
    }
}

fn body(r: &mint::Reply) -> Value {
    serde_json::from_str(&r.response.body).unwrap_or_else(|e| panic!("json body ({e}): {}", r.response.body))
}

fn code(r: &mint::Reply) -> String {
    body(r)["error"]["code"].as_str().unwrap_or_else(|| panic!("no error code: {}", r.response.body)).to_string()
}

/// The Connect member gate as "already delivered" (tests of other gates).
fn ready_gate(_: &str, _: &str) -> Result<ConnectChatGate, String> {
    Ok(ConnectChatGate::Ready)
}

fn req<'a>(bearer: Option<&'a str>, body: &'a [u8]) -> PassRequest<'a> {
    PassRequest { ingress: "loopback", ip: "127.0.0.1", browser: false, bearer, owner_token: OWNER, body, link: None }
}

/// Runs `f` with deps over a fresh ring/limiter/code store and a fixed
/// install id.
fn with_deps<R>(r: &Ring, now: i64, flip: bool, f: impl FnOnce(&Deps<'_>) -> R) -> R {
    let status = FixedStatus(Ok(INSTALL.into()));
    let limiter = Limiter::new(PER_SID, PER_DAEMON);
    let codes = handoff::Store::default();
    let sign = |c: &ChatPassClaims| r.ring.sign(c, now);
    f(&Deps { now, owner_connect_is_k2_owner: flip, status: &status, sign: &sign, limiter: &limiter, codes: &codes, connect_gate: &ready_gate })
}

fn audit_text() -> String {
    std::fs::read_to_string(k2_core::auth_audit::path()).unwrap_or_default()
}

// ── claims ──────────────────────────────────────────────────────────

#[test]
fn caps_claim_is_optional_closed_and_verifies() {
    let r = ring("k7-caps");
    let plain = ChatPassClaims::new(INSTALL, "k2:owner", "own_ab", T0).expect("claims");
    let v = serde_json::to_value(&plain).expect("ser");
    assert!(v.get("caps").is_none(), "no caps → no claim: {v}");
    let with = plain.clone().with_caps(vec!["chat:read".into(), "voice:join".into()]).expect("caps");
    let jwks = r.ring.public_jwks(T0).expect("jwks");
    let tok = r.ring.sign(&with, T0).expect("sign");
    let got = verify_with_jwks(&tok, &jwks, AUD, T0).expect("verifies");
    assert_eq!(got.caps.as_deref(), Some(&["chat:read".to_string(), "voice:join".to_string()][..]));
    let tok = r.ring.sign(&plain, T0).expect("sign");
    assert_eq!(verify_with_jwks(&tok, &jwks, AUD, T0).expect("verifies").caps, None);
    // Empty caps → no claim; an unknown cap is refused.
    assert_eq!(plain.clone().with_caps(vec![]).expect("empty").caps, None);
    assert!(plain.clone().with_caps(vec!["thread:read".into()]).is_err());
    assert!(plain.clone().with_caps(vec!["admin".into()]).is_err());
    assert_eq!(CHAT_CAPS, ["chat:read", "chat:post", "chat:respond", "voice:join"]);
}

#[test]
fn present_caps_always_carry_chat_read() {
    let r = ring("k7-sw3");
    let plain = ChatPassClaims::new(INSTALL, "k2:principal:0f8fad5b-d9cb-469f-a165-70867728950e", "skn_x", T0).expect("claims");
    // with_caps adds chat:read to any non-empty set (a voice-only role).
    let voice = plain.clone().with_caps(vec!["voice:join".into()]).expect("voice only");
    assert_eq!(voice.caps.as_deref(), Some(&["chat:read".to_string(), "voice:join".to_string()][..]));
    let post = plain.clone().with_caps(vec!["chat:post".into(), "chat:post".into()]).expect("dup");
    assert_eq!(post.caps.as_deref(), Some(&["chat:read".to_string(), "chat:post".to_string()][..]));
    let jwks = r.ring.public_jwks(T0).expect("jwks");
    let got = verify_with_jwks(&r.ring.sign(&voice, T0).expect("sign"), &jwks, AUD, T0).expect("verifies");
    assert!(got.caps.expect("caps").iter().any(|c| c == "chat:read"));
    // validate refuses a present caps without chat:read (built by hand) …
    let mut bad = plain.clone();
    bad.caps = Some(vec!["voice:join".into()]);
    let e = bad.validate().expect_err("no chat:read");
    assert!(e.contains("chat:read"), "{e}");
    // … so the keyring never signs one, and the verifier refuses one.
    assert!(r.ring.sign(&bad, T0).is_err(), "signing re-validates");
    bad.caps = Some(vec![]);
    assert!(bad.validate().is_err(), "present but empty");
    // Empty input → no claim (Sew's omitted-caps form).
    assert_eq!(plain.with_caps(vec![]).expect("none").caps, None);
}

#[test]
fn app_role_caps_pass_through_only_chat_caps() {
    let snap = vec!["thread:read".to_string(), "chat:post".into(), "voice:join".into(), "files:read".into()];
    assert_eq!(identity::chat_caps_of(&snap), vec!["chat:post".to_string(), "voice:join".into()]);
    assert!(identity::chat_caps_of(&["thread:read".to_string(), "thread:post".into()]).is_empty());
}

// ── identity ────────────────────────────────────────────────────────

#[test]
fn owner_role_connect_is_its_own_admin_member_by_ruling() {
    // The decided ruling (Rosson 2026-10-15) is pinned.
    const { assert!(!identity::OWNER_CONNECT_GETS_K2_OWNER) };
    let uuid = "0f8fad5b-d9cb-469f-a165-70867728950e";
    let tok = "a".repeat(64);
    let own = identity::connect_identity("olga", k2_core::connect_users::Role::Owner, uuid, &tok, false);
    assert_eq!(own.sub, format!("k2:connect:{uuid}"));
    assert_eq!(own.sew_role, SewRole::Member, "Connect Owner is server control, not Sew admin");
    assert_eq!(own.sew_role.as_str(), "member");
    assert_eq!(own.provenance, Provenance::ConnectLogin);
    assert_eq!(own.sid, k2_core::connect_users::session_sid(&tok));
    assert!(own.sid.starts_with("cu_") && own.sid.len() == 35);
    assert_eq!(own.actor, "user:olga");
    // Flipped: the same login becomes k2:owner, same cu_ sid.
    let flipped = identity::connect_identity("olga", k2_core::connect_users::Role::Owner, uuid, &tok, true);
    assert_eq!(flipped.sub, "k2:owner");
    assert_eq!(flipped.sew_role, SewRole::InstallOwner);
    assert_eq!(flipped.sid, own.sid);
    // Admin and Member roles are plain members either way.
    for flip in [false, true] {
        for role in [k2_core::connect_users::Role::Admin, k2_core::connect_users::Role::Member] {
            let i = identity::connect_identity("m", role, uuid, &tok, flip);
            assert_eq!(i.sub, format!("k2:connect:{uuid}"), "{role:?} flip={flip}");
            assert_eq!(i.sew_role, SewRole::Member);
        }
    }
}

#[test]
fn owner_token_and_refused_credentials_classify() {
    let _home = TempHome::new();
    let i = identity::classify(OWNER, OWNER).expect("owner");
    assert_eq!(i.sub, "k2:owner");
    assert_eq!(i.sid, identity::owner_sid(OWNER));
    assert!(i.sid.starts_with("own_") && i.sid.len() == 36 && !i.sid.contains(&OWNER[..8]));
    assert_eq!(i.provenance.as_str(), "owner-token");
    assert!(matches!(identity::classify("k2sk_abcdef", OWNER), Err(identity::Refusal::NotAllowed { .. })));
    assert_eq!(identity::classify("k2skn_unknownunknownunknown", OWNER), Err(identity::Refusal::Unauthenticated));
    assert_eq!(identity::classify("not-a-thing", OWNER), Err(identity::Refusal::Unauthenticated));
    assert_eq!(identity::classify("", OWNER), Err(identity::Refusal::Unauthenticated));
}

// ── the mint handler ────────────────────────────────────────────────

#[test]
fn pass_gates_run_in_order_and_owner_pass_is_marked_owner_token() {
    let _home = TempHome::new();
    let r = ring("k7-gates");
    with_deps(&r, T0, false, |d| {
        let mut q = req(Some(OWNER), b"{}");
        q.ingress = "tunnel";
        let x = mint::handle_pass_with(&q, d);
        assert_eq!((x.response.status, code(&x).as_str()), ("403 Forbidden", "loopback_only"));
        let mut q = req(Some(OWNER), b"{}");
        q.browser = true;
        assert_eq!(code(&mint::handle_pass_with(&q, d)), "browser_not_allowed");
        let x = mint::handle_pass_with(&req(Some(OWNER), br#"{"x":1}"#), d);
        assert_eq!(x.response.status, "400 Bad Request");
        assert!(body(&x)["error"]["hint"].as_str().expect("hint").contains("unknown field x"));
        let x = mint::handle_pass_with(&req(None, b""), d);
        assert_eq!((x.response.status, code(&x).as_str()), ("401 Unauthorized", "unauthenticated"));

        let ok = mint::handle_pass_with(&req(Some(OWNER), b""), d);
        assert_eq!(ok.response.status, "200 OK", "{}", ok.response.body);
        let b = body(&ok);
        assert_eq!(b["sub"], "k2:owner");
        assert_eq!(b["provenance"], "owner-token");
        assert_eq!(b["personProof"], false);
        assert_eq!(b["sewRole"], "install-owner");
        assert_eq!(b["expiresInMs"], 60_000);
        assert!(b.get("caps").is_none());
        let jwks = r.ring.public_jwks(T0).expect("jwks");
        let c = verify_with_jwks(b["pass"].as_str().expect("pass"), &jwks, AUD, T0).expect("verifies");
        assert_eq!((c.sub.as_str(), c.sid.as_str(), c.jti.as_str()), ("k2:owner", b["sid"].as_str().unwrap(), b["jti"].as_str().unwrap()));
        assert_eq!(c.exp - c.iat, 60);
        assert_eq!(c.app, None, "the owner's pass names no App");
        assert!(verify_with_jwks(b["pass"].as_str().unwrap(), &jwks, "sew:999", T0).is_err(), "wrong aud");
        assert!(verify_with_jwks(b["pass"].as_str().unwrap(), &jwks, AUD, T0 + 60 + 31).is_err(), "expired");
    });
    let audit = audit_text();
    assert!(audit.contains("\"chat-pass\"") && audit.contains("ok:"), "{audit}");
    assert!(audit.contains("loopback_only") && audit.contains("browser_not_allowed"), "{audit}");
    assert!(!audit.contains("eyJ"), "no pass in the audit");
    assert!(!audit.contains(OWNER), "no owner token in the audit");
}

#[test]
fn rate_limit_per_sid_and_per_daemon() {
    let l = Limiter::new(PER_SID, PER_DAEMON);
    for _ in 0..10 {
        l.check_and_record("own_a", T0).expect("under the sid limit");
    }
    let ms = l.check_and_record("own_a", T0 + 5).expect_err("11th in 60 s");
    assert_eq!(ms, 55_000);
    l.check_and_record("own_a", T0 + 60).expect("the window slid");
    let g = Limiter::new(PER_SID, PER_DAEMON);
    for i in 0..600 {
        g.check_and_record(&format!("sid{i}"), T0).expect("under the daemon limit");
    }
    assert!(g.check_and_record("fresh", T0).is_err(), "daemon bucket full");
    assert!(g.tracked_keys() <= super::ratelimit::MAX_KEYS);

    // Through the handler: 11th mint → 429 with retry_after_ms + Retry-After.
    let _home = TempHome::new();
    let r = ring("k7-rate");
    with_deps(&r, T0, false, |d| {
        for _ in 0..10 {
            assert_eq!(mint::handle_pass_with(&req(Some(OWNER), b""), d).response.status, "200 OK");
        }
        let x = mint::handle_pass_with(&req(Some(OWNER), b""), d);
        assert_eq!((x.response.status, code(&x).as_str()), ("429 Too Many Requests", "rate_limited"));
        assert_eq!(body(&x)["error"]["details"]["retry_after_ms"], 60_000);
        assert_eq!(x.retry_after_secs, Some(60));
    });
    assert!(audit_text().contains("rate_limited"));
}

#[test]
fn install_id_cache_fresh_stale_missing_and_mint_503() {
    let _home = TempHome::new();
    let dead = FixedStatus(Err("down".into()));
    assert!(link::install_id_for_mint(T0, &dead).is_err(), "nothing cached, Sew down");
    let live = Calls(std::sync::Mutex::new(0), Ok("4242".into()));
    assert_eq!(link::install_id_for_mint(T0, &live).expect("fetched"), "4242");
    assert_eq!(link::install_id_for_mint(T0 + 599, &live).expect("fresh"), "4242");
    assert_eq!(*live.0.lock().unwrap(), 1, "fresh value used without a call");
    let moved = Calls(std::sync::Mutex::new(0), Ok("777".into()));
    assert_eq!(link::install_id_for_mint(T0 + 600, &moved).expect("refreshed"), "777");
    assert_eq!(link::install_id_for_mint(T0 + 2000, &dead).expect("stale used"), "777");
    for bad in ["0", "abc", "0123", "12345678901234567890"] {
        let b = FixedStatus(Ok(bad.into()));
        link::clear_install_id().expect("clear");
        assert!(link::install_id_for_mint(T0, &b).is_err(), "{bad} refused");
        assert!(link::set_install_id(bad).is_err(), "{bad} refused");
    }
    // The handler: no install id → 503 chat_unavailable.
    link::clear_install_id().expect("clear");
    let r = ring("k7-503");
    let limiter = Limiter::new(PER_SID, PER_DAEMON);
    let codes = handoff::Store::default();
    let sign = |c: &ChatPassClaims| r.ring.sign(c, T0);
    let d = Deps { now: T0, owner_connect_is_k2_owner: false, status: &dead, sign: &sign, limiter: &limiter, codes: &codes, connect_gate: &ready_gate };
    let x = mint::handle_pass_with(&req(Some(OWNER), b""), &d);
    assert_eq!((x.response.status, code(&x).as_str()), ("503 Service Unavailable", "chat_unavailable"));
    // Keys unavailable → 503 too.
    link::set_install_id(INSTALL).expect("set");
    let broken = |_: &ChatPassClaims| -> Result<String, KeyError> { Err(KeyError::Corrupt("x".into())) };
    let d = Deps { sign: &broken, ..d };
    assert_eq!(code(&mint::handle_pass_with(&req(Some(OWNER), b""), &d)), "chat_unavailable");
    let p = link::path();
    assert!(p.ends_with(".k2/sew-link.json"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&p).expect("link file").permissions().mode() & 0o777, 0o600);
    }
}

// ── owner handoff ───────────────────────────────────────────────────

fn owner_caller() -> Result<VaultCaller, VaultError> {
    Ok(VaultCaller { actor: Actor::Owner, channel: Channel::Plain, session_id: None })
}

fn login_caller(role: Role) -> Result<VaultCaller, VaultError> {
    Ok(VaultCaller { actor: Actor::Login { username: "pat".into(), role }, channel: Channel::Login, session_id: None })
}

fn agent_caller() -> Result<VaultCaller, VaultError> {
    Ok(VaultCaller {
        actor: Actor::Agent(AgentActor { workspace_id: "ws1".into(), session_id: Some("s1".into()), shell: false, vault_manage: true }),
        channel: Channel::Plain,
        session_id: Some("s1".into()),
    })
}

#[test]
fn handoff_mint_needs_owner_tier_and_a_person_confirm() {
    let _home = TempHome::new();
    let limiter = Limiter::new(10, 10);
    let codes = handoff::Store::default();
    let ok = |_t: &str, _s: &str| -> Result<String, VaultError> { Ok("local-auth".into()) };
    let no = |_t: &str, _s: &str| -> Result<String, VaultError> { Err(VaultError::forbidden("vault_confirm_required", "confirm again")) };
    let tok = br#"{"confirmToken":"k2vc_x"}"#;
    macro_rules! call {
        ($ing:expr, $c:expr, $b:expr, $take:expr) => {
            mint::handle_owner_handoff_with($ing, $c, $b, T0, $take, &limiter, &codes)
        };
    }

    assert_eq!(code(&call!("tunnel", owner_caller(), tok, &ok)), "loopback_only");
    assert_eq!(code(&call!("loopback", agent_caller(), tok, &ok)), "owner_only", "agents never get install-owner");
    assert_eq!(code(&call!("loopback", login_caller(Role::Admin), tok, &ok)), "owner_only");
    assert_eq!(code(&call!("loopback", login_caller(Role::Member), tok, &ok)), "owner_only");
    // The owner token alone is not a person.
    assert_eq!(code(&call!("loopback", owner_caller(), b"{}", &ok)), "vault_confirm_required");
    assert_eq!(code(&call!("loopback", owner_caller(), tok, &no)), "vault_confirm_required");
    assert_eq!(codes.outstanding(T0), 0);

    let r = call!("loopback", owner_caller(), tok, &ok);
    assert_eq!(r.response.status, "200 OK", "{}", r.response.body);
    let c = body(&r)["code"].as_str().expect("code").to_string();
    assert!(handoff::is_code_shape(&c), "{c}");
    assert_eq!(body(&r)["expiresInMs"], 60_000);
    let r = call!("loopback", login_caller(Role::Owner), tok, &ok);
    assert_eq!(r.response.status, "200 OK", "an Owner-role Connect login with a confirm may open Sew as owner");
    let audit = audit_text();
    assert!(audit.contains("chat-handoff") && audit.contains(&format!("minted:{}", handoff::audit_handle(&c))), "{audit}");
    assert!(!audit.contains(&c), "never the code");
}

#[test]
fn handoff_redeem_single_use_expiry_cap_and_bearer_exclusive() {
    let _home = TempHome::new();
    let r = ring("k7-handoff");
    let status = FixedStatus(Ok(INSTALL.into()));
    let limiter = Limiter::new(PER_SID, PER_DAEMON);
    let codes = handoff::Store::default();
    let sign = |c: &ChatPassClaims| r.ring.sign(c, T0);
    let d = Deps { now: T0, owner_connect_is_k2_owner: false, status: &status, sign: &sign, limiter: &limiter, codes: &codes, connect_gate: &ready_gate };

    let c = codes.mint(T0, "owner-token").expect("mint");
    let b = serde_json::json!({ "handoff": c }).to_string();
    let x = mint::handle_pass_with(&req(Some(OWNER), b.as_bytes()), &d);
    assert_eq!(x.response.status, "400 Bad Request");
    assert!(body(&x)["error"]["hint"].as_str().unwrap().contains("exclusive"));
    let x = mint::handle_pass_with(&req(None, b.as_bytes()), &d);
    assert_eq!(x.response.status, "200 OK", "{}", x.response.body);
    let v = body(&x);
    assert_eq!(v["sub"], "k2:owner");
    assert_eq!(v["sid"], handoff::sid_for(&c));
    assert_eq!(v["provenance"], "owner-handoff");
    assert_eq!(v["personProof"], false);
    let jwks = r.ring.public_jwks(T0).expect("jwks");
    verify_with_jwks(v["pass"].as_str().unwrap(), &jwks, AUD, T0).expect("verifies");
    // Single use.
    let x = mint::handle_pass_with(&req(None, b.as_bytes()), &d);
    assert_eq!((x.response.status, code(&x).as_str()), ("401 Unauthorized", "handoff_invalid"));
    // Browser headers still refused for a handoff.
    let c2 = codes.mint(T0, "owner-token").expect("mint");
    let b2 = serde_json::json!({ "handoff": c2 }).to_string();
    let mut q = req(None, b2.as_bytes());
    q.browser = true;
    assert_eq!(code(&mint::handle_pass_with(&q, &d)), "browser_not_allowed");
    // 60 s expiry.
    assert_eq!(codes.redeem(&c2, T0 + 60), Err(RedeemError::Expired));
    let c3 = codes.mint(T0, "owner-token").expect("mint");
    assert_eq!(codes.redeem(&c3, T0 + 59).map(|_| ()), Ok(()));
    assert_eq!(codes.redeem(&c3, T0 + 59), Err(RedeemError::Used));
    assert_eq!(codes.redeem("k2sho_nope", T0), Err(RedeemError::Unknown));
    // At most 5 outstanding: the oldest goes.
    let s = handoff::Store::default();
    let first = s.mint(T0, "o").expect("mint");
    for i in 1..=5 {
        s.mint(T0 + i, "o").expect("mint");
    }
    assert_eq!(s.outstanding(T0 + 5), 5);
    assert_eq!(s.redeem(&first, T0 + 6), Err(RedeemError::Unknown), "oldest evicted");
    let audit = audit_text();
    assert!(audit.contains("handoff_invalid:used") && audit.contains("redeemed:"), "{audit}");
    assert!(!audit.contains(&c) && !audit.contains("eyJ"), "{audit}");
}

// ── K-M4: two Apps on one box (Hearth + Meet) ───────────────────────

const HEARTH_SVC: &str = "svc-hearth-1111";
const MEET_SVC: &str = "svc-meet-2222";

fn app_grant(principal_id: &str, svc: &str) {
    k2_core::skin::create_grant(&k2_core::skin::SkinGrantWrite {
        subject_kind: "principal".into(),
        subject_id: principal_id.into(),
        kind: "app".into(),
        target_id: svc.into(),
        role: None,
        scope: Some("[]".into()),
        created_by: None,
    })
    .expect("app grant");
}

/// A guest with a chat grant and App grants for `apps`; returns its id.
fn app_guest(name: &str, apps: &[&str]) -> String {
    let p = k2_core::skin::add_principal(name).expect("principal");
    k2_core::skin::set_principal_password(name, Some("s3cret-horse-battery")).expect("pw");
    k2_core::skin::insert_chat_grant_for_tests(&p.id).expect("chat grant");
    for a in apps {
        app_grant(&p.id, a);
    }
    p.id
}

/// K-M4: Meet signs its users in with K2 next to Hearth, through S28 on
/// each App's own link. Nothing assumes one Sew App per box: each App's
/// guest sessions mint on that App's link only, every pass names the
/// person and that App session (`sid`, `app`), the audience is the box's
/// one Sew install for both, and rate limits are per session.
#[test]
fn two_apps_on_one_box_each_mint_for_their_own_guests_k_m4() {
    let _home = TempHome::new();
    let r = ring("k-m4");
    let ada = app_guest("ada", &[HEARTH_SVC, MEET_SVC]);
    let bo = app_guest("bo", &[MEET_SVC]);
    let ada_h = k2_core::skin::create_app_session_token("ada", HEARTH_SVC).expect("ada → Hearth");
    let ada_m = k2_core::skin::create_app_session_token("ada", MEET_SVC).expect("ada → Meet");
    let bo_m = k2_core::skin::create_app_session_token("bo", MEET_SVC).expect("bo → Meet");
    assert_eq!(
        k2_core::skin::create_app_session_token("bo", HEARTH_SVC).expect_err("no Hearth grant"),
        k2_core::skin::HOST_LOGIN_DENIED,
        "an App login needs that App's own grant"
    );
    let meet_pass = k2_core::skin::link::mint_link_pass(MEET_SVC, "meet", "gw:38602", &["thread:read".into()], &["ws-meet".into()])
        .expect("Meet's link pass");
    let hearth = mint::OnLink { app_id: "hearth", service_id: HEARTH_SVC, link_id: "gw:38601" };
    let meet = mint::OnLink { app_id: "meet", service_id: MEET_SVC, link_id: "gw:38602" };
    let jwks = r.ring.public_jwks(T0).expect("jwks");

    with_deps(&r, T0, false, |d| {
        let call = |bearer: &str, body: &[u8], link: Option<mint::OnLink<'_>>| {
            let mut q = req(Some(bearer), body);
            if link.is_some() {
                q.ingress = "app-link";
            }
            q.link = link;
            mint::handle_pass_with(&q, d)
        };
        let minted = |x: &mint::Reply, sid: &str, app: &str, sub: &str| {
            assert_eq!(x.response.status, "200 OK", "{}", x.response.body);
            let b = body(x);
            assert_eq!((b["sub"].as_str(), b["sid"].as_str(), b["app"].as_str()), (Some(sub), Some(sid), Some(app)));
            assert_eq!(b["provenance"], "app-session");
            let c = verify_with_jwks(b["pass"].as_str().expect("pass"), &jwks, AUD, T0).expect("one Sew install: one audience");
            assert_eq!((c.sub.as_str(), c.sid.as_str()), (sub, sid));
            // Spec v1.3: the JWS names the App, the same string as `app`.
            assert_eq!(c.app.as_deref(), Some(app), "the pass's app claim");
        };
        let ada_sub = format!("k2:principal:{ada}");
        let bo_sub = format!("k2:principal:{bo}");
        let sid = |id: &str| format!("skn_{id}");

        // Each App's guests mint on that App's link.
        minted(&call(&ada_h.raw, b"{}", Some(hearth)), &sid(&ada_h.meta.id), HEARTH_SVC, &ada_sub);
        minted(&call(&ada_m.raw, b"{}", Some(meet)), &sid(&ada_m.meta.id), MEET_SVC, &ada_sub);
        minted(&call(&bo_m.raw, b"{}", Some(meet)), &sid(&bo_m.meta.id), MEET_SVC, &bo_sub);
        assert_ne!(ada_h.meta.id, ada_m.meta.id, "one person, one session per App");

        // Another App's session, the owner token, the App's own link pass
        // and a handoff code are refused on a link.
        assert_eq!(code(&call(&ada_h.raw, b"{}", Some(meet))), "pass_wrong_link", "a Hearth session on Meet's link");
        assert_eq!(code(&call(&bo_m.raw, b"{}", Some(hearth))), "pass_wrong_link", "a Meet session on Hearth's link");
        assert_eq!(code(&call(OWNER, b"{}", Some(meet))), "pass_wrong_link", "the owner token names no App guest");
        let x = call(&meet_pass.raw, b"{}", Some(meet));
        assert_eq!((x.response.status, code(&x).as_str()), ("403 Forbidden", "credential_not_allowed"), "the App's own pass names no person");
        let mut q = req(None, br#"{"handoff":"k2sho_x"}"#);
        q.link = Some(meet);
        assert_eq!(mint::handle_pass_with(&q, d).response.status, "400 Bad Request");

        // Off a link (a legacy --cmd BFF on loopback) both sessions mint.
        minted(&call(&ada_m.raw, b"{}", None), &sid(&ada_m.meta.id), MEET_SVC, &ada_sub);

        // Limits are per session: Hearth's session runs out, Meet's doesn't.
        let mut limited = false;
        for _ in 0..super::ratelimit::PER_SID {
            let x = call(&ada_h.raw, b"{}", Some(hearth));
            if x.response.status.starts_with("429") {
                limited = true;
                break;
            }
            assert_eq!(x.response.status, "200 OK", "{}", x.response.body);
        }
        assert!(limited, "Hearth's session hit its own limit");
        minted(&call(&ada_m.raw, b"{}", Some(meet)), &sid(&ada_m.meta.id), MEET_SVC, &ada_sub);
    });

    // The pure rule.
    let i = identity::PassIdentity {
        sub: "k2:principal:x".into(),
        sid: "skn_x".into(),
        actor: "principal:x".into(),
        provenance: Provenance::AppSession,
        sew_role: SewRole::Member,
        caps: Vec::new(),
        app: Some(MEET_SVC.into()),
    };
    assert!(mint::link_admits(&i, meet));
    assert!(!mint::link_admits(&i, hearth));
    let host = identity::PassIdentity { app: Some(k2_core::skin::HOST_APP_ID.into()), ..i.clone() };
    assert!(!mint::link_admits(&host, meet), "a box login is no App's guest");
    let connect = identity::PassIdentity { provenance: Provenance::ConnectLogin, app: None, ..i };
    assert!(!mint::link_admits(&connect, meet));
}

/// Spec v1.3's `app` claim: App sessions only, in Sew's grammar; K2's
/// service ids (lowercase UUIDs) always fit.
#[test]
fn app_claim_is_for_app_sessions_only_and_fits_sews_grammar() {
    let app_session = identity::PassIdentity {
        sub: "k2:principal:x".into(),
        sid: "skn_x".into(),
        actor: "principal:x".into(),
        provenance: Provenance::AppSession,
        sew_role: SewRole::Member,
        caps: Vec::new(),
        app: Some(MEET_SVC.into()),
    };
    assert_eq!(mint::app_claim(&app_session).as_deref(), Some(MEET_SVC));
    let uuid = uuid::Uuid::new_v4().to_string();
    let i = identity::PassIdentity { app: Some(uuid.clone()), ..app_session.clone() };
    assert_eq!(mint::app_claim(&i), Some(uuid.clone()), "a real service id fits");
    assert!(super::app_claim_ok(&uuid) && uuid.len() <= super::MAX_APP_CLAIM);
    let host = identity::PassIdentity { app: Some(k2_core::skin::HOST_APP_ID.into()), ..app_session.clone() };
    assert_eq!(mint::app_claim(&host), None, "a box login is no App");
    let odd = identity::PassIdentity { app: Some("Svc With Spaces".into()), ..app_session.clone() };
    assert_eq!(mint::app_claim(&odd), None, "outside Sew's grammar: left out, never refused");
    for p in [Provenance::OwnerToken, Provenance::ConnectLogin, Provenance::OwnerHandoff] {
        let i = identity::PassIdentity { provenance: p, ..app_session.clone() };
        assert_eq!(mint::app_claim(&i), None, "{p:?}");
    }
    assert!(!super::app_claim_ok("") && !super::app_claim_ok(&"a".repeat(129)) && super::app_claim_ok(&"a".repeat(128)));
    // Signed and verified, and refused by validate when malformed.
    let r = ring("k7-app");
    let c = ChatPassClaims::new(INSTALL, "k2:principal:p", "skn_1", T0).unwrap().with_app(Some(&uuid)).unwrap();
    let got = verify_with_jwks(&r.ring.sign(&c, T0).unwrap(), &r.ring.public_jwks(T0).unwrap(), AUD, T0).unwrap();
    assert_eq!(got.app.as_deref(), Some(uuid.as_str()));
    assert!(ChatPassClaims::new(INSTALL, "k2:principal:p", "skn_1", T0).unwrap().with_app(Some("UPPER")).is_err());
    let none = serde_json::to_value(ChatPassClaims::new(INSTALL, "k2:owner", "own_1", T0).unwrap()).unwrap();
    assert!(none.get("app").is_none(), "absent, never null: {none}");
}

#[test]
fn browser_header_detection() {
    let head = "POST /cli/sew/pass HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer y\r\n\r\n";
    assert!(!mint::has_browser_headers(head));
    assert!(mint::has_browser_headers("POST / HTTP/1.1\r\norigin: https://x\r\n\r\n"));
    // A browser's fetch (Chrome/Firefox/Safari shape).
    assert!(mint::has_browser_headers(
        "POST / HTTP/1.1\r\nSec-Fetch-Site: same-origin\r\nSec-Fetch-Mode: cors\r\nSec-Fetch-Dest: empty\r\n\r\n"
    ));
    for one in ["Sec-Fetch-Site: cross-site", "sec-fetch-dest: document", "Sec-Fetch-User: ?1", "Sec-Fetch-Mode: navigate", "Sec-Fetch-Mode: no-cors"] {
        assert!(mint::has_browser_headers(&format!("POST / HTTP/1.1\r\n{one}\r\n\r\n")), "{one}");
    }
    // Node's fetch (undici) on a BFF: exactly this, and it is a server.
    let node = "POST /cli/sew/pass HTTP/1.1\r\nhost: 127.0.0.1:8730\r\nconnection: keep-alive\r\ncontent-type: application/json\r\naccept: */*\r\naccept-language: *\r\nsec-fetch-mode: cors\r\nuser-agent: node\r\naccept-encoding: gzip, deflate\r\ncontent-length: 2\r\n\r\n";
    assert!(!mint::has_browser_headers(node), "@k2/server's fetch is not a browser");
    // Only the head counts, never the body.
    assert!(!mint::has_browser_headers("POST / HTTP/1.1\r\nHost: x\r\n\r\nOrigin: https://x\r\n"));
}

// ── SW2: a Connect login mints only once its member is delivered ─────

/// Sew never invents members (a pass whose `sub` has no member gets 403
/// `not_provisioned`), so a Connect login's mint asks the member gate
/// first: queued → 503 `chat_member_pending` + `Retry-After`, never a
/// pass; blocked → 503; grant off → 403 `no_chat_grant`; Sew off → 503.
/// Retries do not spend the BFF's mint budget. The owner token never
/// reaches the gate.
#[test]
fn connect_login_mints_only_after_its_member_is_delivered() {
    let _home = TempHome::new();
    let r = ring("k7-connect-gate");
    k2_core::connect_users::add_user("mia", "password-for-tests-1").expect("add login");
    let tok = k2_core::connect_users::create_session("mia");
    let uuid = k2_core::connect_users::user_uuid("mia").expect("uuid").expect("exists");
    let status = FixedStatus(Ok(INSTALL.into()));
    let limiter = Limiter::new(PER_SID, PER_DAEMON);
    let codes = handoff::Store::default();
    let sign = |c: &ChatPassClaims| r.ring.sign(c, T0);
    let seen = std::sync::Mutex::new(Vec::<(String, String)>::new());
    let state = std::sync::Mutex::new(ConnectChatGate::Pending);
    let gate = |id: &str, login: &str| -> Result<ConnectChatGate, String> {
        seen.lock().unwrap().push((id.to_string(), login.to_string()));
        Ok(state.lock().unwrap().clone())
    };
    let d = Deps {
        now: T0,
        owner_connect_is_k2_owner: false,
        status: &status,
        sign: &sign,
        limiter: &limiter,
        codes: &codes,
        connect_gate: &gate,
    };
    let mint = || mint::handle_pass_with(&req(Some(&tok), b"{}"), &d);

    // Queued: more retries than the per-sid budget, none of them a pass.
    for _ in 0..PER_SID + 2 {
        let x = mint();
        assert_eq!((x.response.status, code(&x).as_str()), ("503 Service Unavailable", "chat_member_pending"));
        assert_eq!(x.retry_after_secs, Some(mint::MEMBER_PENDING_RETRY_SECS));
        assert_eq!(body(&x)["error"]["details"]["retry_after_ms"], 2000);
        assert!(!x.response.body.contains("\"pass\""), "{}", x.response.body);
    }
    assert_eq!(seen.lock().unwrap()[0], (uuid.clone(), "mia".to_string()), "gate gets the id and the login");

    *state.lock().unwrap() = ConnectChatGate::Blocked("username 'mia-1a2b' is taken in chat".into());
    let x = mint();
    assert_eq!((x.response.status, code(&x).as_str()), ("503 Service Unavailable", "chat_member_blocked"));
    assert!(x.response.body.contains("is taken in chat"), "{}", x.response.body);
    *state.lock().unwrap() = ConnectChatGate::Disabled;
    let x = mint();
    assert_eq!((x.response.status, code(&x).as_str()), ("403 Forbidden", "no_chat_grant"));
    *state.lock().unwrap() = ConnectChatGate::Off;
    assert_eq!(code(&mint()), "chat_unavailable");

    // Delivered: the pass names the login's own member. The pending
    // retries above spent nothing of the per-sid budget.
    *state.lock().unwrap() = ConnectChatGate::Ready;
    let x = mint();
    assert_eq!(x.response.status, "200 OK", "{}", x.response.body);
    assert_eq!(body(&x)["sub"], format!("k2:connect:{uuid}"));

    // The owner token never asks the Connect gate.
    let before = seen.lock().unwrap().len();
    let x = mint::handle_pass_with(&req(Some(OWNER), b"{}"), &d);
    assert_eq!(x.response.status, "200 OK", "{}", x.response.body);
    assert_eq!(seen.lock().unwrap().len(), before);
    let audit = audit_text();
    assert!(audit.contains("chat_member_pending") && audit.contains("chat_member_blocked"), "{audit}");
}
