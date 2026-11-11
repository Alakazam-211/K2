//! Reading a structure index (slice A2): the outline order and `#N`, the kit's one address
//! grammar over nodes, the diff of two versions, and the read budget with its cursors
//! (K-READ-1…3). Pure functions over [`IndexView`]s: the session calls
//! ([`crate::session::BlobSession::index_list`] and the rest) add rights and versions, and
//! [`crate::session::BlobSession::run_tool`] the verbs.
//!
//! **The outline order** is a depth-first walk: each node, then its children in sibling order
//! (`order`). `#N` is the Nth node of that walk in one version, the Nth line the list verb
//! prints with no filter; filters never renumber.
//!
//! **An address names one node or nothing.** A key (`@k:…`) or a native ID (`@n:42`) names the
//! node holding it, a name (`name:Logo`, or a bare word) the one node with that name, `#N`
//! the Nth. A native ID two nodes share and a name two nodes have are refused
//! (`MPC-E170`, every holder a candidate); an address that names nothing is reported missing
//! with what does exist. Nothing is ever guessed: no nearest name, no first match, no
//! number read as a position.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::{IndexNode, IndexView, KeyClass, NodeKey, NodeRect};
use crate::addr::{self, Addr, AddrError, Candidate, Forms};
use crate::errors::{ids, ErrorReport, Value};
use crate::hash::{hex, HmacSha256};
use crate::store::{DocKey, VersionId};

/// The default read budget (K-READ-2): the most bytes one page of a structure read returns,
/// counted as the canonical JSON of the page (its compact form, the one `--json` prints) in
/// every output mode, so a cursor means the same page in text and JSON. A host sets another
/// with [`crate::tools::ToolOptions::read_budget`].
pub const READ_BUDGET: usize = 32_768;

/// The smallest budget a host may set: room for a page's frame, its first page's list of
/// missing addresses (a quarter of the budget at most) and any one item at its shortest (a
/// stub of a few bounded fields), so every page fits.
pub const READ_BUDGET_MIN: usize = 16_384;

/// How many versions a blame walks back before it stops (`IndexBlame::stopped_at`).
pub const BLAME_WALK_MAX: usize = 256;

/// At most this many candidates are listed for one missing or ambiguous address (K-READ-3);
/// `candidates_total` says how many there are.
pub const CANDIDATES_MAX: usize = 20;

/// A node as the outline lists it: where it is and what it is, without its text, attributes
/// or hashes ([`crate::session::BlobSession::index_list`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct OutlineNode {
    /// `#N`: its place in the version's outline order, from 1.
    pub position: u32,
    pub key: NodeKey,
    pub key_class: KeyClass,
    pub kind: String,
    pub name: String,
    pub parent: Option<NodeKey>,
    pub depth: u16,
    pub order: u32,
    pub visible: bool,
    pub visible_in_tree: bool,
}

/// What [`crate::session::BlobSession::index_list`] lists: a version (`None`, the current
/// one), only nodes of one kind, only nodes down to a depth (0: the roots), only the nodes
/// under one node (an address; the node itself left out).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ListQuery {
    pub version: Option<VersionId>,
    pub kind: Option<String>,
    pub depth: Option<u16>,
    pub under: Option<String>,
}

impl ListQuery {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn version(mut self, v: VersionId) -> Self {
        self.version = Some(v);
        self
    }
    pub fn kind(mut self, k: impl Into<String>) -> Self {
        self.kind = Some(k.into());
        self
    }
    pub fn depth(mut self, d: u16) -> Self {
        self.depth = Some(d);
        self
    }
    pub fn under(mut self, addr: impl Into<String>) -> Self {
        self.under = Some(addr.into());
        self
    }
}

/// A version's outline ([`crate::session::BlobSession::index_list`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Outline {
    pub version: VersionId,
    /// The file's own format stamp.
    pub source: String,
    /// Every node in the version (the filters don't change it).
    pub total: u32,
    /// The nodes the query keeps, in outline order.
    pub nodes: Vec<OutlineNode>,
}

/// One node read in full, with where it sits ([`crate::session::BlobSession::index_nodes`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct NodeRead {
    /// `#N` in the version read.
    pub position: u32,
    pub node: IndexNode,
    /// Its ancestors, the root first.
    pub ancestors: Vec<OutlineNode>,
    /// Its descendants down to the depth asked (none by default), in outline order.
    pub descendants: Vec<OutlineNode>,
}

/// An address that names nothing, with what does exist (K-READ-3).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct MissingAddress {
    /// As given.
    pub address: String,
    /// Nodes it may have meant (a name differing only in case, or containing it), at most
    /// [`CANDIDATES_MAX`]; each by its key and `#N`. Never chosen for the caller.
    pub candidates: Vec<Candidate>,
    /// How many there are (`candidates` holds the first [`CANDIDATES_MAX`]).
    pub candidates_total: usize,
}

/// [`crate::session::BlobSession::index_nodes`]: one entry per address asked, in that order,
/// `None` for each that names nothing (listed in `missing`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct NodesRead {
    pub version: VersionId,
    pub nodes: Vec<Option<NodeRead>>,
    pub missing: Vec<MissingAddress>,
}

