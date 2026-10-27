//! K2-defined primitives and pack schemas.
//!
//! A pack's schema names its roots and gives each a primitive kind. Only [`Kind::Text`]
//! is implemented today; the rest are specified in `docs/PRIMITIVES.md` and are built
//! when the first pack needs them, each with its conformance tests. Declaring an unimplemented kind is refused at
//! [`crate::Doc::new`], loudly.

/// A primitive a pack can build on. Each kind is defined by K2's merge semantics, written
/// as a conformance suite every engine must pass (`src/conformance/`), not by what one
/// engine happens to offer.
///
/// Not kinds:
/// - **blobs** are a kit service (content-addressed store + pointer + lease), never CRDT values;
/// - **tables** compose kinds: movable rows/columns, a cell map, formulas recalculated by the daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Plain text (Unicode), character-granular merge.
    Text,
    /// Text with formatting marks (per-mark expand rules) and embedded blocks.
    RichText,
    /// An ordered sequence of values.
    List,
    /// An ordered sequence with a true move: a concurrent move never duplicates an item.
    MovableList,
    /// String keys to values.
    Map,
    /// A tree with a true move: no duplicates, no cycles.
    Tree,
}

impl Kind {
    /// True when this build of the kit implements the kind.
    pub fn implemented(self) -> bool {
        matches!(self, Kind::Text)
    }
}

/// A pack's data model: named roots, each one primitive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Schema {
    /// Pack name, e.g. "markdown".
    pub name: &'static str,
    pub roots: &'static [(&'static str, Kind)],
}

impl Schema {
    pub fn kind_of(&self, root: &str) -> Option<Kind> {
        self.roots.iter().find(|(n, _)| *n == root).map(|(_, k)| *k)
    }
}
