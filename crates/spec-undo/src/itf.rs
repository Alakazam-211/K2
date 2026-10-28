//! ITF traces of `spec/quint/undo.qnt` (`quint run --out-itf`, through the wrapper
//! `undo_itf`, which adds the action taken and its outcome as variables `act` and `out`),
//! read into the bounded world's terms ([`crate::bounded`]): each state's `st` as a
//! [`View`], `act` as an [`Act`], `out` as an [`Outcome`] (lines `(1, 1)`, reports sorted).
//! A small JSON reader of its own: the crate takes no dependency for it.

use crate::bounded::{Act, VRecord, VRegion, VStacks, View};
use crate::model::hunk::Edit;
use crate::model::{Kind, Mode, Notice, Outcome, Report};

/// A JSON value.
#[derive(Debug, Clone, PartialEq)]
pub enum J {
    Null,
    Bool(bool),
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
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }
    fn peek(&mut self) -> Result<u8, String> {
        self.ws();
        self.s
            .get(self.i)
            .copied()
            .ok_or_else(|| "unexpected end".into())
    }
    fn eat(&mut self, c: u8) -> Result<(), String> {
        if self.peek()? != c {
            return Err(format!("'{}' expected at {}", c as char, self.i));
        }
        self.i += 1;
        Ok(())
    }
    fn val(&mut self) -> Result<J, String> {
        match self.peek()? {
            b'{' => {
                self.i += 1;
                let mut v = Vec::new();
                if self.peek()? == b'}' {
                    self.i += 1;
                    return Ok(J::Obj(v));
                }
                loop {
                    let J::Str(k) = self.val()? else {
                        return Err(format!("a key at {}", self.i));
                    };
                    self.eat(b':')?;
                    v.push((k, self.val()?));
                    match self.peek()? {
                        b',' => self.i += 1,
                        b'}' => {
                            self.i += 1;
                            return Ok(J::Obj(v));
                        }
                        _ => return Err(format!("',' or '}}' at {}", self.i)),
                    }
                }
            }
            b'[' => {
                self.i += 1;
                let mut v = Vec::new();
                if self.peek()? == b']' {
                    self.i += 1;
                    return Ok(J::Arr(v));
                }
                loop {
                    v.push(self.val()?);
                    match self.peek()? {
                        b',' => self.i += 1,
                        b']' => {
                            self.i += 1;
                            return Ok(J::Arr(v));
                        }
                        _ => return Err(format!("',' or ']' at {}", self.i)),
                    }
                }
            }
            b'"' => {
                self.i += 1;
                let mut out = Vec::new();
                while self.i < self.s.len() && self.s[self.i] != b'"' {
                    if self.s[self.i] == b'\\' {
                        self.i += 1;
                        let c = *self.s.get(self.i).ok_or("escape at end")?;
                        out.push(match c {
                            b'n' => b'\n',
                            b't' => b'\t',
                            b'u' => {
                                return Err("\\u escapes are not expected in an ITF trace".into())
                            }
                            c => c,
                        });
                    } else {
                        out.push(self.s[self.i]);
                    }
                    self.i += 1;
                }
                self.eat(b'"')?;
                String::from_utf8(out)
                    .map(J::Str)
                    .map_err(|_| "a string that isn't UTF-8".into())
            }
            b't' if self.s[self.i..].starts_with(b"true") => {
                self.i += 4;
                Ok(J::Bool(true))
            }
            b'f' if self.s[self.i..].starts_with(b"false") => {
                self.i += 5;
                Ok(J::Bool(false))
            }
            b'n' if self.s[self.i..].starts_with(b"null") => {
                self.i += 4;
                Ok(J::Null)
            }
            _ => {
                let st = self.i;
                while self.i < self.s.len()
                    && (self.s[self.i] == b'-' || self.s[self.i].is_ascii_digit())
                {
                    self.i += 1;
                }
                if st == self.i {
                    return Err(format!("a value at {st}"));
                }
                Ok(J::Num(
                    String::from_utf8_lossy(&self.s[st..self.i]).into_owned(),
                ))
            }
        }
    }
}