/// What changed about one node between two versions ([`NodeChange::changes`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum NodeChangeKind {
    Added,
    Removed,
    /// Another parent, or another place among the siblings both versions share (a node that
    /// only shifted because a sibling came or went is not moved).
    Moved,
    Renamed,
    /// Kind, visibility, bounds, attributes, flags, or the record alone ([`NodeChange::props`]).
    Props,
    /// The content as stored.
    Content,
    Text,
}

impl NodeChangeKind {
    /// `added`, `removed`, `moved`, `renamed`, `props`, `content`, `text`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Removed => "removed",
            Self::Moved => "moved",
            Self::Renamed => "renamed",
            Self::Props => "props",
            Self::Content => "content",
            Self::Text => "text",
        }
    }
}

/// One node's change ([`IndexDiff`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct NodeChange {
    pub key: NodeKey,
    /// Its name in the later version (`None`: removed).
    pub name_now: Option<String>,
    /// Its name in the earlier version (`None`: added).
    pub name_then: Option<String>,
    pub changes: BTreeSet<NodeChangeKind>,
    /// With [`NodeChangeKind::Props`]: `kind`, `visible`, `bounds`, `attrs.<name>`,
    /// `flags.<name>`, or `record` (the record changed in a way no field shows).
    pub props: Vec<String>,
}

/// [`crate::session::BlobSession::index_diff`]: every node that changed from `from` to `to`,
/// in `to`'s outline order, then the removed ones in `from`'s.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct IndexDiff {
    pub from: VersionId,
    pub to: VersionId,
    pub changes: Vec<NodeChange>,
}

/// Who last changed one node ([`IndexBlame`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct NodeBlame {
    pub key: NodeKey,
    /// The version that made the node as it is now.
    pub version: VersionId,
    pub author: String,
    pub author_name: String,
    pub at_ms: u64,
    pub via: String,
    pub on_behalf_of: Option<String>,
    /// False when the walk stopped before finding where the node came from
    /// ([`IndexBlame::stopped_at`]): `version` is then the oldest one it could look at.
    pub settled: bool,
}

/// [`crate::session::BlobSession::index_blame`]: per node of a version, in outline order, the
/// version that made it as it is.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct IndexBlame {
    pub version: VersionId,
    pub rows: Vec<NodeBlame>,
    /// Where the walk stopped short: a version with no index (listed in `missing`), or the
    /// walk's bound ([`BLAME_WALK_MAX`]). `None`: every row is settled.
    pub stopped_at: Option<VersionId>,
    /// The versions it couldn't use: not indexed yet, past a cap, or purged.
    pub missing: Vec<VersionId>,
}

/// What a roster shows for a node someone is on ([`crate::session::BlobSession::node_labels`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct NodeLabel {
    pub key: NodeKey,
    pub name: String,
    pub kind: String,
    /// Shown: its own flag and every ancestor's.
    pub visible: bool,
}

// ---------------------------------------------------------------------------------------
// the outline
// ---------------------------------------------------------------------------------------

/// A view with its outline order worked out.
pub(crate) struct Walk<'a> {
    pub view: &'a IndexView,
    /// Node indices in outline order.
    pub order: Vec<usize>,
    /// `#N` of each node, by node index.
    pub pos: Vec<u32>,
    by_key: HashMap<&'a str, usize>,
}

impl<'a> Walk<'a> {
    pub fn new(view: &'a IndexView) -> Self {
        let n = view.nodes.len();
        let by_key: HashMap<&str, usize> = view
            .nodes
            .iter()
            .enumerate()
            .map(|(i, x)| (x.key.as_str(), i))
            .collect();
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
        let mut roots = Vec::new();
        for (i, x) in view.nodes.iter().enumerate() {
            match x.parent.as_ref().and_then(|p| by_key.get(p.as_str())) {
                // a parent always comes first; anything else is read as a root
                Some(&p) if p < i => children[p].push(i),
                _ => roots.push(i),
            }
        }
        let by_order = |v: &mut Vec<usize>| v.sort_by_key(|&i| (view.nodes[i].order, i));
        by_order(&mut roots);
        for c in &mut children {
            by_order(c);
        }
        let mut order = Vec::with_capacity(n);
        let mut stack: Vec<usize> = roots.into_iter().rev().collect();
        while let Some(i) = stack.pop() {
            order.push(i);
            stack.extend(children[i].iter().rev());
        }
        let mut pos = vec![0u32; n];
        for (k, &i) in order.iter().enumerate() {
            pos[i] = k as u32 + 1;
        }
        Walk {
            view,
            order,
            pos,
            by_key,
        }
    }

    pub fn index_of(&self, key: &str) -> Option<usize> {
        self.by_key.get(key).copied()
    }

    pub fn outline_node(&self, i: usize) -> OutlineNode {
        let n = &self.view.nodes[i];
        OutlineNode {
            position: self.pos[i],
            key: n.key.clone(),
            key_class: n.key_class,
            kind: n.kind.clone(),
            name: n.name.clone(),
            parent: n.parent.clone(),
            depth: n.depth,
            order: n.order,
            visible: n.visible,
            visible_in_tree: n.visible_in_tree,
        }
    }

