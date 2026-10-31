//! A doc's content as a read-only, engine-free value (A6): what a structured edit is computed
//! from ([`super::Session::edit_ops_with`]), what [`super::Pack::reconcile`] aligns a file
//! against, and what [`super::Pack::line_map`] maps.

use std::collections::BTreeMap;

use crate::doc::{Edit, Neutral};
use crate::elem::{Anchor, Key, KeyedRoot, KeyedState, ListOp, MapOp, Value};
use crate::table::{Table, TableError};
use crate::text::diff;

/// A doc's roots by key: text as text, a list's live elements in order with its deleted
/// ones (and where they sit), a map's keys and values; and the doc's codec settings when the
/// session took it ([`View::codec`]). A snapshot: it never changes after it is taken.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct View {
    state: KeyedState,
    codec: Vec<u8>,
    /// The value limit [`View::table`]'s table carries (`None`: the default).
    max_value: Option<usize>,
}

impl View {
    pub fn new(state: KeyedState) -> Self {
        Self {
            state,
            codec: Vec::new(),
            max_value: None,
        }
    }

    /// The same view, carrying the doc's codec settings (the session sets them on the views
    /// it hands an edit closure or a verb).
    pub fn with_codec(mut self, codec: &[u8]) -> Self {
        self.codec = codec.to_vec();
        self
    }

    /// The same view, its [`View::table`] refusing values over `max` bytes (the session
    /// sets the host's `Config::grid_max_value_len` on the views it hands out).
    pub fn with_max_value(mut self, max: usize) -> Self {
        self.max_value = Some(max);
        self
    }

    /// The doc's codec settings (opaque to the kit, the pack's: a CSV pack's dialect), as
    /// they were when the session took this view, under the same lock: what an edit closure
    /// ([`super::Session::edit_ops_with`]) or a structured verb reads to follow the file's
    /// format (`header on` held to the codec's rule, a block decoded in the doc's dialect).
    /// Empty for a view the session didn't hand out ([`View::new`], [`View::of_content`])
    /// unless [`View::with_codec`] set it.
    pub fn codec(&self) -> &[u8] {
        &self.codec
    }

    /// A view of decoded or stored content (a file's [`super::PackContent`], a version):
    /// lists have no deleted elements.
    pub fn of_content(content: &[(String, Neutral)]) -> Self {
        let roots = content
            .iter()
            .map(|(r, n)| {
                let x = match n {
                    Neutral::Text(t) => KeyedRoot::Text(t.clone()),
                    Neutral::List(l) => KeyedRoot::List {
                        live: l.clone(),
                        hidden: BTreeMap::new(),
                        places: Vec::new(),
                    },
                    Neutral::Map(m) => KeyedRoot::Map(m.clone()),
                };
                (r.clone(), x)
            })
            .collect();
        Self {
            state: KeyedState { roots },
            codec: Vec::new(),
            max_value: None,
        }
    }

    pub fn keyed(&self) -> &KeyedState {
        &self.state
    }

    pub fn into_keyed(self) -> KeyedState {
        self.state
    }

    fn root(&self, root: &str) -> Option<&KeyedRoot> {
        self.state
            .roots
            .iter()
            .find(|(r, _)| r == root)
            .map(|(_, x)| x)
    }

    /// A text root's text.
    pub fn text(&self, root: &str) -> Option<&str> {
        match self.root(root)? {
            KeyedRoot::Text(t) => Some(t),
            _ => None,
        }
    }

    /// A list root's live elements, in order.
    pub fn list(&self, root: &str) -> Option<&[(Key, Value)]> {
        match self.root(root)? {
            KeyedRoot::List { live, .. } => Some(live),
            _ => None,
        }
    }

    /// A list root's deleted elements and their values.
    pub fn hidden(&self, root: &str) -> Option<&BTreeMap<Key, Value>> {
        match self.root(root)? {
            KeyedRoot::List { hidden, .. } => Some(hidden),
            _ => None,
        }
    }

    /// A map root's keys and values.
    pub fn map(&self, root: &str) -> Option<&BTreeMap<String, Value>> {
        match self.root(root)? {
            KeyedRoot::Map(m) => Some(m),
            _ => None,
        }
    }

    /// The table the view holds (a [`crate::table`] schema's six roots).
    pub fn table(&self) -> Result<Table, TableError> {
        let t = Table::from_keyed(&self.state)?;
        Ok(match self.max_value {
            Some(m) => t.with_max_value(m),
            None => t,
        })
    }

    /// The live content, as a pack encodes it ([`super::Pack::encode`]).
    pub fn content(&self) -> Vec<(String, Neutral)> {
        self.state
            .roots
            .iter()
            .map(|(r, x)| {
                let n = match x {
                    KeyedRoot::Text(t) => Neutral::Text(t.clone()),
                    KeyedRoot::List { live, .. } => Neutral::List(live.clone()),
                    KeyedRoot::Map(m) => Neutral::Map(m.clone()),
                };
                (r.clone(), n)
            })
            .collect()
    }

    /// A rough size in bytes (keys and values), for caches.
    pub(crate) fn bytes(&self) -> usize {
        let v = |x: &Value| x.as_str().map_or(0, str::len);
        self.state
            .roots
            .iter()
            .map(|(_, x)| match x {
                KeyedRoot::Text(t) => t.len(),
                KeyedRoot::List { live, hidden, .. } => {
                    live.iter()
                        .map(|(k, x)| k.as_str().len() + v(x))
                        .sum::<usize>()
                        + hidden
                            .iter()
                            .map(|(k, x)| k.as_str().len() + v(x))
                            .sum::<usize>()
                }
                KeyedRoot::Map(m) => m.iter().map(|(k, x)| k.len() + v(x)).sum(),
            })
            .sum()
    }
}

/// An import's edits: each text root's text, each list root's elements as one forward run
/// of inserts, each map root's keys as sets (what a session applies when it imports a
/// pack's decoded content, in schema order). Packs' tests reach it as
/// [`super::test_support::import_edits`] (feature `test-support`).
pub fn import_edits(ordered: &[(String, Neutral)]) -> Vec<Edit> {
    let mut out = Vec::new();
    for (root, n) in ordered {
        match n {
            Neutral::Text(t) if !t.is_empty() => out.push(Edit::Text {
                root: root.clone(),
                hunks: diff::hunks("", t),
            }),
            Neutral::Text(_) => {}
            Neutral::List(items) if !items.is_empty() => {
                let mut at = Anchor::Start;
                let mut ops = Vec::with_capacity(items.len());
                for (k, v) in items {
                    ops.push(ListOp::Insert {
                        key: k.clone(),
                        at: at.clone(),
                        value: v.clone(),
                    });
                    at = Anchor::After(k.clone());
                }
                out.push(Edit::List {
                    root: root.clone(),
                    ops,
                });
            }
            Neutral::List(_) => {}
            Neutral::Map(m) if !m.is_empty() => out.push(Edit::Map {
                root: root.clone(),
                ops: m
                    .iter()
                    .map(|(k, v)| MapOp::Set {
                        key: k.clone(),
                        value: v.clone(),
                    })
                    .collect(),
            }),
            Neutral::Map(_) => {}
        }
    }
    out
}
