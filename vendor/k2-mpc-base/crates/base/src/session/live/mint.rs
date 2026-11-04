//! Tagged keys in sessions ([`crate::mint`]): the writer-tag rule on a pack's declared roots
//! ([`Pack::tagged_keys`]) and minting a key for a connection.
//!
//! Writer tags come only from the client IDs a session mints (random per connection,
//! `mint_client_id`); a host editing a [`crate::Doc`] directly with a client it chose must not
//! mint tagged keys from it.

use super::*;
use crate::elem::{ElemState, Key, KeyedRoot, ListOp};
use crate::mint::{next_for, retired_mark, MintError, TaggedKey, WriterTag};

impl Session {
    /// The writer tag of `conn`: its client ID in base 36 ([`WriterTag`]). Keys it inserts
    /// into a root its pack declares tagged ([`Pack::tagged_keys`]) must carry it.
    pub fn writer_tag(&self, conn: ConnId) -> Result<WriterTag> {
        let st = self.lock()?;
        let i = Self::member(&st, conn)?;
        writer_tag_of(st.members[i].client)
    }

    /// A key `<prefix><n>.<tag>` for `conn` to insert into `root`, a MovableList root its pack
    /// declares tagged with `prefix` ([`Pack::tagged_keys`]): `n` one past the highest of its
    /// own (`conn`'s tag and `prefix`) among every element the list knows, live or deleted
    /// ([`crate::mint::next_for`]). It writes nothing: two connections never mint one key, so
    /// minting needs no lock beyond the read. Needs the connection only (a read).
    ///
    /// Refusals: [`SessionError::BadEdit`] when the root or prefix isn't declared;
    /// [`SessionError::KeySpaceFull`] (exit 2) when no such key fits a list key's 24 bytes.
    pub fn mint_key(&self, conn: ConnId, root: &str, prefix: &str) -> Result<Key> {
        let st = self.lock()?;
        let i = Self::member(&st, conn)?;
        let tag = writer_tag_of(st.members[i].client)?;
        let declared = self
            .pack
            .tagged_keys()
            .iter()
            .find(|(r, _)| *r == root)
            .map(|(_, p)| *p)
            .ok_or_else(|| SessionError::BadEdit(format!("{root} has no tagged keys")))?;
        if !declared.contains(&prefix) {
            return Err(SessionError::BadEdit(format!(
                "{root} has no tagged key prefix {prefix:?}"
            )));
        }
        let state = st.doc.keyed_state()?;
        let places: Vec<&Key> = state
            .roots
            .iter()
            .filter(|(r, _)| r == root)
            .flat_map(|(_, k)| match k {
                KeyedRoot::List { live, hidden, .. } => live
                    .iter()
                    .map(|(k, _)| k)
                    .chain(hidden.keys())
                    .collect::<Vec<_>>(),
                _ => Vec::new(),
            })
            .collect();
        let full = |e: MintError| match e {
            MintError::TooLong { .. } | MintError::Exhausted { .. } => SessionError::KeySpaceFull {
                prefix: prefix.to_owned(),
                tag: tag.to_string(),
            },
            e => SessionError::BadEdit(e.to_string()),
        };
        let retired = match self.pack.mint_marks() {
            Some(m) => match state.roots.iter().find(|(r, _)| r == m) {
                Some((_, KeyedRoot::Map(marks))) => retired_mark(
                    marks.iter().map(|(k, v)| (k.as_str(), v)),
                    root,
                    prefix,
                    tag,
                ),
                _ => 0,
            },
            None => 0,
        };
        let n = next_for(places, retired, prefix, tag).map_err(full)?;
        TaggedKey::new(prefix, n, tag)
            .and_then(|t| t.to_key())
            .map_err(full)
    }

    /// The writer-tag rule ([`Pack::tagged_keys`]): each list insert into a declared root
    /// names a key new to the list that parses as `<prefix><n>.<tag>`, with a declared prefix
    /// and the inserting connection's own tag ([`SessionError::KeyNotOwn`], exit 2). Checked
    /// here, on edits arriving from a connection, never in the engine: stored records, imports
    /// and outside saves don't come through it. A key the list already knows is left to the
    /// engine (a reused key, E804).
    pub(super) fn check_tagged_keys(&self, st: &State, i: usize, edits: &[Edit]) -> Result<()> {
        let decl = self.pack.tagged_keys();
        if decl.is_empty() {
            return Ok(());
        }
        let tag = writer_tag_of(st.members[i].client)?;
        for e in edits {
            let Edit::List { root, ops } = e else {
                continue;
            };
            let Some((_, prefixes)) = decl.iter().find(|(r, _)| *r == root.as_str()) else {
                continue;
            };
            for o in ops {
                let ListOp::Insert { key, .. } = o else {
                    continue;
                };
                if st.doc.elem(root, key)? != ElemState::Unknown {
                    continue;
                }
                let own = TaggedKey::parse(key).is_some_and(|t| {
                    prefixes.contains(&t.prefix()) && (t.tag() == tag || planted_any_tag())
                });
                if !own {
                    return Err(SessionError::KeyNotOwn {
                        root: root.clone(),
                        key: key.to_string(),
                        tag: tag.to_string(),
                    });
                }
            }
        }
        Ok(())
    }
}

/// A member's tag. Its client is one the session minted (never 0, 1 or past 53 bits), so this
/// refuses nothing in practice.
fn writer_tag_of(client: crate::doc::ClientId) -> Result<WriterTag> {
    WriterTag::of(client)
        .ok_or_else(|| SessionError::BadEdit(format!("client {client} has no writer tag")))
}

/// The planted fault [`crate::differential::MintMutant::AnyWriterTag`]: the writer-tag rule
/// takes any writer's tag. Without the `differential` feature there is no hook: `false`.
#[cfg(any(test, feature = "differential"))]
fn planted_any_tag() -> bool {
    crate::differential::mint_mutant() == Some(crate::differential::MintMutant::AnyWriterTag)
}

#[cfg(not(any(test, feature = "differential")))]
fn planted_any_tag() -> bool {
    false
}
