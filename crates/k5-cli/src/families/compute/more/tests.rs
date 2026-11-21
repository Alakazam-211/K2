use super::trust::{self, Ask, List};
use super::writes::Words;

fn post(path: &'static str, body: &str, tcp: bool, long: bool) -> Words<Ask> {
    Words::Ask(Ask::Post {
        path,
        body: body.to_string(),
        tcp,
        long,
    })
}

#[test]
fn the_roster_words() {
    assert_eq!(
        trust::parse("roster", &[]),
        Words::Ask(Ask::Get {
            path: "/cli/compute/roster",
            query: String::new(),
            list: List::Roster,
        })
    );
    assert_eq!(trust::parse("roster", &["key"]), Words::Ask(Ask::RosterKey));
    assert_eq!(
        trust::parse(
            "roster",
            &["attest", "mini", "--site", "s1", "--why", "a", "--reason", "b"]
        ),
        post(
            "/cli/compute/roster/propose",
            r#"{"op": "set", "node": "mini", "site": "s1", "reason": "b"}"#,
            false,
            false
        )
    );
    assert_eq!(
        trust::parse("roster", &["set", "mini", "--host"]),
        Words::ShiftFailed("--host".into())
    );
    assert_eq!(
        trust::parse("roster", &["set", "--host", "h"]),
        Words::Usage(trust::ROSTER_SET_USAGE.to_string())
    );
    assert_eq!(
        trust::parse("roster", &["publish", "x", "--resign"]),
        post(
            "/cli/compute/roster/publish",
            r#"{"resign": true}"#,
            false,
            true
        )
    );
    assert_eq!(
        trust::parse("roster", &["publish"]),
        post("/cli/compute/roster/publish", "{}", false, true)
    );
    assert_eq!(
        trust::parse("roster", &["bogus"]),
        Words::Usage(trust::ROSTER_USAGE.to_string())
    );
}

#[test]
fn the_seats_and_trust_words() {
    assert_eq!(
        trust::parse("seats", &["request", "box", "--secrets", "--no-secrets"]),
        post(
            "/cli/compute/seats/request",
            r#"{"owner": "box", "secrets": false}"#,
            false,
            false
        )
    );
    assert_eq!(
        trust::parse("seats", &["offer", "--nodes", "a"]),
        Words::Usage(trust::SEATS_OFFER_USAGE.to_string())
    );
    // `seats end --why x`: `--why` is the seat, then `x` is a word the
    // shell doesn't know.
    assert_eq!(
        trust::parse("seats", &["end", "--why", "x"]),
        Words::Usage(format!(
            "usage: {} compute seats end <seat> [--why TEXT]",
            k2_client::brand::CLI_NAME
        ))
    );
    assert_eq!(
        trust::parse("trust", &["--all"]),
        Words::Ask(Ask::Get {
            path: "/cli/compute/trust/requests",
            query: "state=all".into(),
            list: List::Requests,
        })
    );
    assert_eq!(
        trust::parse("trust", &["deny", "--why", "x"]),
        post(
            "/cli/compute/trust/deny",
            r#"{"request": "--why"}"#,
            false,
            false
        )
    );
    assert_eq!(
        trust::parse("owners", &["consent", "acme", "maybe"]),
        Words::Usage(trust::CONSENT_USAGE.to_string())
    );
    assert_eq!(
        trust::parse("owners", &["consent", "", "off"]),
        post(
            "/cli/compute/owners/consent",
            r#"{"on": false}"#,
            false,
            false
        )
    );
}
