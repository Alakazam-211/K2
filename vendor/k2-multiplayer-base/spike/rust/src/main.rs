//! D0 spike: Yrs vs Loro for the Markdown core.
//!
//! Subcommands:
//!   bench <trace.json> [--label L]   replay a trace on Yrs and Loro, print JSON numbers
//!   paste                            the 1 MiB paste + 1,000 edits trace on both engines
//!   blame                            can Yrs attribute each live char to a client?
//!   forge                            does Update::insertions() catch foreign client IDs?
//!   peer                             stdio JSON-lines peer for the Node interop harness
//!
//! Throwaway code: it answers the D0 questions and seeds the C9 benchmarks.

mod alloc;
mod peer;

use std::time::Instant;

use loro::{ExportMode, LoroDoc};
use serde_json::{json, Value};
use yrs::updates::decoder::Decode;
use yrs::{
    Doc, GetString, OffsetKind, Options, ReadTxn, StateVector, Text, Transact, Update,
};

#[global_allocator]
static GLOBAL: alloc::Counting = alloc::Counting;

/// One edit, positions in UTF-16 code units (what Yjs uses).
#[derive(Clone)]
pub struct Patch {
    pub pos: u32,
    pub del: u32,
    pub ins: String,
}

pub struct Trace {
    pub patches: Vec<Patch>,
    pub end: String,
}

fn utf16_len(s: &str) -> u32 {
    s.encode_utf16().count() as u32
}

/// Load an editing-traces JSON (positions in code points) and convert positions to
/// UTF-16 by replaying on a mirror (untimed). ASCII traces skip the conversion.
pub fn load_trace(path: &str) -> Trace {
    let v: Value = serde_json::from_slice(&std::fs::read(path).expect("read trace")).expect("json");
    let end = v["endContent"].as_str().unwrap().to_string();
    let start = v["startContent"].as_str().unwrap_or("").to_string();
    assert!(start.is_empty(), "spike assumes empty start content");
    let mut raw = Vec::new();
    for t in v["txns"].as_array().unwrap() {
        for p in t["patches"].as_array().unwrap() {
            raw.push((
                p[0].as_u64().unwrap() as usize,
                p[1].as_u64().unwrap() as usize,
                p[2].as_str().unwrap().to_string(),
            ));
        }
    }
    let ascii = end.is_ascii() && raw.iter().all(|(_, _, s)| s.is_ascii());
    let mut patches = Vec::with_capacity(raw.len());
    if ascii {
        for (pos, del, ins) in raw {
            patches.push(Patch { pos: pos as u32, del: del as u32, ins });
        }
    } else {
        let mut mirror: Vec<char> = Vec::new();
        for (pos, del, ins) in raw {
            let pos16: u32 = mirror[..pos].iter().map(|c| c.len_utf16() as u32).sum();
            let del16: u32 = mirror[pos..pos + del].iter().map(|c| c.len_utf16() as u32).sum();
            mirror.splice(pos..pos + del, ins.chars());
            patches.push(Patch { pos: pos16, del: del16, ins });
        }
        let got: String = mirror.into_iter().collect();
        assert_eq!(got, end, "mirror replay must reproduce endContent");
    }
    Trace { patches, end }
}

pub fn ydoc(client: u64) -> Doc {
    let mut o = Options::default();
    o.client_id = yrs::block::ClientID::new(client);
    o.offset_kind = OffsetKind::Utf16;
    Doc::with_options(o)
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let i = ((v.len() as f64 - 1.0) * p).round() as usize;
    v[i]
}

fn us(d: std::time::Duration) -> f64 {
    d.as_secs_f64() * 1e6
}

// ---------------------------------------------------------------- Yrs

