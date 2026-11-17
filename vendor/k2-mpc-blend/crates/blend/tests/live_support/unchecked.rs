//! The live pack with the kit's edit check off ([`NoEditCheck`]): `BlendLive` in every hook but
//! `checks_edits`, which is `false`, so the kit takes a change the pack refuses at the call (a
//! list move, a collection's field). For the view rows whose subject is the pack's view derive
//! of such a change, not who may make it: with `BlendLive` itself no writer makes it live
//! (`tests/edit_hook.rs`).

use std::sync::Arc;

use k2_mpc_base::doc::{Edit, Neutral};
use k2_mpc_base::elem::{Applied, Key};
use k2_mpc_base::errors::ErrorEntry;
use k2_mpc_base::placement::{Placement, Space};
use k2_mpc_base::schema::Schema;
use k2_mpc_base::session::{
    Declined, Pack, PackContent, PackError, PackRefusal, Pairing, Reconciled, Session, SpliceError,
    VerbState, View,
};
use k2_mpc_base::tools::PackVerb;
use k2_mpc_base::view_index::{ViewIndex, ViewSchema, ViewSource, ViewTouched};
use k2_mpc_blend::live::BlendLive;

use super::{fx, key, Fx};

/// `BlendLive`, its edit check off.
#[derive(Debug, Default)]
pub struct NoEditCheck(pub BlendLive);

impl Pack for NoEditCheck {
    fn schema(&self) -> Schema {
        self.0.schema()
    }
    fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
        self.0.decode(b)
    }
    fn encode(&self, c: &[(String, Neutral)], codec: &[u8]) -> Result<Vec<u8>, PackError> {
        self.0.encode(c, codec)
    }
    fn wants_carrier(&self) -> bool {
        self.0.wants_carrier()
    }
    fn wants_pairing(&self) -> bool {
        self.0.wants_pairing()
    }
    /// The one hook that differs: the kit never asks.
    fn checks_edits(&self) -> bool {
        false
    }
    fn check_edit(&self, view: &View, edits: &[Edit]) -> Result<(), PackRefusal> {
        self.0.check_edit(view, edits)
    }
    fn encode_from_paired(
        &self,
        carrier: &[u8],
        pairing: &Pairing,
        c: &[(String, Neutral)],
        codec: &[u8],
    ) -> Result<Vec<u8>, SpliceError> {
        self.0.encode_from_paired(carrier, pairing, c, codec)
    }
    fn view_schema(&self) -> Option<ViewSchema> {
        self.0.view_schema()
    }
    fn view_derive(&self, source: ViewSource<'_>, index: &mut ViewIndex) {
        self.0.view_derive(source, index);
    }
    fn view_derive_change(
        &self,
        source: ViewSource<'_>,
        index: &mut ViewIndex,
        touched: &ViewTouched,
    ) -> bool {
        self.0.view_derive_change(source, index, touched)
    }
    fn view_placement(&self, source: ViewSource<'_>, root: &str, key: &Key) -> Option<Placement> {
        self.0.view_placement(source, root, key)
    }
    fn view_space(&self, source: ViewSource<'_>) -> Option<Space> {
        self.0.view_space(source)
    }
    fn address_of(&self, root: &str, key: &Key) -> String {
        self.0.address_of(root, key)
    }
    fn key_of(&self, root: &str, address: &str) -> Option<Key> {
        self.0.key_of(root, address)
    }
    fn reconcile(&self, synced: &View, bytes: &[u8], codec: &[u8]) -> Result<Reconciled, Declined> {
        self.0.reconcile(synced, bytes, codec)
    }
    fn verbs(&self) -> &'static [PackVerb] {
        self.0.verbs()
    }
    fn errors(&self) -> &'static [ErrorEntry] {
        self.0.errors()
    }
    fn verb_state_build(&self, view: &View) -> Option<Box<dyn VerbState>> {
        self.0.verb_state_build(view)
    }
    fn verb_state_change(
        &self,
        state: &mut (dyn VerbState + 'static),
        applied: &[Applied],
        view: &View,
    ) -> bool {
        self.0.verb_state_change(state, applied, view)
    }
    fn composite(&self) -> &'static [(&'static str, &'static str)] {
        self.0.composite()
    }
    fn check_merged_view(&self, view: &View) -> Result<(), Declined> {
        self.0.check_merged_view(view)
    }
}

/// `bytes` opened live with [`NoEditCheck`].
pub fn open_unchecked(tag: &str, bytes: &[u8]) -> (Fx, Arc<Session>) {
    let f = fx(tag, |_| {});
    f.write(bytes);
    let s = f
        .mgr
        .open_file(Arc::new(NoEditCheck::default()), &key(), &f.path(), &f.ws)
        .expect("the scene opens live");
    (f, s)
}
