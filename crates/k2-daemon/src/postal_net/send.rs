//! Send (prd-postal-in-k2-v1 S5): `k2 msg <handle>::<label>.postal.bot`
//! through `POST /cli/postal/send`.
//!
//! - **The sender is the credential's**: an agent sends as its own
//!   workspace's address (its passport); a person names the workspace.
//!   Never a `from` in the body.
//! - **Only to a friend** of that address, with its key pinned.
//! - **The redaction backstop**: a text holding a vault secret's value is
//!   refused (`postal_secret_in_text`); secrets go by `k2 vault send`.
//! - **256 KiB** at most.
//! - **The SDK outbox**: one file per message (0600). Each try signs the
//!   message again with the same id and a fresh `sent_at` (a copy older
//!   than 300 s is refused live as stale; the receiver dedupes on the id).
//! - **Hold only for now**: K2 seals to the friend's pinned key and parks
//!   it on the plane. Live delivery (`POST https://<label>.postal.bot/postal/v2/msg`)
//!   is S6, which waits on the plane routing K2 boxes' `<label>.postal.bot`
//!   names (K2 Web's P-6). Every live reply goes through the SDK's
//!   `Item::on_live` ([`on_live`]), which follows the wire v2 reply table:
//!   a 429 from a receiver that is up but busy is **hold now**, like a 503,
//!   a refused connection or a 404; a timeout, a 408 or another 5xx retries
//!   live with backoff.
//! - The answer carries the badge of the copy the plane holds and a status:
//!   `queued` (not on the plane yet), `held`, `delivered` (the receiving
//!   host stored it), `failed` (with the reason).

use k2_core::postal::files::{self, Outgoing};
use k2_core::postal::sdk::address::Address;
use k2_core::postal::sdk::badge::{badge, envelope_digest, sha256, Envelope};
use k2_core::postal::sdk::hold;
use k2_core::postal::sdk::host::Mode;
use k2_core::postal::sdk::outbox::{DeadReason, Item, State, Step};
use k2_core::postal::sdk::pairing::PublicKey;
use k2_core::postal::sdk::plane::{self, HoldPutOk};
use k2_core::postal::sdk::wire::{self, Draft};
use k2_core::postal::store;
use serde_json::{json, Value};

use super::plane::{CallError, Plane};
use super::receive::MAX_MESSAGE;

const CONTENT_TYPE: &str = "text/plain; charset=utf-8";

fn db<T>(f: impl FnOnce(&rusqlite::Connection) -> T) -> T {
    let shared = k2_core::db::shared();
    let conn = shared.lock();
    f(&conn)
}

/// Why a send was refused before anything was queued.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    pub status: &'static str,
    pub code: &'static str,
    pub hint: String,
}

fn refused(status: &'static str, code: &'static str, hint: impl Into<String>) -> Refused {
    Refused { status, code, hint: hint.into() }
}

/// The redaction backstop: does `text` hold a vault secret's value?
pub fn holds_secret(text: &str, redact: impl Fn(&str) -> String) -> bool {
    redact(text) != text
}