    pub fn outline(&self, q: &ListQuery) -> Result<Outline, ReadRefusal> {
        // `under`: the run after that node in the walk that is deeper than it
        let range = match &q.under {
            None => 0..self.order.len(),
            Some(given) => {
                let r = self.nodes(std::slice::from_ref(given), 0)?;
                let Some(Some(n)) = r.nodes.first() else {
                    return Err(ReadRefusal::Report(Box::new(not_found(&r))));
                };
                let i = self.index_of(n.node.key.as_str()).expect("a node read");
                let start = self.pos[i] as usize;
                let base = self.view.nodes[i].depth;
                let len = self.order[start..]
                    .iter()
                    .take_while(|&&j| self.view.nodes[j].depth > base)
                    .count();
                start..start + len
            }
        };
        let nodes = self.order[range]
            .iter()
            .copied()
            .filter(|&i| {
                let n = &self.view.nodes[i];
                !matches!(q.depth, Some(d) if n.depth > d)
                    && !matches!(&q.kind, Some(k) if n.kind != *k)
            })
            .map(|i| self.outline_node(i))
            .collect();
        Ok(Outline {
            version: self.view.version,
            source: self.view.source.clone(),
            total: self.view.nodes.len() as u32,
            nodes,
        })
    }

    fn candidate(&self, i: usize) -> Candidate {
        Candidate::new(self.view.nodes[i].key.as_str()).at(format!("#{}", self.pos[i]))
    }

    /// Resolve one address (see the module docs).
    pub fn resolve(&self, given: &str) -> Result<Found, AddrError> {
        let nodes = &self.view.nodes;
        let all = |f: &dyn Fn(&IndexNode) -> bool| -> Vec<usize> {
            self.order
                .iter()
                .copied()
                .filter(|&i| f(&nodes[i]))
                .collect()
        };
        let a = addr::parse(given, Forms::none().native().nth())?;
        let hits: Vec<usize> = match &a {
            Addr::Native(_) => {
                let s = a.to_string();
                all(&|n| n.native.as_ref().is_some_and(|k| k.as_str() == s))
            }
            Addr::Key(k) => {
                let s = format!("@k:{k}");
                all(&|n| n.key.as_str() == s)
            }
            Addr::Nth(n) => {
                #[cfg(any(test, feature = "differential"))]
                if crate::differential::index_mutant()
                    == Some(crate::differential::IndexMutant::NthClamps)
                {
                    return Ok(match self.order.len() {
                        0 => Found::Missing(Vec::new(), 0),
                        len => Found::One(self.order[(*n).min(len) - 1]),
                    });
                }
                match self.order.get(n - 1) {
                    Some(&i) => vec![i],
                    None => Vec::new(),
                }
            }
            // no line form here: `L7` is a name
            Addr::Line(_) => unreachable!("not in these forms"),
            Addr::Name(s) | Addr::Word(s) => all(&|n| n.name == *s),
        };
        Ok(match hits.as_slice() {
            [i] => Found::One(*i),
            [] => {
                let (near, total) = self.near(&a);
                #[cfg(any(test, feature = "differential"))]
                if crate::differential::index_mutant()
                    == Some(crate::differential::IndexMutant::GuessesNearName)
                {
                    if let Some(c) = near.first() {
                        let i = self.index_of(&c.address).expect("a candidate is a node");
                        return Ok(Found::One(i));
                    }
                }
                Found::Missing(near, total)
            }
            many => {
                #[cfg(any(test, feature = "differential"))]
                if crate::differential::index_mutant()
                    == Some(crate::differential::IndexMutant::AmbiguousPicksFirst)
                {
                    return Ok(Found::One(many[0]));
                }
                // the holders, the first CANDIDATES_MAX of them, and how many: the caller
                // picks, never the kit
                Found::Many(
                    many.iter()
                        .take(candidates_max())
                        .map(|&i| self.candidate(i))
                        .collect(),
                    many.len(),
                )
            }
        })
    }

    /// What a missing name may have meant: names equal to it but for case, or containing it
    /// (case aside). Only for names: a key, an ID or a position that names nothing has no
    /// near miss worth naming.
    fn near(&self, a: &Addr) -> (Vec<Candidate>, usize) {
        let (Addr::Name(s) | Addr::Word(s)) = a else {
            return (Vec::new(), 0);
        };
        let low = s.to_lowercase();
        if low.is_empty() {
            return (Vec::new(), 0);
        }
        let all: Vec<usize> = self
            .order
            .iter()
            .copied()
            .filter(|&i| self.view.nodes[i].name.to_lowercase().contains(&low))
            .collect();
        let shown = all
            .iter()
            .take(candidates_max())
            .map(|&i| self.candidate(i))
            .collect();
        (shown, all.len())
    }

    /// Ancestors of node `i`, the root first.
    fn ancestors(&self, i: usize) -> Vec<OutlineNode> {
        let mut out = Vec::new();
        let mut at = i;
        while let Some(p) = self.view.nodes[at]
            .parent
            .as_ref()
            .and_then(|p| self.index_of(p.as_str()))
            .filter(|&p| p < at)
        {
            out.push(self.outline_node(p));
            at = p;
        }
        out.reverse();
        out
    }

    /// Descendants of node `i` down `depth` levels, in outline order: the run after it in the
    /// walk that is deeper than it.
    fn descendants(&self, i: usize, depth: u16) -> Vec<OutlineNode> {
        if depth == 0 {
            return Vec::new();
        }
        let base = self.view.nodes[i].depth;
        let start = self.pos[i] as usize;
        self.order[start..]
            .iter()
            .copied()
            .take_while(|&j| self.view.nodes[j].depth > base)
            .filter(|&j| self.view.nodes[j].depth - base <= depth)
            .map(|j| self.outline_node(j))
            .collect()
    }

