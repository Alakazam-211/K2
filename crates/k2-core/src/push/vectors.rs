//! The E2E push test vectors for the phones (prd-e2e-push-v1 rev 2 §7.3):
//! `tests/data/e2e_push_vectors.json`, made by the REAL plaintext, fitting,
//! padding, sealing and wire-body code, with fixed keys and a fixed
//! ephemeral seed so the file is reproducible. K2 Companion builds its
//! decrypt (iOS + Android) against it.
//!
//! `vectors_file_has_not_drifted` regenerates the file in memory and fails
//! if it differs from the committed one. To change it on purpose (a format
//! change the phones must follow), run once with `BLESS_E2E_PUSH_VECTORS=1`
//! and tell K2 Companion.

#![cfg(test)]

use super::e2e::{self, SealCtx, SealedEntry};
use super::seal::{self, FixedIkm, SealKind};
use super::*;
use base64::Engine as _;

pub(crate) const VECTORS_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/e2e_push_vectors.json");

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The phone's fixed test key: `DeriveKeyPair(ikmR)` with `ikmR` = 32 × 0x11.
pub(crate) fn recipient() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let (sk, pk) = seal::derive_keypair(&[0x11; 32]);
    let psk: Vec<u8> = (0u8..32).map(|i| 0xa0 ^ i).collect();
    (sk, pk, psk)
}

fn thread(preview: &str, agent: &str) -> PushEvent {
    PushEvent::ThreadPost(ThreadPush {
        agent_name: agent.into(),
        conversation_id: "c-77".into(),
        addr: "k2/reviewer".into(),
        seq: 418,
        project_id: "proj-1".into(),
        msg_kind: ThreadMsgKind::Text,
        count: 1,
        principal: "owner".into(),
        device_usernames: vec!["owner".into()],
        unread_total: "4".into(),
        waiting_total: "1".into(),
        avatar_rev: "ba7816bf8f01cfea".into(),
        agent_color: "#3b82f6".into(),
        preview: preview.into(),
        msg: None,
        budget: crate::push::ThreadBudget::Thread,
    })
}

/// The same Thread push with its `msg` (the post itself).
fn with_msg(event: PushEvent, text: &str) -> PushEvent {
    let PushEvent::ThreadPost(mut t) = event else { unreachable!("a thread push") };
    t.msg = Some(ThreadMsg::new("ov-0f8fad5b", t.seq, &t.conversation_id, "cortana", 1_760_959_998, t.msg_kind, text));
    PushEvent::ThreadPost(t)
}

