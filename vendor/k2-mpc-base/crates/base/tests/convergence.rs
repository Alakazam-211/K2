//! Seeded convergence simulator (v1).
//!
//! A session engine plays the server. Clients are raw Yrs docs, standing in for Yjs
//! (D0 proved byte-level interop), with their own client IDs. Every tick, clients type
//! and delete at random spots. Delivery is delayed and reordered per client, clients go
//! offline and resync by state vector, and agents edit the session directly
//! (`replace_text_as`) or write whole texts against an old base (`merge3`, as `write --base`).
//!
//! Asserts, loudly:
//! - every client update passes the session's client-ID check;
//! - once everything is delivered, every client equals the session, byte for byte;
//! - `write --base` either applies exactly the agent's change or reports a conflict.
//!
//! A failing seed reproduces exactly: `SIM_SEED=<n> cargo test --test convergence`. A wider
//! sweep: `SIM_SEEDS=200`.

use std::collections::VecDeque;

use rand::{rngs::SmallRng, Rng, SeedableRng};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{Doc as YDoc, GetString, OffsetKind, Options, ReadTxn, Text, TextRef, Transact, Update};

use k2_mpc_base::text::merge::{merge3, Merge};
use k2_mpc_base::{Doc, Edit, Kind, Schema};

const S: Schema = Schema {
    name: "test",
    roots: &[(ROOT, Kind::Text)],
};
const ROOT: &str = "source";

struct Client {
    id: u64,
    doc: YDoc,
    text: TextRef,
    online: bool,
    offline_until: u32,
    /// Server -> client messages in flight: (deliver at tick, update).
    inbox: VecDeque<(u32, Vec<u8>)>,
    /// Client -> server messages in flight.
    outbox: VecDeque<(u32, Vec<u8>)>,
}

fn client(id: u64) -> Client {
    let doc = YDoc::with_options(Options {
        client_id: yrs::block::ClientID::new(id),
        offset_kind: OffsetKind::Utf16,
        ..Options::default()
    });
    let text = doc.get_or_insert_text(ROOT);
    Client {
        id,
        doc,
        text,
        online: true,
        offline_until: 0,
        inbox: VecDeque::new(),
        outbox: VecDeque::new(),
    }
}

const SNIPPETS: &[&str] = &[
    "a", "e", " ", "the ", "\n", "## ", "- [ ] ", "**", "é", "日本", "🎉", "`x`",
];

/// A UTF-16 index that doesn't split a surrogate pair (editors never do).
fn safe16(s: &str, want: u32) -> u32 {
    let mut at = 0u32;
    for c in s.chars() {
        let w = c.len_utf16() as u32;
        if at + w > want {
            return at;
        }
        at += w;
    }
    at
}

fn sync_from_session(c: &mut Client, s: &Doc) {
    let sv = c.doc.transact().state_vector().encode_v1();
    let u = s.encode_diff(&sv).unwrap();
    c.doc
        .transact_mut()
        .apply_update(Update::decode_v1(&u).unwrap())
        .unwrap();
}

