//! The view index and element addresses on a live doc
//! ([`crate::view_index`]). A child of `live`, so it works on the session's own state.
//!
//! - [`Session::view_index`]: the doc's view index at its current change, from the live view's
//!   keyed state (no copy of the doc), the pack's view schema and, for a pack that keeps one,
//!   the doc's carrier ([`ViewCarrier`]). **Kept** by (carrier id, rev) and reused until
//!   either changes: N reads of an unchanged doc build it once, and the carrier is decoded once
//!   per carrier, never per read or per change. **Kept through changes** when the pack derives
//!   by change ([`Pack::view_derive_change`]): each change's applied stream says which units
//!   and containers it touched (`ViewDirty`), and the next read builds those again and keeps
//!   the rest (`ViewIndex::live_update`); test builds check the result against a whole build.
//! - [`Session::view_unit`], [`Session::view_container`] and [`Session::view_derived`]: one
//!   unit, one container, or one container's units' derived values, from the kept index,
//!   without a page.
//! - [`Session::address_of`] and [`Session::resolve_address`]: the kit's edge between the live
//!   store's keys and the addresses everything on the wire carries. The presence setters and
//!   claims by address ([`Session::set_focus_at`], [`Session::set_selection_at`],
//!   [`Session::set_range_at`], [`Session::claim_elems_at`]) resolve through it.

use super::*;
use crate::elem::{ElemState, Key};
use crate::session::awareness::{ClaimInfo, CLAIM_KEYS_MAX};
use crate::view_index::{
    key_candidates, AddressError, Container, Unit, UnitsDerived, ViewCarrier, ViewDirty, ViewIndex,
    ViewSchema,
};

/// The doc's view index as last built ([`Session::view_index`]), with what it was built from:
/// the carrier's id (none without one) and the doc's version, what the changes since touched,
/// and (once a read by address asks) where each unit is.
pub(super) struct ViewCache {
    carrier: Option<[u8; 32]>,
    /// The carrier's pairing the index was built with (a new pairing on the same carrier is a
    /// whole build: the hooks find units' file facts through it).
    pairing: Option<Arc<crate::session::Pairing>>,
    rev: u64,
    index: Arc<ViewIndex>,
    dirty: ViewDirty,
    at: std::sync::OnceLock<HashMap<String, usize>>,
}

/// Whether the index kept through a change is checked against a whole build: test builds,
/// unless switched off on this thread (`differential::set_view_checks`), as the counts do.
fn update_checks_on() -> bool {
    #[cfg(any(test, feature = "differential"))]
    {
        crate::differential::view_checks()
    }
    #[cfg(not(any(test, feature = "differential")))]
    {
        false
    }
}

/// The planted fault [`crate::differential::ViewMutant::DeriveIgnoresCarrier`].
fn planted_derive_ignores_carrier() -> bool {
    plant_on!(
        crate::differential::view_mutant()
            == Some(crate::differential::ViewMutant::DeriveIgnoresCarrier)
    )
}

/// The planted fault [`crate::differential::ViewMutant::DeriveCacheStale`].
fn planted_derive_cache_stale() -> bool {
    plant_on!(
        crate::differential::view_mutant()
            == Some(crate::differential::ViewMutant::DeriveCacheStale)
    )
}

impl Session {
    /// The pack's view schema ([`Pack::view_schema`]), for a watch's snapshot.
    pub fn view_schema(&self) -> Option<ViewSchema> {
        self.pack.view_schema()
    }