/// Check and queue one message from `h` (the caller's own address).
pub fn queue(h: &store::Handle, to: &str, text: &str, wake: bool, actor: &str, sender_ws: Option<&str>) -> Result<Outgoing, Refused> {
    let Some(peer) = k2_core::postal::parse_address(to) else {
        return Err(refused("400 Bad Request", "bad_postal_address", "to must be a Postal address: handle::label.postal.bot"));
    };
    let peer = peer.as_string();
    let text = text.trim_end();
    if text.trim().is_empty() {
        return Err(refused("400 Bad Request", "usage", "say something: the text is empty"));
    }
    if text.len() > MAX_MESSAGE {
        return Err(refused("413 Payload Too Large", "too_large", "a Postal message is at most 256 KiB"));
    }
    if holds_secret(text, k2_core::redaction::redact) {
        return Err(refused(
            "403 Forbidden",
            "postal_secret_in_text",
            format!(
                "this text holds the value of a secret in this server's vault, so it isn't sent: send secrets with {} vault send, never in a message",
                k2_core::cli_name!()
            ),
        ));
    }
    let friend = db(|c| store::friend_by_peer(c, &h.id, &peer));
    match friend.as_ref().map(|f| f.status.as_str()) {
        Some("accepted") => {}
        Some("rekey_pending") => {
            return Err(refused("409 Conflict", "postal_rekey_pending", "this friend's key changed: a person accepts the new key in the Postal tab first"))
        }
        _ => {
            return Err(refused(
                "403 Forbidden",
                "postal_not_friends",
                format!("{peer} isn't a Postal friend of {}: ask first with {} postal invite {peer} --why \"…\"", h.address, k2_core::cli_name!()),
            ))
        }
    }
    let (Ok(from), Ok(to_addr)) = (Address::parse(&h.address), Address::parse(&peer)) else {
        return Err(refused("400 Bad Request", "bad_postal_address", "not a Postal address"));
    };
    let _ = from;
    let id = k2_core::postal::new_id();
    let now = super::pairing::now();
    let mut item = Item::new(&id, to_addr, now);
    // S6 (live) waits on the plane's P-6: every message goes by hold.
    item.hold_next = true;
    // The sender's own record (`m_`), for `k2 msg verify` and status.
    let record_id = k2_core::hire::deliveries::new_id();
    let rec = k2_core::hire::deliveries::Delivery {
        id: record_id.clone(),
        sender_workspace_id: sender_ws.map(str::to_string),
        sender_label: Some(h.address.clone()),
        via: "postal".into(),
        actor: Some(actor.to_string()),
        target: peer.clone(),
        body_sha256: Some(k2_core::hire::deliveries::body_sha256(text)),
        sender_address: Some(h.address.clone()),
        landed: Some(json!({ "outgoing": id, "state": "queued" }).to_string()),
        ..Default::default()
    };
    db(|c| k2_core::hire::deliveries::record(c, &rec)).map_err(|e| refused("500 Internal Server Error", "internal", e))?;
    let o = Outgoing {
        item,
        handle_id: h.id.clone(),
        workspace_id: h.workspace_id.clone(),
        from: h.address.clone(),
        body: text.to_string(),
        content_type: CONTENT_TYPE.into(),
        wake,
        tray: false,
        badge: None,
        record_id,
    };
    files::save_outgoing(&o).map_err(|e| refused("500 Internal Server Error", "internal", e))?;
    Ok(o)
}

/// The badge of one signed copy (the SDK's formula over its envelope).
fn badge_of(m: &wire::WireMessage) -> Option<String> {
    let from = m.from.to_string();
    let to = m.to.to_string();
    let d = envelope_digest(&Envelope {
        version: m.v,
        id: &m.id,
        from: &from,
        to: &to,
        sent_at: m.sent_at,
        mode: m.mode,
        content_type: &m.content_type,
        files: &[],
        body_sha256: sha256(m.body.as_bytes()),
    })
    .ok()?;
    Some(badge(&d))
}

fn jitter() -> f64 {
    let mut b = [0u8; 2];
    let _ = getrandom::getrandom(&mut b);
    f64::from(u16::from_le_bytes(b)) / 65_536.0
}

/// Live delivery (S6) waits on the plane's P-6; until then every message
/// goes by hold.
const LIVE_SEND: bool = false;

/// One live try's reply, fed to the SDK's outbox: it decides hold now
/// (429, 503 `host_down`, refused, 404), retry live with backoff (timeout,
/// 408, other 5xx) or final (other 4xx). K2 never second-guesses it.
pub fn on_live(o: &mut Outgoing, reply: &k2_core::postal::sdk::outbox::Live, now: i64) {
    o.item.on_live(reply, now, jitter());
}

/// The state word the answer and the record carry.
pub fn state_word(o: &Outgoing) -> &'static str {
    match &o.item.state {
        State::Queued => "queued",
        State::Held { .. } => "held",
        State::Delivered => "delivered",
        State::Dead { .. } => "failed",
    }
}