fn run(seed: u64, ticks: u32) {
    let mut r = SmallRng::seed_from_u64(seed);
    let mut s = Doc::new(S).unwrap();
    s.replace_text_as(2, ROOT, "# Shared notes\n\nfirst paragraph\n")
        .unwrap(); // "imported from disk"
    let mut cs: Vec<Client> = (0..4).map(|i| client(100 + i)).collect();
    for c in cs.iter_mut() {
        sync_from_session(c, &s);
    }
    // Agent bases kept for write --base: (agent id, version text read at that point)
    let mut agent_reads: Vec<(u64, String)> = Vec::new();
    let (mut merges_clean, mut merges_conflict) = (0, 0);

    for tick in 0..ticks {
        // 1. clients type
        for c in cs.iter_mut() {
            if r.gen_bool(0.7) {
                let cur = c.text.get_string(&c.doc.transact());
                let len16 = cur.encode_utf16().count() as u32;
                let upd = {
                    let mut t = c.doc.transact_mut();
                    if len16 > 20 && r.gen_bool(0.2) {
                        let p = safe16(&cur, r.gen_range(0..len16 - 1));
                        let end = safe16(&cur, (p + r.gen_range(1..5)).min(len16));
                        if end > p {
                            c.text.remove_range(&mut t, p, end - p);
                        }
                    } else {
                        let p = safe16(&cur, r.gen_range(0..=len16));
                        c.text
                            .insert(&mut t, p, SNIPPETS[r.gen_range(0..SNIPPETS.len())]);
                    }
                    t.encode_update_v1()
                };
                c.outbox.push_back((tick + r.gen_range(0..3), upd));
            }
        }
        // 2. connectivity: drop, and reconnect with a state-vector resync both ways
        for i in 0..cs.len() {
            let c = &mut cs[i];
            if c.online && r.gen_bool(0.01) {
                c.online = false;
                c.offline_until = tick + r.gen_range(5..40);
                c.inbox.clear(); // in-flight frames to a dropped socket are lost
            } else if !c.online && tick >= c.offline_until {
                // client's step 2: everything the session lacks (only its own content)
                let sv = s.state_vector();
                let mine = c
                    .doc
                    .transact()
                    .encode_state_as_update_v1(&yrs::StateVector::decode_v1(&sv).unwrap());
                c.outbox.clear(); // superseded by the resync
                let info = s.apply_client_update(&mine, c.id).unwrap_or_else(|e| {
                    panic!("seed {seed} tick {tick}: resync from {} refused: {e}", c.id)
                });
                assert!(info.inserts_from.iter().all(|&x| x == c.id));
                sync_from_session(c, &s);
                c.online = true;
                for o in cs.iter_mut() {
                    if o.online && o.id != cs_id(i) {
                        o.inbox.push_back((tick + 1, mine.clone()));
                    }
                }
            }
        }
        // 3. deliver client -> session (only from online clients, in order per client)
        for i in 0..cs.len() {
            if !cs[i].online {
                continue;
            }
            while cs[i].outbox.front().is_some_and(|(at, _)| *at <= tick) {
                let (_, u) = cs[i].outbox.pop_front().unwrap();
                let id = cs[i].id;
                match s.apply_client_update(&u, id) {
                    Ok(_) => {}
                    Err(k2_mpc_base::DocError::MissingDeps { .. }) => {
                        // an earlier frame was lost with a drop: the client resyncs, as a real provider does
                        let sv = s.state_vector();
                        let mine = cs[i]
                            .doc
                            .transact()
                            .encode_state_as_update_v1(&yrs::StateVector::decode_v1(&sv).unwrap());
                        s.apply_client_update(&mine, id).unwrap();
                        cs[i].outbox.clear();
                        broadcast(&mut cs, id, tick, mine);
                        continue;
                    }
                    Err(e) => panic!("seed {seed} tick {tick}: update from {id} refused: {e}"),
                }
                broadcast(&mut cs, id, tick + r.gen_range(0..3), u);
            }
        }
        // 4. agents
        if tick % 25 == 7 {
            let id = 5000 + (tick as u64 % 3);
            let cur = s.text(ROOT).unwrap();
            let new = if r.gen_bool(0.5) {
                format!("{cur}\n> agent {id} note at {tick}\n")
            } else {
                cur.replacen("the ", "THE ", 1)
            };
            if let Some(u) = s.replace_text_as(id, ROOT, &new).unwrap() {
                broadcast(&mut cs, id, tick + 1, u);
            }
            agent_reads.push((id, s.text(ROOT).unwrap()));
        }
        if tick % 40 == 39 && !agent_reads.is_empty() {
            // write --base: the agent rewrites the version it read some time ago
            let (id, base) = agent_reads.remove(r.gen_range(0..agent_reads.len()));
            let ours = base.replacen("\n", "\n<!-- reviewed -->\n", 1);
            let live = s.text(ROOT).unwrap();
            match merge3(&base, &ours, &live) {
                Merge::Clean(m) => {
                    if let Some(u) = s
                        .edit_as(
                            id,
                            &[Edit::Text {
                                root: ROOT.into(),
                                hunks: m.hunks.clone(),
                            }],
                        )
                        .unwrap()
                    {
                        broadcast(&mut cs, id, tick + 1, u);
                    }
                    assert_eq!(
                        s.text(ROOT).unwrap(),
                        m.text,
                        "seed {seed}: edit_as(merge hunks) must give the merged text"
                    );
                    merges_clean += 1;
                }
                Merge::Conflicts(c) => {
                    assert!(!c.is_empty());
                    merges_conflict += 1;
                }
            }
        }
        // 5. deliver session -> clients
        for c in cs.iter_mut() {
            if !c.online {
                continue;
            }
            // reordering within the in-flight window is fine: Yjs buffers until deps arrive
            let mut keep = VecDeque::new();
            while let Some((at, u)) = c.inbox.pop_front() {
                if at <= tick {
                    c.doc
                        .transact_mut()
                        .apply_update(Update::decode_v1(&u).unwrap())
                        .unwrap();
                } else {
                    keep.push_back((at, u));
                }
            }
            c.inbox = keep;
        }
    }

    // Drain: everyone online, everything delivered, then resync to be sure nothing was lost.
    for c in cs.iter_mut() {
        let sv = s.state_vector();
        let mine = c
            .doc
            .transact()
            .encode_state_as_update_v1(&yrs::StateVector::decode_v1(&sv).unwrap());
        s.apply_client_update(&mine, c.id)
            .unwrap_or_else(|e| panic!("seed {seed}: final resync from {} refused: {e}", c.id));
    }
    let final_text = s.text(ROOT).unwrap();
    for c in cs.iter_mut() {
        sync_from_session(c, &s);
        assert_eq!(
            c.text.get_string(&c.doc.transact()),
            final_text,
            "seed {seed}: client {} diverged",
            c.id
        );
    }
    // And the store round trip keeps it.
    assert_eq!(
        Doc::load(S, &s.encode_state().unwrap())
            .unwrap()
            .text(ROOT)
            .unwrap(),
        final_text
    );
    assert!(merges_clean + merges_conflict > 0 || ticks < 40);
}

fn cs_id(i: usize) -> u64 {
    100 + i as u64
}

fn broadcast(cs: &mut [Client], from: u64, at: u32, u: Vec<u8>) {
    for c in cs.iter_mut() {
        if c.online && c.id != from {
            c.inbox.push_back((at, u.clone()));
        }
    }
}

#[test]
fn converges_across_seeds() {
    let (from, n): (u64, u64) = match std::env::var("SIM_SEED") {
        Ok(s) => (s.parse().expect("SIM_SEED must be a number"), 1),
        // SIM_SEEDS=n widens the sweep to seeds 1..=n (default 40)
        Err(_) => (
            1,
            std::env::var("SIM_SEEDS")
                .map_or(40, |n| n.parse().expect("SIM_SEEDS must be a number")),
        ),
    };
    for seed in from..from + n {
        run(seed, 600);
    }
}
