//! An entity's effective style and visibility: its own values, else its layer's, else its block
//! reference's, as the format's rules say. Two cases inside a block definition can't be
//! resolved without the reference, and say so with the word `insert`: a value by block, and a
//! value by layer on layer `0` (a block's layer-0 entities take the reference's layer).

use k2_mpc_base::errors::Value;

use super::{aci, DrawTables, Ent};
use crate::model::{BlockKind, Doc, Entity};
use crate::mutant;

/// A value, or "the referencing block reference's".
#[derive(Debug, Clone, PartialEq)]
pub enum Res<T> {
    Value(T),
    Insert,
}

/// An entity's effective style.
#[derive(Debug, Clone, PartialEq)]
pub struct Style {
    pub rgb: Res<[u8; 3]>,
    /// The foreground colour (number 7): the viewer swaps black and white with its background.
    pub fg: bool,
    /// The colour number it resolved through, when it did.
    pub aci: Option<i64>,
    pub book: Option<String>,
    pub alpha: Res<f64>,
    pub ltype: Res<String>,
    pub lt_scale: f64,
    pub lw_mm: Res<f64>,
}

fn num(v: Option<&str>) -> Option<i64> {
    v.and_then(|v| v.trim().parse::<i64>().ok())
}

/// Whether `e` is inside a block definition.
fn in_block(doc: &Doc, e: &Entity) -> bool {
    doc.owner_kind(e) == Some(BlockKind::Block)
}

/// Whether `e` is a block definition's entity on layer `0`: its by-layer values are its
/// reference's.
fn layer0_in_block(doc: &Doc, e: &Entity) -> bool {
    in_block(doc, e) && &*e.layer == "0"
}

fn alpha_of(code: i64) -> Option<f64> {
    (code & 0x0200_0000 != 0).then(|| (code & 0xFF) as f64 / 255.0)
}

/// Entity `i`'s effective style.
pub fn resolve(doc: &Doc, t: &DrawTables, i: usize, ent: &Ent) -> Style {
    let e = &doc.entities[i];
    let p = e.props.as_deref();
    let layer = doc.layer_named(&e.layer);
    let ld = layer.and_then(|l| t.layers.get(l));
    let block = in_block(doc, e);
    let by_layer_insert = layer0_in_block(doc, e) && !mutant::active("ByBlockResolvedInBlock");
    let by_block_insert = block && !mutant::active("ByBlockResolvedInBlock");

    // colour: own true colour, own number, else the layer's
    let own_rgb = num(p.and_then(|p| p.rgb.as_deref())).and_then(|c| u32::try_from(c).ok());
    let own_aci = num(p.and_then(|p| p.color.as_deref()));
    let layer_colour = || -> (Res<[u8; 3]>, bool, Option<i64>) {
        if by_layer_insert {
            return (Res::Insert, false, None);
        }
        if mutant::active("DrawByLayerUnresolved") {
            return (Res::Value([255, 255, 255]), true, Some(256));
        }
        if let Some(c) = ld.and_then(|d| d.rgb) {
            return (Res::Value(aci::from_u32(c)), false, None);
        }
        let n = layer
            .and_then(|l| doc.layers[l].color)
            .map(i64::abs)
            .filter(|n| (1..=255).contains(n))
            .unwrap_or(7);
        (Res::Value(aci::rgb(n).unwrap_or([255; 3])), n == 7, Some(n))
    };
    let (rgb, fg, aci_n) = match (own_rgb, own_aci) {
        (Some(c), _) => (Res::Value(aci::from_u32(c & 0xFF_FFFF)), false, None),
        (None, Some(0)) => {
            if by_block_insert {
                (Res::Insert, false, None)
            } else {
                (Res::Value([255, 255, 255]), true, Some(7))
            }
        }
        (None, Some(n)) if (1..=255).contains(&n.abs()) => {
            let n = n.abs();
            (Res::Value(aci::rgb(n).unwrap_or([255; 3])), n == 7, Some(n))
        }
        _ => layer_colour(),
    };

    // linetype: own, else the layer's, else continuous
    let own_lt = p.and_then(|p| p.ltype.as_deref());
    let layer_lt = || -> Res<String> {
        if by_layer_insert {
            return Res::Insert;
        }
        if mutant::active("DrawByLayerUnresolved") {
            return Res::Value("BYLAYER".into());
        }
        Res::Value(
            layer
                .and_then(|l| doc.layers[l].ltype.clone())
                .filter(|n| {
                    !n.eq_ignore_ascii_case("BYLAYER") && !n.eq_ignore_ascii_case("BYBLOCK")
                })
                .unwrap_or_else(|| "CONTINUOUS".into()),
        )
    };
    let ltype = match own_lt {
        None => layer_lt(),
        Some(n) if n.eq_ignore_ascii_case("BYLAYER") => layer_lt(),
        Some(n) if n.eq_ignore_ascii_case("BYBLOCK") => {
            if by_block_insert {
                Res::Insert
            } else {
                Res::Value("CONTINUOUS".into())
            }
        }
        Some(n) => Res::Value(n.to_owned()),
    };

    // lineweight, hundredths of a millimetre: own, else the layer's, else the default
    let default_mm = t.settings.lwdefault.max(0) as f64 / 100.0;
    let layer_lw = || -> Res<f64> {
        if by_layer_insert {
            return Res::Insert;
        }
        match layer.and_then(|l| num(doc.layers[l].lw.as_deref())) {
            Some(w) if w >= 0 => Res::Value(w as f64 / 100.0),
            _ => Res::Value(default_mm),
        }
    };
    let lw_mm = match num(p.and_then(|p| p.lw.as_deref())) {
        None | Some(-1) => layer_lw(),
        Some(-2) => {
            if by_block_insert {
                Res::Insert
            } else {
                Res::Value(default_mm)
            }
        }
        Some(w) if w >= 0 => Res::Value(w as f64 / 100.0),
        Some(_) => Res::Value(default_mm),
    };

    // transparency: own, else the layer's, else opaque
    let alpha = match num(p.and_then(|p| p.alpha.as_deref())) {
        Some(c) if c & 0x0200_0000 != 0 => Res::Value(alpha_of(c).unwrap_or(1.0)),
        Some(c) if c & 0x0100_0000 != 0 => {
            if by_block_insert {
                Res::Insert
            } else {
                Res::Value(1.0)
            }
        }
        _ => {
            if by_layer_insert {
                Res::Insert
            } else {
                Res::Value(ld.and_then(|d| d.alpha).and_then(alpha_of).unwrap_or(1.0))
            }
        }
    };

    Style {
        rgb,
        fg,
        aci: aci_n,
        book: ent.book.clone(),
        alpha,
        ltype,
        lt_scale: ent.lt_scale * t.settings.ltscale,
        lw_mm,
    }
}

