//! Pairing over the plane (prd-postal-in-k2-v1 S3, network half): postal-sdk's
//! SAS v2 state machines, driven from the plane's pair rows, with K2's
//! handshake object holding each one in memory.
//!
//! The steps, each box doing only its own:
//! 1. **Ask** (this box, `from`): a person allowed the invite (a card, or
//!    the tab), so the request row waits `pending_out` with no pair id.
//!    [`send_unsent`] makes the commitment and `POST /pairs`, once this
//!    box holds the owner's decide token (so it can confirm the answer).
//! 2. **Respond** (the other box, `to`): K2 answers a request for a claimed
//!    handle by itself (its key and nonce only; nothing is trusted yet),
//!    unless the peer is blocked or the handle has too many waiting.
//! 3. **Reveal** (`from`), then each box has the handshake and its own six
//!    digits. K2's `HumanGate` records the request and puts the
//!    `postal.pair.accept` card up **on both boxes**; the code is kept for
//!    people only (the Postal tab, the card's own form), never an agent.
//! 4. **Confirm**: the person types the code shown on the OTHER box.
//!    [`confirm`] checks it here (the SDK, in constant time) and only then
//!    signs `accept` with the handle key; the daemon posts it with the
//!    owner's decide token (`owner_required` for any box credential). A
//!    code that doesn't match signs `mismatch`, and the pair fails on both.
//! 5. **Pin**: when the plane says `accepted`, the other side's signed
//!    accept is checked against the peer key over THIS box's transcript;
//!    only then is the friend `accepted`, with its key pinned.
//!
//! The handshake lives in memory (the SDK's states can't be stored, by
//! design: the nonces are secrets). A restart mid-pairing loses it; the
//! request then starts again or expires on the plane (600 s).

use std::collections::HashMap;
use std::sync::Mutex;

use k2_core::postal::sdk::address::Address;
use k2_core::postal::sdk::pairing::{
    Attempts, Commitment, Confirmed, Decision, Handshake, Initiator, PairContext, PublicKey, Responder, Response, Reveal, Side,
};
use k2_core::postal::sdk::plane::{self, PairItem, PairList, PairStatus, Typ};
use k2_core::postal::store;

use super::plane::{CallError, Plane};

/// One pairing's state in this process.
enum State {
    Initiator(Initiator),
    Responder(Responder),
    Handshake(Handshake),
    Awaiting(Box<k2_core::postal::sdk::pairing::AwaitingPeer>),
}

static PAIRS: Mutex<Option<HashMap<String, State>>> = Mutex::new(None);

fn with_pairs<T>(f: impl FnOnce(&mut HashMap<String, State>) -> T) -> T {
    let mut g = PAIRS.lock().unwrap_or_else(|p| p.into_inner());
    f(g.get_or_insert_with(HashMap::new))
}

fn put(id: &str, s: State) {
    with_pairs(|m| m.insert(id.to_string(), s));
}

fn take(id: &str) -> Option<State> {
    with_pairs(|m| m.remove(id))
}

fn has(id: &str) -> bool {
    with_pairs(|m| m.contains_key(id))
}

/// Tests: hold a finished handshake for `pair_id`, as step 3 or 4 would.
#[cfg(test)]
pub(crate) fn hold_handshake_for_test(pair_id: &str, hs: Handshake) {
    put(pair_id, State::Handshake(hs));
}

/// Is a handshake waiting for this pair's person (the code check)?
pub fn handshake_waiting(pair_id: &str) -> bool {
    with_pairs(|m| matches!(m.get(pair_id), Some(State::Handshake(_))))
}

pub(crate) fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

// ---------------------------------------------------------------- attempts (B3)

fn attempts_path() -> std::path::PathBuf {
    k2_core::paths::k2_home().join("postal-attempts.json")
}

