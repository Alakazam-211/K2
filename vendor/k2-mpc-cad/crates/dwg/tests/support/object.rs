//! Writes one object's bytes for unit rows only. Each row packs that object's bit fields itself
//! (its data, its strings for 2007 and later, its handles); this puts its size fields and
//! CRC-16 around them, in the 2004 or the 2010 layout. Test code only (never in the library):
//! one object at a time, never a section, a file or a drawing.
#![allow(dead_code)]

use k2_mpc_dwg::crc::{crc16, CRC16_SEED};
use k2_mpc_dwg::frame::Ver;

use super::{bytes, coded, pack};

/// Bit fields, `(value, width)`.
pub type F = Vec<(u64, u32)>;

pub fn b(x: bool) -> F {
    vec![(u64::from(x), 1)]
}

pub fn bb(x: u8) -> F {
    vec![(u64::from(x), 2)]
}

pub fn rc(x: u8) -> F {
    vec![(u64::from(x), 8)]
}

pub fn bs(v: u16) -> F {
    match v {
        0 => vec![(2, 2)],
        256 => vec![(3, 2)],
        1..=255 => coded(1, &[v as u8]),
        _ => coded(0, &v.to_le_bytes()),
    }
}

pub fn bl(v: u32) -> F {
    match v {
        0 => vec![(2, 2)],
        1..=255 => coded(1, &[v as u8]),
        _ => coded(0, &v.to_le_bytes()),
    }
}

pub fn rd(v: f64) -> F {
    bytes(&v.to_le_bytes())
}

pub fn bd(v: f64) -> F {
    if v == 1.0 {
        vec![(1, 2)]
    } else if v == 0.0 {
        vec![(2, 2)]
    } else {
        coded(0, &v.to_le_bytes())
    }
}

pub fn bd3(p: [f64; 3]) -> F {
    [bd(p[0]), bd(p[1]), bd(p[2])].concat()
}

/// `DD` equal to its default.
pub fn dd_same() -> F {
    vec![(0, 2)]
}

/// `DD` as a whole double.
pub fn dd(v: f64) -> F {
    coded(3, &v.to_le_bytes())
}

/// A handle reference: its code, then its value's bytes, fewest that hold it.
pub fn h(code: u8, value: u64) -> F {
    let all = value.to_be_bytes();
    let first = all.iter().position(|&x| x != 0).unwrap_or(8);
    let v = &all[first..];
    let mut f = rc((code << 4) | v.len() as u8);
    f.extend(bytes(v));
    f
}

/// `TU`.
pub fn tu(s: &str) -> F {
    let units: Vec<u16> = s.encode_utf16().collect();
    let mut f = bs(units.len() as u16);
    for u in units {
        f.extend(bytes(&u.to_le_bytes()));
    }
    f
}

/// `TV` (2004): its bytes as given.
pub fn tv(s: &[u8]) -> F {
    let mut f = bs(s.len() as u16);
    f.extend(bytes(s));
    f
}

fn width(f: &F) -> usize {
    f.iter().map(|x| x.1 as usize).sum()
}

/// One object's parts: its type and handle, its extension data (`BS` 0 when none), its data,
/// its strings (2007 and later) and its handles.
pub struct Parts {
    pub ty: u16,
    pub handle: u64,
    pub eed: F,
    pub data: F,
    pub strings: F,
    pub handles: F,
}

impl Parts {
    pub fn new(ty: u16, handle: u64) -> Parts {
        Parts {
            ty,
            handle,
            eed: bs(0),
            data: Vec::new(),
            strings: Vec::new(),
            handles: Vec::new(),
        }
    }
}