/// Build the whole vectors document.
pub(crate) fn build() -> serde_json::Value {
    let (sk, pk, psk) = recipient();
    let kid = seal::key_id(&pk);
    let stamp = PushStamp { subdomain: "rosson".into(), instance_id: "9f2c0a1b3d4e5f60".into() };
    let bk = "0123456789abcdef0123456789abcdef".to_string();
    let ctx = |ctr: u64| SealCtx { kid: kid.clone(), ctr, ts: 1_760_960_000, stamp: stamp.clone(), badge_key: bk.clone() };

    let long_body: String = "Deploy finished — all checks green 🚀 ".repeat(60);
    let long_agent: String = "Cortana-the-very-long-agent-name-".repeat(4);
    // (name, kind, plaintext value, previews on)
    let cases: Vec<(&str, SealKind, serde_json::Value, String)> = vec![
        (
            "alert-thread-preview",
            SealKind::Alert,
            e2e::alert_plaintext(&thread("Deploy finished, 3 checks green…", "Cortana"), &ctx(1_760_960_000_123))
                .expect("alert"),
            "A Thread post, previews on: title = agent, body = preview.".into(),
        ),
        (
            "alert-thread-no-preview",
            SealKind::Alert,
            e2e::alert_plaintext(
                &thread("Deploy finished, 3 checks green…", "Cortana").without_preview(),
                &ctx(1_760_960_000_124),
            )
            .expect("alert"),
            "The same post for a phone with previews off: today's generic line by kind.".into(),
        ),
        (
            "alert-ticket",
            SealKind::Alert,
            e2e::alert_plaintext(
                &PushEvent::FeedbackCreated { agent_name: "Cortana".into(), feedback_id: "fb-42".into(), assignees: vec![] },
                &ctx(1_760_960_000_125),
            )
            .expect("alert"),
            "A ticket: title K2, body \"<agent> needs you on a ticket\", group ticket:<id>.".into(),
        ),
        (
            "alert-project",
            SealKind::Alert,
            e2e::alert_plaintext(
                &PushEvent::ProjectPost(ProjectPush {
                    project_name: "Apollo".into(),
                    group_id: "pg-9".into(),
                    principal: "owner".into(),
                    device_usernames: vec!["owner".into()],
                    unread_total: "7".into(),
                    waiting_total: "2".into(),
                }),
                &ctx(1_760_960_000_126),
            )
            .expect("alert"),
            "A project chat post, with the person's totals in data.".into(),
        ),
        (
            "alert-max-truncated",
            SealKind::Alert,
            e2e::alert_plaintext(
                &PushEvent::ProjectPost(ProjectPush {
                    project_name: long_body.clone(),
                    group_id: "pg-9".into(),
                    principal: "owner".into(),
                    device_usernames: vec!["owner".into()],
                    unread_total: "7".into(),
                    waiting_total: "2".into(),
                }),
                &ctx(1_760_960_000_127),
            )
            .expect("alert"),
            "A project post whose name makes the body (\"New message in <project>\") far over the 1536-byte \
             bucket: the body is cut at a character boundary with \"…\" until the JSON fills the bucket. \
             (Thread previews are already capped at 140 characters.)"
                .into(),
        ),
        (
            "alert-thread-long-agent-name",
            SealKind::Alert,
            e2e::alert_plaintext(&thread("Deploy finished.", &long_agent), &ctx(1_760_960_000_129)).expect("alert"),
            "A 132-character agent name: it fits the bucket, so it is NOT capped (names are capped at 64 only when \
             cutting the body can't make room)."
                .into(),
        ),
        (
            "nudge",
            SealKind::Nudge,
            e2e::nudge_plaintext(
                &BadgePush {
                    principal: "owner".into(),
                    device_usernames: vec!["owner".into()],
                    unread_total: "4".into(),
                    waiting_total: "1".into(),
                    stale: Default::default(),
                },
                &ctx(1_760_960_000_128),
            ),
            "A silent nudge: totals only, no ids.".into(),
        ),
        (
            "alert-thread-msg-full",
            SealKind::Alert,
            e2e::alert_plaintext(
                &with_msg(
                    thread("Deploy finished, 3 checks green…", "Cortana"),
                    "Deploy finished, 3 checks green. Staging is on 0.46.3-rc2; smoke tests next, then I'll post the \
                     release notes for review. Nothing needs you yet. 🚀",
                ),
                &ctx(1_760_960_000_130),
            )
            .expect("alert"),
            "A Thread post with `msg` (follow-up 2026-10-21): the whole message text rides sealed, \
             `truncated: false`. `msg` is present whatever the previews switches say (they decide only the \
             visible title/body); it is never in a legacy push or a nudge."
                .into(),
        ),
        (
            "alert-thread-msg-truncated",
            SealKind::Alert,
            e2e::alert_plaintext(
                &with_msg(thread("Release notes draft…", "Cortana"), &"Release notes draft, section by section. ".repeat(80)),
                &ctx(1_760_960_000_131),
            )
            .expect("alert"),
            "A Thread post too long for the bucket: `msg.text` is cut at a character boundary until the JSON fills \
             the 1536-byte bucket and `truncated: true` (the app fetches the rest). The visible body is not cut."
                .into(),
        ),
    ];

    let mut out_cases = Vec::new();
    for (i, (name, kind, value, note)) in cases.into_iter().enumerate() {
        let json = match kind {
            SealKind::Alert => e2e::fit_alert(&value).expect("fits"),
            SealKind::Nudge => e2e::fit_nudge(&value).expect("fits"),
        };
        let padded = seal::pad(&json, kind).expect("pad");
        let ikm_e = [0x21u8 + i as u8; 32];
        let (sk_e, pk_e) = seal::derive_keypair(&ikm_e);
        let blob = seal::seal_with_rng(kind, &pk, &psk, &json, &mut FixedIkm(ikm_e)).expect("seal");
        assert_eq!(&blob[2..67], &pk_e[..], "enc is DeriveKeyPair(ikmE)");
        let b64 = seal::B64.encode(&blob);
        assert_eq!(seal::open(kind, &sk, &psk, &b64).expect("opens"), json, "{name} round trip");
        let body = e2e::sealed_dispatch_body(
            kind,
            &[SealedEntry { platform: "apns".into(), token: "<apns-device-token>".into(), e2e: b64.clone() }],
        );
        out_cases.push(serde_json::json!({
            "name": name,
            "note": note,
            "kind": kind.wire_kind(),
            "infoHex": hex(kind.info()),
            "aadHex": hex(&blob[..2]),
            "ikmEHex": hex(&ikm_e),
            "ephemeralPrivateKeyHex": hex(&sk_e),
            "encHex": hex(&blob[2..67]),
            "ctr": value["ctr"],
            "ts": value["ts"],
            "plaintextJson": String::from_utf8(json.clone()).expect("utf8"),
            "plaintextJsonBytes": json.len(),
            "paddedPlaintextHex": hex(&padded),
            "ciphertextHex": hex(&blob[67..]),
            "blobHex": hex(&blob),
            "e2e": b64,
            "e2eLength": blob.len() * 4 / 3 + usize::from(blob.len() % 3 != 0),
            "relayDispatchBody": body,
        }));
    }

    serde_json::json!({
        "about": "K2 push-e2e-v1 test vectors (prd-e2e-push-v1 rev 2 §3, §7.3). Generated by the K2 daemon's real \
                  seal code (crates/k2-core/src/push/vectors.rs) with fixed keys and a fixed ephemeral seed. \
                  HPKE RFC 9180 mode_psk, DHKEM(P-256, HKDF-SHA256) 0x0010, HKDF-SHA256 0x0001, AES-256-GCM 0x0002. \
                  The ephemeral key of each case is DeriveKeyPair(ikmE). Decrypt: enc = blob[2..67], \
                  ct = blob[67..], aad = blob[0..2], info per kind, psk + pskId below, sequence 0; then unpad: \
                  L = u16be(pt[0..2]), JSON = pt[2..2+L], every remaining byte 0x00.",
        "version": 1,
        "suite": {
            "name": seal::SUITE_NAME,
            "mode": 1,
            "kemId": 16,
            "kdfId": 1,
            "aeadId": 2,
            "formatByte": seal::FORMAT_V1,
            "suiteByte": seal::SUITE_ID,
        },
        "pskId": String::from_utf8(seal::PSK_ID.to_vec()).expect("ascii"),
        "pskIdHex": hex(seal::PSK_ID),
        "info": {
            "alert": String::from_utf8_lossy(SealKind::Alert.info()),
            "alertHex": hex(SealKind::Alert.info()),
            "nudge": String::from_utf8_lossy(SealKind::Nudge.info()),
            "nudgeHex": hex(SealKind::Nudge.info()),
        },
        "sizes": {
            "alertPaddedPlaintext": seal::ALERT_PAD,
            "nudgePaddedPlaintext": seal::NUDGE_PAD,
            "alertBlobBytes": SealKind::Alert.blob_len(),
            "nudgeBlobBytes": SealKind::Nudge.blob_len(),
            "alertBase64urlChars": SealKind::Alert.b64_len(),
            "nudgeBase64urlChars": SealKind::Nudge.b64_len(),
            "encBytes": seal::ENC_LEN,
            "tagBytes": seal::TAG_LEN,
        },
        "recipient": {
            "ikmRHex": hex(&[0x11; 32]),
            "privateKeyHex": hex(&sk),
            "publicKeyHex": hex(&pk),
            "publicKeyB64url": seal::B64.encode(&pk),
            "pskHex": hex(&psk),
            "pskB64url": seal::B64.encode(&psk),
            "keyId": kid,
            "server": { "sub": stamp.subdomain, "iid": stamp.instance_id },
            "badgeKey": bk,
        },
        "registerBody": {
            "deviceId": "<per (install, server entry)>",
            "platform": "apns",
            "token": "<apns-device-token>",
            "previews": true,
            "e2e": {
                "v": 1,
                "suite": seal::SUITE_NAME,
                "publicKey": seal::B64.encode(&pk),
                "psk": seal::B64.encode(&psk),
                "createdAt": 1_760_960_000,
            },
        },
        "cases": out_cases,
    })
}