/// Parse one JSON document.
pub fn parse(s: &str) -> Result<J, String> {
    let mut p = P {
        s: s.as_bytes(),
        i: 0,
    };
    let v = p.val()?;
    p.ws();
    if p.i != p.s.len() {
        return Err(format!("trailing data at {}", p.i));
    }
    Ok(v)
}

type R<T> = Result<T, String>;

impl J {
    fn get(&self, k: &str) -> R<&J> {
        match self {
            J::Obj(v) => v
                .iter()
                .find(|(x, _)| x == k)
                .map(|(_, v)| v)
                .ok_or_else(|| format!("no field {k:?}")),
            _ => Err(format!("field {k:?} of a non-object")),
        }
    }
    fn int(&self) -> R<i64> {
        match self {
            J::Num(n) => n.parse().map_err(|_| format!("number {n}")),
            J::Obj(_) => match self.get("#bigint")? {
                J::Str(n) => n.parse().map_err(|_| format!("bigint {n}")),
                _ => Err("a #bigint that isn't a string".into()),
            },
            _ => Err(format!("an int expected, got {self:?}")),
        }
    }
    fn uint(&self) -> R<usize> {
        usize::try_from(self.int()?).map_err(|_| "a negative int".into())
    }
    fn boolean(&self) -> R<bool> {
        match self {
            J::Bool(b) => Ok(*b),
            _ => Err(format!("a bool expected, got {self:?}")),
        }
    }
    fn string(&self) -> R<&str> {
        match self {
            J::Str(s) => Ok(s),
            _ => Err(format!("a string expected, got {self:?}")),
        }
    }
    /// A list (`[..]`), or a tagged collection's items (`{"#set": [..]}`, `#tup`, `#map`).
    fn items(&self) -> R<&[J]> {
        match self {
            J::Arr(v) => Ok(v),
            J::Obj(v) if v.len() == 1 && v[0].0.starts_with('#') => match &v[0].1 {
                J::Arr(a) => Ok(a),
                _ => Err("a tagged collection that isn't an array".into()),
            },
            _ => Err(format!("a collection expected, got {self:?}")),
        }
    }
    /// A map's `(key, value)` pairs.
    fn map(&self) -> R<Vec<(&J, &J)>> {
        self.get("#map")?
            .items()?
            .iter()
            .map(|kv| match kv.items()? {
                [k, v] => Ok((k, v)),
                _ => Err("a map entry that isn't a pair".into()),
            })
            .collect()
    }
}

fn letters(j: &J) -> R<String> {
    j.items()?
        .iter()
        .map(|c| match c.int()? {
            0 => Ok('a'),
            1 => Ok('b'),
            x => Err(format!("letter {x}")),
        })
        .collect()
}

fn region(j: &J) -> R<VRegion> {
    Ok(VRegion {
        start: j.get("start")?.uint()?,
        end: j.get("end")?.uint()?,
        before: letters(j.get("before")?)?,
        after: letters(j.get("after")?)?,
        tainted: j.get("tainted")?.boolean()?,
        base_doc_nonempty: j.get("bdn")?.boolean()?,
        erased: j.get("erased")?.boolean()?,
    })
}

fn batches(j: &J) -> R<Vec<Vec<VRegion>>> {
    j.items()?
        .iter()
        .map(|b| b.items()?.iter().map(region).collect())
        .collect()
}