fn bench_yrs(t: &Trace) -> Value {
    // 1. Author replays the trace, one transaction per patch, encoding each update
    //    (what a client does per keystroke).
    let base = alloc::live();
    let author = ydoc(1);
    let text = author.get_or_insert_text("source");
    let mut updates: Vec<Vec<u8>> = Vec::with_capacity(t.patches.len());
    let mut local = Vec::with_capacity(t.patches.len());
    let t0 = Instant::now();
    for p in &t.patches {
        let s = Instant::now();
        let mut txn = author.transact_mut();
        if p.del > 0 {
            text.remove_range(&mut txn, p.pos, p.del);
        }
        if !p.ins.is_empty() {
            text.insert(&mut txn, p.pos, &p.ins);
        }
        updates.push(txn.encode_update_v1());
        drop(txn);
        local.push(us(s.elapsed()));
    }
    let local_total = t0.elapsed();
    assert_eq!(text.get_string(&author.transact()), t.end, "yrs author diverged from trace");
    let author_heap = alloc::live() - base;

    // 2. The daemon applies each client update (decode + apply), the hot path.
    let base_d = alloc::live();
    let daemon = ydoc(2);
    let dtext = daemon.get_or_insert_text("source");
    let mut remote = Vec::with_capacity(updates.len());
    let t1 = Instant::now();
    for u in &updates {
        let s = Instant::now();
        let upd = Update::decode_v1(u).expect("decode");
        daemon.transact_mut().apply_update(upd).expect("apply");
        remote.push(us(s.elapsed()));
    }
    let remote_total = t1.elapsed();
    assert_eq!(dtext.get_string(&daemon.transact()), t.end, "yrs daemon diverged");
    let daemon_heap_live = alloc::live() - base_d;
    drop(updates);

    // 3. Snapshot size, then a cold load from it (what a session load costs).
    let state = daemon.transact().encode_state_as_update_v1(&StateVector::default());
    let state_v2 = daemon.transact().encode_state_as_update_v2(&StateVector::default());
    drop(daemon);
    let base_l = alloc::live();
    let t2 = Instant::now();
    let loaded = ydoc(3);
    let ltext = loaded.get_or_insert_text("source");
    loaded
        .transact_mut()
        .apply_update(Update::decode_v1(&state).unwrap())
        .unwrap();
    let load_ms = t2.elapsed().as_secs_f64() * 1e3;
    let loaded_heap = alloc::live() - base_l;
    assert_eq!(ltext.get_string(&loaded.transact()), t.end);

    // 4. A per-participant replica (agent) synced from the loaded doc.
    let base_r = alloc::live();
    let replica = ydoc(4);
    let rtext = replica.get_or_insert_text("source");
    {
        let sv = replica.transact().state_vector();
        let diff = loaded.transact().encode_state_as_update_v1(&sv);
        replica.transact_mut().apply_update(Update::decode_v1(&diff).unwrap()).unwrap();
    }
    let replica_heap = alloc::live() - base_r;

    // 5. Agent edits: whole-text replace computed by diff (line then char), 50 reps,
    //    applied on the replica and relayed into the session.
    let mut agent = Vec::new();
    let mut agent_upd_bytes = 0usize;
    for i in 0..50 {
        let cur = rtext.get_string(&replica.transact());
        let new = agent_rewrite(&cur, i);
        let s = Instant::now();
        let u = peer::apply_text_diff(&replica, &rtext, &cur, &new);
        loaded.transact_mut().apply_update(Update::decode_v1(&u).unwrap()).unwrap();
        agent.push(us(s.elapsed()));
        agent_upd_bytes += u.len();
        assert_eq!(rtext.get_string(&replica.transact()), new);
    }
    assert_eq!(ltext.get_string(&loaded.transact()), rtext.get_string(&replica.transact()));

    let text_bytes = t.end.len();
    json!({
        "engine": "yrs 0.28.0",
        "ops": t.patches.len(),
        "text_bytes": text_bytes,
        "local_edit_us": {"p50": pct(&mut local, 0.5), "p95": pct(&mut local, 0.95), "p99": pct(&mut local, 0.99), "total_ms": local_total.as_secs_f64()*1e3},
        "remote_apply_us": {"p50": pct(&mut remote, 0.5), "p95": pct(&mut remote, 0.95), "p99": pct(&mut remote, 0.99), "total_ms": remote_total.as_secs_f64()*1e3},
        "author_heap_bytes": author_heap,
        "daemon_heap_after_replay_bytes": daemon_heap_live,
        "state_v1_bytes": state.len(),
        "state_v2_bytes": state_v2.len(),
        "cold_load_ms": load_ms,
        "loaded_heap_bytes": loaded_heap,
        "replica_heap_bytes": replica_heap,
        "agent_edit_us": {"p50": pct(&mut agent, 0.5), "p95": pct(&mut agent, 0.95)},
        "agent_update_avg_bytes": agent_upd_bytes / 50,
        "budget_bytes": 3 * text_bytes + 2 * 1024 * 1024,
        "session_estimate_bytes": loaded_heap + replica_heap,
    })
}

