//! **Anchors** (slice A3): where a comment is pinned, and what that pin means in a given
//! version of a file (`docs/SESSIONS.md`, "Anchors").
//!
//! Pure and generic: it reads no store and knows no file type, so it is tested alone. An
//! [`Anchor`] names a target (the whole doc, or a node of the structure index by its key)
//! and keeps what identified the node when it was made (its [`Fingerprint`] and the pack's
//! opaque `hint`). [`resolve`] turns an anchor and one version's nodes into exactly one
//! [`AnchorState`], by a fixed precedence ([`resolve`]'s table), with [`Candidate`]s when the
//! key is gone or ambiguous. The file-type half (whether a found node is still the same
//! thing, and what counts as a candidate) is the pack's, through [`AnchorRules`]; the
//! defaults ([`judge_by_default`], [`candidate_by_default`]) use only the fingerprint.
//!
//! **Never moved silently.** Resolving reads an anchor and never changes it: a state is
//! computed on read, for the version asked, and not stored as the anchor. Only an explicit
//! re-anchor (`reanchor`, confirmed by a person) changes where it points; a candidate is a
//! suggestion, shown, never taken. This is the [`crate::refs`] rule for references inside a
//! CRDT doc, applied to files: an identity is followed, never guessed.

use std::collections::HashMap;

use crate::index::{Fingerprint, IndexNode, KeyClass, NodeKey, NodeRect};

/// Most candidates one resolution lists, in the version's reading order.
pub const ANCHOR_CANDIDATES_MAX: usize = 8;
/// Longest pack hint on an anchor, in bytes (the kit never reads it).
pub const ANCHOR_HINT_MAX: usize = 256;

/// Whether the anchor or annotation mutant `$m` is planted on this thread (tests and the
/// `differential` feature only; `false` in any other build).
macro_rules! planted {
    ($m:ident) => {{
        #[cfg(any(test, feature = "differential"))]
        let hit = crate::differential::annot_mutant() == Some(crate::differential::AnnotMutant::$m);
        #[cfg(not(any(test, feature = "differential")))]
        let hit = false;
        hit
    }};
}
pub(crate) use planted;

/// What an anchor points at.
///
/// `#[non_exhaustive]`: a list element (a grid row, a board shape) and a text range come
/// with their resolvers; the tables, rights and events that carry anchors don't look at the
/// target.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AnchorTarget {
    /// The whole doc: always attached, in every version, with or without an index.
    Doc,
    /// A node of the structure index, by its durable key (`@n:42`, `@k:r4.1`).
    Node { key: NodeKey },
}

/// Where an anchor's node was last found: a record for showing an orphan where it was, never
/// where the anchor points.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct LastSeen {
    /// The version (tier 0: a [`crate::store::VersionId`]).
    pub version: u64,
    pub name: String,
    pub bounds: Option<NodeRect>,
}

impl LastSeen {
    pub fn new(version: u64, name: impl Into<String>, bounds: Option<NodeRect>) -> Self {
        Self {
            version,
            name: name.into(),
            bounds,
        }
    }

    /// Where `node` is in `version`.
    pub fn of(node: &IndexNode, version: u64) -> Self {
        Self::new(version, node.name.clone(), node.bounds)
    }
}

/// One anchor: a target, and what identified it when it was made.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Anchor {
    /// The version it was made (or last re-anchored) on.
    pub created_version: u64,
    pub target: AnchorTarget,
    /// The pack's own detail (PSD: a point in the layer's own box). Opaque to the kit, at most
    /// [`ANCHOR_HINT_MAX`] bytes.
    pub hint: Option<String>,
    /// The node as it was when the anchor was made. `None` on a node anchor: a purge took it
    /// (a name is content); the anchor then reads [`AnchorDoubt::HintsPurged`] while its key
    /// is found, and has no candidates once it isn't.
    pub fingerprint: Option<Fingerprint>,
    pub last_seen: Option<LastSeen>,
}

impl Anchor {
    /// The whole doc, from `version`.
    pub fn doc(version: u64) -> Self {
        Self {
            created_version: version,
            target: AnchorTarget::Doc,
            hint: None,
            fingerprint: None,
            last_seen: None,
        }
    }

    /// `node` of `version`'s index, with the pack's `hint`.
    pub fn on_node(node: &IndexNode, version: u64, hint: Option<String>) -> Self {
        Self {
            created_version: version,
            target: AnchorTarget::Node {
                key: node.key.clone(),
            },
            hint,
            fingerprint: Some(node.fingerprint.clone()),
            last_seen: Some(LastSeen::of(node, version)),
        }
    }

    /// The node key it points at (`None`: the whole doc).
    pub fn key(&self) -> Option<&NodeKey> {
        match &self.target {
            AnchorTarget::Node { key } => Some(key),
            AnchorTarget::Doc => None,
        }
    }
}

