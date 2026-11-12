//! The view index and element addresses on a live doc (rendering-data contract gaps 1–3,
//! [`crate::view_index`]). A child of `live`, so it works on the session's own state.
//!
//! - [`Session::view_index`]: the doc's view index at its current change, from the live view's
//!   keyed state (no copy of the doc), the pack's view schema and, for a pack that keeps one,
//!   the doc's carrier (K1, [`ViewCarrier`]). **Kept** by (carrier id, rev) and reused until
//!   either changes: N reads of an unchanged doc build it once, and the carrier is decoded once
//!   per carrier, never per read or per change.
//! - [`Session::address_of`] and [`Session::resolve_address`]: the kit's edge between the live
//!   store's keys and the addresses everything on the wire carries. The presence setters and
//!   claims by address ([`Session::set_focus_at`], [`Session::set_selection_at`],
//!   [`Session::set_range_at`], [`Session::claim_elems_at`]) resolve through it.

use super::*;
use crate::elem::{ElemState, Key};
use crate::session::awareness::{ClaimInfo, CLAIM_KEYS_MAX};
use crate::view_index::{key_candidates, AddressError, ViewCarrier, ViewIndex, ViewSchema};

/// The doc's view index as last built ([`Session::view_index`]), with what it was built from:
/// the carrier's id (none without one) and the doc's version.
pub(super) struct ViewCache {
    carrier: Option<[u8; 32]>,
    rev: u64,
    index: Arc<ViewIndex>,
}

/// The planted fault [`crate::differential::ViewMutant::DeriveIgnoresCarrier`].
fn planted_derive_ignores_carrier() -> bool {
    #[cfg(any(test, feature = "differential"))]
    if crate::differential::view_mutant()
        == Some(crate::differential::ViewMutant::DeriveIgnoresCarrier)
    {
        return true;
    }
    false
}

/// The planted fault [`crate::differential::ViewMutant::DeriveCacheStale`].
fn planted_derive_cache_stale() -> bool {
    #[cfg(any(test, feature = "differential"))]
    if crate::differential::view_mutant() == Some(crate::differential::ViewMutant::DeriveCacheStale)
    {
        return true;
    }
    false
}

impl Session {
    /// The pack's view schema ([`Pack::view_schema`]), for a watch's snapshot.
    pub fn view_schema(&self) -> Option<ViewSchema> {
        self.pack.view_schema()
    }

    /// The doc's view index at its current change (contract Item 1, gap 3): `tier` live, `rev`
    /// the doc's version, built from its keyed state through the pack's view schema and derive
    /// hook ([`ViewIndex::live_carried`]). Needs read. `None` when the pack has no view schema
    /// (or the doc isn't keyed: a text doc or a table). A schema that doesn't fit the doc's
    /// roots ([`ViewSchema::check`]) is the pack's bug: [`SessionError::Pack`].
    ///
    /// - **The carrier** (K1): for a pack that keeps one ([`Pack::wants_carrier`]), the hook
    ///   reads the doc's carrier, when the kit holds it, for what the live store doesn't model
    ///   ([`crate::view_index::ViewSource::Live`]); its decode runs once per carrier
    ///   ([`ViewCarrier::decoded`]).
    /// - **Kept:** the index is kept by (carrier id, rev) and a read reuses it until either
    ///   changes: a change of the doc (a new rev) builds it again from the state and the kept
    ///   decode, and a new carrier (the kit's write, an outside save, a reload) decodes again.
    ///
    /// For a watch: [`ViewIndex::snapshot`] is the snapshot's part (with
    /// [`Session::view_schema`]), and [`ViewIndex::units_page`] reads a container's units in
    /// pages.
    pub fn view_index(&self, conn: ConnId) -> Result<Option<ViewIndex>> {
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        self.admit(st, conn, Need::Read, Action::Read, now)?;
        Ok(self.kept_view_index(st)?.map(|ix| (*ix).clone()))
    }

    /// The doc's view index at its current change, kept by (carrier id, rev)
    /// ([`Session::view_index`]'s, without its rights check): the one place the live tier
    /// builds it, for a read, a watch's snapshot and a watch's change frames alike.
    pub(super) fn kept_view_index(&self, st: &mut State) -> Result<Option<Arc<ViewIndex>>> {
        let Some(vs) = self.pack.view_schema() else {
            return Ok(None);
        };
        if !self.keeps_view() {
            return Ok(None);
        }
        vs.check(&self.schema, self.pack.composite())
            .map_err(|m| SessionError::Pack(PackError(m)))?;
        let carrier = self.view_carrier(st);
        let id = carrier.as_ref().map(|c| *c.id());
        let rev = st.version;
        if let Some(c) = &st.view_cache {
            if c.carrier == id && (c.rev == rev || planted_derive_cache_stale()) {
                return Ok(Some(c.index.clone()));
            }
        }
        let state = self.live_state(st)?;
        let Some(ix) = ViewIndex::live_carried(&*self.pack, &state, rev, carrier.as_deref()) else {
            return Ok(None);
        };
        let ix = Arc::new(ix);
        st.view_cache = Some(ViewCache {
            carrier: id,
            rev,
            index: ix.clone(),
        });
        Ok(Some(ix))
    }

