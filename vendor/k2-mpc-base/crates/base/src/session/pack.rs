//! What a file-type pack gives a session: its schema and its file codec.

use std::borrow::Cow;
use std::fmt;

use super::view::View;
use crate::codec::text_file::{self, Eol, FileFormat};
use crate::doc::{Edit, Neutral};
use crate::elem::Key;
use crate::schema::{Kind, Schema};
use crate::table::{self, LineMap, Table};

/// A file decoded into doc content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackContent {
    /// Each schema root's content, in any order; every root exactly once.
    pub roots: Vec<(String, Neutral)>,
    /// The codec's own settings for this file (e.g. BOM and line endings), opaque to the
    /// kit. Stored with the doc and handed back to [`Pack::encode`].
    pub codec: Vec<u8>,
}

/// Why a file can't be a live doc (it then opens the old way).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declined {
    /// Stable machine code, e.g. `too_large`, `not_utf8`, `binary`, `mixed_line_endings`.
    pub code: &'static str,
    /// Human-readable reason.
    pub message: String,
}

impl fmt::Display for Declined {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

/// What [`Pack::reconcile`] made of a file's new bytes: the edits that make the doc as the
/// file last held it into what the file holds now (one change, applied **by key**), and the
/// file's codec settings now (an outside save can change the dialect).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Reconciled {
    pub edits: Vec<Edit>,
    pub codec: Vec<u8>,
    /// A table's rows the file left exact ([`table::Reconciled::exact_rows`], v0.10.0). Any
    /// other kept row merges as one unit: a pack that reports none gets the strict reading, so
    /// it can't leave the row rule off by leaving this out. Ignored for text.
    pub exact_rows: Vec<crate::elem::Key>,
    /// The column mapping was exact ([`table::Reconciled::cols_exact`]): false (the default) is
    /// the strict reading, a remap, which refuses every keyed write from an older base.
    pub cols_exact: bool,
}

/// A pack failed to encode (bad stored codec settings) or returned content that doesn't fit
/// its schema. Always a bug in the pack or a damaged store.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("pack: {0}")]
pub struct PackError(pub String);

/// The keys of the units a carrier holds (`docs/SESSIONS.md`, "The carrier's pairing"):
/// root by root, in the order the pack's decode of the carrier yields them. **Keys only, never
/// a value**: the values a pack splices against come from the carrier itself. Made by
/// [`Pack::pairing`] at every sync point and kept by the kit with every carrier, live and per
/// version, for a pack that mints its keys ([`Pack::wants_pairing`]).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Pairing {
    pub roots: Vec<(String, Vec<Key>)>,
}

/// The record version of [`Pairing::to_bytes`].
const PAIRING_V1: u8 = 1;

impl Pairing {
    /// The keys of root `root`, if the pairing names it.
    pub fn keys(&self, root: &str) -> Option<&[Key]> {
        self.roots
            .iter()
            .find(|(r, _)| r == root)
            .map(|(_, k)| k.as_slice())
    }

    /// How many keys in all.
    pub fn len(&self) -> usize {
        self.roots.iter().map(|(_, k)| k.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The stored form: a version byte, then each root's name and keys, length-prefixed.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = vec![PAIRING_V1];
        let mut put = |b: &[u8]| {
            out.extend_from_slice(&(b.len() as u32).to_le_bytes());
            out.extend_from_slice(b);
        };
        put(&(self.roots.len() as u32).to_le_bytes());
        for (root, keys) in &self.roots {
            put(root.as_bytes());
            put(&(keys.len() as u32).to_le_bytes());
            for k in keys {
                put(k.as_str().as_bytes());
            }
        }
        out
    }

    /// [`Pairing::to_bytes`]'s inverse; `None` for another version or a damaged record (read as
    /// no pairing: the kit makes it again or refuses, never guesses).
    pub fn from_bytes(b: &[u8]) -> Option<Pairing> {
        let (&v, mut rest) = b.split_first()?;
        if v != PAIRING_V1 {
            return None;
        }
        fn take<'a>(rest: &mut &'a [u8]) -> Option<&'a [u8]> {
            let n = u32::from_le_bytes(rest.get(..4)?.try_into().ok()?) as usize;
            let body = rest.get(4..4usize.checked_add(n)?)?;
            *rest = &rest[4 + n..];
            Some(body)
        }
        fn count(b: &[u8]) -> Option<usize> {
            Some(u32::from_le_bytes(b.try_into().ok()?) as usize)
        }
        let roots = count(take(&mut rest)?)?;
        let mut out = Vec::new();
        for _ in 0..roots {
            let root = std::str::from_utf8(take(&mut rest)?).ok()?.to_owned();
            let n = count(take(&mut rest)?)?;
            let mut keys = Vec::new();
            for _ in 0..n {
                keys.push(Key::new(std::str::from_utf8(take(&mut rest)?).ok()?).ok()?);
            }
            out.push((root, keys));
        }
        rest.is_empty().then_some(Pairing { roots: out })
    }
}

/// Why [`Pack::encode_from_paired`] didn't splice.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SpliceError {
    /// The pairing doesn't fit the carrier (another number of units in a root, a root it
    /// doesn't name): the pack can't tell which file unit a key is, so nothing was spliced.
    #[error("the carrier's pairing doesn't fit it: {0}")]
    PairingMismatch(String),
    #[error(transparent)]
    Pack(#[from] PackError),
}

