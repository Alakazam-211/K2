//! Replay ITF traces of `spec/quint/text.qnt` (from `quint run --out-itf`) on the text
//! reference model (PRD E-PROOF-7).
//!
//! `itf_replay [--rule fuguemax|fugue] [--expect-mismatch] <trace.itf.json | dir>...`
//!
//! Each step of a trace is classified by its state diff:
//! - a new insert (a, s) in `ev`: agent a inserts on its reference replica at the visible
//!   position just after the trace event's left origin; the reference event must have the
//!   same ID, left and right origins, and parents;
//! - a new delete: agent a deletes the trace event's target, by visible position;
//! - one replica gains one event: that event is delivered to its reference replica;
//! - no change (a stutter, i.e. a re-delivery): every reference replica gets each of its
//!   events delivered again, which must change nothing.
//!
//! After every step, for every replica and the ghost (a reference replica that receives each
//! event when it is generated): list with tombstones, visible list and frontier must equal
//! the trace's. Exit 0: all traces replayed with no mismatch (or, with --expect-mismatch, at
//! least one mismatch). Exit 1 otherwise.
#![forbid(unsafe_code)]

use mpce_model::text::{Doc, Event, Id, Op};
use mpce_model::tree::{Origin, SiblingRule};
use std::collections::{BTreeMap, BTreeSet};

// ---------------------------------------------------------------------------------------
// Minimal JSON

#[derive(Debug, Clone)]
enum J {
    Null,
    Bool,
    Num(String),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(String, J)>),
}

struct P<'a> {
    s: &'a [u8],
    i: usize,
}

impl P<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && (self.s[self.i] as char).is_ascii_whitespace() {
            self.i += 1;
        }
    }
    fn val(&mut self) -> Result<J, String> {
        self.ws();
        let c = *self.s.get(self.i).ok_or("eof")?;
        match c {
            b'{' => {
                self.i += 1;
                let mut v = Vec::new();
                self.ws();
                if self.s[self.i] == b'}' {
                    self.i += 1;
                    return Ok(J::Obj(v));
                }
                loop {
                    self.ws();
                    let k = match self.val()? {
                        J::Str(k) => k,
                        _ => return Err("key".into()),
                    };
                    self.ws();
                    if self.s[self.i] != b':' {
                        return Err(format!("':' at {}", self.i));
                    }
                    self.i += 1;
                    v.push((k, self.val()?));
                    self.ws();
                    match self.s[self.i] {
                        b',' => self.i += 1,
                        b'}' => {
                            self.i += 1;
                            return Ok(J::Obj(v));
                        }
                        _ => return Err(format!("object at {}", self.i)),
                    }
                }
            }
            b'[' => {
                self.i += 1;
                let mut v = Vec::new();
                self.ws();
                if self.s[self.i] == b']' {
                    self.i += 1;
                    return Ok(J::Arr(v));
                }
                loop {
                    v.push(self.val()?);
                    self.ws();
                    match self.s[self.i] {
                        b',' => self.i += 1,
                        b']' => {
                            self.i += 1;
                            return Ok(J::Arr(v));
                        }
                        _ => return Err(format!("array at {}", self.i)),
                    }
                }
            }
            b'"' => {
                self.i += 1;
                let mut out = String::new();
                loop {
                    let c = *self.s.get(self.i).ok_or("eof in string")?;
                    self.i += 1;
                    match c {
                        b'"' => return Ok(J::Str(out)),
                        b'\\' => {
                            let e = self.s[self.i];
                            self.i += 1;
                            match e {
                                b'n' => out.push('\n'),
                                b't' => out.push('\t'),
                                b'r' => out.push('\r'),
                                b'b' => out.push('\u{8}'),
                                b'f' => out.push('\u{c}'),
                                b'u' => {
                                    let h = std::str::from_utf8(&self.s[self.i..self.i + 4]).map_err(|e| e.to_string())?;
                                    self.i += 4;
                                    out.push(char::from_u32(u32::from_str_radix(h, 16).map_err(|e| e.to_string())?).unwrap_or('?'));
                                }
                                other => out.push(other as char),
                            }
                        }
                        _ => {
                            // copy one UTF-8 sequence
                            let start = self.i - 1;
                            let mut end = self.i;
                            while end < self.s.len() && (self.s[end] & 0xC0) == 0x80 {
                                end += 1;
                            }
                            out.push_str(std::str::from_utf8(&self.s[start..end]).map_err(|e| e.to_string())?);
                            self.i = end;
                        }
                    }
                }
            }
            b't' => {
                self.i += 4;
                Ok(J::Bool)
            }
            b'f' => {
                self.i += 5;
                Ok(J::Bool)
            }
            b'n' => {
                self.i += 4;
                Ok(J::Null)
            }
            _ => {
                let start = self.i;
                while self.i < self.s.len() && matches!(self.s[self.i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') {
                    self.i += 1;
                }
                if start == self.i {
                    return Err(format!("unexpected {:?} at {}", c as char, self.i));
                }
                Ok(J::Num(String::from_utf8_lossy(&self.s[start..self.i]).into_owned()))
            }
        }
    }
}

