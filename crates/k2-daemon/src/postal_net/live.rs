//! Live receive, `POST /postal/v2/msg` (prd-postal-in-k2-v1 S6; postal-bot
//! `docs/postal-wire-v2.md`): **not mounted in this release.** A K2 box's
//! `<label>.postal.bot` name has to reach the box's own K2 tunnel first,
//! and that routing is the plane's P-6 (K2 Web). Until then every Postal
//! message to a K2 box comes by hold.
//!
//! What is here is the receiver the listener will run, in the wire v2
//! order. The v2 route has no hop proof (the `p5-msg-v1` proof belongs to
//! p5's old `POST /p5/msg`, which K2 never serves); the sender's signature
//! inside the body is the authentication:
//! 1. the size of the bytes read (413 `too_large`), then the JSON (400
//!    `bad_wire`);
//! 2. the recipient must be a handle this box serves (403 `not_connected`);
//! 3. **`wire::precheck`** (version, recipient, time, shape) before the
//!    sender is looked up, so a junk message gets the same answer whether
//!    or not `from` is a friend (400 `bad_wire` / `stale`);
//! 4. the sender and its signature as **one** answer: an unknown sender and
//!    a bad signature are the same 403 `not_connected`;
//! 5. the seen insert and the store in one transaction (the delivery row,
//!    as for held mail): a replay is 200 `{already: true}`, nothing stored.
//!
//! No plane call is made on any of it (F5).

// Not mounted until S6 (above); its tests run it.
#![cfg_attr(not(test), allow(dead_code))]

use k2_core::postal::sdk::host::{Arrival, DeliverError, RefuseCode};
use k2_core::postal::sdk::pairing::PublicKey;
use k2_core::postal::sdk::wire::{self, Expect, WireError};
use k2_core::postal::seen::{Pending, TxSeen};
use k2_core::postal::store;
use serde_json::{json, Value};

/// The route the listener will serve.
pub const MSG_PATH: &str = "/postal/v2/msg";

fn refusal(status: u16, code: &str) -> (u16, Value) {
    let message = match code {
        "not_connected" => "not connected",
        "stale" => "the message's time is outside the allowed window",
        "too_large" => "message too large",
        "host_down" => "this server can't take the message now",
        _ => "not a wire v2 message",
    };
    (status, json!({ "error": { "code": code, "message": message } }))
}

/// The one answer for an unknown recipient, an unknown sender and a bad
/// signature: the same status and body.
fn not_connected() -> (u16, Value) {
    refusal(403, "not_connected")
}

fn checked(e: &WireError) -> (u16, Value) {
    match e {
        WireError::Stale => refusal(400, "stale"),
        WireError::TooLarge => refusal(413, "too_large"),
        WireError::NotForUs => not_connected(),
        _ => refusal(400, "bad_wire"),
    }
}

