//! A multileader's content ([`Mleader`], the model's format-neutral type), read from its
//! pairs: its text with where it sits, its attachment and height, each leader (its landing
//! point and its lines, each line's points from its arrow), and its arrowhead (its block, by
//! handle, and its size). One reader for every use: the model keeps its lines and text
//! ([`crate::convert`]); the drawing read draws it ([`crate::draw`]). Lines and points past the
//! bounds are counted, not kept.
//!
//! The pairs, in order: the context (`300 CONTEXT_DATA{` … `301 }`) holds the text (a `304`
//! outside a leader), where it sits (`12`, `22`), its height (`41`) and attachment (`171`),
//! then the leaders (`302 LEADER{` … `303 }`), each with its landing point (`10`, `20`) and its
//! lines (`304 LEADER_LINE{` … `305 }`, their points `10`, `20`); after the context, the
//! entity's own arrowhead block (`342`) and size (`42`).

use crate::structure::{EntityUnit, Structure};

pub use k2_mpc_cad_model::leader::{Branch, Mleader, LINES_MAX, POINTS_MAX};

/// Reads a multileader's pairs one at a time ([`Reader::pair`]), then gives its content
/// ([`Reader::finish`]). Extension data and `102` groups are the caller's to leave out.
#[derive(Debug, Default)]
pub struct Reader {
    ctx: bool,
    in_leader: bool,
    in_line: bool,
    x: Option<f64>,
    ctx_arrow: Option<f64>,
    /// Lines and points kept so far, and whether the open line is kept.
    lines: usize,
    points: usize,
    line_kept: bool,
    m: Mleader,
}

fn num(v: &[u8]) -> Option<f64> {
    std::str::from_utf8(v)
        .ok()?
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|x| x.is_finite())
}

impl Reader {
    /// One pair, in the file's order.
    pub fn pair(&mut self, c: i32, v: &[u8]) {
        let word = |w: &str| v.trim_ascii().eq_ignore_ascii_case(w.as_bytes());
        let n = num(v);
        match c {
            300 if word("CONTEXT_DATA{") => self.ctx = true,
            301 => self.ctx = false,
            302 if word("LEADER{") => {
                self.m.leaders.push(Branch::default());
                self.in_leader = true;
            }
            303 => self.in_leader = false,
            304 if word("LEADER_LINE{") => {
                self.line_kept = false;
                if let Some(b) = self.m.leaders.last_mut() {
                    if self.lines < LINES_MAX {
                        b.lines.push(Vec::new());
                        self.lines += 1;
                        self.line_kept = true;
                    } else {
                        self.m.dropped += 1;
                    }
                }
                self.in_line = true;
            }
            305 => self.in_line = false,
            304 if self.ctx && !self.in_leader => {
                let t = crate::token::unescape(&crate::token::decode(v)).into_owned();
                match &mut self.m.text {
                    Some(have) => have.push_str(&t),
                    None => self.m.text = Some(t),
                }
            }
            10 | 12 => self.x = n,
            20 | 22 => {
                let Some(p) = self.x.take().zip(n).map(|(x, y)| [x, y]) else {
                    return;
                };
                let b = self.m.leaders.last_mut();
                match (c, b) {
                    (20, Some(b)) if self.in_line && self.line_kept => {
                        if self.points < POINTS_MAX {
                            if let Some(l) = b.lines.last_mut() {
                                l.push(p);
                                self.points += 1;
                            }
                        } else {
                            self.m.dropped += 1;
                        }
                    }
                    (20, _) if self.in_line => {}
                    (20, Some(b)) if self.in_leader && b.landing.is_none() => b.landing = Some(p),
                    (22, _) if self.ctx && !self.in_leader && self.m.at.is_none() => {
                        self.m.at = Some(p)
                    }
                    _ => {}
                }
            }
            41 if self.ctx && !self.in_leader => self.m.height = self.m.height.or(n),
            140 if self.ctx && !self.in_leader => self.ctx_arrow = self.ctx_arrow.or(n),
            171 if self.ctx && !self.in_leader => {
                self.m.attachment = self
                    .m
                    .attachment
                    .or(n.filter(|a| a.fract() == 0.0).map(|a| a as i64))
            }
            42 if !self.ctx => self.m.arrow_size = n,
            342 if !self.ctx => {
                self.m.arrow_block = crate::handle::parse(v).filter(|h| *h != 0);
            }
            _ => {}
        }
    }

    /// The content read.
    pub fn finish(mut self) -> Mleader {
        if self.m.arrow_size.is_none() {
            self.m.arrow_size = self.ctx_arrow;
        }
        self.m
    }
}

/// The content of multileader `eu` (its head's pairs; extension data and `102` groups left
/// out), or `None` when it isn't a multileader.
pub fn of_entity(s: &Structure<'_>, eu: &EntityUnit) -> Option<Mleader> {
    if !matches!(&*eu.head.kind, "MULTILEADER" | "MLEADER") {
        return None;
    }
    let mut r = Reader::default();
    let mut in102 = false;
    for i in eu.head.first + 1..eu.head.end {
        let c = s.tok.pairs[i].code;
        if c >= 1000 {
            break;
        }
        let v = s.tok.value(i);
        if c == 102 {
            in102 = v.trim_ascii().starts_with(b"{");
            continue;
        }
        if !in102 {
            r.pair(c, v);
        }
    }
    Some(r.finish())
}