fn parse(s: &str) -> Result<J, String> {
    P { s: s.as_bytes(), i: 0 }.val()
}

impl J {
    fn get(&self, k: &str) -> Option<&J> {
        match self {
            J::Obj(v) => v.iter().find(|(kk, _)| kk == k).map(|(_, v)| v),
            _ => None,
        }
    }
    /// a state variable, by its unqualified name (`mod::text::ev` or `ev`)
    fn var(&self, name: &str) -> Option<&J> {
        match self {
            J::Obj(v) => v.iter().find(|(k, _)| k == name || k.rsplit("::").next() == Some(name)).map(|(_, v)| v),
            _ => None,
        }
    }
    // ITF encodings
    fn int(&self) -> i64 {
        match self {
            J::Num(n) => n.parse().expect("int"),
            J::Obj(_) => match self.get("#bigint") {
                Some(J::Str(s)) => s.parse().expect("bigint"),
                _ => panic!("not an int: {self:?}"),
            },
            _ => panic!("not an int: {self:?}"),
        }
    }
    fn str(&self) -> &str {
        match self {
            J::Str(s) => s,
            _ => panic!("not a string: {self:?}"),
        }
    }
    fn items(&self, tag: &str) -> &[J] {
        match self.get(tag) {
            Some(J::Arr(v)) => v,
            _ => panic!("not a {tag}: {self:?}"),
        }
    }
    fn list(&self) -> &[J] {
        match self {
            J::Arr(v) => v,
            _ => panic!("not a list: {self:?}"),
        }
    }
}

// ---------------------------------------------------------------------------------------
// The trace's view of a state

const START: (i64, i64) = (-1, 0);
const END: (i64, i64) = (-2, 0);

#[derive(Clone, Debug, PartialEq)]
struct TEv {
    parents: BTreeSet<Id>,
    del: bool,
    left: (i64, i64),
    right: (i64, i64),
    target: (i64, i64),
}

#[derive(Clone, Debug, PartialEq)]
struct TRep {
    rcv: BTreeSet<Id>,
    front: BTreeSet<Id>,
    doc: Vec<Id>,
}

#[derive(Clone, Debug)]
struct TState {
    ev: BTreeMap<Id, TEv>,
    reps: BTreeMap<u64, TRep>,
    ghost: TRep,
}

fn tup(j: &J) -> (i64, i64) {
    let v = j.items("#tup");
    (v[0].int(), v[1].int())
}
fn id(j: &J) -> Id {
    let (a, s) = tup(j);
    Id::new(a as u64, s as u64)
}
fn idset(j: &J) -> BTreeSet<Id> {
    j.items("#set").iter().map(id).collect()
}
fn rep(j: &J) -> TRep {
    TRep { rcv: idset(j.get("rcv").unwrap()), front: idset(j.get("front").unwrap()), doc: j.get("doc").unwrap().list().iter().map(id).collect() }
}

