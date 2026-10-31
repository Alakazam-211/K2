//! The plain applier: the kit's ops on a [`Table`], with the sequential semantics of
//! [`crate::elem`] (what the doc does when one participant applies them in order).

use super::*;

impl Axis {
    /// Where an op anchored at `at` puts a new place: an index into the place order.
    fn resolve(&self, root: &str, at: &Anchor, moving: Option<&Key>) -> Result<usize, TableError> {
        let place = |k: &Key| -> Result<usize, TableError> {
            if Some(k) == moving {
                return Err(TableError::SelfAnchor {
                    root: axis_root(root),
                    key: k.clone(),
                });
            }
            self.raw(k).ok_or_else(|| TableError::Unknown {
                root: axis_root(root),
                key: k.clone(),
            })
        };
        Ok(match at {
            Anchor::Start => 0,
            Anchor::End => self.order.len(),
            Anchor::After(k) => place(k)? + 1,
            Anchor::Before(k) => place(k)?,
        })
    }

    pub(crate) fn apply(&mut self, root: &str, op: &ListOp) -> Result<(), TableError> {
        let key = op.key();
        let unknown = || TableError::Unknown {
            root: axis_root(root),
            key: key.clone(),
        };
        match op {
            ListOp::Insert { at, .. } => {
                if self.knows(key) {
                    return Err(TableError::KeyExists {
                        root: axis_root(root),
                        key: key.clone(),
                    });
                }
                let p = self.resolve(root, at, None)?;
                self.order.insert(p, key.clone());
                self.known.insert(key.clone());
            }
            ListOp::Move { to, .. } => {
                let from = self.raw(key).ok_or_else(unknown)?;
                let mut p = self.resolve(root, to, Some(key))?;
                self.order.remove(from);
                if from < p {
                    p -= 1;
                }
                self.order.insert(p, key.clone());
            }
            ListOp::Delete { .. } => {
                if !self.knows(key) {
                    return Err(unknown());
                }
                self.dead.insert(key.clone());
            }
            ListOp::Restore { .. } => {
                if !self.knows(key) {
                    return Err(unknown());
                }
                self.dead.remove(key);
            }
            ListOp::Set { .. } => {
                if !self.knows(key) {
                    return Err(unknown());
                }
            }
            ListOp::Drop { .. } => {
                if !self.knows(key) {
                    return Err(unknown());
                }
                if !self.dead.contains(key) {
                    return Err(TableError::Usage(format!(
                        "{key} is live; only a deleted {} can be dropped",
                        if root == ROWS { "row" } else { "column" }
                    )));
                }
                let at = self.raw(key).expect("a known key has a place");
                self.order.remove(at);
                self.dead.remove(key);
                self.known.remove(key);
            }
        }
        Ok(())
    }

    /// The first live key after `k`'s place (`None`: from the start).
    pub(crate) fn next_live_after(&self, k: Option<&Key>) -> Option<&Key> {
        let from = match k {
            Some(k) => self.raw(k)? + 1,
            None => 0,
        };
        self.order[from..].iter().find(|x| !self.dead.contains(*x))
    }
}

impl Table {
    /// Apply `edits` in order, as the doc applies one change by one participant: list ops on
    /// [`ROWS`] and [`COLS`] (a key is never reused, anchors name known keys, a move or set of
    /// a deleted element stays hidden), sets and deletes on the maps. All or nothing: a
    /// refused op leaves the table as it was. Values in maps must be strings; list element
    /// values are not kept (a table's elements are their keys).
    pub fn apply(&mut self, edits: &[Edit]) -> Result<(), TableError> {
        let mut t = self.clone();
        t.apply_in_place(edits)?;
        *self = t;
        Ok(())
    }

    /// [`Table::apply`] without the copy: a refused op leaves what the ops before it did.
    pub(crate) fn apply_in_place(&mut self, edits: &[Edit]) -> Result<(), TableError> {
        for e in edits {
            match e {
                Edit::List { root, ops } => {
                    for op in ops {
                        self.apply_list(root, op)?;
                    }
                }
                Edit::Map { root, ops } => {
                    for op in ops {
                        self.apply_map(root, op)?;
                    }
                }
                Edit::Text { root, .. } => {
                    return Err(malformed(format!("{root} is not a table root")))
                }
            }
        }
        Ok(())
    }

    pub(crate) fn apply_list(&mut self, root: &str, op: &ListOp) -> Result<(), TableError> {
        match root {
            ROWS => self.rows.apply(root, op),
            COLS => self.cols.apply(root, op),
            _ => Err(malformed(format!("{root} is not a table list"))),
        }
    }

    pub(crate) fn apply_map(&mut self, root: &str, op: &MapOp) -> Result<(), TableError> {
        let key = op.key();
        let v = match op {
            MapOp::Set { value, .. } => Some(str_value(root, key, value)?),
            MapOp::Delete { .. } => None,
        };
        match root {
            CELLS => {
                let at =
                    parse_cell_key(key).ok_or_else(|| malformed(format!("cell key {key:?}")))?;
                match v {
                    Some(v) => self.cells.insert(at, v),
                    None => self.cells.remove(&at),
                };
            }
            NAMES => {
                let c = Key::new(key).map_err(|_| malformed(format!("name key {key:?}")))?;
                match v {
                    Some(v) => self.names.insert(c, v),
                    None => self.names.remove(&c),
                };
            }
            QUOTING => {
                let at =
                    QuoteAt::parse(key).ok_or_else(|| malformed(format!("quoting key {key:?}")))?;
                match v {
                    Some(v) => {
                        let q = Quote::parse(&v)
                            .ok_or_else(|| malformed(format!("quoting[{key:?}] = {v:?}")))?;
                        self.quoting.insert(at, q);
                    }
                    None => {
                        self.quoting.remove(&at);
                    }
                }
            }
            META => self.set_meta(key, v.as_deref())?,
            _ => return Err(malformed(format!("{root} is not a table map"))),
        }
        Ok(())
    }
}