pub(crate) fn render() -> String {
    serde_json::to_string_pretty(&build()).expect("json") + "\n"
}

#[test]
fn vectors_file_has_not_drifted() {
    let fresh = render();
    if std::env::var("BLESS_E2E_PUSH_VECTORS").is_ok() {
        std::fs::write(VECTORS_PATH, &fresh).expect("write vectors");
        return;
    }
    let committed = std::fs::read_to_string(VECTORS_PATH).expect("committed vectors file");
    assert!(
        committed == fresh,
        "tests/data/e2e_push_vectors.json drifted from the seal code. If the format changed on purpose, \
         regenerate with BLESS_E2E_PUSH_VECTORS=1 and tell K2 Companion."
    );
}

/// Every case in the committed file decrypts with the recipient key, its
/// blob has the relay's exact length, and its relay body carries nothing
/// but `{kind, v}` + per-device `e2e`.
#[test]
fn committed_vectors_decrypt_and_match_the_relay_contract() {
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(VECTORS_PATH).expect("vectors")).expect("json");
    let (sk, _, psk) = recipient();
    let cases = doc["cases"].as_array().expect("cases");
    assert_eq!(cases.len(), 9);
    for c in cases {
        let kind = if c["kind"] == "nudge" { SealKind::Nudge } else { SealKind::Alert };
        let b64 = c["e2e"].as_str().expect("e2e");
        assert_eq!(b64.len(), kind.b64_len(), "{}", c["name"]);
        let json = seal::open(kind, &sk, &psk, b64).expect("opens");
        assert_eq!(String::from_utf8(json).expect("utf8"), c["plaintextJson"].as_str().expect("pt"));
        let body = &c["relayDispatchBody"];
        assert_eq!(body["payload"], serde_json::json!({ "data": { "kind": kind.wire_kind(), "v": "1" } }));
    }
    let max = cases.iter().find(|c| c["name"] == "alert-max-truncated").expect("max case");
    let n = max["plaintextJsonBytes"].as_u64().expect("n") as usize;
    assert!(n <= seal::max_json(SealKind::Alert) && n > seal::max_json(SealKind::Alert) - 8, "fills the bucket: {n}");
    assert!(max["plaintextJson"].as_str().expect("pt").contains("…\""), "cut with an ellipsis");
    let pt = |name: &str| -> serde_json::Value {
        let c = cases.iter().find(|c| c["name"] == name).expect("case");
        serde_json::from_str(c["plaintextJson"].as_str().expect("pt")).expect("json")
    };
    let full = pt("alert-thread-msg-full");
    assert_eq!(full["msg"]["truncated"], false);
    assert!(full["msg"]["text"].as_str().expect("t").ends_with("🚀"));
    let cut = pt("alert-thread-msg-truncated");
    assert_eq!(cut["msg"]["truncated"], true);
    assert_eq!(cut["body"], "Release notes draft…", "the visible body is not cut");
    for name in ["alert-thread-preview", "nudge"] {
        assert!(pt(name).get("msg").is_none(), "{name}: no msg");
    }
}