/// Why an anchor is suspect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AnchorDoubt {
    /// Several nodes hold the key in this version (a duplicated native ID).
    DuplicateKey,
    /// The key was carried from the version before by position alone.
    WeakKey,
    /// The node is of another kind now (the pack's judgment).
    KindChanged,
    /// The node is the same key but, by the pack's judgment, not plainly the same thing.
    IdentityDoubt,
    /// The key is gone, and another node looks like it (a [`Candidate`]).
    CandidateOnly,
    /// A purge took what identified the node; the key alone is left.
    HintsPurged,
}

impl AnchorDoubt {
    /// Every doubt, in precedence order.
    pub const ALL: &'static [AnchorDoubt] = &[
        AnchorDoubt::DuplicateKey,
        AnchorDoubt::WeakKey,
        AnchorDoubt::CandidateOnly,
        AnchorDoubt::HintsPurged,
        AnchorDoubt::KindChanged,
        AnchorDoubt::IdentityDoubt,
    ];

    /// Its word in outputs: `duplicate_key`, `weak_key`...
    pub const fn name(self) -> &'static str {
        match self {
            AnchorDoubt::DuplicateKey => "duplicate_key",
            AnchorDoubt::WeakKey => "weak_key",
            AnchorDoubt::KindChanged => "kind_changed",
            AnchorDoubt::IdentityDoubt => "identity_doubt",
            AnchorDoubt::CandidateOnly => "candidate_only",
            AnchorDoubt::HintsPurged => "hints_purged",
        }
    }
}

/// An anchor's state in one version: exactly one of five.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AnchorState {
    /// Found, and the pack judges it the same node.
    Attached,
    /// Found and the same node, but the pack's hint doesn't fall on it now.
    HintStale,
    /// Found but in doubt, or gone with a candidate, or ambiguous: shown at its last good
    /// place, flagged, never moved.
    Suspect(AnchorDoubt),
    /// Found, but not visible (the node's own flag or an ancestor's).
    Hidden,
    /// The key is gone and nothing looks like it. Keeps its last version and position.
    Orphaned,
}

impl AnchorState {
    /// The five state words, as the spec and outputs spell them.
    pub const NAMES: [&'static str; 5] =
        ["attached", "hint-stale", "suspect", "hidden", "orphaned"];

    /// Its word: `attached`, `hint-stale`, `suspect`, `hidden`, `orphaned`.
    pub const fn name(self) -> &'static str {
        match self {
            AnchorState::Attached => "attached",
            AnchorState::HintStale => "hint-stale",
            AnchorState::Suspect(_) => "suspect",
            AnchorState::Hidden => "hidden",
            AnchorState::Orphaned => "orphaned",
        }
    }

    /// The doubt, when suspect.
    pub const fn doubt(self) -> Option<AnchorDoubt> {
        match self {
            AnchorState::Suspect(d) => Some(d),
            _ => None,
        }
    }

    /// One byte for the store.
    pub(crate) fn code(self) -> u8 {
        match self {
            AnchorState::Attached => 1,
            AnchorState::HintStale => 2,
            AnchorState::Hidden => 3,
            AnchorState::Orphaned => 4,
            AnchorState::Suspect(d) => {
                16 + match d {
                    AnchorDoubt::DuplicateKey => 0,
                    AnchorDoubt::WeakKey => 1,
                    AnchorDoubt::KindChanged => 2,
                    AnchorDoubt::IdentityDoubt => 3,
                    AnchorDoubt::CandidateOnly => 4,
                    AnchorDoubt::HintsPurged => 5,
                }
            }
        }
    }

    /// [`AnchorState::code`] back; `None` for a byte it never wrote.
    pub(crate) fn from_code(c: u8) -> Option<Self> {
        Some(match c {
            1 => AnchorState::Attached,
            2 => AnchorState::HintStale,
            3 => AnchorState::Hidden,
            4 => AnchorState::Orphaned,
            16 => AnchorState::Suspect(AnchorDoubt::DuplicateKey),
            17 => AnchorState::Suspect(AnchorDoubt::WeakKey),
            18 => AnchorState::Suspect(AnchorDoubt::KindChanged),
            19 => AnchorState::Suspect(AnchorDoubt::IdentityDoubt),
            20 => AnchorState::Suspect(AnchorDoubt::CandidateOnly),
            21 => AnchorState::Suspect(AnchorDoubt::HintsPurged),
            _ => return None,
        })
    }
}

/// The pack's judgment of a found node against the anchor's fingerprint
/// ([`AnchorRules::judge`]). One answer: the most severe that applies, in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AnchorJudgment {
    /// The same node. A plain rename or a plain move is the same node.
    Same,
    /// Another kind of node now ([`AnchorDoubt::KindChanged`]).
    KindChanged,
    /// Not plainly the same thing ([`AnchorDoubt::IdentityDoubt`]).
    IdentityDoubt,
    /// The same node, but the hint doesn't fall on it now ([`AnchorState::HintStale`]).
    HintOff,
}