    /// The doc's view index at its current change: `tier` live, `rev`
    /// the doc's version, built from its keyed state through the pack's view schema and derive
    /// hook ([`ViewIndex::live_carried`]). Needs read. `None` when the pack has no view schema
    /// (or the doc isn't keyed: a text doc or a table). A schema that doesn't fit the doc's
    /// roots ([`ViewSchema::check`]) is the pack's bug: [`SessionError::Pack`].
    ///
    /// - **The carrier**: for a pack that keeps one ([`Pack::wants_carrier`]), the hook
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
        let mut st = self.lock_for(conn)?;
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
        let pairing = carrier.as_ref().and_then(|c| c.pairing_arc().cloned());
        let same_pairing = |c: &ViewCache| match (&c.pairing, &pairing) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        };
        let rev = st.version;
        if let Some(c) = &st.view_cache {
            if c.carrier == id && same_pairing(c) && (c.rev == rev || planted_derive_cache_stale())
            {
                return Ok(Some(c.index.clone()));
            }
        }
        let state = self.live_state(st)?;
        // kept through the changes since, when the pack derives by change and the kit took
        // every one of them, in order, on the same carrier
        let mut kept = None;
        if let Some(c) = &st.view_cache {
            let fits = c.carrier == id
                && same_pairing(c)
                && !c.dirty.wide
                && c.dirty.upto == rev
                && c.rev < rev;
            if fits && st.view_by_change != Some(false) {
                kept = ViewIndex::live_update(
                    &*self.pack,
                    &c.index,
                    &state,
                    rev,
                    carrier.as_deref(),
                    &c.dirty,
                );
                st.view_by_change = Some(kept.is_some());
            }
        }
        let ix = match kept {
            Some(ix) => {
                if update_checks_on() {
                    self.check_kept_index(&st.key.path, &ix, &state, rev, carrier.as_deref());
                }
                ix
            }
            None => {
                let Some(ix) =
                    ViewIndex::live_carried(&*self.pack, &state, rev, carrier.as_deref())
                else {
                    return Ok(None);
                };
                ix
            }
        };
        let ix = Arc::new(ix);
        st.view_cache = Some(ViewCache {
            carrier: id,
            pairing,
            rev,
            index: ix.clone(),
            dirty: ViewDirty::at(rev),
            at: std::sync::OnceLock::new(),
        });
        Ok(Some(ix))
    }

    /// Test builds: the index kept through a change is the one a whole build gives, or the
    /// pack's incremental derive (or the kit's update) is wrong: refused loudly, naming the
    /// first unit or container that differs.
    fn check_kept_index(
        &self,
        path: &str,
        kept: &ViewIndex,
        state: &crate::elem::KeyedState,
        rev: u64,
        carrier: Option<&ViewCarrier>,
    ) {
        let Some(whole) = ViewIndex::live_carried(&*self.pack, state, rev, carrier) else {
            return;
        };
        if *kept == whole {
            return;
        }
        let unit = kept
            .units
            .iter()
            .zip(&whole.units)
            .find(|(a, b)| a != b)
            .map(|(a, b)| format!("unit {}: kept {a:?}, whole {b:?}", a.key));
        let container = kept
            .containers
            .iter()
            .zip(&whole.containers)
            .find(|(a, b)| a != b)
            .map(|(a, b)| format!("container {}: kept {a:?}, whole {b:?}", a.key));
        panic!(
            "{path}: the view index kept through a change at rev {rev} isn't a whole build's \
             ({} units kept, {} whole; {} containers kept, {} whole): {}",
            kept.units.len(),
            whole.units.len(),
            kept.containers.len(),
            whole.containers.len(),
            unit.or(container)
                .unwrap_or_else(|| "the page, fonts or layouts".to_owned())
        );
    }

    /// After a change that took the doc from `before` to `after` (the live view kept): the
    /// kept view index learns what its applied `stream` touched.
    pub(super) fn view_index_after(
        &self,
        st: &mut State,
        before: Seq,
        after: Seq,
        stream: &[crate::elem::Applied],
    ) {
        if st.view_by_change == Some(false) {
            return;
        }
        let Some(c) = st.view_cache.as_mut() else {
            return;
        };
        if let Some(vs) = self.pack.view_schema() {
            c.dirty.take(&*self.pack, &vs, before, after, stream);
        }
    }

    /// One unit of the doc's view index (a nested one too), by its address, at the doc's
    /// current change: its fields and its derived values ([`Unit::derived`]), without reading
    /// a page. Needs read. `None` when the index holds no such unit, or the pack has no view
    /// schema. From the kept index ([`Session::view_index`]), so a read after a change costs
    /// what the change touched.
    pub fn view_unit(&self, conn: ConnId, address: &str) -> Result<Option<Unit>> {
        let now = self.now();
        let mut st = self.lock_for(conn)?;
        let st = &mut *st;
        self.admit(st, conn, Need::Read, Action::Read, now)?;
        let Some(ix) = self.kept_view_index(st)? else {
            return Ok(None);
        };
        let Some(c) = st.view_cache.as_ref() else {
            return Ok(ix.unit(address).cloned());
        };
        let at = c.at.get_or_init(|| {
            ix.units
                .iter()
                .enumerate()
                .map(|(i, u)| (u.key.clone(), i))
                .collect()
        });
        Ok(at.get(address).map(|&i| ix.units[i].clone()))
    }

    /// One container of the doc's view index, by its address, at its current change (its
    /// fields, `unit_count` among them). Needs read. `None` when there is none.
    pub fn view_container(&self, conn: ConnId, address: &str) -> Result<Option<Container>> {
        let now = self.now();
        let mut st = self.lock_for(conn)?;
        let st = &mut *st;
        self.admit(st, conn, Need::Read, Action::Read, now)?;
        Ok(self
            .kept_view_index(st)?
            .and_then(|ix| ix.container(address).cloned()))
    }

    /// The derived values of every unit of one container, by unit address, back to front
    /// ([`ViewIndex::derived_of`]), units with none left out. Needs read. `None` when the index
    /// holds no such container.
    pub fn view_derived(&self, conn: ConnId, container: &str) -> Result<Option<UnitsDerived>> {
        let now = self.now();
        let mut st = self.lock_for(conn)?;
        let st = &mut *st;
        self.admit(st, conn, Need::Read, Action::Read, now)?;
        Ok(self.kept_view_index(st)?.and_then(|ix| {
            ix.derived_of(container).map(|d| {
                d.into_iter()
                    .map(|(u, v)| (u.to_owned(), v.to_vec()))
                    .collect()
            })
        }))
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
        // the carrier's pairing, for a pack that pairs: the hooks find units by key through it
        let pairing = match self.pack.wants_pairing() {
            true => st.pairing.as_ref().map(|(_, p)| p.clone()),
            false => None,
        };
        match &st.view_carrier {
            Some(vc) if vc.holds(&bytes) && vc.has_pairing(pairing.as_ref()) => Some(vc.clone()),
            Some(vc) if vc.holds(&bytes) => {
                // a new pairing on the same carrier: its decode is kept
                let vc = Arc::new(vc.with_pairing(pairing));
                st.view_carrier = Some(vc.clone());
                Some(vc)
            }
            _ => {
                let vc = Arc::new(ViewCarrier::new(bytes, &st.hash_key).with_pairing(pairing));
                st.view_carrier = Some(vc.clone());
                Some(vc)
            }
        }
    }

    /// The address of element `key` of `root` ([`Pack::address_of`]): how the
    /// wire names it. Reads no doc.
    pub fn address_of(&self, root: &str, key: &Key) -> String {
        self.pack.address_of(root, key)
    }

    /// The element `address` names, as `(list root, key)`: the one list root
    /// whose [`Pack::key_of`] takes it and whose element with that key this doc knows (live or
    /// deleted; a setter then asks for a live one). Needs read. Refused
    /// [`SessionError::BadPosition`] when no element has it, or elements of several roots do
    /// (a pack whose addresses are its keys, with two list roots that share a key); a pack
    /// whose mapping isn't one to one is [`SessionError::Pack`].
    pub fn resolve_address(&self, conn: ConnId, address: &str) -> Result<(String, Key)> {
        let now = self.now();
        let mut st = self.lock_for(conn)?;
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

    /// [`Session::claim_elems`] by address: every address names a live element
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