/// [`Pack::pairing`]'s default: each list root's keys, in list order.
pub fn list_pairing(content: &[(String, Neutral)]) -> Pairing {
    Pairing {
        roots: content
            .iter()
            .filter_map(|(r, n)| match n {
                Neutral::List(l) => Some((r.clone(), l.iter().map(|(k, _)| k.clone()).collect())),
                _ => None,
            })
            .collect(),
    }
}

/// The element a key of a [composite](Pack::composite) Map root belongs to: its list root and
/// the element's key, which is everything before the key's first
/// [`crate::table::CELL_SEP`]. `None` when `map_root` isn't declared, the key has no separator,
/// or what comes before it isn't a valid [`Key`]. It reads no doc: the element may be live,
/// deleted or unknown.
pub fn elem_of_map_key<'a>(
    composites: &[(&'a str, &'a str)],
    map_root: &str,
    key: &str,
) -> Option<(&'a str, Key)> {
    let (_, list) = composites.iter().find(|(m, _)| *m == map_root)?;
    let (elem, _) = key.split_once(crate::table::CELL_SEP)?;
    Some((*list, Key::new(elem).ok()?))
}

/// A pack's verb state ([`Pack::verb_state_build`]): any type that is `Send + Sync`, `Debug`
/// and `PartialEq` is one (a blanket impl), so a pack returns `Box::new(its_index)` and reads
/// it back with [`crate::tools::VerbCtx::verb_state`] or `downcast_mut` on a `dyn VerbState`. Equality
/// is what test builds compare a kept state with a fresh build by. Hold no interior
/// mutability (no `Cell`, `RefCell` or locks): the kit hands it out as a shared borrow under
/// the session's lock and changes it only through [`Pack::verb_state_change`].
pub trait VerbState: std::any::Any + Send + Sync + fmt::Debug {
    /// As `Any`, to read it back as its type.
    fn as_any(&self) -> &dyn std::any::Any;
    /// As `Any`, mutable.
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
    /// Whether `other` is the same state: the same type and equal.
    fn same_as(&self, other: &dyn VerbState) -> bool;
}

impl<T> VerbState for T
where
    T: std::any::Any + Send + Sync + fmt::Debug + PartialEq,
{
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
    fn same_as(&self, other: &dyn VerbState) -> bool {
        other.as_any().downcast_ref::<T>() == Some(self)
    }
}

impl dyn VerbState {
    /// The state as its type `T`, if it is one.
    pub fn downcast_ref<T: VerbState>(&self) -> Option<&T> {
        self.as_any().downcast_ref::<T>()
    }
    /// The state as its type `T`, mutable, if it is one.
    pub fn downcast_mut<T: VerbState>(&mut self) -> Option<&mut T> {
        self.as_any_mut().downcast_mut::<T>()
    }
}

/// A file type, as a session uses it. Implemented by each pack (Markdown, CSV...).
pub trait Pack: Send + Sync {
    /// The doc model. Its `name` is stamped on each stored doc, and a stored doc only opens
    /// with the pack of the same name.
    fn schema(&self) -> Schema;

    /// File bytes to doc content, or why this file can't be live.
    fn decode(&self, bytes: &[u8]) -> Result<PackContent, Declined>;

    /// Doc content back to file bytes, in the file's own format (`codec` from
    /// [`Pack::decode`]). An untouched file must round-trip byte for byte.
    fn encode(&self, content: &[(String, Neutral)], codec: &[u8]) -> Result<Vec<u8>, PackError>;

    /// Whether the kit keeps this pack's **carrier**: for a format the pack models only in
    /// part (a deck: slides and shapes modelled, the rest of the file opaque), the bytes the kit
    /// last read from the file or wrote to it, so that the doc is written back by splicing its
    /// content into them ([`Pack::encode_from`]) and every byte the pack doesn't model stays
    /// out of the engine. `docs/SESSIONS.md`, "The carrier". Default: `false`, and the kit
    /// keeps nothing and calls [`Pack::encode`], as before.
    fn wants_carrier(&self) -> bool {
        false
    }

    /// Doc content back to file bytes **by splicing it into `carrier`**, the bytes the kit last
    /// read from the file or wrote to it, exactly. Called instead of [`Pack::encode`] for a
    /// live doc whenever [`Pack::wants_carrier`] is `true` and the kit holds a carrier for it;
    /// with no carrier held (a doc never synced with its file, or one whose carrier couldn't be
    /// kept) the kit calls [`Pack::encode`]. The carrier never leaves the kit but to this hook.
    ///
    /// The pack decodes `carrier` itself (the content to diff against is the carrier's, never
    /// another copy) and splices only what differs from `content`; every other byte is copied.
    /// The rules, which the kit relies on:
    /// - **untouched is byte for byte:** when `content` is what `carrier` decodes to, the
    ///   result is `carrier` itself (so it is idempotent: splicing the same content into what
    ///   it returned changes nothing);
    /// - **it decodes to the content:** [`Pack::decode`] of the result gives `content`;
    /// - a pack that can't splice returns [`PackError`] (the write-back fails, the file is left
    ///   as it was).
    ///
    /// Default: [`Pack::encode`], ignoring the carrier.
    fn encode_from(
        &self,
        carrier: &[u8],
        content: &[(String, Neutral)],
        codec: &[u8],
    ) -> Result<Vec<u8>, PackError> {
        let _ = carrier;
        self.encode(content, codec)
    }