/// Why a node is a candidate for a lost or ambiguous anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CandidateWhy {
    /// The same kind and the same name.
    SameKindAndName,
    /// The same kind and a weak ID in common (the fingerprint's `weak`).
    FingerprintMatch,
    /// The anchor's hint falls inside it (a pack's rule).
    HintInside,
    /// It holds the anchor's key, which several nodes hold in this version.
    SharedNativeId,
}

impl CandidateWhy {
    /// Its word: `same_kind_and_name`, `fingerprint_match`, `hint_inside`, `shared_native_id`.
    pub const fn name(self) -> &'static str {
        match self {
            CandidateWhy::SameKindAndName => "same_kind_and_name",
            CandidateWhy::FingerprintMatch => "fingerprint_match",
            CandidateWhy::HintInside => "hint_inside",
            CandidateWhy::SharedNativeId => "shared_native_id",
        }
    }
}

/// A node a person may re-anchor to: a suggestion, never taken by the kit.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Candidate {
    pub key: NodeKey,
    pub why: CandidateWhy,
}

/// An anchor resolved against one version's index.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct AnchorResolution {
    /// The version it was resolved against.
    pub version: u64,
    /// `false`: the version asked for has no index yet (or none it can have), and this is the
    /// newest earlier version that has one. [`resolve`] sets `true`; the caller knows.
    pub current: bool,
    pub state: AnchorState,
    /// The node it resolved to (`None`: the whole doc, gone, or ambiguous).
    pub node: Option<NodeKey>,
    /// At most [`ANCHOR_CANDIDATES_MAX`], in reading order.
    pub candidates: Vec<Candidate>,
}

/// The file-type half of an anchor's state: whether a found node is still the same thing,
/// and what counts as a candidate when the key is gone. A blob pack's
/// ([`crate::session::BlobPack::judge_anchor`], [`crate::session::BlobPack::anchor_candidate`]);
/// [`KitRules`] is the defaults alone.
pub trait AnchorRules {
    /// `now` holds the anchor's key: is it the same node as `was`? Default
    /// [`judge_by_default`].
    fn judge(&self, was: &Fingerprint, hint: Option<&str>, now: &IndexNode) -> AnchorJudgment {
        let _ = hint;
        judge_by_default(was, now)
    }

    /// The key is gone: is `node` a candidate? Default [`candidate_by_default`].
    fn candidate(
        &self,
        was: &Fingerprint,
        hint: Option<&str>,
        node: &IndexNode,
    ) -> Option<CandidateWhy> {
        let _ = hint;
        candidate_by_default(was, node)
    }
}

/// The defaults alone.
#[derive(Debug, Clone, Copy, Default)]
pub struct KitRules;

impl AnchorRules for KitRules {}

/// The kit's judgment, from the fingerprint alone:
///
/// - another kind: [`AnchorJudgment::KindChanged`] (a pack's kinds are its own words: raster
///   to smart, text to raster are kind changes);
/// - renamed **and** moved by more than its old diagonal (both bounds known) **and** its
///   content changed (or either content hash is unknown): [`AnchorJudgment::IdentityDoubt`];
/// - otherwise [`AnchorJudgment::Same`]: a plain rename, a plain move (however far), a
///   reshape and a content edit are the same node.
///
/// Never [`AnchorJudgment::HintOff`]: the kit can't read a hint.
pub fn judge_by_default(was: &Fingerprint, now: &IndexNode) -> AnchorJudgment {
    if was.kind != now.kind {
        return AnchorJudgment::KindChanged;
    }
    let renamed = was.name != now.name;
    if renamed && planted!(RenameIsDoubt) {
        return AnchorJudgment::IdentityDoubt;
    }
    let same_content = was.content.is_some() && was.content == now.content_hash;
    if renamed && jumped(was.bounds, now.bounds) && !same_content {
        return AnchorJudgment::IdentityDoubt;
    }
    AnchorJudgment::Same
}

/// The kit's candidate rule: a node of the same kind that shares a weak ID with the
/// fingerprint ([`CandidateWhy::FingerprintMatch`]), or has its (non-empty) name
/// ([`CandidateWhy::SameKindAndName`]).
pub fn candidate_by_default(was: &Fingerprint, node: &IndexNode) -> Option<CandidateWhy> {
    if node.kind != was.kind {
        return None;
    }
    if was
        .weak
        .iter()
        .any(|(k, v)| node.fingerprint.weak.get(k) == Some(v))
    {
        return Some(CandidateWhy::FingerprintMatch);
    }
    if !was.name.is_empty() && node.name == was.name {
        return Some(CandidateWhy::SameKindAndName);
    }
    None
}

/// Whether `now`'s centre is further from `was`'s than `was`'s diagonal (both known).
/// Coordinates are clamped to ±2^60 first, so the squares fit in an `i128`.
fn jumped(was: Option<NodeRect>, now: Option<NodeRect>) -> bool {
    let (Some(a), Some(b)) = (was, now) else {
        return false;
    };
    const LIM: i64 = 1 << 60;
    let c = |v: i64| i128::from(v.clamp(-LIM, LIM));
    // doubled centres, so no division
    let dx = (c(b.left) + c(b.right)) - (c(a.left) + c(a.right));
    let dy = (c(b.top) + c(b.bottom)) - (c(a.top) + c(a.bottom));
    let w = c(a.right) - c(a.left);
    let h = c(a.bottom) - c(a.top);
    dx * dx + dy * dy > 4 * (w * w + h * h)
}

