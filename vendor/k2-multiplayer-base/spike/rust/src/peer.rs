//! Stdio JSON-lines peer for the Node interop harness.
//!
//! The peer plays "the daemon": one Yrs doc, plus an agent replica that edits by
//! text diff. Binary payloads travel as base64. One request per line, one reply per line.
use std::io::{BufRead, Write};

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde_json::{json, Value};
use similar::{ChangeTag, TextDiff};
use yrs::encoding::read::Cursor;
use yrs::sync::{Awareness, AwarenessUpdate, Message, SyncMessage};
use yrs::updates::decoder::{Decode, DecoderV1};
use yrs::updates::encoder::{Encode, Encoder, EncoderV1};
use yrs::{Doc, GetString, ReadTxn, StateVector, Text, TextRef, Transact, Update};

use crate::ydoc;

fn b64(v: &[u8]) -> String {
    B64.encode(v)
}
fn unb64(v: &Value) -> Vec<u8> {
    B64.decode(v.as_str().expect("base64 string")).expect("base64")
}

/// Apply `new` over `cur` on `doc` as the smallest edit: a line diff, then a char diff
/// inside each changed hunk. Returns the encoded update. Offsets are UTF-16.
pub fn apply_text_diff(doc: &Doc, text: &TextRef, cur: &str, new: &str) -> Vec<u8> {
    let mut txn = doc.transact_mut();
    let lines = TextDiff::configure().algorithm(similar::Algorithm::Myers).diff_lines(cur, new);
    // Walk hunks; keep a running UTF-16 cursor into the evolving text.
    let mut pos16: u32 = 0;
    for op in lines.ops() {
        let (tag, old_r, new_r) = op.as_tag_tuple();
        let old_s: String = lines.old_slices()[old_r.clone()].concat();
        let new_s: String = lines.new_slices()[new_r.clone()].concat();
        match tag {
            similar::DiffTag::Equal => pos16 += old_s.encode_utf16().count() as u32,
            _ => {
                // char diff inside the hunk
                let chars = TextDiff::from_chars(&old_s, &new_s);
                for ch in chars.iter_all_changes() {
                    let v = ch.value();
                    let n = v.encode_utf16().count() as u32;
                    match ch.tag() {
                        ChangeTag::Equal => pos16 += n,
                        ChangeTag::Delete => text.remove_range(&mut txn, pos16, n),
                        ChangeTag::Insert => {
                            text.insert(&mut txn, pos16, v);
                            pos16 += n;
                        }
                    }
                }
            }
        }
    }
    txn.encode_update_v1()
}