    /// Whether the kit keeps a **pairing** with the carrier (`docs/SESSIONS.md`, "The
    /// carrier's pairing"): for a pack that keeps a carrier ([`Pack::wants_carrier`]) and
    /// **mints** its keys (rows with no ids, timeline items, scene objects), so that
    /// [`Pack::decode`] of the carrier can't give the live keys once a live insert or an app's
    /// save has moved its units. The kit then makes a [`Pairing`] at every sync point
    /// ([`Pack::pairing`]), keeps it with every carrier, live and per version, and calls
    /// [`Pack::encode_from_paired`] instead of [`Pack::encode_from`]. Default: `false` (a pack
    /// whose keys are the file's own ids, a deck's shape ids, needs none: nothing is made or
    /// kept, and `encode_from` is called as before).
    fn wants_pairing(&self) -> bool {
        false
    }

    /// The pairing of `content` as the file it renders to holds it: each root's keys in the
    /// order this pack's [`Pack::decode`] of that file yields the units. Called by the kit at
    /// every sync point (a kit write, an outside save taken in, a change that moved no byte, an
    /// open with no pairing stored) with the content the file holds there. Keys only: a key
    /// must hold no user content. Default: [`list_pairing`] (each list root's keys in list
    /// order), right for a pack whose decode yields a list root's units in list order.
    fn pairing(&self, content: &[(String, Neutral)], codec: &[u8]) -> Result<Pairing, PackError> {
        let _ = codec;
        Ok(list_pairing(content))
    }

    /// Doc content back to file bytes by splicing it into `carrier`, whose units carry the keys
    /// `pairing` gives. Called instead of [`Pack::encode_from`] when
    /// [`Pack::wants_pairing`] is `true`, with the carrier's own pairing: the live carrier's for
    /// the live doc, a version's own for a version's file.
    ///
    /// The pack decodes `carrier`, **takes every value from it**, and puts `pairing`'s keys on
    /// its units in order; a pairing that doesn't fit (another number of units in a root, a
    /// root it lacks) is [`SpliceError::PairingMismatch`], and nothing is spliced: **never a
    /// splice by guess**. Then it diffs that keyed carrier against `content` by key and splices
    /// the diff. The rules of [`Pack::encode_from`] hold: untouched is byte for byte, the result
    /// decodes to `content`, a failure leaves the file as it was.
    ///
    /// Default: [`Pack::encode_from`], ignoring the pairing.
    fn encode_from_paired(
        &self,
        carrier: &[u8],
        pairing: &Pairing,
        content: &[(String, Neutral)],
        codec: &[u8],
    ) -> Result<Vec<u8>, SpliceError> {
        let _ = pairing;
        Ok(self.encode_from(carrier, content, codec)?)
    }