/// `a`'s state in `version`, whose index is `nodes` (in reading order), with the pack's
/// `rules`. Exactly one state, by this precedence (the first row that applies):
///
/// | # | State | When |
/// |---|---|---|
/// | 1 | `Suspect(DuplicateKey)`, `Suspect(WeakKey)` | several nodes hold the key; or its key was carried by position alone |
/// | 2 | `Orphaned` | the key is gone and no node is a candidate |
/// | 3 | `Suspect(CandidateOnly)` | the key is gone, and some node is a candidate |
/// | 4 | `Suspect(HintsPurged)` | found, but a purge took the fingerprint |
/// | 4 | `Suspect(KindChanged)`, `Suspect(IdentityDoubt)` | found, and the pack doubts it |
/// | 5 | `Hidden` | found, and not visible in the tree |
/// | 6 | `HintStale` | found, and the pack says the hint is off |
/// | 7 | `Attached` | otherwise |
///
/// A doc anchor is always `Attached`. The anchor is read, never changed: the caller's anchor
/// still points where it did, whatever the state.
pub fn resolve(
    a: &Anchor,
    version: u64,
    nodes: &[IndexNode],
    rules: &dyn AnchorRules,
) -> AnchorResolution {
    resolve_in(a, version, nodes, &NodeIndex::new(nodes), rules)
}

/// One version's nodes by key and by native ID, built once to resolve many anchors against
/// them ([`resolve_in`]): each lookup is a map's, not a scan.
#[derive(Debug, Clone, Default)]
pub struct NodeIndex {
    by_key: HashMap<NodeKey, usize>,
    by_native: HashMap<NodeKey, Vec<usize>>,
}

impl NodeIndex {
    /// The index of `nodes` (a version's, in reading order).
    pub fn new(nodes: &[IndexNode]) -> Self {
        let mut ix = NodeIndex::default();
        for (i, n) in nodes.iter().enumerate() {
            ix.by_key.insert(n.key.clone(), i);
            if let Some(nat) = &n.native {
                ix.by_native.entry(nat.clone()).or_default().push(i);
            }
        }
        ix
    }

    /// Where the node with key `k` is in the nodes this was built from.
    pub fn position(&self, k: &NodeKey) -> Option<usize> {
        self.by_key.get(k).copied()
    }
}

/// [`resolve`] with `nodes`' index built once ([`NodeIndex::new`] of the same `nodes`).
pub fn resolve_in(
    a: &Anchor,
    version: u64,
    nodes: &[IndexNode],
    ix: &NodeIndex,
    rules: &dyn AnchorRules,
) -> AnchorResolution {
    let out = |state, node, candidates| AnchorResolution {
        version,
        current: true,
        state,
        node,
        candidates,
    };
    let key = match &a.target {
        AnchorTarget::Doc => return out(AnchorState::Attached, None, Vec::new()),
        AnchorTarget::Node { key } => key,
    };
    let found = ix.by_key.get(key).and_then(|&i| nodes.get(i));
    // a duplicated native ID: every holder has a minted key, and keeps the ID as `native`
    let holders: Vec<&IndexNode> = ix
        .by_native
        .get(key)
        .into_iter()
        .flatten()
        .filter_map(|&i| nodes.get(i))
        .filter(|n| &n.key != key)
        .collect();
    let held = holders.len() + usize::from(found.is_some());
    let found = if held >= 2 {
        if planted!(DuplicateFirstWins) {
            found.or(holders.first().copied())
        } else {
            let candidates = holders
                .iter()
                .take(ANCHOR_CANDIDATES_MAX)
                .map(|n| Candidate {
                    key: n.key.clone(),
                    why: CandidateWhy::SharedNativeId,
                })
                .collect();
            return out(
                AnchorState::Suspect(AnchorDoubt::DuplicateKey),
                None,
                candidates,
            );
        }
    } else {
        found.or(holders.first().copied())
    };
    let hint = a.hint.as_deref();
    let Some(n) = found else {
        let candidates: Vec<Candidate> = match &a.fingerprint {
            Some(fp) => nodes
                .iter()
                .filter_map(|n| {
                    rules.candidate(fp, hint, n).map(|why| Candidate {
                        key: n.key.clone(),
                        why,
                    })
                })
                .take(ANCHOR_CANDIDATES_MAX)
                .collect(),
            None => Vec::new(),
        };
        if candidates.is_empty() || planted!(CandidateReadsOrphaned) {
            return out(AnchorState::Orphaned, None, candidates);
        }
        return out(
            AnchorState::Suspect(AnchorDoubt::CandidateOnly),
            None,
            candidates,
        );
    };
    let at = Some(n.key.clone());
    if matches!(n.key_class, KeyClass::Minted { weak: true }) && !planted!(WeakKeyTrusted) {
        return out(AnchorState::Suspect(AnchorDoubt::WeakKey), at, Vec::new());
    }
    let Some(fp) = &a.fingerprint else {
        return out(
            AnchorState::Suspect(AnchorDoubt::HintsPurged),
            at,
            Vec::new(),
        );
    };
    let judged = rules.judge(fp, hint, n);
    let doubt = match judged {
        AnchorJudgment::KindChanged => Some(AnchorDoubt::KindChanged),
        AnchorJudgment::IdentityDoubt => Some(AnchorDoubt::IdentityDoubt),
        AnchorJudgment::Same | AnchorJudgment::HintOff => None,
    };
    if !n.visible_in_tree && planted!(HiddenOverDoubt) {
        return out(AnchorState::Hidden, at, Vec::new());
    }
    if let Some(d) = doubt {
        return out(AnchorState::Suspect(d), at, Vec::new());
    }
    if !n.visible_in_tree {
        return out(AnchorState::Hidden, at, Vec::new());
    }
    if judged == AnchorJudgment::HintOff {
        return out(AnchorState::HintStale, at, Vec::new());
    }
    out(AnchorState::Attached, at, Vec::new())
}