/// A deterministic "agent" edit: change one word mid-doc and append a short paragraph.
fn agent_rewrite(cur: &str, i: usize) -> String {
    let mid = cur.len() / 2;
    let mut cut = mid;
    while !cur.is_char_boundary(cut) {
        cut += 1;
    }
    let (a, b) = cur.split_at(cut);
    let b = b.replacen("the", "THE", 1);
    format!("{a}{b}\nAgent note {i}: checked.\n")
}

// ---------------------------------------------------------------- Loro

fn bench_loro(t: &Trace) -> Value {
    let base = alloc::live();
    let author = LoroDoc::new();
    author.set_peer_id(1).unwrap();
    let text = author.get_text("source");
    let collected = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Vec<u8>>::new()));
    let c2 = collected.clone();
    let _sub = author.subscribe_local_update(Box::new(move |b| {
        c2.lock().unwrap().push(b.clone());
        true
    }));
    let mut local = Vec::with_capacity(t.patches.len());
    let t0 = Instant::now();
    for p in &t.patches {
        let s = Instant::now();
        if p.del > 0 {
            text.delete_utf16(p.pos as usize, p.del as usize).unwrap();
        }
        if !p.ins.is_empty() {
            text.insert_utf16(p.pos as usize, &p.ins).unwrap();
        }
        author.commit();
        local.push(us(s.elapsed()));
    }
    let local_total = t0.elapsed();
    assert_eq!(text.to_string(), t.end, "loro author diverged");
    let author_heap = alloc::live() - base;
    drop(_sub);
    let updates = std::mem::take(&mut *collected.lock().unwrap());

    let base_d = alloc::live();
    let daemon = LoroDoc::new();
    daemon.set_peer_id(2).unwrap();
    let mut remote = Vec::with_capacity(updates.len());
    let t1 = Instant::now();
    for u in &updates {
        let s = Instant::now();
        daemon.import(u).expect("import");
        remote.push(us(s.elapsed()));
    }
    let remote_total = t1.elapsed();
    assert_eq!(daemon.get_text("source").to_string(), t.end, "loro daemon diverged");
    let daemon_heap_live = alloc::live() - base_d;
    let n_updates = updates.len();
    drop(updates);

    let snap = daemon.export(ExportMode::Snapshot).unwrap();
    let state_only = daemon.export(ExportMode::StateOnly(None)).unwrap();
    drop(daemon);

    let base_l = alloc::live();
    let t2 = Instant::now();
    let loaded = LoroDoc::new();
    loaded.import(&snap).unwrap();
    let ltext = loaded.get_text("source");
    let s_ = ltext.to_string(); // force state materialisation
    let load_ms = t2.elapsed().as_secs_f64() * 1e3;
    assert_eq!(s_, t.end);
    drop(s_);
    let loaded_heap = alloc::live() - base_l;

    let base_so = alloc::live();
    let so = LoroDoc::new();
    so.import(&state_only).unwrap();
    assert_eq!(so.get_text("source").to_string(), t.end);
    let state_only_heap = alloc::live() - base_so;
    drop(so);

    let base_r = alloc::live();
    let replica = LoroDoc::new();
    replica.set_peer_id(4).unwrap();
    replica.import(&loaded.export(ExportMode::Snapshot).unwrap()).unwrap();
    let rtext = replica.get_text("source");
    let replica_heap = alloc::live() - base_r;

    let mut agent = Vec::new();
    for i in 0..50 {
        let cur = rtext.to_string();
        let new = agent_rewrite(&cur, i);
        let s = Instant::now();
        let vv = replica.oplog_vv();
        rtext.update(&new, Default::default()).unwrap();
        replica.commit();
        let u = replica.export(ExportMode::updates(&vv)).unwrap();
        loaded.import(&u).unwrap();
        agent.push(us(s.elapsed()));
    }
    assert_eq!(ltext.to_string(), rtext.to_string());

    let text_bytes = t.end.len();
    json!({
        "engine": "loro 1.17.1",
        "ops": t.patches.len(),
        "updates": n_updates,
        "text_bytes": text_bytes,
        "local_edit_us": {"p50": pct(&mut local, 0.5), "p95": pct(&mut local, 0.95), "p99": pct(&mut local, 0.99), "total_ms": local_total.as_secs_f64()*1e3},
        "remote_apply_us": {"p50": pct(&mut remote, 0.5), "p95": pct(&mut remote, 0.95), "p99": pct(&mut remote, 0.99), "total_ms": remote_total.as_secs_f64()*1e3},
        "author_heap_bytes": author_heap,
        "daemon_heap_after_replay_bytes": daemon_heap_live,
        "snapshot_bytes": snap.len(),
        "state_only_bytes": state_only.len(),
        "cold_load_ms": load_ms,
        "loaded_heap_bytes": loaded_heap,
        "state_only_loaded_heap_bytes": state_only_heap,
        "replica_heap_bytes": replica_heap,
        "agent_edit_us": {"p50": pct(&mut agent, 0.5), "p95": pct(&mut agent, 0.95)},
        "budget_bytes": 3 * text_bytes + 2 * 1024 * 1024,
        "session_estimate_bytes": loaded_heap + replica_heap,
    })
}