/// Receive one live message (the body bytes as read) at `now`. Returns the
/// HTTP status and JSON body to answer, and the delivery to land.
pub fn receive(bytes: &[u8], now: i64) -> ((u16, Value), Option<String>) {
    if bytes.len() > wire::MAX_WIRE_JSON {
        return (refusal(413, "too_large"), None);
    }
    let msg = match wire::decode_json(bytes) {
        Ok(m) => m,
        Err(e) => return (checked(&e), None),
    };
    let to = msg.to.to_string();
    let Some(h) = k2_core::db::read(|c| store::handle_by_address(c, &to)).filter(|h| h.status == "claimed") else {
        return (not_connected(), None);
    };
    if let Err(e) = wire::precheck(&msg, &msg.to, now, Arrival::Live) {
        return (checked(&e), None);
    }
    // Only now the sender: a friend of THIS handle, with a pinned key.
    let from = msg.from.to_string();
    let pinned = k2_core::db::read(|c| {
        let f = store::friend_by_peer(c, &h.id, &from).filter(|f| f.status == "accepted")?;
        let pem = store::peer_key_pem(c, &f.id)?;
        Some((f, pem))
    });
    let Some((friend, pem)) = pinned else { return (not_connected(), None) };
    let Ok(sender) = PublicKey::from_pem(&pem) else { return (not_connected(), None) };
    // This box's own key must be the one it registered (a local fault: the
    // sender holds the message and tries later).
    let mine = k2_core::postal::keys::identity(&h.address).ok().flatten();
    if mine.as_ref().map(|i| i.fingerprint().to_hex()) != super::pairing::registered_fingerprint(&h) {
        return (refusal(503, "host_down"), None);
    }
    let Some(ws) = crate::postal_routes::ws_of_id(&h.workspace_id) else { return (not_connected(), None) };
    let delivery_id = k2_core::hire::deliveries::new_id();
    let shared = k2_core::db::shared();
    let conn = shared.lock();
    let Ok(tx) = conn.unchecked_transaction() else { return (refusal(503, "host_down"), None) };
    let mut seen = TxSeen::new(&tx, Pending { delivery_id: delivery_id.clone(), target_workspace_id: ws.id.clone(), target: ws.path.clone() });
    let v = match wire::verify(&msg, &Expect { sender: &sender, to: &msg.to, now, arrival: Arrival::Live }, &mut seen) {
        Ok(v) => v,
        Err(WireError::Replayed) => return ((200, json!({ "ok": true, "already": true })), None),
        Err(WireError::BadSig) => return (not_connected(), None),
        Err(WireError::Store(_)) => return (refusal(503, "host_down"), None),
        Err(e) => return (checked(&e), None),
    };
    match super::receive::deliver(&tx, &v, &h, &friend, &ws, &delivery_id, false) {
        Ok(()) => {}
        Err(DeliverError::Refuse(RefuseCode::TooLarge)) => return (refusal(413, "too_large"), None),
        Err(DeliverError::Refuse(_)) => return (not_connected(), None),
        Err(DeliverError::Retry { .. }) => {
            k2_core::postal::files::remove_inbound(&delivery_id);
            return (refusal(503, "host_down"), None);
        }
    }
    if tx.commit().is_err() {
        k2_core::postal::files::remove_inbound(&delivery_id);
        return (refusal(503, "host_down"), None);
    }
    ((200, json!({ "ok": true, "already": false })), Some(delivery_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::postal_net::receive::tests::{befriend, now, rows_for, setup};
    use k2_core::postal::sdk::address::Address;
    use k2_core::postal::sdk::host::Mode;
    use k2_core::postal::sdk::identity::Identity;
    use k2_core::postal::sdk::wire::Draft;

    fn signed(from: &Identity, from_addr: &str, to: &str, sent_at: i64, body: &str) -> (Vec<u8>, String) {
        let id = k2_core::postal::new_id();
        let (fa, ta) = (Address::parse(from_addr).unwrap(), Address::parse(to).unwrap());
        let m = wire::sign(
            from,
            &from.public_key(),
            &Draft { id: &id, from: &fa, to: &ta, sent_at, mode: Mode::Live, content_type: "text/plain; charset=utf-8", body, files: &[], wake: true },
        )
        .unwrap();
        (wire::encode_json(&m).unwrap(), id)
    }

    #[test]
    fn precheck_comes_first_and_an_unknown_sender_reads_like_a_bad_signature() {
        let b = setup();
        let alice = Identity::from_seed([0x51; 32]);
        let mallory = Identity::from_seed([0x52; 32]);
        befriend(&b, "alice::bravo.postal.bot", &alice);
        // A friend's message: stored once; again: already.
        let (good, id) = signed(&alice, "alice::bravo.postal.bot", &b.scout_addr, now(), "live hello");
        let ((s, v), landed) = receive(&good, now());
        assert_eq!((s, v["already"].as_bool()), (200, Some(false)), "{v}");
        assert!(landed.is_some());
        assert_eq!(rows_for(&id), 1);
        let ((s, v), landed) = receive(&good, now());
        assert_eq!((s, v["already"].as_bool(), landed), (200, Some(true), None), "{v}");
        assert_eq!(rows_for(&id), 1, "no second copy");
        // The same answer, byte for byte, for a stranger and a forgery.
        let (stranger, _) = signed(&mallory, "mallory::evil.postal.bot", &b.scout_addr, now(), "hi");
        let (forged, forged_id) = signed(&mallory, "alice::bravo.postal.bot", &b.scout_addr, now(), "I am alice");
        let a = receive(&stranger, now()).0;
        let f = receive(&forged, now()).0;
        assert_eq!(a, f, "no friendship oracle");
        assert_eq!(a.0, 403);
        assert_eq!(rows_for(&forged_id), 0, "a forgery never takes an id");
        // Stale is answered before the sender is looked up: the same for a
        // stranger and a friend.
        let (old_friend, _) = signed(&alice, "alice::bravo.postal.bot", &b.scout_addr, now() - 3_600, "old");
        let (old_stranger, _) = signed(&mallory, "mallory::evil.postal.bot", &b.scout_addr, now() - 3_600, "old");
        assert_eq!(receive(&old_friend, now()).0, receive(&old_stranger, now()).0);
        assert_eq!(receive(&old_friend, now()).0 .1["error"]["code"], "stale");
        // Not ours, junk, too big.
        let (elsewhere, _) = signed(&alice, "alice::bravo.postal.bot", "nobody::acme.postal.bot", now(), "x");
        assert_eq!(receive(&elsewhere, now()).0, a, "an unknown recipient reads the same too");
        assert_eq!(receive(b"{not json", now()).0 .1["error"]["code"], "bad_wire");
        assert_eq!(receive(&vec![b' '; wire::MAX_WIRE_JSON + 1], now()).0 .0, 413);
    }
}