/// The object `p` at byte 3 of a buffer, in `v`'s layout, its CRC-16 after it.
pub fn object(v: Ver, p: &Parts) -> (Vec<u8>, u64) {
    let mut d: F = Vec::new();
    if v >= Ver::R2010 {
        if p.ty < 256 {
            d.push((0, 2));
            d.extend(rc(p.ty as u8));
        } else {
            d.push((2, 2));
            d.extend(bytes(&p.ty.to_le_bytes()));
        }
    } else {
        d.extend(bs(p.ty));
    }
    let mut rest = h(0, p.handle);
    rest.extend(p.eed.clone());
    rest.extend(p.data.clone());
    if v >= Ver::R2010 {
        if p.strings.is_empty() {
            rest.push((0, 1));
        } else {
            rest.extend(p.strings.clone());
            rest.extend(bytes(&(width(&p.strings) as u16).to_le_bytes()));
            rest.push((1, 1));
        }
    }
    if v < Ver::R2010 {
        // the data's size in bits, counted from the object's first data bit
        let bits = width(&d) + 32 + width(&rest);
        d.extend(bytes(&(bits as u32).to_le_bytes()));
    }
    d.extend(rest);
    let mut hs = p.handles.clone();
    let pad = (8 - (width(&d) + width(&hs)) % 8) % 8;
    if pad > 0 {
        hs.push((0, pad as u32));
    }
    let hbits = width(&hs);
    d.extend(hs);
    let body = pack(&d);
    let mut obj = (body.len() as u16).to_le_bytes().to_vec();
    if v >= Ver::R2010 {
        let mut x = hbits as u64;
        loop {
            let low = (x & 0x7F) as u8;
            x >>= 7;
            if x == 0 {
                obj.push(low);
                break;
            }
            obj.push(low | 0x80);
        }
    }
    obj.extend(body);
    let mut buf = vec![0xEE, 0xEE, 0xEE];
    let crc = crc16(CRC16_SEED, &obj);
    buf.extend(obj);
    buf.extend_from_slice(&crc.to_le_bytes());
    buf.extend_from_slice(&[0xEE, 0xEE]);
    (buf, 3)
}

/// An entity's common data and handles: model space (or owned, when `owner`), no reactors, no
/// extension dictionary, colour by layer, linetype by layer, lineweight by layer, on `layer`.
pub struct Common {
    pub owner: Option<u64>,
    pub reactors: Vec<u64>,
    pub xdic: Option<u64>,
    pub colour: F,
    pub book: Option<u64>,
    pub ltype: Option<u64>,
    pub lw: u8,
    pub layer: u64,
    pub graphic: Option<Vec<u8>>,
}

impl Common {
    pub fn on(layer: u64) -> Common {
        Common {
            owner: None,
            reactors: Vec::new(),
            xdic: None,
            colour: bs(256),
            book: None,
            ltype: None,
            lw: 29,
            layer,
            graphic: None,
        }
    }

    /// Its data fields and its handle fields.
    pub fn fields(&self, v: Ver) -> (F, F) {
        let mut d = b(self.graphic.is_some());
        if let Some(g) = &self.graphic {
            if v >= Ver::R2010 {
                d.push((1, 3));
                d.extend(rc(g.len() as u8));
            } else {
                d.extend(bytes(&(g.len() as u32).to_le_bytes()));
            }
            d.extend(bytes(g));
        }
        d.extend(bb(if self.owner.is_some() { 0 } else { 2 }));
        d.extend(bl(self.reactors.len() as u32));
        d.extend(b(self.xdic.is_none()));
        if v >= Ver::R2013 {
            d.extend(b(false));
        }
        d.extend(self.colour.clone());
        d.extend(bd(1.0));
        d.extend(bb(if self.ltype.is_some() { 3 } else { 0 }));
        d.extend(bb(0));
        if v >= Ver::R2010 {
            d.extend(bb(0));
            d.extend(rc(0));
            d.extend([(0, 1), (0, 1), (0, 1)]);
        }
        d.extend(bs(0));
        d.extend(rc(self.lw));
        let mut hs = Vec::new();
        if let Some(o) = self.owner {
            hs.extend(h(4, o));
        }
        for r in &self.reactors {
            hs.extend(h(4, *r));
        }
        if let Some(x) = self.xdic {
            hs.extend(h(3, x));
        }
        if let Some(bk) = self.book {
            hs.extend(h(5, bk));
        }
        hs.extend(h(5, self.layer));
        if let Some(l) = self.ltype {
            hs.extend(h(5, l));
        }
        (d, hs)
    }
}

/// An entity of type `ty` and handle `handle` with common data `c`, its kind's data `data`,
/// strings and own handles.
pub fn ent(
    v: Ver,
    ty: u16,
    handle: u64,
    c: &Common,
    data: F,
    strings: F,
    own: F,
) -> (Vec<u8>, u64) {
    let (mut d, mut hs) = c.fields(v);
    d.extend(data);
    hs.extend(own);
    let mut p = Parts::new(ty, handle);
    p.data = d;
    p.strings = strings;
    p.handles = hs;
    object(v, &p)
}