#[cfg(test)]
mod tests {
    //! The spec: all five states and every precedence edge. Each runs under the mutant
    //! `MPC_ANNOT_MUTANT` names, when set (the sweep, `tools/README.md`).
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;
    use crate::index::NodeHash;

    fn m(f: impl FnOnce()) {
        crate::differential::under_env_annot_mutant(f)
    }

    fn rect(l: i64, t: i64, r: i64, b: i64) -> Option<NodeRect> {
        Some(NodeRect {
            left: l,
            top: t,
            right: r,
            bottom: b,
        })
    }

    /// A node: key, kind, name, bounds, visible in the tree.
    fn node(key: &str, kind: &str, name: &str, bounds: Option<NodeRect>, vis: bool) -> IndexNode {
        IndexNode {
            key: NodeKey(key.to_owned()),
            key_class: if key.starts_with("@k:") {
                KeyClass::Minted { weak: false }
            } else {
                KeyClass::Native
            },
            native: (!key.starts_with("@k:")).then(|| NodeKey(key.to_owned())),
            kind: kind.to_owned(),
            name: name.to_owned(),
            parent: None,
            order: 0,
            depth: 0,
            bounds,
            visible: vis,
            visible_in_tree: vis,
            text: None,
            attrs: BTreeMap::new(),
            flags: BTreeSet::new(),
            record_hash: NodeHash::from_raw([1; 32]),
            content_hash: None,
            fingerprint: Fingerprint {
                kind: kind.to_owned(),
                name: name.to_owned(),
                bounds,
                content: None,
                weak: BTreeMap::new(),
            },
        }
    }

    fn weak(mut n: IndexNode, k: &str, v: &str) -> IndexNode {
        n.fingerprint.weak.insert(k.to_owned(), v.to_owned());
        n
    }

    /// A node holding a duplicated native ID: a minted key, the ID kept.
    fn dup(minted: &str, native: &str, kind: &str, name: &str) -> IndexNode {
        let mut n = node(minted, kind, name, None, true);
        n.native = Some(NodeKey(native.to_owned()));
        n
    }

    /// The anchor on `n` at version 1.
    fn on(n: &IndexNode) -> Anchor {
        Anchor::on_node(n, 1, None)
    }

    fn state(a: &Anchor, nodes: &[IndexNode]) -> AnchorState {
        resolve(a, 2, nodes, &KitRules).state
    }

    const LOGO: Option<NodeRect> = Some(NodeRect {
        left: 0,
        top: 0,
        right: 10,
        bottom: 10,
    });

    /// A pack's rules for the tests: a hint `off` is stale; a hint `in:K` is inside node K.
    struct HintRules;
    impl AnchorRules for HintRules {
        fn judge(&self, was: &Fingerprint, hint: Option<&str>, now: &IndexNode) -> AnchorJudgment {
            match judge_by_default(was, now) {
                AnchorJudgment::Same if hint == Some("off") => AnchorJudgment::HintOff,
                j => j,
            }
        }
        fn candidate(
            &self,
            was: &Fingerprint,
            hint: Option<&str>,
            node: &IndexNode,
        ) -> Option<CandidateWhy> {
            if hint.and_then(|h| h.strip_prefix("in:")) == Some(node.key.as_str()) {
                return Some(CandidateWhy::HintInside);
            }
            candidate_by_default(was, node)
        }
    }