/// One ITF state's `st` as a [`View`].
pub fn view(st: &J, pids: usize) -> R<View> {
    let doc = st
        .get("doc")?
        .items()?
        .iter()
        .map(|e| {
            let ch = match e.get("ch")?.int()? {
                0 => 'a',
                1 => 'b',
                x => return Err(format!("letter {x}")),
            };
            Ok((ch, u8::try_from(e.get("au")?.int()?).map_err(|_| "author")?))
        })
        .collect::<R<Vec<_>>>()?;
    let mut stacks = vec![VStacks::default(); pids];
    for (k, v) in st.get("stacks")?.map()? {
        let p = k.uint()?;
        if p == 0 || p > pids {
            return Err(format!("stacks of pid {p}"));
        }
        stacks[p - 1] = VStacks {
            undo: batches(v.get("undo")?)?,
            redo: batches(v.get("redo")?)?,
            cleared: v.get("cleared")?.boolean()?,
        };
    }
    let mut records = Vec::new();
    for (k, v) in st.get("records")?.map()? {
        let edges = v
            .get("edges")?
            .items()?
            .iter()
            .map(|e| {
                Ok((
                    e.get("s")?.uint()?,
                    e.get("e")?.uint()?,
                    e.get("ts")?.boolean()?,
                    e.get("te")?.boolean()?,
                ))
            })
            .collect::<R<Vec<_>>>()?;
        records.push((
            k.int()? as u64,
            VRecord {
                author: u8::try_from(v.get("author")?.int()?).map_err(|_| "author")?,
                regions: v
                    .get("regions")?
                    .items()?
                    .iter()
                    .map(region)
                    .collect::<R<_>>()?,
                edges,
            },
        ));
    }
    records.sort_by_key(|r| r.0);
    let mut events = vec![0u8; pids];
    for (k, v) in st.get("events")?.map()? {
        let p = k.uint()?;
        if p == 0 || p > pids {
            return Err(format!("events of pid {p}"));
        }
        events[p - 1] = u8::try_from(v.int()?).map_err(|_| "events")?;
    }
    let mut viol: Vec<String> = st
        .get("viol")?
        .items()?
        .iter()
        .map(|v| v.string().map(str::to_owned))
        .collect::<R<_>>()?;
    viol.sort();
    Ok(View {
        doc,
        stacks,
        records,
        head: st.get("head")?.int()? as u64,
        events,
        undos: u8::try_from(st.get("undos")?.int()?).map_err(|_| "undos")?,
        reverts: u8::try_from(st.get("reverts")?.int()?).map_err(|_| "reverts")?,
        viol,
    })
}

fn mode(s: &str) -> R<Mode> {
    match s {
        "all" => Ok(Mode::All),
        "partial" => Ok(Mode::Partial),
        "drop" => Ok(Mode::Drop),
        _ => Err(format!("mode {s:?}")),
    }
}

/// An ITF state's `act` as an [`Act`] (pids from 0).
pub fn act(j: &J) -> R<Act> {
    let p = j.get("p")?.uint()?.checked_sub(1).ok_or("pid 0")?;
    Ok(match j.get("kind")?.string()? {
        "edit" => Act::Edit {
            p,
            hunks: j
                .get("hunks")?
                .items()?
                .iter()
                .map(|h| {
                    Ok(Edit {
                        start: h.get("s")?.uint()?,
                        end: h.get("e")?.uint()?,
                        insert: letters(h.get("ins")?)?,
                    })
                })
                .collect::<R<_>>()?,
        },
        "purge" => Act::Purge {
            p,
            secret: if j.get("secret")?.int()? == 0 {
                'a'
            } else {
                'b'
            },
        },
        "undo" => Act::Undo {
            p,
            mode: mode(j.get("mode")?.string()?)?,
            steps: j.get("steps")?.uint()?,
        },
        "redo" => Act::Redo {
            p,
            mode: mode(j.get("mode")?.string()?)?,
        },
        "revert" => Act::Revert {
            p,
            change: j.get("change")?.int()? as u64,
            partial: j.get("partial")?.boolean()?,
        },
        k => return Err(format!("action kind {k:?}")),
    })
}

fn reports(j: &J) -> R<Vec<Report>> {
    let mut v = j
        .items()?
        .iter()
        .map(|r| {
            Ok(Report {
                root: 0,
                start: r.get("start")?.uint()?,
                end: r.get("end")?.uint()?,
                lines: (1, 1),
                kind: match r.get("kind")?.string()? {
                    "overlap" => Kind::Overlap,
                    "taint" => Kind::Taint,
                    "erased" => Kind::Erased,
                    k => return Err(format!("kind {k:?}")),
                },
            })
        })
        .collect::<R<Vec<_>>>()?;
    v.sort();
    Ok(v)
}