    /// Normalise text an editor or agent is inserting into `root` (e.g. `\r\n` to `\n`).
    /// Applied to every inserted string and to `write` bodies before merging.
    fn normalize_input<'a>(&self, root: &str, text: &'a str) -> Cow<'a, str> {
        let _ = root;
        Cow::Borrowed(text)
    }

    /// Whether `text` may be `root`'s text after a whole-line merge: a format whose
    /// lines depend on each other (JSON's commas and brackets) can refuse a merge that two
    /// valid texts made invalid. The kit asks it of a text doc's merges only, and acts on a
    /// refusal only when both merged texts pass and the merge doesn't:
    /// - an outside save ([`super::Session::file_changed`]): the save is taken whole instead of
    ///   merged. The live text it replaces stays in the "before external edit" version, and
    ///   the removal notes tell its authors what went and which version holds it;
    /// - `write --base`'s clean merge: refused as one conflict over every line either side
    ///   changed ([`crate::text::merge::refused_merge`]); nothing is applied.
    ///
    /// Default: every text passes (the merge is taken as it is).
    fn check_merged(&self, root: &str, text: &str) -> Result<(), Declined> {
        let _ = (root, text);
        Ok(())
    }

    /// Whether `view` may be a keyed doc's content after the kit merged an outside save into
    /// it field by field, from the version the app opened ([`super::Session::note_opened`]):
    /// two changes that were each valid can make a state the format's rules forbid (two clips
    /// over one place, two objects of one name). `Err`: the save keeps both, as with no pin.
    /// Bounded, no I/O. **Default: refused**, so a keyed doc's saves merge only for a pack
    /// that checks its own rules here.
    fn check_merged_view(&self, view: &super::View) -> Result<(), Declined> {
        let _ = view;
        Err(Declined {
            code: "merge_unchecked",
            message: "this file type doesn't check a merged doc".into(),
        })
    }

    /// How this pack's text splits into tokens for D1 (undo spec v1.10): a fresh change
    /// rewrites the tokens it touches wherever someone else wrote them, so it is never built
    /// from someone else's text. Words by default; a code pack would use
    /// [`crate::text::lines::Tokens::Code`].
    fn tokens(&self) -> crate::text::lines::Tokens {
        crate::text::lines::Tokens::Words
    }

    /// Whether the file's bytes put a byte-order mark (U+FEFF, UTF-8 `EF BB BF`) in front
    /// of `root`'s text, under these codec settings. The doc text never holds it, but `cat`
    /// prints the file's bytes, BOM included; so a `write` or `write --base` body that
    /// starts with U+FEFF has that one U+FEFF taken off when this is true (else text read
    /// with `cat` and written back would gain a second BOM).
    ///
    /// Default: for a pack whose schema is one text root, whether [`Pack::encode`] of that
    /// root, empty, starts with the BOM's bytes. A pack with other roots or another encoding
    /// overrides it (or keeps the default, `false`).
    fn writes_bom(&self, root: &str, codec: &[u8]) -> bool {
        let schema = self.schema();
        match schema.roots {
            [(r, Kind::Text)] if *r == root => self
                .encode(&[(root.to_owned(), Neutral::Text(String::new()))], codec)
                .is_ok_and(|b| b.starts_with(text_file::BOM)),
            _ => false,
        }
    }

    /// A block of records in the doc's dialect (a structured verb's stdin: `cells set`'s
    /// pasted block, `row insert`'s values), decoded with the doc's `codec` settings. The kit
    /// never parses a file format itself: a table pack implements this with its codec.
    /// Default: declined `unsupported` (the verb exits 2 with the reason).
    fn decode_block(&self, bytes: &[u8], codec: &[u8]) -> Result<Vec<table::Record>, Declined> {
        let _ = (bytes, codec);
        Err(Declined {
            code: "unsupported",
            message: "this pack reads no block of records from stdin".into(),
        })
    }

    /// `header on|off` on a table (the grid verbs' `header`): the edits that switch `t`'s
    /// header, applied to `t`. **The rule lives in [`table::Table::set_header`]** (the switch
    /// never changes the rendered bytes; a blank only row can't become the header), and the
    /// default delegates to it. A pack overrides this only to decline more for its format
    /// (the verb exits 2 with the reason, nothing applied); it never re-implements the rule.
    fn set_header(
        &self,
        t: &mut table::Table,
        on: bool,
        codec: &[u8],
    ) -> Result<Vec<crate::doc::Edit>, Declined> {
        let _ = codec;
        t.set_header(on).map_err(|e| Declined {
            // the refusal with its own id keeps its name (MPC-E806 at the verb)
            code: match e {
                table::TableError::HeaderNamesUnrendered => "header_names_unrendered",
                table::TableError::HeaderOnlyRowBlank => "header_only_row_blank",
                _ => "refused",
            },
            message: e.to_string(),
        })
    }

    /// How an outside save changed only the file's codec settings (a CSV re-saved with a BOM,
    /// CRLF or another delimiter: the content the same), from `old` to `new`, for the log and
    /// `show` (computed when read, never stored). Default: "codec settings changed".
    fn describe_codec_change(&self, old: &[u8], new: &[u8]) -> String {
        let _ = (old, new);
        "codec settings changed".to_owned()
    }

    /// The pack's own edit verbs (Markdown's `replace-section`), beside the kit's text
    /// tools. [`super::Session::run_tool`] routes a verb no kit tool has by this name, and
    /// [`crate::parity::Registry::for_pack`] registers them for the parity harness. A name
    /// the kit already uses is never routed (the kit's verb wins) and the registry refuses
    /// it. Default: none.
    fn verbs(&self) -> &'static [crate::tools::PackVerb] {
        &[]
    }

    /// **The pack's own refusals, by stable id** (each [`crate::errors::ErrorEntry::pack`]): what
    /// its verbs refuse with ([`crate::tools::StructOutput::refused`]), so a refusal reaches the
    /// host as data (`refusal`, `--json`'s `error`) and `explain` and `capabilities` list it.
    /// [`crate::parity::Registry::for_pack`] checks them (ids `<PACK>-Ennn`, the pack's name
    /// upper-cased; no id or name twice; an exit from the kit's table, never 0). A verb that
    /// still prints `refused: <code>: <why>` itself is reported under the entry whose name is
    /// `<code>`. Ids are stable forever, as the kit's are. Default: none.
    fn errors(&self) -> &'static [crate::errors::ErrorEntry] {
        &[]
    }

    /// The native ID kinds this pack's codec and own verbs read, as data: each
    /// kind's class, scope and the verbs that accept it. The pack's `docs/IDS.md` is
    /// generated from it ([`crate::ids::markdown`]) and checked against its registry
    /// ([`crate::ids::check`]). List only the pack's own kinds (Markdown's headings, docx's
    /// `w14:paraId`): the kit's (lines, `L:C`, grid keys, `#N`, versions...) are
    /// [`crate::ids::KIT`]. Default: none (a format with no IDs of its own, as plain text).
    fn ids(&self) -> &'static [crate::ids::IdKind] {
        &[]
    }

    /// The MovableList roots whose keys are **tagged** ([`crate::mint`]), each with the key
    /// prefixes it takes: `(root, prefixes)`. An insert arriving from a connection into such a
    /// root must name `<prefix><n>.<tag>` with one of the prefixes and the connection's own
    /// writer tag ([`super::SessionError::KeyNotOwn`], exit 2); [`super::Session::mint_key`]
    /// makes one. Imports, outside saves and keys the list already knows are not checked.
    /// Default: none.
    fn tagged_keys(&self) -> &'static [(&'static str, &'static [&'static str])] {
        &[]
    }

    /// The Map root that holds the retired marks of [`Pack::tagged_keys`]' roots
    /// ([`crate::mint::mark_key`]). A change that drops tagged keys writes the dropper's own
    /// marks there in the same change ([`crate::mint::marks_for_drops`]), and
    /// [`super::Session::mint_key`] reads the max over every dropper's. `None` (the default):
    /// [`super::Session::mint_key`] reads no marks.
    fn mint_marks(&self) -> Option<&'static str> {
        None
    }

    /// The Map roots whose keys belong to list elements, as `(map_root, list_root)` pairs: each
    /// key of `map_root` is an element key of `list_root`, then [`crate::table::CELL_SEP`], then
    /// the rest (`<item>\x1f<field>`). So a write to such a key counts as touching its element
    /// ([`elem_of_map_key`]; a claim on the element covers it). **It fails open:** a key that
    /// maps to no element isn't covered by any claim (one with no separator, or whose start
    /// isn't a valid [`Key`]), so a pack writes only keys of this shape to these roots. A map root
    /// appears once: the first pair naming it is the one read. Default: none.
    fn composite(&self) -> &'static [(&'static str, &'static str)] {
        &[]
    }

    /// The container element `key` of the list root `root` sits in, as `(list root, key)`
    /// (`focus` is the container in view). The kit asks it when
    /// a text range is set ([`super::Session::set_range`]: the range's element's container), when
    /// an agent takes a claim on elements ([`super::Session::claim_elems`]: the first container
    /// in document order) and when an agent's verb touches elements.
    ///
    /// **A pack with a view schema doesn't need it** ([`Pack::view_schema`]):
    /// for its units the kit reads the unit's container field
    /// ([`crate::view_index::ViewSchema::unit_container_field`]), and a container is its own;
    /// this is asked for the other roots only. It reads no doc: a container is named by the
    /// element's key alone (a shape `s256-7` on slide `s256`). Default: every element is its own
    /// container. `None`: the element is in no container, and focus is left as it was.
    fn container_of(&self, root: &str, key: &Key) -> Option<(String, Key)> {
        Some((root.to_owned(), key.clone()))
    }

    /// The pack's view schema: which list roots hold the
    /// containers and the units, and where each view field is. The kit builds the view index
    /// from it ([`crate::view_index::ViewIndex::live`], [`super::Session::view_index`]), and finds
    /// a unit's container through it. Default `None`: the pack has no view index (a text pack
    /// is viewed by its lines and carets).
    fn view_schema(&self) -> Option<crate::view_index::ViewSchema> {
        None
    }

    /// The view fields only the pack can work out (the schema's `derived`: a deck's slide
    /// titles, inherited boxes, fonts, a group's children), set on `index` after the kit built
    /// it from the schema; `source` is what the kit built it from. The kit then puts the units
    /// in their containers' order (stable: a nested unit the hook adds goes right after its
    /// parent in its container's run) and counts them. Bounded, no I/O. Default: nothing.
    ///
    /// **The carrier**: on the live tier, for a pack that keeps one
    /// ([`Pack::wants_carrier`]), `source` also holds the doc's carrier when the kit holds it
    /// ([`crate::view_index::ViewSource::Live`], [`crate::view_index::ViewCarrier`]), for what
    /// only the file's bytes hold. The rule: **the live store wins**, and the carrier supplies
    /// only what the store doesn't model (a field the store holds is never read from the
    /// carrier; an element the carrier holds and the store changed is derived from the store's
    /// value, as a group's children from the group's live box). The pack decodes the carrier
    /// through [`crate::view_index::ViewCarrier::decoded`], once per carrier, never per call:
    /// the kit keeps the index by (carrier id, rev), so the hook runs once per change.
    fn view_derive(
        &self,
        source: crate::view_index::ViewSource<'_>,
        index: &mut crate::view_index::ViewIndex,
    ) {
        let _ = (source, index);
    }

    /// [`Pack::view_derive`] for a change, so the kit keeps a live doc's view index through
    /// changes instead of building it again ([`super::Session::view_index`]). `index` is the
    /// index before the changes as the pack derived it, with what `touched` names built again
    /// by the kit: those units and containers have their kit fields and none of the pack's
    /// (no derived fields, no derived values, no nested units under a touched unit); every
    /// other unit and container, and the doc's fonts and layouts, are as the pack left them.
    /// The hook derives again what the changes touched (and anything it knows depends on them:
    /// `touched.roots` names the other roots they wrote) and returns `true`. The result must
    /// be the index [`Pack::view_derive`] gives on a whole build; test builds check it after
    /// every change and fail loudly on a difference.
    ///
    /// Default `false`: the pack doesn't derive by change, and the kit builds the index whole
    /// on every change (as it did before this hook; it asks once a session, then stops).
    fn view_derive_change(
        &self,
        source: crate::view_index::ViewSource<'_>,
        index: &mut crate::view_index::ViewIndex,
        touched: &crate::view_index::ViewTouched,
    ) -> bool {
        let _ = (source, index, touched);
        false
    }

    /// Element `key` of list root `root` (a unit, or a container when the 3D slot has container
    /// fields), placed: read from **its own fields** in `source` (the keyed state, and the
    /// carrier when the pack keeps one), in the file's own form
    /// ([`crate::placement::Placement`]). Pure and bounded. The kit asks it once per element on
    /// a whole build, then only for an element whose placement fields
    /// ([`crate::placement::SpaceSlot::unit_fields`]) a change wrote, which is how it names
    /// `place` deltas from a change's ops: a placement that reads another element's fields
    /// would go stale (test builds check every kept index against a whole build). `None`: not
    /// placed by the slot. Default: none.
    fn view_placement(
        &self,
        source: crate::view_index::ViewSource<'_>,
        root: &str,
        key: &Key,
    ) -> Option<crate::placement::Placement> {
        let _ = (source, root, key);
        None
    }

    /// The doc's space, for a 3D slot whose space is the pack's
    /// ([`crate::placement::SpaceSrc::Pack`]), read from the roots it names. Default: none.
    fn view_space(
        &self,
        source: crate::view_index::ViewSource<'_>,
    ) -> Option<crate::placement::Space> {
        let _ = source;
        None
    }

    /// Per-unit derived values a keyed watch's snapshot carries on the pages of the units'
    /// fields (a grid's formula cells' A1 text, so a formula never shows as its cached number
    /// while the snapshot loads): for each of `units` (keys of the view schema's units, the
    /// ones whose fields the snapshot pages), the values by field. `state` is the doc's keyed
    /// state at the snapshot's rev. The kit puts each value on the page that holds that field's
    /// entry, as `derived.<name>.<unit address>.<field>`; a value for a field with no entry is
    /// left out. Bounded, no I/O, called once per snapshot. Default: none.
    fn page_derived(
        &self,
        state: &crate::elem::KeyedState,
        units: &[Key],
    ) -> Vec<crate::grid::PageDerived> {
        let _ = (state, units);
        Vec::new()
    }

    /// The address of element `key` of the MovableList root `root`: how the
    /// view index, the presence and claim setters by address and the frames name it on the
    /// wire (a deck's `s256-7` is `@sld:256/sp:7`). One to one: [`Pack::key_of`] gives `key`
    /// back, and no element of another root has the same address. Default: the key itself.
    fn address_of(&self, root: &str, key: &Key) -> String {
        let _ = root;
        key.as_str().to_owned()
    }

    /// The inverse of [`Pack::address_of`]: the key of the element of `root` that `address`
    /// names, or `None` when it can't name one of `root`'s (the kit then tries the other roots,
    /// [`crate::view_index::key_candidates`]). It reads no doc: whether the element exists is
    /// the kit's to check. Default: the address as a key.
    fn key_of(&self, root: &str, address: &str) -> Option<Key> {
        let _ = root;
        Key::new(address).ok()
    }

    /// The list and map roots whose values may name a blob ([`crate::blob::BlobId::parse_ref`]:
    /// a value that is exactly `blob:<64 hex>`). The doc holds a reference on each blob such a
    /// value names, so blob GC keeps it while any record of the doc names it; a connection's
    /// edit naming a blob this doc's space doesn't hold is refused
    /// ([`super::SessionError::BlobRefUnknown`], exit 4). A value in any other root is ordinary
    /// text, whatever it spells. Default: none.
    fn blob_ref_roots(&self) -> &'static [&'static str] {
        &[]
    }

    /// This file type's recommended live size limit, for a keyed doc of it that isn't a table:
    /// its elements and fields, as [`super::Config::keyed_max_entries`] counts them, measured on
    /// the type's own shapes (a timeline's items carry many more fields than a board's notes).
    /// The host owns the limit: this applies only when the host set none for this type
    /// ([`super::Config::keyed_max_entries_by_pack`]), and only when it is below the host-wide
    /// one: a pack may lower the limit, never raise it ([`super::Config::keyed_limit`]). A type
    /// that needs more gets it from the host's limit for the type. A refusal names the limit
    /// that applied and where it came from. Default `None`: the host's limits apply.
    fn keyed_entries_default(&self) -> Option<usize> {
        None
    }

    /// The pack's **verb state** for a live keyed doc that isn't a table (`docs/SESSIONS.md`,
    /// "Verb state"): what its structured verbs read beside the doc and would otherwise compute
    /// on every call (a timeline's per-track index of items by start), built from the live
    /// view. The kit builds it the first time a structured verb runs on the doc, keeps it
    /// beside the doc's live view and hands it to every verb as a read-only borrow
    /// ([`crate::tools::VerbCtx::verb_state`]); it keeps it in step through every change
    /// ([`Pack::verb_state_change`]). It is the pack's own: never stored, never on the wire,
    /// dropped with the live view (a reload, a purge or erasure, a demotion or an unload) and
    /// built again on the next verb. Bounded, no I/O, a pure function of `view`.
    ///
    /// Default `None`: the pack keeps none, and nothing changes for it.
    fn verb_state_build(&self, view: &View) -> Option<Box<dyn VerbState>> {
        let _ = view;
        None
    }

    /// Keep the verb state ([`Pack::verb_state_build`]) in step with one change to the doc,
    /// whatever made it: a kit or pack verb, a host's edit, an outside save, an undo or redo,
    /// a restore or revert. Called on every change, in the change's commit, before any verb
    /// runs again. `applied` is the change's applied ops, root by root (a list op with its
    /// positions before and after, a map op with its value before and after); `view` is the
    /// doc after it (a text root's change is read from it). Returns `true` when `state` is the
    /// state [`Pack::verb_state_build`] would give from `view`; `false` drops it, and the kit
    /// builds it again when a verb next runs. Test builds compare the kept state with a fresh
    /// build after every change and at every hand-out, and fail loudly on a difference.
    ///
    /// Default `false`: the state is built again after each change.
    fn verb_state_change(
        &self,
        state: &mut (dyn VerbState + 'static),
        applied: &[crate::elem::Applied],
        view: &View,
    ) -> bool {
        let _ = (state, applied, view);
        false
    }

    /// What a keyed watch's change frame carries from the pack beyond the change's ops
    /// (`client/WIRE.md`, "The keyed watch"): its `derived` values (a slide's title, an inherited
    /// box: what a view needs without running pack code) and the `render_hashes` of the
    /// containers whose hash changed. `ops` is the change, root by root; `state` the doc's keyed
    /// state after the commit that made it (for one of several changes made by one call, an
    /// undo of a group, the state after the last); `view`, for a pack with a view schema, the
    /// view index after it and the one kept before it
    /// ([`crate::view_index::ViewChange`]), built through the index the kit keeps by (carrier
    /// id, rev) ([`super::Session::view_index`]), so what the derive hook read from the
    /// carrier is there with no second build. Called once per change, only while a watch is
    /// open. When the pack gives no `render_hashes`, the kit's are the containers whose render
    /// hash changed in `view` ([`crate::view_index::ViewChange::render_hashes`]). Default: none
    /// (`null` on the wire, but the kit's render hashes).
    fn watch_extras(
        &self,
        ops: &[crate::elem::Applied],
        state: &crate::elem::KeyedState,
        view: Option<crate::view_index::ViewChange<'_>>,
    ) -> super::WatchExtras {
        let _ = (ops, state, view);
        super::WatchExtras::default()
    }

    /// A file has no keys: its new bytes (an outside save, a text edit verb's result,
    /// an agent's `write --base` body), as edits against `synced`, the doc as the file last
    /// held it. A structured doc's session applies them **by key** to the live doc (CSV
    /// seam design §7.7), so changes the file never got are kept. For a table, `synced`'s
    /// key counters are the live doc's, so keys minted here are new to the live doc too.
    ///
    /// Default: [`default_reconcile`] (decode, then a minimal text diff per text root, or
    /// for a [`crate::table`] schema the aligner [`table::reconcile()`]). A file that doesn't
    /// decode is [`Declined`] (it opens the old way until it decodes again).
    fn reconcile(&self, synced: &View, bytes: &[u8], codec: &[u8]) -> Result<Reconciled, Declined> {
        let _ = codec;
        default_reconcile(self, synced, bytes)
    }

    /// Which element each rendered line of `content` belongs to (`L42` addressing, blame
    /// lines, views), or `None` when lines are the text itself (a text pack). Default: for a
    /// [`crate::table`] schema, [`Table::line_map`]; a codec that writes lines of its own in
    /// front (a `sep=;` preamble) overrides it to account for them.
    fn line_map(&self, content: &View, codec: &[u8]) -> Option<LineMap> {
        let _ = codec;
        if table::is_table(&self.schema()) {
            content.table().ok().map(|t| t.line_map())
        } else {
            None
        }
    }
}

