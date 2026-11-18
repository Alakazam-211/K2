//! The SDK's human gates, K2's side (prd-postal-in-k2-v1 §5.1, §5.5).
//!
//! postal-sdk's `HumanGate` (`friend_request`, `rekey`) only **records and
//! returns**: a person answers later, and the host calls back into the SDK
//! with the answer and the pairing code. K2's person is a permission card.
//!
//! **What is real today and what waits.** K2 doesn't link postal-sdk yet:
//! the pin waits on **SDK wire v2 (S1, Postal Bot)**, and the requests
//! themselves come from the **plane's pair API (P-1, P-2, P-3, P-7, K2
//! Web)**. Until both exist:
//! - [`HumanGate`], [`FriendRequest`] and [`RekeyPending`] mirror the SDK's
//!   types (postal-sdk `host.rs` at postal-bot `540d688`), so the daemon's
//!   implementation (`postal_routes::K2Gate`) is the one the SDK will call;
//!   nothing calls it outside tests;
//! - the pairing code ([`set_code`]) is **this box's** six digits from the
//!   SDK's SAS v2 pairing (`FriendRequest::show_code`), for the owner to
//!   read **out** to the other side. It is never shown to an agent (not on
//!   a card, not in `friends`, not in a log): the person types the code
//!   shown on the **other** box, and the SDK's `Handshake::confirm` checks
//!   it. K2 can't check it without the SDK's handshake, so a request that
//!   carries a code answers [`CODE_NOT_YET`] on accept in this release. It
//!   lives here, in memory, like the SDK's pairing state: a restart loses
//!   it, and the pairing starts again;
//! - every person's answer is queued with [`record_answer`] for the network
//!   half to send ([`take_answers`]). The friend row is the durable record;
//!   this queue is the callback seam. [`network_available`] is `false`, and
//!   nothing here pretends a request was sent.
//!
//! **Rules for the S3–S5 wiring** (K2's review of postal-sdk wire v2 at
//! postal-bot `80d13e5ccd489b776c6e93c2d2403cf2f6146ef3`, branch
//! `feat/sdk-s1-wire-v2`; the pin waits on the Steward's OK and is not in
//! this release):
//! 1. K2's `SeenStore` **is** the insert on `msg_deliveries`' unique
//!    `postal_msg_id` index, in the same transaction as storing the
//!    message. That is what makes the SDK's `disposition()` Ack on a replay
//!    safe; never a separate seen table or a second write.
//! 2. On live delivery (`/p5/msg`, S6) verify the `p5-msg-v1` hop proof
//!    before anything else.
//! 3. Stop the hold pull loop when `next_after` doesn't advance (a plane
//!    that returns the same cursor must not spin the daemon).
//! 4. Check the box's own key (the handle's key in the vault) is the one
//!    the message is sealed to before calling `open`.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use serde::Serialize;

/// Is K2 connected to the Postal network? **Not in this release**: native
/// pairing, receive and send wait on SDK wire v2 (S1) and the plane's
/// P-1..P-8. Every answer that would go out says so instead.
pub fn network_available() -> bool {
    false
}

/// The machine-readable state every "would go out" answer carries while
/// [`network_available`] is false.
pub const NOT_YET: &str = "not_available_yet";

/// Accepting a request that carries a pairing code: not in this release.
pub const CODE_NOT_YET: &str = "postal_code_check_not_available";

/// An incoming friend request, waiting for a person (the SDK's
/// `FriendRequest`, plus what K2 shows on the card).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FriendRequest {
    pub pair_id: String,
    /// The local handle's address (`scout::acme.postal.bot`).
    pub local_handle: String,
    pub peer: String,
    pub peer_owner_name: Option<String>,
    /// The peer key's fingerprint, pinned when a person accepts.
    pub fingerprint: Option<String>,
    /// This box's SAS v2 pairing code (six digits, to read out), when the
    /// pairing has one. People only.
    pub code: Option<String>,
    /// The peer's note.
    pub note: Option<String>,
}

/// A friend's key changed on the plane, waiting for a person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RekeyPending {
    pub local_handle: String,
    pub peer: String,
    pub old_fingerprint: String,
    pub new_fingerprint: String,
}

/// Human gates record and return; they never decide (postal-sdk
/// `HumanGate`).
pub trait HumanGate {
    fn friend_request(&self, req: &FriendRequest);
    fn rekey(&self, pending: &RekeyPending);
}