fn tstate(j: &J) -> TState {
    let mut ev = BTreeMap::new();
    for kv in j.var("ev").expect("ev").items("#map") {
        let kv = kv.list();
        let e = &kv[1];
        ev.insert(
            id(&kv[0]),
            TEv {
                parents: idset(e.get("parents").unwrap()),
                del: e.get("kind").unwrap().str() == "del",
                left: tup(e.get("left").unwrap()),
                right: tup(e.get("right").unwrap()),
                target: tup(e.get("target").unwrap()),
            },
        );
    }
    let mut reps = BTreeMap::new();
    for kv in j.var("replicas").expect("replicas").items("#map") {
        let kv = kv.list();
        reps.insert(kv[0].int() as u64, rep(&kv[1]));
    }
    TState { ev, reps, ghost: rep(j.var("ghost").expect("ghost")) }
}

fn visible(s: &TState, r: &TRep) -> Vec<Id> {
    let dead: BTreeSet<Id> = r
        .rcv
        .iter()
        .filter_map(|x| s.ev.get(x).filter(|e| e.del).map(|e| Id::new(e.target.0 as u64, e.target.1 as u64)))
        .collect();
    r.doc.iter().copied().filter(|x| !dead.contains(x)).collect()
}

fn origin(o: Origin<Id>) -> (i64, i64) {
    match o {
        Origin::Start => START,
        Origin::End => END,
        Origin::Elem(x) => (x.agent as i64, x.seq as i64),
    }
}

// ---------------------------------------------------------------------------------------
// Replay

fn compare(step: usize, who: &str, d: &Doc, s: &TState, r: &TRep) -> Result<(), String> {
    let l = d.list_with_tombstones();
    if l != r.doc {
        return Err(format!("step {step}: {who}: list with tombstones: model {l:?}, trace {:?}", r.doc));
    }
    let v = d.visible();
    let tv = visible(s, r);
    if v != tv {
        return Err(format!("step {step}: {who}: visible: model {v:?}, trace {tv:?}"));
    }
    if *d.frontier() != r.front {
        return Err(format!("step {step}: {who}: frontier: model {:?}, trace {:?}", d.frontier(), r.front));
    }
    Ok(())
}

fn replay(states: &[TState], rule: SiblingRule) -> Result<usize, String> {
    let s0 = &states[0];
    let mut docs: BTreeMap<u64, Doc> = s0.reps.keys().map(|&a| (a, Doc::with_rule(a, rule))).collect();
    let mut ghost = Doc::with_rule(0, rule);
    let mut events: BTreeMap<Id, Event> = BTreeMap::new();
    for (a, r) in &s0.reps {
        compare(0, &format!("replica {a}"), &docs[a], s0, r)?;
    }
    for (k, w) in states.windows(2).enumerate() {
        let (p, s) = (&w[0], &w[1]);
        let step = k + 1;
        let new: Vec<Id> = s.ev.keys().filter(|x| !p.ev.contains_key(x)).copied().collect();
        let changed: Vec<u64> = s.reps.keys().filter(|a| s.reps[a] != p.reps[a]).copied().collect();
        if new.len() > 1 {
            return Err(format!("step {step}: {} new events", new.len()));
        }
        if let Some(&x) = new.first() {
            let te = &s.ev[&x];
            let a = x.agent;
            let pv = visible(p, &p.reps[&a]);
            let d = docs.get_mut(&a).ok_or(format!("step {step}: unknown agent {a}"))?;
            let e = if te.del {
                let t = Id::new(te.target.0 as u64, te.target.1 as u64);
                let pos = pv.iter().position(|&y| y == t).ok_or(format!("step {step}: delete target {t:?} not visible"))?;
                d.delete(pos).map_err(|e| format!("step {step}: delete: {e:?}"))?
            } else {
                let pos = if te.left == START {
                    0
                } else {
                    let l = Id::new(te.left.0 as u64, te.left.1 as u64);
                    pv.iter().position(|&y| y == l).ok_or(format!("step {step}: left origin {l:?} not visible"))? + 1
                };
                let e = d.insert(pos, 'x').map_err(|e| format!("step {step}: insert: {e:?}"))?;
                if let Op::Insert { left, right, .. } = e.op {
                    if (origin(left), origin(right)) != (te.left, te.right) {
                        return Err(format!(
                            "step {step}: insert {x:?} at {pos}: model origins ({:?}, {:?}), trace ({:?}, {:?})",
                            origin(left),
                            origin(right),
                            te.left,
                            te.right
                        ));
                    }
                }
                e
            };
            if e.id != x || e.parents != te.parents {
                return Err(format!("step {step}: event {x:?}: model id {:?} parents {:?}, trace parents {:?}", e.id, e.parents, te.parents));
            }
            ghost.deliver(&e).map_err(|e| format!("step {step}: ghost: {e:?}"))?;
            events.insert(x, e);
        } else if changed.len() == 1 {
            let a = changed[0];
            let gained: Vec<Id> = s.reps[&a].rcv.difference(&p.reps[&a].rcv).copied().collect();
            if gained.len() != 1 {
                return Err(format!("step {step}: replica {a} gained {} events", gained.len()));
            }
            let e = &events[&gained[0]];
            match docs.get_mut(&a).unwrap().deliver(e) {
                Ok(true) => {}
                other => return Err(format!("step {step}: deliver {:?} to {a}: {other:?}", gained[0])),
            }
        } else if changed.is_empty() {
            // a stutter (re-delivery): re-deliver everything everywhere; nothing may change
            for d in docs.values_mut() {
                for e in d.log().to_vec() {
                    if d.deliver(&e) != Ok(false) {
                        return Err(format!("step {step}: re-delivery of {:?} changed replica {:?}", e.id, d.agent()));
                    }
                }
            }
        } else {
            return Err(format!("step {step}: {} replicas changed without a new event", changed.len()));
        }
        for (a, r) in &s.reps {
            compare(step, &format!("replica {a}"), &docs[a], s, r)?;
        }
        compare(step, "ghost", &ghost, s, &s.ghost)?;
    }
    Ok(states.len())
}