    #[test]
    fn the_five_states_have_their_words() {
        assert_eq!(
            AnchorState::NAMES,
            ["attached", "hint-stale", "suspect", "hidden", "orphaned"]
        );
        let all = [
            AnchorState::Attached,
            AnchorState::HintStale,
            AnchorState::Suspect(AnchorDoubt::WeakKey),
            AnchorState::Hidden,
            AnchorState::Orphaned,
        ];
        for (s, w) in all.iter().zip(AnchorState::NAMES) {
            assert_eq!(s.name(), w);
        }
        for d in AnchorDoubt::ALL {
            let s = AnchorState::Suspect(*d);
            assert_eq!(AnchorState::from_code(s.code()), Some(s), "{d:?}");
        }
        for s in all {
            assert_eq!(AnchorState::from_code(s.code()), Some(s));
        }
        assert_eq!(AnchorState::from_code(0), None);
        assert_eq!(AnchorState::from_code(22), None);
    }

    #[test]
    fn a_doc_anchor_is_always_attached() {
        m(|| {
            let r = resolve(&Anchor::doc(1), 5, &[], &KitRules);
            assert_eq!(
                (r.state, r.node, r.version),
                (AnchorState::Attached, None, 5)
            );
            assert!(r.current && r.candidates.is_empty());
        });
    }

    #[test]
    fn the_same_node_is_attached() {
        m(|| {
            let n = node("@n:1", "raster", "Logo", LOGO, true);
            let r = resolve(&on(&n), 2, std::slice::from_ref(&n), &KitRules);
            assert_eq!(r.state, AnchorState::Attached);
            assert_eq!(r.node, Some(n.key));
        });
    }

    /// C8: a renamed layer fits no row of the design's table; here it is the same node.
    #[test]
    fn a_renamed_layer_is_attached() {
        m(|| {
            let a = on(&node("@n:1", "raster", "Logo", LOGO, true));
            let now = node("@n:1", "raster", "Logo final", rect(1, 1, 11, 11), true);
            assert_eq!(
                state(&a, &[now]),
                AnchorState::Attached,
                "a rename is attached"
            );
        });
    }

    /// C8: a plain move, however far, is the same node.
    #[test]
    fn a_plain_move_is_attached() {
        m(|| {
            let a = on(&node("@n:1", "raster", "Logo", LOGO, true));
            let far = node("@n:1", "raster", "Logo", rect(500, 500, 510, 510), true);
            assert_eq!(
                state(&a, &[far]),
                AnchorState::Attached,
                "a move is attached"
            );
        });
    }

    #[test]
    fn renamed_and_moved_past_its_diagonal_is_suspect() {
        m(|| {
            let a = on(&node("@n:1", "raster", "Logo", LOGO, true));
            // the diagonal is about 14.1: a centre 14 away is the same node, 15 away is not
            let near = node("@n:1", "raster", "Other", rect(14, 0, 24, 10), true);
            assert_eq!(state(&a, &[near]), AnchorState::Attached);
            let far = node("@n:1", "raster", "Other", rect(15, 0, 25, 10), true);
            assert_eq!(
                state(&a, &[far]),
                AnchorState::Suspect(AnchorDoubt::IdentityDoubt)
            );
            // no bounds on either side: no jump can be seen
            let unknown = node("@n:1", "raster", "Other", None, true);
            assert_eq!(state(&a, &[unknown]), AnchorState::Attached);
        });
    }

    /// The content hash in the fingerprint: renamed and moved far, but the same content, is
    /// the same node; renamed, moved far and new content is not plainly the same thing.
    #[test]
    fn identity_doubt_weighs_the_content() {
        m(|| {
            let with = |mut n: IndexNode, c: u8| {
                n.content_hash = Some(NodeHash::from_raw([c; 32]));
                n.fingerprint.content = n.content_hash;
                n
            };
            let a = on(&with(node("@n:1", "raster", "Logo", LOGO, true), 7));
            let same = with(
                node("@n:1", "raster", "Other", rect(90, 0, 100, 10), true),
                7,
            );
            assert_eq!(state(&a, &[same]), AnchorState::Attached, "same content");
            let new = with(
                node("@n:1", "raster", "Other", rect(90, 0, 100, 10), true),
                8,
            );
            assert_eq!(
                state(&a, &[new]),
                AnchorState::Suspect(AnchorDoubt::IdentityDoubt)
            );
            // a content edit alone, or a rename and a content edit, is the same node
            let edited = with(node("@n:1", "raster", "Other", LOGO, true), 8);
            assert_eq!(state(&a, &[edited]), AnchorState::Attached);
        });
    }

    #[test]
    fn the_jump_test_holds_at_the_i64_extremes() {
        let huge = rect(i64::MIN, i64::MIN, i64::MAX, i64::MAX);
        assert!(!jumped(huge, huge));
        assert!(jumped(LOGO, rect(i64::MAX - 1, 0, i64::MAX, 10)));
    }

    #[test]
    fn another_kind_is_suspect() {
        m(|| {
            let a = on(&node("@n:1", "raster", "Logo", LOGO, true));
            let smart = node("@n:1", "smart", "Logo", LOGO, true);
            assert_eq!(
                state(&a, &[smart]),
                AnchorState::Suspect(AnchorDoubt::KindChanged)
            );
        });
    }