/// A person's answer, for the SDK.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "answer")]
pub enum Answer {
    /// Accept a request (one with no pairing code) or a changed key.
    Accept { friend_id: String, pair_id: Option<String>, peer: String },
    Reject { friend_id: String, pair_id: Option<String>, peer: String },
    /// Removed or blocked: revoke on the plane, refuse inbound at once.
    Revoke { friend_id: String, pair_id: Option<String>, peer: String, block: bool },
    /// An allowed outgoing request: send it.
    Invite { friend_id: String, peer: String },
}

/// The most answers kept for the network half (the oldest drop first; the
/// friend rows keep every answer anyway).
pub const ANSWERS_MAX: usize = 256;

static CODES: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);
static ANSWERS: Mutex<VecDeque<Answer>> = Mutex::new(VecDeque::new());

/// A pairing code is six ASCII digits; spaces and dashes a person types
/// between them are dropped.
pub fn normalize_code(s: &str) -> Option<String> {
    let d: String = s.chars().filter(|c| !matches!(c, ' ' | '-' | '\u{a0}')).collect();
    (d.len() == 6 && d.bytes().all(|b| b.is_ascii_digit())).then_some(d)
}

/// `123 456`, as the Postal tab will show it to the owner.
pub fn show_code(code: &str) -> String {
    if code.len() == 6 {
        format!("{} {}", &code[..3], &code[3..])
    } else {
        code.to_string()
    }
}

/// Keep the pairing code of friend row `friend_id` (a bad code is dropped).
pub fn set_code(friend_id: &str, code: &str) {
    let Some(code) = normalize_code(code) else { return };
    if let Ok(mut g) = CODES.lock() {
        g.get_or_insert_with(HashMap::new).insert(friend_id.to_string(), code);
    }
}

/// The pairing code waiting for friend row `friend_id`, if any.
pub fn code(friend_id: &str) -> Option<String> {
    CODES.lock().ok().and_then(|g| g.as_ref().and_then(|m| m.get(friend_id).cloned()))
}

/// Forget friend row `friend_id`'s code (it was answered).
pub fn clear_code(friend_id: &str) {
    if let Ok(mut g) = CODES.lock() {
        if let Some(m) = g.as_mut() {
            m.remove(friend_id);
        }
    }
}

/// Queue a person's answer for the SDK.
pub fn record_answer(a: Answer) {
    if let Ok(mut q) = ANSWERS.lock() {
        while q.len() >= ANSWERS_MAX {
            q.pop_front();
        }
        q.push_back(a);
    }
}

/// Every queued answer, oldest first (the network half drains them).
pub fn take_answers() -> Vec<Answer> {
    ANSWERS.lock().map(|mut q| q.drain(..).collect()).unwrap_or_default()
}

/// The queued answers about one friend row, without draining (tests and
/// diagnostics).
pub fn answers_for(friend_id: &str) -> Vec<Answer> {
    let id_of = |a: &Answer| match a {
        Answer::Accept { friend_id, .. } | Answer::Reject { friend_id, .. } | Answer::Revoke { friend_id, .. } | Answer::Invite { friend_id, .. } => {
            friend_id.clone()
        }
    };
    ANSWERS.lock().map(|q| q.iter().filter(|a| id_of(a) == friend_id).cloned().collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_six_digits_and_show_in_two_threes() {
        assert_eq!(normalize_code("123 456").as_deref(), Some("123456"));
        assert_eq!(normalize_code("123-456").as_deref(), Some("123456"));
        for bad in ["12345", "1234567", "12a456", "", "١٢٣٤٥٦"] {
            assert!(normalize_code(bad).is_none(), "{bad:?}");
        }
        assert_eq!(show_code("123456"), "123 456");
        let id = format!("f-{}", uuid::Uuid::new_v4());
        set_code(&id, "not a code");
        assert_eq!(code(&id), None, "a bad code is never kept");
        set_code(&id, "654 321");
        assert_eq!(code(&id).as_deref(), Some("654321"));
        clear_code(&id);
        assert_eq!(code(&id), None);
    }

    #[test]
    fn answers_queue_for_the_network_half_and_nothing_is_sent() {
        assert!(!network_available(), "native Postal waits on SDK wire v2 and the plane");
        let id = format!("f-{}", uuid::Uuid::new_v4());
        record_answer(Answer::Reject { friend_id: id.clone(), pair_id: None, peer: "a::bcd.postal.bot".into() });
        assert_eq!(answers_for(&id).len(), 1);
        let v = serde_json::to_value(&answers_for(&id)[0]).unwrap();
        assert_eq!(v["answer"], "reject", "{v}");
    }
}