    /// The view index kept before the change being sent to the watches, if the kit holds one
    /// (the change before it's, while a watch is open: each change frame keeps it current).
    pub(super) fn view_index_kept_before(st: &State) -> Option<Arc<ViewIndex>> {
        st.view_cache.as_ref().map(|c| c.index.clone())
    }

    /// The carrier the view index's derive hook reads: the session's (`st.carrier`) for a
    /// pack that keeps one, with the pack's decode of it kept until it changes. `None` for a
    /// pack that keeps none or before the kit holds one.
    fn view_carrier(&self, st: &mut State) -> Option<Arc<ViewCarrier>> {
        if !self.pack.wants_carrier() || planted_derive_ignores_carrier() {
            return None;
        }
        let bytes = st.carrier.clone()?;
        match &st.view_carrier {
            Some(vc) if vc.holds(&bytes) => Some(vc.clone()),
            _ => {
                let vc = Arc::new(ViewCarrier::new(bytes, &st.hash_key));
                st.view_carrier = Some(vc.clone());
                Some(vc)
            }
        }
    }

    /// The address of element `key` of `root` ([`Pack::address_of`], contract gap 2): how the
    /// wire names it. Reads no doc.
    pub fn address_of(&self, root: &str, key: &Key) -> String {
        self.pack.address_of(root, key)
    }

    /// The element `address` names, as `(list root, key)` (contract gap 2): the one list root
    /// whose [`Pack::key_of`] takes it and whose element with that key this doc knows (live or
    /// deleted; a setter then asks for a live one). Needs read. Refused
    /// [`SessionError::BadPosition`] when no element has it, or elements of several roots do
    /// (a pack whose addresses are its keys, with two list roots that share a key); a pack
    /// whose mapping isn't one to one is [`SessionError::Pack`].
    pub fn resolve_address(&self, conn: ConnId, address: &str) -> Result<(String, Key)> {
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        self.admit(st, conn, Need::Read, Action::View, now)?;
        let candidates = key_candidates(&*self.pack, address).map_err(|e| match e {
            AddressError::NotInverse { .. } => SessionError::Pack(PackError(e.to_string())),
            _ => SessionError::BadPosition(e.to_string()),
        })?;
        let mut known: Vec<(&str, Key)> = Vec::new();
        for (root, key) in candidates {
            if !matches!(st.doc.elem(root, &key)?, ElemState::Unknown) {
                known.push((root, key));
            }
        }
        match known.len() {
            1 => {
                let (root, key) = known.pop().expect("one");
                Ok((root.to_owned(), key))
            }
            0 => Err(SessionError::BadPosition(format!(
                "no element has the address {address:?}"
            ))),
            _ => Err(SessionError::BadPosition(format!(
                "the address {address:?} names an element of each of {:?}",
                known.iter().map(|(r, _)| *r).collect::<Vec<_>>()
            ))),
        }
    }

    /// [`Session::claim_elems`] by address (contract gap 2): every address names a live element
    /// of one list root ([`Session::resolve_address`]).
    pub fn claim_elems_at(
        &self,
        conn: ConnId,
        addresses: &[String],
        ttl_ms: Option<u64>,
        note: Option<String>,
        hold: bool,
    ) -> Result<ClaimInfo> {
        if addresses.len() > CLAIM_KEYS_MAX {
            return Err(SessionError::BadClaim(format!(
                "{} elements in one claim; at most {CLAIM_KEYS_MAX}",
                addresses.len()
            )));
        }
        let mut root: Option<String> = None;
        let mut keys = Vec::with_capacity(addresses.len());
        for a in addresses {
            let (r, k) = self.resolve_address(conn, a)?;
            match &root {
                Some(x) if *x != r => {
                    return Err(SessionError::BadClaim(format!(
                        "{a} is an element of {r:?}, not of {x:?}: a claim is in one root"
                    )))
                }
                Some(_) => {}
                None => root = Some(r),
            }
            keys.push(k);
        }
        let Some(root) = root else {
            return Err(SessionError::BadClaim(
                "a claim on elements names at least one".into(),
            ));
        };
        self.claim_elems(conn, &root, &keys, ttl_ms, note, hold)
    }
}
