//! A connection's element selection, pointer, viewport, focus, text range, view address and
//! time position. A child of `live`, so it works on the session's own state.
//!
//! All five stay outside the doc's numbering: nothing here takes or moves a version, writes to
//! the store, or marks the doc dirty. Subscribers see them through the roster
//! ([`RosterEvent::Selection`](super::roster::RosterEvent::Selection) and its siblings), one
//! event per connection per flush, the last value. A watcher (past
//! [`Config::participants_max`]) is refused every one of them ([`SessionError::Watching`]).
//!
//! **Focus and range** (wire 3): the focus is the container in
//! view (a slide); a range is a caret or selection inside one element's text field. Setting a
//! range sets the connection's focus to that element's container ([`Pack::container_of`]), so a
//! range is never shown on a container nobody has in view. A range is never moved: it is dropped
//! when its field changes, by anyone, or its element stops being live.
//!
//! **View address and time position**: the view a participant is looking at, by address, and
//! their place in time (a timeline's playhead), on any live doc. People set their own; a pack's
//! verb sets the acting agent's when it says so ([`crate::tools::StructOutput::viewing`],
//! [`crate::tools::StructOutput::time_position`]), and a host sets one for any connection (an
//! agent's, when it renders a still for it).
//!
//! **An agent's verbs move its presence**: when a pack's verb changes
//! elements, the kit sets the acting agent's focus to the container of the last element it
//! touched (the last container touched wins) and its selection to the elements it touched. A
//! person's presence is never moved by the kit: people set their own.

use std::collections::BTreeMap;

use super::*;
use crate::elem::{Applied as ElemApplied, Effect, ElemState, Key};
use crate::session::participant::{
    presence_address, ElemFocus, ElemRange, ElemSelection, Point, Rect, TimePosition,
    ELEM_SELECTION_MAX,
};

/// One connection's values.
#[derive(Default)]
pub(super) struct ElemPresence {
    selection: Option<ElemSelection>,
    pointer: Option<Point>,
    viewport: Option<Rect>,
    focus: Option<ElemFocus>,
    range: Option<ElemRange>,
    /// The cell caret and ranges on a grid (`cells`), on one container.
    pub(super) cells: Option<crate::grid::Cells>,
    /// The view it is looking at, by address.
    view_address: Option<String>,
    /// Its place in time, stamped when set.
    time: Option<TimePosition>,
}

impl ElemPresence {
    /// Set the focus; cells on another container go (a cells value belongs to one container),
    /// and so do they when the focus is cleared.
    pub(super) fn set_focus(&mut self, focus: Option<ElemFocus>) {
        if let Some(c) = &self.cells {
            if focus.as_ref().map(|f| &f.key) != c.container.as_ref() {
                self.cells = None;
            }
        }
        self.focus = focus;
    }

    /// An agent's verb or cells claim moved it ([`Session::verb_cells`]): its focus and cells,
    /// together.
    pub(super) fn cells_moved(&mut self, focus: ElemFocus, cells: Option<crate::grid::Cells>) {
        self.focus = Some(focus);
        self.cells = cells;
    }

    /// An agent's verb's change is at `address` ([`super::view_at::ViewAt::address`]): its view
    /// address moves there, as its focus and selection follow what the verb touched.
    pub(super) fn verb_viewed(&mut self, address: String) {
        self.view_address = Some(address);
    }

    /// An agent's verb changed elements ([`Session::verb_presence`]): its focus moves to `focus`
    /// (when a touched element has a live container), and its selection is replaced by
    /// `selection` (`None`: none of what it touched is live).
    fn verb_touched(&mut self, focus: Option<ElemFocus>, selection: Option<ElemSelection>) {
        if focus.is_some() {
            self.set_focus(focus);
        }
        self.selection = selection;
    }