    /// Several addresses in one version (K-READ-1): one entry per address, in order. A name
    /// or ID two nodes share refuses the whole read (`MPC-E170`), as does one that doesn't
    /// parse.
    pub fn nodes(&self, addrs: &[String], depth: u16) -> Result<NodesRead, ReadRefusal> {
        let mut nodes = Vec::with_capacity(addrs.len());
        let mut missing = Vec::new();
        for given in addrs {
            match self.resolve(given).map_err(ReadRefusal::Parse)? {
                Found::One(i) => nodes.push(Some(NodeRead {
                    position: self.pos[i],
                    node: self.view.nodes[i].clone(),
                    ancestors: self.ancestors(i),
                    descendants: self.descendants(i, depth),
                })),
                Found::Missing(candidates, candidates_total) => {
                    nodes.push(None);
                    missing.push(MissingAddress {
                        address: given.clone(),
                        candidates,
                        candidates_total,
                    });
                }
                Found::Many(c, total) => {
                    return Err(ReadRefusal::Report(Box::new(
                        addr::ambiguous(
                            format!(
                                "{given} names more than one node in version {}",
                                self.view.version
                            ),
                            "node",
                            given,
                            &c,
                        )
                        .with("candidates_total", total),
                    )))
                }
            }
        }
        Ok(NodesRead {
            version: self.view.version,
            nodes,
            missing,
        })
    }
}

/// One address's lookup: a missing or ambiguous one with its first candidates, and how many
/// there are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Found {
    One(usize),
    Missing(Vec<Candidate>, usize),
    Many(Vec<Candidate>, usize),
}

/// [`CANDIDATES_MAX`] (no cap under the planted fault).
fn candidates_max() -> usize {
    #[cfg(any(test, feature = "differential"))]
    if crate::differential::index_mutant()
        == Some(crate::differential::IndexMutant::CandidatesUncapped)
    {
        return usize::MAX;
    }
    CANDIDATES_MAX
}

/// The most missing addresses one refusal's details list (`details.missing_total` counts them
/// all).
pub const MISSING_LISTED_MAX: usize = 20;

/// `MPC-E172` for a node read with misses: the count, the version, and the first
/// [`MISSING_LISTED_MAX`] misses with their candidates.
pub(crate) fn not_found(r: &NodesRead) -> ErrorReport {
    let count = r.missing.len();
    ErrorReport::new(
        &ids::ADDRESS_NOT_FOUND,
        format!(
            "{count} of the addresses name nothing in version {}",
            r.version
        ),
    )
    .with("count", count)
    .with("version", r.version)
    .with(
        "missing",
        Value::List(
            r.missing
                .iter()
                .take(MISSING_LISTED_MAX)
                .map(MissingAddress::to_value)
                .collect(),
        ),
    )
    .with("missing_total", count)
}

/// Why a read of addresses was refused.
#[derive(Debug)]
pub(crate) enum ReadRefusal {
    Parse(AddrError),
    Report(Box<ErrorReport>),
}

// ---------------------------------------------------------------------------------------
// the diff
// ---------------------------------------------------------------------------------------

/// The longest increasing run of `seq` (positions kept): what stayed in order.
fn lis_keep(seq: &[usize]) -> Vec<bool> {
    let n = seq.len();
    let mut tails: Vec<usize> = Vec::new(); // index into seq of the smallest tail per length
    let mut prev = vec![usize::MAX; n];
    for i in 0..n {
        let k = tails.partition_point(|&t| seq[t] < seq[i]);
        if k > 0 {
            prev[i] = tails[k - 1];
        }
        if k == tails.len() {
            tails.push(i);
        } else {
            tails[k] = i;
        }
    }
    let mut keep = vec![false; n];
    let mut at = tails.last().copied();
    while let Some(i) = at {
        keep[i] = true;
        at = (prev[i] != usize::MAX).then_some(prev[i]);
    }
    keep
}

/// The fields of `b` that differ from `a`, as [`NodeChange::props`] names them.
fn props(a: &IndexNode, b: &IndexNode) -> Vec<String> {
    let mut p = Vec::new();
    if a.kind != b.kind {
        p.push("kind".to_owned());
    }
    if a.visible != b.visible {
        p.push("visible".to_owned());
    }
    if a.bounds != b.bounds {
        p.push("bounds".to_owned());
    }
    let keys: BTreeSet<&String> = a.attrs.keys().chain(b.attrs.keys()).collect();
    for k in keys {
        if a.attrs.get(k) != b.attrs.get(k) {
            p.push(format!("attrs.{k}"));
        }
    }
    for f in a.flags.symmetric_difference(&b.flags) {
        p.push(format!("flags.{f}"));
    }
    p.sort();
    p
}

/// Whether `a` and `b` are the same node, unchanged (a blame keeps walking back): the same
/// record, content, place in the tree and every field. A shift among siblings is not a change.
pub(crate) fn unchanged(a: &IndexNode, b: &IndexNode) -> bool {
    a.record_hash == b.record_hash
        && a.content_hash == b.content_hash
        && a.parent == b.parent
        && a.name == b.name
        && a.text == b.text
        && props(a, b).is_empty()
}

