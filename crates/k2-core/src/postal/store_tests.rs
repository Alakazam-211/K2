//! The Postal friend store (0311): one address per workspace, per-handle
//! friends, every request waiting for a person, blocks and the cap.

use super::*;
use crate::postal::parse_address;

fn conn() -> Connection {
    let c = Connection::open_in_memory().expect("memory");
    crate::db::run_migrations(&c).expect("migrate");
    c
}

fn a(s: &str) -> crate::postal::Address {
    parse_address(s).expect("address")
}

#[test]
fn one_address_per_workspace_and_per_server() {
    let c = conn();
    let h = claim(&c, "ws-1", &a("Scout::Initech.postal.bot"), "owner").expect("claim");
    assert_eq!((h.address.as_str(), h.status.as_str(), h.claimed_by.as_deref()), ("scout::initech.postal.bot", "local", Some("owner")));
    assert_eq!(claim(&c, "ws-1", &a("scout::initech.postal.bot"), "owner").expect("same again").id, h.id);
    assert_eq!(claim(&c, "ws-1", &a("other::initech.postal.bot"), "owner"), Err(StoreError::HasAddress("scout::initech.postal.bot".into())));
    assert_eq!(claim(&c, "ws-2", &a("scout::initech.postal.bot"), "owner"), Err(StoreError::AddressTaken));
    assert_eq!(handles(&c).len(), 1);
    assert!(handle_for_workspace(&c, "ws-2").is_none());
}

#[test]
fn requests_wait_for_a_person_both_ways() {
    let c = conn();
    let h = claim(&c, "ws-1", &a("scout::initech.postal.bot"), "owner").unwrap();
    // Outgoing: pending_out, unsent until the network half sends it.
    let out = record_outgoing(&c, &h, &a("ops::beta.postal.bot"), Some("to review PRs"), "agent:scout").unwrap();
    assert_eq!((out.status.as_str(), out.pair_id.as_deref(), out.note.as_deref()), ("pending_out", None, Some("to review PRs")));
    assert_eq!(record_outgoing(&c, &h, &a("ops::beta.postal.bot"), None, "owner").unwrap().id, out.id, "asking again is the same request");
    assert_eq!(unsent_outgoing(&c).iter().map(|f| f.id.clone()).collect::<Vec<_>>(), vec![out.id.clone()]);
    assert_eq!(record_outgoing(&c, &h, &a("scout::initech.postal.bot"), None, "owner"), Err(StoreError::SelfFriend));

    // Incoming: pending_in; the same request again is known.
    let Incoming::New(inc) = record_incoming(&c, &h, &a("lab::gamma.postal.bot"), Some("Dana"), Some("SHA256:k1"), Some("pr_1"), None).unwrap() else {
        panic!("new")
    };
    assert_eq!((inc.status.as_str(), inc.owner_name.as_deref(), inc.pinned_at), ("pending_in", Some("Dana"), None));
    assert!(matches!(record_incoming(&c, &h, &a("lab::gamma.postal.bot"), None, None, Some("pr_1"), None).unwrap(), Incoming::Known(_)));
    // Asking someone who asked first: answer theirs.
    assert_eq!(record_outgoing(&c, &h, &a("lab::gamma.postal.bot"), None, "owner"), Err(StoreError::TheyAsked(inc.id.clone())));
    assert_eq!(counts(&c, &h.id), (0, 1, 1));

    // A person accepts: the key the request carried is pinned.
    let ok = decide(&c, &inc.id, true, "owner").unwrap();
    assert_eq!((ok.status.as_str(), ok.decided_by.as_deref()), ("accepted", Some("owner")));
    assert!(ok.pinned_at.is_some());
    assert_eq!(decide(&c, &inc.id, false, "owner"), Err(StoreError::NotPending("accepted".into())));
    assert_eq!(record_outgoing(&c, &h, &a("lab::gamma.postal.bot"), None, "owner"), Err(StoreError::AlreadyFriends));
    assert_eq!(counts(&c, &h.id), (1, 0, 1));

    // Removed is history; asking again makes a new request on the same row.
    let gone = remove(&c, &h.id, "lab::gamma.postal.bot", "owner").unwrap();
    assert_eq!(gone.status, "revoked");
    let again = record_outgoing(&c, &h, &a("lab::gamma.postal.bot"), None, "owner").unwrap();
    assert_eq!((again.id.as_str(), again.status.as_str()), (inc.id.as_str(), "pending_out"));
}

#[test]
fn a_blocked_peer_is_refused_and_the_queue_has_a_cap() {
    let c = conn();
    let h = claim(&c, "ws-1", &a("scout::initech.postal.bot"), "owner").unwrap();
    let b = block(&c, &h, &a("spam::evil.postal.bot"), "owner").unwrap();
    assert_eq!(b.status, "blocked");
    assert_eq!(record_incoming(&c, &h, &a("spam::evil.postal.bot"), None, None, None, None), Err(StoreError::Blocked));
    assert_eq!(record_outgoing(&c, &h, &a("spam::evil.postal.bot"), None, "owner"), Err(StoreError::Blocked));
    // Removing the block makes it history; a new request may come.
    remove(&c, &h.id, "spam::evil.postal.bot", "owner").unwrap();
    assert!(matches!(record_incoming(&c, &h, &a("spam::evil.postal.bot"), None, None, None, None).unwrap(), Incoming::New(_)));
    for i in 1..PENDING_IN_CAP {
        record_incoming(&c, &h, &a(&format!("p{i}::evil.postal.bot")), None, None, None, None).expect("under the cap");
    }
    assert_eq!(record_incoming(&c, &h, &a("one-more::evil.postal.bot"), None, None, None, None), Err(StoreError::TooManyPending));
    // Friends are per handle: another workspace's handle has its own.
    let h2 = claim(&c, "ws-2", &a("ops::initech.postal.bot"), "owner").unwrap();
    assert!(matches!(record_incoming(&c, &h2, &a("spam::evil.postal.bot"), None, None, None, None).unwrap(), Incoming::New(_)));
    assert!(friend_by_peer(&c, &h2.id, "p1::evil.postal.bot").is_none());
}

#[test]
fn a_changed_key_waits_for_a_person_only_on_a_friend() {
    let c = conn();
    let h = claim(&c, "ws-1", &a("scout::initech.postal.bot"), "owner").unwrap();
    assert_eq!(mark_rekey(&c, &h.id, "ops::beta.postal.bot"), Err(StoreError::NotFound));
    let f = match record_incoming(&c, &h, &a("ops::beta.postal.bot"), None, Some("SHA256:old"), Some("pr_r"), None).unwrap() {
        Incoming::New(f) => f,
        other => panic!("{other:?}"),
    };
    assert_eq!(mark_rekey(&c, &h.id, "ops::beta.postal.bot"), Err(StoreError::NotPending("pending_in".into())), "a request isn't a friend yet");
    decide(&c, &f.id, true, "owner").unwrap();
    let r = mark_rekey(&c, &h.id, "ops::beta.postal.bot").unwrap();
    assert_eq!((r.status.as_str(), r.peer_fingerprint.as_deref()), ("rekey_pending", Some("SHA256:old")), "the old key stays until a person accepts");
    assert_eq!(mark_rekey(&c, &h.id, "ops::beta.postal.bot").unwrap().status, "rekey_pending", "twice is the same");
    assert_eq!(decide(&c, &f.id, true, "owner").unwrap().status, "accepted");
}