    /// An agent took a claim ([`Session::claim_elems`]): its focus moves to `focus` (when the
    /// claim has a container), and its selection is replaced by `selection` (when given).
    pub(super) fn claimed(&mut self, focus: Option<ElemFocus>, selection: Option<ElemSelection>) {
        if focus.is_some() {
            self.set_focus(focus);
        }
        if selection.is_some() {
            self.selection = selection;
        }
    }
}

/// Fill a roster row from its connections, most active first.
pub(super) fn fold(conns: &[&Member], entry: &mut RosterEntry) {
    entry.selection = conns.iter().find_map(|m| m.elem.selection.clone());
    entry.pointer = conns.iter().find_map(|m| m.elem.pointer);
    entry.viewport = conns.iter().find_map(|m| m.elem.viewport);
    entry.focus = conns.iter().find_map(|m| m.elem.focus.clone());
    entry.range = conns.iter().find_map(|m| m.elem.range.clone());
    entry.cells = conns.iter().find_map(|m| m.elem.cells.clone());
    entry.view_address = conns.iter().find_map(|m| m.elem.view_address.clone());
    entry.time = conns.iter().find_map(|m| m.elem.time.clone());
    for d in &mut entry.devices {
        if let Some(m) = conns.iter().find(|m| m.client == d.client) {
            d.selection = m.elem.selection.clone();
            d.pointer = m.elem.pointer;
            d.viewport = m.elem.viewport;
            d.focus = m.elem.focus.clone();
            d.range = m.elem.range.clone();
            d.cells = m.elem.cells.clone();
            d.view_address = m.elem.view_address.clone();
            d.time = m.elem.time.clone();
        }
    }
}

/// The planted fault [`crate::differential::ViewMutant::RosterKeysLeak`].
fn planted_roster_keys_leak() -> bool {
    plant_on!(
        crate::differential::view_mutant() == Some(crate::differential::ViewMutant::RosterKeysLeak)
    )
}

/// Elements `keys` of list root `root` by address ([`Pack::address_of`]).
pub(super) fn addresses<P: Pack + ?Sized>(pack: &P, root: &str, keys: &[Key]) -> Vec<String> {
    keys.iter().map(|k| address(pack, root, k)).collect()
}

fn address<P: Pack + ?Sized>(pack: &P, root: &str, key: &Key) -> String {
    if planted_roster_keys_leak() {
        return key.as_str().to_owned();
    }
    pack.address_of(root, key)
}

/// A selection, focus and range with their elements' addresses filled in.
pub(super) fn address_selection<P: Pack + ?Sized>(pack: &P, s: &mut ElemSelection) {
    s.addresses = addresses(pack, &s.root, &s.keys);
}

pub(super) fn address_focus<P: Pack + ?Sized>(pack: &P, f: &mut ElemFocus) {
    f.address = address(pack, &f.root, &f.key);
}

pub(super) fn address_range<P: Pack + ?Sized>(pack: &P, r: &mut ElemRange) {
    r.address = address(pack, &r.root, &r.key);
}

/// A roster row as the kit hands it out: every element its selection, focus, range (its own and
/// each device's) and element claims name, by address too. The roster, its snapshot and its
/// events all read their rows here ([`Session::roster`]), so no key reaches the wire where an
/// address belongs.
pub(super) fn address_entry<P: Pack + ?Sized>(pack: &P, e: &mut RosterEntry) {
    let one =
        |s: &mut Option<ElemSelection>, f: &mut Option<ElemFocus>, r: &mut Option<ElemRange>| {
            if let Some(s) = s {
                address_selection(pack, s);
            }
            if let Some(f) = f {
                address_focus(pack, f);
            }
            if let Some(r) = r {
                address_range(pack, r);
            }
        };
    one(&mut e.selection, &mut e.focus, &mut e.range);
    if let Some(c) = e.cells.as_mut() {
        super::cells::address_cells(pack, c);
    }
    for d in &mut e.devices {
        one(&mut d.selection, &mut d.focus, &mut d.range);
        if let Some(c) = d.cells.as_mut() {
            super::cells::address_cells(pack, c);
        }
    }
    for c in &mut e.claims {
        if let awareness::ClaimSpan::Elems { root, keys } = &c.span {
            c.addresses = addresses(pack, root, keys);
        }
    }
}