/// Every node that changed from `a` to `b` (see [`IndexDiff`]).
pub(crate) fn diff(a: &IndexView, b: &IndexView) -> Vec<NodeChange> {
    let (wa, wb) = (Walk::new(a), Walk::new(b));
    // sibling groups that kept their parent: who stayed in order
    let mut moved: BTreeSet<usize> = BTreeSet::new();
    let mut groups: BTreeMap<Option<&str>, Vec<usize>> = BTreeMap::new();
    for &j in &wb.order {
        let n = &b.nodes[j];
        if let Some(i) = wa.index_of(n.key.as_str()) {
            if a.nodes[i].parent == n.parent {
                groups
                    .entry(n.parent.as_ref().map(NodeKey::as_str))
                    .or_default()
                    .push(j);
            } else {
                moved.insert(j);
            }
        }
    }
    for members in groups.values() {
        // the shared siblings in `a`'s order, each as its place in `b`'s
        let mut by_a: Vec<(u32, usize)> = members
            .iter()
            .map(|&j| {
                (
                    wa.pos[wa.index_of(b.nodes[j].key.as_str()).expect("shared")],
                    j,
                )
            })
            .collect();
        by_a.sort();
        let seq: Vec<usize> = by_a.iter().map(|&(_, j)| wb.pos[j] as usize).collect();
        for (keep, (_, j)) in lis_keep(&seq).into_iter().zip(&by_a) {
            if !keep {
                moved.insert(*j);
            }
        }
    }
    let mut out = Vec::new();
    for &j in &wb.order {
        let nb = &b.nodes[j];
        let Some(i) = wa.index_of(nb.key.as_str()) else {
            out.push(NodeChange {
                key: nb.key.clone(),
                name_now: Some(nb.name.clone()),
                name_then: None,
                changes: [NodeChangeKind::Added].into(),
                props: Vec::new(),
            });
            continue;
        };
        let na = &a.nodes[i];
        let mut changes = BTreeSet::new();
        let mut p = props(na, nb);
        if moved.contains(&j) {
            changes.insert(NodeChangeKind::Moved);
        }
        if na.name != nb.name {
            changes.insert(NodeChangeKind::Renamed);
        }
        if na.content_hash != nb.content_hash {
            changes.insert(NodeChangeKind::Content);
        }
        if na.text != nb.text {
            changes.insert(NodeChangeKind::Text);
        }
        if p.is_empty() && changes.is_empty() && na.record_hash != nb.record_hash {
            p.push("record".to_owned());
        }
        if !p.is_empty() {
            changes.insert(NodeChangeKind::Props);
        }
        if !changes.is_empty() {
            out.push(NodeChange {
                key: nb.key.clone(),
                name_now: Some(nb.name.clone()),
                name_then: Some(na.name.clone()),
                changes,
                props: p,
            });
        }
    }
    for &i in &wa.order {
        let na = &a.nodes[i];
        if wb.index_of(na.key.as_str()).is_none() {
            out.push(NodeChange {
                key: na.key.clone(),
                name_now: None,
                name_then: Some(na.name.clone()),
                changes: [NodeChangeKind::Removed].into(),
                props: Vec::new(),
            });
        }
    }
    out
}

// ---------------------------------------------------------------------------------------
// JSON: the canonical form a page is counted in
// ---------------------------------------------------------------------------------------

fn key_class_word(c: KeyClass) -> &'static str {
    match c {
        KeyClass::Native => "native",
        KeyClass::Minted { weak: false } => "minted",
        KeyClass::Minted { weak: true } => "minted_weak",
    }
}

fn opt_key(k: &Option<NodeKey>) -> Value {
    Value::from(k.as_ref().map(NodeKey::as_str))
}

fn rect(r: &Option<NodeRect>) -> Value {
    match r {
        Some(r) => Value::obj([
            ("left", Value::Int(r.left)),
            ("top", Value::Int(r.top)),
            ("right", Value::Int(r.right)),
            ("bottom", Value::Int(r.bottom)),
        ]),
        None => Value::Null,
    }
}

impl OutlineNode {
    pub(crate) fn to_value(&self) -> Value {
        Value::obj([
            ("position", Value::from(self.position)),
            ("key", Value::from(self.key.as_str())),
            ("key_class", Value::from(key_class_word(self.key_class))),
            ("kind", Value::from(self.kind.as_str())),
            ("name", Value::from(self.name.as_str())),
            ("parent", opt_key(&self.parent)),
            ("depth", Value::from(u32::from(self.depth))),
            ("order", Value::from(self.order)),
            ("visible", Value::from(self.visible)),
            ("visible_in_tree", Value::from(self.visible_in_tree)),
        ])
    }
}

