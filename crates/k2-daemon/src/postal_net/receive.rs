//! Receive (prd-postal-in-k2-v1 S4): held mail is pulled, opened and
//! verified by postal-sdk, stored durably, ACKed, then landed by K2's
//! routing rules.
//!
//! **The order, never cut** (K2's review rules 1, 3 and 4):
//! 1. A `hold` event (or the 60 s poll) → `GET /hold`, page by page. The
//!    loop stops if the plane's `next_after` doesn't move.
//! 2. Per item: the handle must be this box's; the sender an accepted
//!    friend **of that handle** with a pinned key (a request, a blocked or
//!    removed peer is refused `gated`; a friend whose key changed waits);
//!    this box's own key must be the one it registered (else the item stays
//!    on the plane: a local fault, not the sender's).
//! 3. One SQLite transaction: the SDK opens and verifies the item, and its
//!    seen insert **is** the message's delivery row (unique Postal id);
//!    `deliver` fills that row in and writes the inbound file. Commit.
//! 4. Only then `POST /hold/ack`. A replay finds the row: the SDK says
//!    `Ack`, nothing new is stored. Anything that can't open or verify is
//!    refused (`gated`, or `too_large`), so the sender's receipt ends.
//! 5. `notify` lands it (terminal by default, waking the agent; inbox; a
//!    person's Thread), on the agent's delivery lane, and never types a
//!    message twice: it marks the terminal try in the record first.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use k2_core::postal::files::{self, Inbound};
use k2_core::postal::route::{self, Facts, Plan, Rule, Target};
use k2_core::postal::sdk::hold::{self, Disposition, HoldError, OpenWith};
use k2_core::postal::sdk::host::{DeliverError, RefuseCode, VerifiedMessage};
use k2_core::postal::sdk::pairing::PublicKey;
use k2_core::postal::sdk::plane::{self, HoldItem, HoldPage};
use k2_core::postal::seen::{self, Pending, TxSeen};
use k2_core::postal::store;
use serde::{Deserialize, Serialize};

use super::plane::{CallError, Plane};

/// K2's cap on one message (body and files, decoded), under the SDK's.
pub const MAX_MESSAGE: usize = 256 * 1024;
/// Messages one friend may land per hour before the rest go quietly to
/// the inbox (§7).
pub const PER_FRIEND_HOUR: usize = 60;
/// Most pages one pull walks.
const MAX_PAGES: usize = 50;

/// What one pull did (tests and the status answer).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pulled {
    pub stored: usize,
    pub acked_replays: usize,
    pub refused: usize,
    pub left: usize,
    /// The plane's cursor didn't move: the pull stopped (rule 3).
    pub cursor_stuck: bool,
}

fn db<T>(f: impl FnOnce(&rusqlite::Connection) -> T) -> T {
    let shared = k2_core::db::shared();
    let conn = shared.lock();
    f(&conn)
}

/// Pull every held item for this box and store, ACK or refuse each.
pub fn pull(net: &Plane) -> Result<Pulled, CallError> {
    let mut out = Pulled::default();
    let mut after: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let page: HoldPage = net.box_json(&plane::hold_list(after.as_deref(), plane::HOLD_PAGE))?;
        let mut ack = Vec::new();
        let mut refuse: Vec<(plane::RefuseCode, Vec<String>)> = Vec::new();
        let mut landed = Vec::new();
        for item in &page.items {
            match take_item(item) {
                Took::Stored(delivery_id) => {
                    out.stored += 1;
                    ack.push(item.id.clone());
                    landed.push(delivery_id);
                }
                Took::Replay => {
                    out.acked_replays += 1;
                    ack.push(item.id.clone());
                }
                Took::Refuse(code) => {
                    out.refused += 1;
                    match refuse.iter_mut().find(|(c, _)| *c == code) {
                        Some((_, ids)) => ids.push(item.id.clone()),
                        None => refuse.push((code, vec![item.id.clone()])),
                    }
                }
                Took::Leave => out.left += 1,
            }
        }
        for chunk in ack.chunks(plane::MAX_IDS) {
            let req = plane::hold_ack(chunk).map_err(CallError::Plane)?;
            let _: plane::Acked = net.box_json(&req)?;
        }
        for (code, ids) in refuse {
            for chunk in ids.chunks(plane::MAX_IDS) {
                let req = plane::hold_refuse(chunk, code).map_err(CallError::Plane)?;
                let _: serde_json::Value = net.box_json(&req)?;
            }
        }
        for d in landed {
            land_now(&d);
        }
        let next = page.next_after(plane::HOLD_PAGE).map(str::to_string);
        match next {
            None => break,
            Some(n) if Some(&n) == after.as_ref() => {
                // Rule 3: a plane that hands back the same cursor must not
                // spin the daemon.
                out.cursor_stuck = true;
                k2_core::log_debug!("[postal] hold pull stopped: the plane's next_after didn't move");
                break;
            }
            Some(n) => after = Some(n),
        }
    }
    Ok(out)
}