/// [`Pack::reconcile`]'s default: decode `bytes`, then
/// - every root text: each text root's edit from `synced`'s text, whole lines first
///   ([`crate::text::lines::edit_hunks`]);
/// - a [`crate::table`] schema: the decoded table's rendering, aligned to `synced`'s table
///   by [`table::reconcile()`] (rows and columns matched by content, keys kept, moves found);
/// - anything else: declined `cannot_reconcile` (the pack overrides it).
pub fn default_reconcile<P: Pack + ?Sized>(
    pack: &P,
    synced: &View,
    bytes: &[u8],
) -> Result<Reconciled, Declined> {
    let content = pack.decode(bytes)?;
    let schema = pack.schema();
    let cant = |why: String| Declined {
        code: "cannot_reconcile",
        message: why,
    };
    if schema.roots.iter().all(|(_, k)| *k == Kind::Text) {
        let mut edits = Vec::new();
        for (root, n) in &content.roots {
            let new = n
                .as_text()
                .ok_or_else(|| cant(format!("root {root:?} isn't text")))?;
            let old = synced.text(root).unwrap_or_default();
            // whole lines first, as every edit given as a new text (v0.9.7.1's one pairing
            // rule): a character-trimmed diff pairs lines that start alike one line off
            let hunks = crate::text::lines::edit_hunks(old, new);
            if !hunks.is_empty() {
                edits.push(Edit::Text {
                    root: root.clone(),
                    hunks,
                });
            }
        }
        return Ok(Reconciled {
            edits,
            codec: content.codec,
            exact_rows: Vec::new(),
            cols_exact: false,
        });
    }
    if !table::is_table(&schema) {
        return Err(cant(format!(
            "pack {:?} has structured roots and no reconcile",
            schema.name
        )));
    }
    let old = synced.table().map_err(|e| cant(e.to_string()))?;
    let new = Table::from_keyed(View::of_content(&content.roots).keyed())
        .map_err(|e| cant(e.to_string()))?;
    let records = new.render();
    let r = table::reconcile(&old, &records).map_err(|e| cant(e.to_string()))?;
    Ok(Reconciled {
        edits: r.edits,
        codec: content.codec,
        exact_rows: r.exact_rows,
        cols_exact: r.cols_exact,
    })
}

