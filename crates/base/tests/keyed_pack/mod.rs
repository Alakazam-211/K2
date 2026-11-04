//! A keyed doc that isn't a table, for the kit's tests: a MovableList `items`, a Map `props` with
//! keys `<item>\x1f<field>`, and a Map `links` with keys `<link>\x1f{from,to}`. The file holds one
//! item key per line. It has no reconcile, so it is opened without a file behind it.
#![allow(dead_code)]

use std::collections::BTreeMap;

use k2_mpc_base::doc::Neutral;
use k2_mpc_base::schema::{Kind, Schema};
use k2_mpc_base::session::{Declined, Pack, PackContent, PackError};
use k2_mpc_base::{Key, Value};

pub struct Keyed;

pub const ITEMS: &str = "items";
pub const PROPS: &str = "props";
pub const LINKS: &str = "links";

pub const KEYED: Schema = Schema {
    name: "keyed",
    roots: &[
        (ITEMS, Kind::MovableList),
        (PROPS, Kind::Map),
        (LINKS, Kind::Map),
    ],
};

impl Pack for Keyed {
    fn schema(&self) -> Schema {
        KEYED
    }
    fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
        let bad = |message: String| Declined {
            code: "malformed",
            message,
        };
        let s = std::str::from_utf8(b).map_err(|e| bad(e.to_string()))?;
        let items = s
            .lines()
            .map(|l| Ok((Key::new(l).map_err(|e| bad(e.to_string()))?, Value::Null)))
            .collect::<Result<Vec<_>, Declined>>()?;
        Ok(PackContent {
            roots: vec![
                (ITEMS.into(), Neutral::List(items)),
                (PROPS.into(), Neutral::Map(BTreeMap::new())),
                (LINKS.into(), Neutral::Map(BTreeMap::new())),
            ],
            codec: vec![1],
        })
    }
    fn encode(&self, c: &[(String, Neutral)], _: &[u8]) -> Result<Vec<u8>, PackError> {
        let Some((_, Neutral::List(items))) = c.iter().find(|(r, _)| r == ITEMS) else {
            return Err(PackError("no items".into()));
        };
        let mut out = String::new();
        for (k, _) in items {
            out.push_str(k.as_str());
            out.push('\n');
        }
        Ok(out.into_bytes())
    }
}