// ---------------------------------------------------------------- traces

/// 1 MiB paste into an empty doc, then 1,000 small seeded edits.
fn paste_trace() -> Trace {
    use rand::{rngs::SmallRng, Rng, SeedableRng};
    let mut rng = SmallRng::seed_from_u64(0xD0);
    let line = "Lorem ipsum dolor sit amet, **consectetur** adipiscing elit — sed do [eiusmod](x.md) tempor.\n";
    let mut big = String::new();
    while big.len() < 1024 * 1024 {
        big.push_str(line);
    }
    let mut mirror: Vec<u16> = big.encode_utf16().collect();
    let mut patches = vec![Patch { pos: 0, del: 0, ins: big }];
    for _ in 0..1000 {
        let len = mirror.len() as u32;
        let pos = rng.gen_range(0..len);
        // stay off surrogate halves: the text is BMP-only, so any index is safe
        if rng.gen_bool(0.7) {
            let ins = ["a", "word ", "## H\n", "é", "日本"][rng.gen_range(0..5)].to_string();
            let enc: Vec<u16> = ins.encode_utf16().collect();
            mirror.splice(pos as usize..pos as usize, enc);
            patches.push(Patch { pos, del: 0, ins });
        } else {
            let del = rng.gen_range(1..8).min(len - pos);
            mirror.drain(pos as usize..(pos + del) as usize);
            patches.push(Patch { pos, del, ins: String::new() });
        }
    }
    Trace { patches, end: String::from_utf16(&mirror).unwrap() }
}

// ---------------------------------------------------------------- Q5 blame

fn blame() -> Value {
    // Three clients interleave edits; then ask Yrs which client wrote each live char.
    let a = ydoc(11);
    let b = ydoc(22);
    let c = ydoc(33);
    let docs = [&a, &b, &c];
    let texts: Vec<_> = docs.iter().map(|d| d.get_or_insert_text("source")).collect();
    let sync_all = || {
        for i in 0..3 {
            for j in 0..3 {
                if i != j {
                    let sv = docs[j].transact().state_vector();
                    let u = docs[i].transact().encode_state_as_update_v1(&sv);
                    docs[j].transact_mut().apply_update(Update::decode_v1(&u).unwrap()).unwrap();
                }
            }
        }
    };
    texts[0].insert(&mut a.transact_mut(), 0, "aaaa\n");
    sync_all();
    texts[1].insert(&mut b.transact_mut(), 2, "BB");
    sync_all();
    texts[2].insert(&mut c.transact_mut(), 7, "ccc\n");
    texts[0].remove_range(&mut a.transact_mut(), 0, 1);
    sync_all();
    let s = texts[0].get_string(&a.transact());

    // Approach: diff_range(now, empty snapshot) marks every live char "Added" with its item ID.
    let mut txn = a.transact_mut();
    let now = txn.snapshot();
    let empty = yrs::Snapshot::default();
    let diffs = texts[0].diff_range(&mut txn, Some(&now), Some(&empty), |ch| ch);
    let mut runs = Vec::new();
    for d in diffs {
        let ins = match &d.insert {
            yrs::Out::Any(yrs::Any::String(s)) => s.to_string(),
            other => format!("{other:?}"),
        };
        let who = d.ychange.as_ref().map(|y| (y.id.client.get(), format!("{:?}", y.kind)));
        runs.push(json!({"text": ins, "ychange": who}));
    }
    json!({"text": s, "runs": runs, "gc": "default (on)"})
}

// ---------------------------------------------------------------- Q3 forgery