/// A node in full. `with_text` false leaves the text out (a node bigger than a page): `text`
/// is then null, with `text_omitted`, `text_bytes`, and `text_read` (the verb that reads it
/// whole).
fn node_value(n: &IndexNode, position: u32, with_text: bool, text_read: Option<&str>) -> Value {
    let mut o = vec![
        ("position".to_owned(), Value::from(position)),
        ("key".to_owned(), Value::from(n.key.as_str())),
        (
            "key_class".to_owned(),
            Value::from(key_class_word(n.key_class)),
        ),
        (
            "native".to_owned(),
            Value::from(n.native.as_ref().map(NodeKey::as_str)),
        ),
        ("kind".to_owned(), Value::from(n.kind.as_str())),
        ("name".to_owned(), Value::from(n.name.as_str())),
        ("parent".to_owned(), opt_key(&n.parent)),
        ("depth".to_owned(), Value::from(u32::from(n.depth))),
        ("order".to_owned(), Value::from(n.order)),
        ("bounds".to_owned(), rect(&n.bounds)),
        ("visible".to_owned(), Value::from(n.visible)),
        ("visible_in_tree".to_owned(), Value::from(n.visible_in_tree)),
    ];
    match (&n.text, with_text) {
        (Some(t), false) => {
            o.push(("text".to_owned(), Value::Null));
            o.push(("text_omitted".to_owned(), Value::from(true)));
            o.push(("text_bytes".to_owned(), Value::from(t.len())));
            o.push(("text_read".to_owned(), Value::from(text_read)));
        }
        (t, _) => o.push(("text".to_owned(), Value::from(t.as_deref()))),
    }
    o.push((
        "attrs".to_owned(),
        Value::Obj(
            n.attrs
                .iter()
                .map(|(k, v)| (k.clone(), Value::from(v.as_str())))
                .collect(),
        ),
    ));
    o.push((
        "flags".to_owned(),
        Value::List(n.flags.iter().map(|f| Value::from(f.as_str())).collect()),
    ));
    o.push((
        "record_hash".to_owned(),
        Value::from(n.record_hash.to_hex()),
    ));
    o.push((
        "content_hash".to_owned(),
        Value::from(n.content_hash.map(|h| h.to_hex())),
    ));
    Value::Obj(o)
}

impl NodeRead {
    /// One entry of a node read: the address asked, then the node (or null). `full_text`
    /// false: the text is left out (a node bigger than a page).
    pub(crate) fn entry_value(given: &str, r: Option<&NodeRead>, full_text: bool) -> Value {
        Self::entry_value_reading(given, r, full_text, None)
    }

    /// [`NodeRead::entry_value`], saying how to read a text left out (`text_read`).
    pub(crate) fn entry_value_reading(
        given: &str,
        r: Option<&NodeRead>,
        full_text: bool,
        text_read: Option<&str>,
    ) -> Value {
        match r {
            None => Value::obj([
                ("address", Value::from(given)),
                ("node", Value::Null),
                ("ancestors", Value::List(Vec::new())),
                ("descendants", Value::List(Vec::new())),
            ]),
            Some(r) => Value::obj([
                ("address", Value::from(given)),
                (
                    "node",
                    node_value(&r.node, r.position, full_text, text_read),
                ),
                (
                    "ancestors",
                    Value::List(r.ancestors.iter().map(OutlineNode::to_value).collect()),
                ),
                (
                    "descendants",
                    Value::List(r.descendants.iter().map(OutlineNode::to_value).collect()),
                ),
            ]),
        }
    }
}

impl NodeChange {
    pub(crate) fn to_value(&self) -> Value {
        Value::obj([
            ("key", Value::from(self.key.as_str())),
            ("name_now", Value::from(self.name_now.as_deref())),
            ("name_then", Value::from(self.name_then.as_deref())),
            (
                "changes",
                Value::List(
                    self.changes
                        .iter()
                        .map(|c| Value::from(c.as_str()))
                        .collect(),
                ),
            ),
            (
                "props",
                Value::List(self.props.iter().map(|p| Value::from(p.as_str())).collect()),
            ),
        ])
    }
}

impl NodeBlame {
    pub(crate) fn to_value(&self) -> Value {
        Value::obj([
            ("key", Value::from(self.key.as_str())),
            ("version", Value::from(self.version)),
            ("author", Value::from(self.author.as_str())),
            ("author_name", Value::from(self.author_name.as_str())),
            ("at_ms", Value::from(self.at_ms)),
            ("via", Value::from(self.via.as_str())),
            ("on_behalf_of", Value::from(self.on_behalf_of.as_deref())),
            ("settled", Value::from(self.settled)),
        ])
    }
}

fn candidate_value(c: &Candidate) -> Value {
    Value::obj([
        ("address", Value::from(c.address.as_str())),
        ("position", Value::from(c.position.as_deref())),
    ])
}

impl MissingAddress {
    pub(crate) fn to_value(&self) -> Value {
        Value::obj([
            ("address", Value::from(self.address.as_str())),
            (
                "candidates",
                Value::List(self.candidates.iter().map(candidate_value).collect()),
            ),
            ("candidates_total", Value::from(self.candidates_total)),
        ])
    }
}

impl Outline {
    pub(crate) fn frame(&self) -> Frame {
        Frame {
            head: vec![
                ("version", Value::from(self.version)),
                ("source", Value::from(self.source.as_str())),
                ("total", Value::from(self.total)),
            ],
            items: "nodes",
            tail: Vec::new(),
        }
    }

    pub(crate) fn items(&self) -> Vec<Value> {
        self.nodes.iter().map(OutlineNode::to_value).collect()
    }

    /// The whole outline as the list verb's `--json` prints it in one page.
    pub fn to_json(&self) -> String {
        self.frame().whole(self.items()).to_json()
    }
}

impl NodesRead {
    /// The frame of the whole result: every missing address, and how many.
    pub(crate) fn frame(&self) -> Frame {
        self.frame_with(self.missing.iter().map(MissingAddress::to_value).collect())
    }