/// The composite map root holding the fields of `list`'s elements ([`Pack::composite`]).
fn fields_root(composites: &[(&'static str, &'static str)], list: &str) -> Option<&'static str> {
    composites.iter().find(|(_, l)| *l == list).map(|(m, _)| *m)
}

/// The map key of `field` of element `key` (`<key>\x1f<field>`, [`Pack::composite`]).
fn field_key(key: &Key, field: &str) -> String {
    format!("{key}{}{field}", crate::table::CELL_SEP)
}

/// The elements a change names, in the order of its ops, each once at its last place: a list
/// op's key, and a map op's key mapped to its element through the pack's composite roots
/// ([`elem_of_map_key`](crate::session::pack::elem_of_map_key)). What a pack verb's change moves
/// an agent's presence to ([`Session::verb_presence`]), before it applies.
pub(super) fn touched_in_order(
    composites: &[(&'static str, &'static str)],
    edits: &[Edit],
) -> Vec<(String, Key)> {
    let mut last: BTreeMap<(String, Key), usize> = BTreeMap::new();
    let mut at = 0usize;
    let mut put = |root: &str, key: Key| {
        last.insert((root.to_owned(), key), at);
        at += 1;
    };
    for e in edits {
        match e {
            Edit::List { root, ops } => {
                for o in ops {
                    put(root, o.key().clone());
                }
            }
            Edit::Map { root, ops } => {
                for o in ops {
                    if let Some((list, k)) =
                        crate::session::pack::elem_of_map_key(composites, root, o.key())
                    {
                        put(list, k);
                    }
                }
            }
            _ => {}
        }
    }
    let mut out: Vec<((String, Key), usize)> = last.into_iter().collect();
    out.sort_by_key(|(_, at)| *at);
    out.into_iter().map(|(e, _)| e).collect()
}

impl Session {
    /// Select elements by key, or clear with `None` (or no keys). Needs read. The root must be a
    /// MovableList and every key a live element of it ([`SessionError::BadPosition`] otherwise),
    /// with at most [`ELEM_SELECTION_MAX`] keys ([`SessionError::SelectionTooLarge`]); a refusal
    /// leaves the selection as it was. A repeated key counts once. Returns the selection as
    /// stored. It follows moves, and a key leaves it when its element stops being live, for good:
    /// bringing the element back doesn't select it again. A watcher is refused
    /// ([`SessionError::Watching`]).
    pub fn set_selection(
        &self,
        conn: ConnId,
        selection: Option<ElemSelection>,
    ) -> Result<Option<ElemSelection>> {
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        let i = self.admit(st, conn, Need::Read, Action::Cursor, now)?;
        self.refuse_watcher(st, i)?;
        let selection = match selection {
            Some(s) if !s.keys.is_empty() => Some(self.checked_selection(st, s)?),
            _ => None,
        };
        let m = &mut st.members[i];
        if selection.is_some() {
            m.last_cursor_ms = Some(now);
        }
        let mut out = selection.clone();
        m.elem.selection = selection;
        if let Some(s) = out.as_mut() {
            address_selection(&*self.pack, s);
        }
        Ok(out)
    }

    /// Set this connection's pointer, or clear it with `None`. Needs read. Subscribers get the
    /// latest value at each roster flush, however many calls came between. A watcher is refused
    /// ([`SessionError::Watching`]).
    pub fn set_pointer(&self, conn: ConnId, pointer: Option<Point>) -> Result<()> {
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        let i = self.admit(st, conn, Need::Read, Action::Cursor, now)?;
        self.refuse_watcher(st, i)?;
        st.members[i].elem.pointer = pointer;
        Ok(())
    }