enum Took {
    Stored(String),
    Replay,
    Refuse(plane::RefuseCode),
    Leave,
}

fn refuse_of(c: RefuseCode) -> plane::RefuseCode {
    c.into()
}

fn take_item(item: &HoldItem) -> Took {
    let (outer, sealed) = match item.decode() {
        Ok(x) => x,
        Err(_) => {
            let big = item.sealed.len() > hold::MAX_SEALED.div_ceil(3) * 4;
            return Took::Refuse(if big { plane::RefuseCode::TooLarge } else { plane::RefuseCode::Gated });
        }
    };
    let to = outer.to.to_string();
    let from = outer.from.to_string();
    let Some(h) = db(|c| store::handle_by_address(c, &to)).filter(|h| h.status == "claimed") else {
        return Took::Refuse(plane::RefuseCode::NoAgent);
    };
    let friend = db(|c| store::friend_by_peer(c, &h.id, &from));
    let friend = match friend {
        Some(f) if f.status == "accepted" => f,
        // The key changed: held until a person decides. Or this box's
        // person accepted and the other side's signed accept is still
        // being checked here (it pins within a pass): wait for it.
        Some(f) if f.status == "rekey_pending" || (f.status == "pending_out" && f.decided_by.is_some()) => return Took::Leave,
        _ => return Took::Refuse(plane::RefuseCode::Gated),
    };
    let Some(pem) = db(|c| store::peer_key_pem(c, &friend.id)) else { return Took::Refuse(plane::RefuseCode::Gated) };
    let Ok(sender) = PublicKey::from_pem(&pem) else { return Took::Refuse(plane::RefuseCode::Gated) };
    // Rule 4: this box's own key, the one it registered for the handle.
    let ident = match k2_core::postal::keys::identity(&h.address) {
        Ok(Some(i)) => i,
        _ => {
            k2_core::log_debug!("[postal] {}: this server's Postal key can't be read; held mail stays on the plane", h.address);
            return Took::Leave;
        }
    };
    if super::pairing::registered_fingerprint(&h).as_deref() != Some(ident.fingerprint().to_hex().as_str()) {
        k2_core::log_debug!("[postal] {}: the stored key isn't the one registered; held mail stays on the plane", h.address);
        return Took::Leave;
    }
    let Some(ws) = crate::postal_routes::ws_of_id(&h.workspace_id) else { return Took::Refuse(plane::RefuseCode::NoAgent) };
    let me = ident.public_key();
    let delivery_id = k2_core::hire::deliveries::new_id();
    let shared = k2_core::db::shared();
    let conn = shared.lock();
    let Ok(tx) = conn.unchecked_transaction() else { return Took::Leave };
    let mut seen_store = TxSeen::new(
        &tx,
        Pending { delivery_id: delivery_id.clone(), target_workspace_id: ws.id.clone(), target: ws.path.clone() },
    );
    let opened = hold::open(
        &outer,
        &sealed,
        &OpenWith { opener: &ident, me: &me, to: &outer.to, sender: &sender, now: super::pairing::now() },
        &mut seen_store,
    );
    let v = match opened {
        Ok(v) => v,
        Err(e) => {
            drop(tx);
            return disposed(&e, item);
        }
    };
    match deliver(&tx, &v, &h, &friend, &ws, &delivery_id, true) {
        Ok(()) => match tx.commit() {
            Ok(()) => Took::Stored(delivery_id),
            Err(_) => {
                files::remove_inbound(&delivery_id);
                Took::Leave
            }
        },
        Err(DeliverError::Refuse(c)) => {
            drop(tx);
            Took::Refuse(refuse_of(c))
        }
        Err(DeliverError::Retry { .. }) => {
            drop(tx);
            files::remove_inbound(&delivery_id);
            Took::Leave
        }
    }
}