/// Whether entity `i` is visible: not invisible, its layer on and thawed; inside a block, a
/// layer-0 entity follows its reference's layer (`insert`).
pub fn visible(doc: &Doc, i: usize, ent: &Ent) -> Res<bool> {
    let e = &doc.entities[i];
    if ent.invisible {
        return Res::Value(false);
    }
    if layer0_in_block(doc, e) {
        return Res::Insert;
    }
    match doc.layer_named(&e.layer) {
        Some(l) => Res::Value(doc.layers[l].on() && !doc.layers[l].frozen),
        None => Res::Value(true),
    }
}

fn fl(x: f64) -> Value {
    Value::float(x).unwrap_or(Value::Null)
}

fn res<T>(r: &Res<T>, f: impl Fn(&T) -> Value) -> Value {
    match r {
        Res::Value(v) => f(v),
        Res::Insert => Value::from("insert"),
    }
}

impl Style {
    /// The shape's `style` object.
    pub fn value(&self) -> Value {
        let mut v = vec![
            (
                "rgb",
                res(&self.rgb, |c| {
                    Value::List(c.iter().map(|x| Value::from(u64::from(*x))).collect())
                }),
            ),
            ("fg", Value::from(self.fg)),
            ("aci", self.aci.map_or(Value::Null, Value::from)),
            ("alpha", res(&self.alpha, |a| fl(*a))),
            ("ltype", res(&self.ltype, |s| Value::from(s.as_str()))),
            ("lt_scale", fl(self.lt_scale)),
            ("lw_mm", res(&self.lw_mm, |w| fl(*w))),
        ];
        if let Some(b) = &self.book {
            v.push(("book", Value::from(b.as_str())));
        }
        Value::obj(v)
    }
}

/// `visible` as the shape gives it.
pub fn visible_value(v: &Res<bool>) -> Value {
    res(v, |b| Value::from(*b))
}