/// A pack for a plain-text file type in a few lines: one text root named
/// [`TextFilePack::ROOT`], decoded by [`crate::codec::text_file`] (BOM and CRLF restored on
/// encode; NUL, invalid UTF-8, mixed line endings and files over `limit` declined).
///
/// ```
/// use k2_mpc_base::session::TextFilePack;
/// pub const MARKDOWN: TextFilePack = TextFilePack::new("markdown", 4 << 20);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextFilePack {
    name: &'static str,
    limit: usize,
    tokens: crate::text::lines::Tokens,
}

const CODEC_V1: u8 = 1;

impl TextFilePack {
    /// The single text root.
    pub const ROOT: &'static str = "source";

    /// `name` is the schema name (stamped on stored docs); `limit` the largest file, in bytes.
    pub const fn new(name: &'static str, limit: usize) -> Self {
        Self {
            name,
            limit,
            tokens: crate::text::lines::Tokens::Words,
        }
    }

    /// The same pack with another token rule for D1 (v0.9.7; words by default). The undo
    /// rules' worked cases on single letters use [`crate::text::lines::Tokens::Chars`], under
    /// which D1 is the identity, as the oracle's do.
    pub const fn with_tokens(mut self, tokens: crate::text::lines::Tokens) -> Self {
        self.tokens = tokens;
        self
    }