fn main() {
    let mut rule = SiblingRule::FugueMax;
    let mut expect_mismatch = false;
    let mut paths = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--rule" => {
                rule = match args.next().as_deref() {
                    Some("fuguemax") => SiblingRule::FugueMax,
                    Some("fugue") => SiblingRule::Fugue,
                    _ => panic!("--rule fuguemax|fugue"),
                }
            }
            "--expect-mismatch" => expect_mismatch = true,
            _ => paths.push(a),
        }
    }
    let mut files = Vec::new();
    for p in paths {
        let md = std::fs::metadata(&p).expect("path");
        if md.is_dir() {
            let mut v: Vec<_> = std::fs::read_dir(&p).unwrap().map(|e| e.unwrap().path()).filter(|p| p.to_string_lossy().ends_with(".itf.json")).collect();
            v.sort();
            files.extend(v);
        } else {
            files.push(p.into());
        }
    }
    let (mut ok, mut bad, mut steps, mut with_del, mut stutters) = (0usize, 0usize, 0usize, 0usize, 0usize);
    let mut first: Option<String> = None;
    for f in &files {
        let txt = std::fs::read_to_string(f).expect("read");
        let j = parse(&txt).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        let states: Vec<TState> = j.get("states").expect("states").list().iter().map(tstate).collect();
        if states.last().is_some_and(|s| s.ev.values().any(|e| e.del)) {
            with_del += 1;
        }
        stutters += states.windows(2).filter(|w| w[0].reps == w[1].reps && w[0].ev.len() == w[1].ev.len()).count();
        match replay(&states, rule) {
            Ok(n) => {
                ok += 1;
                steps += n - 1;
            }
            Err(e) => {
                bad += 1;
                if first.is_none() {
                    first = Some(format!("{}: {e}", f.display()));
                }
            }
        }
    }
    if let Some(m) = &first {
        println!("first mismatch: {m}");
    }
    println!(
        "SUMMARY itf_replay rule={:?} traces={} matched={} mismatched={} steps_replayed={} traces_with_deletes={} stutter_steps={}",
        rule,
        files.len(),
        ok,
        bad,
        steps,
        with_del,
        stutters
    );
    let pass = if expect_mismatch { bad > 0 } else { bad == 0 && !files.is_empty() };
    std::process::exit(if pass { 0 } else { 1 });
}