fn forge() -> Value {
    // The connection was assigned client 100. Check three updates against it.
    let assigned = 100u64;
    let honest = ydoc(assigned);
    let ht = honest.get_or_insert_text("source");
    let other = ydoc(200);
    let ot = other.get_or_insert_text("source");

    ot.insert(&mut other.transact_mut(), 0, "owned by 200\n");
    let seed = other.transact().encode_state_as_update_v1(&StateVector::default());
    honest.transact_mut().apply_update(Update::decode_v1(&seed).unwrap()).unwrap();

    let check = |bytes: &[u8]| -> (bool, Vec<u64>) {
        let u = Update::decode_v1(bytes).unwrap();
        let clients: Vec<u64> = u.insertions(true).client_ids().map(|c| c.get()).collect();
        let upper: Vec<u64> = u.state_vector().iter().map(|(c, _)| c.get()).collect();
        let ok = clients.iter().chain(upper.iter()).all(|c| *c == assigned);
        let mut all = clients;
        all.extend(upper);
        all.sort();
        all.dedup();
        (ok, all)
    };

    // (a) honest insert by 100
    let ua = {
        let mut t = honest.transact_mut();
        ht.insert(&mut t, 0, "hi ");
        t.encode_update_v1()
    };
    // (b) honest delete of 200's text, by 100
    let ub = {
        let mut t = honest.transact_mut();
        ht.remove_range(&mut t, 3, 5);
        t.encode_update_v1()
    };
    // (c) forged: a doc pretending to be 200 inserts, sent over 100's connection
    let fake = ydoc(200);
    let ft = fake.get_or_insert_text("source");
    fake.transact_mut().apply_update(Update::decode_v1(&seed).unwrap()).unwrap();
    let uc = {
        let mut t = fake.transact_mut();
        ft.insert(&mut t, 0, "forged ");
        t.encode_update_v1()
    };
    // (d) a full-state update relayed by 100 (contains 200's items): must be refused
    let ud = honest.transact().encode_state_as_update_v1(&StateVector::default());

    let (a_ok, a_c) = check(&ua);
    let (b_ok, b_c) = check(&ub);
    let (c_ok, c_c) = check(&uc);
    let (d_ok, d_c) = check(&ud);
    assert!(a_ok && b_ok && !c_ok && !d_ok, "forgery check failed");
    json!({
        "assigned": assigned,
        "honest_insert": {"accepted": a_ok, "clients": a_c},
        "honest_delete_of_others": {"accepted": b_ok, "clients": b_c},
        "forged_insert_as_200": {"accepted": c_ok, "clients": c_c},
        "relayed_full_state": {"accepted": d_ok, "clients": d_c},
    })
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("bench") => {
            let path = &args[2];
            let label = args.iter().position(|a| a == "--label").map(|i| args[i + 1].clone()).unwrap_or_else(|| path.clone());
            let only = args.iter().position(|a| a == "--engine").map(|i| args[i + 1].clone());
            let t = load_trace(path);
            let mut out = json!({"trace": label});
            if only.as_deref() != Some("loro") {
                out["yrs"] = bench_yrs(&t);
            }
            if only.as_deref() != Some("yrs") {
                out["loro"] = bench_loro(&t);
            }
            println!("{}", serde_json::to_string_pretty(&out).unwrap());
        }
        Some("paste") => {
            let t = paste_trace();
            let out = json!({"trace": "paste-1MiB+1000", "yrs": bench_yrs(&t), "loro": bench_loro(&t)});
            println!("{}", serde_json::to_string_pretty(&out).unwrap());
        }
        Some("blame") => println!("{}", serde_json::to_string_pretty(&blame()).unwrap()),
        Some("forge") => println!("{}", serde_json::to_string_pretty(&forge()).unwrap()),
        Some("peer") => peer::run(),
        Some("empty") => {
            let b = alloc::live();
            let d = ydoc(9);
            let t = d.get_or_insert_text("source");
            let one = alloc::live() - b;
            t.insert(&mut d.transact_mut(), 0, "x");
            let after_insert = alloc::live() - b;
            let mut many = Vec::new();
            let b2 = alloc::live();
            for i in 0..100u64 { let d = ydoc(100 + i); let t = d.get_or_insert_text("source"); t.insert(&mut d.transact_mut(), 0, "x"); many.push(d); }
            let per = (alloc::live() - b2) / 100;
            let b3 = alloc::live();
            let l = LoroDoc::new(); l.get_text("source").insert(0, "x").unwrap(); l.commit();
            let loro_one = alloc::live() - b3;
            println!("{}", json!({"yrs_empty_doc": one, "yrs_after_1_insert": after_insert, "yrs_per_doc_of_100": per, "loro_1_insert": loro_one}));
        }
        _ => {
            eprintln!("usage: d0-spike bench <trace.json> [--label L] [--engine yrs|loro] | paste | blame | forge | peer");
            std::process::exit(2);
        }
    }
}