    /// Set the area this connection is looking at, or clear it with `None`. Needs read. Delivered
    /// as [`Session::set_pointer`]. For a text doc's lines, [`Session::set_view`] is the call. A
    /// watcher is refused ([`SessionError::Watching`]).
    pub fn set_viewport(&self, conn: ConnId, viewport: Option<Rect>) -> Result<()> {
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        let i = self.admit(st, conn, Need::Read, Action::View, now)?;
        self.refuse_watcher(st, i)?;
        st.members[i].elem.viewport = viewport;
        Ok(())
    }

    /// Set the view this connection is looking at, by address, or clear it with `None`. Needs
    /// read. Any address the kit's grammar reads ([`crate::addr::parse`], native IDs too; a
    /// bare key is one), at most [`crate::session::PRESENCE_ADDRESS_MAX`] bytes, else
    /// [`SessionError::BadPosition`] and nothing changes; the kit doesn't resolve it. Returns
    /// it as stored. Delivered as [`Session::set_pointer`]: the latest at each roster flush
    /// ([`RosterEvent::ViewAddress`](super::roster::RosterEvent::ViewAddress)). A host sets an
    /// agent's when it renders a still for it. A watcher is refused
    /// ([`SessionError::Watching`]).
    pub fn set_view_address(&self, conn: ConnId, address: Option<&str>) -> Result<Option<String>> {
        let address = address
            .map(|a| presence_address("the view address", a))
            .transpose()?;
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        let i = self.admit(st, conn, Need::Read, Action::View, now)?;
        self.refuse_watcher(st, i)?;
        st.members[i].elem.view_address = address.clone();
        Ok(address)
    }

    /// Set this connection's place in time (a playhead: [`TimePosition`]), or clear it with
    /// `None`. Needs read. The kit stamps it with the host clock (`at_ms`) and returns it as
    /// stored. Delivered as [`Session::set_pointer`]
    /// ([`RosterEvent::TimePosition`](super::roster::RosterEvent::TimePosition)). A watcher is
    /// refused ([`SessionError::Watching`]).
    pub fn set_time_position(
        &self,
        conn: ConnId,
        time: Option<TimePosition>,
    ) -> Result<Option<TimePosition>> {
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        let i = self.admit(st, conn, Need::Read, Action::View, now)?;
        self.refuse_watcher(st, i)?;
        let time = time.map(|t| t.stamped(now));
        st.members[i].elem.time = time.clone();
        Ok(time)
    }

    /// A pack's verb said where its agent is looking and where its playhead is
    /// ([`crate::tools::StructOutput::viewing`], [`crate::tools::StructOutput::time_position`]):
    /// set on the verb's connection, as the verb's focus and selection are, for an agent only
    /// (a person's presence is never moved, nor a watcher's). A view address the grammar
    /// refuses is the pack's bug: it is left as it was.
    pub(crate) fn verb_view_time(
        &self,
        conn: ConnId,
        view: Option<&str>,
        time: Option<&TimePosition>,
    ) {
        if view.is_none() && time.is_none() {
            return;
        }
        let now = self.now();
        let Ok(mut st) = self.lock() else {
            return;
        };
        let st = &mut *st;
        let Some(m) = st.members.iter_mut().find(|m| m.conn == conn) else {
            return;
        };
        if m.p.kind != ParticipantKind::Agent || m.watcher {
            return;
        }
        if let Some(v) = view.and_then(|v| presence_address("the view address", v).ok()) {
            m.elem.view_address = Some(v);
        }
        if let Some(t) = time {
            m.elem.time = Some(t.clone().stamped(now));
        }
    }