    /// A frame with `missing` (some of the misses) and the count of them all.
    pub(crate) fn frame_with(&self, missing: Vec<Value>) -> Frame {
        Frame {
            head: vec![
                ("version", Value::from(self.version)),
                ("missing", Value::List(missing)),
                ("missing_total", Value::from(self.missing.len())),
            ],
            items: "nodes",
            tail: Vec::new(),
        }
    }

    /// The frame of a page after the first: the misses are the first page's.
    pub(crate) fn frame_rest(&self) -> Frame {
        Frame {
            head: vec![("version", Value::from(self.version))],
            items: "nodes",
            tail: Vec::new(),
        }
    }

    /// One item per address asked (`addrs`, as given, in order), every text in full.
    pub(crate) fn items(&self, addrs: &[String]) -> Vec<Value> {
        addrs
            .iter()
            .zip(&self.nodes)
            .map(|(a, r)| NodeRead::entry_value(a, r.as_ref(), true))
            .collect()
    }

    /// The read as the show verb's `--json` prints it in one page, for the addresses asked.
    pub fn to_json(&self, addrs: &[String]) -> String {
        self.frame().whole(self.items(addrs)).to_json()
    }
}

impl IndexDiff {
    pub(crate) fn frame(&self) -> Frame {
        Frame {
            head: vec![
                ("from", Value::from(self.from)),
                ("to", Value::from(self.to)),
            ],
            items: "changes",
            tail: Vec::new(),
        }
    }

    pub(crate) fn items(&self) -> Vec<Value> {
        self.changes.iter().map(NodeChange::to_value).collect()
    }

    /// The whole diff as the diff verb's `--json` prints it in one page.
    pub fn to_json(&self) -> String {
        self.frame().whole(self.items()).to_json()
    }
}

impl IndexBlame {
    pub(crate) fn frame(&self) -> Frame {
        Frame {
            head: vec![("version", Value::from(self.version))],
            items: "rows",
            tail: vec![
                ("stopped_at", Value::from(self.stopped_at)),
                (
                    "missing",
                    Value::List(self.missing.iter().map(|v| Value::from(*v)).collect()),
                ),
            ],
        }
    }

    pub(crate) fn items(&self) -> Vec<Value> {
        self.rows.iter().map(NodeBlame::to_value).collect()
    }

    /// The whole blame as the blame verb's `--json` prints it in one page.
    pub fn to_json(&self) -> String {
        self.frame().whole(self.items()).to_json()
    }
}

/// A page's frame: its fields but the item list, in order, with the list's name. The page is
/// `{head…, <items>: [...], tail…, "truncated": t, "cursor": c}`.
pub(crate) struct Frame {
    pub head: Vec<(&'static str, Value)>,
    pub items: &'static str,
    pub tail: Vec<(&'static str, Value)>,
}

impl Frame {
    /// The page holding `items`, with `cursor` when more follow.
    pub(crate) fn page(&self, items: Vec<Value>, cursor: Option<&str>) -> Value {
        let mut o: Vec<(String, Value)> = self
            .head
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect();
        o.push((self.items.to_owned(), Value::List(items)));
        o.extend(self.tail.iter().map(|(k, v)| ((*k).to_owned(), v.clone())));
        o.push(("truncated".to_owned(), Value::from(cursor.is_some())));
        o.push(("cursor".to_owned(), Value::from(cursor)));
        Value::Obj(o)
    }

    /// The whole result as one page (what a typed call's `to_json` prints).
    pub fn whole(&self, items: Vec<Value>) -> Value {
        self.page(items, None)
    }
}

/// One page: items `start..end`, and whether more follow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Cut {
    pub end: usize,
    pub more: bool,
}

/// Where the page that starts at `start` ends under `budget`: as many whole items as fit.
/// `env` is the page's bytes with no items (`[last page, a page with a cursor]`), `sizes`
/// each item's; items are one byte apart. The caller shortens any item that couldn't fit a
/// page alone first, so every page fits; were one still too large, it would be a page by
/// itself.
pub(crate) fn cut(env: [usize; 2], sizes: &[usize], start: usize, budget: usize) -> Cut {
    let len = sizes.len();
    let bytes = |n: usize, more: bool| -> usize {
        let items: usize = sizes[start..start + n].iter().sum();
        env[usize::from(more)] + items + n.saturating_sub(1)
    };
    #[cfg(any(test, feature = "differential"))]
    if crate::differential::index_mutant() == Some(crate::differential::IndexMutant::NoBudget) {
        return Cut {
            end: len,
            more: false,
        };
    }
    if bytes(len - start, false) <= budget {
        return Cut {
            end: len,
            more: false,
        };
    }
    let env = bytes(0, true);
    let mut used = env;
    let mut n = 0;
    while start + n < len {
        let add = sizes[start + n] + usize::from(n > 0);
        if used + add > budget {
            break;
        }
        used += add;
        n += 1;
    }
    // an item larger than a page is a page by itself
    let n = n.max(usize::from(start < len));
    Cut {
        end: start + n,
        more: start + n < len,
    }
}

/// The bytes of `frame`'s page with no items (`more`: with a cursor, every cursor being one
/// length).
pub(crate) fn frame_len(frame: &Frame, more: bool) -> usize {
    frame
        .page(Vec::new(), more.then_some(CURSOR_SAMPLE))
        .to_json()
        .len()
}

// ---------------------------------------------------------------------------------------
// cursors
// ---------------------------------------------------------------------------------------

/// The public salt of the key cursors are tagged under (the space key is the message, as
/// for [`super::digest_key`]).
const CURSOR_KEY_DOMAIN: &[u8] = b"k2-mpc-base/index/cursor/v1";
const CURSOR_PREFIX: &str = "c1.";
/// A cursor's bytes: version (8), offset (4), tag (16).
const CURSOR_BYTES: usize = 28;
/// Every cursor's length, in bytes.
pub(crate) const CURSOR_LEN: usize = CURSOR_SAMPLE.len();
/// Every cursor is this long; a frame is measured with one.
const CURSOR_SAMPLE: &str = "c1.00000000000000000000000000000000000000000000000000000000";

/// What a cursor is bound to: one read of one doc at one version with the same arguments.
pub(crate) struct CursorScope<'a> {
    pub space_key: &'a [u8; 32],
    pub doc: &'a DocKey,
    pub version: VersionId,
    /// The verb, as the kit names it (`list`, `show`, `diff`, `blame`).
    pub verb: &'static str,
    /// The arguments that pick the items, canonically (never the output mode).
    pub args: &'a str,
}