// ── Format 2 (push-e2e-v2, 0.46.7) ───────────────────────────────────────

pub(crate) const VECTORS_V2_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/e2e_push_vectors_v2.json");

fn badge(stale: StaleHint) -> BadgePush {
    BadgePush {
        principal: "owner".into(),
        device_usernames: vec!["owner".into()],
        unread_total: "4".into(),
        waiting_total: "1".into(),
        stale,
    }
}

/// The format-2 vectors document: the same recipient, PSK and shape as the
/// format-1 file, every case sealed in format 2 (byte 0 = 0x02, 2880-byte
/// alert / 1024-byte nudge plaintext) with a fixed ephemeral seed.
pub(crate) fn build_v2() -> serde_json::Value {
    let f = seal::Format::V2;
    let (sk, pk, psk) = recipient();
    let kid = seal::key_id(&pk);
    let stamp = PushStamp { subdomain: "rosson".into(), instance_id: "9f2c0a1b3d4e5f60".into() };
    let bk = "0123456789abcdef0123456789abcdef".to_string();
    let ctx = |ctr: u64| SealCtx { kid: kid.clone(), ctr, ts: 1_760_960_000, stamp: stamp.clone(), badge_key: bk.clone() };
    // ~2 KB: whole in format 2, cut in format 1.
    let reply_2k = "All 14 checks passed; the release notes are drafted. ".repeat(38);
    let reply_long = "Release notes draft, section by section. ".repeat(120);
    let stale9: Vec<(String, i64)> =
        (0..9).map(|i| (format!("7f3c2a1e-0b4d-4c8e-9a1f-{:012x}", 0xa0 + i), 400 - i as i64)).collect();
    let cases: Vec<(&str, SealKind, serde_json::Value, String)> = vec![
        (
            "v2-alert-thread-msg-full",
            SealKind::Alert,
            e2e::alert_plaintext(&with_msg(thread("All 14 checks passed…", "Cortana"), &reply_2k), &ctx(1_760_960_000_201))
                .expect("alert"),
            "A ~2 KB reply (2014 characters): whole in format 2 (`msg.truncated: false`); format 1 would cut it.".into(),
        ),
        (
            "v2-alert-thread-msg-truncated",
            SealKind::Alert,
            e2e::alert_plaintext(&with_msg(thread("Release notes draft…", "Cortana"), &reply_long), &ctx(1_760_960_000_202))
                .expect("alert"),
            "A reply too long for the 2880-byte bucket: `msg.text` is cut at a character boundary until the JSON \
             fills the bucket and `truncated: true` (the app fetches the rest). The visible body is not cut."
                .into(),
        ),
        (
            "v2-nudge",
            SealKind::Nudge,
            e2e::nudge_plaintext_in(&badge(StaleHint::default()), &ctx(1_760_960_000_203), f),
            "A format-2 nudge with no stale chats: totals only.".into(),
        ),
        (
            "v2-nudge-stale",
            SealKind::Nudge,
            e2e::nudge_plaintext_in(
                &badge(StaleHint::of(vec![("c-77".into(), 418), ("c-9".into(), 7)])),
                &ctx(1_760_960_000_204),
                f,
            ),
            "push-content-available-v1: `data.stale` = conversationId:seq pairs, newest first (ids and seqs only).".into(),
        ),
        (
            "v2-nudge-stale-more",
            SealKind::Nudge,
            e2e::nudge_plaintext_in(&badge(StaleHint::of(stale9)), &ctx(1_760_960_000_205), f),
            "Nine stale chats: the newest 8 ride, `staleMore: \"1\"` says more changed (catch up on foreground).".into(),
        ),
    ];

    let mut out_cases = Vec::new();
    for (i, (name, kind, value, note)) in cases.into_iter().enumerate() {
        let json = match kind {
            SealKind::Alert => e2e::fit_alert_in(&value, f).expect("fits"),
            SealKind::Nudge => e2e::fit_nudge_in(&value, f).expect("fits"),
        };
        let padded = seal::pad_in(&json, kind, f).expect("pad");
        let ikm_e = [0x41u8 + i as u8; 32];
        let (sk_e, pk_e) = seal::derive_keypair(&ikm_e);
        let blob = seal::seal_with_rng_in(kind, f, &pk, &psk, &json, &mut FixedIkm(ikm_e)).expect("seal");
        assert_eq!(&blob[2..67], &pk_e[..], "enc is DeriveKeyPair(ikmE)");
        let b64 = seal::B64.encode(&blob);
        assert_eq!(seal::open_format(kind, &sk, &psk, &b64).expect("opens"), (f, json.clone()), "{name} round trip");
        let body = e2e::sealed_dispatch_body_in(
            kind,
            f,
            &[SealedEntry { platform: "apns".into(), token: "<apns-device-token>".into(), e2e: b64.clone() }],
        );
        out_cases.push(serde_json::json!({
            "name": name,
            "note": note,
            "kind": kind.wire_kind(),
            "format": f.number(),
            "infoHex": hex(kind.info()),
            "aadHex": hex(&blob[..2]),
            "ikmEHex": hex(&ikm_e),
            "ephemeralPrivateKeyHex": hex(&sk_e),
            "encHex": hex(&blob[2..67]),
            "ctr": value["ctr"],
            "ts": value["ts"],
            "plaintextJson": String::from_utf8(json.clone()).expect("utf8"),
            "plaintextJsonBytes": json.len(),
            "paddedPlaintextHex": hex(&padded),
            "ciphertextHex": hex(&blob[67..]),
            "blobHex": hex(&blob),
            "e2e": b64,
            "e2eLength": f.b64_len(kind),
            "relayDispatchBody": body,
        }));
    }

    serde_json::json!({
        "about": "K2 push-e2e-v2 test vectors: format 2 (the bigger buckets). Generated by the K2 daemon's real \
                  seal code (crates/k2-core/src/push/vectors.rs, build_v2) with the SAME recipient key and PSK as \
                  e2e_push_vectors.json and a fixed ephemeral seed per case. Identical to format 1 (suite, info, \
                  PSK, psk_id, padding rule, plaintext schema with \"v\":1) except: blob byte 0 = 0x02 (in the \
                  AAD), the alert plaintext is padded to 2880 bytes and the nudge to 1024, so a blob is 2963 / 1107 \
                  bytes = 3951 / 1476 base64url characters, and the relay's data.v is \"2\". A format-2 nudge may \
                  carry data.stale / data.staleMore. Pick the format by length; byte 0 must agree.",
        "version": 2,
        "suite": {
            "name": seal::SUITE_NAME,
            "mode": 1,
            "kemId": 16,
            "kdfId": 1,
            "aeadId": 2,
            "formatByte": seal::FORMAT_V2,
            "suiteByte": seal::SUITE_ID,
        },
        "pskId": String::from_utf8(seal::PSK_ID.to_vec()).expect("ascii"),
        "pskIdHex": hex(seal::PSK_ID),
        "info": {
            "alert": String::from_utf8_lossy(SealKind::Alert.info()),
            "alertHex": hex(SealKind::Alert.info()),
            "nudge": String::from_utf8_lossy(SealKind::Nudge.info()),
            "nudgeHex": hex(SealKind::Nudge.info()),
        },
        "sizes": {
            "alertPaddedPlaintext": f.pad_len(SealKind::Alert),
            "nudgePaddedPlaintext": f.pad_len(SealKind::Nudge),
            "alertBlobBytes": f.blob_len(SealKind::Alert),
            "nudgeBlobBytes": f.blob_len(SealKind::Nudge),
            "alertBase64urlChars": f.b64_len(SealKind::Alert),
            "nudgeBase64urlChars": f.b64_len(SealKind::Nudge),
            "encBytes": seal::ENC_LEN,
            "tagBytes": seal::TAG_LEN,
            "relayApnsAlertBodyBytes": 4072,
            "vendorPayloadMax": seal::VENDOR_PAYLOAD_MAX,
        },
        "recipient": {
            "ikmRHex": hex(&[0x11; 32]),
            "privateKeyHex": hex(&sk),
            "publicKeyHex": hex(&pk),
            "publicKeyB64url": seal::B64.encode(&pk),
            "pskHex": hex(&psk),
            "pskB64url": seal::B64.encode(&psk),
            "keyId": kid,
            "server": { "sub": stamp.subdomain, "iid": stamp.instance_id },
            "badgeKey": bk,
        },
        "registerBody": {
            "deviceId": "<per (install, server entry)>",
            "platform": "apns",
            "token": "<apns-device-token>",
            "previews": true,
            "e2e": {
                "v": 2,
                "suite": seal::SUITE_NAME,
                "publicKey": seal::B64.encode(&pk),
                "psk": seal::B64.encode(&psk),
                "createdAt": 1_760_960_000,
            },
        },
        "cases": out_cases,
    })
}

