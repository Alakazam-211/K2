//! A keyed doc that isn't a table, for the kit's tests: a MovableList `items`, a Map `props` with
//! keys `<item>\x1f<field>`, and a Map `links` with keys `<link>\x1f{from,to}`. The file holds one
//! item key per line. [`Keyed`] has no reconcile, so it is opened without a file behind it;
//! [`KeyedFile`] is the same doc with one, by key, opened from a file so an outside save
//! changes it, and with addresses of its own (`@item:<key>`).
#![allow(dead_code)]

use std::collections::BTreeMap;

use k2_mpc_base::doc::Edit;
use k2_mpc_base::doc::Neutral;
use k2_mpc_base::elem::{Anchor, ListOp};
use k2_mpc_base::schema::{Kind, Schema};
use k2_mpc_base::session::{Declined, Pack, PackContent, PackError, Reconciled, View};
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

/// [`Keyed`] with a file behind it: an outside save applies by key (an item missing from the
/// file is deleted, a new one is inserted at the end; nothing else changes), and an item's
/// address is `@item:<key>`.
pub struct KeyedFile;

impl Pack for KeyedFile {
    fn schema(&self) -> Schema {
        KEYED
    }
    fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
        Keyed.decode(b)
    }
    fn encode(&self, c: &[(String, Neutral)], codec: &[u8]) -> Result<Vec<u8>, PackError> {
        Keyed.encode(c, codec)
    }
    fn reconcile(&self, synced: &View, bytes: &[u8], _: &[u8]) -> Result<Reconciled, Declined> {
        let content = Keyed.decode(bytes)?;
        let Some((_, Neutral::List(new))) = content.roots.iter().find(|(r, _)| r == ITEMS) else {
            return Ok(Reconciled::default());
        };
        let old = synced.list(ITEMS).unwrap_or(&[]);
        let mut ops: Vec<ListOp> = old
            .iter()
            .filter(|(k, _)| !new.iter().any(|(n, _)| n == k))
            .map(|(k, _)| ListOp::Delete { key: k.clone() })
            .collect();
        ops.extend(
            new.iter()
                .filter(|(k, _)| !old.iter().any(|(o, _)| o == k))
                .map(|(k, v)| ListOp::Insert {
                    key: k.clone(),
                    at: Anchor::End,
                    value: v.clone(),
                }),
        );
        let edits = if ops.is_empty() {
            Vec::new()
        } else {
            vec![Edit::List {
                root: ITEMS.into(),
                ops,
            }]
        };
        Ok(Reconciled {
            edits,
            codec: content.codec.clone(),
            ..Reconciled::default()
        })
    }
    fn address_of(&self, root: &str, key: &Key) -> String {
        match root {
            ITEMS => format!("@item:{key}"),
            _ => key.to_string(),
        }
    }
    fn key_of(&self, root: &str, address: &str) -> Option<Key> {
        match root {
            ITEMS => Key::new(address.strip_prefix("@item:")?).ok(),
            _ => None,
        }
    }
}