    pub fn limit(&self) -> usize {
        self.limit
    }

    fn codec_bytes(f: FileFormat) -> Vec<u8> {
        vec![
            CODEC_V1,
            u8::from(f.bom),
            match f.eol {
                Eol::Lf => 0,
                Eol::Crlf => 1,
            },
        ]
    }

    fn format(codec: &[u8]) -> Result<FileFormat, PackError> {
        match codec {
            [CODEC_V1, bom @ (0 | 1), eol @ (0 | 1)] => Ok(FileFormat {
                bom: *bom == 1,
                eol: if *eol == 1 { Eol::Crlf } else { Eol::Lf },
            }),
            _ => Err(PackError(format!("bad text-file codec settings {codec:?}"))),
        }
    }
}

impl Pack for TextFilePack {
    fn tokens(&self) -> crate::text::lines::Tokens {
        self.tokens
    }

    fn schema(&self) -> Schema {
        Schema {
            name: self.name,
            roots: &[(Self::ROOT, Kind::Text)],
        }
    }

    fn decode(&self, bytes: &[u8]) -> Result<PackContent, Declined> {
        let (text, f) = text_file::decode(bytes, self.limit).map_err(|d| Declined {
            code: d.code(),
            message: d.to_string(),
        })?;
        Ok(PackContent {
            roots: vec![(Self::ROOT.to_owned(), Neutral::Text(text))],
            codec: Self::codec_bytes(f),
        })
    }