fn disposed(e: &HoldError, item: &HoldItem) -> Took {
    match e.disposition() {
        Disposition::Ack => Took::Replay,
        Disposition::Retry => Took::Leave,
        Disposition::Refuse(code) => {
            // The id only: never the body, never the key.
            k2_core::log_debug!("[postal] held item {} refused ({:?}): {e}", item.id, code);
            Took::Refuse(code)
        }
    }
}

// ---------------------------------------------------------------- deliver

static RATE: Mutex<Option<HashMap<String, VecDeque<i64>>>> = Mutex::new(None);

/// Count one message from `peer` to `handle`; `false` past the hourly cap.
fn under_rate(handle: &str, peer: &str, now: i64) -> bool {
    let mut g = RATE.lock().unwrap_or_else(|p| p.into_inner());
    let q = g.get_or_insert_with(HashMap::new).entry(format!("{handle} {peer}")).or_default();
    while q.front().is_some_and(|t| now - *t >= 3600) {
        q.pop_front();
    }
    q.push_back(now);
    q.len() <= PER_FRIEND_HOUR
}

/// Where a stored message has landed, kept in its delivery record.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Landed {
    pub plan: Option<Plan>,
    #[serde(default)]
    pub done: Vec<String>,
    /// A terminal inject was started: never typed again.
    #[serde(default)]
    pub terminal_started: bool,
    #[serde(default)]
    pub inbox_id: Option<String>,
    #[serde(default)]
    pub pointer_done: bool,
    #[serde(default)]
    pub held: bool,
}

/// The SDK's `deliver`, K2's side: durable, inside the storing transaction
/// (the delivery row the seen insert made, filled in, and the inbound file).
pub fn deliver(
    tx: &rusqlite::Connection,
    v: &VerifiedMessage,
    h: &store::Handle,
    friend: &store::Friend,
    ws: &crate::postal_routes::Ws,
    delivery_id: &str,
    held: bool,
) -> Result<(), DeliverError> {
    let size = v.body().len() + v.files().iter().map(|f| f.bytes().len()).sum::<usize>();
    if size > MAX_MESSAGE {
        return Err(DeliverError::Refuse(RefuseCode::TooLarge));
    }
    let text = std::str::from_utf8(v.body()).ok();
    let facts = Facts {
        body_len: v.body().len(),
        has_files: !v.files().is_empty(),
        plain_text: text.is_some() && route::is_text_plain(v.content_type()),
        tray: v.mode() == k2_core::postal::sdk::host::Mode::Tray,
        wake: v.wake(),
    };
    let agent_rule = Rule::from_json(h.route.as_deref());
    let friend_rule = Rule::from_json(friend.route.as_deref());
    let mut plan = route::plan(agent_rule.as_ref(), friend_rule.as_ref(), &facts);
    if !under_rate(&h.address, &friend.peer_address, super::pairing::now()) {
        plan = Plan { targets: vec![Target::Inbox { pointer: false }], wake: false, adjusted: vec!["rate_limited".into()] };
    }
    let body_hash = match text {
        Some(t) => k2_core::hire::deliveries::body_sha256(t),
        None => v.body_sha256().iter().map(|b| format!("{b:02x}")).collect(),
    };
    let landed = Landed { plan: Some(plan.clone()), held, ..Landed::default() };
    let landed_json = serde_json::to_string(&landed).unwrap_or_else(|_| "{}".into());
    seen::complete(tx, delivery_id, v.badge(), &body_hash, &v.sender_fingerprint().to_hex(), &landed_json)
        .map_err(|e| DeliverError::Retry { after: None, reason: e })?;
    let inbound = Inbound {
        delivery_id: delivery_id.to_string(),
        message_id: v.id().to_string(),
        workspace_id: ws.id.clone(),
        to: v.to().to_string(),
        from: v.from().to_string(),
        badge: v.badge().to_string(),
        content_type: v.content_type().to_string(),
        body_b64: Inbound::encode_body(v.body()),
        files: v.files().iter().map(|f| Inbound::encode_file(f.storage_name(), f.bytes())).collect(),
        held,
        plan,
        tries: 0,
        next_at: 0,
    };
    files::save_inbound(&inbound).map_err(|e| DeliverError::Retry { after: None, reason: e })
}