impl CursorScope<'_> {
    fn tag(&self, offset: u32) -> [u8; 16] {
        let mut k = HmacSha256::new(CURSOR_KEY_DOMAIN);
        k.update(self.space_key);
        let key = k.finish();
        let mut m = HmacSha256::new(&key);
        for part in [
            self.doc.space.as_bytes(),
            self.doc.path.as_bytes(),
            self.verb.as_bytes(),
            self.args.as_bytes(),
        ] {
            m.update(&(part.len() as u64).to_be_bytes());
            m.update(part);
        }
        m.update(&self.version.to_be_bytes());
        m.update(&offset.to_be_bytes());
        let mut t = [0u8; 16];
        t.copy_from_slice(&m.finish()[..16]);
        t
    }

    /// The cursor for the page that starts at item `offset`. No path, no workspace: the
    /// version, the offset and a keyed tag.
    pub fn make(&self, offset: u32) -> String {
        let mut b = Vec::with_capacity(CURSOR_BYTES);
        b.extend_from_slice(&self.version.to_be_bytes());
        b.extend_from_slice(&offset.to_be_bytes());
        b.extend_from_slice(&self.tag(offset));
        format!("{CURSOR_PREFIX}{}", hex(&b))
    }

    /// The offset `cursor` continues at: `MPC-E174 bad_cursor` when it isn't one, `MPC-E173
    /// stale_cursor` when it is for another version, doc or read.
    pub fn take(&self, cursor: &str) -> Result<u32, Box<ErrorReport>> {
        let bad = |reason: &str| {
            Box::new(
                ErrorReport::new(
                    &ids::BAD_CURSOR,
                    format!("not a cursor ({reason}); nothing was read"),
                )
                .with("reason", reason),
            )
        };
        let Some(h) = cursor.strip_prefix(CURSOR_PREFIX) else {
            return Err(bad("it doesn't start as a cursor does"));
        };
        let b = unhex(h).ok_or_else(|| bad("it isn't hex"))?;
        if b.len() != CURSOR_BYTES {
            return Err(bad("its length is wrong"));
        }
        let version = u64::from_be_bytes(b[..8].try_into().expect("8 bytes"));
        let offset = u32::from_be_bytes(b[8..12].try_into().expect("4 bytes"));
        let stale = |why: String| {
            Box::new(
                ErrorReport::new(
                    &ids::STALE_CURSOR,
                    "the cursor is for another version, doc or call; nothing was read",
                )
                .with("cursor_version", version)
                .with("version", self.version)
                .with("why", why),
            )
        };
        #[cfg(any(test, feature = "differential"))]
        let check = crate::differential::index_mutant()
            != Some(crate::differential::IndexMutant::CursorUnchecked);
        #[cfg(not(any(test, feature = "differential")))]
        let check = true;
        if check {
            if version != self.version {
                return Err(stale(format!(
                    "it is for version {version}; this read is of version {}",
                    self.version
                )));
            }
            if b[12..] != self.tag(offset) {
                return Err(stale(
                    "it was made for another doc, or a read with other arguments".to_owned(),
                ));
            }
        }
        Ok(offset)
    }
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cursor_sample_is_a_cursor_s_length() {
        let doc = DocKey {
            space: "ws".into(),
            path: "a.idx".into(),
        };
        let s = CursorScope {
            space_key: &[7; 32],
            doc: &doc,
            version: u64::MAX,
            verb: "list",
            args: "",
        };
        assert_eq!(s.make(u32::MAX).len(), CURSOR_SAMPLE.len());
        assert_eq!(s.take(&s.make(5)).unwrap(), 5);
    }

    #[test]
    fn the_longest_increasing_run_is_kept() {
        assert_eq!(lis_keep(&[1, 2, 3]), [true, true, true]);
        assert_eq!(lis_keep(&[3, 1, 2]), [false, true, true]);
        assert_eq!(lis_keep(&[]), Vec::<bool>::new());
        let k = lis_keep(&[2, 3, 1, 4]);
        assert_eq!(k.iter().filter(|x| **x).count(), 3);
        assert!(!k[2]);
    }
}