fn reason_of(o: &Outgoing) -> Option<String> {
    match &o.item.state {
        State::Dead { why } => Some(match why {
            DeadReason::Expired => "expired".into(),
            DeadReason::Refused { code } => format!("refused:{}", serde_json::to_value(code).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()),
            DeadReason::Plane { code } => code.clone(),
            DeadReason::Live { status } => format!("live_{status}"),
        }),
        _ => o.item.last_error.clone(),
    }
}

/// The send answer for an outgoing message.
pub fn answer(o: &Outgoing) -> Value {
    json!({
        "success": !matches!(o.item.state, State::Dead { .. }),
        "id": o.item.id,
        "deliveryId": o.record_id,
        "badge": o.badge,
        "from": o.from,
        "to": o.item.to.to_string(),
        "status": state_word(o),
        "reason": reason_of(o),
        "via": "postal",
        "live": "not_yet",
    })
}

fn save(o: &Outgoing) {
    let landed = json!({ "outgoing": o.item.id, "state": state_word(o), "badge": o.badge, "reason": reason_of(o) }).to_string();
    let delivered = matches!(o.item.state, State::Delivered);
    let _ = db(|c| k2_core::postal::seen::set_landed(c, &o.record_id, &landed, delivered));
    if o.item.is_done() {
        files::remove_outgoing(o);
    } else if let Err(e) = files::save_outgoing(o) {
        k2_core::log_debug!("[postal] outbox {}: {e}", o.item.id);
    }
}

/// Move one outgoing message as far as it goes now.
pub fn advance(net: &Plane, o: &mut Outgoing) -> Result<(), CallError> {
    for _ in 0..4 {
        let now = super::pairing::now();
        // Removed or blocked meanwhile: dead-lettered, never sent.
        let friend = db(|c| store::friend_by_peer(c, &o.handle_id, &o.item.to.to_string()));
        if !matches!(o.item.state, State::Delivered | State::Dead { .. } | State::Held { .. }) && friend.as_ref().map(|f| f.status.as_str()) != Some("accepted") {
            o.item.state = State::Dead { why: DeadReason::Plane { code: "not_friends".into() } };
            save(o);
            return Ok(());
        }
        match o.item.next(now) {
            Step::Done => {
                save(o);
                return Ok(());
            }
            Step::Wait { .. } => {
                save(o);
                return Ok(());
            }
            Step::Receipt { at } if at > now => {
                save(o);
                return Ok(());
            }
            Step::Receipt { .. } => {
                let req = plane::hold_sent(std::slice::from_ref(&o.item.id)).map_err(CallError::Plane)?;
                let (s, b) = net.box_call(&req)?;
                let rc = plane::parse_receipts(s, &b).map_err(CallError::Plane)?;
                match rc.get(&o.item.id) {
                    Some(r) => o.item.on_receipt(r, now),
                    None => o.item.on_receipt(&plane::Receipt::Other("unknown".into()), now),
                }
                save(o);
                if matches!(o.item.state, State::Held { .. }) {
                    return Ok(());
                }
            }
            Step::Live => {
                // Not in this release (S6): hold instead.
                debug_assert!(!LIVE_SEND, "live send is S6: its replies go through on_live");
                o.item.hold_next = true;
            }
            Step::Hold => {
                let Some(f) = friend.as_ref() else { return Ok(()) };
                let pem = db(|c| store::peer_key_pem(c, &f.id)).ok_or_else(|| CallError::NoConnect("friend key missing".into()))?;
                let key = PublicKey::from_pem(&pem).map_err(|_| CallError::NoConnect("friend key unreadable".into()))?;
                let ident = k2_core::postal::keys::identity(&o.from)
                    .map_err(CallError::NoConnect)?
                    .ok_or_else(|| CallError::NoConnect(format!("{} has no Postal key", o.from)))?;
                let (from, to) = (Address::parse(&o.from).map_err(|_| CallError::NoConnect("from".into()))?, o.item.to.clone());
                let msg = wire::sign(
                    &ident,
                    &ident.public_key(),
                    &Draft {
                        id: &o.item.id,
                        from: &from,
                        to: &to,
                        sent_at: now,
                        mode: if o.tray { Mode::Tray } else { Mode::Live },
                        content_type: &o.content_type,
                        body: &o.body,
                        files: &[],
                        wake: o.wake,
                    },
                )
                .map_err(|e| CallError::NoConnect(format!("sign: {e}")))?;
                let sealed = hold::seal(&key, &msg).map_err(|e| CallError::NoConnect(format!("seal: {e}")))?;
                let req = plane::hold_put(&hold::Outer::of(&msg), &sealed).map_err(CallError::Plane)?;
                let (s, b) = net.box_call(&req)?;
                match plane::parse::<HoldPutOk>(s, &b) {
                    Ok(ok) => {
                        // First write wins on the plane: the badge is the
                        // copy it keeps (a duplicate keeps the earlier one).
                        if !ok.duplicate || o.badge.is_none() {
                            o.badge = badge_of(&msg);
                        }
                        o.item.on_hold(Ok(&ok), now, jitter());
                    }
                    Err(e) => o.item.on_hold(Err(&e), now, jitter()),
                }
                save(o);
                // Ask for the receipt at once: the receiver may be up.
                o.item.next_at = now;
            }
        }
    }
    save(o);
    Ok(())
}