// ---------------------------------------------------------------- notify

fn landed_of(delivery_id: &str) -> (Landed, bool) {
    db(|c| k2_core::hire::deliveries::get(c, delivery_id))
        .map(|d| (d.landed.as_deref().and_then(|l| serde_json::from_str(l).ok()).unwrap_or_default(), d.delivered))
        .unwrap_or_default()
}

fn save_landed(delivery_id: &str, l: &Landed, delivered: bool) {
    let json = serde_json::to_string(l).unwrap_or_else(|_| "{}".into());
    if let Err(e) = db(|c| seen::set_landed(c, delivery_id, &json, delivered)) {
        k2_core::log_debug!("[postal] {delivery_id}: {e}");
    }
}

/// Land a stored message now, on its agent's delivery lane.
pub fn land_now(delivery_id: &str) {
    let Some(i) = files::inbound().into_iter().find(|i| i.delivery_id == delivery_id) else { return };
    land_on_lane(i);
}

fn land_on_lane(i: Inbound) {
    let key = format!("postal:{}", i.workspace_id);
    let id = i.delivery_id.clone();
    let r = crate::delivery_queue::run_in_order(&key, std::time::Duration::from_secs(120), move || notify(&i));
    if r.is_none() {
        k2_core::log_debug!("[postal] {id}: landing timed out on the lane; it is tried again");
    }
}

/// Land every stored message still waiting (after a restart, or a busy
/// agent's pointer).
pub fn land_waiting() {
    let now = super::pairing::now();
    for i in files::inbound().into_iter().filter(|i| i.next_at <= now) {
        land_on_lane(i);
    }
}

/// What landing did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notified {
    Done,
    /// Waiting for the agent (busy) to type the pointer.
    Later,
}

/// The SDK's `notify`, K2's side: land a stored message where its plan
/// says, once each, then drop its inbound file.
pub fn notify(i: &Inbound) -> Notified {
    let (mut l, delivered) = landed_of(&i.delivery_id);
    if delivered {
        files::remove_inbound(&i.delivery_id);
        return Notified::Done;
    }
    let Some(ws) = crate::postal_routes::ws_of_id(&i.workspace_id) else {
        // The workspace is gone: the stored file is all that's left.
        files::remove_inbound(&i.delivery_id);
        return Notified::Done;
    };
    let body = i.body();
    let text = String::from_utf8_lossy(&body).to_string();
    let plan = l.plan.clone().unwrap_or_else(|| i.plan.clone());
    let mut pending_pointer = false;
    for t in &plan.targets {
        let name = t.as_string();
        if l.done.contains(&name) {
            continue;
        }
        let ok = match t {
            Target::Terminal => {
                if l.terminal_started {
                    // A try already started: never type it again; keep it
                    // in the inbox instead.
                    inbox(i, &ws, &text, &mut l, false, plan.wake);
                    true
                } else if crate::v1_ws_message::busy_status_for_path(&ws.path).is_some() {
                    // Busy (mid-work or on a prompt): an outsider's text is
                    // never eaten as an answer. Inbox now, pointer when idle.
                    inbox(i, &ws, &text, &mut l, true, plan.wake) || {
                        pending_pointer = true;
                        false
                    }
                } else {
                    l.terminal_started = true;
                    save_landed(&i.delivery_id, &l, false);
                    let stamp = crate::workspace_msg::postal_label(&i.from, &i.badge);
                    let r = crate::workspace_msg::deliver_live_with_receipt(
                        &ws.path,
                        &text,
                        crate::workspace_msg::Sender::Postal(&stamp),
                        &i.from,
                        &i.delivery_id,
                        "",
                        plan.wake,
                        crate::workspace_msg::DEFAULT_WAKE_TIMEOUT,
                        "postal",
                        None,
                    );
                    if !r.success {
                        // No agent, or it couldn't start: stored in the inbox.
                        inbox(i, &ws, &text, &mut l, false, plan.wake);
                    }
                    true
                }
            }
            Target::Inbox { pointer } => {
                let ok = inbox(i, &ws, &text, &mut l, *pointer, plan.wake);
                if !ok {
                    pending_pointer = true;
                }
                ok
            }
            Target::Thread(person) => {
                if let Err(e) = crate::overlay_routes::post_postal_thread_line(&ws.path, &i.from, &i.badge, person, &text) {
                    k2_core::log_debug!("[postal] {}: Thread line not posted ({e}); stored in the inbox", i.delivery_id);
                    inbox(i, &ws, &text, &mut l, false, false);
                }
                true
            }
        };
        if ok {
            l.done.push(name);
            save_landed(&i.delivery_id, &l, false);
        }
    }
    if pending_pointer {
        let mut later = i.clone();
        later.tries += 1;
        later.next_at = super::pairing::now() + 30;
        let _ = files::save_inbound(&later);
        return Notified::Later;
    }
    save_landed(&i.delivery_id, &l, true);
    files::remove_inbound(&i.delivery_id);
    Notified::Done
}