    /// [`Session::set_selection`] by address ([`Pack::address_of`]): every
    /// address names a live element of one list root ([`Session::resolve_address`]); none clears
    /// it. Returns the selection as stored, by address.
    pub fn set_selection_at(&self, conn: ConnId, addresses: &[String]) -> Result<Vec<String>> {
        let selection = match addresses {
            [] => None,
            _ if addresses.len() > ELEM_SELECTION_MAX => {
                return Err(SessionError::SelectionTooLarge {
                    len: addresses.len(),
                    max: ELEM_SELECTION_MAX,
                })
            }
            _ => {
                let mut root: Option<String> = None;
                let mut keys = Vec::with_capacity(addresses.len());
                for a in addresses {
                    let (r, k) = self.resolve_address(conn, a)?;
                    match &root {
                        Some(x) if *x != r => {
                            return Err(SessionError::BadPosition(format!(
                                "{a} is in {r:?}, not {x:?}: a selection is in one root"
                            )))
                        }
                        Some(_) => {}
                        None => root = Some(r),
                    }
                    keys.push(k);
                }
                let root = root.expect("one address at least");
                Some(ElemSelection::new(root, keys))
            }
        };
        let out = self.set_selection(conn, selection)?;
        Ok(out.map_or_else(Vec::new, |s| {
            s.keys
                .iter()
                .map(|k| self.pack.address_of(&s.root, k))
                .collect()
        }))
    }

    /// [`Session::set_focus`] by address: the address names a live element
    /// ([`Session::resolve_address`]); `None` clears it. Returns the focus as stored, by
    /// address.
    pub fn set_focus_at(&self, conn: ConnId, address: Option<&str>) -> Result<Option<String>> {
        let focus = match address {
            None => None,
            Some(a) => {
                let (root, key) = self.resolve_address(conn, a)?;
                Some(ElemFocus::new(root, key))
            }
        };
        let out = self.set_focus(conn, focus)?;
        Ok(out.map(|f| self.pack.address_of(&f.root, &f.key)))
    }

    /// [`Session::set_range`] by address: `(address, field, anchor, head)`, the
    /// element by address ([`Session::resolve_address`]); `None` clears it. Returns the stored
    /// range's element by address.
    pub fn set_range_at(
        &self,
        conn: ConnId,
        range: Option<(&str, &str, usize, usize)>,
    ) -> Result<Option<String>> {
        let range = match range {
            None => None,
            Some((a, field, anchor, head)) => {
                let (root, key) = self.resolve_address(conn, a)?;
                Some(ElemRange::new(root, key, field, anchor, head))
            }
        };
        let out = self.set_range(conn, range)?;
        Ok(out.map(|r| self.pack.address_of(&r.root, &r.key)))
    }

    /// Set the container this connection has in view, or
    /// clear it with `None`. Needs read. The root must be a MovableList and the key a live
    /// element of it ([`SessionError::BadPosition`] otherwise; the focus stays as it was). A
    /// watcher is refused ([`SessionError::Watching`]). It leaves when its element stops being
    /// live. An agent's focus is also set by its claims ([`Session::claim_elems`]); a range sets
    /// it too ([`Session::set_range`]).
    pub fn set_focus(&self, conn: ConnId, focus: Option<ElemFocus>) -> Result<Option<ElemFocus>> {
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        let i = self.admit(st, conn, Need::Read, Action::View, now)?;
        self.refuse_watcher(st, i)?;
        if let Some(f) = &focus {
            self.check_live_elem(st, &f.root, &f.key)?;
        }
        let mut out = focus.clone();
        st.members[i].elem.set_focus(focus);
        if let Some(f) = out.as_mut() {
            address_focus(&*self.pack, f);
        }
        Ok(out)
    }