/// Advance every queued message (the loop's tick).
pub fn advance_all(net: &Plane) -> Result<(), CallError> {
    for mut o in files::outgoing() {
        if let Err(e) = advance(net, &mut o) {
            if e.is_retryable() {
                return Err(e);
            }
            k2_core::log_debug!("[postal] outbox {}: {}", o.item.id, e.code());
        }
    }
    Ok(())
}

/// A receipt event for `client_id`: advance that message now.
pub fn on_receipt_event(net: &Plane, client_id: &str) {
    if let Some(mut o) = files::outgoing_by_id(client_id) {
        o.item.next_at = 0;
        if let State::Held { .. } = o.item.state {
            let _ = advance(net, &mut o);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outgoing() -> Outgoing {
        Outgoing {
            item: Item::new("01ARZ3NDEKTSV4RRFFQ69G5FAV", Address::parse("scout::bravo.postal.bot").unwrap(), 1_800_000_000),
            handle_id: "h".into(),
            workspace_id: "w".into(),
            from: "alice::acme.postal.bot".into(),
            body: "hi".into(),
            content_type: CONTENT_TYPE.into(),
            wake: true,
            tray: false,
            badge: None,
            record_id: "m_aaaaaaaaaa".into(),
        }
    }

    #[test]
    fn a_live_429_holds_now_and_a_timeout_retries_live() {
        use k2_core::postal::sdk::outbox::Live;
        let t = 1_800_000_000;
        let mut busy = outgoing();
        on_live(&mut busy, &Live::Status(429, r#"{"error":{"code":"rate_limited","message":"","retry_after_s":30}}"#.into()), t);
        assert_eq!(busy.item.next(t), Step::Hold, "a receiver that is up but busy: hold now");
        let mut down = outgoing();
        on_live(&mut down, &Live::Status(503, r#"{"error":{"code":"host_down","message":""}}"#.into()), t);
        assert_eq!(down.item.next(t), Step::Hold);
        let mut slow = outgoing();
        on_live(&mut slow, &Live::Transport("timed out".into()), t);
        assert!(matches!(slow.item.next(t), Step::Wait { .. }), "a timeout retries live later");
        let mut refused = outgoing();
        on_live(&mut refused, &Live::Status(403, r#"{"error":{"code":"not_connected","message":""}}"#.into()), t);
        assert_eq!((refused.item.next(t), state_word(&refused)), (Step::Done, "failed"));
    }

    #[test]
    fn the_backstop_refuses_a_text_with_a_vault_value() {
        let redact = |s: &str| s.replace("hunter2-very-secret", "[k2v:DB_PASSWORD]");
        assert!(holds_secret("the password is hunter2-very-secret", redact));
        assert!(!holds_secret("nothing secret here", redact));
    }
}