pub(crate) fn render_v2() -> String {
    serde_json::to_string_pretty(&build_v2()).expect("json") + "\n"
}

#[test]
fn vectors_v2_file_has_not_drifted() {
    let fresh = render_v2();
    if std::env::var("BLESS_E2E_PUSH_VECTORS").is_ok() {
        std::fs::write(VECTORS_V2_PATH, &fresh).expect("write vectors");
        return;
    }
    let committed = std::fs::read_to_string(VECTORS_V2_PATH).expect("committed v2 vectors file");
    assert!(
        committed == fresh,
        "tests/data/e2e_push_vectors_v2.json drifted from the seal code. If the format changed on purpose, \
         regenerate with BLESS_E2E_PUSH_VECTORS=1 and tell K2 Companion."
    );
}

/// The daemon's seal of each committed plaintext, with the case's ikmE,
/// equals the committed blob byte for byte; every blob opens as format 2,
/// has the relay's exact length, and its relay body says `v: "2"`.
#[test]
fn the_daemon_seals_the_v2_vectors_byte_for_byte() {
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(VECTORS_V2_PATH).expect("v2 vectors")).expect("json");
    let (sk, pk, psk) = recipient();
    let unhex = |s: &str| -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex")).collect()
    };
    assert_eq!(doc["sizes"]["alertPaddedPlaintext"], 2880);
    assert_eq!(doc["sizes"]["nudgePaddedPlaintext"], 1024);
    let cases = doc["cases"].as_array().expect("cases");
    assert_eq!(cases.len(), 5);
    for c in cases {
        let name = c["name"].as_str().expect("name");
        let kind = if c["kind"] == "nudge" { SealKind::Nudge } else { SealKind::Alert };
        let json = c["plaintextJson"].as_str().expect("pt").as_bytes().to_vec();
        let ikm: [u8; 32] = unhex(c["ikmEHex"].as_str().expect("ikm")).try_into().expect("32 bytes");
        let blob = seal::seal_with_rng_in(kind, seal::Format::V2, &pk, &psk, &json, &mut FixedIkm(ikm)).expect("seal");
        assert_eq!(hex(&blob), c["blobHex"].as_str().expect("blobHex"), "{name}: byte for byte");
        assert_eq!(seal::B64.encode(&blob), c["e2e"].as_str().expect("e2e"), "{name}");
        assert_eq!(blob[0], seal::FORMAT_V2, "{name}");
        let b64 = c["e2e"].as_str().expect("e2e");
        let want = if kind == SealKind::Alert { 3951 } else { 1476 };
        assert_eq!(b64.len(), want, "{name}");
        assert_eq!(seal::open_format(kind, &sk, &psk, b64).expect("opens"), (seal::Format::V2, json), "{name}");
        assert_eq!(c["relayDispatchBody"]["payload"], serde_json::json!({ "data": { "kind": kind.wire_kind(), "v": "2" } }));
    }
    let pt = |name: &str| -> serde_json::Value {
        let c = cases.iter().find(|c| c["name"] == name).expect("case");
        serde_json::from_str(c["plaintextJson"].as_str().expect("pt")).expect("json")
    };
    let full = pt("v2-alert-thread-msg-full");
    assert_eq!(full["msg"]["truncated"], false);
    assert!(full["msg"]["text"].as_str().expect("t").chars().count() > 2000);
    let cut = cases.iter().find(|c| c["name"] == "v2-alert-thread-msg-truncated").expect("case");
    let n = cut["plaintextJsonBytes"].as_u64().expect("n") as usize;
    assert!(n <= 2878 && n > 2866, "fills the 2880 bucket: {n}");
    assert_eq!(pt("v2-alert-thread-msg-truncated")["msg"]["truncated"], true);
    assert_eq!(pt("v2-nudge")["data"], serde_json::json!({ "unreadTotal": "4", "waitingTotal": "1" }));
    assert_eq!(pt("v2-nudge-stale")["data"]["stale"], "c-77:418,c-9:7");
    let more = pt("v2-nudge-stale-more");
    assert_eq!(more["data"]["staleMore"], "1");
    assert_eq!(more["data"]["stale"].as_str().expect("stale").split(',').count(), 8);
}
