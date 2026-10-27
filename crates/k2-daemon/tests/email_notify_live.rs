//! Email Notifications LIVE check (prd-email-notifications-core-v1 N5):
//! the pinned Stalwart, for real, inside the N0 spike's loopback-only
//! network namespace on k2-soak. Ignored by default; never part of CI.
//!
//! ```text
//! ssh k2-soak /root/n0-20261026/setup.sh          # Stalwart + the sink in netns k2n0
//! set -a; . /root/n0-20261026/n0.env; set +a
//! ip netns exec k2n0 ./email_notify_live-<hash> --ignored email_notify_live
//! ssh k2-soak /root/n0-20261026/teardown.sh
//! ```
//!
//! It creates a sender account the way K2 does (no credential, logins
//! off), checks Stalwart refuses a login as it, submits one notification by
//! delegation with `RET=HDRS` (to the netns sink: no packet can leave),
//! checks the sink copy is `multipart/alternative` and DKIM-signed for the
//! sender's domain, and retires the account. It panics when
//! `K2_N0_JMAP_URL` is missing: a live test that silently skips proves
//! nothing.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use k2_daemon::email_notify::compose;
use k2_daemon::mail::jmap::StalwartClient;

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is required: source n0.env (setup.sh) and run inside `ip netns exec k2n0`"))
}

#[test]
#[ignore]
fn email_notify_live() {
    let url = env("K2_N0_JMAP_URL");
    let key = std::fs::read_to_string(env("K2_N0_SERVICE_KEY_FILE")).expect("service key file").trim().to_string();
    let domain = env("K2_N0_NOTIFY_DOMAIN");
    let domain_id = env("K2_N0_NOTIFY_DOMAIN_ID");
    let sink = std::path::PathBuf::from(env("K2_N0_SINK_DIR"));
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock").as_secs();
    let local = format!("k2live{stamp}");
    let address = format!("{local}@{domain}");
    let c = StalwartClient::new(url.clone(), key);

    // E3: the account K2 creates for a sender.
    let id = c.sender_account_create(&local, &domain_id).expect("sender account");
    let got = c.account_get_permissions(std::slice::from_ref(&id)).expect("read back");
    let off = &got[0]["permissions"]["disabledPermissions"];
    for p in ["authenticate", "imapAuthenticate", "pop3Authenticate", "emailSend"] {
        assert_eq!(off[p], serde_json::json!(true), "{p} is off: {}", got[0]);
    }
    // The fence on Stalwart's side: nobody logs in as a sender.
    let login = reqwest::blocking::Client::new()
        .get(format!("{url}/jmap/session"))
        .basic_auth(&address, Some("any-password"))
        .timeout(Duration::from_secs(10))
        .send()
        .expect("jmap session");
    assert!(matches!(login.status().as_u16(), 401 | 403), "login as a sender refused: {}", login.status());

    // E8: one notification by delegation, headers-only DSNs, no copy kept.
    let subject = format!("K2 live check {local}");
    let mid = format!("{local}@{domain}");
    let to = vec!["user@sink.example.net".to_string()];
    let headers: BTreeMap<String, String> = [("X-Live-Check".to_string(), "1".to_string())].into_iter().collect();
    let email = compose::email_json(&compose::Message {
        from_name: "K2 Live",
        from: &address,
        to: &to,
        cc: &[],
        subject: &subject,
        text: Some("plain part"),
        html: Some("<p>html part</p>"),
        reply_to: &[],
        headers: &headers,
        message_id: &mid,
    });
    c.submission_send_envelope(&id, &address, &to, email, Some(&compose::envelope_params("live-1")), true).expect("submitted");

    // The sink (inside the netns) gets it, signed for the sender's domain.
    let deadline = Instant::now() + Duration::from_secs(60);
    let found = loop {
        let hit = std::fs::read_dir(&sink)
            .expect("sink dir")
            .flatten()
            .filter_map(|e| std::fs::read_to_string(e.path()).ok())
            .find(|t| t.contains(&subject));
        if let Some(t) = hit {
            break t;
        }
        assert!(Instant::now() < deadline, "the message never reached the sink");
        std::thread::sleep(Duration::from_millis(500));
    };
    assert!(found.contains("multipart/alternative"), "{found}");
    assert!(found.contains("DKIM-Signature") && found.contains(&format!("d={domain}")), "DKIM for the sender's domain: {found}");
    assert!(found.contains(&mid), "K2's Message-ID: {found}");

    // E26: removing the sender rename-retires its account.
    c.account_disable(&id).expect("retired");
}