    fn encode(&self, content: &[(String, Neutral)], codec: &[u8]) -> Result<Vec<u8>, PackError> {
        let f = Self::format(codec)?;
        match content {
            [(root, Neutral::Text(t))] if root == Self::ROOT => Ok(text_file::encode(t, f)),
            _ => Err(PackError(format!(
                "text-file pack expects one root {:?}",
                Self::ROOT
            ))),
        }
    }

    fn normalize_input<'a>(&self, _root: &str, text: &'a str) -> Cow<'a, str> {
        text_file::normalize_input(text)
    }

    fn writes_bom(&self, root: &str, codec: &[u8]) -> bool {
        root == Self::ROOT && Self::format(codec).is_ok_and(|f| f.bom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: TextFilePack = TextFilePack::new("markdown", 64);

    #[test]
    fn round_trips_and_declines() {
        let b = b"\xEF\xBB\xBF# T\r\nx\r\n";
        let c = P.decode(b).unwrap();
        assert_eq!(
            c.roots,
            vec![("source".into(), Neutral::Text("# T\nx\n".into()))]
        );
        assert_eq!(P.encode(&c.roots, &c.codec).unwrap(), b);
        assert_eq!(P.decode(&[b'a'; 65]).unwrap_err().code, "too_large");
        assert_eq!(
            P.decode(b"a\r\nb\n").unwrap_err().code,
            "mixed_line_endings"
        );
        assert_eq!(P.decode(b"\xff").unwrap_err().code, "not_utf8");
        assert!(P.encode(&c.roots, &[9]).is_err(), "bad codec is loud");
        assert!(P.encode(&[], &c.codec).is_err());
        assert_eq!(P.normalize_input("source", "a\r\nb\rc"), "a\nb\nc");
        assert_eq!(P.schema().roots, &[("source", Kind::Text)]);
    }
}