    /// Set this connection's caret or selection inside one element's text field (rendering-data
    /// contract Item 3), or clear it with `None`. Needs edit, as a text caret does
    /// ([`Session::set_cursor`]); a watcher is refused ([`SessionError::Watching`]). Refused
    /// with [`SessionError::BadPosition`], the range as it was, unless: the root is a
    /// MovableList with a [composite](Pack::composite) map, the key is a live element of it, the
    /// field (`<key>\x1f<field>` in that map) holds a string, and both offsets are character
    /// boundaries of it (UTF-8 bytes).
    ///
    /// **It sets the focus** to the element's container ([`Pack::container_of`]), so a range is
    /// never shown on a container out of view. It is never moved: when the field changes (by
    /// anyone) or the element stops being live, it is dropped (`None`), and the owner sends a
    /// new one.
    pub fn set_range(&self, conn: ConnId, range: Option<ElemRange>) -> Result<Option<ElemRange>> {
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        let i = self.admit(st, conn, Need::Edit, Action::Cursor, now)?;
        self.refuse_watcher(st, i)?;
        let Some(r) = range else {
            st.members[i].elem.range = None;
            return Ok(None);
        };
        self.check_live_elem(st, &r.root, &r.key)?;
        let bad = |m: String| Err(SessionError::BadPosition(m));
        let Some(map) = fields_root(self.pack.composite(), &r.root) else {
            return bad(format!("{:?} has no fields to hold a text range", r.root));
        };
        let text = match st.doc.map_get(map, &field_key(&r.key, &r.field))? {
            Some(crate::elem::Value::Str(s)) => s,
            _ => {
                return bad(format!(
                    "{} {:?} is not a text field of element {}",
                    r.root, r.field, r.key
                ))
            }
        };
        for (what, at) in [("anchor", r.anchor), ("head", r.head)] {
            if !text.is_char_boundary(at) {
                return bad(format!(
                    "the range's {what} {at} is not a character boundary of {:?} ({} bytes)",
                    r.field,
                    text.len()
                ));
            }
        }
        let focus = self.container_focus(st, &r.root, &r.key);
        let m = &mut st.members[i];
        m.last_cursor_ms = Some(now);
        if !planted_range_keeps_focus() {
            if let Some(f) = focus {
                m.elem.set_focus(Some(f));
            }
        }
        let mut out = r.clone();
        m.elem.range = Some(r);
        address_range(&*self.pack, &mut out);
        Ok(Some(out))
    }

    /// After a pack verb's change by member `i` landed (`named`: what
    /// the change named, in order, [`touched_in_order`]; `stream`: what it applied): an
    /// **agent's** presence follows what the verb touched.
    ///
    /// - Its **focus** goes to the container ([`Pack::container_of`]) of the last element the
    ///   change touched whose container is live: the last container touched wins.
    /// - Its **selection** is replaced by the elements the change touched in the list root of the
    ///   last one, in order, live ones only (a deleted element can't be selected; none live:
    ///   no selection), at most [`ELEM_SELECTION_MAX`] (the last ones).
    /// - What touched means: an op that landed (`landed_elems`); an op with no
    ///   effect touches nothing, and a change that touched no element moves nothing.
    ///
    /// A person's presence is never moved (people set their own), nor a watcher's (it has
    /// none), nor any on a table (a table's presence is its own: `grid_presence`). Set on the
    /// verb's connection, so the roster carries it at its next flush. Later calls override it.
    pub(super) fn verb_presence(
        &self,
        st: &mut State,
        i: usize,
        named: &[(String, Key)],
        stream: &[ElemApplied],
    ) {
        let m = &st.members[i];
        let agent = m.p.kind == ParticipantKind::Agent || planted_verb_moves_person();
        if !agent || m.watcher || self.is_grid() || named.is_empty() {
            return;
        }
        let landed = super::elem_claims::landed_elems(self.pack.composite(), stream);
        let touched: Vec<&(String, Key)> = named
            .iter()
            .filter(|(r, k)| landed.get(r.as_str()).is_some_and(|s| s.contains(k)))
            .collect();
        let Some((root, _)) = touched.last() else {
            return;
        };
        let focus = touched
            .iter()
            .rev()
            .find_map(|(r, k)| self.container_focus(st, r, k));
        let mut keys: Vec<Key> = touched
            .iter()
            .filter(|(r, k)| r == root && self.elem_live(st, r, k))
            .map(|(_, k)| k.clone())
            .collect();
        if keys.len() > ELEM_SELECTION_MAX {
            keys.drain(..keys.len() - ELEM_SELECTION_MAX);
        }
        let selection = (!keys.is_empty()).then(|| ElemSelection::new(root.clone(), keys));
        st.members[i].elem.verb_touched(focus, selection);
    }