/// The box's own count of pairing attempts per (local handle, peer),
/// persisted (the plane's limit is not the only one, rule B3).
fn attempts_for(local: &str, peer: &str) -> Attempts {
    let key = format!("{local} {peer}");
    let all: HashMap<String, Vec<(i64, String)>> =
        std::fs::read(attempts_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    Attempts::from_log(all.get(&key).cloned().unwrap_or_default())
}

fn save_attempts(local: &str, peer: &str, a: &Attempts) {
    let key = format!("{local} {peer}");
    let mut all: HashMap<String, Vec<(i64, String)>> =
        std::fs::read(attempts_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    all.insert(key, a.log().to_vec());
    if let Ok(bytes) = serde_json::to_vec(&all) {
        let _ = k2_core::fs_atomic::atomic_write_private(&attempts_path(), &bytes);
    }
}

fn new_pair_id() -> String {
    k2_core::postal::new_id()
}

fn identity_of(address: &str) -> Result<k2_core::postal::sdk::identity::Identity, CallError> {
    k2_core::postal::keys::identity(address)
        .map_err(CallError::NoConnect)?
        .ok_or_else(|| CallError::NoConnect(format!("{address} has no Postal key on this server yet")))
}

fn db<T>(f: impl FnOnce(&rusqlite::Connection) -> T) -> T {
    let shared = k2_core::db::shared();
    let conn = shared.lock();
    f(&conn)
}

// ---------------------------------------------------------------- handles

/// Register every local handle on this box's label with the plane (`PUT
/// /me` with its key): `local` → `claimed`. A handle on another label
/// can't be this box's and stays `local` (the status says why).
pub fn register_handles(net: &Plane) -> Result<(), CallError> {
    let label = net.label()?;
    for h in db(store::handles) {
        let Some(addr) = k2_core::postal::parse_address(&h.address) else { continue };
        if addr.label != label {
            continue;
        }
        let ident = k2_core::postal::keys::identity_or_create(&h.address).map_err(CallError::NoConnect)?;
        let fp = ident.fingerprint().to_hex();
        let key_name = format!("{}@{fp}", k2_core::postal::keys::identity_name(&h.address));
        if h.status == "claimed" && h.key_name.as_deref() == Some(key_name.as_str()) {
            continue;
        }
        let sdk_addr = Address::parse(&h.address).map_err(|_| CallError::NoConnect(format!("{} isn't a Postal address", h.address)))?;
        let req = plane::me_put(&sdk_addr, &ident.public_key(), Typ::Session);
        let (s, b) = net.box_call(&req)?;
        plane::parse::<serde_json::Value>(s, &b).map_err(CallError::Plane)?;
        db(|c| store::set_handle_status(c, &h.id, "claimed", Some(&key_name))).map_err(|e| CallError::NoConnect(e.hint()))?;
        k2_core::log_debug!("[postal] {} is on the Postal network (key {})", h.address, &fp[..16]);
    }
    Ok(())
}

/// The fingerprint this box registered for a handle (from `key_name`).
pub fn registered_fingerprint(h: &store::Handle) -> Option<String> {
    h.key_name.as_deref()?.rsplit_once('@').map(|(_, fp)| fp.to_string())
}

// ---------------------------------------------------------------- 1. ask

/// Send every request a person allowed that isn't on the plane yet, and
/// only once this server can accept the answer: with no approval token
/// here it is asked for first (unattended, so it waits after a failure);
/// still none, nothing goes out and the requests wait here (if an invite
/// can be sent, it can be accepted).
pub fn send_unsent(net: &Plane) -> Result<(), CallError> {
    let unsent = db(store::unsent_outgoing);
    if unsent.is_empty() {
        return Ok(());
    }
    if let Err(e) = super::decide_token::ready(net, super::decide_token::Why::Invite { person: false }, "server") {
        k2_core::log_debug!("[postal] {} friend request(s) wait: no approval token here ({})", unsent.len(), e.code());
        return Ok(());
    }
    for f in unsent {
        let Some(h) = db(|c| store::handle(c, &f.handle_id)) else { continue };
        if h.status != "claimed" {
            continue;
        }
        let (Ok(from), Ok(to)) = (Address::parse(&h.address), Address::parse(&f.peer_address)) else { continue };
        let ident = identity_of(&h.address)?;
        let id = new_pair_id();
        let ctx = PairContext { pair_id: id.clone(), initiator: h.address.clone(), responder: f.peer_address.clone() };
        let mut attempts = attempts_for(&h.address, &f.peer_address);
        let (init, c) = match Initiator::start(ctx, ident.public_key(), &mut attempts, now()) {
            Ok(x) => x,
            Err(e) => {
                k2_core::log_debug!("[postal] request to {} not sent: {e}", f.peer_address);
                continue;
            }
        };
        save_attempts(&h.address, &f.peer_address, &attempts);
        let req = plane::pair_create(&id, &from, &to, Typ::Session, &ident.public_key(), &c).map_err(CallError::Plane)?;
        let (s, b) = net.box_call(&req)?;
        match plane::parse::<serde_json::Value>(s, &b) {
            Ok(v) if v["existing"].as_bool() == Some(true) => {
                // Already friends on the plane with this very key.
                k2_core::log_debug!("[postal] {} → {}: an accepted pair exists already", h.address, f.peer_address);
            }
            Ok(_) => {
                db(|c| store::set_pair_id(c, &f.id, &id)).map_err(|e| CallError::NoConnect(e.hint()))?;
                put(&id, State::Initiator(init));
            }
            Err(e) if e.is_retryable() => return Err(CallError::Plane(e)),
            Err(e) => {
                // A request the plane will never take (no such address, …).
                k2_core::log_debug!("[postal] request to {} refused by the plane: {e}", f.peer_address);
                let _ = db(|c| store::network_status(c, &f.id, "rejected"));
                crate::permission::actions::postal::close_pair_cards_by_network(&h.workspace_id, &f.id, "refused by the Postal network");
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- 2–5. drive

/// Walk every SAS v2 pair the plane has for this box and take this box's
/// next step on each.
pub fn sync(net: &Plane) -> Result<(), CallError> {
    let list: PairList = net.box_json(&plane::pairs_list())?;
    for item in list.pairs.iter().filter(|p| p.sas_version == 2) {
        if let Err(e) = step(net, item) {
            if !e.is_retryable() {
                k2_core::log_debug!("[postal] pair {}: {}", item.id, e.code());
            } else {
                return Err(e);
            }
        }
    }
    Ok(())
}

fn step(net: &Plane, item: &PairItem) -> Result<(), CallError> {
    let local = db(|c| store::handle_by_address(c, &item.handle));
    let Some(h) = local.filter(|h| h.status == "claimed") else { return Ok(()) };
    let friend = db(|c| store::friend_by_pair(c, &item.id));
    match item.status {
        PairStatus::Accepted => return pin(item, &h, friend),
        PairStatus::Rejected | PairStatus::Failed | PairStatus::Expired => return ended(item, &h, friend, "rejected"),
        PairStatus::Revoked => return ended(item, &h, friend, "revoked"),
        PairStatus::PendingIn | PairStatus::PendingOut | PairStatus::Unknown => {}
    }
    let side = item.side.as_deref();
    let step = item.step.as_deref();
    match (side, step, item.status.clone()) {
        // B: answer a request for one of this box's handles.
        (Some("to"), Some("requested"), PairStatus::PendingIn) if !has(&item.id) => respond(net, item, &h),
        // A: the other box answered; reveal and get the handshake.
        (Some("from"), Some("responded"), PairStatus::PendingIn) => reveal(net, item, &h, friend),
        // B: A revealed; check it against the commitment kept in step 2.
        (Some("to"), Some("revealed"), PairStatus::PendingIn) => finish_responder(item, &h),
        _ => Ok(()),
    }
}

fn respond(net: &Plane, item: &PairItem, h: &store::Handle) -> Result<(), CallError> {
    let Some(peer) = k2_core::postal::parse_address(&item.peer) else { return Ok(()) };
    // Blocked, or too many waiting: no answer (the plane expires it).
    let refused = db(|c| {
        let blocked = store::friend_by_peer(c, &h.id, &peer.as_string()).is_some_and(|f| f.status == "blocked");
        let (_, waiting, _) = store::counts(c, &h.id);
        blocked || waiting >= store::PENDING_IN_CAP
    });
    if refused {
        return Ok(());
    }
    let (Some(pem), Some(c_hex)) = (item.peer_key_pem.as_deref(), item.commitment.as_deref()) else { return Ok(()) };
    let (Ok(peer_key), Ok(c)) = (PublicKey::from_pem(pem), Commitment::from_hex(c_hex)) else { return Ok(()) };
    let ident = identity_of(&h.address)?;
    let ctx = PairContext { pair_id: item.id.clone(), initiator: item.peer.clone(), responder: h.address.clone() };
    let mut attempts = attempts_for(&h.address, &item.peer);
    let (state, resp) = match Responder::respond(ctx, peer_key, c, ident.public_key(), &mut attempts, now()) {
        Ok(x) => x,
        Err(e) => {
            k2_core::log_debug!("[postal] request {} not answered: {e}", item.id);
            return Ok(());
        }
    };
    save_attempts(&h.address, &item.peer, &attempts);
    let req = plane::pair_respond(&item.id, &resp, Typ::Session).map_err(CallError::Plane)?;
    let (s, b) = net.box_call(&req)?;
    plane::parse::<serde_json::Value>(s, &b).map_err(CallError::Plane)?;
    put(&item.id, State::Responder(state));
    Ok(())
}

fn reveal(net: &Plane, item: &PairItem, h: &store::Handle, friend: Option<store::Friend>) -> Result<(), CallError> {
    let Some(State::Initiator(init)) = take(&item.id) else {
        // A restart lost the commitment's nonce: ask again with a new id.
        if let Some(f) = friend.filter(|f| f.status == "pending_out" && f.decided_by.is_none()) {
            let _ = db(|c| c.execute("UPDATE postal_friends SET pair_id = NULL WHERE id = ?1", rusqlite::params![f.id]));
        }
        return Ok(());
    };
    let (Some(pem), Some(nb)) = (item.peer_key_pem.as_deref(), item.nb.as_deref()) else {
        put(&item.id, State::Initiator(init));
        return Ok(());
    };
    let Ok(resp) = Response::from_parts(pem, nb) else { return Ok(()) };
    let (rv, hs) = match init.finish(resp, now()) {
        Ok(x) => x,
        Err(e) => {
            k2_core::log_debug!("[postal] pair {}: {e}", item.id);
            return Ok(());
        }
    };
    let req = plane::pair_reveal(&item.id, &rv).map_err(CallError::Plane)?;
    let (s, b) = net.box_call(&req)?;
    plane::parse::<serde_json::Value>(s, &b).map_err(CallError::Plane)?;
    raise(&hs, h, item);
    put(&item.id, State::Handshake(hs));
    Ok(())
}

fn finish_responder(item: &PairItem, h: &store::Handle) -> Result<(), CallError> {
    if handshake_waiting(&item.id) || with_pairs(|m| matches!(m.get(&item.id), Some(State::Awaiting(_)))) {
        return Ok(());
    }
    let Some(State::Responder(r)) = take(&item.id) else { return Ok(()) };
    let Some(Ok(rv)) = item.na.as_deref().map(Reveal::from_hex) else {
        put(&item.id, State::Responder(r));
        return Ok(());
    };
    match r.finish(rv, now()) {
        Ok(hs) => {
            raise(&hs, h, item);
            put(&item.id, State::Handshake(hs));
        }
        // The reveal doesn't open the commitment kept in step 2: a plane
        // that steered the code. Nothing is raised; the pair dies.
        Err(e) => k2_core::log_debug!("[postal] pair {}: the reveal was refused ({e})", item.id),
    }
    Ok(())
}

/// The SDK's human gate, on both boxes: record the request and put the
/// card up; the code is kept for people only.
fn raise(hs: &Handshake, h: &store::Handle, item: &PairItem) {
    let req = k2_core::postal::gate::FriendRequest {
        pair_id: item.id.clone(),
        local_handle: h.address.clone(),
        peer: item.peer.clone(),
        peer_owner_name: None,
        side: hs.side(),
        show_code: hs.code().as_str().to_string(),
        peer_fingerprint: hs.peer_key().fingerprint(),
    };
    use k2_core::postal::gate::HumanGate;
    crate::postal_routes::K2Gate.friend_request(&req);
}

fn pin(item: &PairItem, h: &store::Handle, friend: Option<store::Friend>) -> Result<(), CallError> {
    let Some(f) = friend else { return Ok(()) };
    if f.status == "accepted" {
        return Ok(());
    }
    let Some(State::Awaiting(w)) = take(&item.id) else {
        // Accepted on the plane, but this box can't check the other
        // side's signature (no transcript: a restart, or never confirmed
        // here). Not a friend; the person pairs again.
        return Ok(());
    };
    let Some(pc) = item.peer_confirmation() else {
        put(&item.id, State::Awaiting(w));
        return Ok(());
    };
    let Some(decision) = Decision::parse(&pc.decision) else {
        // Not a decision this SDK knows: nothing to verify yet.
        k2_core::log_debug!("[postal] pair {}: the other side's decision isn't one this server knows", item.id);
        put(&item.id, State::Awaiting(w));
        return Ok(());
    };
    match w.peer_confirmation(decision, pc.sig.as_deref().unwrap_or("")) {
        Ok(pinned) => {
            let fp = pinned.key.fingerprint().to_hex();
            let f = db(|c| store::pin_accepted(c, &f.id, &fp, pinned.key.pem())).map_err(|e| CallError::NoConnect(e.hint()))?;
            k2_core::log_debug!("[postal] {} and {} are friends (key {})", h.address, f.peer_address, &fp[..16]);
            crate::permission::actions::postal::close_pair_cards_by_network(&h.workspace_id, &f.id, "friends now");
        }
        Err(e) => {
            k2_core::log_debug!("[postal] pair {}: the other side's accept didn't verify ({e}); not a friend", item.id);
            let _ = db(|c| store::network_status(c, &f.id, "rejected"));
        }
    }
    Ok(())
}

fn ended(item: &PairItem, h: &store::Handle, friend: Option<store::Friend>, status: &str) -> Result<(), CallError> {
    let _ = take(&item.id);
    let Some(f) = friend else { return Ok(()) };
    let live = match status {
        "revoked" => matches!(f.status.as_str(), "accepted" | "rekey_pending"),
        _ => matches!(f.status.as_str(), "pending_in" | "pending_out"),
    };
    if live {
        let _ = db(|c| store::network_status(c, &f.id, status));
        k2_core::postal::gate::clear_code(&f.id);
        crate::permission::actions::postal::close_pair_cards_by_network(&h.workspace_id, &f.id, if status == "revoked" { "removed by the other side" } else { "the request ended" });
    }
    Ok(())
}

// ---------------------------------------------------------------- the person's answer

/// What [`confirm`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Signed and sent; waiting for the other side (or friends already).
    Accepted { friends: bool },
    /// The code didn't match: `mismatch` signed and sent; the pair failed.
    Mismatch,
}

/// A person typed the code shown on the other box for request `f`.
pub fn confirm(net: &Plane, f: &store::Friend, typed: &str, by: &str) -> Result<Outcome, CallError> {
    let pair_id = f.pair_id.clone().ok_or_else(|| CallError::NoConnect("this request has no pairing on the Postal network yet".into()))?;
    let h = db(|c| store::handle(c, &f.handle_id)).ok_or_else(|| CallError::NoConnect("no Postal address".into()))?;
    let label = k2_core::postal::parse_address(&h.address).map(|a| a.label).unwrap_or_default();
    // Without the owner's token nothing can be confirmed: say so first,
    // and keep the handshake for when it is linked.
    if k2_core::postal::keys::decide_token(&label).map_err(CallError::NoConnect)?.is_none() {
        return Err(CallError::NoOwnerLink(label));
    }
    let Some(State::Handshake(hs)) = take(&pair_id) else {
        return Err(CallError::NoConnect("this pairing's code check isn't waiting any more (it expired, or this server restarted): pair again".into()));
    };
    let ident = identity_of(&h.address)?;
    let confirmed = hs.confirm(typed, &ident).map_err(|e| CallError::NoConnect(format!("pairing: {e}")))?;
    match confirmed {
        Confirmed::Accept { confirmation, awaiting } => {
            let req = plane::pair_confirm(&confirmation).map_err(CallError::Plane)?;
            let (s, b) = net.owner_call(&label, &req)?;
            match plane::parse::<PairItem>(s, &b) {
                Ok(_) => {}
                Err(e) if e.code() == Some(&plane::Code::AlreadyDecided) => {}
                Err(e) => return Err(CallError::Plane(e)),
            }
            db(|c| store::confirmed_waiting_peer(c, &f.id, by)).map_err(|e| CallError::NoConnect(e.hint()))?;
            k2_core::postal::gate::clear_code(&f.id);
            put(&pair_id, State::Awaiting(awaiting));
            // The other side may have confirmed first.
            let item: Result<PairItem, _> = net.box_json(&plane::pair_get(&pair_id).map_err(CallError::Plane)?);
            if let Ok(item) = item {
                pin(&item, &h, db(|c| store::friend(c, &f.id)))?;
            }
            let friends = db(|c| store::friend(c, &f.id)).is_some_and(|f| f.status == "accepted");
            Ok(Outcome::Accepted { friends })
        }
        Confirmed::Mismatch { confirmation } => {
            let req = plane::pair_confirm(&confirmation).map_err(CallError::Plane)?;
            let _ = net.owner_call(&label, &req)?;
            let _ = db(|c| store::network_status(c, &f.id, "rejected"));
            k2_core::postal::gate::clear_code(&f.id);
            Ok(Outcome::Mismatch)
        }
    }
}

/// A person said no to a request whose handshake is waiting: sign
/// `reject` so the other side hears it. Without a handshake there is
/// nothing to sign; the plane lets the request expire.
pub fn reject(net: &Plane, f: &store::Friend) -> Result<(), CallError> {
    let Some(pair_id) = f.pair_id.clone() else { return Ok(()) };
    let Some(State::Handshake(hs)) = take(&pair_id) else { return Ok(()) };
    let h = db(|c| store::handle(c, &f.handle_id)).ok_or_else(|| CallError::NoConnect("no Postal address".into()))?;
    let label = k2_core::postal::parse_address(&h.address).map(|a| a.label).unwrap_or_default();
    let ident = identity_of(&h.address)?;
    let c = hs.reject(&ident).map_err(|e| CallError::NoConnect(format!("pairing: {e}")))?;
    let req = plane::pair_confirm(&c).map_err(CallError::Plane)?;
    let (s, b) = net.owner_call(&label, &req)?;
    plane::parse::<serde_json::Value>(s, &b).map(|_| ()).map_err(CallError::Plane)
}

/// Removed or blocked: revoke an accepted pair on the plane (owner token).
pub fn revoke(net: &Plane, f: &store::Friend) -> Result<(), CallError> {
    let Some(pair_id) = f.pair_id.clone() else { return Ok(()) };
    let _ = take(&pair_id);
    let h = db(|c| store::handle(c, &f.handle_id)).ok_or_else(|| CallError::NoConnect("no Postal address".into()))?;
    let label = k2_core::postal::parse_address(&h.address).map(|a| a.label).unwrap_or_default();
    let req = plane::pair_revoke(&pair_id).map_err(CallError::Plane)?;
    let (s, b) = net.owner_call(&label, &req)?;
    match plane::parse::<serde_json::Value>(s, &b) {
        Ok(_) => Ok(()),
        // Not accepted there (a request that never finished): nothing to revoke.
        Err(e) if e.code() == Some(&plane::Code::PairNotAccepted) => Ok(()),
        Err(e) => Err(CallError::Plane(e)),
    }
}

/// Which side this box is, for a pair it holds (tests, status).
pub fn side_of(pair_id: &str) -> Option<Side> {
    with_pairs(|m| match m.get(pair_id) {
        Some(State::Handshake(h)) => Some(h.side()),
        _ => None,
    })
}