/// Store in the inbox (once) with a Chatter line; with `pointer`, also
/// type `[inbox:<id>]` when the agent isn't busy. `false`: the pointer
/// still waits.
fn inbox(i: &Inbound, ws: &crate::postal_routes::Ws, text: &str, l: &mut Landed, pointer: bool, wake: bool) -> bool {
    if l.inbox_id.is_none() {
        match write_inbox(i, ws, text) {
            Ok((id, title, n)) => {
                l.inbox_id = Some(format!("{id}\u{1f}{title}\u{1f}{n}"));
                save_landed(&i.delivery_id, l, false);
                // Postal writes its own Chatter line: the plain inbox path
                // writes none (§5.3).
                crate::overlay_routes::record_inject_chatter(&ws.path, &i.from, text, "postal", Some(&i.delivery_id), None);
            }
            Err(e) => {
                k2_core::log_debug!("[postal] {}: inbox item not written ({e})", i.delivery_id);
                return false;
            }
        }
    }
    if !pointer || l.pointer_done {
        return true;
    }
    if crate::v1_ws_message::busy_status_for_path(&ws.path).is_some() {
        return false;
    }
    let parts: Vec<&str> = l.inbox_id.as_deref().unwrap_or("").split('\u{1f}').collect();
    let (id, title, n) = (parts.first().copied().unwrap_or(""), parts.get(1).copied().unwrap_or(""), parts.get(2).and_then(|n| n.parse().ok()));
    let line = k2_core::inbox::wake_pointer_text_n(id, title, n);
    let r = crate::workspace_msg::deliver_live_with_via(&ws.path, &line, &i.from, "", wake, crate::workspace_msg::DEFAULT_WAKE_TIMEOUT, "inbox");
    // Typed or not (no agent to type into), the item is in the inbox.
    let _ = r;
    l.pointer_done = true;
    true
}

/// The message (and its files) as one inbox package.
fn write_inbox(i: &Inbound, ws: &crate::postal_routes::Ws, text: &str) -> Result<(String, String, usize), String> {
    let dir = files::inbound_root().join(format!("{}.files", i.delivery_id));
    k2_core::fs_atomic::create_private_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut paths = Vec::new();
    let body_path = dir.join("message.md");
    let quoted = crate::workspace_msg::quote_every_line(text);
    let cover = format!("From {} (Postal, badge {})\n\n{}\n", i.from, i.badge, quoted);
    k2_core::fs_atomic::atomic_write_private(&body_path, cover.as_bytes()).map_err(|e| e.to_string())?;
    paths.push(body_path);
    for f in &i.files {
        let p = dir.join(&f.storage_name);
        k2_core::fs_atomic::atomic_write_private(&p, &Inbound::file_bytes(f)).map_err(|e| e.to_string())?;
        paths.push(p);
    }
    let title = format!("Postal from {}", i.from);
    let r = k2_core::inbox::deliver_files(std::path::Path::new(&ws.path), &paths, Some(&title), Some(&i.from), Some("postal"), None);
    let _ = std::fs::remove_dir_all(&dir);
    let pkg = r?;
    Ok((pkg.id, pkg.title, paths.len()))
}

#[cfg(test)]
#[path = "receive_tests.rs"]
mod tests;