pub fn run() {
    let mut daemon = ydoc(1);
    let mut dtext = daemon.get_or_insert_text("source");
    let mut agent = ydoc(2);
    let mut atext = agent.get_or_insert_text("source");
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line.expect("stdin");
        let req: Value = serde_json::from_str(&line).expect("request json");
        let reply = match req["c"].as_str().unwrap() {
            "reset" => {
                daemon = ydoc(req["daemon"].as_u64().unwrap_or(1));
                dtext = daemon.get_or_insert_text("source");
                agent = ydoc(req["agent"].as_u64().unwrap_or(2));
                atext = agent.get_or_insert_text("source");
                json!({"ok": true})
            }
            // Apply a client update; report the clients it inserts under.
            "apply" => {
                let bytes = unb64(&req["u"]);
                match Update::decode_v1(&bytes) {
                    Ok(u) => {
                        let mut ins: Vec<u64> = u.insertions(true).client_ids().map(|c| c.get()).collect();
                        ins.sort();
                        let r = daemon.transact_mut().apply_update(u);
                        json!({"ok": r.is_ok(), "err": r.err().map(|e| e.to_string()), "inserts": ins})
                    }
                    Err(e) => json!({"ok": false, "err": e.to_string()}),
                }
            }
            "sv" => json!({"sv": b64(&daemon.transact().state_vector().encode_v1())}),
            // Sync step 2: what the client with state vector `sv` lacks.
            "diff" => {
                let sv = StateVector::decode_v1(&unb64(&req["sv"])).unwrap();
                json!({"u": b64(&daemon.transact().encode_state_as_update_v1(&sv))})
            }
            "text" => json!({"text": dtext.get_string(&daemon.transact())}),
            // The agent writes the whole text; its replica is synced first, the diff
            // becomes ops, and the resulting update is applied to the daemon.
            "agent_write" => {
                let sv = agent.transact().state_vector();
                let d = daemon.transact().encode_state_as_update_v1(&sv);
                agent.transact_mut().apply_update(Update::decode_v1(&d).unwrap()).unwrap();
                let cur = atext.get_string(&agent.transact());
                let new = req["new"].as_str().unwrap();
                let u = apply_text_diff(&agent, &atext, &cur, new);
                daemon.transact_mut().apply_update(Update::decode_v1(&u).unwrap()).unwrap();
                json!({"u": b64(&u), "bytes": u.len()})
            }
            // Agent edit expressed as a function of the *agent's* current view.
            "agent_upcase_word" => {
                let sv = agent.transact().state_vector();
                let d = daemon.transact().encode_state_as_update_v1(&sv);
                agent.transact_mut().apply_update(Update::decode_v1(&d).unwrap()).unwrap();
                let cur = atext.get_string(&agent.transact());
                let new = upcase_nth_word(&cur, req["n"].as_u64().unwrap() as usize);
                let u = apply_text_diff(&agent, &atext, &cur, &new);
                daemon.transact_mut().apply_update(Update::decode_v1(&u).unwrap()).unwrap();
                json!({"u": b64(&u)})
            }
            // y-protocols message: decode with yrs and re-encode; report kind + byte equality.
            "msg" => {
                let bytes = unb64(&req["m"]);
                let mut dec = DecoderV1::new(Cursor::new(&bytes));
                match Message::decode(&mut dec) {
                    Ok(m) => {
                        let kind = match &m {
                            Message::Sync(SyncMessage::SyncStep1(_)) => "sync1",
                            Message::Sync(SyncMessage::SyncStep2(_)) => "sync2",
                            Message::Sync(SyncMessage::Update(_)) => "update",
                            Message::Awareness(_) => "awareness",
                            _ => "other",
                        };
                        let detail = match &m {
                            Message::Awareness(a) => {
                                let mut v: Vec<Value> = a.clients.iter().map(|(c, e)| json!({"client": c.get(), "clock": e.clock, "json": e.json.to_string()})).collect();
                                v.sort_by_key(|x| x["client"].as_u64());
                                json!(v)
                            }
                            _ => Value::Null,
                        };
                        let mut enc = EncoderV1::new();
                        m.encode(&mut enc);
                        let re = enc.to_vec();
                        json!({"kind": kind, "same_bytes": re == bytes, "detail": detail})
                    }
                    Err(e) => json!({"kind": "error", "err": e.to_string()}),
                }
            }
            // Build y-protocols messages in Rust for Node to decode.
            "make_msgs" => {
                let sv = daemon.transact().state_vector();
                let s1 = Message::Sync(SyncMessage::SyncStep1(sv)).encode_v1();
                let s2 = Message::Sync(SyncMessage::SyncStep2(
                    daemon.transact().encode_state_as_update_v1(&StateVector::default()),
                ))
                .encode_v1();
                let mut aw = Awareness::new(ydoc(77));
                aw.set_local_state(json!({"user": {"name": "agent-x", "color": "#ff8800"}, "cursor": null})).unwrap();
                let au: AwarenessUpdate = aw.update().unwrap();
                let am = Message::Awareness(au).encode_v1();
                json!({"sync1": b64(&s1), "sync2": b64(&s2), "awareness": b64(&am)})
            }
            "mem" => json!({"live": crate::alloc::live()}),
            // Breakdown: heap held by the live daemon doc, by the agent replica, and by a
            // doc reloaded from the daemon's own encoded state (in-memory compaction).
            "mem_breakdown" => {
                let state = daemon.transact().encode_state_as_update_v1(&StateVector::default());
                let total = crate::alloc::live();
                let b0 = crate::alloc::live();
                let fresh = ydoc(1);
                let ft = fresh.get_or_insert_text("source");
                fresh.transact_mut().apply_update(Update::decode_v1(&state).unwrap()).unwrap();
                let reloaded = crate::alloc::live() - b0;
                assert_eq!(ft.get_string(&fresh.transact()), dtext.get_string(&daemon.transact()));
                drop(fresh);
                let b1 = crate::alloc::live();
                drop(std::mem::replace(&mut agent, ydoc(2)));
                atext = agent.get_or_insert_text("source");
                let agent_freed = b1 - crate::alloc::live();
                let b2 = crate::alloc::live();
                // swap the daemon doc for its reloaded twin: same item IDs, same text
                let twin = ydoc(1);
                let tt = twin.get_or_insert_text("source");
                twin.transact_mut().apply_update(Update::decode_v1(&state).unwrap()).unwrap();
                let old = std::mem::replace(&mut daemon, twin);
                dtext = tt;
                drop(old);
                let after_swap = crate::alloc::live();
                json!({"total_before": total, "agent_replica": agent_freed, "reloaded_doc": reloaded,
                       "daemon_live_doc_freed_by_swap": (b2 + reloaded).saturating_sub(after_swap),
                       "state_bytes": state.len(), "total_after_swap_and_no_replica": after_swap})
            }
            other => json!({"err": format!("unknown command {other}")}),
        };
        writeln!(out, "{}", reply).unwrap();
        out.flush().unwrap();
    }
}

fn upcase_nth_word(s: &str, n: usize) -> String {
    let words: Vec<(usize, &str)> = s
        .split_inclusive(char::is_whitespace)
        .scan(0usize, |off, w| {
            let o = *off;
            *off += w.len();
            Some((o, w))
        })
        .filter(|(_, w)| w.trim().len() > 2)
        .collect();
    if words.is_empty() {
        return s.to_string();
    }
    let (o, w) = words[n % words.len()];
    let t = w.trim_end();
    format!("{}{}{}", &s[..o], t.to_uppercase(), &s[o + t.len()..])
}