    /// `key` is a live element of the MovableList root `root`, or [`SessionError::BadPosition`].
    fn check_live_elem(&self, st: &State, root: &str, key: &Key) -> Result<()> {
        if self.schema.kind_of(root) != Some(Kind::MovableList) {
            return Err(SessionError::BadPosition(format!(
                "{root:?} is not a list of elements in this doc"
            )));
        }
        if !self.elem_live(st, root, key) {
            return Err(SessionError::BadPosition(format!(
                "element {key} of {root:?} is not live (unknown or deleted)"
            )));
        }
        Ok(())
    }

    /// The focus on the container of element `key` of `root`, when that is a live element of a
    /// list root. With a view schema ([`Pack::view_schema`]) a unit's container
    /// is the one its container field names ([`Session::schema_container`]) and a container is
    /// its own; any other root's (or a pack's without a schema) is [`Pack::container_of`]'s.
    pub(super) fn container_focus(&self, st: &State, root: &str, key: &Key) -> Option<ElemFocus> {
        let (croot, ckey) = match self.pack.view_schema() {
            Some(vs) if root == vs.units => self.schema_container(st, &vs, key)?,
            Some(vs) if root == vs.containers => (root.to_owned(), key.clone()),
            _ => self.pack.container_of(root, key)?,
        };
        self.check_live_elem(st, &croot, &ckey).ok()?;
        Some(ElemFocus::new(croot, ckey))
    }

    /// The container unit `key` is in, through the view schema: the value of its
    /// `unit_container_field` is the container's key, or, with a `container_id_field`, the
    /// value of that field of the container. The pack's [`Pack::container_of`] is tried first
    /// as a hint (checked: its container must hold that id), so a pack that names containers by
    /// key costs one field read; otherwise the live containers are read in order and the first
    /// that holds the id is it.
    pub(super) fn schema_container(
        &self,
        st: &State,
        vs: &crate::view_index::ViewSchema,
        key: &Key,
    ) -> Option<(String, Key)> {
        let composites = self.pack.composite();
        let read = |map: &str, k: &Key, field: &str| match st.doc.map_get(map, &field_key(k, field))
        {
            Ok(Some(crate::elem::Value::Str(s))) => Some(s),
            _ => None,
        };
        let id = read(
            fields_root(composites, vs.units)?,
            key,
            vs.unit_container_field,
        )?;
        let ckey = match vs.container_id_field {
            None => Key::new(id).ok()?,
            Some(f) => {
                let cmap = fields_root(composites, vs.containers)?;
                let holds = |k: &Key| read(cmap, k, f).is_some_and(|v| v == id);
                match self.pack.container_of(vs.units, key) {
                    Some((r, k)) if r == vs.containers && holds(&k) => k,
                    _ => st
                        .doc
                        .list(vs.containers)
                        .ok()?
                        .into_iter()
                        .map(|(k, _)| k)
                        .find(|k| holds(k))?,
                }
            }
        };
        Some((vs.containers.to_owned(), ckey))
    }

    fn refuse_watcher(&self, st: &State, i: usize) -> Result<()> {
        if st.members[i].watcher {
            return Err(SessionError::Watching {
                max: self.config.participants_max,
            });
        }
        Ok(())
    }

    fn checked_selection(&self, st: &State, s: ElemSelection) -> Result<ElemSelection> {
        if s.keys.len() > ELEM_SELECTION_MAX {
            return Err(SessionError::SelectionTooLarge {
                len: s.keys.len(),
                max: ELEM_SELECTION_MAX,
            });
        }
        let is_list = self
            .schema
            .roots
            .iter()
            .any(|(r, k)| *r == s.root && *k == Kind::MovableList);
        if !is_list {
            return Err(SessionError::BadPosition(format!(
                "{:?} is not a list of elements in this doc",
                s.root
            )));
        }
        let mut keys: Vec<Key> = Vec::with_capacity(s.keys.len());
        for k in s.keys {
            if !matches!(st.doc.elem(&s.root, &k), Ok(ElemState::Live(_))) {
                return Err(SessionError::BadPosition(format!(
                    "element {k} of {:?} is not live (unknown or deleted)",
                    s.root
                )));
            }
            if !keys.contains(&k) {
                keys.push(k);
            }
        }
        Ok(ElemSelection::new(s.root, keys))
    }