/// An ITF state's `out` as an [`Outcome`] (a change's id is not compared: `Changed(Some(0))`).
pub fn outcome(j: &J) -> R<Outcome> {
    Ok(match j.get("kind")?.string()? {
        "changed" => Outcome::Changed(j.get("changed")?.boolean()?.then_some(0)),
        "conflicts" => Outcome::Conflicts {
            step: j.get("step")?.uint()?,
            regions: reports(j.get("regions")?)?,
        },
        "blocked" => Outcome::Blocked {
            regions: reports(j.get("regions")?)?,
        },
        "applied" => Outcome::Applied {
            steps: j.get("steps")?.uint()?,
            skipped: reports(j.get("skipped")?)?,
            blocked: if j.get("hasBlocked")?.boolean()? {
                Some(reports(j.get("blocked")?)?)
            } else {
                None
            },
        },
        "dropped" => Outcome::Dropped,
        "nothing" => Outcome::Nothing(match j.get("notice")?.string()? {
            "plain" => Notice::Plain,
            "cleared" => Notice::ClearedByPurge,
            n => return Err(format!("notice {n:?}")),
        }),
        k => return Err(format!("outcome kind {k:?}")),
    })
}

/// The oracle's outcome as the model writes it: lines `(1, 1)` (no `\n` in the alphabet),
/// reports sorted, a change's id dropped.
pub fn normalise(o: &Outcome) -> Outcome {
    let rs = |v: &[Report]| -> Vec<Report> {
        let mut v: Vec<Report> = v
            .iter()
            .map(|r| Report {
                lines: (1, 1),
                ..r.clone()
            })
            .collect();
        v.sort();
        v
    };
    match o {
        Outcome::Changed(c) => Outcome::Changed(c.map(|_| 0)),
        Outcome::Conflicts { step, regions } => Outcome::Conflicts {
            step: *step,
            regions: rs(regions),
        },
        Outcome::Blocked { regions } => Outcome::Blocked {
            regions: rs(regions),
        },
        Outcome::Applied {
            steps,
            skipped,
            blocked,
        } => Outcome::Applied {
            steps: *steps,
            skipped: rs(skipped),
            blocked: blocked.as_deref().map(rs),
        },
        o => o.clone(),
    }
}

/// A state's variable `name`, as `name` or namespaced (`<module>::name`).
fn var<'a>(s: &'a J, name: &str) -> R<&'a J> {
    match s {
        J::Obj(v) => v
            .iter()
            .find(|(k, _)| k == name || k.rsplit("::").next() == Some(name))
            .map(|(_, v)| v)
            .ok_or_else(|| {
                let keys: Vec<&str> = v.iter().map(|(k, _)| k.as_str()).collect();
                format!("no variable {name:?} (the state has {keys:?})")
            }),
        _ => Err("a state that isn't an object".into()),
    }
}

/// One trace's states: `(st, act, out)` per state (state 0's act and out are placeholders).
pub fn states(doc: &J) -> R<Vec<(&J, &J, &J)>> {
    doc.get("states")?
        .items()?
        .iter()
        .map(|s| Ok((var(s, "st")?, var(s, "act")?, var(s, "out")?)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_an_itf_state() {
        let j = parse(
            r##"{"states":[{"st":{"doc":[{"ch":{"#bigint":"0"},"au":{"#bigint":"0"}}],
            "stacks":{"#map":[[{"#bigint":"1"},{"undo":[],"redo":[],"cleared":false}]]},
            "records":{"#map":[]},"head":{"#bigint":"1"},
            "events":{"#map":[[{"#bigint":"1"},{"#bigint":"0"}]]},"undos":{"#bigint":"0"},
            "reverts":{"#bigint":"0"},"viol":{"#set":["U9"]}},
            "act":{"kind":"undo","p":{"#bigint":"1"},"hunks":[],"secret":{"#bigint":"0"},
            "mode":"partial","steps":{"#bigint":"2"},"change":{"#bigint":"0"},"partial":false},
            "out":{"kind":"nothing","notice":"plain"}}]}"##,
        )
        .unwrap();
        let s = states(&j).unwrap();
        let v = view(s[0].0, 1).unwrap();
        assert_eq!(v.doc, vec![('a', 0)]);
        assert_eq!(v.viol, vec!["U9".to_string()]);
        assert_eq!(
            act(s[0].1).unwrap(),
            Act::Undo {
                p: 0,
                mode: Mode::Partial,
                steps: 2
            }
        );
        assert_eq!(outcome(s[0].2).unwrap(), Outcome::Nothing(Notice::Plain));
    }
}