    #[test]
    fn an_invisible_node_is_hidden() {
        m(|| {
            let a = on(&node("@n:1", "raster", "Logo", LOGO, true));
            let mut n = node("@n:1", "raster", "Logo", LOGO, true);
            // its own flag on, an ancestor's off
            n.visible_in_tree = false;
            assert_eq!(state(&a, &[n]), AnchorState::Hidden);
        });
    }

    /// Row 4 before row 5: a kind change shows, hidden or not.
    #[test]
    fn a_doubt_wins_over_hidden() {
        m(|| {
            let a = on(&node("@n:1", "raster", "Logo", LOGO, true));
            let n = node("@n:1", "smart", "Logo", LOGO, false);
            assert_eq!(
                state(&a, &[n]),
                AnchorState::Suspect(AnchorDoubt::KindChanged),
                "a doubt wins over hidden"
            );
        });
    }

    #[test]
    fn a_hint_off_the_node_is_hint_stale() {
        m(|| {
            let n = node("@n:1", "raster", "Logo", LOGO, true);
            let a = Anchor::on_node(&n, 1, Some("off".into()));
            let r = resolve(&a, 2, std::slice::from_ref(&n), &HintRules);
            assert_eq!(r.state, AnchorState::HintStale);
            // the kit's rules never call a hint stale: they can't read one
            assert_eq!(resolve(&a, 2, &[n], &KitRules).state, AnchorState::Attached);
        });
    }

    /// Row 5 before row 6.
    #[test]
    fn hidden_wins_over_a_stale_hint() {
        m(|| {
            let n = node("@n:1", "raster", "Logo", LOGO, false);
            let a = Anchor::on_node(&n, 1, Some("off".into()));
            assert_eq!(resolve(&a, 2, &[n], &HintRules).state, AnchorState::Hidden);
        });
    }

    /// Row 4 before row 6: the judge gives its most severe answer.
    #[test]
    fn a_doubt_wins_over_a_stale_hint() {
        m(|| {
            let a = Anchor::on_node(
                &node("@n:1", "raster", "Logo", LOGO, true),
                1,
                Some("off".into()),
            );
            let n = node("@n:1", "smart", "Logo", LOGO, true);
            assert_eq!(
                resolve(&a, 2, &[n], &HintRules).state,
                AnchorState::Suspect(AnchorDoubt::KindChanged)
            );
        });
    }

    #[test]
    fn a_gone_key_with_nothing_like_it_is_orphaned() {
        m(|| {
            let a = on(&node("@n:1", "raster", "Logo", LOGO, true));
            let before = a.clone();
            let others = [
                node("@n:2", "raster", "Background", LOGO, true),
                node("@n:3", "text", "Logo", LOGO, true),
            ];
            let r = resolve(&a, 2, &others, &KitRules);
            assert_eq!(r.state, AnchorState::Orphaned);
            assert!(r.node.is_none() && r.candidates.is_empty());
            // it keeps where it was, and still points at its key
            assert_eq!(a, before);
            assert_eq!(a.last_seen.as_ref().map(|l| l.version), Some(1));
        });
    }

    /// C9 and K-ANN-9: an orphan with a suggested re-anchor reads suspect, with the
    /// candidate, and still points at its own key.
    #[test]
    fn an_orphan_with_a_candidate_is_suspect() {
        m(|| {
            let a = on(&node("@n:1", "raster", "Logo", LOGO, true));
            let twin = node("@n:7", "raster", "Logo", rect(50, 50, 60, 60), true);
            let r = resolve(&a, 2, &[twin], &KitRules);
            assert_eq!(
                r.state,
                AnchorState::Suspect(AnchorDoubt::CandidateOnly),
                "an orphan with a candidate reads suspect"
            );
            assert_eq!(r.node, None, "a candidate is never taken");
            assert_eq!(
                r.candidates,
                vec![Candidate {
                    key: NodeKey("@n:7".into()),
                    why: CandidateWhy::SameKindAndName
                }]
            );
            assert_eq!(a.key().map(NodeKey::as_str), Some("@n:1"));
        });
    }

    #[test]
    fn a_weak_id_in_common_makes_a_candidate_before_a_name() {
        m(|| {
            let a = on(&weak(
                node("@n:1", "smart", "Logo", LOGO, true),
                "placed",
                "P1",
            ));
            let renamed = weak(node("@n:8", "smart", "Brand", LOGO, true), "placed", "P1");
            let named = node("@n:9", "smart", "Logo", LOGO, true);
            let r = resolve(&a, 2, &[named, renamed], &KitRules);
            assert_eq!(r.state, AnchorState::Suspect(AnchorDoubt::CandidateOnly));
            let why: Vec<(&str, CandidateWhy)> = r
                .candidates
                .iter()
                .map(|c| (c.key.as_str(), c.why))
                .collect();
            assert_eq!(
                why,
                [
                    ("@n:9", CandidateWhy::SameKindAndName),
                    ("@n:8", CandidateWhy::FingerprintMatch)
                ],
                "in reading order"
            );
        });
    }