    /// After anything that changed the doc (`changed`: what it applied; `None`: anything may
    /// have, a reload): every selection drops the keys of elements that aren't live now, and a
    /// selection left with none is gone; a focus on an element that isn't live is gone; a range
    /// is gone when its element isn't live, or `changed` wrote its field (or is `None`).
    pub(super) fn elem_presence_after(&self, st: &mut State, changed: Option<&[ElemApplied]>) {
        let written = changed.map(written_fields);
        let composites = self.pack.composite();
        // liveness from the live view (no engine order rebuilt for it), read before the members
        // are changed
        let mut members = std::mem::take(&mut st.members);
        let live = |root: &str, k: &Key| self.elem_live(st, root, k);
        for m in members.iter_mut() {
            if !planted_selection_keeps_deleted() {
                if let Some(s) = m.elem.selection.as_mut() {
                    let ElemSelection { root, keys, .. } = s;
                    keys.retain(|k| live(root, k));
                    if keys.is_empty() {
                        m.elem.selection = None;
                    }
                }
            }
            if let Some(f) = &m.elem.focus {
                if !live(&f.root, &f.key) {
                    m.elem.focus = None;
                }
            }
            if let Some(c) = &m.elem.cells {
                // its container went: so does it (its corners are checked below)
                let gone = match (self.pack.view_schema(), &c.container) {
                    (Some(vs), Some(k)) => !live(vs.containers, k),
                    _ => true,
                };
                if gone {
                    m.elem.cells = None;
                }
            }
            if let Some(r) = &m.elem.range {
                let gone = !live(&r.root, &r.key)
                    || match &written {
                        None => true,
                        Some(w) => fields_root(composites, &r.root).is_some_and(|map| {
                            w.contains(&(map.to_owned(), field_key(&r.key, &r.field)))
                        }),
                    };
                if gone {
                    m.elem.range = None;
                }
            }
        }
        st.members = members;
        self.cells_after(st, changed);
    }
}

/// The map fields a change wrote: every map op with an effect, as `(map root, key)`.
fn written_fields(changed: &[ElemApplied]) -> std::collections::HashSet<(String, String)> {
    let mut out = std::collections::HashSet::new();
    for a in changed {
        if let ElemApplied::Map { root, ops } = a {
            for o in ops {
                if !matches!(o.effect, Effect::None { .. }) {
                    out.insert((root.clone(), o.key.clone()));
                }
            }
        }
    }
    out
}

/// Planted ([`crate::differential::ContractMutant::VerbMovesPersonPresence`]): a person's verb
/// moves their presence as an agent's does. Without the `differential` feature: `false`.
fn planted_verb_moves_person() -> bool {
    plant_on!(
        crate::differential::contract_mutant()
            == Some(crate::differential::ContractMutant::VerbMovesPersonPresence)
    )
}

/// Planted ([`crate::differential::ContractMutant::RangeDoesNotSetFocus`]): a range leaves the
/// focus where it was. Without the `differential` feature there is no hook: `false`.
fn planted_range_keeps_focus() -> bool {
    plant_on!(
        crate::differential::contract_mutant()
            == Some(crate::differential::ContractMutant::RangeDoesNotSetFocus)
    )
}

/// Planted ([`crate::differential::SelectionMutant::KeepsDeleted`]): selections keep every key.
/// Without the `differential` feature there is no hook: `false`.
fn planted_selection_keeps_deleted() -> bool {
    plant_on!(
        crate::differential::selection_mutant()
            == Some(crate::differential::SelectionMutant::KeepsDeleted)
    )
}