    #[test]
    fn a_packs_rule_makes_its_own_candidates() {
        m(|| {
            let mut a = on(&node("@n:1", "raster", "Logo", LOGO, true));
            a.hint = Some("in:@n:5".into());
            let other = node("@n:5", "group", "Header", LOGO, true);
            let r = resolve(&a, 2, std::slice::from_ref(&other), &HintRules);
            assert_eq!(r.state, AnchorState::Suspect(AnchorDoubt::CandidateOnly));
            assert_eq!(r.candidates[0].why, CandidateWhy::HintInside);
            assert_eq!(
                state(&a, &[other]),
                AnchorState::Orphaned,
                "not by the kit's"
            );
        });
    }

    #[test]
    fn candidates_are_capped() {
        m(|| {
            let a = on(&node("@n:1", "raster", "Logo", LOGO, true));
            let many: Vec<IndexNode> = (2..40)
                .map(|i| node(&format!("@n:{i}"), "raster", "Logo", LOGO, true))
                .collect();
            let r = resolve(&a, 2, &many, &KitRules);
            assert_eq!(r.candidates.len(), ANCHOR_CANDIDATES_MAX);
            assert_eq!(r.candidates[0].key.as_str(), "@n:2");
        });
    }

    /// Row 1 before rows 2 and 3: a duplicated key is ambiguous, whatever else looks alike.
    #[test]
    fn a_duplicated_key_is_suspect_with_its_holders() {
        m(|| {
            let a = on(&node("@n:4", "raster", "Logo", LOGO, true));
            let nodes = [
                dup("@k:r2.0", "@n:4", "raster", "Logo"),
                node("@n:6", "raster", "Logo", LOGO, true),
                dup("@k:r2.1", "@n:4", "raster", "Logo copy"),
            ];
            let r = resolve(&a, 2, &nodes, &KitRules);
            assert_eq!(
                r.state,
                AnchorState::Suspect(AnchorDoubt::DuplicateKey),
                "a duplicated key is ambiguous"
            );
            assert_eq!(r.node, None, "neither holder is taken");
            let keys: Vec<&str> = r.candidates.iter().map(|c| c.key.as_str()).collect();
            assert_eq!(keys, ["@k:r2.0", "@k:r2.1"]);
            assert!(r
                .candidates
                .iter()
                .all(|c| c.why == CandidateWhy::SharedNativeId));
        });
    }

    /// Row 1 before rows 4 and 5: a key carried by position alone is suspect first.
    #[test]
    fn a_weak_key_is_suspect_before_any_judgment() {
        m(|| {
            let was = node("@k:r1.0", "raster", "Logo", LOGO, true);
            let a = on(&was);
            let mut now = node("@k:r1.0", "smart", "Logo", LOGO, false);
            now.key_class = KeyClass::Minted { weak: true };
            let r = resolve(&a, 2, &[now], &KitRules);
            assert_eq!(
                r.state,
                AnchorState::Suspect(AnchorDoubt::WeakKey),
                "a weak key is suspect"
            );
            assert_eq!(r.node.as_ref().map(NodeKey::as_str), Some("@k:r1.0"));
            // a minted key carried by alignment is trusted
            assert_eq!(state(&a, &[was]), AnchorState::Attached);
        });
    }

    #[test]
    fn purged_hints_leave_the_key_alone() {
        m(|| {
            let n = node("@n:1", "raster", "Logo", LOGO, true);
            let mut a = on(&n);
            a.fingerprint = None;
            assert_eq!(
                state(&a, std::slice::from_ref(&n)),
                AnchorState::Suspect(AnchorDoubt::HintsPurged)
            );
            // gone: without a fingerprint nothing can be a candidate
            let twin = node("@n:7", "raster", "Logo", LOGO, true);
            assert_eq!(state(&a, &[twin]), AnchorState::Orphaned);
            // a weak key still reads weak first (row 1)
            let mut w = n;
            w.key_class = KeyClass::Minted { weak: true };
            assert_eq!(state(&a, &[w]), AnchorState::Suspect(AnchorDoubt::WeakKey));
        });
    }

    #[test]
    fn the_default_judge_is_the_spec() {
        m(|| {
            let was = node("@n:1", "raster", "Logo", LOGO, true).fingerprint;
            let j = |n: IndexNode| judge_by_default(&was, &n);
            assert_eq!(
                j(node("@n:1", "raster", "Logo", LOGO, true)),
                AnchorJudgment::Same
            );
            assert_eq!(
                j(node("@n:1", "raster", "Renamed", LOGO, true)),
                AnchorJudgment::Same,
                "a plain rename is the same node"
            );
            assert_eq!(
                j(node("@n:1", "text", "Logo", LOGO, true)),
                AnchorJudgment::KindChanged
            );
            assert_eq!(
                j(node("@n:1", "raster", "R", rect(100, 0, 110, 10), true)),
                AnchorJudgment::IdentityDoubt
            );
        });
    }
}
